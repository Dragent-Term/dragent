//! Workspace sharing and sessions actions.
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
    pub(crate) fn handle_sharing_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::JoinSession { session, secret } => {
                self.join_session(session.clone(), secret.clone(), ctx);
            }
            WorkspaceAction::RequestSessionEdit => {
                let sm = SessionManager::handle(ctx);
                ctx.read_model(&sm, |m, _| m.request_control());
            }
            WorkspaceAction::OpenParticipantAccess { participant } => {
                self.participant_access_open = Some(participant.clone());
                ctx.notify();
            }
            WorkspaceAction::CloseParticipantAccess => {
                self.participant_access_open = None;
                ctx.notify();
            }
            WorkspaceAction::OpenExternalUrl { url } => {
                self.open_link(url, ctx);
            }
            WorkspaceAction::ShareSession { tab_id } => {
                // Dismiss the tab menu / content-header popover regardless of outcome.
                self.content_header_menu_open = false;
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                // Artifact sharing gate (Settings → Skills → Sharing).
                if !crate::settings::AppSettings::global()
                    .sharing
                    .artifact_sharing_enabled
                {
                    self.show_share_notice(
                        "Artifact sharing is disabled — enable it in Settings → Skills → Sharing.",
                        ctx,
                    );
                    return;
                }
                // Sharing requires an account — prompt sign-in when signed out.
                if !self.account_logged_in_cache {
                    self.login_prompt_open = true;
                    ctx.notify();
                    return;
                }
                // Ask what the participant should see first — everything already
                // in this terminal, or a clean screen. Sharing starts once chosen
                // (`ConfirmShareScope`), because the answer decides what is
                // broadcast and cannot be changed afterwards.
                self.share_scope_tab = Some(*tab_id);
                ctx.notify();
            }
            WorkspaceAction::CancelShareScope => {
                self.share_scope_tab = None;
                ctx.notify();
            }
            WorkspaceAction::OpenShareSheet => {
                self.content_header_menu_open = false;
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                if !crate::settings::AppSettings::global()
                    .sharing
                    .artifact_sharing_enabled
                {
                    self.show_share_notice(
                        "Artifact sharing is disabled — enable it in Settings → Skills → Sharing.",
                        ctx,
                    );
                    return;
                }
                self.share_open = true;
                self.share_qr_open = false;
                self.share_open_dropdown = None;
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::RequestStopSharing => {
                // Confirm before ending — closing the sheet must never end the
                // session, only this explicit confirmation does. Close the share
                // sheet first: it stacks above the confirmation, so leaving it
                // open would hide the "End session?" modal behind it.
                self.content_header_menu_open = false;
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                self.share_open = false;
                self.share_qr_open = false;
                self.share_open_dropdown = None;
                self.stop_confirm_open = true;
                ctx.notify();
            }
            WorkspaceAction::CancelStopSharing => {
                self.stop_confirm_open = false;
                ctx.notify();
            }
            WorkspaceAction::StopSharing => {
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| m.end(mctx));
                self.share_open = false;
                self.share_qr_open = false;
                self.share_open_dropdown = None;
                self.content_header_menu_open = false;
                self.stop_confirm_open = false;
                self.refresh_share_snapshot(ctx);
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::KeepSessionAlive => {
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| m.keep_session_alive(mctx));
                self.idle_warning_open = false;
                ctx.notify();
            }
            WorkspaceAction::EndIdleSession => {
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| {
                    m.end_with_reason(Some("Session ended due to inactivity".into()), mctx)
                });
                self.idle_warning_open = false;
                self.share_open = false;
                self.share_qr_open = false;
                self.content_header_menu_open = false;
                self.refresh_share_snapshot(ctx);
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::CloseShareSheet => {
                self.share_open = false;
                self.share_qr_open = false;
                self.share_open_dropdown = None;
                ctx.notify();
            }
            WorkspaceAction::CopyShareValue { value } => {
                if !value.is_empty() {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(value.clone()));
                    // Copying is instant and silent — confirm it, or the click
                    // gives no feedback at all.
                    self.show_share_notice("Link copied", ctx);
                }
            }
            WorkspaceAction::CopyPortForwardValue { port, value } => {
                if !value.is_empty() {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(value.clone()));
                    self.show_ports_notice(format!("Port {port} address copied"), ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::OpenPortForward { url } => {
                if !url.is_empty() {
                    self.open_link(url, ctx);
                }
            }
            WorkspaceAction::InviteEmail => {
                let email = self
                    .share_email_editor
                    .read(ctx, |e: &InputEditor, _| e.get_text().trim().to_string());
                if email.contains('@') {
                    let sm = SessionManager::handle(ctx);
                    ctx.update_model(&sm, |m, mctx| m.invite_email(mctx, email));
                    ctx.update_view(&self.share_email_editor, |e, e_ctx| e.set_text("", e_ctx));
                }
                self.share_open_dropdown = None;
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::CreatePortForward => {
                if !self.account_logged_in_cache {
                    self.login_prompt_open = true;
                    ctx.notify();
                    return;
                }
                let port = self.ports_editor.read(ctx, |e: &InputEditor, _| {
                    e.get_text().trim().parse::<u16>().ok()
                });
                if let Some(port) = port.filter(|p| *p != 0) {
                    let sm = SessionManager::handle(ctx);
                    ctx.update_model(&sm, |m, mctx| {
                        m.create_port_forward(mctx, port, format!("Port {port}"))
                    });
                    ctx.update_view(&self.ports_editor, |e, e_ctx| e.set_text("", e_ctx));
                } else {
                    self.show_ports_notice("Enter a valid port", ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::ClosePortForward { forward_id } => {
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| {
                    m.close_port_forward(mctx, forward_id.clone())
                });
                ctx.notify();
            }
            WorkspaceAction::SetInviteRole { control } => {
                let control = *control;
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| m.set_invite_role(mctx, control));
                self.share_open_dropdown = None;
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::SetLinkAccess { access } => {
                let level = match access {
                    super::action::LinkAccess::None => None,
                    super::action::LinkAccess::View => Some(AccessLevel::ViewOnly),
                    super::action::LinkAccess::Edit => Some(AccessLevel::Control),
                };
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| m.set_link_access(mctx, level));
                self.share_open_dropdown = None;
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::RemoveParticipant { participant } => {
                let sm = SessionManager::handle(ctx);
                let pid = participant.clone();
                ctx.read_model(&sm, |m, _| m.remove_participant(&pid));
                self.show_share_notice("Participant removed", ctx);
                self.share_open_dropdown = None;
                self.participant_access_open = None;
                ctx.notify();
            }
            WorkspaceAction::OpenShareQr => {
                // Encode the share URL into a QR bitmap and register it as a raw
                // image asset the QR sheet can render.
                self.share_qr_asset = None;
                if let Some(url) = self.share_snapshot.link_url.clone() {
                    if let Some(qr) = crate::realtime::qr::encode(&url) {
                        match warpui_core::image_cache::CustomImageHeader::prepend_custom_header(
                            qr.rgba,
                            qr.size,
                            qr.size,
                            warpui_core::image_cache::CustomImageFormat::Rgba,
                        ) {
                            Ok(bytes) => {
                                let cache =
                                    warpui_core::assets::asset_cache::AssetCache::handle(ctx);
                                let id = qr.id.clone();
                                ctx.update_model(&cache, |c, cctx| {
                                    c.insert_raw_asset_bytes::<warpui_core::image_cache::ImageType>(
                                        id, &bytes, cctx,
                                    )
                                });
                                self.share_qr_asset = Some(qr.id);
                            }
                            Err(e) => log::warn!("share QR encode failed: {e:?}"),
                        }
                    }
                }
                self.share_qr_open = true;
                self.share_open_dropdown = None;
                ctx.notify();
            }
            WorkspaceAction::CloseShareQr => {
                self.share_qr_open = false;
                ctx.notify();
            }
            WorkspaceAction::ConfirmShareScope {
                tab_id,
                include_scrollback,
            } => {
                // Backend enforcement: the actual start_host call never runs
                // while artifact sharing is off, even if the scope dialog was
                // reached before the toggle flipped.
                if !crate::settings::AppSettings::global()
                    .sharing
                    .artifact_sharing_enabled
                {
                    self.share_scope_tab = None;
                    self.show_share_notice(
                        "Artifact sharing is disabled — enable it in Settings → Skills → Sharing.",
                        ctx,
                    );
                    return;
                }
                self.share_scope_tab = None;
                let tab = self.tabs.iter().find(|t| t.id == *tab_id);
                let title = tab
                    .map(|t| t.display_label().to_string())
                    .unwrap_or_else(|| "Terminal".to_string());
                let pane = tab.map(|t| t.pane.pane_uuid(ctx));
                let Some(pane) = pane else {
                    return;
                };
                // Blocks that already exist, captured BEFORE the session starts.
                // On a clean share these are marked already-sent so the replay
                // skips them; the participant then only ever sees new output.
                let existing: Vec<usize> = match tab.map(|t| &t.pane) {
                    Some(super::pane::WorkspacePane::Terminal(h)) => {
                        h.read(ctx, |tv, tctx| tv.current_block_ids(tctx))
                    }
                    _ => Vec::new(),
                };
                let include = *include_scrollback;
                let sm = SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| {
                    m.start_host(mctx, pane, title);
                    if !include {
                        m.suppress_scrollback(existing);
                    }
                });
                self.share_open = true;
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::SetParticipantAccess {
                participant,
                control,
            } => {
                if let Ok(uuid) = uuid::Uuid::parse_str(participant) {
                    let access = if *control {
                        AccessLevel::Control
                    } else {
                        AccessLevel::ViewOnly
                    };
                    let sm = SessionManager::handle(ctx);
                    ctx.update_model(&sm, |m, _| {
                        m.set_participant_role(ParticipantId(uuid), access)
                    });
                    // Revoking control: the viewer stops driving the PTY size, so
                    // restore the host pane's own window size.
                    if !*control {
                        for tab in &self.tabs {
                            if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                                let h = h.clone();
                                ctx.update_view(&h, |tv, tctx| tv.clear_remote_resize(tctx));
                            }
                        }
                    }
                }
                self.share_open_dropdown = None;
                self.participant_access_open = None;
                // Sent over the session socket, so it lands as soon as it is
                // dispatched — confirm it rather than leaving the menu to close
                // with no indication anything happened.
                self.show_share_notice(
                    if *control {
                        "Edit access granted"
                    } else {
                        "Changed to view only"
                    },
                    ctx,
                );
                self.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            WorkspaceAction::ResolveAccessRequest {
                request_id,
                approve,
            } => {
                let sm = SessionManager::handle(ctx);
                let pid = request_id.clone();
                let approve = *approve;
                ctx.read_model(&sm, |m, _| m.resolve_access_request(&pid, approve));
                self.pending_access_request = None;
                ctx.notify();
            }
            _ => {}
        }
    }
}
