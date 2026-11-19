//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agentic_attention::require_non_empty;
use crate::is_safe_relative_path;
use crate::DomainValidationError;
use crate::ExecutionLocation;
use crate::{
    ActorId, AgentInstanceId, BillingOperationId, ExecutionAttemptId, ExecutionIsolationId,
    ExecutionLeaseId, PlanTaskId, ProjectId, RoleDefinitionId, WorkContractId, WorkItemId,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentInstanceStatus {
    #[default]
    Pending,
    Ready,
    Running,
    Waiting,
    Blocked,
    Reviewing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentInstance {
    pub id: AgentInstanceId,
    pub work_item_id: WorkItemId,
    pub role_definition_id: RoleDefinitionId,
    pub contract_id: WorkContractId,
    pub status: AgentInstanceStatus,
    pub execution_location: ExecutionLocation,
    pub model_route: String,
    pub active_lease_id: Option<ExecutionLeaseId>,
    pub started_at_unix_ms: Option<i64>,
    pub updated_at_unix_ms: i64,
}

impl AgentInstance {
    /// Validate durable runtime identity independently from process launch.
    /// A Pending instance is provisioned metadata; it has not started a model
    /// turn, acquired a lease, or received tool authority.
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("agent model route", &self.model_route)?;
        if self.updated_at_unix_ms <= 0 {
            return Err(DomainValidationError::InvalidTimestamp(
                "agent instance updated_at_unix_ms",
            ));
        }
        if self.status == AgentInstanceStatus::Pending && self.active_lease_id.is_some() {
            return Err(DomainValidationError::ExecutionLeaseForbidden);
        }
        if matches!(
            self.status,
            AgentInstanceStatus::Ready
                | AgentInstanceStatus::Running
                | AgentInstanceStatus::Waiting
                | AgentInstanceStatus::Blocked
                | AgentInstanceStatus::Reviewing
        ) && self.active_lease_id.is_none()
        {
            return Err(DomainValidationError::ExecutionLeaseRequired);
        }
        if self.status == AgentInstanceStatus::Pending && self.started_at_unix_ms.is_some() {
            return Err(DomainValidationError::AgentStartTimestampForbidden);
        }
        if matches!(
            self.status,
            AgentInstanceStatus::Running
                | AgentInstanceStatus::Waiting
                | AgentInstanceStatus::Blocked
                | AgentInstanceStatus::Reviewing
                | AgentInstanceStatus::Completed
                | AgentInstanceStatus::Failed
        ) && self.started_at_unix_ms.is_none()
        {
            return Err(DomainValidationError::AgentStartTimestampRequired);
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ChargingMode {
    #[default]
    LocalCompute,
    BringYourOwnKey,
    PlatformCredits,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChargingAuthorization {
    pub owner_id: ActorId,
    pub mode: ChargingMode,
    /// Platform-credit execution cites its durable billing preflight hold.
    /// Local compute and BYOK never masquerade as platform-billed work.
    pub billing_operation_id: Option<BillingOperationId>,
}

impl ChargingAuthorization {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.owner_id.0.is_nil() {
            return Err(DomainValidationError::ChargingOwnerRequired);
        }
        match (self.mode, self.billing_operation_id) {
            // Pay-as-you-go platform billing: the provider enforces balance
            // per call (402 on exhaustion → execution failure → attention),
            // and leases plus contract budgets bound exposure. A preflight
            // hold is recorded when cloud authority issues one, but its
            // absence must not block execution — shaping already spends
            // platform credits the same way.
            (ChargingMode::PlatformCredits, _) => Ok(()),
            (ChargingMode::LocalCompute | ChargingMode::BringYourOwnKey, Some(_)) => {
                Err(DomainValidationError::BillingOperationForbidden)
            }
            _ => Ok(()),
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionLeaseStatus {
    #[default]
    Active,
    Released,
    Expired,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecutionLease {
    pub id: ExecutionLeaseId,
    pub work_item_id: WorkItemId,
    pub agent_instance_id: AgentInstanceId,
    pub contract_id: WorkContractId,
    pub status: ExecutionLeaseStatus,
    pub charging: ChargingAuthorization,
    pub acquired_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl ExecutionLease {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        self.charging.validate()?;
        if self.acquired_at_unix_ms <= 0
            || self.updated_at_unix_ms <= 0
            || self.expires_at_unix_ms <= self.acquired_at_unix_ms
        {
            return Err(DomainValidationError::InvalidExecutionLeaseWindow);
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttemptStatus {
    #[default]
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionReviewResolution {
    Accepted,
    Integrated,
    RevisionRequested,
    Discarded,
}

/// Canonical identity of the filesystem namespace assigned to one execution.
/// Absolute paths deliberately stay in the host runtime: this descriptor is
/// safe to persist and synchronize across desktop and cloud authorities.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionIsolationKind {
    /// Read-only agents may inspect the shared project checkout.
    #[default]
    SharedProject,
    /// Writable agents execute in a detached Git worktree.
    GitWorktree,
    /// A read-only reviewer is attached to one unresolved writable attempt's
    /// checkout. The worker remains its sole owner.
    ReviewCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecutionIsolation {
    pub id: ExecutionIsolationId,
    pub kind: ExecutionIsolationKind,
    /// Immutable Git commit from which this execution's changes are measured.
    /// Required for Git worktrees and absent for a shared read-only checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<String>,
    /// Present only for `review_candidate`; points to the exact immutable
    /// attempt identity whose candidate filesystem is being reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_attempt_id: Option<ExecutionAttemptId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionFileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecutionFileChange {
    pub relative_path: PathBuf,
    pub kind: ExecutionFileChangeKind,
    pub added_lines: Option<u64>,
    pub deleted_lines: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecutionChangeSummary {
    pub isolation_id: ExecutionIsolationId,
    pub base_revision: String,
    pub files: Vec<ExecutionFileChange>,
    pub captured_at_unix_ms: i64,
}

impl ExecutionChangeSummary {
    pub fn validate(&self, isolation: &ExecutionIsolation) -> Result<(), DomainValidationError> {
        if isolation.kind != ExecutionIsolationKind::GitWorktree
            || self.isolation_id != isolation.id
            || isolation.base_revision.as_deref() != Some(self.base_revision.as_str())
            || self.captured_at_unix_ms <= 0
            || self.base_revision.trim().is_empty()
        {
            return Err(DomainValidationError::InvalidExecutionChangeSummary);
        }
        let mut paths = HashSet::new();
        for file in &self.files {
            if !is_safe_relative_path(&file.relative_path)
                || !paths.insert(file.relative_path.clone())
            {
                return Err(DomainValidationError::InvalidExecutionChangeSummary);
            }
        }
        Ok(())
    }
}

impl ExecutionIsolation {
    pub fn shared(project_id: ProjectId) -> Self {
        Self {
            id: ExecutionIsolationId(Uuid::new_v5(&project_id.0, b"isolation:shared-project")),
            kind: ExecutionIsolationKind::SharedProject,
            base_revision: None,
            source_attempt_id: None,
        }
    }

    pub fn git_worktree(attempt_id: ExecutionAttemptId, base_revision: String) -> Self {
        Self {
            id: ExecutionIsolationId(Uuid::new_v5(&attempt_id.0, b"isolation:git-worktree")),
            kind: ExecutionIsolationKind::GitWorktree,
            base_revision: Some(base_revision),
            source_attempt_id: None,
        }
    }

    pub fn review_candidate(
        attempt_id: ExecutionAttemptId,
        source_attempt_id: ExecutionAttemptId,
        base_revision: String,
    ) -> Self {
        Self {
            id: ExecutionIsolationId(Uuid::new_v5(&attempt_id.0, b"isolation:review-candidate")),
            kind: ExecutionIsolationKind::ReviewCandidate,
            base_revision: Some(base_revision),
            source_attempt_id: Some(source_attempt_id),
        }
    }

    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.id.0.is_nil() {
            return Err(DomainValidationError::MissingExecutionIsolation);
        }
        match (
            self.kind,
            self.base_revision.as_deref(),
            self.source_attempt_id,
        ) {
            (ExecutionIsolationKind::SharedProject, None, None) => Ok(()),
            (ExecutionIsolationKind::GitWorktree, Some(revision), None)
                if !revision.trim().is_empty()
                    && revision.len() <= 128
                    && revision
                        .chars()
                        .all(|character| character.is_ascii_hexdigit()) =>
            {
                Ok(())
            }
            (ExecutionIsolationKind::ReviewCandidate, Some(revision), Some(source_attempt_id))
                if !source_attempt_id.0.is_nil()
                    && !revision.trim().is_empty()
                    && revision.len() <= 128
                    && revision
                        .chars()
                        .all(|character| character.is_ascii_hexdigit()) =>
            {
                Ok(())
            }
            _ => Err(DomainValidationError::InvalidExecutionIsolation),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecutionAttempt {
    pub id: ExecutionAttemptId,
    pub work_item_id: WorkItemId,
    pub task_id: PlanTaskId,
    pub agent_instance_id: AgentInstanceId,
    pub contract_id: WorkContractId,
    pub lease_id: ExecutionLeaseId,
    pub attempt_number: u32,
    pub status: ExecutionAttemptStatus,
    pub model_route: String,
    /// `None` is accepted only while replaying legacy events. New execution
    /// starts are required by authority to provide a validated descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<ExecutionIsolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_summary: Option<ExecutionChangeSummary>,
    /// Durable outcome of the review boundary. This prevents resolved
    /// worktrees from reappearing after event replay or application restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_resolution: Option<ExecutionReviewResolution>,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
    pub updated_at_unix_ms: i64,
}

impl ExecutionAttempt {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("execution attempt model route", &self.model_route)?;
        if let Some(isolation) = &self.isolation {
            isolation.validate()?;
        }
        match (&self.change_summary, &self.isolation) {
            (Some(summary), Some(isolation)) => summary.validate(isolation)?,
            (Some(_), None) => return Err(DomainValidationError::InvalidExecutionChangeSummary),
            (None, _) => {}
        }
        if self.status == ExecutionAttemptStatus::Running && self.change_summary.is_some() {
            return Err(DomainValidationError::InvalidExecutionChangeSummary);
        }
        if self.review_resolution.is_some() && self.status != ExecutionAttemptStatus::Succeeded {
            return Err(DomainValidationError::InvalidExecutionReviewResolution);
        }
        if matches!(
            self.review_resolution,
            Some(ExecutionReviewResolution::Integrated)
        ) && self.change_summary.is_none()
        {
            return Err(DomainValidationError::InvalidExecutionReviewResolution);
        }
        if self.attempt_number == 0 {
            return Err(DomainValidationError::ZeroExecutionAttempt);
        }
        if self.started_at_unix_ms <= 0 || self.updated_at_unix_ms <= 0 {
            return Err(DomainValidationError::InvalidTimestamp(
                "execution attempt timestamp",
            ));
        }
        match (self.status, self.finished_at_unix_ms) {
            (ExecutionAttemptStatus::Running, None) => Ok(()),
            (ExecutionAttemptStatus::Running, Some(_)) => {
                Err(DomainValidationError::ExecutionAttemptFinishForbidden)
            }
            (_, Some(finished_at)) if finished_at >= self.started_at_unix_ms => Ok(()),
            _ => Err(DomainValidationError::ExecutionAttemptFinishRequired),
        }
    }
}
