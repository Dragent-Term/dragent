//! Dev-gated "frame presented" hook for benchmarks (`dragent/scripts/bench/`).
//!
//! The app crate registers its `bench::note_frame_presented` here at startup;
//! every platform backend reports through
//! [`WindowCallbackDispatcher::frame_drawn`](crate::windowing::WindowCallbackDispatcher::frame_drawn),
//! which calls [`note_frame_presented`] once the frame is on screen.
//!
//! Gated on `DRAGENT_BENCH_INPUT=1`, `DRAGENT_BENCH_FRAME=1`, or
//! `DRAGENT_PERF_OVERLAY=1` (read once per process), plus a runtime override
//! for the dev-only perf overlay toggle: without any of them, a presented
//! frame costs one cached-bool load plus one atomic load — no allocation, no
//! logging, no callback.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};

fn var_on(name: &str) -> bool {
    std::env::var(name).ok().as_deref() == Some("1")
}

static FORCED: AtomicBool = AtomicBool::new(false);

/// Arm the frame hook at runtime (dev-only perf overlay toggle). The env
/// vars above are read once; this flag covers toggles after startup.
pub fn force_enable() {
    FORCED.store(true, Ordering::Relaxed);
}

/// Whether any bench frame logging was requested (`DRAGENT_BENCH_INPUT=1` for
/// key→frame latency, `DRAGENT_BENCH_FRAME=1` for per-frame times,
/// `DRAGENT_PERF_OVERLAY=1` for the dev overlay). Env read once; the frame
/// path must stay allocation-free when benchmarking is off.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        var_on("DRAGENT_BENCH_INPUT")
            || var_on("DRAGENT_BENCH_FRAME")
            || var_on("DRAGENT_PERF_OVERLAY")
    }) || FORCED.load(Ordering::Relaxed)
}

type Callback = Box<dyn Fn() + Send + Sync + 'static>;

static CALLBACK: OnceLock<Mutex<Option<Callback>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Callback>> {
    CALLBACK.get_or_init(|| Mutex::new(None))
}

/// Register the function to run after every presented frame. Only the app
/// crate's bench module calls this, once at startup; later calls replace it.
pub fn set_frame_presented_callback(f: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut slot) = slot().lock() {
        *slot = Some(Box::new(f));
    }
}

/// Report one presented frame. Backends call this from `frame_drawn`; it is a
/// no-op (one cached bool load plus one atomic load) unless a `DRAGENT_BENCH_*`
/// var or `DRAGENT_PERF_OVERLAY=1` (or the runtime overlay toggle) is set.
pub fn note_frame_presented() {
    if !enabled() {
        return;
    }
    let guard = slot().lock().ok();
    if let Some(callback) = guard.as_ref().and_then(|slot| slot.as_ref()) {
        callback();
    }
}
