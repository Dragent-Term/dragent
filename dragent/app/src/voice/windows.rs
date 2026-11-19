//! Windows speech-to-text via WinRT `Windows.Media.SpeechRecognition`.
//!
//! Unlike the macOS path, the WinRT recognizer owns the microphone capture —
//! no manual audio engine. A continuous dictation session streams
//! hypotheses (partials within the current utterance) and results (finalized
//! utterance segments); we accumulate finalized segments and present
//! `committed + current hypothesis` as the cumulative partial, matching the
//! macOS event contract exactly.
//!
//! Requirements on the user's machine: a microphone, and the
//! "Online speech recognition" toggle (Settings → Privacy → Speech) for
//! languages without on-device dictation support. Errors from either surface
//! as `VoiceEvent::Error` with a pointer to the setting.
//!
//! Consent, and why we cannot prompt: unlike macOS — where
//! `SFSpeechRecognizer::requestAuthorization` raises a real system dialog —
//! Windows has no way for an *unpackaged* Win32 app (which is what our .exe/.msi
//! ships as) to request microphone consent. Desktop apps are governed by a
//! single all-or-nothing "Let desktop apps access your microphone" toggle, and
//! the speech privacy policy is a separate system consent with no request API at
//! all. The closest we can get to "asking" is to detect the missing consent the
//! moment the user clicks the mic and open the exact Settings page for it.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::Foundation::TypedEventHandler;
use windows::Globalization::Language;
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs,
    SpeechContinuousRecognitionResultGeneratedEventArgs,
    SpeechRecognitionHypothesisGeneratedEventArgs, SpeechRecognizer,
};

use super::VoiceEvent;
use crate::process_ext::NoWindow;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// `SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED` — thrown by every `SpeechRecognizer`
/// call when the user has never accepted the system speech privacy policy.
const SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED: i32 = 0x8004_5509_u32 as i32;

const MIC_CONSENT_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// Take the user to a Settings page. This is our stand-in for a consent prompt.
fn open_settings(page: &str) {
    if let Err(err) = Command::new("cmd")
        .no_window()
        .args(["/c", "start", page])
        .spawn()
    {
        log::warn!("[voice] could not open {page}: {err}");
    }
}

/// Whether microphone capture is explicitly blocked for desktop apps.
///
/// An absent key means "never configured", which Windows treats as allowed — so
/// only an explicit `Deny` counts. Inferring denial from a missing key would
/// wrongly block users whose mic works fine.
fn microphone_denied() -> bool {
    let denied = |path: &str| {
        windows_registry::CURRENT_USER
            .open(path)
            .ok()
            .and_then(|key| key.get_string("Value").ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("Deny"))
    };
    denied(MIC_CONSENT_KEY) || denied(&format!(r"{MIC_CONSENT_KEY}\NonPackaged"))
}

pub(crate) fn run_session(
    tx: &async_channel::Sender<VoiceEvent>,
    stop_flag: Arc<AtomicBool>,
    language: &str,
) -> Result<(), String> {
    // Initialize COM/WinRT for this thread (MTA).
    let _guard = unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() {
            struct ComGuard;
            impl Drop for ComGuard {
                fn drop(&mut self) {
                    unsafe { CoUninitialize() };
                }
            }
            Some(ComGuard)
        } else {
            None
        }
    };

    let friendly = |e: &windows::core::Error| -> String {
        if e.code().0 == SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED {
            open_settings("ms-settings:privacy-speech");
            return "Windows needs you to accept the speech privacy policy before dictation can run. \
                    We opened Settings → Privacy & security → Speech — turn on \"Online speech recognition\", \
                    then click the mic again."
                .to_string();
        }
        format!(
            "Speech recognition failed: {}. Check Settings → Privacy & security → Speech (online speech recognition) and Microphone access (under 'Let desktop apps access your microphone').",
            e.message()
        )
    };

    if microphone_denied() {
        open_settings("ms-settings:privacy-microphone");
        return Err("Microphone access is turned off for desktop apps. \
                    We opened Settings → Privacy & security → Microphone — scroll down and turn on \
                    \"Let desktop apps access your microphone\", then click the mic again."
            .to_string());
    }

    if stop_flag.load(Ordering::Relaxed) {
        let _ = tx.send_blocking(VoiceEvent::Final(String::new()));
        return Ok(());
    }
    let locale = super::recognizer_locale(language);
    let speech_language = Language::CreateLanguage(&windows::core::HSTRING::from(locale))
        .map_err(|e| friendly(&e))?;
    let recognizer = SpeechRecognizer::Create(&speech_language).map_err(|e| friendly(&e))?;
    let compilation = recognizer
        .CompileConstraintsAsync()
        .and_then(|op| op.join())
        .map_err(|e| friendly(&e))?;
    if compilation.Status().map_err(|e| friendly(&e))?
        != windows::Media::SpeechRecognition::SpeechRecognitionResultStatus::Success
    {
        return Err(format!(
            "Windows speech is unavailable for '{language}'. Install speech support in Settings → Time & language → Speech, or choose another language in Dragent Settings → Voice.",
        ));
    }

    let session = recognizer
        .ContinuousRecognitionSession()
        .map_err(|e| friendly(&e))?;

    // committed = finalized utterance segments so far; the hypothesis extends it.
    let committed: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let ended = Arc::new(AtomicBool::new(false));

    // Partials: within-utterance hypotheses.
    {
        let tx = tx.clone();
        let committed = committed.clone();
        recognizer
            .HypothesisGenerated(&TypedEventHandler::<
                SpeechRecognizer,
                SpeechRecognitionHypothesisGeneratedEventArgs,
            >::new(move |_, args| {
                if let Some(args) = args.as_ref() {
                    if let Ok(hyp) = args.Hypothesis().and_then(|h| h.Text()) {
                        let base = committed.lock().unwrap().clone();
                        let full = if base.is_empty() {
                            hyp.to_string()
                        } else {
                            format!("{base} {hyp}")
                        };
                        let _ = tx.send_blocking(VoiceEvent::Partial(full));
                    }
                }
                Ok(())
            }))
            .map_err(|e| friendly(&e))?;
    }

    // Finalized segments: append to committed, emit as partial (the session
    // continues — Final only fires when the user stops).
    {
        let tx = tx.clone();
        let committed = committed.clone();
        session
            .ResultGenerated(&TypedEventHandler::<
                windows::Media::SpeechRecognition::SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionResultGeneratedEventArgs,
            >::new(move |_, args| {
                if let Some(args) = args.as_ref() {
                    if let Ok(text) = args.Result().and_then(|r| r.Text()) {
                        let text = text.to_string();
                        if !text.is_empty() {
                            let mut c = committed.lock().unwrap();
                            if c.is_empty() {
                                *c = text;
                            } else {
                                c.push(' ');
                                c.push_str(&text);
                            }
                            let _ = tx.send_blocking(VoiceEvent::Partial(c.clone()));
                        }
                    }
                }
                Ok(())
            }))
            .map_err(|e| friendly(&e))?;
    }

    // Session ended on its own (timeout, error, audio problem).
    {
        let ended = ended.clone();
        session
            .Completed(&TypedEventHandler::<
                windows::Media::SpeechRecognition::SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionCompletedEventArgs,
            >::new(move |_, _| {
                ended.store(true, Ordering::Relaxed);
                Ok(())
            }))
            .map_err(|e| friendly(&e))?;
    }

    session
        .StartAsync()
        .and_then(|op| op.join())
        .map_err(|e| friendly(&e))?;

    // Capture is genuinely running — UI flips connecting → recording.
    let _ = tx.send_blocking(VoiceEvent::Live);

    // Wait for the user's stop click (or self-termination).
    while !stop_flag.load(Ordering::Relaxed) && !ended.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if !ended.load(Ordering::Relaxed) {
        // StopAsync flushes: pending ResultGenerated callbacks fire before it
        // completes, so `committed` is complete afterwards.
        let _ = session.StopAsync().and_then(|op| op.join());
    }

    let final_text = committed.lock().unwrap().clone();
    let _ = tx.send_blocking(VoiceEvent::Final(final_text));
    Ok(())
}
