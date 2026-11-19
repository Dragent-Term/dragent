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
use crate::settings::widgets::NAV_ITEM_H_MARGIN;
use crate::settings::widgets::NAV_ITEM_RADIUS;

pub fn nav_section_label(theme: &Theme, ui_font: FamilyId, label: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_uppercase(), ui_font, crate::theme::text_size(10.0))
            .with_color(theme.disabled_text())
            .finish(),
    )
    .with_padding_left(20.0)
    .with_padding_right(12.0)
    .with_padding_top(14.0)
    .with_padding_bottom(6.0)
    .finish()
}

pub fn nav_divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(warpui::elements::Empty::new().finish())
        .with_margin_left(NAV_ITEM_H_MARGIN + 6.0)
        .with_margin_right(NAV_ITEM_H_MARGIN + 6.0)
        .with_margin_top(6.0)
        .with_margin_bottom(6.0)
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
}

pub fn map_icon_str(icon: &str) -> AppIcon {
    match icon {
        "Account" => AppIcon::User,
        "Sliders" => AppIcon::Sliders,
        "Key" => AppIcon::Key,
        "Server" => AppIcon::Server,
        "Code" => AppIcon::Code,
        "Palette" => AppIcon::Palette,
        "Terminal" => AppIcon::Terminal,
        "Keyboard" => AppIcon::Keyboard,
        "Eye" => AppIcon::Eye,
        "Info" => AppIcon::Info,
        "Settings" => AppIcon::Settings,
        "General" => AppIcon::Sliders,
        "Editor" => AppIcon::Edit,
        "Search" => AppIcon::Search,
        "Git" => AppIcon::GitBranch,
        "Commands" => AppIcon::CornerDownLeft,
        "Teams" => AppIcon::Layers,
        "Projects" => AppIcon::Folder,
        "Members" => AppIcon::User,
        "Billing" => AppIcon::Clock,
        "Integrations" => AppIcon::Grid,
        "Notifications" => AppIcon::MessageSquare,
        "Mobile" => AppIcon::Smartphone,
        "Remote" => AppIcon::Lock,
        "Mic" => AppIcon::Microphone,
        "Usage" => AppIcon::Asset("icons/activity.svg"),
        _ => AppIcon::Sliders,
    }
}

pub fn nav_sub_item(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    icon: &str,
    section: super::action::SettingsSection,
    selected: bool,
) -> Box<dyn Element> {
    nav_item_inner(
        theme,
        ui_font,
        view_id,
        hover,
        label,
        icon,
        SettingsAction::SelectSection(section),
        selected,
        20.0,
        12.5,
    )
}

pub fn nav_umbrella_header(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    expanded: bool,
    child_active: bool,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let chevron = if expanded { "▾" } else { "▸" };
    Box::new(
        Hoverable::new(hover, move |state| {
            let hovered = state.is_hovered();
            let bg = if child_active {
                theme.accent_overlay()
            } else if hovered {
                theme.sidebar_row_hover()
            } else {
                ColorU::new(0, 0, 0, 0)
            };

            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                            .with_color(if child_active || hovered {
                                theme.main_text()
                            } else {
                                theme.sub_text()
                            })
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Text::new(chevron.to_string(), ui_font, crate::theme::text_size(10.0))
                        .with_color(theme.disabled_text())
                        .finish(),
                )
                .finish();

            Container::new(row)
                .with_margin_left(NAV_ITEM_H_MARGIN)
                .with_margin_right(NAV_ITEM_H_MARGIN)
                .with_margin_top(2.0)
                .with_margin_bottom(2.0)
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(NAV_ITEM_RADIUS)))
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
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

pub fn nav_item(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    icon: &str,
    section: super::action::SettingsSection,
    selected: bool,
) -> Box<dyn Element> {
    nav_item_inner(
        theme,
        ui_font,
        view_id,
        hover,
        label,
        icon,
        SettingsAction::SelectSection(section),
        selected,
        12.0,
        13.0,
    )
}
