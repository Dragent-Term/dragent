//! Turns a conversation into the lines the chat view draws.
//!
//! A conversation is a flat list of [`Entry`]s: your messages, the agent's
//! replies, its steps (tool calls, thoughts), cards that need you (approvals,
//! questions, plans), and turn ends. [`layout`] groups them into turns and
//! decides what each turn shows: your message, the agent's output in order
//! (replies in full, steps on the guide line, back-to-back steps under one
//! tally), the turn timer (last while the turn runs so it stays above the
//! message box, under your message once it's done), and a card listing the
//! files the turn touched.
//!
//! Pure data, no UI, so the rules are unit tested here; the phone follows the
//! same rules.

use std::collections::HashMap;

/// What kind of work a step is: picks its icon and how it is counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepKind {
    Read,
    Edit,
    Delete,
    Search,
    Execute,
    Fetch,
    Think,
    Agent,
    Plan,
    Other,
}

/// One tool call (or thought) as a line on the guide.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub id: String,
    pub kind: StepKind,
    /// "Read", "Edited", "Searched", "Running"…: what happened, or is happening.
    pub verb: Option<String>,
    /// A file the step read or changed, shown as a chip (its name).
    pub file: Option<String>,
    /// The rest of the line: a command (mono), a query, a URL, a description.
    pub label: String,
    /// The label is a command, set in a monospace face.
    pub mono: bool,
    /// What opens under the line when there is more than it shows.
    pub detail: Option<String>,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub running: bool,
    pub failed: bool,
    /// When the step started (Unix ms), for a running step's seconds.
    pub started_at: Option<i64>,
}

/// A message of yours that hasn't settled.
#[derive(Clone, Debug, PartialEq)]
pub enum YouNote {
    Sending,
    Failed(String),
    Queued,
}

/// A row of the conversation, before layout. `C` is a card only the caller
/// knows how to draw (an approval, a question, a plan).
#[derive(Clone, Debug)]
pub enum Entry<C> {
    You { id: String, text: String, at: Option<i64>, note: Option<YouNote> },
    Reply { id: String, text: String, at: Option<i64>, streaming: bool },
    Step { step: Step, at: Option<i64> },
    Card { id: String, card: C, at: Option<i64> },
    /// The agent finished a turn; `duration_ms` when it said how long it took.
    End { id: String, at: Option<i64>, duration_ms: Option<i64> },
}

impl<C> Entry<C> {
    fn at(&self) -> Option<i64> {
        match self {
            Entry::You { at, .. }
            | Entry::Reply { at, .. }
            | Entry::Step { at, .. }
            | Entry::Card { at, .. }
            | Entry::End { at, .. } => *at,
        }
    }

    fn id(&self) -> &str {
        match self {
            Entry::Step { step, .. } => &step.id,
            Entry::You { id, .. } | Entry::Reply { id, .. } | Entry::Card { id, .. } | Entry::End { id, .. } => id,
        }
    }
}

/// A file a finished turn changed, with its line counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub name: String,
    pub added: u32,
    pub removed: u32,
}

/// The turn timer: sweeping while the turn runs, then how long it took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub running: bool,
    /// Held on something only you can answer.
    pub waiting: bool,
}

#[derive(Clone, Debug)]
pub enum Line<C> {
    You { key: String, id: String, text: String, note: Option<YouNote>, turn_start: bool },
    Clock { key: String, clock: Clock },
    Reply {
        key: String,
        id: String,
        text: String,
        streaming: bool,
        /// The turn's last reply, settled: it gets Copy and its time.
        last_reply: bool,
        at: Option<i64>,
        /// Right under work (steps or the timer): sits closer.
        after_work: bool,
    },
    /// Back-to-back steps under one tally.
    Run { key: String, steps: Vec<Step>, active: bool },
    /// A step on its own.
    Step { key: String, step: Step },
    Card { key: String, card: C },
    Changes { key: String, files: Vec<ChangedFile> },
}

impl<C> Line<C> {
    pub fn key(&self) -> &str {
        match self {
            Line::You { key, .. }
            | Line::Clock { key, .. }
            | Line::Reply { key, .. }
            | Line::Run { key, .. }
            | Line::Step { key, .. }
            | Line::Card { key, .. }
            | Line::Changes { key, .. } => key,
        }
    }
}

/// Lay `entries` out as lines. `working`: the agent is on the latest turn;
/// `waiting`: and is held on something only you can answer.
pub fn layout<C: Clone>(entries: &[Entry<C>], working: bool, waiting: bool) -> Vec<Line<C>> {
    // Split into turns: each starts at a run of your messages.
    let mut turns: Vec<Vec<&Entry<C>>> = Vec::new();
    let mut current: Vec<&Entry<C>> = Vec::new();
    let mut saw_agent = false;
    for entry in entries {
        let yours = matches!(entry, Entry::You { .. });
        if yours && saw_agent {
            turns.push(std::mem::take(&mut current));
            saw_agent = false;
        }
        if !yours {
            saw_agent = true;
        }
        current.push(entry);
    }
    if !current.is_empty() {
        turns.push(current);
    }

    let mut lines: Vec<Line<C>> = Vec::new();
    let count = turns.len();
    for (turn_index, turn) in turns.into_iter().enumerate() {
        let latest = turn_index + 1 == count;
        let yours: Vec<&Entry<C>> = turn.iter().copied().filter(|e| matches!(e, Entry::You { .. })).collect();
        let work: Vec<&Entry<C>> = turn.iter().copied().filter(|e| !matches!(e, Entry::You { .. })).collect();
        let ended = work.iter().any(|e| matches!(e, Entry::End { .. }));
        let running = latest && working && !ended;
        let any_pending = yours
            .iter()
            .any(|e| matches!(e, Entry::You { note: Some(note), .. } if !matches!(note, YouNote::Failed(_))));
        let start = yours.iter().find_map(|e| e.at()).or_else(|| work.iter().find_map(|e| e.at()));
        let completed_at = if running {
            None
        } else {
            let duration = work.iter().find_map(|e| match e {
                Entry::End { duration_ms: Some(ms), .. } => Some(*ms),
                _ => None,
            });
            match (duration, start) {
                (Some(ms), Some(start)) => Some(start + ms),
                _ => work.iter().filter_map(|e| e.at()).max(),
            }
        };

        let turn_has_lines = !lines.is_empty();
        for (index, entry) in yours.iter().enumerate() {
            if let Entry::You { id, text, note, .. } = entry {
                lines.push(Line::You {
                    key: format!("you:{id}"),
                    id: id.clone(),
                    text: text.clone(),
                    note: note.clone(),
                    turn_start: index == 0 && turn_has_lines,
                });
            }
        }
        if yours.is_empty() && work.is_empty() {
            continue;
        }
        let clock_live = running || (latest && any_pending && work.is_empty());
        let clock = (!work.is_empty() || clock_live).then(|| Line::Clock {
            key: format!(
                "clock:{}",
                yours.first().map(|e| e.id().to_string()).or_else(|| work.first().map(|e| e.id().to_string())).unwrap_or_else(|| turn_index.to_string())
            ),
            clock: Clock { started_at: start, completed_at, running: clock_live, waiting: running && waiting },
        });
        // Running: the timer goes last so it sits just above the message box
        // and never scrolls away while output streams in. Finished: it moves
        // up under your message as the turn's duration.
        let mut clock = clock;
        if !clock_live {
            if let Some(line) = clock.take() {
                lines.push(line);
            }
        }

        let mut open_run: Option<usize> = None;
        let mut last_reply_at: Option<usize> = None;
        let mut files: Vec<ChangedFile> = Vec::new();
        let mut file_index: HashMap<String, usize> = HashMap::new();
        let end_run = |lines: &mut Vec<Line<C>>, open_run: &mut Option<usize>| {
            if let Some(at) = open_run.take() {
                if let Line::Run { steps, .. } = &lines[at] {
                    if steps.len() == 1 {
                        let step = steps[0].clone();
                        lines[at] = Line::Step { key: format!("step:{}", step.id), step };
                    }
                }
            }
        };
        for entry in &work {
            match entry {
                Entry::Step { step, .. } => {
                    if step.kind == StepKind::Edit && (step.added.is_some() || step.removed.is_some()) {
                        if let Some(name) = &step.file {
                            let path = step.detail.clone().unwrap_or_else(|| name.clone());
                            let at = *file_index.entry(path.clone()).or_insert_with(|| {
                                files.push(ChangedFile { path: path.clone(), name: name.clone(), added: 0, removed: 0 });
                                files.len() - 1
                            });
                            files[at].added += step.added.unwrap_or(0);
                            files[at].removed += step.removed.unwrap_or(0);
                        }
                    }
                    match open_run {
                        Some(at) => {
                            if let Line::Run { steps, .. } = &mut lines[at] {
                                steps.push(step.clone());
                            }
                        }
                        None => {
                            lines.push(Line::Run { key: format!("run:{}", step.id), steps: vec![step.clone()], active: false });
                            open_run = Some(lines.len() - 1);
                        }
                    }
                }
                Entry::End { .. } => {}
                Entry::Reply { id, text, at, streaming } => {
                    end_run(&mut lines, &mut open_run);
                    let after_work = matches!(lines.last(), Some(Line::Run { .. } | Line::Step { .. } | Line::Clock { .. }));
                    lines.push(Line::Reply {
                        key: format!("reply:{id}"),
                        id: id.clone(),
                        text: text.clone(),
                        streaming: *streaming,
                        last_reply: false,
                        at: *at,
                        after_work,
                    });
                    last_reply_at = Some(lines.len() - 1);
                }
                Entry::Card { id, card, .. } => {
                    end_run(&mut lines, &mut open_run);
                    lines.push(Line::Card { key: format!("card:{id}"), card: card.clone() });
                }
                Entry::You { .. } => {}
            }
        }
        if running {
            if let Some(at) = open_run {
                if at + 1 == lines.len() {
                    if let Line::Run { active, .. } = &mut lines[at] {
                        *active = true;
                    }
                }
            }
        }
        end_run(&mut lines, &mut open_run);
        if let Some(line) = clock {
            lines.push(line);
        }

        if !running {
            if let Some(at) = last_reply_at {
                if let Line::Reply { streaming: false, last_reply, .. } = &mut lines[at] {
                    *last_reply = true;
                }
            }
            if !files.is_empty() {
                let card = Line::Changes {
                    key: format!("changes:{}", yours.first().map(|e| e.id().to_string()).unwrap_or_else(|| turn_index.to_string())),
                    files,
                };
                match last_reply_at {
                    Some(at) => lines.insert(at + 1, card),
                    None => lines.push(card),
                }
            }
        }
    }
    lines
}

/// "6 commands · 3 edits · 2 reads": a tally of what a group of steps did.
pub fn run_summary(steps: &[Step]) -> String {
    const ORDER: &[(&[StepKind], &str, &str)] = &[
        (&[StepKind::Execute], "command", "commands"),
        (&[StepKind::Edit, StepKind::Delete], "edit", "edits"),
        (&[StepKind::Read], "read", "reads"),
        (&[StepKind::Search], "search", "searches"),
        (&[StepKind::Fetch], "fetch", "fetches"),
        (&[StepKind::Agent, StepKind::Plan, StepKind::Other], "other step", "other steps"),
    ];
    let parts: Vec<String> = ORDER
        .iter()
        .filter_map(|(kinds, one, many)| {
            let count = steps.iter().filter(|s| kinds.contains(&s.kind)).count();
            (count > 0).then(|| format!("{count} {}", if count == 1 { one } else { many }))
        })
        .collect();
    // A group made only of thoughts has nothing to count.
    if parts.is_empty() { "Reasoning".into() } else { parts.join(" · ") }
}

/// The kind most of a run's steps are (its icon).
pub fn dominant_kind(steps: &[Step]) -> StepKind {
    let mut counts: Vec<(StepKind, usize)> = Vec::new();
    for step in steps.iter().filter(|s| s.kind != StepKind::Think) {
        match counts.iter_mut().find(|(k, _)| *k == step.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((step.kind, 1)),
        }
    }
    counts.into_iter().fold((StepKind::Other, 0), |best, c| if c.1 > best.1 { c } else { best }).0
}

fn span(seconds: i64) -> String {
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m {}s", seconds / 60, seconds % 60) }
}

/// "Working · 12s" while running, "Done in 1m 3s" after, "Waiting on you" when held.
pub fn clock_label(clock: &Clock, now_ms: i64) -> Option<String> {
    if clock.waiting {
        return Some("Waiting on you".into());
    }
    let Some(start) = clock.started_at else {
        return clock.running.then(|| "Working".into());
    };
    let end = if clock.running { Some(now_ms) } else { clock.completed_at };
    let Some(end) = end else {
        return clock.running.then(|| "Working".into());
    };
    let seconds = ((end - start) / 1000).max(0);
    if !clock.running && seconds == 0 {
        return None;
    }
    Some(if clock.running { format!("Working · {}", span(seconds)) } else { format!("Done in {}", span(seconds)) })
}

/// "12s", "1m 3s": how long a step has been running.
pub fn running_time(started_at: i64, now_ms: i64) -> String {
    span(((now_ms - started_at) / 1000).max(0))
}

/// The first non-empty line of `text`, cut to `max` characters.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > max {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, kind: StepKind) -> Step {
        Step {
            id: id.into(),
            kind,
            verb: None,
            file: None,
            label: id.into(),
            mono: false,
            detail: None,
            added: None,
            removed: None,
            running: false,
            failed: false,
            started_at: None,
        }
    }
    fn you(id: &str, at: Option<i64>) -> Entry<()> {
        Entry::You { id: id.into(), text: id.into(), at, note: None }
    }
    fn reply(id: &str, at: Option<i64>) -> Entry<()> {
        Entry::Reply { id: id.into(), text: id.into(), at, streaming: false }
    }
    fn work(id: &str, kind: StepKind) -> Entry<()> {
        Entry::Step { step: step(id, kind), at: None }
    }
    fn shape(lines: &[Line<()>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match line {
                Line::Run { steps, active, .. } => format!("run({}{})", steps.len(), if *active { "*" } else { "" }),
                Line::Step { step, .. } => format!("step:{}", step.id),
                Line::Reply { id, last_reply, .. } => format!("reply:{id}{}", if *last_reply { "!" } else { "" }),
                Line::You { id, .. } => format!("you:{id}"),
                Line::Changes { files, .. } => format!("changes({})", files.len()),
                Line::Clock { .. } => "clock".into(),
                Line::Card { .. } => "card".into(),
            })
            .collect()
    }

    #[test]
    fn a_finished_turn_groups_steps_and_marks_its_last_reply() {
        let lines = layout(
            &[you("q", None), reply("a1", None), work("1", StepKind::Read), work("2", StepKind::Execute), reply("a2", None), work("3", StepKind::Execute), reply("a3", None)],
            false,
            false,
        );
        assert_eq!(shape(&lines), ["you:q", "clock", "reply:a1", "run(2)", "reply:a2", "step:3", "reply:a3!"]);
    }

    #[test]
    fn while_working_the_trailing_run_is_active_and_the_timer_is_last() {
        let lines = layout(&[you("q", None), work("1", StepKind::Execute), work("2", StepKind::Execute)], true, false);
        assert_eq!(shape(&lines), ["you:q", "run(2*)", "clock"]);
        let entries = [you("q", Some(1_000)), reply("a1", Some(2_000)), work("1", StepKind::Execute), reply("a2", Some(9_000))];
        assert_eq!(shape(&layout(&entries, true, false)), ["you:q", "reply:a1", "step:1", "reply:a2", "clock"]);
        assert_eq!(shape(&layout(&entries, false, false)), ["you:q", "clock", "reply:a1", "step:1", "reply:a2!"]);
    }

    #[test]
    fn a_finished_turn_that_changed_files_lists_them_after_its_last_reply() {
        let edit = |id: &str, path: &str, added: u32, removed: u32| Entry::Step {
            step: Step {
                file: Some(path.rsplit('/').next().unwrap().into()),
                detail: Some(path.into()),
                added: Some(added),
                removed: Some(removed),
                ..step(id, StepKind::Edit)
            },
            at: None,
        };
        let lines = layout(&[you("q", None), edit("1", "/x/a.ts", 3, 1), edit("2", "/x/a.ts", 2, 0), edit("3", "/x/b.ts", 10, 0), reply("done", None)], false, false);
        assert_eq!(shape(&lines), ["you:q", "clock", "run(3)", "reply:done!", "changes(2)"]);
        let Line::Changes { files, .. } = &lines[4] else { panic!() };
        let view: Vec<_> = files.iter().map(|f| (f.name.as_str(), f.added, f.removed)).collect();
        assert_eq!(view, [("a.ts", 5, 1), ("b.ts", 10, 0)]);
    }

    #[test]
    fn the_timer_reads_plainly() {
        let lines = layout(&[you("q", Some(1_000)), reply("a", Some(64_000))], false, false);
        let Line::Clock { clock, .. } = &lines[1] else { panic!() };
        assert_eq!(clock_label(clock, 0).as_deref(), Some("Done in 1m 3s"));
        let live = layout(&[you("q", Some(1_000))], true, false);
        let Line::Clock { clock, .. } = &live[1] else { panic!() };
        assert_eq!(clock_label(clock, 13_500).as_deref(), Some("Working · 12s"));
        let held = layout(&[you("q", Some(1_000)), reply("a", None)], true, true);
        let Line::Clock { clock, .. } = &held[2] else { panic!() };
        assert_eq!(clock_label(clock, 5_000).as_deref(), Some("Waiting on you"));
    }

    #[test]
    fn a_group_is_tallied_by_what_it_did() {
        let steps = [step("1", StepKind::Execute), step("2", StepKind::Execute), step("3", StepKind::Edit), step("4", StepKind::Read)];
        assert_eq!(run_summary(&steps), "2 commands · 1 edit · 1 read");
        assert_eq!(run_summary(&[step("t", StepKind::Think)]), "Reasoning");
        assert_eq!(dominant_kind(&steps), StepKind::Execute);
        assert_eq!(first_line("\n  hello\nworld", 120), "hello");
        assert_eq!(first_line("abcdef", 4), "abc…");
    }
}
