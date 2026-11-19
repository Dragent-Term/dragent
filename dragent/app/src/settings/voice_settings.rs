//! Voice settings.
//!
//! Pure serde models shared by persistence (`AppSettings`), the settings UI
//! and the voice pipeline's gate. No UI or OS dependencies — unit-testable.

use serde::{Deserialize, Serialize};

/// How dictation is triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DictationMode {
    /// Tap to start, tap to stop.
    #[default]
    Toggle,
    /// Dictate while held.
    Hold,
    /// Push-to-talk key.
    #[serde(rename = "push_to_talk")]
    PushToTalk,
}

impl DictationMode {
    pub const ALL: [Self; 3] = [Self::Toggle, Self::Hold, Self::PushToTalk];

    pub fn label(self) -> &'static str {
        match self {
            Self::Toggle => "Toggle",
            Self::Hold => "Hold",
            Self::PushToTalk => "Push to talk",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "toggle" => Some(Self::Toggle),
            "hold" => Some(Self::Hold),
            "push_to_talk" | "push-to-talk" | "push to talk" => Some(Self::PushToTalk),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Hold => "hold",
            Self::PushToTalk => "push_to_talk",
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_language() -> String {
    "en".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceSettings {
    /// Master switch for voice input. On by default: dictation was always
    /// available before this switch existed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Engine preference: empty = automatic, "os-default" = system speech.
    /// Older model ids remain readable, but resolve to automatic: the app
    /// has no selectable cloud or local-model implementation.
    #[serde(default)]
    pub stt_model: String,
    /// Legacy local-model directory, preserved for compatibility only.
    #[serde(default)]
    pub models_dir: String,
    /// BCP-47 language passed to the STT backend.
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub dictation_mode: DictationMode,
    /// When true, a finished dictation waits for explicit user confirmation
    /// instead of being inserted into the composer automatically.
    #[serde(default)]
    pub confirm_before_insert: bool,
    /// Legacy custom model ids, preserved for compatibility only.
    #[serde(default)]
    pub user_models: Vec<String>,
    /// Legacy microphone preference. Capture uses the system default input.
    #[serde(default)]
    pub microphone_device_id: Option<String>,
    #[serde(default)]
    pub microphone_label: Option<String>,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            stt_model: String::new(),
            models_dir: String::new(),
            language: default_language(),
            dictation_mode: DictationMode::Toggle,
            confirm_before_insert: false,
            user_models: Vec::new(),
            microphone_device_id: None,
            microphone_label: None,
        }
    }
}

/// The engines actually implemented by the recording pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceBackend {
    Automatic,
    System,
}

impl VoiceSettings {
    pub fn preferred_backend(&self) -> VoiceBackend {
        if self.stt_model.trim() == "os-default" {
            VoiceBackend::System
        } else {
            VoiceBackend::Automatic
        }
    }

    /// Gate a dictation start. Returns the backend label to use, or a
    /// human-readable error — never a fake transcript.
    pub fn dictation_backend(&self, platform_supported: bool) -> Result<VoiceBackend, String> {
        if !self.enabled {
            return Err("Voice input is disabled. Enable it in Settings → Voice.".to_string());
        }
        if !platform_supported {
            return Err("Voice input is not yet supported on this platform.".to_string());
        }
        Ok(self.preferred_backend())
    }

    /// Effective language tag sent to the backend (falls back to "en").
    pub fn effective_language(&self) -> &str {
        if self.language.trim().is_empty() {
            "en"
        } else {
            self.language.trim()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_defaults_keep_dictation_on() {
        let v = VoiceSettings::default();
        assert!(v.enabled);
        let legacy: VoiceSettings = serde_json::from_str("{}").unwrap();
        assert!(legacy.enabled);
        assert!(v.stt_model.is_empty());
        assert!(v.models_dir.is_empty());
        assert_eq!(v.language, "en");
        assert_eq!(v.dictation_mode, DictationMode::Toggle);
        assert!(!v.confirm_before_insert);
        assert!(v.user_models.is_empty());
        assert_eq!(v.microphone_device_id, None);
        assert_eq!(v.microphone_label, None);
    }

    #[test]
    fn round_trip_json() {
        let v = VoiceSettings {
            enabled: true,
            stt_model: "nova-2".into(),
            models_dir: "/models".into(),
            language: "de".into(),
            dictation_mode: DictationMode::PushToTalk,
            confirm_before_insert: true,
            user_models: vec!["custom-1".into()],
            microphone_device_id: Some("mic-1".into()),
            microphone_label: Some("Built-in".into()),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: VoiceSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn legacy_missing_sections_use_defaults() {
        let v: VoiceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(v, VoiceSettings::default());
    }

    #[test]
    fn legacy_dictation_mode_camel_or_missing() {
        // Unknown values fall back via explicit parse helper, not serde;
        // serde default covers the missing case.
        let v: VoiceSettings = serde_json::from_str(r#"{"dictation_mode":"hold"}"#).unwrap();
        assert_eq!(v.dictation_mode, DictationMode::Hold);
        assert_eq!(
            DictationMode::parse("push_to_talk"),
            Some(DictationMode::PushToTalk)
        );
        assert_eq!(DictationMode::parse("bogus"), None);
    }

    #[test]
    fn dictation_backend_gate_is_honest() {
        let mut v = VoiceSettings::default();
        // Disabled → disabled error, never a transcript.
        v.enabled = false;
        assert!(v.dictation_backend(true).is_err());
        v.enabled = true;
        // A model name cannot make an unsupported platform record audio.
        assert!(v.dictation_backend(false).is_err());
        assert_eq!(v.dictation_backend(true).unwrap(), VoiceBackend::Automatic);
        v.stt_model = "os-default".into();
        assert_eq!(v.dictation_backend(true).unwrap(), VoiceBackend::System);
        v.stt_model = "nova-2".into();
        assert_eq!(v.dictation_backend(true).unwrap(), VoiceBackend::Automatic);
        assert!(v.dictation_backend(false).is_err());
        // Language falls back to "en" when blank.
        v.language = "  ".into();
        assert_eq!(v.effective_language(), "en");
        v.language = "fr".into();
        assert_eq!(v.effective_language(), "fr");
    }
}
