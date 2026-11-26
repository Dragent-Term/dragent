//! The team courier: carries what happens in a team to the agents in it,
//! typing into their sessions the way a person would.
//!
//! Agents only act on their turn. A lead that ended its turn while its
//! workers carry on, or a worker that finished a turn while the lead
//! answered its question, would otherwise never hear of it. So when an
//! agent sits idle at its prompt (its hooks reported `Stop`) and something
//! is waiting for it, the courier types it in: a short digest for the lead
//! (which reads the details with `dragent tasks wait`), the messages
//! themselves for a worker. It also reminds a worker that went quiet
//! without finishing, tells the lead when a worker waits for an approval
//! or stopped, and fails a task whose agent exited.
//!
//! The person's messages to the lead don't wait for any of that: they go in
//! whole, at once, working or not (agents queue what's typed mid-turn).
//! Only a prompt of the agent's own holds them back, since Enter would
//! answer it. Everything else stays out of a terminal someone typed into a
//! moment ago, and out of an agent that's working or showing a prompt.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::team::{
    Change, CourierMark, Member, Message, MessageKind, Party, SessionIdentity, Task, TaskKind,
    TaskStatus, Team, TeamEvent, TeamRequest, TeamStatus, TeamStore, concerns,
};

/// How long an agent sits at its prompt after a turn before the courier
/// types: its screen settles first.
const SETTLE_MS: i64 = 3_000;
/// Nothing is typed into a terminal that had input this recently: someone
/// may be in the middle of a message.
const QUIET_MS: i64 = 15_000;
/// For an agent that doesn't report what it's doing: how long something
/// waits before it's typed in anyway.
const UNREPORTED_MS: i64 = 45_000;
/// A worker idle this long without finishing gets a reminder…
const NUDGE_AFTER_MS: i64 = 30_000;
/// …at most this often, and this many times.
const NUDGE_GAP_MS: i64 = 120_000;
const MAX_NUDGES: u32 = 2;
/// After its last reminder, how long before the lead hears it stopped.
const STALL_AFTER_MS: i64 = 5 * 60_000;
/// An approval prompt open this long is reported to the lead.
const PERMISSION_AFTER_MS: i64 = 20_000;
/// A worker whose agent hasn't shown up this long after it started failed
/// to start.
const NO_SESSION_MS: i64 = 3 * 60_000;
/// A worker whose terminal can't be reached this long (its tab closed)
/// ended without finishing.
const UNREACHABLE_MS: i64 = 5 * 60_000;
/// Longest piece of a message or result quoted into a session.
const QUOTE_CHARS: usize = 400;
/// The person's message only keeps clear of keystrokes still landing.
const PERSON_QUIET_MS: i64 = 2_000;
/// A sleeping agent is asked to wake at most this often.
const WAKE_GAP_MS: i64 = 60_000;
/// News the lead hasn't read this long after it was typed is typed again...
const REMIND_AFTER_MS: i64 = 5 * 60_000;
/// ...this many times at most, and then the person is told.
const MAX_REMINDERS: u32 = 3;

/// What the courier sees of an agent's session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionView {
    pub workspace_id: String,
    pub terminal_id: String,
    /// The agent is still running (its record says so and its terminal
    /// hasn't exited).
    pub active: bool,
    /// The terminal service knows the terminal and it runs.
    pub reachable: bool,
    /// Unreachable for so long that its terminal is taken to be closed (a
    /// closed tab, or one Dragent couldn't bring back).
    pub lost: bool,
    /// Its last hook report (`Start`, `Stop`, `PermissionRequest`,
    /// `Failed`) and when, in Unix milliseconds.
    pub activity: Option<(String, u64)>,
    /// When the terminal last got input, in Unix milliseconds.
    pub last_input_ms: Option<i64>,
    /// The agent was put to sleep on purpose (its terminal was closed to
    /// free memory). That isn't an exit: it's woken when something waits.
    pub asleep: bool,
    /// It can't take anything in at the moment: it hasn't started because it
    /// waits for memory, or it's paused over its memory limit. Nothing is
    /// typed, no reminder is due, and it hasn't failed.
    pub held: bool,
    /// What it held when it was paused over its memory limit, while it is.
    pub paused_holding: Option<u64>,
    /// For an agent that reports no hook events: how long its terminal's
    /// output has been unchanged, in milliseconds.
    pub output_quiet_ms: Option<i64>,
}

/// The sessions behind panes, and typing into them.
pub trait Sessions {
    fn session(&self, pane: &str) -> Option<SessionView>;
    /// The agent session in host terminal `terminal`, for a member known
    /// only by its terminal.
    fn pane_of_terminal(&self, _terminal: &str) -> Option<String> {
        None
    }
    /// Type `text` as a paste, then Enter.
    fn type_text(&self, session: &SessionView, text: &str) -> Result<(), String>;
    /// Bytes the terminal has written so far, when it can be read.
    fn output_total(&self, _terminal: &str) -> Option<u64> {
        None
    }
    /// Ask for the sleeping agent in `pane` to be woken; the app resumes it.
    fn wake(&self, _pane: &str, _reason: &str) {}
    /// The agent conversation running in `pane`, once its agent reported it.
    fn identity(&self, _pane: &str) -> Option<SessionIdentity> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Type `text` into `party`'s session, then mark `read` messages read
    /// and record what was typed.
    Type {
        team: String,
        party: Party,
        session: SessionView,
        text: String,
        read: Vec<u64>,
        revision: Option<u64>,
        nudge: bool,
        /// A reminder that news typed earlier is still unread.
        reminder: bool,
    },
    /// Ask for the sleeping agent of `party`, in `pane`, to be woken.
    Wake {
        team: String,
        party: Party,
        pane: String,
        reason: String,
    },
    /// `party`'s agent answered its wake: what it's typed waits for a sign
    /// of life after it.
    Woke {
        team: String,
        party: Party,
    },
    /// The lead's agent moved from `from` to `lead`: the team's record
    /// follows it, unless the lead was changed meanwhile.
    Relead {
        team: String,
        from: Member,
        lead: Member,
    },
    /// `party`'s seat learns the conversation its agent runs, so the seat
    /// can follow that conversation to another pane.
    Learn {
        team: String,
        party: Party,
        pane: String,
        identity: SessionIdentity,
    },
    /// Tell the person something about the lead.
    TellPerson {
        team: String,
        body: String,
        about: PersonNote,
    },
    Fail {
        team: String,
        task: String,
        reason: String,
    },
    /// A note from Dragent to the lead about `task`.
    Note {
        team: String,
        task: String,
        body: String,
        mark: NoteMark,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PersonNote {
    /// The lead isn't picking up the news typed into it.
    Unheard,
    /// The sleeping lead didn't wake when asked.
    Unwoken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoteMark {
    Permission(u64),
    Stall,
    Paused(u64),
}

fn idle(session: &SessionView) -> Option<u64> {
    match &session.activity {
        Some((event, at))
            if matches!(event.as_str(), "Stop" | "Failed")
                || event == crate::agent_sessions::ACTIVITY_QUIET =>
        {
            Some(*at)
        }
        _ => None,
    }
}

/// Nobody typed into it just now.
fn quiet(session: &SessionView, now: i64) -> bool {
    session.last_input_ms.is_none_or(|at| now - at >= QUIET_MS)
}

/// Whether something waiting since `since` may be typed into `session` now.
fn ready_for(session: &SessionView, now: i64, since: i64) -> bool {
    if !session.reachable || !quiet(session, now) {
        return false;
    }
    match (&session.activity, idle(session)) {
        (_, Some(at)) => now - at as i64 >= SETTLE_MS,
        // Working, or showing an approval prompt.
        (Some(_), None) => false,
        // It doesn't say, but its terminal has gone quiet: type in.
        (None, None)
            if session
                .output_quiet_ms
                .is_some_and(|quiet| quiet >= crate::agent_quiet::SILENT_MS) =>
        {
            true
        }
        // It doesn't say: give it a while.
        (None, None) => now - since >= UNREPORTED_MS,
    }
}

/// Whether the person's message may go into `session` now.
fn ready_for_person(session: &SessionView, now: i64) -> bool {
    session.reachable
        && session
            .last_input_ms
            .is_none_or(|at| now - at >= PERSON_QUIET_MS)
        && !matches!(&session.activity, Some((event, _)) if event == "PermissionRequest")
}

fn quote(text: &str) -> String {
    let text = text.trim();
    let first: String = text.chars().take(QUOTE_CHARS).collect();
    if first.len() < text.len() {
        format!("{}…", first.trim_end())
    } else {
        first
    }
}

fn one_line(text: &str) -> String {
    quote(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Who a lead's news is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Voice {
    /// An agent in a terminal, which runs `dragent` commands.
    Terminal,
    /// Dragent's own agent, which has the coord_ tools.
    Tools,
}

/// The person's messages that haven't gone into the lead's session yet.
pub(crate) fn person_messages(team: &Team) -> Vec<&Message> {
    team.messages
        .iter()
        .filter(|message| message.goes_in_directly() && !message.read)
        .collect()
}

/// Replies that only say "go on": the page's Carry on button, "ok",
/// "thanks"… They ask nothing, so the lead answers them with work, not a
/// message (a reply would only draw another "ok").
pub(crate) fn is_acknowledgement(body: &str) -> bool {
    const ACKS: &[&str] = &[
        "carry on", "continue", "go on", "go ahead", "keep going", "proceed", "ok", "okay", "k",
        "ok thanks", "ok thank you", "ok no problem", "no problem", "thanks", "thank you", "got it",
        "sounds good", "great", "nice", "cool", "perfect", "alright", "all right", "fine", "sure",
        "noted", "👍", "👌",
    ];
    let normal: String = body
        .trim()
        .trim_end_matches(|c: char| matches!(c, '.' | '!' | ','))
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace([',', '.', '!'], "");
    ACKS.contains(&normal.as_str())
}

/// The person's messages as the lead gets them: whole, and plainly theirs.
pub(crate) fn person_text(team: &Team, messages: &[&Message], voice: Voice) -> String {
    // An "ok" to something the lead asked is an answer, not a nod.
    let answers_a_question = |message: &Message| {
        message.reply_to.is_some_and(|id| {
            team.messages.iter().any(|asked| {
                asked.id == id
                    && (matches!(asked.kind, MessageKind::Question | MessageKind::NeedsUser)
                        || !asked.choices.is_empty())
            })
        })
    };
    if !messages.is_empty()
        && messages
            .iter()
            .all(|message| is_acknowledgement(&message.body) && !answers_a_question(message))
    {
        let said = messages
            .iter()
            .map(|message| format!("#{} \"{}\"", message.id, one_line(&message.body)))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "[Dragent team {}] The person acknowledged on the team's page ({said}). Nothing to answer: \
             don't reply on the page, just carry on with the work.",
            team.id
        );
    }
    let mut text = format!(
        "[Dragent team {}] The person wrote to you from the team's page. This is from them, not news from \
         the workers: deal with it first.\n\n",
        team.id
    );
    for message in messages {
        text.push_str(&format!(
            "Message #{}{}:\n{}\n\n",
            message.id,
            kind_label(message.kind),
            message.body.trim()
        ));
    }
    let last = messages
        .last()
        .map(|message| message.id)
        .unwrap_or_default();
    text.push_str(&match voice {
        Voice::Terminal => format!(
            "Act on it now. Reply on that page (`dragent msg reply {last} \"…\"`) only when it asks you \
             something or you need their input; don't reply to acknowledgements like \"Carry on\" or \"ok\". \
             If it changes the plan, update the plan you have (`dragent team guide` says how): don't cancel \
             work or stop the team for it. If a `tasks wait` already showed you #{last}, it's the same message."
        ),
        Voice::Tools => format!(
            "Act on it now. Reply on that page (coord_msg_send to user, reply_to {last}) only when it asks you \
             something or you need their input; don't reply to acknowledgements like \"Carry on\" or \"ok\". \
             If it changes the plan, update the plan you have: don't cancel work or stop the team for it."
        ),
    });
    text
}

/// What the lead hasn't heard yet: finished, failed and blocked tasks, its
/// messages, and the team being done. Cancellations and stops come from
/// the lead or the person themselves, and the person's messages have a way
/// of their own in ([`person_messages`]).
pub(crate) fn lead_news_events(team: &Team) -> Vec<&TeamEvent> {
    let seen = team
        .courier
        .get(&Party::Lead.label())
        .map(|mark| mark.typed_revision)
        .unwrap_or(0)
        .max(team.cursors.get("lead").copied().unwrap_or(0));
    team.events
        .iter()
        .filter(|event| event.revision > seen)
        .filter(|event| concerns(event, &[], Some(&Party::Lead)))
        .filter(|event| {
            !matches!(
                event.change,
                Change::Task {
                    status: TaskStatus::Cancelled,
                    ..
                } | Change::Team {
                    status: TeamStatus::Stopped
                } | Change::Message {
                    from: Some(Party::User),
                    ..
                }
            )
        })
        .collect()
}

/// The digest typed into the lead: one line per finished, failed or
/// blocked task and per message.
pub(crate) fn lead_news(team: &Team, news: &[&TeamEvent], voice: Voice) -> String {
    let start = |task: &str| match voice {
        Voice::Terminal => format!("`dragent tasks start {task}`"),
        Voice::Tools => format!("coord_task_start ({task})"),
    };
    let verify = |task: &str| match voice {
        Voice::Terminal => format!(
            "Verify it (`dragent tasks diff {task}`, rerun its checks), then `dragent tasks integrate {task}`."
        ),
        Voice::Tools => format!(
            "Verify it (coord_task_diff, rerun its checks), then coord_task_integrate ({task})."
        ),
    };
    let mut lines = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for event in news.iter().rev() {
        if let Change::Task { task, .. } = &event.change {
            if seen.contains(&task.as_str()) {
                continue;
            }
            seen.push(task);
            let Some(task) = team.task(task) else {
                continue;
            };
            let line = match task.status {
                TaskStatus::Done if task.kind == TaskKind::Work => {
                    let outside = task.outside_area();
                    format!(
                        "- {} {} is done: {}{} {}",
                        task.id,
                        task.title,
                        task.result
                            .as_ref()
                            .map(|result| one_line(&result.summary))
                            .unwrap_or_default(),
                        if outside.is_empty() {
                            String::new()
                        } else {
                            format!(" It changed files outside its area: {}.", outside.join(", "))
                        },
                        verify(&task.id)
                    )
                }
                TaskStatus::Done => format!(
                    "- {} {} is done: {}",
                    task.id,
                    task.title,
                    task.result
                        .as_ref()
                        .map(|result| one_line(&result.summary))
                        .unwrap_or_default()
                ),
                TaskStatus::Failed => format!(
                    "- {} {} failed: {}",
                    task.id,
                    task.title,
                    one_line(task.reason.as_deref().unwrap_or("no reason given"))
                ),
                TaskStatus::NeedsDecision => format!(
                    "- {} {} needs your decision (not an error): {}",
                    task.id,
                    task.title,
                    one_line(task.reason.as_deref().unwrap_or("no reason given"))
                ),
                TaskStatus::Blocked => format!(
                    "- {} {} is blocked: {}",
                    task.id,
                    task.title,
                    one_line(task.reason.as_deref().unwrap_or("no reason given"))
                ),
                _ => continue,
            };
            lines.push(line);
        }
    }
    lines.reverse();
    for event in news {
        // QA of the integrated work: no one worker gets it back.
        if let Change::ChangesAsked { review, task } = &event.change
            && task.is_empty()
            && let Some(review) = team.task(review)
        {
            lines.push(format!(
                "- {} found problems in the integrated work: {} Send the work responsible back ({}), integrate \
                 the fix, and {} tests again on its own.",
                review.id,
                review
                    .result
                    .as_ref()
                    .map(|result| one_line(&result.summary))
                    .unwrap_or_default(),
                match voice {
                    Voice::Terminal => "`dragent tasks reopen tN --feedback \"…\"`",
                    Voice::Tools => "coord_task_reopen with feedback",
                },
                review.id
            ));
            continue;
        }
        if let Change::ChangesAsked { review, task } = &event.change
            && let (Some(review), Some(task)) = (team.task(review), team.task(task))
            && task.status == TaskStatus::Pending
        {
            let next = if team.auto_start {
                "it starts its next attempt on its own".to_string()
            } else {
                format!("{} gives it its next attempt", start(&task.id))
            };
            lines.push(format!(
                "- {} asked for changes to {} {}: {} It went back to its worker: {next}, and {} looks again after.",
                review.id,
                task.id,
                task.title,
                review
                    .result
                    .as_ref()
                    .map(|result| one_line(&result.summary))
                    .unwrap_or_default(),
                review.id
            ));
        }
    }
    if news.iter().any(|event| {
        event.change
            == Change::Team {
                status: TeamStatus::Done,
            }
    }) {
        lines.push("- Every task in the plan is finished.".into());
    }
    if team.status != TeamStatus::Done && news.iter().any(|event| event.change == Change::QaNeeded)
    {
        lines.push(match voice {
            Voice::Terminal => "- Every task is finished, but nothing has tested the whole feature, so the work isn't done yet. \
             Add the QA pass and start it: `dragent tasks add \"QA the whole feature\" --qa --prompt \"the flows to test\"` \
             (with --integrated when tasks ran side by side: it tests your integrated checkout), then `dragent tasks start`. \
             If the person tells you to skip it, add it and cancel it (`dragent tasks cancel tN`)."
                .into(),
            Voice::Tools => "- Every task is finished, but nothing has tested the whole feature, so the work isn't done yet. \
             Add the QA pass (coord_task_add with qa: true, integrated: true when tasks ran side by side, and the flows to \
             test in its prompt), then coord_task_start. If the person tells you to skip it, add it and cancel it \
             (coord_task_cancel)."
                .into(),
        });
    }
    let mut overheard = false;
    for event in news {
        if let Change::Message { id, .. } = &event.change
            && let Some(message) = team.messages.iter().find(|message| message.id == *id)
        {
            // What workers told each other: the lead listens in.
            if let (Party::Task(from), Party::Task(to)) = (&message.from, &message.to) {
                overheard = true;
                lines.push(format!(
                    "- {from} → {to}{}: {}",
                    kind_label(message.kind),
                    one_line(&message.body)
                ));
                continue;
            }
            lines.push(format!(
                "- Message #{} from {}{}: {}",
                message.id,
                message.from.label(),
                kind_label(message.kind),
                one_line(&message.body)
            ));
        }
    }
    if overheard {
        lines.push(match voice {
            Voice::Terminal => "  (Workers wrote to each other above. Let them work it out, but step in with \
                `dragent msg send tN \"…\"` when it changes the plan or another task, needs the person, or they \
                disagree, and share what they don't know: the person's decisions, how the parts fit.)"
                .into(),
            Voice::Tools => "  (Workers wrote to each other above. Let them work it out, but step in with \
                coord_msg_send when it changes the plan or another task, needs the person, or they disagree, \
                and share what they don't know: the person's decisions, how the parts fit.)"
                .into(),
        });
    }
    match voice {
        Voice::Terminal => format!(
            "[Dragent team {}] News for the lead:\n{}\nRun `dragent tasks wait` (or the team_wait tool) for the details and what's ready to start.",
            team.id,
            lines.join("\n")
        ),
        Voice::Tools => format!(
            "News for the lead of {} (from Dragent, not the person):\n{}\ncoord_team_status shows the whole plan and what's ready to start.",
            team.id,
            lines.join("\n")
        ),
    }
}

fn kind_label(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::Note => "",
        MessageKind::Question => " (question)",
        MessageKind::Answer => " (answer)",
        MessageKind::Handoff => " (handoff)",
        MessageKind::Finding => " (finding)",
        MessageKind::NeedsUser => " (for the person)",
    }
}

fn from_label(party: &Party) -> String {
    match party {
        Party::Lead => "the lead".into(),
        Party::User => "the person you work for".into(),
        Party::Task(id) => format!("the worker on {id}"),
        Party::Dragent => "Dragent".into(),
    }
}

/// Messages typed into a worker, in full.
pub(crate) fn worker_messages(task: &Task, messages: &[&Message]) -> String {
    let mut text = String::new();
    for message in messages {
        text.push_str(&format!(
            "[Dragent team] Message #{} from {}{} about {}:\n{}\n\n",
            message.id,
            from_label(&message.from),
            kind_label(message.kind),
            task.id,
            message.body.trim()
        ));
    }
    let last = messages
        .last()
        .map(|message| message.id)
        .unwrap_or_default();
    text.push_str(&format!(
        "If it needs an answer: `dragent msg reply {last} \"…\"`. Then carry on with your task."
    ));
    text
}

fn nudge(task: &Task) -> String {
    format!(
        "[Dragent team] Task {} ({}) isn't marked finished. If it's done, commit your work and run \
         `dragent tasks done --summary \"in plain words: what now works and how you know\"`. If you can't \
         go on, run `dragent tasks block \"what you need\"`. If you're waiting for an answer, run \
         `dragent tasks wait`.",
        task.id, task.title
    )
}

/// What the courier should do for `team` now.
pub(crate) fn plan(team: &Team, now: i64, sessions: &dyn Sessions) -> Vec<Action> {
    let mut actions = Vec::new();
    // A finished team still owes its lead the news of its last task.
    if team.status == TeamStatus::Stopped {
        return actions;
    }
    learn_conversations(team, sessions, &mut actions);
    let mark = |party: &Party| {
        team.courier
            .get(&party.label())
            .cloned()
            .unwrap_or_default()
    };

    // The lead hears of finished, failed and blocked tasks, of its
    // messages, and of the team being done. Cancellations and stops come
    // from the lead or the person themselves.
    // Dragent's own agent leads from inside the app, which brings it its
    // news (`TakeLeadNews`): it has no terminal to type into.
    let app_lead = team
        .lead
        .as_ref()
        .is_some_and(|lead| lead.agent.as_deref() == Some(crate::team::DRAGENT_AGENT));
    if !app_lead && let Some((pane, session)) = lead_session(team, sessions) {
        if let Some(lead) = &team.lead
            && (lead.pane.as_deref() != Some(pane.as_str())
                || (lead.terminal.is_some()
                    && lead.terminal.as_deref() != Some(session.terminal_id.as_str())))
        {
            actions.push(Action::Relead {
                team: team.id.clone(),
                from: lead.clone(),
                lead: Member {
                    pane: Some(pane.clone()),
                    terminal: Some(session.terminal_id.clone()),
                    // The same conversation, followed to its terminal.
                    ..lead.clone()
                },
            });
        }
        let lead_mark = mark(&Party::Lead);
        let person = person_messages(team);
        let news = lead_news_events(team);
        if session.asleep {
            // Woken for what waits for it; typed into once it's awake. A
            // wake left unanswered (its tab closed) is told to the person
            // once, instead of asking for wakes forever.
            if unanswered(&lead_mark, now) {
                if !lead_mark.wake_told {
                    actions.push(Action::TellPerson {
                        team: team.id.clone(),
                        body: unwoken_note(team),
                        about: PersonNote::Unwoken,
                    });
                }
            } else if (!person.is_empty() || !news.is_empty()) && wake_due(&lead_mark, now) {
                actions.push(Action::Wake {
                    team: team.id.clone(),
                    party: Party::Lead,
                    pane,
                    reason: if person.is_empty() {
                        "team news for the lead"
                    } else {
                        "a message from the person for the lead"
                    }
                    .into(),
                });
            }
        } else if session.active && !session.held {
            let fresh = awake_for_typing(team, &Party::Lead, &lead_mark, &session, now, &mut actions);
            if !fresh {
                // Just woken: it may still be starting up.
            } else if !person.is_empty() {
                if ready_for_person(&session, now) {
                    actions.push(Action::Type {
                        team: team.id.clone(),
                        party: Party::Lead,
                        session,
                        text: person_text(team, &person, Voice::Terminal),
                        read: person.iter().map(|message| message.id).collect(),
                        revision: None,
                        nudge: false,
                        reminder: false,
                    });
                }
            } else if let Some(first) = news.first()
                && ready_for(&session, now, first.at_ms)
            {
                actions.push(Action::Type {
                    team: team.id.clone(),
                    party: Party::Lead,
                    session,
                    text: lead_news(team, &news, Voice::Terminal),
                    read: Vec::new(),
                    revision: Some(team.revision),
                    nudge: false,
                    reminder: false,
                });
            } else if lead_mark.news_typed_ms > 0
                && !news_heard(team, &lead_mark)
                && now - lead_mark.news_typed_ms >= REMIND_AFTER_MS
                && ready_for(&session, now, lead_mark.news_typed_ms)
            {
                if lead_mark.reminders < MAX_REMINDERS {
                    actions.push(Action::Type {
                        team: team.id.clone(),
                        party: Party::Lead,
                        session,
                        text: reminder_text(team),
                        read: Vec::new(),
                        revision: None,
                        nudge: false,
                        reminder: true,
                    });
                } else if !lead_mark.person_told {
                    actions.push(Action::TellPerson {
                        team: team.id.clone(),
                        body: unheard_note(team),
                        about: PersonNote::Unheard,
                    });
                }
            }
        }
    }

    for task in team
        .tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Blocked))
    {
        let Some(pane) = task
            .worker
            .as_ref()
            .and_then(|worker| worker.pane.as_deref())
        else {
            continue;
        };
        let party = Party::Task(task.id.clone());
        let mark = mark(&party);
        let fail = |reason: &str| Action::Fail {
            team: team.id.clone(),
            task: task.id.clone(),
            reason: reason.to_string(),
        };
        let Some(session) = sessions.session(pane) else {
            if task
                .started_at_ms
                .is_some_and(|at| now - at >= NO_SESSION_MS)
            {
                actions.push(fail(
                    "Its agent never started in its terminal. Look at its tab for the error.",
                ));
            }
            continue;
        };
        let unread = unread_for(team, &party);
        if session.asleep {
            // Put to sleep to free memory, so its end isn't a failure. It's
            // woken for what waits for it, and typed into once awake.
            let nudge_due = task.status == TaskStatus::Running
                && idle(&session).is_some_and(|at| {
                    let idle_for = now - at as i64;
                    idle_for >= NUDGE_AFTER_MS
                        && mark.nudges < MAX_NUDGES
                        && now - mark.last_typed_ms >= NUDGE_GAP_MS
                });
            if unanswered(&mark, now) {
                // Asked to wake long ago and still asleep: its tab is gone.
                actions.push(fail(
                    "Its tab was closed while it was asleep (or Dragent couldn't wake it), so it can't go on. \
                     Its checkout keeps the work: send it back to start it again.",
                ));
            } else if (!unread.is_empty() || nudge_due) && wake_due(&mark, now) {
                actions.push(Action::Wake {
                    team: team.id.clone(),
                    party: party.clone(),
                    pane: pane.to_string(),
                    reason: format!("{} has work waiting for it", task.id),
                });
            }
            continue;
        }
        if session.lost {
            actions.push(fail(
                "Its terminal closed before the task was finished (its tab was closed, or Dragent couldn't bring it back). \
                 Its checkout keeps the work: send it back to start it again.",
            ));
            continue;
        }
        if !session.active {
            actions.push(fail("Its agent exited before the task was finished. Its checkout keeps the work: send it back to start it again."));
            continue;
        }
        if session.held {
            // Waiting ends by itself. A pause doesn't: the lead decides.
            if let Some(bytes) = session
                .paused_holding
                .filter(|bytes| *bytes != mark.pause_reported)
            {
                actions.push(Action::Note {
                    team: team.id.clone(),
                    task: task.id.clone(),
                    body: format!(
                        "The worker on {id} ({}) was paused: with everything it started it holds {:.1} GB. \
                         It doesn't go on by itself. Once there is memory for it, let it run with \
                         `dragent agents resume {pane}`, or cancel the task.",
                        task.title,
                        bytes as f64 / (1u64 << 30) as f64,
                        id = task.id,
                    ),
                    mark: NoteMark::Paused(bytes),
                });
            }
            continue;
        }
        if !awake_for_typing(team, &party, &mark, &session, now, &mut actions) {
            // Just woken: it may still be starting up.
            continue;
        }
        if let Some(first) = unread.first() {
            if ready_for(&session, now, first.at_ms) {
                actions.push(Action::Type {
                    team: team.id.clone(),
                    party: party.clone(),
                    session: session.clone(),
                    text: worker_messages(task, &unread),
                    read: unread.iter().map(|message| message.id).collect(),
                    revision: None,
                    nudge: false,
                    reminder: false,
                });
            }
        } else if task.status == TaskStatus::Running
            && let Some(at) = idle(&session)
        {
            let idle_for = now - at as i64;
            if mark.nudges < MAX_NUDGES
                && idle_for >= NUDGE_AFTER_MS
                && now - mark.last_typed_ms >= NUDGE_GAP_MS
                && ready_for(&session, now, at as i64)
            {
                actions.push(Action::Type {
                    team: team.id.clone(),
                    party: party.clone(),
                    session: session.clone(),
                    text: nudge(task),
                    read: Vec::new(),
                    revision: None,
                    nudge: true,
                    reminder: false,
                });
            } else if mark.nudges >= MAX_NUDGES
                && !mark.stall_reported
                && now - mark.last_typed_ms >= STALL_AFTER_MS
                && idle_for >= NUDGE_AFTER_MS
            {
                actions.push(Action::Note {
                    team: team.id.clone(),
                    task: task.id.clone(),
                    body: format!(
                        "The worker on {} ({}) stopped without finishing or saying why, after two reminders. \
                         See its screen with `dragent tasks read {}`; tell it what to do with \
                         `dragent msg send {} …`, or cancel the task.",
                        task.id, task.title, task.id, task.id
                    ),
                    mark: NoteMark::Stall,
                });
            }
        }
        if let Some((event, at)) = &session.activity
            && event == "PermissionRequest"
            && *at != mark.permission_reported
            && now - *at as i64 >= PERMISSION_AFTER_MS
        {
            actions.push(Action::Note {
                team: team.id.clone(),
                task: task.id.clone(),
                body: if team.lead_answers_prompts {
                    format!(
                        "The worker on {id} ({}) is waiting at a permission prompt in its terminal. Read it \
                         (`dragent tasks read {id}`) and answer it when what it asks is inside the task and \
                         safe, by the rules in your playbook: `dragent tasks keys {id} KEY…`, then read the \
                         screen it returns to check. When it's risky, outside the task or you're unsure, \
                         leave it and ask the person: `dragent msg send user … --kind needs_user`.",
                        task.title,
                        id = task.id,
                    )
                } else {
                    format!(
                        "The worker on {} ({}) is waiting for an approval in its terminal. In this team the \
                         person answers those, and Dragent has notified them; if it's holding up the plan, \
                         ask them with `dragent msg send user … --kind needs_user`. To see what it asks: \
                         `dragent tasks read {}`.",
                        task.id, task.title, task.id
                    )
                },
                mark: NoteMark::Permission(*at),
            });
        }
    }
    actions
}

/// Messages to `party` that nobody has taken yet.
/// Seats that don't know their agent's conversation yet learn it once the
/// agent reported it: the lead's, and those of workers under way.
fn learn_conversations(team: &Team, sessions: &dyn Sessions, actions: &mut Vec<Action>) {
    let lead = team
        .lead
        .as_ref()
        .filter(|lead| lead.agent.as_deref() != Some(crate::team::DRAGENT_AGENT))
        .map(|lead| (Party::Lead, lead));
    let workers = team
        .tasks
        .iter()
        .filter(|task| task.under_way())
        .filter_map(|task| Some((Party::Task(task.id.clone()), task.worker.as_ref()?)));
    for (party, member) in lead.into_iter().chain(workers) {
        if member.conversation.is_some() {
            continue;
        }
        let Some(pane) = member.pane.as_deref() else {
            continue;
        };
        let Some(identity) = sessions.identity(pane) else {
            continue;
        };
        if member.agent.as_deref().is_some_and(|agent| agent != identity.agent) {
            continue;
        }
        actions.push(Action::Learn {
            team: team.id.clone(),
            party,
            pane: pane.to_string(),
            identity,
        });
    }
}

pub(crate) fn unread_for<'a>(team: &'a Team, party: &Party) -> Vec<&'a Message> {
    team.messages
        .iter()
        .filter(|message| message.to == *party && !message.read)
        .collect()
}

/// The lead's agent session: its recorded pane, else the pane of its
/// recorded terminal, whichever is live. A sleeping one counts, since it's
/// woken for what waits.
fn lead_session(team: &Team, sessions: &dyn Sessions) -> Option<(String, SessionView)> {
    let lead = team.lead.as_ref()?;
    let by_terminal = lead
        .terminal
        .as_deref()
        .and_then(|terminal| sessions.pane_of_terminal(terminal));
    lead.pane
        .clone()
        .into_iter()
        .chain(by_terminal)
        .find_map(|pane| {
            let session = sessions
                .session(&pane)
                .filter(|session| session.active || session.asleep)?;
            Some((pane, session))
        })
}

/// The lead has read the news typed to it: it waited for it, read its inbox,
/// or took it over.
fn news_heard(team: &Team, mark: &CourierMark) -> bool {
    let waited = team.cursors.get("lead").copied().unwrap_or(0);
    waited >= mark.typed_revision || mark.heard_ms >= mark.news_typed_ms
}

/// A wake for this party's sleeping agent may be requested now.
fn wake_due(mark: &CourierMark, now: i64) -> bool {
    now - mark.wake_requested_ms >= WAKE_GAP_MS
}

/// A wake was asked for so long ago that the agent isn't coming back (its
/// tab was closed while it slept).
fn unanswered(mark: &CourierMark, now: i64) -> bool {
    mark.wake_pending_ms > 0 && now - mark.wake_pending_ms >= UNREACHABLE_MS
}

/// Whether an awake `session` may be typed into as far as waking goes. A
/// woken agent still shows its activity from before it slept while it
/// starts again, so after a wake it needs a sign of life first: a hook
/// report newer than the wake, or output that settled after it. Records
/// the wake answered (`Woke`) the first time it's seen awake.
fn awake_for_typing(
    team: &Team,
    party: &Party,
    mark: &CourierMark,
    session: &SessionView,
    now: i64,
    actions: &mut Vec<Action>,
) -> bool {
    if mark.wake_pending_ms > 0 {
        actions.push(Action::Woke {
            team: team.id.clone(),
            party: party.clone(),
        });
    }
    let since = mark.wake_pending_ms.max(mark.woke_ms);
    since == 0
        || session
            .activity
            .as_ref()
            .is_some_and(|(_, at)| *at as i64 > since)
        || (now - since >= crate::agent_quiet::SILENT_MS
            && session
                .output_quiet_ms
                .is_some_and(|quiet| quiet >= crate::agent_quiet::SILENT_MS))
}

fn unwoken_note(team: &Team) -> String {
    format!(
        "The lead of {} is asleep and didn't wake when Dragent asked (its tab may have been closed). Team news \
         and your messages are waiting for it: open the lead's tab to wake it.",
        team.id
    )
}

fn reminder_text(team: &Team) -> String {
    format!(
        "[Dragent team {}] Reminder: news for the lead is still waiting, typed in a while ago and not read yet. Run `dragent tasks wait` for the details and what's ready to start.",
        team.id
    )
}

fn unheard_note(team: &Team) -> String {
    format!(
        "The lead of {} isn't picking up team news. Dragent typed it into the lead's terminal, reminded the lead {MAX_REMINDERS} times, {} minutes apart, and it still hasn't run `dragent tasks wait`. Check the lead's tab: the team's work is waiting on it.",
        team.id,
        REMIND_AFTER_MS / 60_000
    )
}

fn output_seen() -> &'static Mutex<HashMap<String, (u64, i64)>> {
    static SEEN: std::sync::OnceLock<Mutex<HashMap<String, (u64, i64)>>> =
        std::sync::OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How long `terminal`'s output has been unchanged, as seen by the courier
/// (for agents that report no hook events).
fn output_quiet_ms(sessions: &dyn Sessions, terminal: &str, now: i64) -> Option<i64> {
    let total = sessions.output_total(terminal)?;
    let mut seen = output_seen()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let entry = seen.entry(terminal.to_string()).or_insert((total, now));
    if entry.0 != total {
        *entry = (total, now);
    }
    Some(now - entry.1)
}

impl TeamStore {
    /// Record what the courier did, without a change anyone waits on.
    fn courier_done(&mut self, action: &Action, now: i64) -> Result<(), String> {
        match action {
            Action::Type {
                team,
                party,
                read,
                revision,
                nudge,
                reminder,
                ..
            } => self.update(team, |team| {
                for message in team
                    .messages
                    .iter_mut()
                    .filter(|message| read.contains(&message.id))
                {
                    message.read = true;
                }
                let mark = team.courier.entry(party.label()).or_default();
                if let Some(revision) = revision {
                    mark.typed_revision = mark.typed_revision.max(*revision);
                    // New news: its reminders and the person's note start over.
                    mark.news_typed_ms = now;
                    mark.reminders = 0;
                    mark.person_told = false;
                }
                if *reminder {
                    mark.reminders += 1;
                    mark.news_typed_ms = now;
                }
                if *nudge {
                    mark.nudges += 1;
                }
                mark.last_typed_ms = now;
                // It showed a sign of life after its wake to be typed into.
                mark.woke_ms = 0;
                Ok(())
            }),
            Action::Wake { team, party, .. } => self.update(team, |team| {
                let mark = team.courier.entry(party.label()).or_default();
                mark.wake_requested_ms = now;
                if mark.wake_pending_ms == 0 {
                    mark.wake_pending_ms = now;
                }
                Ok(())
            }),
            Action::Woke { team, party } => self.update(team, |team| {
                let mark = team.courier.entry(party.label()).or_default();
                if mark.wake_pending_ms > 0 {
                    mark.woke_ms = mark.wake_pending_ms;
                    mark.wake_pending_ms = 0;
                    mark.wake_told = false;
                }
                Ok(())
            }),
            // Only when the lead is still the one this was planned from: a
            // lead set meanwhile isn't undone.
            Action::Relead { team, from, lead } => self.update(team, |team| {
                if team
                    .lead
                    .as_ref()
                    .is_some_and(|now| now.pane == from.pane && now.terminal == from.terminal)
                {
                    team.lead = Some(lead.clone());
                }
                Ok(())
            }),
            Action::Learn {
                team,
                party,
                pane,
                identity,
            } => {
                self.learn_conversation(team, party, Some(pane), identity);
                Ok(())
            }
            Action::TellPerson { team, body, about } => {
                self.execute(TeamRequest::Send {
                    team: team.clone(),
                    from: Party::Dragent,
                    to: Party::User,
                    kind: MessageKind::NeedsUser,
                    body: body.clone(),
                    reply_to: None,
                    choices: Vec::new(),
                })?;
                self.update(team, |team| {
                    let mark = team.courier.entry(Party::Lead.label()).or_default();
                    match about {
                        PersonNote::Unheard => mark.person_told = true,
                        PersonNote::Unwoken => mark.wake_told = true,
                    }
                    Ok(())
                })
            }
            Action::Fail { team, task, reason } => self
                .execute(TeamRequest::FailTask {
                    team: team.clone(),
                    task: task.clone(),
                    reason: reason.clone(),
                })
                .map(|_| ()),
            Action::Note {
                team,
                task,
                body,
                mark,
            } => {
                self.execute(TeamRequest::Send {
                    team: team.clone(),
                    from: Party::Dragent,
                    to: Party::Lead,
                    kind: MessageKind::Finding,
                    body: body.clone(),
                    reply_to: None,
                    choices: Vec::new(),
                })?;
                self.update(team, |team| {
                    let entry: &mut CourierMark = team.courier.entry(task.clone()).or_default();
                    match mark {
                        NoteMark::Permission(at) => entry.permission_reported = *at,
                        NoteMark::Stall => entry.stall_reported = true,
                        NoteMark::Paused(bytes) => entry.pause_reported = *bytes,
                    }
                    Ok(())
                })
            }
        }
    }
}

// ── Input seen by the host ───────────────────────────────────────────────────

fn inputs() -> &'static Mutex<HashMap<String, i64>> {
    static INPUTS: std::sync::OnceLock<Mutex<HashMap<String, i64>>> = std::sync::OnceLock::new();
    INPUTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Terminal `terminal_id` just got input (a keystroke, a paste).
pub fn note_input(terminal_id: &str) {
    if let Ok(mut inputs) = inputs().lock() {
        inputs.insert(terminal_id.to_string(), now_ms());
    }
}

pub fn last_input_ms(terminal_id: &str) -> Option<i64> {
    inputs().lock().ok()?.get(terminal_id).copied()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

// ── The host's loop ──────────────────────────────────────────────────────────

/// When each worker's terminal was first found unreachable, by pane. Kept
/// in memory: a fresh host gives every terminal time to come back.
fn unreachable_since() -> &'static Mutex<HashMap<String, i64>> {
    static SINCE: std::sync::OnceLock<Mutex<HashMap<String, i64>>> = std::sync::OnceLock::new();
    SINCE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Unreachable terminals that stayed so long become ended sessions.
struct Patient<'a> {
    sessions: &'a dyn Sessions,
    now: i64,
}

impl Sessions for Patient<'_> {
    fn session(&self, pane: &str) -> Option<SessionView> {
        let mut session = self.sessions.session(pane)?;
        // Wanted for agents that send no hook reports, and for any just woken
        // (its report is from before it slept).
        if session.reachable && !session.asleep {
            session.output_quiet_ms = output_quiet_ms(self.sessions, &session.terminal_id, self.now);
        }
        let mut since = unreachable_since()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if session.reachable || session.asleep {
            since.remove(pane);
        } else if self.now - *since.entry(pane.to_string()).or_insert(self.now) >= UNREACHABLE_MS {
            session.lost = true;
        }
        Some(session)
    }

    fn type_text(&self, session: &SessionView, text: &str) -> Result<(), String> {
        self.sessions.type_text(session, text)
    }

    fn pane_of_terminal(&self, terminal: &str) -> Option<String> {
        self.sessions.pane_of_terminal(terminal)
    }

    fn identity(&self, pane: &str) -> Option<SessionIdentity> {
        self.sessions.identity(pane)
    }
}

/// One round: look at every team, then act without holding the store
/// (typing takes a moment). Returns how many things it did.
pub fn tick(sessions: &dyn Sessions) -> usize {
    let shared = crate::team::shared();
    let dir = crate::team::store_dir();
    let now = now_ms();
    let patient = Patient { sessions, now };
    // A copy, so looking at sessions doesn't hold up team requests.
    let teams: Vec<Team> = {
        let mut guard = shared
            .store
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if guard.as_ref().is_none_or(|store| store.dir != dir) {
            *guard = Some(TeamStore::open(dir.clone()));
        }
        guard
            .as_ref()
            .expect("opened")
            .teams
            .values()
            .filter(|team| team.status != TeamStatus::Stopped)
            .cloned()
            .collect()
    };
    // On a remote machine nothing else starts workers: tasks whose turn has
    // come start here (on a desktop, its window does it).
    if std::env::var_os("DRAGENT_REMOTE_HOST_BINARY").is_some() {
        for team in &teams {
            let due = crate::team_commands::due_to_start(
                team,
                now,
                &crate::team_commands::session_activity,
            );
            if !due.is_empty() {
                crate::team_runtime::start_due_in_background(team.id.clone(), due);
            }
        }
    }
    let actions: Vec<Action> = teams
        .iter()
        .flat_map(|team| plan(team, now, &patient))
        .collect();
    let mut done = 0;
    for action in actions {
        if let Action::Wake { pane, reason, .. } = &action {
            sessions.wake(pane, reason);
        }
        if let Action::Type { session, text, .. } = &action {
            if let Err(error) = sessions.type_text(session, text) {
                log::warn!(
                    "[team] couldn't type into terminal {}: {error}",
                    session.terminal_id
                );
                continue;
            }
            note_input(&session.terminal_id);
        }
        let mut guard = shared
            .store
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(store) = guard.as_mut() {
            match store.courier_done(&action, now) {
                Ok(()) => done += 1,
                Err(error) => log::warn!("[team] courier: {error}"),
            }
        }
        shared.changed.notify_all();
    }
    done
}

/// Until the host stops: a round every two seconds, or sooner when a team
/// changes.
pub fn run(sessions: &dyn Sessions, stopped: &dyn Fn() -> bool) {
    let shared = crate::team::shared();
    while !stopped() {
        tick(sessions);
        let guard = shared
            .store
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let _ = shared
            .changed
            .wait_timeout(guard, std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::{Member, TaskResult, TeamResponse};
    use std::cell::RefCell;

    struct Fake {
        sessions: RefCell<HashMap<String, SessionView>>,
        typed: RefCell<Vec<(String, String)>>,
        /// Panes asked to wake, with the reason.
        woken: RefCell<Vec<(String, String)>>,
        /// Host terminal -> the pane of the agent in it.
        terminals: RefCell<HashMap<String, String>>,
        output: RefCell<Option<u64>>,
        /// The conversation each pane's agent runs.
        identities: RefCell<HashMap<String, SessionIdentity>>,
    }

    impl Sessions for Fake {
        fn session(&self, pane: &str) -> Option<SessionView> {
            self.sessions.borrow().get(pane).cloned()
        }
        fn type_text(&self, session: &SessionView, text: &str) -> Result<(), String> {
            self.typed
                .borrow_mut()
                .push((session.terminal_id.clone(), text.to_string()));
            Ok(())
        }
        fn pane_of_terminal(&self, terminal: &str) -> Option<String> {
            self.terminals.borrow().get(terminal).cloned()
        }
        fn output_total(&self, _terminal: &str) -> Option<u64> {
            *self.output.borrow()
        }
        fn identity(&self, pane: &str) -> Option<SessionIdentity> {
            self.identities.borrow().get(pane).cloned()
        }
        fn wake(&self, pane: &str, reason: &str) {
            self.woken
                .borrow_mut()
                .push((pane.to_string(), reason.to_string()));
        }
    }

    fn view(terminal: &str, activity: Option<(&str, i64)>) -> SessionView {
        SessionView {
            workspace_id: "w".into(),
            terminal_id: terminal.into(),
            active: true,
            reachable: true,
            lost: false,
            activity: activity.map(|(event, at)| (event.to_string(), at as u64)),
            last_input_ms: None,
            asleep: false,
            held: false,
            paused_holding: None,
            output_quiet_ms: None,
        }
    }

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A team with lead pane `lead` and t1 running on worker pane `worker`.
    fn running_team() -> (Scratch, TeamStore, String) {
        let scratch = Scratch(
            std::env::temp_dir().join(format!("dragent-courier-{}", uuid::Uuid::new_v4().simple())),
        );
        let mut store = TeamStore::open(scratch.0.clone());
        let TeamResponse::Team(team) = store
            .execute(TeamRequest::Create {
                title: "Invites".into(),
                project_root: "/w".into(),
                roster: vec![],
                cost: Default::default(),
                max_parallel_per_agent: None,
                lead: Some(Member {
                    pane: Some("lead".into()),
                    terminal: None,
                    agent: Some("claude".into()),
                    ..Default::default()
                }),
            })
            .unwrap()
        else {
            panic!()
        };
        let id = team.id;
        for (title, after) in [("API", vec![]), ("Page", vec!["t1".to_string()])] {
            store
                .execute(TeamRequest::AddTask {
                    owns: Vec::new(),
                    team: id.clone(),
                    title: title.into(),
                    prompt: format!("Build the {title}"),
                    depends_on: after,
                    agent: Some("codex".into()),
                    model: None,
                    review_of: None,
                    qa: false,
                })
                .unwrap();
        }
        store
            .execute(TeamRequest::BeginTask {
                team: id.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: id.clone(),
                task: "t1".into(),
                workspace: "/w/t1".into(),
                branch: Some("api-t1".into()),
                worker: Member {
                    pane: Some("worker".into()),
                    terminal: None,
                    agent: Some("codex".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        (scratch, store, id)
    }

    fn act(store: &mut TeamStore, team: &str, now: i64, fake: &Fake) -> Vec<Action> {
        let actions = plan(store.team(team).unwrap(), now, fake);
        for action in &actions {
            if let Action::Wake { pane, reason, .. } = action {
                fake.wake(pane, reason);
            }
            if let Action::Type { session, text, .. } = action {
                fake.type_text(session, text).unwrap();
            }
            store.courier_done(action, now).unwrap();
        }
        actions
    }

    #[test]
    fn finished_work_asks_the_lead_to_verify_and_integrate_it() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::EditTask {
                team: team.clone(),
                task: "t2".into(),
                edit: crate::team::TaskEdit {
                    owns: Some(vec!["src/".into()]),
                    ..Default::default()
                },
            })
            .unwrap();
        // t1 owns nothing in particular; t2's area comes into play below.
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "API added".into(),
                    files: vec!["src/api.rs".into(), "README.md".into()],
                    ..Default::default()
                },
            })
            .unwrap();
        let current = store.team(&team).unwrap();
        let news = lead_news(&current, &lead_news_events(&current), Voice::Terminal);
        assert!(
            news.contains("t1 API is done: API added Verify it (`dragent tasks diff t1`, rerun its checks), then `dragent tasks integrate t1`."),
            "{news}"
        );
        assert!(!news.contains("outside its area"), "{news}");
        let tools = lead_news(&current, &lead_news_events(&current), Voice::Tools);
        assert!(tools.contains("coord_task_integrate (t1)"), "{tools}");

        let mut t2 = current.task("t2").unwrap().clone();
        t2.result = Some(TaskResult {
            summary: "Page built".into(),
            files: vec!["src/page.rs".into(), "README.md".into()],
            ..Default::default()
        });
        assert_eq!(t2.outside_area(), vec!["README.md".to_string()]);
    }

    #[test]
    fn qa_of_integrated_work_that_finds_problems_goes_to_the_lead() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: team.clone(),
                title: "QA everything".into(),
                prompt: "Test inviting".into(),
                depends_on: vec!["t1".into()],
                agent: Some("codex".into()),
                model: None,
                review_of: None,
                qa: true,
            })
            .unwrap();
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "API added".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        store
            .execute(TeamRequest::CancelTask {
                team: team.clone(),
                task: "t2".into(),
            })
            .unwrap();
        let current = store.team(&team).unwrap();
        assert_eq!(current.waiting_on(current.task("t3").unwrap()), vec!["t1 to be integrated"]);
        store
            .execute(TeamRequest::Integrated {
                team: team.clone(),
                task: "t1".into(),
                integration: crate::team::Integration {
                    commit: "abc".into(),
                    at_ms: i64::MAX / 4,
                    ..Default::default()
                },
            })
            .unwrap();
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().stage(&store.team(&team).unwrap()), "integrated");
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t3".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: team.clone(),
                task: "t3".into(),
                workspace: "/w/qa".into(),
                branch: Some("qa-t3".into()),
                worker: Member {
                    pane: Some("qa".into()),
                    terminal: None,
                    agent: Some("codex".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t3".into(),
                result: TaskResult {
                    summary: "Inviting twice crashes".into(),
                    verdict: Some(crate::team::Verdict::Changes),
                    ..Default::default()
                },
            })
            .unwrap();
        let current = store.team(&team).unwrap();
        assert_eq!(current.task("t3").unwrap().status, TaskStatus::Pending);
        assert_ne!(current.status, TeamStatus::Done);
        let news = lead_news(&current, &lead_news_events(&current), Voice::Terminal);
        assert!(
            news.contains("t3 found problems in the integrated work: Inviting twice crashes Send the work responsible back"),
            "{news}"
        );
    }

    fn fake_with(sessions: &[(&str, SessionView)]) -> Fake {
        Fake {
            sessions: RefCell::new(
                sessions
                    .iter()
                    .map(|(pane, view)| (pane.to_string(), view.clone()))
                    .collect(),
            ),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        }
    }

    #[test]
    fn after_a_resume_the_leads_news_goes_to_its_new_terminal() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let identity = SessionIdentity {
            conversation: "lead-conv".into(),
            agent: "claude".into(),
        };
        // The courier learns the conversation the lead runs.
        let fake = fake_with(&[("lead", view("term-1", Some(("Stop", now - 60_000))))]);
        fake.identities
            .borrow_mut()
            .insert("lead".into(), identity.clone());
        act(&mut store, &team, now, &fake);
        let seat = store.team(&team).unwrap().lead.clone().unwrap();
        assert_eq!(seat.conversation.as_deref(), Some("lead-conv"));

        // Its pane is gone; Dragent resumed the conversation in another.
        let fake = fake_with(&[("lead-2", view("term-2", Some(("Stop", now - 60_000))))]);
        let TeamResponse::Reseated(moved) = store
            .execute(TeamRequest::Reseat {
                pane: "lead-2".into(),
                terminal: None,
                identity: identity.clone(),
                at_ms: Some(now),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(moved, vec![(team.clone(), Party::Lead)]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        act(&mut store, &team, now, &fake);
        let typed = fake.typed.borrow().clone();
        assert_eq!(typed.len(), 1, "{typed:?}");
        assert_eq!(typed[0].0, "term-2");
        assert!(typed[0].1.contains("Which provider?"), "{}", typed[0].1);

        // A resume announced before that one is stale: it doesn't move it back.
        let TeamResponse::Reseated(moved) = store
            .execute(TeamRequest::Reseat {
                pane: "lead-old".into(),
                terminal: None,
                identity: identity.clone(),
                at_ms: Some(now - 1_000),
            })
            .unwrap()
        else {
            panic!()
        };
        assert!(moved.is_empty());
        let seat = store.team(&team).unwrap().lead.clone().unwrap();
        assert_eq!(seat.pane.as_deref(), Some("lead-2"));
        // Another conversation resumed elsewhere moves nothing.
        let TeamResponse::Reseated(moved) = store
            .execute(TeamRequest::Reseat {
                pane: "elsewhere".into(),
                terminal: None,
                identity: SessionIdentity {
                    conversation: "other".into(),
                    agent: "claude".into(),
                },
                at_ms: Some(now + 1_000),
            })
            .unwrap()
        else {
            panic!()
        };
        assert!(moved.is_empty());
    }

    fn person_writes(store: &mut TeamStore, team: &str, body: &str) {
        store
            .execute(TeamRequest::Send {
                team: team.to_string(),
                from: Party::User,
                to: Party::Lead,
                kind: MessageKind::Note,
                body: body.to_string(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
    }

    #[test]
    fn the_persons_message_goes_straight_in_while_the_lead_works_and_whole() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        // Mid-turn, and someone touched its terminal a few seconds ago.
        let mut lead = view("lead-term", Some(("Start", now - 1_000)));
        lead.last_input_ms = Some(now - 5_000);
        let fake = fake_with(&[("lead", lead)]);
        let body = format!("Add Claude Code as a worker. {}", "details ".repeat(200));
        person_writes(&mut store, &team, &body);

        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
        let (terminal, text) = fake.typed.borrow()[0].clone();
        assert_eq!(terminal, "lead-term");
        assert!(text.contains(body.trim()), "cut short: {text}");
        assert!(text.contains("from them"), "{text}");
        assert!(text.contains("msg reply 1"), "{text}");
        assert!(
            store.team(&team).unwrap().messages[0].read,
            "typed means delivered"
        );
        // Once.
        assert!(act(&mut store, &team, now + 60_000, &fake).is_empty());
    }

    #[test]
    fn acknowledgements_are_recognised_and_instructions_are_not() {
        for ack in ["Carry on.", "ok", "OK!", "ok no problem", "Ok, thanks", "  got it ", "👍"] {
            assert!(is_acknowledgement(ack), "{ack}");
        }
        for not in ["ok but use Resend", "Carry on with Codex instead", "stop", "why?", ""] {
            assert!(!is_acknowledgement(not), "{not}");
        }
    }

    #[test]
    fn an_acknowledgement_asks_for_no_reply_and_an_instruction_only_when_needed() {
        let (_scratch, mut store, team) = running_team();
        let fake = fake_with(&[("lead", view("lead-term", None))]);
        person_writes(&mut store, &team, "Carry on.");
        assert_eq!(act(&mut store, &team, 10_000_000, &fake).len(), 1);
        let (_, text) = fake.typed.borrow()[0].clone();
        assert!(text.contains("acknowledged") && text.contains("don't reply"), "{text}");
        assert!(!text.contains("msg reply"), "invites a reply: {text}");

        let state = store.team(&team).unwrap();
        let instruction = Message { id: 9, from: Party::User, to: Party::Lead, kind: MessageKind::Note, body: "Use Codex for t3".into(), reply_to: None, at_ms: 0, read: false, choices: Vec::new() };
        let text = person_text(state, &[&instruction], Voice::Terminal);
        assert!(text.contains("Act on it now") && text.contains("only when"), "{text}");
    }

    #[test]
    fn ok_to_a_leads_question_is_an_answer_not_an_acknowledgement() {
        let (_scratch, store, team) = running_team();
        let mut state = store.team(&team).unwrap().clone();
        state.messages.push(Message { id: 1, from: Party::Lead, to: Party::User, kind: MessageKind::NeedsUser, body: "Delete the old branch?".into(), reply_to: None, at_ms: 0, read: true, choices: Vec::new() });
        let ok = Message { id: 2, from: Party::User, to: Party::Lead, kind: MessageKind::Answer, body: "ok".into(), reply_to: Some(1), at_ms: 0, read: false, choices: Vec::new() };
        let text = person_text(&state, &[&ok], Voice::Terminal);
        assert!(text.contains("Message #2") && !text.contains("acknowledged"), "{text}");
    }

    #[test]
    fn a_wait_that_hands_over_the_persons_message_doesnt_swallow_it() {
        // A wait in a script that drops its output took the message, and the
        // courier used to count that as delivered.
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = fake_with(&[("lead", view("lead-term", Some(("Stop", now - 60_000))))]);
        person_writes(&mut store, &team, "Are you there?");
        match store
            .execute(TeamRequest::Wait {
                team: team.clone(),
                since: None,
                tasks: Vec::new(),
                party: Some(Party::Lead),
                timeout_ms: 1,
            })
            .unwrap()
        {
            TeamResponse::Changes { timed_out, .. } => assert!(!timed_out),
            other => panic!("{other:?}"),
        }
        let inbox = || TeamRequest::Inbox {
            team: team.clone(),
            party: Party::Lead,
            all: false,
            peek: false,
        };
        match store.execute(inbox()).unwrap() {
            TeamResponse::Messages(messages) => assert_eq!(messages.len(), 1),
            other => panic!("{other:?}"),
        }

        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
        assert!(fake.typed.borrow()[0].1.contains("Are you there?"));
        // Delivered now: the inbox and the courier are both done with it.
        match store.execute(inbox()).unwrap() {
            TeamResponse::Messages(messages) => assert!(messages.is_empty()),
            other => panic!("{other:?}"),
        }
        assert!(act(&mut store, &team, now + 60_000, &fake).is_empty());
    }

    #[test]
    fn a_prompt_of_its_own_holds_the_persons_message_back() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = fake_with(&[(
            "lead",
            view("lead-term", Some(("PermissionRequest", now - 60 * 60_000))),
        )]);
        person_writes(&mut store, &team, "Hold on");
        // However long it's been: Enter would answer the prompt.
        assert!(act(&mut store, &team, now, &fake).is_empty());
        assert!(!store.team(&team).unwrap().messages[0].read);

        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Start", now - 1_000))),
        );
        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
    }

    #[test]
    fn the_persons_message_comes_before_the_digest_and_stays_out_of_it() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = fake_with(&[("lead", view("lead-term", Some(("Stop", now - 60_000))))]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        person_writes(&mut store, &team, "Use Resend");

        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
        assert!(fake.typed.borrow()[0].1.contains("Use Resend"));
        // Next, once the terminal's quiet again: the news, without it.
        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Stop", now - 60_000))),
        );
        assert_eq!(act(&mut store, &team, now + 1_000, &fake).len(), 1);
        let digest = fake.typed.borrow()[1].1.clone();
        assert!(digest.contains("Which provider?"), "{digest}");
        assert!(!digest.contains("Use Resend"), "{digest}");
    }

    #[test]
    fn news_reaches_a_lead_that_stopped_without_reporting_it() {
        // Interrupted, say: `agent_quiet` notices its terminal fell silent.
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = fake_with(&[("lead", view("lead-term", Some(("Start", now - 60_000))))]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        assert!(act(&mut store, &team, now, &fake).is_empty(), "working");
        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Quiet", now - SETTLE_MS))),
        );
        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
    }

    #[test]
    fn dragents_own_lead_gets_only_the_persons_message_while_it_works() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        person_writes(&mut store, &team, "Use Resend");
        let take = |store: &mut TeamStore, busy: bool| match store
            .execute(TeamRequest::TakeLeadNews {
                team: team.clone(),
                busy,
            })
            .unwrap()
        {
            TeamResponse::News(news) => news,
            other => panic!("{other:?}"),
        };
        let first = take(&mut store, true).unwrap();
        assert!(first.contains("Use Resend"), "{first}");
        assert!(!first.contains("Which provider?"), "{first}");
        assert_eq!(take(&mut store, true), None);
        let rest = take(&mut store, false).unwrap();
        assert!(rest.contains("Which provider?"), "{rest}");
        assert!(!rest.contains("Use Resend"), "{rest}");
    }

    #[test]
    fn messages_reach_an_idle_worker_and_news_an_idle_lead_once() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Stop", now - 60_000))),
        );
        fake.sessions.borrow_mut().insert(
            "worker".into(),
            view("worker-term", Some(("Start", now - 1_000))),
        );
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Lead,
                to: Party::Task("t1".into()),
                kind: MessageKind::Answer,
                body: "Use Resend".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        // The worker is mid-turn: nothing is typed into it yet.
        assert!(act(&mut store, &team, now, &fake).is_empty());
        // Its turn ends; after the screen settles, the message goes in, once.
        fake.sessions
            .borrow_mut()
            .insert("worker".into(), view("worker-term", Some(("Stop", now))));
        assert!(
            act(&mut store, &team, now + 1_000, &fake).is_empty(),
            "settling"
        );
        let typed = act(&mut store, &team, now + SETTLE_MS, &fake);
        assert_eq!(typed.len(), 1);
        let (terminal, text) = fake.typed.borrow()[0].clone();
        assert_eq!(terminal, "worker-term");
        assert!(
            text.contains("Message #1 from the lead (answer) about t1:\nUse Resend"),
            "{text}"
        );
        assert!(text.contains("dragent msg reply 1"), "{text}");
        assert!(store.team(&team).unwrap().messages[0].read);
        assert!(
            act(&mut store, &team, now + SETTLE_MS + 2_000, &fake).is_empty(),
            "not again"
        );

        // The worker finishes: the idle lead gets one digest, not two.
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "API added; tests pass".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let later = now + 60_000;
        let news = act(&mut store, &team, later, &fake);
        assert!(
            matches!(
                &news[..],
                [Action::Type {
                    party: Party::Lead,
                    ..
                }]
            ),
            "{news:?}"
        );
        let (terminal, text) = fake.typed.borrow().last().cloned().unwrap();
        assert_eq!(terminal, "lead-term");
        assert!(
            text.contains("- t1 API is done: API added; tests pass"),
            "{text}"
        );
        assert!(!text.contains("QA"), "{text}");
        assert!(text.contains("dragent tasks wait"), "{text}");
        assert!(act(&mut store, &team, later + 20_000, &fake).is_empty());

        // The rest is cancelled: everything has finished, but nothing tested
        // the built work as a whole, so the lead hears a QA pass is due.
        store
            .execute(TeamRequest::CancelTask {
                team: team.clone(),
                task: "t2".into(),
            })
            .unwrap();
        let news = act(&mut store, &team, later + 60_000, &fake);
        assert_eq!(news.len(), 1, "{news:?}");
        let (_, text) = fake.typed.borrow().last().cloned().unwrap();
        assert!(
            text.contains("nothing has tested the whole feature") && text.contains("--qa --prompt"),
            "{text}"
        );
    }

    #[test]
    fn dragents_own_lead_takes_its_news_from_the_app_not_a_terminal() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some("lead".into()),
                    terminal: None,
                    agent: Some(crate::team::DRAGENT_AGENT.into()),
                    ..Default::default()
                },
                model: None,
            })
            .unwrap();
        assert_eq!(
            store.team(&team).unwrap().summary().dragent_lead.as_deref(),
            Some("lead")
        );
        // Even with a session that looks idle, nothing is typed into it.
        let now = 10_000_000;
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Stop", now - 60_000))),
        );
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "API added; tests pass".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(act(&mut store, &team, now + 60_000, &fake).is_empty());
        assert!(fake.typed.borrow().is_empty());

        // The app takes it, once, worded for its tools.
        let take = |store: &mut TeamStore| match store
            .execute(TeamRequest::TakeLeadNews {
                team: team.clone(),
                busy: false,
            })
            .unwrap()
        {
            TeamResponse::News(news) => news,
            other => panic!("{other:?}"),
        };
        let news = take(&mut store).unwrap();
        assert!(
            news.contains("from Dragent, not the person")
                && news.contains("- t1 API is done: API added; tests pass")
                && news.contains("coord_team_status")
                && !news.contains("dragent tasks wait"),
            "{news}"
        );
        assert_eq!(take(&mut store), None, "delivered once");
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Task("t1".into()),
                to: Party::Lead,
                kind: MessageKind::Question,
                body: "Resend or SES?".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        let news = take(&mut store).unwrap();
        assert!(
            news.contains("Resend or SES?") && !news.contains("API added"),
            "{news}"
        );
    }

    #[test]
    fn the_lead_hears_what_workers_tell_each_other() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Stop", now - 60_000))),
        );
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Task("t1".into()),
                to: Party::Task("t2".into()),
                kind: MessageKind::Handoff,
                body: "POST /invites returns {id, url}".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        let news = act(&mut store, &team, now + 60_000, &fake);
        assert!(
            news.iter().any(|action| matches!(
                action,
                Action::Type {
                    party: Party::Lead,
                    ..
                }
            )),
            "{news:?}"
        );
        let (terminal, text) = fake.typed.borrow().last().cloned().unwrap();
        assert_eq!(terminal, "lead-term");
        assert!(
            text.contains("- t1 → t2 (handoff): POST /invites returns {id, url}")
                && text.contains("Workers wrote to each other above")
                && text.contains("dragent msg send tN"),
            "{text}"
        );
        // Not marked read for the worker it was meant for.
        assert!(!store.team(&team).unwrap().messages[0].read);
    }

    #[test]
    fn nobody_is_typed_over_and_busy_agents_are_left_alone() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        let mut lead = view("lead-term", Some(("Stop", now - 60_000)));
        lead.last_input_ms = Some(now - 2_000);
        fake.sessions
            .borrow_mut()
            .insert("lead".into(), lead.clone());
        fake.sessions.borrow_mut().insert(
            "worker".into(),
            view("worker-term", Some(("PermissionRequest", now - 1_000))),
        );
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Task("t1".into()),
                to: Party::Lead,
                kind: MessageKind::Question,
                body: "Which provider?".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        // Someone typed into the lead's terminal a moment ago.
        assert!(act(&mut store, &team, now, &fake).is_empty());
        lead.last_input_ms = Some(now - QUIET_MS);
        fake.sessions.borrow_mut().insert("lead".into(), lead);
        let typed = act(&mut store, &team, now, &fake);
        assert!(
            fake.typed.borrow()[0]
                .1
                .contains("Message #1 from t1 (question): Which provider?"),
            "{typed:?}"
        );
        // A worker at an approval prompt is never typed into; the lead
        // hears of it once it's been waiting a while, and only once.
        let waited = now + PERMISSION_AFTER_MS;
        let notes = act(&mut store, &team, waited, &fake);
        assert!(
            matches!(
                &notes[..],
                [Action::Note {
                    mark: NoteMark::Permission(_),
                    ..
                }]
            ),
            "{notes:?}"
        );
        let note = store.team(&team).unwrap().messages.last().unwrap().clone();
        assert_eq!((note.from, note.to), (Party::Dragent, Party::Lead));
        // The lead answers prompts in this team unless the person said not to.
        assert!(
            note.body.contains("waiting at a permission prompt")
                && note.body.contains("`dragent tasks keys t1 KEY…`"),
            "{}",
            note.body
        );
        let again = act(&mut store, &team, waited + 1_000, &fake);
        assert!(
            again
                .iter()
                .all(|action| !matches!(action, Action::Note { .. })),
            "{again:?}"
        );
    }

    #[test]
    fn quiet_workers_are_reminded_then_reported_and_exited_ones_fail() {
        let (_scratch, mut store, team) = running_team();
        let start = 10_000_000;
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        fake.sessions
            .borrow_mut()
            .insert("worker".into(), view("worker-term", Some(("Stop", start))));
        assert!(
            act(&mut store, &team, start + 10_000, &fake).is_empty(),
            "a short pause is fine"
        );
        let first = act(&mut store, &team, start + NUDGE_AFTER_MS, &fake);
        assert!(
            matches!(&first[..], [Action::Type { nudge: true, .. }]),
            "{first:?}"
        );
        assert!(
            fake.typed.borrow()[0]
                .1
                .contains("dragent tasks done --summary")
        );
        assert!(
            act(&mut store, &team, start + NUDGE_AFTER_MS + 60_000, &fake).is_empty(),
            "spaced out"
        );
        let second_at = start + NUDGE_AFTER_MS + NUDGE_GAP_MS;
        assert_eq!(act(&mut store, &team, second_at, &fake).len(), 1);
        assert!(
            act(&mut store, &team, second_at + NUDGE_GAP_MS, &fake).is_empty(),
            "two reminders at most"
        );
        let stalled = act(&mut store, &team, second_at + STALL_AFTER_MS, &fake);
        assert!(
            matches!(
                &stalled[..],
                [Action::Note {
                    mark: NoteMark::Stall,
                    ..
                }]
            ),
            "{stalled:?}"
        );
        assert!(act(&mut store, &team, second_at + STALL_AFTER_MS + 1, &fake).is_empty());

        // The agent exits without finishing: the task fails and the lead hears.
        fake.sessions.borrow_mut().get_mut("worker").unwrap().active = false;
        act(&mut store, &team, second_at + STALL_AFTER_MS + 2, &fake);
        let task = store.team(&team).unwrap().task("t1").unwrap().clone();
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task.reason.unwrap().contains("exited"));
    }

    #[test]
    fn the_lead_hears_when_a_review_sends_work_back() {
        let (_scratch, mut store, team) = running_team();
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        let now = 10_000_000;
        fake.sessions.borrow_mut().insert(
            "lead".into(),
            view("lead-term", Some(("Stop", now - 60_000))),
        );
        store
            .execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: team.clone(),
                title: "Review API".into(),
                prompt: "Review it".into(),
                depends_on: vec![],
                agent: Some("codex".into()),
                model: None,
                review_of: Some("t1".into()),
                qa: false,
            })
            .unwrap();
        for (task, verdict) in [("t1", None), ("t3", Some(crate::team::Verdict::Changes))] {
            if task == "t3" {
                store
                    .execute(TeamRequest::BeginTask {
                        team: team.clone(),
                        task: task.into(),
                    })
                    .unwrap();
                store
                    .execute(TeamRequest::TaskStarted {
                        base: None,
                        team: team.clone(),
                        task: task.into(),
                        workspace: "/w/t1".into(),
                        branch: None,
                        worker: Member::default(),
                    })
                    .unwrap();
            }
            store
                .execute(TeamRequest::FinishTask {
                    team: team.clone(),
                    task: task.into(),
                    result: TaskResult {
                        summary: "Validate the email address".into(),
                        verdict,
                        ..Default::default()
                    },
                })
                .unwrap();
        }
        act(&mut store, &team, now, &fake);
        let text = fake.typed.borrow().last().cloned().unwrap().1;
        assert!(
            text.contains("- t3 asked for changes to t1 API: Validate the email address"),
            "{text}"
        );
        assert!(
            text.contains("it starts its next attempt on its own"),
            "{text}"
        );
        // When tasks wait for the lead, it's told how to start it.
        let mut waiting = store.team(&team).unwrap().clone();
        waiting.auto_start = false;
        // As if nothing had been delivered yet.
        waiting.courier.clear();
        waiting.cursors.clear();
        let news = lead_news_events(&waiting);
        assert!(
            lead_news(&waiting, &news, Voice::Terminal).contains("`dragent tasks start t1`"),
            "{text}"
        );
    }

    #[test]
    fn a_worker_that_never_shows_up_fails_after_a_while() {
        let (_scratch, mut store, team) = running_team();
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        let started = store
            .team(&team)
            .unwrap()
            .task("t1")
            .unwrap()
            .started_at_ms
            .unwrap();
        assert!(act(&mut store, &team, started + 60_000, &fake).is_empty());
        act(&mut store, &team, started + NO_SESSION_MS, &fake);
        assert_eq!(
            store.team(&team).unwrap().task("t1").unwrap().status,
            TaskStatus::Failed
        );
    }

    #[test]
    fn an_unreachable_terminal_counts_as_ended_only_after_a_while() {
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        let mut gone = view("t", Some(("Stop", 0)));
        gone.reachable = false;
        let pane = format!("pane-{}", uuid::Uuid::new_v4());
        fake.sessions.borrow_mut().insert(pane.clone(), gone);
        let at = |now| {
            Patient {
                sessions: &fake,
                now,
            }
            .session(&pane)
            .unwrap()
            .lost
        };
        assert!(!at(1_000));
        assert!(!at(1_000 + UNREACHABLE_MS - 1));
        assert!(at(1_000 + UNREACHABLE_MS));
    }

    #[test]
    fn an_asleep_worker_is_woken_for_its_messages_not_failed() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        // Put to sleep: its terminal is closed, so it's unreachable and not active.
        let mut worker = view("worker-term", Some(("Stop", now - 60_000)));
        worker.asleep = true;
        worker.active = false;
        worker.reachable = false;
        let fake = fake_with(&[("worker", worker)]);
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Lead,
                to: Party::Task("t1".into()),
                kind: MessageKind::Answer,
                body: "Use Resend".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();

        let first = act(&mut store, &team, now, &fake);
        assert!(matches!(&first[..], [Action::Wake { .. }]), "{first:?}");
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().status, TaskStatus::Running);
        assert_eq!(fake.woken.borrow().len(), 1);
        assert!(fake.typed.borrow().is_empty(), "nothing typed while asleep");

        // Asked again only once a minute.
        assert!(act(&mut store, &team, now + 1_000, &fake).is_empty());
        assert_eq!(act(&mut store, &team, now + WAKE_GAP_MS, &fake).len(), 1);
        assert_eq!(fake.woken.borrow().len(), 2);

        // Awake again, but still showing its report from before it slept:
        // it may still be starting, so nothing goes in yet.
        fake.sessions.borrow_mut().insert(
            "worker".into(),
            view("worker-term", Some(("Stop", now - 60_000))),
        );
        let later = now + WAKE_GAP_MS + 1_000;
        let woke = act(&mut store, &team, later, &fake);
        assert!(matches!(&woke[..], [Action::Woke { .. }]), "{woke:?}");
        assert!(fake.typed.borrow().is_empty(), "not typed into a starting agent");
        assert!(act(&mut store, &team, later + 5_000, &fake).is_empty());

        // A report after the wake: the message goes in, once, and it's still running.
        fake.sessions.borrow_mut().insert(
            "worker".into(),
            view("worker-term", Some(("Stop", later + 6_000))),
        );
        assert_eq!(act(&mut store, &team, later + 6_000 + SETTLE_MS, &fake).len(), 1);
        assert!(act(&mut store, &team, later + 60_000, &fake).is_empty(), "once");
        assert_eq!(fake.typed.borrow().len(), 1);
        let (terminal, text) = fake.typed.borrow()[0].clone();
        assert_eq!(terminal, "worker-term");
        assert!(text.contains("Use Resend"), "{text}");
        assert!(store.team(&team).unwrap().messages[0].read);
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().status, TaskStatus::Running);
    }

    /// Waiting for memory, or paused: not failed, not nudged, not typed into,
    /// however long it lasts. Its messages reach it afterwards.
    #[test]
    fn a_held_worker_is_neither_failed_nor_typed_into() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let mut worker = view("worker-term", None);
        worker.held = true;
        worker.output_quiet_ms = Some(60 * 60_000);
        let fake = fake_with(&[("worker", worker.clone())]);
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Lead,
                to: Party::Task("t1".into()),
                kind: MessageKind::Answer,
                body: "Use Resend".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        for later in [0, NO_SESSION_MS, STALL_AFTER_MS, 24 * 60 * 60_000] {
            let actions = act(&mut store, &team, now + later, &fake);
            assert!(actions.is_empty(), "{actions:?}");
        }
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().status, TaskStatus::Running);
        assert!(fake.typed.borrow().is_empty());
        worker.held = false;
        worker.output_quiet_ms = None;
        let later = now + 25 * 60 * 60_000;
        worker.activity = Some(("Stop".into(), (later - 60_000) as u64));
        fake.sessions.borrow_mut().insert("worker".into(), worker);
        act(&mut store, &team, later, &fake);
        assert_eq!(fake.typed.borrow().len(), 1);
    }

    /// A pause needs a decision, and it's the lead's: told once per pause.
    #[test]
    fn a_paused_worker_is_the_leads_to_resume() {
        let (_scratch, mut store, team) = running_team();
        let mut worker = view("worker-term", None);
        worker.held = true;
        let notes = |store: &mut TeamStore, worker: &SessionView, now| {
            let fake = fake_with(&[("worker", worker.clone())]);
            act(store, &team, now, &fake)
                .into_iter()
                .filter_map(|action| match action {
                    Action::Note { body, mark: NoteMark::Paused(_), .. } => Some(body),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // Waiting for memory needs nobody.
        assert!(notes(&mut store, &worker, 10_000_000).is_empty());
        worker.paused_holding = Some(5 << 30);
        let told = notes(&mut store, &worker, 10_001_000);
        assert_eq!(told.len(), 1);
        assert!(told[0].contains("5.0 GB") && told[0].contains("dragent agents resume worker"), "{told:?}");
        assert!(notes(&mut store, &worker, 10_002_000).is_empty(), "told once");
        // Resumed, and paused again later at more.
        worker.paused_holding = Some(8 << 30);
        assert_eq!(notes(&mut store, &worker, 10_003_000).len(), 1);
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().status, TaskStatus::Running);
    }

    #[test]
    fn an_asleep_lead_is_woken_then_gets_its_news() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let mut lead = view("lead-term", Some(("Stop", now - 60_000)));
        lead.asleep = true;
        lead.reachable = false;
        let fake = fake_with(&[("lead", lead)]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();

        let first = act(&mut store, &team, now, &fake);
        assert!(
            matches!(&first[..], [Action::Wake { party: Party::Lead, .. }]),
            "{first:?}"
        );
        assert!(fake.typed.borrow().is_empty());

        // Awake, but its output hasn't settled since the wake: wait.
        let mut awake = view("lead-term", Some(("Stop", now - 60_000)));
        awake.output_quiet_ms = Some(1_000);
        fake.sessions.borrow_mut().insert("lead".into(), awake.clone());
        let woke = act(&mut store, &team, now + 1_000, &fake);
        assert!(matches!(&woke[..], [Action::Woke { party: Party::Lead, .. }]), "{woke:?}");
        assert!(fake.typed.borrow().is_empty(), "not typed into a starting agent");

        // Its output settled after the wake (no report needed): the news goes in.
        awake.output_quiet_ms = Some(crate::agent_quiet::SILENT_MS);
        fake.sessions.borrow_mut().insert("lead".into(), awake);
        let typed = act(&mut store, &team, now + crate::agent_quiet::SILENT_MS, &fake);
        assert_eq!(typed.len(), 1, "{typed:?}");
        let (terminal, text) = fake.typed.borrow()[0].clone();
        assert_eq!(terminal, "lead-term");
        assert!(text.contains("Which provider?"), "{text}");
    }

    #[test]
    fn an_asleep_worker_that_never_wakes_fails_as_lost() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let mut worker = view("worker-term", Some(("Stop", now - 60_000)));
        worker.asleep = true;
        worker.active = false;
        worker.reachable = false;
        let fake = fake_with(&[("worker", worker)]);
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Lead,
                to: Party::Task("t1".into()),
                kind: MessageKind::Answer,
                body: "Use Resend".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        act(&mut store, &team, now, &fake);
        // Asked again each minute; the first unanswered wake still counts.
        act(&mut store, &team, now + WAKE_GAP_MS, &fake);
        assert_eq!(fake.woken.borrow().len(), 2);
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().status, TaskStatus::Running);
        assert!(!act(&mut store, &team, now + UNREACHABLE_MS - 1, &fake)
            .iter()
            .any(|action| matches!(action, Action::Fail { .. })));

        let failed = act(&mut store, &team, now + UNREACHABLE_MS, &fake);
        assert!(
            matches!(&failed[..], [Action::Fail { reason, .. }] if reason.contains("closed while it was asleep")),
            "{failed:?}"
        );
        assert_eq!(store.team(&team).unwrap().task("t1").unwrap().status, TaskStatus::Failed);
    }

    #[test]
    fn an_asleep_lead_that_never_wakes_is_told_to_the_person_once() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let mut lead = view("lead-term", Some(("Stop", now - 60_000)));
        lead.asleep = true;
        lead.active = false;
        lead.reachable = false;
        let fake = fake_with(&[("lead", lead)]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        act(&mut store, &team, now, &fake);
        let told = act(&mut store, &team, now + UNREACHABLE_MS, &fake);
        assert!(
            matches!(&told[..], [Action::TellPerson { about: PersonNote::Unwoken, .. }]),
            "{told:?}"
        );
        let notes = |store: &TeamStore| {
            store
                .team(&team)
                .unwrap()
                .messages
                .iter()
                .filter(|message| message.to == Party::User && message.kind == MessageKind::NeedsUser)
                .count()
        };
        assert_eq!(notes(&store), 1);
        // No more wakes and no second note.
        let woken = fake.woken.borrow().len();
        assert!(act(&mut store, &team, now + UNREACHABLE_MS + 10 * WAKE_GAP_MS, &fake).is_empty());
        assert_eq!(fake.woken.borrow().len(), woken);
        assert_eq!(notes(&store), 1);

        // Its tab opened again: the wake counts as answered.
        fake.sessions
            .borrow_mut()
            .insert("lead".into(), view("lead-term", Some(("Stop", now + UNREACHABLE_MS + 11 * WAKE_GAP_MS))));
        let later = now + UNREACHABLE_MS + 12 * WAKE_GAP_MS;
        let woke = act(&mut store, &team, later, &fake);
        assert!(matches!(&woke[0], Action::Woke { party: Party::Lead, .. }), "{woke:?}");
        let mark = store.team(&team).unwrap().courier[&Party::Lead.label()].clone();
        assert_eq!(mark.wake_pending_ms, 0);
        assert!(!mark.wake_told);
        assert!(fake.typed.borrow().iter().any(|(_, text)| text.contains("Which provider?")));
    }

    #[test]
    fn a_lead_moved_meanwhile_is_not_reverted_by_a_stale_relead() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        // The lead's recorded pane is gone; its terminal's agent is in "lead2".
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some("gone".into()),
                    terminal: Some("old-term".into()),
                    agent: Some("claude".into()),
                    ..Default::default()
                },
                model: None,
            })
            .unwrap();
        let fake = fake_with(&[("lead2", view("old-term", Some(("Stop", now - 60_000))))]);
        fake.terminals.borrow_mut().insert("old-term".into(), "lead2".into());
        let actions = plan(store.team(&team).unwrap(), now, &fake);
        let relead = actions
            .iter()
            .find(|action| matches!(action, Action::Relead { .. }))
            .cloned()
            .expect("follows the lead");
        // The lead is set elsewhere before the courier records its move.
        let chosen = Member {
            pane: Some("lead3".into()),
            terminal: Some("term3".into()),
            agent: Some("codex".into()),
            ..Default::default()
        };
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: chosen.clone(),
                model: None,
            })
            .unwrap();
        store.courier_done(&relead, now).unwrap();
        let lead = store.team(&team).unwrap().lead.clone().unwrap();
        assert_eq!(lead.pane, chosen.pane);
        assert_eq!(lead.terminal, chosen.terminal);
    }

    /// The lead's agent was restarted into a new terminal, and the team
    /// record still holds the old pane and terminal. The fake maps the old
    /// terminal to the live pane, which stands in for "the agent session is
    /// known by its terminal". In the host, `pane_for_terminal(old)` finds
    /// nothing once the record moved on, so there it's the recorded pane
    /// (when still live) that follows a moved lead; both resolve the same.
    #[test]
    fn a_stale_lead_pane_is_followed_by_its_terminal_and_recorded() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some("gone".into()),
                    terminal: Some("old-term".into()),
                    agent: Some("claude".into()),
                    ..Default::default()
                },
                model: None,
            })
            .unwrap();
        let now = 10_000_000;
        let fake = fake_with(&[("lead", view("new-term", Some(("Stop", now - 60_000))))]);
        fake.terminals.borrow_mut().insert("old-term".into(), "lead".into());
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();

        act(&mut store, &team, now, &fake);
        let (terminal, text) = fake.typed.borrow()[0].clone();
        assert_eq!(terminal, "new-term", "typed into the live terminal");
        assert!(text.contains("Which provider?"), "{text}");
        let lead = store.team(&team).unwrap().lead.clone().unwrap();
        assert_eq!(lead.pane.as_deref(), Some("lead"));
        assert_eq!(lead.terminal.as_deref(), Some("new-term"));
    }

    #[test]
    fn a_lead_without_hook_reports_is_typed_into_once_its_output_settles() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let mut lead = view("lead-term", None);
        lead.output_quiet_ms = Some(crate::agent_quiet::SILENT_MS - 1);
        let fake = fake_with(&[("lead", lead)]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        assert!(act(&mut store, &team, now, &fake).is_empty(), "still busy");

        let mut settled = view("lead-term", None);
        settled.output_quiet_ms = Some(crate::agent_quiet::SILENT_MS);
        fake.sessions.borrow_mut().insert("lead".into(), settled);
        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
        assert!(fake.typed.borrow()[0].1.contains("Which provider?"));
    }

    #[test]
    fn output_quiet_time_counts_from_the_last_change() {
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(Some(5)),
            identities: RefCell::new(HashMap::new()),
        };
        let terminal = format!("term-{}", uuid::Uuid::new_v4());
        assert_eq!(output_quiet_ms(&fake, &terminal, 1_000), Some(0));
        assert_eq!(
            output_quiet_ms(&fake, &terminal, 1_000 + 4_000),
            Some(4_000)
        );
        *fake.output.borrow_mut() = Some(6);
        assert_eq!(output_quiet_ms(&fake, &terminal, 6_000), Some(0));
    }

    #[test]
    fn unread_news_is_typed_again_at_most_three_times_then_the_person_is_told() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = fake_with(&[("lead", view("lead-term", Some(("Stop", now - 60_000))))]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        assert_eq!(act(&mut store, &team, now, &fake).len(), 1, "the news");

        for step in 1..=3 {
            let reminded = act(&mut store, &team, now + step * REMIND_AFTER_MS, &fake);
            assert!(
                matches!(
                    &reminded[..],
                    [Action::Type { reminder: true, party: Party::Lead, .. }]
                ),
                "reminder {step}: {reminded:?}"
            );
        }
        let (_, text) = fake.typed.borrow().last().cloned().unwrap();
        assert!(text.contains("Reminder") && text.contains("dragent tasks wait"), "{text}");
        assert_eq!(fake.typed.borrow().len(), 4);

        let told = act(&mut store, &team, now + 4 * REMIND_AFTER_MS, &fake);
        assert!(matches!(&told[..], [Action::TellPerson { .. }]), "{told:?}");
        let note = store
            .team(&team)
            .unwrap()
            .messages
            .iter()
            .rev()
            .find(|message| message.to == Party::User)
            .cloned()
            .unwrap();
        assert_eq!(note.from, Party::Dragent);
        assert_eq!(note.kind, MessageKind::NeedsUser);
        assert!(note.body.contains("isn't picking up team news"), "{}", note.body);
        assert!(act(&mut store, &team, now + 5 * REMIND_AFTER_MS, &fake).is_empty());
    }

    #[test]
    fn news_the_lead_has_read_is_not_reminded() {
        let (_scratch, mut store, team) = running_team();
        let now = 10_000_000;
        let fake = fake_with(&[("lead", view("lead-term", Some(("Stop", now - 60_000))))]);
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which provider?".into(),
            })
            .unwrap();
        assert_eq!(act(&mut store, &team, now, &fake).len(), 1);
        // It reads it with `tasks wait`.
        store
            .execute(TeamRequest::Wait {
                team: team.clone(),
                since: None,
                tasks: Vec::new(),
                party: Some(Party::Lead),
                timeout_ms: 1,
            })
            .unwrap();
        assert!(act(&mut store, &team, now + REMIND_AFTER_MS, &fake).is_empty());
        assert!(fake.typed.borrow().len() == 1);
    }

    #[test]
    fn a_message_to_a_finished_task_goes_to_the_lead_and_the_sender_is_told() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "API added".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Task("t2".into()),
                to: Party::Task("t1".into()),
                kind: MessageKind::Question,
                body: "Which schema did you use?".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        let current = store.team(&team).unwrap().clone();
        let original = current.messages.iter().find(|m| m.body.contains("schema")).unwrap();
        assert!(!original.read, "kept for the task");
        assert!(current.messages.iter().any(|m| {
            m.from == Party::Dragent
                && m.to == Party::Task("t2".into())
                && m.body == "t1 is finished, so it won't see this; your message went to the lead."
        }));
        assert!(current.messages.iter().any(|m| {
            m.from == Party::Task("t2".into())
                && m.to == Party::Lead
                && m.body.contains("Which schema did you use?")
                && m.body.contains("finished, so it won't see this")
        }));
        // The lead hears the copy as news.
        let news = lead_news(&current, &lead_news_events(&current), Voice::Terminal);
        assert!(news.contains("Which schema did you use?"), "{news}");

        // Reopened, its unread message goes into the brief of its next start,
        // and is read there (not typed a second time).
        store
            .execute(TeamRequest::ReopenTask {
                team: team.clone(),
                task: "t1".into(),
                feedback: "Use the new schema".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: team.clone(),
                task: "t1".into(),
                workspace: "/w/t1".into(),
                branch: Some("api-t1".into()),
                worker: Member {
                    pane: Some("worker2".into()),
                    terminal: None,
                    agent: Some("codex".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        let id = original.id;
        store
            .execute(TeamRequest::PromptDelivered {
                team: team.clone(),
                task: "t1".into(),
                messages: vec![id],
            })
            .unwrap();
        let message = store
            .team(&team)
            .unwrap()
            .messages
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .unwrap();
        assert!(message.read);
    }

    #[test]
    fn a_message_to_an_unstarted_task_waits_and_the_sender_is_told() {
        let (_scratch, mut store, team) = running_team();
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::Task("t1".into()),
                to: Party::Task("t2".into()),
                kind: MessageKind::Handoff,
                body: "Schema: id, url".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        let current = store.team(&team).unwrap().clone();
        let message = current
            .messages
            .iter()
            .find(|m| m.body == "Schema: id, url")
            .cloned()
            .unwrap();
        assert!(!message.read);
        assert!(current.messages.iter().any(|m| {
            m.from == Party::Dragent
                && m.to == Party::Task("t1".into())
                && m.body == "t2 hasn't started yet, so your message will be delivered when it starts."
        }));
        // Nothing is typed into a worker that doesn't exist yet, and the
        // courier leaves the message for the start.
        let fake = fake_with(&[]);
        assert!(act(&mut store, &team, 10_000_000, &fake).is_empty());
        assert!(fake.typed.borrow().is_empty());
    }

    /// The terminal timer leaves sleeping agents alone (their terminal is
    /// closed on purpose); a wake left unanswered is what fails them (see
    /// `an_asleep_worker_that_never_wakes_fails_as_lost`).
    #[test]
    fn an_asleep_agents_closed_terminal_is_not_lost_by_the_terminal_timer() {
        let fake = Fake {
            sessions: RefCell::new(HashMap::new()),
            typed: RefCell::new(Vec::new()),
            woken: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            output: RefCell::new(None),
            identities: RefCell::new(HashMap::new()),
        };
        let mut gone = view("t", Some(("Stop", 0)));
        gone.reachable = false;
        gone.asleep = true;
        let pane = format!("pane-{}", uuid::Uuid::new_v4());
        fake.sessions.borrow_mut().insert(pane.clone(), gone);
        let lost = Patient {
            sessions: &fake,
            now: 1_000 + UNREACHABLE_MS * 3,
        }
        .session(&pane)
        .unwrap()
        .lost;
        assert!(!lost);
    }
}
