//! Controls for the Automations page, in the app's settings style but
//! dispatching the page's own actions.

use dragent_automations::{AgentState, Run, RunStatus};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::{Fill, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;
use warpui_core::ui_components::components::{UiComponent, UiComponentStyles};
use warpui_core::ui_components::switch::{Switch, SwitchStateHandle};

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};
use crate::workspace::{InputEditor, InputEditorAction};

use super::view::AutomationsAction;

pub(crate) fn label(ui: FamilyId, text: &str, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(text.to_string(), ui, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}

pub(crate) fn one_line(ui: FamilyId, text: &str, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(text.to_string(), ui, crate::theme::text_size(size))
        .with_color(color)
        .soft_wrap(false)
        .with_clip(ClipConfig::ellipsis())
        .finish()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// A button dispatching `action` to view `view_id` (any page's actions);
/// `None` shows it disabled.
#[allow(clippy::too_many_arguments)]
pub(crate) fn button<A: warpui::Action + Clone + 'static>(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
    icon: Option<AppIcon>,
    text: &str,
    kind: ButtonKind,
    action: Option<A>,
) -> Box<dyn Element> {
    let theme = *theme;
    let text = text.to_string();
    let enabled = action.is_some();
    let hoverable = Hoverable::new(mouse, move |state| {
        let hovered = state.is_hovered() && enabled;
        let (bg, fg, border) = match kind {
            // Dimmed while it can't be pressed, like the prompt's send button.
            ButtonKind::Primary if !enabled => (
                ColorU::new(theme.accent.r, theme.accent.g, theme.accent.b, 90),
                theme.accent_text(),
                None,
            ),
            ButtonKind::Primary => (
                if hovered {
                    ColorU::new(
                        theme.accent.r.saturating_add(15),
                        theme.accent.g.saturating_add(15),
                        theme.accent.b.saturating_add(15),
                        theme.accent.a,
                    )
                } else {
                    theme.accent
                },
                theme.accent_text(),
                None,
            ),
            ButtonKind::Secondary => (
                if hovered { theme.surface_2() } else { ColorU::transparent_black() },
                if hovered { theme.main_text() } else { theme.sub_text() },
                Some(theme.outline()),
            ),
            ButtonKind::Ghost => (
                if hovered { theme.surface_2() } else { ColorU::transparent_black() },
                if hovered { theme.main_text() } else { theme.sub_text() },
                None,
            ),
            ButtonKind::Danger => (
                if hovered {
                    ColorU::new(theme.ui_red().r, theme.ui_red().g, theme.ui_red().b, 28)
                } else {
                    ColorU::transparent_black()
                },
                theme.ui_red(),
                Some(if hovered { theme.ui_red() } else { theme.outline() }),
            ),
        };
        let fg = if enabled { fg } else { theme.disabled_text() };
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_spacing(6.0);
        if let Some(icon) = icon {
            row = row.with_child(render_icon(icon, fg, 13.0));
        }
        if !text.is_empty() {
            row = row.with_child(label(ui, &text, 12.5, fg));
        }
        let centered = ConstrainedBox::new(Align::new(row.finish()).finish())
            .with_height(crate::theme::text_size(18.0))
            .finish();
        let mut container = Container::new(centered)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(if text.is_empty() { 7.0 } else { 12.0 })
            .with_padding_right(if text.is_empty() { 7.0 } else { 12.0 })
            .with_padding_top(6.0)
            .with_padding_bottom(6.0);
        if let Some(border) = border {
            container = container.with_border(Border::all(1.0).with_border_color(border));
        }
        container.finish()
    });
    match action {
        Some(action) => hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
            .finish(),
        None => hoverable.finish(),
    }
}

pub(crate) fn switch(
    theme: &Theme,
    view_id: EntityId,
    on: bool,
    action: AutomationsAction,
) -> Box<dyn Element> {
    Switch::new(
        SwitchStateHandle::default(),
        UiComponentStyles {
            background: Some(Fill::Solid(theme.surface_3())),
            foreground: Some(Fill::Solid(theme.foreground)),
            ..Default::default()
        },
        None,
        Some(UiComponentStyles {
            background: Some(Fill::Solid(theme.accent)),
            ..Default::default()
        }),
        None,
    )
    .check(on)
    .build()
    .on_click(move |ectx: &mut warpui_core::elements::EventContext, _, _| {
        ectx.dispatch_typed_action_for(view_id, action.clone());
    })
    .finish()
}

pub(crate) fn chip(theme: &Theme, ui: FamilyId, icon: AppIcon, text: &str) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(render_icon(icon, theme.sub_text(), 11.0))
            .with_child(
                ConstrainedBox::new(one_line(ui, text, 11.0, theme.sub_text()))
                    .with_max_width(200.0)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .with_padding_left(7.0)
    .with_padding_right(7.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .finish()
}

/// Icon and colour summarising a run's status.
pub(crate) fn status_style(theme: &Theme, status: RunStatus) -> (AppIcon, ColorU, &'static str) {
    match status {
        RunStatus::Scheduled => (AppIcon::Clock, theme.accent, "Scheduled"),
        RunStatus::Queued => (AppIcon::Clock, theme.sub_text(), "Queued"),
        RunStatus::PreparingWorkspace => (AppIcon::Clock, theme.sub_text(), "Preparing workspace"),
        RunStatus::StartingAgent => (AppIcon::Clock, theme.sub_text(), "Starting agent"),
        RunStatus::Started => (AppIcon::Check, theme.ui_green(), "Agent started"),
        RunStatus::Failed => (AppIcon::XClose, theme.ui_red(), "Failed"),
        RunStatus::Skipped => (AppIcon::ArrowRight, theme.sub_text(), "Skipped"),
        RunStatus::Cancelled => (AppIcon::XClose, theme.sub_text(), "Cancelled"),
    }
}

/// Icon and colour for what a started run's agent is doing.
pub(crate) fn agent_state_style(theme: &Theme, state: AgentState) -> (AppIcon, ColorU, &'static str) {
    match state {
        AgentState::Working => (AppIcon::Activity, theme.accent, "Working"),
        AgentState::NeedsInput => (AppIcon::TriangleAlert, theme.ui_yellow(), "Needs input"),
        AgentState::Done => (AppIcon::Check, theme.ui_green(), "Done"),
        AgentState::Failed => (AppIcon::XClose, theme.ui_red(), "Agent failed"),
        AgentState::Ended => (AppIcon::Power, theme.sub_text(), "Ended"),
        AgentState::TimedOut => (AppIcon::Clock, theme.ui_yellow(), "Timed out"),
    }
}

/// A run's status, or for a started run, how its agent is getting on.
pub(crate) fn run_style(theme: &Theme, run: &Run) -> (AppIcon, ColorU, &'static str) {
    match (&run.progress, run.status) {
        (Some(progress), RunStatus::Started) => agent_state_style(theme, progress.state),
        (_, status) => status_style(theme, status),
    }
}

pub(crate) fn status_badge(theme: &Theme, ui: FamilyId, run: &Run) -> Box<dyn Element> {
    let (icon, color, text) = run_style(theme, run);
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(render_icon(icon, color, 11.0))
            .with_child(label(ui, text, 11.0, color))
            .finish(),
    )
    .with_background_color(ColorU::new(color.r, color.g, color.b, 26))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .finish()
}

/// An editor in the app's input box style.
pub(crate) fn field(theme: &Theme, editor: &ViewHandle<InputEditor>, min_height: f32) -> Box<dyn Element> {
    let editor_id = editor.id();
    let field = Container::new(
        ConstrainedBox::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(ChildView::new(editor).finish())
                .finish(),
        )
        .with_min_height(min_height)
        .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .finish();
    // The multiline editor can be shorter than its field. Clicking the
    // padding or empty area must focus it too, just like a native text box.
    EventHandler::new(field)
        .on_left_mouse_down(move |ectx, _, position| {
            ectx.dispatch_typed_action_for(editor_id, InputEditorAction::ClickAt(position));
            DispatchEventResult::StopPropagation
        })
        .finish()
}

pub(crate) fn section_label(theme: &Theme, ui: FamilyId, text: &str) -> Box<dyn Element> {
    Container::new(label(ui, text, 11.5, theme.sub_text()))
        .with_padding_bottom(5.0)
        .finish()
}

/// A form section as a card: title header over the body. Gives long forms
/// visual grouping instead of one flat stack of controls.
pub(crate) fn card_section(
    theme: &Theme,
    ui: FamilyId,
    title: &str,
    body: Box<dyn Element>,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(label(ui, title, 13.0, theme.main_text()))
            .with_child(body)
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .with_padding_left(14.0)
    .with_padding_right(14.0)
    .with_padding_top(12.0)
    .with_padding_bottom(14.0)
    .finish()
}

pub(crate) struct Choice {
    pub label: String,
    pub selected: bool,
    pub mouse: MouseStateHandle,
    pub action: AutomationsAction,
}

/// A dropdown: the button shows `current`; while open, choices float below.
#[allow(clippy::too_many_arguments)]
pub(crate) fn select(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
    current: &str,
    width: f32,
    open: bool,
    toggle: AutomationsAction,
    choices: Vec<Choice>,
    scroll: &ClippedScrollStateHandle,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let current = current.to_string();
    let trigger = Hoverable::new(mouse, move |state| {
        let border = if open {
            theme_owned.accent
        } else if state.is_hovered() {
            theme_owned.sub_text()
        } else {
            theme_owned.outline()
        };
        ConstrainedBox::new(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(
                        Shrinkable::new(1.0, one_line(ui, &current, 12.5, theme_owned.main_text()))
                            .finish(),
                    )
                    .with_child(render_icon(
                        AppIcon::ChevronDown,
                        theme_owned.sub_text(),
                        11.0,
                    ))
                    .finish(),
            )
            .with_background_color(theme_owned.surface_2())
            .with_border(Border::all(1.0).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(9.0)
            .with_padding_right(8.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish(),
        )
        .with_width(width)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, toggle.clone()))
    .finish();
    if !open || choices.is_empty() {
        return trigger;
    }
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(1.0);
    for choice in choices {
        let Choice {
            label: text,
            selected,
            mouse,
            action,
        } = choice;
        let theme_item = *theme;
        list = list.with_child(
            Hoverable::new(mouse, move |state| {
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Shrinkable::new(
                                1.0,
                                one_line(
                                    ui,
                                    &text,
                                    12.5,
                                    if selected { theme_item.accent } else { theme_item.main_text() },
                                ),
                            )
                            .finish(),
                        )
                        .with_child(if selected {
                            render_icon(AppIcon::Check, theme_item.accent, 11.0)
                        } else {
                            Empty::new().finish()
                        })
                        .finish(),
                )
                .with_background_color(if state.is_hovered() {
                    theme_item.surface_3()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
            .finish(),
        );
    }
    let menu = Container::new(
        ConstrainedBox::new(
            ClippedScrollable::vertical(
                scroll.clone(),
                list.finish(),
                ScrollbarWidth::Auto,
                theme.outline().into(),
                theme.sub_text().into(),
                theme.surface_2().into(),
            )
            .finish(),
        )
        .with_max_height(240.0)
        .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_uniform_padding(4.0)
    .finish();
    let offset = OffsetPositioning::offset_from_parent(
        vec2f(0.0, 4.0),
        ParentOffsetBounds::WindowByPosition,
        ParentAnchor::BottomLeft,
        ChildAnchor::TopLeft,
    );
    Box::new(
        Stack::new().with_child(trigger).with_positioned_overlay_child(
            ConstrainedBox::new(menu).with_width(width.max(160.0)).finish(),
            offset,
        ),
    )
}

pub(crate) fn divider(theme: &Theme) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(theme.outline())
            .finish(),
    )
    .with_height(1.0)
    .finish()
}

/// One option of a segmented control: (mouse, label, selected, action).
pub(crate) struct Segment {
    pub mouse: MouseStateHandle,
    pub label: String,
    pub selected: bool,
    /// `None` shows it disabled.
    pub action: Option<AutomationsAction>,
}

/// Options side by side in one rounded track; the selected one is raised.
pub(crate) fn segmented(theme: &Theme, ui: FamilyId, view_id: EntityId, segments: Vec<Segment>) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(3.0);
    for segment in segments {
        let theme = *theme;
        let Segment {
            mouse,
            label: text,
            selected,
            action,
        } = segment;
        let enabled = action.is_some();
        let hoverable = Hoverable::new(mouse, move |state| {
            let color = if !enabled {
                theme.disabled_text()
            } else if selected || state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::Center)
                    .with_child(Shrinkable::new(1.0, one_line(ui, &text, 12.0, color)).finish())
                    .finish(),
            )
            .with_background_color(if selected {
                theme.surface_3()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        });
        let element = match action {
            Some(action) => hoverable
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
                .finish(),
            None => hoverable.finish(),
        };
        row = row.with_child(warpui::elements::Expanded::new(1.0, element).finish());
    }
    Container::new(row.finish())
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_uniform_padding(3.0)
        .finish()
}

/// A row in a popover menu.
pub(crate) fn menu_item(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    text: &str,
    danger: bool,
    action: AutomationsAction,
) -> Box<dyn Element> {
    let theme = *theme;
    let text = text.to_string();
    Hoverable::new(mouse, move |state| {
        let color = if danger { theme.ui_red() } else { theme.main_text() };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(icon, color, 13.0))
                .with_child(label(ui, &text, 12.5, color))
                .finish(),
        )
        .with_background_color(if state.is_hovered() {
            theme.surface_3()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(8.0)
        .with_padding_right(10.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// The card a popover menu's rows sit in.
pub(crate) fn menu_card(theme: &Theme, rows: Vec<Box<dyn Element>>, width: f32) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    for row in rows {
        column = column.with_child(row);
    }
    ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_uniform_padding(4.0)
            .finish(),
    )
    .with_width(width)
    .finish()
}

/// `anchor` with `menu` floating below it, aligned to its right edge.
pub(crate) fn with_menu(anchor: Box<dyn Element>, menu: Option<Box<dyn Element>>, align_right: bool) -> Box<dyn Element> {
    let Some(menu) = menu else {
        return anchor;
    };
    let (parent, child) = if align_right {
        (ParentAnchor::BottomRight, ChildAnchor::TopRight)
    } else {
        (ParentAnchor::BottomLeft, ChildAnchor::TopLeft)
    };
    Box::new(Stack::new().with_child(anchor).with_positioned_overlay_child(
        menu,
        OffsetPositioning::offset_from_parent(vec2f(0.0, 4.0), ParentOffsetBounds::WindowByPosition, parent, child),
    ))
}

/// A small filled circle.
pub(crate) fn dot(color: ColorU, size: f32) -> Box<dyn Element> {
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

/// A tag with a remove control.
pub(crate) fn tag_chip(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
    text: &str,
    remove: Option<AutomationsAction>,
) -> Box<dyn Element> {
    let theme = *theme;
    let text = text.to_string();
    let removable = remove.is_some();
    let hoverable = Hoverable::new(mouse, move |state| {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(render_icon(AppIcon::Folder, theme.sub_text(), 11.0))
            .with_child(label(ui, &text, 11.5, theme.main_text()));
        if removable {
            row = row.with_child(render_icon(
                AppIcon::XClose,
                if state.is_hovered() { theme.main_text() } else { theme.disabled_text() },
                10.0,
            ));
        }
        Container::new(row.finish())
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(7.0)
            .with_padding_right(7.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish()
    });
    match remove {
        Some(action) => hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()))
            .finish(),
        None => hoverable.finish(),
    }
}
