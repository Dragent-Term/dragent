//! When a host or terminal service closes down on its own.
//!
//! Each service has a record file in its data folder (`host-v1.json`,
//! `pty-daemon.json`) that is the only way a client finds it. A service whose
//! data folder or record is gone, or whose record now names another service,
//! can never be reached again: it closes down after two checks in a row
//! (one a minute) agree. A service started by a test also closes down once
//! the test process is gone, even when the test was killed mid-run.
//!
//! Idle rules differ per kind: the host closes after a stretch with no client
//! at all (the app restarts it in well under a second), while the terminal
//! service never closes for want of a client — it keeps shells alive — only
//! when it holds no terminals. Nor does it close for a missing record while it
//! holds any: it writes its record again instead.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How often the record and data folder are looked at.
pub const CHECK_EVERY: Duration = Duration::from_secs(60);
/// Test override for [`CHECK_EVERY`], in milliseconds.
pub const CHECK_EVERY_ENV: &str = "DRAGENT_SERVICE_CHECK_MS";
/// A host with no client for this long closes.
pub const HOST_IDLE_EXIT: Duration = Duration::from_secs(600);
/// Test override for [`HOST_IDLE_EXIT`], in milliseconds.
pub const HOST_IDLE_EXIT_ENV: &str = "DRAGENT_HOST_IDLE_EXIT_MS";
/// A terminal service holding no terminals for this long closes.
pub const PTY_EMPTY_EXIT: Duration = Duration::from_secs(600);
/// Test override for [`PTY_EMPTY_EXIT`], in milliseconds.
pub const PTY_EMPTY_EXIT_ENV: &str = "DRAGENT_PTY_EMPTY_EXIT_MS";
/// Set by test helpers: services close once this process id is gone. Never
/// passed on to shells (see `dragent_pty::shell_env`).
pub const EXIT_WITH_PID_ENV: &str = dragent_pty::EXIT_WITH_PID_ENV;
/// Written by a terminal service that closed because it held nothing, so its
/// supervisor does not mistake the exit for a crash and start it again.
pub const PTY_IDLE_EXIT_MARKER: &str = "pty-daemon-idle-exit";
/// Consecutive bad checks before a service gives up on its record.
pub const STRIKES_TO_EXIT: u32 = 2;

fn duration_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|ms| ms.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(default)
}

pub fn check_every() -> Duration {
    duration_env(CHECK_EVERY_ENV, CHECK_EVERY)
}

pub fn host_idle_exit() -> Duration {
    duration_env(HOST_IDLE_EXIT_ENV, HOST_IDLE_EXIT)
}

pub fn pty_empty_exit() -> Duration {
    duration_env(PTY_EMPTY_EXIT_ENV, PTY_EMPTY_EXIT)
}

/// How often a service's housekeeping loop wakes: often enough to notice its
/// test parent going away quickly, never more than the record check.
pub fn tick() -> Duration {
    check_every()
        .min(Duration::from_secs(2))
        .max(Duration::from_millis(20))
}

/// The process this service must not outlive, read once at start.
pub fn exit_with_pid() -> Option<u32> {
    static PID: OnceLock<Option<u32>> = OnceLock::new();
    *PID.get_or_init(|| {
        std::env::var(EXIT_WITH_PID_ENV)
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 1)
    })
}

/// The test process that started this service has gone.
pub fn parent_gone() -> bool {
    exit_with_pid().is_some_and(|pid| !crate::stale::process_may_be_alive(pid))
}

/// What a service's record says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordState {
    /// The record names this service (or can't be read right now).
    Ours,
    /// The data folder holding the record no longer exists.
    DataFolderGone,
    /// The data folder exists; the record file does not.
    RecordGone,
    /// The record names another service (by pid).
    Replaced { pid: u32 },
}

impl RecordState {
    pub fn describe(&self) -> String {
        match self {
            Self::Ours => "its record names it".into(),
            Self::DataFolderGone => "its data folder is gone".into(),
            Self::RecordGone => "its record file is gone".into(),
            Self::Replaced { pid } => format!("its record names another service (pid {pid})"),
        }
    }
}

/// Look at `record` in `data_dir`: one stat for the folder, one read for the
/// record. `owner` parses the record's text into (pid, is-this-service);
/// a record that doesn't parse counts as ours (it may be mid-write).
pub fn record_state(
    data_dir: &Path,
    record: &Path,
    owner: impl Fn(&str) -> Option<(u32, bool)>,
) -> RecordState {
    if !data_dir.is_dir() {
        return RecordState::DataFolderGone;
    }
    match std::fs::read_to_string(record) {
        Ok(text) => match owner(&text) {
            Some((pid, false)) => RecordState::Replaced { pid },
            _ => RecordState::Ours,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RecordState::RecordGone,
        Err(_) => RecordState::Ours,
    }
}

/// Counts consecutive bad record checks: one odd look (a folder being
/// renamed, a record being replaced and restored by a failed update) never
/// closes a service on its own.
#[derive(Debug, Default)]
pub struct Strikes {
    count: u32,
}

impl Strikes {
    /// Record one check; returns the state to act on once enough checks in
    /// a row were bad.
    pub fn observe(&mut self, state: RecordState) -> Option<RecordState> {
        if state == RecordState::Ours {
            self.count = 0;
            return None;
        }
        self.count += 1;
        (self.count >= STRIKES_TO_EXIT).then_some(state)
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }
}

/// How long something has been idle, counted in short steps of running time.
///
/// Neither the wall clock (it can be set, and runs on through sleep) nor the
/// monotonic one (it includes sleep on some platforms) may decide that a
/// service went unused: a computer waking after a night must not close a
/// service whose app is only just waking too. Any single step counts for at
/// most twice the interval it is looked at.
#[derive(Debug)]
pub struct IdleClock {
    idle: Duration,
    last: Instant,
    max_step: Duration,
}

impl IdleClock {
    /// A clock looked at about every `every`.
    pub fn new(now: Instant, every: Duration) -> Self {
        Self {
            idle: Duration::ZERO,
            last: now,
            max_step: every * 2,
        }
    }

    /// Record one look; returns how long it has been idle without a break.
    pub fn look(&mut self, in_use: bool, now: Instant) -> Duration {
        let step = now.saturating_duration_since(self.last).min(self.max_step);
        self.last = now;
        self.idle = if in_use {
            Duration::ZERO
        } else {
            self.idle + step
        };
        self.idle
    }
}

pub fn idle_marker_path(org_dir: &Path) -> PathBuf {
    org_dir.join(PTY_IDLE_EXIT_MARKER)
}

/// Note that the terminal service `daemon_id` is closing because it is empty.
pub fn mark_idle_exit(org_dir: &Path, daemon_id: &str) {
    let _ = std::fs::write(idle_marker_path(org_dir), daemon_id);
}

/// Consume the idle-exit note, if any: true when the last terminal service
/// closed on purpose rather than crashing.
pub fn take_idle_exit(org_dir: &Path) -> bool {
    std::fs::remove_file(idle_marker_path(org_dir)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn owner(me: u32) -> impl Fn(&str) -> Option<(u32, bool)> {
        move |text| {
            let pid: u32 = text.trim().parse().ok()?;
            Some((pid, pid == me))
        }
    }

    #[test]
    fn record_state_covers_every_case() {
        let dir = scratch();
        let record = dir.join("record.json");
        assert_eq!(
            record_state(&dir, &record, owner(7)),
            RecordState::RecordGone
        );
        std::fs::write(&record, "7").unwrap();
        assert_eq!(record_state(&dir, &record, owner(7)), RecordState::Ours);
        std::fs::write(&record, "8").unwrap();
        assert_eq!(
            record_state(&dir, &record, owner(7)),
            RecordState::Replaced { pid: 8 }
        );
        // Half-written or foreign text never closes a service.
        std::fs::write(&record, "{").unwrap();
        assert_eq!(record_state(&dir, &record, owner(7)), RecordState::Ours);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            record_state(&dir, &record, owner(7)),
            RecordState::DataFolderGone
        );
    }

    #[test]
    fn one_bad_look_is_not_enough() {
        let mut strikes = Strikes::default();
        assert_eq!(strikes.observe(RecordState::RecordGone), None);
        assert_eq!(strikes.observe(RecordState::Ours), None);
        assert_eq!(strikes.observe(RecordState::RecordGone), None);
        assert_eq!(
            strikes.observe(RecordState::DataFolderGone),
            Some(RecordState::DataFolderGone)
        );
    }

    #[test]
    fn time_asleep_is_not_time_idle() {
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let mut clock = IdleClock::new(start, Duration::from_secs(5));
        assert_eq!(clock.look(false, at(2)), Duration::from_secs(2));
        // A night passes between two looks.
        assert_eq!(clock.look(false, at(30_000)), Duration::from_secs(12));
        assert_eq!(clock.look(false, at(30_002)), Duration::from_secs(14));
        // Use starts it over, and a step back in time adds nothing.
        assert_eq!(clock.look(true, at(30_004)), Duration::ZERO);
        assert_eq!(clock.look(false, at(1)), Duration::ZERO);
    }

    #[test]
    fn idle_marker_is_consumed_once() {
        let dir = scratch();
        assert!(!take_idle_exit(&dir));
        mark_idle_exit(&dir, "abc");
        assert!(take_idle_exit(&dir));
        assert!(!take_idle_exit(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
