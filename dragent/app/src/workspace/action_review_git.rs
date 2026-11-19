//! Workspace review and git actions.
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
    pub(crate) fn handle_review_git_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ToggleReviewMenu => {
                self.review_menu_open = !self.review_menu_open;
                ctx.notify();
            }
            WorkspaceAction::CloseReviewMenu => {
                if self.review_menu_open {
                    self.review_menu_open = false;
                    ctx.notify();
                }
            }
            WorkspaceAction::CreatePullRequest => {
                let Some(root) = self.review_ship.project_root.clone() else {
                    self.review_action_error =
                        Some("No git checkout selected for this workspace.".to_string());
                    ctx.notify();
                    return;
                };
                self.review_menu_open = true;
                self.review_action_busy = Some(ReviewGitAction::CreatePr);
                self.review_action_notice = None;
                self.review_action_error = None;
                ctx.notify();
                tick_review_menu_while_busy(ctx);
                ctx.spawn(
                    async move {
                        // Headless PR creation: an existing open PR opens
                        // instead of erroring, otherwise create
                        // non-interactively from the pushed commits.
                        let existing = gh_pr_url(&root);
                        if existing.is_some() {
                            return existing.map(|url| (url, true));
                        }
                        run_gh(&root, &["pr", "create", "--fill"])
                            .map(|output| (pr_url_from_output(&output).unwrap_or_default(), false))
                            .ok()
                    },
                    move |view, result, ctx| {
                        view.review_action_busy = None;
                        match result {
                            Some((url, existed)) if !url.is_empty() => {
                                view.review_action_notice = Some(if existed {
                                    "Opened the existing pull request.".to_string()
                                } else {
                                    "Pull request created.".to_string()
                                });
                                view.refresh_review_ship_state(ctx);
                                view.refresh_review_pr_state(ctx);
                                view.handle_action(
                                    &WorkspaceAction::OpenExternalUrl { url },
                                    ctx,
                                );
                            }
                            _ => {
                                view.review_action_error = Some(
                                    "Could not create the pull request. Check `gh auth status` and network access, then try again."
                                        .to_string(),
                                );
                            }
                        }
                        ctx.notify();
                    },
                );
            }
            WorkspaceAction::RefreshReviewPr => {
                self.refresh_review_pr_state(ctx);
            }
            WorkspaceAction::PushBranch => {
                let Some(root) = self.review_ship.project_root.clone() else {
                    self.review_action_error =
                        Some("No git checkout selected for this workspace.".to_string());
                    ctx.notify();
                    return;
                };
                let branch = self.review_ship.branch.clone();
                let args: Vec<String> = if self.review_ship.has_upstream || branch.is_empty() {
                    vec!["push".to_string()]
                } else {
                    vec![
                        "push".to_string(),
                        "-u".to_string(),
                        "origin".to_string(),
                        branch.clone(),
                    ]
                };
                self.review_menu_open = true;
                self.review_action_busy = Some(ReviewGitAction::Push);
                self.review_action_notice = None;
                self.review_action_error = None;
                ctx.notify();
                tick_review_menu_while_busy(ctx);
                ctx.spawn(
                    async move {
                        crate::git_cmd::GitCommand::new()
                            .args(&args)
                            .current_dir(&root)
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .no_window()
                            .output()
                            .map(|output| {
                                (
                                    output.status.success(),
                                    String::from_utf8_lossy(&output.stderr).into_owned(),
                                )
                            })
                            .unwrap_or((false, "Could not start git.".to_string()))
                    },
                    move |view, (success, stderr), ctx| {
                        view.review_action_busy = None;
                        if success {
                            view.review_action_notice = Some(if branch.is_empty() {
                                "Pushed.".to_string()
                            } else {
                                format!("Pushed {branch}.")
                            });
                        } else {
                            view.review_action_error = Some(push_error_hint(&stderr));
                        }
                        view.refresh_review_ship_state(ctx);
                        view.refresh_review_pr_state(ctx);
                        ctx.notify();
                    },
                );
            }
            WorkspaceAction::StageGitFile { rel_path } => {
                self.run_folder_git_file_op(ctx, rel_path.clone(), FolderGitFileOp::StageToggle);
            }
            WorkspaceAction::DiscardGitFile { rel_path } => {
                self.run_folder_git_file_op(ctx, rel_path.clone(), FolderGitFileOp::Discard);
            }
            WorkspaceAction::StageAllGitFiles => {
                self.run_folder_git_bulk_op(ctx, FolderGitBulkOp::StageAll);
            }
            WorkspaceAction::UnstageAllGitFiles => {
                self.run_folder_git_bulk_op(ctx, FolderGitBulkOp::UnstageAll);
            }
            WorkspaceAction::DiscardAllGitFiles { staged_only } => {
                self.run_folder_git_bulk_op(
                    ctx,
                    if *staged_only {
                        FolderGitBulkOp::DiscardAllStaged
                    } else {
                        FolderGitBulkOp::DiscardAllUnstaged
                    },
                );
            }
            WorkspaceAction::PullActiveGitBranch => {
                if !self.terminal_owns_repository() {
                    return self.sync_folder_git(true, ctx);
                }
                self.run_on_git_terminal(ctx, |pane, ctx| {
                    ctx.update_view(pane, |terminal, terminal_ctx| {
                        terminal.handle_action(
                            &crate::terminal::view::actions::TerminalAction::PullBranchChanges,
                            terminal_ctx,
                        );
                    });
                });
            }
            WorkspaceAction::PushActiveGitBranch => {
                if !self.terminal_owns_repository() {
                    return self.sync_folder_git(false, ctx);
                }
                self.run_on_git_terminal(ctx, |pane, ctx| {
                    ctx.update_view(pane, |terminal, terminal_ctx| {
                        terminal.handle_action(
                            &crate::terminal::view::actions::TerminalAction::PushBranchChanges,
                            terminal_ctx,
                        );
                    });
                });
            }
            WorkspaceAction::OpenGitDiff { rel_path, staged } => {
                // Same root resolution as open_document_path: the document
                // center's project, else the active terminal tab's cwd.
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
                    ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                        model.set_document_notice(
                            false,
                            "Unable to open diff location".to_string(),
                            model_ctx,
                        );
                    });
                    return;
                };
                let root = super::project_sidebar::find_git_root(&root).unwrap_or(root);
                // A diff click is an explicit request to see the editor, like
                // a file click from the explorer: uncover the work item
                // surface (which otherwise keeps covering the editor so the
                // click appears to do nothing) and restore a maximized dock.
                self.work_item_tab_active = false;
                self.document_terminal_maximized = false;
                self.show_document_center();
                self.document_tab_id = Some(self.active_tab_id);
                self.document_owner_root = Some(root.clone());
                self.document_index_root = root.clone();
                let review_path =
                    Self::git_file_diff_tab_path(std::path::Path::new(rel_path), *staged);
                let same_review = self.document_requested_path.as_ref() == Some(&review_path);
                if !self.document_open_paths.contains(&review_path) {
                    self.document_open_paths.push(review_path.clone());
                }
                self.document_requested_path = Some(review_path.clone());
                self.document_preview_path = None;
                self.reveal_document_tab(&review_path);
                let target = if *staged {
                    crate::git_status::diff_head::GitDiffTarget::Staged
                } else {
                    crate::git_status::diff_head::GitDiffTarget::Unstaged
                };
                let cached = self.project_sidebar_model.read(ctx, |model, _| {
                    model.cached_git_file_diff(std::path::Path::new(rel_path), target)
                });
                if !same_review {
                    ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                        view.prepare_git_diff(view_ctx);
                    });
                }
                if let Some(diff) = cached {
                    ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                        model.set_git_file_diff(diff, model_ctx);
                    });
                    return;
                }
                let rel = rel_path.clone();
                let rel_for_task = rel.clone();
                let staged = *staged;
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.set_workspace_root(root.clone());
                    model.begin_git_diff(review_path.clone(), model_ctx);
                });
                ctx.spawn(
                    async move {
                        let (tx, rx) = futures::channel::oneshot::channel();
                        crate::bg::detach(move || {
                            let result = crate::git_status::diff_head::fetch_structured_file_diff(
                                &root,
                                &rel_for_task,
                                if staged {
                                    crate::git_status::diff_head::GitDiffTarget::Staged
                                } else {
                                    crate::git_status::diff_head::GitDiffTarget::Unstaged
                                },
                            );
                            let _ = tx.send(result);
                        });
                        rx.await
                            .map_err(|_| "Git diff worker stopped unexpectedly".to_string())?
                    },
                    move |view, result, ctx| {
                        if view.document_requested_path.as_ref() != Some(&review_path) {
                            log::debug!(
                                "[git-diff] ignored stale result relative={} target={}",
                                rel,
                                if staged { "staged" } else { "working" }
                            );
                            return;
                        }
                        ctx.update_model(&view.project_sidebar_model, |model, model_ctx| {
                            match result {
                                Ok(diff) => model.set_git_file_diff(diff, model_ctx),
                                Err(error) => {
                                    log::warn!("[git-diff] failed to load {rel}: {error}");
                                    model.set_preview(
                                        super::project_documents::ReadOnlyDocument {
                                            relative_path: std::path::PathBuf::from(&rel),
                                            text: String::new(),
                                            visual_path: None,
                                            error: Some(format!(
                                                "Failed to load diff for {rel}:\n{error}"
                                            )),
                                            fingerprint: None,
                                        },
                                        model_ctx,
                                    );
                                }
                            }
                        });
                    },
                );
            }
            WorkspaceAction::OpenGitWorktreeDiff => {
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
                    return;
                };
                let root = super::project_sidebar::find_git_root(&root).unwrap_or(root);
                // Same uncovering as OpenGitDiff: a diff click must hide the
                // work item surface or the editor stays covered.
                self.work_item_tab_active = false;
                self.document_terminal_maximized = false;
                self.show_document_center();
                self.document_tab_id = Some(self.active_tab_id);
                self.document_owner_root = Some(root.clone());
                self.document_index_root = root.clone();
                let review_path = std::path::PathBuf::from(".git-diff/Uncommitted Changes");
                if !self.document_open_paths.contains(&review_path) {
                    self.document_open_paths.push(review_path.clone());
                }
                self.document_requested_path = Some(review_path.clone());
                self.document_preview_path = None;
                self.reveal_document_tab(&review_path);
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.prepare_git_diff(view_ctx);
                });
                let expected_review = review_path.clone();
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.set_workspace_root(root.clone());
                    model.begin_git_multi_diff(review_path, model_ctx);
                });
                ctx.spawn(
                    async move {
                        let (tx, rx) = futures::channel::oneshot::channel();
                        crate::bg::detach(move || {
                            let _ = tx.send(
                                crate::git_status::diff_head::fetch_structured_worktree_files(
                                    &root,
                                ),
                            );
                        });
                        rx.await
                            .map_err(|_| "Git status worker stopped unexpectedly".to_string())?
                    },
                    move |view, result, ctx| {
                        if view.document_requested_path.as_ref() != Some(&expected_review) {
                            return;
                        }
                        ctx.update_model(&view.project_sidebar_model, |model, model_ctx| {
                            match result {
                                Ok(diff) => model.set_git_multi_diff(diff, model_ctx),
                                Err(error) => model.set_preview(
                                    super::project_documents::ReadOnlyDocument {
                                        relative_path: std::path::PathBuf::from(
                                            ".git-diff/all-changes",
                                        ),
                                        text: String::new(),
                                        visual_path: None,
                                        error: Some(format!(
                                            "Failed to load working tree changes:\n{error}"
                                        )),
                                        fingerprint: None,
                                    },
                                    model_ctx,
                                ),
                            }
                        });
                    },
                );
            }
            WorkspaceAction::OpenWorkspaceAttemptChanges { attempt_id } => {
                let Some(attempt) = self.workspace_state.execution_attempt(*attempt_id).cloned()
                else {
                    self.shape_coordination_error = Some(
                        "This agent attempt is no longer available in the workspace history."
                            .to_string(),
                    );
                    ctx.notify();
                    return;
                };
                let Some(summary) = attempt.change_summary.clone() else {
                    self.shape_coordination_error = Some(
                        "This agent attempt has not produced a structured change set yet."
                            .to_string(),
                    );
                    ctx.notify();
                    return;
                };
                let Some(candidate_root) = self
                    .workspace_execution_runtime
                    .prepared_execution_root(*attempt_id)
                    .map(Path::to_path_buf)
                    .filter(|root| root.is_dir())
                else {
                    self.shape_coordination_error = Some(
                        "The isolated checkout for this change set is no longer available. Its durable file summary remains in Changes, but source review ends when the candidate checkout is retired."
                            .to_string(),
                    );
                    ctx.notify();
                    return;
                };
                self.shape_coordination_error = None;
                let task_title = self
                    .workspace_state
                    .latest_structured_plan()
                    .and_then(|plan| plan.tasks.iter().find(|task| task.id == attempt.task_id))
                    .map(|task| task.title.clone())
                    .unwrap_or_else(|| "Agent execution".to_string());
                let review_path = Self::workspace_attempt_diff_tab_path(*attempt_id);
                let short_base = summary.base_revision.chars().take(8).collect::<String>();
                let title = format!("Changes · {task_title}");
                let subtitle = format!(
                    "Agent attempt {} · base {} · read-only candidate",
                    attempt.attempt_number, short_base
                );
                self.show_document_center();
                self.work_item_tab_active = false;
                self.document_tab_id = Some(self.active_tab_id);
                self.document_owner_root = Some(self.workspace_state.project().root.clone());
                if !self.document_open_paths.contains(&review_path) {
                    self.document_open_paths.push(review_path.clone());
                }
                self.document_requested_path = Some(review_path.clone());
                self.document_preview_path = None;
                self.reveal_document_tab(&review_path);
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.prepare_git_diff(view_ctx);
                });
                let expected_review = review_path.clone();
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.begin_read_only_git_multi_diff(
                        review_path,
                        candidate_root.clone(),
                        title.clone(),
                        subtitle.clone(),
                        model_ctx,
                    );
                });
                ctx.spawn(
                    async move {
                        let (tx, rx) = futures::channel::oneshot::channel();
                        crate::bg::detach(move || {
                            let result =
                                crate::git_status::diff_head::fetch_structured_worktree_files(
                                    &candidate_root,
                                )
                                .map(|mut diff| {
                                    diff.title = title;
                                    diff.subtitle = Some(subtitle);
                                    diff.read_only = true;
                                    diff
                                });
                            let _ = tx.send(result);
                        });
                        rx.await.map_err(|_| {
                            "Agent change review worker stopped unexpectedly".to_string()
                        })?
                    },
                    move |view, result, ctx| {
                        if view.document_requested_path.as_ref() != Some(&expected_review) {
                            return;
                        }
                        ctx.update_model(&view.project_sidebar_model, |model, model_ctx| {
                            match result {
                                Ok(diff) => model.set_git_multi_diff(diff, model_ctx),
                                Err(error) => model.set_preview(
                                    super::project_documents::ReadOnlyDocument {
                                        relative_path: expected_review.clone(),
                                        text: String::new(),
                                        visual_path: None,
                                        error: Some(format!(
                                            "Failed to load agent changes:\n{error}"
                                        )),
                                        fingerprint: None,
                                    },
                                    model_ctx,
                                ),
                            }
                        });
                    },
                );
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::OpenGitCommitDiff { sha, short_sha } => {
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
                    return;
                };
                let root = super::project_sidebar::find_git_root(&root).unwrap_or(root);
                // Same uncovering as OpenGitDiff: a diff click must hide the
                // work item surface or the editor stays covered.
                self.work_item_tab_active = false;
                self.document_terminal_maximized = false;
                self.show_document_center();
                self.document_tab_id = Some(self.active_tab_id);
                self.document_owner_root = Some(root.clone());
                self.document_index_root = root.clone();
                ctx.update_view(&self.project_sidebar_view, |view, view_ctx| {
                    view.prepare_git_diff(view_ctx);
                });
                let sha = sha.clone();
                let sha_for_task = sha.clone();
                let short_sha = short_sha.clone();
                let review_path = std::path::PathBuf::from(format!(".git-history/{short_sha}"));
                if !self.document_open_paths.contains(&review_path) {
                    self.document_open_paths.push(review_path.clone());
                }
                self.document_requested_path = Some(review_path.clone());
                self.document_preview_path = None;
                self.reveal_document_tab(&review_path);
                let expected_review = review_path.clone();
                ctx.update_model(&self.project_sidebar_model, |model, model_ctx| {
                    model.set_workspace_root(root.clone());
                    model.begin_git_diff(review_path, model_ctx);
                });
                ctx.spawn(
                    async move {
                        let (tx, rx) = futures::channel::oneshot::channel();
                        crate::bg::detach(move || {
                            let _ = tx.send(crate::git_status::diff_head::fetch_commit_diff(
                                &root,
                                &sha_for_task,
                            ));
                        });
                        rx.await
                            .map_err(|_| "Git commit worker stopped unexpectedly".to_string())?
                    },
                    move |view, result, ctx| {
                        if view.document_requested_path.as_ref() != Some(&expected_review) {
                            return;
                        }
                        ctx.update_model(&view.project_sidebar_model, |model, model_ctx| {
                            match result {
                                Ok(diff) => model.set_git_multi_diff(diff, model_ctx),
                                Err(error) => model.set_preview(
                                    super::project_documents::ReadOnlyDocument {
                                        relative_path: std::path::PathBuf::from(format!(
                                            ".git-history/{short_sha}"
                                        )),
                                        text: String::new(),
                                        visual_path: None,
                                        error: Some(format!(
                                            "Failed to load commit {short_sha}:\n{error}"
                                        )),
                                        fingerprint: None,
                                    },
                                    model_ctx,
                                ),
                            }
                        });
                    },
                );
            }
            WorkspaceAction::CommitGitStaged { message, push } => {
                let message = message.clone();
                let push = *push;
                if !self.terminal_owns_repository() {
                    // No terminal holds this repository (a browser-only
                    // workspace, or folder-only state): commit from here.
                    return self.commit_folder_git(message, push, ctx);
                }
                self.run_on_git_terminal(ctx, |pane, ctx| {
                    // Hand the panel's message to the session's commit editor,
                    // then run the existing commit pipeline (which reads text
                    // + staged set from this session).
                    let action = if push {
                        crate::terminal::view::actions::TerminalAction::CommitAndPushBranch
                    } else {
                        crate::terminal::view::actions::TerminalAction::CommitBranchChanges
                    };
                    ctx.update_view(pane, |terminal, terminal_ctx| {
                        terminal.set_commit_message(&message, terminal_ctx);
                        // We are already inside the target TerminalView. A
                        // context dispatch starts at its parent and therefore
                        // skips this view (WarpUI reports "no view handled
                        // it"). Invoke the typed action handler directly so
                        // commit/push actually enters the terminal's Git
                        // pipeline and publishes its operation state.
                        log::info!(
                            "[source-control] starting {}",
                            if push { "commit & push" } else { "commit" }
                        );
                        terminal.handle_action(&action, terminal_ctx);
                    });
                });
            }
            WorkspaceAction::GenerateGitCommitMessage => {
                let started = ctx
                    .update_view(&self.project_sidebar_view, |sidebar, sidebar_ctx| {
                        sidebar.begin_git_commit_generation(sidebar_ctx)
                    });
                if !started {
                    return;
                }

                let Some(cwd) = self.git_panel_state.borrow().cwd.clone() else {
                    ctx.update_view(&self.project_sidebar_view, |sidebar, sidebar_ctx| {
                        sidebar.finish_git_commit_generation(
                            Err("No Git repository found".to_string()),
                            sidebar_ctx,
                        );
                    });
                    return;
                };
                // Commit AI toggle + agent selection.
                if !crate::settings::git_consumers::commit_ai_enabled() {
                    ctx.update_view(&self.project_sidebar_view, |sidebar, sidebar_ctx| {
                        sidebar.finish_git_commit_generation(
                            Err("Commit-message AI is disabled in Settings → Version Control.".to_string()),
                            sidebar_ctx,
                        );
                    });
                    return;
                }
                let agent_api = crate::agent::default_agent_api();
                let selected = crate::settings::AppSettings::load().selected_model;
                let model = crate::settings::git_consumers::commit_ai_model(&selected);
                ctx.spawn(
                    async move {
                        let (tx, rx) = futures::channel::oneshot::channel();
                        crate::bg::detach(move || {
                            let result = crate::git_status::commit_message::generate_for_repository(
                                &cwd, &agent_api, &model,
                            );
                            let _ = tx.send(result);
                        });
                        rx.await.unwrap_or_else(|_| {
                            Err("The commit-message worker stopped unexpectedly".to_string())
                        })
                    },
                    |view, result, ctx| {
                        ctx.update_view(&view.project_sidebar_view, |sidebar, sidebar_ctx| {
                            sidebar.finish_git_commit_generation(result, sidebar_ctx);
                        });
                    },
                );
            }
            WorkspaceAction::ConfirmDiscardGitHunk => {
                if let Some((rel_path, hunk_index, old_start, new_start)) =
                    self.pending_discard_hunk.take()
                {
                    let cwd = self
                        .project_sidebar_model
                        .read(ctx, |model, _| model.active_cwd.clone());
                    let status_handle = self.project_sidebar_model.clone();
                    if let Some(cwd) = cwd {
                        let refresh_cwd = cwd.clone();
                        let refresh_path = rel_path.clone();
                        ctx.spawn(
                            async move {
                                let operation =
                                    crate::git_status::hunk_stage::discard_hunk_matching(
                                        &cwd,
                                        &rel_path,
                                        hunk_index,
                                        Some((old_start, new_start)),
                                    );
                                let file = crate::git_status::diff_head::fetch_structured_file_diff(
                                    &refresh_cwd,
                                    &refresh_path,
                                    crate::git_status::diff_head::GitDiffTarget::Unstaged,
                                );
                                let multi =
                                    crate::git_status::diff_head::fetch_structured_worktree_files(
                                        &refresh_cwd,
                                    );
                                (operation, file, multi)
                            },
                            move |view, (operation, file, multi), ctx| {
                                if let Err(ref error) = operation {
                                    log::warn!("[git-hunk] discard failed: {error}");
                                }
                                ctx.update_model(&status_handle, |model, model_ctx| {
                                    model.refresh_git_status(model_ctx);
                                    if let Err(error) = &operation {
                                        model.set_document_notice(
                                            false,
                                            format!("Discard hunk failed: {error}"),
                                            model_ctx,
                                        );
                                    }
                                    if model.git_multi_diff.is_some() {
                                        if let Ok(diff) = multi {
                                            model.set_git_multi_diff(diff, model_ctx);
                                        }
                                    } else if model.git_file_diff.is_some() {
                                        if let Ok(diff) = file {
                                            model.set_git_file_diff(diff, model_ctx);
                                        }
                                    }
                                });
                                view.run_on_git_terminal(ctx, |pane, ctx| {
                                    let git_status =
                                        pane.read(ctx, |terminal, _| terminal.git_repo_status());
                                    let cwd = git_status.read(ctx, |model, _| model.cwd.clone());
                                    ctx.update_model(&git_status, |model, model_ctx| {
                                        model.refresh_bypass_throttle(cwd, model_ctx);
                                    });
                                });
                            },
                        );
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::CancelDiscardGitHunk => {
                self.pending_discard_hunk = None;
                ctx.notify();
            }
            WorkspaceAction::RequestDiscardGitHunk {
                rel_path,
                hunk_index,
                old_start,
                new_start,
            } => {
                self.pending_discard_hunk =
                    Some((rel_path.clone(), *hunk_index, *old_start, *new_start));
                ctx.notify();
            }
            WorkspaceAction::StageGitHunk {
                rel_path,
                hunk_index,
                old_start,
                new_start,
                staged,
            } => {
                let rel_path = rel_path.clone();
                let hunk_index = *hunk_index;
                let expected_starts = (*old_start, *new_start);
                let staged = *staged;
                let cwd = self
                    .project_sidebar_model
                    .read(ctx, |model, _| model.active_cwd.clone());
                let status_handle = self.project_sidebar_model.clone();
                if let Some(cwd) = cwd {
                    let refresh_cwd = cwd.clone();
                    let refresh_path = rel_path.clone();
                    ctx.spawn(
                        async move {
                            let hunks = crate::git_status::hunk_stage::stageable_hunks(
                                &cwd, &rel_path, staged,
                            );
                            let operation = hunks
                                .iter()
                                .find(|hunk| (hunk.old_start, hunk.new_start) == expected_starts)
                                .or_else(|| hunks.get(hunk_index))
                                .ok_or_else(|| "Git hunk no longer exists".to_string())
                                .and_then(|hunk| {
                                    if staged {
                                        crate::git_status::hunk_stage::unstage_hunk(
                                            &cwd, &rel_path, &hunk,
                                        )
                                    } else {
                                        crate::git_status::hunk_stage::stage_hunk(
                                            &cwd, &rel_path, &hunk,
                                        )
                                    }
                                });
                            let target = if staged {
                                crate::git_status::diff_head::GitDiffTarget::Staged
                            } else {
                                crate::git_status::diff_head::GitDiffTarget::Unstaged
                            };
                            let file = crate::git_status::diff_head::fetch_structured_file_diff(
                                &refresh_cwd,
                                &refresh_path,
                                target,
                            );
                            let multi =
                                crate::git_status::diff_head::fetch_structured_worktree_files(
                                    &refresh_cwd,
                                );
                            (operation, file, multi)
                        },
                        move |view, (operation, file, multi), ctx| {
                            if let Err(ref error) = operation {
                                log::warn!("[git-hunk] operation failed: {error}");
                            }
                            ctx.update_model(&status_handle, |model, model_ctx| {
                                model.refresh_git_status(model_ctx);
                                if let Err(error) = &operation {
                                    model.set_document_notice(
                                        false,
                                        format!("Git hunk action failed: {error}"),
                                        model_ctx,
                                    );
                                }
                                if model.git_multi_diff.is_some() {
                                    if let Ok(diff) = multi {
                                        model.set_git_multi_diff(diff, model_ctx);
                                    }
                                } else if model.git_file_diff.is_some() {
                                    if let Ok(diff) = file {
                                        model.set_git_file_diff(diff, model_ctx);
                                    }
                                }
                            });
                            view.run_on_git_terminal(ctx, |pane, ctx| {
                                let git_status =
                                    pane.read(ctx, |terminal, _| terminal.git_repo_status());
                                let cwd = git_status.read(ctx, |model, _| model.cwd.clone());
                                ctx.update_model(&git_status, |model, model_ctx| {
                                    model.refresh_bypass_throttle(cwd, model_ctx);
                                });
                            });
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

/// Repaint the open Review menu while a headless action runs so its
/// progress spinner animates. Stops itself when the action clears.
fn tick_review_menu_while_busy(ctx: &mut ViewContext<WorkspaceView>) {
    ctx.spawn(Timer::after(Duration::from_millis(120)), |view, _, ctx| {
        if view.review_action_busy.is_some() {
            ctx.notify();
            tick_review_menu_while_busy(ctx);
        }
    });
}

/// Headless git/GitHub action behind the Review menu. These never open a
/// terminal — the menu tracks busy/result state instead of opening a shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewGitAction {
    Push,
    CreatePr,
}

/// Run `gh` headlessly (same env discipline as the review service).
fn run_gh(root: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = crate::process_ext::gh_command()
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PAGER", "")
        .no_window()
        .output()
        .map_err(|error| format!("Could not start gh: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// URL of the current branch's open PR, if any (open the existing PR
/// instead of erroring when one is already there).
fn gh_pr_url(root: &std::path::Path) -> Option<String> {
    run_gh(root, &["pr", "view", "--json", "url", "--jq", ".url"])
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| url.starts_with("https://"))
}

/// First `https://` URL in `gh pr create` output (it prints the new PR URL).
fn pr_url_from_output(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|token| token.starts_with("https://"))
        .map(str::to_string)
}

/// Friendly push failure: behind-upstream suggests pulling first instead of
/// dumping raw git stderr (the notice carries the same guidance without a
/// force-push footgun).
fn push_error_hint(stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if lower.contains("behind")
        || lower.contains("non-fast-forward")
        || lower.contains("fetch first")
    {
        "Push rejected: the remote has commits you don't. Pull or rebase first, then push again."
            .to_string()
    } else if stderr.trim().is_empty() {
        "Push failed.".to_string()
    } else {
        stderr.trim().chars().take(300).collect()
    }
}
