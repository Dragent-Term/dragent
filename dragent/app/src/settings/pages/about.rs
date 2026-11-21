use warpui::elements::{Element, Flex, MainAxisSize, ParentElement};

use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    link_button, page_header, secondary_button, setting_group, settings_card,
};

pub fn render_about(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let view_id = view.view_id;
    let version = env!("CARGO_PKG_VERSION");

    Flex::column()
        .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(
            theme,
            ui_font,
            "About",
            "Dragent — terminal workspace with an integrated AI agent.",
        ))
        .with_child(setting_group(
            theme,
            ui_font,
            "Dragent",
            vec![settings_card(
                theme,
                Flex::column()
                    .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Start)
                    .with_child(
                        warpui::elements::Text::new(
                            "Dragent".to_string(),
                            ui_font,
                            crate::theme::text_size(22.0),
                        )
                        .with_color(theme.main_text())
                        .finish(),
                    )
                    .with_child(
                        warpui::elements::Container::new(Box::new(
                            warpui::elements::Text::new(
                                format!("Version {version}"),
                                ui_font,
                                crate::theme::text_size(13.0),
                            )
                            .with_color(theme.sub_text()),
                        ))
                        .with_padding_top(4.0)
                        .finish(),
                    )
                    .with_child(
                        warpui::elements::Container::new(Box::new(
                            warpui::elements::Text::new(
                                "Copyright 2026 Dragent".to_string(),
                                ui_font,
                                crate::theme::text_size(12.0),
                            )
                            .with_color(theme.disabled_text()),
                        ))
                        .with_padding_top(8.0)
                        .finish(),
                    )
                    .finish(),
            )],
        ))
        .with_child(setting_group(
            theme,
            ui_font,
            "Community",
            vec![settings_card(
                theme,
                Flex::column()
                    .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Start)
                    .with_spacing(10.0)
                    .with_child(
                        warpui::elements::Text::new(
                            "Dragent is open source (AGPL-3.0). A star helps other developers find it."
                                .to_string(),
                            ui_font,
                            crate::theme::text_size(13.0),
                        )
                        .with_color(theme.sub_text())
                        .finish(),
                    )
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(crate::community::render_star_button(
                                theme,
                                ui_font,
                                crate::community::state(),
                                false,
                                view.hover("about_star"),
                                move |ectx| ectx.dispatch_typed_action_for(view_id, SettingsAction::StarOnGitHub),
                            ))
                            .with_child(crate::community::render_x_button(
                                theme,
                                ui_font,
                                Some("Follow @dragentai on X"),
                                view.hover("about_x"),
                                move |ectx| {
                                    ectx.dispatch_typed_action_for(view_id, SettingsAction::FollowOnX)
                                },
                            ))
                            .finish(),
                    )
                    .finish(),
            )],
        ))
        .with_child(setting_group(
            theme,
            ui_font,
            "Links",
            vec![Flex::row()
                .with_spacing(16.0)
                .with_child(link_button(
                    theme,
                    ui_font,
                    view.view_id,
                    view.hover("about_docs"),
                    "Documentation",
                    SettingsAction::OpenDocumentation,
                ))
                .with_child(link_button(
                    theme,
                    ui_font,
                    view.view_id,
                    view.hover("about_privacy"),
                    "Privacy policy",
                    SettingsAction::OpenPrivacyPolicy,
                ))
                .finish()],
        ))
        .with_child(
            Flex::row()
                .with_spacing(8.0)
                .with_child(secondary_button(
                    theme,
                    ui_font,
                    view.view_id,
                    view.hover("about_copy_version"),
                    "Copy version",
                    SettingsAction::CopyVersion,
                ))
                .finish(),
        )
        .finish()
}
