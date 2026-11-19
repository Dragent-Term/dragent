//! Explorer, polling, and watcher methods for WorkspaceView.
//!
//! Responsible for:
//! - review ship/PR polling and scheduling
//! - document tree watching and refresh
//! - explorer clipboard, rename, mutations, and git status helpers
//!
//! Not responsible for:
//! - explorer rendering (project_sidebar)
//! - git operations themselves (git_status)
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
use super::view::{ExplorerClipboardEntry, ExplorerClipboardMode};

impl WorkspaceView {
    pub(crate) fn schedule_update_poll(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(Duration::from_secs(5)), |view, _, ctx| {
            view.schedule_update_poll(ctx);
            ctx.notify();
        });
    }

    /// Refresh the Review control's ship state now, then re-arm the 10s
    /// poll. The query reads the active project root
    /// at fire time, so project switches and new checkouts are picked up
    /// without explicit invalidation.
    pub(crate) fn refresh_review_ship_state(&mut self, ctx: &mut ViewContext<Self>) {
        let root = self.active_project_root();
        ctx.spawn(
            async move { query_review_ship_state(root) },
            |view, state, ctx| {
                view.review_ship = state;
                view.schedule_review_ship_poll(ctx);
                ctx.notify();
            },
        );
    }

    pub(crate) fn schedule_review_ship_poll(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(Duration::from_secs(10)), |view, _, ctx| {
            view.refresh_review_ship_state(ctx);
        });
    }

    /// Refresh the Review tab's PR data (
    /// PR summary every 10s, comments every third tick). Reads the active
    /// project root at fire time so switches need no invalidation.
    pub(crate) fn refresh_review_pr_state(&mut self, ctx: &mut ViewContext<Self>) {
        self.review_pr_request_epoch = self.review_pr_request_epoch.wrapping_add(1);
        let request_epoch = self.review_pr_request_epoch;
        let project_root = self.active_project_root();
        let Some(root) = project_root.as_deref().and_then(find_git_root) else {
            let sidebar = self.project_sidebar_view.clone();
            ctx.update_view(&sidebar, |ps, ps_ctx| {
                // A normal folder remains fully usable for files, editing,
                // and terminals. Only the Git-backed Review surface is empty.
                ps.review_pr = ReviewPrState::default();
                ps.review_pr.loading_pr = false;
                ps.review_pr.non_git_folder = project_root.is_some();
                ps_ctx.notify();
            });
            self.schedule_review_pr_poll(ctx);
            ctx.notify();
            return;
        };
        let want_comments = self.review_pr_tick % 3 == 0;
        self.review_pr_tick = self.review_pr_tick.wrapping_add(1);
        let sidebar = self.project_sidebar_view.clone();
        ctx.update_view(&sidebar, |ps, ps_ctx| {
            ps.review_pr.non_git_folder = false;
            if ps.review_pr.pr.is_none() {
                ps.review_pr.loading_pr = true;
            }
            if want_comments {
                ps.review_pr.loading_comments = true;
            }
            ps_ctx.notify();
        });
        let expected_root = root.clone();
        ctx.spawn(
            async move { query_review_pr_state(Some(root), want_comments) },
            move |view, queried, ctx| {
                // A slower response for a project that is no longer active
                // must never replace the newly selected project's Review UI.
                if view.review_pr_request_epoch != request_epoch
                    || view.active_review_git_root().as_ref() != Some(&expected_root)
                {
                    view.schedule_review_pr_poll(ctx);
                    return;
                }
                let sidebar = view.project_sidebar_view.clone();
                ctx.update_view(&sidebar, |ps, ps_ctx| {
                    ps.review_pr.pr = queried.pr.clone();
                    if let Some(comments) = queried.comments.as_ref() {
                        ps.review_pr.comments.clone_from(comments);
                    }
                    ps.review_pr.error = queried.error.clone();
                    ps.review_pr.non_git_folder = false;
                    ps.review_pr.loading_pr = false;
                    ps.review_pr.loading_comments = false;
                    ps_ctx.notify();
                });
                // Feed the tab-switch cache (comment ticks merge: a pr-only
                // tick must not wipe previously cached comments). Bounded:
                // evict a non-current root before growing past the cap.
                {
                    let root = expected_root;
                    // Bounded cache: evict one stale root before growing
                    // past the cap instead of accumulating projects forever.
                    if !view.review_pr_cache.contains_key(&root) && view.review_pr_cache.len() >= 16
                    {
                        let stale = view
                            .review_pr_cache
                            .keys()
                            .find(|cached| *cached != &root)
                            .cloned();
                        if let Some(stale) = stale {
                            view.review_pr_cache.remove(&stale);
                        }
                    }
                    let cached = view
                        .review_pr_cache
                        .entry(root)
                        .or_insert_with(CachedReviewPr::default);
                    cached.branch = view.review_ship.branch.clone();
                    cached.pr = queried.pr;
                    if let Some(comments) = queried.comments {
                        cached.comments = comments;
                    }
                    cached.error = queried.error;
                }
                view.schedule_review_pr_poll(ctx);
                ctx.notify();
            },
        );
        // Backfill one uncached sidebar worktree per tick so background rows
        // gain PR badges without a gh burst. The active root above stays authoritative.
        self.fill_sidebar_worktree_pr(ctx);
    }

    /// Spawn a no-comment PR query for the first sidebar worktree root missing
    /// from the cache. One root per call keeps background `gh` traffic flat.
    pub(crate) fn fill_sidebar_worktree_pr(&mut self, ctx: &mut ViewContext<Self>) {
        let missing = self
            .workspaces
            .iter()
            .map(|workspace| {
                workspace
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| workspace.root.clone())
            })
            .find(|root| !self.review_pr_cache.contains_key(root));
        if let Some(root) = missing {
            self.spawn_worktree_pr_refresh(root, ctx);
        }
    }

    pub(crate) fn schedule_review_pr_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if self.review_pr_poll_scheduled {
            return;
        }
        self.review_pr_poll_scheduled = true;
        ctx.spawn(Timer::after(Duration::from_secs(10)), |view, _, ctx| {
            view.review_pr_poll_scheduled = false;
            view.refresh_review_pr_state(ctx);
        });
    }

    pub(crate) fn arm_document_tree_watcher(&mut self, root: &Path) {
        if self.document_tree_watch_root.as_deref() == Some(root) {
            return;
        }
        self.stop_document_tree_watcher();
        if !root.is_dir() {
            return;
        }
        self.document_tree_watch_root = Some(root.to_path_buf());
        self.document_tree_watch_stop = Some(super::project_documents::spawn_tree_watcher(
            root.to_path_buf(),
            self.document_tree_watch_tx.clone(),
        ));
    }

    pub(crate) fn stop_document_tree_watcher(&mut self) {
        if let Some(stop) = self.document_tree_watch_stop.take() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.document_tree_watch_root = None;
    }

    pub(crate) fn refresh_document_tree_from_watcher(
        &mut self,
        event: super::project_documents::TreeWatcherEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        let root = self.document_index_root.clone();
        // Only the root match matters: a stale watcher must never clobber a
        // newer root. Listing refreshes regardless of trust — an external
        // `mkdir` from a terminal must appear in the tree even before the
        // folder is trusted. Trust gates execution, never visibility.
        if event.root != root || self.document_tree_watch_root.as_ref() != Some(&root) {
            return;
        }
        log::debug!(
            "[editor-tree] filesystem update root={} paths={}",
            root.display(),
            event.changed_paths.len()
        );
        self.refresh_document_tree(ctx);
    }

    pub(crate) fn refresh_document_tree(&mut self, ctx: &mut ViewContext<Self>) {
        let root = self.document_index_root.clone();
        if !root.is_dir() {
            return;
        }
        // Folder-only git state tracks external edits too (throttled inside
        // the model): a terminal `git` op or file change refreshes the panel
        // and bottom rail without waiting for the next tab event.
        if let (Some(handle), Some(model_root)) =
            (self.folder_git_model.clone(), self.folder_git_root.clone())
        {
            ctx.update_model(&handle, |model, mctx| model.refresh(model_root, mctx));
        }
        let (loaded_directories, reload_path) = self.project_sidebar_model.read(ctx, |model, _| {
            (
                model.loaded_directories(),
                model
                    .buffer
                    .as_ref()
                    .filter(|buffer| !buffer.is_dirty())
                    .map(|buffer| buffer.document().relative_path.clone()),
            )
        });
        self.document_tree_refresh_epoch = self.document_tree_refresh_epoch.wrapping_add(1);
        let epoch = self.document_tree_refresh_epoch;
        let expected_root = root.clone();
        let sidebar_model = self.project_sidebar_model.clone();
        ctx.spawn(
            async move {
                let index = super::project_documents::discover_loaded_directories(
                    &root,
                    &loaded_directories,
                );
                let preview = reload_path.as_ref().map(|relative_path| {
                    super::project_documents::read_preview(&root, relative_path)
                });
                (root, loaded_directories, index, preview)
            },
            move |view, (scanned_root, loaded_directories, index, preview), ctx| {
                if view.document_tree_refresh_epoch != epoch
                    || view.document_index_root != expected_root
                    || scanned_root != expected_root
                {
                    return;
                }
                log::debug!(
                    "[editor-tree] watcher refresh root={} entries={}",
                    scanned_root.display(),
                    index.entries().len()
                );
                view.document_index = index.clone();
                ctx.update_model(&sidebar_model, |model, model_ctx| {
                    model.set_documents(index, model_ctx);
                    for directory in loaded_directories {
                        model.mark_directory_loaded(directory);
                    }
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

    pub(crate) fn set_explorer_clipboard(
        &mut self,
        relative_path: PathBuf,
        mode: ExplorerClipboardMode,
        ctx: &mut ViewContext<Self>,
    ) {
        self.explorer_clipboard = Some(ExplorerClipboardEntry {
            relative_path: relative_path.clone(),
            mode,
        });
        if let Ok(path) =
            super::project_documents::absolute_entry_path(&self.document_index_root, &relative_path)
        {
            ctx.clipboard().write(ClipboardContent {
                plain_text: path.to_string_lossy().into_owned(),
                paths: Some(vec![path.to_string_lossy().into_owned()]),
                html: None,
                images: None,
            });
        }
        ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
            view.set_explorer_can_paste(true, view_ctx);
        });
        ctx.notify();
    }

    pub(crate) fn explorer_path_has_unsaved_changes(
        &self,
        relative_path: &Path,
        ctx: &ViewContext<Self>,
    ) -> bool {
        self.project_sidebar_model.read(ctx, |model, _| {
            model.working_copies().iter().any(|copy| {
                copy.relative_path == relative_path || copy.relative_path.starts_with(relative_path)
            })
        })
    }

    pub(crate) fn show_explorer_error(
        &mut self,
        error: impl Into<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.explorer_operation_error = Some(error.into());
        ctx.notify();
    }

    pub(crate) fn remap_open_document_prefix(
        &mut self,
        old_prefix: &Path,
        new_prefix: &Path,
        ctx: &mut ViewContext<Self>,
    ) {
        let remap = |path: &Path| {
            path.strip_prefix(old_prefix)
                .ok()
                .map(|suffix| new_prefix.join(suffix))
        };
        for path in &mut self.document_open_paths {
            if let Some(mapped) = remap(path) {
                *path = mapped;
            }
        }
        if let Some(path) = self.document_preview_path.clone() {
            if let Some(mapped) = remap(&path) {
                self.document_preview_path = Some(mapped);
            }
        }
        if let Some(path) = self.document_requested_path.clone() {
            if let Some(mapped) = remap(&path) {
                self.document_requested_path = Some(mapped.clone());
                self.open_document_path(mapped, false, ctx);
            }
        }
    }

    pub(crate) fn close_documents_under(
        &mut self,
        relative_path: &Path,
        ctx: &mut ViewContext<Self>,
    ) {
        let paths = self
            .document_open_paths
            .iter()
            .filter(|path| *path == relative_path || path.starts_with(relative_path))
            .cloned()
            .collect::<Vec<_>>();
        for path in paths {
            self.close_document_tab_now(&path, ctx);
        }
    }

    pub(crate) fn finish_explorer_mutation(
        &mut self,
        changed_path: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        self.explorer_operation_error = None;
        self.record_workspace_event(WorkspaceEventKind::FileChanged {
            relative_path: changed_path,
        });
        self.refresh_document_tree(ctx);
        ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
            model.refresh_git_status(model_ctx);
        });
        ctx.notify();
    }

    pub(crate) fn sync_inline_explorer_creation(&self, ctx: &mut ViewContext<Self>) {
        let creation = match &self.explorer_prompt {
            Some(super::view_overlays::ExplorerPrompt::Create {
                relative_parent,
                directory,
            }) => Some((
                relative_parent.clone(),
                *directory,
                self.explorer_prompt_editor.clone(),
            )),
            _ => None,
        };
        ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
            model.explorer_creation = creation;
            model.explorer_creation_error = self.explorer_operation_error.clone();
            model_ctx.emit(super::project_sidebar::SidebarEvent::Changed);
        });
    }

    pub(crate) fn perform_explorer_rename(
        &mut self,
        source: PathBuf,
        destination: PathBuf,
        update_imports_locally: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(name) = destination
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            self.explorer_rename_pending = false;
            self.show_explorer_error("The new file name is invalid.", ctx);
            return;
        };
        let root = self.document_index_root.clone();
        let operation_root = root.clone();
        let protected_paths = self
            .project_sidebar_model
            .read(ctx, |model, _| model.working_copies())
            .into_iter()
            .map(|copy| copy.relative_path)
            .collect::<HashSet<_>>();
        ctx.spawn(
            async move {
                let result = (|| {
                    let changed_imports = if update_imports_locally {
                        super::project_documents::update_imports_for_rename(
                            &operation_root,
                            &source,
                            &destination,
                            &protected_paths,
                        )?
                    } else {
                        Vec::new()
                    };
                    let new_path =
                        super::project_documents::rename_entry(&operation_root, &source, &name)?;
                    let refreshed_imports = changed_imports
                        .into_iter()
                        .map(|path| {
                            let path = path
                                .strip_prefix(&source)
                                .ok()
                                .map(|suffix| new_path.join(suffix))
                                .unwrap_or(path);
                            super::project_documents::read_preview(&operation_root, &path)
                        })
                        .collect::<Vec<_>>();
                    Ok((new_path, refreshed_imports))
                })();
                (source, destination, result)
            },
            move |view, (source, destination, result), ctx| match result {
                Ok((new_path, refreshed_imports)) => {
                    view.explorer_rename_pending = false;
                    view.explorer_prompt = None;
                    if let Some(client) = view.language_services.as_ref() {
                        if let Err(error) =
                            client.did_rename(root.clone(), source.clone(), new_path.clone())
                        {
                            log::warn!("[lsp] could not queue didRename notification: {error}");
                        }
                    }
                    view.remap_open_document_prefix(&source, &new_path, ctx);
                    for preview in refreshed_imports {
                        let text = preview.text.clone();
                        let reloaded = ctx
                            .update_model(&view.project_sidebar_model, |model, model_ctx| {
                                model.reload_clean_document(&preview, model_ctx)
                            });
                        if reloaded {
                            ctx.update_view(&view.project_sidebar_view, |sidebar, sidebar_ctx| {
                                sidebar.reload_saved_document_text(&text, sidebar_ctx);
                            });
                        }
                    }
                    view.finish_explorer_mutation(new_path, ctx);
                }
                Err(error) => {
                    view.explorer_rename_pending = false;
                    view.explorer_operation_error = Some(error);
                    ctx.notify();
                }
            },
        );
    }
}
