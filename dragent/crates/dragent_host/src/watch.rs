//! Project file watcher (git-porcelain polling).
//!
//! One background thread polls every watched git checkout with
//! `git status --porcelain` and reports changed paths per workspace.
//! Polling (not inotify) is deliberate: no new native deps, uniform
//! behavior on macOS/Linux/Windows, and sane on network filesystems
//! where kernel watches fail. Git-native output means ignored files
//! never fire — the watcher reports exactly what version control sees.
//!
//! This is the host capability half: subscribers (UI now, headless
//! consumers later) fan out from the event stream. The app's existing
//! local watchers stay until a later slice migrates them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Poll cadence per loop pass (sequential across roots in one thread).
pub const WATCH_TICK: Duration = Duration::from_secs(5);
/// `git status` hard timeout per root (hung git must not stall others).
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct WatchedRoot {
    workspace_id: String,
    root: PathBuf,
    last_seen: HashMap<PathBuf, String>,
}

/// Parse `git status --porcelain=v1 -uall -- .` output into changed paths
/// (relative, as reported). Untracked (`??`) and conflicted entries count
/// — anything version control sees is worth a refresh.
pub(crate) fn parse_porcelain(output: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in output.lines() {
        // Rename entries: `R  old -> new` — the new path matters.
        let path_part = match line.split_once(" -> ") {
            Some((_, new)) => new.trim(),
            None => line.get(3..).unwrap_or("").trim(),
        };
        // Quoted paths (special chars): strip the quotes, keep the rest.
        let path_part = path_part
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(path_part);
        if path_part.is_empty() {
            continue;
        }
        paths.push(PathBuf::from(path_part));
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Snapshot one root: map of relative path → status fingerprint. The
/// fingerprint is the porcelain status code pair, so content edits,
/// staging flips, and renames all register as changes. A hung git is
/// cut off at [`GIT_TIMEOUT`] (helper thread + channel — std has no
/// process timeout).
fn snapshot_root(root: &Path) -> Option<HashMap<PathBuf, String>> {
    let root = root.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("dragent-git-snapshot".to_string())
        .spawn(move || {
            let output = crate::git_command()
                .current_dir(&root)
                .args(["status", "--porcelain=v1", "-uall", "--", "."])
                .output()
                .ok();
            let _ = tx.send(output);
        })
        .ok()?;
    let output = rx.recv_timeout(GIT_TIMEOUT).ok()??;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        let code = line.get(..2).unwrap_or("").to_string();
        let path_part = match line.split_once(" -> ") {
            Some((_, new)) => new.trim(),
            None => line.get(3..).unwrap_or("").trim(),
        };
        let path_part = path_part
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(path_part);
        if path_part.is_empty() {
            continue;
        }
        map.insert(PathBuf::from(path_part), code);
    }
    Some(map)
}

/// Diff two snapshots into the changed relative paths.
pub(crate) fn diff_snapshots(
    before: &HashMap<PathBuf, String>,
    after: &HashMap<PathBuf, String>,
) -> Vec<PathBuf> {
    let mut changed: Vec<PathBuf> = after
        .iter()
        .filter(|(path, fingerprint)| before.get(*path) != Some(*fingerprint))
        .map(|(path, _)| path.clone())
        .collect();
    for path in before.keys() {
        if !after.contains_key(path) {
            changed.push(path.clone());
        }
    }
    changed.sort();
    changed.dedup();
    changed
}

pub struct WatchRegistry {
    watched: Mutex<HashMap<String, WatchedRoot>>,
}

impl WatchRegistry {
    pub fn new() -> Self {
        Self {
            watched: Mutex::new(HashMap::new()),
        }
    }

    /// Watch one workspace root (must be a git checkout). Idempotent per
    /// workspace; re-watching a new root replaces the old watch.
    pub fn watch(&self, workspace_id: &str, root: &Path) -> anyhow::Result<()> {
        if !root.join(".git").exists() {
            anyhow::bail!("not a git checkout: {}", root.display());
        }
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let baseline = snapshot_root(&canonical).unwrap_or_default();
        self.watched
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                workspace_id.to_string(),
                WatchedRoot {
                    workspace_id: workspace_id.to_string(),
                    root: canonical,
                    last_seen: baseline,
                },
            );
        Ok(())
    }

    pub fn unwatch(&self, workspace_id: &str) -> bool {
        self.watched
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(workspace_id)
            .is_some()
    }

    pub fn watched_workspaces(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .watched
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// One poll pass over every watched root. Returns
    /// `(workspace_id, changed_paths)` for roots that moved.
    /// Failures (hung git, deleted root) report empty and keep the old
    /// baseline — a flaky poll must never look like mass deletion.
    pub fn poll_once(&self) -> Vec<(String, Vec<PathBuf>)> {
        let mut changed = Vec::new();
        let mut watched = self.watched.lock().unwrap_or_else(|p| p.into_inner());
        let ids: Vec<String> = watched.keys().cloned().collect();
        for id in ids {
            let Some(entry) = watched.get_mut(&id) else {
                continue;
            };
            let Some(current) = snapshot_root(&entry.root) else {
                continue;
            };
            let delta = diff_snapshots(&entry.last_seen, &current);
            entry.last_seen = current;
            if !delta.is_empty() {
                changed.push((entry.workspace_id.clone(), delta));
            }
        }
        changed
    }
}

impl Default for WatchRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Poll loop driver: runs until `shutdown` flips, invoking `on_change`
/// per moved workspace. The caller owns the thread (one loop, all roots).
pub fn drive(
    registry: std::sync::Arc<WatchRegistry>,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut on_change: impl FnMut(String, Vec<PathBuf>),
) {
    let mut first = true;
    loop {
        if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if !first {
            // Jitter the cadence slightly so N hosts on one machine don't
            // git-poll in lockstep.
            std::thread::sleep(WATCH_TICK + Duration::from_millis((now_ms() % 500) as u64));
            if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
        }
        first = false;
        let started = Instant::now();
        for (workspace_id, paths) in registry.poll_once() {
            on_change(workspace_id, paths);
        }
        // A pathological tree must not spin the loop hot: say so loudly.
        let elapsed = started.elapsed();
        if elapsed > WATCH_TICK * 4 {
            log::warn!("watch poll took {}ms (tree too big?)", elapsed.as_millis());
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_parse_covers_rename_untracked_and_quotes() {
        let out =
            " M src/main.rs\nA  new.rs\n?? untracked.txt\nR  old.rs -> new_name.rs\n D gone.rs\n";
        let paths = parse_porcelain(out);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("gone.rs"),
                PathBuf::from("new.rs"),
                PathBuf::from("new_name.rs"),
                PathBuf::from("src/main.rs"),
                PathBuf::from("untracked.txt"),
            ]
        );
    }

    #[test]
    fn diff_detects_modify_add_and_remove() {
        let before: HashMap<PathBuf, String> = [
            (PathBuf::from("a.rs"), "M ".to_string()),
            (PathBuf::from("gone.rs"), "M ".to_string()),
        ]
        .into_iter()
        .collect();
        let after: HashMap<PathBuf, String> = [
            (PathBuf::from("a.rs"), "MM".to_string()),
            (PathBuf::from("b.rs"), "??".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            diff_snapshots(&before, &after),
            vec![
                PathBuf::from("a.rs"),
                PathBuf::from("b.rs"),
                PathBuf::from("gone.rs"),
            ]
        );
    }

    #[test]
    fn watch_rejects_non_git_dirs() {
        let registry = WatchRegistry::new();
        let dir = std::env::temp_dir().join(format!("dragent-watch-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(registry.watch("ws-1", &dir).is_err());
        assert!(registry.watched_workspaces().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn poll_detects_real_file_changes() {
        let dir = std::env::temp_dir().join(format!("dragent-watch-git-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "v1").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);

        let registry = WatchRegistry::new();
        registry.watch("ws-1", &dir).unwrap();
        assert!(registry.poll_once().is_empty());
        std::fs::write(dir.join("a.txt"), "v2").unwrap();
        std::fs::write(dir.join("b.txt"), "new").unwrap();
        let changes = registry.poll_once();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "ws-1");
        assert_eq!(
            changes[0].1,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]
        );
        assert!(registry.poll_once().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
