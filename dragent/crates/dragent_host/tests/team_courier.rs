//! The team courier in a real host, with mock agents in real terminals:
//! a message for a worker waits while it's mid-turn and is typed in once
//! its turn ends; a finished task reaches the idle lead as a digest; one
//! worker's message to another is typed into that worker, and the lead
//! hears it too.

mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dragent_host::agent_sessions;
use dragent_host::team::{Member, MessageKind, Party, TaskResult, TeamRequest, TeamResponse};
use support::{Harness, wait};
use uuid::Uuid;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// What the agent in `pane` reports doing, as its hooks would.
fn report(pane: Uuid, event: &str, at_ms: u64) {
    let record = agent_sessions::load(pane).unwrap();
    let activity = serde_json::json!({
        "generation": record.generation,
        "conversation_id": record.conversation_id.unwrap(),
        "event": event,
        "at_ms": at_ms,
    });
    let dir = dragent_paths::organisation_dir()
        .join("agent-sessions")
        .join(pane.to_string());
    std::fs::write(dir.join("activity.json"), activity.to_string()).unwrap();
    assert_eq!(agent_sessions::load_activity(pane).unwrap().event, event);
}

#[test]
fn the_courier_waits_for_a_turn_to_end_then_types_it_in() {
    let harness = Harness::new();
    let (worker, worker_terminal) = harness.launch("claude");
    let (lead, lead_terminal) = harness.launch("claude");
    let (second, second_terminal) = harness.launch("claude");
    let client = &harness.client;
    let member = |pane: Uuid| Member {
        pane: Some(pane.to_string()),
        terminal: None,
        agent: Some("claude".into()),
    };
    let TeamResponse::Team(team) = client
        .team(TeamRequest::Create {
            title: "Invites".into(),
            project_root: harness.dir.display().to_string(),
            roster: vec![],
            cost: Default::default(),
            max_parallel_per_agent: None,
            lead: Some(member(lead)),
        })
        .unwrap()
    else {
        panic!()
    };
    let team = team.id;
    client
        .team(TeamRequest::AddTask {
            owns: Vec::new(),
            team: team.clone(),
            title: "Invite API".into(),
            prompt: "Add POST /invites".into(),
            depends_on: vec![],
            agent: Some("claude".into()),
            model: None,
            review_of: None,
            qa: false,
        })
        .unwrap();
    client
        .team(TeamRequest::BeginTask {
            team: team.clone(),
            task: "t1".into(),
        })
        .unwrap();
    client
        .team(TeamRequest::TaskStarted {
            base: None,
            team: team.clone(),
            task: "t1".into(),
            workspace: harness.dir.display().to_string(),
            branch: None,
            worker: member(worker),
        })
        .unwrap();

    // The worker is mid-turn: the lead's answer waits.
    report(worker, "Start", now_ms());
    report(lead, "Start", now_ms());
    client
        .team(TeamRequest::Send {
            team: team.clone(),
            from: Party::Lead,
            to: Party::Task("t1".into()),
            kind: MessageKind::Answer,
            body: "Use Resend for the emails".into(),
            reply_to: None,
        })
        .unwrap();
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        !harness.output(&worker_terminal).contains("Use Resend"),
        "typed into a busy agent"
    );

    // Its turn ends: the courier types the message in, once.
    report(worker, "Stop", now_ms() - 10_000);
    wait(|| {
        harness
            .output(&worker_terminal)
            .contains("|Use Resend for the emails")
            .then_some(())
    });
    assert!(
        harness
            .output(&worker_terminal)
            .contains("dragent msg reply 1")
    );
    // Delivered counts as read (recorded once the typing is done).
    wait(|| {
        match client
            .team(TeamRequest::Get { team: team.clone() })
            .unwrap()
        {
            TeamResponse::Team(saved) => saved.messages[0].read.then_some(()),
            _ => None,
        }
    });

    // The worker finishes; the lead, idle, gets the digest.
    client
        .team(TeamRequest::FinishTask {
            team: team.clone(),
            task: "t1".into(),
            result: TaskResult {
                summary: "POST /invites sends a signed link".into(),
                ..Default::default()
            },
        })
        .unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !harness.output(&lead_terminal).contains("News for the lead"),
        "typed into a busy lead"
    );
    report(lead, "Stop", now_ms() - 10_000);
    wait(|| {
        harness
            .output(&lead_terminal)
            .contains("|- t1 Invite API is done: POST /invites sends a signed link")
            .then_some(())
    });
    std::thread::sleep(Duration::from_secs(5));
    // Each delivery shows twice (the terminal's echo, the agent's input);
    // only the agent's line starts with `|`.
    let digests = harness
        .output(&lead_terminal)
        .matches("|- t1 Invite API is done")
        .count();
    assert_eq!(digests, 1, "{}", harness.output(&lead_terminal));
    assert_eq!(
        harness
            .output(&worker_terminal)
            .matches("|Use Resend for the emails")
            .count(),
        1
    );

    // A second worker joins; the first tells it directly how the API
    // answers. It's typed into the second worker, and the lead hears it.
    client
        .team(TeamRequest::AddTask {
            owns: Vec::new(),
            team: team.clone(),
            title: "Invite page".into(),
            prompt: "Build the form".into(),
            depends_on: vec![],
            agent: Some("claude".into()),
            model: None,
            review_of: None,
            qa: false,
        })
        .unwrap();
    client
        .team(TeamRequest::BeginTask {
            team: team.clone(),
            task: "t2".into(),
        })
        .unwrap();
    client
        .team(TeamRequest::TaskStarted {
            base: None,
            team: team.clone(),
            task: "t2".into(),
            workspace: harness.dir.display().to_string(),
            branch: None,
            worker: member(second),
        })
        .unwrap();
    report(second, "Stop", now_ms() - 10_000);
    report(lead, "Start", now_ms());
    client
        .team(TeamRequest::Send {
            team: team.clone(),
            from: Party::Task("t1".into()),
            to: Party::Task("t2".into()),
            kind: MessageKind::Handoff,
            body: "POST /invites returns id and url".into(),
            reply_to: None,
        })
        .unwrap();
    wait(|| {
        harness
            .output(&second_terminal)
            .contains("|POST /invites returns id and url")
            .then_some(())
    });
    assert!(
        harness
            .output(&second_terminal)
            .contains("the worker on t1"),
        "{}",
        harness.output(&second_terminal)
    );
    assert!(
        !harness
            .output(&worker_terminal)
            .contains("returns id and url")
    );
    // The lead is mid-turn: it hears once its turn ends.
    std::thread::sleep(Duration::from_secs(3));
    assert!(!harness.output(&lead_terminal).contains("t1 → t2"));
    report(lead, "Stop", now_ms() - 10_000);
    wait(|| {
        harness
            .output(&lead_terminal)
            .contains("|- t1 → t2 (handoff): POST /invites returns id and url")
            .then_some(())
    });
    assert!(
        harness
            .output(&lead_terminal)
            .contains("Workers wrote to each other above"),
        "{}",
        harness.output(&lead_terminal)
    );
}
