//! Git + Repo/Project + General + Setup-guide settings.
//!
//! - `GitSettings`: branch naming, diff rendering, source-control view, git
//!   behavior, commit-AI, open-in apps, PR bot overrides.
//! - `RepoSettings`: per-project overrides (base ref/path, hooks, issue
//!   source, accounts, identity, visibility, symlinks, grouping) + global
//!   defaults template stored in `AppSettings.repo`.
//! - `GeneralSettings`: workspace dir, nesting, confirmations, links,
//!   session restore, setup-guide checklist.
//!
//! Per-project overrides live in `~/.dragent/repo_settings.json` as a
//! `{project_path: RepoSettings}` map (sidecar-style store keyed by project
//! root). `RepoSettings::effective(root)` merges global defaults with the
//! per-project entry (per-project `Some`/non-empty wins, `order` wins when
//! non-zero).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

// ── Enums ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BranchPrefix {
    #[default]
    GitUsername,
    Custom,
    None,
}

impl BranchPrefix {
    pub fn label(self) -> &'static str {
        match self {
            Self::GitUsername => "Git username",
            Self::Custom => "Custom",
            Self::None => "None",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::GitUsername, Self::Custom, Self::None]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JiraCasing {
    #[default]
    Upper,
    Lower,
    Preserve,
}

impl JiraCasing {
    pub fn label(self) -> &'static str {
        match self {
            Self::Upper => "UPPER",
            Self::Lower => "lower",
            Self::Preserve => "As-is",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Upper, Self::Lower, Self::Preserve]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffView {
    #[default]
    Inline,
    SideBySide,
}

impl DiffView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inline => "Inline",
            Self::SideBySide => "Side-by-side",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScViewMode {
    #[default]
    List,
    Tree,
}

impl ScViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Tree => "Tree",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScGroupOrder {
    #[default]
    Status,
    Path,
    FileType,
}

impl ScGroupOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Path => "Path",
            Self::FileType => "File type",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Status, Self::Path, Self::FileType]
    }
}

// ── GitSettings ──────────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSettings {
    #[serde(default = "default_true")]
    pub auto_rename_branch: bool,
    #[serde(default)]
    pub branch_prefix: BranchPrefix,
    #[serde(default)]
    pub branch_prefix_custom: String,
    #[serde(default)]
    pub jira_casing: JiraCasing,
    #[serde(default)]
    pub diff_view: DiffView,
    #[serde(default)]
    pub diff_word_wrap: bool,
    #[serde(default)]
    pub diff_show_whitespace: bool,
    #[serde(default)]
    pub diff_collapse_unchanged: bool,
    #[serde(default)]
    pub sc_view_mode: ScViewMode,
    #[serde(default)]
    pub sc_group_order: ScGroupOrder,
    #[serde(default)]
    pub sc_compare_upstream: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_true")]
    pub show_ignored: bool,
    #[serde(default)]
    pub pr_bot_overrides: Vec<String>,
    #[serde(default)]
    pub open_in_apps: Vec<String>,
    #[serde(default = "default_true")]
    pub commit_ai_enabled: bool,
    #[serde(default)]
    pub commit_ai_agent: Option<String>,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            auto_rename_branch: true,
            branch_prefix: BranchPrefix::GitUsername,
            branch_prefix_custom: String::new(),
            jira_casing: JiraCasing::Upper,
            diff_view: DiffView::Inline,
            diff_word_wrap: false,
            diff_show_whitespace: false,
            diff_collapse_unchanged: false,
            sc_view_mode: ScViewMode::List,
            sc_group_order: ScGroupOrder::Status,
            sc_compare_upstream: false,
            follow_symlinks: false,
            show_ignored: true,
            pr_bot_overrides: Vec::new(),
            open_in_apps: Vec::new(),
            commit_ai_enabled: true,
            commit_ai_agent: None,
        }
    }
}

impl GitSettings {
    /// Local git username (`git config user.name` slugified, else $USER).
    pub fn git_username() -> String {
        git_username_slugged()
    }

    /// Prefix string for the current config ("" when disabled).
    pub fn prefix_string(&self) -> String {
        match self.branch_prefix {
            BranchPrefix::None => String::new(),
            BranchPrefix::Custom => slug_prefix(&self.branch_prefix_custom),
            BranchPrefix::GitUsername => slug_prefix(&git_username_slugged()),
        }
    }

    /// Apply the configured branch prefix to `raw` (idempotent, no doubles).
    pub fn apply_branch_prefix(&self, raw: &str) -> String {
        let prefix = self.prefix_string();
        if prefix.is_empty() {
            return raw.trim().to_string();
        }
        let raw = raw.trim().trim_matches('/').to_string();
        if raw == prefix || raw.starts_with(&format!("{prefix}/")) {
            return raw;
        }
        format!("{prefix}/{raw}")
    }

    /// Format a Jira-style key (`abc-123`) per casing setting.
    pub fn format_jira_key(&self, key: &str) -> String {
        match self.jira_casing {
            JiraCasing::Upper => key.to_ascii_uppercase(),
            JiraCasing::Lower => key.to_ascii_lowercase(),
            JiraCasing::Preserve => key.to_string(),
        }
    }

    pub fn side_by_side(&self) -> bool {
        matches!(self.diff_view, DiffView::SideBySide)
    }

    /// First configured "Open in" app, if any.
    pub fn first_open_app(&self) -> Option<&str> {
        self.open_in_apps
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .next()
    }

    /// Model override for commit-message generation (None = caller's default).
    pub fn commit_model<'a>(&'a self, default: &'a str) -> &'a str {
        match self.commit_ai_agent.as_deref() {
            Some(agent) if !agent.trim().is_empty() => agent,
            _ => default,
        }
    }
}

fn slug_prefix(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Read once per run: the Version Control page shows the prefix while it
/// renders, and starting `git` there each frame would stall the UI.
fn git_username_slugged() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(read_git_username_slugged).clone()
}

fn read_git_username_slugged() -> String {
    use crate::process_ext::NoWindow as _;
    let from_git = std::process::Command::new("git")
        .args(["config", "--get", "user.name"])
        .no_window()
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let raw = from_git
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_default();
    let slugged = slug_prefix(&raw);
    if slugged.is_empty() {
        "user".to_string()
    } else {
        slugged
    }
}

// ── RepoSettings ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoSettings {
    #[serde(default)]
    pub base_ref: Option<String>,
    #[serde(default)]
    pub base_path: Option<String>,
    #[serde(default)]
    pub hook_mode: Option<String>,
    #[serde(default)]
    pub hook_scripts: Vec<String>,
    #[serde(default = "default_issue_source")]
    pub issue_source: String,
    #[serde(default)]
    pub gh_account: Option<String>,
    #[serde(default)]
    pub fork_sync: Option<String>,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub symlink_paths: Vec<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub host_setup_method: Option<String>,
    #[serde(default)]
    pub windows_runtime: Option<String>,
}

fn default_issue_source() -> String {
    "auto".to_string()
}

impl RepoSettings {
    pub fn is_empty(&self) -> bool {
        self.base_ref.is_none()
            && self.base_path.is_none()
            && self.hook_mode.is_none()
            && self.hook_scripts.is_empty()
            && (self.issue_source == "auto" || self.issue_source.is_empty())
            && self.gh_account.is_none()
            && self.fork_sync.is_none()
            && self.identity.is_none()
            && self.visibility.is_none()
            && self.symlink_paths.is_empty()
            && self.group_id.is_none()
            && self.order == 0
            && self.host_setup_method.is_none()
            && self.windows_runtime.is_none()
    }

    /// Merge global defaults with the per-project override for `root`.
    /// Per-project `Some`/non-empty wins; `order` wins when non-zero.
    pub fn effective(root: Option<&Path>) -> Self {
        let defaults = crate::settings::AppSettings::global().repo.clone();
        let Some(root) = root else {
            return defaults;
        };
        let Some(over) = RepoSettingsStore::get(root) else {
            return defaults;
        };
        Self {
            base_ref: over.base_ref.or(defaults.base_ref),
            base_path: over.base_path.or(defaults.base_path),
            hook_mode: over.hook_mode.or(defaults.hook_mode),
            hook_scripts: if over.hook_scripts.is_empty() {
                defaults.hook_scripts
            } else {
                over.hook_scripts
            },
            issue_source: if over.issue_source.is_empty() || over.issue_source == "auto" {
                defaults.issue_source
            } else {
                over.issue_source
            },
            gh_account: over.gh_account.or(defaults.gh_account),
            fork_sync: over.fork_sync.or(defaults.fork_sync),
            identity: over.identity.or(defaults.identity),
            visibility: over.visibility.or(defaults.visibility),
            symlink_paths: if over.symlink_paths.is_empty() {
                defaults.symlink_paths
            } else {
                over.symlink_paths
            },
            group_id: over.group_id.or(defaults.group_id),
            order: if over.order != 0 {
                over.order
            } else {
                defaults.order
            },
            host_setup_method: over.host_setup_method.or(defaults.host_setup_method),
            windows_runtime: over.windows_runtime.or(defaults.windows_runtime),
        }
    }
}

// ── RepoSettingsStore (sidecar map keyed by project root) ────────────────────

fn repo_store_path() -> PathBuf {
    dragent_paths::home().join("repo_settings.json")
}

fn repo_store_cell() -> &'static RwLock<HashMap<String, RepoSettings>> {
    static CELL: OnceLock<RwLock<HashMap<String, RepoSettings>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(RepoSettingsStore::load_from_disk()))
}

pub struct RepoSettingsStore;

impl RepoSettingsStore {
    fn key(root: &Path) -> String {
        root.display().to_string()
    }

    fn load_from_disk() -> HashMap<String, RepoSettings> {
        let path = repo_store_path();
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        if raw.trim().is_empty() {
            return HashMap::new();
        }
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save_to_disk(map: &HashMap<String, RepoSettings>) {
        let path = repo_store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(map) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn get(root: &Path) -> Option<RepoSettings> {
        repo_store_cell()
            .read()
            .map(|m| m.get(&Self::key(root)).cloned())
            .unwrap_or_default()
    }

    pub fn set(root: &Path, value: RepoSettings) {
        let changed = match repo_store_cell().write() {
            Ok(mut m) => {
                if value.is_empty() {
                    m.remove(&Self::key(root));
                } else {
                    m.insert(Self::key(root), value);
                }
                Some(m.clone())
            }
            Err(mut poisoned) => {
                let m = poisoned.get_mut();
                if value.is_empty() {
                    m.remove(&Self::key(root));
                } else {
                    m.insert(Self::key(root), value.clone());
                }
                Some(m.clone())
            }
        };
        if let Some(map) = changed {
            Self::save_to_disk(&map);
        }
    }

    pub fn mutate(root: &Path, f: impl FnOnce(&mut RepoSettings)) {
        // Start from the stored override (or empty) so global defaults are not
        // baked into the sidecar.
        let mut current = Self::get(root).unwrap_or_default();
        f(&mut current);
        Self::set(root, current);
    }

    pub fn clear(root: &Path) {
        if let Some(map) = match repo_store_cell().write() {
            Ok(mut m) => {
                m.remove(&Self::key(root));
                Some(m.clone())
            }
            Err(mut p) => {
                p.get_mut().remove(&Self::key(root));
                Some(p.get_mut().clone())
            }
        } {
            Self::save_to_disk(&map);
        }
    }

    #[cfg(test)]
    pub fn test_reset() {
        if let Ok(mut m) = repo_store_cell().write() {
            m.clear();
        }
    }
}

// ── GeneralSettings ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(default)]
    pub workspace_dir: Option<String>,
    #[serde(default = "default_true")]
    pub nest_workspaces: bool,
    #[serde(default)]
    pub refresh_base_ref_on_create: bool,
    #[serde(default = "default_true")]
    pub confirm_close_terminal_with_process: bool,
    #[serde(default = "default_true")]
    pub confirm_delete_worktree: bool,
    /// Web links open in Dragent's browser (its bar can still hand a page
    /// to the system browser). Saved under a new key: files written while
    /// this was off by default hold `open_links_in_app: false`, which nobody
    /// chose, so it is ignored and they start on.
    #[serde(rename = "open_links_in_dragent", default = "default_true")]
    pub open_links_in_app: bool,
    #[serde(default = "default_true")]
    pub restore_sessions_on_startup: bool,
    #[serde(default)]
    pub setup_guide_completed: bool,
    #[serde(default)]
    pub setup_guide_dismissed_items: Vec<String>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            workspace_dir: None,
            nest_workspaces: true,
            refresh_base_ref_on_create: false,
            confirm_close_terminal_with_process: true,
            confirm_delete_worktree: true,
            open_links_in_app: true,
            restore_sessions_on_startup: true,
            setup_guide_completed: false,
            setup_guide_dismissed_items: Vec::new(),
        }
    }
}

impl GeneralSettings {
    /// Base dir for new worktrees, when the user overrode the default.
    pub fn workspace_dir_path(&self) -> Option<PathBuf> {
        self.workspace_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }

    pub fn setup_item_dismissed(&self, id: &str) -> bool {
        self.setup_guide_dismissed_items.iter().any(|d| d == id)
    }

    pub fn dismiss_setup_item(&mut self, id: &str) {
        if !self.setup_item_dismissed(id) {
            self.setup_guide_dismissed_items.push(id.to_string());
        }
    }

    pub fn undismiss_setup_item(&mut self, id: &str) {
        self.setup_guide_dismissed_items.retain(|d| d != id);
    }
}

/// Checklist steps for the setup guide (stable ids).
pub fn setup_guide_steps() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "account",
            "Sign in",
            "Connect your Dragent account to sync projects and billing.",
        ),
        (
            "project",
            "Open a project",
            "Add a local folder or clone a repository to start working.",
        ),
        (
            "agent",
            "Run your first agent",
            "Send a prompt in a workspace to start an agent run.",
        ),
        (
            "commit",
            "Make a commit",
            "Stage changes and create a commit from Source Control.",
        ),
        (
            "customize",
            "Customize settings",
            "Pick a theme, shell, and review Git defaults.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_defaults() {
        let g = GitSettings::default();
        assert!(g.auto_rename_branch);
        assert_eq!(g.branch_prefix, BranchPrefix::GitUsername);
        assert_eq!(g.branch_prefix_custom, "");
        assert_eq!(g.jira_casing, JiraCasing::Upper);
        assert_eq!(g.diff_view, DiffView::Inline);
        assert!(!g.diff_word_wrap);
        assert!(!g.diff_show_whitespace);
        assert!(!g.diff_collapse_unchanged);
        assert_eq!(g.sc_view_mode, ScViewMode::List);
        assert_eq!(g.sc_group_order, ScGroupOrder::Status);
        assert!(!g.sc_compare_upstream);
        assert!(!g.follow_symlinks);
        assert!(g.show_ignored);
        assert!(g.pr_bot_overrides.is_empty());
        assert!(g.open_in_apps.is_empty());
        assert!(g.commit_ai_enabled);
        assert!(g.commit_ai_agent.is_none());
    }

    #[test]
    fn general_defaults() {
        let g = GeneralSettings::default();
        assert_eq!(g.workspace_dir, None);
        assert!(g.nest_workspaces);
        assert!(!g.refresh_base_ref_on_create);
        assert!(g.confirm_close_terminal_with_process);
        assert!(g.confirm_delete_worktree);
        assert!(g.open_links_in_app);
        assert!(g.restore_sessions_on_startup);
        assert!(!g.setup_guide_completed);
        assert!(g.setup_guide_dismissed_items.is_empty());
    }

    #[test]
    fn links_open_in_dragent_until_the_user_turns_it_off() {
        // Saved while off was the default: the old key doesn't count.
        let old: GeneralSettings =
            serde_json::from_str(r#"{"open_links_in_app": false}"#).unwrap();
        assert!(old.open_links_in_app);
        let off = GeneralSettings { open_links_in_app: false, ..Default::default() };
        let saved: GeneralSettings =
            serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
        assert!(!saved.open_links_in_app);
    }

    #[test]
    fn branch_prefix_apply_is_idempotent() {
        let mut g = GitSettings::default();
        g.branch_prefix = BranchPrefix::Custom;
        g.branch_prefix_custom = "Ada ".into();
        assert_eq!(g.apply_branch_prefix("fix-login"), "ada/fix-login");
        assert_eq!(g.apply_branch_prefix("ada/fix-login"), "ada/fix-login");
        g.branch_prefix = BranchPrefix::None;
        assert_eq!(g.apply_branch_prefix("fix-login"), "fix-login");
    }

    #[test]
    fn jira_casing_formats_keys() {
        let mut g = GitSettings::default();
        assert_eq!(g.format_jira_key("abc-123"), "ABC-123");
        g.jira_casing = JiraCasing::Lower;
        assert_eq!(g.format_jira_key("ABC-123"), "abc-123");
        g.jira_casing = JiraCasing::Preserve;
        assert_eq!(g.format_jira_key("aBc-123"), "aBc-123");
    }

    #[test]
    fn legacy_json_loads_with_new_defaults() {
        let g: GitSettings = serde_json::from_str("{}").unwrap();
        assert!(g.auto_rename_branch);
        assert_eq!(g.branch_prefix, BranchPrefix::GitUsername);
        assert_eq!(g.diff_view, DiffView::Inline);
        assert!(g.show_ignored);
        assert!(g.commit_ai_enabled);
        let gen: GeneralSettings = serde_json::from_str("{}").unwrap();
        assert!(gen.nest_workspaces);
        assert!(!gen.refresh_base_ref_on_create);
        assert!(gen.restore_sessions_on_startup);
        let repo: RepoSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(repo.issue_source, "auto");
        assert_eq!(repo.order, 0);
    }

    #[test]
    fn settings_round_trip() {
        let mut g = GitSettings::default();
        g.branch_prefix = BranchPrefix::Custom;
        g.branch_prefix_custom = "team".into();
        g.diff_view = DiffView::SideBySide;
        g.pr_bot_overrides = vec!["renovate[bot]".into()];
        g.open_in_apps = vec!["Cursor".into()];
        g.commit_ai_agent = Some("claude".into());
        let json = serde_json::to_string(&g).unwrap();
        let back: GitSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.branch_prefix, BranchPrefix::Custom);
        assert_eq!(back.branch_prefix_custom, "team");
        assert_eq!(back.diff_view, DiffView::SideBySide);
        assert_eq!(back.pr_bot_overrides, vec!["renovate[bot]".to_string()]);
        assert_eq!(back.commit_ai_agent.as_deref(), Some("claude"));

        let mut gen = GeneralSettings::default();
        gen.workspace_dir = Some("/tmp/wt".into());
        gen.dismiss_setup_item("account");
        let json = serde_json::to_string(&gen).unwrap();
        let back: GeneralSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.workspace_dir.as_deref(), Some("/tmp/wt"));
        assert!(back.setup_item_dismissed("account"));
    }

    #[test]
    fn repo_effective_prefers_override() {
        RepoSettingsStore::test_reset();
        let root = Path::new("/tmp/dragent-test-proj");
        RepoSettingsStore::set(
            root,
            RepoSettings {
                base_ref: Some("develop".into()),
                order: 3,
                ..Default::default()
            },
        );
        let eff = RepoSettings::effective(Some(root));
        assert_eq!(eff.base_ref.as_deref(), Some("develop"));
        assert_eq!(eff.order, 3);
        // Empty override falls back to defaults.
        RepoSettingsStore::clear(root);
        let eff = RepoSettings::effective(Some(root));
        assert!(eff.base_ref.is_none());
        RepoSettingsStore::test_reset();
    }
}
