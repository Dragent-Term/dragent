//! How the Agent Coordination page looks. A split view: the coordinations
//! down a list on the left (a state mark, the title over a quiet line of
//! detail, progress and age at the end), the chosen one on the right with
//! its tabs and actions in a bar along the top, a header (state pill, large
//! title, who leads), then the plan beside a column of facts, or the
//! activity as a timeline.

use dragent_host::team::{
    Change, Cost, Message, MessageKind, Party, ReseatReason, Task, TaskStatus, Team, TeamEvent,
    TeamStatus, TeamSummary, Verdict,
};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty,
    Expanded, Flex, MainAxisSize, ParentElement, ScrollbarWidth, Stack, SavePosition,
    EventHandler, DispatchEventResult, OffsetPositioning, PositionedElementOffsetBounds, PositionedElementAnchor, ChildAnchor,
};
use warpui::fonts::FamilyId;
use warpui::AppContext;
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::{summary, DetailTab, ListFilter, TeamsAction, TeamsView, WorkerState};
use crate::automations::widgets::{button, field, label, one_line, ButtonKind};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{medium, semibold};
use crate::theme::Theme;

/// The list pane's width.
pub(super) const LIST_WIDTH: f32 = 320.0;
/// The facts column's width, beside the plan.
const RAIL_WIDTH: f32 = 300.0;
/// Below this, the facts fold under the header.
const WIDE_DETAIL: f32 = 860.0;

pub(super) fn render(view: &TeamsView, app: &AppContext) -> Box<dyn Element> {
    let theme = &view.theme;
    let body: Box<dyn Element> = if view.teams.is_empty() {
        match (&view.error, view.loading) {
            (Some(error), _) => centered(view, error, theme.ui_red()),
            (None, true) => centered(view, "Looking for coordinated work…", theme.sub_text()),
            (None, false) => empty_state(view, theme),
        }
    } else {
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                ConstrainedBox::new(if view.list_collapsed { list_rail(view, theme) } else { list_pane(view, theme) })
                    .with_width(list_width(view))
                    .finish(),
            )
            .with_child(rule(theme, true))
            .with_child(Expanded::new(1.0, detail_pane(view, theme)).finish())
            .finish()
    };
    let body = if let Some(task) = view.model_task.as_ref().and_then(|id| view.team.as_ref()?.task(id)) {
        let agent = task.agent.as_deref().and_then(crate::automations::executor::prompt_agent);
        if let Some(agent) = agent {
            let id = task.id.clone();
            let selected = if task.model_locked { task.user_model.as_deref() } else { task.model.as_deref() };
            let pick_id = id.clone();
            let model_panel = super::models::ModelPanel {
                tab: view.model_tab,
                agents: vec![(agent, super::models::choices(agent, app))],
                on_tab: std::rc::Rc::new(TeamsAction::BrowseModelTab),
                on_pick: std::rc::Rc::new(move |_, model| TeamsAction::SetTaskModel { task: pick_id.clone(), model }),
                on_favorite: std::rc::Rc::new(TeamsAction::ToggleModelFavorite),
            };
            let panel = super::models::picker(theme, view.ui_font, view.view_id, &view.mouse,
                &view.model_scroll, &view.model_editor, &view.model_query, agent, selected,
                &super::models::choices(agent, app),
                move |model| TeamsAction::SetTaskModel { task: id.clone(), model },
                TeamsAction::CloseModelPicker, TeamsAction::ClearModelSearch, Some(model_panel));
            let view_id = view.view_id;
            let catcher = EventHandler::new(Container::new(Empty::new().finish()).finish())
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action_for(view_id, TeamsAction::CloseModelPicker);
                    DispatchEventResult::StopPropagation
                }).finish();
            let panel = EventHandler::new(panel)
                .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation).finish();
            let mut stack = Stack::new().with_child(body);
            stack.add_overlay_child(catcher);
            stack.with_positioned_overlay_child(panel, OffsetPositioning::offset_from_save_position_element(
                    "team_task_model", warpui::geometry::vector::vec2f(0.0, -6.0),
                    PositionedElementOffsetBounds::WindowByPosition, PositionedElementAnchor::TopLeft, ChildAnchor::BottomLeft)).finish()
        } else { body }
    } else { body };
    // Deleting a coordination asks first, over the whole page.
    let body = match (&view.confirm_delete, &view.team) {
        (Some(plan), Some(team)) => {
            let view_id = view.view_id;
            let scrim = EventHandler::new(
                Container::new(Empty::new().finish())
                    .with_background_color(ColorU::new(0, 0, 0, 120))
                    .finish(),
            )
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, TeamsAction::CancelDelete);
                DispatchEventResult::StopPropagation
            })
            .finish();
            let card = EventHandler::new(delete_dialog(view, theme, team, plan))
                .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                .finish();
            let mut stack = Stack::new().with_child(body);
            stack.add_overlay_child(scrim);
            stack.add_overlay_child(Align::new(card).finish());
            stack.finish()
        }
        _ => body,
    };
    Container::new(body)
        .with_background_color(theme.background)
        .finish()
}

/// "Delete <title>?": what goes (the workers' workspaces, tabs and
/// worktrees, uncommitted work named), and that the lead stays.
fn delete_dialog(view: &TeamsView, theme: &Theme, team: &Team, plan: &super::DeletePlan) -> Box<dyn Element> {
    let ui = view.ui_font;
    let names = |ids: &[String]| ids.join(", ");
    let removed: Vec<String> = plan
        .remove
        .iter()
        .map(|(task, path)| {
            let folder = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            format!("{task} ({folder})")
        })
        .collect();
    let mut lines = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(10.0);
    let what = match (removed.len(), plan.workers.len()) {
        (0, 0) => "Nothing of its work is open: it only leaves the list.".to_string(),
        (0, tabs) => format!(
            "Closes {tabs} worker tab{}.",
            if tabs == 1 { "" } else { "s" }
        ),
        (count, _) => format!(
            "Closes the workers' tabs and {count} workspace{}, and removes {}: {}.",
            if count == 1 { "" } else { "s" },
            if count == 1 { "its worktree" } else { "their worktrees" },
            removed.join(", ")
        ),
    };
    lines = lines.with_child(label(ui, &what, 13.0, theme.main_text()));
    if !plan.dirty.is_empty() {
        lines = lines.with_child(label(
            ui,
            &format!("Changes not committed are lost in: {}.", names(&plan.dirty)),
            13.0,
            theme.ui_red(),
        ));
    }
    lines = lines.with_child(label(
        ui,
        "The lead's tab and workspace stay, as an ordinary workspace. Branches keep their commits.",
        13.0,
        theme.sub_text(),
    ));
    let buttons = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.0)
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(button(theme, ui, view.view_id, view.mouse("delete-no"), None, "Cancel", ButtonKind::Secondary, Some(TeamsAction::CancelDelete)))
        .with_child(button(theme, ui, view.view_id, view.mouse("delete-yes"), Some(AppIcon::Trash), "Delete coordination", ButtonKind::Danger, Some(TeamsAction::ConfirmDelete)))
        .finish();
    let card = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0)
        .with_child(
            Flex::row()
                .with_spacing(10.0)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(render_icon(AppIcon::Trash, theme.ui_red(), 16.0))
                .with_child(semibold(ui, format!("Delete “{}”?", team.title), 15.0, theme.main_text()))
                .finish(),
        )
        .with_child(lines.finish())
        .with_child(buttons)
        .finish();
    ConstrainedBox::new(
        Container::new(card)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .with_uniform_padding(20.0)
            .finish(),
    )
    .with_width(480.0)
    .finish()
}

// ── Pieces ───────────────────────────────────────────────────────────────────

/// Where the lead's message remembers being open, beside task ids.
const LEAD_DETAILS: &str = "lead";
/// How many files or checks a task's details list before "and N more".
const DETAIL_LIST_LIMIT: usize = 12;

fn rule(theme: &Theme, vertical: bool) -> Box<dyn Element> {
    let line = Container::new(Empty::new().finish())
        .with_background_color(theme.outline())
        .finish();
    if vertical {
        ConstrainedBox::new(line).with_width(1.0).finish()
    } else {
        ConstrainedBox::new(line).with_height(1.0).finish()
    }
}

fn tint(color: ColorU, alpha: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, alpha)
}

fn centered(view: &TeamsView, text: &str, color: ColorU) -> Box<dyn Element> {
    Align::new(label(view.ui_font, text, 13.0, color)).finish()
}

fn scroll(
    theme: &Theme,
    handle: &warpui::elements::ClippedScrollStateHandle,
    content: Box<dyn Element>,
) -> Box<dyn Element> {
    ClippedScrollable::vertical(
        handle.clone(),
        content,
        ScrollbarWidth::Auto,
        theme.outline().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

/// The word for a state in its own colour, over a light tint of it.
/// A coordination at work: its mark inside a faint gold ring that a light
/// laps, as the sidebar's working tile does, glowing softly in turn.
pub(super) fn working_mark(icon: AppIcon, size: f32) -> Box<dyn Element> {
    let amber = crate::workspace::worktree_status::WORKING_AMBER;
    let side = size + 10.0;
    let ring = Container::new(
        ConstrainedBox::new(Align::new(render_icon(icon, amber, size)).finish())
            .with_width(side - 2.0)
            .with_height(side - 2.0)
            .finish(),
    )
    .with_background_color(tint(amber, 26))
    .with_border(Border::all(1.0).with_border_color(tint(amber, 70)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(side / 2.0)))
    .finish();
    let lapped = Box::new(crate::workspace::workspace_activity::EdgeLap::new(ring, amber, side / 2.0));
    Box::new(crate::workspace::workspace_activity::FacePulse::new(lapped, 0, 3, amber))
}

/// A coordination's state mark: animated while it works.
fn state_mark(icon: AppIcon, color: ColorU, status: TeamStatus, size: f32) -> Box<dyn Element> {
    if status == TeamStatus::Running {
        working_mark(icon, size)
    } else {
        render_icon(icon, color, size)
    }
}

fn pill(ui: FamilyId, mark: Box<dyn Element>, text: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(mark)
            .with_child(medium(ui, text, 11.5, color))
            .finish(),
    )
    .with_background_color(tint(color, 30))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(11.0)))
    .with_padding_left(9.0)
    .with_padding_right(10.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .finish()
}

/// Medium-weight text on one line, cut short with an ellipsis.
fn medium_line(ui: FamilyId, text: &str, size: f32, color: ColorU) -> Box<dyn Element> {
    warpui::elements::Text::new(text.to_string(), ui, crate::theme::text_size(size))
        .with_color(color)
        .with_style(warpui::fonts::Properties::default().weight(warpui::fonts::Weight::Medium))
        .soft_wrap(false)
        .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
        .finish()
}

fn small_count(ui: FamilyId, text: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(medium(ui, text, 10.5, color))
        .with_background_color(tint(color, 34))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish()
}

fn dot(color: ColorU, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size / 2.0)))
            .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

fn separator(view: &TeamsView, theme: &Theme) -> Box<dyn Element> {
    label(view.ui_font, "·", 12.0, theme.disabled_text())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn ago(at_ms: i64) -> String {
    crate::automations::compact_ago(at_ms, now_ms())
}

pub(super) fn project_name(root: &str) -> String {
    std::path::Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string())
}

pub(super) fn agent_label(key: &str) -> String {
    crate::automations::executor::prompt_agent(key)
        .map(|agent| agent.label().to_string())
        .unwrap_or_else(|| key.to_string())
}

/// The agent's own icon, or the generic one for an agent Dragent doesn't know.
pub(super) fn agent_icon(theme: &Theme, key: &str, size: f32) -> Box<dyn Element> {
    match crate::automations::executor::prompt_agent(key) {
        Some(agent) => crate::workspace::prompt_agent_image(agent, theme, size, 1.0),
        None => render_icon(AppIcon::Agent, theme.sub_text(), size),
    }
}

/// Unfinished work whose lead isn't running: nothing moves until a lead
/// takes over, whatever stage the store last recorded.
fn needs_lead(view: &TeamsView, team: &Team) -> bool {
    matches!(team.status, TeamStatus::Planning | TeamStatus::Running)
        && team.lead.is_some()
        && view.lead.state.is_none()
        && view.lead.asleep.is_none()
}

/// A coordination's state: its mark, colour and word.
fn team_style(theme: &Theme, status: TeamStatus) -> (AppIcon, ColorU, &'static str) {
    match status {
        TeamStatus::Planning => (AppIcon::Clock, theme.accent, "Planning"),
        TeamStatus::Running => (AppIcon::Activity, crate::workspace::worktree_status::WORKING_AMBER, "At work"),
        TeamStatus::Done => (AppIcon::CircleCheck, theme.ansi_blue(), "Done"),
        TeamStatus::Stopped => (AppIcon::CircleX, theme.disabled_text(), "Stopped"),
    }
}

fn shows(filter: ListFilter, status: TeamStatus) -> bool {
    match filter {
        ListFilter::All => true,
        ListFilter::Active => matches!(status, TeamStatus::Planning | TeamStatus::Running),
        ListFilter::Finished => matches!(status, TeamStatus::Done | TeamStatus::Stopped),
    }
}

// ── The list ─────────────────────────────────────────────────────────────────

/// The list's width: narrow while folded away.
fn list_width(view: &TeamsView) -> f32 {
    if view.list_collapsed { FOLDED_WIDTH } else { LIST_WIDTH }
}

const FOLDED_WIDTH: f32 = 52.0;

/// Folds the list away to give the chosen one the room, or brings it back.
fn collapse_button(view: &TeamsView, theme: &Theme, collapsed: bool) -> Box<dyn Element> {
    let theme = *theme;
    let view_id = view.view_id;
    Hoverable::new(view.mouse("list-collapse"), move |state| {
        Container::new(render_icon(
            if collapsed { AppIcon::ChevronRight } else { AppIcon::ChevronLeft },
            if state.is_hovered() { theme.main_text() } else { theme.sub_text() },
            14.0,
        ))
        .with_background_color(if state.is_hovered() { theme.surface_3() } else { ColorU::transparent_black() })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_uniform_padding(5.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TeamsAction::ToggleList))
    .finish()
}

/// The folded list: the button to unfold it, then each coordination's
/// state mark (working ones animated), choosing it on click.
fn list_rail(view: &TeamsView, theme: &Theme) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(collapse_button(view, theme, true));
    for team in view.teams.iter().filter(|team| shows(view.filter, team.status)) {
        let (icon, color, _) = team_style(theme, team.status);
        let selected = view.selected.as_deref() == Some(team.id.as_str());
        let theme_copy = *theme;
        let view_id = view.view_id;
        let id = team.id.clone();
        let status = team.status;
        column = column.with_child(
            Hoverable::new(view.mouse(&format!("rail-{}", team.id)), move |state| {
                Container::new(
                    ConstrainedBox::new(Align::new(state_mark(icon, color, status, 14.0)).finish())
                        .with_width(28.0)
                        .with_height(28.0)
                        .finish(),
                )
                .with_background_color(if selected || state.is_hovered() { theme_copy.surface_2() } else { ColorU::transparent_black() })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_uniform_padding(3.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TeamsAction::Select(id.clone())))
            .finish(),
        );
    }
    Container::new(column.finish())
        .with_padding_top(16.0)
        .finish()
}

fn list_pane(view: &TeamsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let waiting: usize = view.teams.iter().map(|team| team.needs_user).sum();
    let mut title = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(render_icon(AppIcon::Layers, theme.accent, 15.0))
        .with_child(
            Expanded::new(
                1.0,
                semibold(ui, "Agent Coordination", 14.0, theme.main_text()),
            )
            .finish(),
        );
    if waiting > 0 {
        title = title.with_child(small_count(ui, &format!("{waiting} for you"), theme.accent));
    }
    title = title.with_child(collapse_button(view, theme, false));
    let mut filters = Flex::row().with_spacing(4.0);
    for (filter, text) in [
        (ListFilter::All, "All"),
        (ListFilter::Active, "Active"),
        (ListFilter::Finished, "Finished"),
    ] {
        filters = filters.with_child(filter_pill(view, theme, filter, text));
    }
    let top = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(title.finish())
            .with_child(filters.finish())
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(14.0)
    .with_padding_top(18.0)
    .with_padding_bottom(12.0)
    .finish();

    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    let mut any = false;
    for team in view
        .teams
        .iter()
        .filter(|team| shows(view.filter, team.status))
    {
        any = true;
        rows = rows.with_child(team_row(view, theme, team));
    }
    let list: Box<dyn Element> = if any {
        Container::new(rows.finish())
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(24.0)
            .finish()
    } else {
        Container::new(label(
            ui,
            match view.filter {
                ListFilter::Active => "Nothing at work right now.",
                ListFilter::Finished => "Nothing finished yet.",
                ListFilter::All => "Nothing here yet.",
            },
            12.5,
            theme.sub_text(),
        ))
        .with_padding_left(18.0)
        .with_padding_top(14.0)
        .finish()
    };
    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(top)
        .with_child(rule(theme, false))
        .with_child(Expanded::new(1.0, scroll(theme, &view.list_scroll, list)).finish())
        .finish()
}

fn filter_pill(
    view: &TeamsView,
    theme: &Theme,
    filter: ListFilter,
    text: &str,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let on = view.filter == filter;
    let text = text.to_string();
    let view_id = view.view_id;
    Hoverable::new(view.mouse(&format!("filter-{text}")), move |state| {
        Container::new(medium(
            ui,
            &text,
            12.0,
            if on || state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
        ))
        .with_background_color(if on {
            theme.surface_2()
        } else if state.is_hovered() {
            theme.surface_1()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, TeamsAction::Filter(filter))
    })
    .finish()
}

fn team_row(view: &TeamsView, theme: &Theme, team: &TeamSummary) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let selected = view.selected.as_deref() == Some(team.id.as_str());
    let (icon, color, _) = team_style(&theme, team.status);
    let status = team.status;
    let title = team.title.clone();
    let mut meta = project_name(&team.project_root);
    if let Some(machine) = crate::remote_fs::machine_name(std::path::Path::new(&team.project_root)) {
        meta.push_str(&format!(" · {machine}"));
    }
    if team.tasks > 0 {
        meta.push_str(&format!(
            " · {} task{}",
            team.tasks,
            if team.tasks == 1 { "" } else { "s" }
        ));
    }
    if team.running > 0 {
        meta.push_str(&format!(" · {} working", team.running));
    }
    let progress = (team.tasks > 0).then(|| format!("{}/{}", team.done, team.tasks));
    let blocked = team.blocked;
    let for_you = team.needs_user;
    let age = ago(team.updated_at_ms);
    let id = team.id.clone();
    let view_id = view.view_id;
    Hoverable::new(view.mouse(&format!("team-{}", team.id)), move |state| {
        let mut end_top = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0);
        if for_you > 0 {
            end_top =
                end_top.with_child(small_count(ui, &format!("{for_you} for you"), theme.accent));
        } else if blocked > 0 {
            end_top =
                end_top.with_child(render_icon(AppIcon::TriangleAlert, theme.ui_yellow(), 12.0));
        }
        if let Some(progress) = &progress {
            end_top = end_top.with_child(label(ui, progress, 11.5, theme.sub_text()));
        }
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(state_mark(icon, color, status, 14.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_spacing(3.0)
                            .with_child(medium_line(ui, &title, 13.0, theme.main_text()))
                            .with_child(one_line(ui, &meta, 11.0, theme.sub_text()))
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::End)
                        .with_spacing(3.0)
                        .with_child(end_top.finish())
                        .with_child(label(ui, &age, 10.5, theme.disabled_text()))
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(if selected {
            theme.surface_2()
        } else if state.is_hovered() {
            theme.surface_1()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(9.0)
        .with_padding_bottom(9.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, TeamsAction::Select(id.clone()))
    })
    .finish()
}

fn empty_state(view: &TeamsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let card = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(12.0)
            .with_child(
                Container::new(render_icon(AppIcon::Layers, theme.accent, 22.0))
                    .with_background_color(tint(theme.accent, 28))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                    .with_padding_left(11.0)
                    .with_padding_right(11.0)
                    .with_padding_top(11.0)
                    .with_padding_bottom(11.0)
                    .finish(),
            )
            .with_child(semibold(ui, "Coordinate agents on one piece of work", 16.0, theme.main_text()))
            .with_child(
                ConstrainedBox::new(label(
                    ui,
                    "A lead agent plans the work and gives each part to an agent of its own: parts that run side by side each get a worktree, parts that follow on share one. You follow it here and answer when it asks. To start, turn on Coordinate next to the agent on a project's prompt screen, or ask any agent in a Dragent terminal to split the work up.",
                    12.5,
                    theme.sub_text(),
                ))
                .with_max_width(460.0)
                .finish(),
            )
            .finish(),
    )
    .with_padding_left(28.0)
    .with_padding_right(28.0)
    .with_padding_top(28.0)
    .with_padding_bottom(28.0)
    .finish();
    Container::new(Align::new(card).finish())
        .with_padding_bottom(80.0)
        .finish()
}

// ── The chosen one ───────────────────────────────────────────────────────────

fn detail_pane(view: &TeamsView, theme: &Theme) -> Box<dyn Element> {
    let Some(team)=&view.team else {return centered(view,"Choose one on the left.",theme.sub_text());};
    if view.tab==DetailTab::Board {return board_pane(view,theme,team);}
    let wide=view.width.get()-list_width(view)>=WIDE_DETAIL;
    let at_work=matches!(team.status,TeamStatus::Planning|TeamStatus::Running);
    let reply=open_question(view,team).is_some();
    let mut column=Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(20.0).with_child(header(view,theme,team));
    for notice in notices(view,theme,team) {column=column.with_child(notice);}
    if !wide && view.tab==DetailTab::Overview {column=column.with_child(facts(view,theme,team,true));}
    column=column.with_child(match view.tab {
        DetailTab::Overview=>overview(view,theme,team),
        DetailTab::Activity=>activity(view,theme,team),
        DetailTab::Board=>super::board::board(view,theme,team),
    });
    let body=Container::new(column.finish()).with_padding_left(28.0).with_padding_right(28.0)
        .with_padding_top(24.0).with_padding_bottom(28.0).finish();
    let main=scroll(theme,&view.detail_scroll,body);
    let content=if wide {
        // The details scroll independently above a fixed message composer.
        let fact_body=Container::new(facts(view,theme,team,false)).with_uniform_padding(20.0).finish();
        let mut rail=Flex::column().with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(Expanded::new(1.0,scroll(theme,&view.rail_scroll,fact_body)).finish());
        if at_work {
            rail=rail.with_child(rule(theme,false)).with_child(
                Container::new(composer(view,theme,team,reply,104.0)).with_uniform_padding(20.0)
                    .with_background_color(theme.surface_1()).finish());
        }
        Flex::row().with_main_axis_size(MainAxisSize::Max).with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(Expanded::new(1.0,main).finish()).with_child(rule(theme,true))
            .with_child(ConstrainedBox::new(rail.finish()).with_width(RAIL_WIDTH).finish()).finish()
    } else {
        // Keep messaging reachable in narrow windows without squeezing the plan.
        let mut content=Flex::column().with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(Expanded::new(1.0,main).finish());
        if at_work {content=content.with_child(rule(theme,false)).with_child(
            Container::new(composer(view,theme,team,reply,72.0)).with_padding_left(20.0).with_padding_right(20.0)
                .with_padding_top(14.0).with_padding_bottom(14.0).with_background_color(theme.surface_1()).finish());}
        content.finish()
    };
    Flex::column().with_main_axis_size(MainAxisSize::Max).with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(top_bar(view,theme,team)).with_child(rule(theme,false))
        .with_child(Expanded::new(1.0,content).finish()).finish()
}

/// Tabs on the left, the actions on the right.
fn top_bar(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let ui = view.ui_font;
    let messages = team.messages.len();
    let activity = if messages > 0 {
        format!("Activity · {messages}")
    } else {
        "Activity".into()
    };
    let tabs = Flex::row()
        .with_spacing(2.0)
        .with_child(tab_button(
            view,
            theme,
            DetailTab::Overview,
            "Overview".into(),
        ))
        .with_child(tab_button(view, theme, DetailTab::Board, board_label(view, team)))
        .with_child(tab_button(view, theme, DetailTab::Activity, activity));
    let mut actions = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0);
    if team
        .lead
        .as_ref()
        .and_then(|lead| lead.pane.as_ref())
        .is_some()
    {
        actions = actions.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("open-lead"),
            Some(AppIcon::Terminal),
            "Open lead",
            ButtonKind::Secondary,
            Some(TeamsAction::OpenLead),
        ));
    }
    if let Some(pane) = view.lead.asleep.clone() {
        actions = actions.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("wake-lead"),
            None,
            "Wake lead",
            ButtonKind::Secondary,
            Some(TeamsAction::Wake(pane)),
        ));
    }
    if matches!(team.status, TeamStatus::Planning | TeamStatus::Running) {
        actions = actions.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("stop-team"),
            None,
            "Stop",
            ButtonKind::Danger,
            Some(TeamsAction::AskStop),
        ));
    } else {
        if view.cleanup.is_some() {
            actions = actions.with_child(button(
                theme,
                ui,
                view.view_id,
                view.mouse("clean-up"),
                None,
                "Clean up",
                ButtonKind::Secondary,
                Some(TeamsAction::AskCleanUp),
            ));
        }
        actions = actions.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("forget"),
            Some(AppIcon::Trash),
            "Delete coordination",
            ButtonKind::Ghost,
            Some(TeamsAction::AskDelete),
        ));
    }
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(tabs.finish())
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(actions.finish())
            .finish(),
    )
    .with_padding_left(24.0)
    .with_padding_right(18.0)
    .with_padding_top(12.0)
    .with_padding_bottom(12.0)
    .finish()
}

fn tab_button(view: &TeamsView, theme: &Theme, tab: DetailTab, text: String) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let on = view.tab == tab;
    let view_id = view.view_id;
    Hoverable::new(view.mouse(&format!("tab-{tab:?}")), move |state| {
        Container::new(medium(
            ui,
            &text,
            12.5,
            if on || state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
        ))
        .with_background_color(if on {
            theme.surface_2()
        } else if state.is_hovered() {
            theme.surface_1()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(11.0)
        .with_padding_right(11.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, TeamsAction::Tab(tab)))
    .finish()
}

/// The state pill beside the project, the title large, then who leads.
fn header(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let ui = view.ui_font;
    let (icon, color, word) = if needs_lead(view, team) {
        (AppIcon::TriangleAlert, theme.ui_yellow(), "Needs a lead")
    } else {
        team_style(theme, team.status)
    };
    let top = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(pill(
            ui,
            if needs_lead(view, team) { render_icon(icon, color, 12.0) } else { state_mark(icon, color, team.status, 12.0) },
            word,
            color,
        ))
        .with_child(label(
            ui,
            &project_name(&team.project_root),
            12.0,
            theme.sub_text(),
        ))
        .finish();
    let mut meta = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(7.0);
    if let Some(lead) = team.lead.as_ref().and_then(|lead| lead.agent.as_deref()) {
        meta = meta
            .with_child(agent_icon(theme, lead, 13.0))
            .with_child(medium(
                ui,
                format!("Led by {} · {}", agent_label(lead), team.lead_model.as_deref().unwrap_or("Agent default")),
                12.0,
                theme.main_text(),
            ))
            .with_child(separator(view, theme));
    }
    meta = meta
        .with_child(label(
            ui,
            &format!("started {}", ago(team.created_at_ms)),
            12.0,
            theme.sub_text(),
        ))
        .with_child(separator(view, theme))
        .with_child(label(
            ui,
            &format!("updated {}", ago(team.updated_at_ms)),
            12.0,
            theme.sub_text(),
        ));
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(top)
        .with_child(semibold(ui, &team.title, 20.0, theme.main_text()))
        .with_child(meta.finish())
        .finish()
}

fn lead_word(theme: &Theme, state: Option<WorkerState>) -> (&'static str, ColorU) {
    match state {
        Some(WorkerState::Working) => ("working", theme.ui_green()),
        Some(WorkerState::NeedsApproval) => ("waiting for your approval", theme.ui_yellow()),
        Some(WorkerState::Idle) => ("idle", theme.sub_text()),
        Some(WorkerState::Unknown) => ("running", theme.sub_text()),
        None => ("not running", theme.ui_red()),
    }
}

fn notices(view: &TeamsView, theme: &Theme, team: &Team) -> Vec<Box<dyn Element>> {
    let ui = view.ui_font;
    let mut notices = Vec::new();
    if view.confirm_stop {
        notices.push(callout(
            view,
            theme,
            theme.ui_red(),
            AppIcon::CircleX,
            "Stop it? Unfinished tasks are cancelled. Work already done stays in its branches, and the agents' tabs stay open.",
            vec![
                button(theme, ui, view.view_id, view.mouse("stop-yes"), None, "Stop", ButtonKind::Danger, Some(TeamsAction::ConfirmStop)),
                button(theme, ui, view.view_id, view.mouse("stop-no"), None, "Keep going", ButtonKind::Secondary, Some(TeamsAction::CancelStop)),
            ],
        ));
    }
    if let (true, Some(plan)) = (view.confirm_cleanup, &view.cleanup) {
        let name = |(task, path): &(String, std::path::PathBuf)| {
            format!(
                "{task} ({})",
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        };
        let list = |items: &[(String, std::path::PathBuf)]| {
            items.iter().map(name).collect::<Vec<_>>().join(", ")
        };
        let mut text = format!(
            "Closes and removes {} worktree{}: {}. Their branches stay, with all the work.",
            plan.remove.len(),
            if plan.remove.len() == 1 { "" } else { "s" },
            list(&plan.remove)
        );
        if !plan.keep.is_empty() {
            text.push_str(&format!(
                " Kept, to look at the result: {}.",
                list(&plan.keep)
            ));
        }
        if !plan.dirty.is_empty() {
            text.push_str(&format!(
                " Left alone, with changes not committed: {}.",
                list(&plan.dirty)
            ));
        }
        notices.push(callout(
            view,
            theme,
            theme.accent,
            AppIcon::FolderGit,
            &text,
            vec![
                button(
                    theme,
                    ui,
                    view.view_id,
                    view.mouse("cleanup-yes"),
                    None,
                    "Clean up",
                    ButtonKind::Primary,
                    Some(TeamsAction::ConfirmCleanUp),
                ),
                button(
                    theme,
                    ui,
                    view.view_id,
                    view.mouse("cleanup-no"),
                    None,
                    "Cancel",
                    ButtonKind::Secondary,
                    Some(TeamsAction::CancelCleanUp),
                ),
            ],
        ));
    }
    if needs_lead(view, team) {
        let text = if team.tasks.is_empty() {
            "The lead stopped before it made a plan. A new lead starts again from your request."
        } else {
            "The lead isn't running, so nobody follows the workers or answers them. A new lead picks up from the plan and the messages."
        };
        notices.push(callout(
            view,
            theme,
            theme.ui_yellow(),
            AppIcon::TriangleAlert,
            text,
            vec![button(
                theme,
                ui,
                view.view_id,
                view.mouse("new-lead"),
                None,
                "Start a new lead",
                ButtonKind::Primary,
                Some(TeamsAction::StartNewLead),
            )],
        ));
    }
    if let Some(error) = &view.error {
        notices.push(callout(
            view,
            theme,
            theme.ui_red(),
            AppIcon::CircleX,
            error,
            Vec::new(),
        ));
    }
    notices
}

fn callout(
    view: &TeamsView,
    theme: &Theme,
    color: ColorU,
    icon: AppIcon,
    text: &str,
    buttons: Vec<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(render_icon(icon, color, 14.0))
        .with_child(
            Expanded::new(1.0, label(view.ui_font, text, 12.5, theme.main_text())).finish(),
        );
    for button in buttons {
        row = row.with_child(button);
    }
    Container::new(row.finish())
        .with_background_color(tint(color, 20))
        .with_border(Border::all(1.0).with_border_color(tint(color, 80)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(14.0)
        .with_padding_right(10.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .finish()
}

// ── Overview ─────────────────────────────────────────────────────────────────

fn overview(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(18.0);
    for card in approval_cards(view, theme, team) {
        column = column.with_child(card);
    }
    let question = view.answering().filter(|message| unanswered(team, message));
    if let Some(question) = question {
        column = column.with_child(question_card(view, theme, question, false));
    }
    if let Some(said) = lead_waiting(view, team) {
        column = column.with_child(lead_waiting_card(view, theme, team, said, false));
    }
    if let Some(summary) = summary_card(view, theme, team) {
        column = column.with_child(summary);
    }
    column = column.with_child(plan(view, theme, team));
    column.finish()
}

/// The lead's question or request to the person, while they haven't
/// answered it. A note or a finding isn't one: nothing waits on a reply.
pub(super) fn open_question<'a>(view: &'a TeamsView, team: &Team) -> Option<&'a Message> {
    view.answering()
        .filter(|message| matches!(message.kind, MessageKind::Question | MessageKind::NeedsUser))
        .filter(|message| unanswered(team, message))
}

/// A message for the person that nothing they sent has followed yet.
pub(super) fn unanswered(team: &Team, message: &Message) -> bool {
    !team
        .messages
        .iter()
        .any(|later| later.id > message.id && later.from == Party::User)
}

fn question_card(
    view: &TeamsView,
    theme: &Theme,
    question: &Message,
    reply: bool,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let asks = matches!(question.kind, MessageKind::NeedsUser | MessageKind::Question);
    // A question waits on the person: the orange every agent waiting on
    // them shows. Anything else is news, in the accent.
    let accent = if asks { crate::workspace::worktree_status::needs_input_color(theme) } else { theme.accent };
    let heading = if asks {
        format!("{} asks you", party_label(&question.from))
    } else {
        format!("From {}", party_label(&question.from))
    };
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(render_icon(AppIcon::MessageSquare, accent, 13.0))
                .with_child(medium(ui, &heading, 12.5, accent))
                .with_child(separator(view, theme))
                .with_child(label(ui, &ago(question.at_ms), 11.5, theme.sub_text()))
                .finish(),
        )
        .with_child(card_message(view, theme, question.body.trim()));
    // One click answers: the first is the suggested one.
    if asks && !question.choices.is_empty() {
        let mut row = warpui::elements::Wrap::row().with_spacing(8.0).with_run_spacing(8.0);
        for (index, choice) in question.choices.iter().enumerate() {
            row = row.with_child(choice_button(view, theme, accent, question.id, choice, index == 0));
        }
        column = column.with_child(row.finish());
    }
    if reply && asks {
        column = column.with_child(card_reply(view, theme, accent));
    }
    Container::new(column.finish())
        .with_background_color(tint(accent, 18))
        .with_border(Border::all(1.0).with_border_color(tint(accent, 110)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
}

/// Text an agent wrote, its markdown drawn as formatting rather than
/// shown as `**` and `#`.
pub(super) fn prose(view: &TeamsView, theme: &Theme, text: &str, size: f32) -> Box<dyn Element> {
    crate::terminal::render::markdown::render_markdown_prose(
        &summary::quiet_headings(text),
        *theme,
        view.ui_font,
        view.mono_font,
        crate::theme::text_size(size),
        1.45,
    )
}

/// The message in a card that waits on the person: formatted, and a long
/// one scrolls inside the card instead of pushing the board down.
fn card_message(view: &TeamsView, theme: &Theme, text: &str) -> Box<dyn Element> {
    ConstrainedBox::new(
        ClippedScrollable::vertical(
            view.card_scroll.clone(),
            Container::new(prose(view, theme, text, 13.0)).with_padding_right(10.0).finish(),
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            ColorU::transparent_black().into(),
        )
        .with_overlayed_scrollbar()
        .finish(),
    )
    .with_max_height(CARD_MESSAGE_MAX)
    .finish()
}

/// How tall a waiting card's message grows before it scrolls.
const CARD_MESSAGE_MAX: f32 = 168.0;

/// One answer to click: the suggested one solid, the others quiet.
fn choice_button(view: &TeamsView, theme: &Theme, color: ColorU, message: u64, choice: &str, first: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let view_id = view.view_id;
    let text = choice.to_string();
    let action = TeamsAction::Choose { message, choice: choice.to_string() };
    Hoverable::new(view.mouse(&format!("choice-{message}-{choice}")), move |state| {
        let hovered = state.is_hovered();
        let (background, foreground) = if first {
            (if hovered { tint(color, 235) } else { color }, ColorU::new(29, 13, 2, 255))
        } else {
            (if hovered { theme.surface_3() } else { theme.surface_2() }, theme.main_text())
        };
        Container::new(medium(ui, &text, 12.5, foreground))
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// The lead's turn ended and it waits on the person, asking in its own
/// terminal rather than on this page (the plan, ready to start; a question
/// to talk over). Nothing the person sent has followed it.
pub(super) fn lead_waiting<'a>(view: &'a TeamsView, team: &Team) -> Option<&'a str> {
    if !matches!(team.status, TeamStatus::Planning | TeamStatus::Running)
        || view.lead.state != Some(WorkerState::Idle)
        || open_question(view, team).is_some()
    {
        return None;
    }
    let text = view.replies.lead.as_deref().map(str::trim).filter(|text| !text.is_empty())?;
    let at = view.replies.lead_at? as i64;
    let answered = team
        .messages
        .iter()
        .any(|message| message.from == Party::User && message.at_ms >= at);
    (!answered).then_some(text)
}

/// "Claude Code is waiting for you": what the lead last said, then the
/// answers to click. While it plans, the go-ahead is one click.
fn lead_waiting_card(view: &TeamsView, theme: &Theme, team: &Team, said: &str, reply: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let orange = crate::workspace::worktree_status::needs_input_color(theme);
    let who = team
        .lead
        .as_ref()
        .and_then(|lead| lead.agent.as_deref())
        .map(agent_label)
        .unwrap_or_else(|| "The lead".into());
    // The end of what it said is where it asks.
    const SHOWN: usize = 900;
    let count = said.chars().count();
    let shown = if count > SHOWN {
        format!("…{}", said.chars().skip(count - SHOWN).collect::<String>())
    } else {
        said.to_string()
    };
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(render_icon(AppIcon::MessageSquare, orange, 13.0))
                .with_child(medium(ui, &format!("{who} is waiting for you"), 12.5, orange))
                .finish(),
        )
        .with_child(card_message(view, theme, &shown));
    let mut buttons = warpui::elements::Wrap::row().with_spacing(8.0).with_run_spacing(8.0);
    if team.status == TeamStatus::Planning {
        buttons = buttons
            .with_child(say_button(view, theme, orange, "Start the plan", "Go ahead: start the plan.", true))
            .with_child(say_button(view, theme, orange, "Change the plan", "Not yet: I want to change the plan. Ask me what to change.", false));
    } else {
        buttons = buttons.with_child(say_button(view, theme, orange, "Carry on", "Carry on.", true));
    }
    column = column.with_child(buttons.finish());
    column = if reply {
        column.with_child(card_reply(view, theme, orange))
    } else {
        column.with_child(label(ui, "Or write back in the box below.", 11.5, theme.sub_text()))
    };
    Container::new(column.finish())
        .with_background_color(tint(orange, 18))
        .with_border(Border::all(1.0).with_border_color(tint(orange, 110)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
}

fn say_button(view: &TeamsView, theme: &Theme, color: ColorU, text: &str, said: &str, first: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let view_id = view.view_id;
    let shown = text.to_string();
    let action = TeamsAction::Say(said.to_string());
    Hoverable::new(view.mouse(&format!("say-{text}")), move |state| {
        let hovered = state.is_hovered();
        let (background, foreground) = if first {
            (if hovered { tint(color, 235) } else { color }, ColorU::new(29, 13, 2, 255))
        } else {
            (if hovered { theme.surface_3() } else { theme.surface_2() }, theme.main_text())
        };
        Container::new(medium(ui, &shown, 12.5, foreground))
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// Agents stopped at a permission prompt (the lead, then workers), each
/// answerable here: the person may never open their terminals.
fn approval_cards(view: &TeamsView, theme: &Theme, team: &Team) -> Vec<Box<dyn Element>> {
    let mut waiting: Vec<(Option<String>, String)> = Vec::new();
    if view.lead.state == Some(WorkerState::NeedsApproval) {
        let who = team
            .lead
            .as_ref()
            .and_then(|lead| lead.agent.as_deref())
            .map(agent_label)
            .unwrap_or_else(|| "The lead".into());
        waiting.push((None, format!("{who}, the lead")));
    }
    for task in &team.tasks {
        if view.workers.get(&task.id) == Some(&WorkerState::NeedsApproval) {
            let who = task.agent.as_deref().map(agent_label).unwrap_or_else(|| "Its agent".into());
            waiting.push((Some(task.id.clone()), format!("{who} on {} · {}", task.id, task.title)));
        }
    }
    waiting
        .into_iter()
        .map(|(task, who)| approval_card(view, theme, task, &who))
        .collect()
}

fn approval_card(view: &TeamsView, theme: &Theme, task: Option<String>, who: &str) -> Box<dyn Element> {
    let ui = view.ui_font;
    let orange = crate::workspace::worktree_status::needs_input_color(theme);
    let key = task.clone().unwrap_or_else(|| "lead".into());
    let open = match &task {
        Some(id) => TeamsAction::OpenWorker(id.clone()),
        None => TeamsAction::OpenLead,
    };
    let buttons = Flex::row()
        .with_spacing(8.0)
        .with_child(button(theme, ui, view.view_id, view.mouse(&format!("allow-{key}")), Some(AppIcon::Check), "Allow", ButtonKind::Primary,
            Some(TeamsAction::Approve { task: task.clone(), allow: true })))
        .with_child(button(theme, ui, view.view_id, view.mouse(&format!("deny-{key}")), None, "Deny", ButtonKind::Secondary,
            Some(TeamsAction::Approve { task: task.clone(), allow: false })))
        .with_child(button(theme, ui, view.view_id, view.mouse(&format!("see-{key}")), Some(AppIcon::Terminal), "See what it asks", ButtonKind::Ghost, Some(open)))
        .finish();
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(7.0)
                    .with_child(render_icon(AppIcon::TriangleAlert, orange, 13.0))
                    .with_child(medium(ui, "Waiting for your permission", 12.5, orange))
                    .finish(),
            )
            .with_child(label(ui, &format!("{who} is stopped at a permission prompt. Allow it to carry on, or deny it."), 13.0, theme.main_text()))
            .with_child(buttons)
            .finish(),
    )
    .with_background_color(tint(orange, 18))
    .with_border(Border::all(1.0).with_border_color(tint(orange, 110)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(14.0)
    .with_padding_bottom(14.0)
    .finish()
}

/// The box to write to the lead in, drawn as the new workspace's prompt
/// is: the green caret and the text, then the mic and the send button in
/// the box's corner. A line under it says where the message goes.
fn composer(view: &TeamsView, theme: &Theme, team: &Team, reply: bool, height: f32) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mono = view.mono_font;
    let view_id = view.view_id;
    let theme = *theme;
    let can_send = !view.message.trim().is_empty() && !view.sending;
    let hint = match (reply, team.lead.is_some()) {
        _ if needs_lead(view, team) => "No lead is running: the next lead reads it.",
        _ if view.lead.state == Some(WorkerState::NeedsApproval) => {
            "The lead is waiting at a permission prompt: your message goes in once that's answered."
        }
        (true, _) => "Your answer goes straight to the lead.",
        (false, true) => "It goes straight to the lead. If the lead is working, it's next in its queue.",
        (false, false) => "There's no lead agent: messages wait in the mailbox.",
    };

    let size = crate::theme::scaled(crate::theme::INPUT_FONT_SIZE);
    let caret = Container::new(
        ConstrainedBox::new(Align::new(render_icon(AppIcon::ChevronRight, theme.ansi_green(), size)).finish())
            .with_width(size)
            .with_height(crate::console::input_layout::line_height_with_spacing(size))
            .finish(),
    )
    .with_padding_right(4.0)
    .finish();
    let editor_id = view.message_editor.id();
    let editor = EventHandler::new(
        ConstrainedBox::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_child(caret)
                .with_child(Expanded::new(1.0, warpui::elements::ChildView::new(&view.message_editor).finish()).finish())
                .finish(),
        )
        .with_min_height(height)
        .finish(),
    )
    .on_left_mouse_down(move |ctx, _app, position| {
        ctx.dispatch_typed_action_for(editor_id, crate::workspace::InputEditorAction::ClickAt(position));
        DispatchEventResult::StopPropagation
    })
    .finish();

    let mut footer = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(6.0)
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if let Some(voice) = voice_button(view, &theme) {
        footer = footer.with_child(voice);
    }
    footer = footer.with_child(send_button(view, &theme, can_send));

    let boxed = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(editor)
            .with_child(footer.finish())
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(10.0)
    .with_padding_bottom(12.0)
    .finish();
    let note = match &view.voice_notice {
        Some(notice) => label(ui, notice, 11.5, theme.ui_yellow()),
        None => label(crate::fonts::ui_role(mono), hint, 11.5, theme.disabled_text()),
    };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(boxed)
        .with_child(note)
        .finish()
}

/// The mic, where voice input is supported.
fn voice_button(view: &TeamsView, theme: &Theme) -> Option<Box<dyn Element>> {
    if !crate::voice::supported() {
        return None;
    }
    let theme = *theme;
    let view_id = view.view_id;
    let voice = view.voice_ui;
    let spin = view.voice_spin;
    Some(
        Hoverable::new(view.mouse("voice"), move |state| {
            let color = match voice {
                crate::voice::VoiceUiState::Idle if state.is_hovered() => theme.main_text(),
                crate::voice::VoiceUiState::Idle => theme.sub_text(),
                crate::voice::VoiceUiState::Connecting => theme.accent,
                crate::voice::VoiceUiState::Recording => theme.ui_red(),
            };
            let icon = if voice == crate::voice::VoiceUiState::Connecting {
                crate::icons::render_spinner(spin, color, 15.0)
            } else {
                render_icon(AppIcon::Microphone, color, 15.0)
            };
            Container::new(icon)
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_background_color(if state.is_hovered() { theme.surface_2() } else { ColorU::transparent_black() })
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| ctx.dispatch_typed_action_for(view_id, TeamsAction::ToggleVoice))
        .finish(),
    )
}

/// The send button: the accent arrow, dim until there's something to send.
fn send_button(view: &TeamsView, theme: &Theme, can_send: bool) -> Box<dyn Element> {
    let theme = *theme;
    let view_id = view.view_id;
    let sending = view.sending;
    Hoverable::new(view.mouse("send"), move |state| {
        let accent = theme.accent;
        let background = if !can_send && !sending {
            ColorU::new(accent.r, accent.g, accent.b, 110)
        } else if state.is_hovered() {
            ColorU::new(accent.r.saturating_add(15), accent.g.saturating_add(15), accent.b.saturating_add(15), accent.a)
        } else {
            accent
        };
        let glyph = if sending {
            crate::icons::render_spinner(0, theme.background, 16.0)
        } else {
            render_icon(AppIcon::ArrowUp, theme.background, 16.0)
        };
        Container::new(glyph)
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_uniform_padding(7.5)
            .finish()
    })
    .with_cursor(if can_send { Cursor::PointingHand } else { Cursor::Arrow })
    .on_click(move |ctx, _app, _pos| {
        if can_send {
            ctx.dispatch_typed_action_for(view_id, TeamsAction::Send);
        }
    })
    .finish()
}

/// The answer box inside a card that waits on the person: one line that
/// grows with the text up to three, the mic and send on its right.
fn card_reply(view: &TeamsView, theme: &Theme, accent: ColorU) -> Box<dyn Element> {
    let can_send = !view.message.trim().is_empty() && !view.sending;
    let editor_id = view.message_editor.id();
    let editor = EventHandler::new(warpui::elements::ChildView::new(&view.message_editor).finish())
        .on_left_mouse_down(move |ctx, _app, position| {
            ctx.dispatch_typed_action_for(editor_id, crate::workspace::InputEditorAction::ClickAt(position));
            DispatchEventResult::StopPropagation
        })
        .finish();
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_spacing(6.0)
        .with_child(Expanded::new(1.0, Container::new(editor).with_padding_bottom(3.0).finish()).finish());
    if let Some(voice) = voice_button(view, theme) {
        row = row.with_child(voice);
    }
    Container::new(row.with_child(send_button(view, theme, can_send)).finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(tint(accent, 90)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(10.0)
        .with_padding_right(5.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish()
}

/// How a task shows: its mark, colour and a word or two.
pub(super) fn task_style(
    theme: &Theme,
    team: &Team,
    task: &Task,
    worker: Option<WorkerState>,
) -> (AppIcon, ColorU, String) {
    let ready = team.ready_tasks().iter().any(|ready| ready.id == task.id);
    let verdict = task.result.as_ref().and_then(|result| result.verdict);
    match task.status {
        TaskStatus::Pending if verdict == Some(Verdict::Changes) => (
            AppIcon::Clock,
            theme.ui_yellow(),
            if task.qa {
                "found problems; tests again after the fix".into()
            } else {
                "asked for changes; looks again after the fix".into()
            },
        ),
        TaskStatus::Pending if ready => (AppIcon::Play, theme.accent, "ready to start".into()),
        TaskStatus::Pending => (
            AppIcon::Clock,
            theme.disabled_text(),
            format!("waits for {}", team.waiting_on(task).join(", ")),
        ),
        TaskStatus::Starting => (AppIcon::Activity, theme.accent, "starting".into()),
        TaskStatus::Running => match worker {
            Some(WorkerState::NeedsApproval) => (
                AppIcon::TriangleAlert,
                theme.ui_yellow(),
                "waiting for your approval".into(),
            ),
            Some(WorkerState::Idle) => (AppIcon::Activity, theme.sub_text(), "idle".into()),
            Some(WorkerState::Working) => (AppIcon::Activity, theme.ui_green(), "working".into()),
            // Its agent hasn't said what it's doing.
            _ => (AppIcon::Activity, theme.sub_text(), "running".into()),
        },
        TaskStatus::Blocked => (AppIcon::TriangleAlert, theme.ui_yellow(), "blocked".into()),
        TaskStatus::Done if verdict == Some(Verdict::Approve) => (
            AppIcon::CircleCheck,
            theme.ui_green(),
            if task.qa { "passed QA" } else { "approved" }.into(),
        ),
        TaskStatus::Done if verdict == Some(Verdict::Changes) => (
            AppIcon::CircleX,
            theme.ui_yellow(),
            if task.qa {
                "found problems"
            } else {
                "asked for changes"
            }
            .into(),
        ),
        // Finished work: how far it got towards the person's checkout.
        TaskStatus::Done => match task.stage(team) {
            "integrated" => (AppIcon::GitBranch, theme.ansi_blue(), "in your checkout".into()),
            "qa_passed" => (AppIcon::CircleCheck, theme.ui_green(), "passed QA".into()),
            "review_approved" => (AppIcon::CircleCheck, theme.ui_green(), "approved".into()),
            _ => (AppIcon::CircleCheck, theme.ui_green(), "done".into()),
        },
        TaskStatus::Failed => (AppIcon::CircleX, theme.ui_red(), "failed".into()),
        // Not an error: the work is kept and waits for someone to choose.
        TaskStatus::NeedsDecision => (
            AppIcon::MessageSquare,
            theme.ui_yellow(),
            "needs decision".into(),
        ),
        TaskStatus::Cancelled => (AppIcon::XClose, theme.disabled_text(), "cancelled".into()),
    }
}

fn plan(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let ui = view.ui_font;
    let done = team
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Done)
        .count();
    let mut head = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(semibold(ui, "Plan", 13.5, theme.main_text()));
    if !team.tasks.is_empty() {
        head = head.with_child(label(
            ui,
            &format!("{done} of {} done", team.tasks.len()),
            12.0,
            theme.sub_text(),
        ));
    }
    let head = Container::new(head.finish())
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(11.0)
        .with_padding_bottom(11.0)
        .finish();
    let mut card = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(head);
    if team.tasks.is_empty() {
        // Planning: the same loader as the Board's Up next column.
        let inner: Box<dyn Element> = if !needs_lead(view, team) && team.status == TeamStatus::Planning {
            super::board::planning_card(view, theme, team)
        } else {
            let empty = if needs_lead(view, team) {
                "No plan yet."
            } else {
                "No tasks yet: the lead is still planning, and shows you the plan in its tab before any worker starts."
            };
            label(ui, empty, 12.5, theme.sub_text())
        };
        card = card.with_child(rule(theme, false)).with_child(
            Container::new(inner)
                .with_padding_left(14.0)
                .with_padding_right(14.0)
                .with_padding_top(12.0)
                .with_padding_bottom(14.0)
                .finish(),
        );
    }
    for task in &team.tasks {
        card = card
            .with_child(rule(theme, false))
            .with_child(task_row(view, theme, team, task));
    }
    Container::new(card.finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .finish()
}

/// How the whole run went: the first thing to read on its page.
fn summary_card(view: &TeamsView, theme: &Theme, team: &Team) -> Option<Box<dyn Element>> {
    if team.tasks.is_empty() && team.outcome.is_none() {
        return None;
    }
    let ui = view.ui_font;
    let finished = matches!(team.status, TeamStatus::Done | TeamStatus::Stopped);
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(semibold(ui, "Summary", 13.5, theme.main_text()))
        .with_child(medium(ui, summary::standing(team), 13.0, theme.main_text()));
    if let Some(outcome) = team.outcome.as_deref() {
        body = body.with_child(prose(view, theme, outcome.trim(), 13.0));
    } else if let Some((preview, more)) = view
        .replies
        .lead
        .as_deref()
        .filter(|_| finished)
        .and_then(summary::lead_preview)
    {
        let open = view.expanded.contains(LEAD_DETAILS);
        let message = view.replies.lead.as_deref().unwrap_or_default().trim();
        body = body
            .with_child(label(ui, "The lead's last message", 11.5, theme.disabled_text()))
            .with_child(if open { prose(view, theme, message, 13.0) } else { label(ui, &preview, 13.0, theme.sub_text()) });
        if more {
            body = body.with_child(left(details_button(view, theme, LEAD_DETAILS, open, "full message")));
        }
    }
    let (files, checks) = summary::totals(team);
    let mut counts = Vec::new();
    if files > 0 {
        counts.push(format!("{files} {} changed", if files == 1 { "file" } else { "files" }));
    }
    if checks > 0 {
        counts.push(format!("{checks} {} run", if checks == 1 { "check" } else { "checks" }));
    }
    if !counts.is_empty() {
        body = body.with_child(label(ui, &counts.join(" · "), 12.0, theme.disabled_text()));
    }
    let open_tasks: Vec<&Task> = team
        .tasks
        .iter()
        .filter(|task| task.status != TaskStatus::Done)
        .collect();
    if finished && !open_tasks.is_empty() {
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(medium(ui, "Not finished", 12.0, theme.main_text()));
        for task in open_tasks {
            list = list.with_child(label(
                ui,
                &format!("{}: {}", task.title, summary::why_unfinished(task)),
                12.0,
                theme.sub_text(),
            ));
        }
        body = body.with_child(list.finish());
    }
    Some(
        Container::new(body.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .with_uniform_padding(14.0)
            .finish(),
    )
}

/// Keeps a button its own size at the start of a column.
fn left(child: Box<dyn Element>) -> Box<dyn Element> {
    Flex::row().with_child(child).finish()
}

fn details_button(view: &TeamsView, theme: &Theme, key: &str, open: bool, what: &str) -> Box<dyn Element> {
    button(
        theme,
        view.ui_font,
        view.view_id,
        view.mouse(&format!("details-{key}")),
        Some(if open { AppIcon::ChevronUp } else { AppIcon::ChevronDown }),
        &format!("{} {what}", if open { "Hide" } else { "Show" }),
        ButtonKind::Ghost,
        Some(TeamsAction::ToggleDetails(key.to_string())),
    )
}

/// One labelled part of a task's details.
fn detail_section(view: &TeamsView, theme: &Theme, title: &str, body: Box<dyn Element>) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0)
        .with_child(label(view.ui_font, title, 11.5, theme.disabled_text()))
        .with_child(body)
        .finish()
}

/// Everything behind "Show details": the raw report, and what it's based on.
fn task_details(view: &TeamsView, theme: &Theme, task: &Task) -> Box<dyn Element> {
    let ui = view.ui_font;
    let text = |value: &str| prose(view, theme, value.trim(), 12.5);
    let list = |items: &[String], font| {
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(2.0);
        for item in items.iter().take(DETAIL_LIST_LIMIT) {
            column = column.with_child(label(font, item, 11.5, theme.sub_text()));
        }
        if items.len() > DETAIL_LIST_LIMIT {
            column = column.with_child(label(
                ui,
                &format!("and {} more", items.len() - DETAIL_LIST_LIMIT),
                11.5,
                theme.disabled_text(),
            ));
        }
        column.finish()
    };
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(11.0);
    if let Some(result) = &task.result {
        let reported = result.summary.trim();
        if summary::headline(reported, task.branch.as_deref()) != reported {
            column = column.with_child(detail_section(view, theme, "What the agent reported", text(reported)));
        }
        if let Some(notes) = result.details.as_deref().filter(|notes| !notes.trim().is_empty()) {
            column = column.with_child(detail_section(view, theme, "Technical notes", text(notes)));
        }
        if !result.checks.is_empty() {
            column = column.with_child(detail_section(view, theme, "Checks", list(&result.checks, ui)));
        }
        if !result.files.is_empty() {
            column = column.with_child(detail_section(
                view,
                theme,
                &format!("Files changed ({})", result.files.len()),
                list(&result.files, view.mono_font),
            ));
        }
    }
    if let Some(reply) = view.replies.tasks.get(&task.id) {
        column = column.with_child(detail_section(view, theme, "The agent's last message", text(reply)));
    }
    if !task.prompt.trim().is_empty() {
        column = column.with_child(detail_section(view, theme, "What it was asked to do", text(&task.prompt)));
    }
    if let Some(branch) = &task.branch {
        column = column.with_child(detail_section(view, theme, "Branch", one_line(view.mono_font, branch, 11.5, theme.sub_text())));
    }
    Container::new(column.finish())
        .with_background_color(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_uniform_padding(12.0)
        .with_margin_top(6.0)
        .finish()
}

fn task_row(view: &TeamsView, theme: &Theme, team: &Team, task: &Task) -> Box<dyn Element> {
    let ui = view.ui_font;
    let worker = view.workers.get(&task.id).copied();
    let (icon, color, state) = task_style(theme, team, task, worker);
    // A worker put to sleep is not failed or stuck: it wakes on request.
    let asleep_pane = task
        .worker
        .as_ref()
        .and_then(|worker| worker.pane.clone())
        .filter(|_| view.asleep_workers.contains(&task.id));
    let (state, color) = if asleep_pane.is_some() {
        ("asleep, wakes on request".to_string(), theme.sub_text())
    } else {
        (state, color)
    };
    let mut meta: Vec<String> = vec![task.id.clone()];
    let model = task.attempt_model.as_deref().or(task.model.as_deref());
    match (&task.agent, model) {
        (Some(agent), Some(model)) => meta.push(format!("{} · {model}", agent_label(agent))),
        (Some(agent), None) => meta.push(agent_label(agent)),
        _ => meta.push("no agent yet".into()),
    }
    if let Some(target) = task.review_of.as_ref().filter(|_| task.qa) {
        meta.push(format!("QA: tests {target} as a whole"));
    } else if task.tests_integrated() {
        meta.push("QA: tests the integrated work".into());
    } else if let Some(target) = &task.review_of {
        meta.push(format!("reviews {target}"));
    } else if !task.depends_on.is_empty() {
        meta.push(format!("after {}", task.depends_on.join(", ")));
    }
    if task.attempts > 1 {
        meta.push(format!("attempt {}", task.attempts));
    }
    if let Some(duration) = summary::duration(task) {
        meta.push(duration);
    }
    let outside = task.outside_area();
    if !outside.is_empty() {
        meta.push(format!("changed outside its area: {}", outside.join(", ")));
    }
    let mut end = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0)
        .with_child(
            Container::new(one_line(ui, &state, 11.5, color))
                .with_padding_right(4.0)
                .finish(),
        );
    if task
        .worker
        .as_ref()
        .and_then(|worker| worker.pane.as_ref())
        .is_some()
    {
        end = end.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse(&format!("open-{}", task.id)),
            Some(AppIcon::Terminal),
            "",
            ButtonKind::Ghost,
            Some(TeamsAction::OpenWorker(task.id.clone())),
        ));
    }
    if let Some(pane) = asleep_pane {
        end = end.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse(&format!("wake-{}", task.id)),
            None,
            "Wake",
            ButtonKind::Secondary,
            Some(TeamsAction::Wake(pane)),
        ));
    }
    if !task.status.is_finished()
        && matches!(team.status, TeamStatus::Planning | TeamStatus::Running)
    {
        end = end.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse(&format!("cancel-{}", task.id)),
            Some(AppIcon::XClose),
            "",
            ButtonKind::Ghost,
            Some(TeamsAction::CancelTask(task.id.clone())),
        ));
    }
    // What this task is, or what came of it, in a line a person can read.
    let line = match (&task.reason, &task.result) {
        (Some(reason), _)
            if !task.status.is_finished()
                || matches!(task.status, TaskStatus::Failed | TaskStatus::NeedsDecision) =>
        {
            summary::plain(reason)
        }
        (_, Some(result)) => summary::headline(&result.summary, task.branch.as_deref()),
        _ => summary::brief(&task.prompt),
    };
    let reported = task.result.is_some();
    let title = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(11.0)
        .with_child(render_icon(icon, color, 15.0))
        .with_child(Expanded::new(1.0, medium(ui, &task.title, 13.5, theme.main_text())).finish())
        .with_child(end.finish())
        .finish();
    let mut text = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.0);
    if !line.is_empty() {
        text = text.with_child(label(
            ui,
            &line,
            12.5,
            if reported { theme.main_text() } else { theme.sub_text() },
        ));
    }
    let mut footer = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(5.0);
    if let Some(agent) = task.agent.as_deref() {
        footer = footer.with_child(agent_icon(theme, agent, 12.0));
    }
    footer = footer.with_child(
        Expanded::new(1.0, one_line(ui, &meta.join(" · "), 11.0, theme.disabled_text())).finish(),
    );
    // The model can only change for work that hasn't finished.
    if !task.status.is_finished()
        && task.agent.as_deref().and_then(crate::automations::executor::prompt_agent).is_some()
    {
        let model_button = button(
            theme,
            ui,
            view.view_id,
            view.mouse(&format!("model-{}", task.id)),
            Some(AppIcon::ChevronDown),
            &format!(
                "Model: {}{}",
                if task.model_locked {
                    task.user_model.as_deref().unwrap_or("Agent default")
                } else {
                    task.model.as_deref().unwrap_or("Agent default")
                },
                if task.model_locked { " · Your choice" } else { "" }
            ),
            ButtonKind::Ghost,
            Some(TeamsAction::ChooseTaskModel(task.id.clone())),
        );
        footer = footer.with_child(if view.model_task.as_deref() == Some(task.id.as_str()) {
            SavePosition::new(model_button, "team_task_model").finish()
        } else {
            model_button
        });
    }
    let open = view.expanded.contains(&task.id);
    let has_details = task.result.is_some() || !task.prompt.trim().is_empty();
    if has_details {
        footer = footer.with_child(details_button(view, theme, &task.id, open, "details"));
    }
    text = text.with_child(footer.finish());
    if open && has_details {
        text = text.with_child(task_details(view, theme, task));
    }
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(title)
            // Under the title, past the status icon.
            .with_child(Container::new(text.finish()).with_padding_left(26.0).finish())
            .finish(),
    )
    .with_padding_left(14.0)
    .with_padding_right(10.0)
    .with_padding_top(12.0)
    .with_padding_bottom(10.0)
    .finish()
}

// ── Facts ────────────────────────────────────────────────────────────────────

/// The facts about it: a column beside the plan, or rows under the header
/// when the page is narrow.
fn facts(view: &TeamsView, theme: &Theme, team: &Team, rows: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let at_work = matches!(team.status, TeamStatus::Planning | TeamStatus::Running);
    let mut entries: Vec<(&str, Box<dyn Element>)> = Vec::new();
    if let Some(lead) = team.lead.as_ref().and_then(|lead| lead.agent.as_deref()) {
        let (state, color) = if view.lead.asleep.is_some() {
            ("asleep, wakes on request", theme.sub_text())
        } else {
            lead_word(theme, view.lead.state)
        };
        entries.push((
            "Lead",
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(agent_icon(theme, lead, 14.0))
                .with_child(medium(ui, agent_label(lead), 12.0, theme.main_text()))
                .with_child(dot(color, 6.0))
                .with_child(label(ui, state, 11.5, color))
                .finish(),
        ));
    }
    let mut agents = Flex::row().with_spacing(5.0);
    if team.roster.is_empty() {
        agents = agents.with_child(label(ui, "Any installed agent", 12.0, theme.main_text()));
    }
    for entry in &team.roster {
        agents = agents.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(5.0)
                    .with_child(agent_icon(theme, &entry.agent, 12.0))
                    .with_child(label(ui, &agent_label(&entry.agent), 11.5, theme.main_text()))
                    .finish(),
            )
            .with_background_color(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(7.0)
            .with_padding_right(7.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish(),
        );
    }
    entries.push(("Agents", agents.finish()));
    entries.push((
        "Models",
        label(
            ui,
            match team.cost {
                Cost::Cheapest => "Cheapest that can do it",
                Cost::Balanced => "Balanced",
                Cost::Best => "Strongest",
            },
            12.0,
            theme.main_text(),
        ),
    ));
    let limit = team.max_parallel_per_agent;
    let mut at_once = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0);
    if at_work {
        at_once = at_once.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("fewer"),
            None,
            "−",
            ButtonKind::Secondary,
            (limit > 1).then_some(TeamsAction::SetParallel(limit - 1)),
        ));
    }
    at_once = at_once.with_child(label(
        ui,
        &format!("{limit} per agent"),
        12.0,
        theme.main_text(),
    ));
    if at_work {
        at_once = at_once.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("more"),
            None,
            "+",
            ButtonKind::Secondary,
            (limit < 10).then_some(TeamsAction::SetParallel(limit + 1)),
        ));
    }
    entries.push(("At once", at_once.finish()));
    // Once the plan is going, a task starts when what it waited for is
    // done, unless the person wants the lead to start each one.
    let mut next = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(label(
            ui,
            if team.auto_start {
                "Start on their own"
            } else {
                "Wait for the lead"
            },
            12.0,
            theme.main_text(),
        ));
    if at_work {
        next = next.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("auto-start"),
            None,
            if team.auto_start { "Turn off" } else { "Turn on" },
            ButtonKind::Ghost,
            Some(TeamsAction::SetAutoStart(!team.auto_start)),
        ));
    }
    entries.push(("Next tasks", next.finish()));
    // Who answers workers' permission prompts: the lead (case by case, on
    // the record) or only the person.
    let mut prompts = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(label(
            ui,
            if team.lead_answers_prompts {
                "The lead answers"
            } else {
                "You answer"
            },
            12.0,
            theme.main_text(),
        ));
    if at_work {
        prompts = prompts.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("lead-answers"),
            None,
            if team.lead_answers_prompts { "Turn off" } else { "Turn on" },
            ButtonKind::Ghost,
            Some(TeamsAction::SetLeadAnswers(!team.lead_answers_prompts)),
        ));
    }
    entries.push(("Permission prompts", prompts.finish()));
    if !team.tasks.is_empty() {
        entries.push((
            "Progress",
            ConstrainedBox::new(progress_bar(theme, team))
                .with_max_width(RAIL_WIDTH)
                .finish(),
        ));
    }
    entries.push(("Machine", one_line(ui, &crate::remote_fs::machine_name(std::path::Path::new(&team.project_root)).unwrap_or_else(||"This computer".into()), 11.5, theme.main_text())));
    entries.push((
        "Project",
        one_line(
            ui,
            &crate::terminal::render::block_list_header::display_cwd(std::path::Path::new(
                &team.project_root,
            )),
            11.5,
            theme.main_text(),
        ),
    ));

    if rows {
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0);
        for (name, value) in entries {
            column = column.with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(12.0)
                    .with_child(
                        ConstrainedBox::new(label(ui, name, 12.0, theme.sub_text()))
                            .with_width(84.0)
                            .finish(),
                    )
                    .with_child(Expanded::new(1.0, value).finish())
                    .finish(),
            );
        }
        return column.finish();
    }
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(18.0);
    for (name, value) in entries {
        column = column.with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(7.0)
                .with_child(label(ui, name, 11.5, theme.sub_text()))
                .with_child(value)
                .finish(),
        );
    }
    column.finish()
}

/// One segment per task, in its state's colour.
fn progress_bar(theme: &Theme, team: &Team) -> Box<dyn Element> {
    let mut bar = Flex::row().with_spacing(3.0);
    for task in &team.tasks {
        let (_, color, _) = task_style(theme, team, task, None);
        let color = match task.status {
            TaskStatus::Pending => theme.surface_3(),
            _ => color,
        };
        bar = bar.with_child(
            Expanded::new(
                1.0,
                ConstrainedBox::new(
                    Container::new(Empty::new().finish())
                        .with_background_color(color)
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                        .finish(),
                )
                .with_height(5.0)
                .finish(),
            )
            .finish(),
        );
    }
    bar.finish()
}

// ── Activity ─────────────────────────────────────────────────────────────────

fn party_label(party: &Party) -> String {
    match party {
        Party::Lead => "The lead".into(),
        Party::User => "You".into(),
        Party::Task(id) => format!("Worker {id}"),
        Party::Dragent => "Dragent".into(),
    }
}

/// One stop on the timeline: its mark, what happened, and (for a message
/// or a result) its words in a card.
struct Stop {
    icon: AppIcon,
    color: ColorU,
    who: String,
    what: String,
    at_ms: i64,
    card: Option<String>,
}

fn stop_for(theme: &Theme, team: &Team, event: &TeamEvent) -> Option<Stop> {
    let task = |id: &str| team.task(id);
    let named = |id: &str| {
        task(id)
            .map(|task| format!("{id} {}", task.title))
            .unwrap_or_else(|| id.to_string())
    };
    let stop = |icon, color, who: String, what: String, card: Option<String>| {
        Some(Stop {
            icon,
            color,
            who,
            what,
            at_ms: event.at_ms,
            card,
        })
    };
    match &event.change {
        Change::Team {
            status: TeamStatus::Planning,
        } => stop(
            AppIcon::Layers,
            theme.accent,
            "The lead".into(),
            "started planning".into(),
            None,
        ),
        Change::Team {
            status: TeamStatus::Running,
        } => None,
        Change::Team {
            status: TeamStatus::Done,
        } => stop(
            AppIcon::CircleCheck,
            theme.ansi_blue(),
            "Every task".into(),
            "is finished".into(),
            None,
        ),
        Change::Team {
            status: TeamStatus::Stopped,
        } => stop(
            AppIcon::CircleX,
            theme.disabled_text(),
            "The work".into(),
            "was stopped".into(),
            None,
        ),
        Change::TaskAdded { task: id } => stop(
            AppIcon::TaskList,
            theme.sub_text(),
            "The lead".into(),
            format!("planned {}", named(id)),
            None,
        ),
        Change::TaskEdited { task: id } => stop(
            AppIcon::Sliders,
            theme.sub_text(),
            "The lead".into(),
            format!("changed {}", named(id)),
            None,
        ),
        Change::Task { task: id, status } => {
            let current = task(id);
            let agent = current
                .and_then(|task| task.agent.as_deref())
                .map(agent_label)
                .unwrap_or_else(|| "A worker".into());
            match status {
                TaskStatus::Starting | TaskStatus::Pending => None,
                TaskStatus::Running => stop(
                    AppIcon::Play,
                    theme.accent,
                    agent,
                    format!("started {}", named(id)),
                    None,
                ),
                TaskStatus::Done => stop(
                    AppIcon::CircleCheck,
                    theme.ui_green(),
                    agent,
                    format!("finished {}", named(id)),
                    current
                        .and_then(|task| task.result.as_ref())
                        .map(|result| result.summary.clone()),
                ),
                TaskStatus::Blocked => stop(
                    AppIcon::TriangleAlert,
                    theme.ui_yellow(),
                    agent,
                    format!("is blocked on {}", named(id)),
                    current.and_then(|task| task.reason.clone()),
                ),
                TaskStatus::Failed => stop(
                    AppIcon::CircleX,
                    theme.ui_red(),
                    named(id),
                    "failed".into(),
                    current.and_then(|task| task.reason.clone()),
                ),
                TaskStatus::NeedsDecision => stop(
                    AppIcon::MessageSquare,
                    theme.ui_yellow(),
                    named(id),
                    "needs a decision".into(),
                    current.and_then(|task| task.reason.clone()),
                ),
                TaskStatus::Cancelled => stop(
                    AppIcon::XClose,
                    theme.disabled_text(),
                    named(id),
                    "was cancelled".into(),
                    None,
                ),
            }
        }
        // QA of the integrated work: the lead decides what goes back.
        Change::ChangesAsked {
            review,
            task: target,
        } if target.is_empty() => stop(
            AppIcon::RotateCcw,
            theme.ui_yellow(),
            format!("QA {review}"),
            "found problems in the integrated work".into(),
            task(review)
                .and_then(|review| review.result.as_ref())
                .map(|result| result.summary.clone()),
        ),
        Change::ChangesAsked {
            review,
            task: target,
        } => stop(
            AppIcon::RotateCcw,
            theme.ui_yellow(),
            if task(review).is_some_and(|review| review.qa) {
                format!("QA {review}")
            } else {
                format!("Review {review}")
            },
            format!("sent {} back for changes", named(target)),
            None,
        ),
        // On the record: what the worker asked and what the lead pressed.
        Change::PromptAnswered {
            task: id,
            keys,
            prompt,
        } => stop(
            AppIcon::Key,
            theme.ui_yellow(),
            "The lead".into(),
            format!("answered {}'s permission prompt ({})", named(id), keys.join(" ")),
            Some(
                prompt
                    .lines()
                    .map(str::trim_end)
                    .filter(|line| !line.trim().is_empty())
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ),
        Change::AgentsAdded { agents } => stop(
            AppIcon::Plus,
            theme.accent,
            "The lead".into(),
            format!("added {} to the team", agents.join(", ")),
            None,
        ),
        Change::Reassigned {
            task: id,
            from,
            to,
        } => stop(
            AppIcon::RotateCcw,
            theme.accent,
            "The lead".into(),
            match from {
                Some(from) => format!("moved {} from {from} to {to}", named(id)),
                None => format!("moved {} to {to}", named(id)),
            },
            None,
        ),
        Change::Reseated { party, reason, .. } => stop(
            AppIcon::RotateCcw,
            theme.accent,
            match party {
                Party::Lead => "The lead".into(),
                Party::Task(id) => format!("{}'s worker", named(id)),
                other => other.label(),
            },
            match reason {
                ReseatReason::Contact | ReseatReason::Resume => {
                    "carries on in its resumed session".into()
                }
                ReseatReason::Taken => "took its seat back in another session".into(),
            },
            None,
        ),
        Change::Integrated { task: id } => stop(
            AppIcon::GitBranch,
            theme.ansi_blue(),
            "The lead".into(),
            format!("brought {} into your checkout", named(id)),
            task(id)
                .and_then(|task| task.integrated.as_ref())
                .filter(|integration| !integration.files.is_empty())
                .map(|integration| format!("Uncommitted: {}", integration.files.join(", "))),
        ),
        Change::QaNeeded => stop(
            AppIcon::Monitor,
            theme.ui_yellow(),
            "Dragent".into(),
            "asked for a QA pass: the work isn't done until the whole feature is tested".into(),
            None,
        ),
        Change::Message { id, .. } => {
            let message = team.messages.iter().find(|message| message.id == *id)?;
            let to = match &message.to {
                Party::User => "you".to_string(),
                other => party_label(other).to_lowercase(),
            };
            let kind = match message.kind {
                MessageKind::Note => "wrote to",
                MessageKind::Question => "asked",
                MessageKind::Answer => "answered",
                MessageKind::Handoff => "handed off to",
                MessageKind::Finding => "told",
                MessageKind::NeedsUser => "needs",
            };
            stop(
                AppIcon::MessageSquare,
                if message.to == Party::User {
                    theme.accent
                } else {
                    theme.sub_text()
                },
                party_label(&message.from),
                format!("{kind} {to}"),
                Some(message.body.clone()),
            )
        }
    }
}

fn activity(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let ui = view.ui_font;
    let stops: Vec<Stop> = team
        .events
        .iter()
        .filter_map(|event| stop_for(theme, team, event))
        .collect();
    let start = stops.len().saturating_sub(120);
    let stops = &stops[start..];
    if stops.is_empty() {
        return label(ui, "Nothing has happened yet.", 12.5, theme.sub_text());
    }
    // The rail is the left border of the list; each stop's mark sits on it.
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0);
    for stop in stops {
        let node = Container::new(render_icon(stop.icon, stop.color, 12.0))
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(11.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_margin_left(-11.5)
            .finish();
        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(6.0)
                        .with_child(medium(ui, &stop.who, 12.0, theme.main_text()))
                        .with_child(
                            Expanded::new(1.0, one_line(ui, &stop.what, 12.0, theme.sub_text()))
                                .finish(),
                        )
                        .with_child(label(ui, &ago(stop.at_ms), 11.0, theme.disabled_text()))
                        .finish(),
                )
                .with_padding_top(3.0)
                .finish(),
            );
        if let Some(card) = stop
            .card
            .as_deref()
            .map(str::trim)
            .filter(|card| !card.is_empty())
        {
            content = content.with_child(
                Container::new(prose(view, theme, card, 12.5))
                    .with_background_color(theme.surface_1())
                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(9.0)
                    .with_padding_bottom(10.0)
                    .finish(),
            );
        }
        list = list.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(12.0)
                .with_child(node)
                .with_child(Expanded::new(1.0, content.finish()).finish())
                .finish(),
        );
    }
    let list = Container::new(list.finish())
        .with_border(Border::left(1.0).with_border_color(theme.outline()))
        .with_margin_left(11.5)
        .finish();
    ConstrainedBox::new(list).with_max_width(820.0).finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> Team {
        let task = |id: &str, status: &str, extra: serde_json::Value| {
            let mut task = serde_json::json!({
                "id": id, "title": id, "prompt": "", "kind": "work", "status": status,
                "created_at_ms": 0, "updated_at_ms": 0,
            });
            for (key, value) in extra.as_object().unwrap() {
                task[key] = value.clone();
            }
            task
        };
        serde_json::from_value(serde_json::json!({
            "id": "team-1", "title": "T", "project_root": "/w/app", "status": "running",
            "created_at_ms": 0, "updated_at_ms": 0,
            "tasks": [
                task("t1", "done", serde_json::json!({})),
                task("t2", "pending", serde_json::json!({"kind": "review", "review_of": "t1", "depends_on": ["t1"],
                    "result": {"summary": "fix it", "verdict": "changes"}})),
                task("t3", "pending", serde_json::json!({"depends_on": ["t2"]})),
                task("t4", "running", serde_json::json!({})),
                task("t5", "pending", serde_json::json!({})),
            ],
            "messages": [
                {"id": 1, "from": "lead", "to": "user", "kind": "needs_user", "body": "Resend or SES?", "at_ms": 0},
                {"id": 2, "from": "user", "to": "lead", "kind": "answer", "body": "Resend", "at_ms": 0},
                {"id": 3, "from": "lead", "to": "user", "kind": "question", "body": "Ship it?", "at_ms": 0},
            ],
            "events": [
                {"revision": 1, "at_ms": 0, "change": {"team": {"status": "planning"}}},
                {"revision": 2, "at_ms": 0, "change": {"task_added": {"task": "t1"}}},
                {"revision": 3, "at_ms": 0, "change": {"task": {"task": "t1", "status": "starting"}}},
                {"revision": 4, "at_ms": 0, "change": {"task": {"task": "t1", "status": "done"}}},
                {"revision": 5, "at_ms": 0, "change": {"changes_asked": {"review": "t2", "task": "t1"}}},
                {"revision": 6, "at_ms": 0, "change": {"message": {"id": 1, "to": "user"}}},
            ],
        }))
        .unwrap()
    }

    #[test]
    fn each_task_says_where_it_stands() {
        let theme = Theme::dark();
        let team = team();
        let style = |id: &str, worker| task_style(&theme, &team, team.task(id).unwrap(), worker).2;
        assert_eq!(style("t1", None), "done");
        assert_eq!(
            style("t2", None),
            "asked for changes; looks again after the fix"
        );
        assert_eq!(style("t3", None), "waits for t2");
        assert_eq!(
            style("t4", Some(WorkerState::NeedsApproval)),
            "waiting for your approval"
        );
        assert_eq!(style("t4", Some(WorkerState::Working)), "working");
        assert_eq!(style("t5", None), "ready to start");
    }

    #[test]
    fn a_question_is_open_until_the_person_answers() {
        let team = team();
        assert!(!unanswered(&team, &team.messages[0]), "answered by #2");
        assert!(unanswered(&team, &team.messages[2]));
        assert_eq!(project_name("/w/app"), "app");
        assert_eq!(
            crate::teams::team_from_alert("dragent-team:team-1"),
            Some("team-1")
        );
        assert_eq!(crate::teams::team_from_alert("dragent-team:"), None);
        assert_eq!(
            crate::teams::team_from_alert("dragent-automation-run:x"),
            None
        );
    }

    #[test]
    fn the_timeline_tells_what_happened_in_words() {
        let theme = Theme::dark();
        let team = team();
        let stops: Vec<(String, String)> = team
            .events
            .iter()
            .filter_map(|event| stop_for(&theme, &team, event))
            .map(|stop| (stop.who, stop.what))
            .collect();
        assert_eq!(
            stops,
            [
                ("The lead".to_string(), "started planning".to_string()),
                ("The lead".into(), "planned t1 t1".into()),
                ("A worker".into(), "finished t1 t1".into()),
                ("Review t2".into(), "sent t1 t1 back for changes".into()),
                ("The lead".into(), "needs you".into()),
            ]
        );
        assert!(
            shows(ListFilter::Active, TeamStatus::Planning)
                && !shows(ListFilter::Active, TeamStatus::Done)
        );
        assert!(
            shows(ListFilter::Finished, TeamStatus::Stopped)
                && shows(ListFilter::All, TeamStatus::Running)
        );
    }
}

/// "Board · 2 need you" when something waits on the person: the same
/// tasks as the board's Needs you column.
fn board_label(view: &TeamsView, team: &Team) -> String {
    let waiting = team
        .tasks
        .iter()
        .filter(|task| super::board::column_of(task, view.workers.get(&task.id).copied()) == Some(super::board::Column::NeedsYou))
        .count();
    match waiting {
        0 => "Board".into(),
        1 => "Board · 1 needs you".into(),
        n => format!("Board · {n} need you"),
    }
}

/// The Board tab: the header, then the columns filling the rest of the
/// page (each scrolls on its own, the board sideways), the message box to
/// the lead below when it has asked something.
fn board_pane(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let at_work = matches!(team.status, TeamStatus::Planning | TeamStatus::Running);
    let question = open_question(view, team);
    let mut top = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0)
        .with_child(header(view, theme, team));
    for notice in notices(view, theme, team) {
        top = top.with_child(notice);
    }
    // What waits on the person comes first, answerable with a click.
    if at_work {
        for card in approval_cards(view, theme, team) {
            top = top.with_child(card);
        }
        if let Some(question) = question {
            top = top.with_child(question_card(view, theme, question, true));
        }
        if let Some(said) = lead_waiting(view, team) {
            top = top.with_child(lead_waiting_card(view, theme, team, said, true));
        }
    }
    let page = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(top_bar(view, theme, team))
        .with_child(rule(theme, false))
        .with_child(
            Container::new(top.finish())
                .with_padding_left(28.0)
                .with_padding_right(28.0)
                .with_padding_top(22.0)
                .with_padding_bottom(18.0)
                .finish(),
        )
        .with_child(Expanded::new(1.0, super::board::board(view, theme, team)).finish());
    page.finish()
}
