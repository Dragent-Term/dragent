//! Main render composition for WorkspaceView.
//!
//! Responsible for:
//! - composing the workspace frame (sidebar, tab bar, pane layout, panels)
//! - editor / split-tree / tab-pane rendering
//! - new-tab picker and add-project menu rendering
//! - the framework View impl (ui_name + render entry point)
//!
//! Not responsible for:
//! - overlay/modal rendering (view_overlays)
//! - event handling and actions (WorkspaceView handlers)
//! - layout tree mutations (pane_layout)
//!
//! Blocks here are impl WorkspaceView / impl View for WorkspaceView: same
//! type, same behavior, organized by responsibility.

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
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_build::*;
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

use super::render_picker::{render_add_project_menu, render_new_tab_picker};
use super::view::WorkspaceView;
use super::view::{AddProjectMenu, ACTIVE_PANE_ACCENT_ALPHA, MIN_RIGHT_PANEL_WIDTH};

impl Drop for WorkspaceView {
    fn drop(&mut self) {
        self.stop_document_tree_watcher();
        if let Some(terminal) = self.prompt_install_terminal.take() {
            self.close_host_pane_session(terminal.id());
        }
        for owned in self.host_owned_tabs.values() {
            if let Some(cancel) = &owned.stream {
                cancel.store(true, std::sync::atomic::Ordering::Release);
            }
            owned.slot.detach();
        }
    }
}

impl Entity for WorkspaceView {
    type Event = ();
}

impl View for WorkspaceView {
    fn ui_name() -> &'static str {
        "WorkspaceView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        // Observe the workspace itself: child mutations need not re-render
        // RootView. Collection only reads the resolved sidebar snapshot.
        #[cfg(not(target_family = "wasm"))]
        super::desktop_catalog::note_rendered(self, app);
        let active_layout = self.active_builtin_layout();
        if let Some(state) = &self.workspace_automation_state {
            let work = self.workspace_state.active_work();
            let plan = self.workspace_state.latest_structured_plan();
            // Latest Principal failure lives as an Error card in the work
            // item's conversation now (RELEASE-005 keeps it inspectable).
            let principal_error = self
                .work_item_agents
                .get(&self.workspace_work_item_id)
                .and_then(|agent| {
                    agent.read(app, |agent, _| {
                        agent.controller().read(app, |controller, _| {
                            controller.agent_blocks().iter().rev().find_map(|block| {
                                match &block.kind {
                                    crate::agent::AgentBlockKind::Error { message, .. } => {
                                        Some(message.clone())
                                    }
                                    _ => None,
                                }
                            })
                        })
                    })
                });
            let snapshot = serde_json::json!({
                "ready": true,
                "enabled": self.agentic_workspace_enabled,
                "layout": format!("{:?}", active_layout).to_ascii_lowercase(),
                "work": work.map(|work| serde_json::json!({
                    "id": work.id.0.to_string(),
                    "title": work.title,
                    "status": format!("{:?}", work.status).to_ascii_lowercase(),
                })),
                "plan": plan.map(|plan| serde_json::json!({
                    "status": format!("{:?}", plan.status).to_ascii_lowercase(),
                    "tasks": plan.tasks.len(),
                    "completed_tasks": plan.tasks.iter().filter(|task| task.status == dragent_workspace_model::PlanTaskStatus::Completed).count(),
                })),
                "attempts": self.workspace_state.execution_attempts().len(),
                "evidence": self.workspace_state.evidence().len(),
                "attention": self.workspace_state.attention_items().iter().filter(|item| matches!(item.status, AttentionStatus::Open | AttentionStatus::Acknowledged)).count(),
                "error": principal_error.as_deref().or(self.shape_coordination_error.as_deref()),
            });
            if let Ok(mut state) = state.lock() {
                *state = snapshot;
            }
        }
        // Workspace accents style content and its tabs, never global chrome.
        let theme = if self.settings_open || self.automations_open || self.teams_open
            || self.tasks_open || self.pull_requests_open || self.project_start_visible {
            self.theme
        } else { self.workspace_theme() };
        let ui = self.ui_font;

        let tabs = &self.tabs;

        let header_mouse_states = &self.header_mouse_states;
        let header_mouse_state = move |key: &'static str| {
            header_mouse_states
                .borrow_mut()
                .entry(key.to_string())
                .or_insert_with(MouseStateHandle::default)
                .clone()
        };

        let active_tab = tabs
            .iter()
            .find(|t| t.id == self.active_tab_id)
            .or_else(|| tabs.first());

        if !self.settings_open {
            if let (Some(reason), Some(tab)) = (self.shared_session_ended.as_deref(), active_tab) {
                if self.mirror_tab_id == Some(tab.id) {
                    return self.render_shared_session_ended(reason, &theme, ui);
                }
            }
        }

        // The Automations page is a full-page surface like Settings.
        let settings_active = self.settings_open || self.automations_open || self.teams_open || self.tasks_open || self.pull_requests_open;
        let new_tab_menu_open = self.new_tab_picker_open;

        // True empty state: the start surface with no tabs and no selected
        // project. The header (every segment but the sidebar's) and the
        // right panel hide here; only the left sidebar stays open. A
        // selected project keeps the window controls available. Workspace
        // tabs are masked separately whenever the creation surface is shown.
        let has_live_project = self.open_project_roots.iter().any(|root| root.is_dir());
        let start_empty = self.project_start_visible
            && !settings_active
            && self.tabs.is_empty()
            && !has_live_project;

        // Standalone and split terminals share one pane renderer so their
        // header geometry, controls, focus treatment, and body cannot drift.
        // Settings remains headerless because it owns its internal chrome.
        #[cfg(not(target_family = "wasm"))]
        let automations_page = if self.pull_requests_open && !self.settings_open {
            if let Some(window) = app.windows().platform_window(self.window_id) {
                let sidebar = if self.left_panel_visible { self.left_panel_state.lock().ok().map(|s|s.size()).unwrap_or(0.0) } else {0.0};
                self.pull_requests_view.read(app, |p,_| p.width.set(window.size().x()-sidebar));
            }
            Some(ChildView::new(&self.pull_requests_view).finish())
        } else if self.teams_open && !self.settings_open {
            // The page lays out by its width (the info column folds under
            // the header when it's narrow).
            if let Some(window) = app.windows().platform_window(self.window_id) {
                let sidebar = self
                    .left_panel_visible
                    .then(|| self.left_panel_state.lock().ok().map(|state| state.size()))
                    .flatten()
                    .unwrap_or(0.0);
                let width = window.size().x() - sidebar;
                self.teams_view.read(app, |page, _| page.width.set(width));
            }
            Some(ChildView::new(&self.teams_view).finish())
        } else if self.tasks_open && !self.settings_open {
            if let Some(window) = app.windows().platform_window(self.window_id) {
                let sidebar = self
                    .left_panel_visible
                    .then(|| self.left_panel_state.lock().ok().map(|state| state.size()))
                    .flatten()
                    .unwrap_or(0.0);
                let width = window.size().x() - sidebar;
                self.tasks_view.read(app, |page, _| page.width.set(width));
            }
            Some(ChildView::new(&self.tasks_view).finish())
        } else {
            (self.automations_open && !self.settings_open)
                .then(|| ChildView::new(&self.automations_view).finish())
        };
        #[cfg(target_family = "wasm")]
        let automations_page: Option<Box<dyn Element>> = None;
        let terminal = if let Some(page) = automations_page {
            page
        } else if settings_active {
            ChildView::new(&self.settings_view).finish()
        } else if let Some(layout) = &self.split_tree {
            if layout.pane_count() > 1 || layout.contains(EDITOR_PANE_ID) {
                self.render_split_tree(layout.root(), &theme, ui, app, header_mouse_state)
            } else if let Some(tab) = active_tab {
                if tab.pane.is_settings() {
                    self.render_tab_pane(&tab.pane, app)
                } else {
                    self.render_split_tab_pane(tab.id, &theme, ui, app)
                }
            } else {
                Empty::new().finish()
            }
        } else if let Some(tab) = active_tab {
            if tab.pane.is_settings() {
                self.render_tab_pane(&tab.pane, app)
            } else {
                self.render_split_tab_pane(tab.id, &theme, ui, app)
            }
        } else {
            Empty::new().finish()
        };
        let start_width = app
            .windows()
            .platform_window(self.window_id)
            .map(|window| {
                let sidebars = self
                    .left_panel_visible
                    .then(|| self.left_panel_state.lock().ok().map(|state| state.size()))
                    .flatten()
                    .unwrap_or(0.0)
                    + (self.right_panel_visible && !self.project_start_visible)
                        .then(|| self.right_panel_state.lock().ok().map(|state| state.size()))
                        .flatten()
                        .unwrap_or(0.0);
                (window.size().x() - sidebars - 48.0).max(320.0)
            })
            .unwrap_or(680.0);
        let selected_project = self.selected_project_root();
        let start_machines = super::project_start::start_machines(app);
        let terminal = if self.project_start_visible && !settings_active {
            if let Some(root) = selected_project.as_ref().filter(|root| root.is_dir()) {
                if self.project_start_mode.is_some() {
                    // Clone/SSH forms keep the classic start surface so their
                    // inputs stay visible while a project is selected.
                    super::project_start::render(super::project_start::ProjectStartProps {
                        workspace_view_id: self.workspace_view_id,
                        theme: &theme,
                        ui_font: ui,
                        editor: &self.project_start_editor,
                        mode: self.project_start_mode,
                        has_input: !self.project_start_text.trim().is_empty(),
                        busy: self.project_start_busy,
                        error: None,
                        active_project: Some(root),
                        recent_projects: &self.project_start_recent_projects,
                        recent_ssh_hosts: &self.project_start_recent_ssh_hosts,
                        ssh_config_hosts: &self.project_start_ssh_config_hosts,
                        machines: &start_machines,
                        available_width: start_width,
                        mouse_states: &self.new_tab_picker_mouse_states,
                    })
                } else {
                    let project_root = super::project_naming::worktree_main_checkout(root)
                        .unwrap_or_else(|| root.clone());
                    let project_name = project_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Project")
                        .to_string();
                    let worktree_name = if self.prompt_selected_worktree.is_some()
                        || !self.prompt_worktree_available()
                    {
                        "Local".to_string()
                    } else {
                        "Worktree".to_string()
                    };
                    super::project_prompt::render(super::project_prompt::ProjectPromptProps {
                        workspace_view_id: self.workspace_view_id,
                        theme: &theme,
                        ui_font: ui,
                        mono_font: self.mono_font,
                        editor: &self.prompt_editor,
                        project_root: root,
                        project_name,
                        agent: self.prompt_agent,
                        team_on: self.prompt_team,
                        lead_model: Some(crate::teams::models::display_label(self.prompt_agent, self.prompt_lead_model.as_deref(), app)),
                        team_agents: self.prompt_team_agents.clone(),
                        prompt_empty: self.prompt_text.trim().is_empty()
                            && self.prompt_attachments.is_empty()
                            && self.prompt_links.is_empty(),
                        busy: self.project_start_busy,
                        attachment_names: self
                            .prompt_attachments
                            .iter()
                            .map(|file| file.name().to_string())
                            .collect(),
                        attachment_images: self
                            .prompt_attachments
                            .iter()
                            .map(|file| file.image_path().map(std::path::Path::to_path_buf))
                            .collect(),
                        linked_items: self.prompt_links.clone(),
                        task: self
                            .prompt_task
                            .as_ref()
                            .map(|task| (task.key.clone(), task.title.clone())),
                        attachment_scroll: &self.prompt_attachment_scroll,
                        voice_ui: self.prompt_voice_ui,
                        voice_spin: self.prompt_voice_spin,
                        notice: self.prompt_notice.as_deref(),
                        current_branch: self.prompt_branch.clone(),
                        worktree_name,
                        device_label: crate::remote_fs::machine_name(root)
                            .unwrap_or_else(super::project_prompt::device_label),
                        machine: crate::remote_fs::resolve(root).map(|remote| {
                            use warpui::SingletonEntity;
                            let store = crate::settings::machines_store::MachinesStore::handle(app);
                            let store = store.as_ref(app);
                            super::project_prompt::PromptMachine {
                                name: store
                                    .machine(&remote.machine_id)
                                    .map(|m| m.name())
                                    .unwrap_or_else(|| "This machine".into()),
                                status: store.status(&remote.machine_id),
                                error: store.connection(&remote.machine_id).and_then(|c| c.error.clone()),
                                runtime_down: store.connection(&remote.machine_id).is_some_and(|c| {
                                    use crate::settings::machines_store::{RuntimeState, SshState};
                                    c.ssh == SshState::Connected
                                        && matches!(c.runtime, RuntimeState::Stopped | RuntimeState::Failed)
                                }),
                                id: remote.machine_id,
                            }
                        }),
                        error: None,
                        available_width: start_width,
                        mouse_states: &self.new_tab_picker_mouse_states,
                    })
                }
            } else {
                super::project_start::render(super::project_start::ProjectStartProps {
                    workspace_view_id: self.workspace_view_id,
                    theme: &theme,
                    ui_font: ui,
                    editor: &self.project_start_editor,
                    mode: self.project_start_mode,
                    has_input: !self.project_start_text.trim().is_empty(),
                    busy: self.project_start_busy,
                    error: None,
                    active_project: None,
                    recent_projects: &self.project_start_recent_projects,
                    recent_ssh_hosts: &self.project_start_recent_ssh_hosts,
                    ssh_config_hosts: &self.project_start_ssh_config_hosts,
                    machines: &start_machines,
                    available_width: start_width,
                    mouse_states: &self.new_tab_picker_mouse_states,
                })
            }
        } else {
            terminal
        };

        let mut terminal_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max);

        // Viewer status bar: shown only on the joined mirror tab.
        if !settings_active && active_tab.is_some_and(|t| Some(t.id) == self.mirror_tab_id) {
            let sm = SessionManager::handle(app);
            let snap = sm.read(app, |m, _| viewer_bar::ViewerBarSnapshot {
                online: m.is_online(),
                can_edit: m.viewer_can_edit(),
                participants: m
                    .participants()
                    .iter()
                    .map(|p| share_sheet::ShareParticipant {
                        id: p.id.0.to_string(),
                        name: if p.display_name.is_empty() {
                            "Guest".to_string()
                        } else {
                            p.display_name.clone()
                        },
                        email: String::new(),
                        avatar_url: p.avatar_url.clone(),
                        control: p.role == Role::Controller,
                        is_host: false,
                    })
                    .collect(),
            });
            terminal_column = terminal_column.with_child(viewer_bar::render_viewer_bar(
                &snap,
                &theme,
                ui,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            ));
        }

        let mut terminal_column =
            terminal_column.with_child(Shrinkable::new(1.0, terminal).finish());

        // Device forwarding is a first-class workspace surface, not part of a
        // shared terminal session. Keep its bar visible on every local terminal
        // tab and preserve it when sharing starts or stops. Editor-only
        // layouts (terminal closed, file open) keep the same bottom rail so
        // git/ports/language stay reachable without a terminal. A selected
        // folder with no tabs yet (just opened, terminal not started) also
        // keeps the rail — the welcome cards are only for the no-session
        // state (no project at all).
        let folder_selected = self
            .open_project_roots
            .last()
            .is_some_and(|root| root.is_dir());
        let editor_only_bottom_bar = self.tabs.is_empty()
            && (self.document_center_open || self.editor_pane_in_layout() || folder_selected);
        if !settings_active
            && !self.project_start_visible
            && (active_tab.is_some_and(|t| Some(t.id) != self.mirror_tab_id)
                || editor_only_bottom_bar)
        {
            let sm = SessionManager::handle(app);
            // Feed the sidebar's source-control panel from the active
            // session's git model (single source of truth: the bottom-bar
            // chips, this snapshot, and the panel all read the same state).
            let (terminal_view_id, git_model, git_cwd) = self
                .tabs
                .iter()
                .find(|tab| tab.id == self.active_tab_id)
                .and_then(|tab| self.git_terminal_for(tab))
                .and_then(|(owner_id, handle)| {
                    let owner = self.tabs.iter().find(|tab| tab.id == owner_id)?;
                    Some((handle, owner.project_root.clone()?))
                })
                .map(|(handle, project_root)| {
                    let status_model = handle.read(app, |view, _| view.git_repo_status());
                    let root = super::project_sidebar::find_git_root(&project_root)
                        .unwrap_or(project_root);
                    (Some(handle.id()), Some(status_model), Some(root))
                })
                .unwrap_or_else(|| {
                    // Folder-only state (no terminal): fall back to the
                    // workspace-owned status model so the bottom rail and
                    // Source Control stay populated for the selected folder.
                    match (self.folder_git_model.clone(), self.folder_git_root.clone()) {
                        (Some(model), Some(root)) => (None, Some(model), Some(root)),
                        _ => (None, None, None),
                    }
                });
            // No live git model anywhere: keep the folder path so the bottom
            // rail and Source Control still point at the user's actual folder
            // instead of going blank/root.
            let git_cwd =
                git_cwd.or_else(|| self.selected_project_root().filter(|root| root.is_dir()));
            {
                let armed = self.git_panel_state.borrow().armed_discard.clone();
                *self.git_panel_state.borrow_mut() = super::project_sidebar::GitPanelState {
                    git_model,
                    terminal_view_id,
                    cwd: git_cwd,
                    armed_discard: armed,
                };
            }
            let git = self
                .git_panel_state
                .borrow()
                .git_model
                .as_ref()
                .and_then(|model| {
                    model.read(app, |m, _| {
                        m.status.clone().map(|status| (status, m.operation))
                    })
                });
            let git_tab_active = self.right_panel_visible
                && self
                    .project_sidebar_view
                    .read(app, |view, _| view.sidebar_tab())
                    == super::project_sidebar::SidebarTab::SourceControl;
            let ports_snapshot = sm.read(app, |m, _| super::ports_bar::PortsSnapshot {
                forwards: m
                    .port_forwards()
                    .iter()
                    .map(|forward| super::ports_bar::PortRow {
                        id: forward.id.clone(),
                        port: forward.port,
                        url: forward.url.clone(),
                    })
                    .collect(),
                busy: m.forward_busy(),
                error: m.forward_error().map(str::to_string),
                notice: self.ports_notice.clone(),
                git: git.map(|(status, operation)| super::ports_bar::GitSnapshot {
                    branch: status.branch.clone(),
                    changed_files: status.total_changes(),
                    staged: status.staged,
                    modified: status.modified,
                    untracked: status.untracked,
                    ahead: status.ahead,
                    behind: status.behind,
                    added: status.added,
                    deleted: status.deleted,
                    operation,
                }),
                language_services: self.language_service_status.clone(),
            });
            terminal_column = terminal_column.with_child(super::ports_bar::render_ports_bar(
                &ports_snapshot,
                self.ports_open,
                self.language_services_open,
                git_tab_active,
                &theme,
                ui,
                self.mono_font,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            ));
        }

        // Keep the settings overlay full-height without placing a max-sized
        // element inside the settings scroll content. This column is already
        // laid out against the bounded workspace viewport.
        let terminal_column: Box<dyn Element> = if settings_active {
            Container::new(terminal_column.finish())
                .with_background_color(theme.background)
                .finish()
        } else {
            terminal_column.finish()
        };

        let mut panels_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max);

        // Project navigation remains available while Settings owns the main
        // screen. Its workspace memberships are unchanged.
        if self.left_panel_visible {
            let left_panel_view_id = self.left_panel_view.id();
            // Full-height column: the navbar segment sits above the panel
            // so the sidebar demarcation passes through the navbar, and the
            // collapse toggle stays reachable on the empty start screen. The
            // panel is Shrinkable (not a bare child): a Flex column
            // measures bare children with an unbounded main axis, which
            // the panel's Max-size column correctly rejects.
            let left_column = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(header::render_left_segment(
                    &self.theme,
                    chrome_inset::header_left_padding(app, self.window_id),
                    self.workspace_view_id,
                    self.left_panel_visible,
                    header_mouse_state,
                ))
                .with_child(
                    Shrinkable::new(1.0, ChildView::new(&self.left_panel_view).finish()).finish(),
                )
                .finish();
            let left_panel = Resizable::new(self.left_panel_state.clone(), left_column)
                .with_dragbar_side(DragBarSide::Right)
                .on_resize(move |ctx, _| {
                    ctx.dispatch_typed_action_for(
                        left_panel_view_id,
                        left_panel::LeftPanelAction::Resize,
                    );
                    ctx.notify();
                })
                .with_bounds_callback(Box::new(|window_size| {
                    let max = window_size.x() * 0.5;
                    (MIN_PANEL_WIDTH, max.max(MIN_PANEL_WIDTH))
                }))
                .finish();
            panels_row = panels_row.with_child(left_panel);
        }

        let review_tab_active = self.project_sidebar_view.read(app, |view, _| {
            view.sidebar_tab == super::project_sidebar::SidebarTab::Review
        });
        // Badge on the navbar Review face: unresolved review threads only,
        // hidden at zero. Read from the same polled state as the tab.
        let open_review_count = self.project_sidebar_view.read(app, |view, _| {
            view.review_pr
                .comments
                .iter()
                .filter(|comment| comment.kind == "review" && !comment.is_resolved)
                .count()
        });
        // Live Resizable width (same pattern as the left panel rows): the
        // strip sizes to this exactly, so it never depends on parent
        // tightening that the framework does not guarantee.
        let right_panel_width = self
            .right_panel_state
            .lock()
            .ok()
            .map(|state| state.size())
            .unwrap_or(DEFAULT_RIGHT_PANEL_WIDTH);
        // Maximize/restore glyph choice for the Windows caption buttons.
        let window_maximized = app
            .windows()
            .platform_window(self.window_id)
            .is_some_and(|w| {
                w.fullscreen_state() == warpui_core::platform::FullscreenState::Maximized
            });
        // The project sidebar is workspace chrome, not terminal chrome. An AI
        // CLI entering the terminal's alternate screen must not hide it or
        // prevent the user from reopening it. Only Settings and the user's
        // explicit collapse control visibility.
        let right_panel_shown = !settings_active
            && self.right_panel_visible
            && !start_empty
            && !self.project_start_visible;
        let mut center_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max);
        // The empty start screen keeps the header row's height but hides
        // every control except the Windows caption cluster (minimal mode);
        // the sidebar header above the left panel stays fully visible.
        // The launcher is outside a workspace. Preserve its sessions in
        // memory while hiding their tabs and add-tab action.
        let header_workspace_tabs = if !self.workspace_tab_header_visible() {
            None
        } else {
            Some(self.render_workspace_tab_strip(app, &theme))
        };
        center_column = center_column.with_child(header::render_center_segment(
            ui,
            &self.theme,
            self.workspace_view_id,
            self.left_panel_visible,
            chrome_inset::header_left_padding(app, self.window_id),
            header_mouse_state,
            settings_active,
            self.account_logged_in_cache,
            self.account_identity(app),
            self.open_attention_entries().len(),
            self.unread_automation_results(app),
            self.attention_bell_open,
            window_maximized,
            right_panel_shown,
            start_empty,
            header_workspace_tabs,
            self.header_pills(app, &self.theme, settings_active || start_empty),
            self.usage_chip(header_mouse_state),
        ));
        let center_column =
            center_column.with_child(Shrinkable::new(1.0, terminal_column).finish());
        panels_row = panels_row.with_child(Shrinkable::new(1.0, center_column.finish()).finish());

        // Agent quick-access rail: icons-only (or expanded) agent launcher
        // directly beside the right sidebar. Workspace chrome only — never
        // on the start screen, the project-prompt/creation surface (tabs may
        // exist behind it while no worktree is open), or without open panes.
        if !settings_active
            && !start_empty
            && !self.project_start_visible
            && !self.tabs.is_empty()
        {
            panels_row = panels_row.with_child(self.render_agent_rail(app, &theme));
        }

        // Full-height column mirroring the left: navbar segment above the
        // panel continues the demarcation line through the bar. The whole
        // strip (Review button included) collapses with the panel.
        let right_panel_content: Box<dyn Element> = ChildView::new(&self.project_sidebar_view).finish();
        if right_panel_shown {
            let right_column = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(header::render_right_segment(
                    ui,
                    &theme,
                    self.workspace_view_id,
                    header_mouse_state,
                    window_maximized,
                    review_tab_active,
                    &self.review_ship,
                    open_review_count,
                    right_panel_width,
                ))
                .with_child(Shrinkable::new(1.0, right_panel_content).finish())
                .finish();
            let right_panel = Resizable::new(self.right_panel_state.clone(), right_column)
                .with_dragbar_side(DragBarSide::Left)
                .on_resize(|ctx, _| ctx.notify())
                .with_bounds_callback(Box::new(|window_size| {
                    let max = window_size.x() * 0.5;
                    (MIN_RIGHT_PANEL_WIDTH, max.max(MIN_RIGHT_PANEL_WIDTH))
                }))
                .finish();
            panels_row = panels_row.with_child(right_panel);
        }

        // Agentic work layouts share the same work-item and surface
        // identities. Selecting a presentation changes only placement; live
        // Develop views and processes remain registered and untouched.
        let work_viewport = app
            .windows()
            .platform_window(self.window_id)
            .map(|window| window.size());
        let work_viewport_width = work_viewport
            .map(|size| size.x() - (WORKSPACE_PADDING * 2.0))
            .unwrap_or(1240.0)
            .max(1.0);
        let work_viewport_height = work_viewport
            .map(|size| size.y() - HEADER_BAR_HEIGHT - (WORKSPACE_PADDING * 2.0))
            .unwrap_or(760.0)
            .max(1.0);
        let active_work_status = self
            .workspace_state
            .active_work()
            .map(|work| work.status)
            .unwrap_or(WorkStatus::Draft);
        let active_work_title = self
            .workspace_state
            .active_work()
            .map(|work| work.title.clone());
        let legacy_account = crate::settings::AccountStore::handle(app);
        let (legacy_display_name, legacy_avatar_url) =
            app.read_model(&legacy_account, |store, _| {
                store
                    .data
                    .as_ref()
                    .map(|d| (d.user.display_name.clone(), d.user.avatar_url.clone()))
                    .unwrap_or_default()
            });
        let mut shape_messages = self
            .workspace_state
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                WorkspaceEventKind::UserMessagePosted { body } => {
                    Some(super::shape_workspace::ConversationMessage {
                        actor: "You".to_string(),
                        time: chrono::DateTime::from_timestamp_millis(event.timestamp_unix_ms)
                            .map(|time| time.format("%-I:%M %p").to_string())
                            .unwrap_or_default(),
                        body: body.clone(),
                        principal: false,
                        display_name: legacy_display_name.clone(),
                        avatar_url: legacy_avatar_url.clone(),
                    })
                }
                WorkspaceEventKind::PrincipalMessagePosted { body } => {
                    Some(super::shape_workspace::ConversationMessage {
                        actor: "Principal Agent".to_string(),
                        time: chrono::DateTime::from_timestamp_millis(event.timestamp_unix_ms)
                            .map(|time| time.format("%-I:%M %p").to_string())
                            .unwrap_or_default(),
                        body: body.clone(),
                        principal: true,
                        display_name: String::new(),
                        avatar_url: String::new(),
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let shape_proposal = self.workspace_state.latest_shaping_proposal().cloned();
        let shape_work_brief = self.workspace_state.latest_work_brief().cloned();
        let shape_records = self.workspace_state.shaping_records_projection();
        let shape_plan = self.workspace_state.latest_structured_plan().cloned();
        let shape_artifacts = self.workspace_state.artifacts();
        let (shape_model_routes, shape_model_picker_locked) = shape_model_route_catalog(app);
        let shape_proposed_roles =
            self.shaping_proposed_role_views(&shape_artifacts, shape_records.as_ref());
        let coordination_readiness = self.workspace_state.coordination_readiness();
        let coordination_ready = coordination_readiness.is_approvable()
            && matches!(
                active_work_status,
                WorkStatus::Planning | WorkStatus::ReadyToExecute
            );
        let coordination_summary = if active_work_title.is_none() {
            "Create work to begin coordination review".to_string()
        } else if coordination_readiness.role_count == 0 {
            "Waiting for a proposed team".to_string()
        } else if !coordination_readiness.plan_present {
            "Waiting for a structured dependency plan".to_string()
        } else if coordination_readiness.issues.iter().any(|issue| {
            matches!(
                issue,
                dragent_workspace_model::CoordinationReadinessIssue::OpenDecisionsRemain(_)
            )
        }) {
            "Waiting for an important shaping decision".to_string()
        } else if coordination_readiness.is_ready() {
            format!(
                "Team ready · {}/{} role contracts approved · no agents running",
                coordination_readiness.approved_role_count, coordination_readiness.role_count
            )
        } else if coordination_readiness.is_approvable() {
            format!(
                "Plan ready for approval · {}/{} role contracts approved",
                coordination_readiness.approved_role_count, coordination_readiness.role_count
            )
        } else {
            format!(
                "Team review · {}/{} role contracts approved",
                coordination_readiness.approved_role_count, coordination_readiness.role_count
            )
        };
        let local_workspace_actor_id =
            ActorRef::local_user(self.workspace_state.project().id, None).id;
        let mut shape_activity = self
            .workspace_state
            .events()
            .iter()
            .rev()
            .filter_map(|event| {
                use super::shape_workspace::{ActivityItem, ActivityTone};
                let time = chrono::DateTime::from_timestamp_millis(event.timestamp_unix_ms)
                    .map(|time| time.format("%-I:%M %p").to_string())
                    .unwrap_or_default();
                let (actor, summary, kind, tone) = match &event.kind {
                    WorkspaceEventKind::WorkCreated { title } => (
                        "You",
                        format!("created {title}"),
                        "Work",
                        ActivityTone::User,
                    ),
                    WorkspaceEventKind::WorkStatusChanged { status } => (
                        "Workspace",
                        format!("moved work to {status:?}"),
                        "Lifecycle",
                        ActivityTone::Work,
                    ),
                    WorkspaceEventKind::UserMessagePosted { .. } => (
                        "You",
                        "added shaping context".into(),
                        "Message",
                        ActivityTone::User,
                    ),
                    WorkspaceEventKind::PrincipalMessagePosted { .. } => (
                        "Principal Agent",
                        "responded to the shaping thread".into(),
                        "Message",
                        ActivityTone::Principal,
                    ),
                    WorkspaceEventKind::ShapingProposalUpdated { .. } => (
                        "Principal Agent",
                        "updated the proposed plan".into(),
                        "Plan",
                        ActivityTone::Principal,
                    ),
                    WorkspaceEventKind::ShapingRecordsUpdated { records } => (
                        "Principal Agent",
                        format!(
                            "structured {} requirements, {} decisions, and {} criteria",
                            records.requirements.len(),
                            records.decisions.len(),
                            records.acceptance_criteria.len(),
                        ),
                        "Work brief",
                        ActivityTone::Principal,
                    ),
                    WorkspaceEventKind::RoleDefinitionUpdated { role_definition } => (
                        "Principal Agent",
                        format!("proposed the {} role", role_definition.name),
                        "Role draft",
                        ActivityTone::Team,
                    ),
                    WorkspaceEventKind::ProposedTeamUpdated { proposed_team } => (
                        "Principal Agent",
                        format!("assembled {} draft roles", proposed_team.roles.len()),
                        "Team draft",
                        ActivityTone::Team,
                    ),
                    WorkspaceEventKind::WorkContractUpdated { work_contract } => (
                        if work_contract.updated_by == local_workspace_actor_id {
                            "You"
                        } else {
                            "Principal Agent"
                        },
                        format!(
                            "updated a role contract to {:?} (v{})",
                            work_contract.status, work_contract.version
                        ),
                        "Role contract",
                        ActivityTone::Team,
                    ),
                    WorkspaceEventKind::WorkBriefUpdated { .. }
                    | WorkspaceEventKind::RequirementUpdated { .. }
                    | WorkspaceEventKind::DecisionUpdated { .. }
                    | WorkspaceEventKind::AcceptanceCriterionUpdated { .. } => (
                        "You",
                        "updated a shaping record".into(),
                        "Edit",
                        ActivityTone::User,
                    ),
                    _ => return None,
                };
                Some(ActivityItem {
                    time,
                    actor: actor.into(),
                    summary,
                    kind: kind.into(),
                    tone,
                })
            })
            .take(6)
            .collect::<Vec<_>>();
        shape_activity.reverse();
        let workspace_checkpoints = self.workspace_execution_runtime.checkpoint_summaries();
        let review_evidence = self
            .workspace_state
            .evidence()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let review_attention = self
            .workspace_state
            .attention_items()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let completion_report = shape_plan.as_ref().and_then(|plan| {
            shape_records.as_ref().map(|records| {
                dragent_workspace_model::build_completion_report(
                    plan,
                    &records.acceptance_criteria,
                    &review_evidence,
                    &review_attention,
                )
            })
        });
        let (legacy_shape_models, legacy_shape_selected_label, legacy_shape_model_locked) =
            shape_composer_model_options(app);
        let body = match active_layout {
            BuiltInLayout::Shape => Shrinkable::new(
                1.0,
                // Enter-to-send wraps INSIDE the Shrinkable: EventHandler does
                // not forward FlexParentData, so wrapping outside would hide
                // the flex factor and collapse layout (unbounded measure).
                self.wrap_composer_enter_to_send(super::shape_workspace::render(
                    &theme,
                    ui,
                    work_viewport_width,
                    work_viewport_height,
                    self.shape_canvas_horizontal_scroll.clone(),
                    self.shape_canvas_vertical_scroll.clone(),
                    self.workspace_view_id,
                    header_mouse_state("approve_plan_and_coordinate"),
                    header_mouse_state("submit_shape_message"),
                    header_mouse_state("describe_shape_role"),
                    self.shape_composer_editor.clone(),
                    !self.shape_composer_text.trim().is_empty() && !self.shape_principal_pending,
                    &shape_messages,
                    shape_proposal.as_ref(),
                    shape_work_brief.as_ref(),
                    shape_records.as_ref(),
                    &shape_artifacts,
                    shape_plan.as_ref(),
                    &shape_proposed_roles,
                    &shape_model_routes,
                    self.shape_model_picker_contract,
                    shape_model_picker_locked,
                    &shape_activity,
                    self.shape_selected_role,
                    self.shape_role_action_pending,
                    None,
                    self.shape_record_edit_target.as_ref(),
                    self.shape_record_editor.clone(),
                    !self.shape_record_edit_text.trim().is_empty(),
                    self.shape_record_edit_pending,
                    None,
                    coordination_ready,
                    self.shape_coordination_pending,
                    &coordination_summary,
                    None,
                    self.shape_principal_pending,
                    &[],
                    active_work_title.as_deref(),
                    active_work_status,
                    &self.shape_composer_attachments,
                    self.shape_composer_notice.as_deref(),
                    self.shape_voice_ui,
                    self.shape_voice_spin,
                    self.shape_model_menu_open,
                    &legacy_shape_models,
                    &legacy_shape_selected_label,
                    legacy_shape_model_locked,
                    header_mouse_state("shape_model_trigger"),
                    header_mouse_state,
                    self.shape_approved,
                    shape_proposal.is_some(),
                    header_mouse_state("approve_shape_message"),
                    self.shape_requirements_complete,
                    self.shape_remove_armed.as_ref(),
                )),
            )
            .finish(),
            BuiltInLayout::Coordinate => Shrinkable::new(
                1.0,
                super::execution_workspace::render(
                    &theme,
                    ui,
                    work_viewport_width,
                    work_viewport_height,
                    self.coordinate_canvas_horizontal_scroll.clone(),
                    self.coordinate_canvas_vertical_scroll.clone(),
                    active_work_title.as_deref(),
                    active_work_status,
                    self.workspace_execution_runtime.active_count(),
                    shape_plan.as_ref(),
                    &shape_proposed_roles,
                    &self
                        .workspace_state
                        .execution_attempts()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                    &self
                        .workspace_state
                        .agent_messages()
                        .into_iter()
                        .map(|(message, routing)| (message.clone(), routing.clone()))
                        .collect::<Vec<_>>(),
                    &review_evidence,
                    &self
                        .workspace_state
                        .attention_items()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                    &workspace_checkpoints,
                    self.workspace_checkpoint_notice.as_deref(),
                    None,
                    self.workspace_view_id,
                    self.coordinate_activity_scroll.clone(),
                    self.coordinate_activity_dock_state.clone(),
                ),
            )
            .finish(),
            BuiltInLayout::Review => Shrinkable::new(
                1.0,
                super::review_workspace::render(
                    &theme,
                    ui,
                    work_viewport_width,
                    work_viewport_height,
                    self.review_canvas_horizontal_scroll.clone(),
                    self.review_canvas_vertical_scroll.clone(),
                    active_work_title.as_deref(),
                    active_work_status,
                    completion_report.as_ref(),
                    &review_evidence,
                    self.workspace_view_id,
                ),
            )
            .finish(),
            _ => Shrinkable::new(1.0, panels_row.finish()).finish(),
        };

        let is_develop = matches!(active_layout, BuiltInLayout::Develop);
        let mut workspace_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max);
        if !is_develop {
            // Agentic full-screen layouts have no side columns: a single
            // strip carries the navbar segments (toggle, center controls,
            // and — on Windows or with the panel up — the right strip).
            let mut strip = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
            if self.left_panel_visible {
                strip = strip.with_child(header::render_left_segment(
                    &self.theme,
                    chrome_inset::header_left_padding(app, self.window_id),
                    self.workspace_view_id,
                    self.left_panel_visible,
                    header_mouse_state,
                ));
            }
            strip = strip.with_child(
                Shrinkable::new(
                    1.0,
                    header::render_center_segment(
                        ui,
                        &self.theme,
                        self.workspace_view_id,
                        self.left_panel_visible,
                        chrome_inset::header_left_padding(app, self.window_id),
                        header_mouse_state,
                        settings_active,
                        self.account_logged_in_cache,
                        self.account_identity(app),
                        self.open_attention_entries().len(),
                        self.unread_automation_results(app),
                        self.attention_bell_open,
                        window_maximized,
                        right_panel_shown,
                        start_empty,
                        // No open panes means no workspace: hide the
                        // workspace tabs and their add button too.
                        if !self.workspace_tab_header_visible() {
                            None
                        } else {
                            Some(self.render_workspace_tab_strip(app, &theme))
                        },
                        self.header_pills(app, &self.theme, settings_active || start_empty),
                        self.usage_chip(header_mouse_state),
                    ),
                )
                .finish(),
            );
            #[cfg(windows)]
            let show_right_strip = true;
            #[cfg(not(windows))]
            let show_right_strip = right_panel_shown;
            if show_right_strip && !start_empty {
                strip = strip.with_child(header::render_right_segment(
                    ui,
                    &theme,
                    self.workspace_view_id,
                    header_mouse_state,
                    window_maximized,
                    review_tab_active,
                    &self.review_ship,
                    open_review_count,
                    right_panel_width,
                ));
            }
            workspace_column = workspace_column.with_child(strip.finish());
        }
        workspace_column = workspace_column.with_child(body);

        // `theme.background` is translucent for background-image themes, so this
        // fill reveals the image (painted behind, below). Opaque chrome that uses
        // blended surface colors (sidebars, popovers, menus) covers it.
        let workspace_fill = Container::new(workspace_column.finish())
            .with_uniform_padding(if settings_active {
                0.0
            } else {
                WORKSPACE_PADDING
            })
            .with_background_color(self.theme.background)
            .finish();

        // Theme background image (Warp themes): opaque base + image behind the
        // (translucent) workspace fill, so it shows through the terminal.
        let root_inner: Box<dyn Element> = match crate::theme::Theme::bg_image_for(theme.id) {
            Some(bg) => {
                let opaque_base = Container::new(Empty::new().finish())
                    .with_background_color(warpui::color::ColorU::new(
                        self.theme.background.r,
                        self.theme.background.g,
                        self.theme.background.b,
                        255,
                    ))
                    .finish();
                // `Original` (GPU scales the texture to cover) instead of `BySize`
                // — the latter resizes the multi-MB source image on the UI thread
                // the first time, which froze the app until it finished.
                let image = warpui::elements::Image::new(
                    bg.asset_source(),
                    warpui::elements::CacheOption::Original,
                )
                .cover()
                // Use the theme's DESIGNED opacity. Warp tunes this per image so
                // its foreground text stays readable — e.g. `winter` ships 47 and
                // `snowy` 20 precisely so the bright snow doesn't wash out their
                // dark/gray text. A previous `.max(0.7)` floor overrode that and
                // forced every image to ≥70%, which crushed contrast (bright
                // images swallowed the text). The opaque base + this opacity give
                // a uniform, readable background; the workspace fill above adds a
                // further tint toward the theme colour for extra contrast.
                .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                .finish();
                Box::new(
                    Stack::new()
                        .with_child(opaque_base)
                        .with_child(image)
                        .with_child(workspace_fill),
                )
            }
            None => workspace_fill,
        };

        // Management sheets are overlays, not children of the terminal/editor
        // column. Keeping them out of that flex layout preserves the editor's
        // viewport, scroll position, and caret geometry while a sheet is open.
        let root_inner = if self.ports_open || self.language_services_open {
            let manager = SessionManager::handle(app);
            let snapshot = manager.read(app, |manager, _| super::ports_bar::PortsSnapshot {
                forwards: manager
                    .port_forwards()
                    .iter()
                    .map(|forward| super::ports_bar::PortRow {
                        id: forward.id.clone(),
                        port: forward.port,
                        url: forward.url.clone(),
                    })
                    .collect(),
                busy: manager.forward_busy(),
                error: manager.forward_error().map(str::to_string),
                notice: self.ports_notice.clone(),
                git: None,
                language_services: self.language_service_status.clone(),
            });
            match super::ports_bar::render_workspace_sheet(
                &snapshot,
                self.ports_open,
                self.language_services_open,
                &theme,
                ui,
                self.mono_font,
                &self.ports_editor,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            ) {
                Some((sheet, offset)) => Box::new(
                    Stack::new()
                        .with_child(root_inner)
                        // Overlay layer: workspace ChildViews (terminal,
                        // editor, sidebars) paint above normal Stack
                        // children, so every window-level popup, menu, and
                        // modal must use the overlay APIs or it hides
                        // behind the editor.
                        .with_positioned_overlay_child(sheet, offset),
                ) as Box<dyn Element>,
                None => root_inner,
            }
        } else {
            root_inner
        };

        // Layer popups above the workspace.
        let root = self.render_menu_overlays(app, &self.theme, ui, header_mouse_state, root_inner);
        let (root, pending_document_ready) = self.render_sheet_overlays(app, &self.theme, ui, root);
        let settings_active = self.settings_open || self.automations_open || self.teams_open || self.tasks_open || self.pull_requests_open;
        let close_document_confirmation_open = pending_document_ready.is_some();
        let close_workspace_confirmation_open = self.pending_close_workspace_tab.is_some();
        let document_center_open = self.document_center_open;
        let buffer_search_open = self
            .project_sidebar_view
            .read(app, |view, _| view.buffer_search_open());
        let language_results_open = self
            .project_sidebar_view
            .read(app, |view, _| view.language_results_open());
        let buffer_replace_focused = self
            .project_sidebar_view
            .read(app, |view, app| view.buffer_replace_focused(app));
        let (markdown_document_open, markdown_preview_open) =
            self.project_sidebar_view.read(app, |view, app| {
                (
                    view.markdown_document_open(app),
                    view.markdown_preview_open(),
                )
            });

        let workspace_view_id = self.workspace_view_id;
        EventHandler::new(root)
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::BlurAll);
                DispatchEventResult::PropagateToParent
            })
            .on_keydown(move |event_ctx, _app, keystroke| {
                if close_workspace_confirmation_open {
                    if keystroke.key == "escape" {
                        event_ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::CancelCloseWorkspaceTab,
                        );
                    }
                    return DispatchEventResult::StopPropagation;
                }
                if new_tab_menu_open && keystroke.key == "escape" {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::CloseNewTabPicker,
                    );
                    return DispatchEventResult::StopPropagation;
                }
                // Workspace shortcuts must not act on the hidden terminal or
                // editor. Back, Escape and close-window-tab return to it.
                if settings_active {
                    if keystroke.key == "escape"
                        || ((keystroke.cmd || keystroke.ctrl) && keystroke.key == "w")
                    {
                        event_ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::CloseSettings,
                        );
                        return DispatchEventResult::StopPropagation;
                    }
                    return DispatchEventResult::PropagateToParent;
                }
                if language_results_open && keystroke.key == "escape" {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::DismissLanguageResults,
                    );
                    return DispatchEventResult::StopPropagation;
                }
                if buffer_search_open && keystroke.key == "escape" {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::CloseBufferSearch,
                    );
                    return DispatchEventResult::StopPropagation;
                }
                if buffer_search_open && keystroke.key == "enter" {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        if buffer_replace_focused && !keystroke.shift {
                            WorkspaceAction::BufferSearchReplaceCurrent
                        } else if keystroke.shift {
                            WorkspaceAction::BufferSearchPrevious
                        } else {
                            WorkspaceAction::BufferSearchNext
                        },
                    );
                    return DispatchEventResult::StopPropagation;
                }
                if close_document_confirmation_open && keystroke.key == "escape" {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::CancelCloseOpenDocument,
                    );
                    return DispatchEventResult::StopPropagation;
                }
                let modifier = keystroke.cmd || keystroke.ctrl;
                if modifier {
                    match keystroke.key.as_str() {
                        "f" if document_center_open && !markdown_preview_open && !keystroke.alt => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::OpenBufferSearch {
                                    seed_selection: true,
                                },
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "v" if document_center_open
                            && markdown_document_open
                            && keystroke.shift
                            && !keystroke.alt =>
                        {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::ToggleMarkdownPreview,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "s" if !keystroke.shift && !keystroke.alt => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::SaveOpenDocument,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "t" if !keystroke.shift && !keystroke.alt => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::NewTab,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "w" if !keystroke.shift && !keystroke.alt => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::CloseActive,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "d" => {
                            if keystroke.shift {
                                event_ctx.dispatch_typed_action_for(
                                    workspace_view_id,
                                    WorkspaceAction::SplitActiveTabHorizontal,
                                );
                            } else {
                                event_ctx.dispatch_typed_action_for(
                                    workspace_view_id,
                                    WorkspaceAction::SplitActiveTabVertical,
                                );
                            }
                            DispatchEventResult::StopPropagation
                        }
                        "{" | "[" if keystroke.shift => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::SwitchTabPrev,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "}" | "]" if keystroke.shift => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::SwitchTabNext,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        "," if !keystroke.shift && !keystroke.alt => {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::OpenSettings,
                            );
                            DispatchEventResult::StopPropagation
                        }
                        _ => DispatchEventResult::PropagateToParent,
                    }
                } else {
                    DispatchEventResult::PropagateToParent
                }
            })
            .finish()
    }
}

impl WorkspaceView {
    /// Automation results nobody has opened yet, for the bell's badge.
    fn unread_automation_results(&self, app: &AppContext) -> usize {
        #[cfg(not(target_family = "wasm"))]
        {
            use warpui::SingletonEntity;
            crate::automations::AutomationRuntime::handle(app)
                .read(app, |runtime, _| runtime.unread_results())
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = app;
            0
        }
    }

    /// The header pill for the active workspace: the issue it was started
    /// from (none on the web viewer).
    fn header_pills(
        &self,
        app: &AppContext,
        theme: &Theme,
        hidden: bool,
    ) -> Option<Box<dyn Element>> {
        if hidden {
            return None;
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = app;
            return self.render_linked_issue_pill(theme);
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = (app, theme);
            None
        }
    }
}
