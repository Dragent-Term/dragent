//! Workspace shaping flows actions.
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
    pub(crate) fn handle_shaping_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::SubmitShapeMessage => {
                // Once coordination has started, the composer's send button
                // means something completely different: `submit_shape_message`
                // below unconditionally reruns the full Shaping pass, which
                // has no idea workers are running. Route to the live
                // coordination path instead — it only ever posts one message,
                // never touches the plan, and never stops an attempt.
                if self
                    .workspace_state
                    .active_work()
                    .is_some_and(|work| work_status_is_post_coordination(work.status))
                {
                    self.submit_live_coordination_message(ctx);
                    return;
                }
                // Composer free text while a Principal ask_user question is
                // open answers it — the terminal agent's own input routing
                // (numbers/labels pick options, anything else is free text).
                if self.answer_open_principal_question_from_composer(ctx) {
                    return;
                }
                // Explicit textual approval ("approve shape", …) is equivalent
                // to the Approve Shape button — the user's own words, not
                // agent initiative — so it unlocks shaping even when the
                // button is still disabled. This is also the escape hatch if
                // the Principal never declares requirements complete.
                if !self.shape_approved
                    && self.workspace_state.latest_shaping_proposal().is_none()
                    && super::shape_principal::is_shape_approval_text(&self.shape_composer_text)
                {
                    self.approve_shape(ctx);
                    return;
                }
                self.submit_shape_message(ctx);
            }
            WorkspaceAction::ApproveShape => {
                self.approve_shape(ctx);
            }
            WorkspaceAction::CancelRemoveConfirm => {
                if self.shape_remove_armed.take().is_some() {
                    ctx.notify();
                }
            }
            WorkspaceAction::ToggleShapeModelMenu => {
                self.shape_model_menu_open = !self.shape_model_menu_open;
                ctx.notify();
            }
            WorkspaceAction::SelectShapeModel { model_id, effort } => {
                self.select_shape_principal_model(model_id.clone(), effort.clone(), ctx);
            }
            WorkspaceAction::CopyShapeMessageText { text } => {
                ctx.clipboard()
                    .write(warpui_core::clipboard::ClipboardContent::plain_text(
                        text.clone(),
                    ));
            }
            WorkspaceAction::OpenShapeAttachmentPicker => {
                self.open_shape_attachment_picker(ctx);
            }
            WorkspaceAction::AttachShapeFiles { paths } => {
                self.receive_shape_context_files(paths.clone(), ctx);
            }
            WorkspaceAction::RemoveShapeAttachment { index } => {
                if *index < self.shape_composer_attachments.len() {
                    self.shape_composer_attachments.remove(*index);
                    ctx.notify();
                }
            }
            WorkspaceAction::ToggleShapeVoiceInput => {
                self.toggle_shape_voice_input(ctx);
            }
            WorkspaceAction::BeginDescribeRole => {
                self.begin_describe_role(ctx);
            }
            WorkspaceAction::SelectProposedRole { work_contract_id } => {
                self.select_proposed_role(*work_contract_id, ctx);
            }
            WorkspaceAction::ToggleShapeRoleModelPicker { work_contract_id } => {
                self.toggle_shape_role_model_picker(*work_contract_id, ctx);
            }
            WorkspaceAction::CloseProposedRole => self.close_proposed_role(ctx),
            WorkspaceAction::ApproveProposedRole { work_contract_id } => {
                self.approve_proposed_role(*work_contract_id, ctx);
            }
            WorkspaceAction::StartShapeRecordEdit { target } => {
                self.start_shape_record_edit(*target, ctx);
            }
            WorkspaceAction::MoveShapePlanTask { task_id, direction } => {
                self.move_shape_plan_task(*task_id, *direction, ctx);
            }
            WorkspaceAction::RemoveShapePlanTask { task_id } => {
                self.remove_shape_plan_task(*task_id, ctx);
            }
            WorkspaceAction::RemoveShapeRecord { target } => {
                self.remove_shape_record(*target, ctx);
            }
            WorkspaceAction::SaveShapeRecordEdit => self.save_shape_record_edit(ctx),
            WorkspaceAction::CancelShapeRecordEdit => self.cancel_shape_record_edit(ctx),
            WorkspaceAction::SelectShapeRoleModel {
                work_contract_id,
                model_route,
            } => {
                self.select_shape_role_model(*work_contract_id, model_route.clone(), ctx);
            }
            _ => {}
        }
    }
}
