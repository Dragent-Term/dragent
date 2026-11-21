//! What the person writes to the lead, in a real host with a mock agent in a
//! real terminal: it goes in at once, even mid-turn and even after a `wait`
//! picked it up. (A file of its own: the harness sets process-wide paths, so
//! one test runs per binary.)

mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use dragent_host::agent_sessions;
use dragent_host::team::{Member, MessageKind, Party, TeamRequest, TeamResponse};
use support::{wait, Harness};
use uuid::Uuid;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The agent in `pane` reports working, as its hooks would.
fn report_working(pane: Uuid) {
    let record = agent_sessions::load(pane).unwrap();
    let activity = serde_json::json!({
        "generation": record.generation,
        "conversation_id": record.conversation_id.unwrap(),
        "event": "Start",
        "at_ms": now_ms(),
    });
    let dir = dragent_paths::organisation_dir()
        .join("agent-sessions")
        .join(pane.to_string());
    std::fs::write(dir.join("activity.json"), activity.to_string()).unwrap();
    assert_eq!(agent_sessions::load_activity(pane).unwrap().event, "Start");
}

#[test]
fn what_the_person_writes_reaches_a_working_lead_at_once() {
    let harness = Harness::new();
    let (lead, lead_terminal) = harness.launch("claude");
    let client = &harness.client;
    let TeamResponse::Team(team) = client
        .team(TeamRequest::Create {
            title: "Invites".into(),
            project_root: harness.dir.display().to_string(),
            roster: vec![],
            cost: Default::default(),
            max_parallel_per_agent: None,
            lead: Some(Member {
                pane: Some(lead.to_string()),
                terminal: None,
                agent: Some("claude".into()),
            }),
        })
        .unwrap()
    else {
        panic!()
    };
    let team = team.id;
    report_working(lead);
    client
        .team(TeamRequest::Send {
            team: team.clone(),
            from: Party::User,
            to: Party::Lead,
            kind: MessageKind::Note,
            body: "Add Claude Code as a worker".into(),
            reply_to: None,
            choices: Vec::new(),
        })
        .unwrap();
    // A wait takes it first, and its output may never reach the lead.
    client
        .team(TeamRequest::Inbox {
            team: team.clone(),
            party: Party::Lead,
            all: false,
            peek: false,
        })
        .unwrap();
    wait(|| {
        harness
            .output(&lead_terminal)
            .contains("|Add Claude Code as a worker")
            .then_some(())
    });
    wait(|| {
        match client
            .team(TeamRequest::Get { team: team.clone() })
            .unwrap()
        {
            TeamResponse::Team(saved) => saved.messages[0].read.then_some(()),
            _ => None,
        }
    });
}
