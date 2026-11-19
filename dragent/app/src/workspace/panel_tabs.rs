// Panel tab rows, menus, and helpers.
//!
//! Moved verbatim; same behavior.

//!  session sidebar — search, tab list, context menus, colors.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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
    AcceptedByDropTarget, DispatchEventResult, Draggable, DraggableState, EventHandler, Hoverable,
    MouseStateHandle,
};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_SM};
use crate::theme::Theme;

use super::left_panel::{
    sidebar_mouse, tab_drag, LeftPanelAction, ProjectData, SidebarDragPreview,
    SidebarDropDestination, SidebarSortMode, TabData, FONT_MENU_SECTION,
};
use super::panel_nav::{menu_item, menu_separator, render_color_strip};
use super::sidebar::{tab_color_options, SidebarFocus, SidebarViewMode};
use super::WorkspaceAction;

/// Compute a tab's accent bar / highlight color, mirroring PaneTab::accent_color.
pub(crate) fn tab_accent_color(tab: &TabData, is_active: bool, theme_accent: ColorU) -> ColorU {
    tab.color.unwrap_or(if is_active {
        theme_accent
    } else {
        ColorU::transparent_black()
    })
}

pub struct LeftPanelProps<'a> {
    pub ui_font: FamilyId,
    pub theme: &'a Theme,
    pub workspace_view_id: EntityId,
    pub tabs: &'a [TabData],
    pub projects: &'a [ProjectData],
    pub hidden_projects: &'a [ProjectData],
    pub active_tab_id: Option<usize>,
    pub search: &'a str,
    pub view_mode: SidebarViewMode,
    pub menu_tab_id: Option<usize>,
    pub sidebar_focus: SidebarFocus,
    pub search_editor: &'a ViewHandle<crate::workspace::InputEditor>,
    pub rename_editor: &'a ViewHandle<crate::workspace::InputEditor>,
    pub tab_mouse_states: &'a RefCell<HashMap<usize, MouseStateHandle>>,
    pub tab_drag_states: &'a RefCell<HashMap<usize, DraggableState>>,
    pub sidebar_mouse_states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    /// Automation failures the user hasn't seen (the nav badge).
    pub automation_failures: usize,
    /// Messages teams' leads left for the person (the Teams badge).
    pub teams_needing_you: usize,
    /// Which nav screen owns the center, if any — its row stays highlighted.
    pub nav_active: super::panel_nav::SidebarNavActive,
    pub collapsed_project_groups: &'a HashSet<String>,
    pub collapsed_workspace_groups: &'a HashSet<(String, String)>,
    pub coordination: &'a [super::panel_coordination::SidebarCoordination],
    pub collapsed_coordination: &'a HashSet<String>,
    pub drag_preview: Option<&'a super::left_panel::SidebarDragPreview>,
    pub drag_target_state: &'a Arc<Mutex<Option<SidebarDragPreview>>>,
    pub project_drag_states: &'a RefCell<HashMap<String, DraggableState>>,
    pub project_drag_preview: Option<&'a super::left_panel::ProjectDragPreview>,
    pub projects_collapsed: bool,
    pub hidden_projects_collapsed: bool,
    pub projects_search_expanded: bool,
    pub sort_mode: SidebarSortMode,
    pub scroll_state: ClippedScrollStateHandle,
    /// Fallback panel width (from the resizable state) used until the first
    /// real measurement lands in the position cache.
    pub panel_width: f32,
    /// Host window id — lets render read last-frame element bounds so rows
    /// size against the REAL rendered width, not a stale resizable value.
    pub window_id: warpui_core::WindowId,
    /// Bottom-left organisation area, pinned under the list.
    pub footer: Box<dyn Element>,
}

/// Truncate `s` with an ellipsis so it fits roughly `avail_px` at `font_px`.
/// Uses an average glyph width of ~0.55 × font size (sans-serif heuristic).
/// NOTE: tab rows no longer pre-trim with this — they rely on the framework's
/// pixel-exact ellipsis clip. Kept for potential non-critical labels + tests.
#[allow(dead_code)]
pub(crate) fn truncate_px(s: &str, avail_px: f32, font_px: f32) -> String {
    let char_w = (font_px * 0.55).max(1.0);
    let budget = ((avail_px / char_w).floor() as usize).max(4);
    if s.chars().count() <= budget {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(budget.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

pub(crate) fn tab_matches_search(tab: &TabData, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    tab.label.to_lowercase().contains(search)
        || tab.purpose_label.to_lowercase().contains(search)
        || tab.title.to_lowercase().contains(search)
        || tab.cwd.to_lowercase().contains(search)
}

pub(crate) fn sort_group_tabs(tabs: &mut Vec<&TabData>, sort_mode: SidebarSortMode) {
    if sort_mode == SidebarSortMode::Name {
        tabs.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    }
    // Workspaces whose agents are at work rise to the top of their project
    // (and folder); the rest keep their order. The sort is stable, so the
    // working ones keep theirs too, and a workspace goes back to its place
    // when its agents stop.
    tabs.sort_by_key(|tab| !is_at_work(tab));
}

/// An agent in the workspace is working, or stopped mid-task for the
/// person's permission.
pub(crate) fn is_at_work(tab: &TabData) -> bool {
    use super::worktree_status::WorktreeAgentStatus::{Permission, Working};
    matches!(tab.agent_status, Working { .. } | Permission)
        || tab.agents.iter().any(|agent| matches!(agent.status, Working { .. } | Permission))
}

pub(crate) fn project_group_is_collapsed(
    project_key: Option<&str>,
    search: &str,
    collapsed_project_groups: &HashSet<String>,
) -> bool {
    search.is_empty() && project_key.is_some_and(|key| collapsed_project_groups.contains(key))
}

/// Compact status dot; the workspace icon tile owns the working animation.
pub(crate) fn worktree_activity_indicator(
    status: &super::worktree_status::WorktreeAgentStatus,
    theme: &Theme,
) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(status.color(theme))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish()
}

/// Checkout-scoped details; rendering never runs Git or host I/O.
/// Compact: the title with its status, the branch with its changes, the
/// agents, and the pull request when there is one, one line each.
pub(crate) fn worktree_hover_card(
    tab: &TabData,
    theme: &Theme,
    font: FamilyId,
    workspace_id: EntityId,
    branch_mouse: MouseStateHandle,
) -> Box<dyn Element> {
    use super::worktree_status::{SidebarPrChecks, WorktreeAgentStatus};
    let line = |value: String, size: f32, color: ColorU| -> Box<dyn Element> {
        Text::new(value, font, crate::theme::text_size(size))
            .with_color(color)
            .soft_wrap(false)
            .with_clip(ClipConfig::ellipsis())
            .finish()
    };
    // The card exists to show what the row cuts short: these wrap.
    let wrap = |value: String, size: f32, color: ColorU| -> Box<dyn Element> {
        Text::new(value, font, crate::theme::text_size(size))
            .with_color(color)
            .soft_wrap(true)
            .finish()
    };
    let dot = |color: ColorU| -> Box<dyn Element> {
        Container::new(ConstrainedBox::new(Empty::new().finish()).with_width(6.0).with_height(6.0).finish())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish()
    };
    let (status, status_color) = match &tab.agent_status {
        WorktreeAgentStatus::Idle => (
            "Idle",
            theme.sub_text(),
        ),
        WorktreeAgentStatus::Working { .. } => ("Working", super::worktree_status::WORKING_AMBER),
        WorktreeAgentStatus::Permission => ("Needs input", super::worktree_status::needs_input_color(theme)),
        WorktreeAgentStatus::Failed => ("Failed", theme.ui_red()),
        WorktreeAgentStatus::Review => ("Review", theme.ui_green()),
    };
    let status_pill = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(dot(status_color))
            .with_child(line(status.into(), 10.0, theme.sub_text()))
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(7.0)
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .with_background_color(theme.surface_1())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish();
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(8.0)
                .with_child(Expanded::new(1.0, wrap(tab.label.clone(), 12.5, theme.main_text())).finish())
                .with_child(status_pill)
                .finish(),
        );

    // Branch (click to rename) and its changes.
    let clean = tab.diff_files == 0 && tab.diff_added == 0 && tab.diff_deleted == 0;
    let changes: Box<dyn Element> = if clean {
        Empty::new().finish()
    } else {
        Flex::row()
            .with_spacing(4.0)
            .with_child(line(format!("+{}", tab.diff_added), 10.5, theme.ui_green()))
            .with_child(line(format!("−{}", tab.diff_deleted), 10.5, theme.ui_red()))
            .finish()
    };
    let branch_label = tab.branch.clone().unwrap_or_else(|| "No branch".into());
    let root = tab.git_root.clone();
    let has_branch = tab.branch.is_some();
    let branch = Hoverable::new(branch_mouse, move |state| {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(5.0)
            .with_child(Container::new(render_icon(AppIcon::GitBranch, theme.disabled_text(), 11.0)).with_padding_top(2.0).finish())
            .with_child(Expanded::new(1.0, wrap(branch_label.clone(), 11.0, theme.sub_text())).finish());
        if has_branch && state.is_hovered() {
            row = row.with_child(render_icon(AppIcon::Edit, theme.sub_text(), 11.0));
        }
        Container::new(row.finish())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_background_color(if has_branch && state.is_hovered() {
                theme.accent_overlay()
            } else {
                ColorU::transparent_black()
            })
            .finish()
    })
    .on_click({
        let branch = tab.branch.clone().unwrap_or_default();
        move |ctx, _, _| {
            if let Some(root) = root.as_ref().filter(|_| !branch.is_empty()) {
                ctx.dispatch_typed_action_for(
                    workspace_id,
                    WorkspaceAction::RenameWorktreeBranch { root: root.clone(), branch: branch.clone() },
                );
            }
        }
    })
    .with_cursor(Cursor::PointingHand)
    .finish();
    content = content.with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.0)
            .with_child(Expanded::new(1.0, branch).finish())
            .with_child(changes)
            .finish(),
    );

    if let Some(root) = &tab.git_root {
        if let Some(remote) = crate::remote_fs::resolve(root) {
            let machine = crate::remote_fs::machine_name(root).unwrap_or(remote.machine_id);
            content = content.with_child(wrap(format!("On {machine}"), 10.5, theme.sub_text()));
        }
    }

    // Agents: a small overlapping pile and a count.
    if !tab.agents.is_empty() {
        const VISIBLE: usize = 4;
        const SIDE: f32 = 18.0;
        const STEP: f32 = 13.0;
        let count = tab.agents.len().min(VISIBLE);
        let mut pile = Stack::new().with_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(SIDE + (count - 1) as f32 * STEP)
                .with_height(SIDE)
                .finish(),
        );
        for (index, agent) in tab.agents.iter().take(VISIBLE).enumerate() {
            use warpui_core::assets::asset_cache::AssetSource;
            use warpui_core::image_cache::CacheOption;
            let source = match &agent.image {
                Some(super::AgentImage::Bundled(path)) => Some(AssetSource::Bundled { path }),
                Some(super::AgentImage::File(path)) => Some(AssetSource::LocalFile { path: path.clone() }),
                None => None,
            };
            let artwork = match source {
                Some(source) => ConstrainedBox::new(
                    warpui::elements::Image::new(source, CacheOption::BySize).contain().finish(),
                )
                .with_width(11.0)
                .with_height(11.0)
                .finish(),
                None => match agent.tool {
                    Some(tool) => render_icon(crate::console::ai_toolbar::brand_icon(tool), theme.main_text(), 10.0),
                    None => render_icon(AppIcon::Agent, theme.sub_text(), 10.0),
                },
            };
            let avatar = Container::new(
                ConstrainedBox::new(Align::new(artwork).finish())
                    .with_width(SIDE - 2.0)
                    .with_height(SIDE - 2.0)
                    .finish(),
            )
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.surface_2()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(SIDE / 2.0)))
            .finish();
            pile = pile.with_positioned_child(
                avatar,
                OffsetPositioning::offset_from_parent(
                    vec2f(index as f32 * STEP, 0.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        let working = tab
            .agents
            .iter()
            .filter(|agent| matches!(agent.status, WorktreeAgentStatus::Working { .. }))
            .count();
        let sessions = tab.agents.len();
        let label = match (working, sessions) {
            (0, 1) => "1 agent".to_string(),
            (0, n) => format!("{n} agents"),
            (w, n) if w == n => format!("{w} working"),
            (w, n) => format!("{w} working · {n} agents"),
        };
        content = content.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(pile.finish())
                .with_child(Expanded::new(1.0, line(label, 10.5, theme.sub_text())).finish())
                .finish(),
        );
    }

    // The pull request on one line, its checks as a dot.
    if let Some(pr) = &tab.pr {
        let checks = match pr.checks {
            SidebarPrChecks::Passing => theme.ui_green(),
            SidebarPrChecks::Failing => theme.ui_red(),
            SidebarPrChecks::Pending => super::worktree_status::WORKING_AMBER,
            SidebarPrChecks::None => theme.disabled_text(),
        };
        let draft = if pr.is_draft { " · draft" } else { "" };
        content = content.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(5.0)
                .with_child(Container::new(render_icon(AppIcon::PullRequest, theme.sub_text(), 11.0)).with_padding_top(2.0).finish())
                .with_child(
                    Expanded::new(
                        1.0,
                        wrap(format!("#{} {}{draft}", pr.number, pr.title), 11.0, theme.main_text()),
                    )
                    .finish(),
                )
                .with_child(Container::new(dot(checks)).with_padding_top(5.0).finish())
                .finish(),
        );
    }
    let card = Container::new(
        ConstrainedBox::new(content.finish())
            .with_width(crate::theme::scaled(236.0))
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(10.0)
    .with_padding_bottom(10.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .finish();
    EventHandler::new(card)
        .on_mouse_in(|_, _, _| DispatchEventResult::StopPropagation, None)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .on_left_mouse_up(|_, _, _| DispatchEventResult::StopPropagation)
        .finish()
}

pub(crate) fn render_tab_row(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    tab: &TabData,
    is_active: bool,
    is_renaming: bool,
    menu_open: bool,
    accent: ColorU,
    view_mode: SidebarViewMode,
    rename_editor: &ViewHandle<crate::workspace::InputEditor>,
    mouse_state: MouseStateHandle,
    drag_state: DraggableState,
    drag_target_state: Arc<Mutex<Option<SidebarDragPreview>>>,
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    depth: usize,
) -> Box<dyn Element> {
    // Soft-card metrics: full-bleed rows, no dividers. Text is NOT pre-trimmed
    // by character estimates — the title/subtitle receive the remaining flex
    // width and the framework's ellipsis clip cuts only when text overflows.
    let tab_id = tab.id;
    let navigation_workspace_id = tab.workspace_id;
    let project_key = tab.project_key.clone();
    let accepted_project_key = project_key.clone();
    let preview_project_key = project_key.clone();
    let preview_target_state = drag_target_state.clone();
    let label = tab.label.clone();
    let branch_subtitle = if navigation_workspace_id.is_some() {
        None
    } else {
        tab.branch_subtitle.clone()
    };
    let running = tab.is_running && navigation_workspace_id.is_none();
    let is_shared = tab.is_shared;
    let v_pad = if view_mode == SidebarViewMode::Compact {
        5.0
    } else {
        7.0
    };
    let text_color = theme.main_text();
    // Only this workspace row receives its custom selection color.
    let mut selection_theme = *theme;
    selection_theme.accent = accent;

    // Resolve the card's content once; the Hoverable closure below rebuilds
    // the subtree on hover so the row actions can reveal themselves.
    let (tile_icon, tile_icon_color, title_text) = if navigation_workspace_id.is_some() {
        (AppIcon::GitBranch, None, label.clone())
    } else if let Some(ai_tool) = &tab.ai_tool {
        let (icon, color) = match ai_tool {
            crate::console::ai_toolbar::AiTool::Claude => {
                (AppIcon::ClaudeLogo, ColorU::new(204, 120, 92, 255))
            }
            other => (crate::console::ai_toolbar::brand_icon(*other), text_color),
        };
        (icon, Some(color), label.clone())
    } else {
        let is_settings_tab = tab.last_command.as_deref() == Some("settings");
        let (icon, purpose) = if tab.is_work_item {
            (AppIcon::Agent, label.clone())
        } else if is_settings_tab {
            (AppIcon::Settings, "Settings".to_string())
        } else {
            let purpose = tab.purpose_label.clone();
            let icon = match purpose.as_str() {
                "Agent" => AppIcon::Agent,
                "Terminal" | "SSH" => AppIcon::Terminal,
                "GitHub" => AppIcon::GitBranch,
                _ => AppIcon::Code,
            };
            (icon, purpose)
        };
        (icon, None, purpose)
    };

    let menu_key = format!("menu_{tab_id}");
    let close_key = format!("close_{tab_id}");
    // A workspace shows as the same card a coordination's workspaces use.
    let card_agents = super::panel_coordination::one_per_kind(&tab.agents);
    let card_agent_mice: Vec<MouseStateHandle> = card_agents
        .iter()
        .map(|agent| sidebar_mouse(sidebar_mouse_states, &format!("coordination_agent:{}", agent.pane_id)))
        .collect();
    let card_menu_mouse = sidebar_mouse(sidebar_mouse_states, &menu_key);
    let card_close_mouse = sidebar_mouse(sidebar_mouse_states, &close_key);
    let card_branch = tab.branch.clone().unwrap_or_default();
    let card_root = std::path::PathBuf::from(&tab.cwd);

    let row_click = Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        if navigation_workspace_id.is_some() && !is_renaming {
            let card = super::panel_coordination::workspace_card(
                &super::panel_coordination::CardFace {
                    theme: *theme,
                    ui: ui_font,
                    workspace_id,
                    tab_id,
                    navigation: navigation_workspace_id,
                    is_active,
                    menu_open,
                    hovered,
                    accent,
                    title: &label,
                    lead: false,
                    status: &tab.agent_status,
                    branch: &card_branch,
                    diff: (tab.diff_added, tab.diff_deleted),
                    agents: &card_agents,
                    tasks: &[],
                    workspace_root: &card_root,
                    depth,
                },
                &super::panel_coordination::CardMice {
                    menu: &card_menu_mouse,
                    close: &card_close_mouse,
                    agents: &card_agent_mice,
                },
            );
            return if hovered && !menu_open {
                Stack::new()
                    .with_child(card)
                    .with_positioned_overlay_child(
                        worktree_hover_card(tab, theme, ui_font, workspace_id, sidebar_mouse(sidebar_mouse_states, &format!("hover_branch_{tab_id}"))),
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
            };
        }
        let bg = if is_active {
            selection_theme.accent_overlay()
        } else if hovered {
            theme.sidebar_row_hover()
        } else {
            ColorU::transparent_black()
        };

        // Accent edge flush with the card's left side.
        let accent_pill = Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(3.0)
                .with_height(24.0)
                .finish(),
        )
        .with_background_color(if is_active {
            accent
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
        // The icon tile starts under its project's or folder's icon.
        .with_margin_right(super::panel_nav::tree_lead(depth) - 3.0)
        .finish();

        // Icon tile gives the card its visual anchor.
        let tile_bg = if is_active {
            selection_theme.accent_bg()
        } else {
            theme.surface_2()
        };
        let tile_icon_tint = tile_icon_color.unwrap_or(if is_active { accent } else { text_color });
        let icon_tile = if navigation_workspace_id.is_some() {
            super::workspace_activity::workspace_icon_tile(
                tile_icon,
                tile_icon_tint,
                tile_bg,
                &tab.agent_status,
                theme.accent,
                theme,
            )
        } else {
            Container::new(
                ConstrainedBox::new(
                    Align::new(render_icon(tile_icon, tile_icon_tint, super::workspace_activity::TILE_ICON)).finish(),
                )
                .with_width(super::workspace_activity::TILE_SIDE)
                .with_height(super::workspace_activity::TILE_SIDE)
                .finish(),
            )
            .with_background_color(tile_bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        };

        // Title line: directory name, then the share glyph and the running
        // indicator (glow ring + dot). The title gets the flexible portion of
        // this row, while trailing status icons retain their fixed width.
        let title_text_el = Expanded::new(
            1.0,
            Text::new(title_text.clone(), ui_font, crate::theme::text_size(12.5))
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .with_color(text_color)
                .finish(),
        )
        .finish();
        let mut title_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(6.0)
            .with_child(title_text_el);
        if is_shared && !is_renaming {
            title_row = title_row.with_child(
                Container::new(render_icon(AppIcon::Share, theme.ui_green(), 11.0)).finish(),
            );
        }
        if running && !is_renaming {
            let glow = theme.ui_green();
            title_row = title_row.with_child(
                Container::new(
                    Container::new(
                        ConstrainedBox::new(Empty::new().finish())
                            .with_width(5.0)
                            .with_height(5.0)
                            .finish(),
                    )
                    .with_background_color(glow)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish(),
                )
                .with_background_color(ColorU::new(glow.r, glow.g, glow.b, 40))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(2.5)
                .with_padding_right(2.5)
                .with_padding_top(2.5)
                .with_padding_bottom(2.5)
                .finish(),
            );
        }

        let mut text_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(1.0)
            .with_child(title_row.finish());
        if let Some(branch) = &branch_subtitle {
            text_col = text_col.with_child(
                Text::new(branch.clone(), ui_font, crate::theme::text_size(10.5))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );
        }
        // The text column owns all remaining row width. The title is
        // single-line and clipped, so long labels end in an ellipsis
        // instead of increasing the row height or pushing actions out of view.
        let text_col = Expanded::new(1.0, text_col.finish()).finish();

        // Status and close share the same fixed trailing slot. Details belong
        // in the hover card, so they never squeeze the workspace title.
        // Actions reveal on hover (or while the tab menu is open); an
        // invisible spacer keeps the layout from shifting.
        let actions: Box<dyn Element> = if hovered || menu_open {
            let overflow_btn =
                Hoverable::new(sidebar_mouse(sidebar_mouse_states, &menu_key), |_| {
                    Container::new(render_icon(
                        AppIcon::DotsVertical,
                        if menu_open {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        },
                        ICON_SIZE_SM,
                    ))
                    .with_padding_left(4.0)
                    .with_padding_right(2.0)
                    .with_padding_top(1.0)
                    .with_padding_bottom(1.0)
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_id,
                        WorkspaceAction::ToggleTabMenu { tab_id },
                    );
                })
                .finish();
            let overflow_btn =
                SavePosition::new(overflow_btn, &format!("tab_menu_btn_{tab_id}")).finish();

            let close_btn = Hoverable::new(sidebar_mouse(sidebar_mouse_states, &close_key), |_| {
                Container::new(render_icon(AppIcon::XClose, theme.sub_text(), 11.0))
                    .with_padding_left(2.0)
                    .with_padding_right(4.0)
                    .with_padding_top(1.0)
                    .with_padding_bottom(1.0)
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| match navigation_workspace_id {
                Some(project_workspace_id) => {
                    event_ctx.dispatch_typed_action_for(
                        workspace_id,
                        WorkspaceAction::CloseProjectWorkspace {
                            workspace_id: project_workspace_id,
                        },
                    );
                }
                None => {
                    event_ctx.dispatch_typed_action_for(
                        workspace_id,
                        WorkspaceAction::CloseTab { tab_id },
                    );
                }
            })
            .finish();

            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        ConstrainedBox::new(Align::new(overflow_btn).finish())
                            .with_width(20.0)
                            .with_height(22.0)
                            .finish(),
                    )
                    .with_child(
                        ConstrainedBox::new(Align::new(close_btn).finish())
                            .with_width(20.0)
                            .with_height(22.0)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_right(8.0)
            .finish()
        } else {
            // A workspace's status is the mark on its icon tile; other rows
            // keep their dot here.
            let indicator = if navigation_workspace_id.is_some() {
                Empty::new().finish()
            } else {
                worktree_activity_indicator(&tab.agent_status, theme)
            };
            Container::new(
                ConstrainedBox::new(
                    Align::new(indicator)
                    .finish(),
                )
                .with_width(20.0)
                .with_height(22.0)
                .finish(),
            )
            .with_padding_left(20.0)
            .with_padding_right(8.0)
            .finish()
        };

        let middle: Box<dyn Element> = if is_renaming {
            Expanded::new(
                1.0,
                crate::shared_input::shared_input(
                    theme,
                    rename_editor,
                    crate::shared_input::SharedInputOpts {
                        ..Default::default()
                    },
                ),
            )
            .finish()
        } else {
            Expanded::new(
                1.0,
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(icon_tile)
                        .with_child(text_col)
                        .finish(),
                )
                .finish(),
            )
            .finish()
        };

        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_child(accent_pill)
            .with_child(middle)
            .with_child(actions);

        // Full-bleed card: edge to edge, no horizontal padding, no radius.
        let row = Container::new(row.finish())
            .with_background_color(bg)
            .with_margin_top(1.0)
            .with_margin_bottom(2.0)
            .with_padding_top(v_pad)
            .with_padding_bottom(v_pad)
            .finish();
        if hovered && !is_renaming && !menu_open && navigation_workspace_id.is_some() {
            Stack::new()
                .with_child(row)
                .with_positioned_overlay_child(
                    worktree_hover_card(tab, theme, ui_font, workspace_id, sidebar_mouse(sidebar_mouse_states, &format!("hover_branch_{tab_id}"))),
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopRight,
                        ChildAnchor::TopLeft,
                    ),
                )
                .finish()
        } else {
            row
        }
    })
    .with_defer_events_to_children()
    .on_hover({
        let root = tab.git_root.clone();
        move |hovered, ctx, _, _| {
            if hovered {
                if let Some(root) = &root {
                    ctx.dispatch_typed_action_for(workspace_id, WorkspaceAction::RefreshWorktreeDetails { root: root.clone() });
                }
            }
        }
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        if !is_renaming {
            event_ctx.dispatch_typed_action_for(
                workspace_id,
                match navigation_workspace_id {
                    Some(workspace_id) => WorkspaceAction::SelectProjectWorkspace { workspace_id },
                    None => WorkspaceAction::SelectTab { tab_id },
                },
            );
        }
    })
    .finish();

    let row_click = EventHandler::new(row_click)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();

    // Drag the rendered row itself so its title, subtitle, actions and measured
    // sidebar width remain intact. Keep the source row in place while dragging.
    // Resolve the destination at the pointer, not from the row's large bounds.
    let row_draggable = Draggable::new(drag_state, row_click)
        .with_keep_original_visible(true)
        .with_drop_target_at_pointer()
        .with_accepted_by_drop_target_fn(move |data, _app| {
            let same_project_group = data.as_any()
                .downcast_ref::<super::panel_nav::SidebarGroupDropTarget>()
                .is_some_and(|target| accepted_project_key.as_deref() == Some(target.project_key.as_str()));
            let same_project_tab = data.as_any()
                .downcast_ref::<super::panel_nav::SidebarTabDropTarget>()
                .is_some_and(|target| accepted_project_key.as_deref() == Some(target.project_key.as_str()));
            if same_project_group || same_project_tab {
                AcceptedByDropTarget::Yes
            } else {
                AcceptedByDropTarget::No
            }
        })
        .on_drag(move |event_ctx, _app, _rect, data| {
            let destination = data.and_then(|data| {
                if let Some(target) = data.as_any().downcast_ref::<super::panel_nav::SidebarTabDropTarget>() {
                    if target.tab_id == tab_id {
                        // The previewed row can move under the pointer. Keep
                        // its last destination until another row is crossed.
                        return Some(SidebarDropDestination::Tab {
                            tab_id,
                            group: target.group.clone(),
                        });
                    }
                    return Some(SidebarDropDestination::Tab {
                        tab_id: target.tab_id,
                        group: target.group.clone(),
                    });
                }
                data.as_any().downcast_ref::<super::panel_nav::SidebarGroupDropTarget>()
                    .map(|target| SidebarDropDestination::Group(target.group.clone()))
            });
            if matches!(destination, Some(SidebarDropDestination::Tab { tab_id: target_id, .. }) if target_id == tab_id) {
                return;
            }
            let preview = preview_project_key.as_ref().and_then(|project_key| destination.map(|destination| SidebarDragPreview {
                source_tab_id: tab_id,
                project_key: project_key.clone(),
                destination,
            }));
            let changed = if let Ok(mut current) = preview_target_state.lock() {
                if *current == preview {
                    false
                } else {
                    *current = preview.clone();
                    true
                }
            } else {
                false
            };
            if changed {
                event_ctx.dispatch_typed_action(LeftPanelAction::PreviewWorkspaceDrop(preview));
            }
        })
        .on_drop(move |event_ctx, _app, _rect, data| {
            let destination = data.and_then(|data| {
                if let Some(target) = data.as_any().downcast_ref::<super::panel_nav::SidebarTabDropTarget>() {
                    return Some(SidebarDropDestination::Tab { tab_id: target.tab_id, group: target.group.clone() });
                }
                data.as_any().downcast_ref::<super::panel_nav::SidebarGroupDropTarget>()
                    .map(|target| SidebarDropDestination::Group(target.group.clone()))
            });
            if let Some(project_key) = &project_key {
                event_ctx.dispatch_typed_action(LeftPanelAction::CommitWorkspaceDrop {
                    source_tab_id: tab_id,
                    project_key: project_key.clone(),
                    destination,
                });
            }
        })
        .finish();

    row_draggable
}

pub(crate) fn render_tab_menu(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_id: EntityId,
    tab: &TabData,
    groups: &[String],
    sidebar_mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let tab_id = tab.id;
    let mut menu = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    // Host of THIS pane: "View link" + "Stop session"; everyone else: "Share session".
    let share_items: Vec<(&str, AppIcon, bool, WorkspaceAction)> = if tab.workspace_id.is_some() {
        vec![]
    } else if tab.is_shared {
        vec![
            (
                "View link",
                AppIcon::Share,
                false,
                WorkspaceAction::OpenShareSheet,
            ),
            (
                "Stop session",
                AppIcon::XClose,
                true,
                WorkspaceAction::RequestStopSharing,
            ),
        ]
    } else {
        vec![(
            "Share session",
            AppIcon::Share,
            true,
            WorkspaceAction::ShareSession { tab_id },
        )]
    };
    let rest: Vec<(&str, AppIcon, bool, WorkspaceAction)> =
        if let Some(workspace_id) = tab.workspace_id {
            vec![
                (
                    "Rename workspace",
                    AppIcon::Edit,
                    false,
                    WorkspaceAction::StartRenameTab { tab_id },
                ),
                (
                    "Sleep workspace",
                    AppIcon::Asset("icons/moon.svg"),
                    false,
                    WorkspaceAction::SleepProjectWorkspace { workspace_id },
                ),
                (
                    "Close workspace",
                    AppIcon::XClose,
                    true,
                    WorkspaceAction::CloseProjectWorkspace { workspace_id },
                ),
            ]
        } else {
            vec![
                (
                    "Copy pane title",
                    AppIcon::Copy,
                    false,
                    WorkspaceAction::CopyTabTitle { tab_id },
                ),
                (
                    "Copy working dir",
                    AppIcon::Copy,
                    false,
                    WorkspaceAction::CopyTabCwd { tab_id },
                ),
                (
                    "Rename tab",
                    AppIcon::Edit,
                    false,
                    WorkspaceAction::StartRenameTab { tab_id },
                ),
                (
                    "Move tab up",
                    AppIcon::ArrowUp,
                    false,
                    WorkspaceAction::MoveTabUp { tab_id },
                ),
                (
                    "Move tab down",
                    AppIcon::ArrowDown,
                    false,
                    WorkspaceAction::MoveTabDown { tab_id },
                ),
                (
                    "Close tab",
                    AppIcon::XClose,
                    true,
                    WorkspaceAction::CloseTab { tab_id },
                ),
                (
                    "Close other tabs",
                    AppIcon::XClose,
                    true,
                    WorkspaceAction::CloseOtherTabs { tab_id },
                ),
            ]
        };

    let sep_after_share = share_items.len();
    let mut items: Vec<(&str, AppIcon, bool, WorkspaceAction)> = share_items;
    items.extend(rest);

    // Separators after share block, after rename, and before close actions.
    let rename_idx = sep_after_share + 2; // copy title, copy cwd, then rename
    let close_idx = items.len() - 2;

    for (i, (label, icon, danger, action)) in items.iter().enumerate() {
        if i == sep_after_share || i == rename_idx || i == close_idx {
            menu = menu.with_child(menu_separator(theme));
        }
        let item_key = format!("mi_{tab_id}_{i}");
        let mouse_state = sidebar_mouse(sidebar_mouse_states, item_key);
        menu = menu.with_child(menu_item(
            ui_font,
            theme,
            workspace_id,
            label,
            *icon,
            *danger,
            action.clone(),
            mouse_state,
        ));
    }

    if let Some(project_key) = tab.project_key.as_ref().filter(|_| !groups.is_empty()) {
        menu = menu.with_child(menu_separator(theme));
        menu = menu.with_child(
            Container::new(
                Text::new("MOVE TO GROUP".to_string(), ui_font, crate::theme::text_size(FONT_MENU_SECTION))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_left(9.0)
            .with_padding_top(5.0)
            .with_padding_bottom(4.0)
            .finish(),
        );
        menu = menu.with_child(menu_item(
            ui_font,
            theme,
            workspace_id,
            "No group",
            AppIcon::Folder,
            false,
            WorkspaceAction::MoveWorkspaceToGroup {
                tab_id,
                project_root: project_key.as_str().into(),
                group: None,
            },
            sidebar_mouse(sidebar_mouse_states, format!("mi_{tab_id}_ungroup")),
        ));
        for (index, group) in groups.iter().enumerate() {
            menu = menu.with_child(menu_item(
                ui_font,
                theme,
                workspace_id,
                group,
                AppIcon::Folder,
                false,
                WorkspaceAction::MoveWorkspaceToGroup {
                    tab_id,
                    project_root: project_key.as_str().into(),
                    group: Some(group.clone()),
                },
                sidebar_mouse(sidebar_mouse_states, format!("mi_{tab_id}_group_{index}")),
            ));
        }
    }
    menu = menu.with_child(menu_separator(theme));
    menu = menu.with_child(
        Container::new(Box::new(
            Text::new("COLOR".to_string(), ui_font, crate::theme::text_size(FONT_MENU_SECTION)).with_color(theme.sub_text()),
        ))
        .with_padding_left(9.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish(),
    );
    menu = menu.with_child(render_color_strip(
        theme,
        workspace_id,
        tab,
        sidebar_mouse_states,
    ));

    Container::new(
        Container::new(menu.finish())
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .with_padding_left(3.0)
            .with_padding_right(3.0)
            .finish(),
    )
    .with_background_color(theme.sidebar_menu_background())
    .with_border(Border::all(1.0).with_border_color(theme.border()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .with_margin_left(8.0)
    .with_margin_right(8.0)
    .with_margin_bottom(2.0)
    .finish()
}

/// The hover card with sample data, for the screen previews.
pub(crate) fn hover_card_preview(font: FamilyId, theme: &Theme, workspace_id: EntityId) -> Box<dyn Element> {
    use crate::console::ai_toolbar::AiTool;
    use super::worktree_status::{SidebarAgent, SidebarPrBadge, SidebarPrChecks, WorktreeAgentStatus};
    let agent = |tool: AiTool, status: WorktreeAgentStatus| SidebarAgent {
        image: None,
        pane_id: uuid::Uuid::new_v4(),
        tool: Some(tool),
        status: status.clone(),
        base_status: status,
        cli: true,
        activity_known: true,
    };
    let working = WorktreeAgentStatus::Working { label: String::new() };
    let tab = TabData {
        id: 1,
        workspace_id: Some(1),
        label: "Sign-in screen and session restore, with the GitHub OAuth callback and token refresh".into(),
        branch_subtitle: None,
        group: None,
        color: None,
        is_running: false,
        title: String::new(),
        cwd: "/work/wt/login".into(),
        project_key: Some("/work/dragent".into()),
        project_label: Some("Dragent".into()),
        purpose_label: String::new(),
        is_work_item: true,
        ai_tool: None,
        last_command: None,
        agent_status: working.clone(),
        agents: vec![agent(AiTool::OpenCode, working), agent(AiTool::Claude, WorktreeAgentStatus::Review)],
        diff_files: 7,
        diff_added: 189,
        diff_deleted: 687,
        pr: Some(SidebarPrBadge {
            number: 214,
            title: "Sign in with GitHub".into(),
            state: "open".into(),
            is_draft: false,
            checks: SidebarPrChecks::Passing,
        }),
        git_root: Some("/work/wt/login".into()),
        branch: Some("sign-in-screen-and-session-restore-t2-3f2a".into()),
        is_shared: false,
    };
    Container::new(worktree_hover_card(&tab, theme, font, workspace_id, MouseStateHandle::default()))
        .with_padding_left(16.0)
        .with_padding_top(16.0)
        .finish()
}
