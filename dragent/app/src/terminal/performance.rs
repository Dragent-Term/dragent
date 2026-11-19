//! Allocation-free aggregate timings for terminal hot paths.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
pub(crate) struct Timing {
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
}
impl Timing {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
        }
    }
    pub(crate) fn record(&self, elapsed: Duration) {
        let us = elapsed.as_micros().min(u64::MAX as u128) as u64;
        self.total_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }
    fn snapshot(&self) -> serde_json::Value {
        let count = self.count.load(Ordering::Relaxed);
        serde_json::json!({ "count": count, "average_ms": self.total_us.load(Ordering::Relaxed) as f64 / count.max(1) as f64 / 1000.0, "max_ms": self.max_us.load(Ordering::Relaxed) as f64 / 1000.0 })
    }
}
pub(crate) static FEED_LOCK_WAIT: Timing = Timing::new();
pub(crate) static HOST_FEED: Timing = Timing::new();
pub(crate) static HOST_INPUT_QUEUE: Timing = Timing::new();
pub(crate) static HOST_INPUT_RPC: Timing = Timing::new();
pub(crate) static RESIZE_APPLY: Timing = Timing::new();
pub(crate) fn snapshot() -> serde_json::Value {
    serde_json::json!({ "host_input_queue": HOST_INPUT_QUEUE.snapshot(), "host_input_rpc": HOST_INPUT_RPC.snapshot(), "host_feed_lock_wait": FEED_LOCK_WAIT.snapshot(), "host_feed": HOST_FEED.snapshot(), "resize_apply": RESIZE_APPLY.snapshot() })
}
