#[cfg(test)]
mod tests {

    use crate::store_agent::load_scheduler_authority_state;
    use crate::store_execution::finalize_accepted_task;
    use crate::store_execution::resolve_agent_execution_review;
    use crate::workspace_store::*;
    use crate::WorkspaceIntegrationIntent;
    use anyhow::Context;
    use diesel::Connection;
    use dragent_session_protocol::workspace::WorkspaceCommandEnvelope;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandId, WorkspaceCommandPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_session_protocol::workspace::{
        WorkspaceEventEnvelope, WorkspaceEventId, WorkspaceEventPayload,
    };
    use dragent_workspace_model::ChargingAuthorization;
    use dragent_workspace_model::Evidence;
    use dragent_workspace_model::{
        AcceptanceCriterionId, AcceptanceStatus, ActorRef, AgentInstanceId, AgentInstanceStatus,
        AttentionItemId, AttentionKind, AttentionResolution, AttentionStatus, EvidenceId,
        ExecutionAttemptId, ExecutionAttemptStatus, ExecutionIsolation, ExecutionLeaseId,
        ExecutionLeaseStatus, ExecutionReviewResolution, Project, ProjectId, RoleDefinitionId,
        WorkItem, WorkItemId, WorkStatus,
    };
    use dragent_workspace_model::{
        ActorId, AgentMessage, AgentMessageId, AgentMessageKind, AgentMessageRouting, ArtifactId,
        ArtifactKind, ArtifactRef, EvidenceKind, ExecutionChangeSummary, MessageDeliveryState,
        Plan, PlanId, PlanStatus, PlanTask, PlanTaskId, PlanTaskStatus, ProposedTeam,
        ProposedTeamRole, Provenance, ProvenanceSource, RoleDefinition, RoleDefinitionScope,
        ShapingProposal, StructuredRecordStatus, TeamId, ThreadId, WorkBudget, WorkContract,
        WorkContractId, WorkContractStatus, WorkspaceEventKind,
    };
    use std::collections::HashMap;
    use tempfile::tempdir;

    #[test]
    fn proposed_role_contracts_are_accepted_as_canonical_drafts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-proposed-team.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let correlation_id = CorrelationId::new();
        save_workspace_project(&mut connection, &project).unwrap();
        let create = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms: 2,
            expected_revision: None,
            payload: WorkspaceCommandPayload::CreateWorkItem {
                title: "Review a proposed team".into(),
                execution_location: dragent_workspace_model::ExecutionLocation::Local,
            },
        };
        accept_local_workspace_command(&mut connection, &create).unwrap();

        let principal_message_id = AgentMessageId::new();
        let thread_id = ThreadId::new();
        let proposal = ShapingProposal {
            revision: 1,
            outcome: "Review a proposed team".into(),
            requirements: vec!["Keep authority explicit".into()],
            open_decisions: Vec::new(),
            acceptance_criteria: vec!["Every role contract is approved".into()],
            plan_summary: "Review contracts before coordination".into(),
        };
        let records = dragent_workspace_model::ShapingRecords::from_proposal(
            work_item_id,
            &proposal,
            thread_id,
            principal_message_id,
            actor_id,
            3,
        );
        accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 3,
                expected_revision: None,
                payload: WorkspaceCommandPayload::CompleteShapingTurn {
                    message: AgentMessage {
                        id: principal_message_id,
                        thread_id,
                        work_item_id,
                        sender_id: actor_id,
                        kind: AgentMessageKind::StatusUpdate,
                        body: "I prepared a reviewable team.".into(),
                        review_finding: None,
                        reply_to: None,
                        artifact_ids: Vec::new(),
                        evidence_ids: Vec::new(),
                        delivery_state: MessageDeliveryState::LocalOnly,
                        created_at_unix_ms: 3,
                        updated_at_unix_ms: 3,
                    },
                    proposal,
                    records: records.clone(),
                    artifacts: Vec::new(),
                    role_definitions: Vec::new(),
                    work_contracts: Vec::new(),
                    proposed_team: None,
                    plan: None,
                },
            },
        )
        .unwrap();

        let role_definition = RoleDefinition {
            id: dragent_workspace_model::RoleDefinitionId::new(),
            scope: RoleDefinitionScope::Project(project.id),
            kind: dragent_workspace_model::AgentRoleKind::Reviewer,
            name: "Security Reviewer".into(),
            mission: "Review trust boundaries".into(),
            capabilities: Vec::new(),
            preferred_tools: vec!["repository_search".into()],
            created_by: actor_id,
            created_at_unix_ms: 4,
            updated_at_unix_ms: 4,
        };
        let role_event = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 4,
                expected_revision: None,
                payload: WorkspaceCommandPayload::UpsertRoleDefinition {
                    role_definition: role_definition.clone(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            role_event[0].payload,
            WorkspaceEventPayload::RoleDefinitionUpdated { .. }
        ));

        let plan_id = PlanId::new();
        let task_id = PlanTaskId::new();
        let criterion_id = records.acceptance_criteria[0].id;
        let work_contract = WorkContract {
            id: WorkContractId::new(),
            work_item_id,
            role_definition_id: role_definition.id,
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Proposed,
            mission: role_definition.mission.clone(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Review report".into()],
            success_criteria: vec![criterion_id],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: role_definition.preferred_tools.clone(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 5,
            updated_at_unix_ms: 5,
        };
        let mut writable_reviewer = work_contract.clone();
        writable_reviewer.write_scope = vec![std::path::PathBuf::from("app")];
        writable_reviewer.permissions = vec![dragent_workspace_model::PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app".into()),
            requires_approval: true,
        }];
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 5,
                expected_revision: None,
                payload: WorkspaceCommandPayload::UpsertWorkContract {
                    work_contract: writable_reviewer,
                },
            },
        )
        .is_err());
        let contract_event = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 5,
                expected_revision: Some(5),
                payload: WorkspaceCommandPayload::UpsertWorkContract {
                    work_contract: work_contract.clone(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            contract_event[0].payload,
            WorkspaceEventPayload::WorkContractUpdated { .. }
        ));
        assert!(work_contract.agent_instance_id.is_none());

        let mut plan = Plan {
            id: plan_id,
            work_item_id,
            version: 1,
            status: PlanStatus::Draft,
            tasks: vec![PlanTask {
                id: task_id,
                plan_id,
                title: "Review trust boundaries".into(),
                description: "Produce an evidence-backed review".into(),
                status: PlanTaskStatus::Proposed,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: vec![criterion_id],
                assigned_contract_id: Some(work_contract.id),
            }],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 6,
            updated_at_unix_ms: 6,
        };
        let plan_event = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 6,
                expected_revision: Some(6),
                payload: WorkspaceCommandPayload::UpsertPlan { plan: plan.clone() },
            },
        )
        .unwrap();
        assert!(matches!(
            plan_event[0].payload,
            WorkspaceEventPayload::PlanUpdated { .. }
        ));

        let proposed_team = ProposedTeam {
            id: TeamId::new(),
            work_item_id,
            version: 1,
            roles: vec![ProposedTeamRole {
                role_definition_id: role_definition.id,
                work_contract_id: work_contract.id,
                rationale: "The work crosses a trust boundary".into(),
            }],
            status: StructuredRecordStatus::Draft,
            created_by: actor_id,
            created_at_unix_ms: 6,
            updated_at_unix_ms: 6,
        };
        let team_event = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 6,
                expected_revision: Some(7),
                payload: WorkspaceCommandPayload::UpsertProposedTeam { proposed_team },
            },
        )
        .unwrap();
        assert!(matches!(
            team_event[0].payload,
            WorkspaceEventPayload::ProposedTeamUpdated { .. }
        ));

        let transition = |expected_revision, status, timestamp_unix_ms| WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms,
            expected_revision: Some(expected_revision),
            payload: WorkspaceCommandPayload::TransitionWork { status },
        };
        accept_local_workspace_command(
            &mut connection,
            &transition(8, WorkStatus::Understanding, 7),
        )
        .unwrap();
        accept_local_workspace_command(&mut connection, &transition(9, WorkStatus::Planning, 8))
            .unwrap();
        assert!(accept_local_workspace_command(
            &mut connection,
            &transition(10, WorkStatus::ReadyToExecute, 9),
        )
        .is_err());

        let editor_id = ActorId::new();
        let mut approved_contract = work_contract.clone();
        approved_contract.version = 2;
        approved_contract.status = WorkContractStatus::Approved;
        approved_contract.updated_by = editor_id;
        approved_contract.updated_at_unix_ms = 10;
        let approval_event = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: editor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10,
                expected_revision: Some(10),
                payload: WorkspaceCommandPayload::UpsertWorkContract {
                    work_contract: approved_contract.clone(),
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::WorkContractUpdated { work_contract } =
            &approval_event[0].payload
        else {
            panic!("approval emitted the wrong event")
        };
        assert_eq!(work_contract.version, 2);
        assert_eq!(work_contract.created_by, actor_id);
        assert_eq!(work_contract.updated_by, editor_id);
        assert!(work_contract.agent_instance_id.is_none());
        plan.version = 2;
        plan.status = PlanStatus::Approved;
        plan.approved_by = Some(editor_id);
        plan.approved_at_unix_ms = Some(11);
        plan.updated_at_unix_ms = 11;
        accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: editor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 11,
                expected_revision: Some(11),
                payload: WorkspaceCommandPayload::UpsertPlan { plan },
            },
        )
        .unwrap();
        let ready = accept_local_workspace_command(
            &mut connection,
            &transition(12, WorkStatus::ReadyToExecute, 12),
        )
        .unwrap();
        assert!(matches!(
            ready[0].payload,
            WorkspaceEventPayload::WorkStatusChanged {
                current: WorkStatus::ReadyToExecute,
                ..
            }
        ));
        let task_ready = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 13,
                expected_revision: Some(13),
                payload: WorkspaceCommandPayload::TransitionPlanTask {
                    task_id,
                    status: PlanTaskStatus::Ready,
                },
            },
        )
        .unwrap();
        assert_eq!(task_ready[0].task_id, Some(task_id));
        let WorkspaceEventPayload::PlanTaskStatusChanged {
            plan,
            previous,
            current,
            ..
        } = &task_ready[0].payload
        else {
            panic!("task transition emitted the wrong event")
        };
        assert_eq!(*previous, PlanTaskStatus::Proposed);
        assert_eq!(*current, PlanTaskStatus::Ready);
        assert_eq!(plan.tasks[0].status, PlanTaskStatus::Ready);
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 14,
                expected_revision: Some(14),
                payload: WorkspaceCommandPayload::TransitionPlanTask {
                    task_id,
                    status: PlanTaskStatus::Running,
                },
            },
        )
        .is_err());
        let agent_instance_id = AgentInstanceId::new();
        let provisioned = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 15,
                expected_revision: Some(14),
                payload: WorkspaceCommandPayload::ProvisionAgentInstance {
                    agent_instance_id,
                    contract_id: approved_contract.id,
                    model_route: "auto".into(),
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentInstanceProvisioned {
            agent_instance,
            work_contract,
        } = &provisioned[0].payload
        else {
            panic!("agent provisioning emitted the wrong event")
        };
        assert_eq!(agent_instance.id, agent_instance_id);
        assert_eq!(agent_instance.status, AgentInstanceStatus::Pending);
        assert_eq!(agent_instance.started_at_unix_ms, None);
        assert_eq!(agent_instance.model_route, "auto");
        assert_eq!(work_contract.status, WorkContractStatus::Approved);
        assert_eq!(work_contract.agent_instance_id, Some(agent_instance_id));
        assert_eq!(work_contract.version, 3);

        // Provisioning creates durable identity and binding only. Until a
        // runtime handshake activates the contract, the scheduler still
        // rejects execution.
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 16,
                expected_revision: Some(15),
                payload: WorkspaceCommandPayload::TransitionPlanTask {
                    task_id,
                    status: PlanTaskStatus::Running,
                },
            },
        )
        .is_err());

        // Platform-credit leases activate pay-as-you-go: the provider
        // enforces balance per call, so no preflight hold is required. A
        // rejected command must not advance revision.
        let lease_id = ExecutionLeaseId::new();
        let activated = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 16,
                expected_revision: Some(15),
                payload: WorkspaceCommandPayload::ActivateAgentInstance {
                    agent_instance_id,
                    lease_id,
                    effective_model_route: "cloud/default-agent".into(),
                    charging: ChargingAuthorization {
                        owner_id: actor_id,
                        mode: dragent_workspace_model::ChargingMode::PlatformCredits,
                        billing_operation_id: None,
                    },
                    lease_expires_at_unix_ms: 16 + 60_000,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentInstanceActivated {
            agent_instance,
            work_contract,
            lease,
        } = &activated[0].payload
        else {
            panic!("agent activation emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Ready);
        assert_eq!(agent_instance.active_lease_id, Some(lease_id));
        assert_eq!(agent_instance.started_at_unix_ms, None);
        assert_eq!(work_contract.status, WorkContractStatus::Active);
        assert_eq!(work_contract.version, 4);
        assert_eq!(lease.status, ExecutionLeaseStatus::Active);
        assert_eq!(lease.charging.owner_id, actor_id);

        // Generic task transitions cannot claim that execution started. The
        // authority must commit the agent, attempt, lease, task, and work state
        // together before an adapter is allowed to invoke the selected model.
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 17,
                expected_revision: Some(16),
                payload: WorkspaceCommandPayload::TransitionPlanTask {
                    task_id,
                    status: PlanTaskStatus::Running,
                },
            },
        )
        .is_err());

        let attempt_id = ExecutionAttemptId::new();
        let running = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 17,
                expected_revision: Some(16),
                payload: WorkspaceCommandPayload::StartAgentExecution {
                    attempt_id,
                    task_id,
                    agent_instance_id,
                    lease_id,
                    isolation: ExecutionIsolation::shared(project.id),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            running[0].payload,
            WorkspaceEventPayload::WorkStatusChanged {
                current: WorkStatus::Executing,
                ..
            }
        ));
        let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        } = &running[1].payload
        else {
            panic!("execution start emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Running);
        assert_eq!(agent_instance.started_at_unix_ms, Some(17));
        assert_eq!(lease.id, lease_id);
        assert_eq!(attempt.id, attempt_id);
        assert_eq!(attempt.attempt_number, 1);
        assert_eq!(attempt.status, ExecutionAttemptStatus::Running);
        assert_eq!(
            plan.tasks
                .iter()
                .find(|task| task.id == task_id)
                .unwrap()
                .status,
            PlanTaskStatus::Running
        );
        assert_eq!(
            load_canonical_workspace_events(&mut connection, project.id, work_item_id, 0, 20)
                .unwrap()
                .len(),
            18
        );

        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 18,
                expected_revision: Some(18),
                payload: WorkspaceCommandPayload::TransitionPlanTask {
                    task_id,
                    status: PlanTaskStatus::Reviewing,
                },
            },
        )
        .is_err());

        let finished = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 18,
                expected_revision: Some(18),
                payload: WorkspaceCommandPayload::FinishAgentExecution {
                    attempt_id,
                    outcome: ExecutionAttemptStatus::Succeeded,
                    summary: "Implementation is ready for independent review.".into(),
                    change_summary: None,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentExecutionFinished {
            agent_instance,
            lease,
            attempt,
            plan,
            ..
        } = &finished[0].payload
        else {
            panic!("execution finish emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Reviewing);
        assert_eq!(agent_instance.active_lease_id, Some(lease_id));
        assert_eq!(lease.status, ExecutionLeaseStatus::Active);
        assert_eq!(attempt.status, ExecutionAttemptStatus::Succeeded);
        assert_eq!(attempt.finished_at_unix_ms, Some(18));
        assert_eq!(
            plan.tasks
                .iter()
                .find(|task| task.id == task_id)
                .unwrap()
                .status,
            PlanTaskStatus::Reviewing
        );
        assert_eq!(
            load_canonical_workspace_events(&mut connection, project.id, work_item_id, 0, 20)
                .unwrap()
                .len(),
            19
        );

        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 19,
                expected_revision: Some(19),
                payload: WorkspaceCommandPayload::StartAgentExecution {
                    attempt_id: ExecutionAttemptId::new(),
                    task_id,
                    agent_instance_id,
                    lease_id,
                    isolation: ExecutionIsolation::shared(project.id),
                },
            },
        )
        .is_err());

        let (accepted_agent, accepted_contract, accepted_lease, accepted_attempt, accepted_plan) =
            resolve_agent_execution_review(
                &mut connection,
                project.id,
                work_item_id,
                actor_id,
                attempt_id,
                ExecutionReviewResolution::Accepted,
                19,
                19,
            )
            .unwrap();
        assert_eq!(accepted_agent.status, AgentInstanceStatus::Completed);
        assert_eq!(accepted_contract.status, WorkContractStatus::Completed);
        assert_eq!(accepted_lease.status, ExecutionLeaseStatus::Released);
        assert_eq!(
            accepted_attempt.review_resolution,
            Some(ExecutionReviewResolution::Accepted)
        );
        assert_eq!(accepted_plan.tasks[0].status, PlanTaskStatus::Verifying);
        let (evidence, criteria, completed_plan, work_complete) = finalize_accepted_task(
            &mut connection,
            project.id,
            work_item_id,
            actor_id,
            &accepted_attempt,
            &accepted_contract,
            &accepted_plan,
            accepted_agent.id,
            ExecutionReviewResolution::Accepted,
            19,
            19,
        )
        .unwrap();
        assert!(
            finalize_accepted_task(
                &mut connection,
                project.id,
                work_item_id,
                ActorRef::principal_agent(project.id).id,
                &accepted_attempt,
                &accepted_contract,
                &accepted_plan,
                accepted_agent.id,
                ExecutionReviewResolution::Accepted,
                19,
                19,
            )
            .is_err(),
            "automatic acceptance without an independent passing review must fail"
        );
        assert_eq!(evidence.len(), 1);
        let evidence = &evidence[0];
        assert_eq!(evidence.task_id, Some(task_id));
        assert_eq!(evidence.kind, EvidenceKind::UserConfirmation);
        assert_eq!(criteria.len(), 1);
        assert_eq!(criteria[0].status, AcceptanceStatus::Satisfied);
        assert_eq!(criteria[0].evidence_ids, vec![evidence.id]);
        assert_eq!(completed_plan.tasks[0].status, PlanTaskStatus::Completed);
        assert!(work_complete);

        // Import the exact legacy shape inside a savepoint: NeedsRevision
        // candidate, but the old Principal policy completed its reviewer.
        let recovery_probe: anyhow::Result<()> = connection.transaction(|connection| {
            let principal = ActorRef::principal_agent(project.id).id;
            let mut candidate = attempt.clone();
            candidate.id = ExecutionAttemptId::new();
            candidate.task_id = PlanTaskId::new();
            candidate.review_resolution = Some(ExecutionReviewResolution::RevisionRequested);
            let mut old_reviewer = accepted_attempt.clone();
            old_reviewer.isolation = Some(ExecutionIsolation::review_candidate(old_reviewer.id, candidate.id, "a".repeat(40)));
            let mut old_contract = accepted_contract.clone(); old_contract.updated_by = principal;
            let mut legacy_plan = completed_plan.clone();
            let mut candidate_task = legacy_plan.tasks[0].clone();
            candidate_task.id = candidate.task_id; candidate_task.status = PlanTaskStatus::Ready;
            legacy_plan.tasks.push(candidate_task);
            let mut reviewing_plan = legacy_plan.clone();
            reviewing_plan.tasks[0].status = PlanTaskStatus::Verifying;
            reviewing_plan.tasks[1].status = PlanTaskStatus::Reviewing;
            let message = AgentMessage {
                id: AgentMessageId::new(), thread_id: ThreadId::new(), work_item_id,
                sender_id: ActorRef::agent_instance(accepted_agent.id, dragent_workspace_model::ActorKind::ReviewerAgent).id,
                kind: AgentMessageKind::ReviewFinding, body: "Needs another revision".into(),
                review_finding: Some(dragent_workspace_model::StructuredReviewFinding {
                    candidate_attempt_id: candidate.id, verdict: dragent_workspace_model::ReviewVerdict::NeedsRevision,
                    summary: "Needs another revision".into(), findings: vec![dragent_workspace_model::ReviewFindingItem {
                        severity: dragent_workspace_model::ReviewFindingSeverity::Error,
                        summary: "Handle the empty input".into(), relative_path: None, line: None,
                    }],
                }), reply_to: None, artifact_ids: vec![], evidence_ids: vec![],
                delivery_state: MessageDeliveryState::LocalOnly, created_at_unix_ms: 20, updated_at_unix_ms: 20,
            };
            let command = WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(), schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id, work_item_id, actor_id: principal, device_id: None, correlation_id,
                timestamp_unix_ms: 24, expected_revision: Some(24),
                payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                    attempt_id, resolution: ExecutionReviewResolution::RevisionRequested,
                },
            };
            let payloads = vec![
                WorkspaceEventPayload::AgentExecutionFinished { agent_instance: agent_instance.clone(), lease: lease.clone(),
                    attempt: candidate, plan: reviewing_plan.clone(), summary: "Revision requested".into() },
                WorkspaceEventPayload::MessagePosted { message, routing: Some(AgentMessageRouting {
                    recipient_ids: vec![principal], task_id: Some(task_id), execution_attempt_id: Some(attempt_id),
                }) },
                WorkspaceEventPayload::AgentExecutionReviewResolved { agent_instance: accepted_agent.clone(),
                    work_contract: old_contract, lease: accepted_lease.clone(), attempt: old_reviewer,
                    plan: reviewing_plan, resolution: ExecutionReviewResolution::Accepted },
                WorkspaceEventPayload::PlanTaskStatusChanged { task_id, previous: PlanTaskStatus::Verifying,
                    current: PlanTaskStatus::Completed, plan: legacy_plan },
                WorkspaceEventPayload::AcceptanceCriterionUpdated { acceptance_criterion: criteria[0].clone() },
            ];
            let imported: Vec<_> = payloads.into_iter().enumerate().map(|(i, payload)| WorkspaceEventEnvelope {
                event_id: WorkspaceEventId::new(), sequence: 20 + i as u64,
                actor_id: match &payload {
                    WorkspaceEventPayload::MessagePosted { message, .. } => message.sender_id,
                    WorkspaceEventPayload::AcceptanceCriterionUpdated { .. } => actor_id,
                    _ => principal,
                },
                task_id: match &payload {
                    WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } => Some(attempt.task_id),
                    WorkspaceEventPayload::MessagePosted { .. }
                    | WorkspaceEventPayload::AgentExecutionReviewResolved { .. }
                    | WorkspaceEventPayload::PlanTaskStatusChanged { .. } => Some(task_id),
                    _ => None,
                },
                payload, ..finished[0].clone()
            }).collect();
            let mut import_command = command.clone(); import_command.command_id = WorkspaceCommandId::new();
            insert_canonical_outcome(connection, &import_command, &imported)?;
            let proof_probe: anyhow::Result<()> = connection.transaction(|connection| {
                let mut pass_event = imported[1].clone();
                pass_event.event_id = WorkspaceEventId::new(); pass_event.sequence = 25;
                let WorkspaceEventPayload::MessagePosted { message, .. } = &mut pass_event.payload else { unreachable!() };
                message.id = AgentMessageId::new();
                let finding = message.review_finding.as_mut().unwrap();
                finding.verdict = dragent_workspace_model::ReviewVerdict::Pass;
                finding.findings.clear();
                let source_candidate = finding.candidate_attempt_id;
                import_command.command_id = WorkspaceCommandId::new();
                insert_canonical_outcome(connection, &import_command, &[pass_event])?;
                let (mut verifying_plan, _, _, _, _) = load_scheduler_authority_state(connection, project.id, work_item_id, 25)?;
                verifying_plan.tasks[0].status = PlanTaskStatus::Verifying;
                let check = |connection: &mut diesel::sqlite::SqliteConnection| {
                    finalize_accepted_task(connection, project.id, work_item_id, principal,
                        &accepted_attempt, &accepted_contract, &verifying_plan, accepted_agent.id,
                        ExecutionReviewResolution::Accepted, 26, 25)
                };
                let missing = check(connection).unwrap_err();
                assert!(missing.to_string().contains("no host-recorded verification"), "{missing:#}");
                let mut receipt = crate::host_verification::CommandReceipt {
                    id: "authority-check".into(), work_item_id, attempt_id,
                    candidate_attempt_id: source_candidate, criteria: vec![criteria[0].id],
                    command: "cargo test".into(), cwd: "/candidate".into(),
                    started_at_unix_ms: accepted_attempt.started_at_unix_ms,
                    finished_at_unix_ms: None, exit_code: None, output: "test output".into(),
                    snapshot_before: "candidate-fingerprint".into(), snapshot_after: None,
                };
                crate::host_verification::start(connection, &receipt)?;
                assert!(check(connection).is_err());
                receipt.finished_at_unix_ms = accepted_attempt.finished_at_unix_ms;
                receipt.exit_code = Some(1); receipt.snapshot_after = Some(receipt.snapshot_before.clone());
                crate::host_verification::finish(connection, &receipt)?;
                assert!(check(connection).is_err());
                receipt.exit_code = Some(0);
                crate::host_verification::finish(connection, &receipt)?;
                assert!(check(connection).is_ok());
                anyhow::bail!("rollback verification probe")
            });
            let proof_error = proof_probe.unwrap_err();
            assert!(proof_error.to_string().contains("rollback verification probe"), "{proof_error:#}");
            let repaired = accept_local_workspace_command(connection, &command)?;
            let WorkspaceEventPayload::AgentExecutionReviewResolved { agent_instance, work_contract, lease, plan, .. } = &repaired[0].payload else { panic!("missing repair") };
            assert_eq!(agent_instance.status, AgentInstanceStatus::Ready);
            assert_eq!(agent_instance.active_lease_id, Some(lease.id));
            assert_eq!(work_contract.status, WorkContractStatus::Active);
            assert_eq!(lease.status, ExecutionLeaseStatus::Active);
            assert!(lease.expires_at_unix_ms > command.timestamp_unix_ms);
            assert_eq!(plan.tasks[0].status, PlanTaskStatus::Ready);
            assert!(repaired.iter().any(|e| matches!(&e.payload, WorkspaceEventPayload::AcceptanceCriterionUpdated { acceptance_criterion }
                if acceptance_criterion.status == AcceptanceStatus::Pending && !acceptance_criterion.evidence_ids.contains(&evidence.id))));
            assert_eq!(accept_local_workspace_command(connection, &command)?, repaired);
            let mut repeated = command.clone(); repeated.command_id = WorkspaceCommandId::new();
            repeated.expected_revision = Some(repaired.last().unwrap().sequence);
            assert!(accept_local_workspace_command(connection, &repeated).is_err());
            anyhow::bail!("rollback legacy recovery probe")
        });
        let recovery_error = recovery_probe.unwrap_err();
        assert!(
            recovery_error
                .to_string()
                .contains("rollback legacy recovery probe"),
            "{recovery_error:#}"
        );

        // Exercise the public authority command atomically without consuming
        // the execution needed by the retry assertions below. The enclosing
        // savepoint is deliberately rolled back after inspecting the batch.
        let acceptance_probe: anyhow::Result<()> =
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                let command = WorkspaceCommandEnvelope {
                    command_id: WorkspaceCommandId::new(),
                    schema_version: WORKSPACE_PROTOCOL_VERSION,
                    project_id: project.id,
                    work_item_id,
                    actor_id,
                    device_id: None,
                    correlation_id,
                    timestamp_unix_ms: 19,
                    expected_revision: Some(19),
                    payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                        attempt_id,
                        resolution: ExecutionReviewResolution::Integrated,
                    },
                };
                insert_workspace_integration_intent(
                    connection,
                    &WorkspaceIntegrationIntent {
                        attempt_id,
                        project_id: project.id,
                        work_item_id,
                        command_id: command.command_id,
                        command: command.clone(),
                        integration_plan_json: "test-plan".into(),
                        state: "prepared".into(),
                        claim_owner: None,
                        claim_expires_at_unix_ms: None,
                        created_at_unix_ms: 19,
                        completed_at_unix_ms: None,
                        last_error: None,
                    },
                )?;
                let accepted = accept_local_workspace_command(connection, &command)?;
                assert!(matches!(
                    accepted[0].payload,
                    WorkspaceEventPayload::AgentExecutionReviewResolved { .. }
                ));
                assert!(accepted.iter().any(|event| matches!(
                    event.payload,
                    WorkspaceEventPayload::EvidenceRecorded { .. }
                )));
                assert!(accepted.iter().any(|event| matches!(
                    event.payload,
                    WorkspaceEventPayload::AcceptanceCriterionUpdated {
                        ref acceptance_criterion
                    } if acceptance_criterion.status == AcceptanceStatus::Satisfied
                )));
                assert!(accepted.iter().any(|event| matches!(
                    event.payload,
                    WorkspaceEventPayload::PlanTaskStatusChanged {
                        current: PlanTaskStatus::Completed,
                        ..
                    }
                )));
                assert!(accepted.iter().any(|event| matches!(
                    event.payload,
                    WorkspaceEventPayload::WorkStatusChanged {
                        current: WorkStatus::Completed,
                        ..
                    }
                )));
                assert_eq!(
                    load_workspace_integration_intent(connection, attempt_id)?
                        .context("completed integration intent is missing")?
                        .state,
                    "completed"
                );
                Err(anyhow::anyhow!("rollback acceptance probe"))
            });
        assert!(acceptance_probe.is_err());

        let revision_requested = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 19,
                expected_revision: Some(19),
                payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                    attempt_id,
                    resolution: ExecutionReviewResolution::RevisionRequested,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentExecutionReviewResolved {
            agent_instance,
            work_contract,
            lease,
            attempt,
            plan,
            resolution,
        } = &revision_requested[0].payload
        else {
            panic!("revision request emitted the wrong event")
        };
        assert_eq!(*resolution, ExecutionReviewResolution::RevisionRequested);
        assert_eq!(agent_instance.status, AgentInstanceStatus::Ready);
        assert_eq!(agent_instance.active_lease_id, Some(lease_id));
        assert_eq!(work_contract.status, WorkContractStatus::Active);
        assert_eq!(lease.status, ExecutionLeaseStatus::Active);
        assert_eq!(attempt.review_resolution, Some(*resolution));
        assert_eq!(plan.tasks[0].status, PlanTaskStatus::Ready);

        let revision_attempt_id = ExecutionAttemptId::new();
        let revision_attempt = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 19,
                expected_revision: Some(20),
                payload: WorkspaceCommandPayload::StartAgentExecution {
                    attempt_id: revision_attempt_id,
                    task_id,
                    agent_instance_id,
                    lease_id,
                    isolation: ExecutionIsolation::shared(project.id),
                },
            },
        )
        .unwrap();
        assert_eq!(revision_attempt.len(), 1);
        let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            attempt,
            ..
        } = &revision_attempt[0].payload
        else {
            panic!("review revision emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Running);
        assert_eq!(agent_instance.started_at_unix_ms, Some(17));
        assert_eq!(attempt.id, revision_attempt_id);
        assert_eq!(attempt.attempt_number, 2);

        let unauthorized_actor = ActorId::new();
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: unauthorized_actor,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 20,
                expected_revision: Some(21),
                payload: WorkspaceCommandPayload::FinishAgentExecution {
                    attempt_id: revision_attempt_id,
                    outcome: ExecutionAttemptStatus::Failed,
                    summary: "An unrelated actor cannot finish this execution.".into(),
                    change_summary: None,
                },
            },
        )
        .is_err());
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 60_016,
                expected_revision: Some(21),
                payload: WorkspaceCommandPayload::RenewExecutionLease {
                    lease_id,
                    lease_expires_at_unix_ms: 120_016,
                },
            },
        )
        .is_err());

        let renewed = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 20,
                expected_revision: Some(21),
                payload: WorkspaceCommandPayload::RenewExecutionLease {
                    lease_id,
                    lease_expires_at_unix_ms: 20 + 120_000,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::ExecutionLeaseRenewed {
            agent_instance,
            lease,
        } = &renewed[0].payload
        else {
            panic!("lease renewal emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Running);
        assert_eq!(lease.status, ExecutionLeaseStatus::Active);
        assert_eq!(lease.expires_at_unix_ms, 20 + 120_000);
        assert_eq!(lease.updated_at_unix_ms, 20);

        let failed = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 21,
                expected_revision: Some(22),
                payload: WorkspaceCommandPayload::FinishAgentExecution {
                    attempt_id: revision_attempt_id,
                    outcome: ExecutionAttemptStatus::Failed,
                    summary: "The provider returned a retryable failure.".into(),
                    change_summary: None,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentExecutionFinished {
            agent_instance,
            lease,
            attempt,
            plan,
            ..
        } = &failed[0].payload
        else {
            panic!("failed execution emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Failed);
        assert_eq!(agent_instance.active_lease_id, None);
        assert_eq!(lease.status, ExecutionLeaseStatus::Released);
        assert_eq!(attempt.status, ExecutionAttemptStatus::Failed);
        assert_eq!(plan.tasks[0].status, PlanTaskStatus::Failed);

        let retry_lease_id = ExecutionLeaseId::new();
        let retry_command = |timestamp_unix_ms, expected_revision| WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms,
            expected_revision: Some(expected_revision),
            payload: WorkspaceCommandPayload::PrepareAgentExecutionRetry {
                failed_attempt_id: revision_attempt_id,
                lease_id: retry_lease_id,
                lease_expires_at_unix_ms: timestamp_unix_ms + 60_000,
            },
        };
        assert!(
            accept_local_workspace_command(&mut connection, &retry_command(10_020, 23)).is_err()
        );
        let retry_prepared =
            accept_local_workspace_command(&mut connection, &retry_command(10_021, 23)).unwrap();
        let WorkspaceEventPayload::AgentExecutionRetryPrepared {
            agent_instance,
            lease,
            plan,
        } = &retry_prepared[0].payload
        else {
            panic!("retry preparation emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Ready);
        assert_eq!(agent_instance.active_lease_id, Some(retry_lease_id));
        assert_eq!(lease.id, retry_lease_id);
        assert_eq!(lease.status, ExecutionLeaseStatus::Active);
        assert_eq!(plan.tasks[0].status, PlanTaskStatus::Ready);

        let retry_attempt_id = ExecutionAttemptId::new();
        let retried = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_022,
                expected_revision: Some(24),
                payload: WorkspaceCommandPayload::StartAgentExecution {
                    attempt_id: retry_attempt_id,
                    task_id,
                    agent_instance_id,
                    lease_id: retry_lease_id,
                    isolation: ExecutionIsolation::shared(project.id),
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } = &retried[0].payload
        else {
            panic!("retried execution emitted the wrong event")
        };
        assert_eq!(attempt.attempt_number, 3);

        let paused = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_023,
                expected_revision: Some(25),
                payload: WorkspaceCommandPayload::PauseAgentExecution {
                    attempt_id: retry_attempt_id,
                    reason: "Paused by the workspace owner.".into(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            paused[0].payload,
            WorkspaceEventPayload::WorkStatusChanged {
                current: WorkStatus::Paused,
                ..
            }
        ));
        let WorkspaceEventPayload::AgentExecutionFinished {
            agent_instance,
            lease,
            attempt,
            plan,
            ..
        } = &paused[1].payload
        else {
            panic!("execution pause emitted the wrong terminal event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Ready);
        assert_eq!(agent_instance.active_lease_id, Some(retry_lease_id));
        assert_eq!(lease.status, ExecutionLeaseStatus::Active);
        assert_eq!(attempt.status, ExecutionAttemptStatus::Paused);
        assert_eq!(plan.tasks[0].status, PlanTaskStatus::Ready);

        accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_024,
                expected_revision: Some(27),
                payload: WorkspaceCommandPayload::TransitionWork {
                    status: WorkStatus::Executing,
                },
            },
        )
        .unwrap();
        let resumed_attempt_id = ExecutionAttemptId::new();
        let resumed = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_025,
                expected_revision: Some(28),
                payload: WorkspaceCommandPayload::StartAgentExecution {
                    attempt_id: resumed_attempt_id,
                    task_id,
                    agent_instance_id,
                    lease_id: retry_lease_id,
                    isolation: ExecutionIsolation::shared(project.id),
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } = &resumed[0].payload
        else {
            panic!("resumed execution emitted the wrong event")
        };
        assert_eq!(attempt.id, resumed_attempt_id);
        assert_eq!(attempt.attempt_number, 4);

        // Cancellation is an authority command, not a process-local UI flag.
        // The canonical terminal event is committed before an execution host
        // reacts by stopping its provider session.
        let cancelled = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_026,
                expected_revision: Some(29),
                payload: WorkspaceCommandPayload::CancelAgentExecution {
                    attempt_id: resumed_attempt_id,
                    reason: "Cancelled by the workspace owner.".into(),
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::AgentExecutionFinished {
            agent_instance,
            lease,
            attempt,
            plan,
            summary,
        } = &cancelled[0].payload
        else {
            panic!("execution cancellation emitted the wrong event")
        };
        assert_eq!(agent_instance.status, AgentInstanceStatus::Cancelled);
        assert_eq!(agent_instance.active_lease_id, None);
        assert_eq!(lease.status, ExecutionLeaseStatus::Cancelled);
        assert_eq!(attempt.status, ExecutionAttemptStatus::Cancelled);
        assert_eq!(attempt.finished_at_unix_ms, Some(10_026));
        assert_eq!(summary, "Cancelled by the workspace owner.");
        assert_eq!(plan.tasks[0].status, PlanTaskStatus::Cancelled);

        let principal = ActorRef::principal_agent(project.id);
        let worker = ActorRef::agent_instance(
            agent_instance_id,
            dragent_workspace_model::ActorKind::WorkerAgent,
        );
        let routed = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: principal.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_027,
                expected_revision: None,
                payload: WorkspaceCommandPayload::PostMessage {
                    message: AgentMessage {
                        id: AgentMessageId::new(),
                        thread_id: ThreadId::new(),
                        work_item_id,
                        sender_id: principal.id,
                        kind: AgentMessageKind::Handoff,
                        body: "Revise the implementation using the review findings.".into(),
                        review_finding: None,
                        reply_to: None,
                        artifact_ids: Vec::new(),
                        evidence_ids: Vec::new(),
                        delivery_state: MessageDeliveryState::Pending,
                        created_at_unix_ms: 10_027,
                        updated_at_unix_ms: 10_027,
                    },
                    routing: Some(AgentMessageRouting {
                        recipient_ids: vec![worker.id],
                        task_id: Some(task_id),
                        execution_attempt_id: Some(resumed_attempt_id),
                    }),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            routed[0].payload,
            WorkspaceEventPayload::MessagePosted {
                routing: Some(_),
                ..
            }
        ));
        let WorkspaceEventPayload::MessagePosted {
            message: routed_message,
            ..
        } = &routed[0].payload
        else {
            unreachable!()
        };
        let delivered = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: worker.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_028,
                expected_revision: Some(routed[0].sequence),
                payload: WorkspaceCommandPayload::UpdateMessageDelivery {
                    message_id: routed_message.id,
                    delivery_state: MessageDeliveryState::Delivered,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::MessageDeliveryUpdated { message, routing } =
            &delivered[0].payload
        else {
            panic!("delivery update emitted the wrong event")
        };
        assert_eq!(message.delivery_state, MessageDeliveryState::Delivered);
        assert_eq!(routing.recipient_ids, [worker.id]);

        let read = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: worker.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_029,
                expected_revision: Some(delivered[0].sequence),
                payload: WorkspaceCommandPayload::UpdateMessageDelivery {
                    message_id: routed_message.id,
                    delivery_state: MessageDeliveryState::Read,
                },
            },
        )
        .unwrap();
        let WorkspaceEventPayload::MessageDeliveryUpdated { message, .. } = &read[0].payload else {
            panic!("read update emitted the wrong event")
        };
        assert_eq!(message.delivery_state, MessageDeliveryState::Read);

        let evidence = Evidence {
            id: EvidenceId::new(),
            work_item_id,
            task_id: Some(task_id),
            kind: EvidenceKind::Test,
            summary: "The focused implementation tests passed.".into(),
            source: "cargo test -p implementation".into(),
            produced_by: worker.id,
            created_at_unix_ms: 10_030,
        };
        let recorded = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: worker.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_030,
                expected_revision: Some(read[0].sequence),
                payload: WorkspaceCommandPayload::RecordEvidence {
                    evidence: evidence.clone(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            recorded[0].payload,
            WorkspaceEventPayload::EvidenceRecorded { .. }
        ));

        let mut satisfied = records.acceptance_criteria[0].clone();
        satisfied.status = AcceptanceStatus::Satisfied;
        satisfied.evidence_ids = vec![evidence.id];
        satisfied.updated_at_unix_ms = 10_031;
        satisfied.provenance.push(Provenance {
            source: ProvenanceSource::Evidence {
                evidence_id: evidence.id,
            },
            recorded_by: worker.id,
            recorded_at_unix_ms: 10_031,
        });
        let criterion_updated = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: worker.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_031,
                expected_revision: Some(recorded[0].sequence),
                payload: WorkspaceCommandPayload::UpsertAcceptanceCriterion {
                    acceptance_criterion: satisfied,
                },
            },
        )
        .unwrap();
        assert!(matches!(
            criterion_updated[0].payload,
            WorkspaceEventPayload::AcceptanceCriterionUpdated { .. }
        ));
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: principal.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_032,
                expected_revision: Some(criterion_updated[0].sequence),
                payload: WorkspaceCommandPayload::TransitionWork {
                    status: WorkStatus::Completed,
                },
            },
        )
        .is_err());
        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: principal.id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_030,
                expected_revision: Some(read[0].sequence),
                payload: WorkspaceCommandPayload::UpdateMessageDelivery {
                    message_id: routed_message.id,
                    delivery_state: MessageDeliveryState::Delivered,
                },
            },
        )
        .is_err());

        let mut unknown_recipient = routed[0].clone();
        let WorkspaceEventPayload::MessagePosted { routing, message } =
            &mut unknown_recipient.payload
        else {
            unreachable!()
        };
        routing.as_mut().unwrap().recipient_ids = vec![dragent_workspace_model::ActorId::new()];
        let invalid_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id: principal.id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms: 10_028,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    created_at_unix_ms: 10_028,
                    updated_at_unix_ms: 10_028,
                    ..message.clone()
                },
                routing: routing.clone(),
            },
        };
        assert!(accept_local_workspace_command(&mut connection, &invalid_command).is_err());

        assert!(accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 10_027,
                expected_revision: Some(29),
                payload: WorkspaceCommandPayload::CancelAgentExecution {
                    attempt_id: resumed_attempt_id,
                    reason: "Duplicate cancellation.".into(),
                },
            },
        )
        .is_err());
    }
}
