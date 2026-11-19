//! Small, UI-agnostic domain types for Dragent's engineering workspace.
//!
//! This crate intentionally contains no filesystem, database, model, or UI
//! code. It is the stable vocabulary shared by persistence, the Rust workspace
//! shell, and the future orchestration event stream.

use std::path::PathBuf;

use crate::is_safe_relative_path;
use crate::AcceptanceCriterion;
use crate::AgentInstance;
use crate::AgentMessage;
use crate::AgentMessageRouting;
use crate::ArtifactRef;
use crate::AttentionItem;
use crate::Decision;
use crate::DomainValidationError;
use crate::Evidence;
use crate::ExecutionAttempt;
use crate::ExecutionLease;
use crate::ExecutionReviewResolution;
use crate::Plan;
use crate::ProjectId;
use crate::ProposedTeam;
use crate::Requirement;
use crate::RoleDefinition;
use crate::ShapingMemoryNote;
use crate::ShapingRecords;
use crate::WorkBrief;
use crate::WorkContract;
use crate::WorkItemId;
use crate::WorkStatus;
use serde::{Deserialize, Serialize};

/// A deliberately conservative bound for in-memory semantic history. Durable
/// persistence owns the complete audit trail; the UI only needs a recent tail.

impl ArtifactRef {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if !is_safe_relative_path(&self.relative_path) {
            return Err(DomainValidationError::InvalidArtifactPath {
                path: self.relative_path.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEventKind {
    WorkCreated {
        title: String,
    },
    WorkStatusChanged {
        status: WorkStatus,
    },
    WorkBriefUpdated {
        work_brief: WorkBrief,
    },
    RequirementAdded {
        summary: String,
    },
    DecisionRequired {
        summary: String,
    },
    DecisionRecorded {
        summary: String,
    },
    PlanUpdated {
        summary: String,
    },
    PlanApproved {
        summary: String,
    },
    UserMessagePosted {
        body: String,
    },
    PrincipalMessagePosted {
        body: String,
    },
    AgentMessagePosted {
        message: AgentMessage,
        routing: AgentMessageRouting,
    },
    AgentMessageDeliveryUpdated {
        message: AgentMessage,
        routing: AgentMessageRouting,
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
    RequirementUpdated {
        requirement: Requirement,
    },
    DecisionUpdated {
        decision: Decision,
    },
    ShapingMemoryRecorded {
        note: ShapingMemoryNote,
    },
    AcceptanceCriterionUpdated {
        acceptance_criterion: AcceptanceCriterion,
    },
    EvidenceRecorded {
        evidence: Evidence,
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
        resolution: ExecutionReviewResolution,
    },
    AttentionUpdated {
        attention_item: AttentionItem,
    },
    ProposedTeamUpdated {
        proposed_team: ProposedTeam,
    },
    StructuredPlanUpdated {
        plan: Plan,
    },
    FileChanged {
        relative_path: PathBuf,
    },
    CommandStarted {
        summary: String,
    },
    CommandCompleted {
        summary: String,
        success: bool,
    },
    TestCompleted {
        summary: String,
        success: bool,
    },
    ReviewFinding {
        summary: String,
    },
    PermissionRequired {
        summary: String,
    },
    CheckpointCreated {
        summary: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceEvent {
    pub sequence: u64,
    pub timestamp_unix_ms: i64,
    pub project_id: ProjectId,
    pub work_item_id: Option<WorkItemId>,
    pub kind: WorkspaceEventKind,
}

/// Latest structured result of a Principal shaping turn. This is a bounded
/// desktop projection record, not executable authority: it can describe a
/// plan, but it cannot create agents, grant permissions, or start tasks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShapingProposal {
    pub revision: u64,
    pub outcome: String,
    pub requirements: Vec<String>,
    pub open_decisions: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub plan_summary: String,
}

impl ShapingProposal {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.revision == 0 {
            return Err("shaping proposal revision must be positive");
        }
        if self.outcome.trim().is_empty() {
            return Err("shaping proposal outcome must not be empty");
        }
        if self.plan_summary.trim().is_empty() {
            return Err("shaping proposal plan summary must not be empty");
        }
        if self
            .requirements
            .iter()
            .chain(self.open_decisions.iter())
            .chain(self.acceptance_criteria.iter())
            .any(|item| item.trim().is_empty())
        {
            return Err("shaping proposal list items must not be empty");
        }
        Ok(())
    }
}
