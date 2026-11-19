//! Recent log output that Give feedback can attach when the user opts in.
//!
//! The logger tees every record into a bounded in-memory buffer, and a
//! redacted copy into `<DRAGENT_HOME>/logs/desktop.log` (two files of at most
//! 5 MB), so an app opened from the Finder, whose stderr goes nowhere, still
//! leaves a log to read. What leaves the machine goes through [`redact`]
//! first: tokens, keys, emails, and the home directory never reach the report.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

use regex::Regex;

/// Upper bound on buffered log text.
const MAX_BUFFERED_BYTES: usize = 512 * 1024;

struct Buffer {
    lines: VecDeque<String>,
    bytes: usize,
}

static RECENT: Mutex<Buffer> = Mutex::new(Buffer {
    lines: VecDeque::new(),
    bytes: 0,
});

/// Largest the log file grows before it's moved to `desktop.log.1`.
const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Append one (redacted) line to the log file, rotating it when full.
fn persist(line: &str) {
    if cfg!(test) {
        return;
    }
    static FILE: Mutex<Option<(std::fs::File, u64)>> = Mutex::new(None);
    let Ok(mut slot) = FILE.lock() else { return };
    let dir = dragent_paths::home().join("logs");
    let path = dir.join("desktop.log");
    if slot.is_none() {
        let _ = std::fs::create_dir_all(&dir);
        let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        *slot = Some((file, len));
    }
    let Some((file, len)) = slot.as_mut() else { return };
    let line = redact(line);
    if file.write_all(line.as_bytes()).is_ok() {
        *len += line.len() as u64;
    }
    if *len > MAX_LOG_FILE_BYTES {
        let _ = std::fs::rename(&path, dir.join("desktop.log.1"));
        *slot = None;
    }
}

fn record(line: String) {
    persist(&line);
    let Ok(mut buffer) = RECENT.lock() else {
        return;
    };
    buffer.bytes += line.len();
    buffer.lines.push_back(line);
    while buffer.bytes > MAX_BUFFERED_BYTES {
        match buffer.lines.pop_front() {
            Some(old) => buffer.bytes -= old.len(),
            None => break,
        }
    }
}

/// Log target that writes to stderr (as before) and keeps a copy of each
/// complete line.
#[derive(Default)]
pub struct TeeStderr {
    partial: Vec<u8>,
}

impl Write for TeeStderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.partial.extend_from_slice(buf);
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            record(String::from_utf8_lossy(&line).into_owned());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

/// The buffered log, oldest first, redacted.
pub fn recent_log() -> String {
    let text = RECENT
        .lock()
        .map(|buffer| buffer.lines.iter().map(String::as_str).collect::<String>())
        .unwrap_or_default();
    redact(&text)
}

struct Rule {
    pattern: Regex,
    replacement: &'static str,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |pattern: &str, replacement| Rule {
            pattern: Regex::new(pattern).expect("valid redaction pattern"),
            replacement,
        };
        vec![
            rule(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]+", "${1}[redacted]"),
            rule(
                r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+",
                "[redacted-jwt]",
            ),
            rule(
                r"\b(?:sk-(?:ant-)?[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35})",
                "[redacted-key]",
            ),
            rule(
                r#"(?i)\b((?:access|refresh|id|api|secret|auth|session)[_-]?(?:token|key)|password|passwd|secret|token)(["']?\s*[:=]\s*["']?)[^\s"'&,;]+"#,
                "${1}${2}[redacted]",
            ),
            rule(
                r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
                "[redacted-email]",
            ),
        ]
    })
}

/// Remove secrets and personal details from log text.
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if home.len() > 1 {
            out = out.replace(home.as_ref(), "~");
        }
    }
    for rule in rules() {
        out = rule.pattern.replace_all(&out, rule.replacement).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_personal_details_are_redacted() {
        let log = "GET /v1 Authorization: Bearer abc.def-123\n\
                   token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTYifQ.sig\n\
                   key sk-ant-api03-ABCDEFGHIJKLMNOPQRST and ghp_ABCDEFGHIJKLMNOPQRSTUVWX\n\
                   {\"refresh_token\": \"r3fr3sh\"} password=hunter2&x=1\n\
                   signed in as someone@example.com\n\
                   ordinary line stays";
        let out = redact(log);
        for secret in [
            "abc.def-123",
            "eyJhbGci",
            "sk-ant-api03",
            "ghp_ABC",
            "r3fr3sh",
            "hunter2",
            "someone@example.com",
        ] {
            assert!(!out.contains(secret), "{secret} leaked:\n{out}");
        }
        assert!(out.contains("Bearer [redacted]"));
        assert!(out.contains("&x=1"));
        assert!(out.contains("ordinary line stays"));
    }

    #[test]
    fn home_directory_is_shortened() {
        if let Some(home) = dirs::home_dir() {
            let line = format!("opened {}/project/file.rs", home.display());
            assert_eq!(redact(&line), "opened ~/project/file.rs");
        }
    }

    #[test]
    fn tee_keeps_complete_lines() {
        let mut tee = TeeStderr::default();
        tee.write_all(b"diagnostics-test first\ndiagnostics-test sec").unwrap();
        tee.write_all(b"ond\n").unwrap();
        let log = recent_log();
        assert!(log.contains("diagnostics-test first\n"));
        assert!(log.contains("diagnostics-test second\n"));
    }
}
