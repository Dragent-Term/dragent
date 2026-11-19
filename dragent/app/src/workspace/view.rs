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
use super::view_overlays::ExplorerPrompt;
// Re-exported so existing `super::view::…` state paths keep working during
// the migration; new code should import from `view_state` directly.
use super::agent_definition::CustomAgentDefinition;
use super::sidebar::color_from_hex;
pub(crate) use super::view_state::{
    AttemptStart, DocumentTerminalTab, ExplorerClipboardEntry, ExplorerClipboardMode,
    FolderGitBulkOp, FolderGitFileOp, LanguageDocumentSyncState, LanguageRequestPurpose,
    ParkedWorkspaceWork, PendingWorkspaceToolAcknowledgement, RemotePointer, WorkSurfaceIds,
    WorkspaceIntegrationReconciliation, WorkspaceRuntimeSurface,
};
use super::workspace_tab::WorkspaceTab;
use super::{
    chrome_inset, content_header, header, left_panel, pane_layout, persistence, sidebar,
    split_tab_header, workspace_tab, DropSide, PaneDropZone, ProjectFolderIntent, SidebarFocus,
    WorkItemView, WorkspaceAction, DEFAULT_SESSION_TITLE,
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

pub(crate) const MIN_RIGHT_PANEL_WIDTH: f32 = MIN_PANEL_WIDTH + 50.0;
pub(crate) const ACTIVE_PANE_ACCENT_ALPHA: u8 = 77;
pub(crate) const NEW_WORK_LABEL: &str = "New work";
/// Placeholder reply the Principal posts when it escalates instead of
/// answering; a parked worker question stays parked until the human answers.
pub(crate) const PRINCIPAL_ESCALATED_REPLY: &str = "Escalated to the user for a decision.";

/// Add-project popover content under the sidebar add button.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AddProjectMenu {
    #[default]
    Closed,
    Add,
    CloneForm,
}

#[cfg(test)]
mod workspace_layout_recovery_tests {
    use super::super::project_naming::{
        bounded_project_memory_text, clone_target_name, is_default_work_label,
        should_replace_project_tab_label,
    };
    use super::super::ssh::SshDestination;
    use super::super::view_persistence::{is_legacy_agentic_fixture, reconciled_focus_history};
    use super::{SurfaceId, WorkspaceView, MAX_LAYOUT_FOCUS_HISTORY};
    use dragent_workspace_model::{
        ExecutionAttemptId, Project, ProjectId, WorkItemId, WorkspaceEventKind, WorkspaceState,
    };
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    #[test]
    pub(crate) fn develop_capture_drops_focus_from_other_projects_and_keeps_active_last() {
        let current_a = SurfaceId::stable("current-a");
        let current_b = SurfaceId::stable("current-b");
        let stale = SurfaceId::stable("stale-project-surface");

        let history = reconciled_focus_history(
            &[stale, current_b, current_a, current_b],
            &[current_a, current_b],
            current_a,
        );

        assert_eq!(history, vec![current_b, current_a]);
        assert!(!history.contains(&stale));
    }

    #[test]
    pub(crate) fn develop_capture_bounds_recovered_focus_history() {
        let available = (0..MAX_LAYOUT_FOCUS_HISTORY + 8)
            .map(|index| SurfaceId::stable(&format!("surface-{index}")))
            .collect::<Vec<SurfaceId>>();
        let active = *available.last().unwrap();
        let history = reconciled_focus_history(&available, &available, active);

        assert_eq!(history.len(), MAX_LAYOUT_FOCUS_HISTORY);
        assert_eq!(history.last(), Some(&active));
    }

    #[test]
    pub(crate) fn legacy_billing_demo_can_never_be_restored_as_user_work() {
        let project_id = ProjectId::new();
        let legacy = WorkItemId(Uuid::new_v5(&project_id.0, b"fixture:organization-billing"));
        assert!(is_legacy_agentic_fixture(project_id, legacy));
        assert!(!is_legacy_agentic_fixture(project_id, WorkItemId::new()));
    }

    #[test]
    pub(crate) fn only_placeholder_work_labels_are_replaced_by_inferred_intent() {
        assert!(is_default_work_label("New work"));
        assert!(is_default_work_label("Work · dragent"));
        assert!(is_default_work_label("Untitled work"));
        assert!(!is_default_work_label("Fix search result navigation"));
        assert!(should_replace_project_tab_label(
            Some("dragent"),
            Some("dragent")
        ));
        assert!(should_replace_project_tab_label(None, Some("dragent")));
        assert!(!should_replace_project_tab_label(
            Some("Search performance"),
            Some("dragent")
        ));
        assert_eq!(
            bounded_project_memory_text("  a long value  ", 6),
            "a long…"
        );
    }

    #[test]
    pub(crate) fn new_work_reuses_only_an_untouched_draft_projection() {
        let project = Project::new_for_organisation(
            &dragent_persistence::current_org(),
            PathBuf::from("/tmp/pristine-work-project"),
            0,
        );
        let mut state = WorkspaceState::new(project);

        // Terminal lifecycle events are project bookkeeping when no canonical
        // work item exists and must not force creation of another empty tab.
        state.record(
            1,
            WorkspaceEventKind::CommandStarted {
                summary: "terminal status changed".into(),
            },
        );
        assert!(WorkspaceView::is_pristine_work_state(&state));

        let work_item_id = WorkItemId::new();
        state.start_work_with_id(work_item_id, "New work", 2);
        assert!(WorkspaceView::is_pristine_work_state(&state));

        // Conversation starts with a work-scoped message event but deliberately
        // remains Draft until the user explicitly asks the Principal to shape.
        state.record(
            3,
            WorkspaceEventKind::UserMessagePosted {
                body: "Hello".into(),
            },
        );
        assert!(!WorkspaceView::is_pristine_work_state(&state));
    }

    #[test]
    pub(crate) fn project_start_validates_clone_names_and_ssh_destinations() {
        assert_eq!(
            clone_target_name("https://github.com/dragent/app.git"),
            Some("app".to_string())
        );
        assert_eq!(
            clone_target_name("git@github.com:dragent/desktop.git"),
            Some("desktop".to_string())
        );
        assert_eq!(clone_target_name("https://github.com/"), None);
        let destination = SshDestination::parse("engineer@example.com:2222").unwrap();
        assert_eq!(
            destination.shell_command(),
            "ssh -p 2222 -- 'engineer@example.com'"
        );
        assert!(SshDestination::parse("host; touch /tmp/unsafe").is_err());
    }

    #[test]
    pub(crate) fn agent_change_review_tabs_round_trip_without_matching_physical_files() {
        let attempt_id = ExecutionAttemptId::new();
        let path = WorkspaceView::workspace_attempt_diff_tab_path(attempt_id);

        assert_eq!(
            WorkspaceView::parse_workspace_attempt_diff_tab_path(&path),
            Some(attempt_id)
        );
        assert!(
            WorkspaceView::parse_workspace_attempt_diff_tab_path(Path::new(
                ".work-changes/not-a-uuid"
            ))
            .is_none()
        );
        assert!(
            WorkspaceView::parse_workspace_attempt_diff_tab_path(Path::new("src/work-changes.rs"))
                .is_none()
        );
    }
}

/// One terminal session per pane tab.
#[derive(Clone)]
pub struct PaneTab {
    pub id: usize,
    /// Stable surface identity. `id` above remains a compatibility placement
    /// key while the live split renderer migrates to `SurfaceLayoutNode`.
    pub surface_id: SurfaceId,
    pub title: String,
    pub custom_label: Option<String>,
    pub color: Option<warpui::color::ColorU>,
    /// Every top-level project/terminal tab owns a distinct agentic work
    /// identity. Settings and remote mirror tabs deliberately have none.
    pub work_item_id: Option<WorkItemId>,
    /// Explicit project/workspace scope. A terminal may `cd` freely without
    /// changing this root or moving the tab's work item to another project.
    pub project_root: Option<PathBuf>,
    /// Split-created panes live in the mosaic, not the sidebar tab list:
    /// showing a card per split would duplicate every division. Popping a
    /// tab out of its split clears this again so it never strands invisible.
    pub hidden_from_sidebar: bool,
    pub pane: WorkspacePane,
}

pub struct WorkspaceView {
    /// Immutable owner of this view and its background persistence work.
    pub(crate) organisation_id: String,
    pub(crate) workspace_view_id: EntityId,
    /// The OS window this workspace lives in — for per-window chrome (e.g. header
    /// padding around the macOS traffic lights). Captured once at construction so
    /// we never fall back to `window_ids().next()` (always the first window).
    pub(crate) window_id: warpui_core::WindowId,
    pub(crate) theme: Theme,
    pub(crate) ui_font: FamilyId,
    pub(crate) left_panel_state: ResizableStateHandle,
    pub(crate) right_panel_state: ResizableStateHandle,
    pub(crate) tabs: Vec<PaneTab>,
    /// Pane-tab ids in most-recently-used order (front = active). Drives
    /// Ctrl+Tab when Settings → Keyboard → Ctrl+Tab order is "Recently
    /// used"; "Creation order" ignores it and walks `tabs` positionally.
    pub(crate) tab_mru: Vec<usize>,
    /// Inactive top-level workspace tabs (Worktree = workspace > Tab > Panes).
    /// The ACTIVE tab's state lives in the live fields (`tabs`, `split_tree`,
    /// `active_tab_id`, `active_pane_id`, …) so existing pane code paths keep
    /// working untouched; inactive tabs park here verbatim and swap back on
    /// selection. Pane-tab ids stay globally unique via `next_tab_id`, so a
    /// parked tab can never collide in host/session maps.
    pub(crate) parked_workspace_tabs: Vec<WorkspaceTab>,
    /// Header strip order (surface ids, including the active one).
    pub(crate) workspace_tab_order: Vec<usize>,
    pub(crate) workspaces: Vec<dragent_persistence::SerializedWorkspace>,
    pub(crate) closed_workspace_roots: Vec<PathBuf>,
    pub(crate) active_workspace_tab_id: usize,
    pub(crate) active_workspace_tab_name: String,
    pub(crate) next_workspace_tab_id: usize,
    /// (surface id, foreground-command summary) awaiting close confirmation.
    pub(crate) pending_close_project_workspace: Option<usize>,
    pub(crate) pending_close_workspace_tab: Option<(usize, String)>,
    pub(crate) close_workspace_tab_error: Option<String>,
    /// Surface id whose inline rename editor is open in the header strip.
    pub(crate) renaming_workspace_tab: Option<usize>,
    /// Inline rename editor rendered inside the header-strip chip.
    pub(crate) workspace_tab_rename_editor: ViewHandle<InputEditor>,
    /// Horizontal scroll for the header workspace-tab strip.
    pub(crate) workspace_tab_strip_scroll: ClippedScrollStateHandle,
    /// Per-chip drag state for reordering the header workspace tabs.
    pub(crate) workspace_tab_drag_states:
        RefCell<HashMap<usize, warpui_core::elements::DraggableState>>,
    /// Agent quick-access rail beside the right sidebar: false shows icons
    /// only, true shows icon + name + install actions. Workspace chrome —
    /// never rendered without an open workspace.
    pub(crate) agent_rail_expanded: bool,
    /// Vertical scroll for the agent rail's agent catalog.
    pub(crate) agent_rail_scroll: ClippedScrollStateHandle,
    /// Installed user-provided agents (`custom:<slug>` ids), refreshed
    /// alongside `prompt_installed_agents` by the same background probe.
    pub(crate) custom_installed_agents: std::collections::HashSet<String>,
    /// User-provided custom agent with an open installer modal, if any.
    /// Built-ins use `prompt_install_agent`; customs carry their definition
    /// snapshot here so the shared modal needs no second implementation.
    pub(crate) custom_install_agent: Option<CustomAgentDefinition>,
    /// Cached custom definitions for render paths (registry lookups must
    /// never hit the settings file per frame). Refreshed with detection.
    pub(crate) cached_custom_agents: Vec<CustomAgentDefinition>,
    /// Open project containers outlive their terminal tabs. This is the
    /// workspace-level identity used by the empty project surface and sidebar.
    pub(crate) open_project_roots: Vec<PathBuf>,
    pub(crate) selected_project_root: Option<PathBuf>,
    /// Owns live surfaces independently from their current tab/dock placement.
    /// Compatibility placement structs retain clones until generic layout
    /// mounting becomes the sole renderer.
    pub(crate) surface_registry: SurfaceRegistry<WorkspaceRuntimeSurface>,
    pub(crate) document_surface_id: SurfaceId,
    pub(super) workspace_work_item_id: WorkItemId,
    pub(crate) work_surface_ids: WorkSurfaceIds,
    /// Cached release gate used by rendering and action dispatch. Settings
    /// changes refresh this value immediately without rebuilding the workspace.
    pub(crate) agentic_workspace_enabled: bool,
    /// Present only under the loopback automation gate. Render updates this
    /// snapshot so E2E tests observe the same canonical projection as users.
    pub(crate) workspace_automation_state: Option<Arc<std::sync::Mutex<serde_json::Value>>>,
    /// Persisted client-local layout instances. Develop still uses the mature
    /// compatibility renderer; Shape and future presentations select state
    /// through this collection without rebuilding registered surfaces.
    pub(crate) layout_collection: LayoutCollection,
    /// Overlay state for the workspace-level layout switcher in the stable
    /// header. Layout selection itself lives in `layout_collection`.
    pub(crate) layout_menu_open: bool,
    pub(crate) review_menu_open: bool,
    /// Shape and Coordinate retain independent two-axis viewport positions.
    /// Switching layouts changes presentation without losing the user's place.
    pub(crate) shape_canvas_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) shape_canvas_vertical_scroll: ClippedScrollStateHandle,
    pub(crate) coordinate_canvas_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) coordinate_canvas_vertical_scroll: ClippedScrollStateHandle,
    pub(crate) coordinate_activity_scroll: ClippedScrollStateHandle,
    pub(crate) coordinate_activity_dock_state: ResizableStateHandle,
    pub(crate) review_canvas_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) review_canvas_vertical_scroll: ClippedScrollStateHandle,
    /// Independent editor-hosted work viewports. The Conversation composer
    /// remains fixed, and switching to Plan does not inherit its scroll offset.
    pub(crate) work_item_conversation_scroll: ClippedScrollStateHandle,
    /// Cross-block drag-to-select state for the Conversation scrollback —
    /// same mechanism the terminal's own scratch-shell output uses.
    pub(crate) work_item_conversation_selectable: warpui::elements::SelectionHandle,
    pub(crate) work_item_plan_scroll: ClippedScrollStateHandle,
    pub(crate) work_item_coordinate_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) work_item_coordinate_vertical_scroll: ClippedScrollStateHandle,
    pub(crate) work_item_changes_scroll: ClippedScrollStateHandle,
    pub(crate) work_item_review_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) work_item_agent_timeline_scroll: ClippedScrollStateHandle,
    pub(crate) work_item_review_vertical_scroll: ClippedScrollStateHandle,
    /// Real multiline shaping input. Its text is transient draft UI state;
    /// submitted messages become durable semantic workspace events.
    pub(crate) shape_composer_editor: ViewHandle<InputEditor>,
    pub(crate) shape_composer_text: String,
    /// Uses the same attachment loading and multimodal payload types as the
    /// standard terminal agent composer. These remain private to one work item.
    pub(crate) shape_composer_attachments: Vec<crate::agent::attachments::LoadedAttachment>,
    pub(crate) shape_composer_notice: Option<String>,
    /// Principal composer model picker open state — same global-model picker
    /// affordance as the terminal agent input (branch UI deliberately omitted).
    pub(crate) shape_model_menu_open: bool,
    pub(crate) shape_voice_ui: crate::voice::VoiceUiState,
    pub(crate) shape_voice_gen: u64,
    pub(crate) shape_voice_spin: usize,
    pub(crate) shape_voice_session: Option<crate::voice::VoiceSession>,
    pub(crate) shape_voice_fold: crate::voice::fold::VoiceFold,
    pub(crate) shape_voice_rebase_pending: bool,
    /// Finished dictation held for explicit user confirmation (Settings →
    /// Voice → "Confirm before insert"). The next mic toggle inserts it.
    pub(crate) shape_voice_pending_confirm: Option<String>,
    /// Work Item Flow gate: true once the user confirmed requirements AND
    /// clicked Approve Shape for this work item. Only then may turns create
    /// or revise the shape. Per-work transient state, parked on tab switch
    /// like the rest of the turn UI; restarts default to unapproved (safe).
    pub(crate) shape_approved: bool,
    /// Set only by Approve Shape: the upcoming turn must return proposal
    /// JSON, driven by the bounded force loop below. Consumed at submit.
    pub(crate) shape_force_proposal: bool,
    /// Enduring per-work turn memory (opencode-style): digest lines of prior
    /// tool findings + answered questions, fed into every turn so the model
    /// never re-reads researched files or re-asks settled questions.
    /// Session-scoped per work item (parked on tab switch); bounded.
    pub(crate) shape_turn_memory: Vec<String>,
    /// Set when a Gather turn returns requirements_complete: the Principal
    /// declares requirements fully gathered and confirmed-summary presented.
    /// Unlocks the Approve Shape button. Reset on every new submit (fresh
    /// requirements invalidate prior completeness) and parked per work item.
    pub(crate) shape_requirements_complete: bool,
    /// The running Principal turn, saved with the session while it runs: a
    /// turn cut short by a crash is resumed on the next start (once).
    pub(crate) shape_interrupted_turn: Option<dragent_persistence::SerializedShapeTurn>,
    /// The interrupted turn `submit_shape_message` resumes instead of the
    /// composer's text. Consumed by that call.
    pub(crate) shape_resume_turn: Option<dragent_persistence::SerializedShapeTurn>,
    pub(crate) shape_principal_pending: bool,
    pub(crate) shape_principal_request_epoch: u64,
    pub(crate) shape_record_editor: ViewHandle<InputEditor>,
    pub(crate) shape_record_edit_target: Option<super::ShapeRecordEditTarget>,
    pub(crate) shape_record_edit_text: String,
    pub(crate) shape_record_edit_pending: bool,
    pub(crate) shape_record_edit_error: Option<String>,
    pub(crate) shape_record_edit_epoch: u64,
    /// Armed remove target for two-step deletion confirmation. First press
    /// on Remove arms ("Confirm remove"); second press executes.
    pub(crate) shape_remove_armed: Option<super::ShapeRecordEditTarget>,
    pub(crate) shape_selected_role: Option<dragent_workspace_model::WorkContractId>,
    /// Set when a model-route edit changed execution inputs on an approved
    /// plan. Consumed on save success by resetting the plan to Draft through
    /// the authority — never by mutating UI state.
    pub(crate) shape_plan_reset_after_edit: bool,
    pub(crate) shape_model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    pub(crate) shape_role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    pub(crate) shape_role_action_error: Option<String>,
    pub(crate) shape_role_action_epoch: u64,
    pub(crate) shape_coordination_pending: bool,
    /// When the coordination request started. A stuck flag (background
    /// worker died without answering) used to wedge Start Coordination
    /// silently forever; the approve path now times it out visibly.
    pub(crate) shape_coordination_pending_since: Option<i64>,
    pub(crate) shape_coordination_error: Option<String>,
    /// Startup recovery notice (interrupted execution sessions found at
    /// launch). Informational only — kept separate from coordination errors
    /// so it is never mislabeled as a coordination failure. Cleared once a
    /// fresh coordination succeeds and supersedes it.
    pub(crate) shape_recovery_notice: Option<String>,
    pub(crate) shape_coordination_epoch: u64,
    /// Last authority-assigned sequence for the active work item. Strict
    /// artifact edits use this exact revision; compact projection sequence is
    /// never substituted for it.
    pub(super) workspace_canonical_revision: u64,
    pub(crate) active_tab_id: usize,
    pub(crate) next_tab_id: usize,
    pub(crate) active_pane_id: Option<EntityId>,
    /// The editor pane (reserved layout id, no tab) holds keyboard focus.
    /// Terminal focus always clears it; editor clicks set it. Drives only
    /// the pane focus ring — `active_tab_id` keeps pointing at a real tab.
    pub(crate) editor_pane_focused: bool,
    /// When set, the main area shows these tabs in a horizontal split (split-tab).
    /// Live pane layout: at most one tree per window. `None` means a single
    /// tab is shown and no dividers exist.
    pub(crate) split_tree: Option<PaneLayout>,
    /// Transient drag state per divider, keyed by `(split id, divider
    /// index)`. Weights live in the tree; these exist only so a drag
    /// survives across re-renders, and are pruned on structural changes.
    pub(crate) split_drag_handles: RefCell<HashMap<(SplitId, usize), DragResizeHandle>>,
    /// True while a sidebar tab is being dragged — gates the pane drop-zone overlays.
    pub(crate) drag_in_progress: bool,
    pub(crate) split_close_mouse_states: RefCell<HashMap<usize, MouseStateHandle>>,
    pub(crate) split_action_mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) header_mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) new_tab_mouse_state: MouseStateHandle,
    pub(crate) left_panel_visible: bool,
    pub(crate) right_panel_visible: bool,
    pub(crate) workspace_trust: super::trust_store::WorkspaceTrustStore,
    pub(crate) add_project: Option<super::add_project::AddProjectDialog>,
    /// Machines whose paths were reachable at the last store update; remote
    /// views reload only when this changes, not on every health poll.
    pub(crate) remote_connected: Vec<String>,
    pub(crate) project_start_recent_projects: Vec<PathBuf>,
    pub(crate) project_start_recent_ssh_hosts: Vec<super::ssh::SshDestination>,
    pub(crate) project_start_ssh_config_hosts: Vec<super::ssh::SshDestination>,
    pub(crate) pending_workspace_trust: Option<PathBuf>,
    /// Folder picked for opening that is not a git checkout yet:
    /// confirm `git init` before importing it as a project.
    pub(crate) pending_git_init_root: Option<PathBuf>,
    /// File selection that triggered trust. It is opened only after approval,
    /// so declining trust never reads the requested document.
    pub(crate) pending_workspace_trust_document: Option<PathBuf>,
    /// Operation resumed after the user approves the selected project root.
    pub(crate) pending_project_folder_intent: Option<ProjectFolderIntent>,
    /// First-run project chooser. The underlying terminal exists only as a
    /// reusable implementation surface and is not shown until explicitly
    /// requested by the user.
    pub(crate) project_start_visible: bool,
    pub(crate) project_start_mode: Option<super::project_start::ProjectStartMode>,
    pub(crate) project_start_editor: ViewHandle<InputEditor>,
    pub(crate) project_start_text: String,
    pub(crate) project_start_busy: bool,
    pub(crate) project_start_error: Option<String>,
    pub(crate) error_notices: std::collections::VecDeque<(String, String)>,
    pub(crate) error_notice_scroll: ClippedScrollStateHandle,
    pub(crate) error_notice_copied: Option<String>,
    pub(crate) pending_clone_repository: Option<String>,
    /// Add-project popover (anchored to the sidebar add button) and the
    /// centered create-project modal. Window-level overlays, like the
    /// new-tab picker.
    pub(crate) add_project_menu: AddProjectMenu,
    pub(crate) project_context_menu_root: Option<PathBuf>,
    pub(crate) new_project_group_root: Option<PathBuf>,
    pub(crate) project_group_error: Option<String>,
    pub(crate) project_group_name_editor: ViewHandle<InputEditor>,
    pub(crate) pending_project_removal: Option<(PathBuf, bool)>,
    pub(crate) pending_project_group_removal: Option<String>,
    pub(crate) project_removal_busy: bool,
    pub(crate) project_removal_error: Option<String>,
    pub(crate) importable_project_worktrees: Option<(PathBuf, Vec<(PathBuf, String)>)>,
    pub(crate) import_project_worktrees_busy: bool,
    pub(crate) import_project_worktrees_error: Option<String>,
    pub(crate) review_ship: ReviewShipState,
    pub(crate) review_pr_tick: u64,
    pub(crate) review_pr_request_epoch: u64,
    pub(crate) review_pr_poll_scheduled: bool,
    /// A coalesced sidebar sync / session save is pending (see
    /// `schedule_left_panel_sync`).
    pub(crate) left_panel_sync_scheduled: bool,
    pub(crate) agent_theme_restarts_scheduled: bool,
    /// A background pass is working out workspaces' project headings.
    pub(crate) project_roots_resolving: std::cell::Cell<bool>,
    pub(crate) project_icons_resolving: std::cell::Cell<bool>,
    /// This run has checked every folder's heading once (one may have moved).
    pub(crate) project_roots_checked: std::cell::Cell<bool>,
    pub(crate) session_save_scheduled: bool,
    pub(crate) review_pr_cache: HashMap<PathBuf, CachedReviewPr>,
    /// Headless Review-menu action state (push/PR run in the background
    /// with menu feedback, never in a terminal).
    pub(crate) review_action_busy: Option<super::action_review_git::ReviewGitAction>,
    pub(crate) review_action_notice: Option<String>,
    pub(crate) review_action_error: Option<String>,
    /// Per-worktree git status for the sidebar trailing slot, keyed by
    /// canonical worktree root. Refreshed off-thread on terminal events;
    /// renders read the cache only, never spawn git themselves.
    pub(crate) worktree_git_cache: HashMap<PathBuf, super::worktree_status::WorktreeGitStatus>,
    /// Roots whose git status query is running; a refresh for one of them
    /// waits for that answer instead of starting another git.
    pub(crate) worktree_git_inflight: std::cell::RefCell<std::collections::HashSet<PathBuf>>,
    /// Per-pane agent completion records for the sidebar review dot:
    /// a successful stop newer than its last-seen moment renders green.
    /// Keyed by pane UUID; pruned as panes close.
    pub(crate) agent_stop_state: HashMap<uuid::Uuid, super::worktree_status::AgentStopState>,
    pub(crate) create_project_open: bool,
    pub(crate) create_project_location: PathBuf,
    pub(crate) create_project_error: Option<String>,
    /// New-worktree modal (branch prompt prefilled with the local branch).
    /// The worktree is checked out under the managed root, outside the repo.
    pub(crate) worktree_prompt_open: bool,
    pub(crate) worktree_rename_branch: bool,
    pub(crate) worktree_base_root: Option<PathBuf>,
    pub(crate) worktree_start_branch: Option<String>,
    pub(crate) worktree_busy: bool,
    pub(crate) worktree_error: Option<String>,
    pub(crate) worktree_branch_editor: ViewHandle<InputEditor>,
    /// Prompt-first project screen: prompt editor, picked agent, mirrored
    /// prompt text, open menu, and cached branch/worktree listings.
    pub(crate) prompt_editor: ViewHandle<InputEditor>,
    pub(crate) prompt_agent: super::action::PromptAgent,
    /// The prompt goes to a team the picked agent leads, which may give
    /// work to `prompt_team_agents` (and to its own kind of agent).
    pub(crate) prompt_team: bool,
    pub(crate) prompt_lead_model: Option<String>,
    pub(crate) prompt_model_tab: crate::teams::models::ModelTab,
    pub(crate) prompt_team_agents: Vec<super::action::PromptAgent>,
    pub(crate) prompt_team_cost: dragent_host::team::Cost,
    pub(crate) prompt_installed_agents: HashSet<super::action::PromptAgent>,
    pub(crate) prompt_agent_checking: bool,
    pub(crate) prompt_agent_check_generation: u64,
    /// A background agent-detection pass is running; refreshes requested
    /// meanwhile set `prompt_agent_refresh_pending` instead of restarting it.
    pub(crate) prompt_agent_refresh_in_flight: bool,
    pub(crate) prompt_agent_refresh_pending: bool,
    pub(crate) prompt_install_agent: Option<super::action::PromptAgent>,
    pub(crate) prompt_install_terminal: Option<ViewHandle<TerminalView>>,
    /// Interactive command shown over its originating page, never a workspace tab.
    pub(crate) page_command: Option<(String, String)>,
    pub(crate) prompt_install_error: Option<String>,
    pub(crate) prompt_install_generation: u64,
    /// The install sheet's subject when it installs a tool on a machine
    /// (Settings → Machines → System tools) rather than an agent here.
    pub(crate) remote_tool_install: Option<super::remote_project::RemoteToolJob>,
    pub(crate) remote_tool_checking: bool,
    pub(crate) prompt_text: String,
    pub(crate) prompt_attachments: Vec<crate::agent::attachments::LoadedAttachment>,
    pub(crate) prompt_links: Vec<super::project_links::PromptLink>,
    /// A task being started from the Tasks page: it names the new workspace
    /// and gets linked to it.
    pub(crate) prompt_task: Option<dragent_tasks::Task>,
    /// More selected tasks to start, one after another, once the one in the
    /// prompt has its workspace.
    pub(crate) task_start_queue: Vec<dragent_tasks::Task>,
    /// A task (or tracker item) whose Start hit a project that isn't open
    /// locally: the clone form opens prefilled, and the start resumes
    /// automatically once that project opens.
    pub(crate) pending_task_start: Option<super::tasks_host::PendingStart>,
    pub(crate) prompt_attachment_scroll: ClippedScrollStateHandle,
    pub(crate) prompt_link_results: Vec<super::project_links::PromptLink>,
    pub(crate) prompt_link_loading: bool,
    pub(crate) prompt_link_error: Option<String>,
    pub(crate) prompt_link_generation: u64,
    pub(crate) prompt_link_query: String,
    pub(crate) prompt_link_search_editor: ViewHandle<InputEditor>,
    pub(crate) prompt_notice: Option<String>,
    pub(crate) prompt_voice_ui: crate::voice::VoiceUiState,
    pub(crate) prompt_voice_session: Option<crate::voice::VoiceSession>,
    pub(crate) prompt_voice_gen: u64,
    pub(crate) prompt_voice_spin: usize,
    pub(crate) prompt_voice_fold: crate::voice::fold::VoiceFold,
    /// Finished dictation held for explicit user confirmation (Settings →
    /// Voice → "Confirm before insert"). The next mic toggle inserts it.
    pub(crate) prompt_voice_pending_confirm: Option<String>,
    pub(crate) prompt_menu: Option<super::action::PromptMenu>,
    pub(crate) prompt_menu_scroll: ClippedScrollStateHandle,
    pub(crate) prompt_device_remote_open: bool,
    pub(crate) prompt_branch: Option<String>,
    pub(crate) prompt_selected_worktree: Option<PathBuf>,
    pub(crate) prompt_branches: Vec<String>,
    pub(crate) prompt_branch_search_editor: ViewHandle<InputEditor>,
    pub(crate) prompt_branch_search: String,
    pub(crate) prompt_agent_search_editor: ViewHandle<InputEditor>,
    pub(crate) prompt_agent_search: String,
    pub(crate) prompt_worktrees: Vec<super::action::PromptWorktree>,
    pub(crate) clone_url_editor: ViewHandle<InputEditor>,
    pub(crate) clone_url_error: Option<String>,
    pub(crate) clone_location_editor: ViewHandle<InputEditor>,
    pub(crate) clone_url_text: String,
    pub(crate) clone_location_text: String,
    pub(crate) clone_busy: bool,
    pub(crate) create_project_name_editor: ViewHandle<InputEditor>,
    pub(crate) create_project_location_editor: ViewHandle<InputEditor>,
    /// The user explicitly closed the right panel, so auto-visibility must stop
    /// reopening it. `apply_sidebar_visibility` re-derives the panel from "is the
    /// active pane in a git repo" and runs on cwd changes, pane switches, tab
    /// changes and startup — which silently undid a deliberate close. Cleared
    /// only when the user opens the panel again.
    pub(crate) right_panel_user_closed: bool,
    /// The sidebar as a proper first-class View — owns all sidebar state and
    /// its editor children so they appear in the presenter's parent map under
    /// LeftPanelView rather than directly under WorkspaceView.
    pub(crate) left_panel_view: ViewHandle<LeftPanelView>,
    /// Project info sidebar (right panel) — shows git/runtime info for the active CWD.
    pub(crate) project_sidebar_model: warpui::ModelHandle<ProjectSidebarModel>,
    pub(crate) project_sidebar_view: ViewHandle<ProjectSidebarView>,
    /// Live git snapshot shared with the sidebar's source-control panel.
    pub(crate) git_panel_state:
        std::rc::Rc<std::cell::RefCell<super::project_sidebar::GitPanelState>>,
    /// Workspace-owned git status for folder-only state (no terminal tab).
    /// Terminals own their status model; when none exists this keeps the
    /// sidebar and bottom rail populated for the selected folder.
    pub(crate) folder_git_model: Option<warpui::ModelHandle<crate::git_status::GitRepoStatusModel>>,
    pub(crate) folder_git_root: Option<PathBuf>,
    pub(crate) persistence_writer: WriterHandles,
    /// Only the primary window owns the shared restart snapshot.
    pub(crate) persists_session_snapshot: bool,
    /// Bounded semantic projection for the engineering-workspace surfaces.
    /// Terminal output remains owned by each pane; this stores only metadata
    /// events so the workspace cannot duplicate command contents in memory.
    pub(super) workspace_state: WorkspaceState,
    /// Inactive top-level tabs retain independent canonical projections and
    /// execution registries here. The active tab remains in the established
    /// fields to keep the execution path focused and allocation-free.
    pub(crate) parked_workspace_work: HashMap<WorkItemId, ParkedWorkspaceWork>,
    /// Every pane's persisted agent blocks, loaded once at boot (a bulk load
    /// of the `agent_blocks` table keyed by pane UUID). A work item's own id
    /// is its pane UUID, so a work item opened in a tab for the first time
    /// this run, well after boot, restores its conversation into a fresh
    /// `AgentController` exactly like `TerminalView::new_with_restore` does.
    pub(crate) workspace_boot_restored_agent_blocks: persistence::RestoredAgentBlockLists,
    /// One agent conversation per work item — the terminal agent's own
    /// controller + block renderer (`AgentConversationView`), created lazily
    /// by `ensure_work_item_agent`. Keyed per item so a tab switch never
    /// touches another item's turn.
    pub(crate) work_item_agents:
        HashMap<WorkItemId, ViewHandle<crate::agent::conversation_view::AgentConversationView>>,
    /// One agent conversation per mounted execution attempt: each worker /
    /// reviewer runs on the terminal agent's own `AgentController` (contract
    /// tools + policy injected), rendered by the same `AgentConversationView`
    /// the Principal uses. Pane UUID = attempt id, so its transcript persists
    /// and restores like a terminal pane. Clicking a role opens this.
    /// Tasks agent CLIs are working on (see `cli_workers`), by attempt.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) cli_workers: HashMap<ExecutionAttemptId, super::cli_workers::CliWorker>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) cli_worker_poll_scheduled: bool,
    /// Lead replies already handed to a waiting `dragent work ask`, so the
    /// task thread doesn't type them in a second time.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) cli_worker_answered: std::collections::HashSet<dragent_workspace_model::AgentMessageId>,
    pub(super) attempt_agents: HashMap<
        ExecutionAttemptId,
        ViewHandle<crate::agent::conversation_view::AgentConversationView>,
    >,
    /// Attention items raised for a controller's parked approval / question
    /// card, keyed by (attempt, action id) so either side can resolve the other.
    pub(crate) attempt_attention: HashMap<(ExecutionAttemptId, Uuid), AttentionItemId>,
    /// Worker questions parked on the Principal's answer (`ask_principal`
    /// tool calls and workers' own `ask_user` cards), keyed by the
    /// DecisionRequest message id the reply will answer.
    pub(super) pending_worker_questions:
        HashMap<AgentMessageId, super::worker_tools::PendingWorkerQuestion>,
    /// Mutating host tools awaiting their canonical persistence outcome.
    /// Command ids bind each acknowledgement to exactly one accepted write.
    pub(crate) pending_workspace_tool_acknowledgements:
        HashMap<WorkspaceCommandId, PendingWorkspaceToolAcknowledgement>,
    /// Worker questions whose DecisionRequest command is awaiting SQLite.
    /// A rejected command must fail the parked tool/card instead of leaving it
    /// waiting for a Principal reply that can never exist.
    pub(super) pending_worker_question_submissions: HashMap<WorkspaceCommandId, AgentMessageId>,
    /// A Principal coordination turn in flight on a work item's conversation
    /// (epoch, the message it answers, its routing). Answered from
    /// `TurnCompleted` by parsing the structured reply.
    pub(crate) principal_coordination_turns:
        HashMap<WorkItemId, (u64, WorkspaceAgentMessage, AgentMessageRouting)>,
    /// Reply channel of the provider pass currently running on a work item's
    /// controller; answered from `AgentConversationEvent::TurnCompleted` /
    /// `Error`. Dropping one ends the orchestrator's turn ("stopped").
    pub(crate) principal_pass_replies: HashMap<
        WorkItemId,
        std::sync::mpsc::Sender<Result<super::shape_principal::PrincipalPassOutcome, String>>,
    >,
    /// Active execution sessions are workspace-owned, not render-owned. A
    /// layout switch therefore cannot duplicate or cancel provider work.
    pub(super) workspace_execution_runtime: WorkspaceExecutionRuntime,
    pub(crate) workspace_checkpoint_notice: Option<String>,
    /// Serializes scheduler authority mutations so every command is built
    /// against the latest accepted revision.
    pub(super) workspace_launch_pending: bool,
    /// When the in-flight launch request started. Same stuck-flag hazard as
    /// coordination requests: the launch loop times it out visibly instead
    /// of halting all execution driving silently.
    pub(crate) workspace_launch_pending_since: Option<i64>,
    /// Whether a Principal-coordination turn (the Principal replying to or
    /// escalating a worker's Blocker/DecisionRequest) is in flight. Distinct
    /// from `shape_coordination_pending`, which guards submitting the
    /// Start-Coordination command itself, not an ongoing coordination turn.
    pub(crate) workspace_principal_coordination_pending: bool,
    /// Same stuck-flag hazard as `workspace_launch_pending_since`.
    pub(crate) workspace_principal_coordination_pending_since: Option<i64>,
    /// What the running coordination turn is about ("a request from
    /// Backend engineer", "your message"), shown under the composer so the
    /// user knows the Principal is busy with an agent and why.
    pub(crate) workspace_principal_coordination_activity: Option<String>,
    /// Coordination targets this session already replied to. A reply the
    /// authority rejected leaves its target canonically unanswered; this
    /// keeps the driver from re-answering it forever (the rejection is shown
    /// as an error card instead).
    pub(crate) principal_replies_submitted: HashSet<AgentMessageId>,
    /// Bumped on every new coordination turn so a stale background stream's
    /// result — from a turn the stuck-flag guard already cleared — cannot
    /// clear a different, newer turn's pending flag.
    pub(crate) workspace_principal_coordination_epoch: u64,
    /// A coordination turn's escalation, queued for a `RaiseAttention`
    /// submission once its own reply has been accepted — commands share the
    /// single serialized submission path, so two commands from one turn's
    /// result must be submitted one at a time, not concurrently.
    pub(crate) workspace_principal_escalation_queue:
        VecDeque<dragent_workspace_model::AttentionItem>,
    /// Sequential commands from one coordination turn's add_tasks/
    /// propose_roles — see the `ParkedWorkspaceWork` field of the same name
    /// for why this needs a queue and not a single in-flight value.
    pub(crate) workspace_principal_plan_actions_queue: VecDeque<(
        WorkItemId,
        dragent_workspace_model::ActorId,
        WorkspaceCommandPayload,
    )>,
    pub(crate) workspace_execution_poll_scheduled: bool,
    pub(crate) workspace_retry_wakeup_scheduled: bool,
    /// Bounded project tree metadata; file contents are loaded only on demand.
    pub(crate) document_index: ProjectDocumentIndex,
    /// Root represented by `document_index`; changes when the active tab's CWD changes.
    pub(crate) document_index_root: PathBuf,
    /// Debounced filesystem watcher that keeps the visible explorer projection
    /// synchronized with files created, removed, or renamed outside the app.
    pub(crate) document_tree_watch_tx:
        async_channel::Sender<super::project_documents::TreeWatcherEvent>,
    pub(crate) document_tree_watch_root: Option<PathBuf>,
    pub(crate) document_tree_watch_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) document_tree_refresh_epoch: u64,
    /// When true, the selected document replaces the terminal in the center
    /// workspace until the user closes it.
    pub(crate) document_center_open: bool,
    /// Restored document state stays dormant until an explicit file click.
    /// Tabs, drafts, and dock metadata are retained without mounting the
    /// editor or issuing a file read during application startup.
    pub(crate) document_restore_pending: bool,
    /// Tab that owns the currently open editor. A new terminal tab must never
    /// inherit the previous tab's editor surface.
    pub(crate) document_tab_id: Option<usize>,
    /// Project root captured when the document was opened. The active terminal
    /// tab can move `document_index_root` while the editor is hidden; keeping
    /// the owner root separately lets the original tab restore its exact tree
    /// and resolve saves without duplicating the document buffer.
    pub(crate) document_owner_root: Option<PathBuf>,
    /// File requested for the current editor load. This disambiguates two
    /// rapid clicks within the same tab/root: an older asynchronous read must
    /// never replace the newer file when it completes later.
    pub(crate) document_requested_path: Option<PathBuf>,
    /// Open editor tabs in user-defined order. Only paths are retained here;
    /// buffers and recovery copies remain owned by `ProjectSidebarModel`.
    pub(crate) document_open_paths: Vec<PathBuf>,
    /// The one clean, replaceable editor tab created by an explorer/link click.
    /// Editing or manually reordering promotes it by clearing this marker.
    pub(crate) document_preview_path: Option<PathBuf>,
    pub(crate) document_tabs_scroll: ClippedScrollStateHandle,
    /// Horizontal scroll for the work item surface's view-route strip
    /// (Conversation/Plan/Coordinate/…). The strip is six fixed-width
    /// buttons, so a pane dragged narrower than their total would otherwise
    /// force the header wider than its pane and paint over the neighbour.
    pub(crate) work_item_view_tabs_scroll: ClippedScrollStateHandle,
    /// First-class editor tab for the active canonical work item. This remains
    /// presentation state; `WorkspaceState` owns the work itself.
    pub(crate) work_item_tab_open: bool,
    pub(crate) work_item_tab_active: bool,
    pub(crate) work_item_view: WorkItemView,
    pub(crate) workspace_agent_tabs: Vec<super::editor_tabs::AgentEditorTab>,
    pub(crate) workspace_active_agent_tab: Option<super::action::WorkspaceAgentTabId>,
    /// Selected runtime agent for the inspectable Agent tab. Stable
    /// identities only (task + attempt); presentation state, never execution
    /// ownership. History renders from the durable `WorkspaceState` event
    /// log, so opening or closing this tab cannot restart or alter work.
    pub(crate) workspace_selected_agent_task: Option<dragent_workspace_model::PlanTaskId>,
    pub(crate) workspace_selected_agent_attempt:
        Option<dragent_workspace_model::ExecutionAttemptId>,
    /// Full-history expansion for the agent timeline. Presentation only.
    pub(crate) workspace_agent_timeline_expanded: bool,
    /// Whether the active terminal is docked below the open document editor.
    pub(crate) document_terminal_visible: bool,
    /// Terminal group rendered inside the document dock. Kept separate from
    /// `tabs` so dock terminals never leak into the workspace tab/sidebar UI.
    pub(crate) document_terminals: Vec<DocumentTerminalTab>,
    pub(crate) document_terminal_active_id: Option<usize>,
    pub(crate) document_terminal_split_id: Option<usize>,
    pub(crate) document_terminal_next_id: usize,
    pub(crate) document_terminal_maximized: bool,
    /// Persistent vertical split size for the document-terminal layout.
    pub(crate) document_terminal_state: ResizableStateHandle,
    /// Horizontal viewport for the dock's terminal tabs. Toolbar actions live
    /// outside this viewport and therefore never move when tabs overflow.
    pub(crate) document_terminal_tabs_scroll: ClippedScrollStateHandle,
    /// Whether the session-type picker popup is visible.
    pub(crate) new_tab_picker_open: bool,
    /// Account picker popover for rail launches: agent id awaiting account choice.
    /// None = closed. System fallback applies when closed/launched without a pick.
    pub(crate) agent_account_picker: Option<String>,
    /// (tab_id, foreground command or empty string) awaiting close confirmation.
    pub(crate) pending_close_tab: Option<(usize, String)>,
    pub(crate) closing_worktree_tab: Option<usize>,
    pub(crate) closed_worktree_main_checkout: Option<PathBuf>,
    pub(crate) close_tab_error: Option<String>,
    pub(crate) close_other_tabs_keep_id: Option<usize>,
    /// Agents panel state: host agent runs across open projects.
    pub(crate) agents_panel_open: bool,
    pub(crate) agent_runs: Vec<super::agents_panel::AgentRunRow>,
    pub(crate) agents_busy: bool,
    pub(crate) agents_error: Option<String>,
    /// Dirty document awaiting a Save / Don't Save decision.
    pub(crate) pending_close_document: Option<PathBuf>,
    /// Pending hunk discard awaiting confirmation sheet.
    pub(crate) pending_discard_hunk: Option<(String, usize, usize, usize)>,
    pub(crate) explorer_clipboard: Option<ExplorerClipboardEntry>,
    pub(crate) explorer_prompt: Option<ExplorerPrompt>,
    pub(crate) explorer_prompt_editor: ViewHandle<InputEditor>,
    pub(crate) explorer_rename_pending: bool,
    pub(crate) pending_explorer_delete: Option<(PathBuf, bool)>,
    pub(crate) explorer_operation_error: Option<String>,
    /// Monotonic debounce generation for crash-recovery working-copy writes.
    pub(crate) document_backup_epoch: u64,
    /// Lazily started native LSP coordinator. It owns all process work on a
    /// dedicated thread and is absent until a supported document is opened.
    pub(crate) language_services: Option<LanguageServiceClient>,
    pub(crate) language_server_catalog: Arc<LanguageServerCatalog>,
    pub(crate) language_document_sync: Option<LanguageDocumentSyncState>,
    /// Last known states for every server attached to the active document.
    /// Rendering never queries the worker.
    pub(crate) language_service_status: Vec<super::language_services_tray::ServiceSnapshot>,
    pub(crate) language_navigation_capabilities:
        HashMap<LanguageServerKey, LanguageNavigationCapabilities>,
    pub(crate) language_file_rename_servers: HashSet<LanguageServerKey>,
    pub(crate) language_services_open: bool,
    /// Missing servers whose automatic install prompt has already been shown.
    /// This avoids reopening the tray after a user deliberately dismisses it.
    pub(crate) language_service_install_prompts: HashSet<LanguageServerKey>,
    pub(crate) language_services_enabled: bool,
    pub(crate) language_sync_epoch: u64,
    pub(crate) language_event_subscription_epoch: u64,
    pub(crate) language_completion_epoch: u64,
    pub(crate) language_completion_token: Option<u64>,
    /// Non-completion requests are tracked by token because responses may
    /// arrive out of order while the user continues editing.
    pub(crate) language_request_tokens: HashMap<u64, LanguageRequestPurpose>,
    pub(crate) pending_language_navigation: Option<(PathBuf, TextRange)>,
    pub(crate) definition_hover_latest_request_id: Option<u64>,
    pub(crate) definition_hover_resolution: Option<DefinitionHoverResolution>,
    /// True when the picker was opened via the sidebar button (anchors beside it),
    /// false when opened via the header + button (anchors top-right).
    pub(crate) new_tab_picker_from_sidebar: bool,
    pub(crate) new_tab_picker_mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) settings_view: ViewHandle<SettingsView>,
    /// The Automations page replaces the content area while open.
    pub(crate) automations_open: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) automations_view: ViewHandle<crate::automations::AutomationsView>,
    /// The Teams page replaces the content area while open.
    pub(crate) pull_requests_open: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) pull_requests_view: ViewHandle<crate::pull_requests::PullRequestsView>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) review_setup: Option<crate::pull_requests::ReviewSetup>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) review_instructions_editor: ViewHandle<InputEditor>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) review_instructions: String,
    pub(crate) teams_open: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) teams_view: ViewHandle<crate::teams::TeamsView>,
    /// The Tasks page replaces the content area while open.
    pub(crate) tasks_open: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) tasks_view: ViewHandle<crate::tasks_page::TasksView>,
    /// Messages for the person waiting unread in teams at work (the
    /// sidebar badge), and the teams they're in, to notify of new ones.
    pub(crate) teams_needing_you: usize,
    /// Each coordination's lead and task checkouts (the sidebar groups).
    pub(crate) coordination_groups: Vec<crate::teams::sidebar::CoordinationGroup>,
    pub(crate) teams_waiting: std::collections::HashMap<String, usize>,
    pub(crate) teams_badge_seeded: bool,
    /// Teams whose Dragent lead is being handed its news right now.
    pub(crate) lead_news_pending: std::collections::HashSet<String>,
    /// Screen-level navigation. Settings never owns a workspace tab or pane.
    pub(crate) settings_open: bool,
    pub(crate) settings_project_root: Option<PathBuf>,
    pub(crate) account_logged_in_cache: bool,
    /// Header account menu (avatar → documentation / upgrade / sign out).
    pub(crate) account_menu_open: bool,
    /// Header usage popover, and the agent tab it shows.
    pub(crate) usage_menu_open: bool,
    pub(crate) usage_menu_agent: Option<super::action::PromptAgent>,
    /// Sideways scroll for the usage popover's agent-icon row.
    pub(crate) usage_tabs_scroll: ClippedScrollStateHandle,
    /// Latest plan-limit readings per agent account, and whether a read is
    /// under way.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) agent_usage: Vec<dragent_host::agent_usage::AccountUsage>,
    pub(crate) agent_usage_loading: bool,
    /// The "Past sessions" list is open, with what it found.
    pub(crate) past_sessions_open: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) past_sessions: Vec<dragent_host::agent_history::PastSession>,
    pub(crate) past_sessions_loading: bool,
    pub(crate) past_sessions_scroll: warpui::elements::ClippedScrollStateHandle,
    /// Its search field, and what is typed in it.
    pub(crate) past_sessions_search_editor: ViewHandle<InputEditor>,
    pub(crate) past_sessions_search: String,
    /// The conversation whose Delete is waiting to be confirmed.
    pub(crate) past_sessions_confirm: Option<usize>,
    /// What is happening ("Deleting…"), and what last happened.
    pub(crate) past_sessions_busy: Option<String>,
    pub(crate) past_sessions_note: Option<String>,
    /// Space deleted conversations left inside OpenCode's and MiMo's
    /// databases: (agent command, bytes).
    pub(crate) past_sessions_reclaimable: Vec<(String, u64)>,
    /// "Log out?" confirmation, and its "Don't ask again" checkbox.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_asks: super::computer_tools::ComputerAsks,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_trusted_apps: std::collections::HashSet<String>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_ask_scroll: ClippedScrollStateHandle,
    pub(crate) logout_confirm_open: bool,
    pub(crate) logout_dont_ask: bool,
    /// Sidebar organisation area: organisation menu, its open submenu, Help.
    pub(crate) org_menu_open: bool,
    pub(crate) org_submenu: Option<super::org_menu::OrgSubmenu>,
    pub(crate) help_menu_open: bool,
    /// The Give feedback dialog, while open.
    pub(crate) feedback: Option<super::feedback::FeedbackDraft>,
    /// Mono font, for the URL in the share sheet.
    pub(crate) mono_font: FamilyId,
    /// Whether the share-session sheet is visible.
    pub(crate) share_open: bool,
    /// Whether the QR sub-sheet is visible (over the share sheet).
    pub(crate) share_qr_open: bool,
    /// Asset id of the QR bitmap currently registered for the share URL.
    pub(crate) share_qr_asset: Option<String>,
    /// Which share-sheet dropdown, if any, is open.
    pub(crate) share_open_dropdown: Option<super::action::ShareDropdown>,
    /// The live email-invite input for the share sheet.
    pub(crate) share_email_editor: ViewHandle<InputEditor>,
    /// The local-port input owned by the standalone Ports panel.
    pub(crate) ports_editor: ViewHandle<InputEditor>,
    /// Navbar attention bell dropdown: open attention across all work items
    /// with routing, so approvals/answers never require hunting the
    /// Coordinate rail.
    pub(crate) attention_bell_open: bool,
    /// Attention item pinned as the dropdown detail card; None shows the
    /// request list.
    pub(crate) attention_popover_item: Option<AttentionItemId>,
    /// Response input for decision/failure attention in the popover.
    pub(crate) attention_response_editor: ViewHandle<InputEditor>,
    pub(crate) attention_response_text: String,
    /// Session standing approvals: (work item, tool name) pairs the owner
    /// marked "always allow". Each auto-approval still emits a canonical
    /// ResolveAttention event; nothing here survives restart.
    pub(crate) workspace_standing_tool_approvals: HashSet<(WorkItemId, String)>,
    /// Whether the VS Code-style Ports panel is expanded.
    pub(crate) ports_open: bool,
    /// Transient result text shown in the Ports panel.
    pub(crate) ports_notice: Option<String>,
    pub(crate) ports_notice_epoch: u64,
    /// Idle agents are being checked off the UI thread (one sweep at a time).
    pub(crate) agent_sleep_sweeping: bool,
    /// "Put N idle agents to sleep" notice, and its epoch for the timeout.
    pub(crate) agent_sleep_notice: Option<String>,
    pub(crate) agent_sleep_notice_epoch: u64,
    /// An agent start held while memory is critical, awaiting an answer.
    /// The held start is running now: skip the memory check once.
    /// Cached share snapshot, refreshed on `SessionManagerEvent::Changed`.
    pub(crate) share_snapshot: share_sheet::ShareSnapshot,
    /// Whether the "sign in to share" prompt is visible.
    pub(crate) login_prompt_open: bool,
    /// The remote MIRROR view (when this workspace has joined a shared session),
    /// fed the host's broadcast output.
    pub(crate) mirror_pane: Option<ViewHandle<crate::terminal::view::TerminalView>>,
    /// The tab id of the joined mirror session (for showing the viewer bar).
    pub(crate) mirror_tab_id: Option<usize>,
    /// Set when the host ends a joined session so the mirror does not fall back
    /// to its empty/connecting surface.
    pub(crate) shared_session_ended: Option<String>,
    /// Host-session mirror binding (agents panel / adopt flow). The pane is
    /// fed by `poll_host_mirror`; clearing the binding stops the pump while
    /// the host run continues headless.
    pub(crate) host_mirror_pane: Option<ViewHandle<crate::terminal::view::TerminalView>>,
    pub(crate) host_mirror_tab_id: Option<usize>,
    pub(crate) host_mirror: Option<super::host_mirror::HostMirror>,
    pub(crate) mirror_client: Option<dragent_host::HostClient>,
    /// Host-OWNED terminal tabs (daemon PTYs that outlive the app), keyed by
    /// tab id. Fed by per-tab output streams; tab close disposes the session
    /// while app quit only detaches, so relaunch re-attaches to live sessions.
    pub(crate) host_owned_tabs: HashMap<usize, super::host_tabs::HostOwnedTab>,
    /// Where every host-tab stream worker delivers (created on first use).
    pub(crate) host_stream_tx: Option<super::host_tabs::HostStreamSender>,
    /// Host: a pending Edit request awaiting approve/decline (participant id, name).
    pub(crate) pending_access_request: Option<(String, String)>,
    /// Whether the content-header ⋮ share popover is open.
    pub(crate) content_header_menu_open: bool,
    /// Split pane whose ⋮ share menu is open (anchored to that pane).
    pub(crate) split_share_menu_tab_id: Option<usize>,
    /// Whether the "End session?" confirmation modal is open.
    pub(crate) stop_confirm_open: bool,
    /// Tab awaiting the host's choice of what participants see on join
    /// (scrollback vs clean screen). `None` when the chooser is closed.
    pub(crate) share_scope_tab: Option<usize>,
    /// Transient confirmation shown in the share sheet ("Link copied", "Invite
    /// sent"). Every networked control acknowledges itself: a spinner while it
    /// runs, this pill once it lands. Cleared by a timer (`share_notice_epoch`
    /// invalidates a pending clear if a newer notice replaces it).
    pub(crate) share_notice: Option<String>,
    pub(crate) share_notice_epoch: u64,
    /// Host: whether the inactivity-warning modal is open (session idle 10m,
    /// auto-ends in 5m unless kept).
    pub(crate) idle_warning_open: bool,
    /// Host: whether the content-header participants popover is open.
    pub(crate) participants_menu_open: bool,
    /// Host: the participant id whose access sub-menu (View/Edit/Remove) is open.
    pub(crate) participant_access_open: Option<String>,
    /// Host-side live collaborator pointers, keyed by participant id.
    pub(crate) remote_pointers: HashMap<ParticipantId, RemotePointer>,
}

/// Two-click discard arm lifetime for folder-only git ops.
pub(crate) const FOLDER_GIT_DISCARD_ARM_SECS: u64 = 6;

impl WorkspaceView {
    /// Re-read the shared-session snapshot from the `SessionManager` singleton.

    pub(crate) fn active_builtin_layout(&self) -> BuiltInLayout {
        if self.project_start_visible || self.settings_open {
            return BuiltInLayout::Develop;
        }
        let persisted = self
            .layout_collection
            .active()
            .and_then(|layout| layout.built_in)
            .unwrap_or(BuiltInLayout::Develop);
        crate::workspace_features::effective_workspace_layout(
            persisted,
            self.workspace_state.active_work().map(|work| work.status),
            self.agentic_workspace_enabled,
        )
    }
}
