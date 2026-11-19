use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify_debouncer_full::notify::event::ModifyKind;
use notify_debouncer_full::notify::{Config, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{NoCache, new_debouncer_opt};

use super::manager::set_background_qos;
use super::snapshot::IndexSnapshot;
use super::store::VectorStore;
use super::walk::FileTreeWalker;
use crate::CodeIndexManager;
use crate::RepoIndexState;
use crate::index_batch::db_path_for_repo;
use crate::index_batch::delete_index_artifacts;
use crate::index_batch::diff_trees;
use crate::index_batch::embed_files;
use crate::index_batch::ensure_store_matches_tree;
use crate::index_batch::expected_store_entries;
use crate::index_batch::full_rescan;
use crate::index_batch::process_batch;
use crate::index_batch::snapshot_path_for_repo;
use crate::manager::RepoActivity;
use crate::manager::RepoHandle;
use crate::manager::RepoState;

impl CodeIndexManager {
    pub(crate) fn index_and_watch(&'static self, root: PathBuf, max_files: usize) -> Result<()> {
        // Join the single in-flight load (or initialize on demand) and hold a
        // lease only for this initial pass.
        let embedder = self.acquire_embedder(Duration::from_secs(60));
        let Some(embedder) = embedder else {
            log::info!("[index] embedder unavailable — skipping {}", root.display());
            return Ok(());
        };
        if !self.is_enabled() {
            return Ok(());
        }

        // Open (or create) the vector store and load the previous snapshot.
        let snapshot_path = snapshot_path_for_repo(&root);
        let db_path = db_path_for_repo(&root);

        let store = VectorStore::open(&db_path)?;
        // If the DB was wiped for a schema/model change, the old snapshot is stale:
        // ignore it so every file is re-embedded into the fresh store. Otherwise a
        // "0 to embed" diff would leave the index permanently empty.
        let mut prev_tree = if store.schema_was_reset() {
            log::info!(
                "[index] schema changed — rebuilding index for {}",
                root.display()
            );
            None
        } else {
            IndexSnapshot::load(&snapshot_path).ok()
        };
        // A valid snapshot is not sufficient: a crash, interrupted rebuild, or
        // an older watcher racing a manual reindex can leave it describing rows
        // that are absent from the vector DB. Trusting that snapshot makes the
        // normal tree diff embed only later edits and leaves the rest of the
        // repository permanently unsearchable.
        if let Some(tree) = prev_tree.as_ref() {
            let expected = expected_store_entries(&root, tree);
            let actual = store.entries()?;
            if actual != expected {
                log::warn!(
                    "[index] snapshot/store mismatch for {} (snapshot={} rows, store={} rows) — rebuilding",
                    root.display(),
                    expected.len(),
                    actual.len()
                );
                store.clear()?;
                prev_tree = None;
            }
        }
        // No snapshot but a populated store = force re-index (snapshot deleted
        // by `reindex`). Clear so no stale chunks survive the rebuild.
        if prev_tree.is_none() && store.count() > 0 {
            log::info!(
                "[index] no snapshot for {} — clearing store for full rebuild",
                root.display()
            );
            let _ = store.clear();
        }

        // Walk the current file tree.
        let current_tree = FileTreeWalker::build_tree(&root, max_files)?;

        // Diff against the previous snapshot to find what actually changed.
        let (to_embed, to_remove) = diff_trees(prev_tree.as_ref(), &current_tree);

        log::info!(
            "[index] {} to embed, {} to remove for {}",
            to_embed.len(),
            to_remove.len(),
            root.display()
        );

        // Remove chunks for deleted files.
        for rel in &to_remove {
            store.remove_file(&root.join(rel))?;
        }

        // Embed only the changed/new files.
        embed_files(&embedder, &store, &root, &current_tree, &to_embed)?;

        ensure_store_matches_tree(&store, &root, &current_tree)?;

        // Persist the snapshot so the next launch skips unchanged files.
        IndexSnapshot::save(&current_tree, &snapshot_path)?;

        log::info!(
            "[index] initial pass done — {} chunks for {}",
            store.count(),
            root.display()
        );

        // Register repo state so searches work immediately.
        let files = current_tree.fragments.len();
        let repo_state = Arc::new(Mutex::new(RepoState {
            root: root.clone(),
            tree: current_tree,
            store,
            snapshot_path,
            bm25_cache: None,
            last_snapshot_save: std::time::Instant::now(),
        }));
        let activity = Arc::new(RepoActivity::new(RepoIndexState::Watching));
        activity.set_counts(files, repo_state.lock().unwrap().store.count());
        activity.touch_synced();
        let stop = Arc::new(AtomicBool::new(false));
        let remove_files = Arc::new(AtomicBool::new(false));
        let (watcher_done_tx, watcher_done) = std::sync::mpsc::channel();
        {
            let mut inner = self.inner.lock().unwrap();
            if !inner.registering.contains(&root) {
                // The index was deleted (remove_index) while this initial pass
                // was walking/embedding: don't register — discard the artifacts
                // the pass just wrote. Drop the state first so the SQLite
                // handle is closed before the delete (required on Windows).
                drop(inner);
                drop(repo_state);
                delete_index_artifacts(&root);
                log::info!(
                    "[index] {} deleted during initial pass — discarded",
                    root.display()
                );
                return Ok(());
            }
            if !self.is_enabled() {
                // Disabled while this registration's walk/embed was running:
                // don't start a watcher; park the root for a later enable.
                inner.disabled_roots.push((root.clone(), max_files));
                return Ok(());
            }
            inner.repos.insert(
                root.clone(),
                RepoHandle {
                    state: repo_state.clone(),
                    activity: activity.clone(),
                    stop: stop.clone(),
                    remove_files: remove_files.clone(),
                    watcher_done,
                    max_files,
                    last_used: std::sync::atomic::AtomicU64::new(crate::manager::epoch_secs()),
                },
            );
        }

        // Start the file watcher — runs until `stop` is raised or the app exits.
        if let Err(error) = self.start_watcher(
            root.clone(),
            max_files,
            repo_state,
            activity,
            stop,
            remove_files,
            watcher_done_tx,
        ) {
            self.inner.lock().unwrap().repos.remove(&root);
            return Err(error);
        }
        Ok(())
    }

    fn start_watcher(
        &'static self,
        root: PathBuf,
        max_files: usize,
        repo_state: Arc<Mutex<RepoState>>,
        activity: Arc<RepoActivity>,
        stop: Arc<AtomicBool>,
        remove_files: Arc<AtomicBool>,
        watcher_done: Sender<()>,
    ) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        // The default macOS cache (`FileIdMap`) recursively walks and retains
        // EVERY path below the watch root before the watcher starts. That scan
        // ignores our Git/.dragent filters and file budget, so a repo containing
        // large generated trees (`target`, `node_modules`, `.next`) can consume
        // tens of gigabytes even though none of those files are indexed. Rename
        // stitching by file ID is optional; the macOS backend still reports the
        // underlying create/remove/modify events, and our `ChangeBatch` plus
        // quiet-period full rescan reconcile them. Use `NoCache` so memory is
        // proportional to the debounced event burst, not total repository size.
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
            Duration::from_millis(500),
            None,
            tx,
            NoCache::new(),
            Config::default(),
        )?;
        debouncer.watch(&root, RecursiveMode::Recursive)?;

        // The initial walk excludes gitignored files, but notify reports raw
        // paths — without the same filter here, an `npm install` (or `cargo
        // build`) floods the watcher with tens of thousands of node_modules /
        // target events, each one synchronously embedded. Same ignore sources
        // as the walk (.gitignore, .dragentignore, user-global); a missing or
        // unparsable file degrades to the hardcoded component skip-list in
        // `is_watch_ignored`.
        let ignore_matcher = {
            let mut builder = GitignoreBuilder::new(&root);
            for ignore_file in super::walk::ignore_sources(&root) {
                let _ = builder.add(ignore_file);
            }
            builder.build().ok()
        };

        let mgr: &'static CodeIndexManager = self;
        std::thread::spawn(move || {
            set_background_qos();
            // Keep debouncer alive for the duration of this thread.
            let _debouncer = debouncer;

            // Coalesce → batch → embed. Blocking on the first batch, then
            // draining everything already queued means a burst (branch switch,
            // codegen, formatter sweep) collapses to one pass where each file
            // is chunked and embedded once, newest state wins — instead of one
            // synchronous embed per raw event.
            //
            // The duty-cycle breaker is the backstop the ignore filter can't
            // be: if legitimate (non-ignored) churn keeps the watcher busy
            // beyond its budget, indexing pauses entirely and a single full
            // rescan reconciles once the repo goes quiet.
            let mut breaker = DutyCycleBreaker::default();
            let mut paused = false;
            let mut last_event = Instant::now();
            loop {
                if stop.load(Ordering::SeqCst) {
                    log::info!("[index] watcher for {} stopped", root.display());
                    break;
                }
                match rx.recv_timeout(WATCHER_TICK) {
                    Ok(first) => {
                        // Disabling/removing a repo can race a debouncer event
                        // already being delivered. Do not let that final event
                        // reload the model after the watcher was stopped.
                        if stop.load(Ordering::SeqCst) || !mgr.is_enabled() {
                            break;
                        }
                        let mut batch = ChangeBatch::default();
                        accumulate_events(&mut batch, first, &root, ignore_matcher.as_ref());
                        while let Ok(more) = rx.try_recv() {
                            accumulate_events(&mut batch, more, &root, ignore_matcher.as_ref());
                        }
                        if batch.is_empty() {
                            continue;
                        }
                        last_event = Instant::now();
                        if paused || !mgr.auto_index() {
                            // Breaker open (or auto-index switched off in
                            // settings): count but don't process — the resume
                            // rescan (or a manual Re-index) reconciles at once.
                            activity.pending.fetch_add(batch.len(), Ordering::Relaxed);
                            continue;
                        }
                        activity.pending.store(batch.len(), Ordering::Relaxed);
                        let t0 = Instant::now();
                        let synced = match mgr.acquire_embedder(Duration::from_secs(60)) {
                            Some(embedder) => {
                                process_batch(&repo_state, &embedder, batch, max_files, &activity)
                            }
                            None => false,
                        };
                        activity
                            .pending
                            .store(if synced { 0 } else { 1 }, Ordering::Relaxed);
                        if synced {
                            activity.touch_synced();
                        }
                        if breaker.note_busy(t0.elapsed(), Instant::now()) {
                            paused = true;
                            activity.set_state(RepoIndexState::Paused);
                            log::warn!(
                                "[index] sustained churn — indexing paused for {} until quiet",
                                root.display()
                            );
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        // Two resume cases share the rescan: the duty-cycle
                        // breaker reopening after quiet, and auto-index being
                        // switched back on with deferred changes pending.
                        let deferred_pending = !paused
                            && mgr.auto_index()
                            && activity.pending.load(Ordering::Relaxed) > 0;
                        if (paused && mgr.auto_index() && last_event.elapsed() >= RESUME_QUIET)
                            || deferred_pending
                        {
                            log::info!("[index] reconciling {} with a full rescan", root.display());
                            let Some(embedder) = mgr.acquire_embedder(Duration::from_secs(60))
                            else {
                                activity.pending.store(1, Ordering::Relaxed);
                                activity.set_state(RepoIndexState::Paused);
                                paused = true;
                                last_event = Instant::now();
                                continue;
                            };
                            if let Err(e) = full_rescan(&repo_state, &embedder, max_files) {
                                log::warn!("[index] resume rescan failed: {e}");
                                activity.pending.store(1, Ordering::Relaxed);
                                activity.set_state(RepoIndexState::Paused);
                                paused = true;
                                last_event = Instant::now();
                                continue;
                            }
                            {
                                let state = repo_state.lock().unwrap();
                                activity
                                    .set_counts(state.tree.fragments.len(), state.store.count());
                            }
                            activity.pending.store(0, Ordering::Relaxed);
                            activity.touch_synced();
                            paused = false;
                            breaker = DutyCycleBreaker::default();
                            activity.set_state(RepoIndexState::Watching);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            // Exit cleanup: if this stop was a removal, delete the on-disk
            // artifacts HERE — after the last possible snapshot/DB write, with
            // the SQLite handle closed first so the delete sticks on Windows.
            if remove_files.load(Ordering::SeqCst) {
                match Arc::try_unwrap(repo_state) {
                    Ok(state) => drop(state),
                    // A concurrent search still holds a momentary clone; its
                    // open handle can make the DB delete fail on Windows. The
                    // stale file is harmless: the repo is unregistered, and a
                    // re-open clears the store on the way in.
                    Err(arc) => {
                        log::warn!(
                            "[index] state still shared while removing {} — delete may be partial",
                            root.display()
                        );
                        drop(arc);
                    }
                }
                delete_index_artifacts(&root);
                log::info!("[index] deleted on-disk index for {}", root.display());
            } else {
                // Close the retiring SQLite handle before signalling reindex;
                // this is required on Windows and removes the last DB race on
                // every platform.
                drop(repo_state);
            }
            let _ = watcher_done.send(());
        });

        Ok(())
    }
}

// ── Thread priority ───────────────────────────────────────────────────────────

/// Drop the calling thread to background (Utility) scheduling so indexing —
/// walking, chunking, and especially ONNX embedding — never competes with the
/// UI thread for cores. On Apple platforms the ONNX Runtime's intra-op pool
/// inherits the QoS of the thread that creates the session, so calling this
/// before `Embedder::load` covers inference threads too. The terminal must
/// never be the reason the app stutters.

/// A coalesced set of pending changes: each path appears at most once, in the
/// set matching its most recent event. A modify after a remove (file recreated)
/// moves it back to `modified`; a remove after modifies supersedes them.
#[derive(Default)]
pub(crate) struct ChangeBatch {
    pub(crate) modified: HashSet<PathBuf>,
    pub(crate) removed: HashSet<PathBuf>,
}

impl ChangeBatch {
    pub(crate) fn record(&mut self, rel: PathBuf, is_remove: bool) {
        if is_remove {
            self.modified.remove(&rel);
            self.removed.insert(rel);
        } else {
            self.removed.remove(&rel);
            self.modified.insert(rel);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.removed.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.modified.len() + self.removed.len()
    }
}

/// Fold one debouncer result into the batch, dropping directories, paths
/// outside the root, and anything the ignore filter rejects.
fn accumulate_events(
    batch: &mut ChangeBatch,
    result: std::result::Result<
        Vec<notify_debouncer_full::DebouncedEvent>,
        Vec<notify_debouncer_full::notify::Error>,
    >,
    root: &Path,
    matcher: Option<&Gitignore>,
) {
    let events = match result {
        Ok(events) => events,
        Err(errors) => {
            for e in errors {
                log::warn!("[index] watcher error: {e:?}");
            }
            return;
        }
    };
    for event in events {
        let kind = &event.event.kind;
        let is_modify = matches!(
            kind,
            EventKind::Create(_)
                | EventKind::Modify(ModifyKind::Data(_))
                | EventKind::Modify(ModifyKind::Any)
        );
        let is_remove = matches!(kind, EventKind::Remove(_));
        if !is_modify && !is_remove {
            continue;
        }

        for abs_path in &event.event.paths {
            // Skip non-source files (directories, hidden, etc.)
            if abs_path.is_dir() {
                continue;
            }
            let rel = match abs_path.strip_prefix(root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => continue,
            };
            // Skip generated/ignored paths (node_modules, target,
            // .git internals, anything gitignored).
            if is_watch_ignored(matcher, &rel) {
                continue;
            }
            batch.record(rel, is_remove);
        }
    }
}

/// Above this many coalesced changes, a walk+diff rebuild is cheaper and more
/// correct than touching files one by one (and the walk re-applies every
/// exclusion rule, so a filter gap can't amplify into per-file work).
pub(crate) const FULL_RESCAN_THRESHOLD: usize = 2_000;

/// Watcher receive-loop tick — bounds how fast stop/resume conditions are noticed.
const WATCHER_TICK: Duration = Duration::from_secs(1);

/// Quiet period after the last event before a paused watcher resumes.
const RESUME_QUIET: Duration = Duration::from_secs(10);

/// Duty-cycle circuit breaker: if batch processing consumes more than
/// `MAX_BUSY` of wall time inside any `WINDOW`, the watcher pauses. This is
/// budget enforcement, not tuning — no matter what slips past the ignore
/// filter, background indexing cannot monopolise the machine the way the
/// original unfiltered watcher did (5 cores / 7.4 GB during one npm install).
#[derive(Debug)]
pub(crate) struct DutyCycleBreaker {
    pub(crate) window_start: Instant,
    pub(crate) busy: Duration,
}

impl DutyCycleBreaker {
    pub(crate) const WINDOW: Duration = Duration::from_secs(60);
    const MAX_BUSY: Duration = Duration::from_secs(20);

    /// Record `work` done at `now`; returns true when the budget is blown.
    pub(crate) fn note_busy(&mut self, work: Duration, now: Instant) -> bool {
        if now.duration_since(self.window_start) > Self::WINDOW {
            self.window_start = now;
            self.busy = Duration::ZERO;
        }
        self.busy += work;
        self.busy >= Self::MAX_BUSY
    }
}

impl Default for DutyCycleBreaker {
    fn default() -> Self {
        Self {
            window_start: Instant::now(),
            busy: Duration::ZERO,
        }
    }
}

/// Breather between embedding batches. Apple inherits our Utility QoS into
/// the ort pool and Linux inherits nice, but Windows thread priority is NOT
/// inherited — pool threads run at normal priority there. This yield gives
/// the UI thread guaranteed scheduling slack on every OS (~172 batches on a
/// 1k-file repo = under 2 s of added wall-clock).
pub(crate) const EMBED_BATCH_BREATHER: Duration = Duration::from_millis(10);

/// Chunk texts per inference call — mirrors `embed_files`' batching.
/// Deliberately small: transformer activation memory scales with
/// batch × seq_len², and ONNX Runtime's arena allocator retains the peak for
/// the process lifetime. Batch 32 drove RSS to 2.6 GB on the initial pass of
/// a 1k-file repo (system-wide memory pressure = perceived app freeze);
/// batch 8 keeps the arena in the low hundreds of MB at ~identical
/// throughput on CPU.
pub(crate) const EMBED_BATCH: usize = 8;

pub(crate) const MODEL_REAPER_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) const DEFAULT_MODEL_IDLE_SECS: u64 = 5 * 60;
pub(crate) const DEFAULT_REPO_IDLE_SECS: u64 = 30 * 60;

// ── File event handlers ────────────────────────────────────────────────────────

/// Directory names that are always machine-generated churn — never worth
/// indexing regardless of what .gitignore says. Checked against every path
/// component so nested occurrences (e.g. `pkg/node_modules/...`) match too.
const SKIP_DIR_COMPONENTS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    ".next",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".cache",
    ".turbo",
    "coverage",
];

/// Whether a watcher event for `rel` (relative to the watch root) should be
/// dropped: inside a known generated directory, or matched by the repo's
/// .gitignore. `is_dir=false` is safe — directories are filtered out before
/// this runs, and removed paths no longer exist to ask.
pub(crate) fn is_watch_ignored(matcher: Option<&Gitignore>, rel: &Path) -> bool {
    for comp in rel.components() {
        if let std::path::Component::Normal(name) = comp {
            if name
                .to_str()
                .is_some_and(|n| SKIP_DIR_COMPONENTS.contains(&n))
            {
                return true;
            }
        }
    }
    matcher.is_some_and(|m| m.matched_path_or_any_parents(rel, false).is_ignore())
}

/// Persist the snapshot at most every few seconds. Watcher bursts (branch
/// switches, builds) otherwise rewrite the whole snapshot file once per event;
/// a lost tail is self-healing — the next launch's tree diff re-embeds it.
pub(crate) fn maybe_save_snapshot(state: &mut RepoState) {
    if state.last_snapshot_save.elapsed() >= Duration::from_secs(5) {
        let _ = IndexSnapshot::save(&state.tree, &state.snapshot_path);
        state.last_snapshot_save = std::time::Instant::now();
    }
}
