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
use crate::icons::AppIcon;
use crate::settings::widgets::SHORTCUT_ROW_HEIGHT;
use crate::settings::widgets::SHORTCUT_ROW_PADDING_X;
use crate::settings::widgets::SHORTCUT_ROW_PADDING_Y;
use crate::settings::widgets_rows::key_chip;

fn hint_chip(theme: &Theme, ui_font: FamilyId, label: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_string(), ui_font, crate::theme::text_size(11.0))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .finish()
}

fn tokenize_mac_keys(binding: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut current = String::new();
    for ch in binding.trim().chars() {
        if matches!(ch, '⌘' | '⇧' | '⌥' | '⌃') {
            if !current.is_empty() {
                keys.push(current.clone());
                current.clear();
            }
            keys.push(ch.to_string());
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        keys.push(current);
    }
    keys
}

fn is_keyboard_binding(binding: &str) -> bool {
    binding.contains('⌘')
        || binding.contains('⌃')
        || binding.contains('⇧')
        || binding.starts_with("Ctrl")
}

pub fn shortcut_binding_display(
    theme: &Theme,
    ui_font: FamilyId,
    binding: &str,
) -> Box<dyn Element> {
    if !is_keyboard_binding(binding) {
        return hint_chip(theme, ui_font, binding);
    }

    let alternatives: Vec<_> = binding.split(" / ").map(str::trim).collect();
    let mut alt_row = Flex::row().with_spacing(6.0);

    for (i, alt) in alternatives.iter().enumerate() {
        if i > 0 {
            alt_row = alt_row.with_child(
                Text::new("/".to_string(), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            );
        }
        let keys = tokenize_mac_keys(alt);
        let mut combo = Flex::row().with_spacing(3.0);
        for key in keys {
            combo = combo.with_child(key_chip(theme, ui_font, &key));
        }
        alt_row = alt_row.with_child(combo.finish());
    }

    alt_row.finish()
}

pub fn search_input_mock(theme: &Theme, ui_font: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_spacing(6.0)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(render_icon(AppIcon::Search, theme.disabled_text(), 11.0))
            .with_child(
                Text::new(
                    "Search by name or by keys (ex. \"cmd...\")".to_string(),
                    ui_font,
                    crate::theme::text_size(11.0),
                )
                .with_color(theme.disabled_text())
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_3())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .with_padding_left(8.0)
    .with_padding_right(16.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish()
}

pub fn shortcuts_table_header(theme: &Theme, ui_font: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Text::new("Command".to_string(), ui_font, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .finish(),
            )
            .with_child(Shrinkable::new(1.0, search_input_mock(theme, ui_font)).finish())
            .with_child(
                Text::new("Shortcut".to_string(), ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(SHORTCUT_ROW_PADDING_X)
    .with_padding_right(SHORTCUT_ROW_PADDING_X)
    .with_padding_top(14.0)
    .with_padding_bottom(14.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
    .finish()
}

pub fn shortcuts_section_header(
    theme: &Theme,
    ui_font: FamilyId,
    title: &str,
    first: bool,
) -> Box<dyn Element> {
    let mut container = Container::new(
        Text::new(title.to_uppercase(), ui_font, crate::theme::text_size(10.0))
            .with_color(theme.disabled_text())
            .finish(),
    )
    .with_padding_left(SHORTCUT_ROW_PADDING_X)
    .with_padding_right(SHORTCUT_ROW_PADDING_X)
    .with_padding_top(if first { 12.0 } else { 18.0 })
    .with_padding_bottom(10.0);

    if !first {
        container = container.with_border(Border::top(1.0).with_border_color(theme.outline()));
    }

    container.finish()
}

pub fn shortcut_row(
    theme: &Theme,
    ui_font: FamilyId,
    command: &str,
    binding: &str,
    zebra: bool,
    show_divider: bool,
) -> Box<dyn Element> {
    let bg = if zebra {
        theme.fg_overlay_1()
    } else {
        ColorU::new(0, 0, 0, 0)
    };

    let mut row_container = Container::new(
        ConstrainedBox::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(command.to_string(), ui_font, crate::theme::text_size(13.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(shortcut_binding_display(theme, ui_font, binding))
                .finish(),
        )
        .with_min_height(SHORTCUT_ROW_HEIGHT - SHORTCUT_ROW_PADDING_Y * 2.0)
        .finish(),
    )
    .with_background_color(bg)
    .with_padding_left(SHORTCUT_ROW_PADDING_X)
    .with_padding_right(SHORTCUT_ROW_PADDING_X)
    .with_padding_top(SHORTCUT_ROW_PADDING_Y)
    .with_padding_bottom(SHORTCUT_ROW_PADDING_Y);

    if show_divider {
        row_container =
            row_container.with_border(Border::bottom(1.0).with_border_color(theme.outline()));
    }

    row_container.finish()
}

pub fn shortcuts_table(
    theme: &Theme,
    ui_font: FamilyId,
    title: &str,
    rows: Vec<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut inner = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(shortcuts_table_header(theme, ui_font));

    for row in rows {
        inner = inner.with_child(row);
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
        .with_child(
            Container::new(inner.finish())
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .finish(),
        )
        .with_child(
            Container::new(warpui::elements::Empty::new().finish())
                .with_padding_bottom(16.0)
                .finish(),
        )
        .finish()
}
