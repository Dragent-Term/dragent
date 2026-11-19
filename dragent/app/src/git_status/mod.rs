//! Layer 6a — Git status & diff-vs-HEAD module.
//!
//! `GitRepoStatusModel` replaces the older `GitWatcher` with throttled refresh,
//! richer metadata (line stats, main-branch detection), and the standard
//! `GitStatusMetadata` type shared across crates.

#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
pub mod commit_message;
pub mod diff_head;
pub mod hunk_stage;
pub mod repo_model;
pub mod worktree;

pub use repo_model::{GitOperation, GitRepoStatusEvent, GitRepoStatusModel};

/// The git work-tree root containing `cwd`, or `None` outside a repository.
pub fn git_toplevel(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    use crate::process_ext::NoWindow;
    crate::git_cmd::GitCommand::new()
        .no_window()
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| std::path::PathBuf::from(s.trim()))
}

/// The work-tree root for `cwd` (falls back to `cwd` outside a repo).
///
/// Every path we hold — `git status --porcelain`, `git diff --numstat` — is
/// reported relative to the repo ROOT, but git resolves a pathspec against the
/// process's current directory. So an index command run from the terminal's cwd
/// silently misses the file whenever that cwd is a subdirectory: `git add --
/// sub/deep/f.txt` from `sub/` looks for `sub/sub/deep/f.txt` and fails. Index
/// commands must therefore run from the root.
pub fn repo_root(cwd: &std::path::Path) -> std::path::PathBuf {
    git_toplevel(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

// `GitStatusMetadata` is a type alias for the existing `GitStatus` struct so
// that `input_bar`, `ai_toolbar`, and other callers that reference
// `crate::console::git_status::GitStatus` continue to compile without changes.
pub type GitStatusMetadata = crate::console::git_status::GitStatus;

#[cfg(test)]
mod tests {
    use super::GitStatusMetadata;

    #[test]
    fn metadata_has_changes_reflects_counts() {
        let m = GitStatusMetadata {
            changed_files: 1,
            ..Default::default()
        };
        assert!(m.has_changes());
    }

    #[test]
    fn metadata_no_changes_when_clean() {
        let m = GitStatusMetadata::default();
        assert!(!m.has_changes());
    }

    use super::is_status_relevant;
    use std::path::Path;

    #[test]
    fn worktree_source_edits_are_relevant() {
        assert!(is_status_relevant(Path::new("/repo/src/main.rs")));
        assert!(is_status_relevant(Path::new("/repo/README.md")));
    }

    #[test]
    fn build_output_churn_is_ignored() {
        // A `cargo build` writing into target/ must not trigger a git status.
        assert!(!is_status_relevant(Path::new(
            "/repo/target/debug/deps/foo.rlib"
        )));
        assert!(!is_status_relevant(Path::new(
            "/repo/node_modules/react/index.js"
        )));
        assert!(!is_status_relevant(Path::new("/repo/ui/dist/bundle.js")));
        assert!(!is_status_relevant(Path::new("/repo/.next/cache/x")));
    }

    #[test]
    fn only_meaningful_git_internals_are_relevant() {
        assert!(is_status_relevant(Path::new("/repo/.git/HEAD")));
        assert!(is_status_relevant(Path::new("/repo/.git/index")));
        assert!(is_status_relevant(Path::new("/repo/.git/refs/heads/main")));
        // Object/lock churn (including what our own refresh writes) would loop.
        assert!(!is_status_relevant(Path::new("/repo/.git/objects/ab/cdef")));
        assert!(!is_status_relevant(Path::new("/repo/.git/index.lock")));
    }

    /// Windows-path coverage for the same rule. `Path` parsing is
    /// platform-defined — backslash separators only split on Windows —
    /// so these assertions live here, not in the portable test above
    /// (which would fail on Unix where the whole string is one component).
    #[cfg(windows)]
    #[test]
    fn windows_git_internals_follow_the_same_rule() {
        assert!(!is_status_relevant(Path::new(
            r"C:\repo\.git\objects\ab\cdef"
        )));
        assert!(!is_status_relevant(Path::new(r"C:\repo\.git\index.lock")));
        assert!(is_status_relevant(Path::new(r"C:\repo\.git\HEAD")));
    }

    #[test]
    fn a_file_merely_named_like_a_build_dir_is_still_relevant() {
        // "target" as a *file* name component still matches; but a source file
        // named e.g. build.rs must not be filtered.
        assert!(is_status_relevant(Path::new("/repo/build.rs")));
        assert!(is_status_relevant(Path::new("/repo/src/targets.rs")));
    }
}

// ── Real-time file watcher ────────────────────────────────────────────────────

/// Directory names whose contents can never change `git status` (they are
/// gitignored build/dependency output in every project we watch) but which
/// churn violently during a build. `notify` watches the git toplevel
/// recursively, so without this filter a single `cargo build` (target/) or
/// `npm run dev` (node_modules/, dist/) produces a continuous event stream and
/// we re-run `git status` forever.
const IGNORED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".venv",
    "__pycache__",
    "coverage",
    ".pytest_cache",
    ".gradle",
    ".idea",
];

/// True when a changed path could actually alter `git status` output.
///
/// Two rejections: churn inside `.git` that is not HEAD/index/refs (object and
/// lock traffic, including what our own refresh writes — that would loop), and
/// anything under a known build/dependency directory.
fn is_status_relevant(path: &std::path::Path) -> bool {
    // Use components rather than slash-delimited strings: Windows watcher
    // paths contain backslashes, and missing `.git` there creates a feedback
    // loop as our own Git subprocess activity triggers another refresh.
    let components: Vec<_> = path.components().collect();
    if let Some(git_index) = components.iter().position(|component| {
        matches!(component, std::path::Component::Normal(name) if *name == std::ffi::OsStr::new(".git"))
    }) {
        let inner = &components[git_index + 1..];
        return matches!(inner.first(), Some(std::path::Component::Normal(name)) if *name == std::ffi::OsStr::new("HEAD") || *name == std::ffi::OsStr::new("index"))
            || matches!(inner.first(), Some(std::path::Component::Normal(name)) if *name == std::ffi::OsStr::new("refs"));
    }
    // Anywhere under an ignored build dir → irrelevant.
    !path.components().any(|c| {
        matches!(c, std::path::Component::Normal(name)
            if IGNORED_DIRS.iter().any(|d| name == std::ffi::OsStr::new(d)))
    })
}

/// Wasm: no filesystem watcher or git subprocess in the viewer. Returns an inert
/// stop flag so callers compile; git status comes over the session stream.
#[cfg(target_family = "wasm")]
pub fn spawn_git_file_watcher(
    _cwd: std::path::PathBuf,
    _tx: async_channel::Sender<Vec<std::path::PathBuf>>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

/// Watch a repo's worktree (resolved to the git toplevel, so `.git/HEAD`
/// branch switches are seen even when the pane sits in a subdirectory) and
/// send the relevant changed paths per debounced batch. The receiver scopes
/// its `git status` to those paths (incremental refresh) instead
/// of rescanning the whole repo. An empty vec means "scope unknown — fetch
/// everything". Returns a stop flag; setting it terminates the watcher
/// thread on its next tick.
#[cfg(not(target_family = "wasm"))]
pub fn spawn_git_file_watcher(
    cwd: std::path::PathBuf,
    tx: async_channel::Sender<Vec<std::path::PathBuf>>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use crate::process_ext::NoWindow;
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    std::thread::Builder::new()
        .name("git-file-watcher".into())
        .spawn(move || {
            let Some(toplevel) = crate::git_cmd::GitCommand::new()
                .no_window()
                .arg("-C")
                .arg(&cwd)
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| std::path::PathBuf::from(s.trim()))
            else {
                // A terminal may legitimately start in a non-repository path
                // (commonly the user's home directory). Watching that path as a
                // fallback recursively observes unrelated applications and can
                // create an endless Git-status refresh storm. There is no Git
                // state to track here, so leave the watcher inert.
                log::debug!("[git-watch] {} is not a git worktree", cwd.display());
                return;
            };

            let (raw_tx, raw_rx) = std::sync::mpsc::channel();
            // `notify-debouncer-full` defaults to a `FileIdMap` cache on macOS.
            // Registering a recursive watch then walks and retains every path in
            // the worktree before the watcher starts, including ignored build
            // trees such as `target` and `node_modules`. Git status only needs a
            // signal that something changed; rename stitching by file ID adds no
            // value here, so avoid memory proportional to repository size.
            let mut debouncer = match notify_debouncer_full::new_debouncer_opt::<
                _,
                notify_debouncer_full::notify::RecommendedWatcher,
                notify_debouncer_full::NoCache,
            >(
                std::time::Duration::from_millis(600),
                None,
                raw_tx,
                notify_debouncer_full::NoCache::new(),
                notify_debouncer_full::notify::Config::default(),
            ) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!(
                        "[git-watch] debouncer failed for {}: {e}",
                        toplevel.display()
                    );
                    return;
                }
            };
            if let Err(e) = debouncer.watch(
                &toplevel,
                notify_debouncer_full::notify::RecursiveMode::Recursive,
            ) {
                log::warn!("[git-watch] watch failed for {}: {e}", toplevel.display());
                return;
            }
            log::info!("[git-watch] watching {}", toplevel.display());
            // Periodic quiet fetch so the behind-count (↓ chip) tracks the
            // remote without anyone running `git fetch` by hand. First fetch
            // soon after arming, then every 5 minutes; failures (offline,
            // no remote) are silent and retried on the next tick.
            const FETCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);
            let mut next_fetch = web_time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if stop_t.load(Ordering::Relaxed) {
                    return;
                }
                if web_time::Instant::now() >= next_fetch {
                    next_fetch = web_time::Instant::now() + FETCH_INTERVAL;
                    let fetched = crate::git_cmd::GitCommand::new()
                        .no_window()
                        .arg("-C")
                        .arg(&toplevel)
                        .args(["fetch", "--quiet"])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                    if fetched {
                        // Refresh so a new behind-count shows immediately.
                        // Empty scope = full refresh.
                        let _ = tx.try_send(Vec::new());
                    }
                }
                match raw_rx.recv_timeout(std::time::Duration::from_secs(1)) {
                    Ok(Ok(events)) => {
                        let paths: Vec<std::path::PathBuf> = events
                            .iter()
                            .flat_map(|event| event.paths.clone())
                            .filter(|path| is_status_relevant(path))
                            .collect();
                        if !paths.is_empty() {
                            // Bounded channel: under an absurd sustained
                            // storm the batch drops and we fall back to a
                            // full refresh so nothing goes silently stale.
                            if tx.try_send(paths).is_err() {
                                let _ = tx.try_send(Vec::new());
                            }
                        }
                    }
                    Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("spawn git watcher");
    stop
}
