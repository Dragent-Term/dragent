//!  session sidebar — search, tab list, context menus, colors.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{
    AppContext, Entity, EntityId, TypedActionView, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::elements::{
    AcceptedByDropTarget, DispatchEventResult, Draggable, DraggableState, DropTarget, EventHandler,
    Hoverable, MouseStateHandle,
};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_SM};
use crate::theme::Theme;

use super::sidebar::{tab_color_options, SidebarFocus, SidebarViewMode};
use super::{PaneDropZone, WorkspaceAction};

pub(crate) const FONT_MENU: f32 = 13.0;

/// SavePosition tag for the tab list — measured every frame so row text
/// budgets use the REAL rendered panel width instead of the stale resizable
/// state (which only updates on drag-resize, not window resizes).
pub(crate) const LEFT_PANEL_ROWS_POSITION_ID: &str = "left_panel_rows";
pub(crate) const MENU_ICON_SIZE: f32 = 14.0;
pub(crate) const MENU_ICON_SLOT: f32 = 16.0;
pub(crate) const FONT_MENU_SECTION: f32 = 10.0;
pub(crate) const FONT_FOOTER: f32 = 13.5;
pub(crate) const FONT_TOGGLE: f32 = 14.0;
pub(crate) const FONT_EMPTY: f32 = 13.0;

pub(crate) fn tab_drag(
    states: &RefCell<HashMap<usize, DraggableState>>,
    tab_id: usize,
) -> DraggableState {
    states
        .borrow_mut()
        .entry(tab_id)
        .or_insert_with(DraggableState::default)
        .clone()
}

pub(crate) fn project_drag(
    states: &RefCell<HashMap<String, DraggableState>>,
    project_key: &str,
) -> DraggableState {
    states
        .borrow_mut()
        .entry(project_key.to_string())
        .or_default()
        .clone()
}

/// A project section as the sidebar lists it: a drop target for workspace
/// rows (its ungrouped bucket) and for other project headings, with the
/// accent line where a dragged project would land.
fn project_section(
    section: Box<dyn Element>,
    project_key: Option<&str>,
    landing: Option<&(String, bool)>,
    theme: &Theme,
) -> Vec<Box<dyn Element>> {
    let Some(key) = project_key else {
        return vec![section];
    };
    let section = DropTarget::new(
        DropTarget::new(
            section,
            super::panel_nav::SidebarGroupDropTarget {
                project_key: key.to_string(),
                group: None,
            },
        )
        .finish(),
        super::panel_nav::SidebarProjectDropTarget {
            project_key: key.to_string(),
        },
    )
    .finish();
    match landing {
        Some((target, below)) if target == key => {
            let line = Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(2.0)
                    .finish(),
            )
            .with_background_color(theme.accent)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.0)))
            .with_margin_left(8.0)
            .with_margin_right(8.0)
            .finish();
            if *below {
                vec![section, line]
            } else {
                vec![line, section]
            }
        }
        _ => vec![section],
    }
}

pub(crate) fn tab_mouse(
    states: &RefCell<HashMap<usize, MouseStateHandle>>,
    tab_id: usize,
) -> MouseStateHandle {
    states
        .borrow_mut()
        .entry(tab_id)
        .or_insert_with(MouseStateHandle::default)
        .clone()
}

pub(crate) fn sidebar_mouse(
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: impl Into<String>,
) -> MouseStateHandle {
    let key = key.into();
    states
        .borrow_mut()
        .entry(key)
        .or_insert_with(MouseStateHandle::default)
        .clone()
}

/// Display data for a single sidebar tab — a snapshot derived from PaneTab
/// and its panes, owned by LeftPanelView so it can render without touching WorkspaceView.
#[derive(Clone)]
pub struct TabData {
    pub id: usize,
    pub workspace_id: Option<usize>,
    /// Computed display label (custom_label if set, otherwise title).
    pub label: String,
    pub branch_subtitle: Option<String>,
    pub group: Option<String>,
    pub color: Option<ColorU>,
    pub is_running: bool,
    /// Representative pane title (used for sidebar search).
    pub title: String,
    /// Representative pane cwd string (used for sidebar search).
    pub cwd: String,
    /// Stable project container used to group related work and tool tabs.
    pub project_key: Option<String>,
    pub project_label: Option<String>,
    /// Semantic label for non-work tabs, such as Develop or Terminal.
    pub purpose_label: String,
    /// A work item remains a work item even while its dedicated surface is hidden.
    pub is_work_item: bool,
    /// Active AI tool in the representative pane, if any.
    pub ai_tool: Option<crate::console::ai_toolbar::AiTool>,
    pub last_command: Option<String>,
    /// Lifecycle status of this worktree's agents (highest priority across
    /// its panes). Drives the trailing status dot; `Idle` renders nothing.
    pub agent_status: super::worktree_status::WorktreeAgentStatus,
    pub agents: Vec<super::worktree_status::SidebarAgent>,
    /// Diff stats scoped to this worktree's checkout (0/0/0 = clean = hidden).
    pub diff_files: usize,
    pub diff_added: usize,
    pub diff_deleted: usize,
    /// PR badge for this worktree's branch. None = no PR = hidden.
    pub pr: Option<super::worktree_status::SidebarPrBadge>,
    /// Canonical worktree root these statuses were resolved for.
    pub git_root: Option<PathBuf>,
    /// Branch of that worktree (tooltip context only, never a row line).
    pub branch: Option<String>,
    /// True when this tab's pane is currently being SHARED by this host — the tab
    /// menu then offers "Stop session" instead of "Share session".
    pub is_shared: bool,
}

impl TabData {
    /// The person is looking at this workspace: what its agents finished is
    /// seen, and stays seen after a restart.
    pub(crate) fn acknowledge_activity(&mut self) {
        use super::worktree_status::WorktreeAgentStatus as Status;
        let mut seen = Vec::new();
        for agent in &mut self.agents {
            // A finish or a failure: either is seen once looked at.
            if agent.cli && matches!(agent.status, Status::Review | Status::Failed) {
                agent.status = Status::Idle;
                seen.push(agent.pane_id);
            }
        }
        if !seen.is_empty() {
            let now = super::worktree_status::now_ms() as u64;
            crate::bg::detach(move || {
                for pane in seen {
                    dragent_host::agent_sessions::mark_activity_seen(pane, now);
                }
            });
        }
        self.agent_status = self.agents.iter().fold(Status::Idle, |status, agent| {
            Status::pick_higher(status, agent.status.clone())
        });
    }

    /// The person looked at one specific pane: only that pane's finished
    /// agent counts as seen. Other panes in the same workspace keep their
    /// Review dot until their own tab is opened.
    pub(crate) fn acknowledge_pane_activity(&mut self, pane_id: uuid::Uuid) {
        use super::worktree_status::WorktreeAgentStatus as Status;
        let mut seen = false;
        for agent in &mut self.agents {
            if agent.pane_id == pane_id && agent.cli && matches!(agent.status, Status::Review | Status::Failed) {
                agent.status = Status::Idle;
                seen = true;
            }
        }
        if seen {
            let now = super::worktree_status::now_ms() as u64;
            crate::bg::detach(move || {
                dragent_host::agent_sessions::mark_activity_seen(pane_id, now);
            });
            self.agent_status = self.agents.iter().fold(Status::Idle, |status, agent| {
                Status::pick_higher(status, agent.status.clone())
            });
        }
    }
}

/// A project container is independent from its terminal sessions. Keeping
/// this snapshot separate lets an empty project remain visible after its last
/// terminal closes.
#[derive(Clone)]
pub struct ProjectData {
    pub key: String,
    pub label: String,
    pub groups: Vec<String>,
    /// Its picture (own icon or owner's avatar); `None` shows its letter.
    pub icon: Option<std::path::PathBuf>,
}

/// Small view-local actions. Workspace-level mutations remain coordinated by
/// `WorkspaceView`; resize only asks this child to reread its live panel width.
#[derive(Debug, Clone)]
pub enum LeftPanelAction {
    Focus,
    Resize,
    ToggleProjectGroup {
        project_key: String,
    },
    ToggleWorkspaceGroup {
        project_key: String,
        group: String,
    },
    /// Fold or unfold a coordination's group.
    ToggleCoordinationGroup {
        team_id: String,
    },
    PreviewWorkspaceDrop(Option<SidebarDragPreview>),
    CommitWorkspaceDrop {
        source_tab_id: usize,
        project_key: String,
        destination: Option<SidebarDropDestination>,
    },
    PreviewProjectDrop(Option<ProjectDragPreview>),
    CommitProjectDrop {
        source: String,
        target: Option<String>,
    },
    ToggleProjects,
    ToggleHiddenProjects,
    ToggleProjectsSearch,
    ClearProjectsSearch,
    CycleSortMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarDragPreview {
    pub source_tab_id: usize,
    pub project_key: String,
    pub destination: SidebarDropDestination,
}

/// A project heading dragged over another project: it lands above that
/// project when moving up, below it when moving down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDragPreview {
    pub source: String,
    pub target: String,
}

/// `projects` in the order the person arranged (`order`, project keys).
/// Projects not in it keep their own order after the arranged ones.
pub(crate) fn order_projects(projects: &mut [ProjectData], order: &[String]) {
    if order.is_empty() {
        return;
    }
    projects.sort_by_key(|project| {
        order
            .iter()
            .position(|key| *key == project.key)
            .unwrap_or(usize::MAX)
    });
}

/// Move `item` onto `target`'s place in `items` (a drag and drop): it lands
/// after the target when moving down or right, before it when moving up or
/// left. False when either is missing or nothing moves.
pub(crate) fn move_onto<T: PartialEq>(items: &mut Vec<T>, item: &T, target: &T) -> bool {
    let (Some(from), Some(to)) = (
        items.iter().position(|candidate| candidate == item),
        items.iter().position(|candidate| candidate == target),
    ) else {
        return false;
    };
    if from == to {
        return false;
    }
    let moved = items.remove(from);
    items.insert(to, moved);
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarDropDestination {
    Group(Option<String>),
    Tab {
        tab_id: usize,
        group: Option<String>,
    },
}

fn preview_bucket_tabs<'a>(
    tabs: &[&'a TabData],
    named_groups: &[String],
    bucket: Option<&str>,
    collapsed: bool,
    project_key: Option<&str>,
    preview: Option<&SidebarDragPreview>,
) -> Vec<&'a TabData> {
    let mut bucket_tabs: Vec<_> = tabs
        .iter()
        .copied()
        .filter(|tab| {
            if collapsed {
                return false;
            }
            match bucket {
                Some(name) => tab.group.as_deref() == Some(name),
                None => tab
                    .group
                    .as_deref()
                    .is_none_or(|name| !named_groups.iter().any(|group| group == name)),
            }
        })
        .collect();
    let Some(preview) = preview.filter(|preview| project_key == Some(preview.project_key.as_str()))
    else {
        return bucket_tabs;
    };
    bucket_tabs.retain(|tab| tab.id != preview.source_tab_id);
    let destination_group = match &preview.destination {
        SidebarDropDestination::Group(group) => group.as_deref(),
        SidebarDropDestination::Tab { group, .. } => group.as_deref(),
    };
    if bucket == destination_group {
        if let Some(source) = tabs
            .iter()
            .copied()
            .find(|tab| tab.id == preview.source_tab_id)
        {
            let insert_at = match &preview.destination {
                SidebarDropDestination::Group(_) => bucket_tabs.len(),
                SidebarDropDestination::Tab { tab_id, .. } => {
                    let target = bucket_tabs
                        .iter()
                        .position(|tab| tab.id == *tab_id)
                        .unwrap_or(bucket_tabs.len());
                    let source_was_above = tabs.iter().position(|tab| tab.id == source.id)
                        < tabs.iter().position(|tab| tab.id == *tab_id);
                    target + usize::from(source_was_above && target < bucket_tabs.len())
                }
            };
            bucket_tabs.insert(insert_at, source);
        }
    }
    bucket_tabs
}

/// Tab ordering inside each project group. Tabs sort within their
/// project; the project order itself never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarSortMode {
    #[default]
    Manual,
    Name,
}

/// The sidebar as a first-class View peer of the terminal pane area.
///
/// Owns all sidebar UI state (focus, menus, search/rename editors, scroll) and
/// a snapshot of the tab display data synced from WorkspaceView.  Rendering is
/// delegated to the existing `render_left_panel` function.
pub struct LeftPanelView {
    /// EntityId of the parent WorkspaceView — used for action dispatch targets.
    pub workspace_view_id: EntityId,
    pub ui_font: FamilyId,
    pub theme: Theme,
    /// Tab display snapshots — synced by WorkspaceView after every tab mutation.
    pub tabs: Vec<TabData>,
    pub projects: Vec<ProjectData>,
    pub hidden_projects: Vec<ProjectData>,
    /// Selected workspace row; standalone pages have no workspace selection.
    pub active_tab_id: Option<usize>,
    // Sidebar UI state
    pub sidebar_focus: SidebarFocus,
    pub sidebar_menu_tab_id: Option<usize>,
    pub sidebar_view_mode: SidebarViewMode,
    pub sidebar_search_editor: ViewHandle<crate::workspace::InputEditor>,
    pub sidebar_rename_editor: ViewHandle<crate::workspace::InputEditor>,
    pub tab_mouse_states: RefCell<HashMap<usize, MouseStateHandle>>,
    /// Per-tab drag state for dragging a tab into a split (one per tab_id).
    pub tab_drag_states: RefCell<HashMap<usize, DraggableState>>,
    pub sidebar_mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    /// Independently collapsed project containers, keyed by canonical root.
    pub collapsed_project_groups: HashSet<String>,
    pub collapsed_workspace_groups: HashSet<(String, String)>,
    /// Agent Coordination groups, and those folded (by team id).
    pub coordination: Vec<super::panel_coordination::SidebarCoordination>,
    pub collapsed_coordination: HashSet<String>,
    pub sidebar_drag_preview: Option<SidebarDragPreview>,
    pub sidebar_drag_target_state: Arc<Mutex<Option<SidebarDragPreview>>>,
    /// Per-project drag state for reordering project headings.
    pub project_drag_states: RefCell<HashMap<String, DraggableState>>,
    pub project_drag_preview: Option<ProjectDragPreview>,
    /// Project keys in the order the person dragged them into; empty until
    /// they do.
    pub project_order: Vec<String>,
    /// Whole project list collapsed (the Projects section header stays).
    pub projects_collapsed: bool,
    pub hidden_projects_collapsed: bool,
    /// The Projects-header search icon expands into the filter input.
    pub projects_search_expanded: bool,
    /// Tab ordering within each project group.
    pub sidebar_sort: SidebarSortMode,
    /// Generation guard for the agent-spinner tick loop below.
    pub spinner_tick_gen: u64,
    activity_notifications: HashMap<uuid::Uuid, (uuid::Uuid, u64, String)>,
    pub left_panel_scroll_state: ClippedScrollStateHandle,
    /// The panel's Resizable state — read at render time so text budgets track
    /// the CURRENT width and rows degrade gracefully instead of overflowing.
    pub panel_state: warpui::elements::ResizableStateHandle,
    /// This view's host window — used to read last-frame element positions
    /// from the position cache for width measurement.
    pub window_id: warpui_core::WindowId,
    /// The organisation menu anchored to the footer is open.
    pub org_menu_open: bool,
    /// Automation failures the user hasn't seen yet.
    pub automation_failures: usize,
    /// Messages teams' leads left for the person, unread.
    pub teams_needing_you: usize,
    /// Which nav screen owns the center, if any — its row stays highlighted.
    pub nav_active: super::panel_nav::SidebarNavActive,
}

impl Entity for LeftPanelView {
    type Event = ();
}

impl LeftPanelView {
    /// Only the pane the person actually opened counts as seen. Clicking a
    /// workspace must not clear Review dots on its other tabs.
    pub(crate) fn acknowledge_pane_activity(&mut self, pane_id: uuid::Uuid) {
        for tab in &mut self.tabs {
            if tab.agents.iter().any(|agent| agent.pane_id == pane_id) {
                tab.acknowledge_pane_activity(pane_id);
            }
        }
    }

    pub fn new(
        ctx: &mut ViewContext<Self>,
        workspace_view_id: EntityId,
        ui_font: FamilyId,
        theme: Theme,
        panel_state: warpui::elements::ResizableStateHandle,
    ) -> Self {
        // Create editors as TypedActionView children so they are parented to
        // this view in the presenter's structural hierarchy.  This ensures
        // their deferred WorkspaceAction dispatches (CommitRenameTab,
        // CancelRenameTab, SidebarSearchInput) correctly bubble up:
        //   InputEditor → LeftPanelView → WorkspaceView
        let sidebar_search_editor = ctx.add_typed_action_view(|ctx| {
            crate::workspace::InputEditor::new(
                "Search tabs...",
                ui_font,
                crate::theme::text_size(11.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                crate::workspace::InputMode::Search,
                ctx.focus_handle(),
            )
        });
        let sidebar_rename_editor = ctx.add_typed_action_view(|ctx| {
            crate::workspace::InputEditor::new(
                "Rename Tab…",
                ui_font,
                crate::theme::text_size(13.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                crate::workspace::InputMode::RenameTab(0),
                ctx.focus_handle(),
            )
        });
        {
            use warpui::SingletonEntity;
            let auth = crate::auth::AuthManager::handle(ctx);
            ctx.observe(&auth, |_, _, ctx| ctx.notify());
            let account = crate::settings::AccountStore::handle(ctx);
            ctx.observe(&account, |_, _, ctx| ctx.notify());
            let orgs = crate::settings::OrgStore::handle(ctx);
            ctx.observe(&orgs, |_, _, ctx| ctx.notify());
            let context = crate::organisation_context::OrganisationContext::handle(ctx);
            ctx.observe(&context, |_, _, ctx| ctx.notify());
        }
        Self {
            workspace_view_id,
            ui_font,
            theme,
            tabs: Vec::new(),
            projects: Vec::new(),
            hidden_projects: Vec::new(),
            active_tab_id: None,
            sidebar_focus: SidebarFocus::None,
            sidebar_menu_tab_id: None,
            sidebar_view_mode: SidebarViewMode::List,
            sidebar_search_editor,
            sidebar_rename_editor,
            tab_mouse_states: RefCell::new(HashMap::new()),
            tab_drag_states: RefCell::new(HashMap::new()),
            sidebar_mouse_states: RefCell::new(HashMap::new()),
            collapsed_project_groups: HashSet::new(),
            collapsed_workspace_groups: HashSet::new(),
            coordination: Vec::new(),
            collapsed_coordination: HashSet::new(),
            sidebar_drag_preview: None,
            sidebar_drag_target_state: Arc::new(Mutex::new(None)),
            project_drag_states: RefCell::new(HashMap::new()),
            project_drag_preview: None,
            project_order: Vec::new(),
            projects_collapsed: false,
            hidden_projects_collapsed: true,
            projects_search_expanded: false,
            sidebar_sort: SidebarSortMode::Manual,
            spinner_tick_gen: 0,
            activity_notifications: HashMap::new(),
            left_panel_scroll_state: ClippedScrollStateHandle::default(),
            panel_state,
            window_id: ctx.window_id(),
            org_menu_open: false,
            automation_failures: 0,
            teams_needing_you: 0,
            nav_active: super::panel_nav::SidebarNavActive::None,
        }
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        ctx.update_view(
            &self.sidebar_search_editor,
            |editor: &mut crate::workspace::InputEditor, ectx| editor.set_theme(theme, ectx),
        );
        ctx.update_view(
            &self.sidebar_rename_editor,
            |editor: &mut crate::workspace::InputEditor, ectx| editor.set_theme(theme, ectx),
        );
        ctx.notify();
    }

    /// Poll durable provider events off-thread. No PTY output heuristic and no
    /// per-frame disk reads; only changed statuses invalidate the sidebar.
    pub(crate) fn tick_agent_spinner(&mut self, generation: u64, ctx: &mut ViewContext<Self>) {
        ctx.spawn(warpui::r#async::Timer::after(std::time::Duration::from_secs(1)),
            move |view, _, ctx| {
                if view.spinner_tick_gen != generation { return; }
                use super::worktree_status::WorktreeAgentStatus;
                let panes: Vec<_> = view.tabs.iter().flat_map(|tab| tab.agents.iter())
                    .filter(|agent| agent.cli)
                    .map(|agent| (
                        agent.pane_id,
                        view.activity_notifications.contains_key(&agent.pane_id),
                        matches!(agent.status, WorktreeAgentStatus::Review | WorktreeAgentStatus::Failed),
                    ))
                    .collect();
                if panes.is_empty() { view.spinner_tick_gen = 0; return; }
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    use dragent_host::agent_sessions::{activity_seen, activity_seen_or, load_activity};
                    let events: HashMap<_, _> = panes.into_iter().map(|(pane, known, in_review)| {
                        let event = load_activity(pane);
                        // What a finished turn said, for its notification.
                        let reply = event.as_ref()
                            .filter(|event| event.event == "Stop")
                            .and_then(|_| dragent_host::agent_sessions::load_reply(pane))
                            .map(|reply| reply.text);
                        // The first look at a session asks what was seen; so
                        // does a finish waiting for review, which the person
                        // may have looked at elsewhere (their phone).
                        let seen = match (&event, known) {
                            (Some(event), false) => Some(activity_seen_or(pane, event.at_ms)),
                            (Some(_), true) if in_review => activity_seen(pane),
                            _ => None,
                        };
                        (pane, (event, seen, known, reply))
                    }).collect();
                    let _ = tx.send(events);
                });
                ctx.spawn(rx, move |view, events, ctx| {
                    if view.spinner_tick_gen != generation { return; }
                    let mut changed = false;
                    let mut notifications = Vec::new();
                    if let Ok(events) = events {
                        use super::worktree_status::WorktreeAgentStatus as Status;
                        for tab in &mut view.tabs {
                            for agent in &mut tab.agents {
                                if !agent.cli { continue; }
                                let (event, seen, known_before, reply) = events.get(&agent.pane_id)
                                    .map_or((None, None, false, None), |(event, seen, known, reply)| (event.as_ref(), *seen, *known, reply.as_deref()));
                                let mut new_event = false;
                                if let Some(event) = event {
                                    let stamp = (event.generation, event.at_ms, event.event.clone());
                                    let previous = view.activity_notifications.insert(agent.pane_id, stamp.clone());
                                    new_event = super::worktree_status::is_new_activity(previous.as_ref(), &stamp, seen);
                                    // First snapshot after reconnect is state, not a fresh notification.
                                    if event.notification_after(previous.as_ref()).is_some() {
                                        let kind = match event.event.as_str() {
                                            "PermissionRequest" => crate::notifications::Kind::AgentNeedsYou,
                                            "Failed" => crate::notifications::Kind::AgentError,
                                            _ => crate::notifications::Kind::AgentFinished,
                                        };
                                        let agent_name = agent.tool.map(|tool| tool.label()).unwrap_or("Agent");
                                        let (title, body) = crate::notifications::agent_alert(agent_name, &tab.label, kind, reply);
                                        notifications.push((agent.pane_id, kind, title, body, tab.id));
                                    }
                                }
                                let mut status = agent.status.after_cli_event(
                                    event.map(|event| event.event.as_str()), new_event,
                                );
                                // No reports at all (the agent loop runs where
                                // its hooks aren't installed, e.g. a Claude Code
                                // background session viewed here): its
                                // terminal drawing is the only sign of work.
                                if event.is_none() {
                                    if let Some(busy) = crate::terminal::output_activity::busy(agent.pane_id) {
                                        status = if busy { Status::Working { label: String::new() } } else { Status::Idle };
                                    }
                                }
                                // Looked at since it finished (on the phone, say): seen.
                                if known_before
                                    && matches!(status, Status::Review | Status::Failed)
                                    && !new_event
                                    && super::worktree_status::seen_since(event.map(|event| event.at_ms), seen)
                                {
                                    status = Status::Idle;
                                }
                                let known = event.is_some();
                                changed |= agent.status != status || agent.activity_known != known;
                                agent.status = status;
                                agent.activity_known = known;
                            }
                            let aggregate = tab.agents.iter().fold(Status::Idle,
                                |status, agent| Status::pick_higher(status, agent.status.clone()));
                            changed |= tab.agent_status != aggregate;
                            tab.agent_status = aggregate;
                        }
                    }
                    // An automation run's agent alerts as that run instead.
                    #[cfg(not(target_family = "wasm"))]
                    if !notifications.is_empty() {
                        use warpui::SingletonEntity;
                        let runtime = crate::automations::AutomationRuntime::handle(ctx);
                        notifications.retain(|(pane, ..)| {
                            !runtime.read(ctx, |runtime, _| runtime.is_run_pane(&pane.to_string()))
                        });
                        // A team's workers report to its lead; only what needs
                        // the person (an approval) or went wrong reaches them.
                        notifications.retain(|(pane, kind, ..)| {
                            *kind != crate::notifications::Kind::AgentFinished
                                || !crate::teams::is_worker(&pane.to_string())
                        });
                    }
                    // A finish the person watched happen (Dragent in front,
                    // this window active, the agent's workspace on screen) is
                    // no news; what needs them or failed always is.
                    let watching = ctx.windows().app_is_active()
                        && ctx.windows().active_window() == Some(ctx.window_id());
                    notifications.retain(|(_, kind, _, _, row)| {
                        *kind != crate::notifications::Kind::AgentFinished
                            || !(watching && view.active_tab_id == Some(*row))
                    });
                    for (pane, kind, title, body, _) in notifications {
                        // A permission request can be answered from the phone's notification.
                        let answerable = (kind == crate::notifications::Kind::AgentNeedsYou).then_some(pane);
                        crate::notifications::notify_for_pane(ctx, kind, &title, &body, None, answerable);
                    }
                    // The workspace redraws too: its catalog for the phone
                    // carries these states.
                    if changed { ctx.notify(); ctx.emit(()); }
                    view.tick_agent_spinner(generation, ctx);
                });
            });
    }
}

impl View for LeftPanelView {
    fn ui_name() -> &'static str {
        "LeftPanelView"
    }

    /// Called when this view OR one of its children gains focus.
    ///
    /// We use `DescendentFocused` to guard the `SelfFocused` arm so that
    /// programmatic focus (e.g. `FocusSidebarSearch` calling `lp_ctx.focus(…)`)
    /// always lands on a specific child and never on the panel container itself.
    /// When that happens `SelfFocused` would mean nothing sensible — redirect to
    /// the search editor as a safe default.
    fn on_focus(&mut self, focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            // Should not normally happen — LeftPanelView is a container, not a
            // leaf.  As a safe fallback redirect focus to the appropriate editor.
            match self.sidebar_focus {
                SidebarFocus::Rename(_) => {
                    ctx.focus(&self.sidebar_rename_editor);
                }
                _ => {
                    ctx.focus(&self.sidebar_search_editor);
                }
            }
        }
        // DescendentFocused: an editor child already has focus — nothing to do.
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let search_text = self
            .sidebar_search_editor
            .read(app, |e: &crate::workspace::InputEditor, _| {
                e.get_text().to_string()
            });

        render_left_panel(
            LeftPanelProps {
                ui_font: self.ui_font,
                theme: &self.theme,
                workspace_view_id: self.workspace_view_id,
                tabs: &self.tabs,
                projects: &self.projects,
                hidden_projects: &self.hidden_projects,
                active_tab_id: self.active_tab_id,
                search: &search_text,
                view_mode: self.sidebar_view_mode,
                menu_tab_id: self.sidebar_menu_tab_id,
                sidebar_focus: self.sidebar_focus,
                search_editor: &self.sidebar_search_editor,
                rename_editor: &self.sidebar_rename_editor,
                tab_mouse_states: &self.tab_mouse_states,
                tab_drag_states: &self.tab_drag_states,
                sidebar_mouse_states: &self.sidebar_mouse_states,
                automation_failures: self.automation_failures,
                teams_needing_you: self.teams_needing_you,
                nav_active: self.nav_active,
                collapsed_project_groups: &self.collapsed_project_groups,
                collapsed_workspace_groups: &self.collapsed_workspace_groups,
                coordination: &self.coordination,
                collapsed_coordination: &self.collapsed_coordination,
                drag_preview: self.sidebar_drag_preview.as_ref(),
                drag_target_state: &self.sidebar_drag_target_state,
                project_drag_states: &self.project_drag_states,
                project_drag_preview: self.project_drag_preview.as_ref(),
                projects_collapsed: self.projects_collapsed,
                hidden_projects_collapsed: self.hidden_projects_collapsed,
                projects_search_expanded: self.projects_search_expanded,
                sort_mode: self.sidebar_sort,
                scroll_state: self.left_panel_scroll_state.clone(),
                panel_width: self
                    .panel_state
                    .lock()
                    .ok()
                    .map(|s| s.size())
                    .unwrap_or(260.0),
                window_id: self.window_id,
                footer: super::org_menu::render_org_footer(
                    &super::org_menu::OrgSummary::read(app),
                    self.org_menu_open,
                    &self.theme,
                    self.ui_font,
                    self.workspace_view_id,
                    &self.sidebar_mouse_states,
                ),
            },
            app,
        )
    }
}

impl TypedActionView for LeftPanelView {
    type Action = LeftPanelAction;

    fn handle_action(&mut self, action: &LeftPanelAction, ctx: &mut ViewContext<Self>) {
        match action {
            LeftPanelAction::Focus => {
                ctx.focus_self();
            }
            LeftPanelAction::Resize => ctx.notify(),
            LeftPanelAction::ToggleProjectGroup { project_key } => {
                if !self.collapsed_project_groups.remove(project_key) {
                    self.collapsed_project_groups.insert(project_key.clone());
                }
                ctx.notify();
            }
            LeftPanelAction::ToggleWorkspaceGroup { project_key, group } => {
                let key = (project_key.clone(), group.clone());
                if !self.collapsed_workspace_groups.remove(&key) {
                    self.collapsed_workspace_groups.insert(key);
                }
                ctx.notify();
            }
            LeftPanelAction::ToggleCoordinationGroup { team_id } => {
                if !self.collapsed_coordination.remove(team_id) {
                    self.collapsed_coordination.insert(team_id.clone());
                }
                ctx.notify();
            }
            LeftPanelAction::PreviewWorkspaceDrop(preview) => {
                if self.sidebar_drag_preview != *preview {
                    self.sidebar_drag_preview = preview.clone();
                    ctx.notify();
                }
            }
            LeftPanelAction::CommitWorkspaceDrop {
                source_tab_id,
                project_key,
                destination,
            } => {
                let destination = match destination {
                    Some(SidebarDropDestination::Tab { tab_id, .. }) if tab_id == source_tab_id => {
                        self.sidebar_drag_preview
                            .as_ref()
                            .filter(|preview| {
                                preview.source_tab_id == *source_tab_id
                                    && preview.project_key == *project_key
                            })
                            .map(|preview| preview.destination.clone())
                    }
                    other => other.clone(),
                };
                self.sidebar_drag_preview = None;
                if let Ok(mut target) = self.sidebar_drag_target_state.lock() {
                    *target = None;
                }
                if let Some(destination) = destination {
                    let action = match destination {
                        SidebarDropDestination::Group(group) => {
                            WorkspaceAction::MoveWorkspaceToGroup {
                                tab_id: *source_tab_id,
                                project_root: project_key.into(),
                                group,
                            }
                        }
                        SidebarDropDestination::Tab { tab_id, group } => {
                            WorkspaceAction::ReorderWorkspaceTab {
                                tab_id: *source_tab_id,
                                target_tab_id: tab_id,
                                project_root: project_key.into(),
                                group,
                            }
                        }
                    };
                    ctx.dispatch_typed_action_deferred(action);
                }
                ctx.notify();
            }
            LeftPanelAction::PreviewProjectDrop(preview) => {
                if self.project_drag_preview != *preview {
                    self.project_drag_preview = preview.clone();
                    ctx.notify();
                }
            }
            LeftPanelAction::CommitProjectDrop { source, target } => {
                self.project_drag_preview = None;
                if let Some(target) = target.as_ref().filter(|target| *target != source) {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::ReorderProject {
                        root: source.into(),
                        target_root: target.into(),
                    });
                }
                ctx.notify();
            }
            LeftPanelAction::ToggleProjects => {
                self.projects_collapsed = !self.projects_collapsed;
                if self.projects_collapsed {
                    // Collapsing clears the filter: the list
                    // reopens unfiltered.
                    let editor = self.sidebar_search_editor.clone();
                    ctx.update_view(&editor, |editor, editor_ctx| {
                        editor.set_text("", editor_ctx);
                    });
                }
                ctx.notify();
            }
            LeftPanelAction::ToggleHiddenProjects => {
                self.hidden_projects_collapsed = !self.hidden_projects_collapsed;
                ctx.notify();
            }
            LeftPanelAction::ToggleProjectsSearch => {
                self.projects_search_expanded = !self.projects_search_expanded;
                if self.projects_search_expanded {
                    self.sidebar_focus = SidebarFocus::Search;
                    ctx.focus(&self.sidebar_search_editor);
                } else {
                    let editor = self.sidebar_search_editor.clone();
                    ctx.update_view(&editor, |editor, editor_ctx| {
                        editor.set_text("", editor_ctx);
                    });
                }
                ctx.notify();
            }
            LeftPanelAction::ClearProjectsSearch => {
                let editor = self.sidebar_search_editor.clone();
                ctx.update_view(&editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                });
                ctx.notify();
            }
            LeftPanelAction::CycleSortMode => {
                self.sidebar_sort = match self.sidebar_sort {
                    SidebarSortMode::Manual => SidebarSortMode::Name,
                    SidebarSortMode::Name => SidebarSortMode::Manual,
                };
                ctx.notify();
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Render helpers

fn with_opacity(color: ColorU, opacity: f32) -> ColorU {
    ColorU::new(color.r, color.g, color.b, (f32::from(color.a) * opacity).round() as u8)
}

pub fn render_left_panel(props: LeftPanelProps<'_>, app: &AppContext) -> Box<dyn Element> {
    use warpui::SingletonEntity;
    let machines = crate::settings::machines_store::MachinesStore::handle(app);
    let ui = props.ui_font;
    let theme = props.theme;
    let workspace_id = props.workspace_view_id;
    let search = props.search.to_lowercase();
    // Use the live resizable width. Feeding the previous frame's measured row
    // width back into this constraint locked the content to its old size after
    // expanding or collapsing the panel.
    let panel_width = props.panel_width.max(1.0);

    let filtered: Vec<&TabData> = props
        .tabs
        .iter()
        .filter(|t| tab_matches_search(t, &search))
        .collect();
    let filtered_projects: Vec<&ProjectData> = props
        .projects
        .iter()
        .filter(|project| {
            search.is_empty()
                || project.label.to_lowercase().contains(&search)
                || project.key.to_lowercase().contains(&search)
        })
        .collect();

    let projects_header = render_projects_header(
        ui,
        theme,
        workspace_id,
        props.projects_collapsed,
        props.projects_search_expanded,
        props.sort_mode,
        props.search_editor,
        props.sidebar_mouse_states,
    );

    // Where a dragged project heading would land: above its target when
    // moving up, below it when moving down.
    let landing = props.project_drag_preview.and_then(|preview| {
        let at = |key: &str| props.projects.iter().position(|project| project.key == key);
        Some((preview.target.clone(), at(&preview.source)? < at(&preview.target)?))
    });
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if !props.projects_collapsed {
        if filtered.is_empty() && filtered_projects.is_empty() {
            list = list.with_child(
                Container::new(Box::new(
                    Text::new("No sessions".to_string(), ui, crate::theme::text_size(FONT_EMPTY))
                        .with_color(theme.sub_text()),
                ))
                .with_padding_left(14.0)
                .with_padding_right(14.0)
                .with_padding_top(18.0)
                .with_padding_bottom(18.0)
                .finish(),
            );
        } else {
            let mut groups: Vec<(Option<&str>, Option<&str>, Vec<&TabData>)> = Vec::new();
            for project in filtered_projects {
                groups.push((
                    Some(project.key.as_str()),
                    Some(project.label.as_str()),
                    Vec::new(),
                ));
            }
            for tab in filtered {
                let key = tab.project_key.as_deref();
                if let Some((_, _, tabs)) = groups.iter_mut().find(|(group, _, _)| *group == key) {
                    tabs.push(tab);
                } else {
                    groups.push((key, tab.project_label.as_deref(), vec![tab]));
                }
            }
            for (project_key, project_label, mut tabs) in groups {
                let mut project_column =
                    Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
                // Tabs sort within their project; the project order
                // itself never changes.
                sort_group_tabs(&mut tabs, props.sort_mode);
                // Searching temporarily reveals matching children without losing
                // the user's stored collapse preference.
                let is_collapsed = project_group_is_collapsed(
                    project_key,
                    &search,
                    props.collapsed_project_groups,
                );
                if let Some(project_label) = project_label {
                    let machine = project_key
                        .and_then(|key| crate::remote_fs::resolve(std::path::Path::new(key)))
                        .map(|remote| {
                            let store = machines.as_ref(app);
                            (
                                store
                                    .machine(&remote.machine_id)
                                    .map(|m| m.name())
                                    .unwrap_or_else(|| "Remote machine".into()),
                                store.status(&remote.machine_id),
                            )
                        });
                    let header = render_project_group_header(
                        workspace_id,
                        ui,
                        theme,
                        project_key.unwrap_or(project_label),
                        project_label,
                        tabs.len(),
                        is_collapsed,
                        props.sidebar_mouse_states,
                        machine,
                        project_key.map(|key| project_drag(props.project_drag_states, key)),
                        project_key
                            .and_then(|key| props.projects.iter().find(|project| project.key == key))
                            .and_then(|project| project.icon.clone()),
                    );
                    project_column = project_column.with_child(match project_key {
                        Some(key) => DropTarget::new(
                            header,
                            super::panel_nav::SidebarGroupDropTarget {
                                project_key: key.to_string(),
                                group: None,
                            },
                        )
                        .finish(),
                        None => header,
                    });
                }
                if is_collapsed {
                    list = list.with_children(project_section(
                        project_column.finish(),
                        project_key,
                        landing.as_ref(),
                        theme,
                    ));
                    continue;
                }
                // A coordination's workspaces sit in its group, above the
                // project's own rows and folders.
                let in_coordination: std::collections::HashSet<usize> = props
                    .coordination
                    .iter()
                    .filter(|group| group.project_key.as_deref() == project_key)
                    .flat_map(|group| group.members.iter().map(|member| member.tab_id))
                    .collect();
                for group in props
                    .coordination
                    .iter()
                    .filter(|group| group.project_key.as_deref() == project_key)
                {
                    let members: Vec<&TabData> = tabs
                        .iter()
                        .copied()
                        .filter(|tab| group.members.iter().any(|member| member.tab_id == tab.id))
                        .collect();
                    if members.is_empty() {
                        continue;
                    }
                    project_column = project_column.with_child(
                        super::panel_coordination::render_group(
                            group,
                            &super::panel_coordination::CoordinationRender {
                                ui_font: ui,
                                theme,
                                workspace_id,
                                tabs: &members,
                                active_tab_id: props.active_tab_id,
                                menu_tab_id: props.menu_tab_id,
                                collapsed: search.is_empty()
                                    && props.collapsed_coordination.contains(&group.team_id),
                                panel_width,
                                tab_mouse_states: props.tab_mouse_states,
                                sidebar_mouse_states: props.sidebar_mouse_states,
                            },
                        ),
                    );
                }
                tabs.retain(|tab| !in_coordination.contains(&tab.id));
                let named_groups = project_key
                    .and_then(|key| props.projects.iter().find(|project| project.key == key))
                    .map(|project| project.groups.as_slice())
                    .unwrap_or(&[]);
                let buckets = std::iter::once(None)
                    .chain(named_groups.iter().map(|name| Some(name.as_str())));
                for bucket in buckets {
                    let bucket_collapsed = bucket.is_some_and(|name| {
                        search.is_empty()
                            && project_key.is_some_and(|key| {
                                props
                                    .collapsed_workspace_groups
                                    .contains(&(key.to_string(), name.to_string()))
                            })
                    });
                    let mut bucket_column =
                        Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
                    if let Some(name) = bucket {
                        if let Some(project_key) = project_key {
                            let header = super::panel_nav::render_workspace_group_header(
                                workspace_id,
                                ui,
                                theme,
                                project_key,
                                name,
                                bucket_collapsed,
                                props.sidebar_mouse_states,
                            );
                            bucket_column = bucket_column.with_child(
                                DropTarget::new(
                                    header,
                                    super::panel_nav::SidebarGroupDropTarget {
                                        project_key: project_key.to_string(),
                                        group: Some(name.to_string()),
                                    },
                                )
                                .finish(),
                            );
                        }
                    }
                    let bucket_tabs = preview_bucket_tabs(
                        &tabs,
                        named_groups,
                        bucket,
                        bucket_collapsed,
                        project_key,
                        props.drag_preview,
                    );
                    for tab in bucket_tabs {
                        let tab_id = tab.id;
                        let is_active = Some(tab.id) == props.active_tab_id;
                        let is_renaming =
                            matches!(props.sidebar_focus, SidebarFocus::Rename(id) if id == tab_id);
                        let menu_open = props.menu_tab_id == Some(tab_id);
                        let accent = tab_accent_color(tab, is_active, theme.accent);
                        let mouse_state = tab_mouse(props.tab_mouse_states, tab_id);
                        let drag_state = tab_drag(props.tab_drag_states, tab_id);

                        let row = ConstrainedBox::new(render_tab_row(
                            ui,
                            theme,
                            workspace_id,
                            tab,
                            is_active,
                            is_renaming,
                            menu_open,
                            accent,
                            props.view_mode,
                            props.rename_editor,
                            mouse_state,
                            drag_state,
                            props.drag_target_state.clone(),
                            props.sidebar_mouse_states,
                            usize::from(project_label.is_some()) + usize::from(bucket.is_some()),
                        ))
                        .with_width(panel_width)
                        .finish();
                        bucket_column = bucket_column.with_child(match project_key {
                            Some(key) => DropTarget::new(
                                row,
                                super::panel_nav::SidebarTabDropTarget {
                                    project_key: key.to_string(),
                                    tab_id,
                                    group: bucket.map(str::to_string),
                                },
                            )
                            .finish(),
                            None => row,
                        });
                    }
                    // Leave a destination below the last row, including when the
                    // folder is empty. The whole folder section accepts drops.
                    let section = Container::new(bucket_column.finish())
                        .with_padding_bottom(if bucket.is_some() { 12.0 } else { 0.0 })
                        .finish();
                    project_column = project_column.with_child(match (bucket, project_key) {
                        (Some(name), Some(project_key)) => DropTarget::new(
                            section,
                            super::panel_nav::SidebarGroupDropTarget {
                                project_key: project_key.to_string(),
                                group: Some(name.to_string()),
                            },
                        )
                        .finish(),
                        (None, Some(project_key)) => DropTarget::new(
                            section,
                            super::panel_nav::SidebarGroupDropTarget {
                                project_key: project_key.to_string(),
                                group: None,
                            },
                        )
                        .finish(),
                        _ => section,
                    });
                }
                list = list.with_children(project_section(
                    project_column.finish(),
                    project_key,
                    landing.as_ref(),
                    theme,
                ));
            }
        }
    }

    if !props.hidden_projects.is_empty() {
        let hidden_count = props.hidden_projects.len();
        let hidden_collapsed = props.hidden_projects_collapsed;
        let hidden_header = Hoverable::new(
            sidebar_mouse(props.sidebar_mouse_states, "hidden-projects-header"),
            move |state| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(7.0)
                        .with_child(render_icon(
                            if hidden_collapsed {
                                AppIcon::ChevronRight
                            } else {
                                AppIcon::ChevronDown
                            },
                            theme.sub_text(),
                            11.0,
                        ))
                        .with_child(
                            Text::new(
                                format!("Hidden projects ({hidden_count})"),
                                ui,
                                crate::theme::text_size(12.5),
                            )
                            .with_color(theme.sub_text())
                            .finish(),
                        )
                        .finish(),
                )
                .with_background_color(if state.is_hovered() {
                    theme.sidebar_row_hover()
                } else {
                    ColorU::transparent_black()
                })
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(9.0)
                .with_padding_bottom(9.0)
                .finish()
            },
        )
        .with_cursor(Cursor::PointingHand)
        .on_click(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(LeftPanelAction::ToggleHiddenProjects);
        })
        .finish();
        list = list.with_child(hidden_header);
        if !hidden_collapsed {
            for project in props.hidden_projects {
                let key = project.key.clone();
                let label = project.label.clone();
                let mouse_key = format!("hidden-project-{key}");
                let row = Hoverable::new(
                    sidebar_mouse(props.sidebar_mouse_states, mouse_key),
                    move |state| {
                        Container::new(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(8.0)
                                .with_child(render_icon(AppIcon::Folder, theme.sub_text(), 14.0))
                                .with_child(
                                    Expanded::new(
                                        1.0,
                                        Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                                            .with_color(theme.main_text())
                                            .soft_wrap(false)
                                            .with_clip(ClipConfig::ellipsis())
                                            .finish(),
                                    )
                                    .finish(),
                                )
                                .with_child(
                                    Text::new("Show".to_string(), ui, crate::theme::text_size(11.0))
                                        .with_color(theme.sub_text())
                                        .finish(),
                                )
                                .finish(),
                        )
                        .with_background_color(if state.is_hovered() {
                            theme.sidebar_row_hover()
                        } else {
                            ColorU::transparent_black()
                        })
                        .with_padding_left(26.0)
                        .with_padding_right(12.0)
                        .with_padding_top(7.0)
                        .with_padding_bottom(7.0)
                        .finish()
                    },
                )
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_id,
                        WorkspaceAction::ShowProjectInSidebar {
                            root: key.clone().into(),
                        },
                    );
                })
                .finish();
                list = list.with_child(row);
            }
        }
    }

    // A vertical scrollable may measure its content with an unbounded width.
    // Give the rows the actual panel width before their horizontal Expanded
    // children are laid out; otherwise flex cannot calculate the ellipsis
    // budget and debug builds correctly reject the infinite constraint.
    let list = ConstrainedBox::new(list.finish())
        .with_width(panel_width.max(1.0))
        .finish();

    let scroll = ClippedScrollable::vertical(
        props.scroll_state.clone(),
        SavePosition::new(
            Container::new(list)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
            LEFT_PANEL_ROWS_POSITION_ID,
        )
        .finish(),
        ScrollbarWidth::Auto,
        // Softer than the text colours they come from: 50% opacity.
        with_opacity(theme.disabled_text(), 0.5).into(),
        with_opacity(theme.main_text(), 0.5).into(),
        theme.surface_2().into(),
    )
    // Overlay the scrollbar on top of the rows so cards span the full panel
    // width instead of stopping short of the scrollbar gutter.
    .with_overlayed_scrollbar()
    .with_square_thumb()
    .finish();

    let nav = render_sidebar_nav(
        ui,
        theme,
        workspace_id,
        props.sidebar_mouse_states,
        props.automation_failures,
        props.teams_needing_you,
        props.nav_active,
    );

    // Same unbounded-width guard as the rows below: the nav and header
    // rows contain Expanded children, which debug builds reject under an
    // infinite max constraint (first frame before the Resizable sizes us).
    let top = ConstrainedBox::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(nav)
            .with_child(projects_header)
            .finish(),
    )
    .with_width(panel_width.max(1.0))
    .finish();

    let mut root_column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(top);
    // The list takes the remaining height so the organisation area stays
    // pinned to the bottom of the sidebar.
    root_column = root_column.with_child(
        Expanded::new(
            1.0,
            if !props.projects_collapsed || !props.hidden_projects.is_empty() {
                scroll
            } else {
                Empty::new().finish()
            },
        )
        .finish(),
    );
    root_column = root_column.with_child(
        ConstrainedBox::new(props.footer)
            .with_width(panel_width.max(1.0))
            .finish(),
    );
    let root = Container::new(root_column.finish())
        .with_background_color(theme.sidebar_background())
        .with_border(Border::right(1.0).with_border_color(theme.border()))
        .finish();

    EventHandler::new(root)
        .on_left_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(LeftPanelAction::Focus);
            DispatchEventResult::StopPropagation
        })
        .finish()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod truncate_tests {
    use std::collections::HashSet;

    use super::TabData;
    use super::{
        move_onto, order_projects, preview_bucket_tabs, project_group_is_collapsed,
        sort_group_tabs, truncate_px, ProjectData, SidebarDragPreview, SidebarDropDestination,
        SidebarSortMode, SidebarUiState,
    };

    #[test]
    fn a_dropped_item_takes_the_targets_place_from_either_side() {
        let mut items = vec!["a", "b", "c", "d"];
        // Down/right: lands after the target.
        assert!(move_onto(&mut items, &"a", &"c"));
        assert_eq!(items, ["b", "c", "a", "d"]);
        // Up/left: lands before it.
        assert!(move_onto(&mut items, &"d", &"b"));
        assert_eq!(items, ["d", "b", "c", "a"]);
        assert!(!move_onto(&mut items, &"b", &"b"));
        assert!(!move_onto(&mut items, &"x", &"b"));
        assert_eq!(items, ["d", "b", "c", "a"]);
    }

    #[test]
    fn projects_follow_the_arranged_order_and_new_ones_come_after() {
        let project = |key: &str| ProjectData {
            key: key.to_string(),
            label: key.to_string(),
            groups: Vec::new(),
            icon: None,
        };
        let mut projects = vec![project("/a"), project("/new"), project("/b"), project("/c")];
        order_projects(&mut projects, &["/c".to_string(), "/a".to_string(), "/b".to_string()]);
        let keys: Vec<&str> = projects.iter().map(|project| project.key.as_str()).collect();
        assert_eq!(keys, ["/c", "/a", "/b", "/new"]);
        // Nothing arranged yet: the order they were opened in.
        let mut projects = vec![project("/b"), project("/a")];
        order_projects(&mut projects, &[]);
        assert_eq!(projects[0].key, "/b");
    }

    #[test]
    fn the_arranged_project_order_is_saved_and_older_saves_still_load() {
        let older: SidebarUiState = serde_json::from_str(r#"{"compact":true}"#).unwrap();
        assert!(older.project_order.is_empty() && older.compact);
        let saved = SidebarUiState {
            project_order: vec!["/c".into(), "/a".into()],
            ..Default::default()
        };
        let restored: SidebarUiState =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(restored.project_order, ["/c", "/a"]);
    }

    fn test_tab(id: usize, label: &str) -> TabData {
        TabData {
            id,
            label: label.to_string(),
            workspace_id: None,
            branch_subtitle: None,
            group: None,
            color: None,
            is_running: false,
            title: label.to_string(),
            cwd: String::new(),
            project_key: None,
            project_label: None,
            purpose_label: String::new(),
            is_work_item: false,
            ai_tool: None,
            last_command: None,
            is_shared: false,
            agent_status: crate::workspace::worktree_status::WorktreeAgentStatus::Idle,
            agents: Vec::new(),
            diff_files: 0,
            diff_added: 0,
            diff_deleted: 0,
            pr: None,
            git_root: None,
            branch: None,
        }
    }

    #[test]
    fn opening_workspace_acknowledges_completion_without_clearing_live_attention() {
        use crate::workspace::worktree_status::{SidebarAgent, WorktreeAgentStatus as Status};
        let mut tab = test_tab(1, "Workspace");
        tab.agents = [Status::Review, Status::Permission, Status::Working { label: String::new() }]
            .into_iter().map(|status| SidebarAgent {
                image: None, pane_id: uuid::Uuid::new_v4(), tool: None,
                base_status: Status::Idle, status, cli: true, activity_known: true,
            }).collect();
        tab.acknowledge_activity();
        assert_eq!(tab.agents[0].status, Status::Idle);
        assert_eq!(tab.agents[1].status, Status::Permission);
        assert!(matches!(tab.agents[2].status, Status::Working { .. }));
        assert_eq!(tab.agent_status, Status::Permission);
        assert_eq!(tab.agents[0].status.after_cli_event(Some("Stop"), false), Status::Idle);
    }

    #[test]
    fn opening_one_tab_only_clears_that_panes_review() {
        use crate::workspace::worktree_status::{SidebarAgent, WorktreeAgentStatus as Status};
        let mut tab = test_tab(1, "Workspace");
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        tab.agents = [first, second].into_iter().map(|pane_id| SidebarAgent {
            image: None, pane_id, tool: None,
            base_status: Status::Idle, status: Status::Review, cli: true, activity_known: true,
        }).collect();
        tab.agent_status = Status::Review;
        tab.acknowledge_pane_activity(first);
        assert_eq!(tab.agents[0].status, Status::Idle);
        assert_eq!(tab.agents[1].status, Status::Review);
        assert_eq!(tab.agent_status, Status::Review);
    }

    #[test]
    fn drag_preview_moves_out_of_and_back_into_a_folder() {
        let mut source = test_tab(1, "Source");
        source.group = Some("Backend".into());
        let root = test_tab(2, "Root");
        let mut sibling = test_tab(3, "Sibling");
        sibling.group = Some("Backend".into());
        let tabs = [source, root, sibling];
        let refs: Vec<_> = tabs.iter().collect();
        let groups = ["Backend".to_string()];
        let ids = |bucket: Option<&str>, preview: &SidebarDragPreview| -> Vec<_> {
            preview_bucket_tabs(
                &refs,
                &groups,
                bucket,
                false,
                Some("/project"),
                Some(preview),
            )
            .iter()
            .map(|tab| tab.id)
            .collect()
        };
        let out = SidebarDragPreview {
            source_tab_id: 1,
            project_key: "/project".into(),
            destination: SidebarDropDestination::Tab {
                tab_id: 2,
                group: None,
            },
        };
        assert_eq!(ids(None, &out), [2, 1]);
        assert_eq!(ids(Some("Backend"), &out), [3]);
        let back = SidebarDragPreview {
            destination: SidebarDropDestination::Tab {
                tab_id: 3,
                group: Some("Backend".into()),
            },
            ..out
        };
        assert_eq!(ids(None, &back), [2]);
        assert_eq!(ids(Some("Backend"), &back), [3, 1]);
    }

    #[test]
    fn sort_manual_preserves_insertion_order() {
        let tabs = vec![
            test_tab(1, "zebra"),
            test_tab(2, "apple"),
            test_tab(3, "Mango"),
        ];
        let refs: Vec<&TabData> = tabs.iter().collect();
        let mut sorted = refs.clone();
        sort_group_tabs(&mut sorted, SidebarSortMode::Manual);
        let labels: Vec<_> = sorted.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(labels, ["zebra", "apple", "Mango"]);
    }

    #[test]
    fn sort_name_orders_case_insensitively() {
        let tabs = vec![
            test_tab(1, "zebra"),
            test_tab(2, "apple"),
            test_tab(3, "Mango"),
        ];
        let refs: Vec<&TabData> = tabs.iter().collect();
        let mut sorted = refs.clone();
        sort_group_tabs(&mut sorted, SidebarSortMode::Name);
        let labels: Vec<_> = sorted.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(labels, ["apple", "Mango", "zebra"]);
    }

    #[test]
    fn working_workspaces_rise_to_the_top_and_keep_their_order() {
        use super::super::worktree_status::WorktreeAgentStatus;
        let working = || WorktreeAgentStatus::Working { label: "Working".into() };
        let mut tabs = vec![
            test_tab(1, "zebra"),
            test_tab(2, "apple"),
            test_tab(3, "Mango"),
            test_tab(4, "kiwi"),
        ];
        tabs[2].agent_status = working();
        tabs[3].agent_status = WorktreeAgentStatus::Permission;
        tabs[1].agent_status = WorktreeAgentStatus::Review;
        let refs: Vec<&TabData> = tabs.iter().collect();

        let mut manual = refs.clone();
        sort_group_tabs(&mut manual, SidebarSortMode::Manual);
        let labels: Vec<_> = manual.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(labels, ["Mango", "kiwi", "zebra", "apple"], "working, then the rest in their own order");

        let mut by_name = refs.clone();
        sort_group_tabs(&mut by_name, SidebarSortMode::Name);
        let labels: Vec<_> = by_name.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(labels, ["kiwi", "Mango", "apple", "zebra"], "each part still by name");

        // Done: back in its place.
        tabs[2].agent_status = WorktreeAgentStatus::Idle;
        tabs[3].agent_status = WorktreeAgentStatus::Idle;
        let mut settled: Vec<&TabData> = tabs.iter().collect();
        sort_group_tabs(&mut settled, SidebarSortMode::Manual);
        let labels: Vec<_> = settled.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(labels, ["zebra", "apple", "Mango", "kiwi"]);
    }

    #[test]
    fn short_strings_pass_through() {
        assert_eq!(truncate_px("~", 200.0, 15.5), "~");
        assert_eq!(truncate_px("dragent", 200.0, 15.5), "dragent");
    }

    #[test]
    fn long_titles_ellipsize_at_min_panel_width() {
        // MIN_PANEL_WIDTH (180) leaves ~76px for the title → must shrink.
        let t = truncate_px("betahealthtech-ui", 76.0, 15.5);
        assert!(t.ends_with('…'), "expected ellipsis: {t}");
        assert!(t.chars().count() < "betahealthtech-ui".chars().count());
    }

    #[test]
    fn subtitle_fits_one_line_at_min_width() {
        // The exact string from the bug screenshot that wrapped onto 3 lines.
        let s = truncate_px("git pull origin c...", 78.0, 11.5);
        // ~78px at 11.5px font ≈ 12 chars — must be truncated, not wrapped.
        assert!(s.chars().count() <= 13, "still too long: {s}");
        assert!(s.ends_with('…'));
    }

    #[test]
    fn budget_never_underflows() {
        // Degenerate widths keep a minimum 4-char budget instead of panicking.
        let t = truncate_px("abcdefgh", 1.0, 15.5);
        assert_eq!(t.chars().count(), 4); // 3 chars + ellipsis
    }

    #[test]
    fn project_groups_collapse_independently_and_search_reveals_matches() {
        let collapsed = HashSet::from(["/project/a".to_string()]);

        assert!(project_group_is_collapsed(
            Some("/project/a"),
            "",
            &collapsed
        ));
        assert!(!project_group_is_collapsed(
            Some("/project/b"),
            "",
            &collapsed
        ));
        assert!(!project_group_is_collapsed(
            Some("/project/a"),
            "develop",
            &collapsed
        ));
    }
}

/// Re-exported area modules; existing super::left_panel paths keep
/// working. Canonical homes are the panel modules.
pub(crate) use super::panel_nav::*;
pub(crate) use super::panel_tabs::*;

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SidebarUiState {
    project_order: Vec<String>,
    collapsed_projects: HashSet<String>,
    collapsed_groups: HashSet<(String, String)>,
    projects_collapsed: bool,
    hidden_projects_collapsed: bool,
    sort_by_name: bool,
    scroll: f32,
    search_expanded: bool,
    compact: bool,
}

impl LeftPanelView {
    pub(crate) fn persist_ui(&self) -> String {
        serde_json::to_string(&SidebarUiState {
            project_order: self.project_order.clone(),
            collapsed_projects: self.collapsed_project_groups.clone(),
            collapsed_groups: self.collapsed_workspace_groups.clone(),
            projects_collapsed: self.projects_collapsed,
            hidden_projects_collapsed: self.hidden_projects_collapsed,
            sort_by_name: self.sidebar_sort == SidebarSortMode::Name,
            scroll: self.left_panel_scroll_state.scroll_start().as_f32(),
            search_expanded: self.projects_search_expanded,
            compact: self.sidebar_view_mode == SidebarViewMode::Compact,
        })
        .unwrap_or_default()
    }

    /// Move project `source` onto `target`'s place in the sidebar. Hidden
    /// projects keep their place in the saved order after the shown ones.
    pub(crate) fn move_project(&mut self, source: &str, target: &str) -> bool {
        let mut order: Vec<String> =
            self.projects.iter().map(|project| project.key.clone()).collect();
        if !move_onto(&mut order, &source.to_string(), &target.to_string()) {
            return false;
        }
        order.extend(
            self.project_order
                .iter()
                .filter(|key| !order.contains(key))
                .cloned()
                .collect::<Vec<_>>(),
        );
        self.project_order = order;
        order_projects(&mut self.projects, &self.project_order);
        true
    }

    pub(crate) fn restore_ui(&mut self, json: &str) {
        let Ok(state) = serde_json::from_str::<SidebarUiState>(json) else {
            return;
        };
        self.project_order = state.project_order;
        self.collapsed_project_groups = state.collapsed_projects;
        self.collapsed_workspace_groups = state.collapsed_groups;
        self.projects_collapsed = state.projects_collapsed;
        self.hidden_projects_collapsed = state.hidden_projects_collapsed;
        self.projects_search_expanded = state.search_expanded;
        self.sidebar_view_mode = if state.compact {
            SidebarViewMode::Compact
        } else {
            SidebarViewMode::List
        };
        self.left_panel_scroll_state
            .scroll_to(warpui_core::units::Pixels::new(state.scroll));
        self.sidebar_sort = if state.sort_by_name {
            SidebarSortMode::Name
        } else {
            SidebarSortMode::Manual
        };
    }
}
