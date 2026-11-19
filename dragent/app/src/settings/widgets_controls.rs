//! Shared settings row widgets — Dragent settings design system.

use warpui::color::ColorU;
use warpui::elements::{
    Align, CacheOption, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, Element, Expanded, Flex, Image, MainAxisAlignment, MainAxisSize,
    ParentElement, ScrollbarWidth, Shrinkable, Stack, Text, Wrap,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Fill, Hoverable, MouseStateHandle, WrapFill};
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::ui_components::components::UiComponent;
use warpui_core::ui_components::switch::{Switch, SwitchStateHandle};

use crate::theme::Theme;

use super::action::SettingsAction;
use crate::icons::render_icon;
use crate::icons::AppIcon;

pub fn link_button(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let color = if state.is_hovered() {
                theme.accent
            } else {
                theme.link()
            };
            Container::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                    .with_color(color)
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(if state.is_hovered() {
                theme.link_surface()
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

pub fn primary_button(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if state.is_hovered() {
                ColorU::new(
                    theme.accent.r.saturating_add(15),
                    theme.accent.g.saturating_add(15),
                    theme.accent.b.saturating_add(15),
                    theme.accent.a,
                )
            } else {
                theme.accent
            };
            Container::new(Box::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                    .with_color(theme.accent_text()),
            ))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

/// Generic over the action so views outside Settings can share dialogs
/// built from it (e.g. the "Log out?" confirmation).
pub fn secondary_button<A: warpui::Action + Clone>(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: A,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let border = if state.is_hovered() {
                theme.accent
            } else {
                theme.outline()
            };
            let color = if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            let bg = if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            Container::new(Box::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                    .with_color(color),
            ))
            .with_border(Border::all(1.0).with_border_color(border))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

pub fn accent_outline_button(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if state.is_hovered() {
                theme.accent_overlay()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            Container::new(Box::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                    .with_color(theme.accent),
            ))
            .with_border(Border::all(1.0).with_border_color(theme.accent))
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

pub fn danger_button(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let red = theme.ui_red();
            let border = if state.is_hovered() {
                red
            } else {
                theme.outline()
            };
            let bg = if state.is_hovered() {
                ColorU::new(red.r, red.g, red.b, 22)
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            Container::new(Box::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0)).with_color(red),
            ))
            .with_border(Border::all(1.0).with_border_color(border))
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

pub struct SegmentOption {
    pub label: String,
    pub selected: bool,
    pub hover: MouseStateHandle,
    pub action: SettingsAction,
}

/// Pill-style segmented control (Manual / Suggest / Auto …). Stateless: the
/// selected segment comes from settings, clicks dispatch the given action.
pub fn segmented_control(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    options: Vec<SegmentOption>,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(2.0);

    for opt in options {
        let SegmentOption {
            label,
            selected,
            hover,
            action,
        } = opt;
        let theme = *theme;
        row = row.with_child(Box::new(
            Hoverable::new(hover, move |state| {
                let bg = if selected {
                    theme.surface_1()
                } else if state.is_hovered() {
                    theme.surface_2()
                } else {
                    ColorU::new(0, 0, 0, 0)
                };
                let color = if selected {
                    theme.main_text()
                } else {
                    theme.sub_text()
                };
                Container::new(
                    Text::new(label.clone(), ui_font, crate::theme::text_size(12.5))
                        .with_color(color)
                        .finish(),
                )
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(13.0)
                .with_padding_right(13.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, action.clone());
            }),
        ));
    }

    Container::new(row.finish())
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_uniform_padding(3.0)
        .finish()
}

pub struct SelectItem {
    pub label: String,
    pub selected: bool,
    /// Disabled items render greyed out and do not dispatch (e.g. "Cloud —
    /// coming soon").
    pub enabled: bool,
    pub hover: MouseStateHandle,
    pub action: SettingsAction,
}

/// Dropdown select: a button showing the current value; when open, an overlay
/// list of items. Open state lives in `SettingsView::open_picker` keyed by
/// `toggle_action`'s picker key, so at most one dropdown is open at a time.
pub fn select_control(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    button_hover: MouseStateHandle,
    current_label: &str,
    width: f32,
    is_open: bool,
    toggle_action: SettingsAction,
    items: Vec<SelectItem>,
) -> Box<dyn Element> {
    select_control_with_scroll(
        theme,
        ui_font,
        view_id,
        button_hover,
        current_label,
        width,
        is_open,
        toggle_action,
        items,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn select_control_with_scroll(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    button_hover: MouseStateHandle,
    current_label: &str,
    width: f32,
    is_open: bool,
    toggle_action: SettingsAction,
    items: Vec<SelectItem>,
    scroll: Option<ClippedScrollStateHandle>,
) -> Box<dyn Element> {
    let label_owned = current_label.to_string();
    let chevron = if is_open { "▴" } else { "▾" };
    let chevron_owned = chevron.to_string();
    let border_color = if is_open {
        theme.accent
    } else {
        theme.outline()
    };
    let theme_c = *theme;

    let button: Box<dyn Element> = Box::new(
        Hoverable::new(button_hover, move |state| {
            let bg = if state.is_hovered() {
                theme_c.surface_2()
            } else {
                theme_c.surface_1()
            };
            let inner = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                            .with_color(theme_c.main_text())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Text::new(chevron_owned.clone(), ui_font, crate::theme::text_size(10.0))
                        .with_color(theme_c.sub_text())
                        .finish(),
                )
                .finish();
            ConstrainedBox::new(
                Container::new(inner)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(border_color))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .with_padding_left(12.0)
                    .with_padding_right(10.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(8.0)
                    .finish(),
            )
            .with_min_width(width)
            .with_max_width(width)
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, toggle_action.clone());
        }),
    );

    if !is_open || items.is_empty() {
        return button;
    }

    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.0);

    for item in items {
        let SelectItem {
            label,
            selected,
            enabled,
            hover,
            action,
        } = item;
        let theme_i = *theme;
        let el: Box<dyn Element> = Box::new(
            Hoverable::new(hover, move |state| {
                let bg = if selected {
                    theme_i.accent_overlay()
                } else if enabled && state.is_hovered() {
                    theme_i.surface_2()
                } else {
                    ColorU::new(0, 0, 0, 0)
                };
                let color = if !enabled {
                    theme_i.disabled_text()
                } else if selected {
                    theme_i.accent
                } else {
                    theme_i.main_text()
                };
                let row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Shrinkable::new(
                            1.0,
                            Text::new(label.clone(), ui_font, crate::theme::text_size(13.0))
                                .with_color(color)
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(if selected {
                        render_icon(AppIcon::Check, theme_i.accent, 12.0)
                    } else {
                        warpui::elements::Empty::new().finish()
                    })
                    .finish();
                Container::new(row)
                    .with_background_color(bg)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .finish()
            })
            .on_click(move |ctx, _, _| {
                if enabled {
                    ctx.dispatch_typed_action_for(view_id, action.clone());
                }
            }),
        );
        list = list.with_child(el);
    }

    let list: Box<dyn Element> = if let Some(scroll) = scroll {
        ConstrainedBox::new(
            ClippedScrollable::vertical(
                scroll,
                list.finish(),
                ScrollbarWidth::Auto,
                theme.outline().into(),
                theme.sub_text().into(),
                theme.surface_1().into(),
            )
            .finish(),
        )
        .with_max_height(300.0)
        .finish()
    } else {
        list.finish()
    };
    let dropdown = Container::new(list)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(6.0)
        .finish();

    let dropdown_constrained = ConstrainedBox::new(dropdown)
        .with_min_width(width)
        .with_max_width(width.max(220.0))
        .finish();

    let offset = warpui::elements::OffsetPositioning::offset_from_parent(
        warpui::geometry::vector::vec2f(0.0, 4.0),
        warpui::elements::ParentOffsetBounds::WindowByPosition,
        warpui::elements::ParentAnchor::BottomLeft,
        warpui::elements::ChildAnchor::TopLeft,
    );

    Box::new(
        warpui::elements::Stack::new()
            .with_child(button)
            .with_positioned_overlay_child(dropdown_constrained, offset),
    )
}

pub fn slider_control(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    state: warpui_core::ui_components::slider::SliderStateHandle,
    value: f32,
    range: std::ops::Range<f32>,
    step: f32,
    display_value: String,
    width: f32,
    to_action: impl Fn(f32) -> SettingsAction + 'static,
) -> Box<dyn Element> {
    let slider = warpui_core::ui_components::slider::Slider::new(state)
        .with_range(range)
        .with_step(step)
        .with_default_value(value)
        .with_track_height(4.0)
        .with_thumb_size(14.0)
        .with_track_fill(Fill::Solid(theme.surface_3()))
        .with_thumb_fill(Fill::Solid(theme.foreground))
        .on_change(move |ctx, _, v| {
            ctx.dispatch_typed_action_for(view_id, to_action(v));
        })
        .build()
        .finish();

    Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(
            ConstrainedBox::new(slider)
                .with_min_width(width)
                .with_max_width(width)
                .finish(),
        )
        .with_child(
            ConstrainedBox::new(
                Text::new(display_value, ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_min_width(44.0)
            .finish(),
        )
        .finish()
}
