//! Session, sharing, and settings methods for WorkspaceView.
//!
//! Responsible for:
//! - shared-session join and snapshot refresh
//! - settings screen navigation and editor settings application
//!
//! Not responsible for:
//! - realtime transport (realtime manager)
//! - settings UI content (settings_view)
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

use super::host_mirror::{HostMirror, MirrorMsg};
use super::project_naming::default_pane_cwd;
use super::view::PaneTab;
use super::view::WorkspaceView;
#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;

impl WorkspaceView {
    /// Re-read the shared-session snapshot from the `SessionManager` singleton.
    /// Open a mirror tab for a shared session and connect as a viewer.
    pub(crate) fn join_session(
        &mut self,
        session: String,
        secret: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.shared_session_ended = None;
        // Route the mirror pane's input (typed keys + alt-screen keystrokes)
        // through the SessionManager to the host. The server drops it unless
        // this viewer holds Control (Edit), so the sink can always forward.
        let (input_tx, input_rx) = async_channel::unbounded::<Vec<u8>>();
        let sink: crate::terminal::manager::pty_writer::RemoteInputSink =
            std::sync::Arc::new(move |bytes: &[u8]| {
                let _ = input_tx.try_send(bytes.to_vec());
            });
        let pane = super::pane::WorkspacePane::spawn_remote(ctx, default_pane_cwd(), sink);
        if let super::pane::WorkspacePane::Terminal(h) = &pane {
            self.mirror_pane = Some(h.clone());
        }
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.mirror_tab_id = Some(tab_id);
        pane.focus(ctx);
        self.active_pane_id = Some(pane.id());
        let surface_id = self.register_surface(&pane, ctx);
        self.tabs.push(PaneTab {
            id: tab_id,
            surface_id,
            title: "Shared session".to_string(),
            custom_label: Some("Shared".to_string()),
            color: None,
            work_item_id: Some(WorkItemId::new()),
            project_root: None,
            hidden_from_sidebar: false,
            pane,
        });
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.active_tab_id = tab_id;
        let sm = SessionManager::handle(ctx);
        ctx.update_model(&sm, |m, mctx| {
            if let Err(e) = m.start_viewer(mctx, &session, &secret, "Viewer") {
                log::warn!("[share] bad join credentials: {e}");
            }
            // Pump mirror input → host for the life of the session.
            mctx.spawn_stream_local(
                input_rx,
                |m, bytes: Vec<u8>, _mctx| m.send_input(&bytes),
                |_, _| {},
            );
        });
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// Display name + avatar URL for the header avatar, read from the auth
    /// session. Falls back to a blank name (the avatar helper renders an initial
    /// or a placeholder) rather than showing nothing.
    pub(crate) fn account_identity(&self, app: &AppContext) -> (String, String) {
        use crate::auth::AuthStatus;
        use warpui::SingletonEntity;
        let auth = AuthManager::handle(app);
        app.read_model(&auth, |a: &AuthManager, _| match a.status() {
            AuthStatus::LoggedIn(user) => {
                let name = [user.first_name.as_deref(), user.last_name.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                (name, user.avatar_url.clone().unwrap_or_default())
            }
            _ => (String::new(), String::new()),
        })
    }

    pub(crate) fn close_settings(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        #[cfg(not(target_family = "wasm"))]
        ctx.update_view(&self.settings_view, |view, _| {
            view.computer_permission_visible = false
        });
        self.settings_project_root = None;
        // The previous screen, selected tab and split layout stayed in place.
        // Return focus without selecting another pane or rebuilding its layout.
        if self.project_start_visible {
            if self.selected_project_root().is_some() && self.project_start_mode.is_none() {
                ctx.focus(&self.prompt_editor);
            } else {
                ctx.focus(&self.project_start_editor);
            }
        } else if self.active_builtin_layout() == BuiltInLayout::Shape {
            ctx.focus(&self.shape_composer_editor);
        } else if self.active_builtin_layout() != BuiltInLayout::Develop {
            ctx.focus_self();
        } else if self.editor_pane_focused {
            let editor = self
                .project_sidebar_view
                .read(ctx, |view, _| view.document_editor.clone());
            ctx.update_view(&editor, |editor, editor_ctx| {
                editor.request_focus(editor_ctx)
            });
        } else if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| Some(tab.pane.id()) == self.active_pane_id)
        {
            tab.pane.focus(ctx);
        }
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    pub(crate) fn open_settings(&mut self, ctx: &mut ViewContext<Self>) {
        #[cfg(not(target_family = "wasm"))]
        self.close_pull_requests(ctx);
        if !self.settings_open {
            self.settings_project_root = self.active_project_root();
        }
        self.settings_open = true;
        #[cfg(not(target_family = "wasm"))]
        ctx.update_view(&self.settings_view, |view, ctx| {
            if view.section == crate::settings::SettingsSection::ComputerUse {
                view.refresh_computer_permissions(ctx);
            }
        });
        self.automations_open = false;
        #[cfg(not(target_family = "wasm"))]
        self.close_teams(ctx);
        #[cfg(not(target_family = "wasm"))]
        self.close_tasks(ctx);
        self.new_tab_picker_open = false;
        self.close_org_menus();
        self.prompt_menu = None;
        self.sync_left_panel(ctx);
        // Fetch fresh account/usage the moment settings opens (the view is
        // created once at startup, so its new() fetch won't re-run here).
        let settings = self.settings_view.clone();
        // Pages about "this project" (Skills) follow the project in use, not
        // the one the window opened with.
        let project_root = self.settings_project_root.clone();
        ctx.update_view(&settings, |sv, sv_ctx| {
            if let Some(root) = project_root {
                sv.project_root = root;
            }
            sv.on_opened(sv_ctx)
        });
        ctx.focus(&settings);
        self.save_session(ctx);
        ctx.notify();
    }

    pub(crate) fn apply_editor_settings_all(&mut self, ctx: &mut ViewContext<Self>) {
        let settings = crate::console::editor_settings::EditorSettings::global();
        crate::theme::set_terminal_typography(
            settings.terminal.clamped_font_size(),
            settings.terminal.line_height_ratio(),
        );
        for tab in &self.tabs {
            if let WorkspacePane::Terminal(terminal) = &tab.pane {
                ctx.update_view(terminal, |tv, tctx| tv.sync_editor_settings(tctx));
            }
        }
        for tab in &self.document_terminals {
            if let WorkspacePane::Terminal(terminal) = &tab.pane {
                ctx.update_view(terminal, |tv, tctx| tv.sync_editor_settings(tctx));
            }
        }
        let font_cache = ctx.get_singleton_model_handle::<FontCache>();
        let mono_font = ctx.update_model(&font_cache, |cache, _| {
            crate::workspace::prewarm::load_mono_font(cache)
        });
        self.mono_font = mono_font;
        let left_panel = self.left_panel_view.clone();
        ctx.update_view(&left_panel, |panel, panel_ctx| {
            panel.ui_font = self.ui_font;
            for editor in [&panel.sidebar_search_editor, &panel.sidebar_rename_editor] {
                panel_ctx.update_view(editor, |input, input_ctx| {
                    let size = input.font_size();
                    input.set_font(self.ui_font, size, input_ctx);
                    input.set_menu_font(self.ui_font, input_ctx);
                });
            }
            panel_ctx.notify();
        });
        for editor in [
            &self.shape_composer_editor,
            &self.shape_record_editor,
            &self.project_start_editor,
            &self.project_group_name_editor,
            &self.worktree_branch_editor,
            &self.prompt_editor,
            &self.prompt_branch_search_editor,
            &self.prompt_agent_search_editor,
            &self.clone_url_editor,
            &self.clone_location_editor,
            &self.create_project_name_editor,
            &self.create_project_location_editor,
            &self.explorer_prompt_editor,
            &self.share_email_editor,
            &self.ports_editor,
            &self.attention_response_editor,
        ] {
            ctx.update_view(editor, |input, input_ctx| {
                let size = input.font_size();
                input.set_font(mono_font, size, input_ctx);
                input.set_menu_font(self.ui_font, input_ctx);
            });
        }
        let project_sidebar = self.project_sidebar_view.clone();
        ctx.update_view(&project_sidebar, |view, view_ctx| {
            view.sync_editor_typography(self.ui_font, mono_font, view_ctx);
        });
    }
}

impl WorkspaceView {
    /// Bind a CONTROLLING mirror tab to one host-owned terminal session.
    /// Closes any previous mirror binding first (the host run keeps going
    /// headless; only the local pump stops).
    pub(crate) fn open_host_session_mirror(
        &mut self,
        session_id: String,
        title: String,
        project_root: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(tab_id) = self.host_mirror_tab_id {
            self.handle_action(&WorkspaceAction::CloseTabConfirmed { tab_id }, ctx);
        }
        // CONTROLLING sink: forward every input byte to the host's PTY.
        // Best-effort: resolve the host + workspace per keystroke off-thread
        // and drop failures silently (the poll pump owns error surfacing).
        let sink_session = session_id.clone();
        let sink_root = project_root.to_string_lossy().into_owned();
        let sink: crate::terminal::manager::pty_writer::RemoteInputSink =
            std::sync::Arc::new(move |bytes: &[u8]| {
                if bytes.is_empty() {
                    return;
                }
                let text = String::from_utf8_lossy(bytes).into_owned();
                let session_id = sink_session.clone();
                let root = sink_root.clone();
                crate::bg::detach(move || {
                    let Ok(client) = dragent_host::HostClient::ensure_host() else {
                        return;
                    };
                    let Ok(workspace_id) = client.create_workspace(&root) else {
                        return;
                    };
                    let _ = client.send_terminal_input(&workspace_id, &session_id, text.as_bytes());
                });
            });
        let pane = WorkspacePane::spawn_remote(ctx, project_root.clone(), sink);
        if let WorkspacePane::Terminal(handle) = &pane {
            self.host_mirror_pane = Some(handle.clone());
            // A controlling mirror drives input; it is never read-only.
            ctx.update_view(handle, |tv, tctx| {
                tv.set_remote_read_only(false, tctx);
            });
        }
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.host_mirror_tab_id = Some(tab_id);
        pane.focus(ctx);
        self.active_pane_id = Some(pane.id());
        let surface_id = self.register_surface(&pane, ctx);
        self.tabs.push(PaneTab {
            id: tab_id,
            surface_id,
            title,
            custom_label: Some("Agent".to_string()),
            color: None,
            work_item_id: Some(WorkItemId::new()),
            project_root: Some(project_root),
            hidden_from_sidebar: false,
            pane,
        });
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.active_tab_id = tab_id;
        self.host_mirror = Some(HostMirror::new(session_id));
        self.sync_left_panel(ctx);
        self.poll_host_mirror(ctx);
        ctx.notify();
    }

    /// One poll tick of the host-mirror pump. Reads from the cursor, delivers
    /// through [`TerminalView::feed_remote_output`], and reschedules itself on
    /// a 500ms timer while a mirror is bound and not ended.
    pub(crate) fn poll_host_mirror(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(mirror) = self.host_mirror.as_ref() else {
            return;
        };
        if mirror.ended {
            return;
        }
        let session_id = mirror.session_id.clone();
        let offset = mirror.next_offset;
        let Some(tab_id) = self.host_mirror_tab_id else {
            return;
        };
        let Some(root) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.project_root.clone())
        else {
            // Tab went away without clearing the binding; drop it.
            self.host_mirror_pane = None;
            self.host_mirror_tab_id = None;
            self.host_mirror = None;
            return;
        };
        let cached_client = self.mirror_client.clone();
        let root_str = root.to_string_lossy().into_owned();
        let (tx, rx) = async_channel::bounded(1);
        let bg_session_id = session_id.clone();
        crate::bg::detach(move || {
            let client = cached_client.or_else(|| dragent_host::HostClient::ensure_host().ok());
            let msg = match client
                .as_ref()
                .map(|client| client.create_workspace(&root_str))
            {
                Some(Ok(workspace_id)) => match client
                    .as_ref()
                    .expect("client exists")
                    .read_terminal_since(&workspace_id, &bg_session_id, offset, 32 * 1024)
                {
                    Ok((data, base, total)) => MirrorMsg::Chunk {
                        base_offset: base,
                        total_bytes: total,
                        data,
                        exit_code: None,
                    },
                    Err(_) => match client
                        .as_ref()
                        .expect("client exists")
                        .list_terminals(&workspace_id)
                    {
                        Ok(sessions)
                            if !sessions.iter().any(|s| {
                                s.id == bg_session_id
                                    && matches!(s.state, dragent_host::TerminalState::Running)
                            }) =>
                        {
                            MirrorMsg::Gone
                        }
                        _ => MirrorMsg::Unavailable,
                    },
                },
                _ => MirrorMsg::Unavailable,
            };
            let _ = tx.send_blocking((msg, client));
        });
        ctx.spawn_stream_local(
            rx,
            move |view, (msg, client): (MirrorMsg, Option<dragent_host::HostClient>), ctx| {
                // Stale guard: a newer mirror may have bound while we read.
                let stale = view
                    .host_mirror
                    .as_ref()
                    .is_none_or(|mirror| mirror.session_id != session_id);
                if stale {
                    return;
                }
                match msg {
                    MirrorMsg::Chunk {
                        base_offset,
                        total_bytes,
                        data,
                        ..
                    } => {
                        view.mirror_client = client;
                        let Some(mirror) = view.host_mirror.as_mut() else {
                            return;
                        };
                        let feed = mirror.apply_chunk(base_offset, total_bytes, &data);
                        if feed.gap {
                            log::warn!(
                                "[host-mirror] gap resync session={} base={} cursor fed {} bytes",
                                mirror.session_id,
                                base_offset,
                                feed.bytes.len()
                            );
                        }
                        if !feed.bytes.is_empty() {
                            if let Some(pane) = view.host_mirror_pane.clone() {
                                ctx.update_view(&pane, |tv, tctx| {
                                    tv.feed_remote_output(&feed.bytes, tctx)
                                });
                            }
                        }
                    }
                    MirrorMsg::Gone => {
                        // Drop the client so the next tick re-resolves it.
                        view.mirror_client = None;
                        if let Some(mirror) = view.host_mirror.as_mut() {
                            mirror.note_failure();
                        }
                    }
                    MirrorMsg::Unavailable => {
                        view.mirror_client = None;
                    }
                }
                let bound = view
                    .host_mirror_tab_id
                    .is_some_and(|tab_id| view.tabs.iter().any(|tab| tab.id == tab_id))
                    && view
                        .host_mirror
                        .as_ref()
                        .is_some_and(|mirror| !mirror.ended);
                if bound {
                    ctx.spawn(
                        Timer::after(Duration::from_millis(500)),
                        move |view, _, ctx| {
                            view.poll_host_mirror(ctx);
                        },
                    );
                }
                ctx.notify();
            },
            |_, _| {},
        );
    }

    /// Re-bind mirrors for every host agent run across open projects.
    /// Resolves workspace + runs + worktree paths off-thread, then binds one
    /// mirror per delivered run (each bind closes the previous binding, so
    /// the last run wins the visible tab).
    pub(crate) fn adopt_host_runs(&mut self, ctx: &mut ViewContext<Self>) {
        let roots = self.open_project_roots.clone();
        let (tx, rx) = async_channel::bounded::<Vec<(String, String, String)>>(1);
        crate::bg::detach(move || {
            let mut out = Vec::new();
            let Ok(client) = dragent_host::HostClient::ensure_host() else {
                let _ = tx.send_blocking(out);
                return;
            };
            for root in &roots {
                let root_str = root.to_string_lossy().into_owned();
                let Ok(workspace_id) = client.create_workspace(&root_str) else {
                    continue;
                };
                let runs = client.list_agent_runs(&workspace_id).unwrap_or_default();
                let homes = client.list_worktrees(&workspace_id).unwrap_or_default();
                for run in runs {
                    let path = homes
                        .iter()
                        .find(|home| home.id == run.worktree_id)
                        .map(|home| home.path.clone())
                        .unwrap_or_else(|| root_str.clone());
                    out.push((run.id, run.label, path));
                }
            }
            let _ = tx.send_blocking(out);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, runs: Vec<(String, String, String)>, ctx| {
                for (run_id, label, home_path) in runs {
                    view.open_host_session_mirror(run_id, label, PathBuf::from(home_path), ctx);
                }
            },
            |_, _| {},
        );
    }
}
