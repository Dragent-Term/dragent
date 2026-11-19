//! Workspace explorer actions.
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
    pub(crate) fn handle_explorer_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ExplorerCopyEntry { relative_path } => {
                self.set_explorer_clipboard(
                    relative_path.clone(),
                    ExplorerClipboardMode::Copy,
                    ctx,
                );
            }
            WorkspaceAction::ExplorerCutEntry { relative_path } => {
                if self.explorer_path_has_unsaved_changes(relative_path, ctx) {
                    self.show_explorer_error(
                        "Save or close unsaved files before moving this entry.",
                        ctx,
                    );
                } else {
                    self.set_explorer_clipboard(
                        relative_path.clone(),
                        ExplorerClipboardMode::Cut,
                        ctx,
                    );
                }
            }
            WorkspaceAction::ExplorerPasteInto { relative_directory } => {
                let Some(clipboard) = self.explorer_clipboard.clone() else {
                    self.show_explorer_error("Nothing has been copied or cut.", ctx);
                    return;
                };
                if clipboard.mode == ExplorerClipboardMode::Cut
                    && self.explorer_path_has_unsaved_changes(&clipboard.relative_path, ctx)
                {
                    self.show_explorer_error(
                        "Save or close unsaved files before moving this entry.",
                        ctx,
                    );
                    return;
                }
                let root = self.document_index_root.clone();
                let destination = relative_directory.clone();
                let source = clipboard.relative_path.clone();
                ctx.spawn(
                    async move {
                        let result = match clipboard.mode {
                            ExplorerClipboardMode::Copy => {
                                super::project_documents::copy_entry(&root, &source, &destination)
                            }
                            ExplorerClipboardMode::Cut => {
                                super::project_documents::move_entry(&root, &source, &destination)
                            }
                        };
                        (clipboard, result)
                    },
                    |view, (clipboard, result), ctx| match result {
                        Ok(new_path) => {
                            if clipboard.mode == ExplorerClipboardMode::Cut {
                                view.remap_open_document_prefix(
                                    &clipboard.relative_path,
                                    &new_path,
                                    ctx,
                                );
                                view.explorer_clipboard = None;
                                ctx.update_view(
                                    &view.project_sidebar_view,
                                    |sidebar, sidebar_ctx| {
                                        sidebar.set_explorer_can_paste(false, sidebar_ctx)
                                    },
                                );
                            }
                            view.finish_explorer_mutation(new_path, ctx);
                        }
                        Err(error) => view.show_explorer_error(error, ctx),
                    },
                );
            }
            WorkspaceAction::ExplorerDuplicateEntry { relative_path } => {
                let root = self.document_index_root.clone();
                let source = relative_path.clone();
                let destination = source.parent().unwrap_or(Path::new("")).to_path_buf();
                ctx.spawn(
                    async move {
                        super::project_documents::copy_entry(&root, &source, &destination)
                    },
                    |view, result, ctx| match result {
                        Ok(path) => view.finish_explorer_mutation(path, ctx),
                        Err(error) => view.show_explorer_error(error, ctx),
                    },
                );
            }
            WorkspaceAction::ExplorerStartRename { relative_path } => {
                if self.explorer_path_has_unsaved_changes(relative_path, ctx) {
                    self.show_explorer_error(
                        "Save or close unsaved files before renaming this entry.",
                        ctx,
                    );
                    return;
                }
                let name = relative_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_string();
                self.explorer_operation_error = None;
                self.explorer_rename_pending = false;
                self.explorer_prompt = Some(ExplorerPrompt::Rename {
                    relative_path: relative_path.clone(),
                });
                self.sync_inline_explorer_creation(ctx);
                ctx.update_view(&self.explorer_prompt_editor, |editor, editor_ctx| {
                    editor.set_text(&name, editor_ctx);
                    editor_ctx.focus_self();
                });
                ctx.notify();
            }
            WorkspaceAction::ExplorerCommitPrompt => {
                if self.explorer_rename_pending {
                    return;
                }
                let Some(prompt) = self.explorer_prompt.clone() else {
                    return;
                };
                let name = self
                    .explorer_prompt_editor
                    .read(ctx, |editor, _| editor.get_text().trim().to_string());
                if name.is_empty() {
                    self.explorer_operation_error = Some("Enter a name.".to_string());
                    self.sync_inline_explorer_creation(ctx);
                    ctx.notify();
                    return;
                }
                let root = self.document_index_root.clone();
                if let ExplorerPrompt::Rename { relative_path } = &prompt {
                    let destination = match super::project_documents::rename_entry_destination(
                        &root,
                        &relative_path,
                        &name,
                    ) {
                        Ok(destination) => destination,
                        Err(error) => {
                            self.explorer_operation_error = Some(error);
                            ctx.notify();
                            return;
                        }
                    };
                    if destination == *relative_path {
                        self.explorer_prompt = None;
                        self.explorer_operation_error = None;
                        ctx.notify();
                        return;
                    }
                    let supports_import_updates = self
                        .language_file_rename_servers
                        .iter()
                        .any(|key| key.project_root == root);
                    if supports_import_updates {
                        self.flush_language_document_change(ctx);
                        let request = self.language_services.as_ref().map(|client| {
                            client.will_rename(
                                root.clone(),
                                relative_path.clone(),
                                destination.clone(),
                            )
                        });
                        if let Some(Ok(token)) = request {
                            self.explorer_rename_pending = true;
                            self.language_request_tokens.insert(
                                token,
                                LanguageRequestPurpose::FileRename {
                                    source: relative_path.clone(),
                                    destination,
                                },
                            );
                            ctx.notify();
                            return;
                        }
                    }
                    self.perform_explorer_rename(relative_path.clone(), destination, true, ctx);
                    return;
                }
                self.explorer_rename_pending = true;
                ctx.spawn(
                    async move {
                        let result = match &prompt {
                            ExplorerPrompt::Create {
                                relative_parent,
                                directory,
                            } => super::project_documents::create_entry(
                                &root,
                                relative_parent,
                                &name,
                                *directory,
                            ),
                            ExplorerPrompt::Rename { .. } => unreachable!(),
                        };
                        (prompt, result)
                    },
                    |view, (prompt, result), ctx| match result {
                        Ok(new_path) => {
                            view.explorer_rename_pending = false;
                            view.explorer_prompt = None;
                            view.sync_inline_explorer_creation(ctx);
                            view.finish_explorer_mutation(new_path, ctx);
                        }
                        Err(error) => {
                            view.explorer_rename_pending = false;
                            view.explorer_operation_error = Some(error);
                            view.sync_inline_explorer_creation(ctx);
                            ctx.notify();
                        }
                    },
                );
            }
            WorkspaceAction::ExplorerCancelPrompt => {
                if self.explorer_rename_pending {
                    return;
                }
                self.explorer_prompt = None;
                self.explorer_operation_error = None;
                self.sync_inline_explorer_creation(ctx);
                ctx.notify();
            }
            WorkspaceAction::ExplorerConfirmDelete => {
                let Some((relative_path, _)) = self.pending_explorer_delete.clone() else {
                    return;
                };
                let root = self.document_index_root.clone();
                let path_for_result = relative_path.clone();
                ctx.spawn(
                    async move {
                        (
                            relative_path.clone(),
                            super::project_documents::delete_entry(&root, &relative_path),
                        )
                    },
                    move |view, (relative_path, result), ctx| match result {
                        Ok(()) => {
                            view.pending_explorer_delete = None;
                            view.close_documents_under(&relative_path, ctx);
                            view.finish_explorer_mutation(path_for_result.clone(), ctx);
                        }
                        Err(error) => view.show_explorer_error(error, ctx),
                    },
                );
            }
            WorkspaceAction::ExplorerCancelDelete => {
                self.pending_explorer_delete = None;
                self.explorer_operation_error = None;
                ctx.notify();
            }
            WorkspaceAction::ExplorerDismissError => {
                self.explorer_operation_error = None;
                ctx.notify();
            }
            WorkspaceAction::ExplorerRefresh => self.refresh_document_tree(ctx),
            WorkspaceAction::ExplorerCopyPath {
                relative_path,
                absolute,
            } => {
                let value = if *absolute {
                    super::project_documents::absolute_entry_path(
                        &self.document_index_root,
                        relative_path,
                    )
                    .map(|path| path.to_string_lossy().into_owned())
                } else {
                    Ok(relative_path.to_string_lossy().into_owned())
                };
                match value {
                    Ok(value) => ctx.clipboard().write(ClipboardContent::plain_text(value)),
                    Err(error) => self.show_explorer_error(error, ctx),
                }
            }
            WorkspaceAction::ExplorerStartCreate {
                relative_parent,
                directory,
            } => {
                if self.explorer_rename_pending {
                    return;
                }
                self.explorer_operation_error = None;
                self.explorer_prompt = Some(ExplorerPrompt::Create {
                    relative_parent: relative_parent.clone(),
                    directory: *directory,
                });
                ctx.update_view(&self.explorer_prompt_editor, |editor, editor_ctx| {
                    editor.set_text("untitled", editor_ctx);
                    editor.select_all(editor_ctx);
                    editor_ctx.focus_self();
                });
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    for ancestor in relative_parent.ancestors().filter(|path| !path.as_os_str().is_empty()) {
                        model.expanded_directories.insert(ancestor.to_path_buf());
                    }
                    model_ctx.emit(super::project_sidebar::SidebarEvent::Changed);
                });
                self.sync_inline_explorer_creation(ctx);
                self.refresh_document_tree(ctx);
                ctx.notify();
            }
            WorkspaceAction::ExplorerRequestDelete {
                relative_path,
                directory,
            } => {
                if self.explorer_path_has_unsaved_changes(relative_path, ctx) {
                    self.show_explorer_error(
                        "Save or close unsaved files before deleting this entry.",
                        ctx,
                    );
                } else {
                    self.explorer_operation_error = None;
                    self.pending_explorer_delete = Some((relative_path.clone(), *directory));
                    ctx.notify();
                }
            }
            _ => {}
        }
    }
}
