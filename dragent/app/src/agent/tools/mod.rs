//! Agent tool executors — read_files, grep, write_file, edit_file, list_directory, search_codebase, and the registry.

pub mod grep;
pub mod read_files;
pub mod search_codebase;
pub mod write_file;

use std::path::{Path, PathBuf};

use crate::agent::permissions::AllowedRoots;
pub use grep::{GrepExecutor, GrepMatch, GrepResult};
pub use read_files::{FileContent, ReadFilesExecutor, ReadFilesResult};
pub use search_codebase::{FragmentHit, SearchCodebaseExecutor, SearchCodebaseResult};
pub use write_file::{
    DiffHunk, DiffLine, EditFileExecutor, EditFileOutcome, FileCheckpoint, ListDirectoryExecutor,
    ListDirectoryResult, WriteFileExecutor, WriteFileOutcome,
};

// ── ToolResultKind ────────────────────────────────────────────────────────────

pub use dragent_agent::files::ToolResultKind;
// ── ToolContext ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub allowed_roots: AllowedRoots,
}

// ── ToolExecutorRegistry ──────────────────────────────────────────────────────

pub struct ToolExecutorRegistry {
    pub read_files: ReadFilesExecutor,
    pub grep: GrepExecutor,
    pub search: SearchCodebaseExecutor,
    pub write_file: WriteFileExecutor,
    pub edit_file: EditFileExecutor,
    pub list_dir: ListDirectoryExecutor,
    allowed_roots: AllowedRoots,
}

impl ToolExecutorRegistry {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self {
            read_files: ReadFilesExecutor::new(allowed_roots.clone()),
            grep: GrepExecutor::new(),
            search: SearchCodebaseExecutor::new(allowed_roots.clone()),
            write_file: WriteFileExecutor::new(allowed_roots.clone()),
            edit_file: EditFileExecutor::new(allowed_roots.clone()),
            list_dir: ListDirectoryExecutor::new(),
            allowed_roots,
        }
    }

    pub fn run_read_files(&self, paths: &[PathBuf]) -> ToolResultKind {
        ToolResultKind::ReadFiles(self.read_files.execute(paths))
    }

    pub fn run_grep(&self, pattern: &str, path: Option<&Path>) -> ToolResultKind {
        ToolResultKind::Grep(self.grep.execute(pattern, path))
    }

    pub fn run_search_codebase(&self, query: &str, repo_root: &Path) -> ToolResultKind {
        ToolResultKind::SearchCodebase(self.search.execute(query, repo_root))
    }

    pub fn run_write_file(&self, path: &Path, content: &str) -> ToolResultKind {
        ToolResultKind::WriteFile(self.write_file.execute(path, content))
    }

    pub fn run_edit_file(&self, path: &Path, old_str: &str, new_str: &str) -> ToolResultKind {
        ToolResultKind::EditFile(self.edit_file.execute(path, old_str, new_str))
    }

    pub fn run_list_directory(&self, path: &Path, recursive: bool) -> ToolResultKind {
        ToolResultKind::ListDirectory(self.list_dir.execute(path, recursive))
    }

    /// Run a verification/test command in `cwd` and return a compact summary of
    /// its exit status and output. Used by the self-heal agent to confirm its
    /// fix worked. The command's working directory must fall under an allowed
    /// root, and both runtime and captured output are bounded.
    fn run_verification_command(&self, command: &str, cwd: &Path) -> String {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::time::Duration;
        use web_time::Instant;

        if command.trim().is_empty() {
            return "Error: empty command".to_string();
        }
        // The command runs inside cwd — refuse if that directory isn't allowed.
        if self.allowed_roots.check(cwd).is_err() {
            return format!(
                "Permission denied: {} is outside the allowed roots",
                cwd.display()
            );
        }

        const TIMEOUT: Duration = Duration::from_secs(120);
        const MAX_OUTPUT: usize = 16 * 1024;

        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => return format!("Failed to start command: {e}"),
        };

        // Poll for completion up to the timeout, then kill.
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if start.elapsed() >= TIMEOUT {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break None,
            }
        };

        let mut out = String::new();
        if let Some(mut so) = child.stdout.take() {
            let _ = so.read_to_string(&mut out);
        }
        let mut err = String::new();
        if let Some(mut se) = child.stderr.take() {
            let _ = se.read_to_string(&mut err);
        }

        let mut combined = String::new();
        if !out.trim().is_empty() {
            combined.push_str(out.trim_end());
        }
        if !err.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(err.trim_end());
        }
        if combined.len() > MAX_OUTPUT {
            let tail = &combined[combined.len() - MAX_OUTPUT..];
            combined = format!("…(truncated)…\n{tail}");
        }

        match status {
            Some(s) if s.success() => format!("PASSED (exit 0)\n$ {command}\n{combined}")
                .trim_end()
                .to_string(),
            Some(s) => {
                let code = s
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                format!("FAILED (exit {code})\n$ {command}\n{combined}")
                    .trim_end()
                    .to_string()
            }
            None => format!(
                "TIMED OUT after {}s\n$ {command}\n{combined}",
                TIMEOUT.as_secs()
            )
            .trim_end()
            .to_string(),
        }
    }

    pub fn execute_and_summarize(&self, name: &str, args_json: &str, cwd: &Path) -> String {
        match name {
            "read_file" => {
                let path: PathBuf = serde_json::from_str(args_json)
                    .ok()
                    .and_then(|v: serde_json::Value| v["path"].as_str().map(|s| cwd.join(s)))
                    .unwrap_or_else(|| PathBuf::from(args_json.trim_matches('"')));
                match self.read_files.execute(&[path]) {
                    r if r.successful().count() > 0 => r
                        .successful()
                        .map(|f| {
                            format!(
                                "=== {} ===\n{}",
                                f.path.display(),
                                f.content.as_deref().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                    r => format!(
                        "Error reading file: {:?}",
                        r.files.first().and_then(|f| f.skip_reason.as_deref())
                    ),
                }
            }
            "list_directory" => {
                let (path, recursive): (PathBuf, bool) = serde_json::from_str(args_json)
                    .ok()
                    .map(|v: serde_json::Value| {
                        let p = v["path"]
                            .as_str()
                            .map(|s| cwd.join(s))
                            .unwrap_or_else(|| cwd.to_path_buf());
                        let r = v["recursive"].as_bool().unwrap_or(false);
                        (p, r)
                    })
                    .unwrap_or_else(|| (cwd.to_path_buf(), false));
                let result = self.list_dir.execute(&path, recursive);
                let lines: Vec<String> = result
                    .entries
                    .iter()
                    .map(|e| {
                        let kind = if e.is_dir { "dir" } else { "file" };
                        format!("[{}] {}", kind, e.path.display())
                    })
                    .collect();
                let mut out = lines.join("\n");
                if result.truncated {
                    out.push_str("\n(truncated)");
                }
                out
            }
            "grep" => {
                let (pattern, path): (String, Option<PathBuf>) = serde_json::from_str(args_json)
                    .ok()
                    .map(|v: serde_json::Value| {
                        let pat = v["pattern"].as_str().unwrap_or("").to_string();
                        let p = v["path"].as_str().map(|s| cwd.join(s));
                        (pat, p)
                    })
                    .unwrap_or_else(|| (args_json.to_string(), None));
                let result = self.grep.execute(&pattern, path.as_deref());
                result
                    .matches
                    .iter()
                    .map(|m| format!("{}:{}: {}", m.path.display(), m.line_number, m.line_text))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "write_file" => {
                let (path, content): (PathBuf, String) = serde_json::from_str(args_json)
                    .ok()
                    .map(|v: serde_json::Value| {
                        let p = v["path"]
                            .as_str()
                            .map(|s| cwd.join(s))
                            .unwrap_or_else(|| cwd.join("output.txt"));
                        let c = v["content"].as_str().unwrap_or("").to_string();
                        (p, c)
                    })
                    .unwrap_or_else(|| (cwd.join("output.txt"), String::new()));
                match self.write_file.execute(&path, &content) {
                    WriteFileOutcome::Created => format!("Created {}", path.display()),
                    WriteFileOutcome::Written { diff_hunks } => format!(
                        "Updated {} ({} hunks changed)",
                        path.display(),
                        diff_hunks.len()
                    ),
                    WriteFileOutcome::PermissionDenied { path } => {
                        format!("Permission denied: {}", path.display())
                    }
                    WriteFileOutcome::IoError { message, .. } => format!("IO error: {message}"),
                }
            }
            "edit_file" => {
                let (path, old_str, new_str): (PathBuf, String, String) =
                    serde_json::from_str(args_json)
                        .ok()
                        .map(|v: serde_json::Value| {
                            let p = v["path"].as_str().map(|s| cwd.join(s)).unwrap_or_default();
                            let o = v["old_str"].as_str().unwrap_or("").to_string();
                            let n = v["new_str"].as_str().unwrap_or("").to_string();
                            (p, o, n)
                        })
                        .unwrap_or_default();
                match self.edit_file.execute(&path, &old_str, &new_str) {
                    EditFileOutcome::Edited { diff_hunks } => {
                        format!("Edited {} ({} hunks)", path.display(), diff_hunks.len())
                    }
                    EditFileOutcome::OldStrNotFound { .. } => {
                        format!("String not found in {}", path.display())
                    }
                    EditFileOutcome::PermissionDenied { path } => {
                        format!("Permission denied: {}", path.display())
                    }
                    EditFileOutcome::IoError { message, .. } => format!("IO error: {message}"),
                }
            }
            "search_codebase" | "search" => {
                let query: String = serde_json::from_str(args_json)
                    .ok()
                    .and_then(|v: serde_json::Value| v["query"].as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| args_json.trim_matches('"').to_string());
                let result = self.search.execute(&query, cwd);
                if result.hits.is_empty() {
                    format!("No results found for: {query}")
                } else {
                    let lines: Vec<String> = result
                        .hits
                        .iter()
                        .map(|h| {
                            let sym_str = h
                                .matched_symbol
                                .as_deref()
                                .map(|s| format!(" ({})", s))
                                .unwrap_or_default();
                            let preview =
                                h.preview.lines().take(5).collect::<Vec<_>>().join("\n    ");
                            format!(
                                "{}:{}{}\n    {}",
                                h.path.display(),
                                h.byte_start,
                                sym_str,
                                preview
                            )
                        })
                        .collect();
                    lines.join("\n\n")
                }
            }
            "explore" => {
                crate::agent::explore::execute_from_args(args_json, cwd, &self.allowed_roots)
            }
            "run_command" => {
                let command: String = serde_json::from_str(args_json)
                    .ok()
                    .and_then(|v: serde_json::Value| v["command"].as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| args_json.trim_matches('"').to_string());
                self.run_verification_command(&command, cwd)
            }
            _ => format!("Unknown tool: {name}"),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with_tmp(dir: &std::path::Path) -> ToolExecutorRegistry {
        let roots = AllowedRoots::new(std::iter::once(dir.to_path_buf()));
        ToolExecutorRegistry::new(roots)
    }

    #[test]
    fn run_read_files_returns_read_files_result() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "content").unwrap();

        let reg = registry_with_tmp(dir.path());
        let result = reg.run_read_files(&[dir.path().join("a.txt")]);
        assert!(matches!(result, ToolResultKind::ReadFiles(_)));
    }

    #[test]
    fn run_grep_returns_grep_result() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "find_me\n").unwrap();

        let reg = registry_with_tmp(dir.path());
        let result = reg.run_grep("find_me", Some(dir.path()));
        assert!(matches!(result, ToolResultKind::Grep(_)));
    }

    #[cfg(unix)]
    #[test]
    fn run_command_reports_pass_and_fail() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_with_tmp(dir.path());

        let ok = reg.execute_and_summarize("run_command", r#"{"command":"echo hi"}"#, dir.path());
        assert!(ok.starts_with("PASSED"), "expected PASSED, got: {ok}");
        assert!(ok.contains("hi"), "expected stdout captured, got: {ok}");

        let bad = reg.execute_and_summarize("run_command", r#"{"command":"exit 3"}"#, dir.path());
        assert!(
            bad.starts_with("FAILED (exit 3)"),
            "expected FAILED exit 3, got: {bad}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_command_denies_cwd_outside_roots() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_with_tmp(dir.path());
        // A cwd that isn't under the allowed root must be refused.
        let outside = std::env::temp_dir();
        let out = reg.execute_and_summarize("run_command", r#"{"command":"echo hi"}"#, &outside);
        assert!(
            out.starts_with("Permission denied"),
            "expected denial, got: {out}"
        );
    }
}
