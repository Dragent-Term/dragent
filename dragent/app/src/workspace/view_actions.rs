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

impl TypedActionView for WorkspaceView {
    type Action = WorkspaceAction;

    fn handle_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        // A sleeping agent whose tab this action brought on screen wakes.
        // Checked once the action has taken effect.
        if super::agent_sleep::any_asleep() {
            ctx.spawn(Timer::after(std::time::Duration::ZERO), |view, _, ctx| {
                view.wake_visible_sleepers(ctx);
            });
        }
        #[cfg(not(target_family = "wasm"))]
        if matches!(action, WorkspaceAction::CloseActive) {
            if let Some(id) = self
                .computer_asks
                .front()
                .map(|ask| ask.request_id().to_string())
            {
                self.answer_computer_ask(&id, false, false, ctx);
                return;
            }
        }
        if self.prompt_install_terminal.is_some() && matches!(action, WorkspaceAction::CloseActive)
        {
            self.close_prompt_agent_install(ctx);
            return;
        }
        #[cfg(not(target_family = "wasm"))]
        if self.review_setup.is_some() && matches!(action, WorkspaceAction::CloseActive) {
            self.cancel_review_setup(ctx);
            return;
        }
        #[cfg(not(target_family = "wasm"))]
        if self.pull_requests_open && matches!(action, WorkspaceAction::CloseActive) {
            self.close_pull_requests(ctx);
            return;
        }
        if matches!(
            action,
            WorkspaceAction::RefreshDiffCommentAgents | WorkspaceAction::SendDiffComment
        ) {
            self.handle_diff_comment_action(action, ctx);
            return;
        }
        // Explicit navigation leaves Settings (and the Automations page)
        // before activating its target. Background terminal/host events do
        // not dismiss the current screen.
        if (self.settings_open
            || self.automations_open
            || self.teams_open
            || self.tasks_open
            || self.pull_requests_open)
            && matches!(
                action,
                WorkspaceAction::OpenProjectWorkspacePrompt { .. }
                    | WorkspaceAction::SelectProjectWorkspace { .. }
                    | WorkspaceAction::SelectWorkspaceTab { .. }
                    | WorkspaceAction::SelectTab { .. }
                    | WorkspaceAction::NewWorkspaceTab
                    | WorkspaceAction::NewShellWorkspaceTab { .. }
                    | WorkspaceAction::NewDragentCliTab
                    | WorkspaceAction::NewBrowserWorkspaceTab
                    | WorkspaceAction::NewBrowserTab { .. }
                    | WorkspaceAction::SplitActiveTabWithBrowser
                    | WorkspaceAction::NewWorkItemTab
                    | WorkspaceAction::OpenTeamPrompt
                    | WorkspaceAction::NewTab
                    | WorkspaceAction::NewTabWithShell(..)
                    | WorkspaceAction::NewAgentTab
                    | WorkspaceAction::OpenAgentWithAccount { .. }
                    | WorkspaceAction::OpenAgentAccountPicker { .. }
                    | WorkspaceAction::CloseAgentAccountPicker
                    | WorkspaceAction::OpenAgentInNewWorkspaceTab { .. }
                    | WorkspaceAction::ToggleNewTabPickerFromSidebar
                    | WorkspaceAction::OpenRecentProject { .. }
                    | WorkspaceAction::OpenFolder
                    | WorkspaceAction::CreateNewProject
            )
        {
            self.close_settings(ctx);
            #[cfg(not(target_family = "wasm"))]
            self.close_automations(ctx);
            #[cfg(not(target_family = "wasm"))]
            self.close_teams(ctx);
            #[cfg(not(target_family = "wasm"))]
            self.close_tasks(ctx);
            #[cfg(not(target_family = "wasm"))]
            self.close_pull_requests(ctx);
        }
        if matches!(
            action,
            WorkspaceAction::OpenSettings
                | WorkspaceAction::NewWindow
                | WorkspaceAction::OpenDocumentation
                | WorkspaceAction::OpenExternalUrl { .. }
                | WorkspaceAction::SignOutFromHeader
                | WorkspaceAction::SignInFromHeader
                | WorkspaceAction::OpenFeedback
        ) && self.close_org_menus()
        {
            self.sync_org_menu_highlight(ctx);
            ctx.notify();
        }
        match action {
            #[cfg(not(target_family = "wasm"))]
            WorkspaceAction::AnswerComputerAsk {
                request_id,
                allow,
                remember,
            } => {
                self.answer_computer_ask(request_id, *allow, *remember, ctx);
            }
            WorkspaceAction::ShowErrorNotice { .. }
            | WorkspaceAction::DismissErrorNotice
            | WorkspaceAction::CopyErrorNotice => self.handle_error_notice_action(action, ctx),
            WorkspaceAction::RefreshDiffCommentAgents | WorkspaceAction::SendDiffComment => {}
            WorkspaceAction::ToggleOrgMenu
            | WorkspaceAction::CloseOrgMenu
            | WorkspaceAction::HoverOrgSubmenu(..)
            | WorkspaceAction::ToggleHelpMenu
            | WorkspaceAction::SwitchOrganisation { .. }
            | WorkspaceAction::CreateOrganisation
            | WorkspaceAction::OpenSettingsSection(..) => self.handle_org_menu_action(action, ctx),
            WorkspaceAction::OpenFeedback
            | WorkspaceAction::FocusFeedbackDetails
            | WorkspaceAction::CloseFeedback
            | WorkspaceAction::SubmitFeedback
            | WorkspaceAction::ToggleFeedbackDiagnostics
            | WorkspaceAction::AttachFeedbackImages
            | WorkspaceAction::RemoveFeedbackAttachment { .. } => {
                self.handle_feedback_action(action, ctx)
            }
            WorkspaceAction::Remote(action) => self.handle_remote_action(action, ctx),
            WorkspaceAction::AddProject(action) => self.handle_add_project_action(action, ctx),
            WorkspaceAction::ToggleProjectContextMenu { .. }
            | WorkspaceAction::CloseProjectContextMenu
            | WorkspaceAction::OpenProjectInFinder { .. }
            | WorkspaceAction::NewProjectGroup { .. }
            | WorkspaceAction::CancelProjectGroup
            | WorkspaceAction::RemoveProjectGroup { .. }
            | WorkspaceAction::SubmitProjectGroup
            | WorkspaceAction::MoveWorkspaceToGroup { .. }
            | WorkspaceAction::ReorderWorkspaceTab { .. }
            | WorkspaceAction::ReorderProject { .. }
            | WorkspaceAction::ImportProjectWorktrees { .. }
            | WorkspaceAction::ConfirmImportProjectWorktrees
            | WorkspaceAction::CancelImportProjectWorktrees
            | WorkspaceAction::HideProjectFromSidebar { .. }
            | WorkspaceAction::ShowProjectInSidebar { .. }
            | WorkspaceAction::RequestArchiveProject { .. }
            | WorkspaceAction::RequestDeleteProject { .. }
            | WorkspaceAction::CancelProjectRemoval
            | WorkspaceAction::ConfirmProjectRemoval => {
                self.handle_project_context_action(action, ctx)
            }
            WorkspaceAction::OpenProjectWorkspacePrompt { .. }
            | WorkspaceAction::OpenPromptAgentInstall { .. }
            | WorkspaceAction::OpenCustomAgentInstall { .. }
            | WorkspaceAction::ClosePromptAgentInstall
            | WorkspaceAction::FocusPopupTerminal
            | WorkspaceAction::CheckPromptAgentInstall
            | WorkspaceAction::RetryRemoteToolElevated => self.handle_projects_action(action, ctx),
            WorkspaceAction::ToggleLayoutMenu
            | WorkspaceAction::CloseLayoutMenu
            | WorkspaceAction::SelectWorkspaceLayout(..)
            | WorkspaceAction::ToggleAddProjectMenu
            | WorkspaceAction::CloseAddProjectMenu
            | WorkspaceAction::ToggleNewTabPicker
            | WorkspaceAction::ToggleNewTabPickerFromSidebar
            | WorkspaceAction::CloseNewTabPicker
            | WorkspaceAction::ToggleAccountMenu
            | WorkspaceAction::CloseAccountMenu
            | WorkspaceAction::ToggleUsageMenu
            | WorkspaceAction::CloseUsageMenu
            | WorkspaceAction::SelectUsageAgent(_)
            | WorkspaceAction::OpenPastSessions
            | WorkspaceAction::ClosePastSessions
            | WorkspaceAction::ClearPastSessionsSearch
            | WorkspaceAction::AskDeletePastSession(_)
            | WorkspaceAction::CancelDeletePastSession
            | WorkspaceAction::DeletePastSession(_)
            | WorkspaceAction::CompactAgentHistory(_)
            | WorkspaceAction::ResumePastSession(..)
            | WorkspaceAction::ToggleContentHeaderMenu
            | WorkspaceAction::ToggleSplitShareMenu { .. }
            | WorkspaceAction::CloseContentHeaderMenu
            | WorkspaceAction::ToggleParticipantsMenu
            | WorkspaceAction::CloseParticipantsMenu
            | WorkspaceAction::ToggleTabMenu { .. }
            | WorkspaceAction::CloseTabMenu
            | WorkspaceAction::TogglePortsPanel
            | WorkspaceAction::ToggleLanguageServicesPanel
            | WorkspaceAction::OpenShareDropdown { .. }
            | WorkspaceAction::CloseShareDropdown
            | WorkspaceAction::BlurAll
            | WorkspaceAction::ToggleProjectDirectory { .. } => {
                self.handle_menus_action(action, ctx)
            }
            WorkspaceAction::ToggleReviewMenu
            | WorkspaceAction::CloseReviewMenu
            | WorkspaceAction::CreatePullRequest
            | WorkspaceAction::RefreshReviewPr
            | WorkspaceAction::PushBranch
            | WorkspaceAction::StageGitFile { .. }
            | WorkspaceAction::DiscardGitFile { .. }
            | WorkspaceAction::StageAllGitFiles
            | WorkspaceAction::UnstageAllGitFiles
            | WorkspaceAction::DiscardAllGitFiles { .. }
            | WorkspaceAction::PullActiveGitBranch
            | WorkspaceAction::PushActiveGitBranch
            | WorkspaceAction::OpenGitDiff { .. }
            | WorkspaceAction::OpenGitWorktreeDiff
            | WorkspaceAction::OpenWorkspaceAttemptChanges { .. }
            | WorkspaceAction::OpenGitCommitDiff { .. }
            | WorkspaceAction::CommitGitStaged { .. }
            | WorkspaceAction::GenerateGitCommitMessage
            | WorkspaceAction::ConfirmDiscardGitHunk
            | WorkspaceAction::CancelDiscardGitHunk
            | WorkspaceAction::StageGitHunk { .. }
            | WorkspaceAction::RequestDiscardGitHunk { .. } => {
                self.handle_review_git_action(action, ctx)
            }
            WorkspaceAction::SelectWorkspaceAgentTab(..)
            | WorkspaceAction::CloseWorkspaceAgentTab(..)
            | WorkspaceAction::CloseWorkItemTab
            | WorkspaceAction::SelectWorkItemView(..)
            | WorkspaceAction::ApprovePlanAndCoordinate
            | WorkspaceAction::AlwaysAllowWorkspaceTool { .. }
            | WorkspaceAction::TerminalConfirmChanged
            | WorkspaceAction::ToggleWorkspaceAttentionBell
            | WorkspaceAction::SelectWorkspaceAttentionItem { .. }
            | WorkspaceAction::RouteToWorkspaceAttention { .. }
            | WorkspaceAction::RestoreWorkspaceCheckpoint { .. }
            | WorkspaceAction::ToggleWorkspaceAgentTimelineExpanded
            | WorkspaceAction::WaiveWorkspaceCriterion { .. }
            | WorkspaceAction::CancelWorkspaceExecution
            | WorkspaceAction::ToggleWorkspaceExecutionPause
            | WorkspaceAction::DismissCoordinationError
            | WorkspaceAction::StopPrincipalTurn
            | WorkspaceAction::ResolveWorkspaceAttention { .. }
            | WorkspaceAction::ResolveWorkspaceAttentionWithResponse { .. }
            | WorkspaceAction::ResolveWorkspaceExecutionReview { .. }
            | WorkspaceAction::OpenWorkspaceAgentTimeline { .. } => {
                self.handle_agent_action(action, ctx)
            }
            WorkspaceAction::SubmitShapeMessage
            | WorkspaceAction::ApproveShape
            | WorkspaceAction::CancelRemoveConfirm
            | WorkspaceAction::ToggleShapeModelMenu
            | WorkspaceAction::SelectShapeModel { .. }
            | WorkspaceAction::CopyShapeMessageText { .. }
            | WorkspaceAction::OpenShapeAttachmentPicker
            | WorkspaceAction::AttachShapeFiles { .. }
            | WorkspaceAction::RemoveShapeAttachment { .. }
            | WorkspaceAction::ToggleShapeVoiceInput
            | WorkspaceAction::BeginDescribeRole
            | WorkspaceAction::SelectProposedRole { .. }
            | WorkspaceAction::ToggleShapeRoleModelPicker { .. }
            | WorkspaceAction::CloseProposedRole
            | WorkspaceAction::ApproveProposedRole { .. }
            | WorkspaceAction::StartShapeRecordEdit { .. }
            | WorkspaceAction::MoveShapePlanTask { .. }
            | WorkspaceAction::RemoveShapePlanTask { .. }
            | WorkspaceAction::RemoveShapeRecord { .. }
            | WorkspaceAction::SaveShapeRecordEdit
            | WorkspaceAction::CancelShapeRecordEdit
            | WorkspaceAction::SelectShapeRoleModel { .. } => {
                self.handle_shaping_action(action, ctx)
            }
            WorkspaceAction::OpenFolder
            | WorkspaceAction::ShowProjectClone
            | WorkspaceAction::ShowProjectSsh
            | WorkspaceAction::ShowMachines
            | WorkspaceAction::CloseProjectStartForm
            | WorkspaceAction::SubmitProjectStartForm
            | WorkspaceAction::ConnectSshDestination { .. }
            | WorkspaceAction::ConnectGitHub
            | WorkspaceAction::OpenRecentProject { .. }
            | WorkspaceAction::CreateNewProject
            | WorkspaceAction::ShowAddProjectCloneForm
            | WorkspaceAction::SubmitAddProjectClone
            | WorkspaceAction::OpenCreateProject
            | WorkspaceAction::CloseCreateProject
            | WorkspaceAction::BrowseCreateProjectLocation
            | WorkspaceAction::CreateProjectLocationSelected { .. }
            | WorkspaceAction::SubmitCreateProject
            | WorkspaceAction::ShowNewWorktreeForm
            | WorkspaceAction::RenameWorktreeBranch { .. }
            | WorkspaceAction::RefreshWorktreeDetails { .. }
            | WorkspaceAction::CloseNewWorktree
            | WorkspaceAction::SubmitNewWorktree
            | WorkspaceAction::ConfirmGitInit
            | WorkspaceAction::DismissGitInit
            | WorkspaceAction::OpenPromptMenu { .. }
            | WorkspaceAction::ClosePromptMenus
            | WorkspaceAction::ClearPromptAgentSearch
            | WorkspaceAction::SelectPromptAgent { .. }
            | WorkspaceAction::TogglePromptTeam
            | WorkspaceAction::SetPromptLeadModel { .. }
            | WorkspaceAction::BrowsePromptModelTab { .. }
            | WorkspaceAction::PickPromptAgentModel { .. }
            | WorkspaceAction::TogglePromptModelFavorite { .. }
            | WorkspaceAction::TogglePromptTeamAgent { .. }
            | WorkspaceAction::SetPromptTeamCost { .. }
            | WorkspaceAction::CancelReviewSetup
            | WorkspaceAction::SubmitPrompt
            | WorkspaceAction::OpenPromptAttachmentPicker
            | WorkspaceAction::AttachPromptFiles { .. }
            | WorkspaceAction::OpenPromptLinkKind { .. }
            | WorkspaceAction::SelectPromptLink { .. }
            | WorkspaceAction::RemovePromptLink { .. }
            | WorkspaceAction::ClearPromptTask
            | WorkspaceAction::RemovePromptAttachment { .. }
            | WorkspaceAction::TogglePromptVoiceInput
            | WorkspaceAction::SelectPromptCurrentCheckout
            | WorkspaceAction::OpenPromptWorktree { .. }
            | WorkspaceAction::CreatePromptWorktreeFromBranch { .. }
            | WorkspaceAction::TogglePromptRemote
            | WorkspaceAction::ConnectPromptSsh
            | WorkspaceAction::OpenNewWorktreeFromPrompt
            | WorkspaceAction::OpenDragentRepo
            | WorkspaceAction::CloneRepositoryFromUrl { .. }
            | WorkspaceAction::BrowseCloneLocation
            | WorkspaceAction::CloneLocationSelected { .. }
            | WorkspaceAction::CloneDestinationSelected { .. }
            | WorkspaceAction::ProjectFolderSelected { .. }
            | WorkspaceAction::LaunchProjectWork
            | WorkspaceAction::LaunchProjectDragent
            | WorkspaceAction::LaunchProjectCodex
            | WorkspaceAction::LaunchProjectClaude
            | WorkspaceAction::LaunchProjectOpenCode
            | WorkspaceAction::LaunchProjectTerminal
            | WorkspaceAction::BrowseProjectFiles => self.handle_projects_action(action, ctx),
            WorkspaceAction::NewWorkspaceTab
            | WorkspaceAction::NewShellWorkspaceTab { .. }
            | WorkspaceAction::NewDragentCliTab
            | WorkspaceAction::NewWorkItemTab
            | WorkspaceAction::OpenTeamPrompt
            | WorkspaceAction::CloseProjectWorkspace { .. }
            | WorkspaceAction::SleepProjectWorkspace { .. }
            | WorkspaceAction::SelectProjectWorkspace { .. }
            | WorkspaceAction::SelectWorkspaceTab { .. }
            | WorkspaceAction::CloseWorkspaceTab { .. }
            | WorkspaceAction::CloseWorkspaceTabConfirmed { .. }
            | WorkspaceAction::CancelCloseWorkspaceTab
            | WorkspaceAction::StartRenameWorkspaceTab { .. }
            | WorkspaceAction::CommitRenameWorkspaceTab { .. }
            | WorkspaceAction::CancelRenameWorkspaceTab
            | WorkspaceAction::MoveWorkspaceTabTo { .. }
            | WorkspaceAction::MoveWorkspaceTabLeft { .. }
            | WorkspaceAction::MoveWorkspaceTabRight { .. }
            | WorkspaceAction::SetAgentRailExpanded { .. }
            | WorkspaceAction::OpenAgentWithAccount { .. }
            | WorkspaceAction::OpenAgentAccountPicker { .. }
            | WorkspaceAction::CloseAgentAccountPicker
            | WorkspaceAction::OpenAgentInNewWorkspaceTab { .. } => {
                self.handle_workspace_tab_action(action, ctx)
            }
            WorkspaceAction::RestartHostSession { pane_uuid, fresh } => {
                self.restart_host_session(*pane_uuid, *fresh, ctx)
            }
            WorkspaceAction::NewBrowserWorkspaceTab => self.open_browser_workspace_tab(ctx),
            WorkspaceAction::NewBrowserTab { url } => self.open_browser_pane_tab(url.clone(), ctx),
            WorkspaceAction::SplitActiveTabWithBrowser => self.split_with_browser(ctx),
            WorkspaceAction::NewTab
            | WorkspaceAction::NewTabWithShell(..)
            | WorkspaceAction::NewAgentTab
            | WorkspaceAction::SelectTab { .. }
            | WorkspaceAction::FocusSplitPane { .. }
            | WorkspaceAction::RemoveEditorPane
            | WorkspaceAction::CloseTab { .. }
            | WorkspaceAction::CloseTabConfirmed { .. }
            | WorkspaceAction::SplitActiveTabHorizontal
            | WorkspaceAction::SplitActiveTabVertical
            | WorkspaceAction::SplitTabHorizontal { .. }
            | WorkspaceAction::SplitTabVertical { .. }
            | WorkspaceAction::BeginTabDrag
            | WorkspaceAction::EndTabDrag
            | WorkspaceAction::RemoveTabFromSplit { .. }
            | WorkspaceAction::CancelCloseTab
            | WorkspaceAction::CloseActive
            | WorkspaceAction::SwitchTabNext
            | WorkspaceAction::SwitchTabPrev
            | WorkspaceAction::ToggleLeftPanel
            | WorkspaceAction::ToggleRightPanel
            | WorkspaceAction::TrustPendingWorkspace
            | WorkspaceAction::DismissWorkspaceTrust
            | WorkspaceAction::SetSidebarViewMode(..)
            | WorkspaceAction::FocusSidebarSearch
            | WorkspaceAction::SidebarSearchInput { .. }
            | WorkspaceAction::StartRenameTab { .. }
            | WorkspaceAction::CommitRenameTab { .. }
            | WorkspaceAction::CancelRenameTab
            | WorkspaceAction::MoveTabUp { .. }
            | WorkspaceAction::MoveTabDown { .. }
            | WorkspaceAction::SetTabColor { .. }
            | WorkspaceAction::CopyTabTitle { .. }
            | WorkspaceAction::CopyTabCwd { .. }
            | WorkspaceAction::CloseOtherTabs { .. }
            | WorkspaceAction::SplitDividerDragged { .. }
            | WorkspaceAction::DropTabIntoSplit { .. } => self.handle_tabs_action(action, ctx),
            WorkspaceAction::NewWindow
            | WorkspaceAction::MinimizeWindow
            | WorkspaceAction::ToggleMaximizeWindow
            | WorkspaceAction::CloseWindow
            | WorkspaceAction::RunActivePaneCommand { .. } => {
                self.handle_terminal_action(action, ctx)
            }
            WorkspaceAction::OpenProjectSearchResults
            | WorkspaceAction::SelectOpenDocument { .. }
            | WorkspaceAction::CloseDocumentTab { .. }
            | WorkspaceAction::OpenBufferSearch { .. }
            | WorkspaceAction::CloseBufferSearch
            | WorkspaceAction::BufferSearchNext
            | WorkspaceAction::BufferSearchPrevious
            | WorkspaceAction::ToggleBufferSearchCaseSensitive
            | WorkspaceAction::ToggleBufferSearchWholeWord
            | WorkspaceAction::ToggleBufferSearchRegex
            | WorkspaceAction::ToggleBufferSearchReplace
            | WorkspaceAction::BufferSearchReplaceCurrent
            | WorkspaceAction::BufferSearchReplaceAll
            | WorkspaceAction::ToggleMarkdownPreview
            | WorkspaceAction::ToggleDocumentTerminal
            | WorkspaceAction::NewDockedTerminal
            | WorkspaceAction::SelectDockedTerminal { .. }
            | WorkspaceAction::CloseDockedTerminal { .. }
            | WorkspaceAction::SplitDockedTerminal
            | WorkspaceAction::ToggleDockedTerminalMaximized
            | WorkspaceAction::CloseOpenDocument
            | WorkspaceAction::SaveOpenDocument
            | WorkspaceAction::RequestLanguageCompletion { .. }
            | WorkspaceAction::RequestLanguageNavigation { .. }
            | WorkspaceAction::OpenDefinitionHover { .. }
            | WorkspaceAction::RequestLanguageFormatting
            | WorkspaceAction::RequestLanguageCodeActions { .. }
            | WorkspaceAction::StartLanguageRename { .. }
            | WorkspaceAction::RequestLanguageRename { .. }
            | WorkspaceAction::CancelLanguageRename
            | WorkspaceAction::ApplyLanguageCodeAction { .. }
            | WorkspaceAction::DismissLanguageResults
            | WorkspaceAction::SaveOpenDocumentAndClose
            | WorkspaceAction::DiscardOpenDocumentAndClose
            | WorkspaceAction::CancelCloseOpenDocument
            | WorkspaceAction::ApproveOpenDocument
            | WorkspaceAction::RevertOpenDocument
            | WorkspaceAction::RejectOpenDocument
            | WorkspaceAction::CheckpointOpenDocument
            | WorkspaceAction::OpenReadOnlyDocument { .. }
            | WorkspaceAction::ReorderDocumentTab { .. }
            | WorkspaceAction::OpenMarkdownLink { .. }
            | WorkspaceAction::OpenTerminalPath { .. }
            | WorkspaceAction::AttachEditorSymbolToAgent { .. }
            | WorkspaceAction::AttachEditorSymbolToNewAgent { .. }
            | WorkspaceAction::RequestDefinitionHover { .. }
            | WorkspaceAction::OpenLanguageLocation { .. } => {
                self.handle_editor_action(action, ctx)
            }
            WorkspaceAction::ExplorerCopyEntry { .. }
            | WorkspaceAction::ExplorerCutEntry { .. }
            | WorkspaceAction::ExplorerPasteInto { .. }
            | WorkspaceAction::ExplorerDuplicateEntry { .. }
            | WorkspaceAction::ExplorerStartRename { .. }
            | WorkspaceAction::ExplorerCommitPrompt
            | WorkspaceAction::ExplorerCancelPrompt
            | WorkspaceAction::ExplorerConfirmDelete
            | WorkspaceAction::ExplorerCancelDelete
            | WorkspaceAction::ExplorerDismissError
            | WorkspaceAction::ExplorerRefresh
            | WorkspaceAction::ExplorerCopyPath { .. }
            | WorkspaceAction::ExplorerStartCreate { .. }
            | WorkspaceAction::ExplorerRequestDelete { .. } => {
                self.handle_explorer_action(action, ctx)
            }
            WorkspaceAction::JoinSession { .. }
            | WorkspaceAction::RequestSessionEdit
            | WorkspaceAction::OpenParticipantAccess { .. }
            | WorkspaceAction::CloseParticipantAccess
            | WorkspaceAction::OpenExternalUrl { .. }
            | WorkspaceAction::StarOnGitHub
            | WorkspaceAction::FollowOnX
            | WorkspaceAction::ShareSession { .. }
            | WorkspaceAction::CancelShareScope
            | WorkspaceAction::OpenShareSheet
            | WorkspaceAction::RequestStopSharing
            | WorkspaceAction::CancelStopSharing
            | WorkspaceAction::StopSharing
            | WorkspaceAction::KeepSessionAlive
            | WorkspaceAction::EndIdleSession
            | WorkspaceAction::CloseShareSheet
            | WorkspaceAction::CopyShareValue { .. }
            | WorkspaceAction::CopyPortForwardValue { .. }
            | WorkspaceAction::OpenPortForward { .. }
            | WorkspaceAction::InviteEmail
            | WorkspaceAction::CreatePortForward
            | WorkspaceAction::ClosePortForward { .. }
            | WorkspaceAction::SetInviteRole { .. }
            | WorkspaceAction::SetLinkAccess { .. }
            | WorkspaceAction::RemoveParticipant { .. }
            | WorkspaceAction::OpenShareQr
            | WorkspaceAction::CloseShareQr
            | WorkspaceAction::ConfirmShareScope { .. }
            | WorkspaceAction::SetParticipantAccess { .. }
            | WorkspaceAction::ResolveAccessRequest { .. } => {
                self.handle_sharing_action(action, ctx)
            }
            WorkspaceAction::OpenAutomationNotice { .. }
            | WorkspaceAction::OpenAutomationInbox
            | WorkspaceAction::OpenAutomationLink(_)
            | WorkspaceAction::OpenAutomationRun(_) => {
                #[cfg(not(target_family = "wasm"))]
                self.handle_automation_run_menu(action, ctx);
            }
            WorkspaceAction::OpenAutomations | WorkspaceAction::CloseAutomations => {
                #[cfg(not(target_family = "wasm"))]
                if matches!(action, WorkspaceAction::OpenAutomations) {
                    self.close_teams(ctx);
                    self.close_tasks(ctx);
                    self.close_pull_requests(ctx);
                    self.open_automations(ctx);
                } else {
                    self.close_automations(ctx);
                }
            }
            WorkspaceAction::OpenPullRequests | WorkspaceAction::ClosePullRequests => {
                #[cfg(not(target_family = "wasm"))]
                if matches!(action, WorkspaceAction::OpenPullRequests) {
                    self.open_pull_requests(ctx);
                } else {
                    self.close_pull_requests(ctx);
                }
            }
            WorkspaceAction::OpenPullRequest { url } => {
                #[cfg(not(target_family = "wasm"))]
                {
                    let page = self.pull_requests_view.clone();
                    ctx.update_view(&page, |page, _| page.select_url(url.clone()));
                    self.open_pull_requests(ctx);
                }
                #[cfg(target_family = "wasm")]
                let _ = url;
            }
            WorkspaceAction::OpenCoordinationPane { pane, workspace } => {
                self.open_team_pane(pane, workspace.clone(), ctx);
            }
            WorkspaceAction::OpenTeams { team } => {
                #[cfg(not(target_family = "wasm"))]
                self.open_teams(team.clone(), ctx);
                #[cfg(target_family = "wasm")]
                let _ = team;
            }
            WorkspaceAction::OpenTasks => {
                #[cfg(not(target_family = "wasm"))]
                self.open_tasks(ctx);
            }
            WorkspaceAction::OpenSettings
            | WorkspaceAction::CloseSettings
            | WorkspaceAction::SignOutFromHeader
            | WorkspaceAction::ConfirmSignOut
            | WorkspaceAction::CancelSignOut
            | WorkspaceAction::ToggleLogoutDontAsk
            | WorkspaceAction::OpenDocumentation
            | WorkspaceAction::OpenBillingFromHeader
            | WorkspaceAction::UpdateAppFromHeader
            | WorkspaceAction::SignInFromHeader
            | WorkspaceAction::CloseLoginPrompt
            | WorkspaceAction::RestartLanguageServices
            | WorkspaceAction::StopLanguageServices
            | WorkspaceAction::RestartLanguageServer { .. }
            | WorkspaceAction::StopLanguageServer { .. }
            | WorkspaceAction::InstallLanguageServer { .. }
            | WorkspaceAction::SetProjectSidebarTab { .. }
            | WorkspaceAction::OpenCommandPalette => self.handle_settings_action(action, ctx),
            WorkspaceAction::ToggleAgentsPanel
            | WorkspaceAction::RefreshAgentRuns
            | WorkspaceAction::FocusAgentRun { .. }
            | WorkspaceAction::StopAgentRun { .. }
            | WorkspaceAction::CloseAgentsPanel => self.handle_agents_action(action, ctx),
            WorkspaceAction::TogglePerfOverlay => {
                #[cfg(not(target_family = "wasm"))]
                {
                    crate::perf_overlay::toggle();
                }
                ctx.notify();
            }
            WorkspaceAction::ToggleSleepAgent { .. }
            | WorkspaceAction::ConfirmSleepAgents
            | WorkspaceAction::KeepAgentsAwake => self.handle_sleep_prompt_action(action, ctx),
            WorkspaceAction::StartAgentAnyway { .. }
            | WorkspaceAction::ResumeAgent { .. }
            | WorkspaceAction::StopHeldAgent { .. } => self.handle_agent_resources_action(action, ctx),
        }
        if self.project_start_visible {
            self.sync_pane_activity(ctx);
        }
        self.save_session(ctx);
    }
}
