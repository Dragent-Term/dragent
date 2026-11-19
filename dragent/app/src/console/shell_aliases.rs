//! Shell alias discovery and in-buffer expansion.

use crate::process_ext::NoWindow;
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// The aliases from the most recent read of the user's shell, shared by every
/// session. Reading runs an interactive shell (`$SHELL -i -c alias`, which
/// sources the whole rc file: ~300 ms with a typical `.zshrc`), so it never
/// happens on the UI thread. Restoring 20 tabs used to run it 20 times in a
/// row there, freezing launch for seconds.
static LATEST: RwLock<Option<Arc<HashMap<String, String>>>> = RwLock::new(None);
static READING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default)]
pub struct ShellAliases {
    /// Fixed aliases; `None` follows the process-wide [`LATEST`] read.
    map: Option<Arc<HashMap<String, String>>>,
}

impl ShellAliases {
    /// A handle to the shared aliases. Never blocks: it starts a background
    /// re-read (so a profile edited since the last session still reaches new
    /// ones) and expands with the previous read until that finishes.
    pub fn load() -> Self {
        refresh_in_background();
        Self { map: None }
    }

    fn current(&self) -> Option<Arc<HashMap<String, String>>> {
        match &self.map {
            Some(map) => Some(map.clone()),
            None => LATEST
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.current().is_none_or(|map| map.is_empty())
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.current()?.get(name).cloned()
    }

    /// If the buffer is `name` or `name …`, replace `name` with its expansion.
    pub fn try_expand(&self, buffer: &str) -> Option<String> {
        let trimmed = buffer.trim_start();
        if trimmed.is_empty() {
            return None;
        }
        let first_end = trimmed
            .find(|c: char| c.is_whitespace())
            .unwrap_or(trimmed.len());
        let name = &trimmed[..first_end];
        let map = self.current()?;
        let expansion = map.get(name)?;
        let rest = trimmed.get(first_end..).unwrap_or("");
        let lead = &buffer[..buffer.len().saturating_sub(trimmed.len())];
        Some(format!("{lead}{expansion}{rest}"))
    }
}

/// Re-read the aliases on a worker thread; requests while one is running
/// share its result.
fn refresh_in_background() {
    if READING.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::bg::detach(|| {
        let map = Arc::new(parse_alias_output(&read_shell_aliases()));
        *LATEST.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(map);
        READING.store(false, Ordering::Release);
    });
}

fn read_shell_aliases() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    Command::new(&shell)
        .no_window()
        .args(["-i", "-c", "alias"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Parse `alias ll='ls -la'` lines from shell output.
pub fn parse_alias_output(output: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let line = line.trim();
        if !line.starts_with("alias ") {
            continue;
        }
        let body = line.trim_start_matches("alias ").trim();
        let Some((name, value)) = body.split_once('=') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = strip_quotes(value.trim());
        if !name.is_empty() && !value.is_empty() {
            map.insert(name, value);
        }
    }
    map
}

fn strip_quotes(s: &str) -> String {
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        s[1..s.len().saturating_sub(1)].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alias_lines() {
        let out = "alias ll='ls -la'\nalias gs='git status'\n";
        let m = parse_alias_output(out);
        assert_eq!(m.get("ll").map(String::as_str), Some("ls -la"));
        assert_eq!(m.get("gs").map(String::as_str), Some("git status"));
    }

    #[test]
    fn expands_first_token() {
        let mut map = HashMap::new();
        map.insert("ll".into(), "ls -la".into());
        let aliases = ShellAliases {
            map: Some(Arc::new(map)),
        };
        assert_eq!(aliases.try_expand("ll"), Some("ls -la".into()));
        assert_eq!(aliases.try_expand("ll src"), Some("ls -la src".into()));
    }
}
