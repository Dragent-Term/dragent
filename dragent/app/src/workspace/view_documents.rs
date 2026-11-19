//! Document lifecycle methods for WorkspaceView.
//!
//! Responsible for:
//! - opening/closing/revealing editor documents and diff tabs
//! - saving, reverting, and checkpointing open documents
//!
//! Not responsible for:
//! - editor rendering (view_render, editor_render)
//! - document text buffers (dragent_document)
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
use super::view_tabs::{apply_sidebar_visibility, refresh_sidebar_projects};

impl WorkspaceView {
    pub(crate) fn open_document_path(
        &mut self,
        relative_path: PathBuf,
        allow_preview_reuse: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let owner_root = (self.document_center_open
            && self.document_tab_id == Some(self.active_tab_id))
        .then(|| self.document_owner_root.clone())
        .flatten();
        let active_root = self
            .tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .filter(|tab| !tab.pane.is_settings())
            .and_then(|tab| tab.project_root.clone())
            .filter(|root| root.is_dir());
        let Some(root) = owner_root.or(active_root) else {
            log::warn!(
                "[editor-open] active tab has no valid folder tab_id={} relative={}",
                self.active_tab_id,
                relative_path.display()
            );
            return;
        };

        self.open_document_path_at_root(root, relative_path, allow_preview_reuse, ctx);
    }

    /// Open a file path detected in terminal output (block text, an agent
    /// card, or the alt-screen). Unlike `open_document_path`, the caller has
    /// no "active tab" project root to lean on — `path` was already resolved
    /// to an absolute path against the originating block's cwd, so this finds
    /// its own root by walking up to the nearest `.git`, falling back to the
    /// file's parent directory for files outside any repo.
    pub(crate) fn open_terminal_detected_path(
        &mut self,
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !path.is_file() {
            log::info!(
                "[terminal-link] not a file, ignoring click path={}",
                path.display()
            );
            return;
        }
        let root = find_git_root(&path).or_else(|| path.parent().map(|p| p.to_path_buf()));
        let Some(root) = root else {
            log::warn!("[terminal-link] no usable root for path={}", path.display());
            return;
        };
        let relative_path = path
            .strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| PathBuf::from(path.file_name().unwrap_or_default()));

        if let Some(line) = line {
            let position = TextPosition {
                line: line.saturating_sub(1),
                character: column.unwrap_or(1).saturating_sub(1),
            };
            self.pending_language_navigation = Some((
                relative_path.clone(),
                TextRange {
                    start: position,
                    end: position,
                },
            ));
        }

        self.open_document_path_at_root(root, relative_path, true, ctx);
    }

    /// Open a path using the root that produced it. Search and Git paths are
    /// often repository-relative while a terminal tab may currently be inside
    /// a nested directory, so callers from those surfaces must not infer this
    /// identity from the active pane's CWD.
    pub(crate) fn open_document_path_at_root(
        &mut self,
        root: PathBuf,
        relative_path: PathBuf,
        allow_preview_reuse: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let restoring = self.document_restore_pending;
        if !restoring && !root.is_dir() {
            log::warn!(
                "[editor-open] requested root is unavailable root={} relative={}",
                root.display(),
                relative_path.display()
            );
            return;
        }
        self.work_item_tab_active = false;
        // A file click is an explicit request to see the editor. Preserve the
        // running dock, but restore it from full-height so the file is visible
        // immediately rather than appearing to do nothing.
        self.document_terminal_maximized = false;

        if self.document_restore_pending {
            // Saved editor tabs belong to their original project. Opening a
            // file from another root starts a clean tab set rather than mixing
            // relative paths from two workspaces.
            if self.document_owner_root.as_ref() != Some(&root) {
                self.document_open_paths.clear();
                self.document_preview_path = None;
                self.document_terminal_visible = false;
            }
            self.document_restore_pending = false;
        }

        // Relative editor paths are meaningful only within their project
        // directory. Opening a file from another tab/root starts a separate
        // editor surface and must not carry the previous root's file tabs.
        if self.document_owner_root.as_ref() != Some(&root) {
            self.document_open_paths.clear();
            self.document_preview_path = None;
            self.document_terminal_visible = false;
            self.document_terminal_active_id = None;
            self.document_terminal_split_id = None;
        }

        // Single-click reuses one preview tab
        // while on; off opens every file as a permanent tab.
        let allow_preview_reuse = allow_preview_reuse
            && crate::console::editor_settings::EditorSettings::global()
                .editor
                .preview_tabs;
        if !self.document_open_paths.contains(&relative_path) {
            let preview_is_dirty = allow_preview_reuse
                && self
                    .document_preview_path
                    .as_ref()
                    .is_some_and(|preview_path| {
                        self.project_sidebar_model
                            .read(ctx, |model, _| model.document_is_dirty(&root, preview_path))
                    });
            if preview_is_dirty {
                self.document_preview_path = None;
            }
            let replace_index = allow_preview_reuse
                .then(|| {
                    super::editor_tabs::preview_replacement_index(
                        &self.document_open_paths,
                        self.document_preview_path.as_deref(),
                        preview_is_dirty,
                    )
                })
                .flatten();
            if let Some(index) = replace_index {
                self.document_open_paths[index] = relative_path.clone();
            } else {
                self.document_open_paths.push(relative_path.clone());
            }
            self.document_preview_path = allow_preview_reuse.then(|| relative_path.clone());
        }
        self.show_document_center();
        self.document_tab_id = Some(self.active_tab_id);
        if self.document_terminal_visible {
            self.set_document_terminal_backdrops(true, ctx);
        }
        self.arm_document_tree_watcher(&root);
        self.document_index_root = root.clone();
        self.document_owner_root = Some(root.clone());
        let owner_tab_id = self.active_tab_id;
        self.document_requested_path = Some(relative_path.clone());
        self.reveal_document_tab(&relative_path);
        let sidebar_model = self.project_sidebar_model.clone();
        ctx.update_model(&sidebar_model, |model, model_ctx| {
            model.begin_document_load(root.clone(), model_ctx);
        });
        log::info!(
            "[editor-open] requested root={} relative={}",
            root.display(),
            relative_path.display()
        );
        let expected_root = root.clone();
        ctx.spawn(
            async move {
                let mut preview = super::project_documents::read_preview(&root, &relative_path);
                // A checkout or mount may still be becoming available at startup.
                // Retry only I/O availability failures, off the UI thread, and
                // leave the loading surface mounted until the bounded attempt ends.
                for delay_ms in [100, 300, 800] {
                    if !restoring || !matches!(preview.error.as_deref(),
                        Some("Project folder is unavailable"
                            | "File is unavailable. Refresh the file tree and try again."
                            | "File could not be opened"
                            | "File could not be read")) {
                        break;
                    }
                    warpui::r#async::Timer::after(std::time::Duration::from_millis(delay_ms)).await;
                    preview = super::project_documents::read_preview(&root, &relative_path);
                }
                preview
            },
            move |view, preview, ctx| {
                if view.document_tab_id != Some(owner_tab_id)
                    || view.document_owner_root.as_ref() != Some(&expected_root)
                    || view.document_requested_path.as_ref() != Some(&preview.relative_path)
                {
                    log::debug!(
                        "[editor-open] ignored stale result requested_root={} owner_root={} relative={}",
                        expected_root.display(),
                        view.document_owner_root
                            .as_deref()
                            .map(|root| root.display().to_string())
                            .unwrap_or_else(|| "<none>".to_string()),
                        preview.relative_path.display()
                    );
                    return;
                }
                log::info!(
                    "[editor-open] completed root={} relative={} status={}",
                    expected_root.display(),
                    preview.relative_path.display(),
                    if preview.error.is_some() { "error" } else { "ok" }
                );
                let loaded_path = preview.relative_path.clone();
                ctx.update_model(&sidebar_model, |model, model_ctx| {
                    model.set_preview(preview, model_ctx);
                });
                if view
                    .pending_language_navigation
                    .as_ref()
                    .is_some_and(|(path, _)| path == &loaded_path)
                {
                    if let Some((path, range)) = view.pending_language_navigation.take() {
                        ctx.update_view(&view.project_sidebar_view, |sidebar, sidebar_ctx| {
                            sidebar.navigate_active_document_to_range(&path, range, sidebar_ctx);
                        });
                    }
                }
            },
        );
        ctx.notify();
    }

    /// Mount a saved buffer without changing the user's pane focus or dock.
    pub(crate) fn restore_saved_document(
        &mut self,
        root: PathBuf,
        path: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        let layout = self.split_tree.clone();
        let focused = self.editor_pane_focused;
        let active_pane = self.active_pane_id;
        let dock_maximized = self.document_terminal_maximized;
        let work_active = self.work_item_tab_active;
        // These paths identify editor surfaces, not files on disk. Restore
        // them through the same asynchronous loaders as an explicit tab click.
        let virtual_action = if let Some((rel_path, staged)) = Self::parse_git_file_diff_tab_path(&path) {
            Some(WorkspaceAction::OpenGitDiff { rel_path, staged })
        } else if let Some(attempt_id) = Self::parse_workspace_attempt_diff_tab_path(&path) {
            Some(WorkspaceAction::OpenWorkspaceAttemptChanges { attempt_id })
        } else if path == std::path::Path::new(".git-diff/Uncommitted Changes") {
            Some(WorkspaceAction::OpenGitWorktreeDiff)
        } else if path == std::path::Path::new(".search/Search Results") {
            Some(WorkspaceAction::OpenProjectSearchResults)
        } else if let Ok(sha) = path.strip_prefix(".git-history") {
            let sha = sha.to_string_lossy().trim_matches('/').to_string();
            Some(WorkspaceAction::OpenGitCommitDiff { short_sha: sha.clone(), sha })
        } else {
            None
        };
        if let Some(action) = virtual_action {
            self.document_restore_pending = false;
            self.document_owner_root = Some(root.clone());
            self.document_index_root = root.clone();
            self.document_tab_id = Some(self.active_tab_id);
            self.document_center_open = true;
            if matches!(action, WorkspaceAction::OpenProjectSearchResults) {
                // Search results are ephemeral. Mount an empty result surface
                // and rerun the current query rather than reading a fake path
                // or leaving the editor waiting for an absent snapshot.
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.set_global_search_results(root, path,
                        std::sync::Arc::new(super::search_panel::SearchResults::empty(String::new())),
                        model_ctx);
                });
                ctx.update_view(&self.project_sidebar_view, |sidebar, sidebar_ctx| {
                    sidebar.prepare_global_search_editor();
                    sidebar.schedule_global_search(sidebar_ctx);
                });
            } else {
                self.handle_action(&action, ctx);
            }
        } else {
            self.document_restore_pending = true;
            self.open_document_path_at_root(root, path, false, ctx);
        }
        self.split_tree = layout;
        self.editor_pane_focused = focused;
        self.active_pane_id = active_pane;
        self.document_terminal_maximized = dock_maximized;
        self.work_item_tab_active = work_active;
    }

    pub(crate) fn close_document_tab_now(
        &mut self,
        relative_path: &std::path::Path,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(index) = self
            .document_open_paths
            .iter()
            .position(|path| path == relative_path)
        else {
            return;
        };
        let was_active = self.document_requested_path.as_deref() == Some(relative_path);
        if let (Some(client), Some(root)) = (
            self.language_services.as_ref(),
            self.document_owner_root.clone(),
        ) {
            let _ = client.close(root, relative_path.to_path_buf());
        }
        self.document_open_paths.remove(index);
        if self.document_preview_path.as_deref() == Some(relative_path) {
            self.document_preview_path = None;
        }
        if self.document_open_paths.is_empty() {
            self.close_document_now(ctx);
            return;
        }
        if was_active {
            let next_index = index.min(self.document_open_paths.len() - 1);
            let next = self.document_open_paths[next_index].clone();
            if let Some((rel_path, staged)) = Self::parse_git_file_diff_tab_path(&next) {
                self.handle_action(&WorkspaceAction::OpenGitDiff { rel_path, staged }, ctx);
            } else if let Some(attempt_id) = Self::parse_workspace_attempt_diff_tab_path(&next) {
                self.handle_action(
                    &WorkspaceAction::OpenWorkspaceAttemptChanges { attempt_id },
                    ctx,
                );
            } else if next == std::path::Path::new(".git-diff/Uncommitted Changes") {
                self.handle_action(&WorkspaceAction::OpenGitWorktreeDiff, ctx);
            } else if next == std::path::Path::new(".search/Search Results") {
                self.handle_action(&WorkspaceAction::OpenProjectSearchResults, ctx);
            } else if let Ok(short_sha) = next.strip_prefix(".git-history") {
                let short_sha = short_sha.to_string_lossy().trim_matches('/').to_string();
                self.handle_action(
                    &WorkspaceAction::OpenGitCommitDiff {
                        sha: short_sha.clone(),
                        short_sha,
                    },
                    ctx,
                );
            } else {
                self.open_document_path(next, false, ctx);
            }
        } else {
            ctx.notify();
        }
    }

    pub(crate) fn request_close_document_tab(
        &mut self,
        relative_path: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        let dirty = self.project_sidebar_model.read(ctx, |model, _| {
            model.document_is_dirty(&self.document_index_root, &relative_path)
        });
        if dirty {
            // Make the file active before presenting the save decision so the
            // existing save pipeline always operates on the intended buffer.
            if self.document_requested_path.as_ref() != Some(&relative_path) {
                self.open_document_path(relative_path.clone(), false, ctx);
            }
            self.pending_close_document = Some(relative_path);
            ctx.notify();
        } else {
            self.close_document_tab_now(&relative_path, ctx);
        }
    }

    /// Reattach the preserved editor to its owner tab after visiting another
    /// terminal tab. The active tab's explorer may have replaced the shared
    /// path index in the meantime, so restore its path metadata and refresh a
    /// clean buffer from disk. Unsaved buffers remain authoritative and are
    /// never overwritten by a tab switch.
    pub(crate) fn restore_document_owner_tab(
        &mut self,
        tab_id: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        let owns_editor = self.document_tab_id == Some(tab_id) || self.editor_pane_in_layout();
        if self.document_restore_pending && owns_editor {
            if let (Some(root), Some(path)) = (
                self.document_owner_root.clone(),
                self.document_requested_path.clone(),
            ) {
                self.restore_saved_document(root, path, ctx);
                return;
            }
        }
        self.document_center_open = owns_editor;
        if !self.document_center_open {
            self.document_terminal_visible = false;
            self.set_document_terminal_backdrops(false, ctx);
            return;
        }

        let Some(root) = self.document_owner_root.clone() else {
            log::warn!("[editor-tab] owner tab has no document root tab_id={tab_id}");
            return;
        };
        // Dock terminals are grouped by the editor's project directory. A
        // different tab in the same directory may reuse that group, while a
        // tab rooted elsewhere must never inherit its terminals or active
        // split.
        self.select_document_terminal_group(&root);
        // A terminal group's existence must not reopen the dock. Visibility
        // belongs to the selected work item and was restored while switching
        // tabs; only an explicit terminal action changes it.
        self.set_document_terminal_backdrops(self.document_terminal_visible, ctx);
        let root_changed = self.document_index_root != root;
        self.arm_document_tree_watcher(&root);
        self.document_index_root = root.clone();

        let active_cwd = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .filter(|tab| !tab.pane.is_settings())
            .and_then(|tab| tab.project_root.clone());
        let cwds = active_cwd.clone().into_iter().collect::<Vec<_>>();
        let sidebar_model = self.project_sidebar_model.clone();
        ctx.update_model(&sidebar_model, |model, model_ctx| {
            if root_changed {
                model.clear_documents(model_ctx);
            }
            model.refresh_all_cwds(&cwds, active_cwd, model_ctx);
        });
        if root_changed {
            self.explorer_prompt = None;
            self.explorer_operation_error = None;
            self.document_index = ProjectDocumentIndex::default();
        }

        let reload_path = self.project_sidebar_model.read(ctx, |model, _| {
            model
                .buffer
                .as_ref()
                .filter(|buffer| !buffer.is_dirty())
                .map(|buffer| buffer.document().relative_path.clone())
        });

        let expected_tab_id = tab_id;
        let expected_root = root.clone();
        ctx.spawn(
            async move {
                let index = super::project_documents::discover(&root);
                let preview = reload_path.as_ref().map(|relative_path| {
                    super::project_documents::read_preview(&root, relative_path)
                });
                (root, index, preview)
            },
            move |view, (scanned_root, index, preview), ctx| {
                if view.document_tab_id != Some(expected_tab_id)
                    || !view.document_center_open
                    || view.document_owner_root.as_ref() != Some(&expected_root)
                    || scanned_root != expected_root
                {
                    log::debug!(
                        "[editor-tab] ignored stale owner-tree restore tab_id={} root={}",
                        expected_tab_id,
                        scanned_root.display()
                    );
                    return;
                }
                view.document_index_root = scanned_root;
                view.document_index = index.clone();
                ctx.update_model(&sidebar_model, |model, model_ctx| {
                    model.set_documents(index, model_ctx);
                });
                if let Some(preview) = preview {
                    let text = preview.text.clone();
                    let reloaded = ctx.update_model(&sidebar_model, |model, model_ctx| {
                        model.reload_clean_document(&preview, model_ctx)
                    });
                    if reloaded {
                        ctx.update_view(&view.project_sidebar_view, |sidebar, sidebar_ctx| {
                            sidebar.reload_saved_document_text(&text, sidebar_ctx);
                        });
                    }
                }
                ctx.notify();
            },
        );
    }

    pub(crate) fn save_open_document(
        &mut self,
        approved: bool,
        close_after_save: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let snapshot = self
            .project_sidebar_model
            .read(ctx, |model, _| model.document_snapshot());
        let Some((relative_path, text, expected_fingerprint)) = snapshot else {
            log::warn!("[editor-save] ignored: no active document buffer");
            return;
        };
        log::info!(
            "[editor-save] requested path={} bytes={} fingerprint={}",
            relative_path.display(),
            text.len(),
            expected_fingerprint.is_some()
        );
        let root = self.document_index_root.clone();
        let recovery_root = root.clone();
        let sidebar_model = self.project_sidebar_model.clone();
        ctx.spawn(
            async move {
                let result = super::project_documents::save_text(
                    &root,
                    &relative_path,
                    &text,
                    expected_fingerprint,
                );
                (relative_path, result)
            },
            move |view, (relative_path, result), ctx| {
                let succeeded = result.is_ok();
                match &result {
                    Ok(_) => log::info!(
                        "[editor-save] completed path={} status=ok",
                        relative_path.display()
                    ),
                    Err(error) => log::warn!(
                        "[editor-save] completed path={} status=error error={}",
                        relative_path.display(),
                        error
                    ),
                }
                ctx.update_model(&sidebar_model, |model, model_ctx| match result {
                    Ok(fingerprint) => {
                        model.mark_document_saved(&relative_path, fingerprint, model_ctx)
                    }
                    Err(error) => model.set_document_notice(false, error, model_ctx),
                });
                if succeeded {
                    view.notify_language_document_saved(ctx);
                }
                ctx.update_model(&sidebar_model, |model, model_ctx| {
                    model.refresh_git_status(model_ctx);
                });
                if succeeded {
                    view.persistence_writer
                        .delete_working_copy(recovery_root.clone(), relative_path.clone());
                    view.record_workspace_event(WorkspaceEventKind::FileChanged {
                        relative_path: relative_path.clone(),
                    });
                    if approved {
                        view.record_workspace_event(WorkspaceEventKind::ReviewFinding {
                            summary: format!("approved {}", relative_path.display()),
                        });
                    }
                    if close_after_save {
                        view.pending_close_document = None;
                        view.close_document_tab_now(&relative_path, ctx);
                    }
                }
                ctx.notify();
            },
        );
    }

    pub(crate) fn revert_open_document(&mut self, rejected: bool, ctx: &mut ViewContext<Self>) {
        let text = ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
            model.revert_document(model_ctx)
        });
        if let Some(text) = text {
            ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                view.set_document_editor_text(&text, view_ctx);
            });
            if rejected {
                self.record_workspace_event(WorkspaceEventKind::ReviewFinding {
                    summary: "rejected open document changes".to_string(),
                });
            }
        }
    }

    pub(crate) fn checkpoint_open_document(&mut self, ctx: &mut ViewContext<Self>) {
        let snapshot = self
            .project_sidebar_model
            .read(ctx, |model, _| model.checkpoint_snapshot());
        let Some((relative_path, patch)) = snapshot else {
            return;
        };
        let root = self.document_index_root.clone();
        let sidebar_model = self.project_sidebar_model.clone();
        ctx.spawn(
            async move {
                super::project_documents::write_checkpoint(&root, &relative_path, &patch)
            },
            move |view, result, ctx| {
                ctx.update_model(&sidebar_model, |model, model_ctx| match &result {
                    Ok(path) => model.set_document_notice(
                        true,
                        format!("Checkpoint saved: {}", path.display()),
                        model_ctx,
                    ),
                    Err(error) => model.set_document_notice(false, error.clone(), model_ctx),
                });
                if let Ok(path) = result {
                    view.record_workspace_event(WorkspaceEventKind::CheckpointCreated {
                        summary: path.display().to_string(),
                    });
                }
                ctx.notify();
            },
        );
    }

    /// Build a workspace restoring the persisted session (used by the startup window).
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        Self::build(ctx, true)
    }

    /// Build a fresh workspace at the project-start surface. Used for additional
    /// windows opened via New Window — persistence is single-window, so only the
    /// startup window restores; extra windows never create a terminal implicitly.
    pub fn new_empty(ctx: &mut ViewContext<Self>) -> Self {
        Self::build(ctx, false)
    }
}

impl WorkspaceView {
    pub(crate) fn reveal_document_tab(&self, relative_path: &std::path::Path) {
        self.document_tabs_scroll
            .scroll_to_position(warpui::elements::ScrollTarget {
                position_id: super::editor_tabs::position_id(relative_path),
                mode: warpui::elements::ScrollToPositionMode::FullyIntoView,
            });
    }

    pub(crate) fn git_file_diff_tab_path(relative_path: &std::path::Path, staged: bool) -> PathBuf {
        PathBuf::from(".git-diff")
            .join("file")
            .join(if staged { "staged" } else { "working" })
            .join(relative_path)
    }

    pub(crate) fn parse_git_file_diff_tab_path(path: &std::path::Path) -> Option<(String, bool)> {
        let suffix = path.strip_prefix(".git-diff/file").ok()?;
        let mut components = suffix.components();
        let target = components.next()?.as_os_str().to_str()?;
        let staged = match target {
            "staged" => true,
            "working" => false,
            _ => return None,
        };
        let relative_path = components.as_path();
        if relative_path.as_os_str().is_empty() {
            return None;
        }
        Some((relative_path.to_string_lossy().into_owned(), staged))
    }

    pub(crate) fn workspace_attempt_diff_tab_path(attempt_id: ExecutionAttemptId) -> PathBuf {
        PathBuf::from(".work-changes").join(attempt_id.0.to_string())
    }

    pub(crate) fn parse_workspace_attempt_diff_tab_path(
        path: &std::path::Path,
    ) -> Option<ExecutionAttemptId> {
        let suffix = path.strip_prefix(".work-changes").ok()?;
        let value = suffix.to_string_lossy();
        let value = value.trim_matches('/');
        (!value.is_empty())
            .then(|| Uuid::parse_str(value).ok().map(ExecutionAttemptId))
            .flatten()
    }

    pub(crate) fn git_file_diff_tab_label(path: &std::path::Path) -> Option<String> {
        if let Some(attempt_id) = Self::parse_workspace_attempt_diff_tab_path(path) {
            let short = attempt_id.0.simple().to_string();
            return Some(format!("Agent changes · {}", &short[..8]));
        }
        let (relative_path, staged) = Self::parse_git_file_diff_tab_path(path)?;
        let file_name = std::path::Path::new(&relative_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&relative_path);
        Some(format!(
            "{file_name} — {}",
            if staged { "Staged" } else { "Working" }
        ))
    }
}

impl WorkspaceView {
    pub(crate) fn close_document_now(&mut self, ctx: &mut ViewContext<Self>) {
        if let (Some(client), Some(root)) = (
            self.language_services.as_ref(),
            self.document_owner_root.clone(),
        ) {
            for relative_path in &self.document_open_paths {
                let _ = client.close(root.clone(), relative_path.clone());
            }
        }
        self.set_document_terminal_backdrops(false, ctx);
        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
            if view.buffer_search_open() {
                view.close_buffer_search(view_ctx);
            }
        });
        self.document_center_open = false;
        self.work_item_tab_active = self.work_item_tab_open;
        self.document_tab_id = self.work_item_tab_open.then_some(self.active_tab_id);
        self.document_owner_root = self
            .work_item_tab_open
            .then(|| self.document_index_root.clone());
        self.document_requested_path = None;
        self.document_open_paths.clear();
        self.document_preview_path = None;
        self.document_terminal_visible = false;
        // Closing the final file always removes the editor tile. An open Work
        // Item remains available through its own workspace pane.
        self.remove_editor_pane();
        if self.tabs.is_empty() && !self.document_center_open {
            self.project_start_visible = true;
            self.left_panel_visible = !self.open_project_roots.is_empty();
        }
        self.set_active_terminal_sticky_headers(true, ctx);
        refresh_sidebar_projects(self, ctx);
        apply_sidebar_visibility(self, ctx);
        ctx.notify();
    }
}
