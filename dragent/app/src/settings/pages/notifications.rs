//! Settings → Notifications: which moments notify (for every agent and for
//! automation runs), and Dragent's sound for them (see `crate::notifications`).

use warpui::elements::{Container, CrossAxisAlignment, Element, Flex, MainAxisSize, ParentElement};

use crate::notifications::Chime;
use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, group_label, page_header, secondary_button, segmented_control, setting_row,
    toggle_row, SegmentOption,
};

/// A notification switch on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationSwitch {
    AgentFinished,
    AgentNeedsYou,
    AgentErrors,
    Automations,
    Sound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NotificationAction {
    Toggle(NotificationSwitch),
    /// Choose the sound, and hear it.
    SetChime(Chime),
    /// Set the volume (0–100), and hear it.
    SetVolume(u8),
    Preview,
}

/// The volumes the page offers.
pub(crate) const VOLUMES: [(&str, u8); 3] = [("Quiet", 35), ("Medium", 70), ("Loud", 100)];

fn act(action: NotificationAction) -> SettingsAction {
    SettingsAction::Notification(action)
}

pub fn render_notifications(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let settings = &view.app_settings.notifications;
    let switch =
        |key: &str, label: &str, description: &str, on: bool, which: NotificationSwitch| {
            toggle_row(
                theme,
                ui,
                id,
                view.hover(key),
                label,
                Some(description),
                on,
                act(NotificationAction::Toggle(which)),
            )
        };

    let chimes = segmented_control(
        theme,
        ui,
        id,
        Chime::ALL
            .into_iter()
            .map(|chime| SegmentOption {
                label: chime.label().to_string(),
                selected: settings.chime == chime,
                hover: view.hover(&format!("notification_chime_{}", chime.label())),
                action: act(NotificationAction::SetChime(chime)),
            })
            .collect(),
    );
    let volumes = segmented_control(
        theme,
        ui,
        id,
        VOLUMES
            .into_iter()
            .map(|(label, volume)| SegmentOption {
                label: label.to_string(),
                selected: settings.volume == volume,
                hover: view.hover(&format!("notification_volume_{label}")),
                action: act(NotificationAction::SetVolume(volume)),
            })
            .collect(),
    );
    let mut sound_rows = vec![switch(
        "notification_sound",
        "Play a sound",
        "Dragent's own chime, so you hear it even when the system mutes notification sounds.",
        settings.sound,
        NotificationSwitch::Sound,
    )];
    if settings.sound {
        sound_rows.push(setting_row(
            theme,
            ui,
            "Sound",
            Some("Click one to hear it."),
            chimes,
        ));
        sound_rows.push(setting_row(theme, ui, "Volume", None, volumes));
        sound_rows.push(setting_row(
            theme,
            ui,
            "Try it",
            None,
            secondary_button(
                theme,
                ui,
                id,
                view.hover("notification_preview"),
                "Play",
                act(NotificationAction::Preview),
            ),
        ));
    }

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui,
            "Notifications",
            "A desktop notification and a sound when agents are done or need you: Claude Code, Codex, \
             Dragent's own agent and the others, and automation runs.",
        ))
        .with_child(group_label(theme, ui, "Agents"))
        .with_child(card_rows(
            theme,
            vec![
                switch(
                    "notification_finished",
                    "When an agent finishes",
                    "It's done responding and waiting for what's next.",
                    settings.agent_finished,
                    NotificationSwitch::AgentFinished,
                ),
                switch(
                    "notification_needs_you",
                    "When an agent needs you",
                    "It's waiting for your approval, a permission or an answer.",
                    settings.agent_needs_you,
                    NotificationSwitch::AgentNeedsYou,
                ),
                switch(
                    "notification_errors",
                    "When an agent hits an error",
                    "Its turn stopped because something went wrong.",
                    settings.agent_errors,
                    NotificationSwitch::AgentErrors,
                ),
            ],
        ))
        .with_child(group_label(theme, ui, "Automations"))
        .with_child(card_rows(
            theme,
            vec![switch(
                "notification_automations",
                "Automation runs",
                "When a run finishes, fails, waits for someone, or reaches its daily limit.",
                settings.automations,
                NotificationSwitch::Automations,
            )],
        ))
        .with_child(group_label(theme, ui, "Sound"))
        .with_child(card_rows(theme, sound_rows))
        .with_child(
            Container::new(crate::machine_ui::text(
                ui,
                "Notifications show for the tab you're looking at too, in case you're busy elsewhere. \
                 Your system's notification settings can still hide them.",
                11.5,
                theme.disabled_text(),
            ))
            .with_padding_top(12.0)
            .finish(),
        )
        .finish()
}

impl SettingsView {
    pub(crate) fn handle_notification_action(
        &mut self,
        action: &NotificationAction,
        ctx: &mut warpui::ViewContext<Self>,
    ) {
        let settings = &mut self.app_settings.notifications;
        let mut preview = false;
        match action {
            NotificationAction::Toggle(which) => {
                let value = match which {
                    NotificationSwitch::AgentFinished => &mut settings.agent_finished,
                    NotificationSwitch::AgentNeedsYou => &mut settings.agent_needs_you,
                    NotificationSwitch::AgentErrors => &mut settings.agent_errors,
                    NotificationSwitch::Automations => &mut settings.automations,
                    NotificationSwitch::Sound => &mut settings.sound,
                };
                *value = !*value;
            }
            NotificationAction::SetChime(chime) => {
                settings.chime = *chime;
                preview = true;
            }
            NotificationAction::SetVolume(volume) => {
                settings.volume = *volume;
                preview = true;
            }
            NotificationAction::Preview => preview = true,
        }
        if preview {
            crate::notifications::play(settings.chime, settings.volume);
        }
        self.app_settings.save();
        ctx.emit(super::super::SettingsEvent::AppSettingsChanged);
        ctx.notify();
    }
}
