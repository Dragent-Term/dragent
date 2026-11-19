//! Tab, pane, and split management methods for WorkspaceView.
//!
//! Responsible for:
//! - tab lifecycle (create, titles, colors, cwd, running state)
//! - document-terminal groups and agent tabs
//! - split tree mutations (split, move, pop, dissolve, prune)
//! - pane activity, titles, and surface registration
//!
//! Not responsible for:
//! - layout tree algorithms (pane_layout)
//! - rendering tabs/panes (view_render)
//!
//! Blocks here are impl WorkspaceView: same type, same behavior.

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

use super::view::WorkspaceView;
// Re-exported so existing `view_tabs::…` paths keep working during the
// migration; canonical home is now `tabs_panes`.
use super::project_naming::default_pane_cwd;
pub(crate) use super::tabs_panes::{
    apply_sidebar_visibility, refresh_sidebar_projects, subscribe_terminal_pane,
    subscribe_workspace_pane,
};
use super::view::{
    DocumentTerminalTab, FolderGitBulkOp, FolderGitFileOp, PaneTab, ParkedWorkspaceWork,
    WorkSurfaceIds, WorkspaceRuntimeSurface, FOLDER_GIT_DISCARD_ARM_SECS,
};

impl WorkspaceView {
    pub(crate) fn create_tab(
        &mut self,
        ctx: &mut ViewContext<Self>,
        custom_label: Option<String>,
        shell: Option<String>,
    ) -> PaneTab {
        // The generic Terminal picker opens a standalone shell. Project
        // terminals use the explicit create_tab_with_cwd entry point instead.
        // Follow a visible terminal's live directory, otherwise start in home.
        let cwd = (!self.project_start_visible)
            .then(|| {
                self.tabs
                    .iter()
                    .find(|tab| tab.id == self.active_tab_id)
                    .filter(|tab| !tab.pane.is_settings())
                    .map(|tab| tab.pane.cwd(ctx))
                    .filter(|cwd| cwd.is_dir())
            })
            .flatten()
            .unwrap_or_else(default_pane_cwd);
        self.create_tab_with_cwd(ctx, custom_label, shell, cwd, None)
    }

    pub(crate) fn create_tab_with_cwd(
        &mut self,
        ctx: &mut ViewContext<Self>,
        custom_label: Option<String>,
        shell: Option<String>,
        cwd: PathBuf,
        project_root: Option<PathBuf>,
    ) -> PaneTab {
        self.create_tab_with_cwd_focus(ctx, custom_label, shell, cwd, project_root, true)
    }

    pub(crate) fn create_tab_with_cwd_focus(
        &mut self,
        ctx: &mut ViewContext<Self>,
        custom_label: Option<String>,
        shell: Option<String>,
        cwd: PathBuf,
        project_root: Option<PathBuf>,
        focus: bool,
    ) -> PaneTab {
        // Production tabs are host-owned (daemon PTYs survive app quit).
        // Unit tests keep deterministic in-process shells: under `cargo
        // test` the current exe is the test harness, so `ensure_host`
        // would spawn the harness as a daemon (or touch the developer's
        // live daemon) — never do host I/O from unit tests.
        #[cfg(not(test))]
        {
            return self.create_host_tab(ctx, custom_label, shell, cwd, project_root, focus);
        }
        #[cfg(test)]
        {
            self.create_local_tab_with_cwd_focus(ctx, custom_label, shell, cwd, project_root, focus)
        }
    }

    #[cfg(test)]
    pub(crate) fn create_local_tab_with_cwd_focus(
        &mut self,
        ctx: &mut ViewContext<Self>,
        custom_label: Option<String>,
        shell: Option<String>,
        cwd: PathBuf,
        project_root: Option<PathBuf>,
        focus: bool,
    ) -> PaneTab {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let pane = match shell {
            Some(shell) => WorkspacePane::spawn_new_with_shell(
                ctx,
                self.theme,
                self.ui_font,
                cwd.clone(),
                shell,
            ),
            None => WorkspacePane::spawn_new(ctx, self.theme, self.ui_font, cwd),
        };
        subscribe_workspace_pane(tab_id, &pane, self.persistence_writer.sender(), ctx);

        if focus {
            pane.focus(ctx);
            self.active_pane_id = Some(pane.id());
        }
        let surface_id = self.register_surface(&pane, ctx);

        PaneTab {
            id: tab_id,
            surface_id,
            title: DEFAULT_SESSION_TITLE.to_string(),
            custom_label,
            color: None,
            work_item_id: Some(WorkItemId::new()),
            project_root,
            hidden_from_sidebar: false,
            pane,
        }
    }

    /// Ensure the editor has its own terminal group. A dock terminal must never
    /// borrow a top-level pane: one view cannot be laid out at the mosaic size
    /// and the independently resizable dock size at the same time.
    pub(crate) fn ensure_document_terminal_group(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(owner_root) = self.current_document_root(ctx) else {
            return;
        };

        // Migrate any dock created by the old borrowing behavior without
        // stopping the top-level terminal it references.
        let borrowed_ids = self
            .document_terminals
            .iter()
            .filter(|tab| tab.owner_root == owner_root && tab.borrowed)
            .map(|tab| (tab.id, tab.surface_id))
            .collect::<Vec<_>>();
        if !borrowed_ids.is_empty() {
            let ids = borrowed_ids
                .iter()
                .map(|(id, _)| *id)
                .collect::<HashSet<_>>();
            self.document_terminals.retain(|tab| !ids.contains(&tab.id));
            if self
                .document_terminal_active_id
                .is_some_and(|id| ids.contains(&id))
            {
                self.document_terminal_active_id = None;
            }
            if self
                .document_terminal_split_id
                .is_some_and(|id| ids.contains(&id))
            {
                self.document_terminal_split_id = None;
            }
            for (_, surface_id) in borrowed_ids {
                self.release_surface_if_unplaced(surface_id);
            }
        }

        if self
            .document_terminals
            .iter()
            .any(|tab| tab.owner_root == owner_root)
        {
            self.select_document_terminal_group(&owner_root);
            return;
        }

        let id = self.create_document_terminal(owner_root, ctx);
        self.document_terminal_active_id = Some(id);
    }

    pub(crate) fn active_document_terminal(&self) -> Option<&DocumentTerminalTab> {
        let id = self.document_terminal_active_id?;
        let owner_root = self.document_owner_root.as_ref()?;
        self.document_terminals
            .iter()
            .find(|tab| tab.id == id && tab.owner_root == *owner_root)
    }

    pub(crate) fn current_document_root(&self, _ctx: &AppContext) -> Option<PathBuf> {
        if self.document_center_open {
            self.document_owner_root.clone()
        } else {
            self.tabs
                .iter()
                .find(|tab| tab.id == self.active_tab_id)
                .filter(|tab| !tab.pane.is_settings())
                .and_then(|tab| tab.project_root.clone())
                .filter(|root| root.is_dir())
        }
    }

    pub(crate) fn select_document_terminal_group(&mut self, owner_root: &Path) {
        let mut ids = self
            .document_terminals
            .iter()
            .filter(|tab| tab.owner_root == owner_root)
            .map(|tab| tab.id);
        let active = self
            .document_terminal_active_id
            .filter(|id| {
                self.document_terminals
                    .iter()
                    .any(|tab| tab.id == *id && tab.owner_root == owner_root)
            })
            .or_else(|| ids.next());
        self.document_terminal_active_id = active;
        if self.document_terminal_split_id.is_some_and(|id| {
            !self
                .document_terminals
                .iter()
                .any(|tab| tab.id == id && tab.owner_root == owner_root && Some(id) != active)
        }) {
            self.document_terminal_split_id = None;
        }
    }

    /// Add a dock-only terminal. It deliberately does not enter `self.tabs`:
    /// dock sessions belong to the open editor layout, not the workspace's
    /// top-level terminal navigation.
    pub(crate) fn create_document_terminal(
        &mut self,
        cwd: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) -> usize {
        let id = self.document_terminal_next_id;
        self.document_terminal_next_id += 1;
        #[cfg(not(test))]
        let pane = self.create_host_auxiliary_pane(cwd.clone(), ctx);
        #[cfg(test)]
        let pane = WorkspacePane::spawn_new(ctx, self.theme, self.ui_font, cwd.clone());
        let owner_root = self.current_document_root(ctx).unwrap_or_else(|| cwd.clone());
        pane.set_theme(self.document_workspace_theme(&owner_root), ctx);
        if let WorkspacePane::Terminal(terminal) = &pane {
            subscribe_terminal_pane(None, terminal, self.persistence_writer.sender(), false, ctx);
            ctx.update_view(terminal, |view, view_ctx| {
                view.set_docked_backdrop_enabled(true);
                view_ctx.notify();
            });
        }
        pane.focus(ctx);
        let surface_id = self.register_surface(&pane, ctx);
        self.document_terminals.push(DocumentTerminalTab {
            id,
            surface_id,
            pane,
            borrowed: false,
            owner_root,
        });
        id
    }

    pub(crate) fn new_document_terminal(&mut self, split: bool, ctx: &mut ViewContext<Self>) {
        self.ensure_document_terminal_group(ctx);
        let previous = self.document_terminal_active_id;
        let cwd = self
            .active_document_terminal()
            .map(|tab| tab.pane.cwd(ctx))
            .filter(|cwd| !cwd.as_os_str().is_empty())
            .unwrap_or_else(|| self.document_index_root.clone());
        let id = self.create_document_terminal(cwd, ctx);
        self.document_terminal_active_id = Some(id);
        self.document_terminal_split_id = if split { previous } else { None };
        self.document_terminal_visible = true;
        // The new terminal is appended at the end of the strip. Clamping in
        // `ClippedScrollable::after_layout` turns this into the actual maximum.
        self.document_terminal_tabs_scroll
            .scroll_to(warpui_core::units::Pixels::new(f32::MAX));
        ctx.notify();
    }

    pub(crate) fn set_document_terminal_backdrops(
        &self,
        enabled: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        for tab in &self.document_terminals {
            if let WorkspacePane::Terminal(terminal) = &tab.pane {
                ctx.update_view(terminal, |view, view_ctx| {
                    view.set_docked_backdrop_enabled(enabled);
                    view_ctx.notify();
                });
            }
        }
    }

    pub(crate) fn close_document_terminal(
        &mut self,
        terminal_id: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(position) = self
            .document_terminals
            .iter()
            .position(|tab| tab.id == terminal_id)
        else {
            return;
        };
        let removed = self.document_terminals.remove(position);
        let removed_surface_id = removed.surface_id;
        let owner_root = removed.owner_root.clone();
        if !removed.borrowed {
            self.close_host_pane_session(removed.pane.id());
            let pane_uuid = removed.pane.pane_uuid(ctx);
            if let WorkspacePane::Terminal(terminal) = &removed.pane {
                ctx.update_view(terminal, |view, _| view.shutdown());
            }
            crate::confirm::set_foreground_command(pane_uuid, None);
            crate::agent::session_facts::clear_conversation(pane_uuid);
            let project_root = Some(owner_root.clone());
            if project_root.as_ref().is_none_or(|root| root.is_dir()) {
                persistence::clear_pane_data(
                    &self.persistence_writer,
                    pane_uuid,
                    project_root.clone(),
                );
                persistence::clear_agent_blocks(&self.persistence_writer, pane_uuid, project_root);
            }
        }
        if self.document_terminal_split_id == Some(terminal_id) {
            self.document_terminal_split_id = None;
        }
        if self.document_terminal_active_id == Some(terminal_id) {
            self.document_terminal_active_id = self
                .document_terminals
                .iter()
                .filter(|tab| tab.owner_root == owner_root)
                .nth(
                    position.min(
                        self.document_terminals
                            .iter()
                            .filter(|tab| tab.owner_root == owner_root)
                            .count()
                            .saturating_sub(1),
                    ),
                )
                .map(|tab| tab.id);
        }
        if let Some(active) = self.active_document_terminal() {
            active.pane.focus(ctx);
        } else {
            self.document_terminal_visible = false;
            self.document_terminal_maximized = false;
            self.document_terminal_split_id = None;
        }
        self.release_surface_if_unplaced(removed_surface_id);
        ctx.notify();
    }

    pub(crate) fn create_agent_tab(&mut self, ctx: &mut ViewContext<Self>) -> PaneTab {
        let project_root = if self.project_start_visible {
            self.selected_project_root().filter(|root| root.is_dir())
        } else {
            self.active_project_root()
        };
        self.create_agent_tab_at(ctx, project_root)
    }

    pub(crate) fn create_agent_tab_at(
        &mut self,
        ctx: &mut ViewContext<Self>,
        project_root: Option<PathBuf>,
    ) -> PaneTab {
        // Same host/test split as `create_tab_with_cwd_focus`.
        #[cfg(not(test))]
        {
            return self.create_host_agent_tab_at(ctx, project_root);
        }
        #[cfg(test)]
        {
            self.create_local_agent_tab_at(ctx, project_root)
        }
    }

    #[cfg(test)]
    pub(crate) fn create_local_agent_tab_at(
        &mut self,
        ctx: &mut ViewContext<Self>,
        project_root: Option<PathBuf>,
    ) -> PaneTab {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let cwd = project_root.clone().unwrap_or_else(default_pane_cwd);
        let pane = WorkspacePane::spawn_new_as_agent(ctx, self.theme, self.ui_font, cwd);
        subscribe_workspace_pane(tab_id, &pane, self.persistence_writer.sender(), ctx);

        pane.focus(ctx);
        self.active_pane_id = Some(pane.id());
        let surface_id = self.register_surface(&pane, ctx);

        PaneTab {
            id: tab_id,
            surface_id,
            title: "Dragent".to_string(),
            custom_label: Some("Dragent".to_string()),
            color: None,
            work_item_id: Some(WorkItemId::new()),
            project_root,
            hidden_from_sidebar: false,
            pane,
        }
    }

    pub(crate) fn tab_is_running(&self, tab: &PaneTab, app: &AppContext) -> bool {
        tab.pane.is_running(app)
    }

    pub(crate) fn register_surface(&mut self, pane: &WorkspacePane, ctx: &AppContext) -> SurfaceId {
        let descriptor = pane.surface_descriptor(ctx);
        let id = descriptor.id;
        if self.surface_registry.get(id).is_none() {
            if let Err(error) = self
                .surface_registry
                .register(descriptor, WorkspaceRuntimeSurface::Pane(pane.clone()))
            {
                log::error!("[workspace-surface] failed to register surface: {error}");
            }
        }
        id
    }

    /// Drop the registry's owning handle only after the surface has no layout
    /// placement. Borrowing one terminal in the editor dock therefore survives
    /// removal from the top-level tab strip.
    pub(crate) fn release_surface_if_unplaced(&mut self, surface_id: SurfaceId) {
        let placed_in_tabs = self.tabs.iter().any(|tab| tab.surface_id == surface_id);
        let placed_in_dock = self
            .document_terminals
            .iter()
            .any(|tab| tab.surface_id == surface_id);
        if !placed_in_tabs && !placed_in_dock {
            self.surface_registry.remove(surface_id);
        }
    }

    pub(crate) fn tab_cwd(&self, tab: &PaneTab, app: &AppContext) -> String {
        let pane = self.title_pane_for_tab(tab);
        pane.cwd(app).to_string_lossy().into_owned()
    }
}

impl WorkspaceView {
    pub(crate) fn register_work_item_surfaces(
        &mut self,
        project_id: dragent_workspace_model::ProjectId,
        work_item_id: WorkItemId,
    ) -> WorkSurfaceIds {
        let mut register = |kind| {
            let descriptor = SurfaceDescriptor::work_item_singleton(project_id, work_item_id, kind);
            let id = descriptor.id;
            if self.surface_registry.get(id).is_none() {
                if let Err(error) = self
                    .surface_registry
                    .register(descriptor, WorkspaceRuntimeSurface::WorkSurface)
                {
                    log::error!("[workspace-surface] failed to register work surface: {error}");
                }
            }
            id
        };
        WorkSurfaceIds {
            conversation: register(SurfaceKind::PrincipalConversation),
            brief: register(SurfaceKind::WorkBrief),
            organization: register(SurfaceKind::OrganizationMap),
            role_detail: register(SurfaceKind::RoleDetail),
            activity: register(SurfaceKind::Activity),
            attention: register(SurfaceKind::Attention),
            tests: register(SurfaceKind::Tests),
            artifact: register(SurfaceKind::Artifact),
        }
    }

    pub(crate) fn switch_workspace_work_for_tab(
        &mut self,
        tab_id: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some((target_id, root)) =
            self.tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| {
                    tab.work_item_id.map(|work_item_id| {
                        let root = tab
                            .project_root
                            .clone()
                            .unwrap_or_else(|| tab.pane.cwd(ctx));
                        (work_item_id, root)
                    })
                })
        else {
            return;
        };
        if target_id == self.workspace_work_item_id {
            return;
        }

        let mut target_freshly_created = false;
        let mut target = self
            .parked_workspace_work
            .remove(&target_id)
            .unwrap_or_else(|| {
                target_freshly_created = true;
                let project = Project::new_for_organisation(&self.organisation_id, root, 0);
                ParkedWorkspaceWork {
                    surface_ids: self.register_work_item_surfaces(project.id, target_id),
                    state: WorkspaceState::new(project),
                    execution_runtime: WorkspaceExecutionRuntime::default(),
                    canonical_revision: 0,
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
                    tab_open: false,
                    tab_active: false,
                    view: WorkItemView::Conversation,
                    selected_agent_task: None,
                    selected_agent_attempt: None,
                    agent_timeline_expanded: false,
                    dock_visible: false,
                    dock_maximized: false,
                    composer_attachments: Vec::new(),
                    composer_notice: None,
                    principal_pending: false,
                    principal_shape_approved: false,
                    force_proposal: false,
                    requirements_complete: false,
                    turn_memory: Vec::new(),
                    principal_errors: Vec::new(),
                    principal_request_epoch: 0,
                    interrupted_turn: None,
                }
            });
        // First time this work item's tab has been opened this session:
        // its conversation (an `AgentController`, SQLite-restored like a
        // terminal pane's) is created lazily by `ensure_work_item_agent`.
        let _ = target_freshly_created;

        let previous_id = self.workspace_work_item_id;
        let previous = ParkedWorkspaceWork {
            surface_ids: std::mem::replace(&mut self.work_surface_ids, target.surface_ids),
            state: std::mem::replace(&mut self.workspace_state, target.state),
            execution_runtime: std::mem::replace(
                &mut self.workspace_execution_runtime,
                target.execution_runtime,
            ),
            canonical_revision: std::mem::replace(
                &mut self.workspace_canonical_revision,
                target.canonical_revision,
            ),
            checkpoint_notice: std::mem::replace(
                &mut self.workspace_checkpoint_notice,
                target.checkpoint_notice,
            ),
            launch_pending: std::mem::replace(
                &mut self.workspace_launch_pending,
                target.launch_pending,
            ),
            launch_pending_since: std::mem::replace(
                &mut self.workspace_launch_pending_since,
                target.launch_pending_since,
            ),
            principal_coordination_pending: std::mem::replace(
                &mut self.workspace_principal_coordination_pending,
                target.principal_coordination_pending,
            ),
            principal_coordination_pending_since: std::mem::replace(
                &mut self.workspace_principal_coordination_pending_since,
                target.principal_coordination_pending_since,
            ),
            principal_coordination_epoch: std::mem::replace(
                &mut self.workspace_principal_coordination_epoch,
                target.principal_coordination_epoch,
            ),
            principal_escalation_queue: std::mem::replace(
                &mut self.workspace_principal_escalation_queue,
                target.principal_escalation_queue,
            ),
            principal_plan_actions_queue: std::mem::replace(
                &mut self.workspace_principal_plan_actions_queue,
                target.principal_plan_actions_queue,
            ),
            execution_poll_scheduled: std::mem::replace(
                &mut self.workspace_execution_poll_scheduled,
                target.execution_poll_scheduled,
            ),
            retry_wakeup_scheduled: std::mem::replace(
                &mut self.workspace_retry_wakeup_scheduled,
                target.retry_wakeup_scheduled,
            ),
            tab_open: std::mem::replace(&mut self.work_item_tab_open, target.tab_open),
            tab_active: std::mem::replace(&mut self.work_item_tab_active, target.tab_active),
            view: std::mem::replace(&mut self.work_item_view, target.view),
            selected_agent_task: std::mem::replace(
                &mut self.workspace_selected_agent_task,
                target.selected_agent_task,
            ),
            selected_agent_attempt: std::mem::replace(
                &mut self.workspace_selected_agent_attempt,
                target.selected_agent_attempt,
            ),
            agent_timeline_expanded: std::mem::replace(
                &mut self.workspace_agent_timeline_expanded,
                target.agent_timeline_expanded,
            ),
            dock_visible: std::mem::replace(
                &mut self.document_terminal_visible,
                target.dock_visible,
            ),
            dock_maximized: std::mem::replace(
                &mut self.document_terminal_maximized,
                target.dock_maximized,
            ),
            composer_attachments: std::mem::replace(
                &mut self.shape_composer_attachments,
                target.composer_attachments,
            ),
            composer_notice: std::mem::replace(
                &mut self.shape_composer_notice,
                target.composer_notice,
            ),
            principal_pending: std::mem::replace(
                &mut self.shape_principal_pending,
                target.principal_pending,
            ),
            principal_shape_approved: std::mem::replace(
                &mut self.shape_approved,
                target.principal_shape_approved,
            ),
            force_proposal: std::mem::replace(
                &mut self.shape_force_proposal,
                target.force_proposal,
            ),
            turn_memory: std::mem::replace(&mut self.shape_turn_memory, target.turn_memory),
            requirements_complete: std::mem::replace(
                &mut self.shape_requirements_complete,
                target.requirements_complete,
            ),
            principal_errors: Vec::new(),
            principal_request_epoch: std::mem::replace(
                &mut self.shape_principal_request_epoch,
                target.principal_request_epoch,
            ),
            interrupted_turn: std::mem::replace(
                &mut self.shape_interrupted_turn,
                target.interrupted_turn,
            ),
        };
        self.workspace_work_item_id = target_id;
        self.parked_workspace_work.insert(previous_id, previous);
        // The item's conversation (restored from SQLite on first creation)
        // must exist before its tab renders — otherwise an item with history
        // would show the empty state until its next turn.
        self.ensure_work_item_agent(target_id, ctx);
        // Every work item has a database foreign key to its project. A newly
        // opened folder creates this project projection lazily, so persist it
        // as soon as the tab becomes active and before any Principal turn can
        // submit CreateWorkItem through the canonical authority.
        self.persistence_writer
            .save_workspace_project(self.workspace_state.project().clone());
        self.set_document_terminal_backdrops(self.document_terminal_visible, ctx);

        // The in-flight Principal turn is parked per work item above (pending
        // flags, epoch) and its conversation is the item's own controller, so
        // switching tabs never orphans it — like the terminal agent's
        // per-pane blocks. Only the unsent composer draft and live voice
        // capture are tab-local and cleared here.
        if let Some(session) = self.shape_voice_session.take() {
            session.stop();
        }
        self.clear_shape_voice_state();
        self.shape_composer_text.clear();
        ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
            editor.set_text("", editor_ctx);
        });
        // Returning to a live turn resumes shimmer repaints like the
        // terminal's per-pane timers; follow state is left as the user set it.
        if self.shape_principal_pending {
            self.tick_principal_progress(ctx);
        }
        if self.work_item_tab_open && self.work_item_tab_active {
            // Work is rendered by its owning workspace tab. Switching back to
            // it must not recreate the editor's reserved pane.
            self.project_start_visible = false;
            self.document_center_open = false;
            self.editor_pane_focused = false;
            self.document_restore_pending = false;
            self.document_tab_id = Some(tab_id);
            self.document_owner_root = Some(self.workspace_state.project().root.clone());
        }
        // This item's Principal turn was cut short when the app stopped
        // (a turn still running in the background keeps it pending).
        if self.shape_interrupted_turn.is_some() && !self.shape_principal_pending {
            ctx.spawn(Timer::after(Duration::from_millis(300)), move |view, _, ctx| {
                if view.workspace_work_item_id == target_id {
                    view.resume_interrupted_shape_turn(ctx);
                }
            });
        }
        if self.workspace_execution_runtime.active_count() > 0 {
            self.workspace_execution_poll_scheduled = false;
            self.schedule_workspace_execution_poll(ctx);
        }
        // The boot-time reconciliation pass (timing out a dead lease and
        // relaunching) only ever ran for the work item active at process
        // launch. A tab holding a different Executing work item — e.g. one
        // whose session was interrupted by a crash or quit in a prior run —
        // never got that pass, so it would sit frozen with a "Running" card
        // and no session behind it. Run the same reconciliation here.
        if self.workspace_state.active_work().is_some_and(|work| {
            matches!(
                work.status,
                WorkStatus::Executing | WorkStatus::ReadyToExecute | WorkStatus::Paused
            )
        }) {
            ctx.spawn(Timer::after(Duration::from_millis(1)), |view, _, ctx| {
                if view.continue_principal_review_policy(ctx) {
                    return;
                }
                if view.continue_principal_escalation(ctx) {
                    return;
                }
                if view.continue_principal_plan_actions(ctx) {
                    return;
                }
                if view.continue_principal_coordination(ctx) {
                    return;
                }
                view.continue_workspace_launch(ctx);
            });
        }
        self.reconcile_workspace_integrations(ctx);
    }

    /// Surface a host-side Principal failure (validation, authority
    /// rejection) in the ACTIVE work item's conversation — the same Error
    /// card the terminal agent shows for a failed turn.
    pub(crate) fn record_principal_error(&mut self, error: String, ctx: &mut ViewContext<Self>) {
        let work_item_id = self.workspace_work_item_id;
        self.record_principal_error_for(work_item_id, error, ctx);
    }

    pub(crate) fn record_principal_error_for(
        &mut self,
        work_item_id: WorkItemId,
        error: String,
        ctx: &mut ViewContext<Self>,
    ) {
        log::warn!("[workspace-shape] {error}");
        let agent = self.ensure_work_item_agent(work_item_id, ctx);
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        ctx.update_model(&controller, |controller, mctx| {
            controller.push_host_error(error, mctx);
        });
    }

    /// Project root a work item's tools resolve against — the active item's
    /// live state or its parked projection.
    pub(crate) fn work_item_project_root(&self, work_item_id: WorkItemId) -> PathBuf {
        if work_item_id == self.workspace_work_item_id {
            self.workspace_state.project().root.clone()
        } else {
            self.parked_workspace_work
                .get(&work_item_id)
                .map(|parked| parked.state.project().root.clone())
                .unwrap_or_else(|| self.workspace_state.project().root.clone())
        }
    }
}
