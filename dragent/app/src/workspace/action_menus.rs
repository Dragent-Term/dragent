//! Workspace menus and header actions.
//!
//! Handles the corresponding WorkspaceAction variants. Called from the
//! dispatcher (view_actions); bodies moved verbatim.
//!
//! Blocks here are impl WorkspaceView: same type, same behavior.

//! Workspace action dispatch for WorkspaceView.
//!
//! Responsible for:
//! - translating WorkspaceAction intents into domain operations
//! - all action handlers (panes, tabs, terminal, agent, editor, git, sharing)
//!
//! Not responsible for:
//! - rendering (view_render / view_overlays)
//! - defining actions (action.rs)
//! - layout tree algorithms (pane_layout)
//!
//! The block here is the TypedActionView implementation for WorkspaceView:
//! same type, same behavior, organized by responsibility.
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

use super::language_nav::{parse_text_edits_for_path, DefinitionHoverResolution};
use super::view::WorkspaceView;
use super::view::{
    AddProjectMenu, ExplorerClipboardEntry, ExplorerClipboardMode, FolderGitBulkOp,
    FolderGitFileOp, LanguageDocumentSyncState, LanguageRequestPurpose,
};
use super::view_tabs::{apply_sidebar_visibility, refresh_sidebar_projects};

impl WorkspaceView {
    pub(crate) fn handle_menus_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ToggleLayoutMenu => {
                self.layout_menu_open = !self.layout_menu_open;
                self.account_menu_open = false;
                self.close_org_menus();
                ctx.notify();
            }
            WorkspaceAction::CloseLayoutMenu => {
                if self.layout_menu_open {
                    self.layout_menu_open = false;
                    ctx.notify();
                }
            }
            WorkspaceAction::SelectWorkspaceLayout(layout) => {
                self.select_workspace_layout(*layout, ctx);
            }
            WorkspaceAction::ToggleAddProjectMenu => {
                // The sidebar's add button opens the Add Project dialog
                // (Local or a remote machine), not a menu.
                if self.clone_busy {
                    return;
                }
                self.clone_url_error = None;
                self.handle_add_project_action(
                    &super::add_project::AddProjectAction::Open { machine: None },
                    ctx,
                );
            }
            WorkspaceAction::CloseAddProjectMenu => {
                if self.clone_busy {
                    return;
                }
                self.add_project_menu = AddProjectMenu::Closed;
                self.clone_url_error = None;
                ctx.notify();
            }
            WorkspaceAction::ToggleNewTabPicker => {
                self.new_tab_picker_from_sidebar = false;
                self.new_tab_picker_open = !self.new_tab_picker_open;
                ctx.notify();
            }
            WorkspaceAction::ToggleNewTabPickerFromSidebar => {
                self.new_tab_picker_open = false;
                self.new_tab_picker_from_sidebar = false;
                if let Some(root) = self.active_project_root() {
                    self.handle_action(&WorkspaceAction::OpenProjectWorkspacePrompt { root }, ctx);
                } else {
                    self.project_start_visible = true;
                    self.project_start_mode = None;
                    self.project_start_error = None;
                    self.sync_left_panel(ctx);
                    ctx.notify();
                }
            }
            WorkspaceAction::CloseNewTabPicker => {
                self.new_tab_picker_open = false;
                ctx.notify();
            }
            WorkspaceAction::ToggleAccountMenu => {
                let open = !self.account_menu_open;
                self.close_org_menus();
                self.sync_org_menu_highlight(ctx);
                self.account_menu_open = open;
                self.layout_menu_open = false;
                ctx.notify();
            }
            WorkspaceAction::CloseAccountMenu => {
                if self.account_menu_open {
                    self.account_menu_open = false;
                    ctx.notify();
                }
            }
            WorkspaceAction::ToggleUsageMenu => {
                let open = !self.usage_menu_open;
                self.close_org_menus();
                self.sync_org_menu_highlight(ctx);
                self.layout_menu_open = false;
                self.usage_menu_open = open;
                if open {
                    // Each opening starts on the most relevant agent, with
                    // fresh readings.
                    self.usage_menu_agent = None;
                    #[cfg(not(target_family = "wasm"))]
                    self.refresh_agent_usage(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::CloseUsageMenu => {
                if self.usage_menu_open {
                    self.usage_menu_open = false;
                    ctx.notify();
                }
            }
            WorkspaceAction::SelectUsageAgent(agent) => {
                self.usage_menu_agent = Some(*agent);
                ctx.notify();
            }
            WorkspaceAction::OpenPastSessions => {
                #[cfg(not(target_family = "wasm"))]
                self.open_past_sessions(ctx);
            }
            WorkspaceAction::AskDeletePastSession(index) => {
                self.past_sessions_confirm = Some(*index);
                self.past_sessions_note = None;
                ctx.notify();
            }
            WorkspaceAction::CancelDeletePastSession => {
                self.past_sessions_confirm = None;
                ctx.notify();
            }
            WorkspaceAction::DeletePastSession(index) => {
                #[cfg(not(target_family = "wasm"))]
                self.delete_past_session(*index, ctx);
            }
            WorkspaceAction::CompactAgentHistory(provider) => {
                #[cfg(not(target_family = "wasm"))]
                self.compact_agent_history(provider.clone(), ctx);
            }
            WorkspaceAction::ClearPastSessionsSearch => {
                ctx.update_view(&self.past_sessions_search_editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                    editor_ctx.focus_self();
                });
            }
            WorkspaceAction::ClosePastSessions => {
                if self.past_sessions_open {
                    self.past_sessions_open = false;
                    ctx.notify();
                }
            }
            WorkspaceAction::ResumePastSession(index) => {
                #[cfg(not(target_family = "wasm"))]
                self.resume_past_session(*index, ctx);
                #[cfg(target_family = "wasm")]
                let _ = index;
            }
            WorkspaceAction::ToggleContentHeaderMenu => {
                self.content_header_menu_open = !self.content_header_menu_open;
                self.split_share_menu_tab_id = None;
                ctx.notify();
            }
            WorkspaceAction::ToggleSplitShareMenu { tab_id } => {
                let tab_id = *tab_id;
                self.split_share_menu_tab_id = if self.split_share_menu_tab_id == Some(tab_id) {
                    None
                } else {
                    Some(tab_id)
                };
                self.content_header_menu_open = false;
                ctx.notify();
            }
            WorkspaceAction::CloseContentHeaderMenu => {
                self.content_header_menu_open = false;
                self.split_share_menu_tab_id = None;
                ctx.notify();
            }
            WorkspaceAction::ToggleParticipantsMenu => {
                self.participants_menu_open = !self.participants_menu_open;
                self.participant_access_open = None;
                ctx.notify();
            }
            WorkspaceAction::CloseParticipantsMenu => {
                self.participants_menu_open = false;
                self.participant_access_open = None;
                ctx.notify();
            }
            WorkspaceAction::ToggleTabMenu { tab_id } => {
                let tab_id = *tab_id;
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = if lp.sidebar_menu_tab_id == Some(tab_id) {
                        None
                    } else {
                        Some(tab_id)
                    };
                    lp_ctx.notify();
                });
                // The tab-menu overlay is painted by WorkspaceView's own
                // render (see `root` composition), not LeftPanelView's — it
                // needs its own notify to pick up the change immediately.
                ctx.notify();
            }
            WorkspaceAction::CloseTabMenu => {
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                ctx.notify();
            }
            WorkspaceAction::TogglePortsPanel => {
                self.ports_open = !self.ports_open;
                if self.ports_open {
                    self.language_services_open = false;
                }
                ctx.notify();
            }
            WorkspaceAction::ToggleLanguageServicesPanel => {
                self.language_services_open = !self.language_services_open;
                if self.language_services_open {
                    self.ports_open = false;
                }
                ctx.notify();
            }
            WorkspaceAction::OpenShareDropdown { kind } => {
                self.share_open_dropdown = Some(kind.clone());
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::CloseShareDropdown => {
                self.share_open_dropdown = None;
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::BlurAll => {
                self.new_tab_picker_open = false;
                // Reset sidebar focus state explicitly — the framework's on_blur
                // chain doesn't know about SidebarFocus enum values.
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    let mut changed = false;
                    if lp.sidebar_focus != SidebarFocus::None {
                        lp.sidebar_focus = SidebarFocus::None;
                        changed = true;
                    }
                    if lp.sidebar_menu_tab_id.is_some() {
                        lp.sidebar_menu_tab_id = None;
                        changed = true;
                    }
                    if changed {
                        lp_ctx.notify();
                    }
                });
                // Give focus to WorkspaceView itself.  The framework fires on_blur
                // on whoever was focused (InputEditor, ConsolePane, sidebar editor)
                // which stops cursor blinking and clears is_focused automatically.
                ctx.focus_self();
                ctx.notify();
            }
            WorkspaceAction::ToggleProjectDirectory { relative_path } => {
                let relative_path = relative_path.clone();
                let (was_expanded, was_loaded) =
                    self.project_sidebar_model.read(ctx, |model, _| {
                        (
                            model.is_directory_expanded(&relative_path),
                            model.is_directory_loaded(&relative_path),
                        )
                    });
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.explorer_selection = Some((relative_path.clone(), true));
                    model.toggle_directory(relative_path.clone(), model_ctx);
                });
                if !was_expanded && !was_loaded {
                    // An older whole-tree snapshot must not overwrite newly
                    // expanded children when it finishes after this request.
                    self.document_tree_refresh_epoch =
                        self.document_tree_refresh_epoch.wrapping_add(1);
                    let root = self.document_index_root.clone();
                    let sidebar_model = self.project_sidebar_model.clone();
                    let expected_root = root.clone();
                    ctx.spawn(
                        async move {
                            (
                                root.clone(),
                                super::project_documents::discover_directory(&root, &relative_path),
                                relative_path,
                            )
                        },
                        move |view, (scanned_root, index, directory), ctx| {
                            if view.document_index_root != expected_root
                                || scanned_root != expected_root
                            {
                                return;
                            }
                            view.document_index.merge(index.entries().to_vec());
                            ctx.update_model(&sidebar_model, |model, model_ctx| {
                                model.mark_directory_loaded(directory);
                                model.merge_documents(index, model_ctx);
                            });
                            ctx.notify();
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

/// How often the header's usage chip re-reads the agents' plan limits.
#[cfg(not(target_family = "wasm"))]
const AGENT_USAGE_POLL: std::time::Duration = std::time::Duration::from_secs(60);

impl WorkspaceView {
    /// Re-read every agent account's limits off the UI thread.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn refresh_agent_usage(&mut self, ctx: &mut ViewContext<Self>) {
        if self.agent_usage_loading {
            return;
        }
        self.agent_usage_loading = true;
        ctx.spawn(
            async { dragent_host::agent_usage::accounts() },
            |view, accounts, ctx| {
                view.agent_usage_loading = false;
                if view.agent_usage != accounts {
                    view.agent_usage = accounts;
                    ctx.notify();
                }
            },
        );
    }

    /// Read the repository's star count and the person's star off the UI
    /// thread, then redraw the places that show them.
    pub(crate) fn refresh_community(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(async { crate::community::refresh() }, |view, _, ctx| {
            view.community_changed(ctx);
        });
    }

    /// Star right here, like any in-app action: "Starring…", then
    /// "Starred on GitHub". It stars with the person's own `gh` sign-in;
    /// only when that can't be done (no `gh`, signed out) does it open the
    /// repository in the system browser. Already starred: nothing to do.
    pub(crate) fn star_on_github(&mut self, ctx: &mut ViewContext<Self>) {
        if crate::community::state().starred == Some(true)
            || !crate::community::begin_starring()
        {
            return;
        }
        self.community_changed(ctx);
        ctx.spawn(async { crate::community::star() }, |view, result, ctx| {
            if let Err(error) = result {
                log::warn!("starring on GitHub failed, opening the repository: {error}");
                ctx.open_url(crate::community::GITHUB_URL);
            }
            view.community_changed(ctx);
        });
    }

    fn community_changed(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.left_panel_view, |_, lp_ctx| lp_ctx.notify());
        ctx.update_view(&self.settings_view, |_, settings_ctx| settings_ctx.notify());
        ctx.notify();
    }

    /// Keep the chip current: read now, then every minute.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn poll_agent_usage(&mut self, ctx: &mut ViewContext<Self>) {
        self.refresh_agent_usage(ctx);
        ctx.spawn(
            warpui::r#async::Timer::after(AGENT_USAGE_POLL),
            |view, _, ctx| view.poll_agent_usage(ctx),
        );
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn refresh_agent_usage(&mut self, _: &mut ViewContext<Self>) {}

    #[cfg(target_family = "wasm")]
    pub(crate) fn poll_agent_usage(&mut self, _: &mut ViewContext<Self>) {}
}

impl WorkspaceView {
    /// The header's agent usage chip.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn usage_chip(
        &self,
        header_mouse_state: impl Fn(&'static str) -> MouseStateHandle,
    ) -> Option<Box<dyn Element>> {
        Some(super::header_usage::render_usage_button(
            self.ui_font,
            &self.theme,
            self.workspace_view_id,
            header_mouse_state("agent_usage"),
            super::header_usage::headline(&self.agent_usage),
            self.usage_menu_open,
        ))
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn usage_chip(
        &self,
        _: impl Fn(&'static str) -> MouseStateHandle,
    ) -> Option<Box<dyn Element>> {
        None
    }
}
