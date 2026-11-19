//! Desktop notifications and their sound, for every agent (terminal agents
//! and Dragent's own) and for automation runs. What's on, which sound and
//! how loud come from Settings → Notifications.
//!
//! The sound is Dragent's own, played by the app, so it rings on every
//! platform and even when the system mutes notification sounds. It also
//! plays for the tab being looked at: the person may well be busy with
//! something else. The chimes are synthesized here, not bundled audio.

use serde::{Deserialize, Serialize};

/// The sounds to choose from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chime {
    /// Two rising notes.
    #[default]
    Chime,
    /// One bell strike that rings out.
    Bell,
    /// Two soft wooden notes.
    Marimba,
    /// A short drop.
    Pop,
}

impl Chime {
    pub const ALL: [Chime; 4] = [Chime::Chime, Chime::Bell, Chime::Marimba, Chime::Pop];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chime => "Chime",
            Self::Bell => "Bell",
            Self::Marimba => "Marimba",
            Self::Pop => "Pop",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Chime => "chime",
            Self::Bell => "bell",
            Self::Marimba => "marimba",
            Self::Pop => "pop",
        }
    }
}

fn on() -> bool {
    true
}

/// Quiet.
fn default_volume() -> u8 {
    35
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default = "on")]
    pub agent_finished: bool,
    /// Waiting for an approval, a permission or an answer.
    #[serde(default = "on")]
    pub agent_needs_you: bool,
    #[serde(default = "on")]
    pub agent_errors: bool,
    /// Automation runs that finished, failed or need someone.
    #[serde(default = "on")]
    pub automations: bool,
    #[serde(default = "on")]
    pub sound: bool,
    #[serde(default)]
    pub chime: Chime,
    /// 0–100.
    #[serde(default = "default_volume")]
    pub volume: u8,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            agent_finished: true,
            agent_needs_you: true,
            agent_errors: true,
            automations: true,
            sound: true,
            chime: Chime::default(),
            volume: default_volume(),
        }
    }
}

/// What a notification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    AgentFinished,
    AgentNeedsYou,
    AgentError,
    Automation,
}

impl NotificationSettings {
    pub fn wants(&self, kind: Kind) -> bool {
        match kind {
            Kind::AgentFinished => self.agent_finished,
            Kind::AgentNeedsYou => self.agent_needs_you,
            Kind::AgentError => self.agent_errors,
            Kind::Automation => self.automations,
        }
    }
}

// ── The sounds ───────────────────────────────────────────────────────────────

const RATE: u32 = 22_050;

/// One partial of a note: its frequency as a multiple of the note's, its
/// loudness, and how fast it fades (higher partials fade faster).
struct Note {
    frequency: f32,
    start: f32,
    partials: &'static [(f32, f32)],
    decay: f32,
}

fn notes(chime: Chime) -> (Vec<Note>, f32) {
    const BRIGHT: &[(f32, f32)] = &[(1.0, 1.0), (2.0, 0.22), (3.0, 0.06)];
    const BELL: &[(f32, f32)] = &[(1.0, 1.0), (2.0, 0.45), (2.76, 0.3), (5.4, 0.12)];
    const WOOD: &[(f32, f32)] = &[(1.0, 1.0), (4.0, 0.22), (10.0, 0.04)];
    match chime {
        // E6 then A6.
        Chime::Chime => (
            vec![
                Note {
                    frequency: 1318.51,
                    start: 0.0,
                    partials: BRIGHT,
                    decay: 0.22,
                },
                Note {
                    frequency: 1760.0,
                    start: 0.11,
                    partials: BRIGHT,
                    decay: 0.26,
                },
            ],
            0.75,
        ),
        // C6, ringing.
        Chime::Bell => (
            vec![Note {
                frequency: 1046.5,
                start: 0.0,
                partials: BELL,
                decay: 0.5,
            }],
            1.3,
        ),
        // A5 then E6.
        Chime::Marimba => (
            vec![
                Note {
                    frequency: 880.0,
                    start: 0.0,
                    partials: WOOD,
                    decay: 0.12,
                },
                Note {
                    frequency: 1318.51,
                    start: 0.09,
                    partials: WOOD,
                    decay: 0.14,
                },
            ],
            0.55,
        ),
        // Handled as a sweep in `samples`.
        Chime::Pop => (Vec::new(), 0.24),
    }
}

/// The chime as samples in -1..1, at full loudness.
fn samples(chime: Chime) -> Vec<f32> {
    use std::f32::consts::TAU;
    let (notes, length) = notes(chime);
    let count = (length * RATE as f32) as usize;
    let mut out = vec![0.0f32; count];
    if chime == Chime::Pop {
        // A quick downward sweep, 1100 → 560 Hz, with an octave on top so
        // it cuts through.
        let mut phase = 0.0f32;
        for (index, sample) in out.iter_mut().enumerate() {
            let t = index as f32 / RATE as f32;
            let frequency = 560.0 + 540.0 * (-t / 0.03).exp();
            phase += TAU * frequency / RATE as f32;
            *sample = (phase.sin() + 0.3 * (2.0 * phase).sin()) * (-t / 0.06).exp();
        }
    }
    for note in &notes {
        let first = (note.start * RATE as f32) as usize;
        for (index, sample) in out.iter_mut().enumerate().skip(first) {
            let t = (index - first) as f32 / RATE as f32;
            for (ratio, level) in note.partials {
                let decay = note.decay / ratio.sqrt();
                *sample += level * (TAU * note.frequency * ratio * t).sin() * (-t / decay).exp();
            }
        }
    }
    // Soft attack and release, so it never clicks.
    let attack = (0.004 * RATE as f32) as usize;
    let release = (0.02 * RATE as f32) as usize;
    for index in 0..attack.min(count) {
        out[index] *= index as f32 / attack as f32;
    }
    for index in 0..release.min(count) {
        out[count - 1 - index] *= index as f32 / release as f32;
    }
    // Normalise, then round off the peaks so the ring-out carries. The
    // chimes already peak near full scale; it's their brief fade that makes
    // them easy to miss.
    const DRIVE: f32 = 2.5;
    let peak = out
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    if peak > 0.0 {
        for sample in &mut out {
            *sample = 0.98 * (DRIVE * *sample / peak).tanh() / DRIVE.tanh();
        }
    }
    out
}

/// How loud the loudest setting plays, as a fraction of full scale. Full
/// scale was too loud.
const LOUDEST: f32 = 0.40;

/// The gain for a volume setting (0–100): Loud, Medium and Quiet play at
/// 40%, 30% and 20% of full scale; any other value scales linearly up to the
/// loudest.
fn gain(volume: u8) -> f32 {
    match volume.min(100) {
        100 => 0.40,
        70 => 0.30,
        35 => 0.20,
        other => LOUDEST * other as f32 / 100.0,
    }
}

/// The chime at `volume` (0–100) as a 16-bit mono WAV file.
pub fn wav(chime: Chime, volume: u8) -> Vec<u8> {
    let gain = gain(volume);
    let pcm: Vec<i16> = samples(chime)
        .into_iter()
        .map(|sample| (sample * gain * i16::MAX as f32) as i16)
        .collect();
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

// ── Playing it ───────────────────────────────────────────────────────────────

/// Play `chime` at `volume` in the background. Several notifications at
/// once ring once.
#[cfg(not(target_family = "wasm"))]
pub fn play(chime: Chime, volume: u8) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|poison| poison.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_millis(1200)) {
            return;
        }
        *last = Some(Instant::now());
    }
    std::thread::spawn(move || {
        if let Err(error) = play_now(chime, volume) {
            log::info!("[notifications] couldn't play the sound: {error}");
        }
    });
}

#[cfg(target_family = "wasm")]
pub fn play(_chime: Chime, _volume: u8) {}

/// The WAV for `chime` at `volume`, written once to a cache file the
/// system's player can open.
#[cfg(not(target_family = "wasm"))]
fn sound_file(chime: Chime, volume: u8) -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("dragent-sounds");
    std::fs::create_dir_all(&dir)?;
    // The prefix changes whenever the synthesis does, so old files aren't replayed.
    let path = dir.join(format!("v5-{}-{}.wav", chime.key(), volume.min(100)));
    if !path.is_file() {
        let partial = path.with_extension("wav.part");
        std::fs::write(&partial, wav(chime, volume))?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(path)
}

#[cfg(not(target_family = "wasm"))]
fn play_now(chime: Chime, volume: u8) -> Result<(), String> {
    use crate::process_ext::NoWindow;
    use std::process::{Command, Stdio};
    let path = sound_file(chime, volume).map_err(|error| error.to_string())?;
    let file = path.to_string_lossy().to_string();
    let players: Vec<(&str, Vec<String>)> = if cfg!(target_os = "macos") {
        vec![("/usr/bin/afplay", vec![file])]
    } else if cfg!(windows) {
        let quoted = file.replace('\'', "''");
        vec![(
            "powershell",
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                format!("(New-Object System.Media.SoundPlayer '{quoted}').PlaySync()"),
            ],
        )]
    } else {
        vec![
            ("paplay", vec![file.clone()]),
            ("pw-play", vec![file.clone()]),
            ("aplay", vec!["-q".into(), file]),
        ]
    };
    for (program, args) in players {
        let started = Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .no_window()
            .spawn();
        if let Ok(mut child) = started {
            let _ = child.wait();
            return Ok(());
        }
    }
    Err("no audio player found".into())
}

// ── Sending one ──────────────────────────────────────────────────────────────

/// Show a desktop notification and ring, as Settings → Notifications says.
/// `data` comes back when the notification is clicked.
/// Longest finished-turn summary on a notification.
const SUMMARY_CHARS: usize = 160;

/// An agent's notification: titled "<agent> · <workspace>" (the phone shows
/// the agent's icon by the name before the " · "), saying what happened in
/// a short sentence. A finished turn says what the agent replied, its first
/// line, so the notification is worth reading without opening anything.
pub fn agent_alert(agent: &str, workspace: &str, kind: Kind, reply: Option<&str>) -> (String, String) {
    let workspace = workspace.trim();
    let title = if workspace.is_empty() { agent.to_string() } else { format!("{agent} · {workspace}") };
    let body = match kind {
        Kind::AgentNeedsYou => "Wants your permission to continue".to_string(),
        Kind::AgentError => "Stopped with an error".to_string(),
        Kind::AgentFinished | Kind::Automation => reply
            .and_then(summary)
            .unwrap_or_else(|| "Finished · ready for your review".to_string()),
    };
    (title, body)
}

/// A reply's first line of prose: headings, code fences and blank lines
/// skipped, list and quote marks dropped, cut at a word to fit.
fn summary(reply: &str) -> Option<String> {
    let line = reply
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.starts_with("```"))
        .map(|line| line.trim_start_matches(['>', '*', '-', ' ']).trim())
        .find(|line| !line.is_empty())?;
    let line = line.replace("**", "").replace('`', "");
    if line.chars().count() <= SUMMARY_CHARS {
        return Some(line);
    }
    let cut: String = line.chars().take(SUMMARY_CHARS).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head).unwrap_or(&cut);
    Some(format!("{}…", cut.trim_end_matches([',', ';', ':', '.'])))
}

pub fn notify<V: warpui::View>(
    ctx: &mut warpui::ViewContext<V>,
    kind: Kind,
    title: &str,
    body: &str,
    data: Option<String>,
) {
    notify_for_pane(ctx, kind, title, body, data, None);
}

/// [`notify`] about an agent's pane: the phone can answer a permission
/// request from the notification itself.
pub fn notify_for_pane<V: warpui::View>(
    ctx: &mut warpui::ViewContext<V>,
    kind: Kind,
    title: &str,
    body: &str,
    data: Option<String>,
    #[allow(unused_variables)] pane: Option<uuid::Uuid>,
) {
    let settings = crate::settings::AppSettings::global().notifications.clone();
    if !settings.wants(kind) {
        return;
    }
    // Paired phones get the same notification, end-to-end encrypted
    // (docs/design/e2ee.md §8); this setting decides for them too.
    #[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
    {
        use crate::auth::{AuthManager, AuthStatus};
        use warpui::SingletonEntity;
        if let AuthStatus::LoggedIn(user) = AuthManager::as_ref(ctx).status() {
            if let Some(user_id) = user.user_id.as_deref() {
                crate::realtime::e2e_notify::notify_for_pane(user_id, kind, title, body, pane);
            }
        }
    }
    if settings.sound {
        play(settings.chime, settings.volume);
    }
    use warpui_core::notification::UserNotification;
    let notification = UserNotification::new_with_sound(
        truncate(title, UserNotification::MAX_TITLE_LENGTH),
        truncate(body, UserNotification::MAX_BODY_LENGTH),
        data,
        // Dragent rings its own sound above; the system's would double it.
        false,
    );
    ctx.request_desktop_notification_permissions(move |_, outcome, ctx| {
        if matches!(
            outcome,
            warpui_core::notification::RequestPermissionsOutcome::Accepted
        ) {
            ctx.send_desktop_notification(notification, |_, error, _| {
                log::warn!("[notifications] delivery failed: {:?}", error);
            });
        }
    });
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut = cut.trim_end().to_string();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_alert_names_agent_and_workspace_and_says_what_happened() {
        let (title, body) = agent_alert("Claude Code", "fix-auth", Kind::AgentNeedsYou, None);
        assert_eq!((title.as_str(), body.as_str()), ("Claude Code · fix-auth", "Wants your permission to continue"));
        assert_eq!(agent_alert("Codex", "  ", Kind::AgentError, None), ("Codex".into(), "Stopped with an error".into()));
        // A finished turn quotes the first line of prose it replied.
        let reply = "## Summary\n\n**Fixed** the `login` redirect loop.\n\n- Changed router.ts";
        assert_eq!(agent_alert("Codex", "web", Kind::AgentFinished, Some(reply)).1, "Fixed the login redirect loop.");
        assert_eq!(agent_alert("Codex", "web", Kind::AgentFinished, Some("```\n")).1, "Finished · ready for your review");
        assert_eq!(agent_alert("Codex", "web", Kind::AgentFinished, None).1, "Finished · ready for your review");
        // Long replies are cut at a word.
        let long = format!("{} end", "word ".repeat(60));
        let body = agent_alert("Amp", "w", Kind::AgentFinished, Some(&long)).1;
        assert!(body.ends_with("word…") && body.chars().count() <= SUMMARY_CHARS + 1, "{body}");
    }

    #[test]
    fn every_chime_is_a_short_valid_wav() {
        for chime in Chime::ALL {
            let bytes = wav(chime, 70);
            assert_eq!(&bytes[0..4], b"RIFF", "{chime:?}");
            assert_eq!(&bytes[8..16], b"WAVEfmt ");
            let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
            assert_eq!(bytes.len(), 44 + data_len);
            let seconds = data_len as f32 / 2.0 / RATE as f32;
            assert!(seconds > 0.1 && seconds < 1.5, "{chime:?} lasts {seconds}s");
            // It starts and ends quietly (no clicks).
            let first = i16::from_le_bytes([bytes[44], bytes[45]]);
            let last = i16::from_le_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]);
            assert!(first.abs() < 200 && last.abs() < 200, "{chime:?}");
        }
    }

    #[test]
    fn volume_scales_the_sound_and_zero_is_silent() {
        let peak = |volume| {
            wav(Chime::Bell, volume)[44..]
                .chunks(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs())
                .max()
                .unwrap()
        };
        assert_eq!(peak(0), 0);
        assert!(peak(30) < peak(70) && peak(70) < peak(100));
        assert!(peak(100) > 10_000);
    }

    #[test]
    fn loud_medium_and_quiet_play_at_40_30_and_20_percent() {
        use crate::settings::pages::notifications::VOLUMES;
        let gain_of = |name: &str| {
            gain(VOLUMES.iter().find(|(label, _)| *label == name).unwrap().1)
        };
        assert!((gain_of("Loud") - 0.40).abs() < 1e-6);
        assert!((gain_of("Medium") - 0.30).abs() < 1e-6);
        assert!((gain_of("Quiet") - 0.20).abs() < 1e-6);
        // A new install starts on Quiet.
        assert_eq!(NotificationSettings::default().volume, VOLUMES[0].1);
    }

    /// macOS's own audio tools read every sound file the app writes.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reads_every_sound_file() {
        for chime in Chime::ALL {
            let path = sound_file(chime, 40).unwrap();
            let output = std::process::Command::new("/usr/bin/afinfo")
                .arg(&path)
                .output()
                .unwrap();
            let report = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{chime:?}: {report}");
            assert!(report.contains("22050 Hz"), "{chime:?}: {report}");
        }
    }

    #[test]
    fn settings_default_to_everything_on_and_read_old_files() {
        let settings: NotificationSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, NotificationSettings::default());
        assert!(settings.wants(Kind::AgentFinished) && settings.sound);
        let quiet: NotificationSettings =
            serde_json::from_str(r#"{"sound": false, "chime": "pop", "agent_errors": false}"#)
                .unwrap();
        assert!(!quiet.sound && !quiet.wants(Kind::AgentError) && quiet.wants(Kind::Automation));
        assert_eq!(quiet.chime, Chime::Pop);
    }
}
