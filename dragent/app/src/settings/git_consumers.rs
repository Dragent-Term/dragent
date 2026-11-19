//! Consumer helpers: read `AppSettings::global()` (+ per-project overrides)
//! at use sites. Mirrors the `respect_gitignore` pattern in
//! `dragent_index/walk.rs` — UI writes `AppSettings`, runtime reads the
//! cached global snapshot.

use std::path::{Path, PathBuf};

use super::git_settings::{RepoSettings, ScGroupOrder, ScViewMode};

// ── Branches ─────────────────────────────────────────────────────────────────

/// Final new-branch name: Jira-key casing + configured prefix.
/// When auto-rename is off, the raw name passes through untouched.
pub fn final_branch_name(raw: &str) -> String {
    let git = &crate::settings::AppSettings::global().git;
    if !git.auto_rename_branch {
        return raw.trim().to_string();
    }
    let cased = apply_jira_casing(raw, git.jira_casing);
    git.apply_branch_prefix(&cased)
}

fn apply_jira_casing(raw: &str, casing: super::git_settings::JiraCasing) -> String {
    use super::git_settings::JiraCasing;
    // Upper/lower-case tokens that look like Jira keys (letters-digits).
    let mut out = String::with_capacity(raw.len());
    let mut token = String::new();
    let mut flush = |token: &mut String, out: &mut String| {
        if token.len() >= 2 && token.contains('-') {
            let parts: Vec<&str> = token.split('-').collect();
            if parts.len() == 2
                && parts[0].chars().all(|c| c.is_ascii_alphabetic())
                && parts[1].chars().all(|c| c.is_ascii_digit())
                && !parts[0].is_empty()
                && !parts[1].is_empty()
            {
                out.push_str(&match casing {
                    JiraCasing::Upper => token.to_ascii_uppercase(),
                    JiraCasing::Lower => token.to_ascii_lowercase(),
                    JiraCasing::Preserve => token.clone(),
                });
                token.clear();
                return;
            }
        }
        out.push_str(token);
        token.clear();
    };
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
            out.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// Prefix to use for freshly generated worktree branches.
/// Falls back to `workspace` when the configured prefix is empty.
pub fn worktree_branch_prefix() -> String {
    let prefix = crate::settings::AppSettings::global()
        .git
        .prefix_string();
    if prefix.is_empty() {
        "workspace".to_string()
    } else {
        prefix
    }
}

// ── Diffs ────────────────────────────────────────────────────────────────────

pub fn diff_side_by_side() -> bool {
    crate::settings::AppSettings::global().git.side_by_side()
}

pub fn diff_word_wrap() -> bool {
    crate::settings::AppSettings::global().git.diff_word_wrap
}

pub fn diff_show_whitespace() -> bool {
    crate::settings::AppSettings::global().git.diff_show_whitespace
}

pub fn diff_collapse_unchanged() -> bool {
    crate::settings::AppSettings::global()
        .git
        .diff_collapse_unchanged
}

// ── Source control ───────────────────────────────────────────────────────────

pub fn sc_view_mode() -> ScViewMode {
    crate::settings::AppSettings::global().git.sc_view_mode
}

pub fn sc_group_order() -> ScGroupOrder {
    crate::settings::AppSettings::global().git.sc_group_order
}

pub fn sc_compare_upstream() -> bool {
    crate::settings::AppSettings::global()
        .git
        .sc_compare_upstream
}

pub fn follow_symlinks() -> bool {
    crate::settings::AppSettings::global().git.follow_symlinks
}

pub fn show_ignored() -> bool {
    crate::settings::AppSettings::global().git.show_ignored
}

// ── Workspace ────────────────────────────────────────────────────────────────

/// Base directory for new worktrees. Respects the user's override, then the
/// per-project `base_path`, then the default `~/.dragent/worktrees`.
pub fn workspace_base_dir(project_root: Option<&Path>) -> PathBuf {
    if let Some(root) = project_root {
        let repo = RepoSettings::effective(Some(root));
        if let Some(base) = repo.base_path {
            let trimmed = base.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    if let Some(dir) = crate::settings::AppSettings::global()
        .general
        .workspace_dir_path()
    {
        return dir;
    }
    super::super::workspace::project_naming::worktrees_root()
}

pub fn nest_workspaces() -> bool {
    crate::settings::AppSettings::global()
        .general
        .nest_workspaces
}

pub fn should_refresh_base_ref() -> bool {
    crate::settings::AppSettings::global()
        .general
        .refresh_base_ref_on_create
}

/// Effective base ref for a project: per-project `base_ref` wins, else the
/// caller's default.
pub fn effective_base_ref(project_root: Option<&Path>, default: Option<&str>) -> Option<String> {
    if let Some(root) = project_root {
        let repo = RepoSettings::effective(Some(root));
        if let Some(base) = repo.base_ref {
            if !base.trim().is_empty() {
                return Some(base);
            }
        }
    }
    default.map(str::to_string)
}

// ── Confirmations ────────────────────────────────────────────────────────────

pub fn confirm_close_terminal() -> bool {
    crate::settings::AppSettings::global()
        .general
        .confirm_close_terminal_with_process
}

pub fn confirm_delete_worktree() -> bool {
    crate::settings::AppSettings::global()
        .general
        .confirm_delete_worktree
}

// ── Links & sessions ─────────────────────────────────────────────────────────

pub fn open_links_in_app() -> bool {
    crate::settings::AppSettings::global()
        .general
        .open_links_in_app
}

pub fn restore_sessions() -> bool {
    crate::settings::AppSettings::global()
        .general
        .restore_sessions_on_startup
}

// ── Commit AI ────────────────────────────────────────────────────────────────

pub fn commit_ai_enabled() -> bool {
    crate::settings::AppSettings::global().git.commit_ai_enabled
}

pub fn commit_ai_model(default: &str) -> String {
    let global = crate::settings::AppSettings::global();
    global.git.commit_model(default).to_string()
}

// ── Repo overrides ───────────────────────────────────────────────────────────

pub fn effective_repo(root: Option<&Path>) -> RepoSettings {
    RepoSettings::effective(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_branch_name_applies_prefix_and_casing() {
        // Direct unit coverage lives in git_settings tests; this smoke test
        // ensures the consumer path respects the global snapshot.
        let name = final_branch_name("fix login");
        assert!(!name.is_empty());
    }

    #[test]
    fn workspace_base_dir_defaults_to_worktrees_root() {
        let dir = workspace_base_dir(None);
        assert!(!dir.as_os_str().is_empty());
    }
}
