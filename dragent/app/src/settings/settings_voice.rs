//! Voice settings actions: the supported recording preferences.

use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::settings::SettingsEvent;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_voice(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            // ── Voice ────────────────────────────────────────────────────
            SettingsAction::ToggleVoice => {
                let v = &mut self.app_settings.voice;
                v.enabled = !v.enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectVoiceBackend(backend) => {
                self.app_settings.voice.stt_model = match backend {
                    super::voice_settings::VoiceBackend::Automatic => String::new(),
                    super::voice_settings::VoiceBackend::System => "os-default".into(),
                };
                self.open_picker = None;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectLanguage(lang) => {
                self.app_settings.voice.language = lang.clone();
                self.open_picker = None;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleConfirmBeforeInsert => {
                let v = &mut self.app_settings.voice;
                v.confirm_before_insert = !v.confirm_before_insert;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            _ => {}
        }
    }
}
