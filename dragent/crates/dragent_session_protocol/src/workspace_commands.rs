//! Versioned wire contract for synchronized agentic-workspace state.
//!
//! This protocol is distinct from the shared-terminal protocol in the parent
//! module. Workspace commands express user intent and are idempotent by command
//! id. Workspace events are immutable facts ordered within one work item.

use std::collections::{HashMap, HashSet};
use std::fmt;

use dragent_workspace_model::{
    AcceptanceCriterion, ActorId, AgentInstanceId, AgentMessage, ArtifactRef,
    ChargingAuthorization, Constraint, Decision, DeviceId, ExecutionAttemptId,
    ExecutionAttemptStatus, ExecutionLeaseId, ExecutionLocation, Plan, PlanTaskId, PlanTaskStatus,
    ProjectId, ProposedTeam, Requirement, Risk, RoleDefinition, ShapingProposal, ShapingRecords,
    WorkBrief, WorkContract, WorkContractId, WorkContractStatus, WorkItemId, WorkStatus,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::workspace_error::{
    require_text, validate_common, validate_message, validate_message_routing,
    WorkspaceProtocolError,
};

macro_rules! wire_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn is_nil(self) -> bool {
                self.0.is_nil()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

wire_id!(WorkspaceCommandId);
wire_id!(WorkspaceEventId);
wire_id!(CorrelationId);

/// One retry-safe request to mutate authoritative workspace state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceCommandEnvelope {
    pub command_id: WorkspaceCommandId,
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub work_item_id: WorkItemId,
    pub actor_id: ActorId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    pub correlation_id: CorrelationId,
    pub timestamp_unix_ms: i64,
    /// Optimistic concurrency guard for commands that update existing work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    pub payload: WorkspaceCommandPayload,
}

impl WorkspaceCommandEnvelope {
    pub fn validate(&self) -> Result<(), WorkspaceProtocolError> {
        validate_common(
            self.schema_version,
            self.project_id.0,
            self.actor_id.0,
            self.correlation_id.0,
            self.timestamp_unix_ms,
        )?;
        if self.command_id.is_nil() {
            return Err(WorkspaceProtocolError::NilIdentifier("command_id"));
        }
        if self.work_item_id.0.is_nil() {
            return Err(WorkspaceProtocolError::NilIdentifier("work_item_id"));
        }
        if self.device_id.is_some_and(|device_id| device_id.0.is_nil()) {
            return Err(WorkspaceProtocolError::NilIdentifier("device_id"));
        }
        if self.payload.requires_expected_revision() && self.expected_revision.is_none() {
            return Err(WorkspaceProtocolError::ExpectedRevisionRequired);
        }
        if let Some(payload_work_item) = self.payload.work_item_id() {
            if self.work_item_id != payload_work_item {
                return Err(WorkspaceProtocolError::PayloadWorkItemMismatch);
            }
        }
        if let WorkspaceCommandPayload::PostMessage { message, .. } = &self.payload {
            if message.sender_id != self.actor_id {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
        }
        if let WorkspaceCommandPayload::CompleteShapingTurn { message, .. } = &self.payload {
            if message.sender_id != self.actor_id {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
        }
        if let WorkspaceCommandPayload::RaiseAttention { attention_item } = &self.payload {
            if attention_item.requested_by != self.actor_id {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
        }
        let provenance_matches_actor = |provenance: &[dragent_workspace_model::Provenance]| {
            provenance
                .iter()
                .any(|entry| entry.recorded_by == self.actor_id)
        };
        match &self.payload {
            WorkspaceCommandPayload::ActivateAgentInstance {
                charging,
                lease_expires_at_unix_ms,
                ..
            } => {
                if charging.owner_id != self.actor_id {
                    return Err(WorkspaceProtocolError::PayloadActorMismatch);
                }
                if *lease_expires_at_unix_ms <= self.timestamp_unix_ms {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "execution lease must expire after activation".into(),
                    ));
                }
            }
            WorkspaceCommandPayload::UpsertWorkBrief { work_brief }
                if work_brief.updated_by != self.actor_id =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertRequirement { requirement }
                if !provenance_matches_actor(&requirement.provenance) =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertDecision { decision }
                if !provenance_matches_actor(&decision.provenance) =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertAcceptanceCriterion {
                acceptance_criterion,
            } if !provenance_matches_actor(&acceptance_criterion.provenance) => {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::RecordEvidence { evidence }
                if evidence.produced_by != self.actor_id =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertRoleDefinition { role_definition }
                if role_definition.created_by != self.actor_id =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertWorkContract { work_contract }
                if work_contract.updated_by != self.actor_id =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::RevisePlan { work_contracts, .. }
                if work_contracts
                    .iter()
                    .any(|contract| contract.updated_by != self.actor_id) =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::CompleteShapingTurn { work_contracts, .. }
                if work_contracts
                    .iter()
                    .any(|contract| contract.updated_by != self.actor_id) =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertPlan { plan }
                if plan.status == dragent_workspace_model::PlanStatus::Approved
                    && plan.approved_by != Some(self.actor_id)
                    && self.actor_id
                        != dragent_workspace_model::ActorRef::principal_agent(self.project_id)
                            .id =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            WorkspaceCommandPayload::UpsertProposedTeam { proposed_team }
                if proposed_team.created_by != self.actor_id =>
            {
                return Err(WorkspaceProtocolError::PayloadActorMismatch);
            }
            _ => {}
        }
        self.payload.validate()
    }
}

/// Commands initially required by the shaping flow. New variants require a
/// schema-version compatibility decision; clients must not infer commands from
/// conversational text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceCommandPayload {
    CreateWorkItem {
        title: String,
        execution_location: ExecutionLocation,
    },
    TransitionWork {
        status: WorkStatus,
    },
    TransitionPlanTask {
        task_id: PlanTaskId,
        status: PlanTaskStatus,
    },
    ProvisionAgentInstance {
        agent_instance_id: AgentInstanceId,
        contract_id: WorkContractId,
        model_route: String,
    },
    ActivateAgentInstance {
        agent_instance_id: AgentInstanceId,
        lease_id: ExecutionLeaseId,
        effective_model_route: String,
        charging: ChargingAuthorization,
        lease_expires_at_unix_ms: i64,
    },
    RenewExecutionLease {
        lease_id: ExecutionLeaseId,
        lease_expires_at_unix_ms: i64,
    },
    StartAgentExecution {
        attempt_id: ExecutionAttemptId,
        task_id: PlanTaskId,
        agent_instance_id: AgentInstanceId,
        lease_id: ExecutionLeaseId,
        isolation: dragent_workspace_model::ExecutionIsolation,
    },
    FinishAgentExecution {
        attempt_id: ExecutionAttemptId,
        outcome: ExecutionAttemptStatus,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change_summary: Option<dragent_workspace_model::ExecutionChangeSummary>,
    },
    /// Record cancellation intent before the execution host stops the
    /// provider. Authority terminalizes the attempt atomically; hosts react to
    /// the resulting `AgentExecutionFinished` event.
    CancelAgentExecution {
        attempt_id: ExecutionAttemptId,
        reason: String,
    },
    PauseAgentExecution {
        attempt_id: ExecutionAttemptId,
        reason: String,
    },
    PrepareAgentExecutionRetry {
        failed_attempt_id: ExecutionAttemptId,
        lease_id: ExecutionLeaseId,
        lease_expires_at_unix_ms: i64,
    },
    ResolveAgentExecutionReview {
        attempt_id: ExecutionAttemptId,
        resolution: dragent_workspace_model::ExecutionReviewResolution,
    },
    UpsertWorkBrief {
        work_brief: WorkBrief,
    },
    UpsertRequirement {
        requirement: Requirement,
    },
    UpsertDecision {
        decision: Decision,
    },
    UpsertAcceptanceCriterion {
        acceptance_criterion: AcceptanceCriterion,
    },
    UpsertArtifact {
        artifact: ArtifactRef,
    },
    /// Replace the complete user-editable shaping record set. This is used by
    /// explicit add/remove controls; authority rejects removals that would
    /// leave plan or contract references dangling.
    ReplaceShapingRecords {
        records: ShapingRecords,
    },
    /// Replace the complete repository-artifact set for the work item.
    ReplaceArtifacts {
        artifacts: Vec<ArtifactRef>,
    },
    UpsertConstraint {
        constraint: Constraint,
    },
    UpsertRisk {
        risk: Risk,
    },
    UpsertPlan {
        plan: Plan,
    },
    /// Publish a user-edited dependency-plan revision and its derived role
    /// assignments as one authority transaction.
    RevisePlan {
        plan: Plan,
        work_contracts: Vec<WorkContract>,
    },
    UpsertRoleDefinition {
        role_definition: RoleDefinition,
    },
    UpsertWorkContract {
        work_contract: WorkContract,
    },
    UpsertProposedTeam {
        proposed_team: ProposedTeam,
    },
    DescribeRole {
        description: String,
    },
    PostMessage {
        message: AgentMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        routing: Option<dragent_workspace_model::AgentMessageRouting>,
    },
    UpdateMessageDelivery {
        message_id: dragent_workspace_model::AgentMessageId,
        delivery_state: dragent_workspace_model::MessageDeliveryState,
    },
    RecordEvidence {
        evidence: dragent_workspace_model::Evidence,
    },
    /// Append one turn's work-item memory notes to the Principal's brain.
    /// Append-only: notes are never edited or removed once recorded.
    RecordShapingMemory {
        notes: Vec<dragent_workspace_model::ShapingMemoryNote>,
    },
    CompleteShapingTurn {
        message: AgentMessage,
        proposal: ShapingProposal,
        records: ShapingRecords,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<ArtifactRef>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        role_definitions: Vec<RoleDefinition>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        work_contracts: Vec<WorkContract>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proposed_team: Option<ProposedTeam>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<Plan>,
    },
    RaiseAttention {
        attention_item: dragent_workspace_model::AttentionItem,
    },
    ResolveAttention {
        attention_item_id: dragent_workspace_model::AttentionItemId,
        outcome: dragent_workspace_model::AttentionResolution,
        resolution: String,
    },
}

impl WorkspaceCommandPayload {
    pub fn requires_expected_revision(&self) -> bool {
        matches!(
            self,
            Self::TransitionWork { .. }
                | Self::TransitionPlanTask { .. }
                | Self::ProvisionAgentInstance { .. }
                | Self::ActivateAgentInstance { .. }
                | Self::RenewExecutionLease { .. }
                | Self::StartAgentExecution { .. }
                | Self::FinishAgentExecution { .. }
                | Self::CancelAgentExecution { .. }
                | Self::PauseAgentExecution { .. }
                | Self::PrepareAgentExecutionRetry { .. }
                | Self::ResolveAgentExecutionReview { .. }
                | Self::UpsertWorkBrief { .. }
                | Self::UpsertRequirement { .. }
                | Self::UpsertDecision { .. }
                | Self::UpsertAcceptanceCriterion { .. }
                | Self::UpsertArtifact { .. }
                | Self::ReplaceShapingRecords { .. }
                | Self::ReplaceArtifacts { .. }
                | Self::UpsertConstraint { .. }
                | Self::UpsertRisk { .. }
                | Self::UpsertPlan { .. }
                | Self::RevisePlan { .. }
                | Self::UpsertWorkContract { .. }
                | Self::UpsertProposedTeam { .. }
                | Self::RaiseAttention { .. }
                | Self::ResolveAttention { .. }
                | Self::UpdateMessageDelivery { .. }
                | Self::RecordEvidence { .. }
                | Self::RecordShapingMemory { .. }
        ) || matches!(self, Self::CompleteShapingTurn { plan: Some(_), .. })
    }

    fn work_item_id(&self) -> Option<WorkItemId> {
        match self {
            Self::UpsertWorkBrief { work_brief } => Some(work_brief.work_item_id),
            Self::UpsertRequirement { requirement } => Some(requirement.work_item_id),
            Self::UpsertDecision { decision } => Some(decision.work_item_id),
            Self::UpsertAcceptanceCriterion {
                acceptance_criterion,
            } => Some(acceptance_criterion.work_item_id),
            Self::UpsertArtifact { artifact } => Some(artifact.work_item_id),
            Self::ReplaceShapingRecords { records } => Some(records.work_item_id),
            Self::ReplaceArtifacts { artifacts } => {
                artifacts.first().map(|artifact| artifact.work_item_id)
            }
            Self::UpsertConstraint { constraint } => Some(constraint.work_item_id),
            Self::UpsertRisk { risk } => Some(risk.work_item_id),
            Self::UpsertPlan { plan } => Some(plan.work_item_id),
            Self::RevisePlan { plan, .. } => Some(plan.work_item_id),
            Self::UpsertWorkContract { work_contract } => Some(work_contract.work_item_id),
            Self::UpsertProposedTeam { proposed_team } => Some(proposed_team.work_item_id),
            Self::PostMessage { message, .. } => Some(message.work_item_id),
            Self::RecordEvidence { evidence } => Some(evidence.work_item_id),
            Self::RecordShapingMemory { notes } => notes.first().map(|note| note.work_item_id),
            Self::CompleteShapingTurn { message, .. } => Some(message.work_item_id),
            Self::RaiseAttention { attention_item } => Some(attention_item.work_item_id),
            Self::CreateWorkItem { .. }
            | Self::TransitionWork { .. }
            | Self::TransitionPlanTask { .. }
            | Self::ProvisionAgentInstance { .. }
            | Self::ActivateAgentInstance { .. }
            | Self::RenewExecutionLease { .. }
            | Self::StartAgentExecution { .. }
            | Self::FinishAgentExecution { .. }
            | Self::CancelAgentExecution { .. }
            | Self::PauseAgentExecution { .. }
            | Self::PrepareAgentExecutionRetry { .. }
            | Self::ResolveAgentExecutionReview { .. }
            | Self::UpdateMessageDelivery { .. }
            | Self::UpsertRoleDefinition { .. }
            | Self::DescribeRole { .. }
            | Self::ResolveAttention { .. } => None,
        }
    }

    fn validate(&self) -> Result<(), WorkspaceProtocolError> {
        match self {
            Self::CreateWorkItem { title, .. } => require_text("title", title),
            Self::UpsertWorkBrief { work_brief } => work_brief
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::UpsertRequirement { requirement } => requirement
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::UpsertDecision { decision } => decision
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::UpsertAcceptanceCriterion {
                acceptance_criterion,
            } => acceptance_criterion
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::UpsertArtifact { artifact } => artifact
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ReplaceShapingRecords { records } => records
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ReplaceArtifacts { artifacts } => {
                let mut ids = HashSet::new();
                let mut work_item_id = None;
                for artifact in artifacts {
                    artifact.validate().map_err(|error| {
                        WorkspaceProtocolError::InvalidPayload(error.to_string())
                    })?;
                    if work_item_id
                        .replace(artifact.work_item_id)
                        .is_some_and(|id| id != artifact.work_item_id)
                    {
                        return Err(WorkspaceProtocolError::PayloadWorkItemMismatch);
                    }
                    if !ids.insert(artifact.id) {
                        return Err(WorkspaceProtocolError::InvalidPayload(
                            "replacement artifacts contain a duplicate identity".into(),
                        ));
                    }
                }
                Ok(())
            }
            Self::UpsertPlan { plan } => plan
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::RevisePlan {
                plan,
                work_contracts,
            } => validate_plan_revision(plan, work_contracts),
            Self::UpsertRoleDefinition { role_definition } => role_definition
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::UpsertWorkContract { work_contract } => {
                work_contract
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                if work_contract.agent_instance_id.is_some() {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "agent instances must be bound by provision_agent_instance".into(),
                    ));
                }
                Ok(())
            }
            Self::UpsertProposedTeam { proposed_team } => proposed_team
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::DescribeRole { description } => require_text("description", description),
            Self::PostMessage { message, routing } => {
                validate_message(message)?;
                validate_message_routing(message, routing.as_ref())
            }
            Self::UpdateMessageDelivery {
                message_id,
                delivery_state,
            } => {
                if message_id.0.is_nil()
                    || matches!(
                        delivery_state,
                        dragent_workspace_model::MessageDeliveryState::LocalOnly
                            | dragent_workspace_model::MessageDeliveryState::Pending
                    )
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "message delivery updates require a message and terminal forward state"
                            .into(),
                    ));
                }
                Ok(())
            }
            Self::RecordEvidence { evidence } => evidence
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::RecordShapingMemory { notes } => {
                if notes.is_empty() {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "shaping memory batches must contain at least one note".into(),
                    ));
                }
                if notes.len() > dragent_workspace_model::MAX_SHAPING_MEMORY_BATCH {
                    return Err(WorkspaceProtocolError::InvalidPayload(format!(
                        "shaping memory batches hold at most {} notes",
                        dragent_workspace_model::MAX_SHAPING_MEMORY_BATCH
                    )));
                }
                for note in notes {
                    note.validate().map_err(|error| {
                        WorkspaceProtocolError::InvalidPayload(error.to_string())
                    })?;
                }
                Ok(())
            }
            Self::CompleteShapingTurn {
                message,
                proposal,
                records,
                artifacts,
                role_definitions,
                work_contracts,
                proposed_team,
                plan,
            } => {
                validate_message(message)?;
                proposal
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                records
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                if records.proposal_revision != proposal.revision {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "shaping records revision does not match proposal".into(),
                    ));
                }
                if records.work_item_id != message.work_item_id {
                    return Err(WorkspaceProtocolError::PayloadWorkItemMismatch);
                }
                let provenance_matches = |provenance: &[dragent_workspace_model::Provenance]| {
                    provenance.iter().any(|entry| {
                        entry.recorded_by == message.sender_id
                            && matches!(
                                entry.source,
                                dragent_workspace_model::ProvenanceSource::AgentMessage {
                                    thread_id,
                                    message_id,
                                } if thread_id == message.thread_id && message_id == message.id
                            )
                    })
                };
                if records
                    .requirements
                    .iter()
                    .any(|record| !provenance_matches(&record.provenance))
                    || records
                        .decisions
                        .iter()
                        .any(|record| !provenance_matches(&record.provenance))
                    || records
                        .acceptance_criteria
                        .iter()
                        .any(|record| !provenance_matches(&record.provenance))
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "shaping records must cite the Principal message".into(),
                    ));
                }
                if proposed_team.is_some() != plan.is_some() {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "a complete shaping snapshot requires both a plan and proposed team".into(),
                    ));
                }
                for artifact in artifacts {
                    artifact.validate().map_err(|error| {
                        WorkspaceProtocolError::InvalidPayload(error.to_string())
                    })?;
                    if artifact.work_item_id != message.work_item_id {
                        return Err(WorkspaceProtocolError::PayloadWorkItemMismatch);
                    }
                }
                for definition in role_definitions {
                    definition.validate().map_err(|error| {
                        WorkspaceProtocolError::InvalidPayload(error.to_string())
                    })?;
                }
                if let (Some(team), Some(plan)) = (proposed_team, plan) {
                    team.validate().map_err(|error| {
                        WorkspaceProtocolError::InvalidPayload(error.to_string())
                    })?;
                    if team.work_item_id != message.work_item_id
                        || plan.work_item_id != message.work_item_id
                    {
                        return Err(WorkspaceProtocolError::PayloadWorkItemMismatch);
                    }
                    let team_contracts = team
                        .roles
                        .iter()
                        .map(|role| role.work_contract_id)
                        .collect::<HashSet<_>>();
                    let supplied_contracts = work_contracts
                        .iter()
                        .map(|contract| contract.id)
                        .collect::<HashSet<_>>();
                    let team_definitions = team
                        .roles
                        .iter()
                        .map(|role| role.role_definition_id)
                        .collect::<HashSet<_>>();
                    let supplied_definitions = role_definitions
                        .iter()
                        .map(|definition| definition.id)
                        .collect::<HashSet<_>>();
                    if team_contracts != supplied_contracts
                        || team_definitions != supplied_definitions
                    {
                        return Err(WorkspaceProtocolError::InvalidPayload(
                            "the proposed team must exactly match the supplied roles and contracts"
                                .into(),
                        ));
                    }
                    for role in &team.roles {
                        let definition = role_definitions
                            .iter()
                            .find(|definition| definition.id == role.role_definition_id)
                            .expect("set equality checked role definition");
                        let contract = work_contracts
                            .iter()
                            .find(|contract| contract.id == role.work_contract_id)
                            .expect("set equality checked work contract");
                        if contract.role_definition_id != definition.id {
                            return Err(WorkspaceProtocolError::InvalidPayload(
                                "proposed role contract does not match its role definition".into(),
                            ));
                        }
                        if definition.kind == dragent_workspace_model::AgentRoleKind::Reviewer
                            && !contract.write_scope.is_empty()
                        {
                            return Err(WorkspaceProtocolError::InvalidPayload(
                                "reviewer work contracts must remain read-only".into(),
                            ));
                        }
                    }
                    for task in &plan.tasks {
                        let role_kind = task
                            .assigned_contract_id
                            .and_then(|id| work_contracts.iter().find(|contract| contract.id == id))
                            .and_then(|contract| {
                                role_definitions
                                    .iter()
                                    .find(|definition| definition.id == contract.role_definition_id)
                            })
                            .map(|definition| definition.kind)
                            .unwrap_or_default();
                        if (role_kind == dragent_workspace_model::AgentRoleKind::Reviewer)
                            != task.review_target_task_id.is_some()
                        {
                            return Err(WorkspaceProtocolError::InvalidPayload(
                                "reviewer tasks require an explicit review target and worker tasks cannot declare one"
                                    .into(),
                            ));
                        }
                    }
                    validate_plan_revision(plan, work_contracts)?;
                } else if !artifacts.is_empty()
                    || !role_definitions.is_empty()
                    || !work_contracts.is_empty()
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "partial shaping snapshot records are not allowed".into(),
                    ));
                }
                Ok(())
            }
            Self::RaiseAttention { attention_item } => attention_item
                .validate()
                .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string())),
            Self::ResolveAttention {
                attention_item_id,
                outcome: _,
                resolution,
            } => {
                if attention_item_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("attention_item_id"));
                }
                require_text("resolution", resolution)
            }
            Self::TransitionPlanTask { task_id, .. } if task_id.0.is_nil() => {
                Err(WorkspaceProtocolError::NilIdentifier("task_id"))
            }
            Self::ProvisionAgentInstance {
                agent_instance_id,
                contract_id,
                model_route,
            } => {
                if agent_instance_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("agent_instance_id"));
                }
                if contract_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("contract_id"));
                }
                require_text("model_route", model_route)
            }
            Self::ActivateAgentInstance {
                agent_instance_id,
                lease_id,
                effective_model_route,
                charging,
                lease_expires_at_unix_ms,
            } => {
                if agent_instance_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("agent_instance_id"));
                }
                if lease_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("lease_id"));
                }
                require_text("effective_model_route", effective_model_route)?;
                charging
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                if *lease_expires_at_unix_ms <= 0 {
                    return Err(WorkspaceProtocolError::InvalidTimestamp);
                }
                Ok(())
            }
            Self::RenewExecutionLease {
                lease_id,
                lease_expires_at_unix_ms,
            } => {
                if lease_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("lease_id"));
                }
                if *lease_expires_at_unix_ms <= 0 {
                    return Err(WorkspaceProtocolError::InvalidTimestamp);
                }
                Ok(())
            }
            Self::StartAgentExecution {
                attempt_id,
                task_id,
                agent_instance_id,
                lease_id,
                isolation,
            } => {
                if attempt_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("attempt_id"));
                }
                if task_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("task_id"));
                }
                if agent_instance_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("agent_instance_id"));
                }
                if lease_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("lease_id"));
                }
                isolation
                    .validate()
                    .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
                Ok(())
            }
            Self::FinishAgentExecution {
                attempt_id,
                outcome,
                summary,
                change_summary,
            } => {
                if attempt_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("attempt_id"));
                }
                if matches!(
                    outcome,
                    ExecutionAttemptStatus::Running | ExecutionAttemptStatus::Paused
                ) {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "execution finish requires a provider terminal outcome; pauses use pause_agent_execution".into(),
                    ));
                }
                require_text("execution outcome summary", summary)?;
                if change_summary
                    .as_ref()
                    .is_some_and(|summary| summary.captured_at_unix_ms <= 0)
                {
                    return Err(WorkspaceProtocolError::InvalidTimestamp);
                }
                Ok(())
            }
            Self::CancelAgentExecution { attempt_id, reason } => {
                if attempt_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("attempt_id"));
                }
                require_text("execution cancellation reason", reason)
            }
            Self::PauseAgentExecution { attempt_id, reason } => {
                if attempt_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("attempt_id"));
                }
                require_text("execution pause reason", reason)
            }
            Self::PrepareAgentExecutionRetry {
                failed_attempt_id,
                lease_id,
                lease_expires_at_unix_ms,
            } => {
                if failed_attempt_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("failed_attempt_id"));
                }
                if lease_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("lease_id"));
                }
                if *lease_expires_at_unix_ms <= 0 {
                    return Err(WorkspaceProtocolError::InvalidTimestamp);
                }
                Ok(())
            }
            Self::ResolveAgentExecutionReview { attempt_id, .. } => {
                if attempt_id.0.is_nil() {
                    return Err(WorkspaceProtocolError::NilIdentifier("attempt_id"));
                }
                Ok(())
            }
            Self::TransitionWork { .. }
            | Self::TransitionPlanTask { .. }
            | Self::UpsertConstraint { .. }
            | Self::UpsertRisk { .. } => Ok(()),
        }
    }
}

fn validate_plan_revision(
    plan: &Plan,
    work_contracts: &[WorkContract],
) -> Result<(), WorkspaceProtocolError> {
    plan.validate()
        .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
    if plan.status != dragent_workspace_model::PlanStatus::Draft
        || plan.approved_by.is_some()
        || plan.approved_at_unix_ms.is_some()
    {
        return Err(WorkspaceProtocolError::InvalidPayload(
            "a plan revision must return to an unapproved draft".into(),
        ));
    }
    if plan.tasks.is_empty() || work_contracts.is_empty() {
        return Err(WorkspaceProtocolError::InvalidPayload(
            "a plan revision requires tasks and role contracts".into(),
        ));
    }

    let mut contracts = HashMap::with_capacity(work_contracts.len());
    for contract in work_contracts {
        contract
            .validate()
            .map_err(|error| WorkspaceProtocolError::InvalidPayload(error.to_string()))?;
        if contract.work_item_id != plan.work_item_id
            || contract.status != WorkContractStatus::Proposed
            || contract.agent_instance_id.is_some()
        {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "revised role contracts must be proposed, non-runtime contracts for the plan work item"
                    .into(),
            ));
        }
        if contracts.insert(contract.id, contract).is_some() {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "a plan revision contains a duplicate role contract".into(),
            ));
        }
    }

    let mut expected_tasks = HashMap::<WorkContractId, HashSet<PlanTaskId>>::new();
    let mut expected_criteria =
        HashMap::<WorkContractId, HashSet<dragent_workspace_model::AcceptanceCriterionId>>::new();
    for task in &plan.tasks {
        if task.status != PlanTaskStatus::Proposed {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "edited plan tasks must return to proposed".into(),
            ));
        }
        let contract_id = task.assigned_contract_id.ok_or_else(|| {
            WorkspaceProtocolError::InvalidPayload(
                "every edited plan task requires an assigned role contract".into(),
            )
        })?;
        if !contracts.contains_key(&contract_id) {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "a plan task references a contract outside the revision".into(),
            ));
        }
        expected_tasks
            .entry(contract_id)
            .or_default()
            .insert(task.id);
        expected_criteria
            .entry(contract_id)
            .or_default()
            .extend(task.acceptance_criteria.iter().copied());
    }

    for contract in work_contracts {
        let task_ids = contract.task_ids.iter().copied().collect::<HashSet<_>>();
        let criterion_ids = contract
            .success_criteria
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if task_ids.len() != contract.task_ids.len()
            || criterion_ids.len() != contract.success_criteria.len()
            || task_ids != expected_tasks.remove(&contract.id).unwrap_or_default()
            || criterion_ids != expected_criteria.remove(&contract.id).unwrap_or_default()
        {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "role contract task and criterion links must exactly match the revised plan".into(),
            ));
        }
    }
    Ok(())
}
