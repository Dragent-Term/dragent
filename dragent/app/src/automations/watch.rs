//! Automations started by what shows up in a connected service. At each
//! scheduled time the run first searches the service; it goes on only
//! when the search finds items it hadn't found before, and they all go to
//! its agent at once. A check that finds nothing leaves no trace in the
//! history (one started by hand says so).
//!
//! The first check notes what's already there instead of working on it,
//! and a day's runs are capped, so a busy service can't start an agent
//! every few minutes.

use std::collections::VecDeque;
use std::path::PathBuf;

use dragent_automations::{
    codes, Run, RunError, RunStatus, RunStep, Scheduler, Store, Trigger, Watch,
};
use serde::{Deserialize, Serialize};

use crate::integrations::items::{self, Item};
use crate::integrations::Provider;

/// Runs a watching automation may start in a day.
pub(crate) const DAILY_LIMIT: u32 = 10;
/// Items listed in a run's prompt; the rest are counted.
const LISTED: usize = 10;
/// Ids remembered per automation (the oldest are forgotten first).
const REMEMBERED: usize = 1000;
/// Text of each item in the prompt.
const ITEM_CHARS: usize = 1500;

/// What an automation's watch has seen, kept in the store's `meta`.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Seen {
    /// The first check happened.
    #[serde(default)]
    primed: bool,
    #[serde(default)]
    ids: VecDeque<String>,
    /// The day `runs` counts (`YYYY-MM-DD`, local).
    #[serde(default)]
    day: String,
    #[serde(default)]
    runs: u32,
    /// The day's limit was already reported.
    #[serde(default)]
    limit_noted: bool,
}

impl Seen {
    fn remember(&mut self, items: &[Item]) {
        for item in items {
            let id = item.id().to_string();
            if !self.ids.contains(&id) {
                self.ids.push_back(id);
            }
        }
        while self.ids.len() > REMEMBERED {
            self.ids.pop_front();
        }
    }
}

fn key(automation_id: &str) -> String {
    format!("watch:{automation_id}")
}

fn load(store: &Store, automation_id: &str) -> Seen {
    store
        .meta(&key(automation_id))
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(store: &Store, automation_id: &str, seen: &Seen) {
    if let Ok(text) = serde_json::to_string(seen) {
        if let Err(error) = store.set_meta(&key(automation_id), &text) {
            log::warn!("[automations] couldn't note what {automation_id} saw: {error:#}");
        }
    }
}

#[derive(Debug, PartialEq)]
enum Checked {
    /// The first check: the items there now are noted, not worked on.
    Primed(usize),
    Nothing,
    /// The day's runs are used up; `first`: this is the day's first time.
    Limited {
        first: bool,
    },
    Found(Vec<Item>),
}

/// Compare `found` with what was seen before, and note it.
fn compare(seen: &mut Seen, found: Vec<Item>, today: &str) -> Checked {
    if seen.day != today {
        seen.day = today.to_string();
        seen.runs = 0;
        seen.limit_noted = false;
    }
    if !seen.primed {
        seen.primed = true;
        seen.remember(&found);
        return Checked::Primed(found.len());
    }
    let new: Vec<Item> = found
        .into_iter()
        .filter(|item| !seen.ids.iter().any(|id| id == item.id()))
        .collect();
    if new.is_empty() {
        return Checked::Nothing;
    }
    if seen.runs >= DAILY_LIMIT {
        // Not noted as seen: they're still new tomorrow.
        let first = !seen.limit_noted;
        seen.limit_noted = true;
        return Checked::Limited { first };
    }
    seen.runs += 1;
    seen.remember(&new);
    Checked::Found(new)
}

/// The prompt a run gets for the items it found.
fn prompt(prompt: &str, provider: Provider, query: &str, items: &[Item]) -> String {
    let searched = if query.trim().is_empty() {
        String::new()
    } else {
        format!(" for “{}”", query.trim())
    };
    let mut out = format!(
        "{}\n\n## New in {}{searched}\n\nThese showed up since the last check. Dragent's \
         integration tools (or `dragent integrations show {} ID`) read any of them in full.",
        prompt.trim(),
        provider.label(),
        provider.key(),
    );
    for item in items.iter().take(LISTED) {
        let key = if item.key.is_empty() {
            String::new()
        } else {
            format!("{}: ", item.key)
        };
        out.push_str(&format!("\n\n### {key}{}\n{}", item.title, item.url));
        if item.id() != item.key {
            out.push_str(&format!("\nID for the integration tools: {}", item.id()));
        }
        if let Some(detail) = &item.detail {
            out.push_str(&format!("\n{detail}"));
        }
        let text = item.body.trim();
        if !text.is_empty() {
            let mut text: String = text.chars().take(ITEM_CHARS).collect();
            if item.body.trim().chars().count() > ITEM_CHARS {
                text.push('…');
            }
            out.push_str(&format!("\n\n{text}"));
        }
    }
    if items.len() > LISTED {
        out.push_str(&format!(
            "\n\nAnd {} more: search {} for them with the integration tools.",
            items.len() - LISTED,
            provider.label()
        ));
    }
    out
}

/// Where code hosts look for the repository.
fn root(run: &Run) -> PathBuf {
    if run.snapshot.is_session() {
        dirs::home_dir().unwrap_or_default()
    } else {
        run.snapshot.project_root.clone()
    }
}

fn skip(
    store: &Store,
    scheduler: &Scheduler,
    run: &Run,
    code: &str,
    message: String,
) -> Result<(), RunError> {
    scheduler
        .mark_skipped(
            store,
            &run.id,
            RunError::new(RunStep::Queue, code).with_message(message),
            chrono::Utc::now().timestamp_millis(),
        )
        .map(|_| ())
        .map_err(|error| RunError::new(RunStep::Run, "store").with_message(format!("{error:#}")))
}

/// A scheduled check that comes to nothing isn't history.
fn forget(store: &Store, run: &Run) {
    if let Err(error) = store.delete_run(&run.id) {
        log::warn!("[automations] couldn't drop check {}: {error:#}", run.id);
    }
}

/// Search the watched service for `run`. `Some`: the run goes on, its
/// prompt now listing what's new. `None`: it's over (nothing new, or the
/// day's runs are used up).
pub(crate) fn check(
    run: &Run,
    watch: &Watch,
    store: &Store,
    scheduler: &Scheduler,
) -> Result<Option<Run>, RunError> {
    let by_hand = run.trigger == Trigger::Manual;
    let provider = Provider::from_key(&watch.service).ok_or_else(|| {
        RunError::new(RunStep::Run, codes::WATCH_FAILED).with_message(format!(
            "Dragent doesn't know the service “{}”.",
            watch.service
        ))
    })?;
    let found = match items::search(provider, &root(run), &watch.query) {
        Ok(found) => found,
        Err(error) => {
            // Said once, not every few minutes.
            let again = store
                .latest_run(&run.automation_id)
                .ok()
                .flatten()
                .filter(|latest| latest.id != run.id)
                .and_then(|latest| latest.error)
                .is_some_and(|previous| previous.code == codes::WATCH_FAILED);
            if again && !by_hand {
                forget(store, run);
                return Ok(None);
            }
            return Err(RunError::new(RunStep::Run, codes::WATCH_FAILED).with_message(error));
        }
    };
    let mut seen = load(store, &run.automation_id);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let checked = compare(&mut seen, found, &today);
    save(store, &run.automation_id, &seen);
    match checked {
        Checked::Primed(count) => {
            if by_hand {
                skip(store, scheduler, run, codes::NOTHING_NEW, format!(
                    "Watching {} now: the {count} items there are noted, and what shows up next starts a run.",
                    provider.label()
                ))?;
            } else {
                forget(store, run);
            }
            Ok(None)
        }
        Checked::Nothing => {
            if by_hand {
                skip(
                    store,
                    scheduler,
                    run,
                    codes::NOTHING_NEW,
                    "Nothing new since the last check.".into(),
                )?;
            } else {
                forget(store, run);
            }
            Ok(None)
        }
        Checked::Limited { first } => {
            if first || by_hand {
                skip(store, scheduler, run, codes::WATCH_LIMIT, format!(
                    "It started {DAILY_LIMIT} runs today, its limit: what's new waits until tomorrow."
                ))?;
            } else {
                forget(store, run);
            }
            Ok(None)
        }
        Checked::Found(new) => {
            let text = prompt(&run.snapshot.prompt, provider, &watch.query, &new);
            store
                .transition(&run.id, &[RunStatus::PreparingWorkspace], |run| {
                    run.snapshot.prompt = text;
                })
                .map_err(|error| {
                    RunError::new(RunStep::Run, "store").with_message(format!("{error:#}"))
                })
        }
    }
}

/// How a watching automation's schedule reads: "Checks Linear every 5
/// minutes".
pub(crate) fn describe(watch: &Watch, schedule: &str) -> String {
    let service = Provider::from_key(&watch.service)
        .map(Provider::label)
        .unwrap_or(watch.service.as_str());
    let every = interval_minutes(schedule)
        .map(|minutes| match minutes {
            60 => "every hour".to_string(),
            1 => "every minute".to_string(),
            minutes => format!("every {minutes} minutes"),
        })
        .unwrap_or_else(|| format!("on the schedule “{schedule}”"));
    format!("Checks {service} {every}")
}

/// Minutes between checks for the intervals the form offers.
pub(crate) const INTERVALS: [u32; 4] = [5, 15, 30, 60];

pub(crate) fn interval_schedule(minutes: u32) -> String {
    if minutes >= 60 {
        "0 * * * *".to_string()
    } else {
        format!("*/{minutes} * * * *")
    }
}

/// The interval a schedule repeats at, when it's one of the simple ones.
pub(crate) fn interval_minutes(schedule: &str) -> Option<u32> {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    match fields.as_slice() {
        ["0", "*", "*", "*", "*"] => Some(60),
        ["*", "*", "*", "*", "*"] => Some(1),
        [minute, "*", "*", "*", "*"] => minute.strip_prefix("*/")?.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> Item {
        Item {
            provider: Provider::Linear,
            key: id.to_string(),
            title: format!("Issue {id}"),
            url: format!("https://linear.app/a/issue/{id}"),
            body: "Steps to reproduce".into(),
            detail: Some("Engineering · Todo".into()),
            branch: String::new(),
            reference: None,
            state: None,
            assignee: None,
            labels: Vec::new(),
            updated_at_ms: None,
        }
    }

    #[test]
    fn the_first_check_notes_what_is_there_and_later_ones_find_only_the_new() {
        let mut seen = Seen::default();
        assert_eq!(
            compare(&mut seen, vec![item("A-1"), item("A-2")], "2026-10-24"),
            Checked::Primed(2)
        );
        assert_eq!(
            compare(&mut seen, vec![item("A-1"), item("A-2")], "2026-10-24"),
            Checked::Nothing
        );
        let Checked::Found(new) = compare(&mut seen, vec![item("A-3"), item("A-1")], "2026-10-24")
        else {
            panic!("A-3 is new");
        };
        assert_eq!(
            new.iter().map(|item| item.key.as_str()).collect::<Vec<_>>(),
            ["A-3"]
        );
        assert_eq!(
            compare(&mut seen, vec![item("A-3")], "2026-10-24"),
            Checked::Nothing
        );
    }

    #[test]
    fn a_days_runs_are_capped_and_what_waits_is_still_new_tomorrow() {
        let mut seen = Seen {
            primed: true,
            ..Seen::default()
        };
        for n in 0..DAILY_LIMIT {
            assert!(matches!(
                compare(&mut seen, vec![item(&format!("B-{n}"))], "2026-10-24"),
                Checked::Found(_)
            ));
        }
        assert_eq!(
            compare(&mut seen, vec![item("B-99")], "2026-10-24"),
            Checked::Limited { first: true }
        );
        assert_eq!(
            compare(&mut seen, vec![item("B-99")], "2026-10-24"),
            Checked::Limited { first: false }
        );
        assert!(matches!(
            compare(&mut seen, vec![item("B-99")], "2026-10-25"),
            Checked::Found(_)
        ));
    }

    #[test]
    fn what_is_remembered_stays_bounded() {
        let mut seen = Seen::default();
        let many: Vec<Item> = (0..REMEMBERED + 5)
            .map(|n| item(&format!("C-{n}")))
            .collect();
        seen.remember(&many);
        assert_eq!(seen.ids.len(), REMEMBERED);
        assert_eq!(seen.ids.front().map(String::as_str), Some("C-5"));
    }

    #[test]
    fn the_prompt_lists_what_is_new() {
        let mut items: Vec<Item> = (1..=12).map(|n| item(&format!("ENG-{n}"))).collect();
        items[0].reference = Some("uuid-1".into());
        let text = prompt("Triage new bugs.", Provider::Linear, "label:bug", &items);
        assert!(text.starts_with("Triage new bugs.\n\n## New in Linear for “label:bug”"));
        assert!(text.contains("### ENG-1: Issue ENG-1\nhttps://linear.app/a/issue/ENG-1\nID for the integration tools: uuid-1"));
        assert!(text.contains("### ENG-10:") && !text.contains("### ENG-11:"));
        assert!(text.ends_with("And 2 more: search Linear for them with the integration tools."));
    }

    #[test]
    fn intervals_read_back_from_their_schedules() {
        for minutes in INTERVALS {
            assert_eq!(interval_minutes(&interval_schedule(minutes)), Some(minutes));
        }
        assert_eq!(interval_minutes("0 9 * * *"), None);
        let watch = Watch {
            service: "sentry".into(),
            query: String::new(),
        };
        assert_eq!(
            describe(&watch, "*/15 * * * *"),
            "Checks Sentry every 15 minutes"
        );
        assert_eq!(describe(&watch, "0 * * * *"), "Checks Sentry every hour");
    }
}
