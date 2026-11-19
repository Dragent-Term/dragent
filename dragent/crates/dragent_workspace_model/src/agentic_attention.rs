//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ExecutionLocation;
use crate::{
    ActorId, AttentionItemId, CheckpointId, ExecutionAttemptId, PlanTaskId, ThreadId, WorkItemId,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AttentionStatus {
    #[default]
    Open,
    Acknowledged,
    Resolved,
    Dismissed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Decision,
    Approval,
    Permission,
    Conflict,
    Budget,
    Failure,
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionResolution {
    Approved,
    Rejected,
    Answered,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AttentionItem {
    pub id: AttentionItemId,
    pub work_item_id: WorkItemId,
    pub task_id: Option<PlanTaskId>,
    pub thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_attempt_id: Option<ExecutionAttemptId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub kind: AttentionKind,
    pub status: AttentionStatus,
    pub summary: String,
    pub requested_by: ActorId,
    pub assigned_to: Vec<ActorId>,
    pub created_at_unix_ms: i64,
    pub resolved_at_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_kind: Option<AttentionResolution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

impl AttentionItem {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("attention summary", &self.summary)?;
        if self.created_at_unix_ms <= 0 {
            return Err(DomainValidationError::InvalidTimestamp(
                "attention created_at_unix_ms",
            ));
        }
        if self
            .tool_call_id
            .as_deref()
            .is_some_and(|tool_call_id| tool_call_id.trim().is_empty())
        {
            return Err(DomainValidationError::EmptyField("tool_call_id"));
        }
        let is_terminal = matches!(
            self.status,
            AttentionStatus::Resolved | AttentionStatus::Dismissed
        );
        if is_terminal {
            let resolved_at = self
                .resolved_at_unix_ms
                .ok_or(DomainValidationError::ResolutionRequired)?;
            if resolved_at < self.created_at_unix_ms {
                return Err(DomainValidationError::InvalidTimestamp(
                    "attention resolved_at_unix_ms",
                ));
            }
            let resolution_kind = self
                .resolution_kind
                .ok_or(DomainValidationError::ResolutionRequired)?;
            require_non_empty(
                "attention resolution",
                self.resolution
                    .as_deref()
                    .ok_or(DomainValidationError::ResolutionRequired)?,
            )?;
            if self.kind == AttentionKind::Permission
                && !matches!(
                    resolution_kind,
                    AttentionResolution::Approved | AttentionResolution::Rejected
                )
            {
                return Err(DomainValidationError::InvalidAttentionResolution);
            }
            if (self.status == AttentionStatus::Dismissed)
                != (resolution_kind == AttentionResolution::Dismissed)
            {
                return Err(DomainValidationError::InvalidAttentionResolution);
            }
        } else if self.resolved_at_unix_ms.is_some()
            || self.resolution_kind.is_some()
            || self.resolution.is_some()
        {
            return Err(DomainValidationError::ResolutionForbidden);
        }
        if self.kind == AttentionKind::Permission
            && (self.execution_attempt_id.is_none() || self.tool_call_id.is_none())
        {
            return Err(DomainValidationError::AttentionExecutionRequired);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub work_item_id: WorkItemId,
    pub task_id: Option<PlanTaskId>,
    pub execution_location: ExecutionLocation,
    pub revision: u64,
    pub summary: String,
    pub created_by: ActorId,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomainValidationError {
    EmptyField(&'static str),
    ZeroVersion(&'static str),
    ApprovalAttributionRequired,
    EvidenceRequired,
    InvalidEvidence,
    WaiverAttributionRequired,
    ResolutionRequired,
    ResolutionForbidden,
    InvalidMemoryNote,
    AttentionExecutionRequired,
    InvalidAttentionResolution,
    SuccessCriteriaRequired,
    ProposedTeamRequired,
    RuntimeAgentRequired,
    RuntimeAgentForbidden,
    AgentStartTimestampRequired,
    AgentStartTimestampForbidden,
    ExecutionLeaseRequired,
    ExecutionLeaseForbidden,
    ChargingOwnerRequired,
    BillingOperationForbidden,
    InvalidExecutionLeaseWindow,
    MissingExecutionIsolation,
    InvalidExecutionIsolation,
    InvalidExecutionChangeSummary,
    InvalidExecutionReviewResolution,
    ZeroExecutionAttempt,
    ExecutionAttemptFinishForbidden,
    ExecutionAttemptFinishRequired,
    InvalidTimestamp(&'static str),
    DuplicateIdentity(&'static str),
    DifferentWorkItem(&'static str),
    MissingProvenance(&'static str),
    InvalidModelRoute,
    InvalidPermissionGrant,
    InvalidArtifactPath {
        path: PathBuf,
    },
    InvalidWriteScope {
        path: PathBuf,
    },
    DuplicatePlanTask {
        task_id: PlanTaskId,
    },
    PlanTaskBelongsToDifferentPlan {
        task_id: PlanTaskId,
    },
    MissingPlanDependency {
        task_id: PlanTaskId,
        dependency_id: PlanTaskId,
    },
    SelfDependency {
        task_id: PlanTaskId,
    },
    CyclicPlanDependency {
        task_id: PlanTaskId,
    },
}

impl std::fmt::Display for DomainValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}", self)
    }
}

impl std::error::Error for DomainValidationError {}

pub(crate) fn require_non_empty(
    field: &'static str,
    value: &str,
) -> Result<(), DomainValidationError> {
    if value.trim().is_empty() {
        Err(DomainValidationError::EmptyField(field))
    } else {
        Ok(())
    }
}

pub(crate) fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
}
