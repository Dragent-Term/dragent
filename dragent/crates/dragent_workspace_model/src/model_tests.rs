#[cfg(test)]
mod tests {
    use crate::*;
    use std::path::PathBuf;

    #[test]
    fn project_name_is_derived_without_loading_project_files() {
        let project = Project::new(PathBuf::from("/tmp/example-app"), 10);
        assert_eq!(project.display_name, "example-app");
    }

    #[test]
    fn event_log_is_bounded_and_keeps_sequence_order() {
        let project = ProjectId::new();
        let mut log = WorkspaceEventLog::new(2);
        log.append(
            project,
            None,
            1,
            WorkspaceEventKind::PlanUpdated {
                summary: "one".into(),
            },
        );
        log.append(
            project,
            None,
            2,
            WorkspaceEventKind::PlanUpdated {
                summary: "two".into(),
            },
        );
        let evicted = log.append(
            project,
            None,
            3,
            WorkspaceEventKind::PlanUpdated {
                summary: "three".into(),
            },
        );
        assert_eq!(log.len(), 2);
        assert_eq!(evicted.unwrap().sequence, 0);
        assert_eq!(log.iter().next().unwrap().sequence, 1);
        assert_eq!(log.iter().next_back().unwrap().sequence, 2);
    }

    #[test]
    fn zero_capacity_does_not_retain_events() {
        let mut log = WorkspaceEventLog::new(0);
        let evicted = log.append(
            ProjectId::new(),
            None,
            1,
            WorkspaceEventKind::FileChanged {
                relative_path: PathBuf::from("src/main.rs"),
            },
        );
        assert_eq!(log.len(), 0);
        assert_eq!(evicted.unwrap().sequence, 0);
    }

    #[test]
    fn coordinated_statuses_cover_ready_through_paused() {
        for status in [
            WorkStatus::ReadyToExecute,
            WorkStatus::Executing,
            WorkStatus::Reviewing,
            WorkStatus::Verifying,
            WorkStatus::NeedsUserApproval,
            WorkStatus::Completed,
            WorkStatus::Blocked,
            WorkStatus::Paused,
        ] {
            assert!(
                status.has_started_coordination(),
                "{status:?} is coordinated"
            );
        }
        for status in [
            WorkStatus::Draft,
            WorkStatus::Understanding,
            WorkStatus::WaitingForDecision,
            WorkStatus::ReadyToPlan,
            WorkStatus::Planning,
            WorkStatus::Cancelled,
        ] {
            assert!(
                !status.has_started_coordination(),
                "{status:?} is not coordinated"
            );
        }
    }

    #[test]
    fn active_work_transition_is_validated_and_projected_as_an_event() {
        let project = Project::new(PathBuf::from("/tmp/agent-work"), 1);
        let work_item_id = WorkItemId::new();
        let mut state = WorkspaceState::new(project);
        state.start_work_with_id(work_item_id, "Organization billing", 2);

        assert!(state
            .transition_active_work(WorkStatus::Understanding, 3)
            .unwrap()
            .is_ok());
        assert!(state
            .transition_active_work(WorkStatus::Planning, 4)
            .unwrap()
            .is_ok());
        assert!(state
            .transition_active_work(WorkStatus::ReadyToExecute, 5)
            .unwrap()
            .is_ok());
        assert_eq!(
            state.active_work().map(|work| work.status),
            Some(WorkStatus::ReadyToExecute)
        );
        assert_eq!(
            state.events().back().map(|event| &event.kind),
            Some(&WorkspaceEventKind::WorkStatusChanged {
                status: WorkStatus::ReadyToExecute,
            })
        );
    }

    #[test]
    fn restore_filters_foreign_events_and_continues_sequence() {
        let project = Project::new(PathBuf::from("/tmp/restored-work"), 1);
        let work = WorkItem::new(project.id, "Persisted work", 2);
        let foreign_project = ProjectId::new();
        let state = WorkspaceState::restore(
            project.clone(),
            Some(work.clone()),
            [
                WorkspaceEvent {
                    sequence: 7,
                    timestamp_unix_ms: 7,
                    project_id: project.id,
                    work_item_id: Some(work.id),
                    kind: WorkspaceEventKind::UserMessagePosted {
                        body: "Keep the first release seat-based".into(),
                    },
                },
                WorkspaceEvent {
                    sequence: 99,
                    timestamp_unix_ms: 8,
                    project_id: foreign_project,
                    work_item_id: Some(work.id),
                    kind: WorkspaceEventKind::PlanUpdated {
                        summary: "foreign".into(),
                    },
                },
            ],
        );

        assert_eq!(state.active_work().map(|item| item.id), Some(work.id));
        assert_eq!(state.events().len(), 1);
        assert_eq!(state.events().back().map(|event| event.sequence), Some(7));
        assert_eq!(
            state.events().back().map(|event| &event.kind),
            Some(&WorkspaceEventKind::UserMessagePosted {
                body: "Keep the first release seat-based".into(),
            })
        );
    }

    #[test]
    fn latest_shaping_proposal_is_reduced_from_the_event_tail() {
        let project = Project::new(PathBuf::from("/tmp/shaped-work"), 1);
        let mut state = WorkspaceState::new(project);
        state.start_work("Billing", 2);
        for revision in 1..=2 {
            state.record(
                2 + revision as i64,
                WorkspaceEventKind::ShapingProposalUpdated {
                    proposal: ShapingProposal {
                        revision,
                        outcome: format!("Outcome {revision}"),
                        requirements: vec!["Manage seats".into()],
                        open_decisions: vec!["Proration policy?".into()],
                        acceptance_criteria: vec!["Admin can change seats".into()],
                        plan_summary: "Implement behind a feature flag".into(),
                    },
                },
            );
        }

        let proposal = state.latest_shaping_proposal().unwrap();
        assert_eq!(proposal.revision, 2);
        assert_eq!(proposal.outcome, "Outcome 2");
        assert!(proposal.validate().is_ok());

        let work_item_id = state.active_work().unwrap().id;
        let mut brief = WorkBrief::from_shaping_proposal(work_item_id, proposal, ActorId::new(), 5);
        let brief_id = brief.id;
        brief.outcome = "Editable outcome".into();
        state.record(
            5,
            WorkspaceEventKind::WorkBriefUpdated { work_brief: brief },
        );
        assert_eq!(state.latest_work_brief().unwrap().id, brief_id);
        assert_eq!(
            state.latest_work_brief().unwrap().outcome,
            "Editable outcome"
        );

        let proposal = state.latest_shaping_proposal().unwrap();
        let records = ShapingRecords::from_proposal(
            work_item_id,
            proposal,
            ThreadId::new(),
            AgentMessageId::new(),
            ActorId::new(),
            5,
        );
        let requirement_id = records.requirements[0].id;
        state.record(5, WorkspaceEventKind::ShapingRecordsUpdated { records });
        let records = state.latest_shaping_records().unwrap();
        assert_eq!(records.proposal_revision, 2);
        assert_eq!(records.requirements[0].id, requirement_id);

        let mut requirement = records.requirements[0].clone();
        requirement.summary = "Manage seats and usage".into();
        requirement.updated_at_unix_ms = 6;
        state.record(6, WorkspaceEventKind::RequirementUpdated { requirement });
        let projection = state.shaping_records_projection().unwrap();
        assert_eq!(projection.requirements[0].id, requirement_id);
        assert_eq!(projection.requirements[0].summary, "Manage seats and usage");
    }

    #[test]
    fn coordination_requires_every_proposed_contract_to_be_approved() {
        let project = Project::new(PathBuf::from("/tmp/coordination-readiness"), 1);
        let project_id = project.id;
        let mut state = WorkspaceState::new(project);
        let work_item_id = state.start_work("Reviewable work", 2).id;
        state.record(
            3,
            WorkspaceEventKind::ShapingProposalUpdated {
                proposal: ShapingProposal {
                    revision: 1,
                    outcome: "Ship reviewed work".into(),
                    requirements: vec!["Keep authority explicit".into()],
                    open_decisions: Vec::new(),
                    acceptance_criteria: vec!["Every role is approved".into()],
                    plan_summary: "Coordinate reviewed contracts".into(),
                },
            },
        );
        let role_definition_id = RoleDefinitionId::new();
        let contract_id = WorkContractId::new();
        let plan_id = PlanId::new();
        let task_id = PlanTaskId::new();
        let criterion_id = AcceptanceCriterionId::new();
        let actor_id = ActorId::new();
        state.record(
            4,
            WorkspaceEventKind::RoleDefinitionUpdated {
                role_definition: RoleDefinition {
                    id: role_definition_id,
                    scope: RoleDefinitionScope::Project(project_id),
                    kind: AgentRoleKind::Reviewer,
                    name: "Reviewer".into(),
                    mission: "Review the work".into(),
                    capabilities: Vec::new(),
                    preferred_tools: Vec::new(),
                    created_by: actor_id,
                    created_at_unix_ms: 4,
                    updated_at_unix_ms: 4,
                },
            },
        );
        let mut contract = WorkContract {
            id: contract_id,
            work_item_id,
            role_definition_id,
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Proposed,
            mission: "Review the work".into(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Findings".into()],
            success_criteria: vec![criterion_id],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 5,
            updated_at_unix_ms: 5,
        };
        state.record(
            5,
            WorkspaceEventKind::WorkContractUpdated {
                work_contract: contract.clone(),
            },
        );
        state.record(
            6,
            WorkspaceEventKind::ProposedTeamUpdated {
                proposed_team: ProposedTeam {
                    id: TeamId::new(),
                    work_item_id,
                    version: 1,
                    roles: vec![ProposedTeamRole {
                        role_definition_id,
                        work_contract_id: contract_id,
                        rationale: "Independent review".into(),
                    }],
                    status: StructuredRecordStatus::Draft,
                    created_by: actor_id,
                    created_at_unix_ms: 6,
                    updated_at_unix_ms: 6,
                },
            },
        );
        let mut plan = Plan {
            id: plan_id,
            work_item_id,
            version: 1,
            status: PlanStatus::Draft,
            tasks: vec![PlanTask {
                id: task_id,
                plan_id,
                title: "Review implementation".into(),
                description: "Produce evidence-backed findings".into(),
                status: PlanTaskStatus::Proposed,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: vec![criterion_id],
                assigned_contract_id: Some(contract_id),
            }],
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 6,
            updated_at_unix_ms: 6,
        };
        state.record(
            6,
            WorkspaceEventKind::StructuredPlanUpdated { plan: plan.clone() },
        );

        let proposed = state.coordination_readiness();
        assert_eq!(proposed.role_count, 1);
        assert_eq!(proposed.approved_role_count, 0);
        assert!(!proposed.is_approvable());
        assert!(!proposed.is_ready());

        contract.status = WorkContractStatus::Approved;
        contract.version = 2;
        contract.updated_at_unix_ms = 7;
        state.record(
            7,
            WorkspaceEventKind::WorkContractUpdated {
                work_contract: contract.clone(),
            },
        );
        let approved = state.coordination_readiness();
        assert_eq!(approved.approved_role_count, 1);
        assert!(approved.is_approvable());
        assert!(!approved.is_ready());

        // Active and Completed necessarily passed approval (provisioning
        // requires Approved), so they satisfy the gate; Revoked does not.
        for status in [WorkContractStatus::Active, WorkContractStatus::Completed] {
            contract.status = status;
            contract.version += 1;
            contract.updated_at_unix_ms += 1;
            state.record(
                contract.updated_at_unix_ms,
                WorkspaceEventKind::WorkContractUpdated {
                    work_contract: contract.clone(),
                },
            );
            assert_eq!(state.coordination_readiness().approved_role_count, 1);
        }
        contract.status = WorkContractStatus::Revoked;
        contract.version += 1;
        contract.updated_at_unix_ms += 1;
        state.record(
            contract.updated_at_unix_ms,
            WorkspaceEventKind::WorkContractUpdated {
                work_contract: contract.clone(),
            },
        );
        let revoked = state.coordination_readiness();
        assert_eq!(revoked.approved_role_count, 0);
        assert!(!revoked.is_approvable());

        contract.status = WorkContractStatus::Approved;
        contract.version += 1;
        contract.updated_at_unix_ms += 1;
        state.record(
            contract.updated_at_unix_ms,
            WorkspaceEventKind::WorkContractUpdated {
                work_contract: contract,
            },
        );
        assert_eq!(state.coordination_readiness().approved_role_count, 1);

        plan.version = 2;
        plan.status = PlanStatus::Approved;
        plan.approved_by = Some(actor_id);
        plan.approved_at_unix_ms = Some(8);
        plan.updated_at_unix_ms = 8;
        state.record(8, WorkspaceEventKind::StructuredPlanUpdated { plan });
        assert!(state.coordination_readiness().is_ready());
    }

    #[test]
    fn inline_decision_resolution_clears_the_coordination_gate() {
        let project = Project::new(PathBuf::from("/tmp/decision-resolution"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = state.start_work("Decidable work", 2).id;
        state.record(
            3,
            WorkspaceEventKind::ShapingProposalUpdated {
                proposal: ShapingProposal {
                    revision: 1,
                    outcome: "Ship decided work".into(),
                    requirements: Vec::new(),
                    open_decisions: vec!["Postgres or SQLite?".into()],
                    acceptance_criteria: Vec::new(),
                    plan_summary: "Decide then coordinate".into(),
                },
            },
        );
        let proposal = state.latest_shaping_proposal().unwrap().clone();
        let records = ShapingRecords::from_proposal(
            work_item_id,
            &proposal,
            ThreadId::new(),
            AgentMessageId::new(),
            ActorId::new(),
            4,
        );
        state.record(4, WorkspaceEventKind::ShapingRecordsUpdated { records });
        // The proposal snapshot alone blocks coordination…
        let blocked = state.coordination_readiness();
        assert!(blocked
            .issues
            .iter()
            .any(|issue| matches!(issue, CoordinationReadinessIssue::OpenDecisionsRemain(1))));
        // …until the decision is resolved inline — no new shaping turn needed.
        let mut decision = state
            .shaping_records_projection()
            .unwrap()
            .decisions
            .into_iter()
            .next()
            .unwrap();
        decision.resolution = Some("Postgres".into());
        decision.status = DecisionStatus::Resolved;
        decision.decided_by = Some(ActorId::new());
        state.record(5, WorkspaceEventKind::DecisionUpdated { decision });
        let cleared = state.coordination_readiness();
        assert!(!cleared
            .issues
            .iter()
            .any(|issue| matches!(issue, CoordinationReadinessIssue::OpenDecisionsRemain(_))));
    }

    #[test]
    fn shaping_memory_is_append_only_bounded_and_validated() {
        use crate::{
            ShapingMemoryKind, ShapingMemoryNote, MAX_SHAPING_MEMORY_NOTES,
            MAX_SHAPING_MEMORY_TEXT_CHARS,
        };
        assert_eq!(
            ShapingMemoryKind::parse("Approval"),
            ShapingMemoryKind::Approval
        );
        assert_eq!(ShapingMemoryKind::parse("nope"), ShapingMemoryKind::Note);
        let project = Project::new(PathBuf::from("/tmp/shaping-memory"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = state.start_work("Remembered work", 2).id;
        let note = ShapingMemoryNote {
            id: ShapingMemoryNoteId::new(),
            work_item_id,
            kind: ShapingMemoryKind::Decision,
            text: "Chose Postgres".into(),
            recorded_at_unix_ms: 5,
        };
        note.validate().unwrap();
        assert!(ShapingMemoryNote {
            text: String::new(),
            ..note.clone()
        }
        .validate()
        .is_err());
        assert!(ShapingMemoryNote {
            text: "x".repeat(MAX_SHAPING_MEMORY_TEXT_CHARS + 1),
            ..note.clone()
        }
        .validate()
        .is_err());
        for index in 0..(MAX_SHAPING_MEMORY_NOTES + 5) {
            state.record(
                3 + index as i64,
                WorkspaceEventKind::ShapingMemoryRecorded {
                    note: ShapingMemoryNote {
                        id: ShapingMemoryNoteId::new(),
                        text: format!("fact {index}"),
                        ..note.clone()
                    },
                },
            );
        }
        // A repeated delivery never duplicates: without dedup the tail
        // would start at "fact 7".
        state.record(
            999,
            WorkspaceEventKind::ShapingMemoryRecorded { note: note.clone() },
        );
        state.record(
            1000,
            WorkspaceEventKind::ShapingMemoryRecorded { note: note.clone() },
        );
        let memory = state.shaping_memory();
        assert_eq!(memory.len(), MAX_SHAPING_MEMORY_NOTES);
        assert_eq!(memory.first().unwrap().text, "fact 6");
        assert_eq!(memory.last().unwrap().text, "Chose Postgres");
    }

    #[test]
    fn work_item_title_is_derived_from_the_first_meaningful_outcome_line() {
        assert_eq!(
            derive_work_item_title("\n##  Build an agent workspace  \nMore context"),
            "Build an agent workspace"
        );
        assert_eq!(derive_work_item_title("   "), "Untitled work");
        assert_eq!(derive_work_item_title(&"x".repeat(100)).chars().count(), 72);
    }

    #[test]
    fn document_index_sorts_directories_before_files_per_folder() {
        let entry = |path: &str, is_directory| DocumentEntry {
            document: DocumentRef::from_relative_path(PathBuf::from(path)).unwrap(),
            is_directory,
            byte_size: None,
            is_ignored: false,
        };
        let mut index = ProjectDocumentIndex::default();
        index.replace(vec![
            entry("README.md", false),
            entry("src/z.rs", false),
            entry("assets", true),
            entry("src/components", true),
            entry("src/a.rs", false),
            entry("src", true),
            entry("Cargo.toml", false),
        ]);

        let paths = index
            .entries()
            .iter()
            .map(|entry| entry.document.relative_path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "assets",
                "src",
                "src/components",
                "src/a.rs",
                "src/z.rs",
                "Cargo.toml",
                "README.md",
            ]
        );
    }

    #[test]
    fn document_index_merge_replaces_duplicate_paths() {
        let entry = |path: &str, byte_size| DocumentEntry {
            document: DocumentRef::from_relative_path(PathBuf::from(path)).unwrap(),
            is_directory: false,
            byte_size,
            is_ignored: false,
        };
        let mut index = ProjectDocumentIndex::default();
        index.replace(vec![entry("src/lib.rs", Some(10))]);

        index.merge(vec![
            entry("src/lib.rs", Some(20)),
            entry("src/main.rs", Some(30)),
        ]);

        assert_eq!(index.entries().len(), 2);
        assert_eq!(
            index
                .entries()
                .iter()
                .find(|entry| entry.document.relative_path == PathBuf::from("src/lib.rs"))
                .and_then(|entry| entry.byte_size),
            Some(20)
        );
    }

    #[test]
    fn attention_projection_replaces_open_item_with_its_resolution() {
        let project = Project::new(PathBuf::from("/workspace"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = WorkItemId::new();
        state.start_work_with_id(work_item_id, "Handle consent", 2);
        let mut attention = AttentionItem {
            id: AttentionItemId::new(),
            work_item_id,
            task_id: None,
            thread_id: None,
            execution_attempt_id: None,
            tool_call_id: None,
            kind: AttentionKind::Decision,
            status: AttentionStatus::Open,
            summary: "Choose a region".into(),
            requested_by: ActorId::new(),
            assigned_to: vec![ActorId::new()],
            created_at_unix_ms: 3,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        state.record(
            3,
            WorkspaceEventKind::AttentionUpdated {
                attention_item: attention.clone(),
            },
        );
        assert_eq!(state.open_attention_items(), [&attention]);

        attention.status = AttentionStatus::Resolved;
        attention.resolved_at_unix_ms = Some(4);
        attention.resolution_kind = Some(AttentionResolution::Answered);
        attention.resolution = Some("eu-west-2".into());
        state.record(
            4,
            WorkspaceEventKind::AttentionUpdated {
                attention_item: attention.clone(),
            },
        );
        assert!(state.open_attention_items().is_empty());
        assert_eq!(state.attention_items(), [&attention]);
    }

    #[test]
    fn artifact_registry_is_work_scoped_validated_and_last_write_wins() {
        let project = Project::new(PathBuf::from("/workspace"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = WorkItemId::new();
        state.start_work_with_id(work_item_id, "Use repository context", 2);
        let mut artifact = ArtifactRef {
            id: ArtifactId::new(),
            work_item_id,
            relative_path: PathBuf::from("docs/requirements.md"),
            kind: ArtifactKind::Requirements,
        };
        assert_eq!(artifact.validate(), Ok(()));
        state.record(
            3,
            WorkspaceEventKind::ArtifactUpdated {
                artifact: artifact.clone(),
            },
        );
        artifact.kind = ArtifactKind::ProductBrief;
        state.record(
            4,
            WorkspaceEventKind::ArtifactUpdated {
                artifact: artifact.clone(),
            },
        );
        assert_eq!(state.artifacts(), vec![artifact]);

        let replacement = ArtifactRef {
            id: ArtifactId::new(),
            work_item_id,
            relative_path: PathBuf::from("docs/design.md"),
            kind: ArtifactKind::Design,
        };
        state.record(
            5,
            WorkspaceEventKind::ArtifactsReplaced {
                artifacts: vec![replacement.clone()],
            },
        );
        assert_eq!(state.artifacts(), vec![replacement]);
        state.record(
            6,
            WorkspaceEventKind::ArtifactsReplaced {
                artifacts: Vec::new(),
            },
        );
        assert!(state.artifacts().is_empty());

        let unsafe_artifact = ArtifactRef {
            id: ArtifactId::new(),
            work_item_id,
            relative_path: PathBuf::from("../outside.md"),
            kind: ArtifactKind::Other,
        };
        assert!(matches!(
            unsafe_artifact.validate(),
            Err(DomainValidationError::InvalidArtifactPath { .. })
        ));
    }

    #[test]
    fn message_delivery_projection_replaces_the_posted_state() {
        let project = Project::new(PathBuf::from("/workspace"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = WorkItemId::new();
        state.start_work_with_id(work_item_id, "Coordinate agents", 2);
        let recipient_id = ActorId::new();
        let routing = AgentMessageRouting {
            recipient_ids: vec![recipient_id],
            task_id: Some(PlanTaskId::new()),
            execution_attempt_id: None,
        };
        let mut message = AgentMessage {
            id: AgentMessageId::new(),
            thread_id: ThreadId::new(),
            work_item_id,
            sender_id: ActorId::new(),
            kind: AgentMessageKind::Handoff,
            body: "Implementation is ready for review.".into(),
            review_finding: None,
            reply_to: None,
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::Pending,
            created_at_unix_ms: 3,
            updated_at_unix_ms: 3,
        };
        state.record(
            3,
            WorkspaceEventKind::AgentMessagePosted {
                message: message.clone(),
                routing: routing.clone(),
            },
        );
        message.delivery_state = MessageDeliveryState::Read;
        message.updated_at_unix_ms = 4;
        state.record(
            4,
            WorkspaceEventKind::AgentMessageDeliveryUpdated {
                message: message.clone(),
                routing,
            },
        );

        let messages = state.agent_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, &message);
        assert_eq!(messages[0].0.delivery_state, MessageDeliveryState::Read);
    }

    #[test]
    fn communication_threads_group_by_thread_and_derive_open_status() {
        let project = Project::new(PathBuf::from("/workspace"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = WorkItemId::new();
        state.start_work_with_id(work_item_id, "Coordinate agents", 2);
        let principal_id = ActorRef::principal_agent(state.project().id).id;
        let worker_id = ActorId::new();
        let task_id = PlanTaskId::new();

        // An open thread: a worker's Blocker with no Principal reply yet.
        let open_thread_id = ThreadId::new();
        let blocker_id = AgentMessageId::new();
        state.record(
            3,
            WorkspaceEventKind::AgentMessagePosted {
                message: AgentMessage {
                    id: blocker_id,
                    thread_id: open_thread_id,
                    work_item_id,
                    sender_id: worker_id,
                    kind: AgentMessageKind::Blocker,
                    body: "Need a decision on the migration strategy.".into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 3,
                    updated_at_unix_ms: 3,
                },
                routing: AgentMessageRouting {
                    recipient_ids: vec![principal_id],
                    task_id: Some(task_id),
                    execution_attempt_id: None,
                },
            },
        );

        // A resolved thread: a Blocker the Principal already replied to.
        let resolved_thread_id = ThreadId::new();
        let resolved_blocker_id = AgentMessageId::new();
        state.record(
            4,
            WorkspaceEventKind::AgentMessagePosted {
                message: AgentMessage {
                    id: resolved_blocker_id,
                    thread_id: resolved_thread_id,
                    work_item_id,
                    sender_id: worker_id,
                    kind: AgentMessageKind::Blocker,
                    body: "Which logging library should this use?".into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 4,
                    updated_at_unix_ms: 4,
                },
                routing: AgentMessageRouting {
                    recipient_ids: vec![principal_id],
                    task_id: Some(task_id),
                    execution_attempt_id: None,
                },
            },
        );
        state.record(
            5,
            WorkspaceEventKind::AgentMessagePosted {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: resolved_thread_id,
                    work_item_id,
                    sender_id: principal_id,
                    kind: AgentMessageKind::Discussion,
                    body: "Use the existing structured logger.".into(),
                    review_finding: None,
                    reply_to: Some(resolved_blocker_id),
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 5,
                    updated_at_unix_ms: 5,
                },
                routing: AgentMessageRouting {
                    recipient_ids: vec![worker_id],
                    task_id: Some(task_id),
                    execution_attempt_id: None,
                },
            },
        );

        let threads = state.communication_threads();
        assert_eq!(threads.len(), 2);
        // Most recently active first: the resolved thread's reply (t=5) is
        // newer than the open thread's only message (t=3).
        assert_eq!(threads[0].thread_id, resolved_thread_id);
        assert!(!threads[0].is_open);
        assert_eq!(threads[0].last_activity_unix_ms, 5);
        assert_eq!(threads[0].task_id, Some(task_id));
        assert_eq!(threads[0].participant_ids, vec![worker_id, principal_id]);

        assert_eq!(threads[1].thread_id, open_thread_id);
        assert!(threads[1].is_open);
        assert_eq!(threads[1].last_activity_unix_ms, 3);
        assert_eq!(threads[1].participant_ids, vec![worker_id]);
    }

    #[test]
    fn evidence_projection_keeps_canonical_event_order() {
        let project = Project::new(PathBuf::from("/workspace"), 1);
        let mut state = WorkspaceState::new(project);
        let work_item_id = WorkItemId::new();
        state.start_work_with_id(work_item_id, "Verify work", 2);
        let actor_id = ActorId::new();
        let first = Evidence {
            id: EvidenceId::new(),
            work_item_id,
            task_id: Some(PlanTaskId::new()),
            kind: EvidenceKind::Test,
            summary: "Focused tests passed".into(),
            source: "cargo test -p workspace".into(),
            produced_by: actor_id,
            created_at_unix_ms: 3,
        };
        let second = Evidence {
            id: EvidenceId::new(),
            work_item_id,
            task_id: None,
            kind: EvidenceKind::Review,
            summary: "Review accepted".into(),
            source: "review:principal".into(),
            produced_by: actor_id,
            created_at_unix_ms: 4,
        };
        state.record(
            3,
            WorkspaceEventKind::EvidenceRecorded {
                evidence: first.clone(),
            },
        );
        state.record(
            4,
            WorkspaceEventKind::EvidenceRecorded {
                evidence: second.clone(),
            },
        );

        assert_eq!(state.evidence(), vec![&first, &second]);
    }
}
