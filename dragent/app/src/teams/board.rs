//! The coordination's Board: its tasks as cards in columns by where they
//! are (up next, working, in review, needs you, done). Cards move on their
//! own as the lead starts tasks, workers finish and reviews come back: the
//! columns follow each task's state, so nobody drags anything. The person
//! adds tasks from the first column; the lead is told and they start like
//! any other once what they wait for is done.

use dragent_host::team::{Task, TaskKind, TaskStatus, Team, TeamStatus};
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, MainAxisSize,
    ParentElement, Wrap,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::page::{agent_icon, agent_label, task_style};
use super::{summary, TeamsAction, TeamsView, WorkerState};
use crate::automations::widgets::{button, field, label, one_line, ButtonKind};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{medium, semibold};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Column {
    UpNext,
    Working,
    Reviewing,
    NeedsYou,
    Done,
}

const COLUMNS: [Column; 5] = [Column::UpNext, Column::Working, Column::Reviewing, Column::NeedsYou, Column::Done];

/// Where a task sits on the board; cancelled ones aren't shown.
pub(crate) fn column_of(task: &Task, worker: Option<WorkerState>) -> Option<Column> {
    Some(match task.status {
        TaskStatus::Pending => Column::UpNext,
        TaskStatus::Starting | TaskStatus::Running if worker == Some(WorkerState::NeedsApproval) => Column::NeedsYou,
        TaskStatus::Starting | TaskStatus::Running if task.kind == TaskKind::Review => Column::Reviewing,
        TaskStatus::Starting | TaskStatus::Running => Column::Working,
        TaskStatus::Blocked | TaskStatus::Failed | TaskStatus::NeedsDecision => Column::NeedsYou,
        TaskStatus::Done => Column::Done,
        TaskStatus::Cancelled => return None,
    })
}

fn column_title(column: Column) -> &'static str {
    match column {
        Column::UpNext => "Up next",
        Column::Working => "Working",
        Column::Reviewing => "In review",
        Column::NeedsYou => "Needs you",
        Column::Done => "Done",
    }
}

fn column_color(theme: &Theme, column: Column) -> ColorU {
    match column {
        Column::UpNext => theme.sub_text(),
        Column::Working => theme.accent,
        Column::Reviewing => theme.ansi_blue(),
        Column::NeedsYou => theme.ui_yellow(),
        Column::Done => theme.ui_green(),
    }
}

/// The Tasks board's measures, so both boards look alike.
const COLUMN_WIDTH: f32 = 280.0;
const GAP: f32 = 12.0;

/// The board: fixed-width columns that fill the height, each scrolling on
/// its own; the board scrolls sideways when they don't all fit.
pub(super) fn board(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(GAP);
    for (index, column) in COLUMNS.into_iter().enumerate() {
        row = row.with_child(ConstrainedBox::new(column_panel(view, theme, team, column, index)).with_width(COLUMN_WIDTH).finish());
    }
    warpui::elements::ClippedScrollable::horizontal(
        view.board_scroll.clone(),
        Container::new(row.finish())
            .with_padding_left(24.0)
            .with_padding_right(24.0)
            .with_padding_bottom(16.0)
            .finish(),
        warpui::elements::ScrollbarWidth::Auto,
        theme.outline().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

/// A column's mark: a dot in its colour, a tick when done.
fn column_mark(theme: &Theme, column: Column, size: f32) -> Box<dyn Element> {
    let color = column_color(theme, column);
    let mark = match column {
        Column::Done => render_icon(AppIcon::CircleCheck, color, size),
        Column::NeedsYou => render_icon(AppIcon::TriangleAlert, color, size),
        _ => warpui::elements::Align::new(dot(color, size * 0.62)).finish(),
    };
    ConstrainedBox::new(mark).with_width(size).with_height(size).finish()
}

fn column_panel(view: &TeamsView, theme: &Theme, team: &Team, column: Column, index: usize) -> Box<dyn Element> {
    let ui = view.ui_font;
    let tasks: Vec<&Task> = team
        .tasks
        .iter()
        .filter(|task| column_of(task, view.workers.get(&task.id).copied()) == Some(column))
        .collect();
    let heading = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(column_mark(theme, column, 13.0))
            .with_child(medium(ui, column_title(column), 12.5, theme.main_text()))
            .with_child(label(ui, &tasks.len().to_string(), 12.0, theme.sub_text()))
            .finish(),
    )
    .with_padding_left(4.0)
    .with_padding_top(4.0)
    .with_padding_bottom(10.0)
    .finish();

    let mut cards = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(8.0);
    let at_work = matches!(team.status, TeamStatus::Planning | TeamStatus::Running);
    // While the lead writes the plan, a card says so with the app's
    // starting loader: a light lapping its edge, the lead's face glowing.
    if column == Column::UpNext && team.status == TeamStatus::Planning && team.tasks.is_empty() {
        cards = cards.with_child(planning_card(view, theme, team));
    }
    if column == Column::UpNext && at_work {
        cards = cards.with_child(if view.adding_task { add_form(view, theme, team) } else { add_button(view, theme) });
    }
    for task in &tasks {
        cards = cards.with_child(card(view, theme, team, task, column));
    }
    if tasks.is_empty() && !(column == Column::UpNext && at_work) {
        cards = cards.with_child(
            Container::new(label(ui, empty_words(column, team), 12.0, theme.disabled_text()))
                .with_padding_left(4.0)
                .with_padding_top(2.0)
                .finish(),
        );
    }
    let scrolled = warpui::elements::ClippedScrollable::vertical(
        view.column_scrolls[index].clone(),
        Container::new(cards.finish()).with_padding_bottom(40.0).finish(),
        warpui::elements::ScrollbarWidth::Auto,
        theme.outline().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_overlayed_scrollbar()
    .finish();
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(heading)
            .with_child(Expanded::new(1.0, scrolled).finish())
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(8.0)
    .finish()
}

/// "The lead is writing the plan": the lead's face, glowing in turn, and two
/// faint lines where the tasks will be, inside an edge that a light laps.
pub(super) fn planning_card(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let ui = view.ui_font;
    let lead = team.lead.as_ref().and_then(|lead| lead.agent.as_deref());
    let face = Box::new(crate::workspace::workspace_activity::FacePulse::new(
        agent_badge(theme, lead),
        0,
        1,
        theme.accent,
    ));
    let who = lead.map(super::page::agent_label).unwrap_or_else(|| "The lead".to_string());
    let line = |width: f32| -> Box<dyn Element> {
        ConstrainedBox::new(
            Container::new(warpui::elements::Empty::new().finish())
                .with_background_color(theme.surface_3())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                .finish(),
        )
        .with_width(width)
        .with_height(6.0)
        .finish()
    };
    let body = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(10.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(face)
                    .with_child(medium(ui, &format!("{who} is writing the plan"), 12.5, theme.main_text()))
                    .finish(),
            )
            .with_child(line(150.0))
            .with_child(line(104.0))
            .with_child(label(ui, "Tasks land here as it decides them.", 11.0, theme.disabled_text()))
            .finish(),
    )
    .with_background_color(theme.background)
    .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(10.0)
    .with_padding_bottom(10.0)
    .finish();
    Box::new(crate::workspace::workspace_activity::EdgeLap::new(body, theme.accent, 8.0))
}

fn empty_words(column: Column, team: &Team) -> &'static str {
    match column {
        Column::UpNext if team.tasks.is_empty() => "The lead's plan shows here.",
        Column::UpNext => "Nothing waiting.",
        Column::Working => "No worker at work.",
        Column::Reviewing => "No review running.",
        Column::NeedsYou => "Nothing for you.",
        Column::Done => "Nothing finished yet.",
    }
}

/// The agent's mark in a small circle, where the Tasks board shows who.
fn agent_badge(theme: &Theme, agent: Option<&str>) -> Box<dyn Element> {
    let mark = match agent {
        Some(agent) => agent_icon(theme, agent, 12.0),
        None => render_icon(AppIcon::Agent, theme.disabled_text(), 12.0),
    };
    ConstrainedBox::new(
        Container::new(warpui::elements::Align::new(mark).finish())
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish(),
    )
    .with_width(20.0)
    .with_height(20.0)
    .finish()
}

/// A small outlined chip, as the Tasks board's labels.
fn outline_chip(ui: warpui::fonts::FamilyId, theme: &Theme, text: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(one_line(ui, text, 11.0, color))
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish()
}

/// One task.
fn card(view: &TeamsView, theme: &Theme, team: &Team, task: &Task, column: Column) -> Box<dyn Element> {
    let ui = view.ui_font;
    let worker = view.workers.get(&task.id).copied();
    let (_, state_color, state) = task_style(theme, team, task, worker);

    // Top: its state's mark, the id, the kind; who does it on the right.
    let mut meta = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(ConstrainedBox::new(warpui::elements::Align::new(dot(state_color, 7.0)).finish()).with_width(14.0).with_height(14.0).finish())
        .with_child(one_line(view.mono_font, &task.id, 11.5, theme.sub_text()));
    if task.kind == TaskKind::Review {
        meta = meta.with_child(tag(theme, ui, if task.qa || task.tests_integrated() { "QA" } else { "Review" }));
    }
    meta = meta
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(agent_badge(theme, task.agent.as_deref()));

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(meta.finish())
        .with_child(label(ui, &task.title, 13.0, theme.main_text()));

    // Why it waits on the person, or what it achieved.
    let line = match task.status {
        TaskStatus::Done => task.result.as_ref().map(|r| r.summary.clone()),
        TaskStatus::Blocked | TaskStatus::Failed | TaskStatus::NeedsDecision => task.reason.clone(),
        _ => None,
    };
    if let Some(line) = line.filter(|l| !l.trim().is_empty()) {
        body = body.with_child(label(ui, &truncate(&super::summary::plain(&line), 140), 12.0, theme.sub_text()));
    }

    // Chips: its state, the model, how long; then what can be done.
    let mut chips = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
    chips = chips.with_child(outline_chip(ui, theme, short_state(&state), state_color));
    if let Some(model) = task.attempt_model.as_deref().or(task.model.as_deref()) {
        chips = chips.with_child(outline_chip(ui, theme, &truncate(model, 16), theme.sub_text()));
    }
    if let Some(duration) = summary::duration(task) {
        chips = chips.with_child(outline_chip(ui, theme, &duration, theme.sub_text()));
    }
    chips = chips.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    let has_worker = task.worker.as_ref().and_then(|w| w.pane.as_ref()).is_some();
    if has_worker && matches!(column, Column::Working | Column::Reviewing | Column::NeedsYou) {
        chips = chips.with_child(icon_button(view, theme, &format!("board-open-{}", task.id), AppIcon::Terminal, TeamsAction::OpenWorker(task.id.clone())));
    }
    let ready = team.ready_tasks().iter().any(|t| t.id == task.id);
    if column == Column::UpNext && ready && !team.auto_start {
        chips = chips.with_child(icon_button(view, theme, &format!("board-start-{}", task.id), AppIcon::Play, TeamsAction::StartTask(task.id.clone())));
    }
    if matches!(column, Column::UpNext | Column::NeedsYou) {
        chips = chips.with_child(icon_button(view, theme, &format!("board-cancel-{}", task.id), AppIcon::XClose, TeamsAction::CancelTask(task.id.clone())));
    }
    let body = body.with_child(chips.finish()).finish();

    let attention = column == Column::NeedsYou;
    // A worker at it: the planning card's loader, in the sidebar's gold.
    let at_work = matches!(column, Column::Working | Column::Reviewing)
        && worker != Some(WorkerState::NeedsApproval);
    let theme_copy = *theme;
    let view_id = view.view_id;
    let task_id = task.id.clone();
    // Clicking a card opens its details on the Overview.
    let card = Hoverable::new(view.mouse(&format!("board-card-{}", task.id)), move |state| {
        let border = if attention {
            tint(theme_copy.ui_yellow(), if state.is_hovered() { 220 } else { 150 })
        } else if state.is_hovered() {
            theme_copy.sub_text()
        } else {
            theme_copy.outline()
        };
        Container::new(body)
            .with_background_color(theme_copy.pane_surface())
            .with_border(Border::all(1.0).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(9.0)
            .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, TeamsAction::ShowTask(task_id.clone())))
    .finish();
    if at_work {
        Box::new(crate::workspace::workspace_activity::EdgeLap::new(
            card,
            crate::workspace::worktree_status::WORKING_AMBER,
            8.0,
        ))
    } else {
        card
    }
}

/// The state words, short enough for a card.
fn short_state(state: &str) -> &str {
    match state {
        "waiting for your approval" => "approval",
        s if s.starts_with("waits for") => "waiting",
        "ready to start" => "ready",
        s => s,
    }
}

/// "+ Add task", at the top of Up next.
fn add_button(view: &TeamsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let view_id = view.view_id;
    Hoverable::new(view.mouse("board-add"), move |state| {
        let color = if state.is_hovered() { theme.main_text() } else { theme.sub_text() };
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(render_icon(AppIcon::Plus, color, 13.0))
                .with_child(label(ui, "Add task", 12.5, color))
                .finish(),
        )
        .with_border(Border::all(1.0).with_border_color(if state.is_hovered() { theme.sub_text() } else { theme.outline() }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_background_color(if state.is_hovered() { theme.pane_surface() } else { ColorU::transparent_black() })
        .with_padding_left(10.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, TeamsAction::StartAddTask))
    .finish()
}

/// The person's new task: what, who, after what.
fn add_form(view: &TeamsView, theme: &Theme, team: &Team) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mut agents = Wrap::row().with_spacing(6.0).with_run_spacing(6.0);
    let lead_agent = team.lead.as_ref().and_then(|l| l.agent.clone());
    for roster in &team.roster {
        let on = view.new_task_agent.as_ref().or(lead_agent.as_ref()) == Some(&roster.agent);
        agents = agents.with_child(chip(
            view,
            theme,
            &format!("board-agent-{}", roster.agent),
            Some(agent_icon(theme, &roster.agent, 13.0)),
            &agent_label(&roster.agent),
            on,
            TeamsAction::ChooseNewTaskAgent(roster.agent.clone()),
        ));
    }
    let mut after = Wrap::row().with_spacing(6.0).with_run_spacing(6.0);
    let mut any_after = false;
    for task in team.tasks.iter().filter(|t| t.status != TaskStatus::Cancelled && t.kind == TaskKind::Work) {
        any_after = true;
        let on = view.new_task_after.contains(&task.id);
        after = after.with_child(chip(
            view,
            theme,
            &format!("board-after-{}", task.id),
            None,
            &format!("{} {}", task.id, truncate(&task.title, 22)),
            on,
            TeamsAction::ToggleNewTaskAfter(task.id.clone()),
        ));
    }
    let caption = |text: &str| label(ui, text, 11.5, theme.sub_text());
    let mut form = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(semibold(ui, "New task", 13.0, theme.main_text()))
        .with_child(full(view, field(theme, &view.task_title_editor, 30.0)))
        .with_child(full(view, field(theme, &view.task_prompt_editor, 80.0)))
        .with_child(caption("Who does it"))
        .with_child(agents.finish());
    if any_after {
        form = form.with_child(caption("Starts after (optional)")).with_child(after.finish());
    }
    let when = if team.auto_start {
        "It starts on its own once what it waits for is done, and the lead is told."
    } else {
        "The lead is told and starts it; you can also start it from its card."
    };
    form = form
        .with_child(label(ui, when, 11.5, theme.disabled_text()))
        .with_child(
            Flex::row()
                .with_spacing(8.0)
                .with_child(button(theme, ui, view.view_id, view.mouse("board-add-save"), Some(AppIcon::Plus), "Add task", ButtonKind::Primary, (!view.adding_busy).then_some(TeamsAction::AddTask)))
                .with_child(button(theme, ui, view.view_id, view.mouse("board-add-cancel"), None, "Cancel", ButtonKind::Ghost, Some(TeamsAction::CancelAddTask)))
                .finish(),
        );
    Container::new(form.finish())
        .with_background_color(theme.pane_surface())
        .with_border(Border::all(1.0).with_border_color(theme.accent))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_uniform_padding(12.0)
        .finish()
}

/// The form's fields span its width (the column, less its padding).
fn full(view: &TeamsView, child: Box<dyn Element>) -> Box<dyn Element> {
    let _ = view;
    ConstrainedBox::new(child).with_width(COLUMN_WIDTH - 16.0 - 24.0 - 2.0).finish()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

fn chip(view: &TeamsView, theme: &Theme, key: &str, icon: Option<Box<dyn Element>>, text: &str, on: bool, action: TeamsAction) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let view_id = view.view_id;
    let text = text.to_string();
    let mut icon = icon;
    Hoverable::new(view.mouse(key), move |state| {
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(5.0);
        if let Some(icon) = icon.take() {
            row = row.with_child(icon);
        }
        row = row.with_child(label(ui, &text, 11.5, if on { theme.main_text() } else { theme.sub_text() }));
        Container::new(row.finish())
            .with_background_color(if on { theme.surface_3() } else if state.is_hovered() { theme.surface_2() } else { ColorU::transparent_black() })
            .with_border(Border::all(1.0).with_border_color(if on { theme.accent } else { theme.outline() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

fn icon_button(view: &TeamsView, theme: &Theme, key: &str, icon: AppIcon, action: TeamsAction) -> Box<dyn Element> {
    let theme = *theme;
    let view_id = view.view_id;
    Hoverable::new(view.mouse(key), move |state| {
        Container::new(render_icon(icon, if state.is_hovered() { theme.main_text() } else { theme.sub_text() }, 13.0))
            .with_background_color(if state.is_hovered() { theme.surface_3() } else { ColorU::transparent_black() })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_uniform_padding(4.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

fn tag(theme: &Theme, ui: warpui::fonts::FamilyId, text: &str) -> Box<dyn Element> {
    Container::new(label(ui, text, 10.0, theme.ansi_blue()))
        .with_background_color(tint(theme.ansi_blue(), 36))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
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

fn tint(color: ColorU, alpha: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: TaskStatus, kind: TaskKind) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1", "title": "x", "prompt": "y", "kind": "work", "status": "pending",
            "created_at_ms": 0, "updated_at_ms": 0
        }))
        .unwrap();
        task.status = status;
        task.kind = kind;
        task
    }

    #[test]
    fn tasks_land_in_the_column_their_state_says() {
        assert_eq!(column_of(&task(TaskStatus::Pending, TaskKind::Work), None), Some(Column::UpNext));
        assert_eq!(column_of(&task(TaskStatus::Running, TaskKind::Work), Some(WorkerState::Working)), Some(Column::Working));
        assert_eq!(column_of(&task(TaskStatus::Running, TaskKind::Review), None), Some(Column::Reviewing));
        assert_eq!(column_of(&task(TaskStatus::Running, TaskKind::Work), Some(WorkerState::NeedsApproval)), Some(Column::NeedsYou));
        assert_eq!(column_of(&task(TaskStatus::Blocked, TaskKind::Work), None), Some(Column::NeedsYou));
        assert_eq!(column_of(&task(TaskStatus::NeedsDecision, TaskKind::Review), None), Some(Column::NeedsYou));
        assert_eq!(column_of(&task(TaskStatus::Done, TaskKind::Work), None), Some(Column::Done));
        assert_eq!(column_of(&task(TaskStatus::Cancelled, TaskKind::Work), None), None);
    }
}
