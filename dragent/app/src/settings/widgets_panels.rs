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
use crate::icons::render_icon;
use crate::settings::widgets::with_alpha;
use crate::settings::widgets::NAV_ITEM_H_MARGIN;
use crate::settings::widgets::NAV_ITEM_RADIUS;
use crate::settings::widgets::PAGE_CAPTION_SIZE;
use crate::settings::widgets::PAGE_TITLE_SIZE;
use crate::settings::widgets_nav::map_icon_str;
use crate::settings::widgets_rows::settings_card;

pub fn account_avatar(theme: &Theme, ui_font: FamilyId, initials: &str) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Align::new(Box::new(
                Text::new(initials.to_string(), ui_font, crate::theme::text_size(17.0))
                    .with_color(theme.main_text()),
            ))
            .finish(),
        )
        .with_width(52.0)
        .with_height(52.0)
        .finish(),
    )
    .with_background_color(theme.accent_bg())
    .with_border(Border::all(2.0).with_border_color(theme.accent))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(26.0)))
    .finish()
}

pub fn page_header(
    theme: &Theme,
    ui_font: FamilyId,
    title: &str,
    caption: &str,
) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Container::new(Box::new(
                Text::new(
                    title.to_string(),
                    ui_font,
                    crate::theme::text_size(PAGE_TITLE_SIZE),
                )
                .with_color(theme.main_text()),
            ))
            .with_padding_bottom(6.0)
            .finish(),
        )
        .with_child(
            Container::new(Box::new(
                Text::new(
                    caption.to_string(),
                    ui_font,
                    crate::theme::text_size(PAGE_CAPTION_SIZE),
                )
                .with_color(theme.sub_text()),
            ))
            .with_padding_bottom(32.0)
            .finish(),
        )
        .finish()
}

fn nav_item_inner(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    icon: &str,
    action: SettingsAction,
    selected: bool,
    pad_left: f32,
    font_size: f32,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let icon_owned = icon.to_string();
    let click_action = action.clone();
    Box::new(
        Hoverable::new(hover, move |state| {
            let hovered = state.is_hovered();
            let bg = if selected {
                theme.accent_bg()
            } else if hovered {
                theme.sidebar_row_hover()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let color = if selected || hovered {
                theme.main_text()
            } else {
                theme.sub_text()
            };

            let icon_color = if selected { theme.accent } else { color };

            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(render_icon(map_icon_str(&icon_owned), icon_color, 14.0))
                .with_child(
                    Text::new(label_owned.clone(), ui_font, crate::theme::text_size(font_size))
                        .with_color(color)
                        .finish(),
                )
                .finish();

            Container::new(row)
                .with_margin_left(NAV_ITEM_H_MARGIN)
                .with_margin_right(NAV_ITEM_H_MARGIN)
                .with_margin_top(2.0)
                .with_margin_bottom(2.0)
                .with_padding_left(pad_left)
                .with_padding_right(12.0)
                .with_padding_top(7.0)
                .with_padding_bottom(7.0)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(NAV_ITEM_RADIUS)))
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, click_action.clone());
        }),
    )
}

pub fn alert_banner(theme: &Theme, ui_font: FamilyId, message: &str) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(10.0)
            .with_child(
                Container::new(
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_width(3.0)
                        .with_height(14.0)
                        .finish(),
                )
                .with_background_color(theme.accent)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.0,
                    Box::new(
                        Text::new(message.to_string(), ui_font, crate::theme::text_size(12.0))
                            .with_color(theme.main_text()),
                    ),
                )
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.accent_overlay())
    .with_border(Border::all(1.0).with_border_color(with_alpha(theme.accent, 0.35)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(12.0)
    .with_padding_bottom(12.0)
    .finish()
}

pub fn empty_state(theme: &Theme, ui_font: FamilyId, title: &str, body: &str) -> Box<dyn Element> {
    settings_card(
        theme,
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(
                Text::new(title.to_string(), ui_font, crate::theme::text_size(15.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Container::new(Box::new(
                    Text::new(body.to_string(), ui_font, crate::theme::text_size(13.0))
                        .with_color(theme.sub_text()),
                ))
                .with_padding_top(8.0)
                .finish(),
            )
            .finish(),
    )
}

/// Calm placeholder where a feature needs the Dragent server and this build
/// runs without one (local mode). Not an error: nothing failed.
pub fn needs_account_state(theme: &Theme, ui_font: FamilyId) -> Box<dyn Element> {
    empty_state(
        theme,
        ui_font,
        crate::api::NEEDS_ACCOUNT,
        crate::api::NEEDS_ACCOUNT_DETAIL,
    )
}

pub fn status_badge(
    theme: &Theme,
    ui_font: FamilyId,
    label: &str,
    ready: bool,
) -> Box<dyn Element> {
    let (bg, color) = if ready {
        let c = theme.ui_green();
        (ColorU::new(c.r, c.g, c.b, 40), theme.ui_green())
    } else {
        let c = theme.ui_yellow();
        (ColorU::new(c.r, c.g, c.b, 40), theme.ui_yellow())
    };
    Container::new(Box::new(
        Text::new(label.to_string(), ui_font, crate::theme::text_size(11.0)).with_color(color),
    ))
    .with_background_color(bg)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .finish()
}

pub fn circular_avatar(theme: &Theme, ui_font: FamilyId, text: &str) -> Box<dyn Element> {
    let text_owned = text
        .chars()
        .next()
        .unwrap_or(' ')
        .to_uppercase()
        .to_string();
    Container::new(
        ConstrainedBox::new(
            Align::new(Box::new(
                Text::new(text_owned, ui_font, crate::theme::text_size(13.0))
                    .with_color(theme.main_text()),
            ))
            .finish(),
        )
        .with_width(32.0)
        .with_height(32.0)
        .finish(),
    )
    .with_background_color(theme.surface_3())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(16.0)))
    .finish()
}

pub fn usage_percent_row(
    theme: &Theme,
    ui_font: FamilyId,
    label: &str,
    used: i64,
    max: i64,
    bar_color: ColorU,
) -> Box<dyn Element> {
    let ratio = if max > 0 {
        (used as f32 / max as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let pct = (ratio * 100.0).round() as u32;
    // Keep a sliver visible at 0% and never overflow at 100%.
    let fill = ratio.clamp(0.001, 1.0);
    let empty = (1.0 - fill).max(0.0);

    let track_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            Expanded::new(
                fill,
                Container::new(warpui::elements::Empty::new().finish())
                    .with_background_color(bar_color)
                    .finish(),
            )
            .finish(),
        )
        .with_child(Expanded::new(empty, warpui::elements::Empty::new().finish()).finish())
        .finish();

    let track = ConstrainedBox::new(
        Container::new(track_row)
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish(),
    )
    .with_height(6.0)
    .finish();

    let label_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(format!("{pct}%"), ui_font, crate::theme::text_size(12.0))
                .with_color(if pct >= 90 {
                    theme.ui_red()
                } else {
                    theme.sub_text()
                })
                .finish(),
        )
        .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.0)
            .with_child(label_row)
            .with_child(track)
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(14.0)
    .with_padding_bottom(14.0)
    .finish()
}

fn format_num(n: u32) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        let pos_from_end = bytes.len() - 1 - i;
        result.push(b as char);
        if pos_from_end > 0 && pos_from_end % 3 == 0 {
            result.push(',');
        }
    }
    result
}

pub fn usage_progress_row(
    theme: &Theme,
    ui_font: FamilyId,
    label: &str,
    used: u32,
    max: u32,
    bar_color: ColorU,
) -> Box<dyn Element> {
    let fill = if max > 0 {
        (used as f32 / max as f32).clamp(0.001, 0.999)
    } else {
        0.001
    };
    let empty = 1.0 - fill;

    let track_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            Expanded::new(
                fill,
                Container::new(warpui::elements::Empty::new().finish())
                    .with_background_color(bar_color)
                    .finish(),
            )
            .finish(),
        )
        .with_child(Expanded::new(empty, warpui::elements::Empty::new().finish()).finish())
        .finish();

    let track = ConstrainedBox::new(
        Container::new(track_row)
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish(),
    )
    .with_height(6.0)
    .finish();

    let label_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(
                format!("{} / {}", format_num(used), format_num(max)),
                ui_font,
                crate::theme::text_size(12.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.0)
            .with_child(label_row)
            .with_child(track)
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(14.0)
    .with_padding_bottom(14.0)
    .finish()
}
