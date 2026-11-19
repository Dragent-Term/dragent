//! The user's shell history, shared by every session, for autosuggestions.
//!
//! Each terminal used to suggest only from commands typed in that tab since
//! launch, so a new or restored tab had nothing to suggest from. This reads
//! the shell's own history file once, off the UI thread, and every session
//! adds the commands it runs, so a command typed in one tab is suggested in
//! the next.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

/// Oldest first, without duplicates (a repeated command keeps its latest place).
static HISTORY: RwLock<Vec<String>> = RwLock::new(Vec::new());
static LOADING: AtomicBool = AtomicBool::new(false);

/// Only the end of a large history file is read: suggestions favour recent
/// commands, and a 50 MB `.zsh_history` must not cost a launch.
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

/// Read the shell's history file on a worker thread (once per process).
pub fn load_in_background() {
    if LOADING.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::bg::detach(|| {
        let from_file = dedupe(read_history_file());
        let mut history = HISTORY.write().unwrap_or_else(|p| p.into_inner());
        // Commands run before the file finished reading are newer than it.
        let session = std::mem::replace(&mut *history, from_file);
        for command in session {
            push(&mut history, command);
        }
    });
}

/// Remember a command a session ran.
pub fn record(command: &str) {
    let command = command.trim();
    if command.is_empty() {
        return;
    }
    let mut history = HISTORY.write().unwrap_or_else(|p| p.into_inner());
    push(&mut history, command.to_string());
}

/// Visit commands newest first until `f` returns `Some`.
pub fn find_newest<T>(mut f: impl FnMut(&str) -> Option<T>) -> Option<T> {
    let history = HISTORY.read().unwrap_or_else(|p| p.into_inner());
    history.iter().rev().find_map(|command| f(command))
}

fn push(history: &mut Vec<String>, command: String) {
    if history.last() == Some(&command) {
        return;
    }
    if let Some(at) = history.iter().rposition(|c| *c == command) {
        history.remove(at);
    }
    history.push(command);
    if history.len() > MAX_ENTRIES {
        let excess = history.len() - MAX_ENTRIES;
        history.drain(..excess);
    }
}

fn dedupe(commands: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut newest_first: Vec<String> = commands
        .into_iter()
        .rev()
        .filter(|c| seen.insert(c.clone()))
        .take(MAX_ENTRIES)
        .collect();
    newest_first.reverse();
    newest_first
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    Zsh,
    Bash,
    Fish,
    PowerShell,
}

fn read_history_file() -> Vec<String> {
    for (path, format) in candidate_files() {
        if let Some(bytes) = read_tail(&path) {
            let commands = parse(&bytes, format);
            if !commands.is_empty() {
                return commands;
            }
        }
    }
    Vec::new()
}

/// The login shell's history file first, then the other common ones.
fn candidate_files() -> Vec<(PathBuf, Format)> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let shell = std::env::var("SHELL").unwrap_or_default();
    let histfile = std::env::var_os("HISTFILE").map(PathBuf::from);
    let fish_dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("fish");

    let zsh = (histfile.clone().filter(|_| shell.ends_with("zsh")).unwrap_or_else(|| home.join(".zsh_history")), Format::Zsh);
    let bash = (histfile.filter(|_| shell.ends_with("bash")).unwrap_or_else(|| home.join(".bash_history")), Format::Bash);
    let fish = (fish_dir.join("fish_history"), Format::Fish);

    let mut files = Vec::new();
    if cfg!(windows) {
        if let Some(appdata) = dirs::data_dir() {
            files.push((
                appdata.join("Microsoft/Windows/PowerShell/PSReadLine/ConsoleHost_history.txt"),
                Format::PowerShell,
            ));
        }
    }
    if shell.ends_with("bash") {
        files.extend([bash, zsh, fish]);
    } else if shell.ends_with("fish") {
        files.extend([fish, zsh, bash]);
    } else {
        files.extend([zsh, bash, fish]);
    }
    files
}

fn read_tail(path: &PathBuf) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(MAX_READ_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    if start > 0 {
        // Drop the partial first line.
        let first_newline = bytes.iter().position(|b| *b == b'\n')?;
        bytes.drain(..=first_newline);
    }
    Some(bytes)
}

fn parse(bytes: &[u8], format: Format) -> Vec<String> {
    let text = match format {
        Format::Zsh => String::from_utf8_lossy(&unmetafy(bytes)).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    let mut commands = Vec::new();
    match format {
        Format::Zsh | Format::Bash | Format::PowerShell => {
            let continuation = if format == Format::PowerShell { '`' } else { '\\' };
            let mut pending = String::new();
            for line in text.lines() {
                let line = if format == Format::Zsh && pending.is_empty() {
                    // Extended history: `: 1700000000:0;command`.
                    match line.strip_prefix(": ").and_then(|rest| rest.split_once(';')) {
                        Some((_, command)) => command,
                        None => line,
                    }
                } else {
                    line
                };
                if format == Format::Bash
                    && pending.is_empty()
                    && line.starts_with('#')
                    && line[1..].chars().all(|c| c.is_ascii_digit())
                {
                    continue; // HISTTIMEFORMAT timestamp line
                }
                if let Some(head) = line.strip_suffix(continuation) {
                    pending.push_str(head);
                    pending.push('\n');
                    continue;
                }
                pending.push_str(line);
                commands.push(std::mem::take(&mut pending));
            }
        }
        Format::Fish => {
            for line in text.lines() {
                if let Some(command) = line.strip_prefix("- cmd: ") {
                    commands.push(command.replace("\\n", "\n").replace("\\\\", "\\"));
                }
            }
        }
    }
    commands
        .into_iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

/// zsh writes the bytes 0x83..=0xa2 (and NUL) as `0x83, byte ^ 0x20`
/// ("metafied"), which splits multi-byte UTF-8 characters.
fn unmetafy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter();
    while let Some(&b) = iter.next() {
        if b == 0x83 {
            if let Some(&next) = iter.next() {
                out.push(next ^ 0x20);
            }
        } else {
            out.push(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zsh_extended_and_multiline() {
        let text = b": 1700000000:0;git status\n: 1700000001:0;echo one \\\ntwo\nls -la\n";
        assert_eq!(
            parse(text, Format::Zsh),
            vec!["git status", "echo one \ntwo", "ls -la"]
        );
    }

    #[test]
    fn unmetafies_zsh_utf8() {
        // "—" is E2 80 94; zsh metafies the 0x94.
        let metafied = [b'a', b' ', 0xE2, 0x80, 0x83, 0x94 ^ 0x20, b' ', b'b'];
        assert_eq!(parse(&metafied, Format::Zsh), vec!["a — b"]);
    }

    #[test]
    fn parses_bash_with_timestamps() {
        let text = b"#1700000000\ncargo build\n#1700000001\ncargo test\n";
        assert_eq!(parse(text, Format::Bash), vec!["cargo build", "cargo test"]);
    }

    #[test]
    fn parses_fish() {
        let text = b"- cmd: git push\n  when: 1700000000\n- cmd: npm run dev\n  when: 1\n";
        assert_eq!(parse(text, Format::Fish), vec!["git push", "npm run dev"]);
    }

    #[test]
    fn dedupe_keeps_latest_position() {
        let commands = vec!["a".into(), "b".into(), "a".into(), "c".into()];
        assert_eq!(dedupe(commands), vec!["b", "a", "c"]);
    }

    #[test]
    fn push_moves_repeat_to_end() {
        let mut history = vec!["a".to_string(), "b".to_string()];
        push(&mut history, "a".to_string());
        assert_eq!(history, vec!["b", "a"]);
    }
}
