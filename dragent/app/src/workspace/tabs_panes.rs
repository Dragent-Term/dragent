//! Pane and split management methods for workspace tabs.
//!
//! Responsible for: active pane sync, split mutations, pane titles/colors,
//! editor pane management, left panel sync.
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
    clone_target_name, default_projects_dir, is_default_work_label, should_replace_project_tab_label,
    valid_project_name,
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

/// How long bursts of pane events are gathered into one sidebar sync or
/// session save: about one frame, so the result looks immediate.
const PANE_EVENT_COALESCE: Duration = Duration::from_millis(16);

impl WorkspaceView {
    /// Project panes retain their root; standalone terminals follow the shell's CWD.
    pub(crate) fn sidebar_context_root(&self, ctx: &ViewContext<Self>) -> Option<PathBuf> {
        if self.project_start_visible {
            return self.selected_project_root().filter(|root| root.is_dir());
        }
        if self.document_center_open {
            return self
                .document_owner_root
                .clone()
                .filter(|root| root.is_dir());
        }
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
            return match &tab.pane {
                WorkspacePane::Terminal(_) => tab
                    .project_root
                    .clone()
                    .or_else(|| Some(tab.pane.cwd(ctx)))
                    .filter(|root| root.is_dir()),
                // A browser stays scoped to the project it was opened in.
                WorkspacePane::Browser(_) => tab.project_root.clone().filter(|root| root.is_dir()),
                WorkspacePane::Settings(_) => None,
            };
        }
        self.selected_project_root().filter(|root| root.is_dir())
    }

    /// Warm the background git cache for every visible tab's checkout, so
    /// `refresh_all_tab_titles` (cache-only, never git on the UI thread)
    /// rarely renders a tab without its branch suffix. Fire-and-forget:
    /// `refresh_worktree_git_status` dedups inflight roots, and completions
    /// re-sync titles via `on_branch_changed` / `sync_left_panel`.
    pub(crate) fn ensure_tab_branch_cache(&self, ctx: &mut ViewContext<Self>) {
        let mut missing: Vec<std::path::PathBuf> = Vec::new();
        for tab in &self.tabs {
            let Some(root) = tab.project_root.as_ref() else {
                continue;
            };
            if self.worktree_git_cache.contains_key(root) {
                continue;
            }
            let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
            if self.worktree_git_cache.contains_key(&canonical)
                || self.worktree_git_inflight.borrow().contains(&canonical)
                || missing.contains(&canonical)
            {
                continue;
            }
            missing.push(canonical);
        }
        for root in missing {
            self.refresh_worktree_git_status(root, ctx);
        }
    }

    pub(crate) fn update_active_pane(&mut self, ctx: &mut ViewContext<Self>) {
        self.update_active_pane_with_focus(true, ctx);
    }

    /// A native focus event updates context without queuing another focus
    /// request, which could keep two panes redirecting each other forever.
    pub(crate) fn update_active_pane_with_focus(
        &mut self,
        take_focus: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        // Warm branch data for the tab being switched to (and its strip
        // neighbours) before titles render, so a brand-new checkout still
        // gets its suffix as soon as the background query lands.
        self.ensure_tab_branch_cache(ctx);
        if let Some(color) = self.workspace_accent() {
            let theme = self.workspace_theme();
            for tab in &mut self.tabs {
                if tab.color != Some(color) { tab.color = Some(color); tab.pane.set_theme(theme,ctx); }
            }
        }
        // Focusing a terminal pane always releases editor focus with it.
        self.editor_pane_focused = false;
        let active = self.tabs.iter().find(|t| t.id == self.active_tab_id);
        let Some(tab) = active else {
            return;
        };
        let pane_id = tab.pane.id();
        let workspace_context_root = self.sidebar_context_root(ctx);
        let git_owner = self.git_terminal_for(tab).and_then(|(owner_id, handle)| {
            let owner = self.tabs.iter().find(|other| other.id == owner_id)?;
            Some((owner.project_root.clone(), handle))
        });
        let (terminal_view_id, git_model, git_cwd) = match git_owner {
            Some((project_root, handle)) => {
                let status_model = handle.read(ctx, |view, _| view.git_repo_status());
                let cwd = project_root
                    .unwrap_or_else(|| status_model.read(ctx, |model, _| model.cwd.clone()));
                let root = super::project_sidebar::find_git_root(&cwd).unwrap_or(cwd);
                (Some(handle.id()), Some(status_model), Some(root))
            }
            None => (None, None, None),
        };
        {
            let armed = self.git_panel_state.borrow().armed_discard.clone();
            *self.git_panel_state.borrow_mut() = super::project_sidebar::GitPanelState {
                git_model,
                terminal_view_id,
                cwd: git_cwd,
                armed_discard: armed,
            };
        }
        self.active_pane_id = Some(pane_id);
        if take_focus {
            tab.pane.focus(ctx);
        }
        self.sync_pane_activity(ctx);
        // Viewing a pane marks its agent completion seen, clearing a review
        // dot on the next sync. Only re-sync when something was actually
        // marked, so focus moves stay cheap.
        let seen_marked = self.mark_pane_seen(tab.pane.pane_uuid(ctx));
        if seen_marked {
            self.sync_left_panel(ctx);
        }

        refresh_sidebar_projects(self, ctx);
        apply_sidebar_visibility(self, ctx);
        let psv = self.project_sidebar_view.clone();
        ctx.update_view(&psv, |sidebar, sidebar_ctx| {
            sidebar.refresh_workspace_context(workspace_context_root, sidebar_ctx);
            sidebar_ctx.notify();
        });
        self.sync_window_title(ctx);
        ctx.notify();
    }

    /// Keep expensive visual work attached only to panes that are on screen.
    /// A split group is fully visible; otherwise only the selected tab is.
    pub(crate) fn sync_pane_activity(&self, ctx: &mut ViewContext<Self>) {
        let visible_split = self
            .split_tree
            .as_ref()
            .filter(|group| group.contains(self.active_tab_id));
        for tab in &self.tabs {
            let visible = !self.project_start_visible
                && visible_split
                    .map(|group| group.contains(tab.id))
                    .unwrap_or(tab.id == self.active_tab_id);
            tab.pane.set_workspace_active(visible, ctx);
        }
        for surface in &self.parked_workspace_tabs {
            if surface.id != self.active_workspace_tab_id {
                for tab in &surface.panes {
                    tab.pane.set_workspace_active(false, ctx);
                }
            }
        }
        for tab in &self.document_terminals {
            let visible = !self.project_start_visible
                && self.document_terminal_visible
                && (self.document_terminal_active_id == Some(tab.id)
                    || self.document_terminal_split_id == Some(tab.id));
            tab.pane.set_workspace_active(visible, ctx);
        }
    }

    /// Set the native window title so this window has a meaningful, live entry in
    /// the macOS Dock's open-windows list (and the Window menu).
    pub(crate) fn sync_window_title(&self, ctx: &mut ViewContext<Self>) {
        let title = self
            .tabs
            .iter()
            .find(|t| t.id == self.active_tab_id)
            .map(|tab| self.window_title_for_tab(tab, ctx))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SESSION_TITLE.to_string());
        let window_id = ctx.window_id();
        ctx.windows().set_window_title(window_id, &title);
    }

    /// Project windows identify the main project and its worktree. Tabs keep
    /// their own task labels; non-project windows retain the terminal title.
    pub(crate) fn window_title_for_tab(&self, tab: &PaneTab, app: &AppContext) -> String {
        if tab.pane.is_settings() {
            return "Settings".to_string();
        }
        if let Some(root) = tab.project_root.as_ref() {
            let main =
                super::project_naming::worktree_main_checkout(root).unwrap_or_else(|| root.clone());
            let project = super::project_naming::worktree_project_name(&main);
            if main != *root {
                let worktree = super::project_naming::linked_worktree_branch(root)
                    .or_else(|| {
                        root.file_name()
                            .and_then(|name| name.to_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "Worktree".to_string());
                return format!("{project} · {worktree}");
            }
            return project;
        }
        if let Some(label) = tab.custom_label.as_deref() {
            if !label.trim().is_empty() {
                return label.to_string();
            }
        }
        // Just the current folder's name (e.g. "dragent"), not the full path;
        // "~" when sitting in the home directory.
        let cwd = tab.pane.cwd(app);
        let dir = if dirs::home_dir().is_some_and(|home| home == cwd) {
            "~".to_string()
        } else {
            cwd.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "~".to_string())
        };
        // First token of the running command (e.g. "cargo" from "cargo build").
        let running_cmd = tab
            .pane
            .last_command(app)
            .filter(|_| tab.pane.is_running(app))
            .and_then(|c| c.split_whitespace().next().map(str::to_string))
            .filter(|c| !c.is_empty());
        match running_cmd {
            Some(cmd) => format!("{dir} — {cmd}"),
            None => dir,
        }
    }

    pub(crate) fn run_on_active_terminal(
        &self,
        ctx: &mut ViewContext<Self>,
        f: impl FnOnce(&ViewHandle<TerminalView>, &mut ViewContext<Self>),
    ) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == self.active_tab_id) else {
            return;
        };
        let WorkspacePane::Terminal(pane) = &tab.pane else {
            return;
        };
        f(pane, ctx);
    }

    /// Like [`Self::run_on_active_terminal`], for Source Control actions:
    /// while a browser is active they act on the terminal whose repository
    /// the panel shows (see `git_terminal_for`).
    pub(crate) fn run_on_git_terminal(
        &self,
        ctx: &mut ViewContext<Self>,
        f: impl FnOnce(&ViewHandle<TerminalView>, &mut ViewContext<Self>),
    ) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == self.active_tab_id) else {
            return;
        };
        if let Some((_, pane)) = self.git_terminal_for(tab) {
            f(&pane, ctx);
        }
    }

    pub(crate) fn split_mouse_state(
        states: &RefCell<HashMap<usize, MouseStateHandle>>,
        tab_id: usize,
    ) -> MouseStateHandle {
        states
            .borrow_mut()
            .entry(tab_id)
            .or_insert_with(MouseStateHandle::default)
            .clone()
    }

    pub(crate) fn dissolve_split_if_needed(&mut self) {
        if let Some(layout) = &self.split_tree {
            if layout.pane_count() <= 1 {
                // The survivor returns to the sidebar: a dissolved split
                // must never strand its last tab invisible.
                let survivor = layout.panes().into_iter().next();
                // The editor is a first-class standalone surface. When its
                // neighboring terminal closes, retain the one-pane layout so
                // the editor expands to the full center width.
                if survivor == Some(EDITOR_PANE_ID) && self.document_center_open {
                    self.editor_pane_focused = true;
                    self.prune_split_handles();
                    return;
                }
                self.split_tree = None;
                self.prune_split_handles();
                if let Some(id) = survivor {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                        tab.hidden_from_sidebar = false;
                    }
                }
            }
        }
        // The editor pane only exists inside a split: no tree, no editor.
        if !self.editor_pane_in_layout() {
            self.document_center_open = false;
            self.editor_pane_focused = false;
        }
    }

    pub(crate) fn remove_tab_from_split(&mut self, tab_id: usize) {
        if let Some(layout) = &mut self.split_tree {
            layout.close_pane(tab_id);
        }
        self.dissolve_split_if_needed();
    }

    /// Drop divider drag state for splits that no longer exist. Weights
    /// live in the tree; handles are transient interaction state.
    pub(crate) fn prune_split_handles(&mut self) {
        let live: std::collections::HashSet<(SplitId, usize)> = self
            .split_tree
            .as_ref()
            .map(|layout| {
                let mut out = std::collections::HashSet::new();
                fn walk(node: &LayoutNode, out: &mut std::collections::HashSet<(SplitId, usize)>) {
                    if let LayoutNode::Split { id, children, .. } = node {
                        for idx in 0..children.len().saturating_sub(1) {
                            out.insert((*id, idx));
                        }
                        for child in children {
                            walk(child, out);
                        }
                    }
                }
                walk(layout.root(), &mut out);
                out
            })
            .unwrap_or_default();
        self.split_drag_handles
            .borrow_mut()
            .retain(|key, _| live.contains(key));
    }

    /// The layout tree stripped for persistence: the editor pane is a
    /// runtime overlay (rebuilt from restored documents), so it never
    /// reaches storage. `None` when nothing splittable remains.
    pub(crate) fn layout_for_save(&self) -> Option<PaneLayout> {
        Self::layout_for_save_of(self.split_tree.as_ref())
    }

    pub(crate) fn layout_for_save_of(layout: Option<&PaneLayout>) -> Option<PaneLayout> {
        let layout = layout?;
        if !layout.contains(EDITOR_PANE_ID) {
            return Some(layout.clone());
        }
        if layout.pane_count() <= 2 {
            return None;
        }
        let mut stripped = layout.clone();
        stripped.close_pane(EDITOR_PANE_ID);
        if stripped.pane_count() <= 1 {
            None
        } else {
            Some(stripped)
        }
    }

    pub(crate) fn editor_pane_in_layout(&self) -> bool {
        self.split_tree
            .as_ref()
            .is_some_and(|layout| layout.contains(EDITOR_PANE_ID))
    }

    /// Ensure the editor has its own pane beside the active terminal
    /// instead of replacing it. Idempotent; no-ops when the active tab is
    /// missing from the tab list.
    pub(crate) fn ensure_editor_pane(&mut self) {
        if self.editor_pane_in_layout() {
            return;
        }
        if !self.tabs.iter().any(|tab| tab.id == self.active_tab_id) {
            let mut layout = PaneLayout::new(EDITOR_PANE_ID);
            layout.focus(EDITOR_PANE_ID);
            self.split_tree = Some(layout);
            self.prune_split_handles();
            return;
        }
        match &mut self.split_tree {
            None => {
                let mut layout = PaneLayout::new(self.active_tab_id);
                layout.split_pane(self.active_tab_id, EDITOR_PANE_ID, SplitSide::Right);
                layout.focus(EDITOR_PANE_ID);
                self.split_tree = Some(layout);
            }
            Some(layout) => {
                layout.split_pane(self.active_tab_id, EDITOR_PANE_ID, SplitSide::Right);
            }
        }
        self.prune_split_handles();
    }

    /// Show the document surface: flag it visible and ensure it owns a
    /// pane beside the active terminal instead of replacing it. Focus
    /// follows the editor (the file open was an explicit request to see
    /// it). All `document_center_open = true` sites route through here.
    pub(crate) fn show_document_center(&mut self) {
        self.project_start_visible = false;
        self.document_center_open = true;
        self.ensure_editor_pane();
        self.editor_pane_focused = self.editor_pane_in_layout();
    }

    /// Close the editor pane without touching any terminal. The layout
    /// dissolves when nothing splittable remains.
    pub(crate) fn remove_editor_pane(&mut self) {
        if let Some(layout) = &mut self.split_tree {
            layout.close_pane(EDITOR_PANE_ID);
        }
        self.editor_pane_focused = false;
        self.dissolve_split_if_needed();
        if !self.editor_pane_in_layout() {
            self.document_center_open = false;
        }
    }

    /// split-tab split: create a new tab and show it beside or below the active tab.
    /// Split a specific tab (not necessarily the active one): point the
    /// active tab at it, then divide its pane. Deliberately NOT the full
    /// card-click selection path — splitting must not swap workspace work
    /// state, open work surfaces, or save sessions on the way through;
    /// `split_tab` performs its own switch for the new tab.
    pub(crate) fn split_specific_tab(
        &mut self,
        tab_id: usize,
        direction: SplitDirection,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.tabs.iter().any(|tab| tab.id == tab_id) {
            return;
        }
        self.active_tab_id = tab_id;
        // A pane-specific header action must always divide that terminal,
        // even if the editor was the previously focused pane.
        self.editor_pane_focused = false;
        self.split_tab(direction, ctx);
    }

    pub(crate) fn split_tab(&mut self, direction: SplitDirection, ctx: &mut ViewContext<Self>) {
        let current_tab_id = self.active_tab_id;
        let has_active_terminal = self.tabs.iter().any(|tab| tab.id == current_tab_id);
        let split_target_id = if self.editor_pane_focused && self.editor_pane_in_layout() {
            EDITOR_PANE_ID
        } else if has_active_terminal {
            current_tab_id
        } else if self.editor_pane_in_layout() {
            EDITOR_PANE_ID
        } else {
            return;
        };
        // A split is another view of the same workspace, so it must inherit the
        // active terminal's live directory rather than starting at the home/root
        // directory. This also works after `cd` because pane.cwd() reflects the
        // shell hook state, not the directory from when the tab was created.
        let split_cwd = self
            .tabs
            .iter()
            .find(|tab| tab.id == current_tab_id)
            .map(|tab| tab.pane.cwd(ctx))
            .filter(|cwd| cwd.is_dir())
            .or_else(|| self.document_owner_root.clone())
            .or_else(|| self.selected_project_root())
            .unwrap_or_else(default_pane_cwd);
        let split_project_root = self
            .tabs
            .iter()
            .find(|tab| tab.id == current_tab_id)
            .and_then(|tab| tab.project_root.clone())
            .or_else(|| self.document_owner_root.clone())
            .or_else(|| self.selected_project_root());
        let mut new_tab = self.create_tab_with_cwd(ctx, None, None, split_cwd, split_project_root);
        // Split panes live in the mosaic, not the sidebar: a card per split
        // would duplicate every division.
        new_tab.hidden_from_sidebar = true;
        let new_tab_id = new_tab.id;
        self.tabs.push(new_tab);

        // The new pane opens after the target in the requested orientation:
        // Horizontal splits read left-to-right, vertical top-to-bottom.
        let side = match direction {
            SplitDirection::Horizontal => SplitSide::Right,
            SplitDirection::Vertical => SplitSide::Down,
        };
        match &mut self.split_tree {
            None => {
                let mut layout = PaneLayout::new(split_target_id);
                layout.split_pane(split_target_id, new_tab_id, side);
                layout.focus(new_tab_id);
                self.split_tree = Some(layout);
            }
            Some(layout) => {
                // Divide the target pane in place: the rest of the tree
                // keeps its arrangement while the new pane takes half of
                // the target pane's share.
                layout.split_pane(split_target_id, new_tab_id, side);
            }
        }
        self.prune_split_handles();

        self.active_tab_id = new_tab_id;
        self.switch_workspace_work_for_tab(new_tab_id, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
            lp.sidebar_menu_tab_id = None;
            lp_ctx.notify();
        });
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// Place an EXISTING tab's session into a split at `target`'s `side` edge — a
    /// reparent, not a fresh shell. `src` keeps its tab id, so its persistence /
    /// focus subscription stays valid; we only change which tabs the split group
    /// shows together. Drops join same-orientation splits or wrap the target
    /// in a nested split (never flattened).
    pub(crate) fn move_tab_into_split(
        &mut self,
        src: usize,
        target: usize,
        side: DropSide,
        ctx: &mut ViewContext<Self>,
    ) {
        self.drag_in_progress = false;
        let both_exist =
            self.tabs.iter().any(|t| t.id == src) && self.tabs.iter().any(|t| t.id == target);
        if src == target || !both_exist {
            ctx.notify();
            return;
        }

        // Drop edges map to orientations directly now: Left/Right split
        // horizontally (side-by-side), Top/Bottom vertically (stacked).
        let side = match side {
            DropSide::Left => SplitSide::Left,
            DropSide::Right => SplitSide::Right,
            DropSide::Top => SplitSide::Up,
            DropSide::Bottom => SplitSide::Down,
        };

        if self.split_tree.is_none() {
            let (first, second) = if side.before() {
                (src, target)
            } else {
                (target, src)
            };
            let mut layout = PaneLayout::new(first);
            layout.split_pane(first, second, side);
            self.split_tree = Some(layout);
        } else if let Some(layout) = &mut self.split_tree {
            // Same-orientation drops join the target's split; cross-axis
            // drops wrap the target pane in a nested split. Never flattened.
            layout.move_pane_oriented(src, target, side);
        }
        self.prune_split_handles();
        self.dissolve_split_if_needed();

        self.active_tab_id = src;
        self.switch_workspace_work_for_tab(src, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// Pop `tab_id` out of the split WITHOUT closing it (its session stays alive
    /// and it remains a normal sidebar tab). Focus shifts to a remaining split
    /// member so the split (or, for a 2-pane split, its partner) keeps showing.
    pub(crate) fn pop_tab_out_of_split(&mut self, tab_id: usize, ctx: &mut ViewContext<Self>) {
        // The other members of the split, captured before we mutate the group.
        // The editor pane is not a tab and can never own the active tab id.
        let others: Vec<usize> = self
            .split_tree
            .as_ref()
            .map(|layout| {
                layout
                    .panes()
                    .into_iter()
                    .filter(|id| *id != tab_id && *id != EDITOR_PANE_ID)
                    .collect()
            })
            .unwrap_or_default();
        if others.is_empty() {
            return; // not in a split — nothing to pop out
        }

        self.remove_tab_from_split(tab_id); // removes from group, dissolves if ≤1 remains
                                            // Popped tabs return to the sidebar instead of stranding invisible.
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.hidden_from_sidebar = false;
        }
        self.active_tab_id = others[0];
        self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    pub(crate) fn exit_split_unless_contains(&mut self, tab_id: usize) {
        if let Some(tree) = &self.split_tree {
            if !tree.contains(tab_id) {
                self.split_tree = None;
            }
        }
    }

    pub(crate) fn title_pane_for_tab(&self, tab: &PaneTab) -> WorkspacePane {
        tab.pane.clone()
    }

    pub(crate) fn apply_pane_title(&mut self, pane_id: EntityId, title: String) {
        let title = if title.is_empty() {
            DEFAULT_SESSION_TITLE.to_string()
        } else {
            title
        };

        for tab in &mut self.tabs {
            if tab.pane.id() == pane_id {
                tab.title = title;
                break;
            }
        }
    }

    pub(crate) fn refresh_all_tab_titles(&mut self, ctx: &mut ViewContext<Self>) {
        // Branch suffixes resolve from the background worktree git cache —
        // never git on the UI thread (main-thread hang reports sat in exactly
        // this path: sync `local_branch_name` → blocking host RPC → 5s stall,
        // freezing scroll/input). Missing entries trigger one off-thread
        // refresh and render without suffix until it lands; completion
        // re-syncs titles via `on_branch_changed` / `sync_left_panel`.
        // Output format is unchanged: `{title} · {branch}` or plain `title`.
        let mut missing: Vec<std::path::PathBuf> = Vec::new();
        let updates: Vec<(EntityId, String)> = self
            .tabs
            .iter()
            .map(|tab| {
                let pane = self.title_pane_for_tab(tab);
                let title = pane.display_title(ctx);
                // Each tab under a project is a worktree: suffix the cached
                // branch so the tab reads as its checkout. Titles are
                // recomputed from the pane every refresh, so the suffix never
                // accumulates; user-set custom labels render instead of the
                // title and are untouched.
                let branch = tab.project_root.as_ref().and_then(|root| {
                    let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
                    let cached = self
                        .worktree_git_cache
                        .get(&canonical)
                        .or_else(|| self.worktree_git_cache.get(root))
                        .and_then(|status| status.branch.clone());
                    if cached.is_none()
                        && !missing.contains(&canonical)
                        && !self.worktree_git_inflight.borrow().contains(&canonical)
                    {
                        missing.push(canonical);
                    }
                    cached
                });
                let title = match branch {
                    Some(branch) => format!("{title} · {branch}"),
                    None => title,
                };
                (pane.id(), title)
            })
            .collect();

        for (pane_id, title) in updates {
            self.apply_pane_title(pane_id, title);
        }
        for root in missing {
            self.refresh_worktree_git_status(root, ctx);
        }
    }

    /// The current workspace owns its accent across all of its tab layouts.
    pub(crate) fn workspace_accent(&self) -> Option<warpui::color::ColorU> {
        self.tabs.iter().find_map(|tab| tab.color).or_else(|| {
            let workspace = self.workspaces.iter().find(|w| w.tab_ids.contains(&self.active_workspace_tab_id))?;
            self.parked_workspace_tabs.iter().filter(|t| workspace.tab_ids.contains(&t.id))
                .flat_map(|t| &t.panes).find_map(|pane| pane.color)
        })
    }

    pub(crate) fn document_workspace_theme(&self, root: &Path) -> Theme {
        let color = self.workspaces.iter().filter(|w| root.starts_with(&w.root))
            .max_by_key(|w| w.root.components().count()).and_then(|workspace| {
                workspace.tab_ids.iter().find_map(|id| {
                    if *id == self.active_workspace_tab_id { self.tabs.iter().find_map(|tab| tab.color) }
                    else { self.parked_workspace_tabs.iter().find(|tab| tab.id == *id)
                        .and_then(|tab| tab.panes.iter().find_map(|pane| pane.color)) }
                })
            });
        Self::theme_with_workspace_color(self.theme, color)
    }

    pub(crate) fn workspace_theme(&self) -> Theme {
        Self::theme_with_workspace_color(self.theme, self.workspace_accent())
    }

    fn theme_with_workspace_color(mut theme: Theme, color: Option<warpui::color::ColorU>) -> Theme {
        if let Some(color) = color {
            theme.accent = color;
            let bg = theme.background;
            theme.background = warpui::color::ColorU::new(
                bg.r.saturating_add((color.r as f32 * 0.06) as u8),
                bg.g.saturating_add((color.g as f32 * 0.06) as u8),
                bg.b.saturating_add((color.b as f32 * 0.06) as u8), bg.a,
            );
        }
        theme
    }

    pub(crate) fn sync_pane_colors_for_tab(&mut self, tab_idx: usize, ctx: &mut ViewContext<Self>) {
        let tab = &self.tabs[tab_idx];
        tab.pane.set_theme(Self::theme_with_workspace_color(self.theme, tab.color), ctx);
    }

    /// Sidebar colors belong to workspaces, including their parked tab layouts.
    pub(crate) fn set_workspace_color(&mut self, pane_id: usize, color: Option<warpui::color::ColorU>, ctx: &mut ViewContext<Self>) {
        let surface = if self.tabs.iter().any(|t| t.id == pane_id) {
            Some(self.active_workspace_tab_id)
        } else {
            self.parked_workspace_tabs.iter().find(|t| t.panes.iter().any(|p| p.id == pane_id)).map(|t| t.id)
        };
        let Some(surface) = surface else { return; };
        let surfaces = self.workspaces.iter().find(|w| w.tab_ids.contains(&surface))
            .map(|w| w.tab_ids.clone()).unwrap_or_else(|| vec![surface]);
        let theme = Self::theme_with_workspace_color(self.theme, color);
        if surfaces.contains(&self.active_workspace_tab_id) {
            for tab in &mut self.tabs { tab.color = color; tab.pane.set_theme(theme,ctx); }
        }
        for tab in self.parked_workspace_tabs.iter_mut().filter(|t| surfaces.contains(&t.id)) {
            for pane in &mut tab.panes { pane.color = color; pane.pane.set_theme(theme,ctx); }
        }
        for tab in &self.document_terminals {
            let owner = self.workspaces.iter().filter(|w| tab.owner_root.starts_with(&w.root))
                .max_by_key(|w| w.root.components().count());
            if owner.is_some_and(|w| w.tab_ids.iter().any(|id| surfaces.contains(id))) {
                tab.pane.set_theme(theme,ctx);
            }
        }
    }

    /// Restore each workspace's own colors without changing the app theme.
    pub(crate) fn sync_all_pane_colors(&mut self, ctx: &mut ViewContext<Self>) {
        for i in 0..self.tabs.len() { self.sync_pane_colors_for_tab(i,ctx); }
        for surface in &self.parked_workspace_tabs {
            for tab in &surface.panes {
                tab.pane.set_theme(Self::theme_with_workspace_color(self.theme,tab.color),ctx);
            }
        }
        for tab in &self.document_terminals { tab.pane.set_theme(self.document_workspace_theme(&tab.owner_root),ctx); }
    }

    /// Include project headings inferred from workspace tabs as well as projects
    /// opened directly. Both kinds appear in the sidebar and must be hideable.
    pub(crate) fn sidebar_project_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for root in &self.open_project_roots {
            let project = self.project_root_of(root);
            if !roots.contains(&project) {
                roots.push(project);
            }
        }
        for workspace in &self.workspaces {
            let sidebar_root = self.project_root_of(&workspace.root);
            if !roots.contains(&sidebar_root) {
                roots.push(sidebar_root);
            }
        }
        roots
    }

    /// [`Self::sync_left_panel`] once for a burst of pane events. Status,
    /// title and agent-streaming events arrive many times per frame while
    /// agents work, and each sync re-resolves every workspace's project root
    /// on disk: syncing per event saturated the UI thread (main-thread hang
    /// reports sat in exactly this path). One pass per frame shows the same
    /// state.
    pub(crate) fn schedule_left_panel_sync(&mut self, ctx: &mut ViewContext<Self>) {
        if self.left_panel_sync_scheduled {
            return;
        }
        self.left_panel_sync_scheduled = true;
        ctx.spawn(Timer::after(PANE_EVENT_COALESCE), |view, _, ctx| {
            view.left_panel_sync_scheduled = false;
            view.sync_left_panel(ctx);
            ctx.notify();
        });
    }

    /// [`Self::save_session`] once for a burst of pane events; each save
    /// serializes every tab.
    pub(crate) fn schedule_session_save(&mut self, ctx: &mut ViewContext<Self>) {
        if self.session_save_scheduled {
            return;
        }
        self.session_save_scheduled = true;
        ctx.spawn(Timer::after(PANE_EVENT_COALESCE), |view, _, ctx| {
            view.session_save_scheduled = false;
            view.save_session(ctx);
        });
    }

    /// Push a snapshot of all tabs (plus active_tab_id / theme) into the
    /// LeftPanelView so it can render without touching WorkspaceView's fields.
    /// Call this after any mutation that changes what the sidebar should show.
    pub(crate) fn sync_left_panel(&self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.project_sidebar_view, |view, vc| {
            if view.diff_comment.as_ref().and_then(|c| c.workspace.as_ref()).is_some_and(|scope| scope != &(self.organisation_id.clone(), self.active_workspace_tab_id)) {
                view.diff_comment = None;
                view.diff_comment_drag = None;
                vc.notify();
            }
        });
        let sidebar_project_roots = self.sidebar_project_roots();
        // Sidebar navigation is a projection of workspace records only.
        // Neither pane titles nor agent runtime state participate here.
        let mut tab_data: Vec<left_panel::TabData> = self
            .workspaces
            .iter()
            .filter_map(|workspace| {
                let panes = if workspace.active_tab_id == self.active_workspace_tab_id {
                    &self.tabs
                } else {
                    &self
                        .parked_workspace_tabs
                        .iter()
                        .find(|surface| surface.id == workspace.active_tab_id)?
                        .panes
                };
                let pane = panes.first()?;
                let project = self.project_root_of(&workspace.root);
                if self.workspace_trust.is_hidden(&project) {
                    return None;
                }
                // Agent lifecycle across every surface of this workspace —
                // active and parked — folded by highest priority, so a
                // background failure or permission still surfaces.
                let mut agents = Vec::new();
                let mut agent_status = super::worktree_status::WorktreeAgentStatus::Idle;
                for surface_id in &workspace.tab_ids {
                    let surface_panes = if *surface_id == self.active_workspace_tab_id {
                        Some(&self.tabs)
                    } else {
                        self.parked_workspace_tabs
                            .iter()
                            .find(|surface| surface.id == *surface_id)
                            .map(|surface| &surface.panes)
                    };
                    if let Some(surface_panes) = surface_panes {
                        for pane_tab in surface_panes {
                            let stop = self.agent_stop_state.get(&pane_tab.pane.pane_uuid(ctx));
                            let status = super::worktree_status::pane_agent_status(
                                &pane_tab.pane,
                                stop,
                                ctx,
                            );
                            let tool = pane_tab.pane.active_ai_tool(ctx).or_else(|| {
                                pane_tab
                                    .pane
                                    .agent_id(ctx)
                                    .as_deref()
                                    .and_then(crate::console::ai_toolbar::AiTool::from_command)
                            });
                            if tool.is_some()
                                || pane_tab.pane.agent_id(ctx).is_some()
                                || pane_tab.pane.tab_session_kind(ctx)
                                    == crate::agent::TabSessionKind::Agent
                            {
                                let chip = super::workspace_tab_strip::chip_for_pane(
                                    pane_tab,
                                    ctx,
                                    &self.cached_custom_agents,
                                    self.theme.is_dark,
                                );
                                let image = match chip.icon {
                                    super::workspace_tab_strip::SurfaceChipIcon::Asset(path) => {
                                        Some(super::AgentImage::Bundled(path))
                                    }
                                    super::workspace_tab_strip::SurfaceChipIcon::File(path) => {
                                        Some(super::AgentImage::File(path))
                                    }
                                    _ => None,
                                };
                                agents.push(super::worktree_status::SidebarAgent {
                                    image,
                                    pane_id: pane_tab.pane.pane_uuid(ctx),
                                    tool,
                                    cli: tool.is_some(),
                                    activity_known: tool.is_none(),
                                    status: status.clone(),
                                    base_status: status.clone(),
                                });
                            }
                            agent_status = super::worktree_status::WorktreeAgentStatus::pick_higher(
                                agent_status,
                                status,
                            );
                        }
                    }
                }
                // Per-worktree git + PR state resolve from caches (populated
                // off-thread); the row hides whatever is missing or clean.
                let git_root = workspace
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| workspace.root.clone());
                let git = self.worktree_git_cache.get(&git_root).cloned();
                if git.is_none() {
                    self.refresh_worktree_git_status(git_root.clone(), ctx);
                }
                let git = git.unwrap_or_default();
                let pr = self
                    .review_pr_cache
                    .get(&git_root)
                    .and_then(super::worktree_status::pr_badge_from_cached);
                Some(left_panel::TabData {
                    id: pane.id,
                    workspace_id: Some(workspace.id),
                    label: workspace.title.clone(),
                    title: workspace.title.clone(),
                    branch_subtitle: None,
                    group: self
                        .workspace_trust
                        .workspace_group(&workspace.root)
                        .map(str::to_string),
                    color: panes.iter().find_map(|pane| pane.color),
                    is_running: false,
                    cwd: workspace.root.to_string_lossy().into_owned(),
                    project_key: Some(project.to_string_lossy().into_owned()),
                    project_label: project
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned()),
                    purpose_label: workspace.title.clone(),
                    is_work_item: true,
                    ai_tool: None,
                    last_command: None,
                    is_shared: false,
                    agent_status,
                    agents,
                    diff_files: git.files_changed + git.untracked,
                    diff_added: git.insertions,
                    diff_deleted: git.deletions,
                    pr,
                    git_root: Some(git_root),
                    branch: git.branch,
                })
            })
            .collect();
        let projects = sidebar_project_roots
            .iter()
            .filter(|root| !self.workspace_trust.is_hidden(root))
            .map(|root| left_panel::ProjectData {
                key: root.to_string_lossy().into_owned(),
                groups: self.workspace_trust.groups(root),
                label: super::project_naming::sidebar_project_label(root),
                icon: super::project_icons::icon_for(root),
            })
            .collect();
        let hidden_projects = sidebar_project_roots
            .iter()
            .filter(|root| self.workspace_trust.is_hidden(root))
            .map(|root| left_panel::ProjectData {
                key: root.to_string_lossy().into_owned(),
                groups: self.workspace_trust.groups(root),
                label: super::project_naming::sidebar_project_label(root),
                icon: None,
            })
            .collect();
        #[cfg(not(target_family = "wasm"))]
        let automation_failures = self.automation_failures(ctx);
        #[cfg(target_family = "wasm")]
        let automation_failures = 0;
        let teams_needing_you = self.teams_needing_you;
        let active_tab_id = tab_data
            .iter()
            .find(|row| {
                row.workspace_id.is_some_and(|id| {
                    self.workspaces.iter().any(|workspace| {
                        workspace.id == id
                            && workspace.tab_ids.contains(&self.active_workspace_tab_id)
                    })
                })
            })
            .map(|row| row.id)
            .unwrap_or(self.active_tab_id);
        let workspace_visible = !(self.settings_open
            || self.automations_open
            || self.teams_open
            || self.tasks_open
            || self.pull_requests_open
            || self.project_start_visible);
        let theme = self.theme;
        let content_theme = if workspace_visible { self.workspace_theme() } else { self.theme };
        // The pane the person actually opened: only its Review dot clears.
        // Other tabs in the same workspace keep theirs until opened.
        let active_pane_uuid = self
            .tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .map(|tab| tab.pane.pane_uuid(ctx));
        ctx.update_view(&self.left_panel_view, |lp, lp_ctx| {
            // Preserve the latest hook snapshot between ordinary UI syncs.
            for tab in &mut tab_data {
                for agent in &mut tab.agents {
                    if agent.cli {
                        if let Some(old) = lp
                            .tabs
                            .iter()
                            .flat_map(|tab| &tab.agents)
                            .find(|old| old.pane_id == agent.pane_id && old.activity_known)
                        {
                            agent.status = old.status.clone();
                            agent.activity_known = true;
                        }
                    }
                }
                tab.agent_status = tab.agents.iter().fold(
                    super::worktree_status::WorktreeAgentStatus::Idle,
                    |status, agent| {
                        super::worktree_status::WorktreeAgentStatus::pick_higher(
                            status,
                            agent.status.clone(),
                        )
                    },
                );
            }
            let newly_active = workspace_visible && lp.active_tab_id != Some(active_tab_id);
            lp.coordination = super::panel_coordination::assign(&self.coordination_groups, &tab_data);
            lp.tabs = tab_data;
            if newly_active {
                if let Some(pane) = active_pane_uuid {
                    lp.acknowledge_pane_activity(pane);
                }
            }
            lp.automation_failures = automation_failures;
            lp.teams_needing_you = teams_needing_you;
            lp.nav_active = if self.settings_open {
                super::panel_nav::SidebarNavActive::None
            } else if self.automations_open {
                super::panel_nav::SidebarNavActive::Automations
            } else if self.teams_open {
                super::panel_nav::SidebarNavActive::Teams
            } else if self.tasks_open {
                super::panel_nav::SidebarNavActive::Tasks
            } else if self.pull_requests_open {
                super::panel_nav::SidebarNavActive::PullRequests
            } else {
                super::panel_nav::SidebarNavActive::None
            };
            let mut projects: Vec<left_panel::ProjectData> = projects;
            left_panel::order_projects(&mut projects, &lp.project_order);
            lp.projects = projects;
            lp.hidden_projects = hidden_projects;
            lp.active_tab_id = workspace_visible.then_some(active_tab_id);
            lp.theme = theme;
            // Repaint while any synced row needs animation. Static states
            // (idle, review) clear on focus-driven syncs instead.
            if lp
                .tabs
                .iter()
                .any(|tab| tab.agents.iter().any(|agent| agent.cli))
                && lp.spinner_tick_gen == 0
            {
                lp.spinner_tick_gen += 1;
                let generation = lp.spinner_tick_gen;
                lp.tick_agent_spinner(generation, lp_ctx);
            }
            lp_ctx.notify();
        });
        ctx.update_view(&self.project_sidebar_view, |ps, ps_ctx| {
            if ps.theme.accent != content_theme.accent
                || ps.theme.background != content_theme.background
                || ps.theme.id != content_theme.id {
                ps.set_theme(content_theme, ps_ctx);
            }
            ps_ctx.notify();
        });
        #[cfg(not(target_family = "wasm"))]
        self.push_run_activity(ctx);
        // The Tasks page's attachable projects follow the open projects,
        // so a project opened while the page stays visible shows up in the
        // task detail panel without reopening the page. Unchanged lists
        // skip the repaint inside the setter.
        if self.tasks_open {
            let projects = super::tasks_host::project_refs_for_roots(&sidebar_project_roots);
            let page = self.tasks_view.clone();
            ctx.update_view(&page, |page, page_ctx| {
                page.set_available_projects(projects, page_ctx)
            });
        }
        self.resolve_project_roots_later(ctx);
        self.resolve_project_icons_later(ctx);
    }

    /// Find pictures for projects the sidebar showed without one yet, on a
    /// background thread; re-sync when one turns up.
    pub(crate) fn resolve_project_icons_later(&self, ctx: &mut ViewContext<Self>) {
        if self.project_icons_resolving.get() {
            return;
        }
        let roots = super::project_icons::take_pending();
        if roots.is_empty() {
            return;
        }
        self.project_icons_resolving.set(true);
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    roots
                        .into_iter()
                        .map(|root| {
                            let icon = super::project_icons::resolve(&root);
                            (root, icon)
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default()
            },
            |view, found, ctx| {
                view.project_icons_resolving.set(false);
                if super::project_icons::remember(found) {
                    view.schedule_left_panel_sync(ctx);
                } else {
                    view.resolve_project_icons_later(ctx);
                }
            },
        );
    }

    /// The sidebar project heading of a workspace folder, never touching the
    /// disk on the main thread: a folder not worked out yet stands for
    /// itself until the background pass names it.
    pub(crate) fn project_root_of(&self, root: &Path) -> PathBuf {
        super::project_naming::known_sidebar_project_root(root)
    }

    /// Work out, on a background thread, the headings the last sync asked
    /// about (and, once per run, every folder's again); re-sync when one
    /// changes.
    pub(crate) fn resolve_project_roots_later(&self, ctx: &mut ViewContext<Self>) {
        if self.project_roots_resolving.get() {
            return;
        }
        let mut roots = super::project_naming::take_pending_project_roots();
        if !self.project_roots_checked.replace(true) {
            roots.extend(self.open_project_roots.iter().cloned());
            roots.extend(self.workspaces.iter().map(|workspace| workspace.root.clone()));
        }
        roots.sort();
        roots.dedup();
        if roots.is_empty() {
            return;
        }
        self.project_roots_resolving.set(true);
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    roots
                        .into_iter()
                        .map(|root| {
                            let project = super::project_naming::resolve_sidebar_project_root(&root);
                            (root, project)
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default()
            },
            |view, found, ctx| {
                view.project_roots_resolving.set(false);
                if super::project_naming::remember_project_roots(found) {
                    view.schedule_left_panel_sync(ctx);
                } else {
                    // Folders asked about while this pass ran.
                    view.resolve_project_roots_later(ctx);
                }
            },
        );
    }

    /// Refresh one worktree's cached git status off-thread, then re-sync the
    /// sidebar so the trailing slot paints it. Fire-and-forget: missing cache
    /// entries render clean until the query lands.
    pub(crate) fn refresh_worktree_git_status(
        &self,
        root: std::path::PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        // Each query holds a background thread until git answers; repeated
        // refreshes of one slow repository used to occupy all of them.
        if !self.worktree_git_inflight.borrow_mut().insert(root.clone()) {
            return;
        }
        let key = root.clone();
        ctx.spawn(
            async move { super::worktree_status::query_worktree_git_status(&root) },
            move |view, status, ctx| {
                view.worktree_git_inflight.borrow_mut().remove(&key);
                if let Some(status) = status {
                    let previous = view
                        .worktree_git_cache
                        .get(&key)
                        .filter(|known| known.branch != status.branch)
                        .map(|known| known.branch.clone());
                    let current = status.branch.clone();
                    view.worktree_git_cache.insert(key.clone(), status);
                    if let Some(previous) = previous {
                        view.on_branch_changed(&key, previous.as_deref(), current.as_deref(), ctx);
                    }
                    view.sync_left_panel(ctx);
                }
            },
        );
    }

    /// A terminal's git model saw `branch` at `repo_root`. When the sidebar
    /// knows a different one, the branch was switched or renamed (anywhere,
    /// even outside Dragent): refreshing the worktree's status spreads it.
    pub(crate) fn notice_branch(&self, repo_root: &std::path::Path, branch: &str, ctx: &mut ViewContext<Self>) {
        let known = self
            .worktree_git_cache
            .iter()
            .map(|(root, status)| (root.as_path(), status.branch.as_deref()));
        if let Some(root) = stale_checkout(known, repo_root, branch) {
            self.refresh_worktree_git_status(root, ctx);
        }
    }

    /// A checkout's branch changed (renamed in Dragent, or switched or
    /// renamed in a terminal): every place that shows it updates now. Tab
    /// titles, the window title, the sidebar's project cards and rows, the
    /// project screen's branch picker, the status bar and Source Control.
    pub(crate) fn on_branch_changed(
        &mut self,
        root: &std::path::Path,
        previous: Option<&str>,
        current: Option<&str>,
        ctx: &mut ViewContext<Self>,
    ) {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let under = |path: &std::path::Path| {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            path.starts_with(&canonical)
        };
        // A workspace named after its branch (no name of its own) follows it.
        if let (Some(previous), Some(current)) = (previous, current) {
            let mut renamed = false;
            for workspace in &mut self.workspaces {
                let same = workspace.root.canonicalize().unwrap_or_else(|_| workspace.root.clone())
                    == canonical;
                if same && workspace.title == previous {
                    workspace.title = current.to_string();
                    renamed = true;
                }
            }
            if renamed {
                self.save_session(ctx);
            }
        }
        // Remote checkouts keep their `.git` answers briefly.
        #[cfg(not(target_family = "wasm"))]
        crate::remote_fs::forget_cached();
        // A pull request belongs to its branch.
        self.review_pr_cache.remove(&canonical);
        self.review_pr_cache.remove(root);
        self.spawn_worktree_pr_refresh(root.to_path_buf(), ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_window_title(ctx);
        if self.selected_project_root().is_some_and(|selected| under(&selected)) {
            self.refresh_prompt_branches(ctx);
            self.refresh_prompt_worktrees(ctx);
        }
        let cards = self.project_sidebar_model.clone();
        ctx.update_model(&cards, |model, model_ctx| model.refresh_branch(&canonical, model_ctx));
        // The status bar and Source Control read each terminal's git model.
        let terminals: Vec<_> = self
            .tabs
            .iter()
            .chain(self.parked_workspace_tabs.iter().flat_map(|surface| surface.panes.iter()))
            .filter_map(|tab| match &tab.pane {
                WorkspacePane::Terminal(terminal) => {
                    let cwd = tab.project_root.clone().unwrap_or_else(|| tab.pane.cwd(ctx));
                    under(&cwd).then(|| (terminal.clone(), cwd))
                }
                _ => None,
            })
            .collect();
        for (terminal, cwd) in terminals {
            let model = terminal.read(ctx, |terminal, _| terminal.git_repo_status());
            ctx.update_model(&model, |model, model_ctx| model.refresh_bypass_throttle(cwd, model_ctx));
        }
        if let (Some(model), Some(folder)) = (self.folder_git_model.clone(), self.folder_git_root.clone()) {
            if under(&folder) {
                ctx.update_model(&model, |model, model_ctx| model.refresh_bypass_throttle(folder, model_ctx));
            }
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    /// Refresh one worktree's cached PR state off-thread (no comments — the
    /// badge only needs identity, state, and checks). Keyed by the same
    /// canonical root the sidebar rows read.
    pub(crate) fn spawn_worktree_pr_refresh(
        &mut self,
        root: std::path::PathBuf,
        ctx: &mut ViewContext<Self>,
    ) {
        // Same canonical key the sidebar rows read, whatever form callers pass.
        let key = root.canonicalize().unwrap_or_else(|_| root.clone());
        ctx.spawn(
            async move { super::review_pr_service::query_review_pr_state(Some(root), false) },
            move |view, queried, ctx| {
                if let Some(pr) = queried.pr {
                    #[cfg(not(target_family = "wasm"))]
                    view.sync_task_pull_request(&key, &pr, ctx);
                    let branch = pr.head_branch.clone();
                    view.review_pr_cache.insert(
                        key,
                        crate::workspace::review_pr_service::CachedReviewPr {
                            branch,
                            pr: Some(pr),
                            comments: Vec::new(),
                            error: queried.error,
                        },
                    );
                    view.sync_left_panel(ctx);
                } else {
                    view.review_pr_cache.remove(&key);
                    view.sync_left_panel(ctx);
                }
            },
        );
    }

    /// Record a fresh completion for the green review dot until the user
    /// acknowledges it by visiting the pane. Stale records for closed panes are
    /// pruned here so the map never grows without bound.
    /// The person looked at this pane (here or on their phone): its last
    /// finish or failure is seen. With no turn recorded yet, a failed last
    /// command would show Failed for good; a seen, finished turn is noted
    /// instead. True when anything changed.
    pub(crate) fn mark_pane_seen(&mut self, pane: Uuid) -> bool {
        let now = super::worktree_status::now_ms();
        match self.agent_stop_state.get_mut(&pane) {
            Some(record) if record.seen_at_ms < record.stopped_at_ms => {
                record.seen_at_ms = now;
                true
            }
            Some(_) => false,
            None => {
                self.agent_stop_state.insert(
                    pane,
                    super::worktree_status::AgentStopState { stopped_at_ms: now, seen_at_ms: now, failed: false },
                );
                true
            }
        }
    }

    pub(crate) fn record_agent_stop(
        &mut self,
        pane_uuid: Uuid,
        failed: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let now = super::worktree_status::now_ms();
        self.agent_stop_state.insert(
            pane_uuid,
            super::worktree_status::AgentStopState {
                stopped_at_ms: now,
                seen_at_ms: 0,
                failed,
            },
        );
        let live: std::collections::HashSet<Uuid> = self
            .tabs
            .iter()
            .map(|tab| tab.pane.pane_uuid(ctx))
            .chain(
                self.parked_workspace_tabs
                    .iter()
                    .flat_map(|surface| surface.panes.iter())
                    .map(|pane| pane.pane.pane_uuid(ctx)),
            )
            .collect();
        self.agent_stop_state.retain(|uuid, _| live.contains(uuid));
    }

    /// Stable project root owning a tab id, across active and parked surfaces.
    pub(crate) fn project_root_for_tab(&self, tab_id: usize) -> Option<std::path::PathBuf> {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.project_root.clone())
            .or_else(|| {
                self.parked_workspace_tabs
                    .iter()
                    .flat_map(|surface| surface.panes.iter())
                    .find(|pane| pane.id == tab_id)
                    .and_then(|pane| pane.project_root.clone())
            })
    }
}

/// Re-detect project cards from the open tabs' CWDs.
/// git roots actually changed (see `ProjectSidebarModel::refresh_all_cwds`).
pub(crate) fn refresh_sidebar_projects(
    view: &mut WorkspaceView,
    ctx: &mut ViewContext<WorkspaceView>,
) {
    // Folder git status is independent of explorer ownership: even while the
    // editor owns the center, a folder-only workspace keeps its status fresh.
    view.sync_folder_git_model(ctx);
    // While the document editor owns the center workspace, its docked terminal
    // is an auxiliary shell. Its `cd` events must not move the explorer or
    // invalidate the file currently being edited; the normal terminal layout
    // resumes root tracking when the document is closed.
    let follows_terminal_cwd = !view.document_center_open
        && !view.project_start_visible
        && view
            .tabs
            .iter()
            .find(|tab| tab.id == view.active_tab_id)
            .is_some_and(|tab| !tab.pane.is_settings() && tab.project_root.is_none());
    let active_cwd = view.sidebar_context_root(ctx);
    if follows_terminal_cwd {
        // Git actions must use the same live directory as the Files panel.
        view.git_panel_state.borrow_mut().cwd = active_cwd
            .as_ref()
            .map(|cwd| super::project_sidebar::find_git_root(cwd).unwrap_or_else(|| cwd.clone()));
    }
    // CWD changes must also invalidate folder-scoped search and Git history,
    // not just refresh the tree when switching tabs.
    let sidebar_view = view.project_sidebar_view.clone();
    let context_root = active_cwd.clone();
    ctx.update_view(&sidebar_view, |sidebar, sidebar_ctx| {
        sidebar.refresh_workspace_context(context_root, sidebar_ctx);
    });
    let cwds = if let Some(ref cwd) = active_cwd {
        vec![cwd.clone()]
    } else {
        vec![]
    };
    let psm = view.project_sidebar_model.clone();
    ctx.update_model(&psm, |m, mc| {
        m.refresh_all_cwds(&cwds, active_cwd.clone(), mc)
    });
    if view.document_center_open {
        return;
    }
    if let Some(root) = active_cwd.filter(|cwd| cwd.is_dir()) {
        // File listing is always safe: the tree populates immediately on
        // open. Trust only gates code execution (settings, language
        // servers) elsewhere — never the listing.
        view.arm_document_tree_watcher(&root);
        let root_changed = root != view.document_index_root;
        if root_changed || view.document_index.entries().is_empty() {
            if root_changed {
                view.document_index_root = root.clone();
                view.document_index = ProjectDocumentIndex::default();
                view.document_center_open = false;
            }
            let expected_root = root.clone();
            let sidebar_model = view.project_sidebar_model.clone();
            // A hidden document still belongs to its original tab. Refreshing
            // another tab's explorer must not destroy that document's preview,
            // buffer, undo history, or draft. With no owner, this is a normal
            // root change and the obsolete document state can be released.
            let clear_document = view.document_tab_id.is_none();
            if root_changed {
                view.explorer_prompt = None;
                view.explorer_operation_error = None;
                ctx.update_model(&sidebar_model, |model, model_ctx| {
                    model.clear_documents(model_ctx);
                    if clear_document {
                        model.clear_document(model_ctx);
                    }
                });
            }
            // Listing is safe before trust: scan now so the explorer switches
            // to the newly opened folder immediately. Trust only gates
            // servers/settings, handled at their own sites.
            log::debug!(
                "[editor-tree] scanning root={} trusted={}",
                root.display(),
                view.workspace_trust.is_trusted(&root)
            );
            // Populate the top level synchronously so the Files tab can never
            // sit empty while the full recursive scan runs in the background.
            // Single-level `read_dir` is cheap; the async pass backfills depth.
            let top_level =
                super::project_documents::discover_directory(&root, std::path::Path::new(""));
            log::debug!(
                "[editor-tree] top-level root={} entries={}",
                root.display(),
                top_level.entries().len()
            );
            view.document_index = top_level.clone();
            ctx.update_model(&sidebar_model, |model, model_ctx| {
                model.set_documents(top_level, model_ctx);
            });
            // Standalone shells may start in home or another large directory.
            // Their explorer loads children on expansion instead of walking
            // the entire directory tree every time the shell changes CWD.
            if follows_terminal_cwd {
                ctx.notify();
                return;
            }
            ctx.spawn(
                async move { (root.clone(), super::project_documents::discover(&root)) },
                move |view, (root, index), ctx| {
                    if view.document_index_root != expected_root || root != expected_root {
                        log::debug!(
                            "[editor-tree] ignored stale refresh scanned_root={} current_root={}",
                            root.display(),
                            view.document_index_root.display()
                        );
                        return;
                    }
                    log::debug!(
                        "[editor-tree] applied refreshed root={} entries={}",
                        root.display(),
                        index.entries().len()
                    );
                    view.document_index = index.clone();
                    ctx.update_model(&sidebar_model, |model, model_ctx| {
                        model.set_documents(index, model_ctx);
                    });
                    ctx.notify();
                },
            );
        }
    }
}
/// Show/hide the explorer based on whether the active pane has a valid folder.
///
/// A deliberate close WINS: once the user has closed the panel it stays closed
/// until they open it again, no matter how many times this runs.
pub(crate) fn apply_sidebar_visibility(view: &mut WorkspaceView, ctx: &ViewContext<WorkspaceView>) {
    // Right panel stays visible until the user collapses it themselves.
    // No auto-hide on empty session / no file / untrusted root.
    if view.right_panel_user_closed {
        view.right_panel_visible = false;
        return;
    }
    let _ = ctx;
    view.right_panel_visible = true;
}
pub(crate) fn subscribe_workspace_pane(
    tab_id: usize,
    pane: &WorkspacePane,
    writer: std::sync::mpsc::SyncSender<dragent_persistence::ModelEvent>,
    ctx: &mut ViewContext<WorkspaceView>,
) {
    match pane {
        WorkspacePane::Terminal(terminal) => {
            subscribe_terminal_pane(Some(tab_id), terminal, writer, true, ctx)
        }
        WorkspacePane::Browser(browser) => {
            super::browser_tabs::subscribe_browser_pane(tab_id, browser, ctx)
        }
        WorkspacePane::Settings(_) => {}
    }
}
pub(crate) fn subscribe_terminal_pane(
    tab_id: Option<usize>,
    pane: &ViewHandle<TerminalView>,
    writer: std::sync::mpsc::SyncSender<dragent_persistence::ModelEvent>,
    tracks_workspace_context: bool,
    ctx: &mut ViewContext<WorkspaceView>,
) {
    let pane_id = pane.id();
    let pane_uuid = pane.read(ctx, |p, _| p.pane_uuid());
    let pane_handle = pane.clone();
    let model = pane.read(ctx, |tv, _| tv.terminal_model());
    let git_repo_status = pane.read(ctx, |tv, _| tv.git_repo_status());

    // Wire SQLite persistence into the agent controller so every push_block
    // call durably saves the block keyed by this pane's UUID.
    {
        let writer_for_agent = writer.clone();
        ctx.update_view(pane, |tv, tv_ctx| {
            tv.set_agent_persistence(pane_uuid, writer_for_agent, tv_ctx);
        });
    }

    let pane_clone = pane.clone();
    ctx.subscribe_to_view(pane, move |view, _, _event, ctx| {
        let has_focus = pane_clone.read(ctx, |tv, app| tv.pane_has_focus(app));
        if let Some(tab_id) = tab_id {
            if has_focus && view.active_tab_id != tab_id && view.terminal_focus_can_activate(tab_id)
            {
                view.select_tab_with_focus(tab_id, false, ctx);
            }
        }
    });

    // The git repo status model debounces filesystem changes off the hot path,
    // so reacting to it is a safe place to refresh git status / branch.
    ctx.subscribe_to_model(&git_repo_status, move |view, _, event, ctx| {
        // Any terminal, active or not, may be where the branch changed.
        if let crate::git_status::GitRepoStatusEvent::Updated {
            repo_root,
            status: Some(status),
        } = event
        {
            view.notice_branch(repo_root, &status.branch, ctx);
        }
        if tracks_workspace_context && view.active_pane_id == Some(pane_id) {
            let active_file_diff =
                matches!(event, crate::git_status::GitRepoStatusEvent::Updated { .. })
                    .then(|| {
                        view.document_requested_path
                            .as_deref()
                            .and_then(WorkspaceView::parse_git_file_diff_tab_path)
                    })
                    .flatten();
            let operation_succeeded = matches!(
                event,
                crate::git_status::GitRepoStatusEvent::OperationChanged {
                    completed: Some(Ok(())),
                    ..
                }
            );
            // The shared Git model refresh updates the source-control panel,
            // but the editor gutter uses the sidebar model's line-level cache.
            // Refresh that cache for every Git status update (including the
            // post-commit update) so an open editor clears its markers without
            // requiring a close/reopen.
            let sidebar_model = view.project_sidebar_model.clone();
            ctx.update_model(&sidebar_model, |model, model_ctx| {
                model.refresh_git_status(model_ctx);
            });
            refresh_sidebar_projects(view, ctx);
            apply_sidebar_visibility(view, ctx);
            let psv = view.project_sidebar_view.clone();
            ctx.update_view(&psv, |sidebar, c| {
                if operation_succeeded {
                    sidebar.git_operation_completed(true, c);
                } else {
                    c.notify();
                }
            });
            if let Some((rel_path, staged)) = active_file_diff {
                // HEAD/index/worktree may have changed externally (save,
                // `git add`, reset, checkout). Recompute the active comparison
                // from its typed identity instead of leaving stale hunks open.
                view.handle_action(&WorkspaceAction::OpenGitDiff { rel_path, staged }, ctx);
            }
            ctx.notify();
        }
    });

    ctx.subscribe_to_model(&model, move |view, m, event, ctx| {
        view.record_terminal_event(event);
        // An agent reattached after a restart may have been told another theme.
        if matches!(event, TerminalEvent::HostStateRestored) {
            view.schedule_agent_theme_restarts(ctx);
        }
        if let TerminalEvent::BlockCompleted { context } = event {
            let block_id = context.block_id;
            let snapshot = m.read(ctx, |model, _| {
                model.with_core(|core| {
                    let block = core.blocks().get(block_id)?;
                    Some((
                        persistence::block_to_persisted(block),
                        core.blocks().index_of(block_id).unwrap_or(0) as i32,
                    ))
                })
            });
            if let Some((block, sort_order)) = snapshot {
                // Record into the GLOBAL, never-cleared command history (the ↑/↓
                // popover) — separate from `blocks`, which is per-pane and wiped
                // on close. Fires for every completed command in any pane.
                let _ = writer.send(dragent_persistence::ModelEvent::RecordCommand {
                    command: block.command.clone(),
                    at_unix_ms: block.created_at_unix_ms,
                });
                // Route pane blocks by tab root, falling back to the block's
                // cwd repo root (None = legacy global for projectless panes).
                let project_root = tab_id
                    .and_then(|id| {
                        view.tabs
                            .iter()
                            .find(|tab| tab.id == id)
                            .and_then(|tab| tab.project_root.clone())
                    })
                    .or_else(|| dragent_persistence::project_root_for(&block.cwd));
                let _ = writer.send(dragent_persistence::ModelEvent::SaveBlock {
                    pane_uuid,
                    block,
                    sort_order,
                    project_root,
                });
            }
        }

        let is_active = tracks_workspace_context && view.active_pane_id == Some(pane_id);

        if !matches!(event, TerminalEvent::PtyWrite(_)) {
            let writer_clone = writer.clone();
            pane_handle.update(ctx, |tv, c| {
                tv.persist_clear_gap_if_changed(&writer_clone, c);
            });
        }

        // A command run on a remote machine may have switched branches or
        // worktrees: drop the remembered remote `.git` answers first.
        if matches!(event, TerminalEvent::BlockCompleted { .. }) {
            crate::remote_fs::forget_cached();
        }

        // Sidebar updates are wired ONLY to low-frequency events — never to
        // `OutputAppended`, which fires once per output chunk and previously
        // caused per-chunk subprocess detection (the UI freeze).
        let mut sidebar_touched = false;
        if is_active {
            match event {
                // CWD / git changed, or a new command block started.
                TerminalEvent::StatusChanged | TerminalEvent::BlocksChanged => {
                    refresh_sidebar_projects(view, ctx);
                    apply_sidebar_visibility(view, ctx);
                    sidebar_touched = true;
                }
                // A command finished: refresh once and harvest its errors
                // from the snapshotted output (not the live grid).
                TerminalEvent::BlockCompleted { context } => {
                    refresh_sidebar_projects(view, ctx);
                    let snap = m.read(ctx, |model, _| {
                        completed_block_snapshot(model, context.block_id)
                    });
                    if let Some(snap) = snap {
                        let psm = view.project_sidebar_model.clone();
                        ctx.update_model(&psm, |m, mc| m.harvest_errors(&[snap], mc));
                    }
                    apply_sidebar_visibility(view, ctx);
                    sidebar_touched = true;
                }
                // Alternate-screen transitions still repaint workspace chrome,
                // but they never own project-sidebar visibility.
                TerminalEvent::AltEntered | TerminalEvent::AltExited => {
                    ctx.notify();
                }
                _ => {}
            }
        }

        if sidebar_touched {
            let psv = view.project_sidebar_view.clone();
            ctx.update_view(&psv, |_, c| c.notify());
        }

        if matches!(
            event,
            TerminalEvent::StatusChanged | TerminalEvent::TitleChanged
        ) {
            let title = pane_handle.read(ctx, |tv, c| tv.display_title(c));
            let title = if title.is_empty() {
                DEFAULT_SESSION_TITLE.to_string()
            } else {
                title
            };
            if tracks_workspace_context {
                view.apply_pane_title(pane_id, title);
            } else {
                // Dock-only terminals do not participate in the workspace tab
                // model, but their header still needs to reflect `cd`/title
                // changes immediately.
                ctx.notify();
            }
            ctx.update_view(&pane_handle, |_, pane_ctx| pane_ctx.notify());
            // Keep the window/Dock title in sync with the active pane's cwd/command.
            if view.active_pane_id == Some(pane_id) {
                view.sync_window_title(ctx);
            }
        }

        // A completed command may have changed its worktree (files, branch,
        // pushed PR checks) — refresh that worktree's sidebar git + PR state
        // even when the pane is in the background. Agent panes additionally
        // record the stop for the review/failed dots.
        if let TerminalEvent::BlockCompleted { context } = event {
            let failed = !context.success();
            let agent_stop = tab_id.and_then(|id| {
                let found = view.tabs.iter().find(|tab| tab.id == id).or_else(|| {
                    view.parked_workspace_tabs
                        .iter()
                        .flat_map(|surface| surface.panes.iter())
                        .find(|pane| pane.id == id)
                })?;
                let is_agent = found.pane.agent_id(ctx).is_some()
                    || found.pane.tab_session_kind(ctx) == crate::agent::TabSessionKind::Agent
                    || found.pane.active_ai_tool(ctx).is_some();
                is_agent.then(|| found.pane.pane_uuid(ctx))
            });
            if let Some(uuid) = agent_stop {
                view.record_agent_stop(uuid, failed, ctx);
            }
            if let Some(root) = tab_id.and_then(|id| view.project_root_for_tab(id)) {
                view.refresh_worktree_git_status(root.clone(), ctx);
                view.spawn_worktree_pr_refresh(root, ctx);
            }
        }

        // `OutputAppended` fires once per output chunk. Rebuilding the left
        // panel, repainting the whole workspace, and serializing every tab on
        // each chunk is O(tabs) work per chunk — with several tabs streaming
        // output that becomes O(tabs²) per second and saturates the UI thread.
        // The visible terminal repaints itself via its own subscription, the
        // tab "running" indicator only changes on command start/finish, and the
        // session layout doesn't change per chunk — so skip all of this for
        // pure output events.
        if tracks_workspace_context && !matches!(event, TerminalEvent::OutputAppended) {
            view.schedule_left_panel_sync(ctx);
            ctx.notify();
            view.schedule_session_save(ctx);
        }
    });

    // Built-in agent turns don't emit terminal model events on submit, so the
    // pane subscription above never sees them start. Mirror turn boundaries
    // into sidebar snapshots directly: BlocksChanged covers submit/streaming,
    // TurnCompleted/Error record the stop (green/red dots) and re-sync.
    let controller = pane.read(ctx, |tv, _| tv.agent_controller.clone());
    let stop_uuid = pane_uuid;
    ctx.subscribe_to_model(&controller, move |view, _, event, ctx| {
        use crate::agent::AgentControllerEvent;
        use dragent_automations::AgentState;
        match event {
            AgentControllerEvent::TurnCompleted { assistant_text, .. } => {
                view.record_agent_stop(stop_uuid, false, ctx);
                view.sync_left_panel(ctx);
                let reply = Some(assistant_text.trim().to_string()).filter(|text| !text.is_empty());
                view.note_run_agent(stop_uuid, AgentState::Done, reply, ctx);
            }
            AgentControllerEvent::Error { .. } => {
                view.record_agent_stop(stop_uuid, true, ctx);
                view.sync_left_panel(ctx);
                view.note_run_agent(stop_uuid, AgentState::Failed, None, ctx);
            }
            AgentControllerEvent::BlocksChanged => {
                // Fires per streamed chunk; one sync per frame is enough.
                view.schedule_left_panel_sync(ctx);
            }
            AgentControllerEvent::ConsentRequested { .. }
            | AgentControllerEvent::QuestionOpened { .. } => {
                view.note_run_agent(stop_uuid, AgentState::NeedsInput, None, ctx);
            }
            AgentControllerEvent::QuestionAnswered { .. } => {
                view.note_run_agent(stop_uuid, AgentState::Working, None, ctx);
            }
            _ => {}
        }
    });
}

/// The known checkout holding `folder` whose recorded branch isn't
/// `branch` any more. The deepest one counts: a worktree inside the project
/// folder must not be compared with the project's own branch. A detached
/// HEAD (no recorded branch) never counts.
fn stale_checkout<'a>(
    known: impl Iterator<Item = (&'a std::path::Path, Option<&'a str>)>,
    folder: &std::path::Path,
    branch: &str,
) -> Option<std::path::PathBuf> {
    let canonical = |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let folder = canonical(folder);
    let (root, recorded) = known
        .filter(|(root, _)| folder.starts_with(canonical(root)))
        .max_by_key(|(root, _)| canonical(root).components().count())?;
    recorded
        .is_some_and(|recorded| recorded != branch)
        .then(|| root.to_path_buf())
}

#[cfg(test)]
mod branch_tests {
    use super::stale_checkout;
    use std::path::Path;

    #[test]
    fn a_switched_branch_is_noticed_in_the_deepest_checkout() {
        let known = [
            (Path::new("/work/app"), Some("main")),
            (Path::new("/work/app/.worktrees/feature"), Some("feature")),
            (Path::new("/work/other"), None),
        ];
        let stale = |folder: &str, branch: &str| {
            stale_checkout(known.iter().copied(), Path::new(folder), branch)
        };
        // The project checked out another branch, from a subfolder.
        assert_eq!(stale("/work/app/src", "release").as_deref(), Some(Path::new("/work/app")));
        // Unchanged.
        assert_eq!(stale("/work/app", "main"), None);
        // The worktree inside it is compared with its own branch only.
        assert_eq!(stale("/work/app/.worktrees/feature", "feature"), None);
        assert_eq!(
            stale("/work/app/.worktrees/feature/lib", "feature-2").as_deref(),
            Some(Path::new("/work/app/.worktrees/feature"))
        );
        // A detached HEAD, and folders nobody knows, never count.
        assert_eq!(stale("/work/other", "HEAD"), None);
        assert_eq!(stale("/elsewhere", "main"), None);
    }
}
