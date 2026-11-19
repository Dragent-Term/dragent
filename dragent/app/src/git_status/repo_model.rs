//! Warpui Model that replaces `GitWatcher` with throttled refresh and richer metadata.

use crate::process_ext::NoWindow;
use std::path::{Component, Path, PathBuf};
use web_time::SystemTime;

use warpui::{Entity, ModelContext};

use crate::git_status::GitStatusMetadata;

/// Minimum milliseconds between background git refreshes when using `refresh()`
/// (the throttled variant). Watcher-driven `refresh_scoped()` bypasses this:
/// scoped fetches stay cheap regardless of repo size.
pub const GIT_REFRESH_THROTTLE_MS: u64 = 500;

// ── Event ─────────────────────────────────────────────────────────────────────

pub enum GitRepoStatusEvent {
    Updated {
        repo_root: PathBuf,
        status: Option<GitStatusMetadata>,
    },
    OperationChanged {
        operation: GitOperation,
        completed: Option<Result<(), String>>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitOperation {
    #[default]
    Idle,
    Committing,
    Pushing,
    Pulling,
}

// ── Model ─────────────────────────────────────────────────────────────────────

pub struct GitRepoStatusModel {
    /// Most recent git status snapshot, or `None` if not yet fetched / not a repo.
    pub status: Option<GitStatusMetadata>,
    /// The working directory for which the status was last fetched.
    pub cwd: PathBuf,
    /// User-initiated operation shared by every Git UI surface.
    pub operation: GitOperation,
    /// Most recent commit/push failure, cleared when a new operation starts.
    pub operation_error: Option<String>,
    /// Wall-clock time of the most recent completed refresh (for throttle checks).
    last_refresh: Option<SystemTime>,
    /// True while a background refresh is running. Guards against piling up
    /// overlapping `git` subprocesses when the file watcher fires faster than
    /// `git status` completes (a build touching thousands of files will do
    /// exactly that); the coalesced trailing refresh below catches us up.
    in_flight: bool,
    /// Set when a refresh was requested while one was already running, so we
    /// run exactly one more afterwards instead of dropping the change.
    pending: bool,
    /// Changed paths accumulated while a fetch is in flight (or while the
    /// throttle holds). Drained into the next fetch's scope so bursts
    /// coalesce instead of piling up subprocesses.
    pending_paths: Vec<PathBuf>,
    /// A snapshot withheld from display pending verification (see below).
    suspect_snapshot: Option<GitStatusMetadata>,
}

impl Entity for GitRepoStatusModel {
    type Event = GitRepoStatusEvent;
}

impl GitRepoStatusModel {
    /// Create a new model and immediately queue a background refresh.
    pub fn new(cwd: PathBuf, ctx: &mut ModelContext<Self>) -> Self {
        let mut model = Self {
            status: None,
            cwd: cwd.clone(),
            operation: GitOperation::Idle,
            operation_error: None,
            last_refresh: None,
            in_flight: false,
            pending: false,
            pending_paths: Vec::new(),
            suspect_snapshot: None,
        };
        model.do_refresh(cwd, Vec::new(), ctx);
        model
    }

    /// Throttled refresh: skips the subprocess if `GIT_REFRESH_THROTTLE_MS` has
    /// not elapsed since the last completed refresh.
    pub fn refresh(&mut self, cwd: PathBuf, ctx: &mut ModelContext<Self>) {
        if should_skip_refresh(self.last_refresh, GIT_REFRESH_THROTTLE_MS) {
            return;
        }
        self.do_refresh(cwd, Vec::new(), ctx);
    }

    /// Incremental refresh scoped to watcher-reported paths. Never throttled:
    /// a scoped `git status` touches only the given pathspecs, so bursts stay
    /// cheap and the UI tracks the filesystem within one debounce window. An
    /// empty path list means "scope unknown" and fetches everything.
    pub fn refresh_scoped(
        &mut self,
        cwd: PathBuf,
        paths: Vec<PathBuf>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.do_refresh(cwd, paths, ctx);
    }

    /// Bypass-throttle refresh: always spawns a subprocess regardless of elapsed time.
    /// Use this after a block completes or when the user explicitly requests it.
    pub fn refresh_bypass_throttle(&mut self, cwd: PathBuf, ctx: &mut ModelContext<Self>) {
        self.do_refresh(cwd, Vec::new(), ctx);
    }

    /// Stash changed paths for the trailing fetch, degrading to a full
    /// refresh if the backlog grows pathological.
    fn stash_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.pending_paths.extend(paths);
        if self.pending_paths.len() > 2000 {
            self.pending_paths.clear();
        }
    }

    pub fn begin_operation(&mut self, operation: GitOperation, ctx: &mut ModelContext<Self>) {
        self.operation = operation;
        self.operation_error = None;
        ctx.emit(GitRepoStatusEvent::OperationChanged {
            operation,
            completed: None,
        });
        ctx.notify();
    }

    pub fn finish_operation(
        &mut self,
        operation: GitOperation,
        result: Result<(), String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.operation = GitOperation::Idle;
        self.operation_error = result.as_ref().err().cloned();
        ctx.emit(GitRepoStatusEvent::OperationChanged {
            operation,
            completed: Some(result),
        });
        ctx.notify();
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn do_refresh(&mut self, cwd: PathBuf, paths: Vec<PathBuf>, ctx: &mut ModelContext<Self>) {
        // Collapse a burst into "the one running + one more". Without this, a
        // watcher firing during a build spawns a fresh pair of `git` processes
        // per event, unboundedly. Changed paths accumulate so the trailing
        // refresh covers everything that arrived mid-fetch.
        if self.in_flight {
            self.pending = true;
            self.stash_paths(paths);
            return;
        }
        self.stash_paths(paths);
        let drained: Vec<PathBuf> = std::mem::take(&mut self.pending_paths);
        // Without a baseline there is nothing to merge into: a scoped first
        // fetch would display a partial tree, so fall back to full.
        let scope = if self.status.is_none() {
            FetchScope::Full
        } else {
            scope_for_fetch(&cwd, &drained)
        };
        self.in_flight = true;
        self.cwd = cwd.clone();
        match scope {
            FetchScope::Full => {
                ctx.spawn(
                    async move { fetch_git_status(cwd) },
                    |model, result, ctx| {
                        model.finish_refresh(result, ctx);
                    },
                );
            }
            FetchScope::Scoped { specs, changed } => {
                ctx.spawn(
                    async move { fetch_status_parts(cwd, &specs) },
                    move |model: &mut Self, result, ctx| {
                        let merged = result.and_then(|(branch, parse, files)| {
                            model.apply_scoped_fetch(changed, branch, parse, files)
                        });
                        model.finish_refresh(merged, ctx);
                    },
                );
            }
        }
    }

    /// Shared completion: drop-quorum guard, swap, emit, and the trailing
    /// refresh for anything that arrived mid-fetch.
    fn finish_refresh(&mut self, result: Option<GitStatusMetadata>, ctx: &mut ModelContext<Self>) {
        self.in_flight = false;
        // Drop quorum: a snapshot that halves the entry count with no
        // local operation in flight is almost certainly a transient
        // read taken mid-mutation by another process (external `git
        // stash/add/checkout` racing our poll, or a cold-start read
        // during index setup) rather than 50 files genuinely
        // vanishing in one refresh. Withhold it and run one
        // verification fetch immediately: a real external change
        // confirms on re-read (applied ~one fetch late, invisible to
        // the user) while a transient state self-heals instead of
        // blanking the panel until the next unrelated event. The same
        // applies to a first-ever snapshot that arrives empty: a
        // genuinely clean repo just pays for one extra fetch.
        let sharp_drop = snapshot_drop_is_suspect(
            self.status.as_ref(),
            result.as_ref(),
            self.operation,
            self.suspect_snapshot.is_some(),
        );
        if sharp_drop {
            log::debug!(
                "[git-status] sharp entry drop {} -> {}; verifying before display",
                self.status
                    .as_ref()
                    .map_or(0, |status| status.entries.len()),
                result.as_ref().map_or(0, |status| status.entries.len()),
            );
            self.suspect_snapshot = result;
            self.last_refresh = Some(SystemTime::now());
            ctx.notify();
            self.pending = true;
        } else {
            if let Some(new) = result.as_ref() {
                let staged = new
                    .entries
                    .iter()
                    .filter(|entry| entry.index != ' ' && entry.index != '?')
                    .count();
                let unstaged = new.entries.iter().filter(|entry| entry.work != ' ').count();
                log::info!(
                    "[git-status] snapshot applied root={} staged={} unstaged={}",
                    self.cwd.display(),
                    staged,
                    unstaged,
                );
            } else {
                log::info!(
                    "[git-status] snapshot applied root={} status=None (not a repo?)",
                    self.cwd.display(),
                );
            }
            self.status = result.clone();
            self.suspect_snapshot = None;
            self.last_refresh = Some(SystemTime::now());
            ctx.emit(GitRepoStatusEvent::Updated {
                repo_root: self.cwd.clone(),
                status: result,
            });
            ctx.notify();
        }
        // Something changed while we were fetching — run once more so
        // the final state is never stale. Accumulated paths ride the
        // trailing fetch; an empty set fetches everything.
        if std::mem::take(&mut self.pending) {
            let cwd = self.cwd.clone();
            let paths = std::mem::take(&mut self.pending_paths);
            self.do_refresh(cwd, paths, ctx);
        }
    }

    /// Merge one scoped fetch into the live snapshot. Purges entries covered
    /// by the changed paths, inserts the fresh records (with the same D+??
    /// cross-merge the full parser applies), then recomputes every count so
    /// the merged tree is indistinguishable from a full fetch of the same
    /// state. Returns `None` only when there is no baseline to merge into.
    fn apply_scoped_fetch(
        &mut self,
        changed: Vec<String>,
        _branch: String,
        parse: PorcelainParse,
        files: Vec<crate::console::git_status::GitDiffFileStat>,
    ) -> Option<GitStatusMetadata> {
        let old = self.status.clone()?;
        let mut entries = old.entries;
        merge_status_records(&mut entries, parse.entries, &changed);
        let mut diff_files = old.diff_files;
        merge_diff_files(&mut diff_files, files, &changed);
        let (changed_files, staged, modified, untracked, staged_rel_paths) =
            summarize_entries(&entries);
        let added = diff_files.iter().map(|file| file.added).sum();
        let deleted = diff_files.iter().map(|file| file.deleted).sum();
        Some(GitStatusMetadata {
            branch: old.branch,
            changed_files,
            staged,
            modified,
            untracked,
            ahead: if parse.has_branch_header {
                parse.ahead
            } else {
                old.ahead
            },
            behind: if parse.has_branch_header {
                parse.behind
            } else {
                old.behind
            },
            added,
            deleted,
            diff_files,
            staged_rel_paths,
            entries,
        })
    }
}

// ── Throttle helper (pure, testable) ─────────────────────────────────────────

/// Returns `true` if a refresh should be skipped because `throttle_ms` has not
/// elapsed since `last`. Returns `false` when `last` is `None` (never refreshed).
pub fn should_skip_refresh(last: Option<SystemTime>, throttle_ms: u64) -> bool {
    let Some(last) = last else { return false };
    last.elapsed()
        .map(|e| e.as_millis() < throttle_ms as u128)
        .unwrap_or(false)
}

// ── Background git subprocess ─────────────────────────────────────────────────

/// Run git commands synchronously on a background thread and build `GitStatusMetadata`.
fn fetch_git_status(cwd: PathBuf) -> Option<GitStatusMetadata> {
    // 1. Branch name.
    let branch_out = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&cwd)
        .output()
        .ok()?;
    if !branch_out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&branch_out.stdout)
        .trim()
        .to_string();

    // 2. Parse `git status --porcelain --branch -z --untracked-files=all`.
    // `-z` (NUL records) disables git's C-style quoting entirely, so paths
    // with spaces or non-ASCII bytes arrive intact. The old newline parser
    // merely stripped the surrounding quotes and left octal escapes
    // (`\303\251`) in place — those rows displayed as garbage and matched
    // nothing downstream (open, discard, staged lookup).
    // `-uall` lists every untracked file instead of collapsing directories
    // to single `dir/` rows that cannot be opened or diffed (and that
    // disagreed with the diff view, which already used `-uall`). Ignored
    // trees are still excluded by git itself, and the existing throttle +
    // watcher relevance filter bound refresh cost.
    // `--no-optional-locks` keeps this background poll from taking `index.lock`
    // to rewrite the index stat-cache. Without it, a refresh that lands while
    // the user is running their own `git commit`/`rebase` can lose the race and
    // make THEIR command fail with "index.lock: File exists" — and the index
    // write also feeds the file watcher that triggered us.
    //
    // MUST be `--porcelain`, not `--short`: `--short` honors `status.relativePaths`
    // and reports paths relative to the invocation cwd, so from a subdirectory a
    // sibling file comes back as `../other-folder/file.txt`. `staged_rel_paths`
    // parsed from that then can't match `diff_files[].rel_path` (always
    // root-relative, from `git diff --numstat`) — a staged file's checkbox would
    // insert under one string and get looked up under the other, so it flips back
    // to unstaged on the very next refresh even though the index is correct.
    // `--porcelain` is root-relative unconditionally, by design, for exactly this
    // kind of script consumption — same two-char status format as `--short`.
    let status_out = crate::git_cmd::GitCommand::new()
        .no_window()
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--branch",
            "-z",
            "--untracked-files=all",
        ])
        .current_dir(&cwd)
        .output()
        .ok()?;
    if !status_out.status.success() {
        return None;
    }
    let parsed = parse_porcelain_nul(&status_out.stdout);

    let entries: Vec<crate::console::git_status::GitStatusEntry> = parsed
        .entries
        .into_iter()
        .map(|record| crate::console::git_status::GitStatusEntry {
            rel_path: record.rel_path,
            index: record.index,
            work: record.work,
        })
        .collect();
    let (changed_files, staged, modified, untracked, staged_rel_paths) =
        summarize_entries(&entries);
    let ahead = parsed.ahead;
    let behind = parsed.behind;

    // 4. Line-level stats vs HEAD (also captures per-file breakdown for the diff bar UI).
    let (added, deleted, diff_files) = crate::git_status::diff_head::fetch_file_stats(&cwd);

    Some(GitStatusMetadata {
        branch,
        changed_files,
        staged,
        modified,
        untracked,
        ahead,
        behind,
        added,
        deleted,
        diff_files,
        staged_rel_paths,
        entries,
    })
}

/// Scoped fetch for incremental refresh: branch name, parsed records, and
/// per-file numstat rows for exactly the given pathspecs. Callers merge the
/// parts into the live snapshot; nothing here mutates shared state.
fn fetch_status_parts(
    cwd: PathBuf,
    specs: &[String],
) -> Option<(
    String,
    PorcelainParse,
    Vec<crate::console::git_status::GitDiffFileStat>,
)> {
    let branch_out = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&cwd)
        .output()
        .ok()?;
    if !branch_out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&branch_out.stdout)
        .trim()
        .to_string();
    let mut args: Vec<String> = vec![
        "--no-optional-locks".to_string(),
        "status".to_string(),
        "--porcelain".to_string(),
        "--branch".to_string(),
        "-z".to_string(),
        "--untracked-files=all".to_string(),
        "--".to_string(),
    ];
    args.extend(specs.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let status_out = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(&arg_refs)
        .current_dir(&cwd)
        .output()
        .ok()?;
    if !status_out.status.success() {
        return None;
    }
    let parse = parse_porcelain_nul(&status_out.stdout);
    let files = crate::git_status::diff_head::fetch_file_stats_scoped(&cwd, specs);
    Some((branch, parse, files))
}

/// One parsed porcelain record: two status columns plus the path git
/// reports (rename destination, matching previous display behavior).
struct PorcelainRecord {
    rel_path: String,
    index: char,
    work: char,
}

struct PorcelainParse {
    entries: Vec<PorcelainRecord>,
    ahead: u32,
    behind: u32,
    has_branch_header: bool,
}

/// Parse NUL-separated `git status --porcelain -z` output. Pure and
/// testable; the fetch only supplies bytes.
fn parse_porcelain_nul(output: &[u8]) -> PorcelainParse {
    let records: Vec<&[u8]> = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect();
    let mut entries: Vec<PorcelainRecord> = Vec::new();
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut has_branch_header = false;
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() >= 2 && &record[..2] == b"##" {
            has_branch_header = true;
            let text = String::from_utf8_lossy(record);
            if let Some(bracket_start) = text.find('[') {
                let bracket = &text[bracket_start..];
                if let Some(a) = parse_tracking_number(bracket, "ahead ") {
                    ahead = a;
                }
                if let Some(b) = parse_tracking_number(bracket, "behind ") {
                    behind = b;
                }
            }
            index += 1;
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            index += 1;
            continue;
        }
        let index_ch = record[0] as char;
        let work_ch = record[1] as char;
        // -z never C-quotes: raw bytes are the path, so spaces and
        // non-ASCII names survive intact (lossy only for invalid UTF-8,
        // which previously vanished or broke byte slicing).
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        // -z reverses rename pairs (destination first, source second):
        // consume the source record and keep the destination.
        let renamed = matches!(index_ch, 'R' | 'C') || matches!(work_ch, 'R' | 'C');
        if renamed {
            index += 1;
        }
        // Collapsed untracked directories cannot be opened or diffed; with
        // -uall they never occur, so reaching one means unexpected output.
        // Skip it rather than render a dead row.
        if path.ends_with('/') {
            index += 1;
            continue;
        }
        // A path deleted in the index and recreated in the worktree arrives
        // as two records (`D ` + `??`). Merge them into one DA entry so the
        // panel shows a single row instead of two unrelated ones.
        if let Some(existing) = entries.iter_mut().find(|entry| entry.rel_path == path) {
            if existing.index == 'D' && existing.work == ' ' && index_ch == '?' && work_ch == '?' {
                existing.work = 'A';
                index += 1;
                continue;
            }
            if existing.index == '?' && existing.work == '?' && index_ch == 'D' && work_ch == ' ' {
                existing.index = 'D';
                existing.work = 'A';
                index += 1;
                continue;
            }
        }
        entries.push(PorcelainRecord {
            rel_path: path,
            index: index_ch,
            work: work_ch,
        });
        index += 1;
    }
    PorcelainParse {
        entries,
        ahead,
        behind,
        has_branch_header,
    }
}

/// Full refresh or a pathspec-scoped incremental one (incremental: only the
/// watcher-reported paths are re-queried, then merged into the live tree).
enum FetchScope {
    Full,
    Scoped {
        /// `:(literal)`-prefixed, forward-slash repo-relative pathspecs.
        specs: Vec<String>,
        /// Same paths without the magic prefix, for purging merged state.
        changed: Vec<String>,
    },
}

/// Decide full vs scoped fetch for watcher-reported absolute paths.
/// Anything under `.git/`, unresolvable, or empty forces full: index and
/// branch transitions deserve a complete re-read, never a merge.
fn scope_for_fetch(cwd: &Path, paths: &[PathBuf]) -> FetchScope {
    if paths.is_empty() {
        return FetchScope::Full;
    }
    let mut specs = Vec::with_capacity(paths.len());
    let mut changed = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(rel) = path.strip_prefix(cwd) else {
            return FetchScope::Full;
        };
        if rel.as_os_str().is_empty() {
            return FetchScope::Full;
        }
        if rel.components().any(|component| {
            matches!(component, Component::Normal(name) if name == std::ffi::OsStr::new(".git"))
        }) {
            return FetchScope::Full;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        changed.push(rel_str.clone());
        specs.push(format!(":(literal){rel_str}"));
    }
    specs.sort();
    specs.dedup();
    changed.sort();
    changed.dedup();
    FetchScope::Scoped { specs, changed }
}

/// True when a live entry is covered by the changed set: the exact path or
/// anything underneath a changed directory.
fn path_covered(rel_path: &str, changed: &[String]) -> bool {
    changed
        .iter()
        .any(|changed| rel_path == changed || rel_path.starts_with(&format!("{changed}/")))
}

/// Merge fresh records into the live entry list: purge everything the
/// changed set covers, then insert (with the same D+?? cross-merge the
/// full parser applies for paths recreated between event and fetch).
fn merge_status_records(
    existing: &mut Vec<crate::console::git_status::GitStatusEntry>,
    fresh: Vec<PorcelainRecord>,
    changed: &[String],
) {
    existing.retain(|entry| !path_covered(&entry.rel_path, changed));
    for record in fresh {
        if let Some(entry) = existing
            .iter_mut()
            .find(|entry| entry.rel_path == record.rel_path)
        {
            match (entry.index, entry.work, record.index, record.work) {
                ('D', ' ', '?', '?') => entry.work = 'A',
                ('?', '?', 'D', ' ') => {
                    entry.index = 'D';
                    entry.work = 'A';
                }
                _ => {
                    entry.index = record.index;
                    entry.work = record.work;
                }
            }
            continue;
        }
        existing.push(crate::console::git_status::GitStatusEntry {
            rel_path: record.rel_path,
            index: record.index,
            work: record.work,
        });
    }
}

/// Merge fresh per-file numstat rows: same purge rule, then insert.
fn merge_diff_files(
    existing: &mut Vec<crate::console::git_status::GitDiffFileStat>,
    fresh: Vec<crate::console::git_status::GitDiffFileStat>,
    changed: &[String],
) {
    existing.retain(|file| !path_covered(&file.rel_path, changed));
    existing.extend(fresh);
}

/// Recompute every count from an entry list with exactly the rules the full
/// fetch applies, so merged trees are indistinguishable from full ones.
fn summarize_entries(
    entries: &[crate::console::git_status::GitStatusEntry],
) -> (u32, u32, u32, u32, Vec<String>) {
    let mut changed_files = 0u32;
    let mut staged = 0u32;
    let mut modified = 0u32;
    let mut untracked = 0u32;
    let mut staged_rel_paths = Vec::new();
    for entry in entries {
        if entry.index == '?' && entry.work == '?' {
            untracked += 1;
            continue;
        }
        changed_files += 1;
        if entry.index != ' ' && entry.index != '?' {
            staged += 1;
            staged_rel_paths.push(entry.rel_path.clone());
        }
        if entry.work != ' ' && entry.work != '?' {
            modified += 1;
        }
    }
    (changed_files, staged, modified, untracked, staged_rel_paths)
}

/// True when a fresh snapshot should be withheld pending one verification
/// re-fetch: the entry count halved with no local operation to explain it
/// and no verification already in flight — or the first-ever snapshot
/// arrived with zero entries, which reads like a cold-start race rather
/// than a genuinely clean tree.
fn snapshot_drop_is_suspect(
    old: Option<&GitStatusMetadata>,
    new: Option<&GitStatusMetadata>,
    operation: GitOperation,
    verification_in_flight: bool,
) -> bool {
    if operation != GitOperation::Idle || verification_in_flight {
        return false;
    }
    match (old, new) {
        (Some(old), Some(new)) => {
            !old.entries.is_empty() && new.entries.len() * 2 < old.entries.len()
        }
        (None, Some(new)) => new.entries.is_empty(),
        _ => false,
    }
}

/// Parse a number after a keyword like "ahead " or "behind " within a bracket string.
fn parse_tracking_number(text: &str, keyword: &str) -> Option<u32> {
    let pos = text.find(keyword)?;
    let after = &text[pos + keyword.len()..];
    let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn nul_fixture(parts: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(part.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn parse_porcelain_nul_counts() {
        // NUL-separated records: "XY <path>", branch record first.
        let parsed = parse_porcelain_nul(&nul_fixture(&[
            "## main...origin/main [ahead 1, behind 2]",
            "M  src/foo.rs",
            " M src/bar.rs",
            "MM src/baz.rs",
            "?? untracked.txt",
            "A  new_file.rs",
        ]));
        let paths: Vec<&str> = parsed
            .entries
            .iter()
            .map(|entry| entry.rel_path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "src/foo.rs",
                "src/bar.rs",
                "src/baz.rs",
                "untracked.txt",
                "new_file.rs"
            ]
        );
        assert_eq!(parsed.ahead, 1, "ahead");
        assert_eq!(parsed.behind, 2, "behind");
    }

    #[test]
    fn parse_porcelain_nul_keeps_special_paths_intact() {
        // -z never C-quotes: spaces and non-ASCII bytes arrive raw, unlike
        // the old newline parser which left octal escapes behind.
        let parsed = parse_porcelain_nul(&nul_fixture(&[
            "?? my notes/todo list.txt",
            " M caf\u{e9}.rs",
            "A  spaced dir/new file.rs",
        ]));
        let paths: Vec<&str> = parsed
            .entries
            .iter()
            .map(|entry| entry.rel_path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "my notes/todo list.txt",
                "caf\u{e9}.rs",
                "spaced dir/new file.rs"
            ]
        );
    }

    #[test]
    fn parse_porcelain_nul_rename_pair_keeps_destination() {
        // -z reverses rename pairs: destination record first, source second.
        let parsed = parse_porcelain_nul(&nul_fixture(&["R  new.rs", "old.rs"]));
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].rel_path, "new.rs");
        assert_eq!(parsed.entries[0].index, 'R');
    }

    #[test]
    fn parse_porcelain_nul_skips_collapsed_dirs() {
        let parsed = parse_porcelain_nul(&nul_fixture(&["?? build-out/", " M src/main.rs"]));
        let paths: Vec<&str> = parsed
            .entries
            .iter()
            .map(|entry| entry.rel_path.as_str())
            .collect();
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn parse_porcelain_nul_merges_delete_plus_untracked() {
        // Deleted in the index, recreated in the worktree: one DA row, not
        // two unrelated rows.
        let parsed = parse_porcelain_nul(&nul_fixture(&["D  gone.rs", "?? gone.rs"]));
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].rel_path, "gone.rs");
        assert_eq!(parsed.entries[0].index, 'D');
        assert_eq!(parsed.entries[0].work, 'A');
        // Order-independent: untracked record first.
        let parsed = parse_porcelain_nul(&nul_fixture(&["?? gone.rs", "D  gone.rs"]));
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].index, 'D');
        assert_eq!(parsed.entries[0].work, 'A');
    }

    fn metadata_with_entries(count: usize) -> GitStatusMetadata {
        GitStatusMetadata {
            branch: "main".to_string(),
            changed_files: count as u32,
            staged: 0,
            modified: 0,
            untracked: 0,
            ahead: 0,
            behind: 0,
            added: 0,
            deleted: 0,
            diff_files: Vec::new(),
            staged_rel_paths: Vec::new(),
            entries: (0..count)
                .map(|index| crate::console::git_status::GitStatusEntry {
                    rel_path: format!("file{index}.rs"),
                    index: ' ',
                    work: 'M',
                })
                .collect(),
        }
    }

    #[test]
    fn sharp_idle_drop_is_suspect() {
        let old = metadata_with_entries(50);
        let new = metadata_with_entries(0);
        assert!(snapshot_drop_is_suspect(
            Some(&old),
            Some(&new),
            GitOperation::Idle,
            false,
        ));
        // A verification is already running: accept whatever lands.
        assert!(!snapshot_drop_is_suspect(
            Some(&old),
            Some(&new),
            GitOperation::Idle,
            true,
        ));
        // A local operation explains the drop: apply immediately.
        assert!(!snapshot_drop_is_suspect(
            Some(&old),
            Some(&new),
            GitOperation::Committing,
            false,
        ));
        // Small dips are normal churn, not mid-mutation reads.
        let slight = metadata_with_entries(40);
        assert!(!snapshot_drop_is_suspect(
            Some(&old),
            Some(&slight),
            GitOperation::Idle,
            false,
        ));
        // First-ever snapshot arriving empty reads as a cold-start race:
        // verify once. A first snapshot with content applies immediately.
        assert!(snapshot_drop_is_suspect(
            None,
            Some(&new),
            GitOperation::Idle,
            false,
        ));
        let klein = metadata_with_entries(2);
        assert!(!snapshot_drop_is_suspect(
            None,
            Some(&klein),
            GitOperation::Idle,
            false,
        ));
        // Fetch failed: never suspect.
        assert!(!snapshot_drop_is_suspect(
            Some(&old),
            None,
            GitOperation::Idle,
            false,
        ));
    }

    #[test]
    fn live_fetch_reports_staged_unstaged_and_untracked() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?} failed");
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.name", "Status Test"]);
        git(&["config", "user.email", "status@example.test"]);
        std::fs::write(root.join("staged.rs"), "fn staged() {}\n").unwrap();
        std::fs::write(root.join("work.rs"), "fn v1() {}\n").unwrap();
        git(&["add", "staged.rs", "work.rs"]);
        git(&["commit", "--quiet", "-m", "initial"]);
        // Staged change, unstaged change, and a brand-new untracked file.
        std::fs::write(root.join("staged.rs"), "fn staged2() {}\n").unwrap();
        git(&["add", "staged.rs"]);
        std::fs::write(root.join("work.rs"), "fn v2() {}\n").unwrap();
        std::fs::write(root.join("fresh.txt"), "new\n").unwrap();

        let snapshot =
            fetch_git_status(root.to_path_buf()).expect("status fetch failed in a valid repo");
        let staged: Vec<&str> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.index != ' ' && entry.index != '?')
            .map(|entry| entry.rel_path.as_str())
            .collect();
        let unstaged: Vec<&str> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.work != ' ')
            .map(|entry| entry.rel_path.as_str())
            .collect();
        assert_eq!(staged, vec!["staged.rs"]);
        assert!(unstaged.contains(&"work.rs"), "unstaged: {unstaged:?}");
        assert!(unstaged.contains(&"fresh.txt"), "unstaged: {unstaged:?}");
    }

    #[test]
    fn scope_for_fetch_full_on_empty_git_internals_and_outside() {
        let cwd = PathBuf::from("/repo");
        assert!(matches!(scope_for_fetch(&cwd, &[]), FetchScope::Full));
        assert!(matches!(
            scope_for_fetch(&cwd, &[PathBuf::from("/repo/.git/HEAD")]),
            FetchScope::Full
        ));
        assert!(matches!(
            scope_for_fetch(&cwd, &[PathBuf::from("/other/x.rs")]),
            FetchScope::Full
        ));
        match scope_for_fetch(
            &cwd,
            &[
                PathBuf::from("/repo/sub/c.rs"),
                PathBuf::from("/repo/a b.rs"),
            ],
        ) {
            FetchScope::Scoped { specs, changed } => {
                // Sorted, forward slashes, glob magic neutralized.
                assert_eq!(
                    specs,
                    vec![
                        ":(literal)a b.rs".to_string(),
                        ":(literal)sub/c.rs".to_string()
                    ]
                );
                assert_eq!(changed, vec!["a b.rs".to_string(), "sub/c.rs".to_string()]);
            }
            FetchScope::Full => panic!("expected scoped fetch"),
        }
    }

    fn test_entry(
        path: &str,
        index: char,
        work: char,
    ) -> crate::console::git_status::GitStatusEntry {
        crate::console::git_status::GitStatusEntry {
            rel_path: path.to_string(),
            index,
            work,
        }
    }

    fn test_record(path: &str, index: char, work: char) -> PorcelainRecord {
        PorcelainRecord {
            rel_path: path.to_string(),
            index,
            work,
        }
    }

    #[test]
    fn scoped_merge_purges_covered_and_inserts_fresh() {
        let mut existing = vec![
            test_entry("a.rs", 'M', ' '),
            test_entry("b.rs", ' ', 'M'),
            test_entry("sub/old.rs", 'M', ' '),
        ];
        // b.rs modified again upstream: replaced, others untouched.
        merge_status_records(
            &mut existing,
            vec![test_record("b.rs", 'M', 'M')],
            &["b.rs".to_string()],
        );
        assert_eq!(existing.len(), 3);
        let b = existing
            .iter()
            .find(|entry| entry.rel_path == "b.rs")
            .unwrap();
        assert_eq!((b.index, b.work), ('M', 'M'));
        // Directory scope purges everything underneath it.
        merge_status_records(&mut existing, vec![], &["sub".to_string()]);
        assert_eq!(existing.len(), 2);
        assert!(existing.iter().all(|entry| entry.rel_path != "sub/old.rs"));
        // Covered path: purge drops the stale D row, fresh ?? inserts clean.
        let mut existing = vec![test_entry("gone.rs", 'D', ' ')];
        merge_status_records(
            &mut existing,
            vec![test_record("gone.rs", '?', '?')],
            &["gone.rs".to_string()],
        );
        assert_eq!(existing.len(), 1);
        assert_eq!((existing[0].index, existing[0].work), ('?', '?'));
        // Backstop: fresh ?? for a path OUTSIDE the changed set merges with
        // a surviving D row (recreated between event batch and fetch).
        let mut existing = vec![test_entry("gone.rs", 'D', ' ')];
        merge_status_records(
            &mut existing,
            vec![test_record("gone.rs", '?', '?')],
            &["other.rs".to_string()],
        );
        assert_eq!(existing.len(), 1);
        assert_eq!((existing[0].index, existing[0].work), ('D', 'A'));
    }

    #[test]
    fn throttle_skips_burst() {
        // A refresh that happened 1 second ago should be skipped given a 5s throttle.
        let one_sec_ago = SystemTime::now()
            .checked_sub(std::time::Duration::from_millis(1_000))
            .unwrap();
        assert!(
            should_skip_refresh(Some(one_sec_ago), 5_000),
            "should skip when only 1s has elapsed with 5s throttle"
        );

        // A refresh that happened 6 seconds ago should NOT be skipped.
        let six_secs_ago = SystemTime::now()
            .checked_sub(std::time::Duration::from_millis(6_000))
            .unwrap();
        assert!(
            !should_skip_refresh(Some(six_secs_ago), 5_000),
            "should not skip when 6s has elapsed with 5s throttle"
        );

        // No previous refresh — should not skip.
        assert!(
            !should_skip_refresh(None, 5_000),
            "should not skip when no previous refresh"
        );
    }
}
