//! Settings → Developer permissions: macOS permission state, keep-awake
//! behaviour, and computer-use safety.

use warpui::elements::{
    Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize, ParentElement, Text,
};

use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::shortcut_settings::{
    self, AwakeMode, PermissionState,
};
use super::super::widgets::{
    card_rows, group_label, page_header, secondary_button, segmented_control, setting_row,
    status_badge, toggle_row, SegmentOption,
};

fn permission_row(
    view: &SettingsView,
    theme: &Theme,
    label: &str,
    description: &str,
    state: PermissionState,
) -> Box<dyn Element> {
    use crate::theme::text_size;
    let ui_font = view.ui_font;
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(16.0)
            .with_child(
                Expanded::new(
                    1.0,
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_main_axis_size(MainAxisSize::Min)
                        .with_child(
                            Text::new(label.to_string(), ui_font, text_size(13.5))
                                .with_color(theme.main_text())
                                .finish(),
                        )
                        .with_child(
                            Container::new(
                                Text::new(description.to_string(), ui_font, text_size(12.0))
                                    .with_color(theme.sub_text())
                                    .finish(),
                            )
                            .with_padding_top(3.0)
                            .finish(),
                        )
                        .finish(),
                )
                .finish(),
            )
            .with_child(status_badge(theme, ui_font, state.label(), state.granted()))
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(13.0)
    .with_padding_bottom(13.0)
    .finish()
}

pub fn render_developer_permissions(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::theme::text_size;
    let ui_font = view.ui_font;
    let id = view.view_id;
    let dev = &view.app_settings.dev_permissions;
    let report = shortcut_settings::query_permission_report();

    let permissions_card = card_rows(
        theme,
        vec![
            permission_row(
                view,
                theme,
                "Accessibility",
                "Required for computer use and global shortcuts. Grant in System Settings → Privacy & Security → Accessibility.",
                report.accessibility,
            ),
            permission_row(
                view,
                theme,
                "Full Disk Access",
                "Required to read files anywhere on disk. Grant in System Settings → Privacy & Security → Full Disk Access.",
                report.full_disk_access,
            ),
            setting_row(
                theme,
                ui_font,
                "Permission status",
                Some("Re-query the OS state (no prompts — pure queries)."),
                secondary_button(
                    theme,
                    ui_font,
                    id,
                    view.hover("dev_perm_recheck"),
                    "Recheck",
                    SettingsAction::RecheckDeveloperPermissions,
                ),
            ),
        ],
    );

    let awake_control = segmented_control(
        theme,
        ui_font,
        id,
        [AwakeMode::On, AwakeMode::Off, AwakeMode::Auto]
            .into_iter()
            .map(|mode| SegmentOption {
                label: mode.label().to_string(),
                selected: dev.awake_mode == mode,
                hover: view.hover(&format!("awake_mode_{:?}", mode)),
                action: SettingsAction::SelectAwakeMode(mode),
            })
            .collect(),
    );
    let held = shortcut_settings::wake_lock_held();
    let awake_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("keep_awake"),
                "Keep computer awake while agents run",
                Some("Holds an OS wake lock during automation runs (Auto mode)."),
                dev.keep_awake_while_agents_run,
                SettingsAction::ToggleKeepAwake,
            ),
            setting_row(
                theme,
                ui_font,
                "Computer awake mode",
                Some(if held {
                    "Wake lock is currently held."
                } else {
                    "No wake lock held right now."
                }),
                awake_control,
            ),
        ],
    );

    let mut page = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui_font,
            "Developer permissions",
            "OS permissions and keep-awake behaviour for long agent runs.",
        ))
        .with_child(group_label(theme, ui_font, "macOS permissions"))
        .with_child(permissions_card)
        .with_child(group_label(theme, ui_font, "Keep awake"))
        .with_child(awake_card);

    if let Some(note) = &view.dev_perm_note {
        page = page.with_child(
            Container::new(
                Text::new(note.clone(), ui_font, text_size(12.5))
                    .with_color(theme.accent)
                    .finish(),
            )
            .with_padding_left(4.0)
            .finish(),
        );
    }

    page.finish()
}
