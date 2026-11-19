//! An agent that stops without reporting it, in a real host with a mock
//! agent in a real terminal: reported as working, its terminal falls silent,
//! and it stops counting as working. (A file of its own: the harness sets
//! process-wide paths, so one test runs per binary.)

mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use dragent_host::agent_sessions::{self, ACTIVITY_QUIET};
use support::{wait, Harness};
use uuid::Uuid;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// What the agent in `pane` reports doing, as its hooks would.
fn report(pane: Uuid, event: &str) {
    let record = agent_sessions::load(pane).unwrap();
    let activity = serde_json::json!({
        "generation": record.generation,
        "conversation_id": record.conversation_id.unwrap(),
        "event": event,
        "at_ms": now_ms(),
    });
    let dir = dragent_paths::organisation_dir()
        .join("agent-sessions")
        .join(pane.to_string());
    std::fs::write(dir.join("activity.json"), activity.to_string()).unwrap();
}

#[test]
fn a_silent_working_agent_stops_counting_as_working_but_a_prompt_stays() {
    let harness = Harness::new();
    let (working, _) = harness.launch("claude");
    let (asking, _) = harness.launch("claude");
    report(working, "Start");
    report(asking, "PermissionRequest");

    wait(|| (agent_sessions::load_activity(working)?.event == ACTIVITY_QUIET).then_some(()));
    assert_eq!(
        agent_sessions::load_activity(asking).unwrap().event,
        "PermissionRequest",
        "a prompt is silent while it waits for the person"
    );

    // Its next report takes over again.
    report(working, "Start");
    assert_eq!(
        agent_sessions::load_activity(working).unwrap().event,
        "Start"
    );

    // What the person has seen is kept on disk, and only set once on its own.
    assert_eq!(agent_sessions::activity_seen_or(working, 100), 100);
    assert_eq!(agent_sessions::activity_seen_or(working, 999), 100);
    agent_sessions::mark_activity_seen(working, 500);
    assert_eq!(agent_sessions::activity_seen_or(working, 999), 500);
}
