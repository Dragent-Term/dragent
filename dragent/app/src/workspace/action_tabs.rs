//! Workspace tabs, panes, and splits actions.
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
    /// Synchronize a tab with native focus without requesting that focus again.
    /// Explicit tab selections still move the keyboard; focus notifications do not.
    pub(crate) fn select_tab_with_focus(
        &mut self,
        tab_id: usize,
        take_focus: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.tabs.iter().any(|tab| tab.id == tab_id) {
            if let Some(pos) = self
                .parked_workspace_tabs
                .iter()
                .position(|surface| surface.panes.iter().any(|tab| tab.id == tab_id))
            {
                let target = self.parked_workspace_tabs.remove(pos);
                self.store_active_workspace_tab();
                self.restore_workspace_tab(target);
            }
        }
        if self.tabs.iter().any(|t| t.id == tab_id) {
            self.project_start_visible = false;
            self.project_start_mode = None;
            self.project_start_error = None;
            self.prompt_menu = None;
            self.exit_split_unless_contains(tab_id);
            self.switch_workspace_work_for_tab(tab_id, ctx);
            self.active_tab_id = tab_id;
            self.touch_tab_mru(tab_id);
            self.restore_document_owner_tab(tab_id, ctx);
            self.set_active_terminal_sticky_headers(true, ctx);
            self.update_active_pane_with_focus(take_focus, ctx);
            let selected_work_pane = take_focus
                && self.work_item_tab_open
                && self.work_item_tab_active
                && self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .is_some_and(|tab| {
                        tab.work_item_id == Some(self.workspace_work_item_id)
                    });
            if selected_work_pane {
                ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
                    editor.request_focus(editor_ctx)
                });
            }
            self.refresh_all_tab_titles(ctx);
            ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                lp.sidebar_menu_tab_id = None;
                lp.sidebar_focus = SidebarFocus::None;
                lp_ctx.notify();
            });
            self.sync_left_panel(ctx);
            self.save_session(ctx);
            ctx.notify();
        }
    }

    pub(crate) fn handle_tabs_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::NewTab => {
                self.new_tab_picker_open = false;
                self.document_center_open = false;
                self.document_terminal_visible = false;
                let new_tab = self.create_tab(ctx, None, None);
                self.active_tab_id = new_tab.id;
                self.split_tree = None;
                self.tabs.push(new_tab);
                self.touch_tab_mru(self.active_tab_id);
                self.project_start_visible = false;
                self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
                self.set_active_terminal_sticky_headers(true, ctx);
                self.update_active_pane(ctx);
                self.refresh_all_tab_titles(ctx);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::NewTabWithShell(shell) => {
                self.new_tab_picker_open = false;
                self.document_center_open = false;
                self.document_terminal_visible = false;
                let new_tab = self.create_tab(ctx, None, Some(shell.clone()));
                self.active_tab_id = new_tab.id;
                self.split_tree = None;
                self.tabs.push(new_tab);
                self.touch_tab_mru(self.active_tab_id);
                self.project_start_visible = false;
                self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
                self.set_active_terminal_sticky_headers(true, ctx);
                self.update_active_pane(ctx);
                self.refresh_all_tab_titles(ctx);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::NewAgentTab => {
                self.new_tab_picker_open = false;
                self.document_center_open = false;
                self.document_terminal_visible = false;
                let new_tab = self.create_agent_tab(ctx);
                new_tab.pane.set_agent_id(ctx, Some("dragent".to_string()));
                self.active_tab_id = new_tab.id;
                self.split_tree = None;
                self.tabs.push(new_tab);
                self.touch_tab_mru(self.active_tab_id);
                self.project_start_visible = false;
                self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
                self.set_active_terminal_sticky_headers(true, ctx);
                self.update_active_pane(ctx);
                self.refresh_all_tab_titles(ctx);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::SelectTab { tab_id } => {
                self.select_tab_with_focus(*tab_id, true, ctx);
            }
            WorkspaceAction::FocusSplitPane { tab_id } => {
                let tab_id = *tab_id;
                if tab_id == EDITOR_PANE_ID {
                    // Editor clicks focus natively; the flag drives only the
                    // pane ring. Tabs, work state, and documents stay put.
                    self.editor_pane_focused = true;
                    ctx.notify();
                    return;
                }
                if tab_id == self.active_tab_id {
                    let active_is_work_pane = self.work_item_tab_open
                        && self
                            .tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .is_some_and(|tab| {
                                tab.work_item_id == Some(self.workspace_work_item_id)
                            });
                    if active_is_work_pane {
                        // Work owns its own composer focus. Its tab still has a
                        // backing runtime pane, but clicks inside Work must not
                        // redirect keyboard focus into that hidden terminal.
                        self.editor_pane_focused = false;
                        ctx.notify();
                        return;
                    }
                    // The unified terminal renderer emits pane-focus clicks in
                    // standalone mode too. Refocusing the current terminal is
                    // a no-op and must never route through tab selection or
                    // reveal a dormant work-item/editor surface.
                    self.editor_pane_focused = false;
                    self.set_active_terminal_sticky_headers(true, ctx);
                    self.update_active_pane(ctx);
                    return;
                }
                let target = self.tabs.iter().find(|t| t.id == tab_id);
                let same_project = match (target, &self.split_tree) {
                    (Some(tab), Some(tree)) if tree.contains(tab_id) => self
                        .tabs
                        .iter()
                        .find(|t| t.id == self.active_tab_id)
                        .is_some_and(|active| active.project_root == tab.project_root),
                    _ => false,
                };
                if !same_project {
                    // Not a same-split focus move: full context change.
                    self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
                    return;
                }
                // Mosaic focus: move the active tab + terminal focus and
                // repaint highlights. Deliberately no work-item park/unpark,
                // no work-surface open, no document-owner change — the work
                // stays put while focus moves between panes.
                self.active_tab_id = tab_id;
                self.set_active_terminal_sticky_headers(true, ctx);
                self.update_active_pane(ctx);
                self.refresh_all_tab_titles(ctx);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp.sidebar_focus = SidebarFocus::None;
                    lp_ctx.notify();
                });
                self.sync_left_panel(ctx);
                ctx.notify();
            }
            WorkspaceAction::RemoveEditorPane => {
                self.remove_editor_pane();
                ctx.notify();
            }
            WorkspaceAction::CloseTab { tab_id } => {
                let tab_id = *tab_id;
                if !self.tabs.iter().any(|tab| tab.id == tab_id) {
                    self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
                }
                let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else {
                    return;
                };
                if tab.pane.is_settings() {
                    self.handle_action(&WorkspaceAction::CloseTabConfirmed { tab_id }, ctx);
                    return;
                }
                let command = crate::confirm::foreground_command_for(tab.pane.pane_uuid(ctx));
                // Each case asks only while its confirmation is on: deleting
                // the worktree this tab is the last one in, stopping a running
                // command, or closing an idle tab.
                let removes_worktree = self.worktree_removed_on_close(tab_id).is_some();
                let ask = (removes_worktree
                    && crate::settings::git_consumers::confirm_delete_worktree())
                    || (command.is_some()
                        && crate::settings::git_consumers::confirm_close_terminal())
                    || (!removes_worktree
                        && command.is_none()
                        && crate::settings::AppSettings::global()
                            .agent
                            .confirm_close_idle_tab);
                // `CloseTabConfirmed` removes the worktree only for the pending tab.
                self.pending_close_tab = Some((tab_id, command.unwrap_or_default()));
                self.close_tab_error = None;
                if !ask {
                    self.handle_action(&WorkspaceAction::CloseTabConfirmed { tab_id }, ctx);
                    return;
                }
                ctx.notify();
            }
            WorkspaceAction::CloseTabConfirmed { tab_id } => {
                let tab_id = *tab_id;
                if self.closing_worktree_tab.is_some() {
                    return;
                }
                if self
                    .pending_close_tab
                    .as_ref()
                    .is_some_and(|(pending_id, _)| *pending_id == tab_id)
                {
                    if let Some((main, root)) = self.worktree_removed_on_close(tab_id) {
                        self.closing_worktree_tab = Some(tab_id);
                        self.close_tab_error = None;
                        // The close is confirmed: end the tab's shell now. On
                        // Windows a folder that is a running shell's working
                        // directory cannot be deleted.
                        self.close_host_tab_session(tab_id);
                        ctx.notify();
                        let checkout = main.clone();
                        let removed_project = root.clone();
                        let data = super::project_data::ProjectData::locate(&root);
                        ctx.spawn(
                            async move {
                                crate::git_status::worktree::force_remove_worktree(&main, &root)
                            },
                            move |view, result: Result<(), String>, ctx| {
                                view.closing_worktree_tab = None;
                                match result {
                                    Ok(()) => {
                                        data.forget(&view.persistence_writer);
                                        view.persistence_writer.delete_workspace_project(
                                            dragent_workspace_model::Project::new_for_organisation(
                                                &view.organisation_id,
                                                removed_project,
                                                0,
                                            )
                                            .id,
                                        );
                                        view.pending_close_tab = None;
                                        view.closed_worktree_main_checkout = Some(checkout);
                                        view.handle_action(
                                            &WorkspaceAction::CloseTabConfirmed { tab_id },
                                            ctx,
                                        );
                                    }
                                    Err(error) => {
                                        view.close_tab_error = Some(error);
                                        ctx.notify();
                                    }
                                }
                            },
                        );
                        return;
                    }
                }
                if self.host_mirror_tab_id == Some(tab_id) {
                    // Drop only the local binding so the pump stops; the
                    // host-owned run keeps going headless.
                    self.host_mirror_pane = None;
                    self.host_mirror_tab_id = None;
                    self.host_mirror = None;
                }
                self.pending_close_tab = None;
                self.close_tab_error = None;
                if let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) {
                    if !tab.pane.is_settings() {
                        let pane_uuid = tab.pane.pane_uuid(ctx);
                        let project_root = tab.project_root.clone();
                        // The tab is going away — stop tracking its foreground
                        // command.
                        crate::confirm::set_foreground_command(pane_uuid, None);
                        // Host-owned tab: dispose its daemon session now.
                        // (App quit never reaches this path — sessions
                        // survive quit and re-attach on relaunch.)
                        self.close_host_tab_session(tab_id);
                        crate::agent::session_facts::clear_conversation(pane_uuid);
                        // A linked worktree may already have been removed by
                        // the confirmed close. Routing a clear to that path
                        // would recreate its `.dragent` database on disk.
                        if project_root.as_ref().is_none_or(|root| root.is_dir()) {
                            persistence::clear_pane_data(
                                &self.persistence_writer,
                                pane_uuid,
                                project_root.clone(),
                            );
                            persistence::clear_agent_blocks(
                                &self.persistence_writer,
                                pane_uuid,
                                project_root,
                            );
                        }
                    }
                }
                self.remove_tab_from_split(tab_id);
                self.tab_mru.retain(|id| *id != tab_id);
                if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
                    let removed = self.tabs.remove(pos);
                    let removed_work_item_id = removed.work_item_id;
                    let removed_project_root = removed.project_root.clone();
                    let closed_worktree_main = self.closed_worktree_main_checkout.take();
                    if let Some(root) = &removed_project_root {
                        if closed_worktree_main.is_some() {
                            self.open_project_roots.retain(|open| open != root);
                            if self.prompt_selected_worktree.as_ref() == Some(root) {
                                self.prompt_selected_worktree = None;
                                self.prompt_branch = None;
                            }
                        }
                    }
                    self.release_surface_if_unplaced(removed.surface_id);
                    if self.tabs.is_empty() {
                        if let Some(root) = closed_worktree_main.or(removed_project_root) {
                            if !self.open_project_roots.contains(&root) {
                                self.open_project_roots.push(root.clone());
                            }
                            self.document_index_root = root;
                        }
                        self.active_tab_id = 0;
                        self.active_pane_id = None;
                        let editor_survives = self.document_center_open
                            && !self.document_open_paths.is_empty()
                            && self.editor_pane_in_layout();
                        if editor_survives {
                            self.editor_pane_focused = true;
                            self.project_start_visible = false;
                        } else {
                            self.split_tree = None;
                            self.project_start_visible = true;
                        }
                        self.left_panel_visible = !self.open_project_roots.is_empty();
                    } else if self.active_tab_id == tab_id {
                        let new_pos = if pos >= self.tabs.len() {
                            self.tabs.len() - 1
                        } else {
                            pos
                        };
                        self.active_tab_id = self.tabs[new_pos].id;
                    }
                    self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
                    if let Some(removed_work_item_id) = removed_work_item_id {
                        self.parked_workspace_work.remove(&removed_work_item_id);
                    }
                    self.update_active_pane(ctx);
                    refresh_sidebar_projects(self, ctx);
                    apply_sidebar_visibility(self, ctx);
                    self.sync_left_panel(ctx);
                    self.save_session(ctx);
                    ctx.notify();
                    if let Some(keep_id) = self.close_other_tabs_keep_id {
                        if let Some(next_id) = self
                            .tabs
                            .iter()
                            .find(|tab| tab.id != keep_id)
                            .map(|tab| tab.id)
                        {
                            self.handle_action(&WorkspaceAction::CloseTab { tab_id: next_id }, ctx);
                        } else {
                            self.close_other_tabs_keep_id = None;
                        }
                    }
                }
            }
            WorkspaceAction::SplitActiveTabHorizontal => {
                self.split_tab(SplitDirection::Horizontal, ctx);
            }
            WorkspaceAction::SplitActiveTabVertical => {
                self.split_tab(SplitDirection::Vertical, ctx);
            }
            WorkspaceAction::SplitTabHorizontal { tab_id } => {
                self.split_specific_tab(*tab_id, SplitDirection::Horizontal, ctx);
            }
            WorkspaceAction::SplitTabVertical { tab_id } => {
                self.split_specific_tab(*tab_id, SplitDirection::Vertical, ctx);
            }
            WorkspaceAction::BeginTabDrag => {
                if !self.drag_in_progress {
                    self.drag_in_progress = true;
                    ctx.notify();
                }
            }
            WorkspaceAction::EndTabDrag => {
                if self.drag_in_progress {
                    self.drag_in_progress = false;
                    ctx.notify();
                }
            }
            WorkspaceAction::RemoveTabFromSplit { tab_id } => {
                self.pop_tab_out_of_split(*tab_id, ctx);
            }
            WorkspaceAction::CancelCloseTab => {
                if self.closing_worktree_tab.is_some() {
                    return;
                }
                self.pending_close_tab = None;
                self.close_tab_error = None;
                self.close_other_tabs_keep_id = None;
                ctx.notify();
            }
            WorkspaceAction::CloseActive => {
                let tab_id = self.active_tab_id;
                self.handle_action(&WorkspaceAction::CloseTab { tab_id }, ctx);
            }
            WorkspaceAction::SwitchTabNext => {
                self.switch_tab_by_order(ctx, true);
            }
            WorkspaceAction::SwitchTabPrev => {
                self.switch_tab_by_order(ctx, false);
            }
            WorkspaceAction::ToggleLeftPanel => {
                self.left_panel_visible = !self.left_panel_visible;
                ctx.notify();
            }
            WorkspaceAction::ToggleRightPanel => {
                if !self.right_panel_visible {
                    let root = self
                        .project_sidebar_model
                        .read(ctx, |model, _| model.active_cwd.clone());
                    if let Some(root) = root.filter(|root| {
                        !self.workspace_trust.is_trusted(root)
                            && super::action_projects::workspace_trust_enforced()
                    }) {
                        self.pending_workspace_trust_document = None;
                        self.pending_workspace_trust = Some(root);
                        ctx.notify();
                        return;
                    }
                }
                self.right_panel_visible = !self.right_panel_visible;
                // Record the intent: closing locks the panel shut against
                // auto-visibility, opening hands control back to it.
                self.right_panel_user_closed = !self.right_panel_visible;
                if !self.right_panel_visible {
                    // The Review dropdown anchors to the strip, which
                    // collapses with the panel — never strand it open.
                    self.review_menu_open = false;
                }
                ctx.notify();
            }
            WorkspaceAction::TrustPendingWorkspace => {
                if let Some(root) = self.pending_workspace_trust.take() {
                    let pending_document = self.pending_workspace_trust_document.take();
                    let pending_project_intent = self.pending_project_folder_intent.take();
                    self.workspace_trust.trust(&root);
                    #[cfg(not(target_family="wasm"))]
                    if self.review_setup.as_ref().is_some_and(|review| review.target.root == root) {
                        self.start_review_setup(ctx);
                        return;
                    }
                    self.right_panel_visible = true;
                    self.right_panel_user_closed = false;
                    // Force the previously deferred explorer scan now that the
                    // root has crossed the trust boundary.
                    self.document_index_root = PathBuf::new();
                    refresh_sidebar_projects(self, ctx);
                    self.language_sync_epoch = self.language_sync_epoch.wrapping_add(1);
                    self.language_document_sync = None;
                    self.language_services.take();
                    if let Some(intent) = pending_project_intent {
                        self.continue_project_folder_intent(root, intent, ctx);
                    } else if let Some(relative_path) = pending_document {
                        self.open_document_path_at_root(root, relative_path, true, ctx);
                    } else {
                        self.synchronize_language_document(ctx);
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::DismissWorkspaceTrust => {
                self.pending_workspace_trust = None;
                self.pending_workspace_trust_document = None;
                self.pending_project_folder_intent = None;
                ctx.notify();
            }
            WorkspaceAction::SetSidebarViewMode(mode) => {
                let mode = *mode;
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_view_mode = mode;
                    lp_ctx.notify();
                });
            }
            WorkspaceAction::FocusSidebarSearch => {
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_focus = SidebarFocus::Search;
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.focus(&lp.sidebar_search_editor);
                    lp_ctx.notify();
                });
                ctx.notify();
            }
            WorkspaceAction::SidebarSearchInput { text: _ } => {
                // The InputEditor owns its own text; just trigger a re-render
                // of LeftPanelView so the filtered tab list updates.
                ctx.update_view(&self.left_panel_view, |_, lp_ctx| {
                    lp_ctx.notify();
                });
            }
            WorkspaceAction::StartRenameTab { tab_id } => {
                let tab_id = *tab_id;
                if !self.tabs.iter().any(|tab| tab.id == tab_id) {
                    self.handle_action(&WorkspaceAction::SelectTab { tab_id }, ctx);
                }
                if let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) {
                    let tab_label = self
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id))
                        .map(|workspace| workspace.title.clone())
                        .unwrap_or_else(|| tab.display_label().to_string());
                    ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                        lp_ctx.update_view(
                            &lp.sidebar_rename_editor,
                            |e: &mut crate::workspace::InputEditor, e_ctx| {
                                e.set_text(&tab_label, e_ctx);
                                e.set_mode(crate::workspace::InputMode::RenameTab(tab_id));
                                // Focus the rename editor — NOT WorkspaceView, so focus stays here.
                                e_ctx.focus_self();
                            },
                        );
                        lp.sidebar_focus = SidebarFocus::Rename(tab_id);
                        lp.sidebar_menu_tab_id = None;
                        lp_ctx.notify();
                    });
                    ctx.notify();
                }
            }
            WorkspaceAction::CommitRenameTab { tab_id } => {
                let tab_id = *tab_id;
                // Read the rename buffer out of LeftPanelView, clear the editor,
                // and reset focus state — all in a single update_view call.
                let label = ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    let text = lp
                        .sidebar_rename_editor
                        .read(lp_ctx, |e: &crate::workspace::InputEditor, _| {
                            e.get_text().to_string()
                        })
                        .trim()
                        .to_string();
                    lp_ctx.update_view(
                        &lp.sidebar_rename_editor,
                        |e: &mut crate::workspace::InputEditor, e_ctx| {
                            e.set_text("", e_ctx);
                        },
                    );
                    lp.sidebar_focus = SidebarFocus::None;
                    lp_ctx.notify();
                    text
                });
                if let Some(workspace) = self
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id))
                {
                    if !label.is_empty() {
                        workspace.title = label;
                    }
                } else if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.custom_label = if label.is_empty() { None } else { Some(label) };
                }
                self.update_active_pane(ctx);
                self.sync_left_panel(ctx);
                self.sync_window_title(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::CancelRenameTab => {
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp_ctx.update_view(
                        &lp.sidebar_rename_editor,
                        |e: &mut crate::workspace::InputEditor, e_ctx| {
                            e.set_text("", e_ctx);
                        },
                    );
                    lp.sidebar_focus = SidebarFocus::None;
                    lp_ctx.notify();
                });
                self.update_active_pane(ctx);
                ctx.notify();
            }
            WorkspaceAction::MoveTabUp { tab_id } => {
                let tab_id = *tab_id;
                if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
                    if pos > 0 {
                        self.tabs.swap(pos, pos - 1);
                        ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                            lp.sidebar_menu_tab_id = None;
                            lp_ctx.notify();
                        });
                        self.sync_left_panel(ctx);
                        ctx.notify();
                    }
                }
            }
            WorkspaceAction::MoveTabDown { tab_id } => {
                let tab_id = *tab_id;
                if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
                    if pos + 1 < self.tabs.len() {
                        self.tabs.swap(pos, pos + 1);
                        ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                            lp.sidebar_menu_tab_id = None;
                            lp_ctx.notify();
                        });
                        self.sync_left_panel(ctx);
                        ctx.notify();
                    }
                }
            }
            WorkspaceAction::SetTabColor { tab_id, color } => {
                let tab_id = *tab_id;
                self.set_workspace_color(tab_id, *color, ctx);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            WorkspaceAction::CopyTabTitle { tab_id } => {
                let tab_id = *tab_id;
                if let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) {
                    ctx.clipboard().write(ClipboardContent::plain_text(
                        tab.display_label().to_string(),
                    ));
                }
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                ctx.notify();
            }
            WorkspaceAction::CopyTabCwd { tab_id } => {
                let tab_id = *tab_id;
                if let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) {
                    let cwd = self.tab_cwd(tab, ctx);
                    ctx.clipboard().write(ClipboardContent::plain_text(cwd));
                }
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                ctx.notify();
            }
            WorkspaceAction::CloseOtherTabs { tab_id } => {
                let tab_id = *tab_id;
                self.close_other_tabs_keep_id = Some(tab_id);
                ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
                    lp.sidebar_menu_tab_id = None;
                    lp_ctx.notify();
                });
                if let Some(next_id) = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id != tab_id)
                    .map(|tab| tab.id)
                {
                    self.handle_action(&WorkspaceAction::CloseTab { tab_id: next_id }, ctx);
                } else {
                    self.close_other_tabs_keep_id = None;
                }
                ctx.notify();
            }
            WorkspaceAction::DropTabIntoSplit {
                src_tab_id,
                target_tab_id,
                side,
            } => {
                self.move_tab_into_split(*src_tab_id, *target_tab_id, *side, ctx);
            }
            WorkspaceAction::SplitDividerDragged {
                split_id,
                divider_idx,
                delta_px,
                extent_px,
            } => {
                let (split_id, divider_idx) = (*split_id, *divider_idx);
                let delta_px = *delta_px;
                let extent_px = *extent_px;
                let min = self
                    .split_tree
                    .as_ref()
                    .and_then(|layout| layout.split_direction(split_id))
                    .map(|direction| match direction {
                        SplitDirection::Horizontal => MIN_PANE_WIDTH,
                        SplitDirection::Vertical => MIN_PANE_HEIGHT,
                    });
                if let (Some(layout), Some(min)) = (self.split_tree.as_mut(), min) {
                    if layout.resize_by_delta(split_id, divider_idx, delta_px, extent_px, min) {
                        ctx.notify();
                    }
                }
            }
            _ => {}
        }
    }

    /// The worktree that closing `tab_id` removes — its main checkout and its
    /// own folder — when no other tab, parked or not, still works in it.
    fn worktree_removed_on_close(
        &self,
        tab_id: usize,
    ) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let root = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.project_root.as_ref())?;
        let shared = self
            .tabs
            .iter()
            .chain(
                self.parked_workspace_tabs
                    .iter()
                    .flat_map(|surface| surface.panes.iter()),
            )
            .any(|tab| tab.id != tab_id && tab.project_root.as_ref() == Some(root));
        if shared {
            return None;
        }
        super::project_naming::worktree_main_checkout(root).map(|main| (main, root.clone()))
    }

    /// Record `tab_id` as most-recently-used (front of [`Self::tab_mru`]).
    pub(crate) fn touch_tab_mru(&mut self, tab_id: usize) {
        self.tab_mru.retain(|id| *id != tab_id);
        self.tab_mru.insert(0, tab_id);
    }

    /// Next/previous tab honoring Settings → Keyboard → Ctrl+Tab order:
    /// MRU walks recency (active leads, wraps around), Creation walks the
    /// strip left-to-right. Both dispatch `SelectTab`, which re-touches MRU.
    fn switch_tab_by_order(&mut self, ctx: &mut ViewContext<Self>, forward: bool) {
        if self.tabs.is_empty() {
            return;
        }
        let mru = crate::settings::AppSettings::global()
            .shortcuts
            .ctrl_tab_order
            == crate::settings::CtrlTabOrder::Mru;
        let next_id = if mru {
            let mut order: Vec<usize> = self
                .tab_mru
                .iter()
                .copied()
                .filter(|id| self.tabs.iter().any(|t| t.id == *id))
                .collect();
            for tab in &self.tabs {
                if !order.contains(&tab.id) {
                    order.push(tab.id);
                }
            }
            if order.is_empty() {
                return;
            }
            let pos = order
                .iter()
                .position(|id| *id == self.active_tab_id)
                .unwrap_or(0);
            let next_pos = if forward {
                (pos + 1) % order.len()
            } else {
                (pos + order.len() - 1) % order.len()
            };
            order[next_pos]
        } else {
            let pos = self
                .tabs
                .iter()
                .position(|t| t.id == self.active_tab_id)
                .unwrap_or(0);
            let next_pos = if forward {
                (pos + 1) % self.tabs.len()
            } else if pos == 0 {
                self.tabs.len() - 1
            } else {
                pos - 1
            };
            self.tabs[next_pos].id
        };
        self.handle_action(&WorkspaceAction::SelectTab { tab_id: next_id }, ctx);
    }
}
