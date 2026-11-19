use crate::theme::Theme;
use dragent_pty::PtyFeedStats;
use dragent_terminal::{
    Emulator, HookEvent, MouseEvent, MouseReportingMode, SemanticMarker, TermEvent, TermMode,
};
use std::path::PathBuf;

use super::alt_screen_state::AltScreenState;
use super::block::Block;
use super::block_context::block_context_from_lines;
use super::block_list::BlockList;
use super::events::TerminalEvent;
use super::input_mode::InputMode;
use super::selection::{selection_text, CellSelection};
use super::size::{SizeInfo, SizeReason, SizeUpdate};
use std::sync::Arc;

/// Resolve the target directory for a simple `cd` command (first segment only).
pub(crate) fn resolve_cd_target(current: &std::path::Path, command: &str) -> Option<PathBuf> {
    use std::path::Component;

    let head = command
        .split(&['|', ';', '&'][..])
        .next()
        .unwrap_or(command)
        .trim();
    let arg = cd_argument(head)?;

    let target = match arg.as_str() {
        "-" => return None,
        "~" => dirs::home_dir()?,
        raw if raw.starts_with("~/") => dirs::home_dir()?.join(raw.trim_start_matches("~/")),
        raw if raw.starts_with('/') => PathBuf::from(raw),
        raw => current.join(raw),
    };

    let mut normalized = PathBuf::new();
    for component in target.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    Some(std::fs::canonicalize(&normalized).unwrap_or(normalized))
}

fn cd_argument(command_head: &str) -> Option<String> {
    let trimmed = command_head.trim();
    if trimmed == "cd" {
        return Some("~".to_string());
    }
    let rest = trimmed.strip_prefix("cd ")?.trim();
    if rest.is_empty() {
        return Some("~".to_string());
    }
    if let Some(inner) = rest
        .strip_prefix('"')
        .and_then(|s| s.split_once('"').map(|(a, _)| a))
    {
        return Some(inner.to_string());
    }
    if let Some(inner) = rest
        .strip_prefix('\'')
        .and_then(|s| s.split_once('\'').map(|(a, _)| a))
    {
        return Some(inner.to_string());
    }
    Some(rest.split_whitespace().next()?.to_string())
}

pub(crate) fn cwd_changed(current: &PathBuf, new: &PathBuf) -> bool {
    let current = std::fs::canonicalize(current).unwrap_or_else(|_| current.clone());
    let new = std::fs::canonicalize(new).unwrap_or_else(|_| new.clone());
    current != new
}

/// Git Bash / MSYS2 shells report `$PWD` in POSIX drive form (`/c/Users/x`).
/// Interpreted as a Windows path that becomes the nonexistent `C:\c\Users\x`,
/// silently breaking everything keyed on the session cwd: the Tab path picker
/// (directory listing fails → picker never opens), dir-listing blocks, git
/// status polling, and the cwd-basename tab title. Convert the drive form back
/// to native. POSIX paths inside the MSYS root (`/usr/bin`) have no Windows
/// equivalent worth guessing and pass through unchanged.
pub(crate) fn normalize_shell_cwd(cwd: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = cwd.to_string_lossy();
        let b = s.as_bytes();
        let is_msys_drive = b.len() >= 2
            && b[0] == b'/'
            && b[1].is_ascii_alphabetic()
            && (b.len() == 2 || b[2] == b'/');
        if is_msys_drive {
            let drive = (b[1] as char).to_ascii_uppercase();
            let rest = s[2..].replace('/', "\\");
            let rest = if rest.is_empty() {
                "\\".to_string()
            } else {
                rest
            };
            return PathBuf::from(format!("{drive}:{rest}"));
        }
    }
    cwd
}

/// True for OSC window titles the shell/console sets BY DEFAULT — noise like
/// the shell's own executable path — as opposed to a title an application set
/// deliberately (e.g. Claude Code's status line). Default titles are dropped
/// so the tab header falls back to the session's cwd basename, which our
/// precmd hook keeps current across `cd`. Known defaults:
/// - conhost/ConPTY initial title: the spawned exe path, with an
///   "Administrator: " prefix when elevated ("Administrator: C:\WINDOWS\
///   System32\WindowsPowerShell\v1.0\powershell.exe").
/// - Git Bash / MSYS2: "<MSYSTEM>:<posix path>" on every cd
///   ("CLANGARM64:/c/Users/x/dev", "MINGW64:~") and the bash.exe path at start.
/// - PowerShell hosts that title themselves "Windows PowerShell"/"PowerShell 7".
/// - cmd/conhost variants that set the bare cwd path as the title.
pub(crate) fn is_default_shell_title(title: &str) -> bool {
    let t = title.trim();

    // "Administrator: <anything>" — conhost's elevation prefix on its default.
    let t = t.strip_prefix("Administrator:").map(str::trim).unwrap_or(t);

    // The spawned executable's path (or any *.exe) — conhost's initial title.
    if t.to_ascii_lowercase().ends_with(".exe") {
        return true;
    }

    // Shells titling themselves by product name.
    if t == "Windows PowerShell" || t == "PowerShell" || t.starts_with("PowerShell 7") {
        return true;
    }

    // MSYS2/Git Bash: "<MSYSTEM>:<path>" where MSYSTEM is an all-caps token
    // (MINGW64, MSYS, CLANGARM64, UCRT64, …) and the path is POSIX or ~.
    if let Some((prefix, rest)) = t.split_once(':') {
        let msys_prefix = prefix.len() >= 4
            && prefix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if msys_prefix && (rest.starts_with('/') || rest.starts_with('~')) {
            return true;
        }
    }

    // A bare filesystem path (Windows drive or POSIX absolute, no other text).
    let looks_like_windows_path = t.len() >= 3
        && t.as_bytes()[1] == b':'
        && (t.as_bytes()[2] == b'\\' || t.as_bytes()[2] == b'/');
    if (looks_like_windows_path || t.starts_with('/') || t.starts_with("~/")) && !t.contains(' ') {
        return true;
    }

    // Linux default: Ubuntu's stock bashrc (and many zsh configs) title the
    // window "user@host: dir" on every prompt.
    if let Some((head, rest)) = t.split_once(':') {
        let user_at_host = head.contains('@') && !head.contains(' ');
        let rest = rest.trim_start();
        if user_at_host && (rest.starts_with('/') || rest.starts_with('~') || rest.is_empty()) {
            return true;
        }
    }

    false
}
