//! Design primitives for the remote experience: Settings → Machines, the
//! workspace's Add Project dialog and the remote folder browser.
//!
//! They speak Dragent's settings language (border-only cards, uppercase group
//! labels, the cyan accent) on one shared scale, so every surface lines up:
//! spacing steps of 4, controls and inputs [`CONTROL_HEIGHT`] tall, rows
//! padded [`ROW_PAD_X`] × [`ROW_PAD_Y`], radii from [`RADIUS_CONTROL`] up to
//! [`RADIUS_DIALOG`]. Every helper dispatches a caller-supplied typed action,
//! so the settings view and the workspace view share them.
use crate::icons::{render_icon, AppIcon};
use crate::settings::machines_store::MachineStatus;
use crate::theme::Theme;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex,
    MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::vec2f;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, DropShadow, Radius};
use warpui_core::text_layout::ClipConfig;

/// Height shared by buttons, selects, segmented controls and text inputs.
pub const CONTROL_HEIGHT: f32 = 34.0;
/// Compact controls inside list rows.
pub const CONTROL_HEIGHT_SM: f32 = 28.0;
pub const RADIUS_CONTROL: f32 = 8.0;
pub const RADIUS_CARD: f32 = 12.0;
pub const RADIUS_DIALOG: f32 = 16.0;
/// Inner padding of list rows and card sections.
pub const ROW_PAD_X: f32 = 16.0;
pub const ROW_PAD_Y: f32 = 12.0;
/// Horizontal padding of dialog header, body and footer.
pub const DIALOG_PAD: f32 = 24.0;
/// The smallest gap a dialog keeps from the window's top and bottom edges.
const DIALOG_MARGIN: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Destructive,
    Ghost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// Row controls (Connect, Install, Open …): [`CONTROL_HEIGHT_SM`].
    Xs,
    /// Toolbars and dialog footers: [`CONTROL_HEIGHT`], level with inputs.
    Md,
}

pub fn alpha(color: ColorU, a: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, a)
}

fn lighten(color: ColorU, by: u8) -> ColorU {
    ColorU::new(
        color.r.saturating_add(by),
        color.g.saturating_add(by),
        color.b.saturating_add(by),
        color.a,
    )
}

pub fn text(ui: FamilyId, value: impl Into<String>, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(value.into(), ui, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}

pub fn medium(ui: FamilyId, value: impl Into<String>, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(value.into(), ui, crate::theme::text_size(size))
        .with_color(color)
        .with_style(Properties::default().weight(Weight::Medium))
        .finish()
}

pub fn semibold(ui: FamilyId, value: impl Into<String>, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(value.into(), ui, crate::theme::text_size(size))
        .with_color(color)
        .with_style(Properties::default().weight(Weight::Semibold))
        .finish()
}

/// Single-line text that ellipsizes instead of wrapping.
pub fn line(ui: FamilyId, value: impl Into<String>, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(value.into(), ui, crate::theme::text_size(size))
        .with_color(color)
        .soft_wrap(false)
        .with_clip(ClipConfig::ellipsis())
        .finish()
}

/// A box of exactly `width` × `height` with `child` centred in it.
pub fn sized(child: Box<dyn Element>, width: f32, height: f32) -> Box<dyn Element> {
    ConstrainedBox::new(Align::new(child).finish())
        .with_width(width)
        .with_height(height)
        .finish()
}

/// Keep `child` at its natural width even inside a stretching column.
pub fn hug(child: Box<dyn Element>) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(child)
        .finish()
}

#[allow(clippy::too_many_arguments)]
pub fn button<A: warpui::Action + Clone>(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    icon: Option<AppIcon>,
    label: &str,
    variant: Variant,
    size: Size,
    enabled: bool,
    action: A,
) -> Box<dyn Element> {
    let theme = *theme;
    let label = label.to_string();
    let (height, pad_x, font, icon_size, radius) = match size {
        Size::Xs => (CONTROL_HEIGHT_SM, 10.0, 12.0, 13.0, 7.0),
        Size::Md => (CONTROL_HEIGHT, 14.0, 13.0, 14.0, RADIUS_CONTROL),
    };
    let icon_only = label.is_empty();
    let mut hoverable = Hoverable::new(hover, move |state| {
        let hovered = enabled && state.is_hovered();
        let red = theme.ui_red();
        let (bg, fg, border) = match variant {
            Variant::Primary => (
                if hovered { lighten(theme.accent, 16) } else { theme.accent },
                theme.accent_text(),
                None,
            ),
            Variant::Secondary => (
                if hovered { theme.surface_2() } else { theme.surface_1() },
                theme.main_text(),
                Some(if hovered { theme.border() } else { theme.outline() }),
            ),
            Variant::Destructive => (
                if hovered { alpha(red, 40) } else { alpha(red, 22) },
                red,
                Some(alpha(red, if hovered { 150 } else { 90 })),
            ),
            Variant::Ghost => (
                if hovered { theme.surface_2() } else { ColorU::transparent_black() },
                if hovered { theme.main_text() } else { theme.sub_text() },
                None,
            ),
        };
        let fg = if enabled { fg } else { alpha(fg, 110) };
        let bg = if enabled { bg } else { alpha(bg, bg.a / 2) };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(6.0);
        if let Some(icon) = icon {
            row = row.with_child(render_icon(icon, fg, icon_size));
        }
        if !icon_only {
            row = row.with_child(
                Text::new(label.clone(), ui, crate::theme::text_size(font))
                    .with_color(fg)
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish(),
            );
        }
        // Icon-only buttons are square; labelled ones hug their content.
        let content: Box<dyn Element> = if icon_only {
            sized(row.finish(), height, height)
        } else {
            Container::new(
                ConstrainedBox::new(Align::new(row.finish()).finish())
                    .with_height(height)
                    .finish(),
            )
            .with_padding_left(pad_x)
            .with_padding_right(pad_x)
            .finish()
        };
        let mut container = Container::new(content)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(radius)));
        if let Some(border) = border {
            container = container.with_border(Border::all(1.0).with_border_color(border));
        }
        container.finish()
    });
    if enabled {
        hoverable = hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()));
    }
    hug(hoverable.finish())
}

pub fn status_color(theme: &Theme, status: MachineStatus) -> ColorU {
    match status {
        MachineStatus::Connected => theme.ui_green(),
        MachineStatus::Connecting => theme.accent,
        MachineStatus::Error => theme.ui_red(),
        MachineStatus::Idle => theme.disabled_text(),
    }
}

/// A round status dot, optionally ringed in the surface colour so it reads
/// on top of an icon.
fn dot(color: ColorU, size: f32, ring: Option<ColorU>) -> Box<dyn Element> {
    let mut container = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(size)
            .with_height(size)
            .finish(),
    )
    .with_background_color(color)
    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)));
    if let Some(ring) = ring {
        container = container.with_border(Border::all(2.0).with_border_color(ring));
    }
    container.finish()
}

/// A bare status dot (location menus, chips, sidebar rows).
pub fn status_dot(theme: &Theme, status: MachineStatus, size: f32) -> Box<dyn Element> {
    dot(status_color(theme, status), size, None)
}

/// A machine's identity: the server glyph on a rounded tile, with its
/// connection status as a ringed dot on the bottom-right corner.
pub fn machine_avatar(theme: &Theme, status: MachineStatus, size: f32) -> Box<dyn Element> {
    let tile = icon_tile(theme, AppIcon::Server, size, None);
    let dot_size = (size * 0.26).round().max(8.0);
    Stack::new()
        .with_child(tile)
        .with_positioned_child(
            dot(status_color(theme, status), dot_size, Some(theme.background)),
            OffsetPositioning::offset_from_parent(
                vec2f(3.0, 3.0),
                ParentOffsetBounds::Unbounded,
                ParentAnchor::BottomRight,
                ChildAnchor::BottomRight,
            ),
        )
        .finish()
}

/// Kept for compact places (menu rows): the avatar at small sizes.
pub fn status_icon(theme: &Theme, status: MachineStatus, size: f32) -> Box<dyn Element> {
    let glyph = sized(render_icon(AppIcon::Server, theme.sub_text(), size), size + 3.0, size + 3.0);
    let dot_size = (size * 0.42).round().max(6.0);
    Stack::new()
        .with_child(glyph)
        .with_positioned_child(
            dot(status_color(theme, status), dot_size, Some(theme.sidebar_menu_background())),
            OffsetPositioning::offset_from_parent(
                vec2f(1.0, 1.0),
                ParentOffsetBounds::Unbounded,
                ParentAnchor::BottomRight,
                ChildAnchor::BottomRight,
            ),
        )
        .finish()
}

/// An icon on a rounded square tile. `accent` tints the tile with the
/// brand colour (dialog headers); otherwise it is a quiet surface tile.
pub fn icon_tile(theme: &Theme, icon: AppIcon, size: f32, accent: Option<bool>) -> Box<dyn Element> {
    let accented = accent.unwrap_or(false);
    let (bg, fg, border) = if accented {
        (theme.accent_bg(), theme.accent, alpha(theme.accent, 70))
    } else {
        (theme.surface_2(), theme.sub_text(), theme.outline())
    };
    Container::new(sized(render_icon(icon, fg, (size * 0.48).round()), size, size))
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels((size * 0.28).round())))
        .finish()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Info,
    Warning,
    Error,
    Neutral,
}

fn tone_color(theme: &Theme, tone: Tone) -> ColorU {
    match tone {
        Tone::Success => theme.ui_green(),
        Tone::Info => theme.accent,
        Tone::Warning => theme.ui_yellow(),
        Tone::Error => theme.ui_red(),
        Tone::Neutral => theme.sub_text(),
    }
}

/// A status badge: "● Connected", "● Missing".
pub fn pill(theme: &Theme, ui: FamilyId, label: &str, tone: Tone) -> Box<dyn Element> {
    let color = tone_color(theme, tone);
    let (bg, border) = match tone {
        Tone::Neutral => (theme.surface_2(), theme.outline()),
        _ => (alpha(color, 26), alpha(color, 60)),
    };
    hug(
        Container::new(
            ConstrainedBox::new(
                Align::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_main_axis_size(MainAxisSize::Min)
                        .with_spacing(6.0)
                        .with_child(dot(color, 6.0, None))
                        .with_child(medium(ui, label, 11.5, color))
                        .finish(),
                )
                .finish(),
            )
            .with_height(22.0)
            .finish(),
        )
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(11.0)))
        .with_padding_left(8.0)
        .with_padding_right(9.0)
        .finish(),
    )
}

/// A bordered card, matching Dragent's border-only settings cards.
pub fn card(theme: &Theme, child: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(child)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS_CARD)))
        .with_padding_left(20.0)
        .with_padding_right(20.0)
        .with_padding_top(18.0)
        .with_padding_bottom(18.0)
        .finish()
}

/// A bordered card of rows separated by hairlines; rows bring their own
/// padding (see [`list_row`]).
pub fn list_card(theme: &Theme, rows: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    for (index, row) in rows.into_iter().enumerate() {
        column = column.with_child(if index == 0 {
            row
        } else {
            Container::new(row)
                .with_border(Border::top(1.0).with_border_color(theme.outline()))
                .finish()
        });
    }
    Container::new(column.finish())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS_CARD)))
        .finish()
}

/// One list row: leading visual, title over an optional detail line, and
/// trailing controls, padded on the shared row scale.
pub fn list_row(
    ui: FamilyId,
    theme: &Theme,
    leading: Option<Box<dyn Element>>,
    title: Box<dyn Element>,
    detail: Option<String>,
    trailing: Vec<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut labels = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(3.0)
        .with_child(title);
    if let Some(detail) = detail {
        labels = labels.with_child(line(ui, detail, 12.0, theme.sub_text()));
    }
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0);
    if let Some(leading) = leading {
        row = row.with_child(leading);
    }
    row = row.with_child(Expanded::new(1.0, labels.finish()).finish());
    for control in trailing {
        row = row.with_child(control);
    }
    Container::new(row.finish())
        .with_padding_left(ROW_PAD_X)
        .with_padding_right(ROW_PAD_X - 4.0)
        .with_padding_top(ROW_PAD_Y)
        .with_padding_bottom(ROW_PAD_Y)
        .finish()
}

/// A softly filled inset (resolved SSH details, logs).
pub fn sunken(theme: &Theme, child: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(child)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

pub fn divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(1.0)
            .finish(),
    )
    .with_background_color(theme.outline())
    .finish()
}

/// Uppercase eyebrow label, as Dragent's settings group labels.
/// One tab of a [`tab_strip`]: icon, label, an optional count, and an accent
/// underline while it's the open one.
#[allow(clippy::too_many_arguments)]
pub fn tab<A: warpui::Action + Clone>(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    icon: AppIcon,
    label: &str,
    count: Option<usize>,
    active: bool,
    action: A,
) -> Box<dyn Element> {
    let theme = *theme;
    let label = label.to_string();
    Hoverable::new(hover, move |state| {
        let color = if active || state.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let mut content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(7.0)
            .with_child(render_icon(icon, if active { theme.accent } else { color }, 14.0))
            .with_child(medium(ui, label.clone(), 13.0, color));
        if let Some(count) = count.filter(|n| *n > 0) {
            content = content.with_child(
                Container::new(medium(ui, count.to_string(), 11.0, theme.sub_text()))
                    .with_background_color(theme.surface_2())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .with_padding_top(1.0)
                    .with_padding_bottom(1.0)
                    .finish(),
            );
        }
        Container::new(content.finish())
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_border(Border::bottom(2.0).with_border_color(if active {
                theme.accent
            } else {
                ColorU::transparent_black()
            }))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// [`tab`]s in a row over a hairline.
pub fn tab_strip(theme: &Theme, tabs: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_spacing(4.0);
    for tab in tabs {
        row = row.with_child(tab);
    }
    Container::new(row.finish())
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
}

pub fn eyebrow(ui: FamilyId, theme: &Theme, label: &str) -> Box<dyn Element> {
    medium(ui, label.to_uppercase(), 10.5, theme.disabled_text())
}

/// A section title with an optional description and a trailing control
/// aligned to the title's baseline row.
pub fn section_header(
    ui: FamilyId,
    theme: &Theme,
    title: &str,
    description: Option<&str>,
    trailing: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut labels = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(4.0)
        .with_child(semibold(ui, title, 14.0, theme.main_text()));
    if let Some(description) = description {
        labels = labels.with_child(text(ui, description, 12.5, theme.sub_text()));
    }
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(16.0)
        .with_child(Expanded::new(1.0, labels.finish()).finish());
    if let Some(trailing) = trailing {
        row = row.with_child(trailing);
    }
    row.finish()
}

/// A settings row: label (+ trailing badges) and description on the left,
/// controls on the right.
pub fn settings_row(
    ui: FamilyId,
    theme: &Theme,
    label: &str,
    badges: Vec<Box<dyn Element>>,
    description: Option<Box<dyn Element>>,
    control: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut title = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(medium(ui, label, 13.5, theme.main_text()));
    for badge in badges {
        title = title.with_child(badge);
    }
    let mut left = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(5.0)
        .with_child(title.finish());
    if let Some(description) = description {
        left = left.with_child(description);
    }
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(16.0)
        .with_child(Expanded::new(1.0, left.finish()).finish());
    if let Some(control) = control {
        row = row.with_child(control);
    }
    row.finish()
}

/// A field label over its control, with an optional muted hint.
pub fn field(
    ui: FamilyId,
    theme: &Theme,
    label: &str,
    control: Box<dyn Element>,
    description: Option<&str>,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(medium(ui, label, 12.5, theme.sub_text()))
        .with_child(control);
    if let Some(description) = description {
        column = column.with_child(text(ui, description, 11.5, theme.disabled_text()));
    }
    column.finish()
}

/// A key/value line inside a details inset.
pub fn detail_row(ui: FamilyId, theme: &Theme, label: &str, value: String) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(
            ConstrainedBox::new(text(ui, label, 12.5, theme.sub_text()))
                .with_width(140.0)
                .finish(),
        )
        .with_child(Expanded::new(1.0, line(ui, value, 12.5, theme.main_text())).finish())
        .finish()
}

/// A switch: a pill track with a sliding knob.
pub fn switch<A: warpui::Action + Clone>(
    theme: &Theme,
    view_id: EntityId,
    hover: MouseStateHandle,
    on: bool,
    action: A,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(hover, move |_| {
        let knob = dot(if on { theme.accent_text() } else { theme.main_text() }, 14.0, None);
        let track = if on {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(knob)
        } else {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(knob)
        };
        ConstrainedBox::new(
            Container::new(track.finish())
                .with_background_color(if on { theme.accent } else { theme.surface_3() })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
                .with_uniform_padding(3.0)
                .finish(),
        )
        .with_width(36.0)
        .with_height(20.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// One option of a [`segmented`] control.
pub struct Segment<A> {
    pub hover: MouseStateHandle,
    pub icon: Option<AppIcon>,
    pub label: String,
    pub selected: bool,
    pub action: A,
}

/// A segmented control, [`CONTROL_HEIGHT`] tall. With `fill` the segments
/// share the full width equally; otherwise the control hugs its labels.
pub fn segmented<A: warpui::Action + Clone>(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    segments: Vec<Segment<A>>,
    fill: bool,
) -> Box<dyn Element> {
    let inner = CONTROL_HEIGHT - 8.0;
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(2.0);
    if !fill {
        row = row.with_main_axis_size(MainAxisSize::Min);
    }
    for segment in segments {
        let theme_c = *theme;
        let Segment { hover, icon, label, selected, action } = segment;
        let item = Hoverable::new(hover, move |state| {
            let color = if selected || state.is_hovered() {
                theme_c.main_text()
            } else {
                theme_c.sub_text()
            };
            let mut content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0);
            if let Some(icon) = icon {
                content = content.with_child(render_icon(
                    icon,
                    if selected { theme_c.accent } else { color },
                    13.0,
                ));
            }
            content = content.with_child(medium(ui, label.clone(), 12.5, color));
            let mut container = Container::new(
                ConstrainedBox::new(Align::new(content.finish()).finish())
                    .with_height(inner)
                    .finish(),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0);
            if selected {
                container = container
                    .with_background_color(theme_c.surface_3())
                    .with_border(Border::all(1.0).with_border_color(theme_c.border()));
            } else if state.is_hovered() {
                container = container.with_background_color(theme_c.surface_2());
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
        .finish();
        row = row.with_child(if fill { Expanded::new(1.0, item).finish() } else { item });
    }
    let control = Container::new(row.finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS_CONTROL + 1.0)))
        .with_uniform_padding(3.0)
        .finish();
    if fill {
        control
    } else {
        hug(control)
    }
}

/// An inline banner: tinted by tone, with an icon, a title, an optional
/// muted detail (raw error text goes here, never in the title) and an
/// optional trailing action.
pub fn notice(
    theme: &Theme,
    ui: FamilyId,
    tone: Tone,
    title: &str,
    detail: Option<String>,
    action: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let color = tone_color(theme, tone);
    let icon = match tone {
        Tone::Success => AppIcon::CircleCheck,
        Tone::Error => AppIcon::CircleX,
        Tone::Warning => AppIcon::TriangleAlert,
        Tone::Info => AppIcon::Info,
        Tone::Neutral => AppIcon::Info,
    };
    let (bg, border) = match tone {
        Tone::Neutral => (theme.surface_1(), theme.outline()),
        _ => (alpha(color, 20), alpha(color, 70)),
    };
    let mut labels = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(4.0)
        .with_child(medium(ui, title, 13.0, theme.main_text()));
    if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
        labels = labels.with_child(text(ui, detail, 12.0, theme.sub_text()));
    }
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(10.0)
        .with_child(
            Container::new(render_icon(icon, color, 16.0))
                .with_padding_top(1.0)
                .finish(),
        )
        .with_child(Expanded::new(1.0, labels.finish()).finish());
    if let Some(action) = action {
        row = row.with_child(action);
    }
    Container::new(row.finish())
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(14.0)
        .with_padding_right(12.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

/// A machine's host key that nothing trusts yet, for the person to compare
/// with the machine's own before `action` trusts it.
pub fn host_key_notice(
    theme: &Theme,
    ui: FamilyId,
    machine: &str,
    key: &dragent_host::remote::UnknownHostKey,
    action: Box<dyn Element>,
) -> Box<dyn Element> {
    let mut detail = String::from(
        "This is the first connection to it. Trust it only if this fingerprint matches the one \
         the machine itself reports:\n",
    );
    for fingerprint in &key.fingerprints {
        detail.push('\n');
        detail.push_str(fingerprint);
    }
    let file = key.fingerprints.first().and_then(|fingerprint| {
        match fingerprint.split_whitespace().next()? {
            "ED25519" => Some("ssh_host_ed25519_key.pub"),
            "ECDSA" => Some("ssh_host_ecdsa_key.pub"),
            "RSA" => Some("ssh_host_rsa_key.pub"),
            _ => None,
        }
    });
    if let Some(file) = file {
        detail.push_str(&format!(
            "\n\nOn the machine, `ssh-keygen -lf /etc/ssh/{file}` shows it."
        ));
    }
    notice(
        theme,
        ui,
        Tone::Warning,
        &format!("Check it's really {machine}"),
        Some(detail),
        Some(action),
    )
}

/// A thin usage bar: `fraction` of the track filled in `color`.
pub fn meter(theme: &Theme, fraction: f32, color: ColorU) -> Box<dyn Element> {
    let fraction = fraction.clamp(0.0, 1.0);
    let mut track = Flex::row();
    if fraction > 0.0 {
        track = track.with_child(
            Expanded::new(
                fraction,
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_height(4.0)
                        .finish(),
                )
                .with_background_color(color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                .finish(),
            )
            .finish(),
        );
    }
    if fraction < 1.0 {
        track = track.with_child(Expanded::new(1.0 - fraction, Empty::new().finish()).finish());
    }
    Container::new(
        ConstrainedBox::new(track.finish())
            .with_height(4.0)
            .finish(),
    )
    .with_background_color(theme.surface_3())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
    .finish()
}

/// Meter colour for a usage fraction: calm, then warning, then critical.
pub fn usage_color(theme: &Theme, fraction: f32) -> ColorU {
    if fraction >= 0.9 {
        theme.ui_red()
    } else if fraction >= 0.75 {
        theme.ui_yellow()
    } else {
        theme.accent
    }
}

/// A dialog's title block: an accent icon tile, the title, and a one-line
/// subtitle that says what the dialog is for.
pub fn dialog_title(
    theme: &Theme,
    ui: FamilyId,
    icon: AppIcon,
    title: &str,
    subtitle: Option<&str>,
) -> Box<dyn Element> {
    let mut labels = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(3.0)
        .with_child(semibold(ui, title, 16.0, theme.main_text()));
    if let Some(subtitle) = subtitle {
        labels = labels.with_child(text(ui, subtitle, 12.5, theme.sub_text()));
    }
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(icon_tile(theme, icon, 36.0, Some(true)))
        .with_child(Expanded::new(1.0, labels.finish()).finish())
        .finish()
}

/// Dialog chrome shared by every machine/project dialog: a title row with a
/// close button, a body, and a tinted footer, raised over a dimmed scrim.
/// Clicks on the panel never reach the scrim. The dialog is never taller
/// than the window: when the body outgrows it (Advanced, test logs), the
/// body scrolls with `scroll` between the fixed title and footer.
#[allow(clippy::too_many_arguments)]
pub fn dialog<A: warpui::Action + Clone>(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    close_hover: MouseStateHandle,
    scroll: &ClippedScrollStateHandle,
    title: Box<dyn Element>,
    body: Box<dyn Element>,
    footer: Option<Box<dyn Element>>,
    width: f32,
    dismiss: Option<A>,
) -> Box<dyn Element> {
    let mut header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(12.0)
        .with_child(Expanded::new(1.0, title).finish());
    if let Some(dismiss) = dismiss.clone() {
        header = header.with_child(button(
            theme,
            ui,
            view_id,
            close_hover,
            Some(AppIcon::XClose),
            "",
            Variant::Ghost,
            Size::Xs,
            true,
            dismiss,
        ));
    }
    let mut panel = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(header.finish())
                .with_padding_left(DIALOG_PAD)
                .with_padding_right(16.0)
                .with_padding_top(20.0)
                .with_padding_bottom(18.0)
                .finish(),
        )
        .with_child(
            // Loose flex: the body takes what the title and footer leave,
            // and only scrolls once its content needs more than that.
            Shrinkable::new(
                1.0,
                ClippedScrollable::vertical(
                    scroll.clone(),
                    Container::new(body)
                        .with_padding_left(DIALOG_PAD)
                        .with_padding_right(DIALOG_PAD)
                        .with_padding_bottom(DIALOG_PAD)
                        .finish(),
                    ScrollbarWidth::Auto,
                    theme.outline().into(),
                    theme.sub_text().into(),
                    ColorU::transparent_black().into(),
                )
                .with_overlayed_scrollbar()
                .finish(),
            )
            .finish(),
        );
    if let Some(footer) = footer {
        panel = panel.with_child(
            Container::new(footer)
                .with_background_color(theme.surface_1())
                .with_border(Border::top(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_bottom(Radius::Pixels(RADIUS_DIALOG)))
                .with_padding_left(DIALOG_PAD)
                .with_padding_right(DIALOG_PAD)
                .with_padding_top(14.0)
                .with_padding_bottom(14.0)
                .finish(),
        );
    }
    let panel = Container::new(panel.finish())
        .with_background_color(theme.solid_background())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS_DIALOG)))
        .with_drop_shadow(DropShadow {
            color: ColorU::new(0, 0, 0, 110),
            offset: vec2f(0.0, 18.0),
            blur_radius: 36.0,
            spread_radius: 0.0,
        })
        .finish();
    let panel = EventHandler::new(ConstrainedBox::new(panel).with_width(width).finish())
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();
    // The margin bounds the panel's height to the window, less a gap.
    let scrim = Container::new(Align::new(panel).finish())
        .with_background_color(ColorU::new(0, 0, 0, 160))
        .with_padding_top(DIALOG_MARGIN)
        .with_padding_bottom(DIALOG_MARGIN)
        .finish();
    EventHandler::new(scrim)
        .on_left_mouse_down(move |ctx, _, _| {
            if let Some(dismiss) = dismiss.clone() {
                ctx.dispatch_typed_action_for(view_id, dismiss);
            }
            DispatchEventResult::StopPropagation
        })
        .finish()
}

/// A dialog footer: an optional left-side control, then right-aligned
/// actions with the primary one last.
pub fn dialog_footer(left: Option<Box<dyn Element>>, actions: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut right = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(8.0);
    for action in actions {
        right = right.with_child(action);
    }
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(Expanded::new(1.0, left.unwrap_or_else(|| spacer(0.0))).finish())
        .with_child(right.finish())
        .finish()
}

/// A floating menu panel (⋯ menus, selects, the location picker).
pub fn menu_panel(theme: &Theme, rows: Vec<Box<dyn Element>>, width: f32) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(1.0);
    for row in rows {
        column = column.with_child(row);
    }
    let panel = Container::new(column.finish())
        .with_background_color(theme.sidebar_menu_background())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(5.0)
        .with_drop_shadow(DropShadow {
            color: ColorU::new(0, 0, 0, 90),
            offset: vec2f(0.0, 10.0),
            blur_radius: 24.0,
            spread_radius: 0.0,
        })
        .finish();
    EventHandler::new(ConstrainedBox::new(panel).with_width(width).finish())
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish()
}

/// A muted heading inside a menu ("Remote machines").
pub fn menu_heading(ui: FamilyId, theme: &Theme, label: &str) -> Box<dyn Element> {
    Container::new(eyebrow(ui, theme, label))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(8.0)
        .with_padding_bottom(4.0)
        .finish()
}

/// A hairline between menu groups, inset from the panel edge.
pub fn menu_separator(theme: &Theme) -> Box<dyn Element> {
    Container::new(divider(theme))
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
}

/// One menu row: optional leading element, title, optional detail line, and
/// a check mark when selected.
#[allow(clippy::too_many_arguments)]
pub fn menu_row<A: warpui::Action + Clone>(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    leading: Option<Box<dyn Element>>,
    title: &str,
    detail: Option<&str>,
    selected: bool,
    destructive: bool,
    action: A,
) -> Box<dyn Element> {
    let theme = *theme;
    let title = title.to_string();
    let detail = detail.map(str::to_string);
    let mut leading = leading;
    Hoverable::new(hover, move |state| {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0);
        if let Some(leading) = leading.take() {
            row = row.with_child(sized(leading, 18.0, 18.0));
        }
        let color = if destructive { theme.ui_red() } else { theme.main_text() };
        let mut labels = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(2.0)
            .with_child(line(ui, title.clone(), 13.0, color));
        if let Some(detail) = &detail {
            labels = labels.with_child(line(ui, detail.clone(), 11.5, theme.sub_text()));
        }
        row = row.with_child(Expanded::new(1.0, labels.finish()).finish());
        if selected {
            row = row.with_child(render_icon(AppIcon::Check, theme.accent, 13.0));
        }
        Container::new(row.finish())
            .with_background_color(if state.is_hovered() {
                theme.sidebar_menu_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(8.0)
            .with_padding_right(10.0)
            .with_padding_top(if detail.is_some() { 7.0 } else { 8.0 })
            .with_padding_bottom(if detail.is_some() { 7.0 } else { 8.0 })
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// Anchor `menu` under the bottom-right (or bottom-left) corner of `anchor`.
pub fn with_menu(anchor: Box<dyn Element>, menu: Option<Box<dyn Element>>, align_right: bool) -> Box<dyn Element> {
    let Some(menu) = menu else {
        return anchor;
    };
    let (parent, child) = if align_right {
        (ParentAnchor::BottomRight, ChildAnchor::TopRight)
    } else {
        (ParentAnchor::BottomLeft, ChildAnchor::TopLeft)
    };
    Stack::new()
        .with_child(anchor)
        .with_positioned_overlay_child(
            menu,
            OffsetPositioning::offset_from_parent(
                vec2f(0.0, 6.0),
                ParentOffsetBounds::WindowByPosition,
                parent,
                child,
            ),
        )
        .finish()
}

pub fn spacer(height: f32) -> Box<dyn Element> {
    ConstrainedBox::new(Empty::new().finish())
        .with_height(height)
        .finish()
}

/// A centred empty state: an icon tile, a title, a line of guidance, and an
/// optional call to action, inside a dashed-feeling bordered panel.
pub fn empty_state(
    theme: &Theme,
    ui: FamilyId,
    icon: AppIcon,
    title: &str,
    body: &str,
    action: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(icon_tile(theme, icon, 44.0, None))
        .with_child(spacer(14.0))
        .with_child(semibold(ui, title, 14.0, theme.main_text()))
        .with_child(spacer(6.0))
        .with_child(text(ui, body, 12.5, theme.sub_text()));
    if let Some(action) = action {
        column = column.with_child(spacer(18.0)).with_child(action);
    }
    Container::new(
        ConstrainedBox::new(Align::new(column.finish()).finish())
            .with_min_height(220.0)
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS_CARD)))
    .with_uniform_padding(24.0)
    .finish()
}

/// The first useful line of a raw SSH/runtime error, for titles and rows;
/// the full text stays available as notice detail.
pub fn first_line(error: &str) -> String {
    error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A machine's connection state as a badge, worded for people.
pub fn status_pill(theme: &Theme, ui: FamilyId, status: MachineStatus) -> Box<dyn Element> {
    match status {
        MachineStatus::Connected => pill(theme, ui, "Connected", Tone::Success),
        MachineStatus::Connecting => pill(theme, ui, "Connecting", Tone::Info),
        MachineStatus::Error => pill(theme, ui, "Unreachable", Tone::Error),
        MachineStatus::Idle => pill(theme, ui, "Not connected", Tone::Neutral),
    }
}

/// "macOS · Apple silicon", "Linux · x86-64" from the runtime's `os`/`arch`.
pub fn platform_label(os: &str, arch: &str) -> String {
    let os = match os {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        "freebsd" => "FreeBSD",
        other => other,
    };
    let arch = match arch {
        "aarch64" | "arm64" => "ARM64",
        "x86_64" | "amd64" => "x86-64",
        other => other,
    };
    format!("{os} · {arch}")
}

/// "1 project", "3 projects".
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// A clickable row inside a [`list_card`]: the hover highlight is inset and
/// rounded so it never collides with the card's own corners.
pub fn inset_hover(theme: &Theme, hovered: bool, row: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(
        Container::new(row)
            .with_background_color(if hovered {
                theme.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS_CONTROL)))
            .finish(),
    )
    .with_uniform_padding(4.0)
    .finish()
}

/// Overlay a small text tooltip under `child` while `hovered`.
pub fn with_tooltip(
    theme: &Theme,
    ui: FamilyId,
    child: Box<dyn Element>,
    hovered: bool,
    label: &str,
) -> Box<dyn Element> {
    use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
    use warpui::ui_components::tool_tip::Tooltip;
    if !hovered {
        return child;
    }
    let styles = UiComponentStyles {
        font_family_id: Some(ui),
        font_size: Some(crate::theme::text_size(11.5)),
        font_color: Some(theme.main_text()),
        background: Some(warpui_core::elements::Fill::Solid(theme.sidebar_menu_background())),
        border_color: Some(warpui_core::elements::Fill::Solid(theme.border())),
        border_width: Some(1.0),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(6.0))),
        padding: Some(Coords {
            top: 5.0,
            bottom: 5.0,
            left: 8.0,
            right: 8.0,
        }),
        ..Default::default()
    };
    Stack::new()
        .with_child(child)
        .with_positioned_overlay_child(
            Tooltip::new(label.to_string(), styles).build().finish(),
            OffsetPositioning::offset_from_parent(
                vec2f(0.0, 6.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::BottomMiddle,
                ChildAnchor::TopMiddle,
            ),
        )
        .finish()
}

/// Hover text for a machine's status dot: "Test server · Connected".
pub fn status_tooltip(machine: &str, status: MachineStatus) -> String {
    let state = match status {
        MachineStatus::Connected => "Connected",
        MachineStatus::Connecting => "Connecting…",
        MachineStatus::Error => "Unreachable",
        MachineStatus::Idle => "Not connected",
    };
    format!("{machine} · {state}")
}

#[cfg(test)]
mod dialog_tests {
    use std::collections::HashSet;

    use warpui::elements::SavePosition;
    use warpui::geometry::rect::RectF;
    use warpui::geometry::vector::Vector2F;
    use warpui_core::platform::WindowStyle;
    use warpui_core::units::IntoPixels;
    use warpui_core::{App, Entity, Presenter, TypedActionView, WindowInvalidation};

    use super::*;
    use crate::settings::SettingsAction;

    struct Dialog {
        view_id: EntityId,
        body_height: f32,
        scroll: ClippedScrollStateHandle,
    }

    impl Entity for Dialog {
        type Event = ();
    }

    impl warpui::View for Dialog {
        fn ui_name() -> &'static str {
            "Dialog"
        }

        fn render(&self, _: &warpui::AppContext) -> Box<dyn Element> {
            let block = |height: f32, id: &str| {
                SavePosition::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_height(height)
                        .finish(),
                    id,
                )
                .finish()
            };
            dialog(
                &Theme::dark(),
                FamilyId(0),
                self.view_id,
                MouseStateHandle::default(),
                &self.scroll,
                block(40.0, "title"),
                block(self.body_height, "body"),
                Some(block(40.0, "footer")),
                560.0,
                None::<SettingsAction>,
            )
        }
    }

    impl TypedActionView for Dialog {
        type Action = ();
    }

    const WINDOW_HEIGHT: f32 = 600.0;

    /// The title's, body's and footer's bounds with the dialog laid out in a
    /// 600px-tall window and its body scrolled by `scroll_by`.
    fn layout(body_height: f32, scroll_by: f32) -> [RectF; 3] {
        let bounds = std::sync::Arc::new(std::sync::Mutex::new(None));
        let found = bounds.clone();
        App::test((), |mut app| async move {
            let app = &mut app;
            let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| Dialog {
                view_id: ctx.view_id(),
                body_height,
                scroll: ClippedScrollStateHandle::new(),
            });
            let scroll = view.read(app, |view, _| view.scroll.clone());
            let mut presenter = Presenter::new(window_id);
            let invalidation = WindowInvalidation {
                updated: HashSet::from([app.root_view_id(window_id).unwrap()]),
                ..Default::default()
            };
            let window = Vector2F::new(800.0, WINDOW_HEIGHT);
            *found.lock().unwrap() = Some(app.update(move |ctx| {
                presenter.invalidate(invalidation.clone(), ctx);
                presenter.build_scene(window, 1.0, None, ctx);
                scroll.scroll_to(scroll_by.into_pixels());
                presenter.invalidate(invalidation, ctx);
                presenter.build_scene(window, 1.0, None, ctx);
                let cache = presenter.position_cache();
                ["title", "body", "footer"].map(|id| cache.get_position(id).unwrap())
            }));
        });
        let bounds = bounds.lock().unwrap().take();
        bounds.expect("dialog laid out")
    }

    #[test]
    fn a_tall_dialog_stays_inside_the_window_and_scrolls() {
        let [title, body, footer] = layout(2_000.0, 0.0);
        assert!(title.min_y() >= DIALOG_MARGIN, "title at {title:?}");
        assert!(
            footer.max_y() <= WINDOW_HEIGHT - DIALOG_MARGIN,
            "footer at {footer:?}, below the window"
        );
        // Scrolling moves the body under the fixed title and footer.
        let [title_after, scrolled, footer_after] = layout(2_000.0, 500.0);
        assert_eq!((title_after, footer_after), (title, footer));
        assert!((body.min_y() - scrolled.min_y() - 500.0).abs() < 0.5, "{body:?} -> {scrolled:?}");
    }

    #[test]
    fn a_short_dialog_keeps_its_height() {
        let [title, _, footer] = layout(100.0, 0.0);
        // Header padding, the body and its padding, the footer's border and
        // padding: the dialog is no taller than its content.
        let gap = footer.min_y() - title.max_y();
        assert!((gap - (18.0 + 100.0 + DIALOG_PAD + 1.0 + 14.0)).abs() < 0.5, "gap {gap}");
    }
}
