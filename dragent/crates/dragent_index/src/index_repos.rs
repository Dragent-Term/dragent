use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use super::manager::set_background_qos;
use crate::CodeIndexManager;
use crate::RepoIndexState;
use crate::RepoIndexStatus;
use crate::index_batch::delete_index_artifacts;
use crate::index_batch::normalize_root;
use crate::index_batch::snapshot_path_for_repo;
use crate::manager::DEFAULT_MAX_FILES;
use crate::manager::RepoState;

// ── Thread priority ───────────────────────────────────────────────────────────

/// Drop the calling thread to background (Utility) scheduling so indexing —
/// walking, chunking, and especially ONNX embedding — never competes with the
/// UI thread for cores. On Apple platforms the ONNX Runtime's intra-op pool
/// inherits the QoS of the thread that creates the session, so calling this
/// before `Embedder::load` covers inference threads too. The terminal must
/// never be the reason the app stutters.

// ── Global static ─────────────────────────────────────────────────────────────

/// Default per-repo file budget, used when a parked/unknown root has to be
/// re-registered without its original budget.

impl CodeIndexManager {
    /// Force a full rebuild of one repo's index: stop its watcher, drop the
    /// snapshot (which also clears the store on the way back in), re-register.
    pub fn reindex(&'static self, root: &Path) {
        let root = normalize_root(root);
        let handle = {
            let mut inner = self.inner.lock().unwrap();
            inner.repos.remove(&root)
        };
        let (max_files, watcher_done) = match handle {
            Some(h) => {
                h.stop.store(true, Ordering::Relaxed);
                (h.max_files, Some(h.watcher_done))
            }
            None => (DEFAULT_MAX_FILES, None),
        };
        std::thread::spawn(move || {
            set_background_qos();
            if let Some(done) = watcher_done {
                // Never race a retiring watcher that may still be finishing an
                // embed batch and snapshot write. This wait is not on the UI.
                let _ = done.recv();
            }
            // No snapshot makes registration clear the store and embed every
            // current file rather than trusting an incremental diff.
            let _ = std::fs::remove_file(snapshot_path_for_repo(&root));
            self.register_repo(root, max_files);
        });
    }

    /// Delete a repo's index entirely: stop its watcher, drop it from the
    /// registry, and remove its on-disk store + snapshot. The folder itself is
    /// untouched; re-opening it in a terminal re-indexes from scratch.
    pub fn remove_index(&'static self, root: &Path) {
        let root = normalize_root(root);
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(handle) = inner.repos.remove(&root) {
                // remove_files BEFORE stop (both SeqCst) so the watcher can
                // never observe stop without also seeing the removal request.
                handle.remove_files.store(true, Ordering::SeqCst);
                handle.stop.store(true, Ordering::SeqCst);
            }
            // Dropping the root from `registering` aborts an in-flight initial
            // pass at its registration step (it re-checks membership there).
            inner.registering.remove(&root);
            inner.disabled_roots.retain(|(r, _)| *r != root);
        }
        // Best-effort immediate delete for the no-watcher case. When a watcher
        // exists it deletes again on exit — after the last possible snapshot/DB
        // write and after closing the SQLite handle, which is what makes the
        // delete stick on Windows and immune to a mid-batch rewrite.
        delete_index_artifacts(&root);
        log::info!("[index] removed index for {}", root.display());
    }

    /// Re-index every registered repo (used after ignore rules change).
    pub fn reindex_all(&'static self) {
        let roots: Vec<PathBuf> = {
            let inner = self.inner.lock().unwrap();
            inner.repos.keys().cloned().collect()
        };
        for root in roots {
            self.reindex(&root);
        }
    }

    /// Register a repo and kick off incremental indexing + file watching in the background.
    ///
    /// Idempotent and hierarchy-aware: a root already covered by a registered
    /// ancestor is skipped (the ancestor's index contains it), and registering
    /// an ancestor stops + replaces any registered descendants, so a file is
    /// never indexed by two overlapping watchers.
    pub fn register_repo(&'static self, root: PathBuf, max_files: usize) {
        if std::env::var_os("DRAGENT_INDEX_DISABLE").is_some() {
            return;
        }
        // Canonicalize so the same folder can't register as two repos (the
        // Windows double-index bug).
        let root = normalize_root(&root);
        if user_home().is_some_and(|home| is_too_broad_to_index(&root, &home)) {
            log::info!("[index] not indexing {}: it holds the whole home folder", root.display());
            return;
        }
        if !self.is_enabled() {
            // Park the root so enabling indexing later picks it up without
            // waiting for the next cwd sync.
            let mut inner = self.inner.lock().unwrap();
            if !inner.disabled_roots.iter().any(|(r, _)| *r == root) {
                inner.disabled_roots.push((root, max_files));
            }
            return;
        }
        // Registration is the first real demand for embeddings. Loading here
        // (rather than at application launch) avoids competing with the UI,
        // auth, fonts, and persistence initialization.
        self.init_embedder_background();
        {
            let mut inner = self.inner.lock().unwrap();
            let covered = inner
                .repos
                .keys()
                .chain(inner.registering.iter())
                .any(|existing| root.starts_with(existing));
            if covered {
                for (existing, handle) in &inner.repos {
                    if root.starts_with(existing) {
                        handle.mark_used();
                    }
                }
                return;
            }
            // Descendants are subsumed by this broader root: stop their watchers,
            // drop them, and delete their stores. Every file they hold is
            // re-embedded under `root`, so leaving the store behind just strands
            // a full duplicate copy of the index on disk that nothing will ever
            // open again (the artifacts are a pure cache — a later re-register
            // rebuilds them).
            let children: Vec<PathBuf> = inner
                .repos
                .keys()
                .filter(|r| r.starts_with(&root))
                .cloned()
                .collect();
            for child in children {
                if let Some(handle) = inner.repos.remove(&child) {
                    handle.remove_files.store(true, Ordering::SeqCst);
                    handle.stop.store(true, Ordering::SeqCst);
                    log::info!(
                        "[index] {} subsumed by ancestor {} — stopping its watcher",
                        child.display(),
                        root.display()
                    );
                }
            }
            inner.registering.insert(root.clone());
        }
        std::thread::spawn(move || {
            set_background_qos();
            if let Err(e) = self.index_and_watch(root.clone(), max_files) {
                log::warn!("[index] setup failed: {e}");
            }
            // Successful setup left the root in `repos`, so future calls still
            // no-op; on failure this allows a retry.
            self.inner.lock().unwrap().registering.remove(&root);
        });
    }

    /// Lock-free status of every indexed repo (registrations still in their
    /// initial pass are reported as `Indexing`). Sorted by root for stable UI.
    pub fn index_status(&self) -> Vec<RepoIndexStatus> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<RepoIndexStatus> = inner
            .repos
            .iter()
            .map(|(root, handle)| RepoIndexStatus {
                root: root.clone(),
                state: handle.activity.state(),
                pending: handle.activity.pending.load(Ordering::Relaxed),
                files: handle.activity.files.load(Ordering::Relaxed),
                chunks: handle.activity.chunks.load(Ordering::Relaxed),
                capped: handle.activity.capped.load(Ordering::Relaxed),
                synced_secs_ago: handle.activity.synced_secs_ago(),
            })
            .collect();
        for root in &inner.registering {
            out.push(RepoIndexStatus {
                root: root.clone(),
                state: RepoIndexState::Indexing,
                pending: 0,
                files: 0,
                chunks: 0,
                capped: false,
                synced_secs_ago: None,
            });
        }
        out.sort_by(|a, b| a.root.cmp(&b.root));
        out
    }

    /// Look up the registered repo. Repos are registered under the git project
    /// root, but callers often pass a subdirectory (the terminal cwd). Fall
    /// back to the deepest registered root that contains `repo_root` so a
    /// call from a subdir still hits the right index. Returns the resolved
    /// root alongside its state since callers need it to interpret the
    /// relative paths stored under that root.
    pub(crate) fn resolve_repo_state(
        &self,
        repo_root: &Path,
    ) -> Option<(PathBuf, Arc<Mutex<RepoState>>)> {
        let inner = self.inner.lock().unwrap();
        let (root, handle) = inner.repos.get_key_value(repo_root).or_else(|| {
            inner
                .repos
                .iter()
                .filter(|(root, _)| repo_root.starts_with(root.as_path()))
                .max_by_key(|(root, _)| root.as_os_str().len())
        })?;
        handle.mark_used();
        Some((root.clone(), handle.state.clone()))
    }

    /// Stop watching repositories nobody has searched for `idle`, and let go of
    /// what they hold in memory: the file tree, the keyword index, the watcher.
    /// Everything is on disk, so the next search picks up where it left off and
    /// embeds only what changed in between.
    pub(crate) fn release_idle_repos(&self, idle: std::time::Duration) -> usize {
        let cutoff = crate::manager::epoch_secs().saturating_sub(idle.as_secs());
        let mut inner = self.inner.lock().unwrap();
        let idle_roots: Vec<PathBuf> = inner
            .repos
            .iter()
            .filter(|(_, handle)| {
                is_idle(
                    handle.last_used.load(Ordering::Relaxed),
                    cutoff,
                    handle.activity.state(),
                )
            })
            .map(|(root, _)| root.clone())
            .collect();
        for root in &idle_roots {
            if let Some(handle) = inner.repos.remove(root) {
                handle.stop.store(true, Ordering::SeqCst);
                log::info!("[index] released {}: not searched recently", root.display());
            }
        }
        idle_roots.len()
    }

    /// All indexed file paths (relative to the returned root) for the repo
    /// containing `repo_root` — `None` if no repo covering it is registered.
    pub fn list_indexed_paths(&self, repo_root: &Path) -> Option<(PathBuf, Vec<PathBuf>)> {
        let (root, state) = self.resolve_repo_state(repo_root)?;
        let paths = state
            .lock()
            .unwrap()
            .tree
            .fragments
            .keys()
            .cloned()
            .collect();
        Some((root, paths))
    }

    pub fn is_ready(&self) -> bool {
        // Readiness means a searchable repository is registered. The model may
        // have been idle-evicted; `search` transparently reacquires it. Exposing
        // model residency here caused callers to skip semantic retrieval forever
        // after the first idle eviction.
        self.inner
            .lock()
            .map(|inner| !inner.repos.is_empty())
            .unwrap_or(false)
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(|home| normalize_root(Path::new(&home)))
}

/// A `.git` in the home folder (dotfile setups) makes git report home as the
/// repository of every folder under it that isn't its own repository. Indexing
/// that embeds app data, caches and other projects, up to the file budget.
fn is_too_broad_to_index(root: &Path, home: &Path) -> bool {
    home.starts_with(root)
}

/// A repository still in its first pass stays: stopping it wastes that work.
fn is_idle(last_used: u64, cutoff: u64, state: RepoIndexState) -> bool {
    last_used < cutoff && state != RepoIndexState::Indexing
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn the_home_folder_and_above_are_never_indexed() {
        let home = Path::new("/Users/someone");
        assert!(is_too_broad_to_index(Path::new("/Users/someone"), home));
        assert!(is_too_broad_to_index(Path::new("/Users"), home));
        assert!(is_too_broad_to_index(Path::new("/"), home));
        assert!(!is_too_broad_to_index(Path::new("/Users/someone/dev/app"), home));
        assert!(!is_too_broad_to_index(Path::new("/Users/someone-else"), home));
    }

    #[test]
    fn only_finished_repositories_can_go_idle() {
        assert!(is_idle(10, 20, RepoIndexState::Watching));
        assert!(!is_idle(30, 20, RepoIndexState::Watching));
        assert!(!is_idle(10, 20, RepoIndexState::Indexing));
    }
}
