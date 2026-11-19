//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.
//!
//! Hub: submodule declarations + re-exports. Domain content lives in the
//! `agentic_*` submodules.

pub use super::agentic_actor::*;
pub use super::agentic_attention::*;
pub use super::agentic_comms::*;
pub use super::agentic_contract::*;
pub use super::agentic_execution::*;
pub use super::agentic_plan::*;
pub use super::agentic_shaping::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AcceptanceCriterionId, ActorId, AgentInstanceId, AgentMessageId, AttentionItemId,
        DocumentRef, EvidenceId, ExecutionAttemptId, ExecutionLeaseId, PlanId, PlanTaskId,
        RoleDefinitionId, ShapingProposal, TeamId, ThreadId, WorkBriefId, WorkContractId,
        WorkItemId,
    };
    use crate::{ProjectId, WorkItem, WorkStatus};
    use std::path::PathBuf;

    fn task(plan_id: PlanId, title: &str, depends_on: Vec<PlanTaskId>) -> PlanTask {
        PlanTask {
            id: PlanTaskId::new(),
            plan_id,
            title: title.to_string(),
            description: String::new(),
            status: PlanTaskStatus::Proposed,
            depends_on,
            review_target_task_id: None,
            acceptance_criteria: Vec::new(),
            assigned_contract_id: None,
        }
    }

    #[test]
    fn local_principal_conversation_identity_is_stable_across_reconstruction() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let first_user = ActorRef::local_user(project_id, None);
        let second_user = ActorRef::local_user(project_id, None);
        let principal = ActorRef::principal_agent(project_id);
        let first_thread =
            CommunicationThread::principal(work_item_id, first_user.id, principal.id, 10);
        let second_thread =
            CommunicationThread::principal(work_item_id, second_user.id, principal.id, 20);

        assert_eq!(first_user.id, second_user.id);
        assert_ne!(first_user.id, principal.id);
        assert_eq!(first_thread.id, second_thread.id);
        assert_eq!(first_thread.work_item_id, work_item_id);
        assert_eq!(
            first_thread.participant_ids,
            vec![first_user.id, principal.id]
        );
    }

    #[test]
    fn shaping_records_have_stable_identity_and_principal_message_provenance() {
        let work_item_id = WorkItemId::new();
        let thread_id = ThreadId::new();
        let message_id = AgentMessageId::new();
        let principal_id = ActorId::new();
        let proposal = ShapingProposal {
            revision: 3,
            outcome: "Coordinate a release".into(),
            requirements: vec!["  Preserve   deployment evidence ".into()],
            open_decisions: vec!["Who approves production?".into()],
            acceptance_criteria: vec!["Review must pass".into()],
            plan_summary: "Assign implementation and review agents".into(),
        };

        let first = ShapingRecords::from_proposal(
            work_item_id,
            &proposal,
            thread_id,
            message_id,
            principal_id,
            10,
        );
        let mut normalized_proposal = proposal.clone();
        normalized_proposal.requirements = vec!["preserve deployment evidence".into()];
        let second = ShapingRecords::from_proposal(
            work_item_id,
            &normalized_proposal,
            thread_id,
            message_id,
            principal_id,
            20,
        );

        assert_eq!(first.requirements[0].id, second.requirements[0].id);
        assert_ne!(
            first.requirements[0].created_at_unix_ms,
            second.requirements[0].created_at_unix_ms
        );
        assert_eq!(first.proposal_revision, proposal.revision);
        assert_eq!(first.validate(), Ok(()));
        for provenance in first
            .requirements
            .iter()
            .flat_map(|record| &record.provenance)
            .chain(first.decisions.iter().flat_map(|record| &record.provenance))
            .chain(
                first
                    .acceptance_criteria
                    .iter()
                    .flat_map(|record| &record.provenance),
            )
        {
            assert_eq!(provenance.recorded_by, principal_id);
            assert!(matches!(
                provenance.source,
                ProvenanceSource::AgentMessage {
                    thread_id: source_thread_id,
                    message_id: source_message_id,
                } if source_thread_id == thread_id && source_message_id == message_id
            ));
        }
    }

    #[test]
    fn shaping_supersession_preserves_unchanged_record_identity_and_history() {
        let work_item_id = WorkItemId::new();
        let thread_id = ThreadId::new();
        let principal_id = ActorId::new();
        let first_message = AgentMessageId::new();
        let second_message = AgentMessageId::new();
        let first_proposal = ShapingProposal {
            revision: 1,
            outcome: "Build workspace shaping".into(),
            requirements: vec!["Preserve canonical identity".into()],
            open_decisions: vec!["Choose a rollout boundary".into()],
            acceptance_criteria: vec!["Restart recovery succeeds".into()],
            plan_summary: "Build the durable path".into(),
        };
        let mut first = ShapingRecords::from_proposal(
            work_item_id,
            &first_proposal,
            thread_id,
            first_message,
            principal_id,
            10,
        );
        first.requirements[0].details = "Manually clarified detail".into();
        let second_proposal = ShapingProposal {
            revision: 2,
            outcome: first_proposal.outcome.clone(),
            requirements: vec![" preserve   canonical identity ".into()],
            open_decisions: first_proposal.open_decisions.clone(),
            acceptance_criteria: first_proposal.acceptance_criteria.clone(),
            plan_summary: "Revise the durable path".into(),
        };
        let second = ShapingRecords::supersede_from_proposal(
            Some(&first),
            work_item_id,
            &second_proposal,
            thread_id,
            second_message,
            principal_id,
            20,
        );

        assert_eq!(second.requirements[0].id, first.requirements[0].id);
        assert_eq!(second.requirements[0].created_at_unix_ms, 10);
        assert_eq!(second.requirements[0].details, "Manually clarified detail");
        assert_eq!(second.requirements[0].provenance.len(), 2);
        assert!(second.requirements[0]
            .provenance
            .iter()
            .any(|entry| matches!(
                entry.source,
                ProvenanceSource::AgentMessage { message_id, .. } if message_id == second_message
            )));
    }

    #[test]
    fn shaping_records_retain_grounded_repository_document_provenance() {
        let work_item_id = WorkItemId::new();
        let principal_id = ActorId::new();
        let proposal = ShapingProposal {
            revision: 1,
            outcome: "Improve shaping".into(),
            requirements: vec!["Use repository context".into()],
            open_decisions: vec!["Which index?".into()],
            acceptance_criteria: vec!["Sources are attributable".into()],
            plan_summary: "Reuse the context builder".into(),
        };
        let mut records = ShapingRecords::from_proposal(
            work_item_id,
            &proposal,
            ThreadId::new(),
            AgentMessageId::new(),
            principal_id,
            10,
        );
        let document =
            DocumentRef::from_relative_path(PathBuf::from("app/src/workspace/view.rs")).unwrap();

        records.add_document_provenance(&[document.clone()], principal_id, 20);
        records.add_document_provenance(&[document.clone()], principal_id, 20);

        for provenance in records
            .requirements
            .iter()
            .flat_map(|record| &record.provenance)
            .chain(
                records
                    .decisions
                    .iter()
                    .flat_map(|record| &record.provenance),
            )
            .chain(
                records
                    .acceptance_criteria
                    .iter()
                    .flat_map(|record| &record.provenance),
            )
        {
            if let ProvenanceSource::Document {
                document_id,
                relative_path,
            } = &provenance.source
            {
                assert_eq!(*document_id, document.id);
                assert_eq!(*relative_path, document.relative_path);
            }
        }
        assert_eq!(records.requirements[0].provenance.len(), 2);
        assert_eq!(records.decisions[0].provenance.len(), 2);
        assert_eq!(records.acceptance_criteria[0].provenance.len(), 2);
        assert_eq!(records.validate(), Ok(()));
    }

    #[test]
    fn shaping_records_reject_missing_provenance_and_cross_work_items() {
        let work_item_id = WorkItemId::new();
        let proposal = ShapingProposal {
            revision: 1,
            outcome: "Outcome".into(),
            requirements: vec!["Requirement".into()],
            open_decisions: Vec::new(),
            acceptance_criteria: Vec::new(),
            plan_summary: "Plan".into(),
        };
        let mut records = ShapingRecords::from_proposal(
            work_item_id,
            &proposal,
            ThreadId::new(),
            AgentMessageId::new(),
            ActorId::new(),
            1,
        );
        records.requirements[0].provenance.clear();
        assert_eq!(
            records.validate(),
            Err(DomainValidationError::MissingProvenance("requirement"))
        );

        records.requirements[0].provenance.push(Provenance {
            source: ProvenanceSource::AgentFinding {
                actor_id: ActorId::new(),
                summary: "Recovered".into(),
            },
            recorded_by: ActorId::new(),
            recorded_at_unix_ms: 2,
        });
        records.requirements[0].work_item_id = WorkItemId::new();
        assert_eq!(
            records.validate(),
            Err(DomainValidationError::DifferentWorkItem("shaping record"))
        );
    }

    #[test]
    fn lifecycle_rejects_skipping_from_draft_to_execution() {
        assert!(WorkStatus::Draft.can_transition_to(WorkStatus::Understanding));
        assert!(!WorkStatus::Draft.can_transition_to(WorkStatus::Executing));
        assert!(WorkStatus::Completed.is_terminal());

        let mut work = WorkItem::new(ProjectId::new(), "Agentic workspace", 10);
        assert!(work.transition_to(WorkStatus::Executing, 20).is_err());
        assert_eq!(work.status, WorkStatus::Draft);
        assert_eq!(work.updated_at_unix_ms, 10);
        work.transition_to(WorkStatus::Understanding, 30).unwrap();
        assert_eq!(work.status, WorkStatus::Understanding);
        assert_eq!(work.updated_at_unix_ms, 30);
    }

    #[test]
    fn shaped_records_require_meaningful_content_and_versions() {
        let mut brief = WorkBrief {
            id: WorkBriefId::new(),
            work_item_id: WorkItemId::new(),
            version: 0,
            outcome: "Build the agentic workspace".into(),
            summary: String::new(),
            status: StructuredRecordStatus::Draft,
            created_by: ActorId::new(),
            updated_by: ActorId::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert_eq!(
            brief.validate(),
            Err(DomainValidationError::ZeroVersion("work brief"))
        );
        brief.version = 1;
        brief.outcome = "  ".into();
        assert_eq!(
            brief.validate(),
            Err(DomainValidationError::EmptyField("work brief outcome"))
        );
    }

    #[test]
    fn a_valid_plan_dag_is_accepted_regardless_of_vector_order() {
        let plan_id = PlanId::new();
        let first = task(plan_id, "Define contract", Vec::new());
        let second = task(plan_id, "Build projection", vec![first.id]);
        let plan = Plan {
            id: plan_id,
            work_item_id: WorkItemId::new(),
            version: 1,
            status: PlanStatus::Draft,
            tasks: vec![second, first],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert_eq!(plan.validate(), Ok(()));
    }

    #[test]
    fn plan_rejects_missing_and_cyclic_dependencies() {
        let plan_id = PlanId::new();
        let missing = task(plan_id, "Missing", vec![PlanTaskId::new()]);
        let mut plan = Plan {
            id: plan_id,
            work_item_id: WorkItemId::new(),
            version: 1,
            status: PlanStatus::Draft,
            tasks: vec![missing],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert!(matches!(
            plan.validate(),
            Err(DomainValidationError::MissingPlanDependency { .. })
        ));

        let mut first = task(plan_id, "One", Vec::new());
        let second = task(plan_id, "Two", vec![first.id]);
        first.depends_on.push(second.id);
        plan.tasks = vec![first, second];
        assert!(matches!(
            plan.validate(),
            Err(DomainValidationError::CyclicPlanDependency { .. })
        ));
    }

    #[test]
    fn approved_plan_requires_attributable_approval() {
        let plan = Plan {
            id: PlanId::new(),
            work_item_id: WorkItemId::new(),
            version: 1,
            status: PlanStatus::Approved,
            tasks: Vec::new(),
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert_eq!(
            plan.validate(),
            Err(DomainValidationError::ApprovalAttributionRequired)
        );
    }

    #[test]
    fn satisfied_criteria_require_evidence_and_waivers_require_attribution() {
        let mut criterion = AcceptanceCriterion {
            id: AcceptanceCriterionId::new(),
            work_item_id: WorkItemId::new(),
            description: "State survives restart".into(),
            status: AcceptanceStatus::Satisfied,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: Vec::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert_eq!(
            criterion.validate(),
            Err(DomainValidationError::EvidenceRequired)
        );

        criterion.status = AcceptanceStatus::Waived;
        assert_eq!(
            criterion.validate(),
            Err(DomainValidationError::WaiverAttributionRequired)
        );
        criterion.waiver_reason = Some("Accepted for prototype".into());
        let actor_id = ActorId::new();
        criterion.waived_by = Some(actor_id);
        criterion.provenance.push(Provenance {
            source: ProvenanceSource::AgentFinding {
                actor_id,
                summary: "Prototype review".into(),
            },
            recorded_by: actor_id,
            recorded_at_unix_ms: 2,
        });
        assert_eq!(criterion.validate(), Ok(()));
    }

    #[test]
    fn evidence_requires_identity_attribution_source_and_time() {
        let mut evidence = Evidence {
            id: EvidenceId::new(),
            work_item_id: WorkItemId::new(),
            task_id: Some(PlanTaskId::new()),
            kind: EvidenceKind::Test,
            summary: "Focused tests passed".into(),
            source: "cargo test -p workspace".into(),
            produced_by: ActorId::new(),
            created_at_unix_ms: 10,
        };
        assert_eq!(evidence.validate(), Ok(()));

        evidence.source.clear();
        assert_eq!(
            evidence.validate(),
            Err(DomainValidationError::EmptyField("evidence source"))
        );
        evidence.source = "cargo test -p workspace".into();
        evidence.created_at_unix_ms = 0;
        assert_eq!(
            evidence.validate(),
            Err(DomainValidationError::InvalidEvidence)
        );
    }

    #[test]
    fn draft_contract_does_not_require_or_create_a_runtime_agent() {
        let contract = WorkContract {
            id: WorkContractId::new(),
            work_item_id: WorkItemId::new(),
            role_definition_id: RoleDefinitionId::new(),
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Draft,
            mission: "Review the proposed architecture".into(),
            task_ids: Vec::new(),
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Review report".into()],
            success_criteria: vec![AcceptanceCriterionId::new()],
            write_scope: vec![PathBuf::from("docs/product")],
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Cloud,
            preferred_model_route: None,
            created_by: ActorId::new(),
            updated_by: ActorId::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert_eq!(contract.validate(), Ok(()));
        let mut invalid = contract;
        invalid.agent_instance_id = Some(AgentInstanceId::new());
        assert_eq!(
            invalid.validate(),
            Err(DomainValidationError::RuntimeAgentForbidden)
        );
    }

    #[test]
    fn pending_agent_is_durable_identity_not_started_execution() {
        let mut agent = AgentInstance {
            id: AgentInstanceId::new(),
            work_item_id: WorkItemId::new(),
            role_definition_id: RoleDefinitionId::new(),
            contract_id: WorkContractId::new(),
            status: AgentInstanceStatus::Pending,
            execution_location: ExecutionLocation::Local,
            model_route: "auto".into(),
            active_lease_id: None,
            started_at_unix_ms: None,
            updated_at_unix_ms: 1,
        };
        assert_eq!(agent.validate(), Ok(()));

        agent.started_at_unix_ms = Some(1);
        assert_eq!(
            agent.validate(),
            Err(DomainValidationError::AgentStartTimestampForbidden)
        );
        agent.started_at_unix_ms = None;
        agent.status = AgentInstanceStatus::Running;
        assert_eq!(
            agent.validate(),
            Err(DomainValidationError::ExecutionLeaseRequired)
        );
        agent.active_lease_id = Some(ExecutionLeaseId::new());
        assert_eq!(
            agent.validate(),
            Err(DomainValidationError::AgentStartTimestampRequired)
        );
    }

    #[test]
    fn execution_lease_requires_explicit_non_ambiguous_charging_ownership() {
        let owner_id = ActorId::new();
        let mut charging = ChargingAuthorization {
            owner_id,
            mode: ChargingMode::PlatformCredits,
            billing_operation_id: None,
        };
        // Platform pay-as-you-go needs no preflight hold: the provider
        // enforces balance per call.
        assert_eq!(charging.validate(), Ok(()));
        charging.mode = ChargingMode::LocalCompute;
        assert_eq!(charging.validate(), Ok(()));

        let lease = ExecutionLease {
            id: ExecutionLeaseId::new(),
            work_item_id: WorkItemId::new(),
            agent_instance_id: AgentInstanceId::new(),
            contract_id: WorkContractId::new(),
            status: ExecutionLeaseStatus::Active,
            charging,
            acquired_at_unix_ms: 10,
            expires_at_unix_ms: 20,
            updated_at_unix_ms: 10,
        };
        assert_eq!(lease.validate(), Ok(()));
    }

    #[test]
    fn running_attempt_has_stable_identity_and_no_terminal_timestamp() {
        let mut attempt = ExecutionAttempt {
            id: ExecutionAttemptId::new(),
            work_item_id: WorkItemId::new(),
            task_id: PlanTaskId::new(),
            agent_instance_id: AgentInstanceId::new(),
            contract_id: WorkContractId::new(),
            lease_id: ExecutionLeaseId::new(),
            attempt_number: 1,
            status: ExecutionAttemptStatus::Running,
            model_route: "local/default-agent".into(),
            isolation: Some(ExecutionIsolation::shared(ProjectId::new())),
            change_summary: None,
            review_resolution: None,
            started_at_unix_ms: 10,
            finished_at_unix_ms: None,
            updated_at_unix_ms: 10,
        };
        assert_eq!(attempt.validate(), Ok(()));
        attempt.finished_at_unix_ms = Some(11);
        assert_eq!(
            attempt.validate(),
            Err(DomainValidationError::ExecutionAttemptFinishForbidden)
        );
        attempt.status = ExecutionAttemptStatus::Succeeded;
        assert_eq!(attempt.validate(), Ok(()));
        attempt.review_resolution = Some(ExecutionReviewResolution::Accepted);
        assert_eq!(attempt.validate(), Ok(()));
        attempt.review_resolution = Some(ExecutionReviewResolution::RevisionRequested);
        assert_eq!(attempt.validate(), Ok(()));
        attempt.review_resolution = Some(ExecutionReviewResolution::Integrated);
        assert_eq!(
            attempt.validate(),
            Err(DomainValidationError::InvalidExecutionReviewResolution)
        );
        let isolation = ExecutionIsolation::git_worktree(
            attempt.id,
            "0123456789abcdef0123456789abcdef01234567".into(),
        );
        attempt.change_summary = Some(ExecutionChangeSummary {
            isolation_id: isolation.id,
            base_revision: isolation.base_revision.clone().unwrap(),
            files: Vec::new(),
            captured_at_unix_ms: 11,
        });
        attempt.isolation = Some(isolation);
        assert_eq!(attempt.validate(), Ok(()));
    }

    #[test]
    fn proposed_team_requires_distinct_reviewable_roles() {
        let work_item_id = WorkItemId::new();
        let role_definition_id = RoleDefinitionId::new();
        let work_contract_id = WorkContractId::new();
        let actor_id = ActorId::new();
        let role = ProposedTeamRole {
            role_definition_id,
            work_contract_id,
            rationale: "Independent security review is required".into(),
        };
        let mut team = ProposedTeam {
            id: TeamId::new(),
            work_item_id,
            version: 1,
            roles: vec![role.clone()],
            status: StructuredRecordStatus::Draft,
            created_by: actor_id,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert_eq!(team.validate(), Ok(()));

        team.roles.push(role);
        assert_eq!(
            team.validate(),
            Err(DomainValidationError::DuplicateIdentity(
                "proposed team role"
            ))
        );
        team.roles.clear();
        assert_eq!(
            team.validate(),
            Err(DomainValidationError::ProposedTeamRequired)
        );
    }

    #[test]
    fn work_contract_rejects_unsafe_write_scope() {
        let mut contract = WorkContract {
            id: WorkContractId::new(),
            work_item_id: WorkItemId::new(),
            role_definition_id: RoleDefinitionId::new(),
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Draft,
            mission: "Implement a bounded change".into(),
            task_ids: Vec::new(),
            input_artifact_ids: Vec::new(),
            expected_outputs: Vec::new(),
            success_criteria: vec![AcceptanceCriterionId::new()],
            write_scope: vec![PathBuf::from("../outside")],
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: ActorId::new(),
            updated_by: ActorId::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        assert!(matches!(
            contract.validate(),
            Err(DomainValidationError::InvalidWriteScope { .. })
        ));
        contract.write_scope = vec![PathBuf::from("app/src")];
        contract.permissions = vec![PermissionGrant {
            permission: "project_write".into(),
            scope: Some("outside".into()),
            requires_approval: true,
        }];
        assert_eq!(
            contract.validate(),
            Err(DomainValidationError::InvalidPermissionGrant)
        );
        contract.permissions = vec![PermissionGrant {
            permission: "shell_root".into(),
            scope: None,
            requires_approval: true,
        }];
        assert_eq!(
            contract.validate(),
            Err(DomainValidationError::InvalidPermissionGrant)
        );
        contract.permissions = vec![
            PermissionGrant {
                permission: "project_read".into(),
                scope: None,
                requires_approval: false,
            },
            PermissionGrant {
                permission: "project_write".into(),
                scope: Some("app/src".into()),
                requires_approval: true,
            },
        ];
        contract.preferred_model_route = Some("anthropic/claude-review".into());
        assert_eq!(contract.validate(), Ok(()));
        contract.preferred_model_route = Some(agent_route("codex"));
        assert_eq!(contract.validate(), Ok(()));
        contract.preferred_model_route = Some("anthropic/claude-review".into());
        contract.permissions.push(PermissionGrant {
            permission: "project_write".into(),
            scope: Some("app/src".into()),
            requires_approval: false,
        });
        assert_eq!(
            contract.validate(),
            Err(DomainValidationError::InvalidPermissionGrant)
        );
        contract.permissions.pop();
        contract.preferred_model_route = Some("auto".into());
        assert_eq!(
            contract.validate(),
            Err(DomainValidationError::InvalidModelRoute)
        );
    }

    #[test]
    fn permission_attention_is_bound_to_an_execution_and_resolves_terminally() {
        let mut attention = AttentionItem {
            id: AttentionItemId::new(),
            work_item_id: WorkItemId::new(),
            task_id: Some(PlanTaskId::new()),
            thread_id: None,
            execution_attempt_id: Some(ExecutionAttemptId::new()),
            tool_call_id: Some("tool-call-1".into()),
            kind: AttentionKind::Permission,
            status: AttentionStatus::Open,
            summary: "Allow the command to run".into(),
            requested_by: ActorId::new(),
            assigned_to: vec![ActorId::new()],
            created_at_unix_ms: 10,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        assert_eq!(attention.validate(), Ok(()));

        attention.execution_attempt_id = None;
        assert_eq!(
            attention.validate(),
            Err(DomainValidationError::AttentionExecutionRequired)
        );
        attention.execution_attempt_id = Some(ExecutionAttemptId::new());
        attention.resolution = Some("approved".into());
        assert_eq!(
            attention.validate(),
            Err(DomainValidationError::ResolutionForbidden)
        );
        attention.status = AttentionStatus::Resolved;
        attention.resolved_at_unix_ms = Some(11);
        attention.resolution_kind = Some(AttentionResolution::Approved);
        assert_eq!(attention.validate(), Ok(()));
    }

    #[test]
    fn unchanged_route_keeps_approval_but_any_change_invalidates_it() {
        // Auto is a server-resolved intent, not a stored model: re-saving the
        // same value (including Auto) must not bounce the plan to Draft.
        assert!(!contract_route_change_invalidates_approval(None, None));
        assert!(!contract_route_change_invalidates_approval(
            Some("pro-a"),
            Some("pro-a")
        ));
        assert!(contract_route_change_invalidates_approval(
            None,
            Some("pro-a")
        ));
        assert!(contract_route_change_invalidates_approval(
            Some("pro-a"),
            None
        ));
        assert!(contract_route_change_invalidates_approval(
            Some("pro-a"),
            Some("pro-b")
        ));
    }

    #[test]
    fn coordination_routes_auto_always_locked_free_never_pins() {
        let task = ("Backend", None);
        // Auto passes for every tier, even with an empty catalog.
        assert!(
            validate_task_model_routes_for_coordination(&[task], &[], true, Some("free-tier"))
                .is_ok()
        );
        assert!(validate_task_model_routes_for_coordination(
            &[task],
            &["pro-a".to_string()],
            false,
            Some("free-tier")
        )
        .is_ok());
        // The Principal may pin the free-tier route itself while fully aware
        // of the account's models; locked users cannot unselect it, so it is
        // Auto-equivalent and passes the gate.
        let free_pinned = ("Backend", Some("free-tier"));
        assert!(validate_task_model_routes_for_coordination(
            &[free_pinned],
            &[],
            true,
            Some("free-tier")
        )
        .is_ok());
        // Any other concrete pin on a locked account still blocks.
        let pinned = ("Backend", Some("pro-a"));
        assert!(validate_task_model_routes_for_coordination(
            &[pinned],
            &["pro-a".to_string()],
            true,
            Some("free-tier")
        )
        .is_err());
        // Paid accounts must pin catalog routes when the catalog is known.
        assert!(validate_task_model_routes_for_coordination(
            &[pinned],
            &["pro-a".to_string()],
            false,
            Some("free-tier")
        )
        .is_ok());
        let off_catalog = ("Backend", Some("ultra-z"));
        assert!(validate_task_model_routes_for_coordination(
            &[off_catalog],
            &["pro-a".to_string()],
            false,
            Some("free-tier")
        )
        .is_err());
        // Empty catalog (server list unavailable): stored routes pass here;
        // the execution resolver still fails loudly on unbillable routes.
        assert!(validate_task_model_routes_for_coordination(
            &[off_catalog],
            &[],
            false,
            Some("free-tier")
        )
        .is_ok());
    }

    #[test]
    fn agent_routes_pass_on_any_tier_while_the_agent_is_installed() {
        assert_eq!(agent_route("claude"), "agent:claude");
        assert_eq!(route_agent("agent:codex"), Some("codex"));
        assert_eq!(route_agent("agent:"), None);
        assert_eq!(route_agent("anthropic/claude-sonnet"), None);
        let task = ("Backend", Some("agent:claude"));
        let installed = vec![agent_route("claude")];
        for locked in [true, false] {
            assert!(validate_task_model_routes_for_coordination(
                &[task],
                &installed,
                locked,
                Some("free-tier")
            )
            .is_ok());
            let missing = validate_task_model_routes_for_coordination(
                &[("Backend", Some("agent:codex"))],
                &installed,
                locked,
                Some("free-tier"),
            )
            .unwrap_err();
            assert!(missing.contains("isn't installed"), "{missing}");
        }
    }
}
