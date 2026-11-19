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
use crate::settings::widgets_controls::primary_button;
use crate::settings::widgets_controls::secondary_button;
use crate::settings::widgets_panels::circular_avatar;

pub fn mcp_status_dot(theme: &Theme, enabled: bool, has_token: bool) -> Box<dyn Element> {
    let color = if !enabled {
        theme.disabled_text()
    } else if has_token {
        theme.ui_green() // connected via OAuth
    } else {
        theme.sub_text() // no OAuth token (may not need one)
    };
    ConstrainedBox::new(
        Container::new(warpui::elements::Empty::new().finish())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish(),
    )
    .with_width(10.0)
    .with_height(10.0)
    .finish()
}

/// Small transport-type badge ("stdio" or "SSE").
fn mcp_transport_badge(theme: &Theme, ui_font: FamilyId, label: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_string(), ui_font, crate::theme::text_size(10.0))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .with_padding_left(5.0)
    .with_padding_right(5.0)
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .finish()
}

pub fn mcp_server_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    remove_hover: MouseStateHandle,
    edit_hover: MouseStateHandle,
    toggle_hover: MouseStateHandle,
    disconnect_hover: MouseStateHandle,
    name: &str,
    display_cmd: &str,
    transport_label: &str,
    index: usize,
    enabled: bool,
    has_oauth_token: bool,
) -> Box<dyn Element> {
    let name_owned = name.to_string();

    let title_row = Flex::row()
        .with_spacing(6.0)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(mcp_status_dot(theme, enabled, has_oauth_token))
        .with_child(
            Text::new(name_owned.clone(), ui_font, crate::theme::text_size(14.0))
                .with_color(if enabled {
                    theme.main_text()
                } else {
                    theme.sub_text()
                })
                .finish(),
        )
        .with_child(mcp_transport_badge(theme, ui_font, transport_label))
        .finish();

    let cmd_text = Container::new(
        Text::new(display_cmd.to_string(), ui_font, crate::theme::text_size(12.0))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_padding_top(3.0)
    .finish();

    let left = Flex::row()
        .with_spacing(12.0)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(circular_avatar(theme, ui_font, &name_owned))
        .with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(title_row)
                .with_child(cmd_text)
                .finish(),
        )
        .finish();

    let toggle_label = if enabled { "Disable" } else { "Enable" };
    let mut actions = Flex::row()
        .with_spacing(6.0)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(secondary_button(
            theme,
            ui_font,
            view_id,
            toggle_hover,
            toggle_label,
            SettingsAction::ToggleMcpEnabled(index),
        ))
        .with_child(secondary_button(
            theme,
            ui_font,
            view_id,
            edit_hover,
            "Edit",
            SettingsAction::ShowMcpForm(Some(index)),
        ));

    if has_oauth_token {
        actions = actions.with_child(secondary_button(
            theme,
            ui_font,
            view_id,
            disconnect_hover,
            "Disconnect",
            SettingsAction::DisconnectMcpOAuth(index),
        ));
    }

    actions = actions.with_child(secondary_button(
        theme,
        ui_font,
        view_id,
        remove_hover,
        "Remove",
        SettingsAction::RemoveMcpServer(index),
    ));

    let card_content = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(left)
        .with_child(actions.finish())
        .finish();

    let border_color = if enabled {
        theme.outline()
    } else {
        theme.disabled_text()
    };
    Container::new(card_content)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(border_color))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

pub fn mcp_gallery_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    name: &str,
    description: &str,
    url_hint: Option<&str>,
    button_label: &str,
    action: SettingsAction,
) -> Box<dyn Element> {
    let name_owned = name.to_string();
    let desc_owned = description.to_string();
    let url_hint_owned = url_hint.map(str::to_string);
    let button_owned = button_label.to_string();

    let mut details = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Text::new(name_owned.clone(), ui_font, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Container::new(
                Text::new(desc_owned, ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_top(3.0)
            .finish(),
        );

    if let Some(url) = url_hint_owned {
        details = details.with_child(
            Container::new(
                Text::new(url, ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            )
            .with_padding_top(2.0)
            .finish(),
        );
    }

    let card_content = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Flex::row()
                .with_spacing(12.0)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(circular_avatar(theme, ui_font, &name_owned))
                .with_child(details.finish())
                .finish(),
        )
        .with_child(primary_button(
            theme,
            ui_font,
            view_id,
            hover,
            &button_owned,
            action,
        ))
        .finish();

    Container::new(card_content)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

pub fn mcp_preset_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    name: &str,
    description: &str,
    action: SettingsAction,
) -> Box<dyn Element> {
    let name_owned = name.to_string();
    let desc_owned = description.to_string();

    let details = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Text::new(name_owned.clone(), ui_font, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Container::new(
                Text::new(desc_owned, ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
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
                .with_child(circular_avatar(theme, ui_font, &name_owned))
                .with_child(details)
                .finish(),
        )
        .with_child(primary_button(
            theme, ui_font, view_id, hover, "+ Add", action,
        ))
        .finish();

    Container::new(card_content)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

/// One installed MCP server as a compact row: name and status, address, and
/// a "52 tools · 2 prompts" toggle. The tools, prompts and resources are only
/// listed while the server is expanded, so the list stays short however many
/// tools each server brings.
pub fn mcp_installed_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    edit_hover: MouseStateHandle,
    toggle_hover: MouseStateHandle,
    remove_hover: MouseStateHandle,
    connect_hover: Option<MouseStateHandle>,
    connect_action: Option<super::action::SettingsAction>,
    disconnect_hover: Option<MouseStateHandle>,
    capabilities_hover: MouseStateHandle,
    name: &str,
    description: &str,
    synced_to: Vec<&'static str>,
    index: usize,
    enabled: bool,
    has_oauth_token: bool,
    expanded: bool,
    tool_count: usize,
    tools: Vec<String>,
    prompt_count: usize,
    prompts: Vec<String>,
    resource_count: usize,
    resources: Vec<String>,
) -> Box<dyn Element> {
    let name_s = name.to_string();
    let capability_total = tool_count + prompt_count + resource_count;
    let is_working = capability_total > 0 || has_oauth_token;
    let status_text = if !enabled {
        "Disabled"
    } else if tool_count > 0 {
        "Connected"
    } else if has_oauth_token {
        "Authorized"
    } else if connect_hover.is_some() {
        "Needs setup"
    } else {
        "Connecting…"
    };
    let status_color = if enabled && is_working {
        theme.ui_green()
    } else if enabled && connect_hover.is_some() {
        theme.ui_yellow()
    } else {
        theme.disabled_text()
    };

    let status = Flex::row()
        .with_spacing(5.0)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(
                ConstrainedBox::new(warpui::elements::Empty::new().finish())
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
            )
            .with_background_color(status_color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish(),
        )
        .with_child(
            Text::new(status_text.to_string(), ui_font, crate::theme::text_size(11.0))
                .with_color(status_color)
                .finish(),
        )
        .finish();
    let title = Flex::row()
        .with_spacing(10.0)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Box::new(Shrinkable::new(
            1.0,
            Text::new(name_s.clone(), ui_font, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .soft_wrap(false)
                .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                .finish(),
        )))
        .with_child(status)
        .finish();

    let mut details = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(3.0)
        .with_child(title)
        .with_child(
            Text::new(description.to_string(), ui_font, crate::theme::text_size(12.0))
                .with_color(theme.sub_text())
                .soft_wrap(false)
                .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                .finish(),
        );
    if !synced_to.is_empty() {
        details = details.with_child(
            Text::new(
                format!("Also in {}", synced_to.join(", ")),
                ui_font,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.disabled_text())
            .finish(),
        );
    }
    if capability_total > 0 {
        let summary = capability_summary(tool_count, prompt_count, resource_count);
        let toggle_name = name_s.clone();
        details = details.with_child(Box::new(
            Hoverable::new(capabilities_hover, move |state| {
                let color = if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                };
                Container::new(
                    Flex::row()
                        .with_spacing(4.0)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(render_icon(
                            if expanded {
                                AppIcon::ChevronDown
                            } else {
                                AppIcon::ChevronRight
                            },
                            color,
                            10.0,
                        ))
                        .with_child(
                            Text::new(summary.clone(), ui_font, crate::theme::text_size(12.0))
                                .with_color(color)
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_top(2.0)
                .finish()
            })
            .with_cursor(warpui_core::platform::Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(
                    view_id,
                    SettingsAction::ToggleMcpCapabilities(toggle_name.clone()),
                );
            }),
        ));
    }
    let details = details.finish();

    let toggle_action = SettingsAction::ToggleMcpEnabled(index);
    let remove_action = SettingsAction::RemoveMcpServer(index);

    // Toggle: action goes on the Hoverable wrapper, not the Switch itself.
    // The Switch's internal click handler consumes the event; the Hoverable's
    // on_click is what reliably fires.
    let toggle_el: Box<dyn Element> = Box::new(
        Hoverable::new(toggle_hover, move |_state| {
            let switch_state = SwitchStateHandle::default();
            Switch::new(
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
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, toggle_action.clone());
        }),
    );

    let remove_x: Box<dyn Element> = Box::new(
        Hoverable::new(remove_hover, move |state| {
            let color = if state.is_hovered() {
                theme.ui_red()
            } else {
                theme.disabled_text()
            };
            Container::new(render_icon(AppIcon::XClose, color, 11.0))
                .with_padding_left(8.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, remove_action.clone());
        }),
    );

    let mut right = Flex::row()
        .with_spacing(8.0)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(secondary_button(
            theme,
            ui_font,
            view_id,
            edit_hover,
            "Edit",
            SettingsAction::ShowMcpForm(Some(index)),
        ));

    // Auth buttons. A server that already serves tools needs no Connect.
    if has_oauth_token {
        if let Some(dh) = disconnect_hover {
            right = right.with_child(secondary_button(
                theme,
                ui_font,
                view_id,
                dh,
                "Disconnect",
                SettingsAction::DisconnectMcpOAuth(index),
            ));
        }
    } else if let (Some(ch), Some(ca), 0) = (connect_hover, connect_action, capability_total) {
        right = right.with_child(secondary_button(theme, ui_font, view_id, ch, "Connect", ca));
    }

    right = right.with_child(remove_x).with_child(toggle_el);

    let header = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(16.0)
        .with_child(Box::new(Shrinkable::new(
            1.0,
            Flex::row()
                .with_spacing(12.0)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(circular_avatar(theme, ui_font, &name_s))
                .with_child(Box::new(Shrinkable::new(1.0, details)))
                .finish(),
        )))
        .with_child(right.finish())
        .finish();

    let mut row = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header);
    if expanded && capability_total > 0 {
        let mut groups = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(10.0);
        for (label, items) in [("Tools", &tools), ("Prompts", &prompts), ("Resources", &resources)] {
            if !items.is_empty() {
                groups = groups.with_child(capability_group(theme, ui_font, label, items));
            }
        }
        // Line the lists up with the server name: 32px avatar + 12px gap.
        row = row.with_child(
            Container::new(groups.finish())
                .with_padding_left(44.0)
                .with_padding_top(10.0)
                .finish(),
        );
    }

    Container::new(row.finish())
        .with_padding_left(4.0)
        .with_padding_right(4.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

/// "52 tools · 2 prompts · 4 resources", leaving out empty kinds.
fn capability_summary(tools: usize, prompts: usize, resources: usize) -> String {
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    [
        count(tools, "tool", "tools"),
        count(prompts, "prompt", "prompts"),
        count(resources, "resource", "resources"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// A small heading over the names of one kind of capability, as compact chips.
fn capability_group(
    theme: &Theme,
    ui_font: FamilyId,
    label: &str,
    items: &[String],
) -> Box<dyn Element> {
    let mut chips = Wrap::row().with_spacing(4.0).with_run_spacing(4.0);
    for item in items {
        chips = chips.with_child(
            Container::new(
                Text::new(item.clone(), ui_font, crate::theme::text_size(10.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish(),
        );
    }
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(5.0)
        .with_child(
            Text::new(format!("{label} ({})", items.len()), ui_font, crate::theme::text_size(11.0))
                .with_color(theme.disabled_text())
                .finish(),
        )
        .with_child(chips.finish())
        .finish()
}

#[cfg(test)]
mod tests {
    use super::capability_summary;

    #[test]
    fn summaries_skip_empty_kinds_and_count_in_words() {
        assert_eq!(capability_summary(52, 2, 0), "52 tools · 2 prompts");
        assert_eq!(capability_summary(1, 0, 4), "1 tool · 4 resources");
        assert_eq!(capability_summary(0, 0, 0), "");
    }
}
