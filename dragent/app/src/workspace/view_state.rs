//! Workspace state types for WorkspaceView.
//!
//! Responsible for:
//! - plain state records owned by the workspace (tabs, surfaces, parked
//!   work, acknowledgements, language sync, explorer, folder-git state)
//! - attempt-start descriptors for mounted executions
//!
//! Not responsible for:
//! - the WorkspaceView itself or its behavior (sibling view_* modules)
//! - canonical domain state (dragent_workspace_model)
//!
//! These are data types only: no ViewContext, no rendering, no I/O.

use super::share_sheet;
use super::viewer_bar;
use super::{InputEditor, InputEditorEvent, InputMode, LanguageNavigationCapabilities};
use crate::agent::workspace_execution::{
    next_workspace_launch_action, next_workspace_retry_wait_ms, WorkspaceExecutionRuntime,
    WorkspaceLaunchAction, WorkspaceLaunchPolicy, WorkspacePipelineAction,
    WorkspaceResolvedToolAction, WorkspaceToolRequest,
};
use crate::auth::{AuthManager, AuthManagerEvent};
use crate::realtime::manager::{SessionManager, SessionManagerEvent};
use crate::settings::AccountStore;
use crate::settings::{SettingsEvent, SettingsView};
use crate::setup::UiSettings;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::{
    Theme, DEFAULT_LEFT_PANEL_WIDTH, DEFAULT_RIGHT_PANEL_WIDTH, HEADER_BAR_HEIGHT, MIN_PANEL_WIDTH,
    UI_FONT_SIZE, WORKSPACE_PADDING,
};
use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_session_protocol::{AccessLevel, ParticipantId, Role};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::{
    drag_resize_handle, resizable_state_handle, resizable_state_handle_initialized, Align,
    ChildAnchor, ChildView, ClippedScrollStateHandle, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, DragBarSide, DragResizeAxis, DragResizeElement,
    DragResizeHandle, DropTarget, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    PositionedElementAnchor, PositionedElementOffsetBounds, Resizable, ResizableStateHandle,
    SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, ReadModel, ReadView, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::Cursor;
use warpui_core::platform::FilePickerConfiguration;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::process_ext::NoWindow;
use dragent_editor::{
    char_index_to_lsp_position, lsp_position_to_char_index, LanguageRequest, LanguageRequestKind,
    TextPosition, TextRange,
};
use dragent_language_services::{
    LanguageServerCatalog, LanguageServerKey, LanguageServiceClient, LanguageServiceClientConfig,
    LanguageServiceEvent, SyncedDocument,
};

use super::pane::WorkspacePane;
use super::pane_layout::{
    LayoutNode, PaneLayout, SplitDirection, SplitId, SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT,
    MIN_PANE_WIDTH,
};
use super::project_sidebar::{
    find_git_root, ProjectSidebarModel, ProjectSidebarView, ReviewPrState,
};
// Re-exported so existing `super::view::ReviewShipState` paths keep working
// during the migration; new code should import from `review_pr_service`.
// Re-exported so existing `super::…` and test paths keep working during the
// migration; new code should import from `project_naming`.
// Glob import during migration: shaping helpers moved to `shape_parse`
// verbatim; call sites below are unchanged.
// Glob import during migration: execution delivery helpers moved verbatim.
use super::execution_delivery::*;
// Glob import during migration: language navigation helpers moved verbatim.
use super::language_nav::*;
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_parse::*;
use super::sidebar::color_from_hex;
use super::view_overlays::ExplorerPrompt;
use super::{
    chrome_inset, content_header, header, left_panel, pane_layout, persistence, sidebar,
    split_tab_header, DropSide, PaneDropZone, ProjectFolderIntent, SidebarFocus, WorkItemView,
    WorkspaceAction, DEFAULT_SESSION_TITLE,
};
use dragent_persistence::{SerializedPaneLayout, SerializedSplitDirection};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorKind, ActorRef, AgentMessage as WorkspaceAgentMessage,
    AgentMessageId, AgentMessageKind, AgentMessageRouting, AttentionItem, AttentionItemId,
    AttentionKind, AttentionResolution, AttentionStatus, BuiltInLayout, ChargingAuthorization,
    ChargingMode, CommunicationThread, ExecutionAttemptId, LayoutAxis, LayoutChromeState,
    LayoutCollection, LayoutInstance, LayoutPanelState, LayoutScope, MessageDeliveryState, Project,
    ProjectDocumentIndex, SurfaceDescriptor, SurfaceId, SurfaceKind, SurfaceLayoutNode,
    SurfaceRegistry, ThreadId, WorkItemId, WorkStatus, WorkspaceEvent, WorkspaceEventKind,
    WorkspaceState, MAX_LAYOUT_FOCUS_HISTORY,
};
use left_panel::LeftPanelView;
use persistence::{PersistedSession, WindowChrome, WriterHandles};
use uuid::Uuid;

/// A terminal shown inside the editor dock. Dock terminals own independent
/// shell panes so the same view is never laid out in both the workspace mosaic
/// and the editor dock.
#[derive(Clone)]
pub(crate) struct DocumentTerminalTab {
    pub(crate) id: usize,
    pub(crate) surface_id: SurfaceId,
    pub(crate) pane: WorkspacePane,
    pub(crate) borrowed: bool,
    /// Directory identity of the editor surface that owns this dock group.
    /// Terminal `cd` events must not mutate this value or the explorer root.
    pub(crate) owner_root: PathBuf,
}

/// Runtime values are intentionally absent from the serializable layout tree.
/// This enum lets one registry own heterogeneous live views while layouts keep
/// only their `SurfaceId` references.
#[derive(Clone)]
#[allow(dead_code)] // Compatibility placement still reads several handles directly.
pub(crate) enum WorkspaceRuntimeSurface {
    Pane(WorkspacePane),
    ProjectEditor(ViewHandle<ProjectSidebarView>),
    WorkSurface,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkSurfaceIds {
    pub(crate) conversation: SurfaceId,
    pub(crate) brief: SurfaceId,
    pub(crate) organization: SurfaceId,
    pub(crate) role_detail: SurfaceId,
    pub(crate) activity: SurfaceId,
    pub(crate) attention: SurfaceId,
    pub(crate) tests: SurfaceId,
    pub(crate) artifact: SurfaceId,
}

/// Canonical/runtime state parked while another top-level workspace tab is
/// active. Keeping this keyed by `WorkItemId` prevents a tab switch from
/// presenting or mutating another tab's work while allowing executions to
/// retain their own runtime registry.
pub(crate) struct ParkedWorkspaceWork {
    pub(crate) surface_ids: WorkSurfaceIds,
    pub(crate) state: WorkspaceState,
    pub(crate) execution_runtime: WorkspaceExecutionRuntime,
    pub(crate) canonical_revision: u64,
    pub(crate) checkpoint_notice: Option<String>,
    pub(crate) launch_pending: bool,
    pub(crate) launch_pending_since: Option<i64>,
    pub(crate) principal_coordination_pending: bool,
    pub(crate) principal_coordination_pending_since: Option<i64>,
    pub(crate) principal_coordination_epoch: u64,
    pub(crate) principal_escalation_queue: VecDeque<dragent_workspace_model::AttentionItem>,
    // Sequential commands from one coordination turn's add_tasks/
    // propose_roles — a role needs role_definition, then contract, then team,
    // then its own tasks, each waiting for the previous to land canonically
    // (submit_workspace_runtime_command drops a command outright while
    // another is in flight, so these cannot fire in one synchronous burst).
    // (work_item_id, actor_id, payload) — actor_id varies per command: the
    // Principal for role_definition/work_contract/proposed_team, but the
    // plan's own approved_by for the UpsertPlan step (its actor-mismatch
    // guard requires it once plan.status is Approved).
    pub(crate) principal_plan_actions_queue: VecDeque<(
        WorkItemId,
        dragent_workspace_model::ActorId,
        WorkspaceCommandPayload,
    )>,
    pub(crate) execution_poll_scheduled: bool,
    pub(crate) retry_wakeup_scheduled: bool,
    pub(crate) tab_open: bool,
    pub(crate) tab_active: bool,
    pub(crate) view: WorkItemView,
    /// Parked Agent-tab selection so switching work items preserves each
    /// item's inspected agent. Stable identities only.
    pub(crate) selected_agent_task: Option<dragent_workspace_model::PlanTaskId>,
    pub(crate) selected_agent_attempt: Option<dragent_workspace_model::ExecutionAttemptId>,
    pub(crate) agent_timeline_expanded: bool,
    pub(crate) dock_visible: bool,
    pub(crate) dock_maximized: bool,
    pub(crate) composer_attachments: Vec<crate::agent::attachments::LoadedAttachment>,
    pub(crate) composer_notice: Option<String>,
    // Parked Principal turn flags. The conversation itself (stream, tools,
    // question cards, errors) lives in the work item's own `AgentController`
    // (`WorkspaceView::work_item_agents`), which is per item already — a tab
    // switch can't orphan it, exactly like a background terminal pane.
    pub(crate) principal_pending: bool,
    pub(crate) principal_shape_approved: bool,
    pub(crate) force_proposal: bool,
    pub(crate) requirements_complete: bool,
    pub(crate) turn_memory: Vec<String>,
    /// Host errors raised while this item was parked, drained into its
    /// conversation as Error cards once the parked borrow is released.
    pub(crate) principal_errors: Vec<String>,
    pub(crate) principal_request_epoch: u64,
    pub(crate) interrupted_turn: Option<dragent_persistence::SerializedShapeTurn>,
}

pub(crate) enum WorkspaceIntegrationReconciliation {
    None,
    Ready(dragent_persistence::WorkspaceIntegrationIntent),
    BusyUntil(i64),
}

/// A mutating host-tool call whose result must not be returned until the
/// canonical SQLite authority has accepted (or rejected) its command.
/// Provider call identity is stored here so acknowledgement is independent of
/// whichever work-item tab is visible when persistence finishes.
#[derive(Clone)]
pub(crate) struct PendingWorkspaceToolAcknowledgement {
    pub(crate) runtime_task_id: String,
    pub(crate) runtime_execution_id: String,
    pub(crate) runtime_round: u32,
    pub(crate) call_id: String,
    pub(crate) success: String,
    pub(crate) resolved: Option<(String, bool)>,
    pub(crate) delivery_in_flight: bool,
    pub(crate) durable: bool,
    pub(crate) delivery_attempts: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct LanguageDocumentSyncState {
    pub(crate) project_root: PathBuf,
    pub(crate) relative_path: PathBuf,
    pub(crate) sent_revision: u64,
}

#[derive(Clone, Debug)]
pub(crate) enum LanguageRequestPurpose {
    Completion,
    Navigation(super::LanguageNavigationKind),
    DefinitionHover {
        request_id: u64,
        source_relative_path: PathBuf,
        symbol_range: std::ops::Range<usize>,
        /// Import specifier when the hovered range is a path-like string
        /// literal; used to resolve the file locally if the language server
        /// has no definition for the specifier position.
        string_link: Option<String>,
    },
    Formatting,
    CodeActions,
    Rename,
    FileRename {
        source: PathBuf,
        destination: PathBuf,
    },
}

#[derive(Clone)]
pub(crate) struct RemotePointer {
    pub(crate) display_name: String,
    pub(crate) avatar_url: String,
    pub(crate) position: dragent_session_protocol::PointerPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExplorerClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone, Debug)]
pub(crate) struct ExplorerClipboardEntry {
    pub(crate) relative_path: PathBuf,
    pub(crate) mode: ExplorerClipboardMode,
}

/// Per-file git operations for folder-only state (no terminal owns git).
pub(crate) enum FolderGitFileOp {
    StageToggle,
    Discard,
}

/// Bulk git operations for folder-only state.
pub(crate) enum FolderGitBulkOp {
    StageAll,
    UnstageAll,
    DiscardAllStaged,
    DiscardAllUnstaged,
}

/// How a mounted attempt's controller begins its turn.
pub(crate) enum AttemptStart {
    /// A newly committed start: send the task prompt.
    Fresh,
    /// Restart recovery: resume the interrupted run from its checkpoint.
    Recover {
        run_id: String,
        parked: Vec<crate::agent::controller::RecoveredParkedCall>,
    },
}

impl ParkedWorkspaceWork {
    pub(crate) fn record_principal_error(&mut self, error: String) {
        self.principal_errors.push(error);
    }
}
