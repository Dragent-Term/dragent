/// Async git status watcher model.
/// Spawns short-lived `git` subprocesses on a background thread to avoid blocking the UI.
use std::path::PathBuf;
use warpui::{Entity, ModelContext};

/// Per-file diff statistics from `git diff --numstat HEAD`.
#[derive(Debug, Clone, Default)]
pub struct GitDiffFileStat {
    /// Bare filename, e.g. "block.rs"
    pub filename: String,
    /// Shortened directory path for display, e.g. "…/agent/controller"
    pub dir_short: String,
    pub added: u32,
    pub deleted: u32,
    /// Full relative path from repo root, e.g. "app/src/agent/controller/block.rs".
    /// Used for `git add <rel_path>` when staging.
    pub rel_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct GitStatus {
    pub branch: String,
    /// Distinct tracked files with any change (staged and/or unstaged).
    /// Each file is counted once regardless of whether it has staged, unstaged,
    /// or both types of modifications.
    pub changed_files: u32,
    /// Number of files with staged changes — used for the chip colour logic.
    pub staged: u32,
    /// Number of files with unstaged changes — used for the chip colour logic.
    pub modified: u32,
    pub untracked: u32,
    pub ahead: u32,
    pub behind: u32,
    /// Total lines added across all changed files vs HEAD.
    pub added: u32,
    /// Total lines deleted across all changed files vs HEAD.
    pub deleted: u32,
    /// Per-file breakdown for the git diff bar UI.
    pub diff_files: Vec<GitDiffFileStat>,
    /// Repo-root-relative paths of files that are currently staged in the index.
    /// Populated from `git status --short` (`X` column != ' '/'?').
    pub staged_rel_paths: Vec<String>,
    /// Per-file porcelain entries: (root-relative path, index char, worktree
    /// char). `'?'` marks untracked; `' '` means "no change in this column".
    /// Feeds the source-control panel's staged/changes grouping.
    pub entries: Vec<GitStatusEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct GitStatusEntry {
    pub rel_path: String,
    pub index: char,
    pub work: char,
}

impl GitStatus {
    pub fn has_changes(&self) -> bool {
        self.changed_files > 0 || self.untracked > 0
    }

    /// Total distinct file count (each file counted once).
    pub fn total_changes(&self) -> u32 {
        self.changed_files + self.untracked
    }
}

pub enum GitWatcherEvent {
    Updated(Option<GitStatus>),
}

pub struct GitWatcher {
    pub status: Option<GitStatus>,
    pub cwd: PathBuf,
}

impl Entity for GitWatcher {
    type Event = GitWatcherEvent;
}

impl GitWatcher {
    pub fn new(cwd: PathBuf, ctx: &mut ModelContext<Self>) -> Self {
        let mut watcher = Self {
            status: None,
            cwd: cwd.clone(),
        };
        watcher.refresh(cwd, ctx);
        watcher
    }

    pub fn refresh(&mut self, cwd: PathBuf, ctx: &mut ModelContext<Self>) {
        self.cwd = cwd.clone();
        ctx.spawn(
            async move { fetch_git_status(cwd) },
            |model, result, ctx| {
                model.status = result;
                ctx.emit(GitWatcherEvent::Updated(model.status.clone()));
                ctx.notify();
            },
        );
    }
}

/// Run git commands synchronously (safe on a background tokio thread for short commands).
fn fetch_git_status(cwd: PathBuf) -> Option<GitStatus> {
    // 1. Get branch name.
    // A remote project's status comes from its own machine.
    let (client, cwd_path) = crate::remote_fs::host_for(&cwd).ok()?;
    let (branch_status, branch_stdout, _) = client
        .run_git(
            &cwd_path,
            &["rev-parse", "--abbrev-ref", "HEAD"],
        )
        .ok()?;
    if branch_status != 0 {
        return None;
    }

    let branch = String::from_utf8_lossy(&branch_stdout).trim().to_string();

    // 2. Parse `git status --short --branch -unormal`.
    // `-unormal` shows untracked directories as single entries instead of
    // recursively listing every file (`-uall`), avoiding multi-minute freezes.
    // Show-ignored + follow-symlinks from Version Control.
    // The global snapshot is read off the UI thread (this runs on a worker).
    let show_ignored = std::panic::catch_unwind(crate::settings::git_consumers::show_ignored)
        .unwrap_or(true);
    let follow_symlinks = std::panic::catch_unwind(crate::settings::git_consumers::follow_symlinks)
        .unwrap_or(false);
    let mut status_args: Vec<&str> = vec!["status", "--short", "--branch", "-unormal"];
    if show_ignored {
        status_args.push("--ignored");
    }
    let (status_code, status_stdout, _) = client
        .run_git(&cwd_path, &status_args)
        .ok()?;
    if status_code != 0 {
        return None;
    }

    let status_text = String::from_utf8_lossy(&status_stdout);
    let mut changed_files = 0u32;
    let mut staged = 0u32;
    let mut modified = 0u32;
    let mut untracked = 0u32;
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut staged_rel_paths: Vec<String> = Vec::new();
    let mut entries: Vec<GitStatusEntry> = Vec::new();

    for line in status_text.lines() {
        if line.starts_with("##") {
            // Parse ahead/behind from the branch tracking line.
            // Example: ## main...origin/main [ahead 2, behind 1]
            if let Some(bracket_start) = line.find('[') {
                let bracket_content = &line[bracket_start..];
                if let Some(a) = parse_ahead_behind(bracket_content, "ahead ") {
                    ahead = a;
                }
                if let Some(b) = parse_ahead_behind(bracket_content, "behind ") {
                    behind = b;
                }
            }
            continue;
        }

        // XY status codes: first char = index (staged), second = working tree.
        let bytes = line.as_bytes();
        if bytes.len() < 4 {
            continue;
        }
        let index_ch = bytes[0] as char;
        let work_ch = bytes[1] as char;
        let path_raw = line[3..].trim().trim_matches('"');
        let path = if let Some(arrow) = path_raw.find(" -> ") {
            path_raw[arrow + 4..].trim_matches('"').to_string()
        } else {
            path_raw.trim_matches('"').to_string()
        };

        // Ignored entries (\) only appear when show-ignored is on;
        // they render dimmed and never count as changes.
        let is_ignored = index_ch == '!' || work_ch == '!';
        // When not following symlinks, drop symlink-dir entries (local only;
        // remote hosts resolve through their own FS layer).
        if !follow_symlinks && !is_ignored {
            #[cfg(not(target_family = "wasm"))]
            {
                let abs = std::path::Path::new(&cwd_path).join(&path);
                if std::fs::symlink_metadata(&abs)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    continue;
                }
            }
        }
        entries.push(GitStatusEntry {
            rel_path: path.clone(),
            index: index_ch,
            work: work_ch,
        });

        if is_ignored {
            continue;
        }
        if index_ch == '?' && work_ch == '?' {
            untracked += 1;
            continue;
        }

        // Count this as one distinct changed file regardless of whether it has
        // staged changes, unstaged changes, or both.
        changed_files += 1;

        // Track staged/modified separately for chip colour logic (all-staged = green).
        if index_ch != ' ' && index_ch != '?' {
            staged += 1;
            staged_rel_paths.push(path);
        }
        if work_ch != ' ' && work_ch != '?' {
            modified += 1;
        }
    }

    // 3. Get line-level diff stats vs HEAD (staged + unstaged).
    let (added, deleted, diff_files) = fetch_diff_file_stats(&cwd);

    Some(GitStatus {
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

/// Run git numstat commands to get total lines added/deleted and per-file stats.
fn fetch_diff_file_stats(cwd: &std::path::Path) -> (u32, u32, Vec<GitDiffFileStat>) {
    crate::git_status::diff_head::fetch_file_stats(cwd)
}

/// Parse a number after a keyword like "ahead " or "behind " inside a bracket string.
fn parse_ahead_behind(text: &str, keyword: &str) -> Option<u32> {
    let pos = text.find(keyword)?;
    let after = &text[pos + keyword.len()..];
    let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ahead_behind_values() {
        assert_eq!(parse_ahead_behind("[ahead 3, behind 1]", "ahead "), Some(3));
        assert_eq!(
            parse_ahead_behind("[ahead 3, behind 1]", "behind "),
            Some(1)
        );
        assert_eq!(parse_ahead_behind("[ahead 5]", "behind "), None);
    }

    #[test]
    fn has_changes_reflects_counts() {
        let s = GitStatus {
            branch: "main".into(),
            changed_files: 1,
            modified: 1,
            ..Default::default()
        };
        assert!(s.has_changes());

        let clean = GitStatus::default();
        assert!(!clean.has_changes());
    }
}

// ── Layer 6a re-exports ────────────────────────────────────────────────────────
// New code should use `crate::git_status::GitStatusMetadata` and
// `crate::git_status::GitRepoStatusModel` directly.
pub use crate::git_status::GitRepoStatusModel;
pub use crate::git_status::GitStatusMetadata;
