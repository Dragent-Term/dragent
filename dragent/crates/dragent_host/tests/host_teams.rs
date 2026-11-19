//! Teams through a real `dragent-host` process: requests over the wire, a
//! lead's wait woken by a worker on another connection, change events for
//! subscribers, and the plan surviving a host restart. One test, because
//! it points process-wide variables at private storage.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dragent_host::team::{
    Change, Member, MessageKind, Party, TaskResult, TaskStatus, TeamRequest, TeamResponse,
};
use dragent_host::{Event, HostClient};

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

fn team_id(response: TeamResponse) -> String {
    match response {
        TeamResponse::Team(team) => team.id,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_team_runs_through_the_host_and_survives_its_restart() {
    let dir = std::env::temp_dir().join(format!(
        "dragent-host-teams-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, path) in [
        ("DRAGENT_HOME", dir.join("data")),
        (dragent_host::ENDPOINT_FILE_ENV, dir.join("endpoint.json")),
        ("DRAGENT_WORKSPACE_DB", dir.join("workspace.sqlite")),
        ("HOME", dir.join("home")),
    ] {
        // SAFETY: this executable runs this one test.
        unsafe { std::env::set_var(name, path) };
    }
    let client = HostClient::ensure_host_with(&host_bin()).expect("host should come up");

    let (changed, changes) = mpsc::channel();
    let _events = client
        .subscribe(move |event| {
            if let Event::TeamChanged { team_id } = event {
                let _ = changed.send(team_id);
            }
        })
        .expect("subscribe");

    let team = team_id(
        client
            .team(TeamRequest::Create {
                title: "Invites".into(),
                project_root: dir.display().to_string(),
                roster: vec![],
                cost: Default::default(),
                max_parallel_per_agent: None,
                lead: Some(Member {
                    pane: None,
                    terminal: Some("lead-term".into()),
                    agent: Some("claude".into()),
                }),
            })
            .unwrap(),
    );
    assert_eq!(
        changes.recv_timeout(Duration::from_secs(5)).unwrap(),
        team,
        "subscribers hear of a new team"
    );
    client
        .team(TeamRequest::AddTask {
            owns: Vec::new(),
            team: team.clone(),
            title: "Send invites".into(),
            prompt: "Email an invite link".into(),
            depends_on: vec![],
            agent: Some("codex".into()),
            model: None,
            review_of: None,
            qa: false,
        })
        .unwrap();
    match client
        .team(TeamRequest::Resolve {
            pane: None,
            terminal: Some("lead-term".into()),
        })
        .unwrap()
    {
        TeamResponse::Resolved(found) => assert_eq!(found, Some((team.clone(), Party::Lead))),
        other => panic!("{other:?}"),
    }

    // The lead waits on its own connection; the worker's reports, on others, wake it.
    let waiting_team = team.clone();
    let lead = std::thread::spawn(move || {
        let client = HostClient::connect_existing().unwrap();
        let started = Instant::now();
        let response = client
            .team(TeamRequest::Wait {
                team: waiting_team,
                since: None,
                tasks: vec![],
                party: Some(Party::Lead),
                timeout_ms: 20_000,
            })
            .unwrap();
        (response, started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(300));
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
            workspace: dir.join("t1").display().to_string(),
            branch: None,
            worker: Member {
                pane: None,
                terminal: Some("worker-term".into()),
                agent: Some("codex".into()),
            },
        })
        .unwrap();
    client
        .team(TeamRequest::Send {
            team: team.clone(),
            from: Party::Task("t1".into()),
            to: Party::Lead,
            kind: MessageKind::Note,
            body: "Invites go out through the existing mailer".into(),
            reply_to: None,
        })
        .unwrap();
    let (response, waited) = lead.join().unwrap();
    assert!(
        waited < Duration::from_secs(10),
        "woken by the message, not the timeout: {waited:?}"
    );
    match response {
        TeamResponse::Changes {
            timed_out, events, ..
        } => {
            assert!(!timed_out);
            assert!(events.iter().any(|event| matches!(
                &event.change,
                Change::Message {
                    to: Party::Lead,
                    ..
                }
            )));
        }
        other => panic!("{other:?}"),
    }
    client
        .team(TeamRequest::FinishTask {
            team: team.clone(),
            task: "t1".into(),
            result: TaskResult {
                summary: "Invites send; tests pass".into(),
                ..Default::default()
            },
        })
        .unwrap();

    // A new host process reads the same team back.
    client.shutdown().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while HostClient::connect_existing().is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let restarted = HostClient::ensure_host_with(&host_bin()).expect("host should come back");
    match restarted
        .team(TeamRequest::Get { team: team.clone() })
        .unwrap()
    {
        TeamResponse::Team(saved) => {
            let task = saved.task("t1").unwrap();
            assert_eq!(task.status, TaskStatus::Done);
            assert_eq!(
                task.result.as_ref().unwrap().summary,
                "Invites send; tests pass"
            );
            assert_eq!(saved.messages.len(), 1);
        }
        other => panic!("{other:?}"),
    }
    // Its lead's next wait starts after what it already saw.
    match restarted
        .team(TeamRequest::Wait {
            team: team.clone(),
            since: None,
            tasks: vec![],
            party: Some(Party::Lead),
            timeout_ms: 2_000,
        })
        .unwrap()
    {
        TeamResponse::Changes {
            timed_out, events, ..
        } => {
            assert!(!timed_out, "the finished task is news");
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.change, Change::Message { .. })),
                "{events:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    // Dragent's own agent takes over as the lead: the app takes its news
    // from the host, once, and the list says which session it is.
    restarted
        .team(TeamRequest::SetLead {
            team: team.clone(),
            lead: Member {
                pane: Some("dragent-lead".into()),
                terminal: None,
                agent: Some(dragent_host::team::DRAGENT_AGENT.into()),
            },
            model: None,
        })
        .unwrap();
    restarted
        .team(TeamRequest::Send {
            team: team.clone(),
            from: Party::User,
            to: Party::Lead,
            kind: MessageKind::Note,
            body: "Ship it when QA passes".into(),
            reply_to: None,
        })
        .unwrap();
    let take = || match restarted
        .team(TeamRequest::TakeLeadNews {
            team: team.clone(),
            busy: false,
        })
        .unwrap()
    {
        TeamResponse::News(news) => news,
        other => panic!("{other:?}"),
    };
    let news = take().expect("news for the new lead");
    assert!(news.contains("Ship it when QA passes"), "{news}");
    assert_eq!(take(), None);
    match restarted
        .team(TeamRequest::List { project_root: None })
        .unwrap()
    {
        TeamResponse::Teams(teams) => assert_eq!(
            teams[0].dragent_lead.as_deref(),
            Some("dragent-lead"),
            "{teams:?}"
        ),
        other => panic!("{other:?}"),
    }
    restarted.shutdown().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let _ = std::fs::remove_dir_all(&dir);
}
