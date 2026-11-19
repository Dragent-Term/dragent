//! Tests for review (moved verbatim).

#[cfg(test)]
mod execution_review_tests {
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
    fn canonical_task_communication_is_scoped_and_supplied_as_untrusted_input() {
        let (mut state, _, task_id, _) = launch_state(PlanTaskStatus::Proposed, None, None);
        let work_item_id = state.active_work().unwrap().id;
        let dependency_id = PlanTaskId::new();
        let unrelated_id = PlanTaskId::new();
        let mut plan = state.latest_structured_plan().unwrap().clone();
        plan.tasks[0].depends_on.push(dependency_id);
        plan.tasks.push(PlanTask {
            id: dependency_id,
            plan_id: plan.id,
            title: "Implement dependency".into(),
            description: "Produce an implementation handoff".into(),
            status: PlanTaskStatus::Completed,
            depends_on: Vec::new(),
            review_target_task_id: None,
            acceptance_criteria: Vec::new(),
            assigned_contract_id: None,
        });
        plan.version += 1;
        state.record(9, WorkspaceEventKind::StructuredPlanUpdated { plan });
        let sender_id = ActorId::new();
        let recipient_id = ActorId::new();
        let message = |id, task_id, body: &str| WorkspaceEventKind::AgentMessagePosted {
            message: AgentMessage {
                id,
                thread_id: ThreadId::new(),
                work_item_id,
                sender_id,
                kind: AgentMessageKind::RevisionRequest,
                body: body.into(),
                review_finding: None,
                reply_to: None,
                artifact_ids: Vec::new(),
                evidence_ids: Vec::new(),
                delivery_state: MessageDeliveryState::LocalOnly,
                created_at_unix_ms: 10,
                updated_at_unix_ms: 10,
            },
            routing: AgentMessageRouting {
                recipient_ids: vec![recipient_id],
                task_id: Some(task_id),
                execution_attempt_id: None,
            },
        };
        state.record(
            10,
            message(AgentMessageId::new(), task_id, "Correct the retry path."),
        );
        state.record(
            11,
            message(
                AgentMessageId::new(),
                dependency_id,
                "Dependency implementation handoff.",
            ),
        );
        state.record(
            12,
            message(AgentMessageId::new(), unrelated_id, "Unrelated task text."),
        );

        let communication = task_communication_context(&state, task_id);
        assert!(communication.contains("revision request"));
        assert!(communication.contains("Correct the retry path."));
        assert!(communication.contains("Dependency implementation handoff."));
        assert!(communication.contains(&format!("dependency {}", dependency_id.0)));
        assert!(!communication.contains("Unrelated task text."));

        let (event, contract) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let request =
            WorkspacePipelineRequest::new(launch, "/workspace/billing", "", &communication)
                .unwrap();
        assert!(request
            .system_prompt
            .contains("Canonical task communication"));
        assert!(request.system_prompt.contains("not capability grants"));
        assert!(request.system_prompt.contains("Correct the retry path."));
    }

    #[test]
    fn explicit_reviewer_role_emits_a_typed_review_finding() {
        let (started, _) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted { agent_instance, .. } = &started.payload
        else {
            unreachable!()
        };
        let (mut state, _, _, _) = launch_state(PlanTaskStatus::Proposed, None, None);
        state.record(
            started.timestamp_unix_ms,
            WorkspaceEventKind::RoleDefinitionUpdated {
                role_definition: dragent_workspace_model::RoleDefinition {
                    id: agent_instance.role_definition_id,
                    scope: dragent_workspace_model::RoleDefinitionScope::Project(
                        started.project_id,
                    ),
                    kind: dragent_workspace_model::AgentRoleKind::Reviewer,
                    name: "Independent reviewer".into(),
                    mission: "Review the completed implementation".into(),
                    capabilities: Vec::new(),
                    preferred_tools: Vec::new(),
                    created_by: started.actor_id,
                    created_at_unix_ms: started.timestamp_unix_ms,
                    updated_at_unix_ms: started.timestamp_unix_ms,
                },
            },
        );

        assert_eq!(
            execution_completion_message_kind(
                &state,
                agent_instance,
                ExecutionAttemptStatus::Succeeded,
            ),
            Some((AgentMessageKind::ReviewFinding, ActorKind::ReviewerAgent))
        );
        assert_eq!(
            execution_completion_message_kind(
                &state,
                agent_instance,
                ExecutionAttemptStatus::Failed,
            ),
            Some((AgentMessageKind::Blocker, ActorKind::ReviewerAgent))
        );
    }

    #[test]
    fn reviewer_output_is_bound_to_the_assigned_candidate() {
        let candidate_id = ExecutionAttemptId::new();
        let finding = parse_structured_review_finding(
            r#"{"verdict":"needs_revision","summary":"One defect remains","findings":[{"severity":"error","summary":"Handle the empty case","relative_path":"src/lib.rs","line":42}]}"#,
            candidate_id,
        )
        .unwrap();

        assert_eq!(finding.candidate_attempt_id, candidate_id);
        assert_eq!(finding.verdict, ReviewVerdict::NeedsRevision);
        assert_eq!(finding.findings.len(), 1);
        assert!(parse_structured_review_finding("looks good", candidate_id).is_err());
        assert!(parse_structured_review_finding(
            r#"{"verdict":"pass","summary":"Ready","findings":[{"severity":"critical","summary":"Data loss remains"}]}"#,
            candidate_id,
        ).is_err());
    }

    #[test]
    fn revision_decisions_recover_from_history_and_preserve_detailed_feedback() {
        let (mut state, _, _, _) = launch_state(PlanTaskStatus::Reviewing, None, None);
        let (started, _) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            mut attempt,
            mut plan,
        } = started.payload
        else {
            unreachable!()
        };
        attempt.status = ExecutionAttemptStatus::Succeeded;
        attempt.finished_at_unix_ms = Some(started.timestamp_unix_ms + 1);
        let candidate_id = attempt.id;
        let candidate_task = attempt.task_id;
        let mut reviewer = attempt.clone();
        reviewer.id = ExecutionAttemptId::new();
        reviewer.task_id = PlanTaskId::new();
        let root = reviewer.isolation.as_mut().unwrap();
        root.kind = ExecutionIsolationKind::ReviewCandidate;
        root.source_attempt_id = Some(candidate_id);
        let mut review_task = plan.tasks[0].clone();
        review_task.id = reviewer.task_id;
        review_task.review_target_task_id = Some(candidate_task);
        review_task.depends_on = vec![candidate_task];
        plan.tasks.push(review_task);
        let finished = |attempt: ExecutionAttempt| WorkspaceEventKind::AgentExecutionFinished {
            agent_instance: agent_instance.clone(),
            lease: lease.clone(),
            attempt,
            plan: plan.clone(),
            summary: "Finished".into(),
        };
        state.record(10, finished(attempt.clone()));
        state.record(11, finished(reviewer.clone()));
        state.record(
            12,
            WorkspaceEventKind::AgentMessagePosted {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: ThreadId::new(),
                    work_item_id: attempt.work_item_id,
                    sender_id: ActorId::new(),
                    kind: AgentMessageKind::ReviewFinding,
                    body: "One defect remains".into(),
                    review_finding: Some(StructuredReviewFinding {
                        candidate_attempt_id: candidate_id,
                        verdict: ReviewVerdict::NeedsRevision,
                        summary: "One defect remains".into(),
                        findings: vec![ReviewFindingItem {
                            severity: ReviewFindingSeverity::Error,
                            summary: "Handle the empty case".into(),
                            relative_path: Some("src/lib.rs".into()),
                            line: Some(42),
                        }],
                    }),
                    reply_to: None,
                    artifact_ids: vec![],
                    evidence_ids: vec![],
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 12,
                    updated_at_unix_ms: 12,
                },
                routing: AgentMessageRouting {
                    recipient_ids: vec![],
                    task_id: Some(reviewer.task_id),
                    execution_attempt_id: Some(reviewer.id),
                },
            },
        );
        assert_eq!(
            pending_review_decisions(&state),
            vec![
                (candidate_id, ExecutionReviewResolution::RevisionRequested),
                (reviewer.id, ExecutionReviewResolution::RevisionRequested),
            ]
        );
        let context = task_communication_context(&state, candidate_task);
        assert!(context.contains("Handle the empty case"));
        assert!(context.contains("src/lib.rs"));
        assert!(context.contains("42"));
        // Simulate a restart after just the candidate resolution committed.
        attempt.review_resolution = Some(ExecutionReviewResolution::RevisionRequested);
        state.record(13, finished(attempt.clone()));
        assert_eq!(
            pending_review_decisions(&state),
            vec![(reviewer.id, ExecutionReviewResolution::RevisionRequested),]
        );
        reviewer.review_resolution = Some(ExecutionReviewResolution::RevisionRequested);
        state.record(14, finished(reviewer));
        assert!(pending_review_decisions(&state).is_empty());
        // A human superseding the original decision must stop automatic replay.
        attempt.review_resolution = Some(ExecutionReviewResolution::Discarded);
        state.record(15, finished(attempt));
        assert!(pending_review_decisions(&state).is_empty());
    }

    #[test]
    fn legacy_reviewer_recovery_is_narrow_and_replayable() {
        let (mut state, _, _, _) = launch_state(PlanTaskStatus::Reviewing, None, None);
        let (event, mut contract) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted {
            mut agent_instance,
            mut lease,
            mut attempt,
            mut plan,
        } = event.payload
        else {
            unreachable!()
        };
        attempt.status = ExecutionAttemptStatus::Succeeded;
        attempt.review_resolution = Some(ExecutionReviewResolution::RevisionRequested);
        attempt.finished_at_unix_ms = Some(11);
        let mut reviewer = attempt.clone();
        reviewer.id = ExecutionAttemptId::new();
        reviewer.task_id = PlanTaskId::new();
        reviewer.review_resolution = Some(ExecutionReviewResolution::Accepted);
        reviewer.isolation = Some(ExecutionIsolation::review_candidate(
            reviewer.id,
            attempt.id,
            "a".repeat(40),
        ));
        let mut task = plan.tasks[0].clone();
        task.id = reviewer.task_id;
        task.status = PlanTaskStatus::Completed;
        plan.tasks.push(task.clone());
        contract.write_scope.clear();
        contract.status = dragent_workspace_model::WorkContractStatus::Completed;
        contract.updated_by = ActorRef::principal_agent(state.project().id).id;
        agent_instance.status = dragent_workspace_model::AgentInstanceStatus::Completed;
        agent_instance.active_lease_id = None;
        lease.status = dragent_workspace_model::ExecutionLeaseStatus::Released;
        state.record(
            11,
            WorkspaceEventKind::AgentExecutionFinished {
                agent_instance: agent_instance.clone(),
                lease: lease.clone(),
                attempt: attempt.clone(),
                plan: plan.clone(),
                summary: "Needs revision".into(),
            },
        );
        state.record(
            12,
            WorkspaceEventKind::AgentExecutionReviewResolved {
                agent_instance,
                work_contract: contract.clone(),
                lease,
                attempt: reviewer.clone(),
                plan,
                resolution: ExecutionReviewResolution::Accepted,
            },
        );
        let finding = StructuredReviewFinding {
            candidate_attempt_id: attempt.id,
            verdict: ReviewVerdict::NeedsRevision,
            summary: "Fix bug".into(),
            findings: vec![],
        };
        state.record(
            13,
            WorkspaceEventKind::AgentMessagePosted {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: ThreadId::new(),
                    work_item_id: attempt.work_item_id,
                    sender_id: ActorId::new(),
                    kind: AgentMessageKind::ReviewFinding,
                    body: "Fix bug".into(),
                    review_finding: Some(finding.clone()),
                    reply_to: None,
                    artifact_ids: vec![],
                    evidence_ids: vec![],
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 13,
                    updated_at_unix_ms: 13,
                },
                routing: AgentMessageRouting {
                    recipient_ids: vec![],
                    task_id: Some(reviewer.task_id),
                    execution_attempt_id: Some(reviewer.id),
                },
            },
        );
        assert!(legacy_review_recovery(&state, reviewer.id));
        assert_eq!(
            pending_review_decisions(&state),
            vec![(reviewer.id, ExecutionReviewResolution::RevisionRequested)]
        );
        let principal = contract.updated_by;
        let eligible = |r: &ExecutionAttempt, c: &WorkContract, f: &StructuredReviewFinding| {
            dragent_workspace_model::legacy_review_needs_rearm(r, &attempt, c, &task, f, principal)
        };
        assert!(eligible(&reviewer, &contract, &finding));
        let mut human = contract.clone();
        human.updated_by = ActorId::new();
        assert!(!eligible(&reviewer, &human, &finding));
        let mut pass = finding.clone();
        pass.verdict = ReviewVerdict::Pass;
        assert!(!eligible(&reviewer, &contract, &pass));
        reviewer.review_resolution = Some(ExecutionReviewResolution::Discarded);
        assert!(!eligible(&reviewer, &contract, &finding));
        reviewer.review_resolution = Some(ExecutionReviewResolution::RevisionRequested);
        assert!(!eligible(&reviewer, &contract, &finding));
    }

    #[test]
    fn principal_review_policy_is_deterministic_and_escalates_only_blockers() {
        let candidate_id = ExecutionAttemptId::new();
        let finding = |verdict| StructuredReviewFinding {
            candidate_attempt_id: candidate_id,
            verdict,
            summary: "review outcome".into(),
            findings: Vec::new(),
        };
        let pass = principal_review_policy(&finding(ReviewVerdict::Pass));
        assert_eq!(
            pass.candidate_resolution,
            Some(ExecutionReviewResolution::Integrated)
        );
        assert!(!pass.requires_principal_decision);
        let revise = principal_review_policy(&finding(ReviewVerdict::NeedsRevision));
        assert_eq!(
            revise.reviewer_resolution,
            Some(ExecutionReviewResolution::RevisionRequested)
        );
        assert_eq!(
            revise.candidate_resolution,
            Some(ExecutionReviewResolution::RevisionRequested)
        );
        assert!(!revise.requires_principal_decision);
        let blocked = principal_review_policy(&finding(ReviewVerdict::Blocked));
        assert_eq!(blocked.candidate_resolution, None);
        assert_eq!(blocked.reviewer_resolution, None);
        assert!(blocked.requires_principal_decision);
    }

    #[test]
    fn review_requires_captured_changes() {
        let (started, _) = fixture();
        let worker_task = PlanTaskId::new();
        let review_task = PlanTaskId::new();
        let worker_contract = WorkContractId::new();
        let review_contract = WorkContractId::new();
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
                    title: "Work".into(),
                    description: "Do it.".into(),
                    status: PlanTaskStatus::Reviewing,
                    depends_on: Vec::new(),
                    review_target_task_id: None,
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: Some(worker_contract),
                },
                PlanTask {
                    id: review_task,
                    plan_id,
                    title: "Review".into(),
                    description: "Check it.".into(),
                    status: PlanTaskStatus::Proposed,
                    depends_on: vec![worker_task],
                    review_target_task_id: Some(worker_task),
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: Some(review_contract),
                },
            ],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let mut worker = controller_contract(
            started.work_item_id,
            worker_task,
            Some(AgentInstanceId::new()),
        );
        worker.id = worker_contract;
        let mut review = controller_contract(started.work_item_id, review_task, None);
        review.id = review_contract;
        review.write_scope.clear();
        let state = controller_state(
            WorkStatus::Executing,
            5_000,
            plan,
            vec![worker, review],
            Vec::new(),
        );
        // No Succeeded attempt with changes exists: the reviewer must wait.
        assert!(review_candidate_attempt(&state, review_task).is_none());
    }
}
