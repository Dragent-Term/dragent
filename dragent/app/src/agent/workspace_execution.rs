//! Durable workspace-to-agent execution boundary.
//!
//! The workspace authority commits an `AgentExecutionStarted` event before a
//! provider or tool loop is launched. This module turns that committed fact
//! into an immutable launch request and turns the runner's terminal result
//! back into a revision-guarded workspace command. It deliberately owns no UI
//! state and never starts work while canonical history is merely being replayed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventId, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    evaluate_scheduler, execution_retry_delay_ms, task_coordination_thread_id, ActorId, ActorKind,
    ActorRef, AgentInstance, AgentInstanceId, AgentInstanceStatus, AgentMessageKind, AttentionItem,
    AttentionItemId, AttentionKind, AttentionResolution, AttentionStatus, ChargingAuthorization,
    CoordinationHealth, DeviceId, ExecutionAttempt, ExecutionAttemptId, ExecutionAttemptStatus,
    ExecutionChangeSummary, ExecutionIsolation, ExecutionLease, ExecutionLeaseId,
    ExecutionReviewResolution, PlanTask, PlanTaskId, PlanTaskStatus, ProjectId, ReviewFindingItem,
    ReviewVerdict, StructuredReviewFinding, WorkContract, WorkContractId, WorkContractStatus,
    WorkItemId, WorkStatus, WorkspaceState, MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
};
use thiserror::Error;

/// Re-exported area modules; existing `workspace_execution::…` paths keep
/// working. Canonical homes are the execution_* modules.
pub use super::execution_launch::*;
pub use super::execution_pipeline::*;
pub use super::execution_review::*;
pub use super::execution_runtime::*;

#[cfg(test)]
#[cfg(test)]
pub(crate) mod execution_test_helpers {
    use super::*;
    use dragent_workspace_model::*;
    use uuid::Uuid;

    /// Minimal compact-history builder for controller tests: contracts, plan,
    /// then any extra canonical events (finishes, attention) in sequence.
    pub(crate) fn billing(
        locked_to_free: bool,
        has_premium_funds: bool,
        active_model_ids: &[&str],
    ) -> LaunchBilling {
        LaunchBilling {
            account_known: true,
            locked_to_free,
            has_premium_funds,
            active_model_ids: active_model_ids.iter().map(|id| id.to_string()).collect(),
        }
    }

    pub(crate) fn fixture() -> (WorkspaceEventEnvelope, WorkContract) {
        let project_id = ProjectId(id("11111111-1111-4111-8111-111111111111"));
        let work_item_id = WorkItemId(id("22222222-2222-4222-8222-222222222222"));
        let actor_id = ActorId(id("33333333-3333-4333-8333-333333333333"));
        let device_id = DeviceId(id("44444444-4444-4444-8444-444444444444"));
        let correlation_id = CorrelationId(id("55555555-5555-4555-8555-555555555555"));
        let task_id = PlanTaskId(id("66666666-6666-4666-8666-666666666666"));
        let contract_id = WorkContractId(id("77777777-7777-4777-8777-777777777777"));
        let agent_id = AgentInstanceId(id("88888888-8888-4888-8888-888888888888"));
        let lease_id = ExecutionLeaseId(id("99999999-9999-4999-8999-999999999999"));
        let attempt_id = ExecutionAttemptId(id("aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa"));
        let role_id = RoleDefinitionId::new();
        let criterion_id = AcceptanceCriterionId::new();
        let timestamp = 1_788_796_800_004;
        let task = PlanTask {
            id: task_id,
            plan_id: PlanId::new(),
            title: "Implement billing".into(),
            description: "Implement and verify the approved billing slice.".into(),
            status: PlanTaskStatus::Running,
            depends_on: Vec::new(),
            review_target_task_id: None,
            acceptance_criteria: vec![criterion_id],
            assigned_contract_id: Some(contract_id),
        };
        let plan = Plan {
            id: task.plan_id,
            work_item_id,
            version: 1,
            status: PlanStatus::Approved,
            tasks: vec![task],
            approved_by: Some(actor_id),
            approved_at_unix_ms: Some(timestamp - 10),
            created_at_unix_ms: timestamp - 20,
            updated_at_unix_ms: timestamp - 10,
        };
        let charging = ChargingAuthorization {
            owner_id: actor_id,
            mode: ChargingMode::LocalCompute,
            billing_operation_id: None,
        };
        let lease = ExecutionLease {
            id: lease_id,
            work_item_id,
            agent_instance_id: agent_id,
            contract_id,
            status: ExecutionLeaseStatus::Active,
            charging,
            acquired_at_unix_ms: timestamp - 1,
            expires_at_unix_ms: timestamp + 60_000,
            updated_at_unix_ms: timestamp,
        };
        let agent_instance = AgentInstance {
            id: agent_id,
            work_item_id,
            role_definition_id: role_id,
            contract_id,
            status: AgentInstanceStatus::Running,
            execution_location: ExecutionLocation::Local,
            model_route: "auto".into(),
            active_lease_id: Some(lease_id),
            started_at_unix_ms: Some(timestamp),
            updated_at_unix_ms: timestamp,
        };
        let attempt = ExecutionAttempt {
            id: attempt_id,
            work_item_id,
            task_id,
            agent_instance_id: agent_id,
            contract_id,
            lease_id,
            attempt_number: 1,
            status: ExecutionAttemptStatus::Running,
            model_route: "auto".into(),
            isolation: Some(ExecutionIsolation::git_worktree(
                attempt_id,
                "0123456789abcdef0123456789abcdef01234567".into(),
            )),
            change_summary: None,
            review_resolution: None,
            started_at_unix_ms: timestamp,
            finished_at_unix_ms: None,
            updated_at_unix_ms: timestamp,
        };
        let contract = WorkContract {
            id: contract_id,
            work_item_id,
            role_definition_id: role_id,
            version: 2,
            agent_instance_id: Some(agent_id),
            status: WorkContractStatus::Active,
            mission: "Implement the assigned billing task.".into(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Verified implementation".into()],
            success_criteria: vec![criterion_id],
            write_scope: vec!["app".into()],
            capabilities: Vec::new(),
            tools: vec!["read_file".into(), "edit_file".into()],
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: timestamp - 20,
            updated_at_unix_ms: timestamp - 1,
        };
        let event = WorkspaceEventEnvelope {
            event_id: WorkspaceEventId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            sequence: 17,
            project_id,
            work_item_id,
            actor_id,
            device_id: Some(device_id),
            task_id: Some(task_id),
            correlation_id,
            causation_id: Some(WorkspaceCommandId::new()),
            timestamp_unix_ms: timestamp,
            payload: WorkspaceEventPayload::AgentExecutionStarted {
                agent_instance,
                lease,
                attempt,
                plan,
            },
        };
        (event, contract)
    }

    pub(crate) fn id(value: &str) -> Uuid {
        Uuid::parse_str(value).unwrap()
    }

    pub(crate) fn controller_state(
        work_status: WorkStatus,
        work_updated_at: i64,
        plan: Plan,
        contracts: Vec<WorkContract>,
        extra: Vec<WorkspaceEventKind>,
    ) -> WorkspaceState {
        let (started, _) = fixture();
        let project = Project {
            organisation_id: "local".into(),
            id: started.project_id,
            root: PathBuf::from("/workspace/controller"),
            display_name: "controller".into(),
            created_at_unix_ms: work_updated_at - 100,
            updated_at_unix_ms: work_updated_at,
        };
        let work = WorkItem {
            id: started.work_item_id,
            project_id: started.project_id,
            title: "Controller".into(),
            status: work_status,
            created_at_unix_ms: work_updated_at - 100,
            updated_at_unix_ms: work_updated_at,
        };
        let mut events = Vec::new();
        let mut sequence = 0_u64;
        for contract in contracts {
            sequence += 1;
            events.push(WorkspaceEvent {
                sequence,
                timestamp_unix_ms: work_updated_at,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind: WorkspaceEventKind::WorkContractUpdated {
                    work_contract: contract,
                },
            });
        }
        sequence += 1;
        events.push(WorkspaceEvent {
            sequence,
            timestamp_unix_ms: work_updated_at,
            project_id: started.project_id,
            work_item_id: Some(started.work_item_id),
            kind: WorkspaceEventKind::StructuredPlanUpdated { plan },
        });
        for kind in extra {
            sequence += 1;
            events.push(WorkspaceEvent {
                sequence,
                timestamp_unix_ms: work_updated_at,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind,
            });
        }
        WorkspaceState::restore(project, Some(work), events)
    }

    pub(crate) fn controller_contract(
        work_item_id: WorkItemId,
        task_id: PlanTaskId,
        agent_id: Option<AgentInstanceId>,
    ) -> WorkContract {
        let (_, template) = fixture();
        WorkContract {
            work_item_id,
            task_ids: vec![task_id],
            agent_instance_id: agent_id,
            ..template
        }
    }
    pub(crate) fn pipeline_session(contract: WorkContract) -> WorkspacePipelineSession {
        let (event, _) = fixture();
        let mut adapter = WorkspaceExecutionAdapter::default();
        let launch = adapter.claim_started(&event, &contract).unwrap().unwrap();
        let request = WorkspacePipelineRequest::new(launch, "/workspace/billing", "", "").unwrap();
        let (sender, receiver) = mpsc::channel();
        WorkspacePipelineSession {
            request,
            sender,
            receiver,
            runtime_task_id: None,
            accepted_tool_calls: HashSet::new(),
            parked_tool_calls: HashMap::new(),
            checkpoints: crate::agent::checkpoint_store::CheckpointStore::default(),
            checkpoint_message_id: 0,
            response_text: String::new(),
            terminal: false,
        }
    }

    pub(crate) fn stream_event(
        kind: crate::agent::pipeline::AgentEventKind,
        text: &str,
    ) -> crate::agent::pipeline::PipelineEvent {
        let mut event = tool_call("read_file", "{}", false);
        event.kind = kind;
        event.text = Some(text.into());
        event.tool_id = None;
        event.tool_name = None;
        event.tool_args = None;
        event
    }
    pub(crate) fn tool_call(
        name: &str,
        args: &str,
        requires_approval: bool,
    ) -> crate::agent::pipeline::PipelineEvent {
        crate::agent::pipeline::PipelineEvent {
            kind: crate::agent::pipeline::AgentEventKind::ToolCall,
            text: None,
            tool_id: Some("call-1".into()),
            tool_name: Some(name.into()),
            tool_icon: None,
            args_summary: None,
            tool_args: Some(args.into()),
            requires_approval: Some(requires_approval),
            server_executed: Some(false),
            result_summary: None,
            duration_ms: None,
            tool_output: None,
            tool_is_error: None,
            assistant_content: None,
            compacted_messages: None,
            total_input_tokens: None,
            total_cached_input_tokens: None,
            total_output_tokens: None,
            rounds: None,
            context_tokens: None,
            error: None,
        }
    }
    pub(crate) fn launch_state(
        task_status: PlanTaskStatus,
        runtime: Option<(AgentInstance, WorkContract, Option<ExecutionLease>)>,
        preferred_model_route: Option<&str>,
    ) -> (
        WorkspaceState,
        WorkspaceLaunchPolicy,
        PlanTaskId,
        WorkContractId,
    ) {
        let (started, mut contract) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted { mut plan, .. } = started.payload.clone()
        else {
            unreachable!()
        };
        let task_id = plan.tasks[0].id;
        let contract_id = contract.id;
        plan.tasks[0].status = task_status;
        plan.version = 3;
        contract.status = WorkContractStatus::Approved;
        contract.agent_instance_id = None;
        contract.version = 1;
        contract.preferred_model_route = preferred_model_route.map(str::to_string);
        let timestamp = started.timestamp_unix_ms;
        let project = Project {
            organisation_id: "local".into(),
            id: started.project_id,
            root: PathBuf::from("/workspace/billing"),
            display_name: "billing".into(),
            created_at_unix_ms: timestamp - 100,
            updated_at_unix_ms: timestamp,
        };
        let work = WorkItem {
            id: started.work_item_id,
            project_id: started.project_id,
            title: "Billing".into(),
            status: WorkStatus::ReadyToExecute,
            created_at_unix_ms: timestamp - 100,
            updated_at_unix_ms: timestamp,
        };
        let mut events = vec![
            WorkspaceEvent {
                sequence: 1,
                timestamp_unix_ms: timestamp,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind: WorkspaceEventKind::WorkContractUpdated {
                    work_contract: contract.clone(),
                },
            },
            WorkspaceEvent {
                sequence: 2,
                timestamp_unix_ms: timestamp,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind: WorkspaceEventKind::StructuredPlanUpdated { plan },
            },
        ];
        if let Some((agent_instance, runtime_contract, lease)) = runtime {
            events.push(WorkspaceEvent {
                sequence: 3,
                timestamp_unix_ms: timestamp,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind: if let Some(lease) = lease {
                    WorkspaceEventKind::AgentInstanceActivated {
                        agent_instance,
                        work_contract: runtime_contract,
                        lease,
                    }
                } else {
                    WorkspaceEventKind::AgentInstanceProvisioned {
                        agent_instance,
                        work_contract: runtime_contract,
                    }
                },
            });
        }
        let policy = WorkspaceLaunchPolicy {
            effective_model_route: "ollama/qwen3-coder".into(),
            charging: ChargingAuthorization {
                owner_id: started.actor_id,
                mode: ChargingMode::LocalCompute,
                billing_operation_id: None,
            },
            maximum_parallel_tasks: 4,
            lease_duration_ms: 10 * 60 * 1_000,
        };
        (
            WorkspaceState::restore(project, Some(work), events),
            policy,
            task_id,
            contract_id,
        )
    }
    pub(crate) fn terminal_launch_state(
        outcome: ExecutionAttemptStatus,
        task_status: PlanTaskStatus,
        attempt_number: u32,
    ) -> (
        WorkspaceState,
        WorkspaceLaunchPolicy,
        ExecutionAttemptId,
        AgentInstanceId,
        i64,
    ) {
        let (started, contract) = fixture();
        let WorkspaceEventPayload::AgentExecutionStarted {
            mut agent_instance,
            mut lease,
            mut attempt,
            mut plan,
        } = started.payload
        else {
            unreachable!()
        };
        let finished_at = started.timestamp_unix_ms + 1_000;
        plan.tasks[0].status = task_status;
        plan.version += 1;
        attempt.status = outcome;
        attempt.attempt_number = attempt_number;
        attempt.finished_at_unix_ms = Some(finished_at);
        attempt.updated_at_unix_ms = finished_at;
        match outcome {
            ExecutionAttemptStatus::Failed => {
                agent_instance.status = AgentInstanceStatus::Failed;
                agent_instance.active_lease_id = None;
                lease.status = ExecutionLeaseStatus::Released;
            }
            ExecutionAttemptStatus::TimedOut => {
                agent_instance.status = AgentInstanceStatus::Failed;
                agent_instance.active_lease_id = None;
                lease.status = ExecutionLeaseStatus::Expired;
            }
            ExecutionAttemptStatus::Cancelled => {
                agent_instance.status = AgentInstanceStatus::Cancelled;
                agent_instance.active_lease_id = None;
                lease.status = ExecutionLeaseStatus::Cancelled;
            }
            ExecutionAttemptStatus::Paused => {
                agent_instance.status = AgentInstanceStatus::Ready;
            }
            ExecutionAttemptStatus::Succeeded | ExecutionAttemptStatus::Running => {
                unreachable!()
            }
        }
        agent_instance.updated_at_unix_ms = finished_at;
        lease.updated_at_unix_ms = finished_at;
        let project = Project {
            organisation_id: "local".into(),
            id: started.project_id,
            root: PathBuf::from("/workspace/billing"),
            display_name: "billing".into(),
            created_at_unix_ms: finished_at - 100,
            updated_at_unix_ms: finished_at,
        };
        let work = WorkItem {
            id: started.work_item_id,
            project_id: started.project_id,
            title: "Billing".into(),
            status: if outcome == ExecutionAttemptStatus::Paused {
                WorkStatus::Paused
            } else {
                WorkStatus::Executing
            },
            created_at_unix_ms: finished_at - 100,
            updated_at_unix_ms: finished_at,
        };
        let events = vec![
            WorkspaceEvent {
                sequence: 1,
                timestamp_unix_ms: finished_at - 1,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind: WorkspaceEventKind::WorkContractUpdated {
                    work_contract: contract,
                },
            },
            WorkspaceEvent {
                sequence: 2,
                timestamp_unix_ms: finished_at,
                project_id: started.project_id,
                work_item_id: Some(started.work_item_id),
                kind: WorkspaceEventKind::AgentExecutionFinished {
                    agent_instance: agent_instance.clone(),
                    lease,
                    attempt: attempt.clone(),
                    plan,
                    summary: "terminal result".into(),
                },
            },
        ];
        let policy = WorkspaceLaunchPolicy {
            effective_model_route: "ollama/qwen3-coder".into(),
            charging: ChargingAuthorization {
                owner_id: started.actor_id,
                mode: ChargingMode::LocalCompute,
                billing_operation_id: None,
            },
            maximum_parallel_tasks: 4,
            lease_duration_ms: 10 * 60 * 1_000,
        };
        (
            WorkspaceState::restore(project, Some(work), events),
            policy,
            attempt.id,
            agent_instance.id,
            finished_at,
        )
    }
}
