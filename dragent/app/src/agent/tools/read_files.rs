//! Tool: read one or more files and return their contents.

use std::path::{Path, PathBuf};

use crate::agent::permissions::AllowedRoots;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of bytes read from a single file.
pub const MAX_READ_FILE_BYTES: usize = 262_144; // 256 KiB

// ── Types ─────────────────────────────────────────────────────────────────────
// Single definitions in `dragent_agent::files` shared by the engine core and
// the UI.
pub use dragent_agent::files::{FileContent, ReadFilesResult};

// ── Executor ──────────────────────────────────────────────────────────────────

pub struct ReadFilesExecutor {
    allowed_roots: AllowedRoots,
}

impl ReadFilesExecutor {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self { allowed_roots }
    }

    /// Read each path. Never panics — errors are captured in `FileContent::skip_reason`.
    pub fn execute(&self, paths: &[PathBuf]) -> ReadFilesResult {
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            files.push(self.read_one(path));
        }
        ReadFilesResult { files }
    }

    fn read_one(&self, path: &Path) -> FileContent {
        // Spilled tool outputs are agent-visible by design (the model already
        // saw their head inline): allow exactly that directory. Symlinks
        // resolving outside it fall through to the sandbox check below.
        let spill_allowed = crate::agent::tool_output_spill::is_spill_path(path);
        // Permission check first.
        if !spill_allowed {
            if let Err(reason) = self.allowed_roots.check(path) {
                return FileContent {
                    path: path.to_path_buf(),
                    content: None,
                    skip_reason: Some(format!("access denied: {reason}")),
                };
            }
        }

        // Read raw bytes — cap at MAX_READ_FILE_BYTES.
        let raw = match read_capped(path, MAX_READ_FILE_BYTES) {
            Ok(b) => b,
            Err(e) => {
                return FileContent {
                    path: path.to_path_buf(),
                    content: None,
                    skip_reason: Some(format!("read error: {e}")),
                }
            }
        };

        // Binary detection: reject files with null bytes in the first 8 KiB.
        if is_binary(&raw) {
            return FileContent {
                path: path.to_path_buf(),
                content: None,
                skip_reason: Some("binary file skipped".to_string()),
            };
        }

        // Convert to UTF-8, replacing invalid sequences.
        let content = String::from_utf8_lossy(&raw).into_owned();

        FileContent {
            path: path.to_path_buf(),
            content: Some(content),
            skip_reason: None,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Read up to `max_bytes` from a file.
fn read_capped(path: &Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(max_bytes.min(65_536));
    f.by_ref().take(max_bytes as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Detect binary by looking for null bytes in the first 8 KiB.
fn is_binary(data: &[u8]) -> bool {
    let check_len = data.len().min(8_192);
    data[..check_len].contains(&0u8)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn executor_with_tmp() -> (ReadFilesExecutor, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let roots = AllowedRoots::new(std::iter::once(dir.path().to_path_buf()));
        (ReadFilesExecutor::new(roots), dir)
    }

    #[test]
    fn reads_text_file_successfully() {
        let (exec, dir) = executor_with_tmp();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello world\n").unwrap();

        let result = exec.execute(&[path]);
        assert_eq!(result.files.len(), 1);
        let fc = &result.files[0];
        assert!(fc.content.is_some());
        assert_eq!(fc.content.as_deref(), Some("hello world\n"));
        assert!(fc.skip_reason.is_none());
    }

    #[test]
    fn skips_binary_file() {
        let (exec, dir) = executor_with_tmp();
        let path = dir.path().join("binary.bin");
        // Write bytes containing a null byte.
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&[0x50, 0x00, 0x46, 0x00]).unwrap();

        let result = exec.execute(&[path]);
        let fc = &result.files[0];
        assert!(fc.content.is_none());
        assert!(fc.skip_reason.as_deref().unwrap_or("").contains("binary"));
    }

    #[test]
    fn denies_path_outside_roots() {
        let (exec, _dir) = executor_with_tmp();
        let result = exec.execute(&[PathBuf::from("/etc/passwd")]);
        let fc = &result.files[0];
        assert!(fc.content.is_none());
        assert!(fc.skip_reason.is_some());
    }

    #[test]
    fn missing_file_returns_skip_reason() {
        let (exec, dir) = executor_with_tmp();
        let path = dir.path().join("nonexistent.rs");
        let result = exec.execute(&[path]);
        let fc = &result.files[0];
        assert!(fc.content.is_none());
        assert!(fc.skip_reason.is_some());
    }

    #[test]
    fn total_bytes_sums_successfully_read() {
        let (exec, dir) = executor_with_tmp();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        std::fs::write(&p1, "abc").unwrap();
        std::fs::write(&p2, "de").unwrap();
        let result = exec.execute(&[p1, p2]);
        assert_eq!(result.total_bytes(), 5);
    }

    #[test]
    fn successful_iterator_excludes_skipped() {
        let (exec, dir) = executor_with_tmp();
        let good = dir.path().join("good.txt");
        std::fs::write(&good, "ok").unwrap();
        let bad = PathBuf::from("/etc/passwd");

        let result = exec.execute(&[good, bad]);
        let successful: Vec<_> = result.successful().collect();
        assert_eq!(successful.len(), 1);
        assert_eq!(successful[0].content.as_deref(), Some("ok"));
    }

    #[test]
    fn is_binary_detects_null_bytes() {
        assert!(is_binary(&[0x00, 0x01, 0x02]));
        assert!(!is_binary(b"hello world"));
    }
}
