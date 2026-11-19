//! Tests for launch (moved verbatim).

#[cfg(test)]
mod execution_launch_tests {
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
    fn execution_route_prefers_byok_without_fallback() {
        // Explicit route with a user credential wins, regardless of account.
        let (route, mode) =
            resolve_execution_route("anthropic/claude-opus", false, None, "free-tier", |model| {
                model == "anthropic/claude-opus"
            })
            .unwrap();
        assert_eq!(route, "anthropic/claude-opus");
        assert_eq!(mode, ChargingMode::BringYourOwnKey);
    }

    #[test]
    fn execution_route_uses_free_tier_for_free_accounts() {
        let free = billing(true, false, &["free-tier"]);
        // AUTO and empty both resolve to the platform free-tier route.
        for selected in ["auto", "", "   "] {
            let (route, mode) =
                resolve_execution_route(selected, false, Some(&free), "free-tier", |_| false)
                    .unwrap();
            assert_eq!(route, "free-tier");
            assert_eq!(mode, ChargingMode::PlatformCredits);
        }
    }

    #[test]
    fn execution_route_honors_paid_catalog_without_substitution() {
        let paid = billing(false, true, &["pro-a", "pro-b"]);
        // Explicit catalog model passes through untouched.
        let (route, mode) =
            resolve_execution_route("pro-b", false, Some(&paid), "free-tier", |_| false).unwrap();
        assert_eq!(route, "pro-b");
        assert_eq!(mode, ChargingMode::PlatformCredits);
        // Off-catalog models fail loudly instead of silently substituting.
        let err = resolve_execution_route("ultra-z", false, Some(&paid), "free-tier", |_| false)
            .unwrap_err();
        assert!(err.contains("ultra-z") && err.contains("pro-a"));
        // Paid AUTO falls back to the documented free-tier default.
        let (route, _) =
            resolve_execution_route("auto", false, Some(&paid), "free-tier", |_| false).unwrap();
        assert_eq!(route, "free-tier");
    }

    #[test]
    fn execution_route_blocks_unbillable_cloud_use() {
        // Logged out with no credential: nobody to bill.
        let err =
            resolve_execution_route("pro-a", false, None, "free-tier", |_| false).unwrap_err();
        assert!(err.contains("Sign in"));
    }

    #[test]
    fn execution_route_keeps_local_compute_local() {
        let (route, mode) =
            resolve_execution_route("auto", true, None, "free-tier", |_| false).unwrap();
        assert_eq!(route, "free-tier");
        assert_eq!(mode, ChargingMode::LocalCompute);
        let (route, mode) =
            resolve_execution_route("ollama/qwen", false, None, "free-tier", |_| false).unwrap();
        assert_eq!(route, "ollama/qwen");
        assert_eq!(mode, ChargingMode::LocalCompute);
    }

    #[test]
    fn launch_planner_advances_one_recoverable_authority_step_at_a_time() {
        let (state, policy, task_id, contract_id) =
            launch_state(PlanTaskStatus::Proposed, None, None);
        assert_eq!(
            next_workspace_launch_action(&state, &policy, 1_788_796_900_000).unwrap(),
            Some(WorkspaceLaunchAction::MarkTaskReady { task_id })
        );

        let (state, policy, _, _) = launch_state(PlanTaskStatus::Ready, None, None);
        assert_eq!(
            next_workspace_launch_action(&state, &policy, 1_788_796_900_000).unwrap(),
            Some(WorkspaceLaunchAction::ProvisionAgent {
                task_id,
                contract_id,
                agent_instance_id: stable_agent_instance_id(contract_id),
                model_route: "ollama/qwen3-coder".into(),
            })
        );
    }

    #[test]
    fn launch_planner_resumes_from_provisioned_and_activated_state() {
        let (_, _, task_id, contract_id) = launch_state(PlanTaskStatus::Ready, None, None);
        let agent_id = stable_agent_instance_id(contract_id);
        let (started, mut contract) = fixture();
        contract.id = contract_id;
        contract.agent_instance_id = Some(agent_id);
        contract.status = WorkContractStatus::Approved;
        let mut agent = match started.payload {
            WorkspaceEventPayload::AgentExecutionStarted { agent_instance, .. } => agent_instance,
            _ => unreachable!(),
        };
        agent.id = agent_id;
        agent.contract_id = contract_id;
        agent.status = AgentInstanceStatus::Pending;
        agent.active_lease_id = None;
        agent.started_at_unix_ms = None;
        let (state, policy, _, _) = launch_state(
            PlanTaskStatus::Ready,
            Some((agent.clone(), contract.clone(), None)),
            None,
        );
        let now = 1_788_796_900_000;
        let lease_id = stable_initial_lease_id(agent_id);
        assert_eq!(
            next_workspace_launch_action(&state, &policy, now).unwrap(),
            Some(WorkspaceLaunchAction::ActivateAgent {
                task_id,
                agent_instance_id: agent_id,
                lease_id,
                effective_model_route: "ollama/qwen3-coder".into(),
                charging: policy.charging.clone(),
                lease_expires_at_unix_ms: now + 10 * 60 * 1_000,
            })
        );

        agent.status = AgentInstanceStatus::Ready;
        agent.active_lease_id = Some(lease_id);
        contract.status = WorkContractStatus::Active;
        let lease = ExecutionLease {
            id: lease_id,
            work_item_id: agent.work_item_id,
            agent_instance_id: agent_id,
            contract_id,
            status: ExecutionLeaseStatus::Active,
            charging: policy.charging.clone(),
            acquired_at_unix_ms: now - 1,
            expires_at_unix_ms: now + 10 * 60 * 1_000,
            updated_at_unix_ms: now,
        };
        let (state, policy, _, _) = launch_state(
            PlanTaskStatus::Ready,
            Some((agent, contract, Some(lease))),
            None,
        );
        assert_eq!(
            next_workspace_launch_action(&state, &policy, now).unwrap(),
            Some(WorkspaceLaunchAction::StartExecution {
                task_id,
                agent_instance_id: agent_id,
                lease_id,
                attempt_id: stable_attempt_id(task_id, 1),
            })
        );
    }

    #[test]
    fn launch_policy_rejects_unresolved_auto_route() {
        let (state, mut policy, _, _) = launch_state(PlanTaskStatus::Ready, None, None);
        policy.effective_model_route = "auto".into();
        assert_eq!(
            next_workspace_launch_action(&state, &policy, 1_788_796_900_000),
            Err(WorkspaceExecutionAdapterError::UnresolvedModelRoute)
        );
    }

    #[test]
    fn launch_planner_prefers_the_model_bound_to_the_role_contract() {
        let (state, policy, task_id, contract_id) =
            launch_state(PlanTaskStatus::Ready, None, Some("anthropic/claude-review"));

        assert_eq!(
            next_workspace_launch_action(&state, &policy, 1_788_796_900_000).unwrap(),
            Some(WorkspaceLaunchAction::ProvisionAgent {
                task_id,
                contract_id,
                agent_instance_id: stable_agent_instance_id(contract_id),
                model_route: "anthropic/claude-review".into(),
            })
        );
    }

    #[test]
    fn launch_planner_falls_back_to_policy_route_for_auto_contracts() {
        // Auto (None) is server-resolved: provisioning fills the effective
        // policy route so the provider session runs the selected default.
        let (state, policy, task_id, contract_id) = launch_state(PlanTaskStatus::Ready, None, None);
        assert_eq!(
            next_workspace_launch_action(&state, &policy, 1_788_796_900_000).unwrap(),
            Some(WorkspaceLaunchAction::ProvisionAgent {
                task_id,
                contract_id,
                agent_instance_id: stable_agent_instance_id(contract_id),
                model_route: policy.effective_model_route.clone(),
            })
        );
    }

    #[test]
    fn launch_planner_retries_failures_only_after_backoff_and_with_stable_identity() {
        let (state, policy, failed_attempt_id, agent_id, finished_at) =
            terminal_launch_state(ExecutionAttemptStatus::Failed, PlanTaskStatus::Failed, 1);
        let retry_at = finished_at + execution_retry_delay_ms(1);
        assert_eq!(next_workspace_retry_wait_ms(&state, retry_at - 1), Some(1));
        assert_eq!(
            next_workspace_launch_action(&state, &policy, retry_at - 1).unwrap(),
            None
        );
        assert_eq!(
            next_workspace_launch_action(&state, &policy, retry_at).unwrap(),
            Some(WorkspaceLaunchAction::PrepareRetry {
                failed_attempt_id,
                lease_id: stable_retry_lease_id(agent_id, 2),
                lease_expires_at_unix_ms: retry_at + policy.lease_duration_ms,
            })
        );
    }

    #[test]
    fn launch_planner_never_retries_pause_or_cancelled_attempts() {
        // Paused work never advances on its own.
        let (state, policy, _, _, finished_at) =
            terminal_launch_state(ExecutionAttemptStatus::Paused, PlanTaskStatus::Ready, 1);
        assert_eq!(
            next_workspace_launch_action(&state, &policy, finished_at + 60_000).unwrap(),
            None
        );
        // A cancelled task is never retried — but work left Executing with
        // nothing else running is a silent stall, so the watchdog raises an
        // actionable attention instead of returning idle None.
        let (state, policy, _, _, finished_at) = terminal_launch_state(
            ExecutionAttemptStatus::Cancelled,
            PlanTaskStatus::Cancelled,
            1,
        );
        let action = next_workspace_launch_action(&state, &policy, finished_at + 60_000)
            .unwrap()
            .expect("cancelled-terminal executing work must raise stall attention");
        assert!(
            matches!(action, WorkspaceLaunchAction::EscalateFailure { .. }),
            "cancelled work must escalate, never retry"
        );
    }

    #[test]
    fn launch_planner_escalates_an_exhausted_failure_once_to_the_principal() {
        let (state, policy, attempt_id, _, finished_at) = terminal_launch_state(
            ExecutionAttemptStatus::Failed,
            PlanTaskStatus::Failed,
            MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
        );
        let action = next_workspace_launch_action(&state, &policy, finished_at + 1).unwrap();
        let Some(WorkspaceLaunchAction::EscalateFailure { attention_item }) = action else {
            panic!("an exhausted failure must raise Principal attention")
        };
        assert_eq!(attention_item.kind, AttentionKind::Failure);
        assert_eq!(attention_item.execution_attempt_id, Some(attempt_id));
        // Assigned to the Principal (its coordination turn may authorize the
        // retry) AND the human (Retry in the UI), so an exhausted task is
        // never a dead end.
        assert_eq!(attention_item.assigned_to.len(), 2);
        assert_eq!(
            attention_item.assigned_to[0],
            dragent_workspace_model::ActorRef::principal_agent(state.project().id).id
        );
        assert_eq!(
            attention_item.assigned_to[1],
            dragent_workspace_model::ActorRef::local_user(state.project().id, None).id
        );
        // Regression guard: without a thread id this escalation is invisible
        // in the unified Conversation scrollback — reachable only through the
        // bell popover, with no way to see the worker's own diagnosis
        // alongside it.
        assert_eq!(
            attention_item.thread_id,
            Some(dragent_workspace_model::task_coordination_thread_id(
                attention_item.task_id.expect("escalation is task-scoped")
            ))
        );
    }

    #[test]
    fn resolving_an_exhausted_failure_authorizes_exactly_one_more_attempt() {
        let (mut state, policy, attempt_id, agent_id, finished_at) = terminal_launch_state(
            ExecutionAttemptStatus::Failed,
            PlanTaskStatus::Failed,
            MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
        );
        // Still exhausted and unresolved: escalates, never retries on its own.
        assert!(matches!(
            next_workspace_launch_action(&state, &policy, finished_at + 1_000_000).unwrap(),
            Some(WorkspaceLaunchAction::EscalateFailure { .. })
        ));

        let principal = dragent_workspace_model::ActorRef::principal_agent(state.project().id).id;
        let task_id = state.latest_structured_plan().expect("plan restored").tasks[0].id;
        let work_item_id = state.active_work().expect("work restored").id;
        // The human resolves the escalation (e.g. re-authenticated, fixed the
        // contract) — mediated through the same attention-resolution path
        // every other coordination decision already uses, not a new bypass.
        state.record(
            3,
            WorkspaceEventKind::AttentionUpdated {
                attention_item: AttentionItem {
                    id: AttentionItemId(uuid::Uuid::new_v5(
                        &attempt_id.0,
                        b"execution-retry-exhausted:v1",
                    )),
                    work_item_id,
                    task_id: Some(task_id),
                    thread_id: Some(dragent_workspace_model::task_coordination_thread_id(
                        task_id,
                    )),
                    execution_attempt_id: Some(attempt_id),
                    tool_call_id: None,
                    kind: AttentionKind::Failure,
                    status: AttentionStatus::Resolved,
                    summary: "test".into(),
                    requested_by: principal,
                    assigned_to: vec![principal],
                    created_at_unix_ms: finished_at,
                    resolved_at_unix_ms: Some(finished_at + 1),
                    resolution_kind: Some(AttentionResolution::Answered),
                    resolution: Some("re-authenticated".into()),
                },
            },
        );
        assert_eq!(
            next_workspace_launch_action(&state, &policy, finished_at + 1_000_000).unwrap(),
            Some(WorkspaceLaunchAction::PrepareRetry {
                failed_attempt_id: attempt_id,
                lease_id: stable_retry_lease_id(agent_id, MAX_AUTOMATIC_EXECUTION_ATTEMPTS + 1),
                lease_expires_at_unix_ms: finished_at + 1_000_000 + policy.lease_duration_ms,
            })
        );
    }

    #[test]
    fn dismissing_an_exhausted_failure_does_not_authorize_a_retry() {
        let (mut state, policy, attempt_id, _, finished_at) = terminal_launch_state(
            ExecutionAttemptStatus::Failed,
            PlanTaskStatus::Failed,
            MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
        );
        let principal = dragent_workspace_model::ActorRef::principal_agent(state.project().id).id;
        let task_id = state.latest_structured_plan().expect("plan restored").tasks[0].id;
        let work_item_id = state.active_work().expect("work restored").id;
        // Dismissing (vs. resolving) is "I saw this, not addressing it" — it
        // must not silently grant another attempt.
        state.record(
            3,
            WorkspaceEventKind::AttentionUpdated {
                attention_item: AttentionItem {
                    id: AttentionItemId(uuid::Uuid::new_v5(
                        &attempt_id.0,
                        b"execution-retry-exhausted:v1",
                    )),
                    work_item_id,
                    task_id: Some(task_id),
                    thread_id: Some(dragent_workspace_model::task_coordination_thread_id(
                        task_id,
                    )),
                    execution_attempt_id: Some(attempt_id),
                    tool_call_id: None,
                    kind: AttentionKind::Failure,
                    status: AttentionStatus::Dismissed,
                    summary: "test".into(),
                    requested_by: principal,
                    assigned_to: vec![principal],
                    created_at_unix_ms: finished_at,
                    resolved_at_unix_ms: Some(finished_at + 1),
                    resolution_kind: Some(AttentionResolution::Dismissed),
                    resolution: Some("not now".into()),
                },
            },
        );
        // Dedup already skips re-raising the same dismissed escalation, and
        // this test's real claim is narrower than "nothing at all happens"
        // (a stall watchdog elsewhere in this function may still legitimately
        // fire once nothing is scheduled) — only that dismissal never grants
        // the retry itself.
        assert!(!matches!(
            next_workspace_launch_action(&state, &policy, finished_at + 1_000_000).unwrap(),
            Some(WorkspaceLaunchAction::PrepareRetry { .. })
        ));
    }

    #[test]
    fn write_scope_conflicts_are_hierarchical_and_read_only_scopes_are_shared() {
        assert!(write_scopes_overlap(
            &[PathBuf::from("app/src")],
            &[PathBuf::from("app/src/workspace")],
        ));
        assert!(!write_scopes_overlap(
            &[PathBuf::from("app/src")],
            &[PathBuf::from("docs/product")],
        ));
        assert!(!write_scopes_overlap(&[], &[PathBuf::from("app/src")]));
    }

    #[test]
    fn billing_park_is_stable_and_resolvable_by_owner() {
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
                title: "Parked".into(),
                description: "Waiting on billing.".into(),
                status: PlanTaskStatus::Ready,
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
        let now = 500_000;
        let state = controller_state(
            WorkStatus::ReadyToExecute,
            now,
            plan,
            vec![contract],
            Vec::new(),
        );
        let action = billing_park_attention(&state, "pro-model", now).expect("billing must park");
        let WorkspaceLaunchAction::EscalateFailure { attention_item } = action else {
            unreachable!()
        };
        assert_eq!(attention_item.kind, AttentionKind::Budget);
        assert!(attention_item.summary.contains("Next step"));
        // Parked, not cancelled: the owner (not just the Principal) can
        // resolve it once billing is restored, and resume reuses identities.
        let owner = ActorRef::local_user(started.project_id, None).id;
        assert!(attention_item.assigned_to.contains(&owner));
        let first_id = attention_item.id;
        let state = controller_state(
            WorkStatus::ReadyToExecute,
            now,
            state.latest_structured_plan().unwrap().clone(),
            state.work_contracts().into_iter().cloned().collect(),
            vec![WorkspaceEventKind::AttentionUpdated { attention_item }],
        );
        assert!(billing_park_attention(&state, "pro-model", now).is_none());
        // Stable identity: a fresh omission would reuse the same id.
        let _ = first_id;
    }

    #[test]
    fn handoff_reaches_dependent_agents_without_principal_relay() {
        let (started, _) = fixture();
        let worker_task = PlanTaskId::new();
        let dependent_task = PlanTaskId::new();
        let worker_contract = WorkContractId::new();
        let dependent_contract = WorkContractId::new();
        let worker_agent = AgentInstanceId::new();
        let dependent_agent = AgentInstanceId::new();
        let plan_id = PlanId::new();
        let plan = Plan {
            id: plan_id,
            work_item_id: started.work_item_id,
            version: 1,
            status: PlanStatus::Approved,
            tasks: vec![
                PlanTask {
                    id: worker_task,
                    plan_id,
                    title: "Backend".into(),
                    description: "Build it.".into(),
                    status: PlanTaskStatus::Running,
                    depends_on: Vec::new(),
                    review_target_task_id: None,
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: Some(worker_contract),
                },
                PlanTask {
                    id: dependent_task,
                    plan_id,
                    title: "Frontend".into(),
                    description: "Consume the backend handoff.".into(),
                    status: PlanTaskStatus::Proposed,
                    depends_on: vec![worker_task],
                    review_target_task_id: None,
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: Some(dependent_contract),
                },
            ],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let mut worker = controller_contract(started.work_item_id, worker_task, Some(worker_agent));
        worker.id = worker_contract;
        let mut dependent =
            controller_contract(started.work_item_id, dependent_task, Some(dependent_agent));
        dependent.id = dependent_contract;
        let state = controller_state(
            WorkStatus::Executing,
            5_000,
            plan,
            vec![worker, dependent],
            Vec::new(),
        );
        let attempt = ExecutionAttemptId::new();
        let recipients = handoff_recipient_ids(&state, worker_task, attempt);
        let principal = ActorRef::principal_agent(started.project_id).id;
        let dependent_actor = ActorRef::agent_instance(dependent_agent, ActorKind::WorkerAgent).id;
        // Backend contract: typed handoff to frontend's agent plus the
        // Principal copy. No capability is granted by the message itself.
        assert!(recipients.contains(&principal));
        assert!(recipients.contains(&dependent_actor));
        assert_eq!(recipients.first(), Some(&principal));
    }

    #[test]
    fn blocker_prose_never_becomes_success() {
        assert!(completion_reports_blocker(
            "Blocked: the execution lease expired before the task could be completed."
        ));
        assert!(completion_reports_blocker(
            "I could not safely modify the project: no file-editing tool is available."
        ));
        assert!(!completion_reports_blocker(
            "Implemented the endpoint with tests; all checks pass."
        ));
    }
}
