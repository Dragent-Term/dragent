use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};

use dragent_resource_lifecycle::LazyResource;

use super::embedder::Embedder;
use super::inverted::InvertedIndex;
use super::merkle::MerkleTree;
use super::store::VectorStore;

// ── Thread priority ───────────────────────────────────────────────────────────

/// Drop the calling thread to background (Utility) scheduling so indexing —
/// walking, chunking, and especially ONNX embedding — never competes with the
/// UI thread for cores. On Apple platforms the ONNX Runtime's intra-op pool
/// inherits the QoS of the thread that creates the session, so calling this
/// before `Embedder::load` covers inference threads too. The terminal must
/// never be the reason the app stutters.
pub(crate) fn set_background_qos() {
    #[cfg(target_vendor = "apple")]
    {
        // QOS_CLASS_UTILITY = 0x11 (long-running user-visible-ish work).
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        unsafe {
            pthread_set_qos_class_self_np(0x11, 0);
        }
    }
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Per-thread nice: on Linux, setpriority with a TID (not 0) adjusts
        // only the calling thread. Positive nice = lower priority. Works on
        // x86_64 and aarch64 alike via libc's gettid wrapper.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
        }
    }
}

// ── Global static ─────────────────────────────────────────────────────────────

/// Default per-repo file budget, used when a parked/unknown root has to be
/// re-registered without its original budget.
pub const DEFAULT_MAX_FILES: usize = 10_000;

pub static CODEBASE_INDEX_MANAGER: OnceLock<CodeIndexManager> = OnceLock::new();

pub fn get_manager() -> &'static CodeIndexManager {
    CODEBASE_INDEX_MANAGER.get_or_init(CodeIndexManager::new)
}

// ── Per-repo state ─────────────────────────────────────────────────────────────

pub(crate) struct RepoState {
    pub(crate) root: PathBuf,
    pub(crate) tree: MerkleTree,
    pub(crate) store: VectorStore,
    pub(crate) snapshot_path: PathBuf,
    /// Lazily-built BM25 over the current tree, keyed by `tree.root_hash`.
    /// Memory-only: rebuilt when the hash changes, never persisted.
    pub(crate) bm25_cache: Option<(String, Arc<InvertedIndex>)>,
    /// Last time the snapshot was persisted — watcher events throttle saves
    /// through `maybe_save_snapshot` instead of rewriting the file per event.
    pub(crate) last_snapshot_save: std::time::Instant,
}

/// Lifecycle of a repo's index, surfaced to the UI via [`CodeIndexManager::index_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoIndexState {
    /// Initial walk/embed pass running.
    Indexing,
    /// Watching for changes; index is current.
    Watching,
    /// Circuit breaker open: sustained churn exceeded the watcher's duty-cycle
    /// budget, so indexing is paused until the repo goes quiet, then a full
    /// rescan reconciles. The terminal must never be the reason a build is slow.
    Paused,
}

/// Lock-free view of one repo's indexing activity. The watcher thread updates
/// these atomics; status reads never touch the repo mutex, so the UI can poll
/// at frame rate even mid-embedding.
pub struct RepoActivity {
    pub(crate) state: AtomicU8,
    pub(crate) pending: AtomicUsize,
    pub(crate) files: AtomicUsize,
    pub(crate) chunks: AtomicUsize,
    pub(crate) capped: AtomicBool,
    /// Unix epoch seconds of the last completed sync (initial pass, processed
    /// batch, or rescan). 0 = never synced.
    pub(crate) last_synced_epoch: AtomicU64,
}

impl RepoActivity {
    pub(crate) fn new(state: RepoIndexState) -> Self {
        Self {
            state: AtomicU8::new(state as u8),
            pending: AtomicUsize::new(0),
            files: AtomicUsize::new(0),
            chunks: AtomicUsize::new(0),
            capped: AtomicBool::new(false),
            last_synced_epoch: AtomicU64::new(0),
        }
    }

    pub(crate) fn touch_synced(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.last_synced_epoch.store(now, Ordering::Relaxed);
    }

    pub(crate) fn synced_secs_ago(&self) -> Option<u64> {
        let then = self.last_synced_epoch.load(Ordering::Relaxed);
        if then == 0 {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Some(now.saturating_sub(then))
    }

    pub(crate) fn set_state(&self, state: RepoIndexState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    pub(crate) fn state(&self) -> RepoIndexState {
        match self.state.load(Ordering::Relaxed) {
            0 => RepoIndexState::Indexing,
            2 => RepoIndexState::Paused,
            _ => RepoIndexState::Watching,
        }
    }

    pub(crate) fn set_counts(&self, files: usize, chunks: usize) {
        self.files.store(files, Ordering::Relaxed);
        self.chunks.store(chunks, Ordering::Relaxed);
    }
}

/// Point-in-time status of one indexed repo.
#[derive(Debug, Clone)]
pub struct RepoIndexStatus {
    pub root: PathBuf,
    pub state: RepoIndexState,
    /// Coalesced changes waiting (or discarded-pending-rescan while paused).
    pub pending: usize,
    pub files: usize,
    pub chunks: usize,
    /// True once `max_files` stopped new files from being added.
    pub capped: bool,
    /// Seconds since the last completed sync; `None` = initial pass not done.
    pub synced_secs_ago: Option<u64>,
}

/// Everything the manager holds per registered repo. `stop` lets a nested-root
/// dedup (or future unregister) terminate the watcher thread; it checks the
/// flag every tick of its receive loop.
pub(crate) struct RepoHandle {
    pub(crate) state: Arc<Mutex<RepoState>>,
    pub(crate) activity: Arc<RepoActivity>,
    pub(crate) stop: Arc<AtomicBool>,
    /// Raised (before `stop`) by `remove_index`: the watcher must delete the
    /// on-disk store + snapshot on exit. The watcher owns the deletion because
    /// it can first close the SQLite handle (Windows can't delete an open
    /// file) and because a batch in flight when `stop` is raised would
    /// otherwise rewrite the files after an external delete.
    pub(crate) remove_files: Arc<AtomicBool>,
    /// Completion signal from the watcher. Reindex waits for this off the UI
    /// thread before touching the shared DB and snapshot paths.
    pub(crate) watcher_done: Receiver<()>,
    /// The budget this repo was registered with — reused by force re-index.
    pub(crate) max_files: usize,
    /// Unix seconds of the last search or registration, for idle release.
    pub(crate) last_used: AtomicU64,
}

impl RepoHandle {
    pub(crate) fn mark_used(&self) {
        self.last_used.store(epoch_secs(), Ordering::Relaxed);
    }
}

pub(crate) fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub struct CodeIndexManager {
    pub(crate) inner: Mutex<Inner>,
    /// The ONNX session is expensive (~130 MB before runtime arenas). Leases
    /// keep it alive only while indexing/search is active; the reaper can drop
    /// it after an idle period without racing an operation in flight.
    pub(crate) embedder: LazyResource<Embedder>,
    pub(crate) model_reaper_started: AtomicBool,
    /// Master switch (Settings → Code and Index → Enable codebase indexing).
    pub(crate) enabled: AtomicBool,
    /// Watcher-driven re-indexing (Settings → Auto-index on changes). When
    /// off, watcher events only accumulate a pending count; Re-index or
    /// re-enabling reconciles.
    pub(crate) auto_index: AtomicBool,
}

pub(crate) struct Inner {
    pub(crate) repos: HashMap<PathBuf, RepoHandle>,
    /// Roots with a registration in flight (walk/embed still running). Guards
    /// against `register_repo` being called on every cwd sync — without it each
    /// `cd` stacked another watcher thread on the same root, multiplying every
    /// file event's embedding work.
    pub(crate) registering: HashSet<PathBuf>,
    /// Roots (with their max_files budget) whose watchers were stopped when
    /// indexing was disabled — re-registered when it is enabled again.
    pub(crate) disabled_roots: Vec<(PathBuf, usize)>,
}

impl super::manager::CodeIndexManager {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                repos: HashMap::new(),
                registering: HashSet::new(),
                disabled_roots: Vec::new(),
            }),
            embedder: LazyResource::new(),
            model_reaper_started: AtomicBool::new(false),
            enabled: AtomicBool::new(true),
            auto_index: AtomicBool::new(true),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Master switch. Disabling stops every watcher and remembers the roots;
    /// enabling re-registers them (incremental — the snapshots survive).
    pub fn set_enabled(&'static self, on: bool) {
        let was = self.enabled.swap(on, Ordering::Relaxed);
        if was == on {
            return;
        }
        if !on {
            let mut inner = self.inner.lock().unwrap();
            let mut parked: Vec<(PathBuf, usize)> = Vec::new();
            for (root, handle) in inner.repos.drain() {
                handle.stop.store(true, Ordering::Relaxed);
                parked.push((root, handle.max_files));
            }
            // In-flight registrations bail at their next enabled() check;
            // remember their roots too so enable() restores them.
            for root in inner.registering.drain() {
                parked.push((root, DEFAULT_MAX_FILES));
            }
            inner.disabled_roots.extend(parked);
            if self.embedder.clear_if_unused() {
                log::info!("[index] released embedding model while indexing is disabled");
            }
            log::info!("[index] indexing disabled — all watchers stopped");
        } else {
            let parked: Vec<(PathBuf, usize)> = {
                let mut inner = self.inner.lock().unwrap();
                inner.disabled_roots.drain(..).collect()
            };
            for (root, max_files) in parked {
                self.register_repo(root, max_files);
            }
            log::info!("[index] indexing enabled");
        }
    }

    pub fn auto_index(&self) -> bool {
        self.auto_index.load(Ordering::Relaxed)
    }

    pub fn set_auto_index(&self, on: bool) {
        self.auto_index.store(on, Ordering::Relaxed);
    }

    /// Whether walks honor `.gitignore`. Takes effect on the next (re)index;
    /// the settings UI triggers `reindex_all` after flipping it.
    pub fn set_respect_gitignore(&self, respect: bool) {
        super::walk::set_respect_gitignore(respect);
    }
}
mod tests {
    use crate::index_batch::ensure_store_matches_tree;
    use crate::index_watcher::{ChangeBatch, DutyCycleBreaker, is_watch_ignored};
    use crate::{FileTreeWalker, VectorStore};
    use ignore::gitignore::GitignoreBuilder;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    #[test]
    pub(crate) fn watch_ignores_generated_dirs_at_any_depth() {
        assert!(is_watch_ignored(
            None,
            Path::new("node_modules/next/dist/index.js")
        ));
        assert!(is_watch_ignored(
            None,
            Path::new("packages/app/node_modules/x/y.js")
        ));
        assert!(is_watch_ignored(
            None,
            Path::new("target/debug/build/foo.rs")
        ));
        assert!(is_watch_ignored(None, Path::new(".git/index.lock")));
        assert!(is_watch_ignored(None, Path::new(".next/server/page.js")));
        assert!(!is_watch_ignored(
            None,
            Path::new("src/components/button.tsx")
        ));
        // "build"/"dist" match as components, not substrings.
        assert!(!is_watch_ignored(None, Path::new("src/builder.rs")));
        assert!(!is_watch_ignored(None, Path::new("src/distance.rs")));
    }

    #[test]
    pub(crate) fn watch_ignores_gitignored_paths() {
        let mut builder = GitignoreBuilder::new("/repo");
        builder.add_line(None, "*.log").unwrap();
        builder.add_line(None, "generated/").unwrap();
        let matcher = builder.build().unwrap();

        assert!(is_watch_ignored(Some(&matcher), Path::new("debug.log")));
        assert!(is_watch_ignored(
            Some(&matcher),
            Path::new("generated/api.ts")
        ));
        assert!(!is_watch_ignored(Some(&matcher), Path::new("src/main.rs")));
    }

    #[test]
    pub(crate) fn breaker_opens_on_sustained_busy_and_resets_after_window() {
        let base = Instant::now();
        let mut b = DutyCycleBreaker {
            window_start: base,
            busy: Duration::ZERO,
        };
        // Light work never trips it.
        assert!(!b.note_busy(Duration::from_secs(1), base + Duration::from_secs(5)));
        assert!(!b.note_busy(Duration::from_secs(1), base + Duration::from_secs(10)));
        // Sustained heavy work inside the window trips it.
        assert!(b.note_busy(Duration::from_secs(30), base + Duration::from_secs(30)));
        // A new window forgets old debt.
        assert!(!b.note_busy(
            Duration::from_secs(1),
            base + DutyCycleBreaker::WINDOW + Duration::from_secs(31)
        ));
    }

    #[test]
    pub(crate) fn dragentignore_extends_watch_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".dragentignore"), "secret/\n*.pem\n").unwrap();

        let mut builder = GitignoreBuilder::new(dir.path());
        for src in crate::walk::ignore_sources(dir.path()) {
            let _ = builder.add(src);
        }
        let matcher = builder.build().unwrap();

        assert!(is_watch_ignored(
            Some(&matcher),
            Path::new("secret/keys.rs")
        ));
        assert!(is_watch_ignored(
            Some(&matcher),
            Path::new("certs/server.pem")
        ));
        assert!(!is_watch_ignored(Some(&matcher), Path::new("src/main.rs")));
    }

    #[test]
    pub(crate) fn change_batch_coalesces_newest_wins() {
        let mut batch = ChangeBatch::default();
        // Repeated modifies collapse to one entry.
        batch.record(PathBuf::from("a.rs"), false);
        batch.record(PathBuf::from("a.rs"), false);
        assert_eq!(batch.modified.len(), 1);

        // Remove supersedes earlier modifies.
        batch.record(PathBuf::from("a.rs"), true);
        assert!(batch.modified.is_empty());
        assert!(batch.removed.contains(Path::new("a.rs")));

        // Re-creation moves it back to modified.
        batch.record(PathBuf::from("a.rs"), false);
        assert!(batch.removed.is_empty());
        assert!(batch.modified.contains(Path::new("a.rs")));

        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
    }

    #[test]
    pub(crate) fn detects_snapshot_store_desynchronization() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();
        let tree = FileTreeWalker::build_tree(dir.path(), 10).unwrap();
        let store = VectorStore::open(&dir.path().join("index.db")).unwrap();
        let embedding = vec![1.0f32, 0.0, 0.0];

        let a = tree
            .fragments
            .get(Path::new("a.rs"))
            .unwrap()
            .first()
            .unwrap();
        store
            .upsert(
                &a.fragment_id,
                &dir.path().join("a.rs"),
                a.byte_start,
                a.byte_end,
                "fn a() {}\n",
                &embedding,
            )
            .unwrap();
        assert!(ensure_store_matches_tree(&store, dir.path(), &tree).is_err());

        let b = tree
            .fragments
            .get(Path::new("b.rs"))
            .unwrap()
            .first()
            .unwrap();
        store
            .upsert(
                &b.fragment_id,
                &dir.path().join("b.rs"),
                b.byte_start,
                b.byte_end,
                "fn b() {}\n",
                &embedding,
            )
            .unwrap();
        assert!(ensure_store_matches_tree(&store, dir.path(), &tree).is_ok());
    }
}
