//! WorkspaceView construction.
//!
//! Responsible for:
//! - building the WorkspaceView with all child models and subscriptions
//! - restoring persisted session state into a new view
//!
//! Not responsible for:
//! - rendering, actions, or coordination drivers (sibling view_* modules)
//!
//! The block here is impl WorkspaceView: same type, same behavior.

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

use super::project_naming::default_pane_cwd;
use super::view::WorkspaceView;
use super::view::{
    AddProjectMenu, DocumentTerminalTab, PaneTab, ParkedWorkspaceWork, RemotePointer,
    WorkSurfaceIds, WorkspaceRuntimeSurface, MIN_RIGHT_PANEL_WIDTH,
};
use super::view_persistence::is_legacy_agentic_fixture;
use super::view_tabs::{
    apply_sidebar_visibility, refresh_sidebar_projects, subscribe_terminal_pane,
    subscribe_workspace_pane,
};
use super::workspace_tab::WorkspaceTab;

impl WorkspaceView {
    pub(crate) fn build(ctx: &mut ViewContext<Self>, restore: bool) -> Self {
        Self::build_prepared(ctx, restore, super::prewarm::load_persistence())
    }

    /// Persistence and checkout discovery have already completed on a worker.
    pub(crate) fn build_prepared(
        ctx: &mut ViewContext<Self>,
        restore: bool,
        prepared: (Option<dragent_persistence::PersistedSession>, dragent_persistence::WriterHandles),
    ) -> Self {
        let organisation_id = dragent_persistence::current_org();
        let font_cache = ctx.get_singleton_model_handle::<FontCache>();

        let ui_font = ctx.update_model(&font_cache, |cache, _| super::prewarm::load_ui_font(cache));

        let mono_font = ctx.update_model(&font_cache, |cache, _| {
            super::prewarm::load_mono_font(cache)
        });

        // Extra (New Window) workspaces keep their own persistence writer for their
        // panes' blocks, but never restore the shared single-window session snapshot.
        let (loaded_session, persistence_writer) = prepared;
        let loaded = restore.then_some(loaded_session).flatten();
        let project_start_visible = loaded.as_ref().is_none_or(|loaded| {
            loaded.session.document.project_start_visible || loaded.session.tabs.is_empty()
        });
        let restored_document = loaded
            .as_ref()
            .map(|loaded| loaded.session.document.clone())
            .unwrap_or_default();
        let mut restored_open_project_roots = restored_document.open_project_roots.clone();
        let workspace_trust = super::trust_store::WorkspaceTrustStore::load();
        for root in restored_document
            .workspace_tab_work_items
            .iter()
            .filter_map(|item| item.project_root.clone())
        {
            if root.is_dir()
                && !restored_open_project_roots.contains(&root)
                && !super::project_naming::worktree_main_checkout(&root)
                    .is_some_and(|main| restored_open_project_roots.contains(&main))
            {
                restored_open_project_roots.push(root);
            }
        }
        if restored_open_project_roots.is_empty()
            && restored_document.root.is_dir()
            && restored_document.root != default_pane_cwd()
        {
            restored_open_project_roots.push(restored_document.root.clone());
        }
        if restore
            && restored_open_project_roots.is_empty()
            && loaded
                .as_ref()
                .is_none_or(|session| session.session.tabs.is_empty())
        {
            // Older empty-tab snapshots were discarded by the loader. Recover
            // the selected project from its independent recent-project list.
            restored_open_project_roots =
                workspace_trust.recent_roots().into_iter().rev().collect();
        }
        // Fresh installs (no saved agent) start on the configured default
        // agent (Settings → Agent → General); saved agents restore verbatim
        // even if since disabled.
        let restored_prompt_agent = match restored_document.prompt_agent.as_deref() {
            Some(saved) => super::action::PromptAgent::from_saved(saved),
            None => super::action_projects::default_prompt_agent(),
        };
        let restored_selected_worktree = restored_document
            .prompt_selected_worktree
            .clone()
            .filter(|root| root.is_dir() && root.join(".git").exists());
        let restored_prompt_branch = restored_document.prompt_branch.clone().or_else(|| {
            restored_selected_worktree
                .as_ref()
                .or_else(|| restored_open_project_roots.last())
                .and_then(|root| super::project_naming::local_branch_name(root))
        });
        let restored_tab_work_items = restored_document
            .workspace_tab_work_items
            .iter()
            .map(|item| (item.owner_tab_id, item.clone()))
            .collect::<HashMap<_, _>>();
        let restored_working_copies = loaded
            .as_ref()
            .map(|loaded| loaded.working_copies.clone())
            .unwrap_or_default();
        let restored_blocks = loaded
            .as_ref()
            .map(|loaded| loaded.blocks.clone())
            .unwrap_or_default();
        let restored_agent_blocks = loaded
            .as_ref()
            .map(|loaded| loaded.agent_blocks.clone())
            .unwrap_or_default();
        let restored_layouts = loaded
            .as_ref()
            .map(|loaded| loaded.session.layouts.clone())
            .unwrap_or_default();
        let restored_workspace_work_items = loaded
            .as_ref()
            .map(|loaded| loaded.workspace_work_items.clone())
            .unwrap_or_default();
        let restored_workspace_events = loaded
            .as_ref()
            .map(|loaded| loaded.workspace_events.clone())
            .unwrap_or_default();
        let restored_canonical_workspace_events = loaded
            .as_ref()
            .map(|loaded| loaded.canonical_workspace_events.clone())
            .unwrap_or_default();
        let layout_migration_pending = restored_layouts.instances.is_empty();
        let restored_active_document = restored_document.active_relative_path.clone();
        let mut restored_open_documents = restored_document.open_relative_paths.clone();
        if let Some(active) = restored_active_document.as_ref() {
            if !restored_open_documents.contains(active) {
                restored_open_documents.push(active.clone());
            }
        }

        let mut tabs = vec![];
        let mut active_tab_id = 0;
        let mut next_tab_id = 1;
        let mut active_pane_id = None;
        // Host-owned tabs to re-attach after construction (needs `view`).
        let mut pending_host_reattach: Vec<(usize, super::host_tabs::HostReattach)> = Vec::new();
        // Daemon bindings by pane uuid for the restore loop below.
        let host_bindings: HashMap<Uuid, dragent_persistence::SerializedHostTerminalBinding> =
            restored_document
                .host_terminal_bindings
                .iter()
                .map(|binding| (binding.pane_uuid, binding.clone()))
                .collect();
        let mut split_tree: Option<PaneLayout> = None;
        let mut restored_parked_workspace_tabs: Vec<WorkspaceTab> = Vec::new();
        // Default: both panels open; overridden only when restoring persisted
        // state. The web viewer keeps its sidebar closed.
        let mut restored_chrome = WindowChrome {
            left_panel_open: cfg!(not(target_family = "wasm")),
            right_panel_open: true,
            ..WindowChrome::default()
        };

        let theme = UiSettings::load().theme();

        if let Some(PersistedSession {
            session,
            blocks,
            agent_blocks,
            working_copies: _,
            workspace_work_items: _,
            workspace_events: _,
            canonical_workspace_events: _,
        }) = loaded
        {
            restored_chrome = session.chrome.clone();
            // Split leaves without saved metadata allocate fresh ids; start
            // above every saved id so no workspace can collide with another.
            next_tab_id = session
                .tabs
                .iter()
                .chain(
                    restored_document
                        .parked_workspace_tabs
                        .iter()
                        .flat_map(|parked| parked.tabs.iter()),
                )
                .map(|tab| tab.id + 1)
                .max()
                .unwrap_or(next_tab_id)
                .max(next_tab_id);
            let restore_context = super::view_persistence::SurfaceRestoreContext {
                blocks: &blocks,
                agent_blocks: &agent_blocks,
                host_bindings: &host_bindings,
                tab_work_items: &restored_tab_work_items,
                document: &restored_document,
                theme,
                ui_font,
            };
            let front = Self::restore_surface(
                session.tabs,
                session.active_tab_id,
                session.active_pane_uuid,
                true,
                &restore_context,
                &mut next_tab_id,
                &mut pending_host_reattach,
                ctx,
            );
            tabs = front.tabs;
            split_tree = front.split_tree;
            active_tab_id = front.active_tab_id;
            active_pane_id = front.active_pane_id;
            for parked in restored_document.parked_workspace_tabs.clone() {
                let surface = Self::restore_surface(
                    parked.tabs,
                    parked.active_tab_id,
                    parked.active_pane_uuid,
                    false,
                    &restore_context,
                    &mut next_tab_id,
                    &mut pending_host_reattach,
                    ctx,
                );
                let mut workspace_tab = WorkspaceTab::fresh(parked.id, parked.name);
                if let Some(ui) = restored_document
                    .surface_ui
                    .get(&parked.id)
                    .and_then(|json| {
                        serde_json::from_str::<super::workspace_tab::SurfaceUiState>(json).ok()
                    })
                {
                    workspace_tab.ui = ui;
                }
                workspace_tab.panes = surface.tabs;
                workspace_tab.split_tree = workspace_tab
                    .ui
                    .layout
                    .clone()
                    .filter(|layout| {
                        layout.panes().iter().all(|id| {
                            *id == super::pane_layout::EDITOR_PANE_ID
                                || workspace_tab.panes.iter().any(|pane| pane.id == *id)
                        })
                    })
                    .or(surface.split_tree);
                workspace_tab.editor_pane_focused = workspace_tab.ui.editor_focused;
                workspace_tab.active_tab_id = surface.active_tab_id;
                workspace_tab.active_pane_id = surface.active_pane_id;
                restored_parked_workspace_tabs.push(workspace_tab);
            }
        } else {
            // A fresh/projectless window owns no terminal. The project-start
            // surface is the real empty state; a shell is created only after
            // an explicit project or terminal action. The sidebar stays open
            // so login lands on the main view, not a bare canvas.
            restored_chrome.right_panel_open = false;
        }
        // Nothing to come back to: the start screen always has its sidebar,
        // whatever an earlier window saved.
        let starts_empty = cfg!(not(target_family = "wasm"))
            && split_tree.is_none()
            && restored_parked_workspace_tabs.is_empty()
            && restored_open_project_roots.is_empty();
        let active_workspace_tab_id = restored_document
            .active_workspace_tab_id
            .filter(|id| {
                !restored_parked_workspace_tabs
                    .iter()
                    .any(|workspace_tab| workspace_tab.id == *id)
            })
            .unwrap_or(1);
        let known_workspace_tab_ids = std::iter::once(active_workspace_tab_id)
            .chain(restored_parked_workspace_tabs.iter().map(|tab| tab.id))
            .collect::<Vec<_>>();
        let mut workspace_tab_order = restored_document
            .workspace_tab_order
            .iter()
            .copied()
            .filter(|id| known_workspace_tab_ids.contains(id))
            .fold(Vec::new(), |mut order, id| {
                if !order.contains(&id) {
                    order.push(id);
                }
                order
            });
        for id in &known_workspace_tab_ids {
            if !workspace_tab_order.contains(id) {
                workspace_tab_order.push(*id);
            }
        }
        let next_workspace_tab_id = known_workspace_tab_ids.iter().max().map_or(2, |id| id + 1);

        let left_width = restored_chrome
            .left_panel_width
            .filter(|width| (width - crate::theme::PREVIOUS_LEFT_PANEL_WIDTH).abs() > 0.5)
            .unwrap_or(DEFAULT_LEFT_PANEL_WIDTH);
        let right_width = restored_chrome
            .right_panel_width
            .unwrap_or(DEFAULT_RIGHT_PANEL_WIDTH)
            .max(MIN_RIGHT_PANEL_WIDTH);
        let workspace_view_id = ctx.view_id();

        let mut surface_registry = SurfaceRegistry::new();
        let parked_panes = restored_parked_workspace_tabs
            .iter()
            .flat_map(|workspace_tab| workspace_tab.panes.iter());
        for tab in tabs.iter().chain(parked_panes) {
            let descriptor = tab.pane.surface_descriptor(ctx);
            debug_assert_eq!(descriptor.id, tab.surface_id);
            if let Err(error) = surface_registry
                .register(descriptor, WorkspaceRuntimeSurface::Pane(tab.pane.clone()))
            {
                log::error!("[workspace-surface] failed to register restored surface: {error}");
            }
        }

        // The panel's Resizable state is created up front so the panel view can
        // read its live width at render time (width-aware text truncation).
        let left_panel_state = resizable_state_handle(left_width);

        // Create LeftPanelView via add_typed_action_view so the structural
        // parent link (LeftPanelView → WorkspaceView) is recorded immediately
        // in the presenter's parent map.  Its own constructor creates the two
        // InputEditor children with LeftPanelView as their structural parent.
        let lp_state = left_panel_state.clone();
        let left_panel_view = ctx.add_typed_action_view(|lp_ctx| {
            LeftPanelView::new(lp_ctx, workspace_view_id, ui_font, theme, lp_state)
        });

        let project_sidebar_model = ctx.add_model(|_| ProjectSidebarModel::new());
        let psm = project_sidebar_model.clone();
        let git_panel_state = std::rc::Rc::new(std::cell::RefCell::new(
            super::project_sidebar::GitPanelState::default(),
        ));
        let sidebar_git_panel = git_panel_state.clone();
        let project_sidebar_view = ctx.add_typed_action_view(|vc| {
            ProjectSidebarView::new(
                psm,
                workspace_view_id,
                theme,
                ui_font,
                mono_font,
                sidebar_git_panel,
                vc,
            )
        });

        let workspace_root = tabs
            .iter()
            .find(|tab| tab.id == active_tab_id)
            .or_else(|| tabs.first())
            .map(|tab| {
                tab.project_root
                    .clone()
                    .unwrap_or_else(|| tab.pane.cwd(ctx))
            })
            .filter(|root| !root.as_os_str().is_empty())
            .or_else(|| restored_open_project_roots.last().cloned())
            .unwrap_or_else(default_pane_cwd);
        let workspace_project =
            Project::new_for_organisation(&organisation_id, workspace_root.clone(), 0);
        let document_surface =
            SurfaceDescriptor::project_singleton(workspace_project.id, SurfaceKind::Editor);
        let document_surface_id = document_surface.id;
        if let Err(error) = surface_registry.register(
            document_surface,
            WorkspaceRuntimeSurface::ProjectEditor(project_sidebar_view.clone()),
        ) {
            log::error!("[workspace-surface] failed to register project editor: {error}");
        }
        // Develop snapshots contain one unified surface graph. Prefer it when
        // it references the editor so restore keeps the editor's original
        // position and split proportions instead of appending a new editor
        // pane beside the active terminal.
        if restored_document.center_open
            && restored_layouts
                .active()
                .is_some_and(|layout| layout.built_in == Some(BuiltInLayout::Develop))
        {
            if let Some(layout) = restored_layouts.active() {
                let active_surface = layout.active_surface_id;
                let restored_surface_tree = layout.root.clone();
                let surface_to_pane = |surface_id: SurfaceId| {
                    if surface_id == document_surface_id {
                        return Some(EDITOR_PANE_ID);
                    }
                    tabs.iter()
                        .find_map(|tab| (tab.surface_id == surface_id).then_some(tab.id))
                };
                if let Some(restored_panes) = PaneLayout::from_surface_layout(
                    &restored_surface_tree,
                    surface_to_pane,
                    active_surface,
                ) {
                    if restored_panes.contains(EDITOR_PANE_ID) {
                        split_tree = Some(restored_panes);
                    }
                }
            }
        }
        let restored_active_work_item_id = restored_tab_work_items
            .get(&active_tab_id)
            .map(|item| item.work_item_id);
        let restored_active_shaping = restored_tab_work_items
            .get(&active_tab_id)
            .map(|item| item.shaping.clone())
            .unwrap_or_default();
        let restored_work = restored_workspace_work_items
            .iter()
            .cloned()
            .filter(|work| work.project_id == workspace_project.id)
            // This deterministic identity belonged to the pre-production
            // Organization Billing demo. Never resurrect it as user work,
            // even if an earlier build wrote canonical events for it.
            .filter(|work| !is_legacy_agentic_fixture(workspace_project.id, work.id))
            .filter(|work| {
                restored_active_work_item_id.is_none_or(|work_item_id| work.id == work_item_id)
            })
            .max_by_key(|work| work.updated_at_unix_ms);
        let workspace_work_item_id = restored_active_work_item_id
            .or_else(|| restored_work.as_ref().map(|work| work.id))
            .unwrap_or_else(|| {
                WorkItemId(Uuid::new_v5(&workspace_project.id.0, b"work-item:primary"))
            });
        if !restored_tab_work_items.contains_key(&active_tab_id) {
            if let Some(tab) = tabs.iter_mut().find(|tab| tab.id == active_tab_id) {
                tab.work_item_id = Some(workspace_work_item_id);
            }
        }
        let principal_id = ActorRef::principal_agent(workspace_project.id).id;
        let canonical_events = restored_canonical_workspace_events
            .iter()
            .cloned()
            .filter(|event| {
                event.project_id == workspace_project.id
                    && event.work_item_id == workspace_work_item_id
            })
            .collect::<Vec<_>>();
        let workspace_canonical_revision = canonical_events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(0);
        let canonical_projection_events = canonical_events
            .iter()
            .cloned()
            .filter_map(|event| {
                dragent_session_protocol::workspace::project_envelope_event(event, principal_id)
            })
            .collect::<Vec<_>>();
        let workspace_state = if restored_work.is_some() {
            WorkspaceState::restore(
                workspace_project.clone(),
                restored_work,
                if canonical_projection_events.is_empty() {
                    restored_workspace_events.clone()
                } else {
                    canonical_projection_events
                },
            )
        } else {
            WorkspaceState::new(workspace_project.clone())
        };
        let mut workspace_execution_runtime = WorkspaceExecutionRuntime::default();
        workspace_execution_runtime.restore_history(canonical_events.iter());
        let workspace_recovery_warnings = workspace_execution_runtime
            .recover_interrupted_local_sessions(
                canonical_events.iter(),
                &workspace_state,
                &workspace_project.root,
            );
        for warning in &workspace_recovery_warnings {
            log::warn!("[workspace-execution] {warning}");
        }
        let workspace_recovery_notice = (!workspace_recovery_warnings.is_empty()).then(|| {
            let count = workspace_recovery_warnings.len();
            format!(
                "{count} interrupted execution{} could not continue safely. {} checkpoint and Attention state were preserved.",
                if count == 1 { "" } else { "s" },
                if count == 1 { "Its" } else { "Their" },
            )
        });
        let mut register_work_surface = |kind| {
            let descriptor = SurfaceDescriptor::work_item_singleton(
                workspace_project.id,
                workspace_work_item_id,
                kind,
            );
            let id = descriptor.id;
            if let Err(error) =
                surface_registry.register(descriptor, WorkspaceRuntimeSurface::WorkSurface)
            {
                log::error!("[workspace-surface] failed to register work surface: {error}");
            }
            id
        };
        let work_surface_ids = WorkSurfaceIds {
            conversation: register_work_surface(SurfaceKind::PrincipalConversation),
            brief: register_work_surface(SurfaceKind::WorkBrief),
            organization: register_work_surface(SurfaceKind::OrganizationMap),
            role_detail: register_work_surface(SurfaceKind::RoleDetail),
            activity: register_work_surface(SurfaceKind::Activity),
            attention: register_work_surface(SurfaceKind::Attention),
            tests: register_work_surface(SurfaceKind::Tests),
            artifact: register_work_surface(SurfaceKind::Artifact),
        };
        let mut parked_workspace_work = HashMap::new();
        let parked_panes = restored_parked_workspace_tabs
            .iter()
            .flat_map(|workspace_tab| workspace_tab.panes.iter());
        for tab in tabs
            .iter()
            .filter(|tab| tab.id != active_tab_id)
            .chain(parked_panes)
        {
            let Some(work_item_id) = tab.work_item_id else {
                continue;
            };
            // Project identity follows the tab's stable folder scope, never a
            // terminal's mutable cwd. Otherwise a restored tab whose shell ran
            // `cd` can lose its persisted events and look like an unused draft.
            let project = Project::new_for_organisation(
                &organisation_id,
                tab.project_root
                    .clone()
                    .unwrap_or_else(|| tab.pane.cwd(ctx)),
                0,
            );
            let restored_tab_work = restored_workspace_work_items
                .iter()
                .find(|work| work.project_id == project.id && work.id == work_item_id)
                .cloned();
            let tab_canonical_events = restored_canonical_workspace_events
                .iter()
                .filter(|event| {
                    event.project_id == project.id && event.work_item_id == work_item_id
                })
                .cloned()
                .collect::<Vec<_>>();
            let principal_id = ActorRef::principal_agent(project.id).id;
            let projection_events = tab_canonical_events
                .iter()
                .cloned()
                .filter_map(|event| {
                    dragent_session_protocol::workspace::project_envelope_event(event, principal_id)
                })
                .collect::<Vec<_>>();
            let state = if restored_tab_work.is_some() {
                WorkspaceState::restore(
                    project.clone(),
                    restored_tab_work,
                    if projection_events.is_empty() {
                        restored_workspace_events.clone()
                    } else {
                        projection_events
                    },
                )
            } else {
                WorkspaceState::new(project.clone())
            };
            let mut execution_runtime = WorkspaceExecutionRuntime::default();
            execution_runtime.restore_history(tab_canonical_events.iter());
            let mut register_parked_surface = |kind| {
                let descriptor =
                    SurfaceDescriptor::work_item_singleton(project.id, work_item_id, kind);
                let id = descriptor.id;
                if let Err(error) =
                    surface_registry.register(descriptor, WorkspaceRuntimeSurface::WorkSurface)
                {
                    log::error!("[workspace-surface] failed to register work surface: {error}");
                }
                id
            };
            let presentation = restored_tab_work_items.get(&tab.id);
            parked_workspace_work.insert(
                work_item_id,
                ParkedWorkspaceWork {
                    surface_ids: WorkSurfaceIds {
                        conversation: register_parked_surface(SurfaceKind::PrincipalConversation),
                        brief: register_parked_surface(SurfaceKind::WorkBrief),
                        organization: register_parked_surface(SurfaceKind::OrganizationMap),
                        role_detail: register_parked_surface(SurfaceKind::RoleDetail),
                        activity: register_parked_surface(SurfaceKind::Activity),
                        attention: register_parked_surface(SurfaceKind::Attention),
                        tests: register_parked_surface(SurfaceKind::Tests),
                        artifact: register_parked_surface(SurfaceKind::Artifact),
                    },
                    canonical_revision: tab_canonical_events
                        .last()
                        .map(|event| event.sequence)
                        .unwrap_or(0),
                    state,
                    execution_runtime,
                    checkpoint_notice: None,
                    launch_pending: false,
                    launch_pending_since: None,
                    principal_coordination_pending: false,
                    principal_coordination_pending_since: None,
                    principal_coordination_epoch: 0,
                    principal_escalation_queue: VecDeque::new(),
                    principal_plan_actions_queue: VecDeque::new(),
                    execution_poll_scheduled: false,
                    retry_wakeup_scheduled: false,
                    selected_agent_task: None,
                    selected_agent_attempt: None,
                    agent_timeline_expanded: false,
                    tab_open: presentation.is_some_and(|item| item.open),
                    tab_active: presentation.is_some_and(|item| item.active),
                    view: WorkItemView::from_persistence_key(
                        presentation.and_then(|item| item.view.as_deref()),
                    ),
                    dock_visible: presentation
                        .and_then(|item| item.dock_visible)
                        .unwrap_or(false),
                    dock_maximized: presentation
                        .and_then(|item| item.dock_maximized)
                        .unwrap_or(false),
                    composer_attachments: Vec::new(),
                    composer_notice: None,
                    principal_pending: false,
                    // Shaping progress saved with the session survives a
                    // restart (or a crash) with the work item.
                    principal_shape_approved: presentation
                        .is_some_and(|item| item.shaping.shape_approved),
                    force_proposal: false,
                    requirements_complete: presentation
                        .is_some_and(|item| item.shaping.requirements_complete),
                    turn_memory: presentation
                        .map(|item| item.shaping.turn_memory.clone())
                        .unwrap_or_default(),
                    principal_errors: Vec::new(),
                    principal_request_epoch: 0,
                    interrupted_turn: presentation
                        .and_then(|item| item.shaping.interrupted_turn.clone()),
                },
            );
        }
        let settings_root = workspace_root.clone();
        let settings_view = ctx.add_typed_action_view(|sv_ctx| {
            SettingsView::new(sv_ctx, workspace_view_id, ui_font, settings_root)
        });
        #[cfg(not(target_family = "wasm"))]
        let automations_view = ctx.add_typed_action_view(|page_ctx| {
            crate::automations::AutomationsView::new(page_ctx, ui_font, mono_font, theme)
        });
        #[cfg(not(target_family = "wasm"))]
        let pull_requests_view = ctx.add_typed_action_view(|c| crate::pull_requests::PullRequestsView::new(c, ui_font, mono_font, theme));
        #[cfg(not(target_family = "wasm"))]
        let teams_view = ctx.add_typed_action_view(|page_ctx| {
            crate::teams::TeamsView::new(page_ctx, ui_font, mono_font, theme)
        });
        #[cfg(not(target_family = "wasm"))]
        let tasks_view = ctx.add_typed_action_view(|page_ctx| {
            crate::tasks_page::TasksView::new(page_ctx, ui_font, mono_font, theme)
        });

        let share_email_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Emails",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "share_email".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let ports_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Port (e.g. 3000)",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "ports_port".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let attention_response_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Type your answer…",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "attention_response".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let explorer_prompt_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Name",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "explorer_entry_name".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let project_start_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Repository URL or SSH destination",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "project_start".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let clone_url_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Repository URL…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "add_project_clone_url".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let clone_location_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Project directory…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "clone_location".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let create_project_name_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "my-project",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "create_project_name".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let project_group_name_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Group name…",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "project_group_name".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let create_project_location_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Location…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "create_project_location".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let worktree_branch_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "feature-branch",
                ui_font,
                crate::theme::text_size(UI_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "new_worktree_branch".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let prompt_editor = ctx.add_typed_action_view(|ed_ctx| {
            let mut editor = InputEditor::new(
                "Ask Dragent…",
                mono_font,
                crate::theme::text_size(crate::theme::INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "project_prompt".to_string(),
                    max_height: 140,
                },
                ed_ctx.focus_handle(),
            );
            editor.set_agent_mode(true, ed_ctx);
            editor.cursor_style_override = Some(crate::console::editor_settings::CursorStyle::Bar);
            editor
        });
        #[cfg(not(target_family = "wasm"))]
        let review_instructions_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new("Review instructions…", mono_font, crate::theme::text_size(13.0),
                theme.main_text(), theme.disabled_text(), theme.accent, theme,
                InputMode::MultiLine { key:"review_instructions".into(), max_height:220 }, ed_ctx.focus_handle())
        });
        let prompt_link_search_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Search issues or pull requests…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "prompt_link_search".to_string(),
                },
                ed_ctx.focus_handle(),
            )
        });
        let workspace_tab_rename_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Rename tab…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::RenameWorkspaceTab(0),
                ed_ctx.focus_handle(),
            )
        });
        if !restored_document.prompt_text.is_empty() {
            let draft = restored_document.prompt_text.clone();
            ctx.update_view(&prompt_editor, |editor, editor_ctx| {
                editor.set_text(&draft, editor_ctx)
            });
        }
        let prompt_branch_search_editor = ctx.add_typed_action_view(|ed_ctx| {
            let mut editor = InputEditor::new(
                "Search branches…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "prompt_branch_search".to_string(),
                },
                ed_ctx.focus_handle(),
            );
            editor.cursor_style_override = Some(crate::console::editor_settings::CursorStyle::Bar);
            editor
        });
        let prompt_agent_search_editor = ctx.add_typed_action_view(|ed_ctx| {
            let mut editor = InputEditor::new(
                "Search agents…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "prompt_agent_search".to_string(),
                },
                ed_ctx.focus_handle(),
            );
            editor.cursor_style_override = Some(crate::console::editor_settings::CursorStyle::Bar);
            editor
        });
        let past_sessions_search_editor = ctx.add_typed_action_view(|ed_ctx| {
            let mut editor = InputEditor::new(
                "Search past sessions…",
                ui_font,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "past_sessions_search".to_string(),
                },
                ed_ctx.focus_handle(),
            );
            editor.cursor_style_override = Some(crate::console::editor_settings::CursorStyle::Bar);
            editor
        });
        let shape_composer_editor = ctx.add_typed_action_view(|ed_ctx| {
            // Exact terminal-agent input geometry: mono font at INPUT_FONT_SIZE
            // with TerminalCommand mode so cursor/placeholder placement,
            // growing (up to 336), copy/paste and canvas all match.
            let mut editor = InputEditor::new(
                "Add context, constraints, or examples…",
                mono_font,
                crate::theme::text_size(crate::theme::INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::TerminalCommand,
                ed_ctx.focus_handle(),
            );
            editor.set_agent_mode(true, ed_ctx);
            editor
        });
        let shape_record_editor = ctx.add_typed_action_view(|ed_ctx| {
            InputEditor::new(
                "Describe this item…",
                ui_font,
                crate::theme::text_size(11.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "shape_record_editor".to_string(),
                    max_height: 150,
                },
                ed_ctx.focus_handle(),
            )
        });
        let document_root = if restored_document.root.is_dir() {
            restored_document.root.clone()
        } else {
            workspace_root.clone()
        };
        ctx.update_model(&project_sidebar_model, |model, _| {
            model.configure_recovery(
                document_root.clone(),
                restored_working_copies,
                restored_document.expanded_directories.clone(),
            );
        });
        let language_server_catalog = Arc::new(LanguageServerCatalog::load());
        let (document_tree_watch_tx, document_tree_watch_rx) = async_channel::unbounded();

        let agentic_workspace_enabled = crate::workspace_features::agentic_workspace_enabled(
            crate::settings::AppSettings::global()
                .agent
                .agentic_workspace_enabled,
        );
        let active_work_presentation = restored_tab_work_items.get(&active_tab_id);
        let restored_work_item_view = WorkItemView::from_persistence_key(
            active_work_presentation
                .and_then(|item| item.view.as_deref())
                .or(restored_document.work_item_view.as_deref()),
        );
        let restored_work_item_open = active_work_presentation
            .map(|item| item.open)
            .unwrap_or(restored_document.work_item_open);
        let restored_work_item_active = restored_work_item_open
            && active_work_presentation
                .map(|item| item.active)
                .unwrap_or(restored_document.work_item_active);
        let restored_dock_visible = active_work_presentation
            .and_then(|item| item.dock_visible)
            .unwrap_or(restored_document.dock_visible);
        let restored_dock_maximized = active_work_presentation
            .and_then(|item| item.dock_maximized)
            .unwrap_or(restored_document.dock_maximized);
        let project_start_recent_projects = workspace_trust.recent_roots();
        let project_start_recent_ssh_hosts = workspace_trust.recent_ssh_hosts();
        let project_start_ssh_config_hosts = super::ssh::ssh_config_hosts();
        let mut view = Self {
            organisation_id,
            workspace_view_id,
            window_id: ctx.window_id(),
            theme,
            ui_font,
            left_panel_state,
            right_panel_state: resizable_state_handle(right_width),
            tab_mru: tabs.iter().map(|t| t.id).collect(),
            tabs,
            parked_workspace_tabs: restored_parked_workspace_tabs,
            workspace_tab_order,
            workspaces: restored_document.workspaces.clone(),
            closed_workspace_roots: restored_document.closed_workspace_roots.clone(),
            active_workspace_tab_id,
            // Empty means the chip resolves live from the active pane.
            active_workspace_tab_name: restored_document.active_workspace_tab_name.clone(),
            next_workspace_tab_id,
            pending_close_project_workspace: None,
            pending_close_workspace_tab: None,
            close_workspace_tab_error: None,
            renaming_workspace_tab: None,
            workspace_tab_rename_editor,
            workspace_tab_strip_scroll: ClippedScrollStateHandle::new(),
            workspace_tab_drag_states: RefCell::new(HashMap::new()),
            agent_rail_expanded: false,
            agent_rail_scroll: ClippedScrollStateHandle::new(),
            custom_installed_agents: std::collections::HashSet::new(),
            custom_install_agent: None,
            cached_custom_agents: Vec::new(),
            selected_project_root: restored_document
                .selected_project_root
                .clone()
                .or_else(|| restored_open_project_roots.last().cloned()),
            open_project_roots: restored_open_project_roots,
            surface_registry,
            document_surface_id,
            workspace_work_item_id,
            work_surface_ids,
            agentic_workspace_enabled,
            workspace_automation_state: None,
            layout_collection: restored_layouts,
            layout_menu_open: false,
            review_menu_open: false,
            shape_canvas_horizontal_scroll: ClippedScrollStateHandle::new(),
            shape_canvas_vertical_scroll: ClippedScrollStateHandle::new(),
            coordinate_canvas_horizontal_scroll: ClippedScrollStateHandle::new(),
            coordinate_canvas_vertical_scroll: ClippedScrollStateHandle::new(),
            coordinate_activity_scroll: ClippedScrollStateHandle::new(),
            coordinate_activity_dock_state: resizable_state_handle(190.0),
            review_canvas_horizontal_scroll: ClippedScrollStateHandle::new(),
            review_canvas_vertical_scroll: ClippedScrollStateHandle::new(),
            work_item_conversation_scroll: ClippedScrollStateHandle::new(),
            work_item_conversation_selectable: warpui::elements::SelectionHandle::default(),
            work_item_plan_scroll: ClippedScrollStateHandle::new(),
            work_item_coordinate_horizontal_scroll: ClippedScrollStateHandle::new(),
            work_item_coordinate_vertical_scroll: ClippedScrollStateHandle::new(),
            work_item_changes_scroll: ClippedScrollStateHandle::new(),
            work_item_review_horizontal_scroll: ClippedScrollStateHandle::new(),
            work_item_agent_timeline_scroll: ClippedScrollStateHandle::new(),
            work_item_review_vertical_scroll: ClippedScrollStateHandle::new(),
            shape_composer_editor,
            shape_composer_text: String::new(),
            shape_composer_attachments: Vec::new(),
            shape_composer_notice: None,
            shape_model_menu_open: false,
            shape_voice_ui: crate::voice::VoiceUiState::Idle,
            shape_voice_gen: 0,
            shape_voice_spin: 0,
            shape_voice_session: None,
            shape_voice_fold: Default::default(),
            shape_voice_rebase_pending: false,
            shape_voice_pending_confirm: None,
            shape_approved: restored_active_shaping.shape_approved,
            shape_force_proposal: false,
            shape_turn_memory: restored_active_shaping.turn_memory.clone(),
            shape_requirements_complete: restored_active_shaping.requirements_complete,
            shape_interrupted_turn: restored_active_shaping.interrupted_turn.clone(),
            shape_resume_turn: None,
            shape_principal_pending: false,
            shape_principal_request_epoch: 0,
            shape_record_editor,
            shape_record_edit_target: None,
            shape_record_edit_text: String::new(),
            shape_record_edit_pending: false,
            shape_record_edit_error: None,
            shape_record_edit_epoch: 0,
            shape_remove_armed: None,
            shape_selected_role: None,
            shape_plan_reset_after_edit: false,
            shape_model_picker_contract: None,
            shape_role_action_pending: None,
            shape_role_action_error: None,
            shape_role_action_epoch: 0,
            shape_coordination_pending: false,
            shape_coordination_pending_since: None,
            shape_coordination_error: None,
            shape_recovery_notice: workspace_recovery_notice,
            shape_coordination_epoch: 0,
            workspace_canonical_revision,
            active_tab_id,
            next_tab_id,
            active_pane_id,
            split_tree,
            split_drag_handles: RefCell::new(HashMap::new()),
            editor_pane_focused: false,
            drag_in_progress: false,
            split_close_mouse_states: RefCell::new(HashMap::new()),
            split_action_mouse_states: RefCell::new(HashMap::new()),
            header_mouse_states: RefCell::new(HashMap::new()),
            new_tab_mouse_state: MouseStateHandle::default(),
            left_panel_visible: restored_chrome.left_panel_open || starts_empty,
            right_panel_visible: restored_chrome.right_panel_open,
            workspace_trust,
            add_project: None,
            remote_connected: Vec::new(),
            project_start_recent_projects,
            project_start_recent_ssh_hosts,
            project_start_ssh_config_hosts,
            pending_workspace_trust: None,
            pending_git_init_root: None,
            pending_workspace_trust_document: None,
            pending_project_folder_intent: None,
            project_start_visible,
            project_start_mode: None,
            project_start_editor,
            project_start_text: String::new(),
            project_start_busy: false,
            project_start_error: None,
            error_notices: std::collections::VecDeque::new(),
            error_notice_scroll: ClippedScrollStateHandle::default(),
            error_notice_copied: None,
            pending_clone_repository: None,
            add_project_menu: AddProjectMenu::Closed,
            project_context_menu_root: None,
            new_project_group_root: None,
            project_group_error: None,
            project_group_name_editor,
            pending_project_removal: None,
            pending_project_group_removal: None,
            project_removal_busy: false,
            project_removal_error: None,
            importable_project_worktrees: None,
            import_project_worktrees_busy: false,
            import_project_worktrees_error: None,
            review_ship: ReviewShipState::default(),
            review_pr_tick: 0,
            review_pr_request_epoch: 0,
            review_pr_poll_scheduled: false,
            left_panel_sync_scheduled: false,
            agent_theme_restarts_scheduled: false,
            project_roots_resolving: std::cell::Cell::new(false),
            project_icons_resolving: std::cell::Cell::new(false),
            project_roots_checked: std::cell::Cell::new(false),
            session_save_scheduled: false,
            review_pr_cache: HashMap::new(),
            review_action_busy: None,
            review_action_notice: None,
            review_action_error: None,
            worktree_git_cache: HashMap::new(),
            worktree_git_inflight: Default::default(),
            agent_stop_state: HashMap::new(),
            create_project_open: false,
            create_project_location: default_projects_dir(),
            create_project_error: None,
            worktree_prompt_open: false,
            worktree_rename_branch: false,
            worktree_base_root: None,
            worktree_start_branch: None,
            worktree_busy: false,
            worktree_error: None,
            worktree_branch_editor,
            prompt_editor,
            prompt_agent: restored_prompt_agent,
            prompt_team: false,
            prompt_lead_model: None,
            prompt_model_tab: crate::teams::models::ModelTab::Agent(restored_prompt_agent),
            prompt_team_agents: Vec::new(),
            prompt_team_cost: dragent_host::team::Cost::Balanced,
            prompt_installed_agents: HashSet::new(),
            prompt_agent_checking: false,
            prompt_agent_check_generation: 0,
            prompt_agent_refresh_in_flight: false,
            prompt_agent_refresh_pending: false,
            prompt_install_agent: None,
            prompt_install_terminal: None,
            page_command: None,
            account_sign_in: None,
            prompt_install_error: None,
            prompt_install_generation: 0,
            remote_tool_install: None,
            remote_tool_checking: false,
            prompt_text: restored_document.prompt_text.clone(),
            prompt_attachments: Vec::new(),
            prompt_links: serde_json::from_str(&restored_document.prompt_links_json)
                .unwrap_or_default(),
            prompt_task: None,
            task_start_queue: Vec::new(),
            pending_task_start: None,
            prompt_attachment_scroll: ClippedScrollStateHandle::new(),
            prompt_link_results: Vec::new(),
            prompt_link_loading: false,
            prompt_link_error: None,
            prompt_link_generation: 0,
            prompt_link_query: String::new(),
            prompt_link_search_editor,
            prompt_notice: None,
            prompt_voice_ui: crate::voice::VoiceUiState::Idle,
            prompt_voice_session: None,
            prompt_voice_gen: 0,
            prompt_voice_spin: 0,
            prompt_voice_fold: Default::default(),
            prompt_voice_pending_confirm: None,
            prompt_menu: None,
            prompt_menu_scroll: ClippedScrollStateHandle::new(),
            prompt_device_remote_open: false,
            prompt_branch: restored_prompt_branch,
            prompt_selected_worktree: restored_selected_worktree,
            prompt_branches: Vec::new(),
            prompt_branch_search_editor,
            prompt_branch_search: String::new(),
            prompt_agent_search_editor,
            prompt_agent_search: String::new(),
            prompt_worktrees: Vec::new(),
            clone_url_editor,
            clone_url_error: None,
            clone_location_editor,
            clone_url_text: String::new(),
            clone_location_text: String::new(),
            clone_busy: false,
            create_project_name_editor,
            create_project_location_editor,
            right_panel_user_closed: !restored_chrome.right_panel_open,
            left_panel_view,
            project_sidebar_model,
            project_sidebar_view,
            git_panel_state,
            folder_git_model: None,
            folder_git_root: None,
            persistence_writer,
            persists_session_snapshot: restore,
            workspace_state,
            parked_workspace_work,
            workspace_boot_restored_agent_blocks: restored_agent_blocks.clone(),
            work_item_agents: HashMap::new(),
            attempt_agents: HashMap::new(),
            #[cfg(not(target_family = "wasm"))]
            cli_workers: super::cli_workers::load_workers(),
            #[cfg(not(target_family = "wasm"))]
            cli_worker_poll_scheduled: false,
            #[cfg(not(target_family = "wasm"))]
            cli_worker_answered: std::collections::HashSet::new(),
            attempt_attention: HashMap::new(),
            principal_coordination_turns: HashMap::new(),
            pending_worker_questions: HashMap::new(),
            pending_workspace_tool_acknowledgements: HashMap::new(),
            pending_worker_question_submissions: HashMap::new(),
            principal_pass_replies: HashMap::new(),
            workspace_execution_runtime,
            workspace_checkpoint_notice: None,
            workspace_launch_pending: false,
            workspace_launch_pending_since: None,
            workspace_principal_coordination_pending: false,
            workspace_principal_coordination_pending_since: None,
            workspace_principal_coordination_activity: None,
            principal_replies_submitted: HashSet::new(),
            workspace_principal_coordination_epoch: 0,
            workspace_principal_escalation_queue: VecDeque::new(),
            workspace_principal_plan_actions_queue: VecDeque::new(),
            workspace_execution_poll_scheduled: false,
            workspace_retry_wakeup_scheduled: false,
            document_index: ProjectDocumentIndex::default(),
            document_index_root: document_root.clone(),
            document_tree_watch_tx,
            document_tree_watch_root: None,
            document_tree_watch_stop: None,
            document_tree_refresh_epoch: 0,
            // Files remain dormant until explicitly selected, while a Work
            // Item restores as its own first-class pane.
            document_center_open: false,
            document_restore_pending: !restored_open_documents.is_empty()
                || restored_active_document.is_some(),
            document_tab_id: restored_document.owner_tab_id,
            document_owner_root: restored_document
                .owner_tab_id
                .map(|_| document_root.clone()),
            document_requested_path: restored_active_document.clone(),
            document_open_paths: restored_open_documents,
            // Restored tabs represent deliberate prior work and are permanent.
            document_preview_path: None,
            document_tabs_scroll: ClippedScrollStateHandle::new(),
            work_item_view_tabs_scroll: ClippedScrollStateHandle::new(),
            work_item_tab_open: restored_work_item_open,
            work_item_tab_active: restored_work_item_active,
            work_item_view: restored_work_item_view,
            workspace_agent_tabs: Vec::new(),
            workspace_active_agent_tab: None,
            workspace_selected_agent_task: None,
            workspace_selected_agent_attempt: None,
            workspace_agent_timeline_expanded: false,
            document_terminal_visible: restored_dock_visible,
            document_terminals: Vec::new(),
            document_terminal_active_id: restored_document.dock_active_id,
            document_terminal_split_id: restored_document.dock_split_id,
            document_terminal_next_id: restored_document.dock_next_id.max(1),
            document_terminal_maximized: restored_dock_maximized,
            document_terminal_state: resizable_state_handle(
                restored_document.dock_height.unwrap_or(260.0),
            ),
            document_terminal_tabs_scroll: ClippedScrollStateHandle::new(),
            new_tab_picker_open: false,
            agent_account_picker: None,
            machine_account_caches: HashMap::new(),
            pending_close_tab: None,
            closing_worktree_tab: None,
            closed_worktree_main_checkout: None,
            close_tab_error: None,
            close_other_tabs_keep_id: None,
            agents_panel_open: false,
            agent_runs: Vec::new(),
            agents_busy: false,
            agents_error: None,
            pending_close_document: None,
            pending_discard_hunk: None,
            explorer_clipboard: None,
            explorer_prompt: None,
            explorer_prompt_editor,
            explorer_rename_pending: false,
            pending_explorer_delete: None,
            explorer_operation_error: None,
            document_backup_epoch: 0,
            language_services: None,
            language_server_catalog,
            language_document_sync: None,
            language_service_status: Vec::new(),
            language_navigation_capabilities: HashMap::new(),
            language_file_rename_servers: HashSet::new(),
            language_services_open: false,
            language_service_install_prompts: HashSet::new(),
            language_services_enabled: true,
            language_sync_epoch: 0,
            language_event_subscription_epoch: 0,
            language_completion_epoch: 0,
            language_completion_token: None,
            language_request_tokens: HashMap::new(),
            pending_language_navigation: None,
            definition_hover_latest_request_id: None,
            definition_hover_resolution: None,
            new_tab_picker_from_sidebar: false,
            new_tab_picker_mouse_states: RefCell::new(HashMap::new()),
            settings_view,
            automations_open: false,
            #[cfg(not(target_family = "wasm"))]
            automations_view,
            pull_requests_open: false,
            #[cfg(not(target_family = "wasm"))]
            pull_requests_view,
            #[cfg(not(target_family = "wasm"))]
            review_setup: None,
            #[cfg(not(target_family = "wasm"))]
            review_instructions_editor,
            #[cfg(not(target_family = "wasm"))]
            review_instructions: String::new(),
            teams_open: false,
            #[cfg(not(target_family = "wasm"))]
            teams_view,
            tasks_open: false,
            #[cfg(not(target_family = "wasm"))]
            tasks_view,
            teams_needing_you: 0,
            coordination_groups: Vec::new(),
            teams_waiting: std::collections::HashMap::new(),
            teams_badge_seeded: false,
            lead_news_pending: std::collections::HashSet::new(),
            settings_open: restored_document.settings_open,
            settings_project_root: None,
            account_logged_in_cache: false,
            account_menu_open: false,
            usage_menu_open: false,
            usage_menu_agent: None,
            usage_tabs_scroll: ClippedScrollStateHandle::new(),
            #[cfg(not(target_family = "wasm"))]
            agent_usage: Vec::new(),
            agent_usage_loading: false,
            past_sessions_open: false,
            #[cfg(not(target_family = "wasm"))]
            past_sessions: Vec::new(),
            past_sessions_loading: false,
            past_sessions_scroll: Default::default(),
            past_sessions_search_editor,
            past_sessions_search: String::new(),
            past_sessions_confirm: None,
            past_sessions_busy: None,
            past_sessions_note: None,
            past_sessions_reclaimable: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            computer_asks: Default::default(),
            #[cfg(not(target_family = "wasm"))]
            computer_trusted_apps: Default::default(),
            #[cfg(not(target_family = "wasm"))]
            computer_ask_scroll: Default::default(),
            logout_confirm_open: false,
            logout_dont_ask: false,
            org_menu_open: false,
            org_submenu: None,
            help_menu_open: false,
            feedback: None,
            mono_font,
            share_open: false,
            share_qr_open: false,
            share_qr_asset: None,
            share_open_dropdown: None,
            share_email_editor,
            ports_editor,
            attention_bell_open: false,
            attention_popover_item: None,
            attention_response_editor,
            attention_response_text: String::new(),
            workspace_standing_tool_approvals: HashSet::new(),
            ports_open: false,
            ports_notice: None,
            ports_notice_epoch: 0,
            agent_sleep_sweeping: false,
            agent_sleep_notice: None,
            agent_sleep_notice_epoch: 0,
            agent_sleep_prompt: None,
            agent_sleep_scroll: Default::default(),
            agent_resources: Default::default(),
            share_snapshot: share_sheet::ShareSnapshot::default(),
            login_prompt_open: false,
            mirror_pane: None,
            mirror_tab_id: None,
            shared_session_ended: None,
            host_mirror_pane: None,
            host_mirror_tab_id: None,
            host_mirror: None,
            mirror_client: None,
            host_owned_tabs: HashMap::new(),
            host_stream_tx: None,
            pending_access_request: None,
            content_header_menu_open: false,
            split_share_menu_tab_id: None,
            stop_confirm_open: false,
            share_scope_tab: None,
            share_notice: None,
            share_notice_epoch: 0,
            idle_warning_open: false,
            participants_menu_open: false,
            participant_access_open: None,
            remote_pointers: HashMap::new(),
        };

        if let Some(ui) = restored_document
            .surface_ui
            .get(&view.active_workspace_tab_id)
            .and_then(|json| {
                serde_json::from_str::<super::workspace_tab::SurfaceUiState>(json).ok()
            })
        {
            view.apply_surface_ui(&ui);
            view.editor_pane_focused = ui.editor_focused;
            if let Some(layout) = ui.layout.filter(|layout| {
                layout.panes().iter().all(|id| {
                    *id == super::pane_layout::EDITOR_PANE_ID
                        || view.tabs.iter().any(|pane| pane.id == *id)
                })
            }) {
                view.split_tree = Some(layout);
            }
        }
        ctx.update_view(&view.left_panel_view, |panel, _| {
            panel.restore_ui(&restored_document.sidebar_ui)
        });
        ctx.update_view(&view.project_sidebar_view, |sidebar, _| {
            sidebar.restore_editor_ui(&restored_document.editor_ui)
        });

        // The active work item's conversation (its `AgentController`) is
        // restored from SQLite when it's first needed — see
        // `ensure_work_item_agent`.
        Self::complete_construction(
            &mut view,
            ctx,
            &restored_document,
            document_root,
            layout_migration_pending,
            workspace_work_item_id,
            theme,
            ui_font,
            restored_blocks,
            restored_agent_blocks,
            document_tree_watch_rx,
        );
        // Host-owned tabs: bind live daemon sessions (or respawn fresh in
        // the same home) now that `view` exists to own the bindings.
        view.reconcile_workspace_ownership();
        view.rehome_imported_worktree_tabs(ctx);
        view.reveal_workspace_tab(view.active_workspace_tab_id);
        view.sync_left_panel(ctx);
        if restore {
            crate::startup_timing::expect_terminals(
                pending_host_reattach.iter().map(|(tab_id, _)| *tab_id),
            );
        }
        view.attach_restored_host_tabs(ctx, pending_host_reattach);
        // Resolve agent installation state on startup so the rail and menus
        // never show a false "not installed": the probe flips
        // `prompt_agent_checking` synchronously and reconciles the sets when
        // it lands. Unit tests must never spawn live PATH probes.
        #[cfg(not(test))]
        view.refresh_prompt_agents(ctx);
        view
    }
}
