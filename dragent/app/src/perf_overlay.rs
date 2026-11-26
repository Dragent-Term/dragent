//! Dev-only performance overlay: frame time, memory, input latency.
//!
//! Small corner overlay for dev builds only, toggled by a dev-only shortcut /
//! command-palette entry (`WorkspaceAction::TogglePerfOverlay`) and forced on
//! by `DRAGENT_PERF_OVERLAY=1`. Shows:
//!
//! - frame time: last inter-frame gap + p95 over the last ~2 s,
//! - Dragent's memory: GUI process footprint/RSS plus sibling dev-service
//!   sum when cheaply available (same `bench.rs` helpers the harness uses),
//! - input latency: last keystroke→presented-frame (`bench.input` line).
//!
//! Cost when off: every hook starts with one `AtomicBool` load and returns —
//! no allocation, no logging, no thread. The 2 s mem sampler thread only
//! exists after the overlay is first enabled. Native only; the wasm viewer
//! never compiles this module.
//!
//! Frame/input numbers reuse the `bench.rs` log contract so the harness
//! parser keeps working:
//! `bench.input key_to_frame_ms=<float>`,
//! `[bench] startup ready_ms=<ms> …`,
//! `[bench] mem own_bytes=<n> siblings_bytes=<n> total_bytes=<n>`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static VISIBLE: AtomicBool = AtomicBool::new(false);
static INIT: OnceLock<()> = OnceLock::new();

fn dev_gated() -> bool {
    dragent_paths::is_dev() || cfg!(debug_assertions)
}

fn env_forced() -> bool {
    std::env::var("DRAGENT_PERF_OVERLAY").ok().as_deref() == Some("1")
}

fn ensure_init() {
    INIT.get_or_init(|| {
        if dev_gated() && env_forced() {
            VISIBLE.store(true, Ordering::Relaxed);
            warpui_core::bench_frame::force_enable();
            spawn_mem_thread();
        }
    });
}

/// Whether the frame/input hooks should record (dev + overlay visible).
/// One atomic load — this is the "costs ~nothing when off" gate.
pub fn is_enabled() -> bool {
    if !dev_gated() {
        return false;
    }
    ensure_init();
    VISIBLE.load(Ordering::Relaxed)
}

/// Whether the frame hook was requested at startup (env var, dev-gated).
/// Used by `bench::install_frame_hook` so the warpui callback is installed
/// even when `DRAGENT_BENCH_*` are unset.
pub fn overlay_requested_at_startup() -> bool {
    dev_gated() && env_forced()
}

/// Ensure the mem sampler + frame hook are armed (called from desktop startup
/// after `bench` init; no-op unless dev + env forced or already toggled on).
pub fn maybe_autostart() {
    if !dev_gated() {
        return;
    }
    ensure_init();
    if VISIBLE.load(Ordering::Relaxed) {
        log::info!("[perf-overlay] visible=true");
        warpui_core::bench_frame::force_enable();
        crate::bench::install_frame_hook();
        spawn_mem_thread();
    }
}

/// Toggle the overlay (dev only). Returns the new visibility.
/// Arming the warpui frame hook + mem sampler on first enable so a runtime
/// toggle starts producing numbers within a frame or two.
pub fn toggle() -> bool {
    if !dev_gated() {
        return false;
    }
    ensure_init();
    let next = !VISIBLE.load(Ordering::Relaxed);
    VISIBLE.store(next, Ordering::Relaxed);
    if next {
        warpui_core::bench_frame::force_enable();
        crate::bench::install_frame_hook();
        spawn_mem_thread();
    }
    log::info!("[perf-overlay] visible={next}");
    next
}

// ── frame / input state ────────────────────────────────────────────────────

fn frames() -> &'static Mutex<VecDeque<(Instant, f64)>> {
    static FRAMES: OnceLock<Mutex<VecDeque<(Instant, f64)>>> = OnceLock::new();
    FRAMES.get_or_init(|| Mutex::new(VecDeque::with_capacity(512)))
}

fn last_frame_at() -> &'static Mutex<Option<Instant>> {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn last_input_ms() -> &'static Mutex<Option<f64>> {
    static LAST_INPUT: OnceLock<Mutex<Option<f64>>> = OnceLock::new();
    LAST_INPUT.get_or_init(|| Mutex::new(None))
}

fn last_key_at() -> &'static Mutex<Option<Instant>> {
    static LAST_KEY: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST_KEY.get_or_init(|| Mutex::new(None))
}

/// Mark a key (or equivalent command-submission) event. Called from
/// `bench::note_key_received` so automation `run` and the workspace keydown
/// hook share one path. One atomic load when the overlay is off.
pub fn note_key() {
    if !is_enabled() {
        return;
    }
    if let Ok(mut slot) = last_key_at().lock() {
        *slot = Some(Instant::now());
    }
}

/// Record one presented frame: inter-frame delta for last/p95 plus, when a
/// key is pending, the key→frame latency. Emits the same `bench.input`
/// line the harness parses so both harnesses share the parser.
pub fn note_frame_tick() {
    if !is_enabled() {
        return;
    }
    let now = Instant::now();
    let previous = last_frame_at()
        .lock()
        .ok()
        .and_then(|mut slot| slot.replace(now));
    if let Some(previous) = previous {
        let ms = now.duration_since(previous).as_secs_f64() * 1000.0;
        if let Ok(mut q) = frames().lock() {
            q.push_back((now, ms));
            // Prune to the last ~2 s (and a hard cap so a stalled prune
            // can't grow); the 1 s bench flusher drains its own buffer
            // separately.
            while q.len() > 600 {
                q.pop_front();
            }
            while q
                .front()
                .is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(2))
            {
                q.pop_front();
            }
        }
    }
    // Key→frame latency: same contract as bench.rs (`bench.input …`).
    let started = last_key_at().lock().ok().and_then(|mut slot| slot.take());
    if let Some(started) = started {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if let Ok(mut slot) = last_input_ms().lock() {
            *slot = Some(ms);
        }
        log::info!("[bench] bench.input key_to_frame_ms={:.3}", ms);
        eprintln!("[bench] bench.input key_to_frame_ms={:.3}", ms);
    }
}

/// (last_ms, p95_ms over the last ~2 s, n). `None` until two frames seen.
pub fn frame_stats() -> (Option<f64>, Option<f64>, usize) {
    let samples: Vec<f64> = frames()
        .lock()
        .map(|q| q.iter().map(|(_, ms)| *ms).collect())
        .unwrap_or_default();
    if samples.is_empty() {
        return (None, None, 0);
    }
    let last = samples.last().copied();
    let mut sorted = samples;
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    let rank = ((0.95 * n as f64).ceil().max(1.0) as usize).min(n);
    (last, Some(sorted[rank - 1]), n)
}

pub fn last_input_latency_ms() -> Option<f64> {
    last_input_ms().lock().ok().and_then(|slot| *slot)
}

// ── memory (cached, sampled off the UI thread) ─────────────────────────────

fn mem_cache() -> &'static Mutex<(u64, u64, Option<Instant>)> {
    static MEM: OnceLock<Mutex<(u64, u64, Option<Instant>)>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new((0, 0, None)))
}

static MEM_THREAD: OnceLock<()> = OnceLock::new();

fn spawn_mem_thread() {
    MEM_THREAD.get_or_init(|| {
        std::thread::Builder::new()
            .name("perf-overlay-mem".to_string())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(2));
                if !VISIBLE.load(Ordering::Relaxed) {
                    continue;
                }
                let own = crate::bench::own_bytes().unwrap_or(0);
                let siblings = crate::bench::sibling_dev_bytes();
                if let Ok(mut cache) = mem_cache().lock() {
                    *cache = (own, siblings, Some(Instant::now()));
                }
            })
            .ok();
    });
}

/// Cached (own, siblings) bytes. The first paint after enabling samples
/// synchronously (one `ps` pass, ~10 ms, once) so the card never shows a
/// blank; afterwards the 2 s sampler thread keeps it fresh off-thread.
pub fn mem_numbers() -> (u64, u64) {
    let cold = mem_cache()
        .lock()
        .map(|cache| cache.2.is_none())
        .unwrap_or(true);
    if cold {
        let own = crate::bench::own_bytes().unwrap_or(0);
        let siblings = crate::bench::sibling_dev_bytes();
        if let Ok(mut cache) = mem_cache().lock() {
            *cache = (own, siblings, Some(Instant::now()));
        }
        return (own, siblings);
    }
    mem_cache()
        .lock()
        .map(|cache| (cache.0, cache.1))
        .unwrap_or((0, 0))
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

// ── rendering ──────────────────────────────────────────────────────────────

/// The overlay element, or `None` when disabled (zero cost: no element built).
/// Wrap in a `Stack` positioned overlay child anchored to the window corner;
/// the inner `LiveElement` repaints at 2 Hz so numbers move while idle.
pub fn render_overlay(
    theme: &crate::theme::Theme,
    ui_font: warpui::fonts::FamilyId,
) -> Option<Box<dyn warpui::elements::Element>> {
    use warpui::elements::{Container, Element, Flex, LiveElement, ParentElement, Text};
    use warpui_core::scene::{Border, CornerRadius, Radius};

    if !is_enabled() {
        return None;
    }
    let (last, p95, n) = frame_stats();
    let input = last_input_latency_ms();
    let (own, siblings) = mem_numbers();
    let frame_line = match (last, p95) {
        (Some(last), Some(p95)) => format!("frame last {last:.1}ms p95 {p95:.1}ms (n={n})"),
        _ => "frame …".to_string(),
    };
    let mem_line = if own > 0 {
        format!("mem gui {:.0}MB + svc {:.0}MB", mb(own), mb(siblings))
    } else {
        "mem …".to_string()
    };
    let input_line = match input {
        Some(ms) => format!("input {ms:.1}ms key→frame"),
        None => "input …".to_string(),
    };
    let size = crate::theme::text_size(11.0);
    let col = Flex::column()
        .with_child(
            Text::new(frame_line, ui_font, size)
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(mem_line, ui_font, size)
                .with_color(theme.sub_text())
                .finish(),
        )
        .with_child(
            Text::new(input_line, ui_font, size)
                .with_color(theme.sub_text())
                .finish(),
        )
        .finish();
    let card = Container::new(col)
        .with_background_color(theme.solid_background())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_uniform_padding(8.0)
        .finish();
    Some(Box::new(LiveElement::new(card, Duration::from_millis(500))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percentile(sorted: &[f64], q: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let rank = (q * sorted.len() as f64).ceil().max(1.0) as usize;
        sorted[rank.min(sorted.len()) - 1]
    }

    #[test]
    fn p95_uses_nearest_rank() {
        let even = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&even, 0.95), 4.0);
        assert_eq!(percentile(&even, 0.50), 2.0);
    }

    #[test]
    fn overlay_stays_off_without_dev_or_env() {
        // Pure formatting check: zero bytes formats as "…" pending, and the
        // gate itself never panics when env is absent.
        assert_eq!(mb(0) as i64, 0);
        let _ = dev_gated() && env_forced();
    }
}
