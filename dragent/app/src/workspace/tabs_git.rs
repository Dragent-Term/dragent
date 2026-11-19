//! Folder-git status methods for workspace tabs.
//!
//! Responsible for: syncing folder git models, file/bulk ops,
//! discard arming, git command spawning.
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

use super::project_naming::default_pane_cwd;
use super::view::PaneTab;
use super::view::WorkspaceView;
use super::view::FOLDER_GIT_DISCARD_ARM_SECS;
use super::view_state::{FolderGitBulkOp, FolderGitFileOp};

impl WorkspaceView {
    /// Keeps a workspace-owned git status model for folder-only state. A
    /// terminal tab owns its status model; when no terminal tab carries a
    /// project root, the selected folder gets its own so the sidebar and
    /// bottom rail stay populated instead of going blank.
    pub(crate) fn sync_folder_git_model(&mut self, ctx: &mut ViewContext<Self>) {
        let terminal_owns_git = self.tabs.iter().any(|tab| {
            matches!(&tab.pane, WorkspacePane::Terminal(_)) && tab.project_root.is_some()
        });
        let active_terminal_owns_context = !self.project_start_visible
            && !self.document_center_open
            && self
                .tabs
                .iter()
                .find(|tab| tab.id == self.active_tab_id)
                .is_some_and(|tab| self.git_terminal_for(tab).is_some());
        if terminal_owns_git || active_terminal_owns_context {
            self.folder_git_model = None;
            self.folder_git_root = None;
            return;
        }
        let Some(root) = self.selected_project_root()
            .filter(|root| root.is_dir())
        else {
            self.folder_git_model = None;
            self.folder_git_root = None;
            return;
        };
        if self.folder_git_root.as_deref() == Some(root.as_path())
            && self.folder_git_model.is_some()
        {
            if let Some(handle) = self.folder_git_model.clone() {
                ctx.update_model(&handle, |model, mctx| model.refresh(root.clone(), mctx));
            }
            return;
        }
        let handle =
            ctx.add_model(|mctx| crate::git_status::GitRepoStatusModel::new(root.clone(), mctx));
        ctx.subscribe_to_model(&handle, |view, _, event, ctx| {
            // Live panel: repaint the moment the folder status lands, so
            // external changes and completed ops reflect without another
            // click. Mirrors the terminal-owned subscription.
            if matches!(event, crate::git_status::GitRepoStatusEvent::Updated { .. }) {
                // Keep explorer markers in step with the new index state.
                view.refresh_document_tree(ctx);
                ctx.notify();
            }
        });
        self.folder_git_model = Some(handle);
        self.folder_git_root = Some(root);
    }

    /// Per-file git operation kinds for folder-only state (no terminal).
    /// Two-click discard arm lives on the shared panel state so the row
    /// renders "click again to confirm".
    pub(crate) fn run_folder_git_file_op(
        &mut self,
        ctx: &mut ViewContext<Self>,
        rel_path: String,
        op: FolderGitFileOp,
    ) {
        let Some(handle) = self.folder_git_model.clone() else {
            return;
        };
        match op {
            FolderGitFileOp::StageToggle => {
                let unstaging = handle.read(ctx, |model, _| {
                    model.status.as_ref().is_some_and(|status| {
                        status.entries.iter().any(|entry| {
                            entry.rel_path == rel_path && entry.index != ' ' && entry.index != '?'
                        })
                    })
                });
                let args = if unstaging {
                    vec![
                        "restore".to_string(),
                        "--staged".to_string(),
                        "--".to_string(),
                        rel_path,
                    ]
                } else {
                    vec!["add".to_string(), "--".to_string(), rel_path]
                };
                self.spawn_folder_git_command(ctx, args, "Git stage".to_string());
            }
            FolderGitFileOp::Discard => {
                let key = format!("file:{rel_path}");
                if !self.arm_folder_git_discard(&key, ctx) {
                    return;
                }
                self.spawn_folder_git_command(
                    ctx,
                    vec![
                        "restore".to_string(),
                        "--source=HEAD".to_string(),
                        "--staged".to_string(),
                        "--worktree".to_string(),
                        "--".to_string(),
                        rel_path,
                    ],
                    "Git discard".to_string(),
                );
            }
        }
    }

    /// Bulk stage/unstage/discard-all for folder-only state.
    pub(crate) fn run_folder_git_bulk_op(
        &mut self,
        ctx: &mut ViewContext<Self>,
        op: FolderGitBulkOp,
    ) {
        match op {
            FolderGitBulkOp::StageAll => {
                self.spawn_folder_git_command(
                    ctx,
                    vec!["add".to_string(), ".".to_string()],
                    "Git stage all".to_string(),
                );
            }
            FolderGitBulkOp::UnstageAll => {
                self.spawn_folder_git_command(
                    ctx,
                    vec![
                        "restore".to_string(),
                        "--staged".to_string(),
                        ".".to_string(),
                    ],
                    "Git unstage all".to_string(),
                );
            }
            FolderGitBulkOp::DiscardAllStaged => {
                let Some(handle) = self.folder_git_model.clone() else {
                    return;
                };
                let staged: Vec<String> = handle.read(ctx, |model, _| {
                    model
                        .status
                        .as_ref()
                        .map(|status| status.staged_rel_paths.clone())
                        .unwrap_or_default()
                });
                if staged.is_empty() {
                    return;
                }
                if !self.arm_folder_git_discard("all:staged", ctx) {
                    return;
                }
                let mut args = vec![
                    "restore".to_string(),
                    "--source=HEAD".to_string(),
                    "--staged".to_string(),
                    "--worktree".to_string(),
                    "--".to_string(),
                ];
                args.extend(staged);
                self.spawn_folder_git_command(ctx, args, "Git discard staged".to_string());
            }
            FolderGitBulkOp::DiscardAllUnstaged => {
                if !self.arm_folder_git_discard("all:unstaged", ctx) {
                    return;
                }
                self.spawn_folder_git_command(
                    ctx,
                    vec![
                        "restore".to_string(),
                        "--source=HEAD".to_string(),
                        "--staged".to_string(),
                        "--worktree".to_string(),
                        ".".to_string(),
                    ],
                    "Git discard all".to_string(),
                );
            }
        }
    }

    /// Two-click arm for destructive folder-only git ops. First call arms and
    /// returns false (button shows "click again to confirm"); a fresh second
    /// call disarms and returns true. Arms expire after a few seconds.
    pub(crate) fn arm_folder_git_discard(
        &mut self,
        key: &str,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let armed = self
            .git_panel_state
            .borrow()
            .armed_discard
            .clone()
            .is_some_and(|(armed_key, armed_at)| {
                armed_key == key
                    && armed_at.elapsed()
                        < std::time::Duration::from_secs(FOLDER_GIT_DISCARD_ARM_SECS)
            });
        if !armed {
            self.git_panel_state.borrow_mut().armed_discard =
                Some((key.to_string(), std::time::Instant::now()));
            ctx.notify();
            return false;
        }
        self.git_panel_state.borrow_mut().armed_discard = None;
        true
    }

    /// Repo root backing folder-only git commands: the folder model's cwd,
    /// falling back to the panel cwd.
    pub(crate) fn folder_git_cwd(&self) -> Option<PathBuf> {
        self.folder_git_root
            .clone()
            .filter(|root| root.is_dir())
            .or_else(|| {
                self.git_panel_state
                    .borrow()
                    .cwd
                    .clone()
                    .filter(|root| root.is_dir())
            })
    }

    /// Run a `git` argv from the repo root off-thread, then refresh the
    /// folder status (bypass throttle) and the tree. Errors surface as a
    /// transient bottom-rail notice instead of silently reverting the click.
    /// Source Control commit (and push) when no terminal owns the
    /// repository — a project shown only in a browser pane, or folder-only
    /// state. Commits what is staged in the index, then pushes the branch.
    pub(crate) fn commit_folder_git(
        &mut self,
        message: String,
        push: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        use crate::git_status::GitOperation;
        let (Some(cwd), Some(handle)) = (self.folder_git_cwd(), self.folder_git_model.clone())
        else {
            self.show_ports_notice("No Git repository found for this workspace.".to_string(), ctx);
            return;
        };
        let message = message.trim().to_string();
        let staged = handle.read(ctx, |model, _| {
            model.status.as_ref().is_some_and(|status| {
                status
                    .entries
                    .iter()
                    .any(|entry| entry.index != ' ' && entry.index != '?')
            })
        });
        if staged && message.is_empty() {
            self.show_ports_notice("Write a commit message first.".to_string(), ctx);
            return;
        }
        if !staged && !push {
            self.show_ports_notice("Stage the changes to commit first.".to_string(), ctx);
            return;
        }
        let operation = if staged {
            GitOperation::Committing
        } else {
            GitOperation::Pushing
        };
        ctx.update_model(&handle, |model, model_ctx| {
            model.begin_operation(operation, model_ctx)
        });
        let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
        crate::bg::detach(move || {
            let root = crate::git_status::repo_root(&cwd);
            let run = |args: &[&str]| -> Result<(), String> {
                match crate::git_cmd::GitCommand::new()
                    .no_window()
                    .current_dir(&root)
                    .args(args)
                    .output()
                {
                    Ok(out) if out.status.success() => Ok(()),
                    Ok(out) => Err(format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    )
                    .trim()
                    .to_string()),
                    Err(error) => Err(error.to_string()),
                }
            };
            let mut result = if staged {
                run(&["commit", "-m", &message])
            } else {
                Ok(())
            };
            if result.is_ok() && push {
                let branch = crate::terminal::view::terminal_helpers::current_branch(
                    &root.to_string_lossy(),
                );
                result = run(&["push", "origin", &branch]);
            }
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, vctx| {
                if let Some(handle) = view.folder_git_model.clone() {
                    let refresh_cwd = handle.read(vctx, |model, _| model.cwd.clone());
                    let finished = result.clone();
                    vctx.update_model(&handle, |model, model_ctx| {
                        model.finish_operation(operation, finished, model_ctx);
                        model.refresh_bypass_throttle(refresh_cwd, model_ctx);
                    });
                }
                match result {
                    Ok(()) => {
                        log::info!(
                            "[source-control] {} completed (no terminal)",
                            if push { "commit & push" } else { "commit" }
                        );
                        let editor = view
                            .project_sidebar_view
                            .read(vctx, |sidebar, _| sidebar.git_commit_editor.clone());
                        vctx.update_view(&editor, |editor, editor_ctx| editor.set_text("", editor_ctx));
                    }
                    Err(error) => {
                        let label = if push { "Commit & push" } else { "Commit" };
                        view.show_ports_notice(format!("{label} failed: {error}"), vctx);
                    }
                }
                vctx.notify();
            },
            |_, _| {},
        );
    }

    /// Source Control pull or push when no terminal owns the repository:
    /// the current branch from or to `origin`, as a terminal would.
    pub(crate) fn sync_folder_git(&mut self, pull: bool, ctx: &mut ViewContext<Self>) {
        use crate::git_status::GitOperation;
        let (Some(cwd), Some(handle)) = (self.folder_git_cwd(), self.folder_git_model.clone())
        else {
            self.show_ports_notice("No Git repository found for this workspace.".to_string(), ctx);
            return;
        };
        let operation = if pull {
            GitOperation::Pulling
        } else {
            GitOperation::Pushing
        };
        ctx.update_model(&handle, |model, model_ctx| {
            model.begin_operation(operation, model_ctx)
        });
        let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
        crate::bg::detach(move || {
            let root = crate::git_status::repo_root(&cwd);
            let branch =
                crate::terminal::view::terminal_helpers::current_branch(&root.to_string_lossy());
            let verb = if pull { "pull" } else { "push" };
            let result = match crate::git_cmd::GitCommand::new()
                .no_window()
                .current_dir(&root)
                .args([verb, "origin", branch.as_str()])
                .output()
            {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => Err(format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
                .trim()
                .to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, vctx| {
                if let Some(handle) = view.folder_git_model.clone() {
                    let refresh_cwd = handle.read(vctx, |model, _| model.cwd.clone());
                    let finished = result.clone();
                    vctx.update_model(&handle, |model, model_ctx| {
                        model.finish_operation(operation, finished, model_ctx);
                        model.refresh_bypass_throttle(refresh_cwd, model_ctx);
                    });
                }
                if let Err(error) = result {
                    let label = if pull { "Pull" } else { "Push" };
                    view.show_ports_notice(format!("{label} failed: {error}"), vctx);
                }
                vctx.notify();
            },
            |_, _| {},
        );
    }

    /// Whether a terminal holds the repository the Source Control panel
    /// shows (else panel actions run from the workspace itself).
    pub(crate) fn terminal_owns_repository(&self) -> bool {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .is_some_and(|tab| self.git_terminal_for(tab).is_some())
    }

    pub(crate) fn spawn_folder_git_command(
        &mut self,
        ctx: &mut ViewContext<Self>,
        args: Vec<String>,
        err_label: String,
    ) {
        let Some(cwd) = self.folder_git_cwd() else {
            return;
        };
        if self.folder_git_model.is_none() {
            return;
        }
        let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
        crate::bg::detach(move || {
            let root = crate::git_status::repo_root(&cwd);
            let result = match crate::git_cmd::GitCommand::new()
                .no_window()
                .current_dir(&root)
                .args(&args)
                .output()
            {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    Err(if err.is_empty() {
                        format!("git {} failed", args.join(" "))
                    } else {
                        err
                    })
                }
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, vctx| {
                if let Err(err) = result {
                    log::warn!("[git] {err_label} failed: {err}");
                    view.show_ports_notice(format!("{err_label} failed: {err}"), vctx);
                }
                if let Some(handle) = view.folder_git_model.clone() {
                    let refresh_cwd = handle.read(vctx, |m, _| m.cwd.clone());
                    vctx.update_model(&handle, |model, mctx| {
                        model.refresh_bypass_throttle(refresh_cwd, mctx);
                    });
                }
                view.refresh_document_tree(vctx);
                vctx.notify();
            },
            |_, _| {},
        );
        ctx.notify();
    }
}
