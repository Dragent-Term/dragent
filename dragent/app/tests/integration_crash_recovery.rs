//! Process-level qualification for the Git/SQLite integration crash boundary.
//!
//! The parent creates an isolated repository and database, starts this test
//! executable as a child, waits until the child has durably crossed a selected
//! boundary, and kills it. A fresh child then reconciles the same files and
//! journal. This exercises real process teardown, SQLite reopen/migrations and
//! Git subprocess state rather than returning an injected in-process error.

#![cfg(not(target_family = "wasm"))]

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use dragent::git_status::worktree::{
    apply_agent_integration_plan, classify_agent_integration, prepare_agent_integration_plan,
    AgentIntegrationPlan, IntegrationPlanState,
};
use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    ActorRef, ExecutionAttemptId, ExecutionIsolation, ExecutionReviewResolution, ProjectId,
    WorkItemId,
};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "DRAGENT_INTEGRATION_CRASH_CHILD";
const MODE_ENV: &str = "DRAGENT_INTEGRATION_CRASH_MODE";
const DB_ENV: &str = "DRAGENT_INTEGRATION_CRASH_DB";
const ATTEMPT_ENV: &str = "DRAGENT_INTEGRATION_CRASH_ATTEMPT";
const MARKER_ENV: &str = "DRAGENT_INTEGRATION_CRASH_MARKER";
const DELIVERY_ENV: &str = "DRAGENT_INTEGRATION_CRASH_DELIVERY";

#[derive(Clone, Copy)]
enum Boundary {
    BeforeGitApply,
    AfterGitApply,
    AfterCompletion,
}

impl Boundary {
    fn mode(self) -> &'static str {
        match self {
            Self::BeforeGitApply => "before-git-apply",
            Self::AfterGitApply => "after-git-apply",
            Self::AfterCompletion => "after-completion",
        }
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("start git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_boundary_marker(path: &Path) {
    std::fs::write(path, b"durable\n").expect("write crash-boundary marker");
    let file = std::fs::File::open(path).expect("open crash-boundary marker");
    file.sync_all().expect("sync crash-boundary marker");
}

fn wait_for_marker(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        if let Some(status) = child.try_wait().expect("poll crash child") {
            panic!("crash child exited before its boundary marker: {status}");
        }
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn insert_prevalidated_intent(
    connection: &mut diesel::sqlite::SqliteConnection,
    intent: &dragent_persistence::WorkspaceIntegrationIntent,
) {
    diesel::sql_query(
        "INSERT INTO workspace_integration_intents
         (attempt_id, project_id, work_item_id, command_id, command_json,
          integration_plan_json, state, claim_owner, claim_expires_at_unix_ms,
          created_at_unix_ms, completed_at_unix_ms, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', NULL, NULL, ?7, NULL, NULL)",
    )
    .bind::<Text, _>(intent.attempt_id.0.to_string())
    .bind::<Text, _>(intent.project_id.0.to_string())
    .bind::<Text, _>(intent.work_item_id.0.to_string())
    .bind::<Text, _>(intent.command_id.0.to_string())
    .bind::<Text, _>(serde_json::to_string(&intent.command).unwrap())
    .bind::<Text, _>(&intent.integration_plan_json)
    .bind::<BigInt, _>(intent.created_at_unix_ms)
    .execute(connection)
    .expect("insert isolated prevalidated integration intent");
}

fn mark_completion(
    connection: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
) {
    let changed = diesel::sql_query(
        "UPDATE workspace_integration_intents
         SET state = 'completed', completed_at_unix_ms = 200,
             claim_owner = NULL, claim_expires_at_unix_ms = NULL
         WHERE attempt_id = ?1 AND state = 'prepared'",
    )
    .bind::<Text, _>(attempt_id.0.to_string())
    .execute(connection)
    .expect("commit integration completion marker");
    assert_eq!(changed, 1, "completion must commit exactly once");
}

fn child_paths() -> (PathBuf, ExecutionAttemptId, PathBuf, PathBuf) {
    let database = PathBuf::from(std::env::var_os(DB_ENV).expect("child database path"));
    let attempt = ExecutionAttemptId(
        uuid::Uuid::parse_str(&std::env::var(ATTEMPT_ENV).expect("child attempt id"))
            .expect("parse child attempt id"),
    );
    let marker = PathBuf::from(std::env::var_os(MARKER_ENV).expect("child marker path"));
    let delivery = PathBuf::from(std::env::var_os(DELIVERY_ENV).expect("delivery marker path"));
    (database, attempt, marker, delivery)
}

fn recover_or_stop(mode: &str) {
    let (database, attempt_id, marker, delivery) = child_paths();
    let mut connection = dragent_persistence::setup_connection(&database).expect("open child db");
    let Some(intent) = dragent_persistence::workspace_store::load_workspace_integration_intent(
        &mut connection,
        attempt_id,
    )
    .expect("load integration intent") else {
        panic!("integration intent disappeared")
    };

    if intent.state == "completed" {
        write_boundary_marker(&delivery);
        return;
    }

    let owner = format!("process-{}", std::process::id());
    let claimed = dragent_persistence::claim_workspace_integration(
        &mut connection,
        attempt_id,
        &owner,
        if mode == "recover" { 100 } else { 1 },
        10,
    )
    .expect("claim integration")
    .expect("integration claim should be available");

    if mode == Boundary::BeforeGitApply.mode() {
        write_boundary_marker(&marker);
        loop {
            std::thread::park();
        }
    }

    let plan: AgentIntegrationPlan =
        serde_json::from_str(&claimed.integration_plan_json).expect("decode integration plan");
    apply_agent_integration_plan(&plan).expect("apply or recognize reviewed patch");

    if mode == Boundary::AfterGitApply.mode() {
        write_boundary_marker(&marker);
        loop {
            std::thread::park();
        }
    }

    assert!(dragent_persistence::release_workspace_integration_claim(
        &mut connection,
        attempt_id,
        &owner,
    )
    .expect("release integration claim"));
    mark_completion(&mut connection, attempt_id);

    if mode == Boundary::AfterCompletion.mode() {
        write_boundary_marker(&marker);
        loop {
            std::thread::park();
        }
    }

    write_boundary_marker(&delivery);
}

fn spawn_child(
    mode: &str,
    database: &Path,
    attempt_id: ExecutionAttemptId,
    marker: &Path,
    delivery: &Path,
) -> Child {
    Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--ignored",
            "--exact",
            "integration_crash_child",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .env(MODE_ENV, mode)
        .env(DB_ENV, database)
        .env(ATTEMPT_ENV, attempt_id.0.to_string())
        .env(MARKER_ENV, marker)
        .env(DELIVERY_ENV, delivery)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn crash child")
}

fn run_boundary(boundary: Boundary) {
    let directory = tempfile::tempdir().expect("temporary scenario directory");
    let repository = directory.path().join("repository");
    let candidate = directory.path().join("candidate");
    let database = directory.path().join("workspace.sqlite3");
    let marker = directory.path().join("boundary.marker");
    let delivery = directory.path().join("delivery.marker");
    std::fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q"]);
    git(&repository, &["config", "user.email", "tests@dragent.dev"]);
    git(&repository, &["config", "user.name", "Dragent Tests"]);
    std::fs::write(repository.join("owned.txt"), "before\n").unwrap();
    std::fs::write(repository.join("unrelated.txt"), "keep\n").unwrap();
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "-qm", "base"]);
    git(
        &repository,
        &[
            "worktree",
            "add",
            "--detach",
            candidate.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(candidate.join("owned.txt"), "after\n").unwrap();

    let attempt_id = ExecutionAttemptId::new();
    let head = Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let base = String::from_utf8(head.stdout).unwrap().trim().to_string();
    let isolation = ExecutionIsolation::git_worktree(attempt_id, base);
    let plan = prepare_agent_integration_plan(&repository, &candidate, &isolation).unwrap();

    let project_id = ProjectId::new();
    let work_item_id = WorkItemId::new();
    let command = WorkspaceCommandEnvelope {
        command_id: WorkspaceCommandId::new(),
        schema_version: WORKSPACE_PROTOCOL_VERSION,
        project_id,
        work_item_id,
        actor_id: ActorRef::principal_agent(project_id).id,
        device_id: None,
        correlation_id: CorrelationId::new(),
        timestamp_unix_ms: 1,
        expected_revision: Some(0),
        payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
            attempt_id,
            resolution: ExecutionReviewResolution::Integrated,
        },
    };
    let intent = dragent_persistence::WorkspaceIntegrationIntent {
        attempt_id,
        project_id,
        work_item_id,
        command_id: command.command_id,
        command,
        integration_plan_json: serde_json::to_string(&plan).unwrap(),
        state: "prepared".into(),
        claim_owner: None,
        claim_expires_at_unix_ms: None,
        created_at_unix_ms: 1,
        completed_at_unix_ms: None,
        last_error: None,
    };
    let mut connection = dragent_persistence::setup_connection(&database).unwrap();
    insert_prevalidated_intent(&mut connection, &intent);
    drop(connection);

    let mut crashing = spawn_child(boundary.mode(), &database, attempt_id, &marker, &delivery);
    wait_for_marker(&marker, &mut crashing);
    crashing.kill().expect("kill child at durable boundary");
    crashing.wait().expect("reap killed child");

    let recovered = spawn_child("recover", &database, attempt_id, &marker, &delivery)
        .wait_with_output()
        .expect("run recovery child");
    assert!(
        recovered.status.success(),
        "recovery child failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(delivery.exists(), "restart did not deliver completed state");
    assert_eq!(
        std::fs::read_to_string(repository.join("owned.txt")).unwrap(),
        "after\n"
    );
    assert_eq!(
        std::fs::read_to_string(repository.join("unrelated.txt")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        classify_agent_integration(&plan).unwrap(),
        IntegrationPlanState::Applied
    );

    let mut connection = dragent_persistence::setup_connection(&database).unwrap();
    let completed = dragent_persistence::workspace_store::load_workspace_integration_intent(
        &mut connection,
        attempt_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(completed.state, "completed");
    assert!(
        dragent_persistence::load_pending_workspace_integration_intents(&mut connection)
            .unwrap()
            .is_empty()
    );
}

#[test]
#[ignore = "subprocess helper invoked by the parent boundary tests"]
fn integration_crash_child() {
    if std::env::var_os(CHILD_ENV).is_some() {
        recover_or_stop(&std::env::var(MODE_ENV).expect("child mode"));
    }
}

#[test]
fn process_kill_before_git_apply_recovers_once() {
    run_boundary(Boundary::BeforeGitApply);
}

#[test]
fn process_kill_after_git_apply_before_completion_does_not_reapply() {
    run_boundary(Boundary::AfterGitApply);
}

#[test]
fn process_kill_after_completion_before_delivery_restores_completed_state() {
    run_boundary(Boundary::AfterCompletion);
}
