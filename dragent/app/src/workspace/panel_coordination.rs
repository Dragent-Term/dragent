//! Agent Coordination in the sidebar: a coordination's workspaces (its lead's
//! and each task's) sit together under one group headed by its title, as
//! cards that say who each workspace is for and what its agents do. Every
//! other workspace keeps the ordinary row.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use uuid::Uuid;
use warpui::color::ColorU;
use warpui::elements::{
    ChildAnchor, OffsetPositioning, ParentAnchor, ParentOffsetBounds, Stack,
    Align, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    MainAxisSize, ParentElement, SavePosition, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::vec2f;
use warpui::EntityId;
use warpui_core::elements::{DispatchEventResult, EventHandler, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use dragent_host::team::{TaskStatus, TeamStatus};

use super::left_panel::{sidebar_mouse, tab_mouse, LeftPanelAction, TabData};
use super::worktree_status::{SidebarAgent, WorktreeAgentStatus};
use super::WorkspaceAction;
use crate::icons::{render_icon, AppIcon};
use crate::teams::sidebar::{CoordinationGroup, GroupTask};
use crate::theme::Theme;

/// Where a coordination stands, for its group's heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinationState {
    Planning,
    Working,
    NeedsYou,
    Done,
    Stopped,
}

/// One coordination's group in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarCoordination {
    pub team_id: String,
    pub title: String,
    /// The project heading it sits under.
    pub project_key: Option<String>,
    pub state: CoordinationState,
    pub done: usize,
    pub total: usize,
    /// Lead first, then tasks in plan order.
    pub members: Vec<CoordinationMember>,
}

/// One workspace in a coordination's group.
#[derive(Clone, Debug, PartialEq)]
pub struct CoordinationMember {
    /// The workspace's sidebar row.
    pub tab_id: usize,
    pub lead: bool,
    /// What the workspace is for: the task it works on; for the lead, the
    /// agent leading.
    pub title: String,
    /// The lead's workspace while the team is at work: its tile keeps the
    /// working loader even between the lead's own turns.
    pub at_work: bool,
    /// The task it is on (`t3`), `Lead`: orders the cards, never drawn.
    pub badge: String,
    /// Every task done in this checkout, in plan order. Listed on the card
    /// only when there are several (one task is the card's title).
    pub tasks: Vec<TaskLine>,
}

/// One task on a card that holds several.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskLine {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
}

fn same_folder(a: &str, b: &std::path::Path) -> bool {
    a.trim_end_matches('/') == b.to_string_lossy().trim_end_matches('/')
}

fn unfinished(task: &GroupTask) -> bool {
    !matches!(task.status, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled)
}

/// Put each workspace row that belongs to a coordination into its group:
/// the one its lead runs in, and the ones its tasks work in (by checkout, or
/// by the worker's session). A workspace joins one group at most.
pub(crate) fn assign(groups: &[CoordinationGroup], tabs: &[TabData]) -> Vec<SidebarCoordination> {
    let mut taken: HashSet<usize> = HashSet::new();
    let mut out = Vec::new();
    for group in groups {
        let has_pane = |tab: &TabData, pane: &str| tab.agents.iter().any(|agent| agent.pane_id.to_string() == pane);
        let mut members: Vec<CoordinationMember> = Vec::new();
        for tab in tabs {
            if tab.workspace_id.is_none() || taken.contains(&tab.id) {
                continue;
            }
            let lead = group.lead_pane.as_deref().is_some_and(|pane| has_pane(tab, pane));
            let tasks: Vec<&GroupTask> = group
                .tasks
                .iter()
                .filter(|task| {
                    task.workspace.as_deref().is_some_and(|folder| same_folder(&tab.cwd, folder))
                        || task.worker_pane.as_deref().is_some_and(|pane| has_pane(tab, pane))
                })
                .collect();
            if !lead && tasks.is_empty() {
                continue;
            }
            // The task it is on now, else the last it worked on.
            let current = tasks.iter().find(|task| unfinished(task)).or(tasks.last()).copied();
            let (title, badge) = match (lead, current) {
                (true, _) => (format!("Lead · {}", lead_doing(group)), "Lead".to_string()),
                (false, Some(task)) => (task.title.clone(), task.id.clone()),
                (false, None) => (tab.label.clone(), String::new()),
            };
            let tasks = tasks
                .iter()
                .map(|task| TaskLine { id: task.id.clone(), title: task.title.clone(), status: task.status })
                .collect();
            taken.insert(tab.id);
            let at_work = lead
                && matches!(group.status, TeamStatus::Planning | TeamStatus::Running)
                && group.needs_you == 0;
            members.push(CoordinationMember { tab_id: tab.id, lead, at_work, title, badge, tasks });
        }
        if members.is_empty() {
            continue;
        }
        // Lead first, then the plan's order.
        let order = |member: &CoordinationMember| -> usize {
            if member.lead {
                return 0;
            }
            let id = member.badge.split_whitespace().next().unwrap_or_default();
            1 + group.tasks.iter().position(|task| task.id == id).unwrap_or(usize::MAX - 1)
        };
        members.sort_by_key(order);
        let project_key = tabs
            .iter()
            .find(|tab| tab.id == members[0].tab_id)
            .and_then(|tab| tab.project_key.clone());
        let done = group.tasks.iter().filter(|task| task.status == TaskStatus::Done).count();
        let state = if group.needs_you > 0
            || group.tasks.iter().any(|task| task.status == TaskStatus::NeedsDecision)
        {
            CoordinationState::NeedsYou
        } else {
            match group.status {
                TeamStatus::Planning => CoordinationState::Planning,
                TeamStatus::Running => CoordinationState::Working,
                TeamStatus::Done => CoordinationState::Done,
                TeamStatus::Stopped => CoordinationState::Stopped,
            }
        };
        out.push(SidebarCoordination {
            team_id: group.id.clone(),
            title: group.title.clone(),
            project_key,
            state,
            done,
            total: group.tasks.len(),
            members,
        });
    }
    out
}

/// What the lead is doing, from where its coordination stands: the plan
/// being written, the tasks under way, the person's answer awaited, the
/// finished work being checked.
pub(crate) fn lead_doing(group: &CoordinationGroup) -> String {
    let asks = group.needs_you > 0
        || group.tasks.iter().any(|task| task.status == TaskStatus::NeedsDecision);
    match group.status {
        TeamStatus::Planning => "writes the plan".into(),
        TeamStatus::Done => "done".into(),
        TeamStatus::Stopped => "stopped".into(),
        TeamStatus::Running if asks => "waits for you".into(),
        TeamStatus::Running => {
            let under_way: Vec<&GroupTask> = group
                .tasks
                .iter()
                .filter(|task| matches!(task.status, TaskStatus::Starting | TaskStatus::Running))
                .collect();
            let ids = |tasks: &[&GroupTask]| tasks.iter().map(|task| task.id.as_str()).collect::<Vec<_>>().join(", ");
            let reviews: Vec<&GroupTask> = under_way.iter().copied().filter(|task| task.review).collect();
            if !reviews.is_empty() && reviews.len() == under_way.len() {
                format!("checks {}", ids(&reviews))
            } else if !under_way.is_empty() {
                format!("runs {}", ids(&under_way))
            } else if !group.tasks.is_empty()
                && group.tasks.iter().all(|task| matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled))
            {
                "checks the work".into()
            } else {
                "starts the next task".into()
            }
        }
    }
}

/// What a group needs from the sidebar to draw itself.
pub(crate) struct CoordinationRender<'a> {
    pub ui_font: FamilyId,
    pub theme: &'a Theme,
    pub workspace_id: EntityId,
    pub tabs: &'a [&'a TabData],
    pub active_tab_id: Option<usize>,
    pub menu_tab_id: Option<usize>,
    pub collapsed: bool,
    pub panel_width: f32,
    pub tab_mouse_states: &'a RefCell<HashMap<usize, MouseStateHandle>>,
    pub sidebar_mouse_states: &'a RefCell<HashMap<String, MouseStateHandle>>,
}

/// The group: its heading, then a card per workspace (unless folded).
pub(crate) fn render_group(group: &SidebarCoordination, props: &CoordinationRender<'_>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(render_heading(group, props));
    if !props.collapsed {
        for member in &group.members {
            if let Some(tab) = props.tabs.iter().find(|tab| tab.id == member.tab_id) {
                column = column.with_child(
                    ConstrainedBox::new(render_card(member, tab, props)).with_width(props.panel_width).finish(),
                );
            }
        }
    }
    Container::new(column.finish()).with_padding_bottom(8.0).finish()
}

fn state_color(state: CoordinationState, theme: &Theme) -> ColorU {
    match state {
        CoordinationState::Working | CoordinationState::Planning => super::worktree_status::WORKING_AMBER,
        CoordinationState::NeedsYou => super::worktree_status::needs_input_color(theme),
        CoordinationState::Done => theme.ui_green(),
        CoordinationState::Stopped => theme.disabled_text(),
    }
}

fn render_heading(group: &SidebarCoordination, props: &CoordinationRender<'_>) -> Box<dyn Element> {
    let theme = *props.theme;
    let ui = props.ui_font;
    let workspace_id = props.workspace_id;
    let collapsed = props.collapsed;
    let title = group.title.clone();
    let state = group.state;
    let progress = match state {
        CoordinationState::NeedsYou => "needs you".to_string(),
        CoordinationState::Planning => "planning".to_string(),
        CoordinationState::Stopped => "stopped".to_string(),
        _ if group.total == 0 => String::new(),
        _ => format!("{}/{}", group.done, group.total),
    };
    let team = group.team_id.clone();
    let open_mouse = sidebar_mouse(props.sidebar_mouse_states, format!("coordination_open:{team}"));
    let toggle_team = team.clone();
    Hoverable::new(
        sidebar_mouse(props.sidebar_mouse_states, format!("coordination:{team}")),
        move |state_mouse| {
            let hovered = state_mouse.is_hovered();
            let trailing: Box<dyn Element> = if hovered {
                let team = team.clone();
                Hoverable::new(open_mouse.clone(), move |mouse| {
                    let button = Container::new(render_icon(
                        AppIcon::ArrowRight,
                        if mouse.is_hovered() { theme.main_text() } else { theme.sub_text() },
                        11.0,
                    ))
                    .with_padding_left(4.0)
                    .with_padding_right(4.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .finish();
                    crate::machine_ui::with_tooltip(&theme, ui, button, mouse.is_hovered(), "Open in Agent Coordination")
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action_for(workspace_id, WorkspaceAction::OpenTeams { team: Some(team.clone()) })
                })
                .finish()
            } else {
                Text::new(progress.clone(), ui, crate::theme::text_size(10.5))
                    .with_color(if state == CoordinationState::NeedsYou { super::worktree_status::needs_input_color(&theme) } else { theme.sub_text() })
                    .finish()
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(
                        if collapsed { AppIcon::ChevronRight } else { AppIcon::ChevronDown },
                        theme.disabled_text(),
                        10.0,
                    ))
                    .with_child(render_icon(AppIcon::Layers, state_color(state, &theme), 10.5))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(6.0)
                                .with_child(
                                    warpui::elements::Shrinkable::new(
                                        1.0,
                                        Text::new(title.clone(), ui, crate::theme::text_size(12.5))
                                            .with_color(theme.main_text())
                                            .with_style(Properties::default().weight(Weight::Semibold))
                                            .soft_wrap(false)
                                            .with_clip(ClipConfig::ellipsis())
                                            .finish(),
                                    )
                                    .finish(),
                                )
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(trailing)
                    .finish(),
            )
            .with_background_color(if hovered { theme.sidebar_row_hover() } else { ColorU::transparent_black() })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(super::panel_nav::tree_lead(1))
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(6.0)
            .finish()
        },
    )
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(LeftPanelAction::ToggleCoordinationGroup { team_id: toggle_team.clone() })
    })
    .finish()
}

/// The agent's own mark: its artwork, else its brand icon.
fn agent_glyph(agent: Option<&SidebarAgent>, theme: &Theme) -> Box<dyn Element> {
    use warpui_core::assets::asset_cache::AssetSource;
    use warpui_core::image_cache::CacheOption;
    let source = agent.and_then(|agent| match &agent.image {
        Some(super::AgentImage::Bundled(path)) => Some(AssetSource::Bundled { path }),
        Some(super::AgentImage::File(path)) => Some(AssetSource::LocalFile { path: path.clone() }),
        None => None,
    });
    let glyph = match source {
        Some(source) => warpui::elements::Image::new(source, CacheOption::BySize).contain().finish(),
        None => match agent.and_then(|agent| agent.tool) {
            Some(tool) => render_icon(crate::console::ai_toolbar::brand_icon(tool), theme.main_text(), 9.5),
            None => render_icon(AppIcon::Agent, theme.sub_text(), 9.5),
        },
    };
    ConstrainedBox::new(Align::new(glyph).finish()).with_width(10.5).with_height(10.5).finish()
}

/// A task's state as a small mark: a tick when done, a cross when it
/// failed, a dot otherwise (amber while it runs).
fn task_mark(status: TaskStatus, theme: &Theme) -> Box<dyn Element> {
    let icon = |icon: AppIcon, color: ColorU| -> Box<dyn Element> {
        ConstrainedBox::new(Align::new(render_icon(icon, color, 10.0)).finish())
            .with_width(12.0)
            .with_height(12.0)
            .finish()
    };
    let dot = |color: ColorU| -> Box<dyn Element> {
        ConstrainedBox::new(
            Align::new(
                Container::new(ConstrainedBox::new(Empty::new().finish()).with_width(5.0).with_height(5.0).finish())
                    .with_background_color(color)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.5)))
                    .finish(),
            )
            .finish(),
        )
        .with_width(12.0)
        .with_height(12.0)
        .finish()
    };
    match status {
        TaskStatus::Done => icon(AppIcon::Check, theme.ui_green()),
        TaskStatus::Failed => icon(AppIcon::XClose, theme.ui_red()),
        TaskStatus::Cancelled => icon(AppIcon::XClose, theme.disabled_text()),
        TaskStatus::Running | TaskStatus::Starting => dot(super::worktree_status::WORKING_AMBER),
        TaskStatus::Blocked | TaskStatus::NeedsDecision => dot(super::worktree_status::needs_input_color(theme)),
        TaskStatus::Pending => dot(theme.disabled_text()),
    }
}

/// Trial (2026-11-13): every card keeps the hover background, not only the
/// one under the pointer. `false` restores hover-only.
const ALWAYS_HOVER_BACKGROUND: bool = true;

/// Agents shown in a card's stack; more show as "+N".
const MAX_STACKED_AGENTS: usize = 3;
/// An avatar's side and how far each next one sits (a third overlaps).
const AVATAR: f32 = 17.0;
const STACK_STEP: f32 = 12.0;

/// One logo per kind of agent: three OpenCode sessions show one OpenCode,
/// whose mark is the most urgent of theirs (and opens that session).
pub(crate) fn one_per_kind(agents: &[SidebarAgent]) -> Vec<SidebarAgent> {
    let kind = |agent: &SidebarAgent| match (&agent.tool, &agent.image) {
        (Some(tool), _) => format!("tool:{}", tool.label()),
        (None, Some(super::AgentImage::Bundled(path))) => format!("image:{path}"),
        (None, Some(super::AgentImage::File(path))) => format!("file:{path}"),
        (None, None) => "agent".to_string(),
    };
    let mut out: Vec<(String, SidebarAgent)> = Vec::new();
    for agent in agents {
        let key = kind(agent);
        match out.iter_mut().find(|(seen, _)| *seen == key) {
            Some((_, kept)) => {
                let higher = WorktreeAgentStatus::pick_higher(kept.status.clone(), agent.status.clone());
                if higher != kept.status {
                    *kept = agent.clone();
                }
            }
            None => out.push((key, agent.clone())),
        }
    }
    out.into_iter().map(|(_, agent)| agent).collect()
}

/// A workspace's card: what it's for, then its agents, branch and changes.
fn render_card(member: &CoordinationMember, tab: &TabData, props: &CoordinationRender<'_>) -> Box<dyn Element> {
    let theme = *props.theme;
    let ui = props.ui_font;
    let workspace_id = props.workspace_id;
    let tab_id = tab.id;
    let navigation = tab.workspace_id;
    let is_active = props.active_tab_id == Some(tab_id);
    let menu_open = props.menu_tab_id == Some(tab_id);
    let title = member.title.clone();
    let lead = member.lead;
    // While the team works, the lead's tile shows it working, its own turns
    // or not (it waits on tasks between them).
    let status = match tab.agent_status.clone() {
        WorktreeAgentStatus::Idle if member.at_work => WorktreeAgentStatus::Working { label: String::new() },
        status => status,
    };
    let branch = tab.branch.clone().unwrap_or_default();
    let (added, deleted) = (tab.diff_added, tab.diff_deleted);
    let agents: Vec<SidebarAgent> = one_per_kind(&tab.agents);
    // The other tasks done in this checkout (the current one is the title).
    let tasks: Vec<TaskLine> = member.tasks.iter().filter(|task| task.id != member.badge).cloned().collect();
    let menu_mouse = sidebar_mouse(props.sidebar_mouse_states, format!("menu_{tab_id}"));
    let close_mouse = sidebar_mouse(props.sidebar_mouse_states, format!("close_{tab_id}"));
    let agent_mice: Vec<MouseStateHandle> = agents
        .iter()
        .map(|agent| sidebar_mouse(props.sidebar_mouse_states, format!("coordination_agent:{}", agent.pane_id)))
        .collect();
    let workspace_root = std::path::PathBuf::from(&tab.cwd);
    // The same details card as every workspace row, beside the card.
    let hover_tab = tab.clone();
    let hover_branch_mouse = sidebar_mouse(props.sidebar_mouse_states, format!("hover_branch_{tab_id}"));
    let git_root = tab.git_root.clone();
    let card = Hoverable::new(tab_mouse(props.tab_mouse_states, tab_id), move |state| {
        let hovered = state.is_hovered();
        let card = workspace_card(
            &CardFace {
                theme,
                ui,
                workspace_id,
                tab_id,
                navigation,
                is_active,
                menu_open,
                hovered,
                accent: theme.accent,
                title: &title,
                lead,
                status: &status,
                branch: &branch,
                diff: (added, deleted),
                agents: &agents,
                tasks: &tasks,
                workspace_root: &workspace_root,
                depth: 1,
            },
            &CardMice { menu: &menu_mouse, close: &close_mouse, agents: &agent_mice },
        );
        if hovered && !menu_open {
            Stack::new()
                .with_child(card)
                .with_positioned_overlay_child(
                    super::panel_tabs::worktree_hover_card(
                        &hover_tab,
                        &theme,
                        ui,
                        workspace_id,
                        hover_branch_mouse.clone(),
                    ),
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopRight,
                        ChildAnchor::TopLeft,
                    ),
                )
                .finish()
        } else {
            card
        }
    })
    .with_defer_events_to_children()
    .on_hover(move |hovered, ctx, _, _| {
        // Fresh branch and changes for the card while it's looked at.
        if let Some(root) = git_root.as_ref().filter(|_| hovered) {
            ctx.dispatch_typed_action_for(workspace_id, WorkspaceAction::RefreshWorktreeDetails { root: root.clone() });
        }
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        if let Some(workspace) = navigation {
            ctx.dispatch_typed_action_for(workspace_id, WorkspaceAction::SelectProjectWorkspace { workspace_id: workspace });
        }
    })
    .finish();
    EventHandler::new(card)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish()
}


/// What a workspace card shows. Coordination members and every ordinary
/// workspace row share the card, so the sidebar reads as one list.
pub(crate) struct CardFace<'a> {
    pub theme: Theme,
    pub ui: FamilyId,
    pub workspace_id: EntityId,
    pub tab_id: usize,
    pub navigation: Option<usize>,
    pub is_active: bool,
    pub menu_open: bool,
    pub hovered: bool,
    /// The selection colour (a workspace can choose its own).
    pub accent: ColorU,
    pub title: &'a str,
    pub lead: bool,
    pub status: &'a WorktreeAgentStatus,
    pub branch: &'a str,
    pub diff: (usize, usize),
    pub agents: &'a [SidebarAgent],
    pub tasks: &'a [TaskLine],
    pub workspace_root: &'a std::path::Path,
    /// How deep the row sits in the tree, so its tile lines up.
    pub depth: usize,
}

/// The pointer states a card's buttons keep between frames.
pub(crate) struct CardMice<'a> {
    pub menu: &'a MouseStateHandle,
    pub close: &'a MouseStateHandle,
    pub agents: &'a [MouseStateHandle],
}

/// Space between two cards, so each stands on its own.
pub(crate) const CARD_GAP: f32 = 6.0;

/// A workspace's card: its tile and title, then its agents, branch and
/// changes, and any other tasks it ran.
pub(crate) fn workspace_card(face: &CardFace<'_>, mice: &CardMice<'_>) -> Box<dyn Element> {
    let CardFace {
        theme,
        ui,
        workspace_id,
        tab_id,
        navigation,
        is_active,
        menu_open,
        hovered,
        accent,
        title,
        lead,
        status,
        branch,
        diff: (added, deleted),
        agents,
        tasks,
        workspace_root,
        depth,
    } = *face;
    let (menu_mouse, close_mouse, agent_mice) = (mice.menu, mice.close, mice.agents);
    let selection = {
        let mut selection = theme;
        selection.accent = accent;
        selection
    };
    let text = |text: String, size: f32, color: ColorU| -> Box<dyn Element> {
        Text::new(text, ui, crate::theme::text_size(size))
            .with_color(color)
            .soft_wrap(false)
            .with_clip(ClipConfig::ellipsis())
            .finish()
    };

    // Title line: status (a crown for the lead), what it's for, and
    // the row actions on hover.
    // The workspace tile the ordinary rows use (lap while working,
    // corner mark), with a crown for the lead.
    let tile_bg = if is_active { selection.accent_bg() } else { theme.surface_2() };
    let mark = super::workspace_activity::workspace_icon_tile(
        if lead { AppIcon::Crown } else { AppIcon::GitBranch },
        if is_active { accent } else { theme.main_text() },
        tile_bg,
        status,
        theme.accent,
        &theme,
    );
    let actions: Box<dyn Element> = if hovered || menu_open {
        let menu = Hoverable::new(menu_mouse.clone(), move |_| {
            Container::new(render_icon(
                AppIcon::DotsVertical,
                if menu_open { theme.main_text() } else { theme.sub_text() },
                12.0,
            ))
            .with_padding_left(3.0)
            .with_padding_right(2.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(workspace_id, WorkspaceAction::ToggleTabMenu { tab_id })
        })
        .finish();
        let close = Hoverable::new(close_mouse.clone(), move |_| {
            Container::new(render_icon(AppIcon::XClose, theme.sub_text(), 11.0))
                .with_padding_left(2.0)
                .with_padding_right(2.0)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            if let Some(workspace) = navigation {
                ctx.dispatch_typed_action_for(
                    workspace_id,
                    WorkspaceAction::CloseProjectWorkspace { workspace_id: workspace },
                );
            }
        })
        .finish();
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(SavePosition::new(menu, &format!("tab_menu_btn_{tab_id}")).finish())
            .with_child(close)
            .finish()
    } else {
        Empty::new().finish()
    };
    let title_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.0)
        .with_child(mark)
        .with_child(Expanded::new(1.0, text(title.to_string(), 13.0, theme.main_text())).finish())
        .with_child(actions)
        .finish();

    // Its agents (each opens its tab), stacked like avatars so a busy
    // workspace never pushes its branch and changes off the card.
    let shown = agents.len().min(MAX_STACKED_AGENTS);
    let extra = agents.len() - shown;
    let mut pile = Stack::new().with_child(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(AVATAR + STACK_STEP * shown.saturating_sub(1) as f32)
            .with_height(AVATAR)
            .finish(),
    );
    // Drawn right to left, so the first agent sits on top.
    for (index, (agent, mouse)) in agents.iter().zip(agent_mice.iter()).take(shown).enumerate().rev() {
        let pane = agent.pane_id.to_string();
        let root = workspace_root.to_path_buf();
        let agent = agent.clone();
        let avatar = Hoverable::new(mouse.clone(), move |_| {
            // Round, like the details card's avatars, with a ring in the
            // sidebar's colour that keeps overlapping logos apart. The
            // workspace's tile beside the title carries the status.
            Container::new(
                ConstrainedBox::new(Align::new(agent_glyph(Some(&agent), &theme)).finish())
                    .with_width(AVATAR)
                    .with_height(AVATAR)
                    .finish(),
            )
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.5).with_border_color(theme.sidebar_background()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(AVATAR / 2.0 + 1.5)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                workspace_id,
                WorkspaceAction::OpenCoordinationPane { pane: pane.clone(), workspace: Some(root.clone()) },
            )
        })
        .finish();
        pile = pile.with_positioned_child(
            avatar,
            OffsetPositioning::offset_from_parent(
                vec2f(index as f32 * STACK_STEP - 1.5, -1.5),
                ParentOffsetBounds::Unbounded,
                ParentAnchor::TopLeft,
                ChildAnchor::TopLeft,
            ),
        );
    }
    let mut agents_row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(4.0);
    if shown > 0 {
        agents_row = agents_row.with_child(pile.finish());
    }
    if extra > 0 {
        agents_row = agents_row.with_child(
            Container::new(text(format!("+{extra}"), 10.0, theme.sub_text()))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(1.0)
                .with_padding_bottom(1.0)
                .with_background_color(theme.surface_2())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish(),
        );
    }
    let mut meta = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(5.0);
    if !agents.is_empty() {
        meta = meta.with_child(Container::new(agents_row.finish()).with_margin_right(3.0).finish());
    }
    if !branch.is_empty() {
        meta = meta
            .with_child(render_icon(AppIcon::GitBranch, theme.disabled_text(), 10.0))
            .with_child(Expanded::new(1.0, text(branch.to_string(), 11.0, theme.sub_text())).finish());
    } else {
        meta = meta.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    }
    if added + deleted > 0 {
        // Never wider than its slot: big counts shorten.
        let short = |count: usize| if count >= 10_000 { format!("{}k", count / 1000) } else { count.to_string() };
        meta = meta.with_child(
            Flex::row()
                .with_spacing(3.0)
                .with_child(text(format!("+{}", short(added)), 10.5, theme.ui_green()))
                .with_child(text(format!("−{}", short(deleted)), 10.5, theme.ui_red()))
                .finish(),
        );
    }

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.0)
        .with_child(title_row)
        .with_child(Container::new(meta.finish()).with_padding_left(21.0).with_padding_top(2.0).finish());

    // Other tasks in this checkout: one quiet line each.
    for task in tasks {
        body = body.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_spacing(5.0)
                    .with_child(task_mark(task.status, &theme))
                    .with_child(Expanded::new(1.0, text(task.title.clone(), 11.0, theme.sub_text())).finish())
                    .finish(),
            )
            .with_padding_left(21.0)
            .finish(),
        );
    }

    let background = if is_active {
        selection.accent_overlay()
    } else if hovered {
        theme.sidebar_row_hover()
    } else if ALWAYS_HOVER_BACKGROUND {
        // 70% of the hover colour, so the one under the pointer still stands out.
        let hover = theme.sidebar_row_hover();
        ColorU::new(hover.r, hover.g, hover.b, (f32::from(hover.a) * 0.7).round() as u8)
    } else {
        ColorU::transparent_black()
    };
    Container::new(body.finish())
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        // The tile lines up with the workspace rows' icons.
        .with_margin_left(super::panel_nav::tree_lead(depth) - 8.0)
        .with_margin_right(8.0)
        .with_margin_bottom(CARD_GAP)
        .finish()
}

/// A sample coordination at sidebar width, for the screen previews.
pub(crate) fn preview(ui_font: FamilyId, theme: &Theme, workspace_id: EntityId) -> Box<dyn Element> {
    use crate::console::ai_toolbar::AiTool;
    let (lead, w1, w2, w3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let row = |id: usize, label: &str, cwd: &str, branch: &str, agents: Vec<(Uuid, AiTool, WorktreeAgentStatus)>, diff: (usize, usize)| {
        let status = agents.iter().fold(WorktreeAgentStatus::Idle, |all, (_, _, status)| {
            WorktreeAgentStatus::pick_higher(all, status.clone())
        });
        TabData {
            id,
            workspace_id: Some(id),
            label: label.into(),
            branch_subtitle: None,
            group: None,
            color: None,
            is_running: false,
            title: label.into(),
            cwd: cwd.into(),
            project_key: Some("/work/dragent".into()),
            project_label: Some("Dragent".into()),
            purpose_label: label.into(),
            is_work_item: true,
            ai_tool: None,
            last_command: None,
            agent_status: status,
            agents: agents
                .into_iter()
                .map(|(pane, tool, status)| SidebarAgent {
                    image: None,
                    pane_id: pane,
                    tool: Some(tool),
                    status: status.clone(),
                    base_status: status,
                    cli: true,
                    activity_known: true,
                })
                .collect(),
            diff_files: 0,
            diff_added: diff.0,
            diff_deleted: diff.1,
            pr: None,
            git_root: None,
            branch: Some(branch.into()),
            is_shared: false,
        }
    };
    let working = || WorktreeAgentStatus::Working { label: String::new() };
    let tabs = vec![
        row(1, "Add sign-in with GitHub", "/work/dragent", "main", vec![
            (lead, AiTool::OpenCode, working()),
            (Uuid::new_v4(), AiTool::Claude, WorktreeAgentStatus::Review),
            (Uuid::new_v4(), AiTool::Codex, WorktreeAgentStatus::Idle),
            (Uuid::new_v4(), AiTool::Gemini, WorktreeAgentStatus::Idle),
            (Uuid::new_v4(), AiTool::CursorAgent, WorktreeAgentStatus::Idle),
            (Uuid::new_v4(), AiTool::Copilot, WorktreeAgentStatus::Idle),
        ], (190, 68)),
        row(2, "auth-provider", "/work/wt/auth", "auth-provider-t1-3f2a", vec![(w1, AiTool::Codex, WorktreeAgentStatus::Review)], (214, 18)),
        row(3, "login-screen", "/work/wt/login", "login-screen-t2-3f2a", vec![(w2, AiTool::OpenCode, working()), (w3, AiTool::Claude, WorktreeAgentStatus::Permission)], (96, 4)),
    ];
    let task = |id: &str, title: &str, status: TaskStatus, workspace: &str, pane: Option<Uuid>, review: bool| GroupTask {
        id: id.into(),
        title: title.into(),
        status,
        review,
        workspace: Some(workspace.into()),
        worker_pane: pane.map(|pane| pane.to_string()),
    };
    let group = CoordinationGroup {
        id: "team-preview".into(),
        title: "Add sign-in with GitHub".into(),
        status: TeamStatus::Running,
        needs_you: 0,
        lead_pane: Some(lead.to_string()),
        tasks: vec![
            task("t1", "OAuth provider and token storage", TaskStatus::Done, "/work/wt/auth", Some(w1), false),
            task("t2", "Sign-in screen and session restore", TaskStatus::Running, "/work/wt/login", Some(w2), false),
            task("t3", "Review the sign-in screen", TaskStatus::Running, "/work/wt/login", Some(w3), true),
            task("t4", "QA the whole sign-in flow", TaskStatus::Pending, "/work/wt/qa", None, true),
        ],
    };
    let groups = assign(&[group], &tabs);
    let members: Vec<&TabData> = tabs.iter().collect();
    let tab_mice = RefCell::new(HashMap::new());
    let mice = RefCell::new(HashMap::new());
    let width = crate::theme::DEFAULT_LEFT_PANEL_WIDTH;
    let rendered = render_group(
        &groups[0],
        &CoordinationRender {
            ui_font,
            theme,
            workspace_id,
            tabs: &members,
            active_tab_id: Some(3),
            menu_tab_id: None,
            collapsed: false,
            panel_width: width,
            tab_mouse_states: &tab_mice,
            sidebar_mouse_states: &mice,
        },
    );
    // Project headings: one with its own picture, one with its letter.
    let heading = |key: &str, label: &str, icon: Option<std::path::PathBuf>| {
        super::panel_nav::render_project_group_header(
            workspace_id, ui_font, theme, key, label, 3, false, &mice, None, None, icon,
        )
    };
    let picture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/dragent/agent-images/ampcode.svg");
    // Ordinary workspaces wear the same card: one waits on the person.
    let plain = [
        row(7, "fix-retry-errors", "/work/wt/retry", "fix/retry-errors", vec![(Uuid::new_v4(), AiTool::Claude, WorktreeAgentStatus::Permission)], (42, 7)),
        row(8, "docs-refresh", "/work/wt/docs", "docs/refresh", vec![(Uuid::new_v4(), AiTool::OpenCode, WorktreeAgentStatus::Idle)], (0, 0)),
    ];
    let (menu, close) = (MouseStateHandle::default(), MouseStateHandle::default());
    let mut ordinary = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for tab in &plain {
        let agents = one_per_kind(&tab.agents);
        let agent_mice: Vec<MouseStateHandle> = agents.iter().map(|_| MouseStateHandle::default()).collect();
        let root = std::path::PathBuf::from(&tab.cwd);
        let branch = tab.branch.clone().unwrap_or_default();
        ordinary = ordinary.with_child(workspace_card(
            &CardFace {
                theme: *theme,
                ui: ui_font,
                workspace_id,
                tab_id: tab.id,
                navigation: tab.workspace_id,
                is_active: false,
                menu_open: false,
                hovered: false,
                accent: theme.accent,
                title: &tab.label,
                lead: false,
                status: &tab.agent_status,
                branch: &branch,
                diff: (tab.diff_added, tab.diff_deleted),
                agents: &agents,
                tasks: &[],
                workspace_root: &root,
                depth: 1,
            },
            &CardMice { menu: &menu, close: &close, agents: &agent_mice },
        ));
    }
    let rendered = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(heading("/work/amp", "amp-project", Some(picture)))
        .with_child(heading("/work/myapp", "myapp", None))
        .with_child(ordinary.finish())
        .with_child(rendered)
        .finish();
    Container::new(ConstrainedBox::new(rendered).with_width(width).finish())
        .with_background_color(theme.sidebar_background())
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tab(id: usize, cwd: &str, panes: &[Uuid]) -> TabData {
        TabData {
            id,
            workspace_id: Some(id),
            label: format!("workspace {id}"),
            branch_subtitle: None,
            group: None,
            color: None,
            is_running: false,
            title: String::new(),
            cwd: cwd.to_string(),
            project_key: Some("/work/app".into()),
            project_label: Some("app".into()),
            purpose_label: String::new(),
            is_work_item: true,
            ai_tool: None,
            last_command: None,
            agent_status: WorktreeAgentStatus::Idle,
            agents: panes
                .iter()
                .map(|pane| SidebarAgent {
                    image: None,
                    pane_id: *pane,
                    tool: None,
                    status: WorktreeAgentStatus::Idle,
                    base_status: WorktreeAgentStatus::Idle,
                    cli: true,
                    activity_known: false,
                })
                .collect(),
            diff_files: 0,
            diff_added: 0,
            diff_deleted: 0,
            pr: None,
            git_root: None,
            branch: None,
            is_shared: false,
        }
    }

    fn task(id: &str, status: TaskStatus, workspace: Option<&str>, pane: Option<Uuid>) -> GroupTask {
        GroupTask {
            id: id.into(),
            title: format!("Task {id}"),
            status,
            review: false,
            workspace: workspace.map(PathBuf::from),
            worker_pane: pane.map(|pane| pane.to_string()),
        }
    }

    #[test]
    fn a_coordinations_workspaces_join_its_group_lead_first() {
        let (lead, worker, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let tabs = vec![
            tab(1, "/work/app", &[lead]),
            tab(2, "/work/wt/t1/", &[worker]),
            tab(3, "/work/wt/t2", &[]),
            tab(4, "/work/unrelated", &[other]),
        ];
        let group = CoordinationGroup {
            id: "team-1".into(),
            title: "Add login".into(),
            status: TeamStatus::Running,
            needs_you: 0,
            lead_pane: Some(lead.to_string()),
            tasks: vec![
                task("t1", TaskStatus::Running, Some("/work/wt/t1"), Some(worker)),
                task("t2", TaskStatus::Pending, Some("/work/wt/t2"), None),
            ],
        };
        let groups = assign(&[group], &tabs);
        assert_eq!(groups.len(), 1);
        let members: Vec<(usize, &str)> =
            groups[0].members.iter().map(|m| (m.tab_id, m.badge.as_str())).collect();
        assert_eq!(members, vec![(1, "Lead"), (2, "t1"), (3, "t2")], "the unrelated workspace stays out");
        assert_eq!(groups[0].members[1].title, "Task t1", "a task's card says what it's for");
        assert_eq!(groups[0].members[0].title, "Lead · runs t1", "the lead's card says what it does");
        assert!(groups[0].members[0].at_work && !groups[0].members[1].at_work);
        assert_eq!(groups[0].members[1].tasks.len(), 1);
        assert_eq!(groups[0].project_key.as_deref(), Some("/work/app"));
        assert_eq!((groups[0].done, groups[0].total), (0, 2));
    }

    #[test]
    fn a_shared_checkout_shows_the_task_it_is_on_now() {
        let tabs = vec![tab(5, "/work/wt/lane", &[])];
        let group = CoordinationGroup {
            id: "team-2".into(),
            title: "Lane".into(),
            status: TeamStatus::Running,
            needs_you: 1,
            lead_pane: None,
            tasks: vec![
                task("t1", TaskStatus::Done, Some("/work/wt/lane"), None),
                task("t2", TaskStatus::Running, Some("/work/wt/lane"), None),
            ],
        };
        let groups = assign(&[group], &tabs);
        assert_eq!(groups[0].members[0].badge, "t2");
        let ids: Vec<&str> = groups[0].members[0].tasks.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(ids, vec!["t1", "t2"], "a shared checkout lists each task");
        assert_eq!(groups[0].members[0].title, "Task t2");
        assert_eq!(groups[0].state, CoordinationState::NeedsYou);
    }

    #[test]
    fn agents_of_one_kind_show_once_with_the_most_urgent_mark() {
        use crate::console::ai_toolbar::AiTool;
        let agent = |tool: AiTool, status: WorktreeAgentStatus| SidebarAgent {
            image: None,
            pane_id: Uuid::new_v4(),
            tool: Some(tool),
            status: status.clone(),
            base_status: status,
            cli: true,
            activity_known: true,
        };
        let working = WorktreeAgentStatus::Working { label: String::new() };
        let shown = one_per_kind(&[
            agent(AiTool::OpenCode, WorktreeAgentStatus::Idle),
            agent(AiTool::OpenCode, working.clone()),
            agent(AiTool::OpenCode, WorktreeAgentStatus::Idle),
            agent(AiTool::Codex, WorktreeAgentStatus::Review),
            agent(AiTool::Claude, WorktreeAgentStatus::Idle),
        ]);
        let kinds: Vec<_> = shown.iter().map(|agent| (agent.tool, agent.status.clone())).collect();
        assert_eq!(
            kinds,
            vec![
                (Some(AiTool::OpenCode), working),
                (Some(AiTool::Codex), WorktreeAgentStatus::Review),
                (Some(AiTool::Claude), WorktreeAgentStatus::Idle),
            ]
        );
    }

    #[test]
    fn a_workspace_joins_one_group_and_empty_groups_are_left_out() {
        let tabs = vec![tab(1, "/work/wt/a", &[])];
        let make = |id: &str| CoordinationGroup {
            id: id.into(),
            title: id.into(),
            status: TeamStatus::Done,
            needs_you: 0,
            lead_pane: None,
            tasks: vec![task("t1", TaskStatus::Done, Some("/work/wt/a"), None)],
        };
        let none = CoordinationGroup { tasks: Vec::new(), ..make("empty") };
        let groups = assign(&[make("first"), make("second"), none], &tabs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].team_id, "first");
    }
}

/// The whole sidebar as it looks now (nav, projects, workspace cards), with
/// the projects of the marketing site's recorded Codex session, so the clip
/// can show the current design over the one it was recorded with.
pub(crate) fn marketing_sidebar(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    search: &warpui::ViewHandle<crate::workspace::InputEditor>,
    size: (f32, f32),
) -> Box<dyn Element> {
    use crate::console::ai_toolbar::AiTool;
    let mice = RefCell::new(HashMap::new());
    let card = |id: usize, label: &str, branch: &str, agent: Option<(AiTool, WorktreeAgentStatus)>, diff: (usize, usize), active: bool| {
        let status = agent.as_ref().map(|(_, s)| s.clone()).unwrap_or_default();
        let agents: Vec<SidebarAgent> = agent
            .into_iter()
            .map(|(tool, status)| SidebarAgent {
                image: None,
                pane_id: Uuid::new_v4(),
                tool: Some(tool),
                status: status.clone(),
                base_status: status,
                cli: true,
                activity_known: true,
            })
            .collect();
        let agent_mice: Vec<MouseStateHandle> = agents.iter().map(|_| MouseStateHandle::default()).collect();
        let (menu, close) = (MouseStateHandle::default(), MouseStateHandle::default());
        workspace_card(
            &CardFace {
                theme: *theme,
                ui: ui_font,
                workspace_id,
                tab_id: id,
                navigation: Some(id),
                is_active: active,
                menu_open: false,
                hovered: false,
                accent: theme.accent,
                title: label,
                lead: false,
                status: &status,
                branch,
                diff,
                agents: &agents,
                tasks: &[],
                workspace_root: std::path::Path::new("/work"),
                depth: 1,
            },
            &CardMice { menu: &menu, close: &close, agents: &agent_mice },
        )
    };
    let heading = |key: &str, label: &str, count: usize, collapsed: bool| {
        super::panel_nav::render_project_group_header(
            workspace_id, ui_font, theme, key, label, count, collapsed, &mice, None, None, None,
        )
    };
    let working = WorktreeAgentStatus::Working { label: String::new() };
    let column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(super::panel_nav::render_sidebar_nav(
            ui_font,
            theme,
            workspace_id,
            &mice,
            0,
            0,
            super::panel_nav::SidebarNavActive::None,
        ))
        .with_child(super::panel_nav::render_projects_header(
            ui_font,
            theme,
            workspace_id,
            false,
            false,
            super::left_panel::SidebarSortMode::Manual,
            search,
            &mice,
        ))
        .with_child(heading("/work/acme-web", "Acme Web", 3, false))
        .with_child(card(1, "Review the product tour", "review/product-tour", Some((AiTool::Codex, working)), (0, 0), true))
        .with_child(card(2, "Team invitations", "feat/team-invites", Some((AiTool::Claude, WorktreeAgentStatus::Review)), (184, 22), false))
        .with_child(card(3, "Faster checkout", "fix/checkout", Some((AiTool::OpenCode, WorktreeAgentStatus::Idle)), (46, 11), false))
        .with_child(heading("/work/acme-api", "Acme API", 2, true))
        .with_child(heading("/work/acme-mobile", "Acme Mobile", 1, true))
        .finish();
    ConstrainedBox::new(
        Container::new(column)
            // Opaque: the clip's frame shows through nothing.
            .with_background_color({
                let c = theme.sidebar_background();
                ColorU::new(c.r, c.g, c.b, 255)
            })
            .with_padding_top(2.0)
            .finish(),
    )
    .with_width(size.0)
    .with_height(size.1)
    .finish()
}
