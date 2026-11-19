use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use diesel::connection::SimpleConnection;
use dragent_session_protocol::workspace::{WorkspaceCommandEnvelope, WorkspaceEventEnvelope};
use uuid::Uuid;

use crate::database::{project_root_for, setup_connection};
use crate::session_store::{
    delete_agent_blocks_for_pane, delete_blocks_for_pane, save_agent_block, save_block,
    save_command_history, save_session_snapshot, trim_agent_conversations_for_pane,
    trim_blocks_for_pane,
};
use crate::types::{
    ModelEvent, PersistedAgentBlock, PersistedBlock, SerializedSession, WorkspaceToolOutcome,
};
use crate::working_copy_store::{delete_working_copy, save_working_copy};
use crate::workspace_store::{
    accept_local_workspace_commands_with_tool_outcome, delete_workspace_project,
    load_pending_workspace_tool_outcomes, mark_workspace_tool_outcome_delivered,
    park_workspace_tool_outcome, park_workspace_tool_outcomes_for_run, save_workspace_event,
    save_workspace_project, save_workspace_work_item,
};

const CHANNEL_SIZE: usize = 1024;
pub const MAX_BLOCKS_PER_PANE: i64 = 100;
/// Distinct past conversations retained per pane before the oldest is
/// dropped whole (never trims mid-conversation — see
/// `trim_agent_conversations_for_pane`).
pub const MAX_CONVERSATIONS_PER_PANE: i64 = 20;

// `ModelEvent` moved to `crate::types` (pure data — wasm needs it without diesel).

/// Open connections held by the writer thread: one main (legacy/global)
/// database plus lazily-opened per-root databases.
pub struct WriterConns {
    pub main: diesel::sqlite::SqliteConnection,
    pub main_path: PathBuf,
    pub roots: HashMap<PathBuf, diesel::sqlite::SqliteConnection>,
    pub org: String,
}

impl WriterConns {
    pub fn new(main_path: PathBuf) -> Result<Self> {
        let mut main = setup_connection(&main_path)?;
        let org = crate::database::connection_organisation(&mut main)?;
        Ok(Self {
            main,
            main_path,
            roots: HashMap::new(),
            org,
        })
    }

    /// Resolve the connection for an optional project root. `None` routes to
    /// the main (legacy/global) database for projectless panes. Canonicalizes
    /// the root and lazily opens its database under ~/.dragent.
    pub fn conn_for_root(
        &mut self,
        root: Option<&PathBuf>,
    ) -> &mut diesel::sqlite::SqliteConnection {
        let Some(root) = root else {
            return &mut self.main;
        };
        // Canonicalize for stable file placement (symlinked checkouts share
        // one file); fall back to the raw path when canonicalization fails.
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let file_root = project_root_for(&canonical).unwrap_or(canonical);
        let db_path =
            match crate::database::database_path_for_root_in_organisation(&file_root, &self.org) {
                Ok(path) => path,
                Err(err) => {
                    log::error!(
                        "persistence resolve per-root {}: {err:#}",
                        file_root.display()
                    );
                    return &mut self.main;
                }
            };
        if !self.roots.contains_key(&db_path) {
            match crate::database::setup_connection_in_organisation(&db_path, &self.org) {
                Ok(conn) => {
                    self.roots.insert(db_path.clone(), conn);
                }
                Err(err) => {
                    log::error!("persistence open per-root {}: {err:#}", db_path.display());
                    return &mut self.main;
                }
            }
        }
        // Re-borrow after potential insert.
        if self.roots.contains_key(&db_path) {
            // Borrow dance: return the root conn.
            // SAFETY: key existence checked above.
            unsafe {
                let ptr = self as *mut Self;
                (*ptr).roots.get_mut(&db_path).unwrap()
            }
        } else {
            &mut self.main
        }
    }

    pub fn checkpoint_all(&mut self) {
        let _ = self.main.batch_execute("PRAGMA wal_checkpoint(PASSIVE);");
        for conn in self.roots.values_mut() {
            let _ = conn.batch_execute("PRAGMA wal_checkpoint(PASSIVE);");
        }
    }

    /// Reconnect recovery: reopen main and drop cached per-root connections
    /// so the next routed write reopens them fresh.
    pub fn recover(&mut self) {
        if let Ok(fresh) = setup_connection(&self.main_path) {
            self.main = fresh;
        }
        // Keep the writer's original organisation; selection never retargets it.
        self.roots.clear();
        // Best-effort: re-run migrations state is per-file on next open.
    }
}

pub struct WriterHandles {
    handle: Option<JoinHandle<()>>,
    sender: SyncSender<ModelEvent>,
    pending_working_copies:
        Arc<Mutex<HashMap<(PathBuf, PathBuf), Option<crate::types::PersistedWorkingCopy>>>>,
}

impl WriterHandles {
    /// Fallback when SQLite initialization fails (writes are no-ops).
    pub fn noop() -> Self {
        let (sender, receiver) = sync_channel(1);
        let handle = thread::Builder::new()
            .name("dragent-sqlite-writer-noop".into())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    match event {
                        ModelEvent::AcceptWorkspaceCommand { reply, .. } => {
                            let _ = reply
                                .send(Err("local workspace persistence is unavailable".into()));
                        }
                        ModelEvent::LoadPendingWorkspaceToolOutcomes { reply } => {
                            let _ = reply
                                .send(Err("local workspace persistence is unavailable".into()));
                        }
                        ModelEvent::MarkWorkspaceToolOutcomeDelivered { reply, .. }
                        | ModelEvent::ParkWorkspaceToolOutcome { reply, .. } => {
                            let _ = reply
                                .send(Err("local workspace persistence is unavailable".into()));
                        }
                        ModelEvent::ParkWorkspaceToolOutcomesForRun { reply, .. } => {
                            let _ = reply
                                .send(Err("local workspace persistence is unavailable".into()));
                        }
                        _ => {}
                    }
                }
            })
            .expect("spawn noop writer");
        Self {
            handle: Some(handle),
            sender,
            pending_working_copies: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn sender(&self) -> SyncSender<ModelEvent> {
        self.sender.clone()
    }

    /// Lightweight handle for views — shares the writer thread, no join on drop.
    pub fn clone_for_workspace(&self) -> Self {
        Self {
            handle: None,
            sender: self.sender.clone(),
            pending_working_copies: self.pending_working_copies.clone(),
        }
    }

    pub fn send(&self, event: ModelEvent) {
        let _ = self.sender.send(event);
    }

    pub fn snapshot(&self, session: SerializedSession) {
        self.send(ModelEvent::Snapshot(session));
    }

    pub fn save_window_frame(&self, frame: crate::types::SerializedWindowFrame) {
        self.send(ModelEvent::SaveWindowFrame(frame));
    }

    pub fn save_block(
        &self,
        pane_uuid: Uuid,
        block: PersistedBlock,
        sort_order: i32,
        project_root: Option<PathBuf>,
    ) {
        self.send(ModelEvent::SaveBlock {
            pane_uuid,
            block,
            sort_order,
            project_root,
        });
    }

    pub fn save_agent_block(
        &self,
        pane_uuid: Uuid,
        block: PersistedAgentBlock,
        sort_order: i32,
        project_root: Option<PathBuf>,
    ) {
        self.send(ModelEvent::SaveAgentBlock {
            pane_uuid,
            block,
            sort_order,
            project_root,
        });
    }

    pub fn clear_pane_data(&self, pane_uuid: Uuid, project_root: Option<PathBuf>) {
        self.send(ModelEvent::ClearPaneData {
            pane_uuid,
            project_root,
        });
    }

    pub fn clear_agent_blocks(&self, pane_uuid: Uuid, project_root: Option<PathBuf>) {
        self.send(ModelEvent::ClearAgentBlocks {
            pane_uuid,
            project_root,
        });
    }

    /// Record a run command into the global, never-cleared history table.
    pub fn record_command(&self, command: String, at_unix_ms: i64) {
        self.send(ModelEvent::RecordCommand {
            command,
            at_unix_ms,
        });
    }

    pub fn save_workspace_project(&self, project: dragent_workspace_model::Project) {
        self.send(ModelEvent::SaveWorkspaceProject(project));
    }

    pub fn delete_workspace_project(&self, project_id: dragent_workspace_model::ProjectId) {
        self.send(ModelEvent::DeleteWorkspaceProject(project_id));
    }

    /// Delete a removed root's private data directory
    /// ([`dragent_paths::workspace`]) after the writes queued before it.
    pub fn forget_project_data(&self, data_dir: PathBuf) {
        self.send(ModelEvent::ForgetProjectData(data_dir));
    }

    pub fn save_workspace_work_item(&self, work_item: dragent_workspace_model::WorkItem) {
        self.send(ModelEvent::SaveWorkspaceWorkItem(work_item));
    }

    pub fn append_workspace_event(&self, event: dragent_workspace_model::WorkspaceEvent) {
        self.send(ModelEvent::AppendWorkspaceEvent(event));
    }

    /// Submit through the serialized SQLite authority. Call this from a
    /// background task because it waits for the durable transaction outcome.
    pub fn accept_workspace_command(
        &self,
        command: WorkspaceCommandEnvelope,
    ) -> Result<Vec<WorkspaceEventEnvelope>> {
        self.accept_workspace_commands(vec![command])
    }

    pub fn accept_workspace_commands(
        &self,
        commands: Vec<WorkspaceCommandEnvelope>,
    ) -> Result<Vec<WorkspaceEventEnvelope>> {
        self.accept_workspace_commands_with_tool_outcome(commands, None)
    }

    pub fn accept_workspace_commands_with_tool_outcome(
        &self,
        commands: Vec<WorkspaceCommandEnvelope>,
        tool_outcome: Option<WorkspaceToolOutcome>,
    ) -> Result<Vec<WorkspaceEventEnvelope>> {
        let (reply, outcome) = sync_channel(1);
        self.sender
            .send(ModelEvent::AcceptWorkspaceCommand {
                commands,
                tool_outcome,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("workspace persistence writer is unavailable"))?;
        outcome
            .recv()
            .map_err(|_| anyhow::anyhow!("workspace persistence writer stopped before replying"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn load_pending_workspace_tool_outcomes(&self) -> Result<Vec<WorkspaceToolOutcome>> {
        let (reply, outcome) = sync_channel(1);
        self.sender
            .send(ModelEvent::LoadPendingWorkspaceToolOutcomes { reply })
            .map_err(|_| anyhow::anyhow!("workspace persistence writer is unavailable"))?;
        outcome
            .recv()
            .map_err(|_| anyhow::anyhow!("workspace persistence writer stopped before replying"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn mark_workspace_tool_outcome_delivered(
        &self,
        command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
        runtime_execution_id: String,
        provider_call_id: String,
        delivered_at_unix_ms: i64,
    ) -> Result<bool> {
        let (reply, outcome) = sync_channel(1);
        self.sender
            .send(ModelEvent::MarkWorkspaceToolOutcomeDelivered {
                command_id,
                runtime_execution_id,
                provider_call_id,
                delivered_at_unix_ms,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("workspace persistence writer is unavailable"))?;
        outcome
            .recv()
            .map_err(|_| anyhow::anyhow!("workspace persistence writer stopped before replying"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn park_workspace_tool_outcome(
        &self,
        command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
        runtime_execution_id: String,
        reason: String,
    ) -> Result<bool> {
        let (reply, outcome) = sync_channel(1);
        self.sender
            .send(ModelEvent::ParkWorkspaceToolOutcome {
                command_id,
                runtime_execution_id,
                reason,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("workspace persistence writer is unavailable"))?;
        outcome
            .recv()
            .map_err(|_| anyhow::anyhow!("workspace persistence writer stopped before replying"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn park_workspace_tool_outcomes_for_run(
        &self,
        runtime_run_id: String,
        runtime_execution_id: String,
        reason: String,
    ) -> Result<usize> {
        let (reply, outcome) = sync_channel(1);
        self.sender
            .send(ModelEvent::ParkWorkspaceToolOutcomesForRun {
                runtime_run_id,
                runtime_execution_id,
                reason,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("workspace persistence writer is unavailable"))?;
        outcome
            .recv()
            .map_err(|_| anyhow::anyhow!("workspace persistence writer stopped before replying"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn save_working_copy(&self, copy: crate::types::PersistedWorkingCopy) {
        self.send(ModelEvent::SaveWorkingCopy(copy));
    }

    pub fn delete_working_copy(&self, workspace_root: PathBuf, relative_path: PathBuf) {
        self.stage_working_copy_delete(workspace_root.clone(), relative_path.clone());
        self.send(ModelEvent::DeleteWorkingCopy {
            workspace_root,
            relative_path,
        });
    }

    /// Update the process-local recovery slot immediately without doing disk
    /// I/O on the UI thread. A debounce timer normally drains it; shutdown also
    /// drains it before its writer barrier, closing the type-and-quit race.
    pub fn stage_working_copy(&self, copy: crate::types::PersistedWorkingCopy) {
        if let Ok(mut pending) = self.pending_working_copies.lock() {
            pending.insert(
                (copy.workspace_root.clone(), copy.relative_path.clone()),
                Some(copy),
            );
        }
    }

    pub fn stage_working_copy_delete(&self, workspace_root: PathBuf, relative_path: PathBuf) {
        if let Ok(mut pending) = self.pending_working_copies.lock() {
            pending.insert((workspace_root, relative_path), None);
        }
    }

    pub fn flush_staged_working_copies(&self) {
        let staged = self
            .pending_working_copies
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default();
        for ((workspace_root, relative_path), copy) in staged {
            match copy {
                Some(copy) => self.send(ModelEvent::SaveWorkingCopy(copy)),
                None => self.send(ModelEvent::DeleteWorkingCopy {
                    workspace_root,
                    relative_path,
                }),
            }
        }
    }

    pub fn flush(&self) {
        self.flush_staged_working_copies();
        let (done, acknowledged) = sync_channel(0);
        if self.sender.send(ModelEvent::Flush { done }).is_ok() {
            let _ = acknowledged.recv();
        }
    }
}

impl Drop for WriterHandles {
    fn drop(&mut self) {
        if self.handle.take().is_some() {
            let _ = self.sender.send(ModelEvent::Terminate);
        }
    }
}

pub fn start_writer(database_path: PathBuf) -> Result<WriterHandles> {
    let conns = WriterConns::new(database_path.clone())?;
    let (sender, receiver): (SyncSender<ModelEvent>, Receiver<ModelEvent>) =
        sync_channel(CHANNEL_SIZE);

    let handle = thread::Builder::new()
        .name("dragent-sqlite-writer".into())
        .spawn(move || writer_loop(conns, receiver))?;

    Ok(WriterHandles {
        handle: Some(handle),
        sender,
        pending_working_copies: Arc::new(Mutex::new(HashMap::new())),
    })
}

fn writer_loop(mut conns: WriterConns, receiver: Receiver<ModelEvent>) {
    loop {
        match receiver.recv() {
            Ok(ModelEvent::Terminate) => break,
            Ok(first) => {
                let mut batch = vec![first];
                batch.extend(receiver.try_iter());
                let deduped = deduplicate_events(batch);
                for event in deduped {
                    if let Err(err) = dispatch_event(&mut conns, event) {
                        log::error!("persistence write error: {err:#}");
                        conns.recover();
                    }
                }
            }
            Err(_) => break,
        }
    }
}

fn deduplicate_events(events: Vec<ModelEvent>) -> Vec<ModelEvent> {
    let mut result = Vec::with_capacity(events.len());
    let mut last_snapshot_idx = None;
    for (idx, event) in events.iter().enumerate() {
        if matches!(event, ModelEvent::Snapshot(_)) {
            last_snapshot_idx = Some(idx);
        }
    }
    let mut latest_working_copy = std::collections::HashMap::new();
    for (idx, event) in events.iter().enumerate() {
        match event {
            ModelEvent::SaveWorkingCopy(copy) => {
                latest_working_copy.insert(
                    (copy.workspace_root.clone(), copy.relative_path.clone()),
                    idx,
                );
            }
            ModelEvent::DeleteWorkingCopy {
                workspace_root,
                relative_path,
            } => {
                latest_working_copy.insert((workspace_root.clone(), relative_path.clone()), idx);
            }
            _ => {}
        }
    }
    for (idx, event) in events.into_iter().enumerate() {
        if let ModelEvent::Snapshot(_) = &event {
            if last_snapshot_idx != Some(idx) {
                continue;
            }
        }
        let working_copy_key = match &event {
            ModelEvent::SaveWorkingCopy(copy) => {
                Some((copy.workspace_root.clone(), copy.relative_path.clone()))
            }
            ModelEvent::DeleteWorkingCopy {
                workspace_root,
                relative_path,
            } => Some((workspace_root.clone(), relative_path.clone())),
            _ => None,
        };
        if working_copy_key
            .as_ref()
            .is_some_and(|key| latest_working_copy.get(key) != Some(&idx))
        {
            continue;
        }
        result.push(event);
    }
    result
}

fn dispatch_event(conns: &mut WriterConns, event: ModelEvent) -> Result<()> {
    match event {
        ModelEvent::Snapshot(session) => save_session_snapshot(&mut conns.main, &session),
        ModelEvent::RecordLocalWorkspace(workspace) => {
            crate::workspace_catalogue::record_local_workspace(&mut conns.main, &workspace)
        }
        ModelEvent::CloseLocalWorkspace(root) => {
            crate::workspace_catalogue::close_local_workspace(&mut conns.main, &root)
        }
        ModelEvent::SaveWindowFrame(frame) => {
            crate::session_store::save_window_frame(&mut conns.main, &frame)
        }
        ModelEvent::SaveBlock {
            pane_uuid,
            block,
            sort_order,
            project_root,
        } => {
            // Route pane blocks by root; None stays on main (legacy global
            // for projectless panes).
            let root_owned = project_root.clone();
            let conn = conns.conn_for_root(root_owned.as_ref());
            save_block(conn, pane_uuid, &block, sort_order)?;
            trim_blocks_for_pane(conn, pane_uuid, MAX_BLOCKS_PER_PANE)?;
            Ok(())
        }
        ModelEvent::SaveAgentBlock {
            pane_uuid,
            block,
            sort_order,
            project_root,
        } => {
            let root_owned = project_root.clone();
            let conn = conns.conn_for_root(root_owned.as_ref());
            save_agent_block(conn, pane_uuid, &block, sort_order)?;
            trim_agent_conversations_for_pane(conn, pane_uuid, MAX_CONVERSATIONS_PER_PANE)?;
            Ok(())
        }
        ModelEvent::RecordCommand {
            command,
            at_unix_ms,
        } => save_command_history(&mut conns.main, &command, at_unix_ms),
        ModelEvent::ClearPaneData {
            pane_uuid,
            project_root,
        } => {
            let root_owned = project_root.clone();
            let conn = conns.conn_for_root(root_owned.as_ref());
            delete_blocks_for_pane(conn, pane_uuid)
        }
        ModelEvent::ClearAgentBlocks {
            pane_uuid,
            project_root,
        } => {
            let root_owned = project_root.clone();
            let conn = conns.conn_for_root(root_owned.as_ref());
            delete_agent_blocks_for_pane(conn, pane_uuid)
        }
        ModelEvent::UpdateClearGap {
            pane_uuid,
            height_px,
            before_block_id,
            after_all,
        } => crate::session_store::update_clear_gap(
            &mut conns.main,
            pane_uuid,
            height_px,
            before_block_id,
            after_all,
        ),
        ModelEvent::SaveWorkspaceProject(project) => {
            save_workspace_project(&mut conns.main, &project)
        }
        ModelEvent::DeleteWorkspaceProject(project_id) => {
            delete_workspace_project(&mut conns.main, project_id)
        }
        ModelEvent::ForgetProjectData(data_dir) => {
            // An open SQLite handle would keep its file (and the folder) on Windows.
            conns.roots.retain(|db_path, _| !db_path.starts_with(&data_dir));
            dragent_paths::remove_data_dir(&data_dir)
                .with_context(|| format!("delete project data {}", data_dir.display()))
        }
        ModelEvent::SaveWorkspaceWorkItem(work_item) => {
            save_workspace_work_item(&mut conns.main, &work_item)
        }
        ModelEvent::AppendWorkspaceEvent(event) => save_workspace_event(&mut conns.main, &event),
        ModelEvent::AcceptWorkspaceCommand {
            commands,
            tool_outcome,
            reply,
        } => {
            match accept_local_workspace_commands_with_tool_outcome(
                &mut conns.main,
                &commands,
                tool_outcome.as_ref(),
            ) {
                Ok(events) => {
                    let _ = reply.send(Ok(events));
                    Ok(())
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    let _ = reply.send(Err(message.clone()));
                    Err(anyhow::anyhow!(message))
                }
            }
        }
        ModelEvent::LoadPendingWorkspaceToolOutcomes { reply } => {
            let result = load_pending_workspace_tool_outcomes(&mut conns.main)
                .map_err(|error| format!("{error:#}"));
            let dispatch = result
                .as_ref()
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!(error.clone()));
            let _ = reply.send(result);
            dispatch
        }
        ModelEvent::MarkWorkspaceToolOutcomeDelivered {
            command_id,
            runtime_execution_id,
            provider_call_id,
            delivered_at_unix_ms,
            reply,
        } => {
            let result = mark_workspace_tool_outcome_delivered(
                &mut conns.main,
                command_id,
                &runtime_execution_id,
                &provider_call_id,
                delivered_at_unix_ms,
            )
            .map_err(|error| format!("{error:#}"));
            let dispatch = result
                .as_ref()
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!(error.clone()));
            let _ = reply.send(result);
            dispatch
        }
        ModelEvent::ParkWorkspaceToolOutcome {
            command_id,
            runtime_execution_id,
            reason,
            reply,
        } => {
            let result = park_workspace_tool_outcome(
                &mut conns.main,
                command_id,
                &runtime_execution_id,
                &reason,
            )
            .map_err(|error| format!("{error:#}"));
            let dispatch = result
                .as_ref()
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!(error.clone()));
            let _ = reply.send(result);
            dispatch
        }
        ModelEvent::ParkWorkspaceToolOutcomesForRun {
            runtime_run_id,
            runtime_execution_id,
            reason,
            reply,
        } => {
            let result = park_workspace_tool_outcomes_for_run(
                &mut conns.main,
                &runtime_run_id,
                &runtime_execution_id,
                &reason,
            )
            .map_err(|error| format!("{error:#}"));
            let dispatch = result
                .as_ref()
                .map(|_| ())
                .map_err(|error| anyhow::anyhow!(error.clone()));
            let _ = reply.send(result);
            dispatch
        }
        ModelEvent::SaveWorkingCopy(copy) => save_working_copy(&mut conns.main, &copy),
        ModelEvent::DeleteWorkingCopy {
            workspace_root,
            relative_path,
        } => delete_working_copy(&mut conns.main, &workspace_root, &relative_path),
        ModelEvent::Flush { done } => {
            // Keep every WAL bounded and guarantee a normal exit leaves
            // committed state in each database file.
            conns.checkpoint_all();
            let _ = done.send(());
            Ok(())
        }
        ModelEvent::Terminate => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandId, WorkspaceCommandPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_workspace_model::{
        ActorId, AgentMessage, AgentMessageId, AgentMessageKind, MessageDeliveryState, Project,
        ThreadId, WorkItem, WorkItemId,
    };
    use tempfile::tempdir;

    #[test]
    fn writer_returns_the_durable_canonical_command_outcome() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("writer-authority.sqlite");
        let mut connection = setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work = WorkItem::new(project.id, "Billing", 2);
        crate::workspace_store::save_workspace_project(&mut connection, &project).unwrap();
        crate::workspace_store::save_workspace_work_item(&mut connection, &work).unwrap();
        drop(connection);

        let actor_id = ActorId::new();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id: work.id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 3,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: ThreadId::new(),
                    work_item_id: work.id,
                    sender_id: actor_id,
                    kind: AgentMessageKind::Discussion,
                    body: "Use seat billing".into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 3,
                    updated_at_unix_ms: 3,
                },
                routing: None,
            },
        };
        let writer = start_writer(path).unwrap();
        let accepted = writer.accept_workspace_command(command.clone()).unwrap();
        let retried = writer.accept_workspace_command(command).unwrap();

        assert_eq!(accepted, retried);
        assert_eq!(accepted[0].sequence, 1);
    }

    #[test]
    fn queued_project_is_persisted_before_first_work_item_command() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("new-folder-workspace.sqlite");
        let project = Project::new(directory.path().join("new-project"), 1);
        let work_item_id = WorkItemId::new();
        let writer = start_writer(path).unwrap();

        // This is the ordering used when Open Folder activates a fresh project
        // and the user immediately sends the first Principal message.
        writer.save_workspace_project(project.clone());
        let accepted = writer
            .accept_workspace_command(WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: ActorId::new(),
                device_id: None,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: 2,
                expected_revision: None,
                payload: WorkspaceCommandPayload::CreateWorkItem {
                    title: "First folder task".into(),
                    execution_location: dragent_workspace_model::ExecutionLocation::Local,
                },
            })
            .unwrap();

        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].project_id, project.id);
        assert_eq!(accepted[0].work_item_id, work_item_id);
        assert_eq!(accepted[0].sequence, 1);
    }
}
