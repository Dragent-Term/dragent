//! Dev-only benchmark hooks for `scripts/bench/`.
//!
//! Everything here is fenced: it only runs in Dev-edition builds
//! (`dragent_paths::is_dev()`, i.e. anything not built with
//! `DRAGENT_RELEASE_EDITION=1`) or `debug_assertions`, and only when the
//! operator asks for it:
//!
//! - `--bench-mem` (or `DRAGENT_BENCH_MEM=1`): log this process's memory
//!   (phys footprint on macOS) every 5 s to the log and stderr.
//! - `--bench-probe-startup`: print the `[bench] startup …` line and exit.
//!   Lets `run.py cold-start` time process init without opening a window.
//! - `DRAGENT_BENCH_INPUT=1`: enables the key→frame input-latency hook.
//!   `note_key_received()` marks a key event (automation `run` submissions
//!   count as the key-input equivalent); the warpui frame hook registered by
//!   `install_frame_hook()` calls `note_frame_presented()` after every
//!   presented frame, which logs the elapsed
//!   `bench.input key_to_frame_ms=…` line the harness parses.
//! - `DRAGENT_BENCH_FRAME=1`: per-frame times. Every presented frame records
//!   its delta; a background thread logs one aggregate line per second that
//!   had frames: `bench.frame ms=<p50> p95_ms=<p95> max_ms=<max> n=<n>`.
//!   Either `DRAGENT_BENCH_INPUT` or `DRAGENT_BENCH_FRAME` arms the warpui
//!   hook; without both it is one cached-bool load per frame.
//!
//! Log contract (shared with the perf overlay):
//! `bench.input key_to_frame_ms=<float>`,
//! `[bench] startup ready_ms=<ms> … frame=first-presented` (ready = the first
//! presented frame, which is also when `note_window_open()`'s window becomes
//! visible),
//! `bench.frame ms=<p50> p95_ms=<p95> max_ms=<max> n=<n>` (one line per second
//! with frames, so logs stay small).
//!
//! Native only: this module is not compiled for wasm.

use std::sync::OnceLock;
use std::time::Instant;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

/// Call first in `main()` (and again at the top of `desktop::run()`; the
/// second call is a no-op) so startup timing starts at process launch.
pub fn note_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

fn gated() -> bool {
    dragent_paths::is_dev() || cfg!(debug_assertions)
}

/// Whether `--bench-mem` / `DRAGENT_BENCH_MEM=1` observability is on.
pub fn mem_logging_requested() -> bool {
    if !gated() {
        return false;
    }
    if std::env::var_os("DRAGENT_BENCH_MEM").is_some() {
        return true;
    }
    std::env::args().any(|a| a == "--bench-mem")
}

/// Whether `--bench-probe-startup` headless startup timing is on.
pub fn probe_requested() -> bool {
    if !gated() {
        return false;
    }
    std::env::args().any(|a| a == "--bench-probe-startup")
}

/// Whether key→frame input-latency logging is on (`DRAGENT_BENCH_INPUT=1`).
pub fn input_timing_enabled() -> bool {
    if !gated() {
        return false;
    }
    std::env::var("DRAGENT_BENCH_INPUT").ok().as_deref() == Some("1")
}

/// Whether per-frame time aggregation is on (`DRAGENT_BENCH_FRAME=1`).
pub fn frame_timing_enabled() -> bool {
    if !gated() {
        return false;
    }
    std::env::var("DRAGENT_BENCH_FRAME").ok().as_deref() == Some("1")
}

/// Register `note_frame_presented()` with the warpui frame hook and start the
/// 1 s frame-stats flusher. Call once at startup; no-op unless
/// `DRAGENT_BENCH_INPUT=1`, `DRAGENT_BENCH_FRAME=1`, or the dev perf overlay
/// (`DRAGENT_PERF_OVERLAY=1` / toggle) requested frames (dev-gated inside).
pub fn install_frame_hook() {
    if !(input_timing_enabled() || frame_timing_enabled() || crate::perf_overlay::is_enabled()) {
        return;
    }
    warpui_core::bench_frame::set_frame_presented_callback(note_frame_presented);
    static FLUSHER: OnceLock<()> = OnceLock::new();
    FLUSHER.get_or_init(|| {
        std::thread::Builder::new()
            .name("bench-frames".to_string())
            .spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                flush_frame_stats();
            })
            .ok();
    });
}

static LAST_KEY: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();

fn last_key() -> &'static std::sync::Mutex<Option<Instant>> {
    LAST_KEY.get_or_init(|| std::sync::Mutex::new(None))
}

/// Mark a key (or equivalent command-submission) event. Real input-side hook;
/// call it wherever the app observes key input when `input_timing_enabled()`.
/// Also records for the dev perf overlay (same key→frame contract).
pub fn note_key_received() {
    let overlay = crate::perf_overlay::is_enabled();
    if !input_timing_enabled() && !overlay {
        return;
    }
    if overlay {
        crate::perf_overlay::note_key();
    }
    if !input_timing_enabled() {
        return;
    }
    if let Ok(mut slot) = last_key().lock() {
        *slot = Some(Instant::now());
    }
}

/// Mark the next presented frame after a key event and log the latency in the
/// format `scripts/bench/run.py input-latency` parses. Called after every
/// presented frame via the warpui hook installed by `install_frame_hook()`.
/// Forwards to the dev perf overlay (which emits the same `bench.input` line
/// so both harnesses share the parser) even when the bench vars are unset.
pub fn note_frame_presented() {
    // Overlay first: it gates on its own visibility (one atomic load when
    // off) and emits the shared `bench.input` line for key→frame latency.
    crate::perf_overlay::note_frame_tick();
    if !(input_timing_enabled() || frame_timing_enabled()) {
        return;
    }
    note_first_frame();
    record_frame_delta();
    if !input_timing_enabled() {
        return;
    }
    let started = last_key().lock().ok().and_then(|mut slot| slot.take());
    if let Some(started) = started {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        log::info!("[bench] bench.input key_to_frame_ms={:.3}", ms);
        eprintln!("[bench] bench.input key_to_frame_ms={:.3}", ms);
    }
}

static FIRST_FRAME_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// First presented frame: this is when the window is actually visible, so it
/// is the `ready` moment cold-start measures (see `note_window_open()`).
fn note_first_frame() {
    if FIRST_FRAME_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let ms = process_start().elapsed().as_secs_f64() * 1000.0;
    log::info!(
        "[bench] startup ready_ms={:.0} dev={} probe=false frame=first-presented",
        ms,
        dragent_paths::is_dev()
    );
    eprintln!(
        "[bench] startup ready_ms={:.0} dev={} probe=false frame=first-presented",
        ms,
        dragent_paths::is_dev()
    );
}

static LAST_FRAME_AT: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();
static FRAME_DELTAS_MS: OnceLock<std::sync::Mutex<Vec<f64>>> = OnceLock::new();

fn frame_state() -> (
    &'static std::sync::Mutex<Option<Instant>>,
    &'static std::sync::Mutex<Vec<f64>>,
) {
    (
        LAST_FRAME_AT.get_or_init(|| std::sync::Mutex::new(None)),
        FRAME_DELTAS_MS.get_or_init(|| std::sync::Mutex::new(Vec::new())),
    )
}

/// Record this frame's delta since the previous presented frame. Bounded: the
/// 1 s flusher drains the buffer, so a stalled flush can't grow it.
fn record_frame_delta() {
    let (last_at, deltas) = frame_state();
    let now = Instant::now();
    let previous = last_at.lock().ok().and_then(|mut slot| slot.replace(now));
    let Some(previous) = previous else {
        return;
    };
    let ms = now.duration_since(previous).as_secs_f64() * 1000.0;
    if let Ok(mut deltas) = deltas.lock() {
        if deltas.len() < 10_000 {
            deltas.push(ms);
        }
    }
}

/// Nearest-rank percentile over a sorted slice; NaN-free (deltas are finite).
fn percentile_sorted(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

/// Drain this second's frame deltas and log one aggregate line. Skips empty
/// seconds so idle logs stay small.
fn flush_frame_stats() {
    let (_, deltas) = frame_state();
    let mut samples = deltas
        .lock()
        .map(|mut d| std::mem::take(&mut *d))
        .unwrap_or_default();
    if samples.is_empty() {
        return;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let max = samples[samples.len() - 1];
    let line = format!(
        "[bench] bench.frame ms={:.3} p95_ms={:.3} max_ms={:.3} n={}",
        percentile_sorted(&samples, 0.50),
        percentile_sorted(&samples, 0.95),
        max,
        samples.len()
    );
    log::info!("{}", line);
    eprintln!("{}", line);
}

/// Log the process-launch→usable-window line the harness parses
/// (`bench.startup ready_ms=…`). Call once the first window exists.
pub fn note_window_open() {
    if !gated() {
        return;
    }
    let ms = process_start().elapsed().as_secs_f64() * 1000.0;
    log::info!(
        "[bench] startup ready_ms={:.0} dev={} probe=false",
        ms,
        dragent_paths::is_dev()
    );
}

/// `--bench-probe-startup`: print the startup line and exit before any UI or
/// service boots, so headless CI can time process init. Returns true when it
/// handled the process (caller must exit).
pub fn probe_startup_and_exit() -> bool {
    if !probe_requested() {
        return false;
    }
    let ms = process_start().elapsed().as_secs_f64() * 1000.0;
    // eprintln: logging is not initialised this early; the harness parses
    // stdout+stderr together.
    eprintln!(
        "[bench] startup ready_ms={:.0} dev={} probe=true",
        ms,
        dragent_paths::is_dev()
    );
    true
}

// ── own-process memory ────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub(crate) fn own_bytes() -> Option<u64> {
    // phys_footprint via proc_pid_rusage(RUSAGE_INFO_V2): what the OS kills
    // on, matching what memsample.py reads from outside.
    // SAFETY: zeroed V2 buffer passed for the V2 flavor; a read-only query
    // about our own PID. The buffer address itself is passed (the libc
    // `rusage_info_t` alias is already one pointer level): passing a
    // pointer-to-pointer leaves the struct zeroed and smashes the stack.
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
        );
        if rc == 0 && info.ri_phys_footprint > 0 {
            return Some(info.ri_phys_footprint);
        }
    }
    // Fallback: our own RSS via ps.
    self_rss_via_ps()
}

#[cfg(unix)]
fn self_rss_via_ps() -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "linux")]
pub(crate) fn own_bytes() -> Option<u64> {
    // Pss is the fair per-process share; VmRSS is the fallback.
    if let Ok(text) = std::fs::read_to_string("/proc/self/smaps_rollup") {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Pss:") {
                if let Some(kb) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    return Some(kb * 1024);
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string("/proc/self/status") {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                if let Some(kb) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    return Some(kb * 1024);
                }
            }
        }
    }
    None
}

#[cfg(windows)]
#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[cfg(windows)]
#[link(name = "psapi")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn GetProcessMemoryInfo(
        process: *mut std::ffi::c_void,
        counters: *mut ProcessMemoryCounters,
        cb: u32,
    ) -> i32;
}

#[cfg(windows)]
pub(crate) fn own_bytes() -> Option<u64> {
    // PrivateUsage-equivalent: pagefile_usage (committed private bytes).
    // SAFETY: read-only introspection of our own process.
    unsafe {
        let mut counters: ProcessMemoryCounters = std::mem::zeroed();
        counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let ok = GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
        if ok != 0 {
            return Some(counters.pagefile_usage as u64);
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(crate) fn own_bytes() -> Option<u64> {
    None
}

/// Best-effort sum of sibling dev-edition service processes (unix only;
/// failures yield 0 and the own number still logs). Never touches release
/// processes: only PIDs whose command line names this dev home or a
/// cargo-built `dragent` binary with a service flag are counted.
#[cfg(unix)]
pub(crate) fn sibling_dev_bytes() -> u64 {
    let home = dragent_paths::home();
    let home = home.to_string_lossy();
    let out = std::process::Command::new("ps")
        .args(["-ax", "-o", "pid=,rss=,command="])
        .output();
    let Ok(out) = out else { return 0 };
    let text = String::from_utf8_lossy(&out.stdout);
    let mine = std::process::id();
    let mut sum = 0u64;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(rss)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(pid), Ok(rss)) = (pid.parse::<u32>(), rss.parse::<u64>()) else {
            continue;
        };
        if pid == mine {
            continue;
        }
        let cmd: String = parts.collect::<Vec<_>>().join(" ");
        let is_service = cmd.contains("--dragent-host-service")
            || cmd.contains("--dragent-pty-service")
            || cmd.contains("--dragent-index-service")
            || cmd.contains("--dragent-computer-service");
        if !is_service {
            continue;
        }
        let dev = cmd.contains(home.as_ref())
            || cmd.contains("target/debug")
            || cmd.contains("target/release")
            || cmd.contains(".dragent-dev");
        if dev {
            sum += rss * 1024;
        }
    }
    sum
}

#[cfg(not(unix))]
pub(crate) fn sibling_dev_bytes() -> u64 {
    0
}

/// Spawn the 5 s memory logger when `--bench-mem` / `DRAGENT_BENCH_MEM=1` is
/// set (dev-gated). Logs to the app log and stderr, like the harness expects.
pub fn maybe_start_mem_logger() {
    if !mem_logging_requested() {
        return;
    }
    log::info!("[bench] mem logger on: every 5 s to log + stderr (--bench-mem)");
    eprintln!("[bench] mem logger on: every 5 s to log + stderr (--bench-mem)");
    std::thread::Builder::new()
        .name("bench-mem".to_string())
        .spawn(|| loop {
            let own = own_bytes().unwrap_or(0);
            let siblings = sibling_dev_bytes();
            let line = format!(
                "[bench] mem own_bytes={} siblings_bytes={} total_bytes={}",
                own,
                siblings,
                own + siblings
            );
            log::info!("{}", line);
            eprintln!("{}", line);
            std::thread::sleep(std::time::Duration::from_secs(5));
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::percentile_sorted;

    #[test]
    fn percentiles_use_nearest_rank() {
        let even = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile_sorted(&even, 0.50), 2.0);
        assert_eq!(percentile_sorted(&even, 0.95), 4.0);
        assert_eq!(percentile_sorted(&even, 0.0), 1.0);
        let odd = [10.0, 20.0, 30.0];
        assert_eq!(percentile_sorted(&odd, 0.50), 20.0);
        assert_eq!(percentile_sorted(&[], 0.50), 0.0);
    }

    /// Own-process memory must report a live number, not zero/None: the
    /// perf overlay and `--bench-mem` both read it. (A pointer-to-pointer
    /// passed as the buffer once left the struct zeroed on macOS.)
    #[cfg(target_os = "macos")]
    #[test]
    fn own_bytes_reports_live_footprint() {
        match super::own_bytes() {
            Some(bytes) => assert!(bytes > 0, "footprint should be positive"),
            None => panic!("own_bytes returned None on macOS"),
        }
    }
}
