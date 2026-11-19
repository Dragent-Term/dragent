#[cfg(test)]
mod tests {

    use crate::workspace_store::*;
    use diesel::Connection;
    use dragent_session_protocol::workspace::WorkspaceCommandEnvelope;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandId, WorkspaceCommandPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_session_protocol::workspace::{
        WorkspaceEventEnvelope, WorkspaceEventId, WorkspaceEventPayload,
    };
    use dragent_workspace_model::AttentionItem;
    use dragent_workspace_model::WorkspaceEvent;
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
    fn attention_is_durable_retry_safe_and_resolved_by_an_assignee() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-attention.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work_item_id = WorkItemId::new();
        let requester_id = ActorId::new();
        let assignee_id = ActorId::new();
        let correlation_id = CorrelationId::new();
        save_workspace_project(&mut connection, &project).unwrap();
        accept_local_workspace_command(
            &mut connection,
            &WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id,
                actor_id: requester_id,
                device_id: None,
                correlation_id,
                timestamp_unix_ms: 1,
                expected_revision: None,
                payload: WorkspaceCommandPayload::CreateWorkItem {
                    title: "Choose a release region".into(),
                    execution_location: dragent_workspace_model::ExecutionLocation::Local,
                },
            },
        )
        .unwrap();

        let attention_item = AttentionItem {
            id: AttentionItemId::new(),
            work_item_id,
            task_id: None,
            thread_id: None,
            execution_attempt_id: None,
            tool_call_id: None,
            kind: AttentionKind::Decision,
            status: AttentionStatus::Open,
            summary: "Choose the production region".into(),
            requested_by: requester_id,
            assigned_to: vec![assignee_id],
            created_at_unix_ms: 2,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        let raise = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id: requester_id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms: 2,
            expected_revision: Some(1),
            payload: WorkspaceCommandPayload::RaiseAttention {
                attention_item: attention_item.clone(),
            },
        };
        let raised = accept_local_workspace_command(&mut connection, &raise).unwrap();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].sequence, 2);
        assert_eq!(
            accept_local_workspace_command(&mut connection, &raise).unwrap(),
            raised
        );

        let resolve = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id,
            actor_id: assignee_id,
            device_id: None,
            correlation_id,
            timestamp_unix_ms: 3,
            expected_revision: Some(2),
            payload: WorkspaceCommandPayload::ResolveAttention {
                attention_item_id: attention_item.id,
                outcome: AttentionResolution::Answered,
                resolution: "Use eu-west-2".into(),
            },
        };
        let resolved = accept_local_workspace_command(&mut connection, &resolve).unwrap();
        let WorkspaceEventPayload::AttentionUpdated { attention_item } = &resolved[0].payload
        else {
            panic!("resolution emitted the wrong event")
        };
        assert_eq!(attention_item.status, AttentionStatus::Resolved);
        assert_eq!(
            attention_item.resolution_kind,
            Some(AttentionResolution::Answered)
        );
        assert_eq!(attention_item.resolution.as_deref(), Some("Use eu-west-2"));
        assert_eq!(attention_item.resolved_at_unix_ms, Some(3));
        assert_eq!(
            load_canonical_workspace_events(&mut connection, project.id, work_item_id, 0, 10)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn work_creation_and_lifecycle_transitions_are_canonical_and_durable() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-work-creation.sqlite");
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
                title: "Build the agent workspace".into(),
                execution_location: dragent_workspace_model::ExecutionLocation::Local,
            },
        };
        let created = accept_local_workspace_command(&mut connection, &create).unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].sequence, 1);
        assert!(matches!(
            &created[0].payload,
            WorkspaceEventPayload::WorkCreated { .. }
        ));
        assert_eq!(
            accept_local_workspace_command(&mut connection, &create).unwrap(),
            created
        );
        let work = load_workspace_work_item(&mut connection, project.id, work_item_id)
            .unwrap()
            .unwrap();
        assert_eq!(work.title, "Build the agent workspace");
        assert_eq!(work.status, WorkStatus::Draft);

        let transition =
            |command_id, expected_revision, status, timestamp_unix_ms| WorkspaceCommandEnvelope {
                command_id,
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
        let understanding = accept_local_workspace_command(
            &mut connection,
            &transition(WorkspaceCommandId::new(), 1, WorkStatus::Understanding, 3),
        )
        .unwrap();
        assert_eq!(understanding[0].sequence, 2);
        assert_eq!(
            load_workspace_work_item(&mut connection, project.id, work_item_id)
                .unwrap()
                .unwrap()
                .status,
            WorkStatus::Understanding
        );

        assert!(accept_local_workspace_command(
            &mut connection,
            &transition(WorkspaceCommandId::new(), 1, WorkStatus::Planning, 4),
        )
        .is_err());
        let planning = accept_local_workspace_command(
            &mut connection,
            &transition(WorkspaceCommandId::new(), 2, WorkStatus::Planning, 5),
        )
        .unwrap();
        assert_eq!(planning[0].sequence, 3);
        assert_eq!(
            load_workspace_work_item(&mut connection, project.id, work_item_id)
                .unwrap()
                .unwrap()
                .status,
            WorkStatus::Planning
        );

        let mut duplicate_create = create.clone();
        duplicate_create.command_id = WorkspaceCommandId::new();
        assert!(accept_local_workspace_command(&mut connection, &duplicate_create).is_err());
    }

    #[test]
    fn work_contract_model_route_survives_authority_and_restart() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-model-route.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let correlation_id = CorrelationId::new();
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
                correlation_id,
                timestamp_unix_ms: 1,
                expected_revision: None,
                payload: WorkspaceCommandPayload::CreateWorkItem {
                    title: "Pick models per task".into(),
                    execution_location: dragent_workspace_model::ExecutionLocation::Local,
                },
            },
        )
        .unwrap();

        let mut contract = WorkContract {
            id: WorkContractId::new(),
            work_item_id,
            role_definition_id: dragent_workspace_model::RoleDefinitionId::new(),
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Proposed,
            mission: "Implement the assigned task".into(),
            task_ids: Vec::new(),
            input_artifact_ids: Vec::new(),
            expected_outputs: Vec::new(),
            success_criteria: vec![dragent_workspace_model::AcceptanceCriterionId::new()],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
            preferred_model_route: Some("pro-a".into()),
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 2,
            updated_at_unix_ms: 2,
        };
        let upsert = |connection: &mut diesel::sqlite::SqliteConnection,
                      contract: &WorkContract,
                      revision: u64| {
            accept_local_workspace_command(
                connection,
                &WorkspaceCommandEnvelope {
                    command_id: WorkspaceCommandId::new(),
                    schema_version: WORKSPACE_PROTOCOL_VERSION,
                    project_id: project.id,
                    work_item_id,
                    actor_id,
                    device_id: None,
                    correlation_id,
                    timestamp_unix_ms: 2,
                    expected_revision: Some(revision),
                    payload: WorkspaceCommandPayload::UpsertWorkContract {
                        work_contract: contract.clone(),
                    },
                },
            )
        };
        let events = upsert(&mut connection, &contract, 1).unwrap();
        assert!(matches!(
            events[0].payload,
            WorkspaceEventPayload::WorkContractUpdated { .. }
        ));

        // "auto" is an intent, never a durable route: the authority rejects
        // it so nothing hardcoded can masquerade as server-resolved Auto.
        let mut auto_contract = contract.clone();
        auto_contract.version = 2;
        auto_contract.preferred_model_route = Some("auto".into());
        assert!(upsert(&mut connection, &auto_contract, 2).is_err());

        // A paid-tier selection persists as a second typed command.
        contract.version = 2;
        contract.preferred_model_route = Some("pro-b".into());
        upsert(&mut connection, &contract, 2).unwrap();

        // Restart: reopen the database file and replay the canonical tail.
        drop(connection);
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let tail =
            load_canonical_workspace_events(&mut connection, project.id, work_item_id, 0, 100)
                .unwrap();
        let mut latest_route = None;
        for event in &tail {
            if let WorkspaceEventPayload::WorkContractUpdated { work_contract } = &event.payload {
                if work_contract.id == contract.id {
                    latest_route.clone_from(&work_contract.preferred_model_route);
                }
            }
        }
        assert_eq!(latest_route.as_deref(), Some("pro-b"));
    }

    #[test]
    fn work_item_and_event_tail_survive_a_database_round_trip() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("workspace-round-trip.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 10);
        let mut work = WorkItem::new(project.id, "Organization billing", 11);
        work.transition_to(WorkStatus::Understanding, 12).unwrap();
        work.transition_to(WorkStatus::Planning, 13).unwrap();
        work.transition_to(WorkStatus::ReadyToExecute, 14).unwrap();
        let event = WorkspaceEvent {
            sequence: 4,
            timestamp_unix_ms: 14,
            project_id: project.id,
            work_item_id: Some(work.id),
            kind: WorkspaceEventKind::WorkStatusChanged {
                status: WorkStatus::ReadyToExecute,
            },
        };
        let message_event = WorkspaceEvent {
            sequence: 5,
            timestamp_unix_ms: 15,
            project_id: project.id,
            work_item_id: Some(work.id),
            kind: WorkspaceEventKind::UserMessagePosted {
                body: "Use seat-based billing for the first release".into(),
            },
        };
        let shaping_event = WorkspaceEvent {
            sequence: 6,
            timestamp_unix_ms: 16,
            project_id: project.id,
            work_item_id: Some(work.id),
            kind: WorkspaceEventKind::ShapingProposalUpdated {
                proposal: dragent_workspace_model::ShapingProposal {
                    revision: 1,
                    outcome: "Ship organization billing".into(),
                    requirements: vec!["Support seat-based plans".into()],
                    open_decisions: vec!["Choose a proration policy".into()],
                    acceptance_criteria: vec!["Admins can update seat count".into()],
                    plan_summary: "Implement behind a feature flag".into(),
                },
            },
        };

        save_workspace_project(&mut connection, &project).unwrap();
        save_workspace_work_item(&mut connection, &work).unwrap();
        save_workspace_event(&mut connection, &event).unwrap();
        save_workspace_event(&mut connection, &message_event).unwrap();
        save_workspace_event(&mut connection, &shaping_event).unwrap();

        assert_eq!(
            load_recent_workspace_work_items(&mut connection).unwrap(),
            [work]
        );
        assert_eq!(
            load_recent_workspace_events(&mut connection).unwrap(),
            [event, message_event, shaping_event]
        );
    }

    #[test]
    fn principal_reply_to_a_work_item_level_question_needs_no_task() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-workspace.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work = WorkItem::new(project.id, "Billing", 2);
        let human = ActorRef::local_user(project.id, None);
        let principal = ActorRef::principal_agent(project.id);
        let thread_id = ThreadId::new();
        save_workspace_project(&mut connection, &project).unwrap();
        save_workspace_work_item(&mut connection, &work).unwrap();

        let post = |sender: &ActorRef,
                    kind,
                    reply_to,
                    recipient: ActorId,
                    timestamp: i64,
                    revision: u64| {
            WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: project.id,
                work_item_id: work.id,
                actor_id: sender.id,
                device_id: None,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: timestamp,
                expected_revision: Some(revision),
                payload: WorkspaceCommandPayload::PostMessage {
                    message: AgentMessage {
                        id: AgentMessageId::new(),
                        thread_id,
                        work_item_id: work.id,
                        sender_id: sender.id,
                        kind,
                        body: "which agent is working right now?".into(),
                        review_finding: None,
                        reply_to,
                        artifact_ids: Vec::new(),
                        evidence_ids: Vec::new(),
                        delivery_state: MessageDeliveryState::LocalOnly,
                        created_at_unix_ms: timestamp,
                        updated_at_unix_ms: timestamp,
                    },
                    routing: Some(AgentMessageRouting {
                        recipient_ids: vec![recipient],
                        task_id: None,
                        execution_attempt_id: None,
                    }),
                },
            }
        };

        let question = post(
            &human,
            AgentMessageKind::DecisionRequest,
            None,
            principal.id,
            10,
            0,
        );
        let events = accept_local_post_message(&mut connection, &question).unwrap();
        let question_id = match &question.payload {
            WorkspaceCommandPayload::PostMessage { message, .. } => message.id,
            _ => unreachable!(),
        };
        assert_eq!(events.len(), 1);

        let reply = post(
            &principal,
            AgentMessageKind::Discussion,
            Some(question_id),
            human.id,
            20,
            1,
        );
        let events = accept_local_post_message(&mut connection, &reply)
            .expect("a Principal Discussion reply without a task is valid");
        assert_eq!(events.len(), 1);

        // Operational kinds other than DecisionRequest still need a task.
        let handoff = post(&principal, AgentMessageKind::Handoff, None, human.id, 30, 2);
        assert!(accept_local_post_message(&mut connection, &handoff).is_err());
    }

    #[test]
    fn local_message_acceptance_is_atomic_idempotent_and_sequenced() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("canonical-workspace.sqlite");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().join("project"), 1);
        let work = WorkItem::new(project.id, "Billing", 2);
        let actor_id = ActorId::new();
        let thread_id = ThreadId::new();
        save_workspace_project(&mut connection, &project).unwrap();
        save_workspace_work_item(&mut connection, &work).unwrap();

        let command = |command_id, body: &str| WorkspaceCommandEnvelope {
            command_id,
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id: work.id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 10,
            expected_revision: Some(2),
            payload: WorkspaceCommandPayload::PostMessage {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    thread_id,
                    work_item_id: work.id,
                    sender_id: actor_id,
                    kind: AgentMessageKind::Discussion,
                    body: body.into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 10,
                    updated_at_unix_ms: 10,
                },
                routing: None,
            },
        };

        let first_command = command(WorkspaceCommandId::new(), "Use seat billing");
        let second_command = command(WorkspaceCommandId::new(), "Acknowledged");
        let batch = vec![first_command.clone(), second_command];
        let probe: anyhow::Result<()> = connection.transaction(|conn| {
            let accepted = accept_local_workspace_commands(conn, &batch)?;
            assert_eq!(accepted.len(), 2);
            assert_eq!(accepted[0].sequence, 1);
            assert_eq!(accepted[1].sequence, 2);
            assert_eq!(accept_local_workspace_commands(conn, &batch)?, accepted);
            anyhow::bail!("rollback successful batch probe")
        });
        assert!(probe.is_err());
        // A Principal acknowledgement and its effects must all commit or none
        // may become visible. The second command is deliberately invalid.
        let mut invalid = command(WorkspaceCommandId::new(), "Invalid reply");
        invalid.work_item_id = WorkItemId::new();
        assert!(accept_local_workspace_commands(
            &mut connection,
            &[first_command.clone(), invalid]
        )
        .is_err());
        assert!(load_command_outcome(&mut connection, &first_command)
            .unwrap()
            .is_none());
        let mut invalid = command(WorkspaceCommandId::new(), "Invalid reply");
        invalid.actor_id = ActorId::new(); // Does not match the message sender.
        assert!(accept_local_workspace_commands(
            &mut connection,
            &[first_command.clone(), invalid]
        )
        .is_err());
        assert!(load_command_outcome(&mut connection, &first_command)
            .unwrap()
            .is_none());
        let first = accept_local_post_message(&mut connection, &first_command).unwrap();
        drop(connection);
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let retry = accept_local_post_message(&mut connection, &first_command).unwrap();
        assert_eq!(first, retry);
        assert_eq!(first[0].sequence, 1);
        assert_eq!(
            load_recent_workspace_events(&mut connection)
                .unwrap()
                .into_iter()
                .filter(|event| matches!(event.kind, WorkspaceEventKind::UserMessagePosted { .. }))
                .count(),
            1
        );

        let second = accept_local_post_message(
            &mut connection,
            &command(WorkspaceCommandId::new(), "Add invoice downloads"),
        )
        .unwrap();
        assert_eq!(second[0].sequence, 2);
        assert_eq!(
            load_canonical_workspace_events(&mut connection, project.id, work.id, 0, 100).unwrap(),
            [first[0].clone(), second[0].clone()]
        );
        assert_eq!(
            load_canonical_workspace_events(&mut connection, project.id, work.id, 1, 100).unwrap(),
            [second[0].clone()]
        );
        assert!(load_canonical_workspace_events(
            &mut connection,
            project.id,
            WorkItemId::new(),
            0,
            100
        )
        .unwrap()
        .is_empty());

        let principal_message_id = AgentMessageId::new();
        let proposal = ShapingProposal {
            revision: 1,
            outcome: "Ship organization billing".into(),
            requirements: vec!["Support seats".into()],
            open_decisions: Vec::new(),
            acceptance_criteria: vec!["Admins can update seats".into()],
            plan_summary: "Implement billing behind a feature flag".into(),
        };
        let records = dragent_workspace_model::ShapingRecords::from_proposal(
            work.id,
            &proposal,
            thread_id,
            principal_message_id,
            actor_id,
            20,
        );
        let completed_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id: work.id,
            actor_id,
            device_id: None,
            correlation_id: first_command.correlation_id,
            timestamp_unix_ms: 20,
            expected_revision: None,
            payload: WorkspaceCommandPayload::CompleteShapingTurn {
                message: AgentMessage {
                    id: principal_message_id,
                    thread_id,
                    work_item_id: work.id,
                    sender_id: actor_id,
                    kind: AgentMessageKind::StatusUpdate,
                    body: "I prepared the billing plan".into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 20,
                    updated_at_unix_ms: 20,
                },
                proposal: proposal.clone(),
                records: records.clone(),
                artifacts: Vec::new(),
                role_definitions: Vec::new(),
                work_contracts: Vec::new(),
                proposed_team: None,
                plan: None,
            },
        };
        let completed =
            accept_local_workspace_command(&mut connection, &completed_command).unwrap();
        let completed_retry =
            accept_local_workspace_command(&mut connection, &completed_command).unwrap();
        assert_eq!(completed, completed_retry);
        assert_eq!(
            completed
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        assert!(matches!(
            &completed[0].payload,
            WorkspaceEventPayload::MessagePosted { .. }
        ));
        assert!(matches!(
            &completed[1].payload,
            WorkspaceEventPayload::ShapingProposalUpdated { .. }
        ));
        assert!(matches!(
            &completed[2].payload,
            WorkspaceEventPayload::ShapingRecordsUpdated { .. }
        ));

        let mut requirement = records.requirements[0].clone();
        let requirement_id = requirement.id;
        requirement.summary = "Support metered and seat billing".into();
        requirement.updated_at_unix_ms = 30;
        let edit_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id: work.id,
            actor_id,
            device_id: None,
            correlation_id: first_command.correlation_id,
            timestamp_unix_ms: 30,
            expected_revision: Some(5),
            payload: WorkspaceCommandPayload::UpsertRequirement { requirement },
        };
        let edited = accept_local_workspace_command(&mut connection, &edit_command).unwrap();
        assert_eq!(edited.len(), 1);
        assert_eq!(edited[0].sequence, 6);
        let WorkspaceEventPayload::RequirementUpdated { requirement } = &edited[0].payload else {
            panic!("requirement edit emitted the wrong event")
        };
        assert_eq!(requirement.id, requirement_id);
        assert_eq!(requirement.summary, "Support metered and seat billing");
        assert_eq!(
            accept_local_workspace_command(&mut connection, &edit_command).unwrap(),
            edited
        );

        let mut brief = dragent_workspace_model::WorkBrief::from_shaping_proposal(
            work.id, &proposal, actor_id, 31,
        );
        brief.outcome = "Ship configurable organization billing".into();
        let brief_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: project.id,
            work_item_id: work.id,
            actor_id,
            device_id: None,
            correlation_id: first_command.correlation_id,
            timestamp_unix_ms: 31,
            expected_revision: Some(6),
            payload: WorkspaceCommandPayload::UpsertWorkBrief {
                work_brief: brief.clone(),
            },
        };
        let brief_events = accept_local_workspace_command(&mut connection, &brief_command).unwrap();
        assert_eq!(brief_events.len(), 1);
        assert_eq!(brief_events[0].sequence, 7);
        assert_eq!(
            brief_events[0].payload,
            WorkspaceEventPayload::WorkBriefUpdated { work_brief: brief }
        );
        assert_eq!(
            accept_local_workspace_command(&mut connection, &brief_command).unwrap(),
            brief_events
        );

        let mut stale_edit = edit_command.clone();
        stale_edit.command_id = WorkspaceCommandId::new();
        stale_edit.expected_revision = Some(5);
        assert!(accept_local_workspace_command(&mut connection, &stale_edit).is_err());

        let mut collision = first_command.clone();
        let WorkspaceCommandPayload::PostMessage { message, .. } = &mut collision.payload else {
            unreachable!()
        };
        message.body = "Different content".into();
        assert!(accept_local_post_message(&mut connection, &collision).is_err());
    }
}
