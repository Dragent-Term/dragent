//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agentic_attention::require_non_empty;
use crate::DomainValidationError;
use crate::ExecutionAttempt;
use crate::ExecutionAttemptStatus;
use crate::ExecutionIsolationKind;
use crate::ExecutionReviewResolution;
use crate::PlanTask;
use crate::PlanTaskStatus;
use crate::WorkContract;
use crate::WorkContractStatus;
use crate::{
    ActorId, AgentMessageId, ArtifactId, EvidenceId, ExecutionAttemptId, PlanTaskId, ThreadId,
    WorkItemId,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    #[default]
    Work,
    Task,
    Handoff,
    Attention,
    Review,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommunicationThread {
    pub id: ThreadId,
    pub work_item_id: WorkItemId,
    pub task_id: Option<PlanTaskId>,
    pub kind: ThreadKind,
    pub title: String,
    pub participant_ids: Vec<ActorId>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl CommunicationThread {
    /// One stable shaping conversation per work item. Reconstructing this
    /// value after restart yields the same identity instead of creating a
    /// disconnected thread for every application session.
    pub fn principal(
        work_item_id: WorkItemId,
        local_user_id: ActorId,
        principal_actor_id: ActorId,
        now_unix_ms: i64,
    ) -> Self {
        Self {
            id: ThreadId(Uuid::new_v5(&work_item_id.0, b"thread:principal-shaping")),
            work_item_id,
            task_id: None,
            kind: ThreadKind::Work,
            title: "Shape with Principal".into(),
            participant_ids: vec![local_user_id, principal_actor_id],
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        }
    }
}

/// One stable coordination conversation per task — every attempt's Blocker/
/// Handoff/ReviewFinding message, plus every escalation raised for it, share
/// this same identity so they render as one continuous thread instead of a
/// fresh one per attempt. The single source of truth for this id: every
/// producer (execution-completion messages, exhausted-retry escalations)
/// must call this rather than constructing the `Uuid::new_v5` inline, or a
/// second, silently divergent thread id becomes possible.
pub fn task_coordination_thread_id(task_id: PlanTaskId) -> ThreadId {
    ThreadId(Uuid::new_v5(&task_id.0, b"thread:task-coordination"))
}

/// One stable thread per work item for coordination-time messages that
/// aren't about any single task — a human's live status question ("which
/// agent is working right now?") while agents are running, and the
/// Principal's reply. Deliberately a different identity from
/// `CommunicationThread::principal()`, which is specifically the Shaping
/// conversation's thread (seeded `"thread:principal-shaping"`, titled "Shape
/// with Principal") — reusing that id here would fold live coordination
/// Q&A into Shaping's own history under the wrong title.
pub fn work_item_coordination_thread_id(work_item_id: WorkItemId) -> ThreadId {
    ThreadId(Uuid::new_v5(
        &work_item_id.0,
        b"thread:work-item-coordination",
    ))
}

/// Live summary of one communication thread, derived from `agent_messages()`
/// grouped by `thread_id` — computed on demand by
/// `WorkspaceState::communication_threads()`, never a separately-mutated
/// field. A plain cached field for exactly this kind of summary is the class
/// of bug `WorkspaceState::record()` already had once for `active_work`
/// silently going stale; this projection can't go stale because it has
/// nothing to go stale — it's recomputed from canonical messages every call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunicationThreadSummary {
    pub thread_id: ThreadId,
    /// The task this thread is scoped to, if any — the shaping thread and a
    /// human-only discussion thread have none.
    pub task_id: Option<PlanTaskId>,
    /// First-appearance order of every distinct sender in the thread.
    pub participant_ids: Vec<ActorId>,
    pub last_activity_unix_ms: i64,
    /// True when the thread's most recent Blocker/DecisionRequest from a
    /// non-Principal sender, routed to the Principal, has no later reply
    /// from the Principal in this same thread.
    pub is_open: bool,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageKind {
    #[default]
    Discussion,
    Handoff,
    Blocker,
    DecisionRequest,
    StatusUpdate,
    ReviewFinding,
    RevisionRequest,
    Approval,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryState {
    LocalOnly,
    #[default]
    Pending,
    Delivered,
    Read,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Pass,
    NeedsRevision,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFindingSeverity {
    Note,
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewFindingItem {
    pub severity: ReviewFindingSeverity,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StructuredReviewFinding {
    pub candidate_attempt_id: ExecutionAttemptId,
    pub verdict: ReviewVerdict,
    pub summary: String,
    pub findings: Vec<ReviewFindingItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentMessage {
    pub id: AgentMessageId,
    pub thread_id: ThreadId,
    pub work_item_id: WorkItemId,
    pub sender_id: ActorId,
    pub kind: AgentMessageKind,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_finding: Option<StructuredReviewFinding>,
    pub reply_to: Option<AgentMessageId>,
    pub artifact_ids: Vec<ArtifactId>,
    pub evidence_ids: Vec<EvidenceId>,
    pub delivery_state: MessageDeliveryState,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

/// Optional delivery and work targeting for operational agent communication.
/// Legacy shaping discussion remains valid without routing metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentMessageRouting {
    pub recipient_ids: Vec<ActorId>,
    pub task_id: Option<PlanTaskId>,
    pub execution_attempt_id: Option<ExecutionAttemptId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    FileChange,
    Command,
    Test,
    Review,
    Artifact,
    UserConfirmation,
    ExternalReference,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Evidence {
    pub id: EvidenceId,
    pub work_item_id: WorkItemId,
    pub task_id: Option<PlanTaskId>,
    pub kind: EvidenceKind,
    pub summary: String,
    pub source: String,
    pub produced_by: ActorId,
    pub created_at_unix_ms: i64,
}

/// Compatibility repair for the old policy that accepted a reviewer even when
/// it requested changes. Never use this to reopen a human acceptance or Pass.
pub fn legacy_review_needs_rearm(
    reviewer: &ExecutionAttempt,
    candidate: &ExecutionAttempt,
    contract: &WorkContract,
    task: &PlanTask,
    finding: &StructuredReviewFinding,
    principal: ActorId,
) -> bool {
    reviewer.status == ExecutionAttemptStatus::Succeeded
        && reviewer.review_resolution == Some(ExecutionReviewResolution::Accepted)
        && candidate.review_resolution == Some(ExecutionReviewResolution::RevisionRequested)
        && reviewer.isolation.as_ref().is_some_and(|root| {
            root.kind == ExecutionIsolationKind::ReviewCandidate
                && root.source_attempt_id == Some(candidate.id)
        })
        && finding.candidate_attempt_id == candidate.id
        && finding.verdict == ReviewVerdict::NeedsRevision
        && contract.updated_by == principal
        && contract.write_scope.is_empty()
        && contract.status == WorkContractStatus::Completed
        && task.id == reviewer.task_id
        && task.status == PlanTaskStatus::Completed
}

impl Evidence {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.id.0.is_nil()
            || self.work_item_id.0.is_nil()
            || self.produced_by.0.is_nil()
            || self.task_id.is_some_and(|task_id| task_id.0.is_nil())
            || self.created_at_unix_ms <= 0
        {
            return Err(DomainValidationError::InvalidEvidence);
        }
        require_non_empty("evidence summary", &self.summary)?;
        require_non_empty("evidence source", &self.source)?;
        Ok(())
    }
}
