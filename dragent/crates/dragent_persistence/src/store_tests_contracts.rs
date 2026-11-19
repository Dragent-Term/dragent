#[cfg(test)]
mod tests {

    use crate::workspace_store::*;
    use dragent_session_protocol::workspace::WorkspaceCommandEnvelope;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandId, WorkspaceCommandPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_session_protocol::workspace::{
        WorkspaceEventEnvelope, WorkspaceEventId, WorkspaceEventPayload,
    };
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
    fn work_contract_accepts_only_canonical_input_artifacts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-artifacts.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        save_workspace_project(&mut connection, &project).unwrap();
        accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: 1,
                expected_revision: Some(2),
                payload: WorkspaceCommandPayload::CreateWorkItem {
                    title: "Use repository artifacts".into(),
                    execution_location: dragent_workspace_model::ExecutionLocation::Local,
                },
            },
        )
        .unwrap();

        let artifact = ArtifactRef {
            id: ArtifactId::new(),
            work_item_id,
            relative_path: "docs/requirements.md".into(),
            kind: ArtifactKind::Requirements,
        };
        let contract = WorkContract {
            id: WorkContractId::new(),
            work_item_id,
            role_definition_id: dragent_workspace_model::RoleDefinitionId::new(),
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Proposed,
            mission: "Implement the requirements".into(),
            task_ids: Vec::new(),
            input_artifact_ids: vec![artifact.id],
            expected_outputs: Vec::new(),
            success_criteria: vec![dragent_workspace_model::AcceptanceCriterionId::new()],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 2,
            updated_at_unix_ms: 2,
        };
        let contract_command = |expected_revision| WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: Some(expected_revision),
            payload: WorkspaceCommandPayload::UpsertWorkContract {
                work_contract: contract.clone(),
            },
        };
        assert!(accept_local_workspace_command(&mut connection, &contract_command(1)).is_err());

        let artifact_events = accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: 2,
                expected_revision: Some(1),
                payload: WorkspaceCommandPayload::UpsertArtifact {
                    artifact: artifact.clone(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            artifact_events[0].payload,
            WorkspaceEventPayload::ArtifactUpdated { .. }
        ));
        accept_local_workspace_command(&mut connection, &contract_command(2)).unwrap();
        let removal = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 3,
            expected_revision: Some(3),
            payload: WorkspaceCommandPayload::ReplaceArtifacts {
                artifacts: Vec::new(),
            },
        };
        let error = accept_local_workspace_command(&mut connection, &removal).unwrap_err();
        assert!(error.to_string().contains("still referenced"));
    }

    #[test]
    fn plan_revision_atomically_updates_contracts_and_rejects_stale_versions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-plan-revision.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let correlation_id = CorrelationId::new();
        save_workspace_project(&mut connection, &project).unwrap();
        let command = |expected_revision, payload, timestamp_unix_ms| WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms,
            expected_revision,
            payload,
        };
        accept_local_workspace_command(
            &mut connection,
            &command(
                None,
                WorkspaceCommandPayload::CreateWorkItem {
                    title: "Revise a dependency plan".into(),
                    execution_location: dragent_workspace_model::ExecutionLocation::Local,
                },
                1,
            ),
        )
        .unwrap();
        accept_local_workspace_command(
            &mut connection,
            &command(
                Some(1),
                WorkspaceCommandPayload::TransitionWork {
                    status: WorkStatus::Understanding,
                },
                2,
            ),
        )
        .unwrap();
        accept_local_workspace_command(
            &mut connection,
            &command(
                Some(2),
                WorkspaceCommandPayload::TransitionWork {
                    status: WorkStatus::Planning,
                },
                3,
            ),
        )
        .unwrap();

        let role_definition = RoleDefinition {
            id: dragent_workspace_model::RoleDefinitionId::new(),
            scope: RoleDefinitionScope::Project(project.id),
            kind: dragent_workspace_model::AgentRoleKind::Worker,
            name: "Developer".into(),
            mission: "Implement the plan".into(),
            capabilities: Vec::new(),
            preferred_tools: Vec::new(),
            created_by: actor_id,
            created_at_unix_ms: 4,
            updated_at_unix_ms: 4,
        };
        accept_local_workspace_command(
            &mut connection,
            &command(
                None,
                WorkspaceCommandPayload::UpsertRoleDefinition {
                    role_definition: role_definition.clone(),
                },
                4,
            ),
        )
        .unwrap();
        let plan_id = PlanId::new();
        let task_id = PlanTaskId::new();
        let criterion_id = dragent_workspace_model::AcceptanceCriterionId::new();
        let mut contract = WorkContract {
            id: WorkContractId::new(),
            work_item_id,
            role_definition_id: role_definition.id,
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Proposed,
            mission: role_definition.mission.clone(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Completed implementation".into()],
            success_criteria: vec![criterion_id],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: vec![dragent_workspace_model::PermissionGrant {
                permission: "project_read".into(),
                scope: None,
                requires_approval: false,
            }],
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 5,
            updated_at_unix_ms: 5,
        };
        accept_local_workspace_command(
            &mut connection,
            &command(
                Some(4),
                WorkspaceCommandPayload::UpsertWorkContract {
                    work_contract: contract.clone(),
                },
                5,
            ),
        )
        .unwrap();
        let mut plan = Plan {
            id: plan_id,
            work_item_id,
            version: 1,
            status: PlanStatus::Draft,
            tasks: vec![PlanTask {
                id: task_id,
                plan_id,
                title: "Implement the first draft".into(),
                description: "Initial description".into(),
                status: PlanTaskStatus::Proposed,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: vec![criterion_id],
                assigned_contract_id: Some(contract.id),
            }],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 6,
            updated_at_unix_ms: 6,
        };
        accept_local_workspace_command(
            &mut connection,
            &command(
                Some(5),
                WorkspaceCommandPayload::UpsertPlan { plan: plan.clone() },
                6,
            ),
        )
        .unwrap();
        accept_local_workspace_command(
            &mut connection,
            &command(
                Some(6),
                WorkspaceCommandPayload::UpsertProposedTeam {
                    proposed_team: ProposedTeam {
                        id: TeamId::new(),
                        work_item_id,
                        version: 1,
                        roles: vec![ProposedTeamRole {
                            role_definition_id: role_definition.id,
                            work_contract_id: contract.id,
                            rationale: "Implementation owner".into(),
                        }],
                        status: StructuredRecordStatus::Draft,
                        created_by: actor_id,
                        created_at_unix_ms: 7,
                        updated_at_unix_ms: 7,
                    },
                },
                7,
            ),
        )
        .unwrap();

        plan.version = 2;
        plan.tasks[0].title = "Implement the reviewed revision".into();
        plan.updated_at_unix_ms = 8;
        contract.version = 2;
        contract.updated_at_unix_ms = 8;
        let revision_payload = WorkspaceCommandPayload::RevisePlan {
            plan: plan.clone(),
            work_contracts: vec![contract.clone()],
        };
        let events = accept_local_workspace_command(
            &mut connection,
            &command(Some(7), revision_payload.clone(), 8),
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].sequence, events[1].sequence), (8, 9));
        assert!(matches!(
            events[0].payload,
            WorkspaceEventPayload::WorkContractUpdated { .. }
        ));
        assert!(matches!(
            events[1].payload,
            WorkspaceEventPayload::PlanUpdated { .. }
        ));
        assert!(accept_local_workspace_command(
            &mut connection,
            &command(Some(9), revision_payload, 9),
        )
        .is_err());
    }
}
