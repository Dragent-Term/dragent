//! Wasm stubs for the diesel/SQLite persistence backend.
//!
//! The web viewer has no local database: it renders a live remote session and
//! never restores or writes history. These functions return empty results and
//! the writer is a no-op sink, so the app's persistence call sites type-check
//! and run harmlessly on wasm.
#![allow(dead_code, unused_variables)]

use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};

use anyhow::Result;
use uuid::Uuid;

use crate::types::{
    ConversationSummary, ModelEvent, PersistedAgentBlock, PersistedBlock, PersistedSession,
    SerializedSession, SerializedWindowFrame, WorkspaceIntegrationIntent, WorkspaceToolOutcome,
};
use dragent_session_protocol::workspace::{WorkspaceCommandEnvelope, WorkspaceEventEnvelope};
use dragent_workspace_model::{ExecutionAttemptId, Project, WorkItem, WorkspaceEvent};

/// Opaque stand-in for `diesel::SqliteConnection`. The app only ever holds it as
/// `let mut conn = setup_connection(..)?` and passes `&mut conn` onward, so it is
/// never named explicitly — an empty struct suffices.
pub struct SqliteConnection {
    _private: (),
}

pub fn database_path() -> Result<PathBuf> {
    // No filesystem database on wasm.
    Ok(PathBuf::new())
}

pub const PROJECT_DIR_NAME: &str = ".dragent";
pub const PROJECT_IGNORE_LINE: &str = ".dragent/";

pub fn sanitize_org_id(_raw: &str) -> String {
    "local".to_string()
}

pub fn current_org() -> String {
    "local".to_string()
}

pub fn db_file_for_root(root: &std::path::Path, org: &str) -> PathBuf {
    dragent_paths::workspace(root).join(format!("{org}.sqlite"))
}

pub fn database_path_for_root(_root: &std::path::Path) -> Result<PathBuf> {
    Ok(PathBuf::new())
}

pub fn project_root_for(_path: &std::path::Path) -> Option<PathBuf> {
    None
}

pub fn db_path_for_read(_root: &std::path::Path) -> Result<PathBuf> {
    Ok(PathBuf::new())
}

pub fn db_path_for_write(_root: &std::path::Path) -> Result<PathBuf> {
    Ok(PathBuf::new())
}

pub fn ensure_repo_gitignore(_root: &std::path::Path) -> Result<()> {
    Ok(())
}

pub fn migrate_pane_conns(
    _legacy: &mut SqliteConnection,
    _target: &mut SqliteConnection,
    _pane: Uuid,
) -> Result<usize> {
    Ok(0)
}

pub fn migrate_pane_to_root(_root: &std::path::Path, _org: &str, _pane: Uuid) -> Result<usize> {
    Ok(0)
}

pub fn restore_pane_blocks(
    _cwd: &std::path::Path,
    _org: &str,
    _pane: Uuid,
) -> Option<(Vec<PersistedBlock>, Vec<PersistedAgentBlock>)> {
    None
}

pub fn load_blocks_for_pane(
    _conn: &mut SqliteConnection,
    _pane: Uuid,
) -> Result<Vec<PersistedBlock>> {
    Ok(Vec::new())
}

pub fn load_agent_blocks_for_pane(
    _conn: &mut SqliteConnection,
    _pane: Uuid,
) -> Result<Vec<PersistedAgentBlock>> {
    Ok(Vec::new())
}

pub fn pane_has_rows(_conn: &mut SqliteConnection, _pane: Uuid) -> Result<bool> {
    Ok(false)
}

pub fn copy_pane_blocks(
    _source: &mut SqliteConnection,
    _target: &mut SqliteConnection,
    _pane: Uuid,
) -> Result<usize> {
    Ok(0)
}

pub struct WriterConns {
    _private: (),
}

pub fn setup_connection(_path: &PathBuf) -> Result<SqliteConnection> {
    // Hand back an inert connection; every query below returns empty.
    Ok(SqliteConnection { _private: () })
}

pub fn load_pending_workspace_integration_intents(
    _conn: &mut SqliteConnection,
) -> Result<Vec<WorkspaceIntegrationIntent>> {
    Ok(Vec::new())
}

pub fn block_workspace_integration(
    _conn: &mut SqliteConnection,
    _attempt_id: ExecutionAttemptId,
    _reason: &str,
) -> Result<bool> {
    Ok(false)
}

pub fn claim_workspace_integration(
    _conn: &mut SqliteConnection,
    _attempt_id: ExecutionAttemptId,
    _owner: &str,
    _now_unix_ms: i64,
    _lease_duration_ms: i64,
) -> Result<Option<WorkspaceIntegrationIntent>> {
    Ok(None)
}

pub fn release_workspace_integration_claim(
    _conn: &mut SqliteConnection,
    _attempt_id: ExecutionAttemptId,
    _owner: &str,
) -> Result<bool> {
    Ok(false)
}

pub fn block_claimed_workspace_integration(
    _conn: &mut SqliteConnection,
    _attempt_id: ExecutionAttemptId,
    _owner: &str,
    _reason: &str,
) -> Result<bool> {
    Ok(false)
}

pub fn max_sort_order_for_pane(
    _conn: &mut SqliteConnection,
    _pane_uuid: Uuid,
) -> Result<Option<i32>> {
    Ok(None)
}

pub fn recent_commands(_conn: &mut SqliteConnection, _limit: usize) -> Result<Vec<(String, i64)>> {
    Ok(Vec::new())
}

pub fn load_conversation(
    _conn: &mut SqliteConnection,
    _pane_uuid: Uuid,
    _conversation_id: Uuid,
) -> Result<Vec<PersistedAgentBlock>> {
    Ok(Vec::new())
}

pub fn conversation_summaries_for_pane(
    _conn: &mut SqliteConnection,
    _pane_uuid: Uuid,
    _limit: i64,
) -> Result<Vec<ConversationSummary>> {
    Ok(Vec::new())
}

pub fn warm_in_background() {}

pub fn loaded_session() -> Option<PersistedSession> {
    None
}

pub fn loaded_local_workspaces() -> Vec<crate::types::SerializedWorkspace> {
    Vec::new()
}

pub fn current_state() -> (Option<PersistedSession>, Vec<crate::types::SerializedWorkspace>) {
    (None, Vec::new())
}

pub fn loaded_window_frame() -> Option<SerializedWindowFrame> {
    None
}

pub fn shared_writer() -> WriterHandles {
    WriterHandles::noop()
}

pub fn flush_session_on_close() {}

/// No-op mirror of `writer::WriterHandles`. Holds a sender whose receiver is
/// dropped, so every queued event is silently discarded.
#[derive(Clone)]
pub struct WriterHandles {
    sender: SyncSender<ModelEvent>,
}

impl WriterHandles {
    pub fn noop() -> Self {
        let (sender, _rx) = sync_channel(1);
        Self { sender }
    }

    pub fn sender(&self) -> SyncSender<ModelEvent> {
        self.sender.clone()
    }

    pub fn clone_for_workspace(&self) -> Self {
        self.clone()
    }

    pub fn send(&self, _event: ModelEvent) {}
    pub fn snapshot(&self, _session: SerializedSession) {}
    pub fn save_window_frame(&self, _frame: SerializedWindowFrame) {}
    pub fn save_block(
        &self,
        _pane_uuid: Uuid,
        _block: PersistedBlock,
        _sort_order: i32,
        _project_root: Option<PathBuf>,
    ) {
    }
    pub fn save_agent_block(
        &self,
        _pane_uuid: Uuid,
        _block: PersistedAgentBlock,
        _sort_order: i32,
        _project_root: Option<PathBuf>,
    ) {
    }
    pub fn clear_pane_data(&self, _pane_uuid: Uuid, _project_root: Option<PathBuf>) {}
    pub fn clear_agent_blocks(&self, _pane_uuid: Uuid, _project_root: Option<PathBuf>) {}
    pub fn record_command(&self, _command: String, _at_unix_ms: i64) {}
    pub fn save_workspace_project(&self, _project: Project) {}
    pub fn delete_workspace_project(&self, _project_id: dragent_workspace_model::ProjectId) {}
    pub fn forget_project_data(&self, _data_dir: PathBuf) {}
    pub fn save_workspace_work_item(&self, _work_item: WorkItem) {}
    pub fn append_workspace_event(&self, _event: WorkspaceEvent) {}
    pub fn accept_workspace_command(
        &self,
        _command: WorkspaceCommandEnvelope,
    ) -> Result<Vec<WorkspaceEventEnvelope>> {
        anyhow::bail!("local workspace persistence is unavailable in the web viewer")
    }
    pub fn save_working_copy(&self, _copy: crate::types::PersistedWorkingCopy) {}
    pub fn accept_workspace_commands(
        &self,
        _commands: Vec<WorkspaceCommandEnvelope>,
    ) -> Result<Vec<WorkspaceEventEnvelope>> {
        anyhow::bail!("local workspace persistence is unavailable in the web viewer")
    }
    pub fn accept_workspace_commands_with_tool_outcome(
        &self,
        _commands: Vec<WorkspaceCommandEnvelope>,
        _tool_outcome: Option<WorkspaceToolOutcome>,
    ) -> Result<Vec<WorkspaceEventEnvelope>> {
        anyhow::bail!("local workspace persistence is unavailable in the web viewer")
    }
    pub fn load_pending_workspace_tool_outcomes(&self) -> Result<Vec<WorkspaceToolOutcome>> {
        Ok(Vec::new())
    }
    pub fn mark_workspace_tool_outcome_delivered(
        &self,
        _command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
        _runtime_execution_id: String,
        _provider_call_id: String,
        _delivered_at_unix_ms: i64,
    ) -> Result<bool> {
        Ok(false)
    }
    pub fn park_workspace_tool_outcome(
        &self,
        _command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
        _runtime_execution_id: String,
        _reason: String,
    ) -> Result<bool> {
        Ok(false)
    }
    pub fn park_workspace_tool_outcomes_for_run(
        &self,
        _runtime_run_id: String,
        _runtime_execution_id: String,
        _reason: String,
    ) -> Result<usize> {
        Ok(0)
    }
    pub fn delete_working_copy(&self, _workspace_root: PathBuf, _relative_path: PathBuf) {}
    pub fn stage_working_copy(&self, _copy: crate::types::PersistedWorkingCopy) {}
    pub fn stage_working_copy_delete(&self, _workspace_root: PathBuf, _relative_path: PathBuf) {}
    pub fn flush_staged_working_copies(&self) {}
    pub fn flush(&self) {}
}
