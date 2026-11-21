//! Workspace settings and services actions.
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
    pub(crate) fn handle_settings_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::OpenSettings => {
                self.open_settings(ctx);
            }
            WorkspaceAction::CloseSettings => {
                self.close_settings(ctx);
                #[cfg(not(target_family = "wasm"))]
                self.close_automations(ctx);
                #[cfg(not(target_family = "wasm"))]
                self.close_teams(ctx);
                #[cfg(not(target_family = "wasm"))]
                self.close_tasks(ctx);
                #[cfg(not(target_family = "wasm"))]
                self.close_pull_requests(ctx);
            }
            WorkspaceAction::SignOutFromHeader => {
                self.account_menu_open = false;
                self.close_org_menus();
                // Same confirmation as Settings → Account, and the same
                // "Don't ask again" preference.
                if crate::settings::AppSettings::skip_logout_confirm() {
                    let auth = AuthManager::handle(ctx);
                    ctx.update_model(&auth, |a, mctx| a.log_out(mctx));
                } else {
                    self.logout_dont_ask = false;
                    self.logout_confirm_open = true;
                }
                ctx.notify();
            }
            WorkspaceAction::ConfirmSignOut => {
                if self.logout_dont_ask {
                    crate::settings::AppSettings::set_skip_logout_confirm(true);
                }
                self.logout_confirm_open = false;
                let auth = AuthManager::handle(ctx);
                ctx.update_model(&auth, |a, mctx| a.log_out(mctx));
                ctx.notify();
            }
            WorkspaceAction::CancelSignOut => {
                self.logout_confirm_open = false;
                ctx.notify();
            }
            WorkspaceAction::ToggleLogoutDontAsk => {
                self.logout_dont_ask = !self.logout_dont_ask;
                ctx.notify();
            }
            WorkspaceAction::OpenDocumentation => {
                self.account_menu_open = false;
                self.close_org_menus();
                self.open_link("https://dragent.dev/docs", ctx);
                ctx.notify();
            }
            WorkspaceAction::OpenBillingFromHeader => {
                self.account_menu_open = false;
                // Plans, credits, and usage live in Settings → Billing & Usage.
                self.open_settings(ctx);
                let settings = self.settings_view.clone();
                ctx.update_view(&settings, |sv, sv_ctx| {
                    sv.show_section(crate::settings::SettingsSection::Billing, sv_ctx)
                });
                ctx.notify();
            }
            WorkspaceAction::UpdateAppFromHeader => {
                self.account_menu_open = false;
                self.close_org_menus();
                match crate::autoupdate::update_state() {
                    crate::autoupdate::UpdateState::Ready(version) => {
                        log::info!("[autoupdate] restarting for staged version {version}");
                        // The updater owns the full relaunch sequence. It
                        // starts a detached helper, waits for this process to
                        // exit, then opens the updated executable.
                        crate::autoupdate::restart_to_update();
                    }
                    crate::autoupdate::UpdateState::Available(_)
                    | crate::autoupdate::UpdateState::Unknown => {
                        crate::autoupdate::request_check();
                        ctx.notify();
                    }
                }
            }
            WorkspaceAction::SignInFromHeader => {
                // The prompt stays open showing the code to match in the
                // browser; it closes once the sign-in completes.
                self.login_prompt_open = true;
                let auth = AuthManager::handle(ctx);
                let url =
                    ctx.update_model(&auth, |auth, auth_ctx| auth.begin_browser_sign_in(auth_ctx));
                ctx.open_url(&url);
                ctx.notify();
            }
            WorkspaceAction::CloseLoginPrompt => {
                self.login_prompt_open = false;
                let auth = AuthManager::handle(ctx);
                ctx.update_model(&auth, |auth, _| auth.cancel_browser_sign_in());
                ctx.notify();
            }
            WorkspaceAction::RestartLanguageServices => {
                if let Some(root) = self.document_owner_root.clone().filter(|root| {
                    !self.workspace_trust.is_trusted(root)
                        && super::action_projects::workspace_trust_enforced()
                }) {
                    self.pending_workspace_trust = Some(root);
                    ctx.notify();
                    return;
                }
                self.language_services_enabled = true;
                self.language_sync_epoch = self.language_sync_epoch.wrapping_add(1);
                if self.ensure_language_service_client(ctx) {
                    if let Some(client) = self.language_services.as_ref() {
                        if let Err(error) = client.restart_all() {
                            log::warn!("[lsp] unable to restart language servers: {error}");
                        }
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::StopLanguageServices => {
                self.language_services_enabled = false;
                self.language_sync_epoch = self.language_sync_epoch.wrapping_add(1);
                if let Some(previous) = self.language_document_sync.take() {
                    if let Some(client) = self.language_services.as_ref() {
                        let _ = client.close(previous.project_root, previous.relative_path);
                    }
                }
                self.language_services.take();
                self.language_file_rename_servers.clear();
                self.synchronize_language_document(ctx);
                ctx.notify();
            }
            WorkspaceAction::RestartLanguageServer { server_id } => {
                if let Some(root) = self.document_owner_root.clone().filter(|root| {
                    !self.workspace_trust.is_trusted(root)
                        && super::action_projects::workspace_trust_enforced()
                }) {
                    self.pending_workspace_trust = Some(root);
                } else {
                    self.language_services_enabled = true;
                    let already_running = self.language_service_status.iter().any(|service| {
                        service.server_id == *server_id
                            && matches!(
                                service.state,
                                super::language_services_tray::ServiceState::Ready
                                    | super::language_services_tray::ServiceState::Starting
                            )
                    });
                    let document =
                        self.active_language_document_metadata(ctx)
                            .and_then(|(root, path, _)| {
                                self.language_document_snapshot(root, path, ctx)
                            });
                    if self.ensure_language_service_client(ctx) {
                        if let Some(client) = self.language_services.as_ref() {
                            let result = if already_running {
                                client.restart_server(server_id.clone())
                            } else if let Some(document) = document {
                                client.start_server(document, server_id.clone())
                            } else {
                                return;
                            };
                            if let Err(error) = result {
                                log::warn!("[lsp] unable to start {server_id}: {error}");
                            }
                        }
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::StopLanguageServer { server_id } => {
                if let Some(client) = self.language_services.as_ref() {
                    if let Err(error) = client.stop_server(server_id.clone()) {
                        log::warn!("[lsp] unable to stop {server_id}: {error}");
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::InstallLanguageServer { server_id } => {
                if let Some(root) = self.document_owner_root.clone().filter(|root| {
                    !self.workspace_trust.is_trusted(root)
                        && super::action_projects::workspace_trust_enforced()
                }) {
                    self.pending_workspace_trust = Some(root);
                } else if let Some(root) = self.document_owner_root.clone() {
                    self.language_services_enabled = true;
                    if self.ensure_language_service_client(ctx) {
                        if let Some(client) = self.language_services.as_ref() {
                            if let Err(error) = client.install_server(root, server_id.clone()) {
                                log::warn!("[lsp] unable to install {server_id}: {error}");
                            }
                        }
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::SetProjectSidebarTab { tab } => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.set_sidebar_tab(*tab, view_ctx);
                });

                // This is an "open this sidebar destination" action, not only
                // a selection mutation. Footer Git controls remain available
                // after the user closes the right panel, so selecting Source
                // Control from there must explicitly reverse that close.
                let root = self
                    .project_sidebar_model
                    .read(ctx, |model, _| model.active_cwd.clone());
                if let Some(root) = root.filter(|root| {
                    !self.workspace_trust.is_trusted(root)
                        && super::action_projects::workspace_trust_enforced()
                }) {
                    self.pending_workspace_trust_document = None;
                    self.pending_workspace_trust = Some(root);
                    ctx.notify();
                    return;
                }
                self.right_panel_visible = true;
                self.right_panel_user_closed = false;
                if *tab == super::project_sidebar::SidebarTab::Review {
                    // Paint cached data instantly (no loading flash for
                    // already-seen roots), then revalidate in the background.
                    // A known branch mismatch skips the cache rather than
                    // painting another branch's PR.
                    let root = self.active_review_git_root();
                    let cached = root
                        .as_ref()
                        .and_then(|root| self.review_pr_cache.get(root).cloned());
                    let branch = self.review_ship.branch.clone();
                    if let Some(entry) =
                        cached.filter(|entry| branch.is_empty() || entry.branch == branch)
                    {
                        let sidebar = self.project_sidebar_view.clone();
                        ctx.update_view(&sidebar, |ps, ps_ctx| {
                            ps.review_pr.pr = entry.pr;
                            ps.review_pr.comments = entry.comments;
                            ps.review_pr.error = entry.error;
                            ps.review_pr.loading_pr = false;
                            ps.review_pr.loading_comments = false;
                            ps_ctx.notify();
                        });
                    }
                    self.refresh_review_pr_state(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::OpenCommandPalette => {
                if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
                    if let WorkspacePane::Terminal(handle) = &tab.pane {
                        ctx.update_view(handle, |view, view_ctx| {
                            view.handle_action(
                                &crate::terminal::view::actions::TerminalAction::WorkflowSearchOpen,
                                view_ctx,
                            );
                        });
                    }
                }
            }
            _ => {}
        }
    }
}
