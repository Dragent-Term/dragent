//! Shared settings row widgets — Dragent settings design system.

use warpui::color::ColorU;
use warpui::elements::{
    Align, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex,
    Image, MainAxisAlignment, MainAxisSize, ParentElement, Shrinkable, Stack, Text, Wrap,
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
use crate::settings::widgets::with_alpha;
use crate::settings::widgets_controls::secondary_button;
use crate::settings::widgets_panels::circular_avatar;
use crate::settings::widgets_panels::status_badge;

pub fn info_row(theme: &Theme, ui_font: FamilyId, label: &str, value: &str) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                Text::new(value.to_string(), ui_font, crate::theme::text_size(13.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(10.0)
    .with_padding_bottom(10.0)
    .finish()
}

pub fn row_divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(warpui::elements::Empty::new().finish())
        .with_margin_left(12.0)
        .with_margin_right(12.0)
        .with_margin_top(8.0)
        .with_margin_bottom(8.0)
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
}

pub fn key_chip(theme: &Theme, ui_font: FamilyId, key: &str) -> Box<dyn Element> {
    let is_wide = key.chars().count() > 1;
    let horizontal = if is_wide { 8.0 } else { 6.0 };
    let min_w = if is_wide { 0.0 } else { 22.0 };

    let text = Container::new(
        Text::new(key.to_string(), ui_font, crate::theme::text_size(11.0))
            .with_color(theme.accent)
            .finish(),
    )
    .with_padding_left(horizontal)
    .with_padding_right(horizontal)
    .with_padding_top(5.0)
    .with_padding_bottom(5.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(with_alpha(theme.accent, 0.5)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .finish();

    if min_w > 0.0 {
        ConstrainedBox::new(text).with_min_width(min_w).finish()
    } else {
        text
    }
}

pub fn divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(warpui::elements::Empty::new().finish())
        .with_padding_top(16.0)
        .with_padding_bottom(16.0)
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
}

pub fn disabled_choice_row(
    theme: &Theme,
    ui_font: FamilyId,
    label: &str,
    hint: &str,
) -> Box<dyn Element> {
    let radio = ConstrainedBox::new(
        Container::new(warpui::elements::Empty::new().finish())
            .with_border(Border::all(1.5).with_border_color(theme.surface_3()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
    )
    .with_width(14.0)
    .with_height(14.0)
    .finish();

    let label_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                .with_color(theme.disabled_text())
                .finish(),
        )
        .with_child(
            Container::new(
                Text::new(hint.to_string(), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_top(2.0)
            .finish(),
        )
        .finish();

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1.0, label_col).finish())
            .with_child(radio)
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(11.0)
    .with_padding_bottom(11.0)
    .finish()
}

pub fn settings_card(theme: &Theme, child: Box<dyn Element>) -> Box<dyn Element> {
    // No card fill (border-only delineation — see shared input work).
    // Revert to theme.surface_1() to restore filled cards.
    Container::new(child)
        .with_background_color(ColorU::new(0, 0, 0, 0))
        .with_uniform_padding(14.0)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish()
}

pub fn setting_group(
    theme: &Theme,
    ui_font: FamilyId,
    title: &str,
    children: Vec<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut inner = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0);

    for (i, child) in children.into_iter().enumerate() {
        if i > 0 {
            inner = inner.with_child(row_divider(theme));
        }
        inner = inner.with_child(child);
    }

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(8.0)
        .with_child(
            Container::new(
                Text::new(title.to_string(), ui_font, crate::theme::text_size(13.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(4.0)
            .finish(),
        )
        .with_child(settings_card(theme, inner.finish()))
        .with_child(
            Container::new(warpui::elements::Empty::new().finish())
                .with_padding_bottom(16.0)
                .finish(),
        )
        .finish()
}

pub fn repo_index_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    reindex_hover: MouseStateHandle,
    name: &str,
    detail: &str,
    file_count: u32,
    root_hash: &str,
    reindex_action: SettingsAction,
) -> Box<dyn Element> {
    let hash_short = if root_hash.len() > 12 {
        format!("{}…", &root_hash[..12])
    } else {
        root_hash.to_string()
    };

    let details = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Text::new(name.to_string(), ui_font, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Container::new(
                Text::new(detail.to_string(), ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_top(3.0)
            .finish(),
        )
        .with_child(
            Container::new(
                Text::new(format!("{file_count} files · {hash_short}"), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            )
            .with_padding_top(3.0)
            .finish(),
        )
        .finish();

    let card_content = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Flex::row()
                .with_spacing(12.0)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(circular_avatar(theme, ui_font, name))
                .with_child(details)
                .finish(),
        )
        .with_child(
            Flex::row()
                .with_spacing(8.0)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(status_badge(theme, ui_font, "Ready", true))
                .with_child(secondary_button(
                    theme,
                    ui_font,
                    view_id,
                    reindex_hover,
                    "Re-index",
                    reindex_action,
                ))
                .finish(),
        )
        .finish();

    Container::new(card_content)
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

pub fn toggle_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    description: Option<&str>,
    enabled: bool,
    action: SettingsAction,
) -> Box<dyn Element> {
    toggle_row_with_icon(theme, ui_font, view_id, hover, None, label, description, enabled, action)
}

/// A [`toggle_row`] whose label has an icon beside it (an agent's, say).
#[allow(clippy::too_many_arguments)]
pub fn toggle_row_with_icon(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    icon: Option<Box<dyn Element>>,
    label: &str,
    description: Option<&str>,
    enabled: bool,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let desc_owned = description.map(str::to_string);
    let click_action = action.clone();

    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::new(0, 0, 0, 0)
            };

            let title = Text::new(label_owned.clone(), ui_font, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .finish();
            let title = match icon {
                Some(icon) => Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(icon)
                    .with_child(title)
                    .finish(),
                None => title,
            };
            let mut label_col = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(title);
            if let Some(desc) = &desc_owned {
                label_col = label_col.with_child(
                    Container::new(
                        Text::new(desc.clone(), ui_font, crate::theme::text_size(12.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .with_padding_top(3.0)
                    .finish(),
                );
            }

            let switch_state = SwitchStateHandle::default();
            let toggle = Switch::new(
                switch_state,
                warpui_core::ui_components::components::UiComponentStyles {
                    background: Some(Fill::Solid(theme.surface_3())),
                    foreground: Some(Fill::Solid(theme.foreground)),
                    ..Default::default()
                },
                None,
                Some(warpui_core::ui_components::components::UiComponentStyles {
                    background: Some(Fill::Solid(theme.accent)),
                    ..Default::default()
                }),
                None,
            )
            .check(enabled)
            .build()
            .on_click({
                let action_clone = click_action.clone();
                move |ctx: &mut warpui_core::elements::EventContext, _, _| {
                    ctx.dispatch_typed_action_for(view_id, action_clone.clone());
                }
            })
            .finish();

            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(Shrinkable::new(1.0, label_col.finish()).finish())
                    .with_child(toggle)
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

pub fn choice_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    selected: bool,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::new(0, 0, 0, 0)
            };

            let radio = ConstrainedBox::new(if selected {
                Container::new(
                    Align::new(
                        Container::new(warpui::elements::Empty::new().finish())
                            .with_background_color(theme.accent)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                            .finish(),
                    )
                    .finish(),
                )
                .with_uniform_padding(3.0)
                .with_border(Border::all(1.5).with_border_color(theme.accent))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish()
            } else {
                Container::new(warpui::elements::Empty::new().finish())
                    .with_border(Border::all(1.5).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                    .finish()
            })
            .with_width(14.0)
            .with_height(14.0)
            .finish();

            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                            .with_color(if selected {
                                theme.main_text()
                            } else {
                                theme.sub_text()
                            })
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(radio)
                .finish();

            Container::new(row)
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(11.0)
                .with_padding_bottom(11.0)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}

/// Standalone terminal-mockup preview card for a theme (Warp chooser
/// pattern): theme-background card, accent border when selected. Used by the
/// full `theme_row` below and by the theme-chooser side panel + hero card.
pub fn theme_preview_card(
    chrome_theme: &Theme,
    option_theme: &Theme,
    ui_font: FamilyId,
    selected: bool,
    hovered: bool,
    thumb: Option<AssetSource>,
) -> Box<dyn Element> {
    let font = ui_font;
    let sz = crate::theme::text_size(11.5);
    let fg = option_theme.foreground;
    let dir_c = option_theme.ansi_blue();
    let exec_c = option_theme.ui_red();
    let file_c = option_theme.foreground;
    let cursor_c = option_theme.accent;
    let card_bg = ColorU::new(
        option_theme.background.r,
        option_theme.background.g,
        option_theme.background.b,
        255,
    );
    let sep_c = option_theme.outline();

    let seg = move |text: &str, color: ColorU| -> Box<dyn Element> {
        Text::new(text.to_string(), font, sz)
            .with_color(color)
            .finish()
    };

    // Mockup: a couple of output rows, a divider, then a prompt cursor —
    // matching a real terminal, rendered in this theme's palette.
    let output = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0)
        .with_child(seg("ls", fg))
        .with_child(
            Flex::row()
                .with_spacing(14.0)
                .with_child(seg("dir", dir_c))
                .with_child(seg("executable", exec_c))
                .with_child(seg("file", file_c))
                .finish(),
        )
        .finish();
    let output = Container::new(output).with_padding_bottom(14.0).finish();

    let divider = Container::new(
        ConstrainedBox::new(warpui::elements::Empty::new().finish())
            .with_height(1.0)
            .finish(),
    )
    .with_background_color(sep_c)
    .finish();

    let prompt = Container::new(seg("|", cursor_c))
        .with_padding_top(8.0)
        .finish();

    let mockup = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(output)
        .with_child(divider)
        .with_child(prompt)
        .finish();

    // The preview card, in the theme's own background, selected → accent
    // border (thicker), hovered → brighter outline.
    let (border_w, border_c) = if selected {
        (2.0, cursor_c)
    } else if hovered {
        (1.0, chrome_theme.main_text())
    } else {
        (1.0, chrome_theme.outline())
    };
    let padded_mockup = Container::new(mockup)
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(12.0)
        .with_padding_bottom(10.0)
        .finish();
    // With a background image, preview it behind the mockup. `BySize`
    // resizes to the (small, fixed-height) card so the image FILLS it and
    // does NOT drive the layout to its full source size — `Original` did,
    // which ballooned the cards. A scrim tinted to the theme background
    // keeps the mockup text readable over the image (like Warp).
    let card_content: Box<dyn Element> = match &thumb {
        Some(src) => {
            let base = Container::new(warpui::elements::Empty::new().finish())
                .with_background_color(card_bg)
                .finish();
            let image = Image::new(src.clone(), CacheOption::BySize)
                .cover()
                .finish();
            let scrim = Container::new(warpui::elements::Empty::new().finish())
                .with_background_color(ColorU::new(card_bg.r, card_bg.g, card_bg.b, 140))
                .finish();
            Box::new(
                Stack::new()
                    .with_child(base)
                    .with_child(image)
                    .with_child(scrim)
                    .with_child(padded_mockup),
            )
        }
        None => padded_mockup,
    };
    // Fixed height → uniform cards, and it bounds the image so it can't
    // expand (an unbounded `.cover()` panicked the settings scroll).
    let card_content = ConstrainedBox::new(card_content)
        .with_height(104.0)
        .finish();
    Container::new(card_content)
        .with_background_color(card_bg)
        .with_border(Border::all(border_w).with_border_color(border_c))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish()
}

pub fn theme_row(
    chrome_theme: &Theme,
    option_theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    id: &str,
    selected: bool,
    // Small thumbnail source for themes with a background image — rendered behind
    // the mockup so users see the actual image they're picking.
    thumb: Option<AssetSource>,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let id_owned = id.to_string();

    Box::new(
        Hoverable::new(hover, move |state| {
            let card = theme_preview_card(
                chrome_theme,
                option_theme,
                ui_font,
                selected,
                state.is_hovered(),
                thumb.clone(),
            );
            let name = Container::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                    .with_color(if selected {
                        chrome_theme.accent
                    } else {
                        chrome_theme.main_text()
                    })
                    .finish(),
            )
            .with_padding_left(2.0)
            .finish();

            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0)
                .with_child(card)
                .with_child(name)
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, SettingsAction::SelectTheme(id_owned.clone()));
        }),
    )
}

pub fn group_label(theme: &Theme, ui_font: FamilyId, text: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_uppercase(), ui_font, crate::theme::text_size(10.5))
            .with_color(theme.disabled_text())
            .finish(),
    )
    .with_padding_top(20.0)
    .with_padding_bottom(8.0)
    .finish()
}

pub fn card_rows(theme: &Theme, children: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut inner = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    for (i, child) in children.into_iter().enumerate() {
        if i > 0 {
            inner = inner.with_child(
                Container::new(child)
                    .with_border(Border::top(1.0).with_border_color(theme.outline()))
                    .finish(),
            );
        } else {
            inner = inner.with_child(child);
        }
    }
    Container::new(inner.finish())
        // No card fill (border-only delineation — see shared input work).
        // Revert to theme.surface_1() to restore filled cards.
        .with_background_color(ColorU::new(0, 0, 0, 0))
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .finish()
}

/// Width of a text field (or a field with its button) in a [`setting_row`].
pub const ROW_FIELD_WIDTH: f32 = 320.0;

/// A text field for the control side of a [`setting_row`].
pub fn row_field(
    theme: &Theme,
    editor: &warpui::ViewHandle<crate::workspace::InputEditor>,
) -> Box<dyn Element> {
    use crate::shared_input::{shared_input, SharedInputOpts, SharedInputWidth};
    shared_input(
        theme,
        editor,
        SharedInputOpts {
            width: SharedInputWidth::Fixed(ROW_FIELD_WIDTH),
            ..Default::default()
        },
    )
}

/// A control that fills its width (a field with a button beside it) at the
/// width of a [`row_field`].
pub fn row_control(control: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(control).with_width(ROW_FIELD_WIDTH).finish()
}

/// The control is laid out with unbounded width, so one that fills its width
/// (a text field, a row with an `Expanded` or `Shrinkable` child) must be
/// sized first, with [`row_field`] or [`row_control`]; debug builds panic
/// otherwise.
pub fn setting_row(
    theme: &Theme,
    ui_font: FamilyId,
    label: &str,
    desc: Option<&str>,
    action_child: Box<dyn Element>,
) -> Box<dyn Element> {
    setting_row_with_icon(theme, ui_font, None, label, desc, action_child)
}

/// A [`setting_row`] whose label has an icon beside it (an agent's, say).
pub fn setting_row_with_icon(
    theme: &Theme,
    ui_font: FamilyId,
    icon: Option<Box<dyn Element>>,
    label: &str,
    desc: Option<&str>,
    action_child: Box<dyn Element>,
) -> Box<dyn Element> {
    let title = Text::new(label.to_string(), ui_font, crate::theme::text_size(13.5))
        .with_color(theme.main_text())
        .finish();
    let title = match icon {
        Some(icon) => Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(icon)
            .with_child(title)
            .finish(),
        None => title,
    };
    let mut label_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(title);
    if let Some(d) = desc {
        label_col = label_col.with_child(
            Container::new(
                Text::new(d.to_string(), ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_top(3.0)
            .finish(),
        );
    }
    Container::new(
        ConstrainedBox::new(
            Wrap::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(16.0)
                .with_run_spacing(12.0)
                .with_child(WrapFill::new(200.0, label_col.finish()).finish())
                .with_child(action_child)
                .finish(),
        )
        .with_max_width(960.0)
        .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(13.0)
    .with_padding_bottom(13.0)
    .finish()
}
