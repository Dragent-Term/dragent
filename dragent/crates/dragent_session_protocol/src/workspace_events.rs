//! Versioned wire contract for synchronized agentic-workspace state.
//! (Submodule of the `workspace` hub.)

use std::collections::HashSet;

use dragent_workspace_model::{
    AcceptanceCriterion, ActorId, AgentInstance, AgentInstanceStatus, AgentMessage, ArtifactRef,
    AttentionItem, Checkpoint, Constraint, Decision, DeviceId, Evidence, ExecutionAttempt,
    ExecutionAttemptStatus, ExecutionLease, ExecutionLeaseStatus, ExecutionLocation, Plan,
    PlanTaskId, PlanTaskStatus, ProjectId, ProposedTeam, Requirement, Risk, RoleDefinition,
    ShapingProposal, ShapingRecords, WorkBrief, WorkContract, WorkContractStatus, WorkItemId,
    WorkStatus, WorkspaceEvent, WorkspaceEventKind,
};
use serde::{Deserialize, Serialize};

use super::workspace_commands::{CorrelationId, WorkspaceCommandId, WorkspaceEventId};
use super::workspace_error::{
    require_text, validate_common, validate_message, validate_message_routing,
    WorkspaceProtocolError,
};

/// One immutable semantic fact. `sequence` is monotonic within `work_item_id`;
/// it is not a global database order and must not be synthesized by a renderer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceEventEnvelope {
    pub event_id: WorkspaceEventId,
    pub schema_version: u32,
    pub sequence: u64,
    pub project_id: ProjectId,
    pub work_item_id: WorkItemId,
    pub actor_id: ActorId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<PlanTaskId>,
    pub correlation_id: CorrelationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<WorkspaceCommandId>,
    pub timestamp_unix_ms: i64,
    pub payload: WorkspaceEventPayload,
}

impl WorkspaceEventEnvelope {
    pub fn validate(&self) -> Result<(), WorkspaceProtocolError> {
        validate_common(
            self.schema_version,
            self.project_id.0,
            self.actor_id.0,
            self.correlation_id.0,
            self.timestamp_unix_ms,
        )?;
        if self.event_id.is_nil() {
            return Err(WorkspaceProtocolError::NilIdentifier("event_id"));
        }
        if self.sequence == 0 {
            return Err(WorkspaceProtocolError::InvalidSequence);
        }
        if self.work_item_id.0.is_nil() {
            return Err(WorkspaceProtocolError::NilIdentifier("work_item_id"));
        }
        if self.device_id.is_some_and(|device_id| device_id.0.is_nil()) {
            return Err(WorkspaceProtocolError::NilIdentifier("device_id"));
        }
        if self.task_id.is_some_and(|task_id| task_id.0.is_nil()) {
            return Err(WorkspaceProtocolError::NilIdentifier("task_id"));
        }
        if self.causation_id.is_some_and(WorkspaceCommandId::is_nil) {
            return Err(WorkspaceProtocolError::NilIdentifier("causation_id"));
        }
        if self
            .payload
            .work_item_id()
            .is_some_and(|payload_work_item| payload_work_item != self.work_item_id)
        {
            return Err(WorkspaceProtocolError::PayloadWorkItemMismatch);
        }
        if let Some(payload_task_id) = self.payload.task_id() {
            if self.task_id != Some(payload_task_id) {
                return Err(WorkspaceProtocolError::PayloadTaskMismatch);
            }
        }
        if let WorkspaceEventPayload::MessagePosted { message, .. } = &self.payload {
            if message.sender_id != self.actor_id {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
        }
        if let WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            ..
        } = &self.payload
        {
            if lease.expires_at_unix_ms <= self.timestamp_unix_ms {
                return Err(WorkspaceProtocolError::InvalidPayload(
                    "cannot start execution with an expired lease".into(),
                ));
            }
            if agent_instance
                .started_at_unix_ms
                .is_none_or(|started_at| started_at > self.timestamp_unix_ms)
                || attempt.started_at_unix_ms != self.timestamp_unix_ms
            {
                return Err(WorkspaceProtocolError::InvalidPayload(
                    "execution start timestamps must match the event timestamp".into(),
                ));
            }
        }
        if let WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } = &self.payload {
            if attempt.finished_at_unix_ms != Some(self.timestamp_unix_ms) {
                return Err(WorkspaceProtocolError::InvalidPayload(
                    "execution finish timestamp must match the event timestamp".into(),
                ));
            }
        }
        self.payload.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEventPayload {
    WorkCreated {
        title: String,
        execution_location: ExecutionLocation,
    },
    WorkStatusChanged {
        previous: WorkStatus,
        current: WorkStatus,
    },
    WorkBriefUpdated {
        work_brief: WorkBrief,
    },
    RequirementUpdated {
        requirement: Requirement,
    },
    DecisionUpdated {
        decision: Decision,
    },
    ShapingMemoryRecorded {
        note: dragent_workspace_model::ShapingMemoryNote,
    },
    AcceptanceCriterionUpdated {
        acceptance_criterion: AcceptanceCriterion,
    },
    ConstraintUpdated {
        constraint: Constraint,
    },
    RiskUpdated {
        risk: Risk,
    },
    PlanUpdated {
        plan: Plan,
    },
    PlanTaskStatusChanged {
        plan: Plan,
        task_id: PlanTaskId,
        previous: PlanTaskStatus,
        current: PlanTaskStatus,
    },
    RoleDefinitionUpdated {
        role_definition: RoleDefinition,
    },
    WorkContractUpdated {
        work_contract: WorkContract,
    },
    AgentInstanceProvisioned {
        agent_instance: AgentInstance,
        work_contract: WorkContract,
    },
    AgentInstanceActivated {
        agent_instance: AgentInstance,
        work_contract: WorkContract,
        lease: ExecutionLease,
    },
    ExecutionLeaseRenewed {
        agent_instance: AgentInstance,
        lease: ExecutionLease,
    },
    AgentExecutionRetryPrepared {
        agent_instance: AgentInstance,
        lease: ExecutionLease,
        plan: Plan,
    },
    AgentExecutionStarted {
        agent_instance: AgentInstance,
        lease: ExecutionLease,
        attempt: ExecutionAttempt,
        plan: Plan,
    },
    AgentExecutionFinished {
        agent_instance: AgentInstance,
        lease: ExecutionLease,
        attempt: ExecutionAttempt,
        plan: Plan,
        summary: String,
    },
    AgentExecutionReviewResolved {
        agent_instance: AgentInstance,
        work_contract: WorkContract,
        lease: ExecutionLease,
        attempt: ExecutionAttempt,
        plan: Plan,
        resolution: dragent_workspace_model::ExecutionReviewResolution,
    },
    ProposedTeamUpdated {
        proposed_team: ProposedTeam,
    },
    MessagePosted {
        message: AgentMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        routing: Option<dragent_workspace_model::AgentMessageRouting>,
    },
    MessageDeliveryUpdated {
        message: AgentMessage,
        routing: dragent_workspace_model::AgentMessageRouting,
    },
    ShapingProposalUpdated {
        proposal: ShapingProposal,
    },
    ShapingRecordsUpdated {
        records: ShapingRecords,
    },
    ArtifactUpdated {
        artifact: ArtifactRef,
    },
    ArtifactsReplaced {
        artifacts: Vec<ArtifactRef>,
    },
    EvidenceRecorded {
        evidence: Evidence,
    },
    AttentionUpdated {
        attention_item: AttentionItem,
    },
    CheckpointRecorded {
        checkpoint: Checkpoint,
    },
}

impl WorkspaceEventPayload {
    pub(crate) fn work_item_id(&self) -> Option<WorkItemId> {
        match self {
            Self::WorkBriefUpdated { work_brief } => Some(work_brief.work_item_id),
            Self::RequirementUpdated { requirement } => Some(requirement.work_item_id),
            Self::DecisionUpdated { decision } => Some(decision.work_item_id),
            Self::ShapingMemoryRecorded { note } => Some(note.work_item_id),
            Self::AcceptanceCriterionUpdated {
                acceptance_criterion,
            } => Some(acceptance_criterion.work_item_id),
            Self::ConstraintUpdated { constraint } => Some(constraint.work_item_id),
            Self::RiskUpdated { risk } => Some(risk.work_item_id),
            Self::PlanUpdated { plan } => Some(plan.work_item_id),
            Self::PlanTaskStatusChanged { plan, .. } => Some(plan.work_item_id),
            Self::WorkContractUpdated { work_contract } => Some(work_contract.work_item_id),
            Self::AgentInstanceProvisioned { agent_instance, .. } => {
                Some(agent_instance.work_item_id)
            }
            Self::AgentInstanceActivated { agent_instance, .. } => {
                Some(agent_instance.work_item_id)
            }
            Self::ExecutionLeaseRenewed { agent_instance, .. } => Some(agent_instance.work_item_id),
            Self::AgentExecutionRetryPrepared { agent_instance, .. } => {
                Some(agent_instance.work_item_id)
            }
            Self::AgentExecutionStarted { agent_instance, .. }
            | Self::AgentExecutionFinished { agent_instance, .. }
            | Self::AgentExecutionReviewResolved { agent_instance, .. } => {
                Some(agent_instance.work_item_id)
            }
            Self::ProposedTeamUpdated { proposed_team } => Some(proposed_team.work_item_id),
            Self::MessagePosted { message, .. } | Self::MessageDeliveryUpdated { message, .. } => {
                Some(message.work_item_id)
            }
            Self::ShapingRecordsUpdated { records } => Some(records.work_item_id),
            Self::ArtifactUpdated { artifact } => Some(artifact.work_item_id),
            Self::ArtifactsReplaced { artifacts } => {
                artifacts.first().map(|artifact| artifact.work_item_id)
            }
            Self::EvidenceRecorded { evidence } => Some(evidence.work_item_id),
            Self::AttentionUpdated { attention_item } => Some(attention_item.work_item_id),
            Self::CheckpointRecorded { checkpoint } => Some(checkpoint.work_item_id),
            Self::WorkCreated { .. }
            | Self::WorkStatusChanged { .. }
            | Self::RoleDefinitionUpdated { .. }
            | Self::ShapingProposalUpdated { .. } => None,
        }
    }

    fn task_id(&self) -> Option<PlanTaskId> {
        match self {
            Self::PlanTaskStatusChanged { task_id, .. } => Some(*task_id),
            Self::AgentExecutionStarted { attempt, .. } => Some(attempt.task_id),
            Self::AgentExecutionFinished { attempt, .. }
            | Self::AgentExecutionReviewResolved { attempt, .. } => Some(attempt.task_id),
            Self::MessagePosted {
                routing: Some(routing),
                ..
            } => routing.task_id,
            Self::MessageDeliveryUpdated { routing, .. } => routing.task_id,
            Self::EvidenceRecorded { evidence } => evidence.task_id,
            Self::AttentionUpdated { attention_item } => attention_item.task_id,
            Self::CheckpointRecorded { checkpoint } => checkpoint.task_id,
            _ => None,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), WorkspaceProtocolError> {
        match self {
            Self::WorkCreated { title, .. } => require_text("title", title),
            Self::WorkStatusChanged { previous, current } => {
                if previous.can_transition_to(*current) {
                    Ok(())
                } else {
                    Err(WorkspaceProtocolError::InvalidPayload(format!(
                        "invalid status transition from {previous:?} to {current:?}"
                    )))
                }
            }
            Self::WorkBriefUpdated { work_brief } => work_brief
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::RequirementUpdated { requirement } => requirement
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::DecisionUpdated { decision } => decision
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ShapingMemoryRecorded { note } => note
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::AcceptanceCriterionUpdated {
                acceptance_criterion,
            } => acceptance_criterion
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ArtifactUpdated { artifact } => artifact
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ArtifactsReplaced { artifacts } => {
                let mut ids = HashSet::new();
                for artifact in artifacts {
                    artifact.validate().map_err(|error| {
                        WorkspaceProtocolError::InvalidPayload(error.to_string())
                    })?;
                    if !ids.insert(artifact.id) {
                        return Err(WorkspaceProtocolError::InvalidPayload(
                            "replacement artifacts contain a duplicate identity".into(),
                        ));
                    }
                }
                Ok(())
            }
            Self::PlanUpdated { plan } => plan
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::PlanTaskStatusChanged {
                plan,
                task_id,
                previous,
                current,
            } => {
                plan.validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                let task = plan
                    .tasks
                    .iter()
                    .find(|task| task.id == *task_id)
                    .ok_or_else(|| {
                        WorkspaceProtocolError::InvalidPayload(
                            "task transition references an unknown plan task".into(),
                        )
                    })?;
                if previous == current || task.status != *current {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "task transition status does not match the updated plan".into(),
                    ));
                }
                Ok(())
            }
            Self::WorkContractUpdated { work_contract } => work_contract
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::AgentInstanceProvisioned {
                agent_instance,
                work_contract,
            } => {
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                work_contract
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                if agent_instance.status != AgentInstanceStatus::Pending
                    || work_contract.status != WorkContractStatus::Approved
                    || agent_instance.work_item_id != work_contract.work_item_id
                    || agent_instance.role_definition_id != work_contract.role_definition_id
                    || agent_instance.contract_id != work_contract.id
                    || work_contract.agent_instance_id != Some(agent_instance.id)
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "provisioned agent and approved contract binding do not match".into(),
                    ));
                }
                Ok(())
            }
            Self::AgentInstanceActivated {
                agent_instance,
                work_contract,
                lease,
            } => {
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                work_contract
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                lease
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                if agent_instance.status != AgentInstanceStatus::Ready
                    || work_contract.status != WorkContractStatus::Active
                    || lease.status != ExecutionLeaseStatus::Active
                    || agent_instance.work_item_id != work_contract.work_item_id
                    || lease.work_item_id != agent_instance.work_item_id
                    || agent_instance.role_definition_id != work_contract.role_definition_id
                    || agent_instance.contract_id != work_contract.id
                    || lease.contract_id != work_contract.id
                    || lease.agent_instance_id != agent_instance.id
                    || agent_instance.active_lease_id != Some(lease.id)
                    || work_contract.agent_instance_id != Some(agent_instance.id)
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "activated agent, lease, and active contract do not match".into(),
                    ));
                }
                Ok(())
            }
            Self::ExecutionLeaseRenewed {
                agent_instance,
                lease,
            } => {
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                lease
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                if lease.status != ExecutionLeaseStatus::Active
                    || lease.work_item_id != agent_instance.work_item_id
                    || lease.agent_instance_id != agent_instance.id
                    || lease.contract_id != agent_instance.contract_id
                    || agent_instance.active_lease_id != Some(lease.id)
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "renewed lease and active agent do not match".into(),
                    ));
                }
                Ok(())
            }
            Self::AgentExecutionRetryPrepared {
                agent_instance,
                lease,
                plan,
            } => {
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                lease
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                plan.validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                let task = plan.tasks.iter().find(|task| {
                    task.assigned_contract_id == Some(agent_instance.contract_id)
                        && task.status == PlanTaskStatus::Ready
                });
                if agent_instance.status != AgentInstanceStatus::Ready
                    || lease.status != ExecutionLeaseStatus::Active
                    || lease.work_item_id != agent_instance.work_item_id
                    || plan.work_item_id != agent_instance.work_item_id
                    || lease.agent_instance_id != agent_instance.id
                    || lease.contract_id != agent_instance.contract_id
                    || agent_instance.active_lease_id != Some(lease.id)
                    || task.is_none()
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "retry-prepared agent, lease, and plan do not match".into(),
                    ));
                }
                Ok(())
            }
            Self::AgentExecutionStarted {
                agent_instance,
                lease,
                attempt,
                plan,
            } => {
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                lease
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                attempt
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                plan.validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                let task = plan
                    .tasks
                    .iter()
                    .find(|task| task.id == attempt.task_id)
                    .ok_or_else(|| {
                        WorkspaceProtocolError::InvalidPayload(
                            "execution attempt references an unknown plan task".into(),
                        )
                    })?;
                if agent_instance.status != AgentInstanceStatus::Running
                    || lease.status != ExecutionLeaseStatus::Active
                    || attempt.status != ExecutionAttemptStatus::Running
                    || task.status != PlanTaskStatus::Running
                    || attempt.work_item_id != agent_instance.work_item_id
                    || plan.work_item_id != agent_instance.work_item_id
                    || attempt.agent_instance_id != agent_instance.id
                    || attempt.contract_id != agent_instance.contract_id
                    || attempt.lease_id != lease.id
                    || lease.agent_instance_id != agent_instance.id
                    || lease.contract_id != agent_instance.contract_id
                    || agent_instance.active_lease_id != Some(lease.id)
                    || task.assigned_contract_id != Some(agent_instance.contract_id)
                    || attempt.model_route != agent_instance.model_route
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "running agent, attempt, lease, and plan task do not match".into(),
                    ));
                }
                Ok(())
            }
            Self::AgentExecutionFinished {
                agent_instance,
                lease,
                attempt,
                plan,
                summary,
            } => {
                require_text("execution outcome summary", summary)?;
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                lease
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                attempt
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                plan.validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                let task = plan
                    .tasks
                    .iter()
                    .find(|task| task.id == attempt.task_id)
                    .ok_or_else(|| {
                        WorkspaceProtocolError::InvalidPayload(
                            "finished attempt references an unknown plan task".into(),
                        )
                    })?;
                let lifecycle_matches = match attempt.status {
                    ExecutionAttemptStatus::Succeeded => {
                        agent_instance.status == AgentInstanceStatus::Reviewing
                            && task.status == PlanTaskStatus::Reviewing
                            && lease.status == ExecutionLeaseStatus::Active
                            && agent_instance.active_lease_id == Some(lease.id)
                    }
                    ExecutionAttemptStatus::Failed => {
                        agent_instance.status == AgentInstanceStatus::Failed
                            && task.status == PlanTaskStatus::Failed
                            && lease.status == ExecutionLeaseStatus::Released
                            && agent_instance.active_lease_id.is_none()
                    }
                    ExecutionAttemptStatus::Cancelled => {
                        agent_instance.status == AgentInstanceStatus::Cancelled
                            && task.status == PlanTaskStatus::Cancelled
                            && lease.status == ExecutionLeaseStatus::Cancelled
                            && agent_instance.active_lease_id.is_none()
                    }
                    ExecutionAttemptStatus::TimedOut => {
                        agent_instance.status == AgentInstanceStatus::Failed
                            && task.status == PlanTaskStatus::Failed
                            && lease.status == ExecutionLeaseStatus::Expired
                            && agent_instance.active_lease_id.is_none()
                    }
                    ExecutionAttemptStatus::Paused => {
                        agent_instance.status == AgentInstanceStatus::Ready
                            && task.status == PlanTaskStatus::Ready
                            && lease.status == ExecutionLeaseStatus::Active
                            && agent_instance.active_lease_id == Some(lease.id)
                    }
                    ExecutionAttemptStatus::Running => false,
                };
                if !lifecycle_matches
                    || attempt.work_item_id != agent_instance.work_item_id
                    || plan.work_item_id != agent_instance.work_item_id
                    || attempt.agent_instance_id != agent_instance.id
                    || attempt.contract_id != agent_instance.contract_id
                    || attempt.lease_id != lease.id
                    || lease.agent_instance_id != agent_instance.id
                    || lease.contract_id != agent_instance.contract_id
                    || task.assigned_contract_id != Some(agent_instance.contract_id)
                    || attempt.model_route != agent_instance.model_route
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "finished agent, attempt, lease, and plan task do not match".into(),
                    ));
                }
                Ok(())
            }
            Self::AgentExecutionReviewResolved {
                agent_instance,
                work_contract,
                lease,
                attempt,
                plan,
                resolution,
            } => {
                agent_instance
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                work_contract
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                lease
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                attempt
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                plan.validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                let task = plan
                    .tasks
                    .iter()
                    .find(|task| task.id == attempt.task_id)
                    .ok_or_else(|| {
                        WorkspaceProtocolError::InvalidPayload(
                            "review resolution references an unknown task".into(),
                        )
                    })?;
                let lifecycle_matches = match resolution {
                    dragent_workspace_model::ExecutionReviewResolution::Accepted => {
                        agent_instance.status == AgentInstanceStatus::Completed
                            && work_contract.status == WorkContractStatus::Completed
                            && lease.status == ExecutionLeaseStatus::Released
                            && task.status == PlanTaskStatus::Verifying
                            && agent_instance.active_lease_id.is_none()
                            && work_contract.write_scope.is_empty()
                            && attempt.change_summary.is_none()
                    }
                    dragent_workspace_model::ExecutionReviewResolution::Integrated => {
                        agent_instance.status == AgentInstanceStatus::Completed
                            && work_contract.status == WorkContractStatus::Completed
                            && lease.status == ExecutionLeaseStatus::Released
                            && task.status == PlanTaskStatus::Verifying
                            && agent_instance.active_lease_id.is_none()
                    }
                    dragent_workspace_model::ExecutionReviewResolution::RevisionRequested => {
                        agent_instance.status == AgentInstanceStatus::Ready
                            && work_contract.status == WorkContractStatus::Active
                            && lease.status == ExecutionLeaseStatus::Active
                            && task.status == PlanTaskStatus::Ready
                            && agent_instance.active_lease_id == Some(lease.id)
                    }
                    dragent_workspace_model::ExecutionReviewResolution::Discarded => {
                        agent_instance.status == AgentInstanceStatus::Cancelled
                            && work_contract.status == WorkContractStatus::Revoked
                            && lease.status == ExecutionLeaseStatus::Cancelled
                            && task.status == PlanTaskStatus::Cancelled
                            && agent_instance.active_lease_id.is_none()
                    }
                };
                if !lifecycle_matches
                    || attempt.status != ExecutionAttemptStatus::Succeeded
                    || (*resolution
                        == dragent_workspace_model::ExecutionReviewResolution::Integrated
                        && attempt.change_summary.is_none())
                    || attempt.review_resolution != Some(*resolution)
                    || attempt.agent_instance_id != agent_instance.id
                    || attempt.contract_id != work_contract.id
                    || attempt.lease_id != lease.id
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "resolved execution review lifecycle does not match".into(),
                    ));
                }
                Ok(())
            }
            Self::RoleDefinitionUpdated { role_definition } => role_definition
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ProposedTeamUpdated { proposed_team } => proposed_team
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::MessagePosted { message, routing } => {
                validate_message(message)?;
                validate_message_routing(message, routing.as_ref())
            }
            Self::MessageDeliveryUpdated { message, routing } => {
                validate_message(message)?;
                validate_message_routing(message, Some(routing))
            }
            Self::ShapingProposalUpdated { proposal } => proposal
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ShapingRecordsUpdated { records } => records
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::EvidenceRecorded { evidence } => evidence
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::AttentionUpdated { attention_item } => attention_item
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::CheckpointRecorded { checkpoint } => {
                require_text("checkpoint summary", &checkpoint.summary)
            }
            Self::ConstraintUpdated { .. } | Self::RiskUpdated { .. } => Ok(()),
        }
    }
}

/// Project a canonical event envelope into a domain `WorkspaceEvent`.
/// Pure function shared by the UI and the Host (single authority for the mapping).
pub fn project_envelope_event(
    event: WorkspaceEventEnvelope,
    principal_id: dragent_workspace_model::ActorId,
) -> Option<WorkspaceEvent> {
    let kind = match event.payload {
        WorkspaceEventPayload::WorkCreated { title, .. } => {
            WorkspaceEventKind::WorkCreated { title }
        }
        WorkspaceEventPayload::WorkStatusChanged { current, .. } => {
            WorkspaceEventKind::WorkStatusChanged { status: current }
        }
        WorkspaceEventPayload::WorkBriefUpdated { work_brief } => {
            WorkspaceEventKind::WorkBriefUpdated { work_brief }
        }
        WorkspaceEventPayload::MessagePosted { message, routing } => {
            if let Some(routing) = routing {
                WorkspaceEventKind::AgentMessagePosted { message, routing }
            } else if message.sender_id == principal_id {
                WorkspaceEventKind::PrincipalMessagePosted { body: message.body }
            } else {
                WorkspaceEventKind::UserMessagePosted { body: message.body }
            }
        }
        WorkspaceEventPayload::MessageDeliveryUpdated { message, routing } => {
            WorkspaceEventKind::AgentMessageDeliveryUpdated { message, routing }
        }
        WorkspaceEventPayload::ShapingProposalUpdated { proposal } => {
            WorkspaceEventKind::ShapingProposalUpdated { proposal }
        }
        WorkspaceEventPayload::ShapingRecordsUpdated { records } => {
            WorkspaceEventKind::ShapingRecordsUpdated { records }
        }
        WorkspaceEventPayload::ArtifactUpdated { artifact } => {
            WorkspaceEventKind::ArtifactUpdated { artifact }
        }
        WorkspaceEventPayload::ArtifactsReplaced { artifacts } => {
            WorkspaceEventKind::ArtifactsReplaced { artifacts }
        }
        WorkspaceEventPayload::RequirementUpdated { requirement } => {
            WorkspaceEventKind::RequirementUpdated { requirement }
        }
        WorkspaceEventPayload::DecisionUpdated { decision } => {
            WorkspaceEventKind::DecisionUpdated { decision }
        }
        WorkspaceEventPayload::AcceptanceCriterionUpdated {
            acceptance_criterion,
        } => WorkspaceEventKind::AcceptanceCriterionUpdated {
            acceptance_criterion,
        },
        WorkspaceEventPayload::EvidenceRecorded { evidence } => {
            WorkspaceEventKind::EvidenceRecorded { evidence }
        }
        WorkspaceEventPayload::ShapingMemoryRecorded { note } => {
            WorkspaceEventKind::ShapingMemoryRecorded { note }
        }
        WorkspaceEventPayload::RoleDefinitionUpdated { role_definition } => {
            WorkspaceEventKind::RoleDefinitionUpdated { role_definition }
        }
        WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
            WorkspaceEventKind::WorkContractUpdated { work_contract }
        }
        WorkspaceEventPayload::AgentInstanceProvisioned {
            agent_instance,
            work_contract,
        } => WorkspaceEventKind::AgentInstanceProvisioned {
            agent_instance,
            work_contract,
        },
        WorkspaceEventPayload::AgentInstanceActivated {
            agent_instance,
            work_contract,
            lease,
        } => WorkspaceEventKind::AgentInstanceActivated {
            agent_instance,
            work_contract,
            lease,
        },
        WorkspaceEventPayload::ExecutionLeaseRenewed {
            agent_instance,
            lease,
        } => WorkspaceEventKind::ExecutionLeaseRenewed {
            agent_instance,
            lease,
        },
        WorkspaceEventPayload::AgentExecutionRetryPrepared {
            agent_instance,
            lease,
            plan,
        } => WorkspaceEventKind::AgentExecutionRetryPrepared {
            agent_instance,
            lease,
            plan,
        },
        WorkspaceEventPayload::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        } => WorkspaceEventKind::AgentExecutionStarted {
            agent_instance,
            lease,
            attempt,
            plan,
        },
        WorkspaceEventPayload::AgentExecutionFinished {
            agent_instance,
            lease,
            attempt,
            plan,
            summary,
        } => WorkspaceEventKind::AgentExecutionFinished {
            agent_instance,
            lease,
            attempt,
            plan,
            summary,
        },
        WorkspaceEventPayload::AgentExecutionReviewResolved {
            agent_instance,
            work_contract,
            lease,
            attempt,
            plan,
            resolution,
        } => WorkspaceEventKind::AgentExecutionReviewResolved {
            agent_instance,
            work_contract,
            lease,
            attempt,
            plan,
            resolution,
        },
        WorkspaceEventPayload::ProposedTeamUpdated { proposed_team } => {
            WorkspaceEventKind::ProposedTeamUpdated { proposed_team }
        }
        WorkspaceEventPayload::PlanUpdated { plan } => {
            WorkspaceEventKind::StructuredPlanUpdated { plan }
        }
        WorkspaceEventPayload::PlanTaskStatusChanged { plan, .. } => {
            WorkspaceEventKind::StructuredPlanUpdated { plan }
        }
        WorkspaceEventPayload::AttentionUpdated { attention_item } => {
            WorkspaceEventKind::AttentionUpdated { attention_item }
        }
        _ => return None,
    };
    Some(WorkspaceEvent {
        sequence: event.sequence,
        timestamp_unix_ms: event.timestamp_unix_ms,
        project_id: event.project_id,
        work_item_id: Some(event.work_item_id),
        kind,
    })
}
