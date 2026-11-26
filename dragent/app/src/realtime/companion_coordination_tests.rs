use super::super::desktop_catalog::AccountKey;
use super::*;
use dragent_host::team::{
    Member, Message, MessageKind, Party, Task, TaskKind, TaskResult, TaskStatus, Team, TeamStatus,
    TeamSummary,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};

fn summary(id: &str, project: &str, status: TeamStatus, updated: i64) -> TeamSummary {
    TeamSummary {
        id: id.to_string(),
        title: format!("Team {id}"),
        project_root: project.to_string(),
        status,
        tasks: 2,
        done: 1,
        running: 1,
        blocked: 0,
        needs_user: 0,
        updated_at_ms: updated,
        worker_panes: Vec::new(),
        dragent_lead: None,
        lead_model: None,
    }
}

fn task(id: &str, status: TaskStatus) -> Task {
    Task {
        id: id.to_string(),
        title: format!("Task {id}"),
        prompt: "SECRET PROMPT THAT MUST NEVER LEAVE THE HOST".to_string(),
        kind: TaskKind::Work,
        review_of: None,
        qa: false,
        owns: Vec::new(),
        base: None,
        integrated: None,
        depends_on: vec!["t0".to_string()],
        agent: Some("claude".to_string()),
        model: Some("sonnet".to_string()),
        model_locked: false,
        user_model: None,
        attempt_model: None,
        status,
        workspace: Some("/tmp/secret-checkout".to_string()),
        branch: Some("feat".to_string()),
        worker: Some(Member {
            pane: Some("pane-secret".to_string()),
            terminal: Some("term-secret".to_string()),
            agent: Some("claude".to_string()),
            ..Default::default()
        }),
        result: Some(TaskResult {
            details: None,
            summary: "did the thing".to_string(),
            files: vec!["/etc/passwd".to_string()],
            checks: vec![],
            verdict: None,
        }),
        reason: None,
        attempts: 1,
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: None,
        finished_at_ms: None,
    }
}

fn message(id: u64, body: &str) -> Message {
    Message {
        id,
        from: Party::Lead,
        to: Party::User,
        kind: MessageKind::Note,
        body: body.to_string(),
        reply_to: None,
        choices: Vec::new(),
        at_ms: id as i64 * 1000,
        read: false,
    }
}

fn team_fixture() -> Team {
    Team {
        outcome: None,
        id: "team-abcdef12".to_string(),
        title: "Build it".to_string(),
        project_root: "/repo".to_string(),
        status: TeamStatus::Running,
        lead: None,
        lead_model: None,
        roster: Vec::new(),
        cost: dragent_host::team::Cost::Balanced,
        max_parallel_per_agent: 3,
        auto_start: true,
        lead_answers_prompts: true,
        tasks: vec![
            task("t1", TaskStatus::Running),
            task("t2", TaskStatus::Done),
        ],
        messages: vec![message(1, "hello"), message(2, "world")],
        events: Vec::new(),
        revision: 1,
        cursors: HashMap::new(),
        courier: HashMap::new(),
        next_task: 3,
        next_message: 3,
        created_at_ms: 1,
        updated_at_ms: 2,
    }
}

#[test]
fn status_strings_match_store_enums() {
    assert_eq!(team_status_str(TeamStatus::Planning), "planning");
    assert_eq!(team_status_str(TeamStatus::Running), "running");
    assert_eq!(team_status_str(TeamStatus::Done), "done");
    assert_eq!(team_status_str(TeamStatus::Stopped), "stopped");
    assert_eq!(task_status_str(TaskStatus::Blocked), "blocked");
    assert_eq!(message_kind_str(MessageKind::NeedsUser), "needs_user");
    let (from, label) = party_ids(&Party::Task("t1".to_string()));
    assert_eq!((from, label.as_str()), ("task", "t1"));
}

#[test]
fn list_filters_to_allowlist_and_caps_at_100() {
    let allowed: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    let mut teams = Vec::new();
    for i in 0..105 {
        teams.push(summary(
            &format!("team-{i:04}"),
            "/repo",
            TeamStatus::Running,
            i,
        ));
    }
    // One team from a project that is not in this window's sidebar.
    teams.push(summary("team-evil", "/other", TeamStatus::Running, 9999));
    let result = build_list_result(teams, &allowed, None);
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), MAX_TEAMS);
    assert_eq!(result["truncated"], true);
    assert!(items.iter().all(|t| t["project_path"] == "/repo"));
    // Most recent first.
    assert!(items[0]["updated_at"].as_i64().unwrap() >= items[1]["updated_at"].as_i64().unwrap());
    // No prompt, pane, or credential material in list items.
    let raw = result.to_string();
    assert!(!raw.contains("SECRET"));
    assert!(!raw.contains("pane"));
}

#[test]
fn list_respects_phone_project_filter_without_leaking() {
    let allowed: HashSet<String> = ["/a".to_string(), "/b".to_string()].into_iter().collect();
    let teams = vec![
        summary("team-1", "/a", TeamStatus::Planning, 2),
        summary("team-2", "/b", TeamStatus::Done, 1),
    ];
    let filtered = build_list_result(teams.clone(), &allowed, Some("/b"));
    let items = filtered["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["project_path"], "/b");
    assert_eq!(filtered["truncated"], false);
    // Unknown filter yields nothing here (daemon layer returns gone).
    let empty = build_list_result(teams, &allowed, Some("/missing"));
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);
}

#[test]
fn detail_omits_prompts_panes_and_bounds_text() {
    let mut team = team_fixture();
    team.tasks[0].title = "x".repeat(MAX_TITLE_CHARS + 10);
    team.messages[0].body = "y".repeat(MAX_MESSAGE_CHARS + 10);
    let result = build_detail_result(&team);
    assert_eq!(result["truncated"], true);
    let detail = &result["team"];
    assert_eq!(detail["id"], "team-abcdef12");
    assert_eq!(detail["goal"], "Build it");
    assert_eq!(detail["status"], "running");
    let tasks = detail["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        tasks[0]["title"].as_str().unwrap().chars().count(),
        MAX_TITLE_CHARS
    );
    // Projection keeps only the safe task fields.
    assert!(tasks[0].get("agent").is_some());
    assert!(tasks[0].get("model").is_some());
    assert!(tasks[0].get("depends_on").is_some());
    assert!(tasks[0].get("result_summary").is_some());
    assert!(tasks[0].get("prompt").is_none());
    assert!(tasks[0].get("workspace").is_none());
    assert!(tasks[0].get("worker").is_none());
    let raw_tasks = tasks[0].to_string();
    assert!(!raw_tasks.contains("SECRET"));
    assert!(!raw_tasks.contains("pane-secret"));
    let messages = detail["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["from"], "lead");
    assert_eq!(messages[0]["label"], "lead");
    assert_eq!(messages[0]["kind"], "note");
    assert_eq!(
        messages[0]["text"].as_str().unwrap().chars().count(),
        MAX_MESSAGE_CHARS
    );
}

#[test]
fn detail_caps_tasks_and_messages_to_most_recent() {
    let mut team = team_fixture();
    team.tasks = (0..70)
        .map(|i| task(&format!("t{i}"), TaskStatus::Pending))
        .collect();
    team.messages = (0..150).map(|i| message(i, "hi")).collect();
    let result = build_detail_result(&team);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["team"]["tasks"].as_array().unwrap().len(), MAX_TASKS);
    let messages = result["team"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), MAX_MESSAGES);
    // Most-recent slice kept.
    assert_eq!(messages[0]["id"], json!("50"));
    assert_eq!(messages[99]["id"], json!("149"));
}

#[test]
fn detail_stays_within_512kib() {
    let mut team = team_fixture();
    team.messages = (0..100)
        .map(|i| message(i, &"z".repeat(MAX_MESSAGE_CHARS)))
        .collect();
    let result = build_detail_result(&team);
    let bytes = serde_json::to_vec(&result).unwrap();
    assert!(
        bytes.len() <= MAX_RESPONSE - 1024,
        "detail {} bytes",
        bytes.len()
    );
}

#[test]
fn registry_retires_windows_and_never_relabels_projects() {
    let mut registry = Registry::default();
    registry.note(
        "win-a".into(),
        AccountKey::new("alice", "org-a"),
        vec!["/repo".to_string()],
    );
    let owner = registry.current().unwrap();
    assert!(registry.allowlist(&owner).unwrap().contains("/repo"));
    registry.note(
        "win-b".into(),
        AccountKey::new("alice", "org-a"),
        vec!["/second".to_string()],
    );
    // Same owner: union.
    let union = registry.allowlist(&owner).unwrap();
    assert!(union.contains("/repo") && union.contains("/second"));
    registry.retire("win-a");
    let after = registry.allowlist(&owner).unwrap();
    assert!(!after.contains("/repo") && after.contains("/second"));
    // Account switch discards, never relabels.
    registry.note(
        "win-c".into(),
        AccountKey::new("bob", "org-b"),
        vec!["/repo".to_string()],
    );
    assert!(registry.allowlist(&owner).is_none());
    let bob = registry.current().unwrap();
    assert_eq!(bob.account.user_id, "bob");
    registry.retire("win-c");
    assert!(registry.current().is_none());
}

#[test]
fn a_coordination_the_sidebar_shows_is_listed_from_its_worktree_folder() {
    // The team runs in a worktree that isn't itself a sidebar project.
    let allowed: HashSet<String> = ["/repo".to_string()].into_iter().collect();
    let shown: HashSet<String> = ["team-wt".to_string()].into_iter().collect();
    let teams = vec![
        summary("team-wt", "/worktrees/repo-feature", TeamStatus::Running, 5),
        summary("team-hidden", "/worktrees/other", TeamStatus::Running, 4),
    ];
    let result = build_list_result_with(teams.clone(), &allowed, &shown, None);
    let ids: Vec<_> = result["items"].as_array().unwrap().iter().map(|t| t["id"].clone()).collect();
    assert_eq!(ids, vec![json!("team-wt")], "shown team listed, hidden one not");
    // Without it shown (the old rule), the list came back empty.
    assert!(build_list_result(teams, &allowed, None)["items"].as_array().unwrap().is_empty());
}

#[test]
fn registry_keeps_shown_teams_per_window_and_owner() {
    let mut registry = Registry::default();
    let alice = AccountKey::new("alice", "org-a");
    registry.note("win-a".into(), alice.clone(), vec!["/repo".to_string()]);
    registry.note_teams("win-a", &alice, vec![("team-1".into(), Some("/repo".into()))]);
    let owner = registry.current().unwrap();
    assert_eq!(registry.shown_teams(&owner).get("team-1"), Some(&Some("/repo".to_string())));
    // Another account's note never mixes in, and an unknown window is ignored.
    registry.note_teams("win-a", &AccountKey::new("bob", "org-b"), vec![("team-x".into(), None)]);
    registry.note_teams("win-z", &alice, vec![("team-z".into(), None)]);
    let shown = registry.shown_teams(&owner);
    assert!(shown.contains_key("team-1") && !shown.contains_key("team-x") && !shown.contains_key("team-z"));
    // Closing the window drops them.
    registry.retire("win-a");
    assert!(registry.current().is_none());
    registry.note("win-b".into(), alice, vec!["/repo".to_string()]);
    assert!(registry.shown_teams(&registry.current().unwrap()).is_empty());
}

#[test]
fn truncate_is_char_boundary() {
    let emoji = "é".repeat(MAX_TITLE_CHARS + 5);
    let (cut, was) = truncate_chars(&emoji, MAX_TITLE_CHARS);
    assert!(was);
    assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
    assert!(cut.is_char_boundary(cut.len()));
}

#[test]
fn projections_match_phone_boolean_and_identity_types() {
    let mut entry = summary("team-a", "/repo", TeamStatus::Running, 1);
    entry.needs_user = 3;
    let (item, _) = project_list_item(&entry);
    assert_eq!(item["needs_user"], json!(true));
    let detail = build_detail_result(&team_fixture());
    assert!(detail["team"]["messages"][0]["id"].is_string());
}

#[test]
fn retired_socket_account_cannot_read_successor_project_registry() {
    let mut registry = Registry::default();
    let alice = AccountKey::new("alice", "org-a");
    let bob = AccountKey::new("bob", "org-b");
    registry.note("window".into(), alice.clone(), vec!["/repo".into()]);
    assert!(registry.snapshot_for(&alice).is_some());
    registry.note("window".into(), bob.clone(), vec!["/repo".into()]);
    assert!(registry.snapshot_for(&alice).is_none());
    assert!(registry.snapshot_for(&bob).is_some());
    assert!(registry
        .snapshot_for(&AccountKey::new("bob", "org-a"))
        .is_none());
}

#[test]
fn team_changes_name_their_fields_exactly() {
    let account = AccountKey { user_id: "u".into(), org: "o".into() };
    let refused = |method: &str, params: Value| write_job(&account, method, &params).is_err();
    assert!(refused("coordination.send", json!({"team_id": "team-1"})), "no message");
    assert!(refused("coordination.send", json!({"team_id": "team-1", "body": "   "})));
    assert!(refused("coordination.send", json!({"team_id": "../x", "body": "hi"})));
    assert!(refused("coordination.configure", json!({"team_id": "team-1"})), "nothing to change");
    assert!(refused("coordination.configure", json!({"team_id": "team-1", "max_parallel": 0})));
    assert!(refused("coordination.configure", json!({"team_id": "team-1", "max_parallel": 99})));
    assert!(refused("coordination.stop", json!({"team_id": "team-1", "force": true})));
    assert!(refused("coordination.forget", json!({"team_id": "team-1", "all": true})));
    assert!(refused("coordination.cancel_task", json!({"team_id": "team-1"})), "no task");
    assert!(refused("coordination.configure", json!({"team_id": "team-1", "cost": "free"})));
    assert!(refused("coordination.reassign", json!({"team_id": "team-1"})));
    assert!(refused("coordination.task_model", json!({"team_id": "team-1", "task_id": "t1"})), "no model given");
    assert!(refused("coordination.task_model", json!({"team_id": "team-1", "task_id": "t1", "model": 4})));
    assert!(refused("coordination.task_model", json!({"team_id": "team-1", "task_id": "t1", "model": "none"})), "null means default");
    assert!(refused("coordination.new_lead", json!({"team_id": "team-1", "agent": "claude"})));
    assert!(refused("coordination.clean_up", json!({"team_id": "team-1", "paths": []})));
    let ok = |method: &str, params: Value| write_job(&account, method, &params).is_ok();
    assert!(ok("coordination.send", json!({"team_id": "team-1", "body": "Add a reviewer"})));
    assert!(ok("coordination.stop", json!({"team_id": "team-1"})));
    assert!(ok("coordination.configure", json!({"team_id": "team-1", "max_parallel": 3, "auto_start": false})));
    assert!(ok("coordination.configure", json!({"team_id": "team-1", "lead_answers_prompts": false, "cost": "best"})));
    assert!(ok("coordination.cancel_task", json!({"team_id": "team-1", "task_id": "t2"})));
    assert!(ok("coordination.forget", json!({"team_id": "team-1"})));
    assert!(ok("coordination.task_model", json!({"team_id": "team-1", "task_id": "t1", "model": "opus"})));
    assert!(ok("coordination.task_model", json!({"team_id": "team-1", "task_id": "t1", "model": null})));
    assert!(ok("coordination.new_lead", json!({"team_id": "team-1"})));
    assert!(ok("coordination.clean_up", json!({"team_id": "team-1"})));
}

#[test]
fn unfinished_tasks_offer_their_agents_models() {
    let mut team = team_fixture();
    team.tasks = vec![task("t1", TaskStatus::Running), task("t2", TaskStatus::Done)];
    let detail = build_detail_result(&team);
    assert_eq!(detail["team"]["tasks"][0]["can_set_model"], true);
    assert_eq!(detail["team"]["tasks"][1]["can_set_model"], false);
    let claude = detail["team"]["model_choices"]["claude"].as_array().expect("claude's models");
    assert!(!claude.is_empty());
    assert!(claude.iter().all(|model| model["id"].is_string() && model["label"].is_string()));
    assert_eq!(detail["team"]["needs_new_lead"], false);
    assert_eq!(detail["team"]["cleanup"], Value::Null);
}

#[test]
fn the_open_question_is_the_last_one_nothing_answered() {
    let mut team = team_fixture();
    team.messages = vec![message(1, "Which database?")];
    assert_eq!(open_question(&team).map(|m| m.id), Some(1));
    let detail = build_detail_result(&team);
    assert_eq!(detail["team"]["question"]["text"], "Which database?");
    team.messages.push(Message { from: Party::User, to: Party::Lead, ..message(2, "Postgres") });
    assert!(open_question(&team).is_none());
    assert_eq!(build_detail_result(&team)["team"]["question"], Value::Null);
}

#[test]
fn the_detail_carries_settings_but_no_terminal_internals() {
    let team = team_fixture();
    let detail = build_detail_result(&team);
    assert_eq!(detail["team"]["cost"], "balanced");
    assert!(detail["team"]["lead_answers_prompts"].is_boolean());
    assert_eq!(detail["team"]["lead_tab_id"], Value::Null);
    let text = detail.to_string();
    assert!(!text.contains("pane-secret") && !text.contains("term-secret"));
}

#[test]
fn the_encrypted_channel_supplies_the_lease() {
    let params = with_channel_lease(json!({"team_id": "team-1"}));
    assert!(params["session_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(params["expires_at"].as_u64().is_some());
    // A phone that still sends its own keeps them.
    let own = with_channel_lease(json!({"session_id": "s", "expires_at": 5}));
    assert_eq!(own["session_id"], "s");
    assert_eq!(own["expires_at"], 5);
}
