//! Low-overhead process and rendering measurements used by benchmarks and
//! diagnostics. Recording is atomic and allocation-free on frame paths.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static STARTED_AT: OnceLock<Instant> = OnceLock::new();
static FIRST_FRAME_US: AtomicU64 = AtomicU64::new(0);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static REDRAW_REQUESTS: AtomicU64 = AtomicU64::new(0);
static SCENE_REBUILDS: AtomicU64 = AtomicU64::new(0);
static TOTAL_FRAME_US: AtomicU64 = AtomicU64::new(0);
static MAX_FRAME_US: AtomicU64 = AtomicU64::new(0);
static LAST_FRAME_AT_US: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderSnapshot {
    pub uptime_ms: u64,
    pub first_frame_ms: Option<f64>,
    pub frames: u64,
    pub redraw_requests: u64,
    pub scene_rebuilds: u64,
    pub average_frame_ms: f64,
    pub max_frame_ms: f64,
    pub last_frame_age_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcessMemory {
    pub current_rss_mb: f64,
    pub peak_rss_mb: f64,
}

pub fn init() {
    STARTED_AT.get_or_init(Instant::now);
}

pub fn record_redraw_request() {
    init();
    REDRAW_REQUESTS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_frame(duration: Duration, scene_rebuilt: bool) {
    let started = *STARTED_AT.get_or_init(Instant::now);
    let micros = duration.as_micros().min(u64::MAX as u128) as u64;
    FRAMES.fetch_add(1, Ordering::Relaxed);
    TOTAL_FRAME_US.fetch_add(micros, Ordering::Relaxed);
    MAX_FRAME_US.fetch_max(micros, Ordering::Relaxed);
    if scene_rebuilt {
        SCENE_REBUILDS.fetch_add(1, Ordering::Relaxed);
    }
    let first = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
    LAST_FRAME_AT_US.store(first.max(1), Ordering::Relaxed);
    let _ = FIRST_FRAME_US.compare_exchange(0, first.max(1), Ordering::Relaxed, Ordering::Relaxed);
}

pub fn render_snapshot() -> RenderSnapshot {
    let uptime = STARTED_AT.get().map(Instant::elapsed).unwrap_or_default();
    let frames = FRAMES.load(Ordering::Relaxed);
    let total = TOTAL_FRAME_US.load(Ordering::Relaxed);
    let first = FIRST_FRAME_US.load(Ordering::Relaxed);
    let last_frame = LAST_FRAME_AT_US.load(Ordering::Relaxed);
    let uptime_us = uptime.as_micros().min(u64::MAX as u128) as u64;
    RenderSnapshot {
        uptime_ms: uptime.as_millis().min(u64::MAX as u128) as u64,
        first_frame_ms: (first > 0).then_some(first as f64 / 1000.0),
        frames,
        redraw_requests: REDRAW_REQUESTS.load(Ordering::Relaxed),
        scene_rebuilds: SCENE_REBUILDS.load(Ordering::Relaxed),
        average_frame_ms: if frames == 0 {
            0.0
        } else {
            total as f64 / frames as f64 / 1000.0
        },
        max_frame_ms: MAX_FRAME_US.load(Ordering::Relaxed) as f64 / 1000.0,
        last_frame_age_ms: (last_frame > 0)
            .then_some(uptime_us.saturating_sub(last_frame) as f64 / 1000.0),
    }
}

pub fn process_memory() -> ProcessMemory {
    ProcessMemory {
        current_rss_mb: current_rss_mb(),
        peak_rss_mb: peak_rss_mb(),
    }
}

#[cfg(target_os = "macos")]
fn current_rss_mb() -> f64 {
    let mut info: libc::mach_task_basic_info = unsafe { std::mem::zeroed() };
    let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
        / std::mem::size_of::<libc::natural_t>())
        as libc::mach_msg_type_number_t;
    let result = unsafe {
        #[allow(deprecated)]
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as libc::task_info_t,
            &mut count,
        )
    };
    if result == libc::KERN_SUCCESS {
        info.resident_size as f64 / 1_048_576.0
    } else {
        0.0
    }
}

#[cfg(target_os = "linux")]
fn current_rss_mb() -> f64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0.0;
    };
    let Some(pages) = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return 0.0;
    };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
    pages.saturating_mul(page_size) as f64 / 1_048_576.0
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn current_rss_mb() -> f64 {
    0.0
}

#[cfg(unix)]
fn peak_rss_mb() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0.0;
    }
    #[cfg(target_os = "macos")]
    let bytes = usage.ru_maxrss as f64;
    #[cfg(not(target_os = "macos"))]
    let bytes = usage.ru_maxrss as f64 * 1024.0;
    bytes / 1_048_576.0
}

#[cfg(not(unix))]
fn peak_rss_mb() -> f64 {
    current_rss_mb()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_frames_and_maximum() {
        let before = render_snapshot();
        record_frame(Duration::from_millis(2), true);
        record_frame(Duration::from_millis(5), false);
        let after = render_snapshot();
        assert_eq!(after.frames, before.frames + 2);
        assert!(after.max_frame_ms >= 5.0);
        assert!(after.first_frame_ms.is_some());
    }
}
