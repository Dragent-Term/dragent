//! WriteFile and EditFile tool executors with diff generation.

use std::path::{Path, PathBuf};

use crate::agent::permissions::AllowedRoots;

// ── Diff ──────────────────────────────────────────────────────────────────────

pub use dragent_agent::diff::{line_diff, DiffHunk, DiffLine};

// ── WriteFile ─────────────────────────────────────────────────────────────────

pub use dragent_agent::files::WriteFileOutcome;

pub struct WriteFileExecutor {
    allowed_roots: AllowedRoots,
}

impl WriteFileExecutor {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self { allowed_roots }
    }

    /// Write `content` to `path`. Creates parent directories as needed.
    /// Computes a diff against the previous content if the file already exists.
    pub fn execute(&self, path: &Path, content: &str) -> WriteFileOutcome {
        if self.allowed_roots.check(path).is_err() {
            return WriteFileOutcome::PermissionDenied {
                path: path.to_path_buf(),
            };
        }

        let old_content = std::fs::read_to_string(path).unwrap_or_default();
        let is_new = !path.exists();

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return WriteFileOutcome::IoError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                };
            }
        }

        if let Err(e) = std::fs::write(path, content) {
            return WriteFileOutcome::IoError {
                path: path.to_path_buf(),
                message: e.to_string(),
            };
        }

        if is_new {
            WriteFileOutcome::Created
        } else {
            WriteFileOutcome::Written {
                diff_hunks: line_diff(&old_content, content),
            }
        }
    }
}

// ── EditFile ──────────────────────────────────────────────────────────────────

pub use dragent_agent::files::EditFileOutcome;

pub struct EditFileExecutor {
    allowed_roots: AllowedRoots,
}

impl EditFileExecutor {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self { allowed_roots }
    }

    /// Replace the first occurrence of `old_str` with `new_str` in `path`.
    pub fn execute(&self, path: &Path, old_str: &str, new_str: &str) -> EditFileOutcome {
        if self.allowed_roots.check(path).is_err() {
            return EditFileOutcome::PermissionDenied {
                path: path.to_path_buf(),
            };
        }

        let old_content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                return EditFileOutcome::IoError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                }
            }
        };

        if !old_content.contains(old_str) {
            return EditFileOutcome::OldStrNotFound {
                path: path.to_path_buf(),
                old_str: old_str.to_string(),
            };
        }

        let new_content = old_content.replacen(old_str, new_str, 1);

        if let Err(e) = std::fs::write(path, &new_content) {
            return EditFileOutcome::IoError {
                path: path.to_path_buf(),
                message: e.to_string(),
            };
        }

        EditFileOutcome::Edited {
            diff_hunks: line_diff(&old_content, &new_content),
        }
    }
}

// ── ListDirectory ─────────────────────────────────────────────────────────────

pub use dragent_agent::files::{DirEntry, ListDirectoryResult};

pub struct ListDirectoryExecutor;

impl ListDirectoryExecutor {
    pub fn new() -> Self {
        Self
    }

    pub fn execute(&self, path: &Path, recursive: bool) -> ListDirectoryResult {
        const MAX_ENTRIES: usize = 200;
        let mut entries = Vec::new();
        self.collect(path, recursive, &mut entries, MAX_ENTRIES);
        let truncated = entries.len() >= MAX_ENTRIES;
        ListDirectoryResult { entries, truncated }
    }

    fn collect(&self, path: &Path, recursive: bool, out: &mut Vec<DirEntry>, max: usize) {
        if out.len() >= max {
            return;
        }
        let read_dir = match std::fs::read_dir(path) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        let mut child_dirs = Vec::new();
        for entry in read_dir.flatten() {
            if out.len() >= max {
                break;
            }
            let child_path = entry.path();
            let meta = entry.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);

            // Skip hidden dirs and common noise dirs
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
                continue;
            }

            out.push(DirEntry {
                path: child_path.clone(),
                is_dir,
                size_bytes,
            });
            if is_dir {
                child_dirs.push(child_path);
            }
        }
        if recursive {
            for dir in child_dirs {
                self.collect(&dir, true, out, max);
            }
        }
    }
}

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// Saves a snapshot of file contents before an edit so it can be reverted.
/// Single definition in `dragent_agent::files` shared by the engine core and
/// the Host.
pub use dragent_agent::files::FileCheckpoint;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_file_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let roots =
            crate::agent::permissions::AllowedRoots::new(std::iter::once(dir.path().to_path_buf()));
        let ex = WriteFileExecutor::new(roots);
        assert!(matches!(
            ex.execute(&path, "hello"),
            WriteFileOutcome::Created
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_file_produces_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        std::fs::write(&path, "line1\nline2\n").unwrap();
        let roots =
            crate::agent::permissions::AllowedRoots::new(std::iter::once(dir.path().to_path_buf()));
        let ex = WriteFileExecutor::new(roots);
        let outcome = ex.execute(&path, "line1\nline2 modified\n");
        assert!(matches!(outcome, WriteFileOutcome::Written { .. }));
    }

    #[test]
    fn edit_file_replaces_old_str() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code.rs");
        std::fs::write(&path, "fn main() { println!(\"hello\"); }").unwrap();
        let roots =
            crate::agent::permissions::AllowedRoots::new(std::iter::once(dir.path().to_path_buf()));
        let ex = EditFileExecutor::new(roots);
        let outcome = ex.execute(&path, "hello", "world");
        assert!(matches!(outcome, EditFileOutcome::Edited { .. }));
        assert!(std::fs::read_to_string(&path).unwrap().contains("world"));
    }

    #[test]
    fn edit_file_not_found_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("code.rs");
        std::fs::write(&path, "fn main() {}").unwrap();
        let roots =
            crate::agent::permissions::AllowedRoots::new(std::iter::once(dir.path().to_path_buf()));
        let ex = EditFileExecutor::new(roots);
        assert!(matches!(
            ex.execute(&path, "nonexistent", "x"),
            EditFileOutcome::OldStrNotFound { .. }
        ));
    }

    #[test]
    fn checkpoint_reverts_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "original").unwrap();
        let mut cp = FileCheckpoint::new();
        cp.snapshot(&path);
        std::fs::write(&path, "modified").unwrap();
        cp.revert();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    }

    #[test]
    fn list_directory_returns_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        let result = ListDirectoryExecutor::new().execute(dir.path(), false);
        assert_eq!(result.entries.len(), 2);
    }
}
