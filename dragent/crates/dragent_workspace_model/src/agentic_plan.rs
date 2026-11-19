//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::agentic_attention::require_non_empty;
use crate::ActorKind;
use crate::DomainValidationError;
use crate::Provenance;
use crate::StructuredRecordStatus;
use crate::{
    AcceptanceCriterionId, ActorId, ConstraintId, OrganizationId, PlanId, PlanTaskId, ProjectId,
    RiskId, RoleDefinitionId, WorkContractId, WorkItemId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Constraint {
    pub id: ConstraintId,
    pub work_item_id: WorkItemId,
    pub description: String,
    pub status: StructuredRecordStatus,
    pub provenance: Vec<Provenance>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RiskStatus {
    #[default]
    Open,
    Mitigated,
    Accepted,
    Materialized,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Risk {
    pub id: RiskId,
    pub work_item_id: WorkItemId,
    pub summary: String,
    pub mitigation: Option<String>,
    pub severity: RiskSeverity,
    pub status: RiskStatus,
    pub provenance: Vec<Provenance>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    #[default]
    Draft,
    Approved,
    Superseded,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanTaskStatus {
    #[default]
    Proposed,
    Ready,
    Running,
    Blocked,
    Reviewing,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanTask {
    pub id: PlanTaskId,
    pub plan_id: PlanId,
    pub title: String,
    pub description: String,
    pub status: PlanTaskStatus,
    pub depends_on: Vec<PlanTaskId>,
    /// For an independent review task, identifies the dependency whose
    /// unresolved candidate checkout must be inspected. This is explicit so
    /// scheduling never guesses from titles, role names, or dependency order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_target_task_id: Option<PlanTaskId>,
    pub acceptance_criteria: Vec<AcceptanceCriterionId>,
    pub assigned_contract_id: Option<WorkContractId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Plan {
    pub id: PlanId,
    pub work_item_id: WorkItemId,
    pub version: u64,
    pub status: PlanStatus,
    pub tasks: Vec<PlanTask>,
    pub approved_by: Option<ActorId>,
    pub approved_at_unix_ms: Option<i64>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl Plan {
    /// Validate identity, references, and acyclicity without relying on a UI or
    /// scheduler. Task order in the vector is presentation-only.
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.version == 0 {
            return Err(DomainValidationError::ZeroVersion("plan"));
        }
        if self.status == PlanStatus::Approved
            && (self.approved_by.is_none() || self.approved_at_unix_ms.is_none())
        {
            return Err(DomainValidationError::ApprovalAttributionRequired);
        }

        let mut task_indices = HashMap::with_capacity(self.tasks.len());
        for (index, task) in self.tasks.iter().enumerate() {
            require_non_empty("plan task title", &task.title)?;
            if task.plan_id != self.id {
                return Err(DomainValidationError::PlanTaskBelongsToDifferentPlan {
                    task_id: task.id,
                });
            }
            if task_indices.insert(task.id, index).is_some() {
                return Err(DomainValidationError::DuplicatePlanTask { task_id: task.id });
            }
        }

        for task in &self.tasks {
            if let Some(review_target) = task.review_target_task_id {
                if !task.depends_on.contains(&review_target) {
                    return Err(DomainValidationError::MissingPlanDependency {
                        task_id: task.id,
                        dependency_id: review_target,
                    });
                }
            }
            for dependency in &task.depends_on {
                if *dependency == task.id {
                    return Err(DomainValidationError::SelfDependency { task_id: task.id });
                }
                if !task_indices.contains_key(dependency) {
                    return Err(DomainValidationError::MissingPlanDependency {
                        task_id: task.id,
                        dependency_id: *dependency,
                    });
                }
            }
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        for task in &self.tasks {
            visit_plan_task(
                task.id,
                &self.tasks,
                &task_indices,
                &mut visiting,
                &mut visited,
            )?;
        }
        Ok(())
    }
}

fn visit_plan_task(
    task_id: PlanTaskId,
    tasks: &[PlanTask],
    task_indices: &HashMap<PlanTaskId, usize>,
    visiting: &mut HashSet<PlanTaskId>,
    visited: &mut HashSet<PlanTaskId>,
) -> Result<(), DomainValidationError> {
    if visited.contains(&task_id) {
        return Ok(());
    }
    if !visiting.insert(task_id) {
        return Err(DomainValidationError::CyclicPlanDependency { task_id });
    }
    let task = &tasks[task_indices[&task_id]];
    for dependency in &task.depends_on {
        visit_plan_task(*dependency, tasks, task_indices, visiting, visited)?;
    }
    visiting.remove(&task_id);
    visited.insert(task_id);
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum RoleDefinitionScope {
    BuiltIn,
    Project(ProjectId),
    Organization(OrganizationId),
}

/// Semantic responsibility of a role. This is explicit rather than inferred
/// from a display name so authority, routing, and model prompts agree on who
/// may produce independent review findings.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentRoleKind {
    #[default]
    Worker,
    Reviewer,
}

impl AgentRoleKind {
    pub fn is_worker(value: &Self) -> bool {
        *value == Self::Worker
    }

    pub fn actor_kind(self) -> ActorKind {
        match self {
            Self::Worker => ActorKind::WorkerAgent,
            Self::Reviewer => ActorKind::ReviewerAgent,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CapabilityRequirement {
    pub capability: String,
    pub reason: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoleDefinition {
    pub id: RoleDefinitionId,
    pub scope: RoleDefinitionScope,
    #[serde(default, skip_serializing_if = "AgentRoleKind::is_worker")]
    pub kind: AgentRoleKind,
    pub name: String,
    pub mission: String,
    pub capabilities: Vec<CapabilityRequirement>,
    pub preferred_tools: Vec<String>,
    pub created_by: ActorId,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl RoleDefinition {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("role name", &self.name)?;
        require_non_empty("role mission", &self.mission)?;
        for capability in &self.capabilities {
            require_non_empty("role capability", &capability.capability)?;
            require_non_empty("role capability reason", &capability.reason)?;
        }
        Ok(())
    }
}
