//! Voice input — click-to-toggle dictation for the input bars.
//!
//! Engine selection (macOS): Deepgram streaming (paid tiers; backend mints a
//! short-lived key) → falls back to Apple Speech. Settings can select system
//! speech directly. Windows: WinRT dictation. Both OS engines cost nothing
//! and never touch LLM billing — the mic button is not a pro feature; paid
//! plans also have access to the cloud engine. Linux: hidden for now.
//!
//! `DRAGENT_VOICE_ENGINE=os` also forces system speech (debugging).

pub mod fold;

#[cfg(target_os = "macos")]
mod deepgram;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod windows;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Events streamed from an active voice session.
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// Capture is actually running: permissions granted, engine started and
    /// (for the cloud engine) the socket is open. Until this arrives the UI
    /// must show "connecting", NOT "recording" — permission prompts, the
    /// token fetch, and the WebSocket handshake all happen before it.
    Live,
    /// A (cumulative) partial transcription — replaces the previous partial.
    Partial(String),
    /// The final transcription; the session is over after this.
    Final(String),
    /// Recognition failed or was denied; the session is over.
    Error(String),
}

/// UI-facing lifecycle of the mic button. Derived from `VoiceEvent`s:
/// `Connecting` from the moment the user clicks until `Live` arrives, then
/// `Recording` until `Final`/`Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceUiState {
    #[default]
    Idle,
    Connecting,
    Recording,
}

/// Handle to a running voice capture; dropping does NOT stop it — call `stop`.
pub struct VoiceSession {
    stop_flag: Arc<AtomicBool>,
}

/// Options for one dictation, resolved from settings at session start.
#[derive(Debug, Clone)]
pub struct DictationOpts {
    /// BCP-47 tag passed to the STT backend (defaults to "en").
    pub language: String,
    /// An implemented engine preference, never an arbitrary model id.
    pub backend: crate::settings::voice_settings::VoiceBackend,
}

impl DictationOpts {
    /// Resolve supported options from a settings snapshot before capture.
    pub fn from_settings(
        settings: &crate::settings::VoiceSettings,
        platform_supported: bool,
    ) -> Result<Self, String> {
        let backend = settings.dictation_backend(platform_supported)?;
        Ok(Self {
            language: settings.effective_language().to_string(),
            backend,
        })
    }
}

pub(crate) fn review_notice(text: &str) -> String {
    format!("Dictation ready: “{text}” — click the microphone to insert it.")
}

/// What a finished dictation does with its transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalDisposition {
    /// Insert into the composer right away.
    Insert(String),
    /// Hold for explicit user confirmation first.
    AwaitingConfirm(String),
}

/// Apply the `confirm_before_insert` gate: when confirmation is on, a finished
/// dictation waits for the user instead of being inserted automatically.
pub fn final_disposition(confirm_before_insert: bool, text: String) -> FinalDisposition {
    if confirm_before_insert && !text.trim().is_empty() {
        FinalDisposition::AwaitingConfirm(text)
    } else {
        FinalDisposition::Insert(text)
    }
}

/// Expand the short language tags offered by Settings into recognizer locales.
/// Explicit regional tags in existing settings are preserved.
pub(crate) fn recognizer_locale(language: &str) -> &str {
    match language {
        "en" => "en-US",
        "de" => "de-DE",
        "fr" => "fr-FR",
        "es" => "es-ES",
        "it" => "it-IT",
        "pt" => "pt-BR",
        "nl" => "nl-NL",
        "ja" => "ja-JP",
        "zh" => "zh-CN",
        "ko" => "ko-KR",
        tag => tag,
    }
}

/// Resolve supported options and apply the voice master switch before capture.
pub fn start_gated(tx: async_channel::Sender<VoiceEvent>) -> Result<VoiceSession, String> {
    let settings = crate::settings::AppSettings::global();
    let opts = DictationOpts::from_settings(&settings.voice, supported())?;
    log::info!(
        "[voice] dictation start: backend={:?} lang={} mic=system default",
        opts.backend,
        opts.language,
    );
    start_with_opts(opts, tx)
}

/// Start a session with explicit options (used by `start_gated`; `start`
/// resolves the same options from global settings).
pub fn start_with_opts(
    opts: DictationOpts,
    tx: async_channel::Sender<VoiceEvent>,
) -> Result<VoiceSession, String> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_t = stop_flag.clone();

    #[cfg(target_os = "macos")]
    {
        std::thread::Builder::new()
            .name("voice-stt".into())
            .spawn(move || {
                let panic_tx = tx.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    run_macos_session_with_opts(tx, stop_t, &opts)
                }));
                if let Err(p) = result {
                    let detail = p
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| p.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    log::error!("[voice] session thread panicked: {detail}");
                    let _ = panic_tx
                        .send_blocking(VoiceEvent::Error(format!("Dictation crashed: {detail}")));
                }
            })
            .map_err(|e| format!("could not spawn voice thread: {e}"))?;
        Ok(VoiceSession { stop_flag })
    }
    #[cfg(windows)]
    {
        std::thread::Builder::new()
            .name("voice-stt".into())
            .spawn(move || {
                if let Err(msg) = windows::run_session(&tx, stop_t, &opts.language) {
                    let _ = tx.send_blocking(VoiceEvent::Error(msg));
                }
            })
            .map_err(|e| format!("could not spawn voice thread: {e}"))?;
        Ok(VoiceSession { stop_flag })
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (opts, tx, stop_t);
        Err("Voice input is not yet supported on this platform.".to_string())
    }
}

/// Start capturing microphone audio, streaming transcription events into `tx`.
/// The channel receives `Partial` updates continuously and ends with `Final`
/// or `Error`.
///
/// Disabled voice and unsupported platforms fail before starting capture.
pub fn start(tx: async_channel::Sender<VoiceEvent>) -> Result<VoiceSession, String> {
    start_gated(tx)
}

/// Run the selected macOS engine. Automatic uses the server-selected cloud
/// model, with system speech as fallback. System bypasses cloud completely.
#[cfg(target_os = "macos")]
fn run_macos_session_with_opts(
    tx: async_channel::Sender<VoiceEvent>,
    stop_t: Arc<AtomicBool>,
    opts: &DictationOpts,
) {
    if stop_t.load(Ordering::Relaxed) {
        let _ = tx.send_blocking(VoiceEvent::Final(String::new()));
        return;
    }
    if opts.backend == crate::settings::voice_settings::VoiceBackend::System {
        mac::run_session(tx, stop_t, String::new(), &opts.language);
        return;
    }
    // Both permissions up-front, REGARDLESS of which engine ends up
    // running. The cloud engine doesn't technically need Speech
    // authorization, but the on-device fallback does — asking now
    // means a mid-dictation engine switch never pops a permission
    // dialog in the middle of the user talking (or silently dies).
    if !mac::ensure_microphone_access(&tx) {
        return;
    }
    let speech_ok = mac::ensure_speech_access();
    if !speech_ok {
        // Non-fatal: the cloud engine can still run. Only the
        // fallback is lost — surfaced clearly if it's ever needed.
        log::warn!("[voice] speech recognition not authorized — on-device fallback unavailable");
    }
    // Paid-tier cloud engine first. Falls back to the on-device
    // engine both for pre-capture failures (free tier, offline,
    // unconfigured) AND mid-session drops — carrying over the
    // transcript committed so far, so no words are lost.
    match deepgram::run_session(&tx, &stop_t, &opts.language) {
        Ok(()) => {}
        Err(fb) => {
            // User already hit stop while the cloud engine was
            // failing: end the session with what we have — don't
            // spin up ANOTHER engine nobody asked for.
            if stop_t.load(Ordering::Relaxed) {
                let _ = tx.send_blocking(VoiceEvent::Final(fb.committed));
                return;
            }
            log::info!(
                            "[voice] deepgram unavailable ({}) — switching to on-device engine (seed: {} chars)",
                            fb.reason,
                            fb.committed.len()
                        );
            // Without Speech authorization the on-device engine
            // produces nothing at all. Falling through to it would
            // leave the mic looking live while silently recording
            // into the void — end the session with a real reason.
            if !speech_ok {
                let _ = tx.send_blocking(VoiceEvent::Error(format!(
                    "Dictation unavailable: cloud engine failed ({}) and on-device \
                                 dictation is not authorized. Open System Settings → Privacy & \
                                 Security → Microphone and Speech Recognition, then enable Dragent \
                                 for both.",
                    fb.reason,
                )));
                return;
            }
            // Keep provider/engine details internal. The user only needs a
            // working microphone; fallback transitions are recorded in logs.
            mac::run_session(tx, stop_t, fb.committed, &opts.language);
        }
    }
}

/// True when this build has a voice engine (controls mic button visibility).
pub fn supported() -> bool {
    cfg!(any(target_os = "macos", windows))
}

impl VoiceSession {
    /// Stop capturing; the recognizer finishes processing buffered audio and
    /// then emits `Final`.
    pub fn stop(self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_gate_holds_non_empty_finals() {
        assert_eq!(
            final_disposition(true, "hello world".into()),
            FinalDisposition::AwaitingConfirm("hello world".into())
        );
        // Empty finals insert (nothing to confirm).
        assert_eq!(
            final_disposition(true, "   ".into()),
            FinalDisposition::Insert("   ".into())
        );
        assert_eq!(
            final_disposition(false, "hello".into()),
            FinalDisposition::Insert("hello".into())
        );
    }

    #[test]
    fn settings_resolve_only_implemented_backends() {
        use crate::settings::voice_settings::VoiceBackend;
        let mut settings = crate::settings::VoiceSettings::default();
        assert_eq!(
            DictationOpts::from_settings(&settings, true)
                .unwrap()
                .backend,
            VoiceBackend::Automatic
        );
        settings.stt_model = "os-default".into();
        settings.language = "fr".into();
        let opts = DictationOpts::from_settings(&settings, true).unwrap();
        assert_eq!(opts.backend, VoiceBackend::System);
        assert_eq!(recognizer_locale(&opts.language), "fr-FR");
        for old_model in ["nova-2", "nova-3", "whisper-1", "my-local-model"] {
            settings.stt_model = old_model.into();
            settings.user_models = vec![old_model.into()];
            assert_eq!(
                DictationOpts::from_settings(&settings, true)
                    .unwrap()
                    .backend,
                VoiceBackend::Automatic
            );
            assert!(DictationOpts::from_settings(&settings, false).is_err());
        }
        settings.enabled = false;
        assert!(DictationOpts::from_settings(&settings, true).is_err());
    }

    #[test]
    fn recognizer_locales_expand_short_tags_and_preserve_regions() {
        assert_eq!(recognizer_locale("en"), "en-US");
        assert_eq!(recognizer_locale("zh"), "zh-CN");
        assert_eq!(recognizer_locale("pt"), "pt-BR");
        assert_eq!(recognizer_locale("en-GB"), "en-GB");
    }
}
