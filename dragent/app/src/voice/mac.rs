//! macOS speech-to-text via Apple's Speech framework.
//!
//! Pipeline: AVAudioEngine input tap → SFSpeechAudioBufferRecognitionRequest →
//! SFSpeechRecognizer streaming partials. On-device recognition is requested
//! when the recognizer supports it (no audio leaves the machine).
//!
//! PERMISSIONS: voice input needs THREE things, and missing any one of them
//! fails quietly rather than loudly — NSMicrophoneUsageDescription +
//! NSSpeechRecognitionUsageDescription in the Info.plist, and
//! `com.apple.security.device.audio-input` in the signed bundle's entitlements
//! (the hardened runtime denies the mic outright without it — no prompt, no
//! Privacy pane entry, just silence). See `ensure_microphone_access`.
//!
//! SEGMENTATION: Apple's recognizer treats a pause as an utterance boundary —
//! partials reset to the current utterance, and after enough silence the task
//! finalizes and STOPS. Naively forwarding that made dictation "restart" on
//! every sentence. So this runs a segment loop: each finalized utterance is
//! appended to a shared `committed` transcript, a fresh recognition request is
//! spun up on the same running audio engine, and the UI always receives
//! `committed + current partial`. Nothing is lost until the user stops.
//!
//! Threading: everything Speech-related happens on a dedicated thread; the
//! recognizer callbacks arrive on arbitrary dispatch queues and share state
//! through Arc<Mutex<String>> + channels.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::AnyThread;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::{NSError, NSLocale, NSString};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognizer,
    SFSpeechRecognizerAuthorizationStatus,
};

use super::VoiceEvent;

const VOICE_PERMISSION_HELP: &str =
    "Open System Settings → Privacy & Security → Microphone and Speech Recognition, then enable Dragent for both.";

/// Microphone (TCC) gate, checked before any audio work.
///
/// The microphone and speech recognition are SEPARATE permissions, and only the
/// speech one announces itself: a denied microphone does not make
/// `AVAudioEngine::start` fail — the engine starts happily and taps deliver
/// silent buffers forever, so dictation just sits there transcribing nothing.
/// Asking AVCaptureDevice directly is the only way to tell "no audio yet" from
/// "no audio, ever". Requires `com.apple.security.device.audio-input` in the
/// bundle's entitlements, or the status here is permanently Denied.
pub(crate) fn ensure_microphone_access(tx: &async_channel::Sender<VoiceEvent>) -> bool {
    let denied = || {
        let _ = tx.send_blocking(VoiceEvent::Error(
            format!("Microphone permission denied. {VOICE_PERMISSION_HELP}").into(),
        ));
        false
    };

    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        let _ = tx.send_blocking(VoiceEvent::Error(
            "No microphone available on this system.".into(),
        ));
        return false;
    };

    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) };
    if status == AVAuthorizationStatus::Authorized {
        return true;
    }
    if status != AVAuthorizationStatus::NotDetermined {
        return denied();
    }

    // First use: prompt. The completion block lands on an arbitrary dispatch
    // queue, so relay through a channel and wait here.
    let (granted_tx, granted_rx) = std::sync::mpsc::channel::<bool>();
    unsafe {
        let cb = RcBlock::new(move |granted: Bool| {
            let _ = granted_tx.send(granted.as_bool());
        });
        AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &cb);
    }
    match granted_rx.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok(true) => true,
        Ok(false) => denied(),
        Err(_) => {
            let _ = tx.send_blocking(VoiceEvent::Error(
                "Timed out waiting for microphone permission.".into(),
            ));
            false
        }
    }
}

/// Speech-recognition (TCC) gate. Separate permission from the microphone —
/// requests authorization if undetermined, waiting for the user's answer.
///
/// Called UP-FRONT by the voice orchestrator regardless of which engine will
/// run: the cloud engine doesn't need it, but the on-device fallback does, and
/// asking now means an engine switch mid-dictation never pops a permission
/// dialog while the user is talking. Idempotent — once the user has answered,
/// the callback returns the stored status instantly with no prompt.
pub(crate) fn ensure_speech_access() -> bool {
    speech_access_status().is_ok()
}

/// Inner form with the denial reason, used by `run_session` for its error
/// message when the on-device engine is the one actually running.
fn speech_access_status() -> Result<(), String> {
    let denied = || {
        Err(format!(
            "Speech recognition permission denied. {VOICE_PERMISSION_HELP}"
        ))
    };

    // Cheap status query FIRST — mirrors the microphone gate above. Calling
    // requestAuthorization unconditionally was the bug behind "stuck on
    // connecting forever": it wants to present a TCC dialog, and we call it
    // from a bare worker thread with no run loop, so on an already-decided
    // permission it could return without ever invoking the completion block —
    // leaving the recv below to wait out its full timeout, or hang outright.
    // Once the answer is on record (the common case) we never call it at all.
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
        return Ok(());
    }
    if status != SFSpeechRecognizerAuthorizationStatus::NotDetermined {
        return denied();
    }

    // Genuinely first run: prompt, but never block the session for long. The
    // cloud engine doesn't need this permission, so a slow answer must not
    // hold dictation hostage — 15s, then carry on without the fallback.
    let (auth_tx, auth_rx) = std::sync::mpsc::channel::<SFSpeechRecognizerAuthorizationStatus>();
    unsafe {
        let cb = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
            let _ = auth_tx.send(status);
        });
        SFSpeechRecognizer::requestAuthorization(&cb);
    }
    match auth_rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(s) if s == SFSpeechRecognizerAuthorizationStatus::Authorized => Ok(()),
        Ok(_) => denied(),
        Err(_) => Err("Timed out waiting for speech permission.".into()),
    }
}

/// Append a finished utterance to the running transcript with clean spacing.
fn append_segment(transcript: &mut String, segment: &str) {
    let segment = segment.trim();
    if segment.is_empty() {
        return;
    }
    if !transcript.is_empty() && !transcript.ends_with(char::is_whitespace) {
        transcript.push(' ');
    }
    transcript.push_str(segment);
}

/// Compose transcript + in-flight partial for display.
fn compose(transcript: &str, partial: &str) -> String {
    let mut out = transcript.to_string();
    append_segment(&mut out, partial);
    out
}

/// Run the on-device dictation session. `seed` is the transcript committed by
/// a previous engine before a mid-session handoff (empty when starting fresh)
/// — snapshots stay cumulative across the switch, so the UI keeps every word.
pub(crate) fn run_session(
    tx: async_channel::Sender<VoiceEvent>,
    stop_flag: Arc<AtomicBool>,
    seed: String,
    language: &str,
) {
    // 1. Microphone, before speech recognition: without it the rest of this
    //    pipeline runs perfectly on an audio stream of pure silence.
    if !ensure_microphone_access(&tx) {
        return;
    }

    // 2. Speech-recognition authorization. Normally already resolved by the
    //    orchestrator's up-front request (instant, no prompt); this re-check
    //    is the belt-and-suspenders for direct callers.
    if let Err(msg) = speech_access_status() {
        let _ = tx.send_blocking(VoiceEvent::Error(msg));
        return;
    }

    unsafe {
        if stop_flag.load(Ordering::Relaxed) {
            let _ = tx.send_blocking(VoiceEvent::Final(seed));
            return;
        }
        let locale_tag = super::recognizer_locale(language);
        let locale =
            NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(locale_tag));
        let Some(recognizer) =
            SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale)
        else {
            let _ = tx.send_blocking(VoiceEvent::Error(format!(
                "System speech does not support '{language}'. Choose another language in Settings → Voice.",
            )));
            return;
        };
        // Apple may fall back to the keyboard language. Never transcribe a
        // different language silently when the selected locale is unsupported.
        if recognizer.locale().languageCode().to_string() != locale.languageCode().to_string() {
            let _ = tx.send_blocking(VoiceEvent::Error(format!(
                "System speech does not support '{language}'. Choose another language in Settings → Voice.",
            )));
            return;
        }
        if !recognizer.isAvailable() {
            let _ = tx.send_blocking(VoiceEvent::Error(
                "Speech recognition is temporarily unavailable.".into(),
            ));
            return;
        }

        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        let format = input.outputFormatForBus(0);
        let mut engine_started = false;

        // The full dictation so far (finalized utterance segments). Seeded
        // with whatever a previous engine committed before handing off.
        let committed: Arc<Mutex<String>> = Arc::new(Mutex::new(seed));
        // Consecutive segments that ended without producing any text — guards
        // against a hot restart loop if the recognizer keeps erroring.
        let mut empty_segments = 0u32;

        // ── Segment loop: one recognition request per utterance ────────────
        loop {
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setShouldReportPartialResults(true);
            if recognizer.supportsOnDeviceRecognition() {
                request.setRequiresOnDeviceRecognition(true);
            }

            let req_for_tap: Retained<SFSpeechAudioBufferRecognitionRequest> = request.clone();
            let tap = RcBlock::new(
                move |buf: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                    req_for_tap.appendAudioPCMBuffer(buf.as_ref());
                },
            );
            input.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                Some(&format),
                RcBlock::as_ptr(&tap),
            );

            if !engine_started {
                engine.prepare();
                if let Err(e) = engine.startAndReturnError() {
                    input.removeTapOnBus(0);
                    let _ = tx.send_blocking(VoiceEvent::Error(format!(
                        "Could not start microphone: {}. Check System Settings → Privacy & Security → Microphone.",
                        e.localizedDescription()
                    )));
                    return;
                }
                engine_started = true;
                // Mic is genuinely capturing — UI flips connecting → recording.
                // (After a mid-session handoff the UI is already recording;
                // a second Live is harmless.)
                let _ = tx.send_blocking(VoiceEvent::Live);
            }

            // Segment-end signal: true = this segment contributed text.
            let (seg_tx, seg_rx) = std::sync::mpsc::channel::<bool>();
            let seg_tx_err = seg_tx.clone();
            let tx_cb = tx.clone();
            let committed_cb = committed.clone();
            // The in-flight partial of THIS segment. The recognizer often ends
            // a segment with an ERROR instead of an isFinal result (e.g. after
            // a pause, on-device models report "no speech detected") — without
            // banking the last partial here, that whole sentence would vanish
            // and dictation would look like it "restarts".
            let last_partial: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
            let last_partial_cb = last_partial.clone();
            let handler = RcBlock::new(
                move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                    if !result.is_null() {
                        let result = &*result;
                        let text = result.bestTranscription().formattedString().to_string();
                        if result.isFinal() {
                            // Utterance finished (pause). Bank it and tell the
                            // segment loop to start the next request.
                            last_partial_cb.lock().unwrap().clear();
                            let mut c = committed_cb.lock().unwrap();
                            append_segment(&mut c, &text);
                            let _ = tx_cb.send_blocking(VoiceEvent::Partial(c.clone()));
                            let _ = seg_tx.send(!text.trim().is_empty());
                            return;
                        }
                        *last_partial_cb.lock().unwrap() = text.clone();
                        let c = committed_cb.lock().unwrap();
                        let _ = tx_cb.send_blocking(VoiceEvent::Partial(compose(&c, &text)));
                    }
                    if !error.is_null() {
                        // Segment ended WITHOUT a final result. Bank whatever
                        // partial we heard — it is real speech.
                        let salvaged = std::mem::take(&mut *last_partial_cb.lock().unwrap());
                        let had_text = !salvaged.trim().is_empty();
                        if had_text {
                            let mut c = committed_cb.lock().unwrap();
                            append_segment(&mut c, &salvaged);
                            let _ = tx_cb.send_blocking(VoiceEvent::Partial(c.clone()));
                        }
                        let _ = seg_tx_err.send(had_text);
                    }
                },
            );
            let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler);

            // Wait for the user's stop click or the segment finishing on its own.
            let mut user_stopped = false;
            let mut segment_had_text = false;
            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    user_stopped = true;
                    break;
                }
                match seg_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(had_text) => {
                        segment_had_text = had_text;
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }

            input.removeTapOnBus(0);

            if user_stopped {
                // Stop feeding audio; let the recognizer flush buffered input so
                // the last words land in `committed` via the isFinal callback
                // (or the error-salvage path).
                engine.stop();
                request.endAudio();
                match seg_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(_) => {}
                    Err(_) => {
                        task.cancel();
                        // No callback ever came — salvage the visible partial.
                        let salvaged = std::mem::take(&mut *last_partial.lock().unwrap());
                        if !salvaged.trim().is_empty() {
                            append_segment(&mut committed.lock().unwrap(), &salvaged);
                        }
                    }
                }
                let _ = tx.send_blocking(VoiceEvent::Final(committed.lock().unwrap().clone()));
                return;
            }

            // Segment ended on its own — close out this request and restart a
            // fresh one so dictation continues seamlessly.
            request.endAudio();
            drop(task);

            if segment_had_text {
                empty_segments = 0;
            } else {
                empty_segments += 1;
                // Generous: silence must NOT end the session (the user keeps
                // it open across pauses and even across sends). This only
                // guards against a permanently-erroring recognizer.
                if empty_segments >= 200 {
                    // ~long silence / repeated errors: end the session instead
                    // of looping forever with a hot mic.
                    engine.stop();
                    let _ = tx.send_blocking(VoiceEvent::Final(committed.lock().unwrap().clone()));
                    return;
                }
                // Brief pause so an erroring recognizer can't spin the loop.
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{append_segment, compose};

    #[test]
    fn segments_accumulate_instead_of_replacing() {
        let mut t = String::new();
        append_segment(&mut t, "fix the login bug");
        append_segment(&mut t, "then add a test");
        assert_eq!(t, "fix the login bug then add a test");
        // In-flight partial extends the banked transcript for display.
        assert_eq!(
            compose(&t, "and run the suite"),
            "fix the login bug then add a test and run the suite"
        );
    }

    #[test]
    fn empty_and_whitespace_segments_are_ignored() {
        let mut t = String::from("hello");
        append_segment(&mut t, "   ");
        append_segment(&mut t, "");
        assert_eq!(t, "hello");
        assert_eq!(compose(&t, ""), "hello");
    }
}
