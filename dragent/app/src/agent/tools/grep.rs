//! Tool: search for a pattern in files using ripgrep (`rg`) with a fallback to `grep -rn`.

use crate::process_ext::NoWindow;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of matches returned by a single grep invocation.
pub const MAX_GREP_MATCHES: usize = 50;

// ── Types ─────────────────────────────────────────────────────────────────────

pub use dragent_agent::files::{GrepMatch, GrepResult};
// ── Executor ──────────────────────────────────────────────────────────────────

pub struct GrepExecutor;

impl GrepExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Search for `pattern` in `path` (or current directory if `None`).
    ///
    /// Tries `rg --json` first; falls back to `grep -rn` if `rg` is not found.
    pub fn execute(&self, pattern: &str, path: Option<&Path>) -> GrepResult {
        let search_path = path.unwrap_or_else(|| Path::new("."));

        // Try ripgrep first.
        if let Some(result) = try_ripgrep(pattern, search_path) {
            return result;
        }

        // Fall back to grep.
        try_grep(pattern, search_path)
    }
}

impl Default for GrepExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ── ripgrep backend ───────────────────────────────────────────────────────────

fn try_ripgrep(pattern: &str, path: &Path) -> Option<GrepResult> {
    // Probe whether rg exists.
    let probe = Command::new("rg").no_window().arg("--version").output();
    if probe.is_err() {
        return None;
    }

    let output = Command::new("rg")
        .no_window()
        .args([
            "--json",
            "--max-count",
            &(MAX_GREP_MATCHES + 1).to_string(), // fetch one extra to detect truncation
            "--",
            pattern,
            path.to_str().unwrap_or("."),
        ])
        .output();

    match output {
        Err(_) => None,
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut matches = Vec::new();
            let mut truncated = false;

            for line in stdout.lines() {
                if line.is_empty() {
                    continue;
                }
                let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if val.get("type").and_then(|t| t.as_str()) != Some("match") {
                    continue;
                }
                let Some(data) = val.get("data") else {
                    continue;
                };
                let file_path = data
                    .get("path")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let Some(lines) = data
                    .get("lines")
                    .and_then(|l| l.get("text"))
                    .and_then(|t| t.as_str())
                else {
                    continue;
                };
                let line_number = data
                    .get("line_number")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);

                if matches.len() >= MAX_GREP_MATCHES {
                    truncated = true;
                    break;
                }
                matches.push(GrepMatch {
                    path: PathBuf::from(file_path),
                    line_number,
                    line_text: lines.trim_end_matches('\n').to_string(),
                });
            }

            Some(GrepResult {
                matches,
                truncated,
                backend: "rg".to_string(),
                error: None,
            })
        }
    }
}

// ── grep fallback backend ─────────────────────────────────────────────────────

fn try_grep(pattern: &str, path: &Path) -> GrepResult {
    let output = Command::new("grep")
        .no_window()
        .args(["-rn", "--", pattern, path.to_str().unwrap_or(".")])
        .output();

    match output {
        Err(e) => GrepResult {
            matches: Vec::new(),
            truncated: false,
            backend: "grep".to_string(),
            error: Some(format!("grep failed: {e}")),
        },
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut matches = Vec::new();
            let mut truncated = false;

            for line in stdout.lines() {
                if matches.len() >= MAX_GREP_MATCHES {
                    truncated = true;
                    break;
                }
                // grep -rn output: `path/to/file:line_number:line_text`
                let mut parts = line.splitn(3, ':');
                let file_path = parts.next().unwrap_or("").to_string();
                let line_number: u64 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let line_text = parts.next().unwrap_or("").to_string();

                matches.push(GrepMatch {
                    path: PathBuf::from(file_path),
                    line_number,
                    line_text,
                });
            }

            GrepResult {
                matches,
                truncated,
                backend: "grep".to_string(),
                error: None,
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pattern_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.rs");
        std::fs::write(&path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        let exec = GrepExecutor::new();
        let result = exec.execute("println", Some(dir.path()));
        assert!(
            !result.matches.is_empty(),
            "expected matches for 'println', got none"
        );
        assert!(result
            .matches
            .iter()
            .any(|m| m.line_text.contains("println")));
    }

    #[test]
    fn no_matches_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.rs");
        std::fs::write(&path, "fn foo() {}\n").unwrap();

        let exec = GrepExecutor::new();
        let result = exec.execute("nonexistent_xyz_pattern_12345", Some(dir.path()));
        assert!(result.matches.is_empty());
        assert!(!result.truncated);
    }

    #[test]
    fn respects_max_grep_matches() {
        let dir = tempfile::tempdir().unwrap();
        // Write a file with MAX_GREP_MATCHES + 10 matching lines.
        let mut content = String::new();
        for i in 0..(MAX_GREP_MATCHES + 10) {
            content.push_str(&format!("match_target line {i}\n"));
        }
        std::fs::write(dir.path().join("big.txt"), &content).unwrap();

        let exec = GrepExecutor::new();
        let result = exec.execute("match_target", Some(dir.path()));
        assert!(result.matches.len() <= MAX_GREP_MATCHES);
        assert!(result.truncated, "should report truncation");
    }

    #[test]
    fn grep_match_has_correct_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("check.rs");
        std::fs::write(&path, "// TODO: fix this\nfn bar() {}\n").unwrap();

        let exec = GrepExecutor::new();
        let result = exec.execute("TODO", Some(dir.path()));
        assert!(!result.matches.is_empty());
        let m = &result.matches[0];
        assert_eq!(m.line_number, 1);
        assert!(m.line_text.contains("TODO"));
    }
}
