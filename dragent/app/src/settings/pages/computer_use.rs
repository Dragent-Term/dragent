//! Settings → Computer use: master switch, confirmation policy, and the
//! per-app allowlist gating every computer-use/automation execution path.
//! When disabled, the gated executor refuses all computer
//! actions with an explanatory error.

use warpui::elements::{
    Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisAlignment, MainAxisSize,
    ParentElement, Shrinkable, Text,
};

use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, group_label, link_button, page_header, secondary_button, setting_row, status_badge,
    toggle_row,
};

pub fn render_computer_use(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::theme::text_size;
    let ui_font = view.ui_font;
    let id = view.view_id;
    let settings = &view.app_settings.computer_use;

    let master_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("computer_use_enabled"),
                "Enable computer use",
                Some("Allow agents to read and control desktop apps."),
                settings.enabled,
                SettingsAction::ToggleComputerUse,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("computer_use_confirm"),
                "Require confirmation",
                Some("Approve actions before an agent changes anything in another app."),
                settings.require_confirmation,
                SettingsAction::ToggleComputerUseConfirm,
            ),
        ],
    );

    // Allowlist rows.
    let mut app_rows: Vec<Box<dyn Element>> = Vec::new();
    if settings.allowed_apps.is_empty() {
        app_rows.push(
            Container::new(
                Text::new(
                    "All apps are allowed, except protected apps. Add app names to limit where agents can work."
                        .to_string(),
                    ui_font,
                    text_size(12.5),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish(),
        );
    }
    for app in &settings.allowed_apps {
        let app_name = app.clone();
        let remove_name = app.clone();
        app_rows.push(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Shrinkable::new(
                            1.0,
                            Text::new(app_name, ui_font, text_size(13.0))
                                .with_color(theme.main_text())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(link_button(
                        theme,
                        ui_font,
                        id,
                        view.hover(&format!("computer_app_remove_{app}")),
                        "Remove",
                        SettingsAction::RemoveComputerApp(remove_name),
                    ))
                    .finish(),
            )
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .finish(),
        );
    }
    // Adder row: editor + Add button.
    let adder = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Expanded::new(
                    1.0,
                    crate::shared_input::shared_input(
                        theme,
                        &view.computer_app_editor,
                        crate::shared_input::SharedInputOpts {
                            width: crate::shared_input::SharedInputWidth::Fill,
                            radius: 8.0,
                            background: Some(theme.surface_1()),
                            border: Some(theme.outline()),
                            pad_left: 10.0,
                            pad_right: 10.0,
                            pad_top: 6.0,
                            pad_bottom: 6.0,
                            ..Default::default()
                        },
                    ),
                )
                .finish(),
            )
            .with_child(secondary_button(
                theme,
                ui_font,
                id,
                view.hover("computer_app_add"),
                "Add",
                SettingsAction::AllowComputerApp(String::new()),
            ))
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(10.0)
    .with_padding_bottom(12.0)
    .finish();
    app_rows.push(adder);
    let allowlist_card = card_rows(theme, app_rows);

    let safety_note = Container::new(
        Text::new(
            "Computer use follows these controls for the Dragent agent, CLI and MCP tools. Password managers remain blocked."
                .to_string(),
            ui_font,
            text_size(12.0),
        )
        .with_color(theme.sub_text())
        .finish(),
    )
    .with_padding_left(4.0)
    .finish();

    #[cfg(not(target_family = "wasm"))]
    let permissions_card = permission_controls(view, theme);
    #[cfg(target_family = "wasm")]
    let permissions_card = warpui::elements::Empty::new().finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(16.0)
        .with_child(page_header(
            theme,
            ui_font,
            "Computer use",
            "Choose when agents can use your desktop and which apps they may control.",
        ))
        .with_child(master_card)
        .with_child(permissions_card)
        .with_child(group_label(theme, ui_font, "Allowed apps"))
        .with_child(allowlist_card)
        .with_child(Container::new(safety_note).with_padding_top(16.0).finish())
        .finish()
}

#[cfg(not(target_family = "wasm"))]
fn permission_controls(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::theme::text_size;
    use dragent_computer::{Access, PermissionKind};
    let ui = view.ui_font;
    let id = view.view_id;
    let report = view.computer_permissions.as_ref();
    let label = |value| match value {
        Some(Access::Granted) => "Granted",
        Some(Access::NotNeeded) => "Not required",
        Some(Access::Missing) => "Not enabled",
        Some(Access::Unavailable) => "Unavailable",
        None => "Checking…",
    };
    let mut rows = Vec::new();
    for (name, description, access, kind, key) in [
        (
            "Accessibility",
            "Read app controls and interact with them.",
            report.map(|p| p.accessibility),
            PermissionKind::Accessibility,
            "computer-accessibility",
        ),
        (
            "Screen recording",
            "Include a screenshot with the app’s state.",
            report.map(|p| p.screen_recording),
            PermissionKind::ScreenRecording,
            "computer-screenshots",
        ),
    ] {
        let ready = matches!(access, Some(Access::Granted | Access::NotNeeded));
        let controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(12.0)
            .with_child(status_badge(theme, ui, label(access), ready));
        #[cfg(target_os = "macos")]
        let controls = controls.with_child(secondary_button(
            theme,
            ui,
            id,
            view.hover(key),
            "Open Settings",
            SettingsAction::OpenComputerPermission(kind),
        ));
        #[cfg(not(target_os = "macos"))]
        let _ = (kind, key);
        rows.push(setting_row(
            theme,
            ui,
            name,
            Some(description),
            controls.finish(),
        ));
    }
    rows.push(setting_row(
        theme,
        ui,
        "Mouse & keyboard",
        Some("Send clicks, keys, scrolling and drag actions."),
        status_badge(
            theme,
            ui,
            label(report.map(|p| p.input)),
            matches!(
                report.map(|p| p.input),
                Some(Access::Granted | Access::NotNeeded)
            ),
        ),
    ));
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(group_label(theme, ui, "System permissions"))
        .with_child(card_rows(theme, rows));
    let mut footer = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(link_button(
            theme,
            ui,
            id,
            view.hover("computer-permissions-refresh"),
            if view.computer_permissions_loading {
                "Checking…"
            } else {
                "Refresh status"
            },
            SettingsAction::RefreshComputerPermissions,
        ));
    #[cfg(target_os = "macos")]
    {
        footer = footer.with_child(link_button(
            theme,
            ui,
            id,
            view.hover("computer-reset-request"),
            "Reset access…",
            SettingsAction::RequestComputerPermissionReset,
        ));
    }
    body = body.with_child(footer.finish());
    #[cfg(target_os = "macos")]
    if view.computer_reset_confirm {
        body = body.with_child(card_rows(
            theme,
            vec![setting_row(
                theme,
                ui,
                "Reset OS permissions?",
                Some("You will need to grant Accessibility and Screen Recording again."),
                Flex::row()
                    .with_spacing(8.0)
                    .with_child(secondary_button(
                        theme,
                        ui,
                        id,
                        view.hover("computer-reset-cancel"),
                        "Cancel",
                        SettingsAction::CancelComputerPermissionReset,
                    ))
                    .with_child(secondary_button(
                        theme,
                        ui,
                        id,
                        view.hover("computer-reset-confirm"),
                        "Reset access",
                        SettingsAction::ConfirmComputerPermissionReset,
                    ))
                    .finish(),
            )],
        ));
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(report) = report {
        if !report.notes.is_empty() {
            body = body.with_child(
                Text::new(report.notes.join(" "), ui, text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        }
    }
    if let Some(note) = &view.computer_permission_note {
        body = body.with_child(
            Text::new(note.clone(), ui, text_size(12.0))
                .with_color(theme.sub_text())
                .finish(),
        );
    }
    let skill_enabled = crate::cli::agent_skill::computer_enabled();
    body.with_child(group_label(theme, ui, "Agent access"))
        .with_child(card_rows(
            theme,
            vec![
                setting_row(
                    theme,
                    ui,
                    "Dragent agent",
                    Some("Desktop guidance is included. No installation needed."),
                    status_badge(theme, ui, "Built-in", true),
                ),
                setting_row(
                    theme,
                    ui,
                    "Other local agents",
                    Some("Add desktop-control instructions to their shared skills folder."),
                    secondary_button(
                        theme,
                        ui,
                        id,
                        view.hover("computer-skill-install"),
                        if skill_enabled {
                            "Remove skill"
                        } else {
                            "Install skill"
                        },
                        SettingsAction::ToggleComputerSkill,
                    ),
                ),
            ],
        ))
        .finish()
}
