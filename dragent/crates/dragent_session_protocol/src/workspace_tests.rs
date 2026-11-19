mod tests {
    use crate::workspace::WORKSPACE_PROTOCOL_VERSION;
    use crate::workspace_commands::*;
    use crate::workspace_error::*;
    use crate::workspace_events::*;
    use dragent_workspace_model::{
        AcceptanceCriterion, ActorId, AgentInstance, AgentInstanceId, AgentInstanceStatus,
        AgentMessage, AgentMessageKind, AgentMessageRouting, ArtifactRef, AttentionItem,
        ChargingAuthorization, Checkpoint, Constraint, Decision, DeviceId, Evidence,
        ExecutionAttempt, ExecutionAttemptId, ExecutionAttemptStatus, ExecutionLease,
        ExecutionLeaseId, ExecutionLeaseStatus, ExecutionLocation, Plan, PlanTaskId,
        PlanTaskStatus, ProjectId, ProposedTeam, Requirement, Risk, RoleDefinition,
        ShapingProposal, ShapingRecords, WorkBrief, WorkContract, WorkContractId,
        WorkContractStatus, WorkItemId, WorkStatus,
    };
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    const CREATE_WORK_ITEM_FIXTURE: &str =
        include_str!("../../../../contracts/workspace/v1/fixtures/create_work_item.command.json");
    const WORK_BRIEF_UPDATED_FIXTURE: &str =
        include_str!("../../../../contracts/workspace/v1/fixtures/work_brief_updated.event.json");
    const COMPLETE_SHAPING_TURN_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/complete_shaping_turn.command.json"
    );
    const TRANSITION_PLAN_TASK_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/transition_plan_task.command.json"
    );
    const PROVISION_AGENT_INSTANCE_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/provision_agent_instance.command.json"
    );
    const ACTIVATE_AGENT_INSTANCE_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/activate_agent_instance.command.json"
    );
    const START_AGENT_EXECUTION_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/start_agent_execution.command.json"
    );
    const FINISH_AGENT_EXECUTION_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/finish_agent_execution.command.json"
    );
    const RESOLVE_AGENT_EXECUTION_REVIEW_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/resolve_agent_execution_review.command.json"
    );
    const REQUEST_AGENT_EXECUTION_REVISION_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/request_agent_execution_revision.command.json"
    );
    const ACCEPT_AGENT_EXECUTION_RESULT_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/accept_agent_execution_result.command.json"
    );
    const RAISE_ATTENTION_FIXTURE: &str =
        include_str!("../../../../contracts/workspace/v1/fixtures/raise_attention.command.json");
    const POST_AGENT_MESSAGE_FIXTURE: &str =
        include_str!("../../../../contracts/workspace/v1/fixtures/post_agent_message.command.json");
    const POST_REVIEW_FINDING_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/post_review_finding.command.json"
    );
    const UPDATE_MESSAGE_DELIVERY_FIXTURE: &str = include_str!(
        "../../../../contracts/workspace/v1/fixtures/update_message_delivery.command.json"
    );
    const RECORD_EVIDENCE_FIXTURE: &str =
        include_str!("../../../../contracts/workspace/v1/fixtures/record_evidence.command.json");

    #[test]
    fn shared_create_work_item_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(CREATE_WORK_ITEM_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(CREATE_WORK_ITEM_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_raise_attention_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(RAISE_ATTENTION_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(RAISE_ATTENTION_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_routed_agent_message_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(POST_AGENT_MESSAGE_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let WorkspaceCommandPayload::PostMessage { message, routing } = &envelope.payload else {
            panic!("fixture payload changed")
        };
        assert_eq!(message.kind, AgentMessageKind::Handoff);
        assert_eq!(routing.as_ref().unwrap().recipient_ids.len(), 1);
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(POST_AGENT_MESSAGE_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_structured_review_finding_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(POST_REVIEW_FINDING_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let WorkspaceCommandPayload::PostMessage { message, .. } = &envelope.payload else {
            panic!("fixture payload changed")
        };
        let finding = message
            .review_finding
            .as_ref()
            .expect("review finding remains structured");
        assert_eq!(
            finding.verdict,
            dragent_workspace_model::ReviewVerdict::NeedsRevision
        );
        assert_eq!(finding.findings.len(), 1);
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(POST_REVIEW_FINDING_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_message_delivery_update_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(UPDATE_MESSAGE_DELIVERY_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let WorkspaceCommandPayload::UpdateMessageDelivery { delivery_state, .. } =
            &envelope.payload
        else {
            panic!("fixture payload changed")
        };
        assert_eq!(
            *delivery_state,
            dragent_workspace_model::MessageDeliveryState::Read
        );
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(UPDATE_MESSAGE_DELIVERY_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_record_evidence_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(RECORD_EVIDENCE_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let WorkspaceCommandPayload::RecordEvidence { evidence } = &envelope.payload else {
            panic!("fixture payload changed")
        };
        assert_eq!(evidence.kind, dragent_workspace_model::EvidenceKind::Test);
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(RECORD_EVIDENCE_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_execution_revision_request_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(REQUEST_AGENT_EXECUTION_REVISION_FIXTURE)
                .expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let WorkspaceCommandPayload::ResolveAgentExecutionReview { resolution, .. } =
            &envelope.payload
        else {
            panic!("fixture payload changed")
        };
        assert_eq!(
            *resolution,
            dragent_workspace_model::ExecutionReviewResolution::RevisionRequested
        );
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(REQUEST_AGENT_EXECUTION_REVISION_FIXTURE)
                .expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_execution_result_acceptance_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(ACCEPT_AGENT_EXECUTION_RESULT_FIXTURE)
                .expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let WorkspaceCommandPayload::ResolveAgentExecutionReview { resolution, .. } =
            &envelope.payload
        else {
            panic!("fixture payload changed")
        };
        assert_eq!(
            *resolution,
            dragent_workspace_model::ExecutionReviewResolution::Accepted
        );
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(ACCEPT_AGENT_EXECUTION_RESULT_FIXTURE)
                .expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_work_brief_event_fixture_round_trips_exactly() {
        let envelope: WorkspaceEventEnvelope =
            serde_json::from_str(WORK_BRIEF_UPDATED_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(WORK_BRIEF_UPDATED_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn work_brief_edit_requires_revision_and_updater_actor() {
        let event: WorkspaceEventEnvelope =
            serde_json::from_str(WORK_BRIEF_UPDATED_FIXTURE).expect("decode shared fixture");
        let WorkspaceEventPayload::WorkBriefUpdated { work_brief } = event.payload else {
            panic!("fixture payload changed")
        };
        let mut command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: event.project_id,
            work_item_id: event.work_item_id,
            actor_id: event.actor_id,
            device_id: event.device_id,
            correlation_id: event.correlation_id,
            timestamp_unix_ms: event.timestamp_unix_ms,
            expected_revision: None,
            payload: WorkspaceCommandPayload::UpsertWorkBrief { work_brief },
        };
        assert_eq!(
            command.validate(),
            Err(WorkspaceProtocolError::ExpectedRevisionRequired)
        );
        command.expected_revision = Some(7);
        command.validate().expect("valid work brief edit");
        command.actor_id = ActorId::new();
        assert_eq!(
            command.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
    }

    #[test]
    fn work_contract_edit_uses_updater_actor_and_never_creator_as_authority() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let creator = ActorId::new();
        let updater = ActorId::new();
        let contract = WorkContract {
            id: dragent_workspace_model::WorkContractId::new(),
            work_item_id,
            role_definition_id: dragent_workspace_model::RoleDefinitionId::new(),
            version: 2,
            agent_instance_id: None,
            status: dragent_workspace_model::WorkContractStatus::Proposed,
            mission: "Review the implementation and report evidence.".into(),
            task_ids: Vec::new(),
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Review findings".into()],
            success_criteria: vec![dragent_workspace_model::AcceptanceCriterionId::new()],
            write_scope: vec!["dragent/app/src".into()],
            capabilities: vec![dragent_workspace_model::CapabilityGrant {
                capability: "code_review".into(),
                scope: None,
            }],
            tools: vec!["read_file".into()],
            permissions: vec![dragent_workspace_model::PermissionGrant {
                permission: "project_read".into(),
                scope: None,
                requires_approval: false,
            }],
            budget: dragent_workspace_model::WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: creator,
            updated_by: updater,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 2,
        };
        let mut command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: updater,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: Some(1),
            payload: WorkspaceCommandPayload::UpsertWorkContract {
                work_contract: contract,
            },
        };
        command
            .validate()
            .expect("a later actor can revise a contract without becoming its creator");
        command.actor_id = creator;
        assert_eq!(
            command.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
        command.actor_id = updater;
        let WorkspaceCommandPayload::UpsertWorkContract { work_contract } = &mut command.payload
        else {
            unreachable!()
        };
        work_contract.status = WorkContractStatus::Approved;
        work_contract.agent_instance_id = Some(AgentInstanceId::new());
        assert!(matches!(
            command.validate(),
            Err(WorkspaceProtocolError::InvalidPayload(_))
        ));
    }

    #[test]
    fn artifact_upsert_is_revisioned_and_rejects_unsafe_paths() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let mut command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: ActorId::new(),
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: Some(1),
            payload: WorkspaceCommandPayload::UpsertArtifact {
                artifact: ArtifactRef {
                    id: dragent_workspace_model::ArtifactId::new(),
                    work_item_id,
                    relative_path: "docs/requirements.md".into(),
                    kind: dragent_workspace_model::ArtifactKind::Requirements,
                },
            },
        };
        command.validate().expect("valid artifact upsert");
        command.expected_revision = None;
        assert_eq!(
            command.validate(),
            Err(WorkspaceProtocolError::ExpectedRevisionRequired)
        );
        command.expected_revision = Some(1);
        let WorkspaceCommandPayload::UpsertArtifact { artifact } = &mut command.payload else {
            unreachable!()
        };
        artifact.relative_path = "../outside.md".into();
        assert!(matches!(
            command.validate(),
            Err(WorkspaceProtocolError::InvalidPayload(_))
        ));

        command.payload = WorkspaceCommandPayload::ReplaceArtifacts {
            artifacts: Vec::new(),
        };
        command
            .validate()
            .expect("an empty replacement set can remove all unreferenced artifacts");
        let artifact = ArtifactRef {
            id: dragent_workspace_model::ArtifactId::new(),
            work_item_id,
            relative_path: "docs/design.md".into(),
            kind: dragent_workspace_model::ArtifactKind::Design,
        };
        command.payload = WorkspaceCommandPayload::ReplaceArtifacts {
            artifacts: vec![artifact.clone(), artifact],
        };
        assert!(matches!(
            command.validate(),
            Err(WorkspaceProtocolError::InvalidPayload(_))
        ));
    }

    #[test]
    fn plan_revision_keeps_contract_assignments_atomic_and_unapproved() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let plan_id = dragent_workspace_model::PlanId::new();
        let task_id = PlanTaskId::new();
        let criterion_id = dragent_workspace_model::AcceptanceCriterionId::new();
        let contract_id = WorkContractId::new();
        let contract = WorkContract {
            id: contract_id,
            work_item_id,
            role_definition_id: dragent_workspace_model::RoleDefinitionId::new(),
            version: 2,
            agent_instance_id: None,
            status: WorkContractStatus::Proposed,
            mission: "Implement the revised task".into(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Implementation evidence".into()],
            success_criteria: vec![criterion_id],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: vec![dragent_workspace_model::PermissionGrant {
                permission: "project_read".into(),
                scope: None,
                requires_approval: false,
            }],
            budget: dragent_workspace_model::WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 2,
        };
        let plan = Plan {
            id: plan_id,
            work_item_id,
            version: 2,
            status: dragent_workspace_model::PlanStatus::Draft,
            tasks: vec![dragent_workspace_model::PlanTask {
                id: task_id,
                plan_id,
                title: "Implement revision".into(),
                description: "Keep assignments consistent".into(),
                status: PlanTaskStatus::Proposed,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: vec![criterion_id],
                assigned_contract_id: Some(contract_id),
            }],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 2,
        };
        let mut command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: Some(1),
            payload: WorkspaceCommandPayload::RevisePlan {
                plan: plan.clone(),
                work_contracts: vec![contract.clone()],
            },
        };
        command.validate().expect("valid atomic plan revision");

        if let WorkspaceCommandPayload::RevisePlan { plan, .. } = &mut command.payload {
            plan.status = dragent_workspace_model::PlanStatus::Approved;
            plan.approved_by = Some(actor_id);
            plan.approved_at_unix_ms = Some(2);
        }
        assert!(command.validate().is_err());
        if let WorkspaceCommandPayload::RevisePlan {
            plan,
            work_contracts,
        } = &mut command.payload
        {
            plan.status = dragent_workspace_model::PlanStatus::Draft;
            plan.approved_by = None;
            plan.approved_at_unix_ms = None;
            work_contracts[0].task_ids.clear();
        }
        assert!(command.validate().is_err());
    }

    #[test]
    fn shared_complete_shaping_turn_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(COMPLETE_SHAPING_TURN_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(COMPLETE_SHAPING_TURN_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn complete_shaping_turn_records_must_cite_its_principal_message() {
        let mut envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(COMPLETE_SHAPING_TURN_FIXTURE).expect("decode shared fixture");
        let WorkspaceCommandPayload::CompleteShapingTurn { records, .. } = &mut envelope.payload
        else {
            panic!("fixture payload changed")
        };
        let dragent_workspace_model::ProvenanceSource::AgentMessage { message_id, .. } =
            &mut records.requirements[0].provenance[0].source
        else {
            panic!("fixture provenance changed")
        };
        *message_id = dragent_workspace_model::AgentMessageId::new();

        assert!(matches!(
            envelope.validate(),
            Err(WorkspaceProtocolError::InvalidPayload(_))
        ));
    }

    #[test]
    fn structured_record_edits_require_revision_and_actor_provenance() {
        let complete: WorkspaceCommandEnvelope =
            serde_json::from_str(COMPLETE_SHAPING_TURN_FIXTURE).expect("decode shared fixture");
        let WorkspaceCommandPayload::CompleteShapingTurn { records, .. } = &complete.payload else {
            panic!("fixture payload changed")
        };
        let mut envelope = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: complete.project_id,
            work_item_id: complete.work_item_id,
            actor_id: complete.actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: None,
            payload: WorkspaceCommandPayload::UpsertRequirement {
                requirement: records.requirements[0].clone(),
            },
        };
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::ExpectedRevisionRequired)
        );

        envelope.expected_revision = Some(3);
        envelope.validate().expect("valid strict edit command");

        let WorkspaceCommandPayload::UpsertRequirement { requirement } = &mut envelope.payload
        else {
            unreachable!()
        };
        requirement.provenance = vec![dragent_workspace_model::Provenance {
            source: dragent_workspace_model::ProvenanceSource::ManualEdit,
            recorded_by: envelope.actor_id,
            recorded_at_unix_ms: 2,
        }];
        envelope
            .validate()
            .expect("manual edit provenance from the command actor");

        envelope.actor_id = ActorId::new();
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
    }

    #[test]
    fn validation_rejects_unknown_versions_and_non_idempotent_identity() {
        let mut envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(CREATE_WORK_ITEM_FIXTURE).unwrap();
        envelope.schema_version = WORKSPACE_PROTOCOL_VERSION + 1;
        assert!(matches!(
            envelope.validate(),
            Err(WorkspaceProtocolError::UnsupportedSchemaVersion { .. })
        ));
        envelope.schema_version = WORKSPACE_PROTOCOL_VERSION;
        envelope.command_id = WorkspaceCommandId(Uuid::nil());
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::NilIdentifier("command_id"))
        );
    }

    #[test]
    fn every_command_requires_stable_work_item_identity() {
        let mut envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(CREATE_WORK_ITEM_FIXTURE).unwrap();
        envelope.work_item_id = WorkItemId(Uuid::nil());
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::NilIdentifier("work_item_id"))
        );
    }

    #[test]
    fn task_transitions_require_revision_and_task_identity() {
        let mut envelope = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: ProjectId::new(),
            work_item_id: WorkItemId::new(),
            actor_id: ActorId::new(),
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 1,
            expected_revision: None,
            payload: WorkspaceCommandPayload::TransitionPlanTask {
                task_id: PlanTaskId::new(),
                status: PlanTaskStatus::Ready,
            },
        };
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::ExpectedRevisionRequired)
        );
        envelope.expected_revision = Some(3);
        envelope.validate().expect("valid task transition intent");
        let WorkspaceCommandPayload::TransitionPlanTask { task_id, .. } = &mut envelope.payload
        else {
            unreachable!()
        };
        *task_id = PlanTaskId(Uuid::nil());
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::NilIdentifier("task_id"))
        );
    }

    #[test]
    fn shared_task_transition_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(TRANSITION_PLAN_TASK_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(TRANSITION_PLAN_TASK_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_agent_provisioning_fixture_round_trips_exactly() {
        let mut envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(PROVISION_AGENT_INSTANCE_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(&envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(PROVISION_AGENT_INSTANCE_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);

        let WorkspaceCommandPayload::ProvisionAgentInstance { model_route, .. } =
            &mut envelope.payload
        else {
            unreachable!()
        };
        model_route.clear();
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::EmptyField("model_route"))
        );
    }

    #[test]
    fn shared_agent_activation_fixture_round_trips_exactly() {
        let mut envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(ACTIVATE_AGENT_INSTANCE_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(&envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(ACTIVATE_AGENT_INSTANCE_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);

        let WorkspaceCommandPayload::ActivateAgentInstance { charging, .. } = &mut envelope.payload
        else {
            unreachable!()
        };
        charging.owner_id = ActorId::new();
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
    }

    #[test]
    fn shared_agent_execution_start_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(START_AGENT_EXECUTION_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(&envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(START_AGENT_EXECUTION_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_agent_execution_finish_fixture_round_trips_exactly() {
        let mut envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(FINISH_AGENT_EXECUTION_FIXTURE).expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(&envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(FINISH_AGENT_EXECUTION_FIXTURE).expect("decode fixture value");
        assert_eq!(actual, expected);

        let WorkspaceCommandPayload::FinishAgentExecution { outcome, .. } = &mut envelope.payload
        else {
            unreachable!()
        };
        *outcome = ExecutionAttemptStatus::Running;
        assert!(envelope.validate().is_err());
    }

    #[test]
    fn shared_agent_execution_review_fixture_round_trips_exactly() {
        let envelope: WorkspaceCommandEnvelope =
            serde_json::from_str(RESOLVE_AGENT_EXECUTION_REVIEW_FIXTURE)
                .expect("decode shared fixture");
        envelope.validate().expect("valid shared fixture");
        let actual = serde_json::to_value(&envelope).expect("encode envelope");
        let expected: serde_json::Value =
            serde_json::from_str(RESOLVE_AGENT_EXECUTION_REVIEW_FIXTURE)
                .expect("decode fixture value");
        assert_eq!(actual, expected);
    }

    #[test]
    fn events_require_a_positive_per_work_item_sequence() {
        let mut envelope: WorkspaceEventEnvelope =
            serde_json::from_str(WORK_BRIEF_UPDATED_FIXTURE).unwrap();
        envelope.sequence = 0;
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::InvalidSequence)
        );
    }

    #[test]
    fn payload_identity_cannot_escape_its_envelope_scope() {
        let mut envelope: WorkspaceEventEnvelope =
            serde_json::from_str(WORK_BRIEF_UPDATED_FIXTURE).unwrap();
        let WorkspaceEventPayload::WorkBriefUpdated { work_brief } = &mut envelope.payload else {
            panic!("fixture payload changed")
        };
        work_brief.work_item_id = WorkItemId::new();
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::PayloadWorkItemMismatch)
        );
    }

    #[test]
    fn post_message_cannot_spoof_its_sender() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let envelope = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 1,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message: AgentMessage {
                    id: dragent_workspace_model::AgentMessageId::new(),
                    thread_id: dragent_workspace_model::ThreadId::new(),
                    work_item_id,
                    sender_id: ActorId::new(),
                    kind: dragent_workspace_model::AgentMessageKind::Discussion,
                    body: "Hello".into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: dragent_workspace_model::MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 1,
                    updated_at_unix_ms: 1,
                },
                routing: None,
            },
        };

        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
        let WorkspaceCommandPayload::PostMessage { message, routing } = envelope.payload else {
            unreachable!()
        };
        let event = WorkspaceEventEnvelope {
            event_id: WorkspaceEventId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            sequence: 1,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            task_id: None,
            correlation_id: CorrelationId::new(),
            causation_id: None,
            timestamp_unix_ms: 1,
            payload: WorkspaceEventPayload::MessagePosted { message, routing },
        };
        assert_eq!(
            event.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
    }

    #[test]
    fn attention_commands_require_revision_and_requester_authority() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let attention_item = AttentionItem {
            id: dragent_workspace_model::AttentionItemId::new(),
            work_item_id,
            task_id: None,
            thread_id: None,
            execution_attempt_id: None,
            tool_call_id: None,
            kind: dragent_workspace_model::AttentionKind::Decision,
            status: dragent_workspace_model::AttentionStatus::Open,
            summary: "Choose a deployment region".into(),
            requested_by: actor_id,
            assigned_to: vec![ActorId::new()],
            created_at_unix_ms: 2,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        let mut envelope = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: None,
            payload: WorkspaceCommandPayload::RaiseAttention { attention_item },
        };
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::ExpectedRevisionRequired)
        );
        envelope.expected_revision = Some(1);
        assert_eq!(envelope.validate(), Ok(()));
        envelope.actor_id = ActorId::new();
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::PayloadActorMismatch)
        );
    }

    #[test]
    fn shaping_memory_batches_validate_bounds_and_content() {
        use dragent_workspace_model::{
            ShapingMemoryKind, ShapingMemoryNote, ShapingMemoryNoteId, MAX_SHAPING_MEMORY_BATCH,
        };
        let work_item_id = WorkItemId::new();
        let note = ShapingMemoryNote {
            id: ShapingMemoryNoteId::new(),
            work_item_id,
            kind: ShapingMemoryKind::Preference,
            text: "Prefers Postgres".into(),
            recorded_at_unix_ms: 2,
        };
        let mut envelope = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: ProjectId::new(),
            work_item_id,
            actor_id: ActorId::new(),
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 2,
            expected_revision: None,
            payload: WorkspaceCommandPayload::RecordShapingMemory {
                notes: vec![note.clone()],
            },
        };
        assert_eq!(
            envelope.validate(),
            Err(WorkspaceProtocolError::ExpectedRevisionRequired)
        );
        envelope.expected_revision = Some(1);
        assert_eq!(envelope.validate(), Ok(()));
        // Empty batches, oversize batches, and invalid notes all fail.
        envelope.payload = WorkspaceCommandPayload::RecordShapingMemory { notes: vec![] };
        assert!(envelope.validate().is_err());
        envelope.payload = WorkspaceCommandPayload::RecordShapingMemory {
            notes: vec![note.clone(); MAX_SHAPING_MEMORY_BATCH + 1],
        };
        assert!(envelope.validate().is_err());
        envelope.payload = WorkspaceCommandPayload::RecordShapingMemory {
            notes: vec![ShapingMemoryNote {
                text: String::new(),
                ..note.clone()
            }],
        };
        assert!(envelope.validate().is_err());
        // The event carries the note with its work item for routing.
        let event = WorkspaceEventPayload::ShapingMemoryRecorded { note: note.clone() };
        assert_eq!(event.work_item_id(), Some(work_item_id));
        assert!(event.validate().is_ok());
    }

    /// Directory holding the checked-in canonical schemas. The Rust types are
    /// the single source of truth: `schemars` derives these files from them,
    /// and every client (Rust, Go, future TypeScript/Swift/Kotlin) validates
    /// against the same documents. See `contracts/workspace/v1/README.md`.
    fn schema_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("contracts")
            .join("workspace")
            .join("v1")
            .join("schema")
    }

    fn generated_schemas() -> Vec<(&'static str, serde_json::Value)> {
        vec![
            (
                "workspace-command-envelope.v1.schema.json",
                serde_json::to_value(schemars::schema_for!(WorkspaceCommandEnvelope))
                    .expect("command schema"),
            ),
            (
                "workspace-event-envelope.v1.schema.json",
                serde_json::to_value(schemars::schema_for!(WorkspaceEventEnvelope))
                    .expect("event schema"),
            ),
        ]
    }

    #[test]
    fn checked_in_schemas_match_generated_types() {
        let dir = schema_dir();
        for (filename, schema) in generated_schemas() {
            let path = dir.join(filename);
            let rendered = serde_json::to_string_pretty(&schema).expect("render schema") + "\n";
            match std::fs::read_to_string(&path) {
                Ok(checked_in) if checked_in == rendered => {}
                _ if std::env::var("REGENERATE_SCHEMAS").is_ok() => {
                    std::fs::create_dir_all(&dir).expect("schema dir");
                    std::fs::write(&path, rendered).expect("write schema");
                }
                _ => panic!(
                    "schema {} is stale: re-run with REGENERATE_SCHEMAS=1 after changing wire types",
                    path.display()
                ),
            }
        }
    }

    fn validate_fixture_against_schema(fixture: &str, name: &str, schema: &serde_json::Value) {
        let instance: serde_json::Value =
            serde_json::from_str(fixture).expect("fixture is valid JSON");
        let validator = jsonschema::validator_for(schema).expect("valid schema");
        if !validator.is_valid(&instance) {
            let errors: Vec<String> = validator
                .iter_errors(&instance)
                .map(|error| error.to_string())
                .collect();
            panic!("fixture {name} violates canonical schema: {errors:?}");
        }
    }

    #[test]
    fn all_shared_fixtures_validate_against_canonical_schemas() {
        let dir = schema_dir();
        let command_schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("workspace-command-envelope.v1.schema.json"))
                .expect("checked-in command schema"),
        )
        .expect("parse command schema");
        let event_schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("workspace-event-envelope.v1.schema.json"))
                .expect("checked-in event schema"),
        )
        .expect("parse event schema");
        let commands = [
            ("create_work_item", CREATE_WORK_ITEM_FIXTURE),
            ("complete_shaping_turn", COMPLETE_SHAPING_TURN_FIXTURE),
            ("transition_plan_task", TRANSITION_PLAN_TASK_FIXTURE),
            ("provision_agent_instance", PROVISION_AGENT_INSTANCE_FIXTURE),
            ("activate_agent_instance", ACTIVATE_AGENT_INSTANCE_FIXTURE),
            ("start_agent_execution", START_AGENT_EXECUTION_FIXTURE),
            ("finish_agent_execution", FINISH_AGENT_EXECUTION_FIXTURE),
            (
                "resolve_agent_execution_review",
                RESOLVE_AGENT_EXECUTION_REVIEW_FIXTURE,
            ),
            (
                "request_agent_execution_revision",
                REQUEST_AGENT_EXECUTION_REVISION_FIXTURE,
            ),
            (
                "accept_agent_execution_result",
                ACCEPT_AGENT_EXECUTION_RESULT_FIXTURE,
            ),
            ("raise_attention", RAISE_ATTENTION_FIXTURE),
            ("post_agent_message", POST_AGENT_MESSAGE_FIXTURE),
            ("post_review_finding", POST_REVIEW_FINDING_FIXTURE),
            ("update_message_delivery", UPDATE_MESSAGE_DELIVERY_FIXTURE),
            ("record_evidence", RECORD_EVIDENCE_FIXTURE),
        ];
        for (name, fixture) in commands {
            validate_fixture_against_schema(fixture, name, &command_schema);
        }
        validate_fixture_against_schema(
            WORK_BRIEF_UPDATED_FIXTURE,
            "work_brief_updated",
            &event_schema,
        );
    }
}
