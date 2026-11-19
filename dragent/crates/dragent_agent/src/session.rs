//! Per-request agent session snapshot sent alongside queries.
//!
//! Pure data shared by the engine core, the Host, and the UI. Construction
//! from live models (`from_terminal`) stays app-side — it needs WarpUI model
//! handles and terminal internals.

use std::path::PathBuf;

use super::block::{AttachedBlock, BlockContextSnapshot};
use super::project::ProjectContext;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CodebaseRef {
    pub name: String,
    pub path: PathBuf,
    pub root_hash: Option<String>,
    pub sync_state: String,
}

/// Per-request session snapshot sent alongside the user's query.
#[derive(Clone, Debug)]
pub struct AgentSessionContext {
    pub cwd: PathBuf,
    /// Up to `MAX_RECENT_BLOCKS` completed blocks, newest last.
    pub recent_blocks: Vec<BlockContextSnapshot>,
    /// True when the agent mode was auto-detected (not manually locked).
    pub is_autodetected: bool,
    /// Project context for the current pane — root, git summary, referenced paths.
    pub project: Option<ProjectContext>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub os: String,
    pub shell: String,
    pub codebases: Vec<CodebaseRef>,
    pub attached_blocks: Vec<AttachedBlock>,
    pub skills: Vec<String>,
}

impl AgentSessionContext {
    /// Convenience: find the most recent failed block, if any.
    pub fn last_failed_block(&self) -> Option<&BlockContextSnapshot> {
        self.recent_blocks.iter().rev().find(|b| b.failed)
    }
}
