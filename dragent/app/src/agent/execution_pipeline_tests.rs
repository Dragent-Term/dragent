//! Tests for pipeline (moved verbatim).

#[cfg(test)]
mod execution_pipeline_tests {
    use super::super::workspace_execution::execution_test_helpers::*;
    use crate::agent::workspace_execution::*;
    use dragent_session_protocol::workspace::WorkspaceEventId;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
        WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_workspace_model::*;
    use dragent_workspace_model::{
        evaluate_scheduler, execution_retry_delay_ms, task_coordination_thread_id, ActorId,
        ActorKind, ActorRef, AgentInstance, AgentInstanceId, AgentInstanceStatus, AgentMessageKind,
        AttentionItem, AttentionItemId, AttentionKind, AttentionResolution, AttentionStatus,
        ChargingAuthorization, CoordinationHealth, DeviceId, ExecutionAttempt, ExecutionAttemptId,
        ExecutionAttemptStatus, ExecutionChangeSummary, ExecutionIsolation, ExecutionLease,
        ExecutionLeaseId, ExecutionReviewResolution, PlanTask, PlanTaskId, PlanTaskStatus,
        ProjectId, ReviewFindingItem, ReviewVerdict, StructuredReviewFinding, WorkContract,
        WorkContractId, WorkContractStatus, WorkItemId, WorkStatus, WorkspaceState,
        MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
    };
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use uuid::Uuid;

    #[test]
    fn runtime_activity_is_bounded_coalesced_and_excludes_private_payloads() {
        use crate::agent::pipeline::AgentEventKind;

        let (_, mut contract) = fixture();
        contract.tools.push("ask_user".into());
        let session = pipeline_session(contract);
        let attempt_id = session.attempt_id();
        let sender = session.sender.clone();
        let mut runtime = WorkspaceExecutionRuntime::default();
        assert!(runtime.activity(attempt_id).is_empty());
        runtime.sessions.insert(attempt_id, session);
        let mut identity = stream_event(AgentEventKind::TextDelta, "");
        identity.error = Some("__task_id__synthetic-runtime".into());
        sender.send(Ok(identity)).unwrap();
        sender
            .send(Ok(stream_event(
                AgentEventKind::Thinking,
                "hidden thinking",
            )))
            .unwrap();
        sender
            .send(Ok(stream_event(
                AgentEventKind::ToolCallStarted,
                "raw args",
            )))
            .unwrap();
        sender
            .send(Ok(stream_event(
                AgentEventKind::TextDelta,
                &"界".repeat(3_999),
            )))
            .unwrap();
        sender
            .send(Ok(stream_event(AgentEventKind::TextDelta, "é終")))
            .unwrap();
        let mut call = tool_call("read_file", r#"{"private":"raw arguments"}"#, false);
        call.server_executed = Some(true);
        call.args_summary = Some("private reason".into());
        sender.send(Ok(call.clone())).unwrap();
        call.kind = AgentEventKind::ToolEnd;
        call.tool_output = Some("private output".into());
        sender.send(Ok(call.clone())).unwrap();
        let mut consent = tool_call("ask_user", r#"{"question":"private question"}"#, false);
        consent.tool_id = Some("consent-call".into());
        sender.send(Ok(consent)).unwrap();
        let mut rejected = tool_call("not_granted", "private rejected args", false);
        rejected.tool_id = Some("rejected-call".into());
        sender.send(Ok(rejected)).unwrap();
        runtime.try_next_actions();
        runtime.try_next_actions();
        let activity = runtime.activity(attempt_id);
        assert_eq!(activity.len(), 7);
        assert_eq!(activity[0].kind, "runtime");
        assert_eq!(activity[0].text, "Runtime identified: synthetic-runtime");
        assert_eq!(activity[1].kind, "thinking");
        assert_eq!(activity[1].text, "Thinking…");
        assert_eq!(activity[2].kind, "narration");
        assert_eq!(activity[2].text.chars().count(), 4_000);
        assert_eq!(activity[2].text, format!("{}é終", "界".repeat(3_998)));
        assert_eq!(activity[3].text, "Dispatched: read_file");
        assert_eq!(activity[4].text, "Completed: read_file");
        assert_eq!(activity[5].text, "Approval required: ask_user");
        assert_eq!(activity[6].text, "Rejected: not_granted");
        for (index, entry) in activity.iter().enumerate() {
            assert_eq!(entry.sequence, index as u64 + 1);
            assert!(!entry.text.contains("private"));
            assert!(!entry.text.contains("hidden"));
            assert!(!entry.text.contains("raw"));
        }
        for index in 0..300 {
            call.tool_id = Some(format!("end-{index}"));
            call.tool_is_error = Some(true);
            sender.send(Ok(call.clone())).unwrap();
        }
        sender
            .send(Ok(stream_event(AgentEventKind::Done, "")))
            .unwrap();
        while !runtime.try_next_actions().is_empty() {}
        let activity = runtime.activity(attempt_id);
        assert_eq!(activity.len(), MAX_EXECUTION_ACTIVITY);
        assert_eq!(activity.first().unwrap().sequence, 53);
        assert_eq!(activity.last().unwrap().sequence, 308);
        assert_eq!(activity.last().unwrap().kind, "finished");
        assert_eq!(
            activity.last().unwrap().text,
            "Execution finished: Succeeded"
        );
        assert!(activity
            .iter()
            .all(|entry| entry.text.chars().count() <= 4_000));
        assert!(runtime.activity(ExecutionAttemptId::new()).is_empty());
    }

    #[test]
    fn runtime_drains_at_most_64_events_per_session_without_starving_tools() {
        use crate::agent::pipeline::AgentEventKind;

        let (_, contract) = fixture();
        let mut busy = pipeline_session(contract.clone());
        busy.runtime_task_id = Some("busy-runtime".into());
        let busy_id = busy.attempt_id();
        let mut other = pipeline_session(contract);
        other.request.launch.attempt.id =
            ExecutionAttemptId(id("bbbbbbbb-1111-4111-8111-bbbbbbbbbbbb"));
        other.runtime_task_id = Some("other-runtime".into());
        let other_id = other.attempt_id();
        for _ in 0..62 {
            busy.sender
                .send(Ok(stream_event(AgentEventKind::TextDelta, "delta")))
                .unwrap();
        }
        let call = tool_call(
            "edit_file",
            r#"{"path":"app/a.rs","old_str":"a","new_str":"b"}"#,
            false,
        );
        busy.sender.send(Ok(call.clone())).unwrap();
        for _ in 0..130 {
            busy.sender
                .send(Ok(stream_event(AgentEventKind::Thinking, "hidden")))
                .unwrap();
        }
        let mut second_call = call.clone();
        second_call.tool_id = Some("call-2".into());
        busy.sender.send(Ok(second_call)).unwrap();
        busy.sender
            .send(Ok(stream_event(AgentEventKind::Done, "")))
            .unwrap();
        busy.sender.send(Ok(call.clone())).unwrap();
        other.sender.send(Ok(call)).unwrap();
        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime.sessions.insert(busy_id, busy);
        runtime.sessions.insert(other_id, other);

        let first = runtime.try_next_actions();
        assert_eq!(first.iter().filter(|(id, _)| *id == busy_id).count(), 64);
        assert_eq!(first.iter().filter(|(id, _)| *id == other_id).count(), 1);
        assert!(
            matches!(&first[62], (id, WorkspacePipelineAction::ExecuteTool(_)) if *id == busy_id)
        );
        assert!(first.iter().any(|(id, action)| *id == other_id
            && matches!(action, WorkspacePipelineAction::ExecuteTool(_))));
        assert_eq!(runtime.try_next_actions().len(), 64);
        assert_eq!(runtime.try_next_actions().len(), 64);
        let final_batch = runtime.try_next_actions();
        assert_eq!(final_batch.len(), 3);
        assert!(
            matches!(&final_batch[1].1, WorkspacePipelineAction::ExecuteTool(request) if request.call_id == "call-2")
        );
        assert!(matches!(
            &final_batch[2].1,
            WorkspacePipelineAction::Finished(_)
        ));
        assert!(runtime.try_next_actions().is_empty());
    }

    #[test]
    fn authority_pause_does_not_consume_another_agents_tool_call() {
        use crate::agent::pipeline::AgentEventKind;

        for pause_for_consent in [false, true] {
            let (_, contract) = fixture();
            let mut first = pipeline_session(contract.clone());
            let mut second = pipeline_session(contract);
            let first_id = ExecutionAttemptId(uuid::Uuid::from_u128(1));
            let second_id = ExecutionAttemptId(uuid::Uuid::from_u128(2));
            first.request.launch.attempt.id = first_id;
            second.request.launch.attempt.id = second_id;
            first.runtime_task_id = Some("local-first".into());
            second.runtime_task_id = Some("local-second".into());
            let call = tool_call("read_file", r#"{"path":"app/a.rs"}"#, false);
            if pause_for_consent {
                first
                    .request
                    .launch
                    .contract
                    .permissions
                    .push(PermissionGrant {
                        permission: "project_read".into(),
                        scope: None,
                        requires_approval: true,
                    });
                first.sender.send(Ok(call.clone())).unwrap();
            } else {
                first
                    .sender
                    .send(Ok(stream_event(AgentEventKind::Done, "")))
                    .unwrap();
            }
            second.sender.send(Ok(call)).unwrap();
            let mut runtime = WorkspaceExecutionRuntime::default();
            runtime.sessions.insert(first_id, first);
            runtime.sessions.insert(second_id, second);

            let batch = runtime.try_next_actions();
            assert!(matches!(
                batch[0].1,
                WorkspacePipelineAction::ConsentRequired(_) | WorkspacePipelineAction::Finished(_)
            ));
            drop(batch.into_iter().take(1));
            let resumed = runtime.try_next_actions();
            assert_eq!(resumed.len(), 1);
            assert!(
                matches!(&resumed[0], (id, WorkspacePipelineAction::ExecuteTool(_)) if *id == second_id)
            );
            assert!(runtime.try_next_actions().is_empty());
        }
    }

    #[test]
    fn pipeline_request_is_derived_from_the_committed_contract() {
        let (event, contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let request = WorkspacePipelineRequest::new(
            launch,
            PathBuf::from("/workspace/billing"),
            "The repository uses Rust.",
            "",
        )
        .unwrap();

        assert_eq!(request.launch.contract.tools, ["read_file", "edit_file"]);
        assert!(request
            .system_prompt
            .contains("Allowed tools: read_file, edit_file"));
        assert!(request.system_prompt.contains("Write scope: app"));
        assert!(request.system_prompt.contains("The repository uses Rust."));
        assert!(request.task_prompt.contains("Implement billing"));
    }

    #[test]
    fn pipeline_request_rejects_relative_project_root() {
        let (event, contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        assert_eq!(
            WorkspacePipelineRequest::new(launch, "relative/project", "", "").unwrap_err(),
            WorkspaceExecutionAdapterError::InvalidProjectRoot
        );
    }

    #[test]
    fn tool_gate_enforces_catalog_scope_and_contract_consent() {
        let (event, mut contract) = fixture();
        contract.tools.push("run_command".into());
        contract.permissions.push(PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app".into()),
            requires_approval: false,
        });
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let request = WorkspacePipelineRequest::new(launch, "/workspace/billing", "", "").unwrap();

        assert_eq!(
            request.classify_tool_call(&tool_call(
                "edit_file",
                r#"{"path":"app/src/lib.rs","old_str":"a","new_str":"b"}"#,
                true,
            )),
            WorkspaceToolDisposition::Execute,
        );
        assert!(matches!(
            request.classify_tool_call(&tool_call(
                "edit_file",
                r#"{"path":"../secrets.txt","old_str":"a","new_str":"b"}"#,
                false,
            )),
            WorkspaceToolDisposition::Denied { .. }
        ));
        assert!(matches!(
            request.classify_tool_call(&tool_call("delete_file", r#"{"path":"app/a.rs"}"#, false)),
            WorkspaceToolDisposition::Denied { .. }
        ));
        assert!(matches!(
            request.classify_tool_call(&tool_call(
                "run_command",
                r#"{"command":"cargo test","reason":"verify"}"#,
                false,
            )),
            WorkspaceToolDisposition::NeedsConsent { .. }
        ));
    }

    #[test]
    fn high_risk_actions_always_need_consent_even_with_broad_contract_preauthorization() {
        let (event, mut contract) = fixture();
        contract.tools.push("run_command".into());
        contract.tools.push("delete_file".into());
        contract.tools.push("github_create_pull_request".into());
        // Shaping-time pre-authorization as broad as it gets: no per-call
        // approval required for commands, deletion, or external writes.
        contract.permissions.push(PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app".into()),
            requires_approval: false,
        });
        contract.permissions.push(PermissionGrant {
            permission: "command_execute".into(),
            scope: None,
            requires_approval: false,
        });
        contract.permissions.push(PermissionGrant {
            permission: "external_write".into(),
            scope: None,
            requires_approval: false,
        });
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let request = WorkspacePipelineRequest::new(launch, "/workspace/billing", "", "").unwrap();

        // A safe, in-scope command still auto-runs — pre-authorization keeps
        // working for routine work.
        assert_eq!(
            request.classify_tool_call(&tool_call(
                "run_command",
                r#"{"command":"git status"}"#,
                false,
            )),
            WorkspaceToolDisposition::Execute,
        );
        // PR creation, database writes, deletion, and a real push always ask,
        // no matter how broad the contract's pre-authorization is.
        assert!(matches!(
            request.classify_tool_call(&tool_call(
                "github_create_pull_request",
                r#"{"title":"x"}"#,
                false,
            )),
            WorkspaceToolDisposition::NeedsConsent { .. }
        ));
        assert!(matches!(
            request.classify_tool_call(&tool_call(
                "run_command",
                r#"{"command":"psql -c 'drop table users'"}"#,
                false,
            )),
            WorkspaceToolDisposition::NeedsConsent { .. }
        ));
        assert!(matches!(
            request.classify_tool_call(&tool_call(
                "run_command",
                r#"{"command":"git push origin main"}"#,
                false,
            )),
            WorkspaceToolDisposition::NeedsConsent { .. }
        ));
        assert!(matches!(
            request.classify_tool_call(&tool_call("delete_file", r#"{"path":"app/a.rs"}"#, false)),
            WorkspaceToolDisposition::NeedsConsent { .. }
        ));
    }

    #[test]
    fn pipeline_session_routes_tools_once_and_finishes_with_model_summary() {
        let (_, mut contract) = fixture();
        contract.permissions.push(PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app".into()),
            requires_approval: false,
        });
        let mut session = pipeline_session(contract);
        let mut identity = tool_call("read_file", "{}", false);
        identity.kind = crate::agent::pipeline::AgentEventKind::TextDelta;
        identity.tool_id = None;
        identity.tool_name = None;
        identity.tool_args = None;
        identity.error = Some("__task_id__local-attempt".into());
        assert!(matches!(
            session.ingest(Ok(identity)),
            WorkspacePipelineAction::RuntimeIdentified { .. }
        ));

        let edit = tool_call(
            "edit_file",
            r#"{"path":"app/src/lib.rs","old_str":"a","new_str":"b"}"#,
            true,
        );
        assert!(matches!(
            session.ingest(Ok(edit.clone())),
            WorkspacePipelineAction::ExecuteTool(_)
        ));
        assert!(matches!(
            session.ingest(Ok(edit)),
            WorkspacePipelineAction::ToolRejected { .. }
        ));

        let mut text = tool_call("read_file", "{}", false);
        text.kind = crate::agent::pipeline::AgentEventKind::TextDelta;
        text.text = Some("Implemented and verified billing.".into());
        text.tool_id = None;
        text.tool_name = None;
        text.tool_args = None;
        assert!(matches!(
            session.ingest(Ok(text)),
            WorkspacePipelineAction::TextDelta { .. }
        ));
        let mut done = tool_call("read_file", "{}", false);
        done.kind = crate::agent::pipeline::AgentEventKind::Done;
        assert!(matches!(
            session.ingest(Ok(done)),
            WorkspacePipelineAction::Finished(WorkspaceExecutionCompletion {
                outcome: ExecutionAttemptStatus::Succeeded,
                ref summary,
                ..
            }) if summary == "Implemented and verified billing."
        ));
    }

    #[test]
    fn parked_tool_call_builds_stable_durable_permission_attention() {
        let (_, mut contract) = fixture();
        contract.tools.push("run_command".into());
        let mut session = pipeline_session(contract);
        let mut identity = tool_call("read_file", "{}", false);
        identity.kind = crate::agent::pipeline::AgentEventKind::TextDelta;
        identity.tool_id = None;
        identity.tool_name = None;
        identity.tool_args = None;
        identity.error = Some("__task_id__local-attempt".into());
        session.ingest(Ok(identity));

        let action = session.ingest(Ok(tool_call(
            "run_command",
            r#"{"command":"cargo test","reason":"verify"}"#,
            false,
        )));
        let WorkspacePipelineAction::ConsentRequired(request) = action else {
            panic!("unbounded command was not parked for consent")
        };
        let first = session
            .consent_command(&request, 21, 1_788_796_800_100)
            .unwrap();
        let second = session
            .consent_command(&request, 21, 1_788_796_800_100)
            .unwrap();
        let WorkspaceCommandPayload::RaiseAttention { attention_item } = &first.payload else {
            panic!("consent did not create attention authority")
        };
        let WorkspaceCommandPayload::RaiseAttention {
            attention_item: second_attention,
        } = &second.payload
        else {
            unreachable!()
        };
        assert_eq!(attention_item.id, second_attention.id);
        assert_eq!(
            attention_item.execution_attempt_id,
            Some(session.attempt_id())
        );
        assert_eq!(attention_item.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(
            attention_item.assigned_to,
            [session.request.launch.lease.charging.owner_id]
        );
        assert_eq!(first.expected_revision, Some(21));
        first.validate().unwrap();
        // Regression guard: without a thread id this permission request is
        // invisible in the unified Conversation scrollback — reachable only
        // through the bell popover, with no visible worker card to explain
        // what's actually waiting.
        assert_eq!(
            attention_item.thread_id,
            Some(task_coordination_thread_id(
                session.request.launch.attempt.task_id
            ))
        );
        // Parity with the terminal agent's own live approval card
        // (`build_approval_request`), which always shows the actual
        // command, not just the reason — otherwise the conversation card
        // asks for permission without ever saying for what.
        assert!(attention_item.summary.ends_with("\n$ cargo test"));
    }

    #[test]
    fn consent_summary_falls_back_without_a_parseable_command() {
        let request = WorkspaceToolRequest {
            runtime_task_id: "task-1".into(),
            call_id: "call-1".into(),
            tool_name: "run_command".into(),
            args_json: "{}".into(),
            reason: String::new(),
        };
        assert_eq!(
            request.consent_summary(),
            "Approval required before `run_command` can run."
        );
    }

    #[test]
    fn resolved_permission_resumes_the_exact_parked_call_once() {
        let (started, mut contract) = fixture();
        contract.tools.push("run_command".into());
        let mut session = pipeline_session(contract);
        let mut identity = tool_call("read_file", "{}", false);
        identity.kind = crate::agent::pipeline::AgentEventKind::TextDelta;
        identity.tool_id = None;
        identity.tool_name = None;
        identity.tool_args = None;
        identity.error = Some("__task_id__local-attempt".into());
        session.ingest(Ok(identity));

        let WorkspacePipelineAction::ConsentRequired(request) = session.ingest(Ok(tool_call(
            "run_command",
            r#"{"command":"cargo test","reason":"verify"}"#,
            false,
        ))) else {
            panic!("command was not parked")
        };
        let command = session
            .consent_command(&request, 21, started.timestamp_unix_ms + 1)
            .unwrap();
        let WorkspaceCommandPayload::RaiseAttention { mut attention_item } = command.payload else {
            unreachable!()
        };
        attention_item.status = AttentionStatus::Resolved;
        attention_item.resolved_at_unix_ms = Some(started.timestamp_unix_ms + 2);
        attention_item.resolution_kind = Some(AttentionResolution::Approved);
        attention_item.resolution = Some("Approved once".into());
        let resolution = WorkspaceEventEnvelope {
            event_id: WorkspaceEventId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            sequence: 22,
            project_id: started.project_id,
            work_item_id: started.work_item_id,
            actor_id: started.actor_id,
            device_id: started.device_id,
            task_id: started.task_id,
            correlation_id: started.correlation_id,
            causation_id: Some(WorkspaceCommandId::new()),
            timestamp_unix_ms: started.timestamp_unix_ms + 2,
            payload: WorkspaceEventPayload::AttentionUpdated { attention_item },
        };
        let attempt_id = session.attempt_id();
        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime.sessions.insert(attempt_id, session);

        assert!(matches!(
            runtime.observe_attention_resolution(&resolution),
            Some(WorkspaceResolvedToolAction::Execute {
                attempt_id: resumed_attempt,
                request: resumed,
            }) if resumed_attempt == attempt_id && resumed.call_id == request.call_id
        ));
        assert!(runtime.observe_attention_resolution(&resolution).is_none());
    }

    #[test]
    fn approved_workspace_write_dispatches_to_local_tool() {
        let (_, mut contract) = fixture();
        contract.tools = vec!["write_file".into()];
        contract.write_scope = vec!["app/src".into()];
        contract.permissions = vec![PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app/src".into()),
            requires_approval: false,
        }];
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let file = root.join("app/src/local_dispatch.txt");
        let mut session = pipeline_session(contract);
        session.request.project_root = root.clone();
        session.request.launch.attempt.id = ExecutionAttemptId::new();
        let attempt_id = session.attempt_id();
        session.checkpoint_message_id = checkpoint_message_id(attempt_id);
        crate::agent::local_tools::set_project_cwd(&root, attempt_id.0);
        let runtime_task_id = format!("local-dispatch-{}", attempt_id.0);
        let mut identity = stream_event(crate::agent::pipeline::AgentEventKind::TextDelta, "");
        identity.error = Some(format!("__task_id__{runtime_task_id}"));
        assert_eq!(
            session.ingest(Ok(identity)),
            WorkspacePipelineAction::RuntimeIdentified {
                runtime_task_id: runtime_task_id.clone(),
            }
        );

        let mut event = tool_call(
            "write_file",
            r#"{"path":"app/src/local_dispatch.txt","content":"written by local dispatch\n"}"#,
            true,
        );
        let call_id = format!("write-{}", attempt_id.0);
        event.tool_id = Some(call_id.clone());
        let WorkspacePipelineAction::ExecuteTool(request) = session.ingest(Ok(event)) else {
            panic!("contract-approved write must dispatch to the local tool")
        };
        assert_eq!(request.runtime_task_id, runtime_task_id);
        assert_eq!(request.call_id, call_id);
        assert_eq!(request.tool_name, "write_file");
        assert!(!file.exists());
        session.prepare_tool_checkpoint(&request);
        let (output, is_error) = crate::agent::local_tools::execute_with_call_id(
            &request.call_id,
            &request.tool_name,
            &request.args_json,
            attempt_id.0,
        );
        assert!(!is_error, "{output}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "written by local dispatch\n"
        );
        assert!(session
            .checkpoints
            .has_checkpoint(session.checkpoint_message_id));
        session
            .checkpoints
            .restore(session.checkpoint_message_id)
            .unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn approved_worker_tool_event_executes_a_real_file_edit() {
        let (_, mut contract) = fixture();
        contract.permissions.push(PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app".into()),
            requires_approval: false,
        });
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("app/src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "before\n").unwrap();
        let mut session = pipeline_session(contract);
        session.request.project_root = directory.path().to_path_buf();
        session.runtime_task_id = Some("local-dispatch-test".into());
        let pane_id = uuid::Uuid::new_v4();
        crate::agent::local_tools::set_project_cwd(directory.path(), pane_id);
        let event = tool_call(
            "edit_file",
            r#"{"path":"app/src/lib.rs","old_str":"before","new_str":"after"}"#,
            false,
        );
        let WorkspacePipelineAction::ExecuteTool(request) = session.ingest(Ok(event)) else {
            panic!("approved write was not dispatched");
        };
        session.prepare_tool_checkpoint(&request);
        let (output, is_error) = crate::agent::local_tools::execute_with_call_id(
            &request.call_id,
            &request.tool_name,
            &request.args_json,
            pane_id,
        );
        assert!(!is_error, "{output}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "after\n");
        session
            .checkpoints
            .restore(session.checkpoint_message_id)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before\n");
    }

    #[test]
    fn structured_workspace_write_captures_a_restorable_preimage() {
        let (_, mut contract) = fixture();
        contract.permissions.push(PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app".into()),
            requires_approval: false,
        });
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("app/src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "before\n").unwrap();

        let mut session = pipeline_session(contract);
        session.request.project_root = directory.path().to_path_buf();
        session.checkpoint_message_id = checkpoint_message_id(session.attempt_id());
        let request = WorkspaceToolRequest {
            runtime_task_id: "runtime-1".into(),
            call_id: "write-1".into(),
            tool_name: "edit_file".into(),
            args_json: r#"{"path":"app/src/lib.rs","old_str":"before","new_str":"after"}"#.into(),
            reason: String::new(),
        };

        session.prepare_tool_checkpoint(&request);
        std::fs::write(&file, "after\n").unwrap();
        assert!(session
            .checkpoints
            .has_checkpoint(session.checkpoint_message_id));
        session
            .checkpoints
            .restore(session.checkpoint_message_id)
            .unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "before\n");
    }

    #[test]
    fn pipeline_timeout_maps_to_canonical_timeout() {
        let (_, contract) = fixture();
        let mut session = pipeline_session(contract);
        assert!(matches!(
            session.ingest(Err(crate::agent::pipeline::StreamError::Timeout(
                "provider silent".into()
            ))),
            WorkspacePipelineAction::Finished(WorkspaceExecutionCompletion {
                outcome: ExecutionAttemptStatus::TimedOut,
                ..
            })
        ));
    }
}
