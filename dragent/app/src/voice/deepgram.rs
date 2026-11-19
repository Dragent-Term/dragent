//! Deepgram streaming speech-to-text — the paid-tier dictation engine.
//!
//! Flow: POST /api/v1/voice/token on our backend mints a short-lived,
//! usage-scoped Deepgram key (the master key never leaves the server) →
//! WebSocket to Deepgram's live API → AVAudioEngine microphone tap streams
//! 16-bit PCM frames up → interim/final transcript JSON streams back.
//!
//! Event contract matches the OS engines exactly: cumulative
//! `VoiceEvent::Partial` snapshots (committed + in-flight interim), one
//! `Final` when the user stops. VoiceFold on the UI side is unchanged.
//!
//! `run_session` returns `Err(Fallback)` whenever the on-device engine should
//! take over: pre-capture failures (no entitlement, offline, not configured)
//! AND mid-session drops (socket death, unexpected close). `Fallback.committed`
//! carries the transcript captured so far so the OS engine can continue the
//! same dictation seamlessly — no words lost, no restart visible to the user.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

use block2::RcBlock;
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};

use super::VoiceEvent;

/// Why (and with what state) the caller should switch to the on-device engine.
pub(crate) struct Fallback {
    /// Human-readable cause, for logs only.
    pub reason: String,
    /// Transcript committed before the drop. Empty for pre-capture failures;
    /// seeds the OS engine on a mid-session handoff so nothing is lost.
    pub committed: String,
}

impl Fallback {
    /// Pre-capture failure — nothing recorded yet.
    fn before_capture(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            committed: String::new(),
        }
    }
}

/// Rate we declare to Deepgram and resample every device to. Fixing it is what
/// lets the socket open before the audio device is touched at all.
const TARGET_RATE: u32 = 16_000;

/// Device-rate f32 mono → 16 kHz signed 16-bit little-endian PCM.
///
/// Linear interpolation, resampling each tap buffer independently. Buffer-local
/// resampling drops a sub-sample of phase continuity at each boundary, which is
/// inaudible to a speech recogniser and is what the JS implementation this was
/// modelled on does too. Raw PCM (no Opus/WebM container) is deliberate: a
/// codec would add its own buffering delay ahead of every transcript.
fn resample_to_pcm16(src: &[f32], src_rate: u32) -> Vec<u8> {
    if src.is_empty() || src_rate == 0 {
        return Vec::new();
    }
    let to_i16 = |s: f32| ((s.clamp(-1.0, 1.0)) * i16::MAX as f32) as i16;

    if src_rate == TARGET_RATE {
        let mut out = Vec::with_capacity(src.len() * 2);
        for &s in src {
            out.extend_from_slice(&to_i16(s).to_le_bytes());
        }
        return out;
    }

    let ratio = src_rate as f32 / TARGET_RATE as f32;
    let out_len = (src.len() as f32 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len * 2);
    for i in 0..out_len {
        let pos = i as f32 * ratio;
        let idx = pos as usize;
        let frac = pos - idx as f32;
        let a = src[idx];
        let b = if idx + 1 < src.len() { src[idx + 1] } else { a };
        out.extend_from_slice(&to_i16(a + (b - a) * frac).to_le_bytes());
    }
    out
}

/// Parse one Deepgram live message; returns (transcript, is_final) for
/// non-empty Results messages.
fn parse_transcript(msg: &str) -> Option<(String, bool)> {
    let v: serde_json::Value = serde_json::from_str(msg).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("Results") {
        return None;
    }
    let text = v
        .get("channel")?
        .get("alternatives")?
        .get(0)?
        .get("transcript")?
        .as_str()?
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    let is_final = v.get("is_final").and_then(|f| f.as_bool()).unwrap_or(false);
    Some((text, is_final))
}

/// Case/punctuation-free view, for comparing two transcript fragments for
/// sameness when only their wording matters.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when `committed` already ends with `text`.
///
/// Deepgram can deliver the same segment twice: once when the interim window
/// closes (`is_final`) and again when endpointing ends the utterance — that
/// second message carries `is_final: true` with the SAME transcript. Appending
/// both writes every such segment into the transcript twice. Compared on the
/// normalized form because the repeat is often re-punctuated or re-capitalized
/// rather than byte-identical.
fn already_committed(committed: &str, text: &str) -> bool {
    let (c, t) = (norm(committed), norm(text));
    !t.is_empty() && c.ends_with(&t)
}

/// Join two transcript fragments with a single space.
fn join(a: &str, b: &str) -> String {
    if b.is_empty() {
        a.to_string()
    } else if a.is_empty() {
        b.to_string()
    } else {
        format!("{a} {b}")
    }
}

/// Fetch a short-lived Deepgram key from our backend. Errors are fallback
/// reasons, not user-facing failures.
fn fetch_token() -> Result<(String, String), String> {
    let body = serde_json::json!({});
    match crate::api::API_CLIENT.post("/api/v1/voice/token", &body, 10) {
        Ok(v) => {
            let key = v
                .get("key")
                .and_then(|k| k.as_str())
                .unwrap_or_default()
                .to_string();
            let model = v
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("nova-3")
                .to_string();
            if key.is_empty() {
                return Err("backend returned no key".into());
            }
            Ok((key, model))
        }
        Err(e) => Err(format!("token fetch failed: {e}")),
    }
}

pub fn run_session(
    tx: &async_channel::Sender<VoiceEvent>,
    stop_flag: &Arc<AtomicBool>,
    language: &str,
) -> Result<(), Fallback> {
    match std::env::var("DRAGENT_VOICE_ENGINE").as_deref() {
        Ok("os") => {
            return Err(Fallback::before_capture(
                "forced to OS engine via DRAGENT_VOICE_ENGINE",
            ))
        }
        _ => {}
    }
    let (key, model) = fetch_token().map_err(Fallback::before_capture)?;
    // The token round-trip can take seconds (retries on flaky networks) — the
    // user may have already hit stop. Bail cleanly instead of opening a socket
    // for a session nobody wants.
    if stop_flag.load(Ordering::Relaxed) {
        let _ = tx.send_blocking(VoiceEvent::Final(String::new()));
        return Ok(());
    }

    log::debug!("[voice] stage: token ok, opening websocket");

    unsafe {
        // ── WebSocket to Deepgram, BEFORE any audio work ────────────────────
        // Order matters. We used to read the device's native sample rate off
        // `AVAudioEngine.inputNode` to build this URL, which put all of
        // CoreAudio's initialisation ahead of the socket — and a stall in
        // there (a device held by a previous wedged session, a slow HAL
        // start) blocked the connect phase where none of our timeouts could
        // bound it. Declaring a fixed rate and resampling to it decouples the
        // two, so the network path is established and diagnosable on its own.
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model={model}&language={language}&encoding=linear16&sample_rate={TARGET_RATE}&channels=1&interim_results=true&smart_format=true&punctuate=true"
        );
        let request = tungstenite::client::IntoClientRequest::into_client_request(url.as_str())
            .and_then(|mut r| {
                r.headers_mut().insert(
                    "Authorization",
                    format!("Token {key}")
                        .parse()
                        .map_err(|_| tungstenite::Error::ConnectionClosed)?,
                );
                Ok(r)
            })
            .map_err(|e| Fallback::before_capture(format!("bad request: {e}")))?;
        // Bounded connect. `tungstenite::connect` dials with NO timeout — on a
        // slow or blocked network it hung indefinitely, and since the stop
        // flag is only polled in the stream loop below, the whole session
        // became unkillable ("mic active, no words, stop does nothing").
        // Dial explicitly with a connect timeout and handshake read/write
        // timeouts; ANY failure here is a pre-capture Fallback, so the
        // on-device engine takes over within seconds instead of never.
        log::debug!("[voice] stage: resolving api.deepgram.com");
        let stream = {
            use std::net::ToSocketAddrs;
            let addr = ("api.deepgram.com", 443u16)
                .to_socket_addrs()
                .map_err(|e| Fallback::before_capture(format!("dns failed: {e}")))?
                .next()
                .ok_or_else(|| Fallback::before_capture("dns returned no address"))?;
            let s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(6))
                .map_err(|e| Fallback::before_capture(format!("tcp connect failed: {e}")))?;
            let _ = s.set_read_timeout(Some(Duration::from_secs(6)));
            let _ = s.set_write_timeout(Some(Duration::from_secs(6)));
            s
        };
        log::debug!("[voice] stage: tcp up, starting TLS + websocket handshake");
        let (mut socket, _resp) = tungstenite::client_tls(request, stream)
            .map_err(|e| Fallback::before_capture(format!("connect failed: {e}")))?;
        log::debug!("[voice] stage: websocket open, starting microphone");
        // Stop pressed while we were dialing: shut down before capture starts.
        if stop_flag.load(Ordering::Relaxed) {
            let _ = tx.send_blocking(VoiceEvent::Final(String::new()));
            return Ok(());
        }
        // Short read timeout so one loop services audio, transcripts and stop.
        match socket.get_mut() {
            tungstenite::stream::MaybeTlsStream::Rustls(s) => {
                let _ = s
                    .get_mut()
                    .set_read_timeout(Some(Duration::from_millis(40)));
            }
            tungstenite::stream::MaybeTlsStream::Plain(s) => {
                let _ = s.set_read_timeout(Some(Duration::from_millis(40)));
            }
            _ => {}
        }

        // ── Microphone, now that the socket is up ───────────────────────────
        log::debug!("[voice] stage: initialising audio engine");
        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        let format = input.outputFormatForBus(0);
        let src_rate = format.sampleRate() as u32;
        log::debug!("[voice] stage: audio engine ready ({src_rate} Hz → {TARGET_RATE} Hz)");
        if src_rate == 0 {
            return Err(Fallback::before_capture("no input device"));
        }

        let (pcm_tx, pcm_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let tap = RcBlock::new(
            move |buf: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                let buf = buf.as_ref();
                let frames = buf.frameLength() as usize;
                let channels = buf.floatChannelData();
                if frames == 0 || channels.is_null() {
                    return;
                }
                // Mono: channel 0 only (we declare channels=1 to Deepgram).
                let ch0 = (*channels).as_ptr();
                let src = std::slice::from_raw_parts(ch0, frames);
                let _ = pcm_tx.send(resample_to_pcm16(src, src_rate));
            },
        );

        log::debug!("[voice] stage: starting microphone");
        input.installTapOnBus_bufferSize_format_block(
            0,
            1024,
            Some(&format),
            RcBlock::as_ptr(&tap),
        );
        engine.prepare();
        if let Err(e) = engine.startAndReturnError() {
            input.removeTapOnBus(0);
            return Err(Fallback::before_capture(format!(
                "microphone start failed: {}",
                e.localizedDescription()
            )));
        }

        log::info!(
            "[voice] deepgram session live (model={model}, {src_rate} Hz → {TARGET_RATE} Hz)"
        );
        // Capture is genuinely running now — socket open, mic started. The UI
        // flips from "connecting" to "recording" on this.
        let _ = tx.send_blocking(VoiceEvent::Live);

        // ── Stream loop ─────────────────────────────────────────────────────
        let mut committed = String::new();
        let mut last_keepalive = Instant::now();
        let mut closing_deadline: Option<Instant> = None;

        loop {
            if stop_flag.load(Ordering::Relaxed) && closing_deadline.is_none() {
                // User stopped: no more audio; ask Deepgram to flush finals.
                engine.stop();
                input.removeTapOnBus(0);
                let _ = socket.send(tungstenite::Message::Text(
                    "{\"type\":\"CloseStream\"}".into(),
                ));
                closing_deadline = Some(Instant::now() + Duration::from_secs(3));
            }

            if closing_deadline.is_none() {
                // Forward buffered microphone audio.
                while let Ok(chunk) = pcm_rx.try_recv() {
                    if socket.send(tungstenite::Message::Binary(chunk)).is_err() {
                        // Connection died mid-session: hand off to the
                        // on-device engine with everything committed so far —
                        // the user keeps dictating without interruption.
                        engine.stop();
                        input.removeTapOnBus(0);
                        return Err(Fallback {
                            reason: "socket send failed mid-session".into(),
                            committed,
                        });
                    }
                }
                if last_keepalive.elapsed() > Duration::from_secs(8) {
                    let _ = socket.send(tungstenite::Message::Text(
                        "{\"type\":\"KeepAlive\"}".into(),
                    ));
                    last_keepalive = Instant::now();
                }
            } else if Instant::now() > closing_deadline.unwrap() {
                let _ = tx.send_blocking(VoiceEvent::Final(committed));
                return Ok(());
            }

            match socket.read() {
                Ok(tungstenite::Message::Text(msg)) => {
                    if let Some((text, is_final)) = parse_transcript(&msg) {
                        log::debug!(
                            "[voice] dg {} {:?}",
                            if is_final { "FINAL " } else { "interim" },
                            text
                        );
                        if is_final {
                            // Skip a segment we already hold — see
                            // `already_committed`. Without this, the
                            // end-of-utterance repeat writes every segment
                            // into the transcript a second time.
                            if already_committed(&committed, &text) {
                                log::debug!("[voice] dg duplicate final ignored");
                            } else {
                                committed = join(&committed, &text);
                            }
                            let _ = tx.send_blocking(VoiceEvent::Partial(committed.clone()));
                        } else {
                            let _ = tx.send_blocking(VoiceEvent::Partial(join(&committed, &text)));
                        }
                    }
                }
                Ok(tungstenite::Message::Close(_)) => {
                    if closing_deadline.is_none() {
                        // Deepgram closed on us mid-session (key expired,
                        // server-side hiccup) — switch engines, keep the text.
                        engine.stop();
                        input.removeTapOnBus(0);
                        return Err(Fallback {
                            reason: "server closed stream mid-session".into(),
                            committed,
                        });
                    }
                    // Close during our own graceful shutdown = normal end.
                    let _ = tx.send_blocking(VoiceEvent::Final(committed));
                    return Ok(());
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    log::warn!("[voice] deepgram stream error: {e}");
                    if closing_deadline.is_none() {
                        engine.stop();
                        input.removeTapOnBus(0);
                        return Err(Fallback {
                            reason: format!("stream error mid-session: {e}"),
                            committed,
                        });
                    }
                    // Error while already flushing finals: end with what we have.
                    let _ = tx.send_blocking(VoiceEvent::Final(committed));
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_frame_count_at_double_rate() {
        // 32 kHz → 16 kHz: half the frames, 2 bytes each.
        let src = vec![0.0f32; 320];
        assert_eq!(resample_to_pcm16(&src, 32_000).len(), 160 * 2);
        // Matching rate passes straight through.
        assert_eq!(resample_to_pcm16(&src, TARGET_RATE).len(), 320 * 2);
        // 48 kHz is the common mac device rate: 3:1.
        assert_eq!(resample_to_pcm16(&vec![0.0f32; 480], 48_000).len(), 160 * 2);
    }

    #[test]
    fn resample_preserves_amplitude_and_clamps() {
        let out = resample_to_pcm16(&[1.0, 1.0, 1.0, 1.0], 32_000);
        let first = i16::from_le_bytes([out[0], out[1]]);
        assert_eq!(first, i16::MAX);
        // Out-of-range input must not wrap to a negative sample.
        let hot = resample_to_pcm16(&[9.0, 9.0], TARGET_RATE);
        assert_eq!(i16::from_le_bytes([hot[0], hot[1]]), i16::MAX);
    }

    #[test]
    fn resample_handles_empty_and_zero_rate() {
        assert!(resample_to_pcm16(&[], 48_000).is_empty());
        assert!(resample_to_pcm16(&[0.5], 0).is_empty());
    }

    #[test]
    fn parses_results_messages() {
        let msg = r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":"hello world","confidence":0.99}]}}"#;
        assert_eq!(parse_transcript(msg), Some(("hello world".into(), true)));
        let interim = r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":"hel"}]}}"#;
        assert_eq!(parse_transcript(interim), Some(("hel".into(), false)));
    }

    #[test]
    fn ignores_empty_and_nontranscript_messages() {
        assert_eq!(
            parse_transcript(
                r#"{"type":"Results","channel":{"alternatives":[{"transcript":"  "}]}}"#
            ),
            None
        );
        assert_eq!(parse_transcript(r#"{"type":"Metadata"}"#), None);
        assert_eq!(parse_transcript("not json"), None);
    }

    #[test]
    fn duplicate_final_segment_is_detected() {
        // The exact repeat Deepgram sends at end-of-utterance.
        assert!(already_committed(
            "Never forget. That's what this is.",
            "That's what this is."
        ));
        // Re-punctuated / re-capitalized repeat of the same words.
        assert!(already_committed(
            "Never forget. That's what this is",
            "thats what this is"
        ));
        // Genuinely new speech is not a duplicate.
        assert!(!already_committed("Never forget.", "Don't freeze up."));
        // A segment that merely CONTAINS earlier words mid-way is not a repeat
        // of the tail, so it must still be appended.
        assert!(!already_committed("the cat sat on the mat", "the cat"));
        assert!(!already_committed("", "anything"));
    }

    #[test]
    fn accumulating_finals_with_a_duplicate_writes_each_segment_once() {
        // Simulates: interim window closes, then endpointing repeats it.
        let finals = [
            "Never forget.",
            "That's what this is.",
            "That's what this is.", // endpoint repeat
            "Don't freeze up.",
        ];
        let mut committed = String::new();
        for f in finals {
            if !already_committed(&committed, f) {
                committed = join(&committed, f);
            }
        }
        assert_eq!(
            committed,
            "Never forget. That's what this is. Don't freeze up."
        );
        assert_eq!(committed.matches("That's what this is").count(), 1);
    }
}
