//! Agents that stop without saying so.
//!
//! An agent's status comes from its hook reports, but not every ending has
//! one: Claude Code reports nothing when a turn is interrupted (or a dialog
//! cancelled) with Esc, and Codex nothing when it hits its usage limit. Left
//! alone, such an agent reads "working" until its next prompt.
//!
//! Its terminal gives it away. All of these agents draw a spinner or timer
//! for as long as they work, through long tool calls too (measured: never
//! silent for more than 3 s), and draw nothing once they stop. So an agent
//! reported as working whose terminal stays silent becomes `Quiet`: not
//! working, without claiming it finished, so nothing notifies or asks for a
//! review. Its next hook report takes over again.
//!
//! A permission prompt draws nothing either, so silence alone means nothing
//! there: only a cancelling key pressed at the prompt ends it.

use std::collections::HashMap;
use std::sync::Mutex;

use uuid::Uuid;

use crate::agent_sessions::{ACTIVITY_QUIET, Unsettled};

/// Silence that means a working agent has stopped.
pub const SILENT_MS: i64 = 10_000;
/// The same, right after someone pressed Esc or Ctrl-C.
pub const SILENT_AFTER_KEY_MS: i64 = 3_000;
/// How often terminals are looked at.
pub const POLL_MS: u64 = 2_000;

/// When each terminal last got a key that interrupts an agent.
fn interrupts() -> &'static Mutex<HashMap<String, i64>> {
    static KEYS: std::sync::OnceLock<Mutex<HashMap<String, i64>>> = std::sync::OnceLock::new();
    KEYS.get_or_init(Default::default)
}

/// Esc or Ctrl-C on its own, plainly or as the keyboard protocols agents
/// turn on spell them. Longer sequences that start with Esc are other keys.
fn is_interrupt(data: &[u8]) -> bool {
    matches!(
        data,
        b"\x1b" | b"\x03" | b"\x1b[27u" | b"\x1b[27;1u" | b"\x1b[99;5u"
    )
}

/// Terminal `terminal_id` got `data` as input.
pub fn note_input(terminal_id: &str, data: &[u8], now_ms: i64) {
    if is_interrupt(data)
        && let Ok(mut keys) = interrupts().lock()
    {
        keys.insert(terminal_id.to_string(), now_ms);
    }
}

pub fn last_interrupt_ms(terminal_id: &str) -> Option<i64> {
    interrupts().lock().ok()?.get(terminal_id).copied()
}

/// What's known of a terminal's output and keys.
pub trait Terminals {
    /// Bytes it has written so far; `None` when it can't be read.
    fn output_total(&self, terminal: &str) -> Option<u64>;
    fn last_interrupt_ms(&self, terminal: &str) -> Option<i64>;
}

/// How long each unsettled agent's terminal has been silent.
#[derive(Default)]
pub struct Watch {
    /// By pane: the output seen, and when it last changed.
    seen: HashMap<Uuid, (u64, i64)>,
}

impl Watch {
    /// Which of `agents` have stopped, as of `now`.
    pub fn stopped(
        &mut self,
        now: i64,
        agents: &[Unsettled],
        terminals: &dyn Terminals,
    ) -> Vec<Uuid> {
        self.seen
            .retain(|pane, _| agents.iter().any(|agent| agent.pane == *pane));
        let mut stopped = Vec::new();
        for agent in agents {
            let Some(total) = terminals.output_total(&agent.terminal) else {
                self.seen.remove(&agent.pane);
                continue;
            };
            let since = match self.seen.get_mut(&agent.pane) {
                Some((seen, since)) if *seen == total => *since,
                Some(entry) => {
                    *entry = (total, now);
                    now
                }
                None => {
                    self.seen.insert(agent.pane, (total, now));
                    now
                }
            };
            // A key from before this report is about something else.
            let key = terminals
                .last_interrupt_ms(&agent.terminal)
                .filter(|at| *at >= agent.at_ms as i64);
            let after_key = key.is_some_and(|at| now - since.max(at) >= SILENT_AFTER_KEY_MS);
            let silent = now - since >= SILENT_MS;
            if after_key || (silent && agent.event != "PermissionRequest") {
                stopped.push(agent.pane);
                self.seen.remove(&agent.pane);
            }
        }
        stopped
    }
}

/// Mark `pane` quiet, unless a hook report came in meanwhile.
pub fn settle(agent: &Unsettled) {
    if let Err(error) = crate::agent_sessions::mark_activity(agent, ACTIVITY_QUIET) {
        log::warn!("[agents] couldn't update {}: {error}", agent.pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        total: RefCell<u64>,
        key: RefCell<Option<i64>>,
    }

    impl Terminals for Fake {
        fn output_total(&self, _terminal: &str) -> Option<u64> {
            Some(*self.total.borrow())
        }
        fn last_interrupt_ms(&self, _terminal: &str) -> Option<i64> {
            *self.key.borrow()
        }
    }

    fn agent(event: &str, at_ms: u64) -> Unsettled {
        Unsettled {
            pane: Uuid::from_u128(1),
            terminal: "term".into(),
            generation: Uuid::from_u128(2),
            conversation_id: "conversation".into(),
            event: event.into(),
            at_ms,
        }
    }

    #[test]
    fn a_working_agent_stays_working_while_its_terminal_draws() {
        let fake = Fake::default();
        let mut watch = Watch::default();
        let agents = [agent("Start", 0)];
        for second in 0..60 {
            *fake.total.borrow_mut() += 100;
            assert!(watch.stopped(second * 1_000, &agents, &fake).is_empty());
        }
    }

    #[test]
    fn a_silent_working_agent_has_stopped() {
        let fake = Fake::default();
        let mut watch = Watch::default();
        let agents = [agent("Start", 0)];
        assert!(watch.stopped(1_000, &agents, &fake).is_empty());
        assert!(watch.stopped(1_000 + SILENT_MS - 1, &agents, &fake).is_empty());
        assert_eq!(watch.stopped(1_000 + SILENT_MS, &agents, &fake), [agents[0].pane]);
    }

    #[test]
    fn output_starts_the_silence_over() {
        let fake = Fake::default();
        let mut watch = Watch::default();
        let agents = [agent("Start", 0)];
        watch.stopped(0, &agents, &fake);
        *fake.total.borrow_mut() = 5;
        assert!(watch.stopped(SILENT_MS, &agents, &fake).is_empty());
        assert!(watch.stopped(SILENT_MS * 2 - 1, &agents, &fake).is_empty());
        assert_eq!(watch.stopped(SILENT_MS * 2, &agents, &fake).len(), 1);
    }

    #[test]
    fn esc_settles_a_working_agent_sooner() {
        let fake = Fake::default();
        let mut watch = Watch::default();
        let agents = [agent("Start", 100)];
        watch.stopped(1_000, &agents, &fake);
        *fake.key.borrow_mut() = Some(2_000);
        assert!(watch.stopped(2_000 + SILENT_AFTER_KEY_MS - 1, &agents, &fake).is_empty());
        assert_eq!(watch.stopped(2_000 + SILENT_AFTER_KEY_MS, &agents, &fake).len(), 1);
    }

    #[test]
    fn esc_that_doesnt_stop_it_changes_nothing() {
        // It closed a menu, say: the agent carries on drawing.
        let fake = Fake::default();
        let mut watch = Watch::default();
        let agents = [agent("Start", 100)];
        *fake.key.borrow_mut() = Some(1_000);
        for second in 1..30 {
            *fake.total.borrow_mut() += 100;
            assert!(watch.stopped(second * 1_000, &agents, &fake).is_empty());
        }
    }

    #[test]
    fn a_prompt_waits_however_long_unless_it_was_cancelled() {
        let fake = Fake::default();
        let mut watch = Watch::default();
        let agents = [agent("PermissionRequest", 5_000)];
        watch.stopped(5_000, &agents, &fake);
        assert!(watch.stopped(3_600_000, &agents, &fake).is_empty(), "still asking");

        // A key from before the prompt says nothing about it.
        *fake.key.borrow_mut() = Some(4_000);
        assert!(watch.stopped(3_700_000, &agents, &fake).is_empty());

        *fake.key.borrow_mut() = Some(3_800_000);
        assert!(watch.stopped(3_800_000 + SILENT_AFTER_KEY_MS - 1, &agents, &fake).is_empty());
        assert_eq!(watch.stopped(3_800_000 + SILENT_AFTER_KEY_MS, &agents, &fake).len(), 1);
    }

    #[test]
    fn only_a_lone_interrupt_counts() {
        for key in [&b"\x1b"[..], b"\x03", b"\x1b[27u", b"\x1b[27;1u", b"\x1b[99;5u"] {
            assert!(is_interrupt(key), "{key:?}");
        }
        for other in [&b"\x1b[A"[..], b"\x1b[200~hi\x1b[201~", b"a", b"\r", b""] {
            assert!(!is_interrupt(other), "{other:?}");
        }
    }
}
