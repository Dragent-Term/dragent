//! The one line new agents wait in while the machine is critically low on
//! memory (or the person's optional cap is reached).
//!
//! Every agent, however it was started (a tab, a menu, the command line, a
//! typed command, a team worker), runs under the launch monitor in
//! [`crate::agent_sessions`]. That monitor asks here before it starts the
//! agent program, and keeps asking while it waits. The host owns the line,
//! so starts go one at a time, in the order they arrived, with a pause
//! between them; the waiting itself lives in each monitor's own terminal, so
//! it outlives the host and the app. After a host restart the monitors ask
//! again with the time they first asked, which restores the order.
//!
//! Nothing is ever refused: a wait ends when memory frees, when the person
//! starts it anyway, or when they cancel it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::protocol::WaitReason;
pub use crate::protocol::{StartAnswer as Answer, StartAsk as Ask, WaitingStart};

/// How often a waiting start asks again.
pub const ASK_EVERY: Duration = Duration::from_secs(2);
/// A waiter that hasn't asked for this long is gone (its terminal closed).
pub const FORGET_AFTER: Duration = Duration::from_secs(15);
/// Pause after one released start before the next, so several starts
/// released together can't overshoot the memory that came free.
pub const SETTLE: Duration = Duration::from_secs(5);
/// A new host gives the waiters of the one before this long to ask again
/// before it releases any of them, so they keep their order.
pub const REJOIN: Duration = Duration::from_secs(5);
/// A "start anyway" given before its agent asked stays valid this long.
const ANYWAY_FOR: Duration = Duration::from_secs(120);

struct Ticket {
    ask: Ask,
    seen: Instant,
}

pub struct StartGate {
    waiting: Vec<Ticket>,
    anyway: HashMap<String, Instant>,
    /// Terminals closed a moment ago: one's monitor may have asked again on
    /// its way out.
    closed: HashMap<String, Instant>,
    released: Option<Instant>,
    opened: Instant,
    opened_ms: u64,
}

impl StartGate {
    pub fn new(now: Instant, now_ms: u64) -> Self {
        Self {
            waiting: Vec::new(),
            anyway: HashMap::new(),
            closed: HashMap::new(),
            released: None,
            opened: now,
            opened_ms: now_ms,
        }
    }

    /// Whether `ask` may start now. `held` is why new starts wait at the
    /// moment, if they do; `capped` whether a cap is set (its count only
    /// follows a start a moment later, so starts are spaced under a cap).
    pub fn ask(
        &mut self,
        ask: Ask,
        held: Option<WaitReason>,
        capped: bool,
        now: Instant,
    ) -> Answer {
        self.forget_gone(now);
        let known = self.waiting.iter().position(|t| t.ask.id == ask.id);
        if self.anyway.remove(&ask.id).is_some() || self.anyway.remove(&ask.terminal_id).is_some() {
            if let Some(at) = known {
                self.waiting.remove(at);
            }
            self.released = Some(now);
            return Answer::Go;
        }
        let key = |ask: &Ask| (ask.since_ms, ask.id.clone());
        let ahead = self
            .waiting
            .iter()
            .filter(|t| t.ask.id != ask.id && key(&t.ask) < key(&ask))
            .count();
        let settling = self
            .released
            .is_some_and(|at| now.saturating_duration_since(at) < SETTLE);
        // Others from before this host started may not have asked again yet.
        let rejoining =
            ask.since_ms < self.opened_ms && now.saturating_duration_since(self.opened) < REJOIN;
        if held.is_none() && ahead == 0 && !settling && !rejoining {
            if let Some(at) = known {
                self.waiting.remove(at);
            }
            if known.is_some() || capped || rejoined(&ask, self.opened_ms) {
                self.released = Some(now);
            }
            return Answer::Go;
        }
        if self.closed.contains_key(&ask.terminal_id) {
            return Answer::Wait {
                position: ahead + 1,
                reason: held,
            };
        }
        match known {
            Some(at) => self.waiting[at] = Ticket { ask, seen: now },
            None => self.waiting.push(Ticket { ask, seen: now }),
        }
        Answer::Wait {
            position: ahead + 1,
            reason: held,
        }
    }

    /// The person wants `id` (a pane or terminal) started whatever memory
    /// says. Valid before its agent asked, too.
    pub fn start_anyway(&mut self, id: &str, now: Instant) {
        self.anyway.insert(id.to_string(), now);
    }

    /// `id` no longer wants to start.
    pub fn leave(&mut self, id: &str) {
        self.waiting.retain(|t| t.ask.id != id);
        self.anyway.remove(id);
    }

    /// `terminal_id` was closed, and whoever waited in it with it.
    pub fn terminal_closed(&mut self, terminal_id: &str, now: Instant) {
        self.waiting.retain(|t| t.ask.terminal_id != terminal_id);
        self.anyway.remove(terminal_id);
        self.closed.insert(terminal_id.to_string(), now);
    }

    /// Who waits, first in line first.
    pub fn waiting(&mut self, now: Instant) -> Vec<WaitingStart> {
        self.forget_gone(now);
        let mut asks: Vec<&Ask> = self.waiting.iter().map(|t| &t.ask).collect();
        asks.sort_by(|a, b| (a.since_ms, &a.id).cmp(&(b.since_ms, &b.id)));
        asks.into_iter()
            .enumerate()
            .map(|(at, ask)| WaitingStart {
                id: ask.id.clone(),
                terminal_id: ask.terminal_id.clone(),
                label: ask.label.clone(),
                since_ms: ask.since_ms,
                position: at + 1,
            })
            .collect()
    }

    /// Whether a start arriving now would have to wait.
    pub fn would_wait(&mut self, held: Option<&WaitReason>, now: Instant) -> bool {
        self.forget_gone(now);
        held.is_some() || !self.waiting.is_empty()
    }

    fn forget_gone(&mut self, now: Instant) {
        self.closed
            .retain(|_, at| now.saturating_duration_since(*at) < FORGET_AFTER);
        self.waiting
            .retain(|t| now.saturating_duration_since(t.seen) < FORGET_AFTER);
        self.anyway
            .retain(|_, at| now.saturating_duration_since(*at) < ANYWAY_FOR);
    }
}

/// A waiter of an earlier host: its release is spaced like any other.
fn rejoined(ask: &Ask, opened_ms: u64) -> bool {
    ask.since_ms < opened_ms
}

/// What a waiting start tells the person.
pub fn waiting_text(label: &str, answer: &Answer) -> String {
    let Answer::Wait { position, reason } = answer else {
        return String::new();
    };
    let why = match reason {
        Some(WaitReason::MemoryCritical) => "this computer is critically low on memory".to_string(),
        Some(WaitReason::AtCap { live, max }) => format!("{live} of {max} agents are running"),
        None => "another agent is starting first".to_string(),
    };
    let line = match position.saturating_sub(1) {
        0 => String::new(),
        1 => " 1 agent starts before it.".to_string(),
        ahead => format!(" {ahead} agents start before it."),
    };
    format!("Waiting for memory: {why}. {label} starts on its own.{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRITICAL: Option<WaitReason> = Some(WaitReason::MemoryCritical);

    fn ask(id: &str, since_ms: u64) -> Ask {
        Ask {
            id: id.into(),
            terminal_id: format!("term-{id}"),
            label: "agent".into(),
            since_ms,
        }
    }

    fn gate() -> (StartGate, Instant) {
        let now = Instant::now();
        (StartGate::new(now, 1_000), now)
    }

    fn position(answer: Answer) -> usize {
        match answer {
            Answer::Wait { position, .. } => position,
            Answer::Go => panic!("expected to wait"),
        }
    }

    #[test]
    fn any_number_start_at_once_while_memory_is_fine() {
        let (mut gate, now) = gate();
        for n in 0..100 {
            let answer = gate.ask(ask(&n.to_string(), 2_000 + n), None, false, now);
            assert_eq!(answer, Answer::Go);
        }
    }

    #[test]
    fn critical_memory_holds_starts_then_releases_them_in_order_one_at_a_time() {
        let (mut gate, now) = gate();
        assert_eq!(position(gate.ask(ask("a", 2_000), CRITICAL, false, now)), 1);
        assert_eq!(position(gate.ask(ask("b", 2_001), CRITICAL, false, now)), 2);
        assert_eq!(position(gate.ask(ask("c", 2_002), CRITICAL, false, now)), 3);
        // Memory frees. Whoever asks first, "a" goes first.
        let now = now + Duration::from_secs(1);
        assert_eq!(position(gate.ask(ask("c", 2_002), None, false, now)), 3);
        assert_eq!(position(gate.ask(ask("b", 2_001), None, false, now)), 2);
        assert_eq!(gate.ask(ask("a", 2_000), None, false, now), Answer::Go);
        // "b" is next but waits for "a" to settle, and so does a new start.
        assert_eq!(position(gate.ask(ask("b", 2_001), None, false, now)), 1);
        assert_eq!(position(gate.ask(ask("new", 3_000), None, false, now)), 3);
        let now = now + SETTLE;
        assert_eq!(position(gate.ask(ask("c", 2_002), None, false, now)), 2);
        assert_eq!(gate.ask(ask("b", 2_001), None, false, now), Answer::Go);
        let now = now + SETTLE;
        assert_eq!(position(gate.ask(ask("new", 3_000), None, false, now)), 2);
        assert_eq!(gate.ask(ask("c", 2_002), None, false, now), Answer::Go);
    }

    #[test]
    fn memory_running_short_again_stops_the_line() {
        let (mut gate, now) = gate();
        gate.ask(ask("a", 2_000), CRITICAL, false, now);
        gate.ask(ask("b", 2_001), CRITICAL, false, now);
        assert_eq!(gate.ask(ask("a", 2_000), None, false, now), Answer::Go);
        let now = now + SETTLE * 2;
        assert_eq!(position(gate.ask(ask("b", 2_001), CRITICAL, false, now)), 1);
    }

    #[test]
    fn a_closed_terminal_leaves_the_line_at_once_and_for_good() {
        let (mut gate, now) = gate();
        gate.ask(ask("a", 2_000), CRITICAL, false, now);
        gate.ask(ask("b", 2_001), CRITICAL, false, now);
        gate.terminal_closed("term-a", now);
        // Its monitor asked once more before it ended.
        gate.ask(ask("a", 2_000), CRITICAL, false, now);
        let waiting = gate.waiting(now);
        assert_eq!(waiting.len(), 1);
        assert_eq!((waiting[0].id.as_str(), waiting[0].position), ("b", 1));
    }

    #[test]
    fn start_anyway_goes_at_once_from_any_place_and_is_used_once() {
        let (mut gate, now) = gate();
        gate.ask(ask("a", 2_000), CRITICAL, false, now);
        gate.ask(ask("b", 2_001), CRITICAL, false, now);
        gate.start_anyway("b", now);
        assert_eq!(gate.ask(ask("b", 2_001), CRITICAL, false, now), Answer::Go);
        assert_eq!(position(gate.ask(ask("a", 2_000), CRITICAL, false, now)), 1);
        // Asked for again, it waits like any other.
        assert_eq!(position(gate.ask(ask("b", 2_001), CRITICAL, false, now)), 2);
        // By terminal, and before its agent asked.
        gate.start_anyway("term-c", now);
        assert_eq!(gate.ask(ask("c", 9_000), CRITICAL, true, now), Answer::Go);
    }

    #[test]
    fn a_start_that_went_away_does_not_hold_up_the_rest() {
        let (mut gate, now) = gate();
        gate.ask(ask("gone", 2_000), CRITICAL, false, now);
        gate.ask(ask("here", 2_001), CRITICAL, false, now);
        let now = now + FORGET_AFTER;
        assert_eq!(gate.ask(ask("here", 2_001), None, false, now), Answer::Go);
        gate.ask(ask("cancelled", 2_002), CRITICAL, false, now);
        gate.leave("cancelled");
        assert!(gate.waiting(now).is_empty());
    }

    #[test]
    fn a_new_host_keeps_the_order_whoever_asks_first() {
        let now = Instant::now();
        // Opened after all three had begun to wait.
        let mut gate = StartGate::new(now, 10_000);
        assert_eq!(position(gate.ask(ask("c", 2_002), None, false, now)), 1);
        assert_eq!(position(gate.ask(ask("a", 2_000), None, false, now)), 1);
        assert_eq!(position(gate.ask(ask("b", 2_001), None, false, now)), 2);
        let ids: Vec<String> = gate.waiting(now).into_iter().map(|w| w.id).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        let now = now + REJOIN;
        assert_eq!(position(gate.ask(ask("c", 2_002), None, false, now)), 3);
        assert_eq!(gate.ask(ask("a", 2_000), None, false, now), Answer::Go);
        // An unrelated first start on a new host isn't held back.
        let mut fresh = StartGate::new(now, 10_000);
        assert_eq!(fresh.ask(ask("x", 10_001), None, false, now), Answer::Go);
    }

    #[test]
    fn under_a_cap_starts_are_spaced_so_the_count_can_follow() {
        let (mut gate, now) = gate();
        assert_eq!(gate.ask(ask("a", 2_000), None, true, now), Answer::Go);
        assert_eq!(position(gate.ask(ask("b", 2_001), None, true, now)), 1);
        let full = Some(WaitReason::AtCap { live: 1, max: 1 });
        let now = now + SETTLE;
        assert_eq!(
            gate.ask(ask("b", 2_001), full.clone(), true, now),
            Answer::Wait {
                position: 1,
                reason: full
            }
        );
    }

    #[test]
    fn the_wait_says_why_and_that_nothing_is_needed() {
        let text = waiting_text(
            "claude",
            &Answer::Wait {
                position: 3,
                reason: CRITICAL,
            },
        );
        assert!(text.starts_with("Waiting for memory"), "{text}");
        assert!(text.contains("2 agents start before it"), "{text}");
    }
}
