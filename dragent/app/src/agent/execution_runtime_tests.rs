//! Tests for runtime (moved verbatim).

#[cfg(test)]
mod execution_runtime_tests {
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
    fn runtime_renews_expiring_lease_and_cancels_only_after_authority_event() {
        let (started, contract) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            ..
        } = &started.payload
        else {
            unreachable!()
        };
        let now = started.timestamp_unix_ms + 1;
        let attempt_id = attempt.id;
        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime
            .sessions
            .insert(attempt_id, pipeline_session(contract));

        assert!(runtime
            .next_lease_renewal_command(started.sequence, lease.expires_at_unix_ms)
            .is_none());

        let renewal = runtime
            .next_lease_renewal_command(started.sequence, now)
            .expect("near-expiry lease should renew");
        assert_eq!(renewal.expected_revision, Some(started.sequence));
        let WorkspaceCommandPayload::RenewExecutionLease {
            lease_id,
            lease_expires_at_unix_ms,
        } = renewal.payload
        else {
            unreachable!()
        };
        assert_eq!(lease_id, lease.id);
        assert_eq!(lease_expires_at_unix_ms, now + LEASE_RENEWAL_DURATION_MS);

        let mut renewed_lease = lease.clone();
        renewed_lease.expires_at_unix_ms = lease_expires_at_unix_ms;
        renewed_lease.updated_at_unix_ms = now;
        let renewed = WorkspaceEventEnvelope {
            event_id: WorkspaceEventId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            sequence: started.sequence + 1,
            project_id: started.project_id,
            work_item_id: started.work_item_id,
            actor_id: started.actor_id,
            device_id: started.device_id,
            task_id: None,
            correlation_id: started.correlation_id,
            causation_id: Some(WorkspaceCommandId::new()),
            timestamp_unix_ms: now,
            payload: WorkspaceEventPayload::ExecutionLeaseRenewed {
                agent_instance: agent_instance.clone(),
                lease: renewed_lease,
            },
        };
        renewed.validate().unwrap();
        runtime.observe_lease_renewed(&renewed);
        assert!(runtime
            .next_lease_renewal_command(started.sequence + 1, now)
            .is_none());

        let cancellation = runtime
            .cancellation_command(attempt_id, started.sequence + 1, now + 1)
            .unwrap();
        assert!(matches!(
            cancellation.payload,
            WorkspaceCommandPayload::CancelAgentExecution {
                attempt_id: cancelled,
                ..
            } if cancelled == attempt_id
        ));
        assert_eq!(runtime.active_count(), 1);

        let mut cancelled_attempt = attempt.clone();
        cancelled_attempt.status = ExecutionAttemptStatus::Cancelled;
        cancelled_attempt.finished_at_unix_ms = Some(now + 1);
        cancelled_attempt.updated_at_unix_ms = now + 1;
        let mut cancelled_agent = agent_instance.clone();
        cancelled_agent.status = AgentInstanceStatus::Cancelled;
        cancelled_agent.active_lease_id = None;
        cancelled_agent.updated_at_unix_ms = now + 1;
        let mut cancelled_lease = lease.clone();
        cancelled_lease.status = ExecutionLeaseStatus::Cancelled;
        cancelled_lease.updated_at_unix_ms = now + 1;
        let mut cancelled = started.clone();
        cancelled.sequence += 2;
        cancelled.timestamp_unix_ms = now + 1;
        let WorkspaceEventPayload::AgentExecutionStarted { plan, .. } = &started.payload else {
            unreachable!()
        };
        let mut cancelled_plan = plan.clone();
        cancelled_plan.tasks[0].status = PlanTaskStatus::Cancelled;
        cancelled.payload = WorkspaceEventPayload::AgentExecutionFinished {
            agent_instance: cancelled_agent,
            lease: cancelled_lease,
            attempt: cancelled_attempt,
            plan: cancelled_plan,
            summary: "Cancelled by the workspace owner.".into(),
        };
        runtime.observe_finished(&cancelled);
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn workspace_provider_run_id_is_stable_per_attempt() {
        let (started, _) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } = &started.payload else {
            unreachable!()
        };
        assert_eq!(
            workspace_run_id(attempt.id),
            "local-workspace-aaaaaaaa111141118111aaaaaaaaaaaa"
        );
        assert_eq!(workspace_run_id(attempt.id), workspace_run_id(attempt.id));
    }

    #[test]
    fn committed_start_is_claimed_once() {
        let (event, contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();

        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        assert_eq!(launch.start_revision, 17);
        assert_eq!(launch.task.title, "Implement billing");
        assert!(adapter.claim_started(&event, &contract).unwrap().is_none());
    }

    #[test]
    fn history_replay_does_not_relaunch_terminal_attempt() {
        let (started, contract) = fixture();
        let mut finished = started.clone();
        finished.sequence = 18;
        if let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        } = &started.payload
        {
            let mut attempt = attempt.clone();
            attempt.status = ExecutionAttemptStatus::Succeeded;
            attempt.finished_at_unix_ms = Some(started.timestamp_unix_ms + 1);
            attempt.updated_at_unix_ms = started.timestamp_unix_ms + 1;
            let mut agent_instance = agent_instance.clone();
            agent_instance.status = AgentInstanceStatus::Reviewing;
            agent_instance.updated_at_unix_ms = started.timestamp_unix_ms + 1;
            let mut lease = lease.clone();
            lease.updated_at_unix_ms = started.timestamp_unix_ms + 1;
            let mut plan = plan.clone();
            plan.tasks[0].status = PlanTaskStatus::Reviewing;
            plan.updated_at_unix_ms = started.timestamp_unix_ms + 1;
            finished.timestamp_unix_ms = started.timestamp_unix_ms + 1;
            finished.payload = WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance,
                lease,
                attempt,
                plan,
                summary: "Ready for review".into(),
            };
        }
        let mut adapter = WorkspaceExecutionAdapter::default();
        adapter.restore_history([&started, &finished]);

        assert!(adapter
            .claim_started(&started, &contract)
            .unwrap()
            .is_none());
    }

    #[test]
    fn workspace_runtime_mounts_replay_without_starting_provider_work() {
        let (started, contract) = fixture();
        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime.restore_history([&started]);

        assert_eq!(runtime.active_count(), 0);
        assert!(!runtime
            .mount_committed_start(&started, &contract, "/workspace/billing", "", "")
            .unwrap());
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn writable_execution_never_falls_back_to_the_primary_checkout() {
        let (started, contract) = fixture();
        let mut runtime = WorkspaceExecutionRuntime::default();
        let error = runtime
            .mount_committed_start(&started, &contract, "/workspace/billing", "", "")
            .unwrap_err();
        assert!(matches!(
            error,
            WorkspaceExecutionAdapterError::InvalidEvent(message)
                if message.contains("no prepared isolated checkout")
        ));
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn mount_errors_roll_back_claims_and_allow_preparation_retry_without_provider() {
        let (started, contract) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } = &started.payload else {
            unreachable!()
        };
        let mut runtime = WorkspaceExecutionRuntime::default();
        for _ in 0..2 {
            assert!(matches!(
                runtime.mount_committed_start(&started, &contract, "/workspace/billing", "", ""),
                Err(WorkspaceExecutionAdapterError::InvalidEvent(_))
            ));
            assert_eq!(
                runtime.adapter.attempts.get(&attempt.id),
                Some(&DispatchState::Unclaimed)
            );
            assert_eq!(runtime.active_count(), 0);
        }
        runtime.record_prepared_execution_root(
            attempt.id,
            "relative/checkout".into(),
            "baseline".into(),
        );
        for _ in 0..2 {
            assert_eq!(
                runtime.mount_committed_start(&started, &contract, "/workspace/billing", "", ""),
                Err(WorkspaceExecutionAdapterError::InvalidProjectRoot)
            );
            assert_eq!(
                runtime.adapter.attempts.get(&attempt.id),
                Some(&DispatchState::Unclaimed)
            );
        }
        runtime.record_prepared_execution_root(
            attempt.id,
            "/workspace/isolated".into(),
            "baseline".into(),
        );
        let request = runtime
            .prepare_committed_start(&started, &contract, "/workspace/billing".into(), "", "")
            .unwrap()
            .unwrap();
        assert_eq!(request.project_root, Path::new("/workspace/isolated"));
        assert_eq!(
            runtime.adapter.attempts.get(&attempt.id),
            Some(&DispatchState::Claimed)
        );
        assert!(!runtime
            .mount_committed_start(&started, &contract, "relative", "", "")
            .unwrap());
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn read_only_mount_error_can_be_retried_without_provider() {
        let (started, mut contract) = fixture();
        contract.write_scope.clear();
        let mut runtime = WorkspaceExecutionRuntime::default();
        assert_eq!(
            runtime.mount_committed_start(&started, &contract, "relative", "", ""),
            Err(WorkspaceExecutionAdapterError::InvalidProjectRoot)
        );
        let request = runtime
            .prepare_committed_start(&started, &contract, "/workspace/billing".into(), "", "")
            .unwrap()
            .unwrap();
        assert_eq!(request.project_root, Path::new("/workspace/billing"));
        assert_eq!(runtime.active_count(), 0);
    }

    #[test]
    fn restart_recovery_readopts_agent_cli_attempts_without_a_checkpoint() {
        let (mut started, mut contract) = fixture();
        contract.write_scope.clear();
        let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        } = &mut started.payload
        else {
            unreachable!()
        };
        attempt.model_route = dragent_workspace_model::agent_route("claude");
        agent_instance.model_route = attempt.model_route.clone();
        let (agent_instance, lease, attempt, plan) =
            (agent_instance.clone(), lease.clone(), attempt.clone(), plan.clone());
        let project = Project {
            organisation_id: "local".into(),
            id: started.project_id,
            root: PathBuf::from("/workspace/billing"),
            display_name: "billing".into(),
            created_at_unix_ms: started.timestamp_unix_ms - 1,
            updated_at_unix_ms: started.timestamp_unix_ms,
        };
        let work = WorkItem {
            id: started.work_item_id,
            project_id: started.project_id,
            title: "Billing".into(),
            status: WorkStatus::Executing,
            created_at_unix_ms: started.timestamp_unix_ms - 1,
            updated_at_unix_ms: started.timestamp_unix_ms,
        };
        let state = WorkspaceState::restore(
            project,
            Some(work),
            vec![
                WorkspaceEvent {
                    sequence: 1,
                    timestamp_unix_ms: started.timestamp_unix_ms,
                    project_id: started.project_id,
                    work_item_id: Some(started.work_item_id),
                    kind: WorkspaceEventKind::WorkContractUpdated {
                        work_contract: contract,
                    },
                },
                WorkspaceEvent {
                    sequence: 2,
                    timestamp_unix_ms: started.timestamp_unix_ms,
                    project_id: started.project_id,
                    work_item_id: Some(started.work_item_id),
                    kind: WorkspaceEventKind::AgentExecutionStarted {
                        agent_instance,
                        lease: lease.clone(),
                        attempt: attempt.clone(),
                        plan,
                    },
                },
            ],
        );
        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime.restore_history([&started]);
        let warnings = runtime.recover_interrupted_local_sessions_with(
            [&started],
            &state,
            Path::new("/workspace/billing"),
            started.timestamp_unix_ms + 1,
            |_| panic!("an agent CLI keeps no provider checkpoint to inspect"),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        // Mounted again, so its `dragent work submit` still lands, and handed
        // to the host as an agent CLI to re-adopt, not a controller to resume.
        assert_eq!(runtime.active_count(), 1);
        assert!(runtime.mounted_launch(attempt.id).is_some());
        assert_eq!(runtime.sessions.get(&attempt.id).unwrap().runtime_task_id, None);
        assert_eq!(runtime.take_recovered_cli_attempts(), vec![attempt.id]);
        assert!(runtime.take_recovered_host_attempts().is_empty());

        // An expired lease is still timed out and retried, as for any agent.
        let mut expired = WorkspaceExecutionRuntime::default();
        expired.restore_history([&started]);
        let warnings = expired.recover_interrupted_local_sessions_with(
            [&started],
            &state,
            Path::new("/workspace/billing"),
            lease.expires_at_unix_ms,
            |_| panic!("an expired execution is not recovered"),
        );
        assert_eq!(expired.active_count(), 0);
        assert!(expired.take_recovered_cli_attempts().is_empty());
        assert!(warnings.iter().any(|warning| warning.contains("lease is no longer active")));
    }

    #[test]
    fn restart_recovery_rebuilds_only_exact_canonical_parked_calls() {
        let (started, mut contract) = fixture();
        // This unit isolates recovery matching from Git worktree preparation.
        contract.write_scope.clear();
        let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        } = &started.payload
        else {
            unreachable!()
        };
        let attention = AttentionItem {
            id: AttentionItemId::new(),
            work_item_id: started.work_item_id,
            task_id: Some(attempt.task_id),
            thread_id: None,
            execution_attempt_id: Some(attempt.id),
            tool_call_id: Some("parked-call".into()),
            kind: AttentionKind::Permission,
            status: AttentionStatus::Open,
            summary: "Approve verification".into(),
            requested_by: started.actor_id,
            assigned_to: vec![started.actor_id],
            created_at_unix_ms: started.timestamp_unix_ms,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        let project = Project {
            organisation_id: "local".into(),
            id: started.project_id,
            root: PathBuf::from("/workspace/billing"),
            display_name: "billing".into(),
            created_at_unix_ms: started.timestamp_unix_ms - 1,
            updated_at_unix_ms: started.timestamp_unix_ms,
        };
        let work = WorkItem {
            id: started.work_item_id,
            project_id: started.project_id,
            title: "Billing".into(),
            status: WorkStatus::Executing,
            created_at_unix_ms: started.timestamp_unix_ms - 1,
            updated_at_unix_ms: started.timestamp_unix_ms,
        };
        let state = WorkspaceState::restore(
            project,
            Some(work),
            vec![
                WorkspaceEvent {
                    sequence: 1,
                    timestamp_unix_ms: started.timestamp_unix_ms,
                    project_id: started.project_id,
                    work_item_id: Some(started.work_item_id),
                    kind: WorkspaceEventKind::WorkContractUpdated {
                        work_contract: contract,
                    },
                },
                WorkspaceEvent {
                    sequence: 2,
                    timestamp_unix_ms: started.timestamp_unix_ms,
                    project_id: started.project_id,
                    work_item_id: Some(started.work_item_id),
                    kind: WorkspaceEventKind::AgentExecutionStarted {
                        agent_instance: agent_instance.clone(),
                        lease: lease.clone(),
                        attempt: attempt.clone(),
                        plan: plan.clone(),
                    },
                },
                WorkspaceEvent {
                    sequence: 3,
                    timestamp_unix_ms: started.timestamp_unix_ms,
                    project_id: started.project_id,
                    work_item_id: Some(started.work_item_id),
                    kind: WorkspaceEventKind::AttentionUpdated {
                        attention_item: attention,
                    },
                },
            ],
        );
        let expiry = next_expired_execution_command(
            &state,
            started.actor_id,
            started.device_id,
            started.sequence,
            lease.expires_at_unix_ms,
        )
        .expect("expired running lease should become a durable timeout");
        assert!(matches!(
            expiry.payload,
            WorkspaceCommandPayload::FinishAgentExecution {
                attempt_id,
                outcome: ExecutionAttemptStatus::TimedOut,
                ..
            } if attempt_id == attempt.id
        ));

        let mut expired_runtime = WorkspaceExecutionRuntime::default();
        expired_runtime.restore_history([&started]);
        let expired_warnings = expired_runtime.recover_interrupted_local_sessions_with(
            [&started],
            &state,
            Path::new("/workspace/billing"),
            lease.expires_at_unix_ms,
            |_| panic!("expired execution must not inspect or recover its provider"),
        );
        assert_eq!(expired_runtime.active_count(), 0);
        assert!(expired_warnings
            .iter()
            .any(|warning| warning.contains("lease is no longer active")));

        let mut mismatched_runtime = WorkspaceExecutionRuntime::default();
        mismatched_runtime.restore_history([&started]);
        let mismatch = mismatched_runtime.recover_interrupted_local_sessions_with(
            [&started],
            &state,
            Path::new("/workspace/billing"),
            started.timestamp_unix_ms + 1,
            |_| {
                Ok(
                    crate::agent::pipeline::LocalRunRecoveryState::AwaitingTools(vec![
                        crate::agent::pipeline::RecoveredLocalTool {
                            call_id: "different-call".into(),
                            tool_name: "run_command".into(),
                            args_json: "{}".into(),
                        },
                    ]),
                )
            },
        );
        assert_eq!(mismatched_runtime.active_count(), 0);
        assert!(mismatch
            .iter()
            .any(|warning| warning.contains("do not exactly match canonical Attention")));

        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime.restore_history([&started]);
        let expected_run_id = workspace_run_id(attempt.id);
        let warnings = runtime.recover_interrupted_local_sessions_with(
            [&started],
            &state,
            Path::new("/workspace/billing"),
            started.timestamp_unix_ms + 1,
            |run_id| {
                assert_eq!(run_id, expected_run_id);
                Ok(
                    crate::agent::pipeline::LocalRunRecoveryState::AwaitingTools(vec![
                        crate::agent::pipeline::RecoveredLocalTool {
                            call_id: "parked-call".into(),
                            tool_name: "run_command".into(),
                            args_json: r#"{"command":"cargo test"}"#.into(),
                        },
                    ]),
                )
            },
        );

        assert!(warnings.is_empty());
        assert_eq!(runtime.active_count(), 1);
        let recovered = runtime.sessions.get(&attempt.id).unwrap();
        assert_eq!(
            recovered.runtime_task_id.as_deref(),
            Some(expected_run_id.as_str())
        );
        let handed_off = runtime.take_recovered_host_attempts();
        assert_eq!(handed_off.len(), 1);
        assert_eq!(handed_off[0].attempt_id, attempt.id);
        assert_eq!(handed_off[0].runtime_task_id, expected_run_id);
        assert_eq!(handed_off[0].parked.len(), 1);
        assert_eq!(handed_off[0].parked[0].call_id, "parked-call");
        assert_eq!(
            handed_off[0].parked[0].args_json,
            r#"{"command":"cargo test"}"#
        );
        assert!(runtime.take_recovered_host_attempts().is_empty());
    }

    #[test]
    fn finish_uses_latest_parallel_revision_and_preserves_correlation() {
        let (event, contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let command = adapter
            .finish_command(
                &launch,
                WorkspaceExecutionCompletion::succeeded("  Ready for review.  "),
                24,
                event.timestamp_unix_ms + 10,
            )
            .unwrap();

        assert_eq!(command.expected_revision, Some(24));
        assert_eq!(command.correlation_id, event.correlation_id);
        assert_eq!(command.actor_id, event.actor_id);
        assert!(matches!(
            command.payload,
            WorkspaceCommandPayload::FinishAgentExecution {
                outcome: ExecutionAttemptStatus::Succeeded,
                ref summary,
                ..
            } if summary == "Ready for review."
        ));
        command.validate().unwrap();
    }

    #[test]
    fn provider_failure_becomes_a_terminal_attributable_workspace_command() {
        let (event, contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let command = adapter
            .finish_command(
                &launch,
                WorkspaceExecutionCompletion::failed(
                    "  Provider request failed after the response stream closed.  ",
                ),
                31,
                event.timestamp_unix_ms + 50,
            )
            .unwrap();

        assert_eq!(command.expected_revision, Some(31));
        assert_eq!(command.correlation_id, event.correlation_id);
        assert_eq!(command.actor_id, event.actor_id);
        assert!(matches!(
            command.payload,
            WorkspaceCommandPayload::FinishAgentExecution {
                attempt_id,
                outcome: ExecutionAttemptStatus::Failed,
                ref summary,
                ..
            } if attempt_id == launch.attempt.id
                && summary == "Provider request failed after the response stream closed."
        ));
        command.validate().unwrap();
    }

    #[test]
    fn contract_mismatch_does_not_consume_start() {
        let (event, mut contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        contract.task_ids.clear();
        assert_eq!(
            adapter.claim_started(&event, &contract).unwrap_err(),
            WorkspaceExecutionAdapterError::ContractMismatch
        );
        contract.task_ids.push(match &event.payload {
            WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } => attempt.task_id,
            _ => unreachable!(),
        });
        assert!(adapter.claim_started(&event, &contract).unwrap().is_some());
    }

    #[test]
    fn finished_execution_keeps_checkpoint_available_for_coordinate_restore() {
        let (started, contract) = fixture();
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("app/src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "before\n").unwrap();

        let mut session = pipeline_session(contract);
        session.request.project_root = directory.path().to_path_buf();
        let attempt_id = session.attempt_id();
        session.checkpoint_message_id = checkpoint_message_id(attempt_id);
        session
            .checkpoints
            .capture(session.checkpoint_message_id, &file);
        std::fs::write(&file, "after\n").unwrap();

        let mut runtime = WorkspaceExecutionRuntime::default();
        session
            .sender
            .send(Ok(stream_event(
                crate::agent::pipeline::AgentEventKind::TextDelta,
                "Completed",
            )))
            .unwrap();
        session
            .sender
            .send(Ok(stream_event(
                crate::agent::pipeline::AgentEventKind::Done,
                "",
            )))
            .unwrap();
        runtime.sessions.insert(attempt_id, session);
        runtime.try_next_actions();
        let activity = runtime.activity(attempt_id);
        assert_eq!(activity.len(), 2);
        let mut finished = started.clone();
        if let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        } = &started.payload
        {
            let mut attempt = attempt.clone();
            attempt.status = ExecutionAttemptStatus::Succeeded;
            finished.payload = WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance: agent_instance.clone(),
                lease: lease.clone(),
                attempt,
                plan: plan.clone(),
                summary: "Completed".into(),
            };
        }
        runtime.observe_finished(&finished);

        assert_eq!(runtime.active_count(), 0);
        assert_eq!(runtime.activity(attempt_id), activity);
        assert!(runtime.try_next_actions().is_empty());
        assert_eq!(
            runtime.checkpoint_summaries(),
            [WorkspaceCheckpointSummary {
                attempt_id,
                task_title: "Implement billing".into(),
                changed_files: 1,
                added_lines: 1,
                deleted_lines: 1,
            }]
        );
        assert_eq!(
            runtime.restore_checkpoint(attempt_id).as_deref(),
            Some("Restored checkpoint: 1 file restored.")
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), "before\n");
    }

    #[test]
    fn revision_attempt_adopts_the_reviewed_checkout_without_parallel_ownership() {
        let (started, _) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } = &started.payload else {
            unreachable!()
        };
        let previous_attempt_id = attempt.id;
        let task_id = attempt.task_id;
        let next_attempt_id = ExecutionAttemptId::new();
        let root = PathBuf::from("/workspace/.dragent/revision-checkout");
        let mut runtime = WorkspaceExecutionRuntime::default();
        runtime.record_prepared_execution_root(
            previous_attempt_id,
            root.clone(),
            "baseline-commit".into(),
        );

        assert!(runtime.retain_revision_root(task_id, previous_attempt_id));
        assert!(runtime
            .prepared_execution_root(previous_attempt_id)
            .is_none());
        assert!(runtime.adopt_revision_root(task_id, next_attempt_id));
        assert_eq!(
            runtime.prepared_execution_root(next_attempt_id),
            Some(root.as_path())
        );
        assert_eq!(
            runtime
                .prepared_execution_isolation(next_attempt_id)
                .and_then(|isolation| isolation.base_revision.as_deref()),
            Some("baseline-commit")
        );
        assert_ne!(
            runtime
                .prepared_execution_isolation(next_attempt_id)
                .unwrap()
                .id,
            ExecutionIsolation::git_worktree(previous_attempt_id, "baseline-commit".into()).id
        );
    }

    #[test]
    fn watchdog_fires_on_silent_executing_with_stable_identity() {
        let (started, _) = fixture();
        let task_id = PlanTaskId::new();
        let contract_id = WorkContractId::new();
        let plan = Plan {
            id: PlanId::new(),
            work_item_id: started.work_item_id,
            version: 1,
            status: PlanStatus::Approved,
            tasks: vec![PlanTask {
                id: task_id,
                plan_id: PlanId::new(),
                title: "Cancelled work".into(),
                description: "No runtime activity remains.".into(),
                status: PlanTaskStatus::Cancelled,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: Vec::new(),
                assigned_contract_id: Some(contract_id),
            }],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let mut contract = controller_contract(started.work_item_id, task_id, None);
        contract.id = contract_id;
        // Executing long past the grace period with no attempts at all: the
        // classic "start committed but no provider session came up".
        let now = 1_000_000;
        let state = controller_state(
            WorkStatus::Executing,
            now - COORDINATION_STALL_GRACE_MS - 1,
            plan,
            vec![contract],
            Vec::new(),
        );
        assert_eq!(
            state.coordination_health(3, now),
            CoordinationHealth::Stalled
        );
        let action = next_stall_attention(&state, 3, now).expect("stall must escalate");
        let WorkspaceLaunchAction::EscalateFailure { attention_item } = action else {
            unreachable!()
        };
        assert_eq!(attention_item.kind, AttentionKind::Failure);
        assert!(attention_item.summary.contains("Next step"));
        // Idempotency: once recorded, the same tick raises nothing.
        let state = controller_state(
            WorkStatus::Executing,
            now - COORDINATION_STALL_GRACE_MS - 1,
            state.latest_structured_plan().unwrap().clone(),
            state.work_contracts().into_iter().cloned().collect(),
            vec![WorkspaceEventKind::AttentionUpdated { attention_item }],
        );
        assert!(next_stall_attention(&state, 3, now).is_none());
    }

    #[test]
    fn watchdog_stays_quiet_while_retry_is_pending() {
        // Failed recently: the deterministic retry delay has not elapsed, so
        // the controller reports Waiting and the watchdog must not fire.
        let (state, _, _, _, finished_at) =
            terminal_launch_state(ExecutionAttemptStatus::Failed, PlanTaskStatus::Failed, 1);
        let now = finished_at + 1_000;
        assert_eq!(
            state.coordination_health(4, now),
            CoordinationHealth::Waiting
        );
        assert!(next_stall_attention(&state, 4, now).is_none());
    }
}
