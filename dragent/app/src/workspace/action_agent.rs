//! Workspace agent and coordination actions.
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
    pub(crate) fn handle_agent_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::SelectWorkspaceAgentTab(id) => {
                self.select_workspace_agent_tab(*id, ctx);
            }
            WorkspaceAction::CloseWorkspaceAgentTab(id) => {
                let was_active = self
                    .active_workspace_agent_tab()
                    .is_some_and(|tab| tab.id == *id);
                self.workspace_agent_tabs.retain(|tab| tab.id != *id);
                if was_active {
                    self.workspace_active_agent_tab = None;
                    self.work_item_tab_active = false;
                }
                ctx.notify();
            }
            WorkspaceAction::CloseWorkItemTab => {
                self.work_item_tab_open = false;
                if self.active_workspace_agent_tab().is_none() {
                    if let Some(id) = self
                        .workspace_agent_tabs
                        .iter()
                        .find(|tab| tab.id.work_item_id == self.workspace_work_item_id)
                        .map(|tab| tab.id)
                    {
                        self.workspace_active_agent_tab = Some(id);
                        self.work_item_tab_active = true;
                    } else {
                        self.work_item_tab_active = false;
                        if self.document_open_paths.is_empty() {
                            self.document_center_open = false;
                        }
                    }
                }
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::SelectWorkItemView(view) => {
                // Gate on the pane existing, NOT on `work_item_tab_active`.
                // That flag means "the work item is the focused editor tab",
                // which the split layout retired: work now owns its own pane
                // beside the editor, and `open_document_path_at_root` clears
                // the flag on every file open. Keeping it here made the
                // Plan/Coordinate/Knowledge/Review routes dead the moment a
                // file was opened, even though their pane was right there.
                // `is_work_item_pane` gates rendering on `_open` alone; this
                // now matches it.
                if !self.work_item_tab_open {
                    return;
                }
                self.workspace_active_agent_tab = None;
                self.work_item_view = *view;
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::ApprovePlanAndCoordinate => {
                self.approve_plan_and_coordinate(ctx);
            }
            WorkspaceAction::AlwaysAllowWorkspaceTool { attention_item_id } => {
                self.always_allow_workspace_tool(*attention_item_id, ctx)
            }
            WorkspaceAction::TerminalConfirmChanged => {
                log::debug!("[workspace] terminal confirm changed; refreshing mirror");
                ctx.notify();
            }
            WorkspaceAction::ToggleWorkspaceAttentionBell => {
                self.attention_bell_open = !self.attention_bell_open;
                log::info!(
                    "[workspace] attention bell toggled open={}",
                    self.attention_bell_open
                );
                if !self.attention_bell_open {
                    self.attention_popover_item = None;
                }
                self.attention_response_text.clear();
                ctx.update_view(&self.attention_response_editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::SelectWorkspaceAttentionItem { attention_item_id } => {
                log::info!(
                    "[workspace] attention item selected item_id={:?}",
                    attention_item_id
                );
                self.attention_popover_item = *attention_item_id;
                self.attention_bell_open = true;
                self.attention_response_text.clear();
                ctx.update_view(&self.attention_response_editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::RouteToWorkspaceAttention { attention_item_id } => {
                self.route_to_workspace_attention(*attention_item_id, ctx)
            }
            WorkspaceAction::RestoreWorkspaceCheckpoint { attempt_id } => {
                self.restore_workspace_checkpoint(*attempt_id, ctx)
            }
            WorkspaceAction::ToggleWorkspaceAgentTimelineExpanded => {
                if let Some(id) = self.active_workspace_agent_tab().map(|tab| tab.id) {
                    if let Some(index) =
                        super::editor_tabs::agent_tab_index(&self.workspace_agent_tabs, id)
                    {
                        let tab = &mut self.workspace_agent_tabs[index];
                        tab.expanded = !tab.expanded;
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::WaiveWorkspaceCriterion {
                acceptance_criterion_id,
            } => self.waive_workspace_criterion(*acceptance_criterion_id, ctx),
            WorkspaceAction::CancelWorkspaceExecution => self.cancel_workspace_execution(ctx),
            WorkspaceAction::ToggleWorkspaceExecutionPause => {
                self.toggle_workspace_execution_pause(ctx)
            }
            WorkspaceAction::DismissCoordinationError => {
                if self.shape_coordination_error.take().is_some() {
                    ctx.notify();
                }
            }
            WorkspaceAction::StopPrincipalTurn => {
                self.stop_principal_turn(ctx);
            }
            WorkspaceAction::OpenWorkspaceAgentTimeline {
                task_id,
                attempt_id,
            } => self.open_workspace_agent_timeline(*task_id, *attempt_id, ctx),
            WorkspaceAction::ResolveWorkspaceAttention {
                attention_item_id,
                approve,
            } => self.resolve_workspace_attention(*attention_item_id, *approve, ctx),
            WorkspaceAction::ResolveWorkspaceAttentionWithResponse {
                attention_item_id,
                approve,
                response,
            } => self.resolve_workspace_attention_with_response(
                *attention_item_id,
                *approve,
                response.clone(),
                ctx,
            ),
            WorkspaceAction::ResolveWorkspaceExecutionReview {
                attempt_id,
                resolution,
            } => self.resolve_workspace_execution_review(*attempt_id, *resolution, ctx),
            _ => {}
        }
    }
}
