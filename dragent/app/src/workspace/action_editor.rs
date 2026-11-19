//! Workspace editor and documents actions.
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
    pub(crate) fn handle_editor_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::OpenProjectSearchResults => {
                let results = self
                    .project_sidebar_view
                    .read(ctx, |view, _| view.global_search_snapshot());
                let Some((root, results)) = results else {
                    return;
                };
                let review_path = PathBuf::from(".search/Search Results");
                self.show_document_center();
                self.document_tab_id = Some(self.active_tab_id);
                self.document_owner_root = Some(root.clone());
                self.document_index_root = root.clone();
                if !self.document_open_paths.contains(&review_path) {
                    self.document_open_paths.push(review_path.clone());
                }
                self.document_requested_path = Some(review_path.clone());
                self.document_preview_path = None;
                self.reveal_document_tab(&review_path);
                ctx.update_view(&self.project_sidebar_view, |view, _| {
                    view.prepare_global_search_editor();
                });
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.set_global_search_results(root.clone(), review_path, results, model_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::SelectOpenDocument { relative_path } => {
                if self.document_open_paths.contains(relative_path) {
                    if let Some((rel_path, staged)) =
                        Self::parse_git_file_diff_tab_path(relative_path)
                    {
                        self.handle_action(&WorkspaceAction::OpenGitDiff { rel_path, staged }, ctx);
                    } else if let Some(attempt_id) =
                        Self::parse_workspace_attempt_diff_tab_path(relative_path)
                    {
                        self.handle_action(
                            &WorkspaceAction::OpenWorkspaceAttemptChanges { attempt_id },
                            ctx,
                        );
                    } else if relative_path == std::path::Path::new(".git-diff/Uncommitted Changes")
                    {
                        self.handle_action(&WorkspaceAction::OpenGitWorktreeDiff, ctx);
                    } else if relative_path == std::path::Path::new(".search/Search Results") {
                        self.handle_action(&WorkspaceAction::OpenProjectSearchResults, ctx);
                    } else if let Ok(short_sha) = relative_path.strip_prefix(".git-history") {
                        let short_sha = short_sha.to_string_lossy().trim_matches('/').to_string();
                        self.handle_action(
                            &WorkspaceAction::OpenGitCommitDiff {
                                sha: short_sha.clone(),
                                short_sha,
                            },
                            ctx,
                        );
                    } else {
                        self.open_document_path(relative_path.clone(), false, ctx);
                    }
                }
            }
            WorkspaceAction::CloseDocumentTab { relative_path } => {
                self.request_close_document_tab(relative_path.clone(), ctx);
            }
            WorkspaceAction::OpenBufferSearch { seed_selection } => {
                if self.document_center_open {
                    ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                        view.open_buffer_search(*seed_selection, view_ctx);
                    });
                }
            }
            WorkspaceAction::CloseBufferSearch => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.close_buffer_search(view_ctx);
                });
            }
            WorkspaceAction::BufferSearchNext => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.navigate_buffer_search(false, view_ctx);
                });
            }
            WorkspaceAction::BufferSearchPrevious => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.navigate_buffer_search(true, view_ctx);
                });
            }
            WorkspaceAction::ToggleBufferSearchCaseSensitive => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.toggle_buffer_search_case(view_ctx);
                });
            }
            WorkspaceAction::ToggleBufferSearchWholeWord => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.toggle_buffer_search_whole_word(view_ctx);
                });
            }
            WorkspaceAction::ToggleBufferSearchRegex => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.toggle_buffer_search_regex(view_ctx);
                });
            }
            WorkspaceAction::ToggleBufferSearchReplace => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.toggle_buffer_search_replace(view_ctx);
                });
            }
            WorkspaceAction::BufferSearchReplaceCurrent => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.replace_buffer_search_match(false, view_ctx);
                });
            }
            WorkspaceAction::BufferSearchReplaceAll => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.replace_buffer_search_match(true, view_ctx);
                });
            }
            WorkspaceAction::ToggleMarkdownPreview => {
                if self.document_center_open {
                    ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                        view.toggle_markdown_preview(view_ctx);
                    });
                }
            }
            WorkspaceAction::ToggleDocumentTerminal => {
                if self.document_terminal_visible {
                    self.document_terminal_visible = false;
                    self.document_terminal_maximized = false;
                    self.set_document_terminal_backdrops(false, ctx);
                } else {
                    self.ensure_document_terminal_group(ctx);
                    if self.active_document_terminal().is_none() {
                        self.new_document_terminal(false, ctx);
                    }
                    self.document_terminal_visible = true;
                    self.set_document_terminal_backdrops(true, ctx);
                }
                self.sync_pane_activity(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::NewDockedTerminal => {
                self.new_document_terminal(false, ctx);
                self.sync_pane_activity(ctx);
            }
            WorkspaceAction::SelectDockedTerminal { terminal_id } => {
                let terminal_id = *terminal_id;
                if self
                    .document_terminals
                    .iter()
                    .any(|tab| tab.id == terminal_id)
                {
                    let previous = self.document_terminal_active_id;
                    if self.document_terminal_split_id == Some(terminal_id) {
                        self.document_terminal_split_id = previous.filter(|id| *id != terminal_id);
                    } else if self.document_terminal_split_id.is_some()
                        && previous != Some(terminal_id)
                    {
                        self.document_terminal_split_id = None;
                    }
                    self.document_terminal_active_id = Some(terminal_id);
                    if let Some(tab) = self.active_document_terminal() {
                        tab.pane.focus(ctx);
                    }
                    self.sync_pane_activity(ctx);
                    ctx.notify();
                }
            }
            WorkspaceAction::CloseDockedTerminal { terminal_id } => {
                self.close_document_terminal(*terminal_id, ctx);
                self.sync_pane_activity(ctx);
            }
            WorkspaceAction::SplitDockedTerminal => {
                self.new_document_terminal(true, ctx);
                self.sync_pane_activity(ctx);
            }
            WorkspaceAction::ToggleDockedTerminalMaximized => {
                self.ensure_document_terminal_group(ctx);
                self.document_terminal_visible = true;
                self.set_document_terminal_backdrops(true, ctx);
                self.document_terminal_maximized = !self.document_terminal_maximized;
                self.sync_pane_activity(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::CloseOpenDocument => {
                if let Some(path) = self.document_requested_path.clone() {
                    self.request_close_document_tab(path, ctx);
                }
            }
            WorkspaceAction::SaveOpenDocument => self.save_open_document(false, false, ctx),
            WorkspaceAction::RequestLanguageCompletion { cursor } => {
                self.schedule_language_completion(*cursor, ctx);
            }
            WorkspaceAction::RequestLanguageNavigation { cursor, kind } => {
                self.request_language_navigation(*cursor, *kind, ctx);
            }
            WorkspaceAction::OpenDefinitionHover { request_id, cursor } => {
                self.definition_hover_latest_request_id = None;
                let resolution = self
                    .definition_hover_resolution
                    .take()
                    .filter(|resolution| {
                        resolution.request_id == *request_id
                            && *cursor >= resolution.symbol_range.start
                            && *cursor < resolution.symbol_range.end
                            && self.language_document_sync.as_ref().is_some_and(|sync| {
                                sync.project_root == resolution.project_root
                                    && sync.relative_path == resolution.source_relative_path
                            })
                    });
                if let Some(resolution) = resolution {
                    log::info!(
                        "[definition-link] click navigating cached target id={} locations={}",
                        resolution.request_id,
                        resolution.locations.len()
                    );
                    self.apply_language_navigation(
                        &resolution.project_root,
                        super::LanguageNavigationKind::Definition,
                        resolution.locations,
                        ctx,
                    );
                } else {
                    // Match the standard click fallback: if the validated hover cache
                    // disappeared, invoke the ordinary shared definition
                    // command at the clicked position rather than maintaining
                    // a separate command-click navigation implementation.
                    log::info!(
                        "[definition-link] no cached hover target, falling back to shared go-to-definition cursor={}",
                        cursor
                    );
                    self.request_language_navigation(
                        *cursor,
                        super::LanguageNavigationKind::Definition,
                        ctx,
                    );
                }
            }
            WorkspaceAction::RequestLanguageFormatting => {
                self.request_language_formatting(ctx);
            }
            WorkspaceAction::RequestLanguageCodeActions { cursor } => {
                self.request_language_code_actions(*cursor, ctx);
            }
            WorkspaceAction::StartLanguageRename { cursor } => {
                // The rename editor is owned by the document surface so the
                // command can preserve the clicked symbol while the user
                // enters the replacement name.
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.start_language_rename(*cursor, view_ctx);
                });
            }
            WorkspaceAction::RequestLanguageRename { cursor, new_name } => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.take_language_rename(view_ctx);
                });
                self.request_language_rename(*cursor, new_name.clone(), ctx);
            }
            WorkspaceAction::CancelLanguageRename => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.cancel_language_rename(view_ctx);
                });
            }
            WorkspaceAction::ApplyLanguageCodeAction { action } => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.clear_language_results(view_ctx);
                });
                let Some((root, path, _)) = self.active_language_document_metadata(ctx) else {
                    return;
                };
                if let Some(edit) = action.get("edit") {
                    let edits = parse_text_edits_for_path(&root, &path, edit);
                    self.apply_language_text_edits(&root, edits, ctx);
                } else {
                    log::warn!("[lsp] selected code action has no WorkspaceEdit");
                }
            }
            WorkspaceAction::DismissLanguageResults => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.clear_language_results(view_ctx);
                });
            }
            WorkspaceAction::SaveOpenDocumentAndClose => self.save_open_document(false, true, ctx),
            WorkspaceAction::DiscardOpenDocumentAndClose => {
                self.pending_close_document = None;
                if let Some(relative_path) = self.document_requested_path.clone() {
                    self.persistence_writer.delete_working_copy(
                        self.document_index_root.clone(),
                        relative_path.clone(),
                    );
                    ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                        model.discard_active_document(model_ctx);
                    });
                    self.close_document_tab_now(&relative_path, ctx);
                }
            }
            WorkspaceAction::CancelCloseOpenDocument => {
                self.pending_close_document = None;
                ctx.notify();
            }
            WorkspaceAction::ApproveOpenDocument => self.save_open_document(true, false, ctx),
            WorkspaceAction::RevertOpenDocument => self.revert_open_document(false, ctx),
            WorkspaceAction::RejectOpenDocument => self.revert_open_document(true, ctx),
            WorkspaceAction::CheckpointOpenDocument => self.checkpoint_open_document(ctx),
            WorkspaceAction::OpenReadOnlyDocument {
                root,
                relative_path,
            } => {
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.explorer_selection = Some((relative_path.clone(), false));
                    model_ctx.emit(super::project_sidebar::SidebarEvent::Changed);
                });
                if !self.workspace_trust.is_trusted(root)
                    && super::action_projects::workspace_trust_enforced()
                {
                    self.pending_workspace_trust = Some(root.clone());
                    self.pending_workspace_trust_document = Some(relative_path.clone());
                    ctx.notify();
                    return;
                }
                self.open_document_path_at_root(root.clone(), relative_path.clone(), true, ctx);
            }
            WorkspaceAction::ReorderDocumentTab {
                source_path,
                target_path,
            } => {
                let source_index = self
                    .document_open_paths
                    .iter()
                    .position(|path| path == source_path);
                let target_index = self
                    .document_open_paths
                    .iter()
                    .position(|path| path == target_path);
                if let (Some(source_index), Some(target_index)) = (source_index, target_index) {
                    if source_index != target_index {
                        let moved = self.document_open_paths.remove(source_index);
                        let destination = target_index.min(self.document_open_paths.len());
                        self.document_open_paths.insert(destination, moved);
                        // Manual organization is an explicit keep signal.
                        self.document_preview_path = None;
                        self.reveal_document_tab(source_path);
                        ctx.notify();
                    }
                }
            }
            WorkspaceAction::OpenMarkdownLink {
                source_path,
                target,
            } => {
                let root = self
                    .document_owner_root
                    .as_deref()
                    .unwrap_or(&self.document_index_root);
                match super::project_documents::classify_markdown_link(root, source_path, target) {
                    Some(super::project_documents::MarkdownLinkTarget::ProjectFile(path)) => {
                        log::info!(
                            "[markdown-link] opening project file source={} target={} resolved={}",
                            source_path.display(),
                            target,
                            path.display()
                        );
                        self.open_document_path(path, true, ctx);
                    }
                    Some(super::project_documents::MarkdownLinkTarget::ExternalUrl(url)) => {
                        log::info!("[markdown-link] opening url={url}");
                        self.open_link(&url, ctx);
                    }
                    None => {
                        log::warn!(
                            "[markdown-link] unresolved source={} target={}",
                            source_path.display(),
                            target
                        );
                    }
                }
            }
            WorkspaceAction::OpenTerminalPath { path, line, column } => {
                self.open_terminal_detected_path(path.clone(), *line, *column, ctx);
            }
            WorkspaceAction::RequestDefinitionHover {
                request_id,
                cursor,
                symbol_range,
                string_link,
            } => {
                self.request_definition_hover(
                    *request_id,
                    *cursor,
                    symbol_range.clone(),
                    string_link.clone(),
                    ctx,
                );
            }
            WorkspaceAction::OpenLanguageLocation {
                root,
                relative_path,
                range,
            } => {
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.clear_language_results(view_ctx);
                });
                let active = self.language_document_sync.as_ref().is_some_and(|sync| {
                    sync.project_root == *root
                        && self.document_owner_root.as_ref() == Some(root)
                        && sync.relative_path == *relative_path
                });
                if active {
                    ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                        view.navigate_active_document_to_range(relative_path, *range, view_ctx);
                    });
                } else {
                    self.pending_language_navigation = Some((relative_path.clone(), *range));
                    self.open_document_path_at_root(
                        root.clone(),
                        relative_path.clone(),
                        false,
                        ctx,
                    );
                }
            }
            WorkspaceAction::AttachEditorSymbolToAgent {
                path,
                line,
                end_line,
                symbol_name: _,
            } => {
                self.attach_editor_symbol_to_agent(
                    path.clone(),
                    *line,
                    *end_line,
                    false,
                    ctx,
                );
            }
            WorkspaceAction::AttachEditorSymbolToNewAgent {
                path,
                line,
                end_line,
                symbol_name: _,
            } => {
                self.attach_editor_symbol_to_agent(
                    path.clone(),
                    *line,
                    *end_line,
                    true,
                    ctx,
                );
            }
            _ => {}
        }
    }

    /// Attach `path:line` to an agent composer. With `new_chat`, always opens
    /// a fresh Dragent tab; otherwise targets the active running agent
    /// (active tab preferred, else first running agent pane), falling back to
    /// a fresh Dragent tab when nothing is running.
    fn attach_editor_symbol_to_agent(
        &mut self,
        path: std::path::PathBuf,
        line: u32,
        end_line: u32,
        new_chat: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        use crate::agent::TabSessionKind;
        if new_chat {
            let new_tab = self.create_agent_tab(ctx);
            new_tab
                .pane
                .set_agent_id(ctx, Some("dragent".to_string()));
            let target_id = new_tab.id;
            let terminal = match &new_tab.pane {
                super::pane::WorkspacePane::Terminal(handle) => Some(handle.clone()),
                super::pane::WorkspacePane::Settings(_)
                | super::pane::WorkspacePane::Browser(_) => None,
            };
            self.active_tab_id = target_id;
            self.split_tree = None;
            self.tabs.push(new_tab);
            self.project_start_visible = false;
            if let Some(terminal) = terminal {
                ctx.update_view(&terminal, |tv, tctx| {
                    tv.attach_editor_symbol(&path, line, end_line, tctx)
                });
            }
            self.switch_workspace_work_for_tab(target_id, ctx);
            self.set_active_terminal_sticky_headers(true, ctx);
            self.update_active_pane(ctx);
            self.refresh_all_tab_titles(ctx);
            self.sync_left_panel(ctx);
            self.save_session(ctx);
            ctx.notify();
            return;
        }
        let is_agent_tab = |tab: &super::view::PaneTab, ctx: &ViewContext<Self>| -> bool {
            match &tab.pane {
                super::pane::WorkspacePane::Terminal(_) => {
                    tab.pane.agent_id(ctx).is_some()
                        || tab.pane.active_ai_tool(ctx).is_some()
                        || tab.pane.tab_session_kind(ctx) == TabSessionKind::Agent
                }
                super::pane::WorkspacePane::Settings(_)
                | super::pane::WorkspacePane::Browser(_) => false,
            }
        };
        let target_id = self
            .tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .filter(|tab| is_agent_tab(tab, ctx))
            .map(|tab| tab.id)
            .or_else(|| {
                self.tabs
                    .iter()
                    .filter(|tab| is_agent_tab(tab, ctx))
                    .find(|tab| tab.pane.is_running(ctx))
                    .map(|tab| tab.id)
            })
            .or_else(|| {
                self.tabs
                    .iter()
                    .find(|tab| is_agent_tab(tab, ctx))
                    .map(|tab| tab.id)
            });
        let Some(target_id) = target_id else {
            // No agent running: open a fresh Dragent chat carrying the symbol.
            self.attach_editor_symbol_to_agent(path, line, end_line, true, ctx);
            return;
        };
        let terminal = self
            .tabs
            .iter()
            .find(|tab| tab.id == target_id)
            .and_then(|tab| match &tab.pane {
                super::pane::WorkspacePane::Terminal(handle) => Some(handle.clone()),
                super::pane::WorkspacePane::Settings(_)
                | super::pane::WorkspacePane::Browser(_) => None,
            });
        let Some(terminal) = terminal else {
            return;
        };
        ctx.update_view(&terminal, |tv, tctx| {
            tv.attach_editor_symbol(&path, line, end_line, tctx)
        });
        self.active_tab_id = target_id;
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }
}
