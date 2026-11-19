//! Language-service driver methods for WorkspaceView.
//!
//! Responsible for:
//! - language server client lifecycle per project root
//! - document sync (open/change/save/close) with servers
//! - completion, hover, navigation, formatting, code actions, rename
//! - applying language responses and draining service events
//!
//! Not responsible for:
//! - language parsing helpers (language_nav)
//! - server process management (dragent_language_services crate)
//! - rendering editor UI (view_render)
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
use super::view::{LanguageDocumentSyncState, LanguageRequestPurpose};

impl WorkspaceView {
    pub(crate) fn ensure_language_service_client(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        let workspace_is_trusted = self
            .document_owner_root
            .as_deref()
            .is_some_and(|root| self.workspace_trust.permits(root, super::action_projects::workspace_trust_enforced()));
        if !workspace_is_trusted {
            log::info!("[lsp] startup deferred until the workspace is trusted");
            return false;
        }
        if self.language_services.is_none() {
            let config = LanguageServiceClientConfig {
                // Trust is the security boundary for managed downloads. Once a
                // workspace is trusted, acquire each applicable catalogued
                // server automatically on every supported platform.
                allow_managed_downloads: true,
                catalog: self.language_server_catalog.clone(),
                ..LanguageServiceClientConfig::default()
            };
            match LanguageServiceClient::start(config) {
                Ok(client) => {
                    log::info!("[lsp] trusted background coordinator started");
                    self.language_event_subscription_epoch =
                        self.language_event_subscription_epoch.wrapping_add(1);
                    let epoch = self.language_event_subscription_epoch;
                    let wakeups = client.event_wakeups();
                    ctx.spawn_stream_local(
                        wakeups,
                        move |view, _unit: (), ctx| {
                            if view.language_event_subscription_epoch == epoch {
                                view.drain_language_service_events(ctx);
                            }
                        },
                        |_, _| {},
                    );
                    self.language_services = Some(client);
                }
                Err(error) => {
                    log::warn!("[lsp] unable to start language-service coordinator: {error}");
                    return false;
                }
            }
        }
        true
    }

    pub(crate) fn synchronize_language_document(&mut self, ctx: &mut ViewContext<Self>) {
        let metadata = self.active_language_document_metadata(ctx);
        if let Some((_, relative_path, _)) = metadata.as_ref() {
            let language_server_allowed = self
                .active_document_capabilities(relative_path, ctx)
                .is_none_or(|capabilities| capabilities.language_server);
            if !language_server_allowed {
                // Keep the buffer fully editable, but do not send a large
                // document to a language server that may reparse/synchronize
                // the complete text on every change.
                if let Some(previous) = self.language_document_sync.take() {
                    if let Some(client) = self.language_services.as_ref() {
                        let _ = client.close(previous.project_root, previous.relative_path);
                    }
                }
                self.language_service_status.clear();
                self.refresh_editor_navigation_capabilities(ctx);
                return;
            }
        }
        if !self.language_services_enabled {
            self.language_service_status.clear();
            if let Some((project_root, relative_path, _)) = metadata.as_ref() {
                if let Some(document) = self.language_document_snapshot(
                    project_root.clone(),
                    relative_path.clone(),
                    ctx,
                ) {
                    let language = dragent_editor::Language::from_path(relative_path);
                    for server in self.language_server_catalog.servers_for_language(language) {
                        self.language_service_status.push(
                            super::language_services_tray::ServiceSnapshot {
                                server_id: server.id.to_string(),
                                label: server.display_name.to_string(),
                                detail: "Available".to_string(),
                                state: super::language_services_tray::ServiceState::Available,
                                installable: server.install.is_some(),
                            },
                        );
                    }
                }
            }
            return;
        }
        let identity_changed = match (&self.language_document_sync, &metadata) {
            (Some(current), Some((project_root, relative_path, _))) => {
                current.project_root != *project_root || current.relative_path != *relative_path
            }
            (Some(_), None) | (None, Some(_)) => true,
            (None, None) => false,
        };

        if identity_changed {
            // Switching tabs is not `didClose`: all editor tabs remain open
            // buffers and must be reattached after a server restart, as in other editors.
            self.language_document_sync.take();
        }

        let Some((project_root, relative_path, revision)) = metadata else {
            self.language_service_status.clear();
            return;
        };

        if self.language_document_sync.is_none() {
            let Some(document) =
                self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
            else {
                return;
            };
            let language = dragent_editor::Language::from_path(&relative_path);
            let trusted = self.workspace_trust.permits(&project_root, super::action_projects::workspace_trust_enforced());
            // Catalog entries are controls, not lifecycle state. The worker is
            // the single source of truth and will emit Starting only after a
            // process exists and initialize has actually been sent.
            self.language_service_status = self
                .language_server_catalog
                .servers_for_language(language)
                .into_iter()
                .map(|server| super::language_services_tray::ServiceSnapshot {
                    server_id: server.id.to_string(),
                    label: server.display_name.to_string(),
                    detail: if !trusted {
                        "Trust required"
                    } else {
                        "Available"
                    }
                    .to_string(),
                    state: super::language_services_tray::ServiceState::Available,
                    installable: server.install.is_some(),
                })
                .collect();
            // Render the catalog immediately, even while execution is blocked
            // by trust. This keeps the editor control discoverable and makes
            // the reason for the inactive server explicit.
            ctx.notify();
            if !self.ensure_language_service_client(ctx) {
                return;
            }
            let Some(client) = self.language_services.as_ref() else {
                return;
            };
            match client.open(document.clone()) {
                Ok(()) => {
                    log::debug!(
                        "[lsp] queued open root={} path={} revision={}",
                        document.project_root.display(),
                        document.relative_path.display(),
                        revision
                    );
                    self.language_document_sync = Some(LanguageDocumentSyncState {
                        project_root: document.project_root,
                        relative_path: document.relative_path,
                        sent_revision: revision,
                    });
                    self.refresh_editor_navigation_capabilities(ctx);
                }
                Err(error) => log::warn!("[lsp] unable to queue document open: {error}"),
            }
            return;
        }

        if self
            .language_document_sync
            .as_ref()
            .is_some_and(|state| state.sent_revision != revision)
        {
            self.language_sync_epoch = self.language_sync_epoch.wrapping_add(1);
            let epoch = self.language_sync_epoch;
            ctx.spawn(
                Timer::after(Duration::from_millis(75)),
                move |view, _, ctx| {
                    if view.language_sync_epoch == epoch {
                        view.flush_language_document_change(ctx);
                    }
                },
            );
        }
    }

    pub(crate) fn flush_language_document_change(&mut self, ctx: &mut ViewContext<Self>) {
        let Some((project_root, relative_path, revision)) =
            self.active_language_document_metadata(ctx)
        else {
            return;
        };
        let Some(state) = self.language_document_sync.as_ref() else {
            return;
        };
        if state.project_root != project_root
            || state.relative_path != relative_path
            || state.sent_revision == revision
        {
            return;
        }
        let Some(document) = self.language_document_snapshot(project_root, relative_path, ctx)
        else {
            return;
        };
        let Some(client) = self.language_services.as_ref() else {
            return;
        };
        match client.change(document) {
            Ok(()) => {
                if let Some(state) = self.language_document_sync.as_mut() {
                    state.sent_revision = revision;
                }
            }
            Err(error) => {
                log::warn!("[lsp] unable to queue document change: {error}");
                self.language_sync_epoch = self.language_sync_epoch.wrapping_add(1);
                let epoch = self.language_sync_epoch;
                ctx.spawn(
                    Timer::after(Duration::from_millis(150)),
                    move |view, _, ctx| {
                        if view.language_sync_epoch == epoch {
                            view.flush_language_document_change(ctx);
                        }
                    },
                );
            }
        }
    }

    pub(crate) fn notify_language_document_saved(&mut self, ctx: &mut ViewContext<Self>) {
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        if self
            .active_document_capabilities(&relative_path, ctx)
            .is_some_and(|capabilities| !capabilities.language_server)
        {
            return;
        }
        let Some(document) = self.language_document_snapshot(project_root, relative_path, ctx)
        else {
            return;
        };
        if let Some(client) = self.language_services.as_ref() {
            if let Err(error) = client.save(document) {
                log::warn!("[lsp] unable to queue document save: {error}");
            }
        }
    }

    pub(crate) fn schedule_language_completion(
        &mut self,
        cursor: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        self.language_completion_epoch = self.language_completion_epoch.wrapping_add(1);
        let epoch = self.language_completion_epoch;
        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
            view.set_document_completion(None, view_ctx);
        });
        ctx.spawn(
            // Completion is a background convenience, not part of the typing
            // critical path. A slightly longer debounce coalesces normal
            // typing bursts into one document sync/request instead of asking
            // the language server after each brief pause.
            Timer::after(Duration::from_millis(200)),
            move |view, _, ctx| {
                if view.language_completion_epoch == epoch {
                    view.request_language_completion(cursor, ctx);
                }
            },
        );
    }

    pub(crate) fn request_language_completion(
        &mut self,
        cursor: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        if self
            .active_document_capabilities(&relative_path, ctx)
            .is_some_and(|capabilities| !capabilities.language_server)
        {
            return;
        }
        let Some(document) =
            self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
        else {
            return;
        };
        let position = char_index_to_lsp_position(&document.text, cursor);
        let Some(client) = self.language_services.as_ref() else {
            return;
        };
        match client.request(
            project_root,
            relative_path,
            LanguageRequest {
                position,
                kind: LanguageRequestKind::Completion,
            },
        ) {
            Ok(token) => self.language_completion_token = Some(token),
            Err(error) => log::debug!("[lsp] completion request skipped: {error}"),
        }
    }

    pub(crate) fn request_language_navigation(
        &mut self,
        cursor: usize,
        kind: super::LanguageNavigationKind,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
            view.clear_language_results(view_ctx);
        });
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        if self
            .active_document_capabilities(&relative_path, ctx)
            .is_some_and(|capabilities| !capabilities.language_server)
        {
            return;
        }
        let Some(document) =
            self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
        else {
            return;
        };
        if !self.ensure_language_service_client(ctx) {
            return;
        }
        let request_kind = match kind {
            super::LanguageNavigationKind::Definition => LanguageRequestKind::Definition,
            super::LanguageNavigationKind::TypeDefinition => LanguageRequestKind::TypeDefinition,
            super::LanguageNavigationKind::Implementation => LanguageRequestKind::Implementation,
            super::LanguageNavigationKind::References => LanguageRequestKind::References {
                include_declaration: true,
            },
        };
        let position = char_index_to_lsp_position(&document.text, cursor);
        let Some(client) = self.language_services.as_ref() else {
            return;
        };
        let request_path = relative_path.display().to_string();
        match client.request(
            project_root,
            relative_path,
            LanguageRequest {
                position,
                kind: request_kind,
            },
        ) {
            Ok(token) => {
                log::info!(
                    "[references] navigation request sent kind={kind:?} path={request_path} cursor={cursor}"
                );
                self.language_request_tokens
                    .insert(token, LanguageRequestPurpose::Navigation(kind));
            }
            Err(error) => log::warn!("[lsp] navigation request skipped: {error}"),
        }
    }

    pub(crate) fn request_definition_hover(
        &mut self,
        request_id: u64,
        cursor: usize,
        symbol_range: std::ops::Range<usize>,
        string_link: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.definition_hover_latest_request_id = Some(request_id);
        self.definition_hover_resolution = None;
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        if self
            .active_document_capabilities(&relative_path, ctx)
            .is_some_and(|capabilities| !capabilities.language_server)
        {
            return;
        }
        let Some(document) =
            self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
        else {
            return;
        };
        if !self.ensure_language_service_client(ctx) {
            return;
        }
        let position = char_index_to_lsp_position(&document.text, cursor);
        let Some(client) = self.language_services.as_ref() else {
            log::info!("[definition-link] no language client, hover request skipped");
            return;
        };
        match client.request(
            project_root,
            relative_path.clone(),
            LanguageRequest {
                position,
                kind: LanguageRequestKind::Definition,
            },
        ) {
            Ok(token) => {
                log::info!(
                    "[definition-link] lsp definition request sent id={} path={} cursor={}",
                    request_id,
                    relative_path.display(),
                    cursor
                );
                self.language_request_tokens.insert(
                    token,
                    LanguageRequestPurpose::DefinitionHover {
                        request_id,
                        source_relative_path: relative_path,
                        symbol_range,
                        string_link,
                    },
                );
            }
            Err(error) => log::debug!("[lsp] definition hover skipped: {error}"),
        }
    }

    pub(crate) fn refresh_editor_navigation_capabilities(&self, ctx: &mut ViewContext<Self>) {
        let capabilities = self
            .language_document_sync
            .as_ref()
            .map(|sync| {
                self.language_navigation_capabilities
                    .iter()
                    .filter(|(key, _)| key.project_root == sync.project_root)
                    .fold(
                        LanguageNavigationCapabilities::default(),
                        |mut result, (_, value)| {
                            result.definition |= value.definition;
                            result.type_definition |= value.type_definition;
                            result.implementation |= value.implementation;
                            result.references |= value.references;
                            result
                        },
                    )
            })
            .unwrap_or_default();
        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
            view.set_language_navigation_capabilities(capabilities, view_ctx);
        });
    }

    pub(crate) fn request_language_formatting(&mut self, ctx: &mut ViewContext<Self>) {
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        if self
            .active_document_capabilities(&relative_path, ctx)
            .is_some_and(|capabilities| !capabilities.language_server)
        {
            return;
        }
        let Some(document) =
            self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
        else {
            return;
        };
        if !self.ensure_language_service_client(ctx) {
            return;
        }
        let Some(client) = self.language_services.as_ref() else {
            return;
        };
        match client.request(
            project_root,
            relative_path,
            LanguageRequest {
                position: TextPosition::default(),
                kind: {
                    let prefs = crate::console::editor_settings::EditorSettings::global();
                    LanguageRequestKind::Formatting {
                        tab_size: prefs.editor.clamped_tab_size() as u32,
                        insert_spaces: prefs.editor.insert_spaces,
                    }
                },
            },
        ) {
            Ok(token) => {
                self.language_request_tokens
                    .insert(token, LanguageRequestPurpose::Formatting);
            }
            Err(error) => log::warn!("[lsp] formatting request skipped: {error}"),
        }
    }

    pub(crate) fn request_language_code_actions(
        &mut self,
        cursor: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        if self
            .active_document_capabilities(&relative_path, ctx)
            .is_some_and(|capabilities| !capabilities.language_server)
        {
            return;
        }
        let Some(document) =
            self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
        else {
            return;
        };
        if !self.ensure_language_service_client(ctx) {
            return;
        }
        let position = char_index_to_lsp_position(&document.text, cursor);
        let Some(client) = self.language_services.as_ref() else {
            return;
        };
        match client.request(
            project_root,
            relative_path,
            LanguageRequest {
                position,
                kind: LanguageRequestKind::CodeActions {
                    range: TextRange {
                        start: position,
                        end: position,
                    },
                },
            },
        ) {
            Ok(token) => {
                self.language_request_tokens
                    .insert(token, LanguageRequestPurpose::CodeActions);
            }
            Err(error) => log::warn!("[lsp] code-action request skipped: {error}"),
        }
    }

    pub(crate) fn request_language_rename(
        &mut self,
        cursor: usize,
        new_name: String,
        ctx: &mut ViewContext<Self>,
    ) {
        if new_name.is_empty() {
            return;
        }
        self.flush_language_document_change(ctx);
        let Some((project_root, relative_path, _)) = self.active_language_document_metadata(ctx)
        else {
            return;
        };
        let Some(document) =
            self.language_document_snapshot(project_root.clone(), relative_path.clone(), ctx)
        else {
            return;
        };
        if !self.ensure_language_service_client(ctx) {
            return;
        }
        let position = char_index_to_lsp_position(&document.text, cursor);
        let Some(client) = self.language_services.as_ref() else {
            return;
        };
        match client.request(
            project_root,
            relative_path,
            LanguageRequest {
                position,
                kind: LanguageRequestKind::Rename { new_name },
            },
        ) {
            Ok(token) => {
                self.language_request_tokens
                    .insert(token, LanguageRequestPurpose::Rename);
            }
            Err(error) => log::warn!("[lsp] rename request skipped: {error}"),
        }
    }

    pub(crate) fn apply_language_navigation(
        &mut self,
        root: &Path,
        kind: super::LanguageNavigationKind,
        locations: Vec<LanguageLocation>,
        ctx: &mut ViewContext<Self>,
    ) {
        log::info!(
            "[references] applying navigation kind={:?} locations={}",
            kind,
            locations.len()
        );
        if locations.is_empty() {
            ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                model.set_document_notice(
                    false,
                    language_navigation_empty_message(kind),
                    model_ctx,
                );
            });
            return;
        }
        // References are inherently a result set, even when there is only one
        // match. Definitions and implementations navigate directly only when
        // the server gives us one unambiguous target; otherwise the user gets
        // a picker-like overlay instead of an arbitrary jump to result zero.
        if kind == super::LanguageNavigationKind::References || locations.len() > 1 {
            // Load the line preview for each reference so the peek panel can
            // show real code context (VS Code-style rows). The active buffer
            // uses the live working copy; other files are read from disk.
            let mut file_texts: HashMap<PathBuf, Option<String>> = HashMap::new();
            for location in &locations {
                if file_texts.contains_key(&location.relative_path) {
                    continue;
                }
                let text = if self.language_document_sync.as_ref().is_some_and(|sync| {
                    sync.project_root == root && sync.relative_path == location.relative_path
                }) {
                    self.language_document_snapshot(
                        root.to_path_buf(),
                        location.relative_path.clone(),
                        ctx,
                    )
                    .map(|document| document.text)
                } else {
                    std::fs::read_to_string(root.join(&location.relative_path))
                        .ok()
                        .filter(|text| text.len() <= 1_000_000)
                };
                file_texts.insert(location.relative_path.clone(), text);
            }
            let items = locations
                .iter()
                .map(|location| {
                    let (line_text, match_range) = file_texts
                        .get(&location.relative_path)
                        .cloned()
                        .flatten()
                        .and_then(|text| reference_line_preview(&text, location.range))
                        .map_or((None, None), |(line, start, end)| {
                            (Some(line), Some((start, end)))
                        });
                    super::project_sidebar::LanguageLocationItem {
                        relative_path: location.relative_path.clone(),
                        range: location.range,
                        line_text,
                        match_range,
                    }
                })
                .collect();
            ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                view.set_language_locations(
                    root.to_path_buf(),
                    language_navigation_title(kind),
                    items,
                    view_ctx,
                );
            });
            return;
        }
        let Some(location) = locations.into_iter().next() else {
            log::debug!("[lsp] navigation returned no locations");
            return;
        };
        let active = self.language_document_sync.as_ref().is_some_and(|sync| {
            sync.project_root == root && sync.relative_path == location.relative_path
        });
        log::info!(
            "[navigation] target={} range={:?} same_file={}",
            location.relative_path.display(),
            location.range,
            active
        );
        if active {
            ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                view.navigate_active_document_to_range(
                    &location.relative_path,
                    location.range,
                    view_ctx,
                );
            });
        } else {
            self.pending_language_navigation =
                Some((location.relative_path.clone(), location.range));
            self.open_document_path_at_root(root.to_path_buf(), location.relative_path, false, ctx);
        }
    }

    pub(crate) fn apply_language_text_edits(
        &mut self,
        root: &Path,
        edits: Vec<LanguageTextEdit>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(active_path) = self
            .language_document_sync
            .as_ref()
            .filter(|sync| sync.project_root == root)
            .map(|sync| sync.relative_path.clone())
        else {
            return;
        };
        let mut edits_by_path: HashMap<PathBuf, Vec<LanguageTextEdit>> = HashMap::new();
        for edit in edits {
            edits_by_path
                .entry(edit.relative_path.clone())
                .or_default()
                .push(edit);
        }
        // WorkspaceEdit may include files that are not mounted in the single
        // visible document editor. Apply those files through the project
        // filesystem, while the active buffer continues through the normal
        // editor transaction path below. This keeps semantic rename/actions
        // consistent across the workspace instead of silently truncating them
        // to the current tab.
        let other_paths: Vec<_> = edits_by_path
            .keys()
            .filter(|path| *path != &active_path)
            .cloned()
            .collect();
        for path in other_paths {
            let Some(path_edits) = edits_by_path.remove(&path) else {
                continue;
            };
            let absolute_path = root.join(&path);
            let Ok(text) = std::fs::read_to_string(&absolute_path) else {
                log::warn!(
                    "[lsp] could not read workspace-edit target={}",
                    absolute_path.display()
                );
                continue;
            };
            let Some((updated, _)) = apply_text_edits(&text, 0, &path_edits) else {
                continue;
            };
            if let Err(error) = std::fs::write(&absolute_path, updated) {
                log::warn!(
                    "[lsp] could not write workspace-edit target={} error={error}",
                    absolute_path.display()
                );
            }
        }
        let Some(local_edits) = edits_by_path.remove(&active_path) else {
            return;
        };
        let Some((text, cursor)) = self
            .project_sidebar_view
            .read(ctx, |view, app| view.document_text_and_cursor(app))
        else {
            return;
        };
        let Some((updated, updated_cursor)) = apply_text_edits(&text, cursor, &local_edits) else {
            return;
        };
        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
            view.replace_document_editor_text_with_undo(&updated, updated_cursor, view_ctx);
        });
    }

    pub(crate) fn handle_language_response(
        &mut self,
        purpose: LanguageRequestPurpose,
        key: &LanguageServerKey,
        relative_path: &Path,
        result: Result<serde_json::Value, String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                log::warn!("[lsp] language request failed: {error}");
                if let LanguageRequestPurpose::FileRename {
                    source,
                    destination,
                } = &purpose
                {
                    // Import updating is an enhancement, not permission to
                    // rename. If a server disappears or rejects the request,
                    // preserve the normal explorer operation.
                    self.perform_explorer_rename(source.clone(), destination.clone(), true, ctx);
                } else if let LanguageRequestPurpose::Navigation(kind) = &purpose {
                    ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                        model.set_document_notice(
                            false,
                            format!("{}: {error}", language_navigation_title(*kind)),
                            model_ctx,
                        );
                    });
                } else if let LanguageRequestPurpose::DefinitionHover { request_id, .. } = &purpose
                {
                    if self.definition_hover_latest_request_id == Some(*request_id) {
                        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                            view.set_definition_hover_result(*request_id, false, view_ctx);
                        });
                    }
                }
                return;
            }
        };
        match purpose {
            LanguageRequestPurpose::Navigation(kind) => {
                let locations = parse_language_locations(&key.project_root, &value);
                self.apply_language_navigation(&key.project_root, kind, locations, ctx);
            }
            LanguageRequestPurpose::DefinitionHover {
                request_id,
                source_relative_path,
                symbol_range,
                string_link,
            } => {
                if self.definition_hover_latest_request_id != Some(request_id) {
                    log::info!(
                        "[definition-link] discarding stale lsp response id={} (latest={:?})",
                        request_id,
                        self.definition_hover_latest_request_id
                    );
                    return;
                }
                let active = self.language_document_sync.as_ref().is_some_and(|sync| {
                    sync.project_root == key.project_root
                        && sync.relative_path == source_relative_path
                        && relative_path == source_relative_path
                });
                let mut locations = active
                    .then(|| parse_language_locations(&key.project_root, &value))
                    .unwrap_or_default();
                // Import specifiers: when the language server has no
                // definition for the string position, resolve the path
                // against the source file's directory so `"./a/b/c"` still
                // becomes one link to that file.
                if locations.is_empty() {
                    if let Some(specifier) = &string_link {
                        if let Some(path) = resolve_import_specifier(
                            &key.project_root,
                            &source_relative_path,
                            specifier,
                        ) {
                            log::info!(
                                "[definition-link] specifier {:?} resolved to {}",
                                specifier,
                                path.display()
                            );
                            locations.push(LanguageLocation {
                                relative_path: path,
                                range: TextRange {
                                    start: TextPosition::default(),
                                    end: TextPosition::default(),
                                },
                            });
                        }
                    }
                }
                let available = !locations.is_empty();
                if let Some(first) = locations.first() {
                    log::info!(
                        "[definition-link] lsp response id={} locations={} first={} range={:?}",
                        request_id,
                        locations.len(),
                        first.relative_path.display(),
                        first.range
                    );
                } else {
                    log::info!(
                        "[definition-link] lsp response id={} no definition (active={})",
                        request_id,
                        active
                    );
                }
                self.definition_hover_resolution = available.then(|| DefinitionHoverResolution {
                    request_id,
                    project_root: key.project_root.clone(),
                    source_relative_path,
                    symbol_range,
                    locations,
                });
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.set_definition_hover_result(request_id, available, view_ctx);
                });
            }
            LanguageRequestPurpose::Formatting => {
                let edits = parse_text_edits_for_path(&key.project_root, relative_path, &value);
                self.apply_language_text_edits(&key.project_root, edits, ctx);
            }
            LanguageRequestPurpose::Rename => {
                let edits = parse_text_edits_for_path(&key.project_root, relative_path, &value);
                self.apply_language_text_edits(&key.project_root, edits, ctx);
            }
            LanguageRequestPurpose::FileRename {
                source,
                destination,
            } => {
                let mut edits = parse_text_edits_for_path(&key.project_root, &source, &value);
                // `willRenameFiles` runs before the filesystem move. A server
                // may nevertheless address an edited document by its future
                // URI, so map files inside the destination folder back to
                // their current on-disk locations for this transaction.
                for edit in &mut edits {
                    if let Ok(suffix) = edit.relative_path.strip_prefix(&destination) {
                        edit.relative_path = source.join(suffix);
                    }
                }
                let needs_local_import_fallback = edits.is_empty();
                self.apply_language_text_edits(&key.project_root, edits, ctx);
                self.perform_explorer_rename(source, destination, needs_local_import_fallback, ctx);
            }
            LanguageRequestPurpose::CodeActions => {
                let actions = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|action| {
                        let title = action.get("title")?.as_str()?.to_owned();
                        Some((title, action.clone()))
                    })
                    .collect();
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.set_language_code_actions(actions, view_ctx);
                });
            }
            LanguageRequestPurpose::Completion => {}
        }
    }

    pub(crate) fn drain_language_service_events(&mut self, ctx: &mut ViewContext<Self>) {
        let mut updates = Vec::new();
        let mut install_required = Vec::new();
        let mut installed = Vec::new();
        for _ in 0..64 {
            let Some(event) = self
                .language_services
                .as_ref()
                .and_then(LanguageServiceClient::try_recv)
            else {
                break;
            };
            match event {
                LanguageServiceEvent::InstallRequired(key) => {
                    log::info!(
                        "[lsp] install required server={} root={}",
                        key.server_id,
                        key.project_root.display()
                    );
                    install_required.push(key.clone());
                    updates.push((
                        key,
                        "Download required".to_string(),
                        super::language_services_tray::ServiceState::Unavailable,
                    ));
                }
                LanguageServiceEvent::Downloading(key) => {
                    log::info!(
                        "[lsp] downloading server={} root={}",
                        key.server_id,
                        key.project_root.display()
                    );
                    updates.push((
                        key,
                        "Downloading".to_string(),
                        super::language_services_tray::ServiceState::Downloading,
                    ));
                }
                LanguageServiceEvent::Installed(key) => {
                    log::info!(
                        "[lsp] installed server={} root={}",
                        key.server_id,
                        key.project_root.display()
                    );
                    installed.push(key.clone());
                    updates.push((
                        key,
                        "Installed".to_string(),
                        super::language_services_tray::ServiceState::Installed,
                    ));
                }
                LanguageServiceEvent::Starting(key) => {
                    log::info!(
                        "[lsp] starting server={} root={}",
                        key.server_id,
                        key.project_root.display()
                    );
                    updates.push((
                        key,
                        "Initializing".to_string(),
                        super::language_services_tray::ServiceState::Starting,
                    ));
                }
                LanguageServiceEvent::Ready { key, capabilities } => {
                    log::info!(
                        "[lsp] ready server={} root={}",
                        key.server_id,
                        key.project_root.display()
                    );
                    self.language_navigation_capabilities
                        .insert(key.clone(), language_navigation_capabilities(&capabilities));
                    if capabilities
                        .pointer("/workspace/fileOperations/willRename")
                        .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
                    {
                        self.language_file_rename_servers.insert(key.clone());
                    } else {
                        self.language_file_rename_servers.remove(&key);
                    }
                    self.refresh_editor_navigation_capabilities(ctx);
                    updates.push((
                        key,
                        "Ready".to_string(),
                        super::language_services_tray::ServiceState::Ready,
                    ));
                }
                LanguageServiceEvent::Stopped(key) => {
                    log::debug!(
                        "[lsp] stopped server={} root={}",
                        key.server_id,
                        key.project_root.display()
                    );
                    self.language_navigation_capabilities.remove(&key);
                    self.language_file_rename_servers.remove(&key);
                    self.refresh_editor_navigation_capabilities(ctx);
                    updates.push((
                        key,
                        "Stopped".to_string(),
                        super::language_services_tray::ServiceState::Stopped,
                    ));
                }
                LanguageServiceEvent::RestartScheduled {
                    key,
                    attempt,
                    delay,
                    reason,
                } => {
                    log::warn!(
                        "[lsp] restart scheduled server={} root={} attempt={} delay_ms={} reason={}",
                        key.server_id,
                        key.project_root.display(),
                        attempt,
                        delay.as_millis(),
                        reason
                    );
                    self.language_file_rename_servers.remove(&key);
                    updates.push((
                        key,
                        format!("Retry {attempt}"),
                        super::language_services_tray::ServiceState::Starting,
                    ));
                }
                LanguageServiceEvent::Failed { key, reason } => {
                    log::warn!(
                        "[lsp] unavailable server={} root={} reason={}",
                        key.server_id,
                        key.project_root.display(),
                        reason
                    );
                    self.language_navigation_capabilities.remove(&key);
                    self.language_file_rename_servers.remove(&key);
                    self.refresh_editor_navigation_capabilities(ctx);
                    updates.push((
                        key,
                        "Unavailable".to_string(),
                        super::language_services_tray::ServiceState::Unavailable,
                    ));
                }
                LanguageServiceEvent::Diagnostics(diagnostics) => {
                    // One shared diagnostic model feeds both active-editor
                    // squiggles and the Problems view. Background tabs must not
                    // lose diagnostics merely because focus changed.
                    let server_id = diagnostics.server_id.clone();
                    let relative_path = diagnostics.relative_path.clone();
                    let count = diagnostics.items.len();
                    ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                        model.set_language_diagnostics(diagnostics, model_ctx);
                    });
                    log::info!(
                        "[lsp-lifecycle] diagnostics applied server={} path={} count={}",
                        server_id,
                        relative_path.display(),
                        count
                    );
                }
                LanguageServiceEvent::Response {
                    token,
                    key,
                    relative_path,
                    result,
                } => {
                    log::debug!(
                        "[lsp] response token={} server={} path={} ok={}",
                        token,
                        key.server_id,
                        relative_path.display(),
                        result.is_ok()
                    );
                    if self.language_completion_token == Some(token) {
                        self.language_completion_token = None;
                        let active = self.language_document_sync.as_ref().is_some_and(|sync| {
                            sync.project_root == key.project_root
                                && sync.relative_path == relative_path
                        });
                        if active {
                            let editor_snapshot = self
                                .project_sidebar_view
                                .read(ctx, |view, app| view.document_text_and_cursor(app));
                            let suffix = result.as_ref().ok().and_then(|value| {
                                editor_snapshot.as_ref().and_then(|(text, cursor)| {
                                    completion_suffix(&value, text, *cursor)
                                })
                            });
                            ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                                view.set_document_completion(suffix, view_ctx);
                            });
                        }
                    }
                    if let Some(purpose) = self.language_request_tokens.remove(&token) {
                        self.handle_language_response(purpose, &key, &relative_path, result, ctx);
                    }
                }
                LanguageServiceEvent::ServerMessage { .. } => {
                    // Diagnostics and request responses are consumed by the
                    // next editor presentation slice. Keep polling bounded.
                }
            }
        }
        // Repaint immediately: diagnostics and lifecycle changes just landed
        // in the model, and waiting for the next periodic poll made errors
        // appear seconds late.
        ctx.notify();
        for key in installed {
            self.language_service_install_prompts.remove(&key);
        }
        for key in install_required {
            let is_current_root = self
                .document_owner_root
                .as_ref()
                .is_some_and(|root| *root == key.project_root);
            if is_current_root && self.language_service_install_prompts.insert(key) {
                self.language_services_open = true;
                self.ports_open = false;
            }
        }
        if !updates.is_empty() {
            for (key, detail, state) in updates {
                if self
                    .document_owner_root
                    .as_ref()
                    .is_some_and(|root| *root != key.project_root)
                {
                    continue;
                }
                let label = self
                    .language_server_catalog
                    .server(&key.server_id)
                    .map(|server| server.display_name.clone())
                    .unwrap_or_else(|| key.server_id.clone());
                if let Some(snapshot) = self
                    .language_service_status
                    .iter_mut()
                    .find(|snapshot| snapshot.server_id == key.server_id)
                {
                    snapshot.detail = detail;
                    snapshot.state = state;
                } else {
                    self.language_service_status.push(
                        super::language_services_tray::ServiceSnapshot {
                            server_id: key.server_id.clone(),
                            label,
                            detail,
                            state,
                            installable: self
                                .language_server_catalog
                                .server(&key.server_id)
                                .is_some_and(|server| server.install.is_some()),
                        },
                    );
                }
            }
            ctx.notify();
        }
    }
}

impl WorkspaceView {
    pub(crate) fn active_language_document_metadata(
        &self,
        ctx: &AppContext,
    ) -> Option<(PathBuf, PathBuf, u64)> {
        let project_root = self.document_owner_root.clone()?;
        // Language servers run on this Mac against local files; a remote
        // project's files live on its machine.
        if crate::remote_fs::is_remote(&project_root) {
            return None;
        }
        let (relative_path, revision) = self
            .project_sidebar_model
            .read(ctx, |model, _| model.language_document_metadata())?;
        dragent_editor::LanguageServiceRegistry::for_path(&relative_path)?;
        Some((project_root, relative_path, revision))
    }

    pub(crate) fn language_document_snapshot(
        &self,
        project_root: PathBuf,
        relative_path: PathBuf,
        ctx: &AppContext,
    ) -> Option<SyncedDocument> {
        let text = self
            .project_sidebar_model
            .read(ctx, |model, _| model.language_document_text(&relative_path))?;
        Some(SyncedDocument {
            project_root,
            relative_path,
            text,
        })
    }

    pub(crate) fn active_document_capabilities(
        &self,
        relative_path: &Path,
        ctx: &AppContext,
    ) -> Option<dragent_document::DocumentCapabilities> {
        self.project_sidebar_model.read(ctx, |model, _| {
            model.language_document_capabilities(relative_path)
        })
    }
}
