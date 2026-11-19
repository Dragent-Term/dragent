use std::any::Any;

use dragent_tasks::{project_key, LinkKind, Priority, Status, Task};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex,
    MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, ScrollbarWidth, Stack,
};
use warpui::AppContext;
use warpui_core::elements::{AcceptedByDropTarget, Draggable, DropTarget, DropTargetData, Hoverable};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::{AssigneeFilter, Layout, Menu, Source, StatusFilter, TasksAction, TasksView};
use crate::integrations::items::abilities;
use crate::integrations::tracker::TrackerItem;
use crate::integrations::Provider;
use crate::automations::widgets::{button, label, one_line, ButtonKind};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{medium, semibold};
use crate::theme::Theme;

/// A board column's width.
const COLUMN_WIDTH: f32 = 280.0;
/// The key column in the table.
const KEY_WIDTH: f32 = 74.0;

/// A tracker's board column, where a dragged item card can land.
#[derive(Debug, Clone)]
struct ItemColumnTarget {
    status: Status,
}

impl DropTargetData for ItemColumnTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A board column, where a dragged card can land.
#[derive(Debug, Clone)]
struct ColumnTarget {
    status: Status,
}

impl DropTargetData for ColumnTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(super) fn page(view: &TasksView, _app: &AppContext) -> Box<dyn Element> {
    let theme = &view.theme;
    if view.org.is_none() {
        return Container::new(centered(view, "Sign in to see your organisation's tasks.", theme.sub_text()))
            .with_background_color(theme.background)
            .finish();
    }
    if let Some(open) = &view.open {
        let body = if open.detail.is_some() {
            ChildView::new(&view.detail_view).finish()
        } else if let Some(error) = &open.error {
            centered(view, error, theme.ui_red())
        } else {
            centered(view, "Opening the task…", theme.sub_text())
        };
        return Container::new(body).with_background_color(theme.background).finish();
    }
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(view, theme))
        .with_child(sources(view, theme))
        .with_child(if view.selected.is_empty() {
            toolbar(view, theme)
        } else {
            selection_bar(view, theme)
        });
    if let Some(error) = &view.error {
        column = column.with_child(error_line(view, theme, error));
    }
    if view.draft.is_some() {
        column = column.with_child(draft_card(view, theme));
    }
    column = column.with_child(Expanded::new(1.0, content(view, theme)).finish());
    let body = column.finish();
    let body = match &view.menu {
        Some(menu) => with_menu(view, theme, body, menu),
        None => body,
    };
    Container::new(body).with_background_color(theme.background).finish()
}

// ── Pieces ───────────────────────────────────────────────────────────────────

fn tint(color: ColorU, alpha: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, alpha)
}

fn centered(view: &TasksView, text: &str, color: ColorU) -> Box<dyn Element> {
    Align::new(label(view.ui_font, text, 13.0, color)).finish()
}

fn rule(theme: &Theme) -> Box<dyn Element> {
    ConstrainedBox::new(Container::new(Empty::new().finish()).with_background_color(theme.outline()).finish())
        .with_height(1.0)
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

pub(crate) fn status_color(theme: &Theme, status: Status) -> ColorU {
    match status {
        Status::Backlog => theme.disabled_text(),
        Status::Todo => theme.sub_text(),
        Status::InProgress => theme.ui_yellow(),
        Status::InReview => theme.accent,
        Status::Done => theme.ui_green(),
        Status::Canceled => theme.disabled_text(),
    }
}

/// A status's mark: a dot in its colour, a check when done, a cross when
/// canceled.
pub(crate) fn status_mark(theme: &Theme, status: Status, size: f32) -> Box<dyn Element> {
    let color = status_color(theme, status);
    let mark = match status {
        Status::Done => render_icon(AppIcon::CircleCheck, color, size),
        Status::Canceled => render_icon(AppIcon::CircleX, color, size),
        _ => Align::new(dot(color, size * 0.62)).finish(),
    };
    ConstrainedBox::new(mark).with_width(size).with_height(size).finish()
}

/// Urgent and high stand out; the rest stay quiet; no priority is blank.
fn priority_mark(theme: &Theme, priority: Priority) -> Box<dyn Element> {
    let mark = match priority {
        Priority::Urgent => render_icon(AppIcon::TriangleAlert, theme.ui_red(), 12.0),
        Priority::High => render_icon(AppIcon::ChevronUp, theme.ui_yellow(), 12.0),
        Priority::Medium => render_icon(AppIcon::ChevronUp, theme.sub_text(), 12.0),
        Priority::Low => render_icon(AppIcon::ChevronDown, theme.disabled_text(), 12.0),
        Priority::None => Empty::new().finish(),
    };
    ConstrainedBox::new(Align::new(mark).finish()).with_width(14.0).with_height(14.0).finish()
}

/// A member's initials in a small circle.
fn avatar(view: &TasksView, theme: &Theme, name: &str) -> Box<dyn Element> {
    let initials: String = name
        .split(|c: char| c.is_whitespace() || c == '@' || c == '.')
        .filter(|part| !part.is_empty())
        .take(2)
        .filter_map(|part| part.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    ConstrainedBox::new(
        Container::new(Align::new(medium(view.ui_font, initials, 9.5, theme.main_text())).finish())
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish(),
    )
    .with_width(20.0)
    .with_height(20.0)
    .finish()
}

fn small_pill(view: &TasksView, icon: AppIcon, text: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(render_icon(icon, color, 11.0))
            .with_child(medium(view.ui_font, text, 11.0, color))
            .finish(),
    )
    .with_background_color(tint(color, 26))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .with_padding_left(7.0)
    .with_padding_right(8.0)
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .finish()
}

/// A label chip; clicking it filters by that label.
fn label_chip(view: &TasksView, theme: &Theme, text: &str) -> Box<dyn Element> {
    let theme = *theme;
    let ui = view.ui_font;
    let view_id = view.view_id;
    let shown = text.to_string();
    let chosen = text.to_string();
    Hoverable::new(view.mouse(&format!("label-{text}")), move |state| {
        Container::new(medium(ui, shown.clone(), 11.0, if state.is_hovered() { theme.main_text() } else { theme.sub_text() }))
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_padding_left(7.0)
            .with_padding_right(7.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::SetLabel(Some(chosen.clone()))))
    .finish()
}

/// A button that opens a menu below it.
fn menu_button(view: &TasksView, theme: &Theme, menu: Menu, position: &str, text: String, mark: Option<Box<dyn Element>>) -> Box<dyn Element> {
    let theme = *theme;
    let ui = view.ui_font;
    let view_id = view.view_id;
    let open = view.menu.as_ref() == Some(&menu);
    let button = Hoverable::new(view.mouse(position), move |state| {
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
        if let Some(mark) = mark {
            row = row.with_child(mark);
        }
        Container::new(
            row.with_child(medium(ui, text.clone(), 12.0, theme.main_text()))
                .with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 10.0))
                .finish(),
        )
        .with_background_color(if open || state.is_hovered() { theme.surface_2() } else { theme.surface_1() })
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .with_padding_left(10.0)
        .with_padding_right(9.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::ToggleMenu(menu.clone())))
    .finish();
    SavePosition::new(button, position).finish()
}

fn ago(at_ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    crate::automations::compact_ago(at_ms, now)
}

// ── Bars ─────────────────────────────────────────────────────────────────────

/// Every control in the bars is this tall, so they line up.
const CONTROL_HEIGHT: f32 = 30.0;
/// The page's side margin.
const GUTTER: f32 = 28.0;

/// A control's frame: fixed height, rounded, filled when `filled`.
fn control_frame(theme: &Theme, child: Box<dyn Element>, filled: bool, hovered: bool) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Align::new(child).finish())
            .with_background_color(if hovered {
                theme.surface_2()
            } else if filled {
                theme.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .with_border(Border::all(1.0).with_border_color(if filled || hovered { theme.outline() } else { ColorU::transparent_black() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .finish(),
    )
    .with_height(CONTROL_HEIGHT)
    .finish()
}

/// A square icon-only button.
fn icon_button(view: &TasksView, theme: &Theme, key: &str, icon: AppIcon, action: TasksAction) -> Box<dyn Element> {
    let theme = *theme;
    let view_id = view.view_id;
    Hoverable::new(view.mouse(key), move |state| {
        let hovered = state.is_hovered();
        ConstrainedBox::new(
            Container::new(Align::new(render_icon(icon, if hovered { theme.main_text() } else { theme.sub_text() }, 14.0)).finish())
                .with_background_color(if hovered { theme.surface_2() } else { theme.surface_1() })
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish(),
        )
        .with_width(CONTROL_HEIGHT)
        .with_height(CONTROL_HEIGHT)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// The page's main action: accent-filled, with an icon.
fn primary_button(view: &TasksView, theme: &Theme, key: &str, icon: AppIcon, text: &str, action: TasksAction) -> Box<dyn Element> {
    let theme = *theme;
    let ui = view.ui_font;
    let view_id = view.view_id;
    let text = text.to_string();
    Hoverable::new(view.mouse(key), move |state| {
        let fill = if state.is_hovered() { tint(theme.accent, 225) } else { theme.accent };
        ConstrainedBox::new(
            Container::new(
                Align::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(6.0)
                        .with_child(render_icon(icon, theme.background, 13.0))
                        .with_child(semibold(ui, text.clone(), 12.5, theme.background))
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(fill)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(12.0)
            .with_padding_right(14.0)
            .finish(),
        )
        .with_height(CONTROL_HEIGHT)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// Choices side by side in one rounded track; the chosen one is raised.
fn segmented(view: &TasksView, theme: &Theme, key: &str, items: Vec<(Option<AppIcon>, Option<&str>, bool, TasksAction)>) -> Box<dyn Element> {
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(2.0);
    for (index, (icon, text, on, action)) in items.into_iter().enumerate() {
        let theme = *theme;
        let ui = view.ui_font;
        let view_id = view.view_id;
        let text = text.map(str::to_string);
        row = row.with_child(
            Hoverable::new(view.mouse(&format!("{key}-{index}")), move |state| {
                let color = if on || state.is_hovered() { theme.main_text() } else { theme.sub_text() };
                let mut content = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
                if let Some(icon) = icon {
                    content = content.with_child(render_icon(icon, color, 13.0));
                }
                if let Some(text) = &text {
                    content = content.with_child(medium(ui, text.clone(), 12.0, color));
                }
                ConstrainedBox::new(
                    Container::new(Align::new(content.finish()).finish())
                        .with_background_color(if on { theme.surface_3() } else { ColorU::transparent_black() })
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                        .with_padding_left(if text.is_some() { 10.0 } else { 7.0 })
                        .with_padding_right(if text.is_some() { 10.0 } else { 7.0 })
                        .finish(),
                )
                .with_height(CONTROL_HEIGHT - 6.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
            .finish(),
        );
    }
    ConstrainedBox::new(
        Container::new(Align::new(row.finish()).finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(2.0)
            .with_padding_right(2.0)
            .finish(),
    )
    .with_height(CONTROL_HEIGHT)
    .finish()
}

/// The title row: the page's name and what it holds, then search and its
/// actions.
fn header(view: &TasksView, theme: &Theme) -> Box<dyn Element> {
    let (open, what) = match view.source {
        Source::Dragent => (
            view.tasks().iter().filter(|task| task.status.is_open()).count(),
            "Shared with your organisation".to_string(),
        ),
        Source::Tracker(provider) => (
            view.tracker_items.iter().filter(|item| item.status.is_open()).count(),
            format!("From {}", provider.label()),
        ),
    };
    let search = ConstrainedBox::new(control_frame(
        theme,
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(render_icon(AppIcon::Search, theme.disabled_text(), 13.0))
            .with_child(Expanded::new(1.0, ChildView::new(&view.search_editor).finish()).finish())
            .finish(),
        true,
        false,
    ))
    .with_width(260.0)
    .finish();
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(
            Expanded::new(
                1.0,
                Flex::column()
                    .with_spacing(3.0)
                    .with_child(semibold(view.ui_font, "Tasks", 22.0, theme.main_text()))
                    .with_child(label(view.ui_font, &format!("{what} · {open} open"), 12.5, theme.sub_text()))
                    .finish(),
            )
            .finish(),
        )
        .with_child(search)
        .with_child(icon_button(view, theme, "refresh", AppIcon::RefreshCw, TasksAction::Refresh));
    // Creating belongs to the organisation's own list, except where the
    // connected tracker takes new items itself.
    if view.source == Source::Dragent && view.draft.is_none() {
        row = row.with_child(primary_button(view, theme, "new-task", AppIcon::Plus, "New task", TasksAction::NewTask));
    }
    if let Source::Tracker(provider) = view.source {
        if abilities(provider).create && view.item_draft.is_none() {
            row = row.with_child(primary_button(view, theme, "new-item", AppIcon::Plus, "New item", TasksAction::NewItem));
        }
    }
    Container::new(row.finish())
        .with_padding_left(GUTTER)
        .with_padding_right(GUTTER)
        .with_padding_top(22.0)
        .with_padding_bottom(16.0)
        .finish()
}

/// One source tab: its mark, its name, a count, and an underline when on.
fn source_tab(view: &TasksView, theme: &Theme, key: &str, mark: Box<dyn Element>, text: &str, count: Option<usize>, on: bool, action: TasksAction) -> Box<dyn Element> {
    let theme = *theme;
    let ui = view.ui_font;
    let view_id = view.view_id;
    let text = text.to_string();
    Hoverable::new(view.mouse(key), move |state| {
        let color = if on || state.is_hovered() { theme.main_text() } else { theme.sub_text() };
        let mut content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(7.0)
            .with_child(ConstrainedBox::new(Align::new(mark).finish()).with_width(15.0).with_height(15.0).finish())
            .with_child(medium(ui, text.clone(), 13.0, color));
        if let Some(count) = count {
            content = content.with_child(
                Container::new(medium(ui, count.to_string(), 10.5, if on { theme.main_text() } else { theme.sub_text() }))
                    .with_background_color(theme.surface_2())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .with_padding_top(1.0)
                    .with_padding_bottom(1.0)
                    .finish(),
            );
        }
        // The underline is the tab's bottom edge, as wide as the tab.
        Container::new(
            Container::new(content.finish())
                .with_background_color(if state.is_hovered() && !on { theme.surface_1() } else { ColorU::transparent_black() })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
        )
        .with_padding_bottom(6.0)
        .with_border(Border::bottom(2.0).with_border_color(if on { theme.accent } else { ColorU::transparent_black() }))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// The tabs: the organisation's tasks, then each connected tracker with its
/// logo, over a rule across the page.
fn sources(view: &TasksView, theme: &Theme) -> Box<dyn Element> {
    let open = view.tasks().iter().filter(|task| task.status.is_open()).count();
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::End).with_spacing(4.0).with_child(source_tab(
        view,
        theme,
        "source-dragent",
        render_icon(AppIcon::TaskList, if view.source == Source::Dragent { theme.accent } else { theme.sub_text() }, 14.0),
        "Tasks",
        Some(open),
        view.source == Source::Dragent,
        TasksAction::SetSource(Source::Dragent),
    ));
    for provider in &view.providers {
        let source = Source::Tracker(*provider);
        let on = view.source == source;
        let count = on.then(|| view.tracker_items.iter().filter(|item| item.status.is_open()).count()).filter(|_| !view.tracker_loading);
        row = row.with_child(source_tab(
            view,
            theme,
            &format!("source-{}", provider.key()),
            crate::integrations::marks::mark(theme, view.ui_font, *provider, 14.0),
            provider.label(),
            count,
            on,
            TasksAction::SetSource(source),
        ));
    }
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(row.finish())
                .with_padding_left(GUTTER - 10.0)
                .with_padding_right(GUTTER)
                .finish(),
        )
        .with_child(rule(theme))
        .finish()
}

/// The filters under the tabs: status, whose, label, and the layout.
fn toolbar(view: &TasksView, theme: &Theme) -> Box<dyn Element> {
    let own = view.source == Source::Dragent;
    let status = {
        let theme_copy = *theme;
        let ui = view.ui_font;
        let view_id = view.view_id;
        let open = view.menu.as_ref() == Some(&Menu::Status);
        let chosen = view.status_filter;
        let button = Hoverable::new(view.mouse("tasks_status_filter"), move |state| {
            let mut content = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(7.0);
            content = content.with_child(match chosen {
                StatusFilter::Only(status) => status_mark(&theme_copy, status, 12.0),
                _ => render_icon(AppIcon::Sliders, theme_copy.sub_text(), 12.0),
            });
            content = content
                .with_child(label(ui, "Status", 12.0, theme_copy.sub_text()))
                .with_child(medium(ui, chosen.label(), 12.0, theme_copy.main_text()))
                .with_child(render_icon(AppIcon::ChevronDown, theme_copy.sub_text(), 10.0));
            control_frame(&theme_copy, content.finish(), true, open || state.is_hovered())
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::ToggleMenu(Menu::Status)))
        .finish();
        SavePosition::new(button, "tasks_status_filter").finish()
    };
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(8.0).with_child(status);
    // Who a tracker's items belong to isn't known as a member here.
    if own {
        row = row.with_child(segmented(
            view,
            theme,
            "who",
            vec![
                (None, Some("Everyone"), view.assignee == AssigneeFilter::Anyone, TasksAction::SetAssignee(AssigneeFilter::Anyone)),
                (Some(AppIcon::User), Some("Mine"), view.assignee == AssigneeFilter::Me, TasksAction::SetAssignee(AssigneeFilter::Me)),
                (None, Some("Unassigned"), view.assignee == AssigneeFilter::Unassigned, TasksAction::SetAssignee(AssigneeFilter::Unassigned)),
            ],
        ));
    }
    if let Some(chosen) = view.label.as_ref().filter(|_| own) {
        let theme_copy = *theme;
        let ui = view.ui_font;
        let view_id = view.view_id;
        let text = chosen.clone();
        row = row.with_child(
            Hoverable::new(view.mouse("label-clear"), move |state| {
                control_frame(
                    &theme_copy,
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(6.0)
                        .with_child(dot(theme_copy.accent, 7.0))
                        .with_child(medium(ui, text.clone(), 12.0, theme_copy.main_text()))
                        .with_child(render_icon(AppIcon::XClose, theme_copy.sub_text(), 10.0))
                        .finish(),
                    true,
                    state.is_hovered(),
                )
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::SetLabel(None)))
            .finish(),
        );
    }
    row = row
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(segmented(
            view,
            theme,
            "layout",
            vec![
                (Some(AppIcon::TaskList), Some("List"), view.layout == Layout::Table, TasksAction::SetLayout(Layout::Table)),
                (Some(AppIcon::Grid), Some("Board"), view.layout == Layout::Board, TasksAction::SetLayout(Layout::Board)),
            ],
        ));
    Container::new(row.finish())
        .with_padding_left(GUTTER)
        .with_padding_right(GUTTER)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
}

fn selection_bar(view: &TasksView, theme: &Theme) -> Box<dyn Element> {
    let ids: Vec<String> = view.visible_tasks().iter().filter(|task| view.selected.contains(&task.id)).map(|task| task.id.clone()).collect();
    let count = view.selected.len();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(medium(view.ui_font, format!("{count} selected"), 12.5, theme.main_text()))
            .with_child(button(
                theme,
                view.ui_font,
                view.view_id,
                view.mouse("bulk-start"),
                Some(AppIcon::Play),
                if count == 1 { "Start in a workspace" } else { "Start in workspaces" },
                ButtonKind::Primary,
                (!ids.is_empty()).then_some(TasksAction::Start(ids)),
            ))
            .with_child(menu_button(view, theme, Menu::BulkStatus, "tasks_bulk_status", "Set status".into(), None))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(button(
                theme,
                view.ui_font,
                view.view_id,
                view.mouse("bulk-clear"),
                Some(AppIcon::XClose),
                "Clear selection",
                ButtonKind::Ghost,
                Some(TasksAction::ClearSelection),
            ))
            .finish(),
    )
    .with_background_color(tint(theme.accent, 18))
    .with_padding_left(24.0)
    .with_padding_right(24.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_margin_bottom(6.0)
    .finish()
}

fn error_line(view: &TasksView, theme: &Theme, error: &str) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(render_icon(AppIcon::TriangleAlert, theme.ui_red(), 12.0))
            .with_child(Expanded::new(1.0, label(view.ui_font, error, 12.5, theme.main_text())).finish())
            .with_child(button(
                theme,
                view.ui_font,
                view.view_id,
                view.mouse("error-dismiss"),
                None,
                "Dismiss",
                ButtonKind::Ghost,
                Some(TasksAction::DismissError),
            ))
            .finish(),
    )
    .with_background_color(tint(theme.ui_red(), 22))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .with_padding_left(12.0)
    .with_padding_right(8.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_margin_left(24.0)
    .with_margin_right(24.0)
    .with_margin_bottom(10.0)
    .finish()
}

fn draft_card(view: &TasksView, theme: &Theme) -> Box<dyn Element> {
    let Some(draft) = &view.draft else {
        return Empty::new().finish();
    };
    let assignee = draft
        .assignee
        .as_ref()
        .and_then(|id| view.members().iter().find(|member| &member.id == id))
        .map(|member| member.display().to_string())
        .unwrap_or_else(|| "Unassigned".into());
    let field = |child: Box<dyn Element>| {
        Container::new(child)
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
    };
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(medium(view.ui_font, "New task", 13.0, theme.main_text()))
            .with_child(field(ChildView::new(&view.title_editor).finish()))
            .with_child(field(ChildView::new(&view.description_editor).finish()))
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(menu_button(view, theme, Menu::NewStatus, "tasks_new_status", draft.status.label().into(), Some(status_mark(theme, draft.status, 12.0))))
                    .with_child(menu_button(view, theme, Menu::NewPriority, "tasks_new_priority", draft.priority.label().into(), None))
                    .with_child(menu_button(view, theme, Menu::NewAssignee, "tasks_new_assignee", assignee, None))
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(button(theme, view.ui_font, view.view_id, view.mouse("new-cancel"), None, "Cancel", ButtonKind::Ghost, Some(TasksAction::CancelNew)))
                    .with_child(button(
                        theme,
                        view.ui_font,
                        view.view_id,
                        view.mouse("new-create"),
                        None,
                        if draft.saving { "Creating…" } else { "Create task" },
                        ButtonKind::Primary,
                        (!draft.saving).then_some(TasksAction::SubmitNew),
                    ))
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .with_padding_left(14.0)
    .with_padding_right(14.0)
    .with_padding_top(12.0)
    .with_padding_bottom(12.0)
    .with_margin_left(24.0)
    .with_margin_right(24.0)
    .with_margin_bottom(12.0)
    .finish()
}

/// Where a new item goes, per provider: a team, project or `owner/repo`.
/// Blank means the tracker's default (for code hosts, this project).
fn item_place_hint(provider: Provider) -> &'static str {
    match provider {
        Provider::Github | Provider::Gitlab | Provider::Forgejo => "owner/repo (blank = this project)",
        Provider::Linear => "team key (e.g. ENG, blank = your only team)",
        Provider::Jira => "project key (e.g. OPS)",
        Provider::Plane | Provider::Youtrack | Provider::Asana => "project (blank for default)",
        _ => "where it goes (blank for default)",
    }
}

fn item_draft_card(view: &TasksView, theme: &Theme, provider: Provider) -> Box<dyn Element> {
    let Some(draft) = &view.item_draft else {
        return Empty::new().finish();
    };
    let filing_in = match view.root.as_ref().and_then(|root| crate::workspace::origin_url(root)).and_then(|url| project_key(&url)) {
        Some(key) => format!("Filing in: {key}"),
        None => "No project open: fill in where it goes above.".to_string(),
    };
    let field = |child: Box<dyn Element>| {
        Container::new(child)
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
    };
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(medium(view.ui_font, &format!("New {} item", provider.label()), 13.0, theme.main_text()))
            .with_child(field(ChildView::new(&view.title_editor).finish()))
            .with_child(field(ChildView::new(&view.description_editor).finish()))
            .with_child(field(ChildView::new(&view.item_place_editor).finish()))
            .with_child(label(view.ui_font, item_place_hint(provider), 11.5, theme.disabled_text()))
            .with_child(label(view.ui_font, &filing_in, 11.5, theme.disabled_text()))
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(button(theme, view.ui_font, view.view_id, view.mouse("new-item-cancel"), None, "Cancel", ButtonKind::Ghost, Some(TasksAction::CancelNewItem)))
                    .with_child(button(
                        theme,
                        view.ui_font,
                        view.view_id,
                        view.mouse("new-item-create"),
                        None,
                        if draft.saving { "Creating…" } else { "Create item" },
                        ButtonKind::Primary,
                        (!draft.saving).then_some(TasksAction::SubmitNewItem),
                    ))
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .with_padding_left(14.0)
    .with_padding_right(14.0)
    .with_padding_top(12.0)
    .with_padding_bottom(12.0)
    .with_margin_left(24.0)
    .with_margin_right(24.0)
    .with_margin_bottom(12.0)
    .finish()
}

// ── Menus ────────────────────────────────────────────────────────────────────

fn with_menu(view: &TasksView, theme: &Theme, body: Box<dyn Element>, menu: &Menu) -> Box<dyn Element> {
    let (anchor, items): (&str, Vec<(String, bool, TasksAction)>) = match menu {
        Menu::Status => (
            "tasks_status_filter",
            StatusFilter::CHOICES
                .iter()
                .map(|choice| (choice.label().to_string(), *choice == view.status_filter, TasksAction::SetStatusFilter(*choice)))
                .collect(),
        ),
        Menu::BulkStatus => (
            "tasks_bulk_status",
            Status::ALL.iter().map(|status| (status.label().to_string(), false, TasksAction::SetSelectedStatus(*status))).collect(),
        ),
        Menu::NewStatus => (
            "tasks_new_status",
            Status::ALL
                .iter()
                .map(|status| (status.label().to_string(), view.draft.as_ref().is_some_and(|draft| draft.status == *status), TasksAction::SetNewStatus(*status)))
                .collect(),
        ),
        Menu::NewPriority => (
            "tasks_new_priority",
            Priority::ALL
                .iter()
                .map(|priority| (priority.label().to_string(), view.draft.as_ref().is_some_and(|draft| draft.priority == *priority), TasksAction::SetNewPriority(*priority)))
                .collect(),
        ),
        Menu::NewAssignee => {
            let current = view.draft.as_ref().and_then(|draft| draft.assignee.clone());
            let mut items = vec![("Unassigned".to_string(), current.is_none(), TasksAction::SetNewAssignee(None))];
            items.extend(view.members().iter().map(|member| {
                let mine = view.me.as_deref() == Some(member.id.as_str());
                let name = if mine { format!("{} (you)", member.display()) } else { member.display().to_string() };
                (name, current.as_deref() == Some(member.id.as_str()), TasksAction::SetNewAssignee(Some(member.id.clone())))
            }));
            ("tasks_new_assignee", items)
        }
    };
    let ui = view.ui_font;
    let view_id = view.view_id;
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (index, (text, chosen, action)) in items.into_iter().enumerate() {
        let theme = *theme;
        list = list.with_child(
            Hoverable::new(view.mouse(&format!("menu-{anchor}-{index}")), move |state| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(Expanded::new(1.0, one_line(ui, &text, 12.5, theme.main_text())).finish())
                        .with_child(if chosen { render_icon(AppIcon::Check, theme.accent, 12.0) } else { Empty::new().finish() })
                        .finish(),
                )
                .with_background_color(if state.is_hovered() { theme.surface_2() } else { ColorU::transparent_black() })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
            .finish(),
        );
    }
    let panel = ConstrainedBox::new(
        Container::new(list.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_width(220.0)
    .finish();
    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();
    let catcher = EventHandler::new(Container::new(Empty::new().finish()).finish())
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, TasksAction::CloseMenus);
            DispatchEventResult::StopPropagation
        })
        .finish();
    let mut stack = Stack::new().with_child(body);
    stack.add_overlay_child(catcher);
    stack
        .with_positioned_overlay_child(
            panel,
            OffsetPositioning::offset_from_save_position_element(
                anchor,
                warpui::geometry::vector::vec2f(0.0, 4.0),
                PositionedElementOffsetBounds::WindowByPosition,
                PositionedElementAnchor::BottomLeft,
                ChildAnchor::TopLeft,
            ),
        )
        .finish()
}

// ── Content ──────────────────────────────────────────────────────────────────

fn content(view: &TasksView, theme: &Theme) -> Box<dyn Element> {
    if let Source::Tracker(provider) = view.source {
        return tracker_content(view, theme, provider.label());
    }
    if view.loading && view.tasks().is_empty() {
        return centered(view, "Loading tasks…", theme.sub_text());
    }
    if view.tasks().is_empty() {
        return empty(view, theme, "No tasks yet", "Tasks are shared with everyone in your organisation. Create one, then start it in a workspace with an agent.");
    }
    let visible = view.visible_tasks();
    if visible.is_empty() {
        return empty(view, theme, "No tasks match these filters", "Change the status, assignee, label or search above.");
    }
    match view.layout {
        Layout::Table => table(view, theme, &visible),
        Layout::Board => board(view, theme, &visible),
    }
}

fn empty(view: &TasksView, theme: &Theme, title: &str, detail: &str) -> Box<dyn Element> {
    Align::new(
        ConstrainedBox::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(AppIcon::TaskList, theme.disabled_text(), 28.0))
                .with_child(semibold(view.ui_font, title, 15.0, theme.main_text()))
                .with_child(label(view.ui_font, detail, 12.5, theme.sub_text()))
                .finish(),
        )
        .with_max_width(420.0)
        .finish(),
    )
    .finish()
}

fn scroll_v(theme: &Theme, handle: &warpui::elements::ClippedScrollStateHandle, child: Box<dyn Element>) -> Box<dyn Element> {
    ClippedScrollable::vertical(handle.clone(), child, ScrollbarWidth::Auto, theme.outline().into(), theme.sub_text().into(), theme.background.into())
        .with_overlayed_scrollbar()
        .finish()
}

fn table(view: &TasksView, theme: &Theme, visible: &[&Task]) -> Box<dyn Element> {
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for status in Status::ALL {
        let tasks: Vec<&Task> = visible.iter().copied().filter(|task| task.status == status).collect();
        if tasks.is_empty() {
            continue;
        }
        let collapsed = view.collapsed.contains(&status);
        list = list.with_child(group_header(view, theme, status, tasks.len(), collapsed));
        if !collapsed {
            for task in tasks {
                list = list.with_child(task_row(view, theme, task));
            }
        }
    }
    scroll_v(theme, &view.scroll, Container::new(list.finish()).with_padding_bottom(24.0).finish())
}

fn group_header(view: &TasksView, theme: &Theme, status: Status, count: usize, collapsed: bool) -> Box<dyn Element> {
    let theme = *theme;
    let ui = view.ui_font;
    let view_id = view.view_id;
    Hoverable::new(view.mouse(&format!("group-{}", status.key())), move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(if collapsed { AppIcon::ChevronRight } else { AppIcon::ChevronDown }, theme.disabled_text(), 10.0))
                .with_child(status_mark(&theme, status, 13.0))
                .with_child(medium(ui, status.label(), 12.5, theme.main_text()))
                .with_child(label(ui, &count.to_string(), 12.0, theme.sub_text()))
                .finish(),
        )
        .with_background_color(if state.is_hovered() { theme.surface_2() } else { theme.surface_1() })
        .with_padding_left(24.0)
        .with_padding_right(24.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::ToggleGroup(status)))
    .finish()
}

/// Pills for where a task's work is: its workspaces and pull requests.
fn link_pills(view: &TasksView, theme: &Theme, task: &Task) -> Vec<Box<dyn Element>> {
    let mut pills = Vec::new();
    let workspaces = task.links_of(LinkKind::Workspace).count();
    if workspaces > 0 {
        let text = if workspaces == 1 { "Workspace".to_string() } else { format!("{workspaces} workspaces") };
        pills.push(small_pill(view, AppIcon::Layers, &text, theme.accent));
    }
    if let Some(pr) = task.links_of(LinkKind::PullRequest).last() {
        let state = pr.state.as_deref().unwrap_or("open");
        let color = match state {
            "merged" => theme.ui_green(),
            "closed" => theme.disabled_text(),
            _ => theme.sub_text(),
        };
        pills.push(small_pill(view, AppIcon::PullRequest, state, color));
    }
    pills
}

fn checkbox(view: &TasksView, theme: &Theme, id: &str) -> Box<dyn Element> {
    let theme = *theme;
    let on = view.selected.contains(id);
    let view_id = view.view_id;
    let id = id.to_string();
    let boxed = Hoverable::new(view.mouse(&format!("select-{id}")), move |state| {
        ConstrainedBox::new(
            Container::new(if on { Align::new(render_icon(AppIcon::Check, theme.background, 10.0)).finish() } else { Empty::new().finish() })
                .with_background_color(if on { theme.accent } else { ColorU::transparent_black() })
                .with_border(Border::all(1.0).with_border_color(if on || state.is_hovered() { theme.accent } else { theme.outline() }))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish(),
        )
        .with_width(15.0)
        .with_height(15.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .finish();
    EventHandler::new(boxed)
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, TasksAction::ToggleSelect(id.clone()));
            DispatchEventResult::StopPropagation
        })
        .finish()
}

fn task_row(view: &TasksView, theme: &Theme, task: &Task) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(checkbox(view, theme, &task.id))
        .with_child(priority_mark(theme, task.priority))
        .with_child(ConstrainedBox::new(one_line(view.mono_font, &task.key, 12.0, theme.sub_text())).with_width(KEY_WIDTH).finish())
        .with_child(Expanded::new(1.0, one_line(view.ui_font, &task.title, 13.0, theme.main_text())).finish());
    for label in task.labels.iter().take(2) {
        row = row.with_child(label_chip(view, theme, label));
    }
    for pill in link_pills(view, theme, task) {
        row = row.with_child(pill);
    }
    row = row
        .with_child(match &task.assignee {
            Some(person) => avatar(view, theme, person.display()),
            None => ConstrainedBox::new(Empty::new().finish()).with_width(20.0).finish(),
        })
        .with_child(ConstrainedBox::new(one_line(view.ui_font, &ago(task.updated_at_ms), 11.5, theme.disabled_text())).with_width(58.0).finish());
    let body = row.finish();
    let theme = *theme;
    let view_id = view.view_id;
    let id = task.id.clone();
    let selected = view.selected.contains(&task.id);
    Hoverable::new(view.mouse(&format!("row-{}", task.id)), move |state| {
        Container::new(body)
            .with_background_color(if selected {
                tint(theme.accent, 20)
            } else if state.is_hovered() {
                theme.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .with_border(Border::bottom(1.0).with_border_color(tint(theme.outline(), 120)))
            .with_padding_left(24.0)
            .with_padding_right(24.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::Open(id.clone())))
    .finish()
}

fn board(view: &TasksView, theme: &Theme, visible: &[&Task]) -> Box<dyn Element> {
    let mut columns = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(12.0);
    for status in Status::ALL {
        if !view.status_filter.shows(status) {
            continue;
        }
        let tasks: Vec<&Task> = visible.iter().copied().filter(|task| task.status == status).collect();
        columns = columns.with_child(ConstrainedBox::new(board_column(view, theme, status, &tasks)).with_width(COLUMN_WIDTH).finish());
    }
    ClippedScrollable::horizontal(
        view.board_scroll.clone(),
        Container::new(columns.finish())
            .with_padding_left(24.0)
            .with_padding_right(24.0)
            .with_padding_bottom(16.0)
            .finish(),
        ScrollbarWidth::Auto,
        theme.outline().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

fn board_column(view: &TasksView, theme: &Theme, status: Status, tasks: &[&Task]) -> Box<dyn Element> {
    let mut cards = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(8.0);
    for task in tasks {
        cards = cards.with_child(card(view, theme, task));
    }
    let column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(status_mark(theme, status, 13.0))
                    .with_child(medium(view.ui_font, status.label(), 12.5, theme.main_text()))
                    .with_child(label(view.ui_font, &tasks.len().to_string(), 12.0, theme.sub_text()))
                    .finish(),
            )
            .with_padding_left(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(10.0)
            .finish(),
        )
        .with_child(Expanded::new(1.0, scroll_v(theme, &view.column_scroll(status), Container::new(cards.finish()).with_padding_bottom(40.0).finish())).finish())
        .finish();
    let column = Container::new(column)
        .with_background_color(theme.surface_1())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(8.0)
        .finish();
    DropTarget::new(column, ColumnTarget { status }).finish()
}

fn card(view: &TasksView, theme: &Theme, task: &Task) -> Box<dyn Element> {
    let mut meta = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(priority_mark(theme, task.priority))
        .with_child(one_line(view.mono_font, &task.key, 11.5, theme.sub_text()))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if let Some(person) = &task.assignee {
        meta = meta.with_child(avatar(view, theme, person.display()));
    }
    let mut extras = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
    let mut any_extra = false;
    for label in task.labels.iter().take(3) {
        extras = extras.with_child(label_chip(view, theme, label));
        any_extra = true;
    }
    for pill in link_pills(view, theme, task) {
        extras = extras.with_child(pill);
        any_extra = true;
    }
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(meta.finish())
        .with_child(label(view.ui_font, &task.title, 13.0, theme.main_text()));
    if any_extra {
        body = body.with_child(extras.finish());
    }
    let body = body.finish();
    let theme_copy = *theme;
    let view_id = view.view_id;
    let id = task.id.clone();
    let card = Hoverable::new(view.mouse(&format!("card-{}", task.id)), move |state| {
        Container::new(body)
            .with_background_color(theme_copy.pane_surface())
            .with_border(Border::all(1.0).with_border_color(if state.is_hovered() { theme_copy.sub_text() } else { theme_copy.outline() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(9.0)
            .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::Open(id.clone())))
    .finish();
    // Drag a card onto another column to change its status.
    let from = task.status;
    let moved = task.id.clone();
    Draggable::new(view.drag(&task.id), card)
        .with_keep_original_visible(true)
        .with_drop_target_at_pointer()
        .with_accepted_by_drop_target_fn(move |data, _| match data.as_any().downcast_ref::<ColumnTarget>() {
            Some(target) if target.status != from => AcceptedByDropTarget::Yes,
            _ => AcceptedByDropTarget::No,
        })
        .on_drop(move |ctx, _, _, data| {
            if let Some(target) = data.and_then(|data| data.as_any().downcast_ref::<ColumnTarget>()) {
                ctx.dispatch_typed_action_for(view_id, TasksAction::Move { id: moved.clone(), status: target.status });
            }
        })
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_its_own_colour_except_the_closed_quiet_ones() {
        let theme = Theme::default();
        assert_ne!(status_color(&theme, Status::InProgress), status_color(&theme, Status::Todo));
        assert_eq!(status_color(&theme, Status::Backlog), status_color(&theme, Status::Canceled));
    }
}

// ── A tracker's items ───────────────────────────────────────────────────────

fn tracker_content(view: &TasksView, theme: &Theme, service: &str) -> Box<dyn Element> {
    if let Some(error) = &view.tracker_error {
        if view.tracker_items.is_empty() && view.item_draft.is_none() {
            return empty(view, theme, &format!("{service} didn't answer"), error);
        }
    }
    if view.tracker_loading && view.tracker_items.is_empty() && view.item_draft.is_none() {
        return centered(view, &format!("Asking {service}…"), theme.sub_text());
    }
    let form = match view.source {
        Source::Tracker(provider) if view.item_draft.is_some() => Some(item_draft_card(view, theme, provider)),
        _ => None,
    };
    let visible = view.visible_items();
    if visible.is_empty() && form.is_none() {
        return empty(view, theme, &format!("No {service} items here"), "Open items show here; change the status filter or search above.");
    }
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if let Some(error) = &view.tracker_error {
        column = column.with_child(error_line(view, theme, error));
    }
    if let Some(form) = form {
        column = column.with_child(form);
    }
    let body = match view.layout {
        Layout::Table => {
            let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
            for status in Status::ALL {
                let items: Vec<&TrackerItem> = visible.iter().copied().filter(|item| item.status == status).collect();
                if items.is_empty() {
                    continue;
                }
                let collapsed = view.collapsed.contains(&status);
                list = list.with_child(group_header(view, theme, status, items.len(), collapsed));
                if !collapsed {
                    for item in items {
                        list = list.with_child(item_row(view, theme, item));
                    }
                }
            }
            scroll_v(theme, &view.scroll, Container::new(list.finish()).with_padding_bottom(24.0).finish())
        }
        Layout::Board => {
            let mut columns = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(12.0);
            for status in Status::ALL {
                if !view.status_filter.shows(status) {
                    continue;
                }
                let items: Vec<&TrackerItem> = visible.iter().copied().filter(|item| item.status == status).collect();
                columns = columns.with_child(ConstrainedBox::new(item_column(view, theme, status, &items)).with_width(COLUMN_WIDTH).finish());
            }
            ClippedScrollable::horizontal(
                view.board_scroll.clone(),
                Container::new(columns.finish()).with_padding_left(24.0).with_padding_right(24.0).with_padding_bottom(16.0).finish(),
                ScrollbarWidth::Auto,
                theme.outline().into(),
                theme.sub_text().into(),
                theme.background.into(),
            )
            .with_overlayed_scrollbar()
            .finish()
        }
    };
    column.with_child(Expanded::new(1.0, body).finish()).finish()
}

fn plain_chip(view: &TasksView, theme: &Theme, text: &str) -> Box<dyn Element> {
    Container::new(medium(view.ui_font, text, 11.0, theme.sub_text()))
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish()
}

/// Import button for a tracker item — or Open when the item was already
/// imported as a native task, since importing again would duplicate it.
fn import_button(view: &TasksView, theme: &Theme, item: &TrackerItem, mouse_key: String) -> Box<dyn Element> {
    if let Some(task) = view.imported_task(item) {
        return button(
            theme,
            view.ui_font,
            view.view_id,
            view.mouse(&mouse_key),
            None,
            "Open",
            ButtonKind::Ghost,
            Some(TasksAction::Open(task.id)),
        );
    }
    let importing = view.importing.contains(item.id());
    button(
        theme,
        view.ui_font,
        view.view_id,
        view.mouse(&mouse_key),
        Some(AppIcon::Download),
        if importing { "Importing…" } else { "Import" },
        ButtonKind::Ghost,
        (!importing).then_some(TasksAction::ImportItem(item.id().to_string())),
    )
}

fn item_row(view: &TasksView, theme: &Theme, item: &TrackerItem) -> Box<dyn Element> {    let id = item.id().to_string();
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(ConstrainedBox::new(one_line(view.mono_font, &item.key, 12.0, theme.sub_text())).with_width(KEY_WIDTH + 24.0).finish())
        .with_child(Expanded::new(1.0, one_line(view.ui_font, &item.title, 13.0, theme.main_text())).finish());
    if let Some(state) = &item.state_label {
        row = row.with_child(plain_chip(view, theme, state));
    }
    for label in item.labels.iter().take(2) {
        row = row.with_child(plain_chip(view, theme, label));
    }
    row = row
        .with_child(match &item.assignee {
            Some(name) => avatar(view, theme, name),
            None => ConstrainedBox::new(Empty::new().finish()).with_width(20.0).finish(),
        })
        .with_child(
            ConstrainedBox::new(one_line(view.ui_font, &item.updated_at_ms.map(ago).unwrap_or_default(), 11.5, theme.disabled_text()))
                .with_width(58.0)
                .finish(),
        )
        .with_child(button(
            theme,
            view.ui_font,
            view.view_id,
            view.mouse(&format!("item-start-{id}")),
            Some(AppIcon::Play),
            "Start",
            ButtonKind::Ghost,
            Some(TasksAction::StartItem(id.clone())),
        ))
        .with_child(import_button(view, theme, item, format!("item-import-{id}")));
    let body = row.finish();
    let theme = *theme;
    let view_id = view.view_id;
    Hoverable::new(view.mouse(&format!("item-row-{id}")), move |state| {
        Container::new(body)
            .with_background_color(if state.is_hovered() { theme.surface_1() } else { ColorU::transparent_black() })
            .with_border(Border::bottom(1.0).with_border_color(tint(theme.outline(), 120)))
            .with_padding_left(24.0)
            .with_padding_right(24.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::OpenItem(id.clone())))
    .finish()
}

fn item_column(view: &TasksView, theme: &Theme, status: Status, items: &[&TrackerItem]) -> Box<dyn Element> {
    let mut cards = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(8.0);
    for item in items {
        cards = cards.with_child(item_card(view, theme, item));
    }
    let column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(status_mark(theme, status, 13.0))
                    .with_child(medium(view.ui_font, status.label(), 12.5, theme.main_text()))
                    .with_child(label(view.ui_font, &items.len().to_string(), 12.0, theme.sub_text()))
                    .finish(),
            )
            .with_padding_left(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(10.0)
            .finish(),
        )
        .with_child(Expanded::new(1.0, scroll_v(theme, &view.column_scroll(status), Container::new(cards.finish()).with_padding_bottom(40.0).finish())).finish())
        .finish();
    let column = Container::new(column)
        .with_background_color(theme.surface_1())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(8.0)
        .finish();
    DropTarget::new(column, ItemColumnTarget { status }).finish()
}

fn item_card(view: &TasksView, theme: &Theme, item: &TrackerItem) -> Box<dyn Element> {
    let id = item.id().to_string();
    let mut meta = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(Expanded::new(1.0, one_line(view.mono_font, &item.key, 11.5, theme.sub_text())).finish());
    if let Some(name) = &item.assignee {
        meta = meta.with_child(avatar(view, theme, name));
    }
    let actions = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(button(
            theme,
            view.ui_font,
            view.view_id,
            view.mouse(&format!("item-card-start-{id}")),
            Some(AppIcon::Play),
            "Start",
            ButtonKind::Ghost,
            Some(TasksAction::StartItem(id.clone())),
        ))
        .with_child(import_button(view, theme, item, format!("item-card-import-{id}")))
        .finish();
    let body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(meta.finish())
        .with_child(label(view.ui_font, &item.title, 13.0, theme.main_text()))
        .with_child(actions)
        .finish();
    let theme_copy = *theme;
    let view_id = view.view_id;
    let opened = id.clone();
    let card = Hoverable::new(view.mouse(&format!("item-card-{id}")), move |state| {
        Container::new(body)
            .with_background_color(theme_copy.pane_surface())
            .with_border(Border::all(1.0).with_border_color(if state.is_hovered() { theme_copy.sub_text() } else { theme_copy.outline() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(9.0)
            .finish()
    })
    .with_defer_events_to_children()
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, TasksAction::OpenItem(opened.clone())))
    .finish();
    // Dragging an item's card onto another column changes its status in
    // its service.
    let from = item.status;
    Draggable::new(view.drag(&format!("item-{id}")), card)
        .with_keep_original_visible(true)
        .with_drop_target_at_pointer()
        .with_accepted_by_drop_target_fn(move |data, _| match data.as_any().downcast_ref::<ItemColumnTarget>() {
            Some(target) if target.status != from => AcceptedByDropTarget::Yes,
            _ => AcceptedByDropTarget::No,
        })
        .on_drop(move |ctx, _, _, data| {
            if let Some(target) = data.and_then(|data| data.as_any().downcast_ref::<ItemColumnTarget>()) {
                ctx.dispatch_typed_action_for(view_id, TasksAction::MoveItem { id: id.clone(), status: target.status });
            }
        })
        .finish()
}
