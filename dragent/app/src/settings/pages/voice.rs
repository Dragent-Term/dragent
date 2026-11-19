//! Voice settings backed by the current recording engines.

use warpui::elements::{CrossAxisAlignment, Element, Flex, MainAxisSize, ParentElement};

use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::voice_settings::VoiceBackend;
use super::super::widgets::{
    card_rows, group_label, page_header, select_control, setting_row, toggle_row, SelectItem,
};

const LANGUAGES: &[(&str, &str)] = &[
    ("English", "en"),
    ("German", "de"),
    ("French", "fr"),
    ("Spanish", "es"),
    ("Italian", "it"),
    ("Portuguese", "pt"),
    ("Dutch", "nl"),
    ("Japanese", "ja"),
    ("Chinese", "zh"),
    ("Korean", "ko"),
];

pub fn render_voice(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let voice = &view.app_settings.voice;

    if !crate::voice::supported() {
        return page_header(
            theme,
            ui,
            "Voice",
            "Voice input is available on macOS and Windows.",
        );
    }

    let dictation_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui,
                id,
                view.hover("voice_enabled"),
                "Voice input",
                Some("Click the microphone to start dictating. Click again to stop."),
                voice.enabled,
                SettingsAction::ToggleVoice,
            ),
            toggle_row(
                theme,
                ui,
                id,
                view.hover("voice_confirm"),
                "Confirm before insert",
                Some("Review your transcript, then click the microphone again to insert it."),
                voice.confirm_before_insert,
                SettingsAction::ToggleConfirmBeforeInsert,
            ),
        ],
    );

    let mut speech_rows = Vec::new();
    if cfg!(target_os = "macos") {
        let system = voice.preferred_backend() == VoiceBackend::System;
        let engine_select = select_control(
            theme,
            ui,
            id,
            view.hover("voice_engine_btn"),
            if system { "System speech" } else { "Automatic" },
            200.0,
            view.picker_open("voice_engine"),
            SettingsAction::TogglePicker("voice_engine".into()),
            vec![
                SelectItem {
                    label: "Automatic".into(),
                    selected: !system,
                    enabled: true,
                    hover: view.hover("voice_engine_auto"),
                    action: SettingsAction::SelectVoiceBackend(VoiceBackend::Automatic),
                },
                SelectItem {
                    label: "System speech".into(),
                    selected: system,
                    enabled: true,
                    hover: view.hover("voice_engine_system"),
                    action: SettingsAction::SelectVoiceBackend(VoiceBackend::System),
                },
            ],
        );
        speech_rows.push(setting_row(
            theme,
            ui,
            "Transcription",
            Some(if system {
                "Use Apple Speech directly."
            } else {
                "Cloud when available, with Apple Speech as fallback."
            }),
            engine_select,
        ));
    } else {
        speech_rows.push(setting_row(
            theme,
            ui,
            "Transcription",
            Some("Uses Windows speech recognition."),
            crate::machine_ui::text(ui, "System speech", 12.0, theme.sub_text()),
        ));
    }

    let current_lang = LANGUAGES
        .iter()
        .find(|(_, tag)| *tag == voice.effective_language())
        .map(|(label, _)| *label)
        .unwrap_or_else(|| voice.effective_language());
    let lang_select = select_control(
        theme,
        ui,
        id,
        view.hover("voice_lang_btn"),
        current_lang,
        180.0,
        view.picker_open("voice_lang"),
        SettingsAction::TogglePicker("voice_lang".into()),
        LANGUAGES
            .iter()
            .map(|(label, tag)| SelectItem {
                label: label.to_string(),
                selected: voice.effective_language() == *tag,
                enabled: true,
                hover: view.hover(&format!("voice_lang_{tag}")),
                action: SettingsAction::SelectLanguage(tag.to_string()),
            })
            .collect(),
    );
    speech_rows.push(setting_row(
        theme,
        ui,
        "Language",
        Some("Used by cloud and system speech; language support varies by device."),
        lang_select,
    ));
    speech_rows.push(setting_row(
        theme,
        ui,
        "Microphone",
        Some("Change your default input in your computer’s sound settings."),
        crate::machine_ui::text(ui, "System default", 12.0, theme.sub_text()),
    ));

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme, ui, "Voice",
            "Dictate into the terminal and composer. Recording starts only when you click the microphone.",
        ))
        .with_child(group_label(theme, ui, "Dictation"))
        .with_child(dictation_card)
        .with_child(group_label(theme, ui, "Speech recognition"))
        .with_child(card_rows(theme, speech_rows))
        .finish()
}
