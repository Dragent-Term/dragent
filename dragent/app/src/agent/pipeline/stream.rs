use std::io::{BufRead, BufReader};
use std::sync::mpsc;

#[cfg(not(target_family = "wasm"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_family = "wasm"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;

use super::events::{AgentEventKind, CreateTaskResponse, PipelineEvent};
use crate::api::API_CLIENT;

pub use super::stream_launch::*;
pub use super::stream_local::*;
pub use super::stream_recovery::*;
pub use super::stream_registry::*;
pub use super::stream_tasks::*;
pub use super::stream_types::*;

#[cfg(all(test, not(target_family = "wasm")))]
mod local_pipeline_tests {
    use super::*;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
        WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_workspace_model::{ActorRef, Project, WorkItemId};

    #[test]
    fn bounded_tool_output_preserves_utf8_boundaries() {
        let output = "é".repeat(30_000);
        let bounded = bounded_tool_output(output);
        assert!(bounded.is_char_boundary(bounded.len()));
        assert!(bounded.contains("tool output truncated"));
        assert!(bounded.len() < 49_000);
    }

    #[test]
    fn cacheable_reads_exclude_mutating_tools() {
        assert!(is_cacheable_local_read("read_file"));
        assert!(is_cacheable_local_read("search_files"));
        assert!(!is_cacheable_local_read("write_file"));
        assert!(!is_cacheable_local_read("run_command"));
    }

    #[test]
    fn explicit_tool_scope_filters_extra_definitions() {
        let mut tools = vec![
            serde_json::json!({"name": "read_file"}),
            serde_json::json!({"name": "edit_file"}),
            serde_json::json!({"name": "mcp__issues__read"}),
        ];
        restrict_extra_tools(
            &mut tools,
            Some(&["read_file".into(), "mcp__issues__read".into()]),
        );
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|tool| tool["name"] != "edit_file"));

        restrict_extra_tools(&mut tools, Some(&[]));
        assert!(tools.is_empty());
    }

    #[test]
    fn checkpoint_tool_calls_preserves_structured_calls_for_restart_recovery() {
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            "local-workspace-test".into(),
            serde_json::json!({
                "messages": [
                    {"role": "user", "content": "verify"},
                    {
                        "role": "assistant",
                        "tool_calls": [
                            {"id": "call-read", "name": "read_file", "args": "{\"path\":\"src/lib.rs\"}"},
                            {"id": "call-run", "name": "run_command", "args": {"command": "cargo test"}}
                        ]
                    }
                ]
            }),
        );
        checkpoint.phase = "awaiting_tools".into();

        let calls = checkpoint_tool_calls(&checkpoint).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call_id, "call-read");
        assert_eq!(calls[0].tool_name, "read_file");
        assert_eq!(calls[0].args_json, r#"{"path":"src/lib.rs"}"#);
        assert_eq!(calls[1].call_id, "call-run");
        assert_eq!(calls[1].args_json, r#"{"command":"cargo test"}"#);

        checkpoint.pending_tool_results.insert(
            "call-read".into(),
            super::super::local_run::CachedToolResult {
                output: "contents".into(),
                is_error: false,
            },
        );
        let unresolved = checkpoint_tool_calls(&checkpoint)
            .unwrap()
            .into_iter()
            .filter(|call| !checkpoint.pending_tool_results.contains_key(&call.call_id))
            .collect::<Vec<_>>();
        assert_eq!(unresolved, [calls[1].clone()]);
    }

    #[test]
    fn recovery_state_resumes_provider_phases_and_filters_durable_tool_results() {
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            "local-workspace-state-test".into(),
            serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [
                        {"id": "done", "name": "read_file", "args": "{}"},
                        {"id": "waiting", "name": "run_command", "args": "{}"}
                    ]
                }]
            }),
        );
        assert_eq!(
            local_run_recovery_state(&checkpoint).unwrap(),
            LocalRunRecoveryState::ResumeProvider
        );
        checkpoint.phase = "calling_model".into();
        assert_eq!(
            local_run_recovery_state(&checkpoint).unwrap(),
            LocalRunRecoveryState::ResumeProvider
        );

        checkpoint.phase = "awaiting_tools".into();
        checkpoint.pending_tool_results.insert(
            "done".into(),
            super::super::local_run::CachedToolResult {
                output: "contents".into(),
                is_error: false,
            },
        );
        assert_eq!(
            local_run_recovery_state(&checkpoint).unwrap(),
            LocalRunRecoveryState::AwaitingTools(vec![RecoveredLocalTool {
                call_id: "waiting".into(),
                tool_name: "run_command".into(),
                args_json: "{}".into(),
            }])
        );
    }

    #[test]
    fn host_tool_result_is_recorded_before_delivery_and_survives_recovery_filtering() {
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            "local-workspace-ack-test".into(),
            serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [{"id": "knowledge", "name": "record_knowledge", "args": "{}"}]
                }]
            }),
        );
        checkpoint.phase = "awaiting_tools".into();

        record_local_tool_result(&mut checkpoint, "knowledge", "Recorded", false).unwrap();
        assert_eq!(
            checkpoint.pending_tool_results["knowledge"].output,
            "Recorded"
        );
        assert_eq!(
            local_run_recovery_state(&checkpoint).unwrap(),
            LocalRunRecoveryState::AwaitingTools(Vec::new())
        );
        assert!(record_local_tool_result(&mut checkpoint, "other", "wrong", false).is_err());
    }

    #[test]
    fn committed_outbox_recovers_before_and_after_checkpoint_delivery_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("outbox-recovery.sqlite3");
        let mut connection = dragent_persistence::setup_connection(&database_path).unwrap();
        let project = Project::new_for_organisation(&dragent_persistence::current_org(), directory.path().join("project"), 1);
        dragent_persistence::save_workspace_project(&mut connection, &project).unwrap();
        let work_item_id = WorkItemId::new();
        let actor = ActorRef::local_user(project.id, None);
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 10,
            expected_revision: None,
            payload: WorkspaceCommandPayload::CreateWorkItem {
                title: "Recover acknowledgment".into(),
                execution_location: dragent_workspace_model::ExecutionLocation::Local,
            },
        };

        let run_id = format!("local-workspace-{}", uuid::Uuid::new_v4().simple());
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            run_id.clone(),
            serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [{"id": "knowledge", "name": "record_knowledge", "args": "{}"}]
                }]
            }),
        );
        checkpoint.round = 1;
        checkpoint.phase = "awaiting_tools".into();
        let execution_id = checkpoint.execution_id.clone();
        checkpoint.install().unwrap();
        let outcome = dragent_persistence::WorkspaceToolOutcome {
            command_id: command.command_id,
            project_id: project.id,
            work_item_id,
            runtime_run_id: run_id.clone(),
            runtime_execution_id: execution_id.clone(),
            runtime_round: 1,
            provider_call_id: "knowledge".into(),
            output: Some("Recorded".into()),
            is_error: Some(false),
            source_message_id: None,
            accepted_at_unix_ms: 10,
            delivered_at_unix_ms: None,
            terminal_reason: None,
        };
        dragent_persistence::accept_local_workspace_commands_with_tool_outcome(
            &mut connection,
            std::slice::from_ref(&command),
            Some(&outcome),
        )
        .unwrap();

        // Simulated crash immediately after SQLite commit: the checkpoint is
        // unresolved, but the reloaded outbox contains the exact accepted result.
        assert!(super::super::local_run::LocalRunCheckpoint::load(&run_id)
            .unwrap()
            .pending_tool_results
            .is_empty());
        let recovered =
            dragent_persistence::load_pending_workspace_tool_outcomes(&mut connection).unwrap();
        assert_eq!(recovered, vec![outcome.clone()]);
        assert!(!local_run_ready_to_resume(&run_id, Some(&execution_id)));
        persist_local_tool_result(&run_id, &execution_id, 1, "knowledge", "Recorded", false)
            .unwrap();
        // The same readiness gate used by an already-mounted controller must
        // be re-evaluated after replay. A stale outcome cannot wake a new run.
        assert!(local_run_ready_to_resume(&run_id, Some(&execution_id)));
        assert!(!local_run_ready_to_resume(
            &run_id,
            Some("obsolete-execution")
        ));

        // Simulated second crash after checkpointing but before live delivery:
        // replay is idempotent and the canonical mutation is not repeated.
        persist_local_tool_result(&run_id, &execution_id, 1, "knowledge", "Recorded", false)
            .unwrap();
        let events = dragent_persistence::accept_local_workspace_commands_with_tool_outcome(
            &mut connection,
            std::slice::from_ref(&command),
            Some(&outcome),
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            local_run_recovery_state(
                &super::super::local_run::LocalRunCheckpoint::load(&run_id).unwrap()
            )
            .unwrap(),
            LocalRunRecoveryState::AwaitingTools(Vec::new())
        );
        super::super::local_run::LocalRunCheckpoint::delete(&run_id, &execution_id).unwrap();
    }

    #[test]
    fn duplicate_live_delivery_produces_one_logical_tool_completion() {
        let run_id = format!("local-workspace-{}", uuid::Uuid::new_v4().simple());
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            run_id.clone(),
            serde_json::json!({
                "messages": [{
                    "role": "assistant",
                    "tool_calls": [{"id": "call", "name": "record_knowledge", "args": "{}"}]
                }]
            }),
        );
        checkpoint.round = 1;
        checkpoint.phase = "awaiting_tools".into();
        let execution_id = checkpoint.execution_id.clone();
        checkpoint.install().unwrap();
        persist_local_tool_result(&run_id, &execution_id, 1, "call", "Recorded", false).unwrap();

        let (result_tx, result_rx) = mpsc::channel();
        local_runs().lock().unwrap().insert(
            run_id.clone(),
            LocalRunControl {
                results: result_tx,
                cancelled: Arc::new(AtomicBool::new(false)),
                execution_id: execution_id.clone(),
                delivered_tool_results: HashSet::new(),
            },
        );
        for _ in 0..2 {
            post_tool_result_for_execution(&run_id, &execution_id, 1, "call", "Recorded", false)
                .unwrap();
        }
        assert_eq!(result_rx.try_iter().count(), 1);
        local_runs().lock().unwrap().remove(&run_id);
        super::super::local_run::LocalRunCheckpoint::delete(&run_id, &execution_id).unwrap();
    }

    #[test]
    fn incorporated_old_result_is_idempotent_but_cannot_modify_a_new_execution() {
        let run_id = format!("local-workspace-{}", uuid::Uuid::new_v4().simple());
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            run_id.clone(),
            serde_json::json!({
                "messages": [
                    {"role": "assistant", "tool_calls": [{"id": "call", "name": "record_knowledge", "args": "{}"}]},
                    {"role": "tool", "tool_call_id": "call", "content": "Recorded"}
                ]
            }),
        );
        checkpoint.round = 2;
        checkpoint.phase = "ready_model".into();
        let old_execution = checkpoint.execution_id.clone();
        checkpoint.install().unwrap();
        persist_local_tool_result(&run_id, &old_execution, 1, "call", "Recorded", false).unwrap();
        super::super::local_run::LocalRunCheckpoint::delete(&run_id, &old_execution).unwrap();

        let new_checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            run_id.clone(),
            serde_json::json!({"messages": []}),
        );
        let new_execution = new_checkpoint.execution_id.clone();
        new_checkpoint.install().unwrap();
        assert!(
            persist_local_tool_result(&run_id, &old_execution, 1, "call", "Recorded", false,)
                .is_err()
        );
        assert_eq!(
            super::super::local_run::LocalRunCheckpoint::load(&run_id)
                .unwrap()
                .execution_id,
            new_execution
        );
        super::super::local_run::LocalRunCheckpoint::delete(&run_id, &new_execution).unwrap();
    }

    #[test]
    fn interrupted_provider_resume_retries_the_same_logical_round() {
        let mut checkpoint = super::super::local_run::LocalRunCheckpoint::new(
            "local-workspace-round-test".into(),
            serde_json::json!({"system": "original"}),
        );
        checkpoint.phase = "calling_model".into();
        checkpoint.round = 7;

        assert!(
            prepare_local_checkpoint_for_resume(&mut checkpoint, "fresh repository context")
                .is_ok()
        );
        assert_eq!(checkpoint.round, 6);
        assert!(checkpoint.request["system"]
            .as_str()
            .unwrap()
            .contains("fresh repository context"));

        // `run_local_checkpoint` increments before sending, restoring round 7
        // and therefore the same server billing/idempotency position.
        checkpoint.round += 1;
        assert_eq!(checkpoint.round, 7);
    }
}
