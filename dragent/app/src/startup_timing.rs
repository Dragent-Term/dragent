//! Per-phase startup timing, one log line per launch.
//!
//! Each phase records, once, how long after `main()` it finished. When the
//! window shows its first usable frame the app logs
//!
//! ```text
//! startup: total=812ms settings=40ms window=95ms first_frame=180ms usable=812ms host=pending terminals=pending auth=pending pre_main=60ms
//! ```
//!
//! and once the phases that finish after the window are in (host service
//! connected, restored terminals attached, sign-in refreshed) a second line:
//!
//! ```text
//! startup (complete): total=2210ms usable=812ms host=640ms terminals=1290ms auth=2210ms
//! ```
//!
//! Times count from `main()`; `pre_main` is how long the OS took to get there
//! (loading the executable, and on macOS the one-off scan of a freshly
//! installed binary: several seconds on the first launch after an update).
//! A phase that doesn't apply to this launch (no sign-in to refresh, no
//! terminal to attach) reads `-`; one that never finished within
//! [`COMPLETE_DEADLINE`] reads `pending` on the second line. Both lines go to
//! the app log (`<DRAGENT_HOME>/logs/desktop.log`).
//!
//! `--bench-startup` (Dev edition only) prints the first line on stdout and
//! exits as soon as the first usable frame is drawn, so a harness can run
//! cold starts N times; `--bench-startup=all` waits for the second line.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use web_time::Instant;

/// Command-line flag of the startup benchmark.
pub const BENCH_FLAG: &str = "--bench-startup";

/// How long the late phases get before the second line reports them pending.
const COMPLETE_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Persisted settings read (typography, index settings).
    Settings,
    /// The main window exists.
    Window,
    /// The first frame was drawn (the loading wordmark, at least).
    FirstFrame,
    /// The first frame showing the workspace (or onboarding) was drawn.
    Usable,
    /// The background host service answered for the first time.
    Host,
    /// Every terminal restored at launch is attached to its shell.
    Terminals,
    /// The sign-in refresh answered (success, sign-out, or offline).
    Auth,
}

impl Phase {
    const ALL: [Phase; 7] = [
        Phase::Settings,
        Phase::Window,
        Phase::FirstFrame,
        Phase::Usable,
        Phase::Host,
        Phase::Terminals,
        Phase::Auth,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Phase::Settings => "settings",
            Phase::Window => "window",
            Phase::FirstFrame => "first_frame",
            Phase::Usable => "usable",
            Phase::Host => "host",
            Phase::Terminals => "terminals",
            Phase::Auth => "auth",
        }
    }

    /// Phases that may finish after the window is usable.
    fn is_late(self) -> bool {
        matches!(self, Phase::Host | Phase::Terminals | Phase::Auth)
    }
}

/// What is known about one phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Pending,
    /// Finished this many milliseconds after `main()`.
    Done(u64),
    /// Doesn't apply to this launch.
    Skipped,
}

impl Mark {
    fn settled(self) -> bool {
        !matches!(self, Mark::Pending)
    }

    fn render(self) -> String {
        match self {
            Mark::Pending => "pending".to_string(),
            Mark::Done(ms) => format!("{ms}ms"),
            Mark::Skipped => "-".to_string(),
        }
    }
}

/// The marks of one launch. Lock-free: phases are recorded from the UI
/// thread and from background threads alike.
pub struct Timeline {
    /// 0 = pending, `u64::MAX` = skipped, otherwise ms + 1.
    marks: [AtomicU64; 7],
    usable_reported: AtomicBool,
    complete_reported: AtomicBool,
}

const SKIPPED: u64 = u64::MAX;

impl Timeline {
    pub const fn new() -> Self {
        Self {
            marks: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            usable_reported: AtomicBool::new(false),
            complete_reported: AtomicBool::new(false),
        }
    }

    pub fn get(&self, phase: Phase) -> Mark {
        match self.marks[phase.index()].load(Ordering::Acquire) {
            0 => Mark::Pending,
            SKIPPED => Mark::Skipped,
            raw => Mark::Done(raw - 1),
        }
    }

    /// Record `phase` at `ms`; only the first record of a phase counts.
    /// Returns whether this call recorded it.
    pub fn record(&self, phase: Phase, ms: u64) -> bool {
        let raw = ms.min(SKIPPED - 2) + 1;
        self.marks[phase.index()]
            .compare_exchange(0, raw, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn skip(&self, phase: Phase) -> bool {
        self.marks[phase.index()]
            .compare_exchange(0, SKIPPED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn usable(&self) -> bool {
        self.get(Phase::Usable).settled()
    }

    pub fn complete(&self) -> bool {
        self.usable()
            && Phase::ALL
                .iter()
                .filter(|p| p.is_late())
                .all(|p| self.get(*p).settled())
    }

    /// The first line: everything up to the usable window, late phases as
    /// far as they got.
    pub fn usable_line(&self) -> String {
        let total = self.get(Phase::Usable).render();
        let mut line = format!("startup: total={total}");
        for phase in Phase::ALL {
            line.push_str(&format!(" {}={}", phase.label(), self.get(phase).render()));
        }
        line
    }

    /// The second line: the late phases, with the launch's total being the
    /// last of everything that finished.
    pub fn complete_line(&self) -> String {
        let total = Phase::ALL
            .iter()
            .filter_map(|p| match self.get(*p) {
                Mark::Done(ms) => Some(ms),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let mut line = format!(
            "startup (complete): total={total}ms usable={}",
            self.get(Phase::Usable).render()
        );
        for phase in Phase::ALL.iter().filter(|p| p.is_late()) {
            line.push_str(&format!(" {}={}", phase.label(), self.get(*phase).render()));
        }
        line
    }

    /// Claim the first line, once.
    fn take_usable_report(&self) -> bool {
        self.usable() && !self.usable_reported.swap(true, Ordering::AcqRel)
    }

    /// Claim the second line, once; `force` after the deadline.
    fn take_complete_report(&self, force: bool) -> bool {
        (force || self.complete())
            && self.usable_reported.load(Ordering::Acquire)
            && !self.complete_reported.swap(true, Ordering::AcqRel)
    }
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

static TIMELINE: Timeline = Timeline::new();
/// Milliseconds between the OS starting this process and `main()` (loading
/// the executable; on macOS also the scan of a freshly installed binary,
/// several seconds on its first launch). `u64::MAX` = unknown.
static PRE_MAIN_MS: AtomicU64 = AtomicU64::new(u64::MAX);
static START: OnceLock<Instant> = OnceLock::new();
/// 0 = not a benchmark, 1 = exit at the usable frame, 2 = exit when complete.
static BENCH: AtomicU8 = AtomicU8::new(0);

/// Note when the process started. Call first thing in `main()`; later calls
/// change nothing.
pub fn process_started() {
    START.get_or_init(Instant::now);
    if let Some(ms) = time_before_main_ms() {
        PRE_MAIN_MS.store(ms, Ordering::Relaxed);
    }
    let mode = bench_mode(std::env::args());
    if mode != 0 && dragent_paths::is_dev() {
        BENCH.store(mode, Ordering::Relaxed);
    }
}

fn bench_mode(args: impl IntoIterator<Item = String>) -> u8 {
    for arg in args {
        if arg == BENCH_FLAG {
            return 1;
        }
        if let Some(value) = arg
            .strip_prefix(BENCH_FLAG)
            .and_then(|v| v.strip_prefix('='))
        {
            return if value == "all" { 2 } else { 1 };
        }
    }
    0
}

/// Whether this launch is the startup benchmark.
pub fn is_bench() -> bool {
    BENCH.load(Ordering::Relaxed) != 0
}

fn elapsed_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Record that `phase` finished now.
pub fn mark(phase: Phase) {
    if TIMELINE.record(phase, elapsed_ms()) {
        after_change();
    }
}

/// Record that `phase` doesn't apply to this launch.
pub fn skip(phase: Phase) {
    if TIMELINE.skip(phase) {
        after_change();
    }
}

/// Restored terminals of the launch window still attaching to their shells.
static TERMINALS_WAITING: std::sync::Mutex<Option<std::collections::HashSet<usize>>> =
    std::sync::Mutex::new(None);

/// The terminals (tab ids) the launch window restores. Only the first call
/// counts; with none, the phase doesn't apply.
pub fn expect_terminals(tab_ids: impl IntoIterator<Item = usize>) {
    let Ok(mut waiting) = TERMINALS_WAITING.lock() else {
        return;
    };
    if waiting.is_some() || is_settled(Phase::Terminals) {
        return;
    }
    let ids: std::collections::HashSet<usize> = tab_ids.into_iter().collect();
    let none = ids.is_empty();
    *waiting = Some(ids);
    drop(waiting);
    if none {
        skip(Phase::Terminals);
        // Nothing at launch needs the host service then; it starts when
        // the first terminal opens.
        skip(Phase::Host);
    }
}

/// A restored terminal reached its final state (attached, or settled as
/// gone); the phase ends with the last one.
pub fn terminal_settled(tab_id: usize) {
    let Ok(mut waiting) = TERMINALS_WAITING.lock() else {
        return;
    };
    let Some(ids) = waiting.as_mut() else { return };
    if ids.remove(&tab_id) && ids.is_empty() {
        drop(waiting);
        mark(Phase::Terminals);
    }
}

/// Whether `phase` has been recorded (done or skipped).
pub fn is_settled(phase: Phase) -> bool {
    TIMELINE.get(phase).settled()
}

fn with_pre_main(line: String) -> String {
    match PRE_MAIN_MS.load(Ordering::Relaxed) {
        u64::MAX => format!("{line} pre_main=-"),
        ms => format!("{line} pre_main={ms}ms"),
    }
}

fn after_change() {
    if TIMELINE.take_usable_report() {
        let line = with_pre_main(TIMELINE.usable_line());
        log::info!("{line}");
        if BENCH.load(Ordering::Relaxed) == 1 {
            bench_exit(&line);
        }
        // Report whatever is in by the deadline, so a phase that never
        // finishes still leaves a second line.
        #[cfg(not(target_family = "wasm"))]
        let _ = std::thread::Builder::new()
            .name("dragent-startup-timing".into())
            .spawn(|| {
                std::thread::sleep(COMPLETE_DEADLINE);
                report_complete(true);
            });
    }
    report_complete(false);
}

fn report_complete(force: bool) {
    if TIMELINE.take_complete_report(force) {
        let line = TIMELINE.complete_line();
        log::info!("{line}");
        if BENCH.load(Ordering::Relaxed) == 2 {
            bench_exit(&format!(
                "{}\n{line}",
                with_pre_main(TIMELINE.usable_line())
            ));
        }
    }
}

/// How long ago the OS started this process, read at `main()`.
#[cfg(target_os = "macos")]
fn time_before_main_ms() -> Option<u64> {
    // SAFETY: proc_pidinfo fills at most `size` bytes of a zeroed plain struct.
    let info = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        );
        if got != size {
            return None;
        }
        info
    };
    let started = std::time::UNIX_EPOCH
        + Duration::from_secs(info.pbi_start_tvsec)
        + Duration::from_micros(info.pbi_start_tvusec);
    std::time::SystemTime::now()
        .duration_since(started)
        .ok()
        .map(|age| age.as_millis() as u64)
}

/// How long ago the OS started this process, read at `main()`.
#[cfg(target_os = "linux")]
fn time_before_main_ms() -> Option<u64> {
    // Field 22 of /proc/self/stat: start time in clock ticks after boot
    // (fields counted after the parenthesised command name, which may
    // contain spaces).
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    let start_ticks: f64 = after_name.split_whitespace().nth(19)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks_per_sec <= 0 {
        return None;
    }
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let age = uptime - start_ticks / ticks_per_sec as f64;
    (age >= 0.0).then(|| (age * 1000.0) as u64)
}

/// Not measured on this platform; the line reads `pre_main=-`.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn time_before_main_ms() -> Option<u64> {
    None
}

fn bench_exit(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
    #[cfg(not(target_family = "wasm"))]
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_keeps_its_first_record() {
        let t = Timeline::new();
        assert!(t.record(Phase::Window, 50));
        assert!(!t.record(Phase::Window, 90));
        assert!(!t.skip(Phase::Window));
        assert_eq!(t.get(Phase::Window), Mark::Done(50));
        assert_eq!(t.get(Phase::Host), Mark::Pending);
    }

    #[test]
    fn first_line_waits_for_usable_and_shows_late_phases_pending() {
        let t = Timeline::new();
        t.record(Phase::Settings, 10);
        t.record(Phase::Window, 40);
        t.record(Phase::FirstFrame, 70);
        assert!(!t.take_usable_report());
        t.record(Phase::Usable, 300);
        assert!(t.take_usable_report());
        assert!(!t.take_usable_report(), "reported once");
        assert_eq!(
            t.usable_line(),
            "startup: total=300ms settings=10ms window=40ms first_frame=70ms usable=300ms \
             host=pending terminals=pending auth=pending"
        );
    }

    #[test]
    fn second_line_comes_once_every_late_phase_settled() {
        let t = Timeline::new();
        t.record(Phase::Usable, 300);
        assert!(t.take_usable_report());
        t.record(Phase::Host, 200);
        t.record(Phase::Terminals, 900);
        assert!(!t.take_complete_report(false));
        t.skip(Phase::Auth);
        assert!(t.take_complete_report(false));
        assert!(!t.take_complete_report(true), "reported once");
        assert_eq!(
            t.complete_line(),
            "startup (complete): total=900ms usable=300ms host=200ms terminals=900ms auth=-"
        );
    }

    #[test]
    fn deadline_reports_unfinished_phases_as_pending() {
        let t = Timeline::new();
        t.record(Phase::Usable, 300);
        t.take_usable_report();
        t.record(Phase::Host, 500);
        assert!(t.take_complete_report(true));
        assert_eq!(
            t.complete_line(),
            "startup (complete): total=500ms usable=300ms host=500ms terminals=pending auth=pending"
        );
    }

    #[test]
    fn late_phases_finishing_before_the_window_wait_for_the_first_line() {
        let t = Timeline::new();
        t.record(Phase::Host, 20);
        t.skip(Phase::Terminals);
        t.skip(Phase::Auth);
        assert!(
            !t.take_complete_report(false),
            "no second line before the first"
        );
        t.record(Phase::Usable, 100);
        assert!(t.take_usable_report());
        assert!(t.take_complete_report(false));
    }

    #[test]
    fn bench_flag_parsing() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(bench_mode(args(&["dragent"])), 0);
        assert_eq!(bench_mode(args(&["dragent", "--bench-startup"])), 1);
        assert_eq!(bench_mode(args(&["dragent", "--bench-startup=all"])), 2);
        assert_eq!(bench_mode(args(&["dragent", "--bench-startup=usable"])), 1);
        assert_eq!(bench_mode(args(&["dragent", "--bench-startupx"])), 0);
    }
}
