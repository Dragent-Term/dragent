use std::path::PathBuf;

use dragent_session_protocol::workspace::{
    WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceEventEnvelope,
};
use dragent_workspace_model::{
    LayoutCollection, Project, ProjectId, WorkItem, WorkItemId, WorkspaceEvent,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Serializable terminal output for block persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTextSpan {
    pub text: String,
    pub color: Option<[u8; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedColoredLine {
    pub spans: Vec<PersistedTextSpan>,
    /// Soft-wrapped into the next line. Defaulted so sessions saved before
    /// this field existed still load.
    #[serde(default)]
    pub wrapped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PersistedRunStatus {
    Running,
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedBlock {
    pub command: String,
    pub cwd: PathBuf,
    pub lines: Vec<PersistedColoredLine>,
    pub status: PersistedRunStatus,
    pub exit_code: Option<i32>,
    pub started_at_unix_ms: Option<i64>,
    pub duration_ms: Option<u64>,
    pub expanded: bool,
    pub bookmarked: bool,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq)]
/// Split orientation, fixed project-wide: Horizontal arranges children
/// left-to-right (`A | B`), Vertical stacks them top-to-bottom.
/// (Stored rows predating this convention swap on read in `session_store`.)
pub enum SerializedSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SerializedPaneLayout {
    Single {
        uuid: Uuid,
        cwd: PathBuf,
        shell_title: Option<String>,
        is_active: bool,
        #[serde(default)]
        clear_gap_height_px: f32,
        #[serde(default)]
        clear_gap_before_block_id: Option<i32>,
        #[serde(default)]
        clear_gap_after_all: bool,
        /// Stable agent-registry identity for this session (`"claude"`,
        /// `"dragent"`, `"custom:<slug>"`, …). Restored onto the pane so
        /// tabs, rail, and input editor resolve the definition without
        /// re-detecting it. Absent for plain shells and older sessions.
        #[serde(default)]
        agent_id: Option<String>,
        /// Set for a browser pane: the address it shows. Such a leaf has no
        /// shell; `cwd` holds its project root (empty outside a project).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        browser_url: Option<String>,
    },
    Split {
        direction: SerializedSplitDirection,
        left_or_top: Box<SerializedPaneLayout>,
        right_or_bottom: Box<SerializedPaneLayout>,
        size: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedTab {
    pub id: usize,
    pub title: String,
    #[serde(default)]
    pub custom_label: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub root_layout: SerializedPaneLayout,
    #[serde(default)]
    pub hidden_from_sidebar: bool,
    /// Stable identity of this tab's own pane. A split host stores the whole
    /// tree in `root_layout`, so this field is what maps its tab metadata back
    /// to the correct leaf during restore.
    #[serde(default)]
    pub owner_pane_uuid: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowChrome {
    #[serde(default = "default_true")]
    pub left_panel_open: bool,
    pub left_panel_width: Option<f32>,
    #[serde(default = "default_true")]
    pub right_panel_open: bool,
    pub right_panel_width: Option<f32>,
}

/// Last editor/workspace surface state. File contents are not stored here;
/// unsaved contents live in [`PersistedWorkingCopy`] rows so they can be
/// independently saved or discarded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedDocumentSession {
    /// Owner of projects, workspaces, tabs, and pane bindings in this snapshot.
    /// Snapshots without this field are legacy local data.
    #[serde(default = "dragent_paths::local_organisation_id")]
    pub organisation_id: String,
    #[serde(default)]
    pub root: PathBuf,
    /// Projects remain open independently of terminal tabs. Closing the final
    /// shell must not silently remove its project from the workspace.
    #[serde(default)]
    pub open_project_roots: Vec<PathBuf>,
    #[serde(default)]
    pub selected_project_root: Option<PathBuf>,
    /// The selected-project composer was the active surface at shutdown.
    #[serde(default)]
    pub project_start_visible: bool,
    #[serde(default)]
    pub prompt_text: String,
    #[serde(default)]
    pub prompt_links_json: String,
    #[serde(default)]
    pub prompt_agent: Option<String>,
    #[serde(default)]
    pub prompt_selected_worktree: Option<PathBuf>,
    #[serde(default)]
    pub prompt_branch: Option<String>,
    #[serde(default)]
    pub center_open: bool,
    #[serde(default)]
    pub owner_tab_id: Option<usize>,
    #[serde(default)]
    pub active_relative_path: Option<PathBuf>,
    /// Lightweight open-file order. Contents remain in files/working copies.
    #[serde(default)]
    pub open_relative_paths: Vec<PathBuf>,
    /// Client-local first-class work-item editor tab state. Canonical work
    /// content remains in workspace events; this stores presentation only.
    #[serde(default)]
    pub work_item_open: bool,
    #[serde(default)]
    pub work_item_active: bool,
    #[serde(default)]
    pub work_item_view: Option<String>,
    /// Work-item presentation owned by each top-level workspace tab. This is
    /// additive to the legacy singleton fields above so older sessions remain
    /// readable while newly created tabs never alias the same work item.
    #[serde(default)]
    pub workspace_tab_work_items: Vec<SerializedWorkspaceTabWorkItem>,
    #[serde(default)]
    pub expanded_directories: Vec<PathBuf>,
    #[serde(default)]
    pub dock_visible: bool,
    #[serde(default)]
    pub dock_height: Option<f32>,
    #[serde(default)]
    pub dock_maximized: bool,
    #[serde(default)]
    pub dock_active_id: Option<usize>,
    #[serde(default)]
    pub dock_split_id: Option<usize>,
    #[serde(default)]
    pub dock_next_id: usize,
    #[serde(default)]
    pub dock_terminals: Vec<SerializedDockedTerminal>,
    /// Host-owned terminal tabs: daemon session binding per pane uuid.
    /// Relaunch re-attaches live sessions (output replays from the host
    /// ring) instead of spawning fresh shells. Empty for sessions written
    /// before host-owned tabs existed.
    #[serde(default)]
    pub host_terminal_bindings: Vec<SerializedHostTerminalBinding>,
    /// Top-level workspace tabs other than the one in front at save time.
    /// The front tab is stored in [`SerializedSession::tabs`].
    #[serde(default)]
    pub parked_workspace_tabs: Vec<SerializedWorkspaceTab>,
    /// Strip order of every top-level workspace tab, front tab included.
    #[serde(default)]
    pub workspace_tab_order: Vec<usize>,
    #[serde(default)]
    pub active_workspace_tab_id: Option<usize>,
    #[serde(default)]
    pub active_workspace_tab_name: String,
    /// Explicit workspace ownership; tab ids refer to pane-layout surfaces.
    #[serde(default)]
    pub workspaces: Vec<SerializedWorkspace>,
    /// Checkouts explicitly dismissed by the user. Recovery must not recreate
    /// a closed local workspace simply because its project folder still exists.
    #[serde(default)]
    pub closed_workspace_roots: Vec<PathBuf>,
    /// Settings is a screen layered over the saved workspace, never a tab.
    #[serde(default)]
    pub settings_open: bool,
    #[serde(default)]
    pub surface_ui: std::collections::BTreeMap<usize, String>,
    #[serde(default)]
    pub sidebar_ui: String,
    #[serde(default)]
    pub editor_ui: String,
}

impl Default for SerializedDocumentSession {
    fn default() -> Self {
        Self {
            organisation_id: dragent_paths::organisation_id(),
            root: Default::default(),
            open_project_roots: Default::default(),
            selected_project_root: Default::default(),
            project_start_visible: Default::default(),
            prompt_text: Default::default(),
            prompt_links_json: Default::default(),
            prompt_agent: Default::default(),
            prompt_selected_worktree: Default::default(),
            prompt_branch: Default::default(),
            center_open: Default::default(),
            owner_tab_id: Default::default(),
            active_relative_path: Default::default(),
            open_relative_paths: Default::default(),
            work_item_open: Default::default(),
            work_item_active: Default::default(),
            work_item_view: Default::default(),
            workspace_tab_work_items: Default::default(),
            expanded_directories: Default::default(),
            dock_visible: Default::default(),
            dock_height: Default::default(),
            dock_maximized: Default::default(),
            dock_active_id: Default::default(),
            dock_split_id: Default::default(),
            dock_next_id: Default::default(),
            dock_terminals: Default::default(),
            host_terminal_bindings: Default::default(),
            parked_workspace_tabs: Default::default(),
            workspace_tab_order: Default::default(),
            active_workspace_tab_id: Default::default(),
            active_workspace_tab_name: Default::default(),
            workspaces: Default::default(),
            closed_workspace_roots: Default::default(),
            settings_open: Default::default(),
            surface_ui: Default::default(),
            sidebar_ui: Default::default(),
            editor_ui: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedWorkspace {
    #[serde(default = "dragent_paths::local_organisation_id")]
    pub organisation_id: String,
    pub id: usize,
    pub root: PathBuf,
    pub title: String,
    pub tab_ids: Vec<usize>,
    pub active_tab_id: usize,
}

/// A top-level workspace tab that was not in front at save time, with its
/// pane-tabs and split tree stored exactly like the front tab's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedWorkspaceTab {
    pub id: usize,
    #[serde(default)]
    pub name: String,
    pub tabs: Vec<SerializedTab>,
    pub active_tab_id: usize,
    #[serde(default)]
    pub active_pane_uuid: Option<Uuid>,
}

/// Main window geometry in the framework's rect coordinates. Stored apart
/// from session snapshots so the geometry captured at close is final.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SerializedWindowFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// 0 = normal, 1 = fullscreen, 2 = maximized (`FullscreenState`).
    #[serde(default)]
    pub fullscreen_state: u8,
}

/// Daemon PTY binding for one restored tab: re-attach by session id, or
/// report unavailable when the session is gone. Never silently replace it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedHostTerminalBinding {
    /// Serialized SSH destination. Never contains credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_host: Option<String>,
    pub pane_uuid: Uuid,
    pub workspace_id: String,
    pub home_id: String,
    pub session_id: String,
    #[serde(default)]
    pub agent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedWorkspaceTabWorkItem {
    pub owner_tab_id: usize,
    pub work_item_id: WorkItemId,
    /// Stable project scope for the tab. This is deliberately separate from
    /// the terminal's mutable cwd, so `cd` cannot retarget editors or agents.
    #[serde(default)]
    pub project_root: Option<PathBuf>,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub view: Option<String>,
    /// Whether this work item's docked terminal was explicitly left open.
    /// `None` identifies sessions written before dock state became per-tab,
    /// allowing the active tab to fall back to the legacy document fields.
    #[serde(default)]
    pub dock_visible: Option<bool>,
    #[serde(default)]
    pub dock_maximized: Option<bool>,
    /// Shaping progress that isn't canonical work state, so a crash or
    /// restart doesn't lose it.
    #[serde(default)]
    pub shaping: SerializedShapingProgress,
}

/// A work item's shaping progress outside its canonical events.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SerializedShapingProgress {
    /// The Principal confirmed the requirements: Approve Shape is unlocked.
    #[serde(default)]
    pub requirements_complete: bool,
    /// The user approved shaping and the shape isn't built yet.
    #[serde(default)]
    pub shape_approved: bool,
    /// The Principal's research findings, carried between its turns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn_memory: Vec<String>,
    /// A Principal turn that was running when the app last stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_turn: Option<SerializedShapeTurn>,
}

/// A Principal turn in progress, recorded when it starts and cleared when it
/// ends, so one cut short by a crash is resumed on the next start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerializedShapeTurn {
    /// What the Principal was asked (text attachments inlined).
    pub body: String,
    /// The user's message as their conversation shows it.
    pub display: String,
    #[serde(default)]
    pub shape_approved: bool,
    #[serde(default)]
    pub force_proposal: bool,
    /// The user's message as committed at the start of the turn: a resumed
    /// turn answers it instead of posting it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<WorkspaceEventEnvelope>,
    /// Already resumed once: if the app stopped during that too, the user
    /// retries it themselves rather than looping.
    #[serde(default)]
    pub resumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedDockedTerminal {
    /// Daemon identity for this pane; older saved docks migrate on restore.
    #[serde(default)]
    pub host_binding: Option<SerializedHostTerminalBinding>,
    pub id: usize,
    /// Project directory that owns this dock terminal group. Older sessions
    /// leave this empty and are assigned the restored document root.
    #[serde(default)]
    pub owner_root: PathBuf,
    /// Borrowed terminals refer to a top-level pane by UUID; owned dock
    /// terminals carry their complete pane descriptor.
    pub pane: SerializedPaneLayout,
    #[serde(default)]
    pub borrowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedWorkingCopy {
    pub workspace_root: PathBuf,
    pub relative_path: PathBuf,
    pub draft_text: String,
    pub base_hash: u64,
    pub base_size: u64,
    pub base_modified_unix_ms: Option<i64>,
    pub revision: u64,
    pub updated_at_unix_ms: i64,
}

fn default_true() -> bool {
    true
}

/// Full workspace session snapshot (single-window dragent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedSession {
    pub tabs: Vec<SerializedTab>,
    pub active_tab_id: usize,
    pub active_pane_uuid: Option<Uuid>,
    #[serde(default)]
    pub chrome: WindowChrome,
    #[serde(default)]
    pub document: SerializedDocumentSession,
    /// Client-local layout instances. Legacy pane/document fields above remain
    /// populated during the Develop migration and are the compatibility source
    /// for older databases and application versions.
    #[serde(default)]
    pub layouts: LayoutCollection,
}

/// Durable bridge between one accepted workspace mutation and the exact
/// provider tool call that must receive its outcome. `output == None` denotes
/// a committed DecisionRequest that is waiting for a Principal reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceToolOutcome {
    pub command_id: WorkspaceCommandId,
    pub project_id: ProjectId,
    pub work_item_id: WorkItemId,
    pub runtime_run_id: String,
    pub runtime_execution_id: String,
    pub runtime_round: u32,
    pub provider_call_id: String,
    pub output: Option<String>,
    pub is_error: Option<bool>,
    pub source_message_id: Option<Uuid>,
    pub accepted_at_unix_ms: i64,
    pub delivered_at_unix_ms: Option<i64>,
    pub terminal_reason: Option<String>,
}

/// Durable intent spanning the non-transactional Git apply and the canonical
/// SQLite review-completion transaction. The plan is opaque to persistence;
/// the desktop Git owner validates and reconciles it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceIntegrationIntent {
    pub attempt_id: dragent_workspace_model::ExecutionAttemptId,
    pub project_id: ProjectId,
    pub work_item_id: WorkItemId,
    pub command_id: WorkspaceCommandId,
    pub command: WorkspaceCommandEnvelope,
    pub integration_plan_json: String,
    pub state: String,
    pub claim_owner: Option<String>,
    pub claim_expires_at_unix_ms: Option<i64>,
    pub created_at_unix_ms: i64,
    pub completed_at_unix_ms: Option<i64>,
    pub last_error: Option<String>,
}

/// Blocks keyed by stable pane UUID, loaded separately from layout snapshot.
pub type RestoredBlockLists = std::collections::HashMap<Uuid, Vec<PersistedBlock>>;

#[cfg(test)]
mod work_item_presentation_tests {
    use super::{SerializedDocumentSession, SerializedWorkspaceTabWorkItem};
    use dragent_workspace_model::WorkItemId;
    use std::path::PathBuf;

    #[test]
    fn workspace_tabs_round_trip_distinct_work_item_identities() {
        let first = WorkItemId::new();
        let second = WorkItemId::new();
        let state = SerializedDocumentSession {
            workspace_tab_work_items: vec![
                SerializedWorkspaceTabWorkItem {
                    owner_tab_id: 3,
                    work_item_id: first,
                    project_root: Some(PathBuf::from("/projects/first")),
                    open: true,
                    active: true,
                    view: Some("coordinate".into()),
                    dock_visible: Some(true),
                    dock_maximized: Some(false),
                    shaping: super::SerializedShapingProgress {
                        requirements_complete: true,
                        shape_approved: true,
                        turn_memory: vec!["read_file: src/main.rs".into()],
                        interrupted_turn: Some(super::SerializedShapeTurn {
                            body: "Build it".into(),
                            display: "Build it".into(),
                            shape_approved: true,
                            force_proposal: true,
                            accepted: None,
                            resumed: false,
                        }),
                    },
                },
                SerializedWorkspaceTabWorkItem {
                    owner_tab_id: 8,
                    work_item_id: second,
                    project_root: Some(PathBuf::from("/projects/second")),
                    open: true,
                    active: false,
                    view: Some("plan".into()),
                    dock_visible: Some(false),
                    dock_maximized: Some(false),
                    shaping: Default::default(),
                },
            ],
            ..Default::default()
        };

        let encoded = serde_json::to_string(&state).unwrap();
        let decoded: SerializedDocumentSession = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.workspace_tab_work_items.len(), 2);
        assert_eq!(decoded.workspace_tab_work_items[0].work_item_id, first);
        assert_eq!(decoded.workspace_tab_work_items[1].work_item_id, second);
        assert_eq!(
            decoded.workspace_tab_work_items[0].project_root,
            Some(PathBuf::from("/projects/first"))
        );
        assert_eq!(
            decoded.workspace_tab_work_items[1].project_root,
            Some(PathBuf::from("/projects/second"))
        );
        assert_ne!(first, second);
        assert_eq!(decoded.workspace_tab_work_items[0].dock_visible, Some(true));
        assert_eq!(
            decoded.workspace_tab_work_items[1].dock_visible,
            Some(false)
        );
        // Shaping progress survives, and sessions saved before it existed
        // still load with none.
        assert_eq!(
            decoded.workspace_tab_work_items[0].shaping,
            state.workspace_tab_work_items[0].shaping
        );
        assert_eq!(decoded.workspace_tab_work_items[1].shaping, Default::default());
        let older: super::SerializedWorkspaceTabWorkItem = serde_json::from_value(serde_json::json!({
            "owner_tab_id": 1, "work_item_id": first, "open": true
        }))
        .unwrap();
        assert_eq!(older.shaping, Default::default());
    }

    #[test]
    fn legacy_work_item_presentation_without_project_root_still_decodes() {
        let encoded = format!(
            r#"{{"owner_tab_id":3,"work_item_id":"{}","open":false,"active":false,"view":"conversation"}}"#,
            WorkItemId::new().0
        );
        let decoded: SerializedWorkspaceTabWorkItem = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.project_root, None);
        assert_eq!(decoded.dock_visible, None);
        assert_eq!(decoded.dock_maximized, None);
    }

    #[test]
    fn legacy_document_state_defaults_to_no_per_tab_work_items() {
        let decoded: SerializedDocumentSession = serde_json::from_str("{}").unwrap();
        assert!(decoded.workspace_tab_work_items.is_empty());
    }
}

/// A single persisted agent block (serialized as JSON kind_json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAgentBlock {
    /// Serialised `AgentBlockKind` — stored as opaque JSON so the persistence
    /// crate doesn't depend on the app crate's agent types.
    pub kind_json: String,
    pub conversation_id: Uuid,
    pub created_at_unix_ms: i64,
}

/// Agent blocks keyed by stable pane UUID, loaded on startup.
pub type RestoredAgentBlockLists = std::collections::HashMap<Uuid, Vec<PersistedAgentBlock>>;

/// Summary of one past conversation for a pane, for the session history
/// browser. `first_block_kind_json` is the opaque `kind_json` of the
/// conversation's earliest block (almost always the initial user message) —
/// callers that know about `AgentBlockKind` synthesize a title from it.
#[derive(Debug, Clone)]
pub struct ConversationSummary {
    pub conversation_id: Uuid,
    pub started_at_unix_ms: i64,
    pub last_activity_unix_ms: i64,
    pub block_count: i64,
    pub first_block_kind_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PersistedSession {
    pub session: SerializedSession,
    pub blocks: RestoredBlockLists,
    pub agent_blocks: RestoredAgentBlockLists,
    pub working_copies: Vec<PersistedWorkingCopy>,
    /// Bounded semantic state restored independently from terminal/editor data.
    pub workspace_work_items: Vec<WorkItem>,
    pub workspace_events: Vec<WorkspaceEvent>,
    /// Validated canonical envelopes for authoritative workspace replay. The
    /// compact events above remain a migration fallback only.
    pub canonical_workspace_events:
        Vec<dragent_session_protocol::workspace::WorkspaceEventEnvelope>,
}

/// A write request handed to the background persistence writer thread. Pure data
/// (referenced only through a channel), so it lives here rather than in the
/// diesel-backed `writer` module — the wasm build needs the type without diesel.
pub enum ModelEvent {
    Snapshot(SerializedSession),
    RecordLocalWorkspace(SerializedWorkspace),
    CloseLocalWorkspace(PathBuf),
    SaveBlock {
        pane_uuid: Uuid,
        block: PersistedBlock,
        sort_order: i32,
        project_root: Option<PathBuf>,
    },
    /// Save one agent block for a pane.
    SaveAgentBlock {
        pane_uuid: Uuid,
        block: PersistedAgentBlock,
        sort_order: i32,
        project_root: Option<PathBuf>,
    },
    /// Record a run command into the GLOBAL, never-cleared history table (the
    /// ↑/↓ popover). Not tied to any pane; survives app close.
    RecordCommand {
        command: String,
        at_unix_ms: i64,
    },
    /// Remove all persisted command blocks for a pane (clear / tab close).
    ClearPaneData {
        pane_uuid: Uuid,
        project_root: Option<PathBuf>,
    },
    /// Remove all persisted agent blocks for a pane (clear conversation / tab close).
    ClearAgentBlocks {
        pane_uuid: Uuid,
        project_root: Option<PathBuf>,
    },
    UpdateClearGap {
        pane_uuid: Uuid,
        height_px: f32,
        before_block_id: Option<i32>,
        after_all: bool,
    },
    SaveWorkspaceProject(Project),
    DeleteWorkspaceProject(dragent_workspace_model::ProjectId),
    /// Close any connection into a deleted root's private data directory,
    /// then delete it.
    ForgetProjectData(PathBuf),
    SaveWorkspaceWorkItem(WorkItem),
    AppendWorkspaceEvent(WorkspaceEvent),
    /// Submit a command to the local semantic authority. The SQLite writer
    /// returns the canonical events accepted for this command, including the
    /// original outcome for an idempotent retry.
    AcceptWorkspaceCommand {
        commands: Vec<WorkspaceCommandEnvelope>,
        tool_outcome: Option<WorkspaceToolOutcome>,
        reply: std::sync::mpsc::SyncSender<Result<Vec<WorkspaceEventEnvelope>, String>>,
    },
    LoadPendingWorkspaceToolOutcomes {
        reply: std::sync::mpsc::SyncSender<Result<Vec<WorkspaceToolOutcome>, String>>,
    },
    MarkWorkspaceToolOutcomeDelivered {
        command_id: WorkspaceCommandId,
        runtime_execution_id: String,
        provider_call_id: String,
        delivered_at_unix_ms: i64,
        reply: std::sync::mpsc::SyncSender<Result<bool, String>>,
    },
    ParkWorkspaceToolOutcome {
        command_id: WorkspaceCommandId,
        runtime_execution_id: String,
        reason: String,
        reply: std::sync::mpsc::SyncSender<Result<bool, String>>,
    },
    ParkWorkspaceToolOutcomesForRun {
        runtime_run_id: String,
        runtime_execution_id: String,
        reason: String,
        reply: std::sync::mpsc::SyncSender<Result<usize, String>>,
    },
    SaveWindowFrame(SerializedWindowFrame),
    SaveWorkingCopy(PersistedWorkingCopy),
    DeleteWorkingCopy {
        workspace_root: PathBuf,
        relative_path: PathBuf,
    },
    Flush {
        done: std::sync::mpsc::SyncSender<()>,
    },
    Terminate,
}

impl SerializedWorkspace {
    /// Selection cannot transfer a layout from another workspace.
    pub fn select_tab(&mut self, tab_id: usize) -> bool {
        if !self.tab_ids.contains(&tab_id) {
            return false;
        }
        self.active_tab_id = tab_id;
        true
    }
}

impl SerializedDocumentSession {
    pub fn belongs_to_organisation(&self, organisation: &str) -> bool {
        let owner = if self.organisation_id.is_empty() {
            "local"
        } else {
            &self.organisation_id
        };
        owner == organisation
            && self
                .workspaces
                .iter()
                .all(|workspace| workspace.organisation_id == owner)
    }
}

#[cfg(test)]
mod workspace_ownership_tests {
    use super::*;

    #[test]
    fn selection_is_scoped_and_survives_restart() {
        let mut a = SerializedWorkspace {
            organisation_id: dragent_paths::organisation_id(),
            id: 1,
            root: PathBuf::from("/project/worktree-a"),
            title: "Workspace A".into(),
            tab_ids: vec![10, 11],
            active_tab_id: 10,
        };
        let mut b = SerializedWorkspace {
            organisation_id: dragent_paths::organisation_id(),
            id: 2,
            root: PathBuf::from("/project/worktree-b"),
            title: "Workspace B".into(),
            tab_ids: vec![20],
            active_tab_id: 20,
        };
        assert!(a.select_tab(11));
        assert!(!b.select_tab(11));
        assert_eq!(b.active_tab_id, 20);
        let encoded = serde_json::to_vec(&vec![a, b]).unwrap();
        let mut restored: Vec<SerializedWorkspace> = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored[0].active_tab_id, 11);
        assert_eq!(restored[1].active_tab_id, 20);
        assert!(!restored[0].select_tab(20));
        assert_eq!(restored[0].tab_ids, vec![10, 11]);
        assert_eq!(restored[0].title, "Workspace A");
    }

    #[test]
    fn legacy_snapshot_can_migrate_without_workspace_records() {
        let restored: SerializedDocumentSession = serde_json::from_str("{}").unwrap();
        assert!(restored.workspaces.is_empty());
        assert!(restored.belongs_to_organisation("local"));
        assert!(!restored.belongs_to_organisation("another-org"));
    }

    #[test]
    fn selecting_tabs_keeps_workspace_and_project_order_across_restart() {
        let mut session = SerializedDocumentSession {
            open_project_roots: vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            selected_project_root: Some(PathBuf::from("/b")),
            workspaces: (1..=3)
                .map(|id| SerializedWorkspace {
                    organisation_id: dragent_paths::organisation_id(),
                    id,
                    root: PathBuf::from(format!("/b/tree-{id}")),
                    title: format!("Workspace {id}"),
                    tab_ids: vec![id * 10, id * 10 + 1],
                    active_tab_id: id * 10,
                })
                .collect(),
            ..SerializedDocumentSession::default()
        };
        for selected in [3, 1, 2, 3, 2] {
            for workspace in &mut session.workspaces {
                workspace.select_tab(selected * 10 + 1);
            }
        }
        let restored: SerializedDocumentSession =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        assert_eq!(
            restored
                .workspaces
                .iter()
                .map(|workspace| workspace.id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(restored.open_project_roots, session.open_project_roots);
        assert_eq!(restored.selected_project_root, Some(PathBuf::from("/b")));
        assert_eq!(
            restored
                .workspaces
                .iter()
                .map(|workspace| workspace.active_tab_id)
                .collect::<Vec<_>>(),
            vec![11, 21, 31]
        );
    }
}
