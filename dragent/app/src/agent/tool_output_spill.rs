//! Spilled tool outputs (opencode `TRUNCATION_DIR` parity).
//!
//! Tool results too large for the context window used to be cut with a
//! dead-end marker. Now the full text is spilled to
//! `{data}/dragent/tool-output/tool_<uuid>.txt` and the model gets a pointer
//! it can actually follow: the read path allowlists exactly this directory,
//! so `read_file` with offset/limit retrieves any slice. Stale spills are
//! pruned opportunistically (7-day retention, same as local run checkpoints).
//!
//! Security note: files here contain only tool outputs the model already
//! partially saw — spilling reveals nothing new versus no truncation, and the
//! allowlist covers this single directory (symlinks resolving outside it fall
//! through to the normal sandbox check and are denied).

use std::path::{Path, PathBuf};
use std::time::Duration;

const STALE_SPILL_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Env override for tests and power users; otherwise the shared data dir.
pub fn spill_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("DRAGENT_TOOL_OUTPUT_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    dragent_paths::home().join("tool-output")
}

/// Write the full output to a fresh spill file. Returns the path on success;
/// callers fall back to plain truncation when this is None.
pub fn spill_output(output: &str) -> Option<PathBuf> {
    let dir = spill_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    prune_stale_spills(&dir);
    let path = dir.join(format!("tool_{}.txt", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, output).ok()?;
    Some(path)
}

/// True when `path` resolves inside the spill directory. Canonicalizes both
/// sides, so symlinks escaping the directory (or unresolvable paths) return
/// false and fall through to the normal sandbox check.
pub fn is_spill_path(path: &Path) -> bool {
    let Ok(dir) = spill_dir().canonicalize() else {
        return false;
    };
    // Unresolvable paths fail closed into the normal sandbox check.
    let Ok(target) = path.canonicalize() else {
        return false;
    };
    target == dir || target.starts_with(&dir)
}

fn is_stale_spill(mtime: std::time::SystemTime, now: std::time::SystemTime) -> bool {
    now.duration_since(mtime).unwrap_or_default() >= STALE_SPILL_AGE
}

fn prune_stale_spills(dir: &Path) {
    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|mtime| is_stale_spill(mtime, now))
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env override is process-global: serialize these tests so parallel
    // siblings can never observe a foreign spill dir.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_spill_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let spill = dir.path().join("tool-output");
        (dir, spill)
    }

    #[test]
    fn spill_writes_readable_file_and_allowlists_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, spill) = temp_spill_dir();
        std::env::set_var("DRAGENT_TOOL_OUTPUT_DIR", &spill);
        let path = spill_output("hello spill").expect("spill succeeds");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello spill");
        assert!(is_spill_path(&path));
        std::env::remove_var("DRAGENT_TOOL_OUTPUT_DIR");
    }

    #[test]
    fn paths_outside_spill_dir_are_rejected() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, spill) = temp_spill_dir();
        std::env::set_var("DRAGENT_TOOL_OUTPUT_DIR", &spill);
        std::fs::create_dir_all(&spill).unwrap();
        assert!(!is_spill_path(Path::new("/etc/hostname")));
        assert!(!is_spill_path(Path::new("/nonexistent-xyz/file.txt")));
        std::env::remove_var("DRAGENT_TOOL_OUTPUT_DIR");
    }

    #[test]
    fn staleness_boundary_is_seven_days() {
        let now = std::time::SystemTime::now();
        let old = now - STALE_SPILL_AGE - std::time::Duration::from_secs(1);
        let fresh = now - STALE_SPILL_AGE + std::time::Duration::from_secs(60);
        assert!(is_stale_spill(old, now));
        assert!(!is_stale_spill(fresh, now));
        assert!(!is_stale_spill(now, now));
    }

    #[test]
    fn prune_keeps_fresh_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_tmp, spill) = temp_spill_dir();
        std::env::set_var("DRAGENT_TOOL_OUTPUT_DIR", &spill);
        std::fs::create_dir_all(&spill).unwrap();
        std::fs::write(spill.join("fresh.txt"), "x").unwrap();
        prune_stale_spills(&spill);
        assert!(spill.join("fresh.txt").exists());
        std::env::remove_var("DRAGENT_TOOL_OUTPUT_DIR");
    }
}
