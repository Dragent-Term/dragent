//! Live output delivery to terminal subscribers, with flow control.
//!
//! Every live session owns one [`Fanout`]. Each subscriber (a streaming
//! client connection) gets its own outbound queue; the PTY feed publishes
//! each chunk into every queue and a per-connection writer drains it to the
//! socket.
//!
//! Flow control follows the output, not the reader: when a subscriber's
//! queue grows past [`OUTBOUND_PAUSE_BYTES`] (its writer is blocked on a slow
//! peer), the session stops reading its PTY, so the program blocks on its
//! next write instead of output piling up or being dropped. Reading resumes
//! once every congested queue has fully drained. A subscriber that stays
//! congested for [`STUCK_SUBSCRIBER_TIMEOUT`] is dropped, which resumes the
//! session for everyone else.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Queued bytes per subscriber that pause the session's PTY reads.
pub const OUTBOUND_PAUSE_BYTES: usize = 1024 * 1024;
/// How long a subscriber may stay congested before it is dropped.
pub const STUCK_SUBSCRIBER_TIMEOUT: Duration = Duration::from_secs(30);
/// Test hook overriding [`STUCK_SUBSCRIBER_TIMEOUT`] in milliseconds.
pub const STUCK_TIMEOUT_ENV: &str = "DRAGENT_STUCK_SUBSCRIBER_TIMEOUT_MS";

fn stuck_timeout() -> Duration {
    std::env::var(STUCK_TIMEOUT_ENV)
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(STUCK_SUBSCRIBER_TIMEOUT)
}

/// One item for a subscriber's writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Output starting at absolute offset `base`.
    Data {
        base: u64,
        bytes: Vec<u8>,
    },
    Operations(crate::screen_protocol::OperationFrame),
    /// The shell exited; nothing follows.
    Exit(i32),
    /// The session was closed, or this subscriber was dropped.
    Closed,
}

impl Delivery {
    fn memory_size(&self) -> usize {
        match self {
            Self::Data { bytes, .. } => bytes.len(),
            Self::Operations(frame) => frame.memory_size(),
            _ => 0,
        }
    }
}

#[derive(Default)]
struct Outbox {
    queue: VecDeque<Delivery>,
    queued_bytes: usize,
    /// When the queue last crossed the pause threshold (cleared on drain).
    congested_since: Option<Instant>,
    ended: bool,
}

pub struct Subscriber {
    id: u64,
    operations: bool,
    outbox: Mutex<Outbox>,
    ready: Condvar,
}

impl Subscriber {
    fn push(&self, delivery: Delivery) -> bool {
        let mut outbox = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        if outbox.ended {
            return false;
        }
        outbox.queued_bytes += delivery.memory_size();
        if !matches!(delivery, Delivery::Data { .. } | Delivery::Operations(_)) {
            outbox.ended = true;
        }
        outbox.queue.push_back(delivery);
        let newly_congested =
            outbox.queued_bytes > OUTBOUND_PAUSE_BYTES && outbox.congested_since.is_none();
        if newly_congested {
            outbox.congested_since = Some(Instant::now());
        }
        self.ready.notify_all();
        newly_congested
    }

    /// Next delivery, or `None` when nothing arrived within `timeout` (the
    /// writer uses that to send a keepalive).
    pub fn next(&self, timeout: Duration) -> Option<Delivery> {
        let outbox = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let (mut outbox, _) = self
            .ready
            .wait_timeout_while(outbox, timeout, |o| o.queue.is_empty())
            .unwrap_or_else(|p| p.into_inner());
        let delivery = outbox.queue.pop_front()?;
        outbox.queued_bytes = outbox.queued_bytes.saturating_sub(delivery.memory_size());
        Some(delivery)
    }

    /// Everything queued has been handed to the writer.
    fn is_drained(&self) -> bool {
        self.outbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .queue
            .is_empty()
    }

    fn congested_for(&self) -> Option<Duration> {
        self.outbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .congested_since
            .map(|at| at.elapsed())
    }

    fn clear_congestion(&self) {
        self.outbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .congested_since = None;
    }
}

/// Pause/resume hook onto the session's PTY reader.
pub type PauseHook = Box<dyn Fn(bool) + Send + Sync>;

#[derive(Default)]
pub struct Fanout {
    subscribers: Mutex<Vec<Arc<Subscriber>>>,
    next_id: AtomicU64,
    paused: AtomicBool,
    /// Set once the PTY exists (the feed runs before the handle does).
    pause_hook: OnceLock<PauseHook>,
    /// Final delivery once the session ended, for late subscribers.
    finished: Mutex<Option<Delivery>>,
}

impl Fanout {
    pub fn set_pause_hook(&self, hook: PauseHook) {
        let _ = self.pause_hook.set(hook);
    }

    fn set_paused(&self, paused: bool) {
        if self.paused.swap(paused, Ordering::SeqCst) != paused {
            if let Some(hook) = self.pause_hook.get() {
                hook(paused);
            }
        }
    }

    /// Re-send the current pause state to the PTY (after something else,
    /// such as an abandoned handoff, changed the reader's pause directly).
    pub fn reapply_pause(&self) {
        if self.paused.load(Ordering::SeqCst) {
            if let Some(hook) = self.pause_hook.get() {
                hook(true);
            }
        }
    }

    /// Register a subscriber whose queue starts with `catch_up` (replayed
    /// output). Call while holding the replay-ring lock that `publish` also
    /// runs under, so replay and live output can neither overlap nor gap.
    pub fn subscribe(&self, catch_up: Option<Delivery>) -> Arc<Subscriber> {
        self.subscribe_frames(catch_up.into_iter().collect(), false)
    }

    pub fn subscribe_frames(&self, catch_up: Vec<Delivery>, operations: bool) -> Arc<Subscriber> {
        let subscriber = Arc::new(Subscriber {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            operations,
            outbox: Mutex::new(Outbox::default()),
            ready: Condvar::new(),
        });
        for delivery in catch_up {
            subscriber.push(delivery);
        }
        let finished = self
            .finished
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        match finished {
            Some(end) => {
                subscriber.push(end);
            }
            None => self
                .subscribers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(subscriber.clone()),
        }
        subscriber
    }

    pub fn unsubscribe(self: &Arc<Self>, subscriber: &Subscriber) {
        self.subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|s| s.id != subscriber.id);
        self.resume_if_clear();
    }

    /// Fan one chunk out to every subscriber (run under the ring lock).
    pub fn publish(self: &Arc<Self>, base: u64, bytes: &[u8]) {
        self.publish_inner(base, bytes, None);
    }

    pub fn publish_operations(self: &Arc<Self>, frame: &crate::screen_protocol::OperationFrame) {
        self.publish_inner(frame.base, &frame.data, Some(frame));
    }

    fn publish_inner(
        self: &Arc<Self>,
        base: u64,
        bytes: &[u8],
        frame: Option<&crate::screen_protocol::OperationFrame>,
    ) {
        if bytes.is_empty() {
            return;
        }
        let mut congested = false;
        for subscriber in self
            .subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            let delivery = if subscriber.operations {
                let Some(frame) = frame else { continue };
                Delivery::Operations(frame.clone())
            } else {
                Delivery::Data {
                    base,
                    bytes: bytes.to_vec(),
                }
            };
            congested |= subscriber.push(delivery);
        }
        if congested && !self.paused.load(Ordering::SeqCst) {
            self.set_paused(true);
            self.watch_stuck();
        }
    }

    /// The writer drained a subscriber's queue: that subscriber is no longer
    /// holding the session paused.
    pub fn on_drained(self: &Arc<Self>, subscriber: &Subscriber) {
        if subscriber.is_drained() {
            subscriber.clear_congestion();
            self.resume_if_clear();
        }
    }

    fn resume_if_clear(&self) {
        if !self.paused.load(Ordering::SeqCst) {
            return;
        }
        let any_congested = self
            .subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|s| s.congested_for().is_some());
        if !any_congested {
            self.set_paused(false);
        }
    }

    /// While paused, drop subscribers congested past the timeout.
    fn watch_stuck(self: &Arc<Self>) {
        let fanout = Arc::downgrade(self);
        let timeout = stuck_timeout();
        std::thread::spawn(move || loop {
            std::thread::sleep(
                (timeout / 4).clamp(Duration::from_millis(20), Duration::from_secs(1)),
            );
            let Some(fanout) = fanout.upgrade() else {
                return;
            };
            if !fanout.paused.load(Ordering::SeqCst) {
                return;
            }
            let stuck: Vec<Arc<Subscriber>> = fanout
                .subscribers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .filter(|s| s.congested_for().is_some_and(|d| d >= timeout))
                .cloned()
                .collect();
            for subscriber in stuck {
                log::warn!(
                    "terminal subscriber {} stayed congested for {timeout:?}; dropping it",
                    subscriber.id
                );
                {
                    let mut outbox = subscriber.outbox.lock().unwrap_or_else(|p| p.into_inner());
                    outbox.queue.clear();
                    outbox.queued_bytes = 0;
                    outbox.congested_since = None;
                }
                subscriber.push(Delivery::Closed);
                fanout.unsubscribe(&subscriber);
            }
        });
    }

    /// The session ended: deliver `end` (exit or close) to every subscriber
    /// and to anyone subscribing later.
    pub fn finish(&self, end: Delivery) {
        *self.finished.lock().unwrap_or_else(|p| p.into_inner()) = Some(end.clone());
        let subscribers =
            std::mem::take(&mut *self.subscribers.lock().unwrap_or_else(|p| p.into_inner()));
        for subscriber in subscribers {
            subscriber.push(end.clone());
        }
        self.paused.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording_fanout() -> (Arc<Fanout>, Arc<Mutex<Vec<bool>>>) {
        let fanout = Arc::new(Fanout::default());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log = calls.clone();
        fanout.set_pause_hook(Box::new(move |paused| log.lock().unwrap().push(paused)));
        (fanout, calls)
    }

    #[test]
    fn catch_up_then_live_output_in_order() {
        let (fanout, _) = recording_fanout();
        let sub = fanout.subscribe(Some(Delivery::Data {
            base: 0,
            bytes: b"old".to_vec(),
        }));
        fanout.publish(3, b"new");
        assert_eq!(
            sub.next(Duration::ZERO),
            Some(Delivery::Data {
                base: 0,
                bytes: b"old".to_vec()
            })
        );
        assert_eq!(
            sub.next(Duration::ZERO),
            Some(Delivery::Data {
                base: 3,
                bytes: b"new".to_vec()
            })
        );
        assert_eq!(sub.next(Duration::from_millis(5)), None);
    }

    #[test]
    fn congestion_pauses_and_draining_resumes() {
        let (fanout, calls) = recording_fanout();
        let sub = fanout.subscribe(None);
        let chunk = vec![b'x'; OUTBOUND_PAUSE_BYTES / 2 + 1];
        fanout.publish(0, &chunk);
        assert!(
            calls.lock().unwrap().is_empty(),
            "one chunk is under the threshold"
        );
        fanout.publish(chunk.len() as u64, &chunk);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![true],
            "second chunk crosses it"
        );
        // Output keeps queueing while paused, but never pauses twice.
        fanout.publish(0, b"more");
        assert_eq!(*calls.lock().unwrap(), vec![true]);
        while sub.next(Duration::ZERO).is_some() {}
        fanout.on_drained(&sub);
        assert_eq!(*calls.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn stuck_subscriber_is_dropped_and_the_session_resumes() {
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var(STUCK_TIMEOUT_ENV, "100") };
        let (fanout, calls) = recording_fanout();
        let stuck = fanout.subscribe(None);
        let healthy = fanout.subscribe(None);
        let chunk = vec![b'x'; OUTBOUND_PAUSE_BYTES + 1];
        fanout.publish(0, &chunk);
        // The healthy subscriber keeps up.
        while healthy.next(Duration::ZERO).is_some() {}
        fanout.on_drained(&healthy);
        let deadline = Instant::now() + Duration::from_secs(5);
        while calls.lock().unwrap().len() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe { std::env::remove_var(STUCK_TIMEOUT_ENV) };
        assert_eq!(*calls.lock().unwrap(), vec![true, false]);
        assert_eq!(stuck.next(Duration::ZERO), Some(Delivery::Closed));
        // The healthy one still receives output.
        fanout.publish(9, b"after");
        assert!(matches!(
            healthy.next(Duration::ZERO),
            Some(Delivery::Data { .. })
        ));
    }

    #[test]
    fn finish_reaches_current_and_late_subscribers() {
        let (fanout, _) = recording_fanout();
        let early = fanout.subscribe(None);
        fanout.finish(Delivery::Exit(4));
        assert_eq!(early.next(Duration::ZERO), Some(Delivery::Exit(4)));
        let late = fanout.subscribe(None);
        assert_eq!(late.next(Duration::ZERO), Some(Delivery::Exit(4)));
        // Nothing is delivered after the end.
        fanout.publish(0, b"ignored");
        assert_eq!(early.next(Duration::from_millis(5)), None);
    }

    #[test]
    fn unsubscribing_the_congested_reader_resumes() {
        let (fanout, calls) = recording_fanout();
        let sub = fanout.subscribe(None);
        fanout.publish(0, &vec![b'x'; OUTBOUND_PAUSE_BYTES + 1]);
        assert_eq!(*calls.lock().unwrap(), vec![true]);
        fanout.unsubscribe(&sub);
        assert_eq!(*calls.lock().unwrap(), vec![true, false]);
    }
}
