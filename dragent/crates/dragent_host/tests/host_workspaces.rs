//! Phase-2 workspace authority tests: real host processes + isolated SQLite.
//!
//! Proves the host is the canonical owner for workspaces it manages:
//! create/accept/snapshot, survive UI disconnect, reconnect to the same
//! state, replay durable state after host restart, and idempotent retry.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use dragent_host::HostClient;

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct Isolated {
    _guard: std::sync::MutexGuard<'static, ()>,
    /// Stops the services this test started, pass or fail.
    services: dragent_host::testing::ServiceGuard,
    endpoint: PathBuf,
    db: PathBuf,
}

impl Isolated {
    fn new(name: &str) -> Self {
        let guard = lock().lock().unwrap();
        let tag = format!("dragent-host-ws-{name}-{}", std::process::id());
        let endpoint = std::env::temp_dir().join(format!("{tag}.json"));
        let db = std::env::temp_dir().join(format!("{tag}.sqlite"));
        let _ = std::fs::remove_file(&endpoint);
        let _ = std::fs::remove_file(&db);
        // SAFETY: tests serialize on a global lock, so no concurrent env access.
        unsafe {
            std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &endpoint);
            std::env::set_var("DRAGENT_WORKSPACE_DB", &db);
        }
        Self {
            services: dragent_host::testing::ServiceGuard::new(),
            _guard: guard,
            endpoint,
            db,
        }
    }

    fn ensure(&self) -> HostClient {
        HostClient::ensure_host_with(&host_bin()).expect("host should come up")
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        self.services.stop();
        if let Ok(client) = HostClient::connect_existing() {
            let _ = client.shutdown();
        }
        std::thread::sleep(Duration::from_millis(150));
        let _ = std::fs::remove_file(&self.endpoint);
        let _ = std::fs::remove_file(&self.db);
        unsafe {
            std::env::remove_var(dragent_host::ENDPOINT_FILE_ENV);
            std::env::remove_var("DRAGENT_WORKSPACE_DB");
        }
    }
}

fn create_work_item_command(
    project_id: dragent_workspace_model::ProjectId,
    work_item_id: dragent_workspace_model::WorkItemId,
    title: &str,
) -> dragent_session_protocol::workspace::WorkspaceCommandEnvelope {
    use dragent_session_protocol::workspace::*;
    use dragent_workspace_model::{ActorId, ExecutionLocation};
    WorkspaceCommandEnvelope {
        command_id: WorkspaceCommandId::new(),
        schema_version: WORKSPACE_PROTOCOL_VERSION,
        project_id,
        work_item_id,
        actor_id: ActorId::new(),
        device_id: None,
        correlation_id: CorrelationId::new(),
        timestamp_unix_ms: now_ms(),
        expected_revision: None,
        payload: WorkspaceCommandPayload::CreateWorkItem {
            title: title.to_string(),
            execution_location: ExecutionLocation::Local,
        },
    }
}

#[test]
fn create_list_snapshot_round_trip() {
    let iso = Isolated::new("roundtrip");
    let client = iso.ensure();
    assert_eq!(
        client.list_workspaces().expect("list"),
        Vec::<String>::new()
    );
    let root = std::env::temp_dir().join("dragent-host-ws-roundtrip");
    let id = client
        .create_workspace(&root.to_string_lossy())
        .expect("create");
    assert_eq!(client.list_workspaces().expect("list"), vec![id.clone()]);
    let snap = client.workspace_state(&id).expect("snapshot");
    assert_eq!(snap.workspace_id, id);
    assert!(snap.active_work.is_none());
    assert_eq!(snap.event_count, 0);
    // Re-creating the same root reuses the workspace (no forked authority).
    let again = client
        .create_workspace(&root.to_string_lossy())
        .expect("recreate");
    assert_eq!(again, id);
}

#[test]
fn accept_command_produces_events_and_state() {
    use dragent_workspace_model::WorkItemId;
    let iso = Isolated::new("accept");
    let client = iso.ensure();
    let id = client
        .create_workspace("/tmp/dragent-host-ws-accept")
        .expect("create");
    let snap = client.workspace_state(&id).expect("snapshot");
    let work_id = WorkItemId::new();
    let cmd = create_work_item_command(snap.project.id, work_id, "Host work");
    let count = client.accept_workspace_command(&id, cmd).expect("accept");
    assert!(count >= 1, "expected canonical events, got {count}");
    let after = client.workspace_state(&id).expect("snapshot");
    assert!(after.event_count >= 1);
    let work = after.active_work.expect("active work");
    assert_eq!(work.id, work_id);
}

#[test]
fn state_survives_ui_disconnect_and_reconnect() {
    use dragent_workspace_model::WorkItemId;
    let iso = Isolated::new("rediscon");
    let id = iso
        .ensure()
        .create_workspace("/tmp/dragent-host-ws-rediscon")
        .expect("create");
    let client = HostClient::connect_existing().expect("connect");
    let snap = client.workspace_state(&id).expect("snapshot");
    let cmd = create_work_item_command(snap.project.id, WorkItemId::new(), "No UI here");
    client.accept_workspace_command(&id, cmd).expect("accept");
    let before = client.workspace_state(&id).expect("snapshot");
    drop(client);
    std::thread::sleep(Duration::from_millis(300));
    // A fresh UI connects without spawning and sees identical state.
    let again = HostClient::connect_existing().expect("reconnect");
    let after = again.workspace_state(&id).expect("snapshot");
    assert_eq!(after.event_count, before.event_count);
    assert_eq!(
        after.active_work.map(|w| w.id),
        before.active_work.map(|w| w.id)
    );
}

#[test]
fn host_restart_replays_durable_state() {
    use dragent_workspace_model::WorkItemId;
    let iso = Isolated::new("restart");
    let id = iso
        .ensure()
        .create_workspace("/tmp/dragent-host-ws-restart")
        .expect("create");
    let client = HostClient::connect_existing().expect("connect");
    let snap = client.workspace_state(&id).expect("snapshot");
    let work_id = WorkItemId::new();
    let cmd = create_work_item_command(snap.project.id, work_id, "Durable work");
    client.accept_workspace_command(&id, cmd).expect("accept");
    let before = client.workspace_state(&id).expect("snapshot");
    assert!(before.event_count >= 1);
    drop(client);
    // Kill the host process entirely (Level 2): find it via endpoint, shut
    // down, then start a fresh host on the SAME database file.
    HostClient::connect_existing()
        .expect("connect")
        .shutdown()
        .expect("shutdown");
    std::thread::sleep(Duration::from_millis(400));
    let revived = iso.ensure();
    assert_ne!(revived.host_info().expect("info").host_id, String::new());
    // Same workspace id (stable project identity), state replayed from SQLite.
    assert!(revived.list_workspaces().expect("list").contains(&id));
    let after = revived.workspace_state(&id).expect("snapshot");
    assert_eq!(after.event_count, before.event_count);
    assert_eq!(after.active_work.map(|w| w.id), Some(work_id));
}

#[test]
fn retry_accept_does_not_duplicate_effects() {
    use dragent_workspace_model::WorkItemId;
    let iso = Isolated::new("retry");
    let client = iso.ensure();
    let id = client
        .create_workspace("/tmp/dragent-host-ws-retry")
        .expect("create");
    let snap = client.workspace_state(&id).expect("snapshot");
    let cmd = create_work_item_command(snap.project.id, WorkItemId::new(), "Once");
    let first = client
        .accept_workspace_command(&id, cmd.clone())
        .expect("accept");
    // IPC retry of the SAME command envelope replays the stored outcome.
    let second = client.accept_workspace_command(&id, cmd).expect("retry");
    assert_eq!(first, second);
    let after = client.workspace_state(&id).expect("snapshot");
    assert_eq!(after.event_count, first);
}

#[test]
fn persistence_authority_round_trip() {
    use dragent_workspace_model::{
        Project, WorkItem, WorkItemId, WorkspaceEvent, WorkspaceEventKind,
    };
    let iso = Isolated::new("persist");
    let client = iso.ensure();
    let id = client
        .create_workspace("/tmp/dragent-host-ws-persist")
        .expect("create");
    // Save a project record through the host (overwrites same id; stable identity).
    let snap = client.workspace_state(&id).expect("snapshot");
    client
        .save_workspace_project(snap.project.clone())
        .expect("save project");
    let loaded = client
        .load_workspace_project(&id)
        .expect("load project")
        .expect("project present");
    assert_eq!(loaded.id, snap.project.id);
    // Unknown project loads as None, not an error.
    assert!(
        client
            .load_workspace_project("00000000-0000-0000-0000-000000000000")
            .expect("load missing")
            .is_none()
    );
    // Save a work item + domain event through the host, read them back.
    let work_id = WorkItemId::new();
    let mut work = snap
        .active_work
        .clone()
        .unwrap_or_else(|| WorkItem::new(snap.project.id, "x", 1));
    work.id = work_id;
    client.save_workspace_work_item(work).expect("save work");
    let event = WorkspaceEvent {
        sequence: 0,
        timestamp_unix_ms: 1,
        project_id: snap.project.id,
        work_item_id: Some(work_id),
        kind: WorkspaceEventKind::WorkCreated {
            title: "Host-written".to_string(),
        },
    };
    client.save_workspace_event(event).expect("save event");
    let events = client
        .load_recent_workspace_events(100)
        .expect("load events");
    assert!(
        events.iter().any(|e| e.project_id == snap.project.id),
        "host-written event must be durable"
    );
    let _ = Project::new(std::path::PathBuf::from("/tmp"), 0);
}

#[test]
fn ui_exit_does_not_interrupt_persistence_authority() {
    let iso = Isolated::new("persistlive");
    let id = iso
        .ensure()
        .create_workspace("/tmp/dragent-host-ws-persistlive")
        .expect("create");
    // UI writes through the host, then the UI goes away entirely.
    let client = HostClient::connect_existing().expect("connect");
    let snap = client.workspace_state(&id).expect("snapshot");
    client
        .save_workspace_project(snap.project.clone())
        .expect("save project");
    drop(client);
    std::thread::sleep(Duration::from_millis(300));
    // A fresh UI finds the host authority alive with the write durable.
    let again = HostClient::connect_existing().expect("reconnect");
    let loaded = again
        .load_workspace_project(&id)
        .expect("load")
        .expect("project present");
    assert_eq!(loaded.id, snap.project.id);
}

fn work_envelope(
    project_id: dragent_workspace_model::ProjectId,
    work_item_id: dragent_workspace_model::WorkItemId,
    actor: dragent_workspace_model::ActorId,
    revision: Option<u64>,
    payload: dragent_session_protocol::workspace::WorkspaceCommandPayload,
) -> dragent_session_protocol::workspace::WorkspaceCommandEnvelope {
    use dragent_session_protocol::workspace::*;
    WorkspaceCommandEnvelope {
        command_id: WorkspaceCommandId::new(),
        schema_version: WORKSPACE_PROTOCOL_VERSION,
        project_id,
        work_item_id,
        actor_id: actor,
        device_id: None,
        correlation_id: CorrelationId::new(),
        timestamp_unix_ms: now_ms(),
        expected_revision: revision,
        payload,
    }
}

#[test]
fn scheduler_runs_headless_and_reports_ready_tasks() {
    use dragent_session_protocol::workspace::WorkspaceCommandPayload;
    use dragent_workspace_model::*;
    let iso = Isolated::new("schedhead");
    let id = iso
        .ensure()
        .create_workspace("/tmp/dragent-host-ws-schedhead")
        .expect("create");
    // No UI subscribed anywhere from here on: all driving happens headless.
    let client = HostClient::connect_existing().expect("connect");
    let snap = client.workspace_state(&id).expect("snapshot");
    let project_id = snap.project.id;
    let work_id = WorkItemId::new();
    let actor = ActorId::new();
    let mut revision: Option<u64> = None;
    let mut cmd = |payload| {
        let count = client
            .accept_workspace_command(
                &id,
                work_envelope(project_id, work_id, actor, revision, payload),
            )
            .expect("accept");
        // Optimistic concurrency: chain the observed canonical revision.
        revision = Some(client.workspace_state(&id).expect("snapshot").last_sequence);
        count
    };
    cmd(WorkspaceCommandPayload::CreateWorkItem {
        title: "Scheduled work".to_string(),
        execution_location: ExecutionLocation::Local,
    });
    let role_id = RoleDefinitionId::new();
    cmd(WorkspaceCommandPayload::UpsertRoleDefinition {
        role_definition: RoleDefinition {
            id: role_id,
            scope: RoleDefinitionScope::Project(project_id),
            kind: AgentRoleKind::Worker,
            name: "Developer".into(),
            mission: "Implement".into(),
            capabilities: Vec::new(),
            preferred_tools: Vec::new(),
            created_by: actor,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        },
    });
    let task_id = PlanTaskId::new();
    let criterion_id = AcceptanceCriterionId::new();
    let contract_id = WorkContractId::new();
    cmd(WorkspaceCommandPayload::UpsertWorkContract {
        work_contract: WorkContract {
            id: contract_id,
            work_item_id: work_id,
            role_definition_id: role_id,
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Approved,
            mission: "Implement".into(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: Vec::new(),
            success_criteria: vec![criterion_id],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor,
            updated_by: actor,
            created_at_unix_ms: 2,
            updated_at_unix_ms: 2,
        },
    });
    let plan_id = PlanId::new();
    cmd(WorkspaceCommandPayload::UpsertPlan {
        plan: Plan {
            id: plan_id,
            work_item_id: work_id,
            version: 1,
            status: PlanStatus::Approved,
            tasks: vec![PlanTask {
                id: task_id,
                plan_id,
                title: "Build it".into(),
                description: String::new(),
                status: PlanTaskStatus::Proposed,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: vec![criterion_id],
                assigned_contract_id: Some(contract_id),
            }],
            approved_by: Some(actor),
            approved_at_unix_ms: Some(3),
            created_at_unix_ms: 3,
            updated_at_unix_ms: 3,
        },
    });
    // Headless driver observes a ready task with no UI anywhere in the loop.
    let status = client
        .scheduler_status(&id)
        .expect("status")
        .expect("known");
    assert!(status.has_plan, "plan should project into state");
    assert!(
        status.ready_tasks >= 1,
        "expected ready tasks, got {status:?}"
    );
    drop(client);
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn scheduler_decisions_broadcast_to_subscribers() {
    use dragent_host::Event;
    use dragent_session_protocol::workspace::WorkspaceCommandPayload;
    use dragent_workspace_model::*;
    let iso = Isolated::new("schedevent");
    let id = iso
        .ensure()
        .create_workspace("/tmp/dragent-host-ws-schedevent")
        .expect("create");
    let client = HostClient::connect_existing().expect("connect");
    let (tx, rx) = std::sync::mpsc::channel();
    let _sub = client
        .subscribe(move |event| {
            let _ = tx.send(event);
        })
        .expect("subscribe");
    // Drain the initial HostReady.
    let _ = rx.recv_timeout(Duration::from_secs(5));
    let snap = client.workspace_state(&id).expect("snapshot");
    let work_id = WorkItemId::new();
    let cmd = work_envelope(
        snap.project.id,
        work_id,
        ActorId::new(),
        None,
        WorkspaceCommandPayload::CreateWorkItem {
            title: "Event work".to_string(),
            execution_location: ExecutionLocation::Local,
        },
    );
    client.accept_workspace_command(&id, cmd).expect("accept");
    // A WorkspaceUpdated event must arrive for the accepted command.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let seen = loop {
        if std::time::Instant::now() > deadline {
            break false;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::WorkspaceUpdated { workspace_id }) if workspace_id == id => break true,
            Ok(_) => continue,
            Err(_) => continue,
        }
    };
    assert!(seen, "expected WorkspaceUpdated broadcast");
}
