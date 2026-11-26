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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
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

    /// Everything queued was handed to the writer, after having been
    /// congested.
    fn drained_while_congested(&self) -> bool {
        let outbox = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        outbox.queue.is_empty() && outbox.congested_since.is_some()
    }

    /// Give up on delivering what is queued: end with `Closed` at once.
    fn drop_backlog(&self) {
        let mut outbox = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        // A session that ended meanwhile already queued its final delivery;
        // losing it would leave the reader waiting for ever.
        let last = if outbox.ended {
            outbox.queue.pop_back()
        } else {
            Some(Delivery::Closed)
        };
        outbox.queue.clear();
        outbox.queue.extend(last);
        outbox.queued_bytes = 0;
        outbox.congested_since = None;
        outbox.ended = true;
        self.ready.notify_all();
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

/// Pause/resume hook onto the session's PTY reader. Called under the
/// fan-out's lock so calls arrive in the order the state changed; it must not
/// block or call back into the fan-out.
pub type PauseHook = Box<dyn Fn(bool) + Send + Sync>;

/// Everything that decides whether the PTY is read, under one lock. Deciding
/// and applying in separate steps loses a wake-up: a reader that drains
/// between "queue is congested" and "paused" would find nothing to resume,
/// and the program would then wait for ever on a reader that kept up.
#[derive(Default)]
struct State {
    subscribers: Vec<Arc<Subscriber>>,
    /// What the hook was last told.
    paused: bool,
    /// Reading is stopped for an upgrade handoff, whatever subscribers do.
    held: bool,
    /// A thread is watching for stuck subscribers.
    watching: bool,
    /// Set once the PTY exists (the feed runs before the handle does).
    pause_hook: Option<PauseHook>,
    /// Final delivery once the session ended, for late subscribers.
    finished: Option<Delivery>,
}

impl State {
    fn congested(&self) -> bool {
        self.subscribers.iter().any(|s| s.congested_for().is_some())
    }

    fn call_hook(&self) {
        if let Some(hook) = &self.pause_hook {
            hook(self.paused);
        }
    }

    /// Bring the PTY's pause in line with the subscribers.
    fn update(&mut self) {
        let paused = self.held || self.congested();
        if paused != self.paused {
            self.paused = paused;
            self.call_hook();
        }
    }
}

#[derive(Default)]
pub struct Fanout {
    state: Mutex<State>,
    next_id: AtomicU64,
}

impl Fanout {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Connect the PTY. Output published before this may already have
    /// congested a subscriber, so the current state is applied at once.
    pub fn set_pause_hook(&self, hook: PauseHook) {
        let mut state = self.state();
        if state.pause_hook.is_none() {
            state.pause_hook = Some(hook);
            if state.paused {
                state.call_hook();
            }
        }
    }

    /// Stop (or allow again) reading the PTY for an upgrade handoff. While
    /// held, a subscriber that drains must not resume reading: output read
    /// after the handoff snapshot would reach neither service's successor
    /// state. Always re-sends the state to the PTY.
    pub fn hold(&self, held: bool) {
        let mut state = self.state();
        state.held = held;
        state.paused = held || state.congested();
        state.call_hook();
    }

    /// Register a subscriber whose queue starts with `catch_up` (replayed
    /// output). Call while holding the replay-ring lock that `publish` also
    /// runs under, so replay and live output can neither overlap nor gap.
    pub fn subscribe(self: &Arc<Self>, catch_up: Option<Delivery>) -> Arc<Subscriber> {
        self.subscribe_frames(catch_up.into_iter().collect(), false)
    }

    pub fn subscribe_frames(
        self: &Arc<Self>,
        catch_up: Vec<Delivery>,
        operations: bool,
    ) -> Arc<Subscriber> {
        let subscriber = Arc::new(Subscriber {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            operations,
            outbox: Mutex::new(Outbox::default()),
            ready: Condvar::new(),
        });
        let mut state = self.state();
        for delivery in catch_up {
            subscriber.push(delivery);
        }
        match state.finished.clone() {
            Some(end) => {
                subscriber.push(end);
            }
            None => {
                state.subscribers.push(subscriber.clone());
                // A large replay alone can congest it.
                state.update();
                self.watch_stuck(&mut state);
            }
        }
        subscriber
    }

    pub fn unsubscribe(&self, subscriber: &Subscriber) {
        let mut state = self.state();
        state.subscribers.retain(|s| s.id != subscriber.id);
        state.update();
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
        let mut state = self.state();
        let mut congested = false;
        for subscriber in &state.subscribers {
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
        if congested {
            state.update();
            self.watch_stuck(&mut state);
        }
    }

    /// The writer handed a delivery to the socket: once its queue is empty,
    /// that subscriber is no longer holding the session paused.
    pub fn on_drained(&self, subscriber: &Subscriber) {
        // The common case takes only the subscriber's own lock. Output queued
        // after this look brings the writer back here.
        if !subscriber.drained_while_congested() {
            return;
        }
        let mut state = self.state();
        // Deliveries are only queued under the state lock, so nothing can
        // congest it again between this check and the update.
        if subscriber.drained_while_congested() {
            subscriber.clear_congestion();
            state.update();
        }
    }

    /// While anyone is congested, drop whoever stays so past the timeout.
    fn watch_stuck(self: &Arc<Self>, state: &mut State) {
        if state.watching || !state.congested() {
            return;
        }
        state.watching = true;
        let fanout = Arc::downgrade(self);
        let timeout = stuck_timeout();
        std::thread::spawn(move || loop {
            std::thread::sleep(
                (timeout / 4).clamp(Duration::from_millis(20), Duration::from_secs(1)),
            );
            let Some(fanout) = fanout.upgrade() else {
                return;
            };
            let mut state = fanout.state();
            let stuck: Vec<Arc<Subscriber>> = state
                .subscribers
                .iter()
                .filter(|s| s.congested_for().is_some_and(|d| d >= timeout))
                .cloned()
                .collect();
            for subscriber in stuck {
                log::warn!(
                    "terminal subscriber {} stayed congested for {timeout:?}; dropping it",
                    subscriber.id
                );
                subscriber.drop_backlog();
                state.subscribers.retain(|s| s.id != subscriber.id);
            }
            state.update();
            if !state.congested() {
                state.watching = false;
                return;
            }
        });
    }

    /// The session ended: deliver `end` (exit or close) to every subscriber
    /// and to anyone subscribing later.
    pub fn finish(&self, end: Delivery) {
        let mut state = self.state();
        state.finished = Some(end.clone());
        for subscriber in std::mem::take(&mut state.subscribers) {
            subscriber.push(end.clone());
        }
        state.paused = false;
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

    /// A hook whose PTY state can be asserted, and which lets a test run
    /// something while a call to it is still being applied.
    fn hooked(during: impl Fn(bool) + Send + Sync + 'static) -> (Arc<Fanout>, Arc<Mutex<bool>>) {
        let fanout = Arc::new(Fanout::default());
        let pty = Arc::new(Mutex::new(false));
        let applied = pty.clone();
        fanout.set_pause_hook(Box::new(move |paused| {
            during(paused);
            *applied.lock().unwrap() = paused;
        }));
        (fanout, pty)
    }

    /// The reader drains while the publisher is still queueing: the pause
    /// that follows must not outlive the congestion that caused it.
    #[test]
    fn draining_during_a_publish_never_leaves_the_session_paused() {
        let (fanout, calls) = recording_fanout();
        let quick = fanout.subscribe(None);
        let other = fanout.subscribe(None);
        // The publisher queues for `quick`, then waits here for `other`
        // (already ended: it takes no output and congests nothing).
        let mut gate = other.outbox.lock().unwrap();
        gate.ended = true;
        let publisher = {
            let fanout = fanout.clone();
            std::thread::spawn(move || fanout.publish(0, &vec![b'x'; OUTBOUND_PAUSE_BYTES + 1]))
        };
        let delivered = quick.next(Duration::from_secs(5));
        assert!(matches!(delivered, Some(Delivery::Data { .. })));
        let drainer = {
            let (fanout, quick) = (fanout.clone(), quick.clone());
            std::thread::spawn(move || fanout.on_drained(&quick))
        };
        // Give the drain every chance to get in before the pause is decided.
        std::thread::sleep(Duration::from_millis(200));
        drop(gate);
        publisher.join().unwrap();
        drainer.join().unwrap();
        assert_ne!(calls.lock().unwrap().last(), Some(&true), "left paused");
    }

    /// A resume decided after a pause must also reach the PTY after it.
    #[test]
    fn a_resume_never_overtakes_the_pause_before_it() {
        let (entered, pausing) = std::sync::mpsc::channel();
        let (resumed, seen) = std::sync::mpsc::channel::<()>();
        let (entered, seen) = (Mutex::new(entered), Mutex::new(seen));
        let (fanout, pty) = hooked(move |paused| {
            if paused {
                entered.lock().unwrap().send(()).unwrap();
                // Hold the pause back until a resume got ahead, if one can.
                let _ = seen
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_millis(300));
            }
        });
        let sub = fanout.subscribe(None);
        let drainer = {
            let (fanout, sub) = (fanout.clone(), sub.clone());
            std::thread::spawn(move || {
                pausing.recv().unwrap();
                assert!(sub.next(Duration::ZERO).is_some());
                fanout.on_drained(&sub);
                let _ = resumed.send(());
            })
        };
        fanout.publish(0, &vec![b'x'; OUTBOUND_PAUSE_BYTES + 1]);
        drainer.join().unwrap();
        assert!(!*pty.lock().unwrap(), "the PTY stayed paused");
    }

    /// The session ends as a stuck subscriber is being dropped: its reader
    /// still gets a final delivery instead of waiting for ever.
    #[test]
    fn dropping_a_stuck_subscriber_keeps_the_final_delivery() {
        let (fanout, _) = recording_fanout();
        let sub = fanout.subscribe(None);
        fanout.publish(0, &vec![b'x'; OUTBOUND_PAUSE_BYTES + 1]);
        fanout.finish(Delivery::Exit(3));
        sub.drop_backlog();
        assert_eq!(sub.next(Duration::ZERO), Some(Delivery::Exit(3)));
        assert_eq!(sub.next(Duration::ZERO), None);
    }

    #[test]
    fn a_handoff_hold_survives_a_draining_subscriber() {
        let (fanout, pty) = hooked(|_| {});
        let sub = fanout.subscribe(None);
        fanout.publish(0, &vec![b'x'; OUTBOUND_PAUSE_BYTES + 1]);
        fanout.hold(true);
        while sub.next(Duration::ZERO).is_some() {}
        fanout.on_drained(&sub);
        fanout.unsubscribe(&sub);
        assert!(*pty.lock().unwrap(), "reading resumed during the handoff");
        fanout.hold(false);
        assert!(!*pty.lock().unwrap());
    }

    #[test]
    fn congestion_before_the_pty_connects_still_pauses_it() {
        let fanout = Arc::new(Fanout::default());
        let sub = fanout.subscribe(None);
        fanout.publish(0, &vec![b'x'; OUTBOUND_PAUSE_BYTES + 1]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log = calls.clone();
        fanout.set_pause_hook(Box::new(move |paused| log.lock().unwrap().push(paused)));
        assert_eq!(*calls.lock().unwrap(), vec![true]);
        drop(sub);
    }

    /// A replay larger than the limit counts as congestion like live output;
    /// it used to be marked congested without pausing or being watched, so
    /// later output queued without bound.
    #[test]
    fn a_large_replay_pauses_until_it_drains() {
        let (fanout, calls) = recording_fanout();
        let sub = fanout.subscribe(Some(Delivery::Data {
            base: 0,
            bytes: vec![b'x'; OUTBOUND_PAUSE_BYTES + 1],
        }));
        assert_eq!(*calls.lock().unwrap(), vec![true]);
        while sub.next(Duration::ZERO).is_some() {}
        fanout.on_drained(&sub);
        assert_eq!(*calls.lock().unwrap(), vec![true, false]);
    }
}
