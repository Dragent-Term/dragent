//! Runtime + compile-time gates for staged git gaps (B scaffolding).
//!
//! Cargo features are default-off so `cargo build` remains no-op.
//! Enable with `--features git-gaps` or individual `git-*` features.
//! Runtime env `DRAGENT_GIT_GAPS=1` can also force-enable for testing.

pub const FLAG_HUNK_STAGE: bool = cfg!(feature = "git-hunk-stage");
pub const FLAG_WORKTREE_MANAGER: bool = cfg!(feature = "git-worktree-manager");
pub const FLAG_MULTI_DIFF_EDITOR: bool = cfg!(feature = "git-multi-diff-editor");
pub const FLAG_GRAPH_HISTORY: bool = cfg!(feature = "git-graph-history");

/// Runtime override: set DRAGENT_GIT_GAPS=1 to enable all gaps even without cargo features.
pub fn runtime_override_enabled() -> bool {
    std::env::var("DRAGENT_GIT_GAPS").as_deref() == Ok("1")
}

pub fn hunk_stage_enabled() -> bool {
    FLAG_HUNK_STAGE || runtime_override_enabled()
}
pub fn worktree_manager_enabled() -> bool {
    FLAG_WORKTREE_MANAGER || runtime_override_enabled()
}
pub fn multi_diff_editor_enabled() -> bool {
    FLAG_MULTI_DIFF_EDITOR || runtime_override_enabled()
}
pub fn graph_history_enabled() -> bool {
    FLAG_GRAPH_HISTORY || runtime_override_enabled()
}
