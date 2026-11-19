//! Following a started run's agent: what it is doing, and what it said
//! when its turn ended. An agent in a terminal of its own reports through
//! its session hooks, read here; Dragent's own agent reports from the
//! window pane it runs in.

use std::collections::HashMap;

use dragent_automations::{AgentState, Run, RunProgress, RunTerminal, Store};
use dragent_host::agent_sessions;
use uuid::Uuid;

/// Runs older than this are no longer followed.
const FOLLOW_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// How long a fresh run's agent has to record its session.
const LAUNCH_GRACE_MS: i64 = 2 * 60 * 1000;
/// How long a run's agent may wait for a person before it's stopped.
pub(crate) const WAIT_LIMIT_MS: i64 = 30 * 60 * 1000;

/// A run whose recorded progress changed.
pub(crate) struct Change {
    /// The run as it was before the change.
    pub run: Run,
    pub progress: RunProgress,
}

/// What a pane's agent session says about itself.
#[derive(Debug, Default)]
pub(crate) struct Observed {
    /// Whether the session is still running; `None` when there's no record.
    pub session: Option<bool>,
    /// The latest turn event and when it happened.
    pub activity: Option<(String, i64)>,
    pub reply: Option<String>,
}

impl Observed {
    fn read(pane: Uuid) -> Self {
        let Some(record) = agent_sessions::load(pane) else {
            return Self::default();
        };
        Self {
            session: Some(record.active),
            activity: agent_sessions::load_activity(pane)
                .map(|activity| (activity.event, activity.at_ms as i64)),
            reply: agent_sessions::load_reply(pane).map(|reply| reply.text),
        }
    }
}

/// A turn event (as session hooks record it) as an agent state.
pub(crate) fn state_for(event: &str) -> AgentState {
    match event {
        "PermissionRequest" => AgentState::NeedsInput,
        "Stop" => AgentState::Done,
        "Failed" => AgentState::Failed,
        _ => AgentState::Working,
    }
}

/// The progress to record for a run whose agent got the prompt at
/// `prompted_at_ms`, or `None` when nothing changed.
pub(crate) fn next(
    previous: Option<&RunProgress>,
    observed: Observed,
    prompted_at_ms: i64,
    now_ms: i64,
) -> Option<RunProgress> {
    let reply = observed
        .reply
        .or_else(|| previous.and_then(|progress| progress.reply.clone()));
    let settled = previous.is_some_and(|progress| progress.settled);
    let next = match observed.session {
        // The agent hasn't recorded its session yet.
        None if now_ms - prompted_at_ms < LAUNCH_GRACE_MS => return None,
        Some(true) => {
            let (state, at_ms) = match observed.activity {
                Some((event, at_ms)) => (state_for(&event), at_ms),
                None => (AgentState::Working, prompted_at_ms),
            };
            RunProgress {
                state,
                at_ms,
                reply,
                closed: false,
                settled: settled || RunProgress::settles(state),
            }
        }
        // Closed (or never recorded): keep how it ended, if that's known.
        ended => {
            let state = match previous.map(|progress| progress.state) {
                Some(
                    state @ (AgentState::Done
                    | AgentState::Failed
                    | AgentState::Ended
                    | AgentState::TimedOut),
                ) => state,
                _ if ended.is_some() && reply.is_some() => AgentState::Done,
                _ => AgentState::Ended,
            };
            let at_ms = previous
                .filter(|progress| progress.state == state)
                .map_or(now_ms, |progress| progress.at_ms);
            RunProgress {
                state,
                at_ms,
                reply,
                closed: true,
                settled: true,
            }
        }
    };
    (previous != Some(&next)).then_some(next)
}

/// Whether a run has waited on a person for its own turn past the limit.
pub(crate) fn waited_too_long(progress: &RunProgress, now_ms: i64) -> bool {
    progress.state == AgentState::NeedsInput
        && !progress.settled
        && !progress.closed
        && now_ms - progress.at_ms >= WAIT_LIMIT_MS
}

/// The progress of a run stopped for waiting too long.
pub(crate) fn timed_out(progress: &RunProgress, now_ms: i64) -> RunProgress {
    RunProgress {
        state: AgentState::TimedOut,
        at_ms: now_ms,
        reply: progress.reply.clone(),
        closed: true,
        settled: true,
    }
}

/// Dismiss whatever a terminal agent is asking (Esc cancels a pending
/// approval or question in the agent CLIs), so it stops waiting.
fn interrupt(terminal: &RunTerminal) {
    let sent = dragent_host::HostClient::connect_existing().and_then(|client| {
        client.send_terminal_input(&terminal.workspace_id, &terminal.session_id, b"\x1b")
    });
    if let Err(error) = sent {
        log::warn!("[automations] stopping a waiting agent: {error}");
    }
}

/// The start of an agent's reply as a few plain lines.
pub(crate) fn reply_preview(text: &str) -> String {
    const LINES: usize = 3;
    const CHARS: usize = 280;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(&['#', '>', '-', '*', '•'][..])
                .trim()
                .replace("**", "")
                .replace('`', "")
        })
        .filter(|line| !line.is_empty())
        .take(LINES + 1)
        .collect();
    let mut cut = lines.len() > LINES;
    let mut preview = lines.into_iter().take(LINES).collect::<Vec<_>>().join("\n");
    if preview.chars().count() > CHARS {
        preview = preview.chars().take(CHARS).collect::<String>().trim_end().to_string();
        cut = true;
    }
    if cut {
        preview.push('…');
    }
    preview
}

/// Bring every followed run up to date, and stop terminal agents that
/// waited for a person too long. Returns what changed.
pub(crate) fn follow(store: &Store, now_ms: i64) -> Vec<Change> {
    let runs = match store.followed_runs(now_ms - FOLLOW_FOR_MS) {
        Ok(runs) => runs,
        Err(error) => {
            log::warn!("[automations] reading followed runs: {error:#}");
            return Vec::new();
        }
    };
    // Oldest first, so the latest run in each pane wins: a later run that
    // continued the same session takes it over.
    let latest: HashMap<&str, &str> = runs
        .iter()
        .filter_map(|run| Some((run.agent_pane()?, run.id.as_str())))
        .collect();
    let mut changes = Vec::new();
    for run in &runs {
        let Some(pane) = run.agent_pane() else {
            continue;
        };
        let progress = if latest.get(pane) != Some(&run.id.as_str()) {
            let mut progress = run.progress.clone().unwrap_or(RunProgress {
                state: AgentState::Ended,
                at_ms: now_ms,
                reply: None,
                closed: true,
                settled: true,
            });
            progress.closed = true;
            Some(progress)
        } else if let Some(terminal) = &run.terminal {
            let Ok(uuid) = Uuid::parse_str(pane) else {
                continue;
            };
            let prompted_at = run.finished_at_ms.or(run.started_at_ms).unwrap_or(now_ms);
            let progress = next(run.progress.as_ref(), Observed::read(uuid), prompted_at, now_ms)
                .or_else(|| run.progress.clone());
            match progress {
                Some(progress) if waited_too_long(&progress, now_ms) => {
                    interrupt(terminal);
                    Some(timed_out(&progress, now_ms))
                }
                progress => progress,
            }
        } else {
            // Dragent's own agent: its window reports, and its runs never
            // wait for a person.
            None
        };
        let Some(progress) = progress.filter(|progress| run.progress.as_ref() != Some(progress)) else {
            continue;
        };
        match store.set_progress(&run.id, progress.clone()) {
            Ok(Some(_)) => changes.push(Change {
                run: run.clone(),
                progress,
            }),
            Ok(None) => {}
            Err(error) => log::warn!("[automations] recording run progress: {error:#}"),
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(state: AgentState, reply: Option<&str>, closed: bool) -> RunProgress {
        RunProgress {
            state,
            at_ms: 50,
            reply: reply.map(str::to_owned),
            closed,
            settled: closed || RunProgress::settles(state),
        }
    }

    fn running(event: Option<&str>, reply: Option<&str>) -> Observed {
        Observed {
            session: Some(true),
            activity: event.map(|event| (event.to_string(), 50)),
            reply: reply.map(str::to_owned),
        }
    }

    #[test]
    fn replies_preview_as_a_few_plain_lines() {
        assert_eq!(reply_preview("AUTOMATION TEST OK, 2026-10-23"), "AUTOMATION TEST OK, 2026-10-23");
        assert_eq!(
            reply_preview("# Findings\n\n- **Medium** — `write_replacing` leaks\n\nNo other issues."),
            "Findings\nMedium — write_replacing leaks\nNo other issues."
        );
        assert_eq!(reply_preview("one\ntwo\nthree\nfour"), "one\ntwo\nthree…");
        assert!(reply_preview(&"x".repeat(400)).ends_with('…'));
    }

    #[test]
    fn turn_events_become_states() {
        assert_eq!(state_for("Start"), AgentState::Working);
        assert_eq!(state_for("PermissionRequest"), AgentState::NeedsInput);
        assert_eq!(state_for("Stop"), AgentState::Done);
        assert_eq!(state_for("Failed"), AgentState::Failed);
    }

    #[test]
    fn a_running_agent_reports_its_turns_and_reply() {
        // Prompted, nothing reported yet: working since the prompt.
        let working = next(None, running(None, None), 10, 20).unwrap();
        assert_eq!((working.state, working.at_ms, working.closed), (AgentState::Working, 10, false));
        // Unchanged: nothing to record.
        assert_eq!(next(Some(&working), running(None, None), 10, 30), None);
        let done = next(Some(&working), running(Some("Stop"), Some("All clear")), 10, 60).unwrap();
        assert_eq!(done, progress(AgentState::Done, Some("All clear"), false));
        // A follow-up turn keeps the last reply until a new one comes, and
        // the run stays settled: that turn is someone's follow-up.
        let again = next(Some(&done), running(Some("Start"), None), 10, 70).unwrap();
        assert_eq!(
            again,
            RunProgress {
                settled: true,
                ..progress(AgentState::Working, Some("All clear"), false)
            }
        );
        assert!(!working.settled);
    }

    #[test]
    fn only_the_runs_own_wait_times_out() {
        let waiting = progress(AgentState::NeedsInput, None, false);
        assert!(!waited_too_long(&waiting, 50 + WAIT_LIMIT_MS - 1));
        assert!(waited_too_long(&waiting, 50 + WAIT_LIMIT_MS));
        let stopped = timed_out(&waiting, 50 + WAIT_LIMIT_MS);
        assert_eq!((stopped.state, stopped.closed, stopped.settled), (AgentState::TimedOut, true, true));
        // A follow-up someone typed after the run finished is theirs to answer.
        let follow_up = RunProgress {
            settled: true,
            ..waiting
        };
        assert!(!waited_too_long(&follow_up, 50 + WAIT_LIMIT_MS * 2));
    }

    #[test]
    fn a_closed_session_keeps_how_it_ended() {
        let closed = Observed {
            session: Some(false),
            ..Observed::default()
        };
        let done = progress(AgentState::Done, Some("All clear"), false);
        let ended = next(Some(&done), closed, 10, 90).unwrap();
        assert_eq!(ended, progress(AgentState::Done, Some("All clear"), true));
        // Closed mid-turn, never reported a stop.
        let closed = Observed {
            session: Some(false),
            ..Observed::default()
        };
        let working = progress(AgentState::Working, None, false);
        let ended = next(Some(&working), closed, 10, 90).unwrap();
        assert_eq!((ended.state, ended.at_ms, ended.closed), (AgentState::Ended, 90, true));
    }

    #[test]
    fn a_missing_session_waits_for_launch_then_gives_up() {
        assert_eq!(next(None, Observed::default(), 1_000, 2_000), None);
        let gone = next(None, Observed::default(), 1_000, 1_000 + LAUNCH_GRACE_MS).unwrap();
        assert_eq!((gone.state, gone.closed), (AgentState::Ended, true));
    }
}
