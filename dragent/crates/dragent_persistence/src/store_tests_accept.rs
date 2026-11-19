#[cfg(test)]
mod tests {

    use crate::store_agent::validate_execution_start_isolation;
    use crate::workspace_store::*;
    use crate::WorkspaceIntegrationIntent;
    use crate::WorkspaceToolOutcome;
    use diesel::Connection;
    use dragent_session_protocol::workspace::WorkspaceCommandEnvelope;
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandId, WorkspaceCommandPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_session_protocol::workspace::{
        WorkspaceEventEnvelope, WorkspaceEventId, WorkspaceEventPayload,
    };
    use dragent_workspace_model::ExecutionAttempt;
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
    fn overlapping_writers_require_distinct_verified_worktree_isolations() {
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let role_id = RoleDefinitionId::new();
        let contract = |id, agent_instance_id, scope: &str| WorkContract {
            id,
            work_item_id,
            role_definition_id: role_id,
            version: 1,
            agent_instance_id: Some(agent_instance_id),
            status: WorkContractStatus::Active,
            mission: "Modify an isolated project scope".into(),
            task_ids: vec![PlanTaskId::new()],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Reviewed changes".into()],
            success_criteria: vec![AcceptanceCriterionId::new()],
            write_scope: if scope.is_empty() {
                Vec::new()
            } else {
                vec![scope.into()]
            },
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
            preferred_model_route: Some("local/test".into()),
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let active_contract_id = WorkContractId::new();
        let candidate_contract_id = WorkContractId::new();
        let active_agent_id = AgentInstanceId::new();
        let candidate_agent_id = AgentInstanceId::new();
        let active_contract = contract(active_contract_id, active_agent_id, "app/src");
        let candidate_contract = contract(
            candidate_contract_id,
            candidate_agent_id,
            "app/src/workspace",
        );
        let active_attempt_id = ExecutionAttemptId::new();
        let active_isolation = ExecutionIsolation::git_worktree(
            active_attempt_id,
            "0123456789abcdef0123456789abcdef01234567".into(),
        );
        let active_attempt = ExecutionAttempt {
            id: active_attempt_id,
            work_item_id,
            task_id: active_contract.task_ids[0],
            agent_instance_id: active_agent_id,
            contract_id: active_contract_id,
            lease_id: ExecutionLeaseId::new(),
            attempt_number: 1,
            status: ExecutionAttemptStatus::Running,
            model_route: "local/test".into(),
            isolation: Some(active_isolation.clone()),
            change_summary: None,
            review_resolution: None,
            started_at_unix_ms: 1,
            finished_at_unix_ms: None,
            updated_at_unix_ms: 1,
        };
        let contracts = HashMap::from([(active_contract_id, active_contract)]);
        let candidate_isolation = ExecutionIsolation::git_worktree(
            ExecutionAttemptId::new(),
            "fedcba9876543210fedcba9876543210fedcba98".into(),
        );
        let candidate_task = PlanTask {
            id: candidate_contract.task_ids[0],
            plan_id: PlanId::new(),
            title: "Implement candidate".into(),
            description: "Produce an attributable candidate checkout".into(),
            status: PlanTaskStatus::Ready,
            depends_on: Vec::new(),
            review_target_task_id: None,
            acceptance_criteria: candidate_contract.success_criteria.clone(),
            assigned_contract_id: Some(candidate_contract.id),
        };
        assert!(validate_execution_start_isolation(
            &candidate_task,
            &candidate_contract,
            &candidate_isolation,
            [&active_attempt],
            &contracts,
        )
        .is_ok());
        assert!(validate_execution_start_isolation(
            &candidate_task,
            &candidate_contract,
            &active_isolation,
            [&active_attempt],
            &contracts,
        )
        .is_err());
        assert!(validate_execution_start_isolation(
            &candidate_task,
            &candidate_contract,
            &ExecutionIsolation::shared(ProjectId::new()),
            [&active_attempt],
            &contracts,
        )
        .is_err());

        let reviewer_agent_id = AgentInstanceId::new();
        let reviewer_contract_id = WorkContractId::new();
        let reviewer_contract = contract(reviewer_contract_id, reviewer_agent_id, "");
        let reviewer_task = PlanTask {
            id: reviewer_contract.task_ids[0],
            plan_id: candidate_task.plan_id,
            title: "Review candidate".into(),
            description: "Inspect the exact unresolved candidate".into(),
            status: PlanTaskStatus::Ready,
            depends_on: vec![candidate_task.id],
            review_target_task_id: Some(candidate_task.id),
            acceptance_criteria: reviewer_contract.success_criteria.clone(),
            assigned_contract_id: Some(reviewer_contract.id),
        };
        let mut review_source = active_attempt.clone();
        review_source.task_id = candidate_task.id;
        review_source.contract_id = candidate_contract.id;
        review_source.status = ExecutionAttemptStatus::Succeeded;
        review_source.change_summary = Some(ExecutionChangeSummary {
            isolation_id: active_isolation.id,
            base_revision: active_isolation.base_revision.clone().unwrap(),
            files: Vec::new(),
            captured_at_unix_ms: 2,
        });
        let review_isolation = ExecutionIsolation::review_candidate(
            ExecutionAttemptId::new(),
            review_source.id,
            active_isolation.base_revision.clone().unwrap(),
        );
        let review_contracts = HashMap::from([(candidate_contract.id, candidate_contract)]);
        assert!(validate_execution_start_isolation(
            &reviewer_task,
            &reviewer_contract,
            &review_isolation,
            [&review_source],
            &review_contracts,
        )
        .is_ok());
        let wrong_candidate = ExecutionIsolation::review_candidate(
            ExecutionAttemptId::new(),
            ExecutionAttemptId::new(),
            active_isolation.base_revision.clone().unwrap(),
        );
        assert!(validate_execution_start_isolation(
            &reviewer_task,
            &reviewer_contract,
            &wrong_candidate,
            [&review_source],
            &review_contracts,
        )
        .is_err());
    }

    #[test]
    fn integration_intent_survives_reopen_and_has_explicit_terminal_states() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("integration-intent.sqlite3");
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let attempt_id = ExecutionAttemptId::new();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: ActorRef::principal_agent(project_id).id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 10,
            expected_revision: Some(7),
            payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                attempt_id,
                resolution: ExecutionReviewResolution::Integrated,
            },
        };
        let intent = WorkspaceIntegrationIntent {
            attempt_id,
            project_id,
            work_item_id,
            command_id: command.command_id,
            command,
            integration_plan_json: r#"{"paths":["src/lib.rs"]}"#.into(),
            state: "prepared".into(),
            claim_owner: None,
            claim_expires_at_unix_ms: None,
            created_at_unix_ms: 10,
            completed_at_unix_ms: None,
            last_error: None,
        };
        {
            let mut connection = crate::database::setup_connection(&path).unwrap();
            insert_workspace_integration_intent(&mut connection, &intent).unwrap();
            insert_workspace_integration_intent(&mut connection, &intent).unwrap();
        }
        let mut connection = crate::database::setup_connection(&path).unwrap();
        assert_eq!(
            load_pending_workspace_integration_intents(&mut connection).unwrap(),
            vec![intent.clone()]
        );
        let mut unrelated = intent.command.clone();
        unrelated.command_id = WorkspaceCommandId::new();
        unrelated.expected_revision = None;
        unrelated.payload = WorkspaceCommandPayload::CreateWorkItem {
            title: "must wait for integration".into(),
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
        };
        let fenced = accept_local_workspace_command(&mut connection, &unrelated).unwrap_err();
        assert!(fenced.to_string().contains("pending canonical completion"));
        let first_owner = "first-process";
        assert!(
            claim_workspace_integration(&mut connection, attempt_id, first_owner, 100, 50,)
                .unwrap()
                .is_some()
        );
        assert!(claim_workspace_integration(
            &mut connection,
            attempt_id,
            "competing-process",
            120,
            50,
        )
        .unwrap()
        .is_none());
        assert!(claim_workspace_integration(
            &mut connection,
            attempt_id,
            "recovery-process",
            151,
            50,
        )
        .unwrap()
        .is_some());
        assert!(
            !release_workspace_integration_claim(&mut connection, attempt_id, first_owner,)
                .unwrap()
        );
        assert!(release_workspace_integration_claim(
            &mut connection,
            attempt_id,
            "recovery-process",
        )
        .unwrap());
        drop(connection);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let contenders = ["concurrent-a", "concurrent-b"]
            .into_iter()
            .map(|owner| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut connection = crate::database::setup_connection(&path).unwrap();
                    barrier.wait();
                    (
                        owner,
                        claim_workspace_integration(&mut connection, attempt_id, owner, 200, 50)
                            .unwrap()
                            .is_some(),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = contenders
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|(_, claimed)| *claimed).count(), 1);
        let winner = results
            .iter()
            .find_map(|(owner, claimed)| claimed.then_some(*owner))
            .unwrap();
        let mut connection = crate::database::setup_connection(&path).unwrap();
        assert!(release_workspace_integration_claim(&mut connection, attempt_id, winner).unwrap());
        let rollback: anyhow::Result<()> = connection.transaction(|connection| {
            mark_workspace_integration_completed(connection, attempt_id, intent.command_id, 20)?;
            anyhow::bail!("fault after canonical completion work")
        });
        assert!(rollback.unwrap_err().to_string().contains("fault after"));
        assert_eq!(
            load_pending_workspace_integration_intents(&mut connection).unwrap(),
            vec![intent.clone()]
        );
        let mut collision = intent.clone();
        collision.integration_plan_json = "different".into();
        assert!(insert_workspace_integration_intent(&mut connection, &collision).is_err());
        assert!(block_workspace_integration(
            &mut connection,
            attempt_id,
            "target paths are ambiguous",
        )
        .unwrap());
        let blocked = load_workspace_integration_intent(&mut connection, attempt_id)
            .unwrap()
            .unwrap();
        assert_eq!(blocked.state, "blocked");
        assert_eq!(
            blocked.last_error.as_deref(),
            Some("target paths are ambiguous")
        );
        assert_eq!(
            load_pending_workspace_integration_intents(&mut connection).unwrap(),
            vec![blocked]
        );
    }

    #[test]
    fn integrated_completion_without_matching_intent_fails_closed() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("missing-integration-intent.sqlite3");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project_id = ProjectId::new();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id: WorkItemId::new(),
            actor_id: ActorRef::principal_agent(project_id).id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 10,
            expected_revision: Some(0),
            payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                attempt_id: ExecutionAttemptId::new(),
                resolution: ExecutionReviewResolution::Integrated,
            },
        };
        let error = accept_local_workspace_command(&mut connection, &command).unwrap_err();
        assert!(error.to_string().contains("durable Git integration intent"));
        assert!(load_canonical_workspace_events(
            &mut connection,
            command.project_id,
            command.work_item_id,
            0,
            10,
        )
        .unwrap()
        .is_empty());
        let intent = WorkspaceIntegrationIntent {
            attempt_id: match &command.payload {
                WorkspaceCommandPayload::ResolveAgentExecutionReview { attempt_id, .. } => {
                    *attempt_id
                }
                _ => unreachable!(),
            },
            project_id: command.project_id,
            work_item_id: command.work_item_id,
            command_id: command.command_id,
            command,
            integration_plan_json: "prepared-plan".into(),
            state: "prepared".into(),
            claim_owner: None,
            claim_expires_at_unix_ms: None,
            created_at_unix_ms: 10,
            completed_at_unix_ms: None,
            last_error: None,
        };
        assert!(begin_workspace_integration(&mut connection, &intent).is_err());
        assert!(load_pending_workspace_integration_intents(&mut connection)
            .unwrap()
            .is_empty());

        let mut stale = intent;
        stale.command.expected_revision = Some(7);
        stale.command_id = stale.command.command_id;
        let error = begin_workspace_integration(&mut connection, &stale).unwrap_err();
        assert!(error
            .to_string()
            .contains("revision conflict before Git integration"));
    }

    #[test]
    fn accepted_mutation_and_tool_outcome_survive_database_reopen_atomically() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("workspace-outcome.sqlite3");
        let project = Project::new(directory.path().to_path_buf(), 0);
        let project_id = project.id;
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 10,
            expected_revision: None,
            payload: WorkspaceCommandPayload::CreateWorkItem {
                title: "Durable acknowledgment".into(),
                execution_location: dragent_workspace_model::ExecutionLocation::Local,
            },
        };
        let outcome = WorkspaceToolOutcome {
            command_id: command.command_id,
            project_id,
            work_item_id,
            runtime_run_id: "local-workspace-test".into(),
            runtime_execution_id: "execution-a".into(),
            runtime_round: 1,
            provider_call_id: "call-a".into(),
            output: Some("Committed".into()),
            is_error: Some(false),
            source_message_id: None,
            accepted_at_unix_ms: 10,
            delivered_at_unix_ms: None,
            terminal_reason: None,
        };
        {
            let mut connection = crate::database::setup_connection(&path).unwrap();
            save_workspace_project(&mut connection, &project).unwrap();
            let events = accept_local_workspace_commands_with_tool_outcome(
                &mut connection,
                std::slice::from_ref(&command),
                Some(&outcome),
            )
            .unwrap();
            assert_eq!(events.len(), 1);
        }

        let mut connection = crate::database::setup_connection(&path).unwrap();
        let pending = load_pending_workspace_tool_outcomes(&mut connection).unwrap();
        assert_eq!(pending, vec![outcome.clone()]);
        let retried = accept_local_workspace_commands_with_tool_outcome(
            &mut connection,
            std::slice::from_ref(&command),
            Some(&outcome),
        )
        .unwrap();
        assert_eq!(retried.len(), 1);
        assert_eq!(
            load_pending_workspace_tool_outcomes(&mut connection)
                .unwrap()
                .len(),
            1
        );
        assert!(mark_workspace_tool_outcome_delivered(
            &mut connection,
            command.command_id,
            "execution-a",
            "call-a",
            20,
        )
        .unwrap());
        assert!(load_pending_workspace_tool_outcomes(&mut connection)
            .unwrap()
            .is_empty());
        assert!(!mark_workspace_tool_outcome_delivered(
            &mut connection,
            command.command_id,
            "execution-a",
            "call-a",
            21,
        )
        .unwrap());

        let cancelled_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 30,
            expected_revision: Some(1),
            payload: WorkspaceCommandPayload::TransitionWork {
                status: WorkStatus::Understanding,
            },
        };
        let cancelled_outcome = WorkspaceToolOutcome {
            command_id: cancelled_command.command_id,
            provider_call_id: "call-cancelled".into(),
            accepted_at_unix_ms: 30,
            ..outcome
        };
        accept_local_workspace_commands_with_tool_outcome(
            &mut connection,
            &[cancelled_command],
            Some(&cancelled_outcome),
        )
        .unwrap();
        assert_eq!(
            park_workspace_tool_outcomes_for_run(
                &mut connection,
                &cancelled_outcome.runtime_run_id,
                "execution-a",
                "run was cancelled before delivery",
            )
            .unwrap(),
            1
        );
        assert!(load_pending_workspace_tool_outcomes(&mut connection)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn principal_reply_resolves_waiting_tool_outcome_in_same_transaction() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("workspace-question-outcome.sqlite3");
        let mut connection = crate::database::setup_connection(&path).unwrap();
        let project = Project::new(directory.path().to_path_buf(), 0);
        let project_id = project.id;
        let work = WorkItem::new(project_id, "Question recovery", 0);
        let work_item_id = work.id;
        let worker = ActorRef::local_user(project_id, None).id;
        let principal = ActorRef::principal_agent(project_id).id;
        save_workspace_project(&mut connection, &project).unwrap();
        save_workspace_work_item(&mut connection, &work).unwrap();
        let message_id = AgentMessageId::new();
        let thread_id = ThreadId::new();
        let question = AgentMessage {
            id: message_id,
            thread_id,
            work_item_id,
            sender_id: worker,
            kind: AgentMessageKind::DecisionRequest,
            body: "Which API?".into(),
            review_finding: None,
            reply_to: None,
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::LocalOnly,
            created_at_unix_ms: 10,
            updated_at_unix_ms: 10,
        };
        let question_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: worker,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 10,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message: question,
                routing: Some(AgentMessageRouting {
                    recipient_ids: vec![principal],
                    task_id: None,
                    execution_attempt_id: None,
                }),
            },
        };
        let outcome = WorkspaceToolOutcome {
            command_id: question_command.command_id,
            project_id,
            work_item_id,
            runtime_run_id: "local-worker".into(),
            runtime_execution_id: "execution-question".into(),
            runtime_round: 2,
            provider_call_id: "ask-call".into(),
            output: None,
            is_error: None,
            source_message_id: Some(message_id.0),
            accepted_at_unix_ms: 10,
            delivered_at_unix_ms: None,
            terminal_reason: None,
        };
        accept_local_workspace_commands_with_tool_outcome(
            &mut connection,
            std::slice::from_ref(&question_command),
            Some(&outcome),
        )
        .unwrap();
        let waiting = load_pending_workspace_tool_outcomes(&mut connection).unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].output, None);

        let reply = AgentMessage {
            id: AgentMessageId::new(),
            thread_id,
            work_item_id,
            sender_id: principal,
            kind: AgentMessageKind::Discussion,
            body: "Use the v2 API.".into(),
            review_finding: None,
            reply_to: Some(message_id),
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::LocalOnly,
            created_at_unix_ms: 20,
            updated_at_unix_ms: 20,
        };
        let reply_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: principal,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: 20,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message: reply,
                routing: None,
            },
        };
        // Neither escalation nor an unrelated sender can release the worker.
        for (sender, body) in [
            (principal, "Escalated to the user for a decision."),
            (worker, "Unapproved answer"),
        ] {
            let mut intermediate = reply_command.clone();
            intermediate.command_id = WorkspaceCommandId::new();
            intermediate.actor_id = sender;
            if let WorkspaceCommandPayload::PostMessage { message, .. } = &mut intermediate.payload
            {
                message.id = AgentMessageId::new();
                message.sender_id = sender;
                message.body = body.into();
            }
            accept_local_workspace_commands(&mut connection, &[intermediate]).unwrap();
            assert_eq!(
                load_pending_workspace_tool_outcomes(&mut connection).unwrap()[0].output,
                None
            );
        }
        // Even a Principal reply must match the outcome's project/work scope.
        let mut wrong_scope = reply_command.clone();
        wrong_scope.work_item_id = WorkItemId::new();
        if let WorkspaceCommandPayload::PostMessage { message, .. } = &mut wrong_scope.payload {
            message.work_item_id = wrong_scope.work_item_id;
        }
        resolve_workspace_tool_outcomes_from_command(&mut connection, &wrong_scope).unwrap();
        assert_eq!(
            load_pending_workspace_tool_outcomes(&mut connection).unwrap()[0].output,
            None
        );
        accept_local_workspace_commands(&mut connection, &[reply_command]).unwrap();
        let pending = load_pending_workspace_tool_outcomes(&mut connection).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].output.as_deref(), Some("Use the v2 API."));
        assert_eq!(pending[0].is_error, Some(false));
    }
}
