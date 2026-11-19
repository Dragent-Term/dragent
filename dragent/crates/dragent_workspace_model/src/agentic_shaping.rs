//! Durable vocabulary for shaped work, teams, communication, and execution.
//!
//! These are UI- and transport-independent records. They intentionally carry
//! globally unique identities and millisecond timestamps so the desktop, cloud
//! service, and future companion clients can project the same work state.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agentic_attention::require_non_empty;
use crate::DomainValidationError;
use crate::Provenance;
use crate::ProvenanceSource;
use crate::StructuredRecordStatus;
use crate::{
    AcceptanceCriterionId, ActorId, AgentMessageId, DecisionId, DocumentRef, EvidenceId,
    RequirementId, ShapingMemoryNoteId, ShapingProposal, ThreadId, WorkBriefId, WorkItemId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkBrief {
    pub id: WorkBriefId,
    pub work_item_id: WorkItemId,
    pub version: u64,
    pub outcome: String,
    pub summary: String,
    pub status: StructuredRecordStatus,
    pub created_by: ActorId,
    pub updated_by: ActorId,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl WorkBrief {
    pub fn from_shaping_proposal(
        work_item_id: WorkItemId,
        proposal: &ShapingProposal,
        created_by: ActorId,
        now_unix_ms: i64,
    ) -> Self {
        Self {
            id: WorkBriefId(Uuid::new_v5(&work_item_id.0, b"work-brief")),
            work_item_id,
            version: proposal.revision.max(1),
            outcome: proposal.outcome.clone(),
            summary: proposal.plan_summary.clone(),
            status: StructuredRecordStatus::Draft,
            created_by,
            updated_by: created_by,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        }
    }

    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("work brief outcome", &self.outcome)?;
        if self.version == 0 {
            return Err(DomainValidationError::ZeroVersion("work brief"));
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RequirementPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Requirement {
    pub id: RequirementId,
    pub work_item_id: WorkItemId,
    pub summary: String,
    pub details: String,
    pub priority: RequirementPriority,
    pub status: StructuredRecordStatus,
    pub provenance: Vec<Provenance>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl Requirement {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("requirement summary", &self.summary)?;
        require_provenance("requirement", &self.provenance)
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    #[default]
    Open,
    Resolved,
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Decision {
    pub id: DecisionId,
    pub work_item_id: WorkItemId,
    pub question: String,
    pub resolution: Option<String>,
    pub rationale: Option<String>,
    pub status: DecisionStatus,
    pub provenance: Vec<Provenance>,
    pub decided_by: Option<ActorId>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl Decision {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("decision question", &self.question)?;
        if self.status == DecisionStatus::Resolved
            && self
                .resolution
                .as_deref()
                .is_none_or(|resolution| resolution.trim().is_empty())
        {
            return Err(DomainValidationError::ResolutionRequired);
        }
        require_provenance("decision", &self.provenance)
    }
}

/// Bound for the Principal's durable work-item brain. The canonical log owns
/// the complete audit trail; the live prompt only needs a recent tail.
pub const MAX_SHAPING_MEMORY_NOTES: usize = 50;
/// Hard cap for a single memory fact — notes are one sentence by contract.
pub const MAX_SHAPING_MEMORY_TEXT_CHARS: usize = 500;
/// Hard cap for one recording batch (a turn emits at most 10 by contract).
pub const MAX_SHAPING_MEMORY_BATCH: usize = 20;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ShapingMemoryKind {
    Decision,
    Approval,
    Preference,
    Change,
    Rejected,
    #[default]
    Note,
}

impl ShapingMemoryKind {
    pub fn parse(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "decision" => Self::Decision,
            "approval" => Self::Approval,
            "preference" => Self::Preference,
            "change" => Self::Change,
            "rejected" => Self::Rejected,
            _ => Self::Note,
        }
    }
}

/// One durable fact in the Principal's work-item brain: a decision reached,
/// an approval granted, a user preference, a revision change with its reason,
/// or an option considered and rejected with cause. Append-only — entries are
/// never edited or removed, only rotated out past the bound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShapingMemoryNote {
    pub id: ShapingMemoryNoteId,
    pub work_item_id: WorkItemId,
    pub kind: ShapingMemoryKind,
    pub text: String,
    pub recorded_at_unix_ms: i64,
}

impl ShapingMemoryNote {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.id.0.is_nil() || self.work_item_id.0.is_nil() || self.recorded_at_unix_ms <= 0 {
            return Err(DomainValidationError::InvalidMemoryNote);
        }
        require_non_empty("memory note text", &self.text)?;
        if self.text.chars().count() > MAX_SHAPING_MEMORY_TEXT_CHARS {
            return Err(DomainValidationError::InvalidMemoryNote);
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceStatus {
    #[default]
    Pending,
    Satisfied,
    Failed,
    Waived,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AcceptanceCriterion {
    pub id: AcceptanceCriterionId,
    pub work_item_id: WorkItemId,
    pub description: String,
    pub status: AcceptanceStatus,
    pub evidence_ids: Vec<EvidenceId>,
    pub waiver_reason: Option<String>,
    pub waived_by: Option<ActorId>,
    pub provenance: Vec<Provenance>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl AcceptanceCriterion {
    pub fn validate(&self) -> Result<(), DomainValidationError> {
        require_non_empty("acceptance criterion", &self.description)?;
        if self.status == AcceptanceStatus::Satisfied && self.evidence_ids.is_empty() {
            return Err(DomainValidationError::EvidenceRequired);
        }
        if self.status == AcceptanceStatus::Waived
            && (self.waived_by.is_none()
                || self
                    .waiver_reason
                    .as_deref()
                    .is_none_or(|reason| reason.trim().is_empty()))
        {
            return Err(DomainValidationError::WaiverAttributionRequired);
        }
        require_provenance("acceptance criterion", &self.provenance)
    }
}

/// Complete normalized shaping knowledge for one proposal revision. Records
/// retain stable identities when their normalized text is unchanged, making
/// them safe targets for later plans, contracts, assignments, and reviews.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShapingRecords {
    pub work_item_id: WorkItemId,
    pub proposal_revision: u64,
    pub requirements: Vec<Requirement>,
    pub decisions: Vec<Decision>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
}

impl ShapingRecords {
    pub fn from_proposal(
        work_item_id: WorkItemId,
        proposal: &ShapingProposal,
        source_thread_id: ThreadId,
        source_message_id: AgentMessageId,
        recorded_by: ActorId,
        now_unix_ms: i64,
    ) -> Self {
        let provenance = || Provenance {
            source: ProvenanceSource::AgentMessage {
                thread_id: source_thread_id,
                message_id: source_message_id,
            },
            recorded_by,
            recorded_at_unix_ms: now_unix_ms,
        };
        Self {
            work_item_id,
            proposal_revision: proposal.revision,
            requirements: proposal
                .requirements
                .iter()
                .map(|summary| Requirement {
                    id: RequirementId(stable_record_id(work_item_id, "requirement", summary)),
                    work_item_id,
                    summary: summary.clone(),
                    details: String::new(),
                    priority: RequirementPriority::Normal,
                    status: StructuredRecordStatus::Draft,
                    provenance: vec![provenance()],
                    created_at_unix_ms: now_unix_ms,
                    updated_at_unix_ms: now_unix_ms,
                })
                .collect(),
            decisions: proposal
                .open_decisions
                .iter()
                .map(|question| Decision {
                    id: DecisionId(stable_record_id(work_item_id, "decision", question)),
                    work_item_id,
                    question: question.clone(),
                    resolution: None,
                    rationale: None,
                    status: DecisionStatus::Open,
                    provenance: vec![provenance()],
                    decided_by: None,
                    created_at_unix_ms: now_unix_ms,
                    updated_at_unix_ms: now_unix_ms,
                })
                .collect(),
            acceptance_criteria: proposal
                .acceptance_criteria
                .iter()
                .map(|description| AcceptanceCriterion {
                    id: AcceptanceCriterionId(stable_record_id(
                        work_item_id,
                        "acceptance",
                        description,
                    )),
                    work_item_id,
                    description: description.clone(),
                    status: AcceptanceStatus::Pending,
                    evidence_ids: Vec::new(),
                    waiver_reason: None,
                    waived_by: None,
                    provenance: vec![provenance()],
                    created_at_unix_ms: now_unix_ms,
                    updated_at_unix_ms: now_unix_ms,
                })
                .collect(),
        }
    }

    /// Build the next complete shaping snapshot while retaining the identity
    /// and accumulated provenance of semantically unchanged records.
    pub fn supersede_from_proposal(
        previous: Option<&Self>,
        work_item_id: WorkItemId,
        proposal: &ShapingProposal,
        source_thread_id: ThreadId,
        source_message_id: AgentMessageId,
        recorded_by: ActorId,
        now_unix_ms: i64,
    ) -> Self {
        let mut next = Self::from_proposal(
            work_item_id,
            proposal,
            source_thread_id,
            source_message_id,
            recorded_by,
            now_unix_ms,
        );
        let Some(previous) = previous.filter(|records| records.work_item_id == work_item_id) else {
            return next;
        };
        let new_provenance = Provenance {
            source: ProvenanceSource::AgentMessage {
                thread_id: source_thread_id,
                message_id: source_message_id,
            },
            recorded_by,
            recorded_at_unix_ms: now_unix_ms,
        };
        for record in &mut next.requirements {
            if let Some(current) = previous.requirements.iter().find(|current| {
                normalized_record_text(&current.summary) == normalized_record_text(&record.summary)
            }) {
                record.id = current.id;
                record.details = current.details.clone();
                record.priority = current.priority;
                record.status = current.status;
                record.created_at_unix_ms = current.created_at_unix_ms;
                record.provenance = current.provenance.clone();
                if !record.provenance.contains(&new_provenance) {
                    record.provenance.push(new_provenance.clone());
                }
            }
        }
        for record in &mut next.decisions {
            if let Some(current) = previous.decisions.iter().find(|current| {
                normalized_record_text(&current.question)
                    == normalized_record_text(&record.question)
            }) {
                record.id = current.id;
                record.resolution = current.resolution.clone();
                record.rationale = current.rationale.clone();
                record.status = current.status;
                record.decided_by = current.decided_by;
                record.created_at_unix_ms = current.created_at_unix_ms;
                record.provenance = current.provenance.clone();
                if !record.provenance.contains(&new_provenance) {
                    record.provenance.push(new_provenance.clone());
                }
            }
        }
        for record in &mut next.acceptance_criteria {
            if let Some(current) = previous.acceptance_criteria.iter().find(|current| {
                normalized_record_text(&current.description)
                    == normalized_record_text(&record.description)
            }) {
                record.id = current.id;
                record.status = current.status;
                record.evidence_ids = current.evidence_ids.clone();
                record.waiver_reason = current.waiver_reason.clone();
                record.waived_by = current.waived_by;
                record.created_at_unix_ms = current.created_at_unix_ms;
                record.provenance = current.provenance.clone();
                if !record.provenance.contains(&new_provenance) {
                    record.provenance.push(new_provenance.clone());
                }
            }
        }
        next
    }

    /// Attribute the generated shaping snapshot to repository files that
    /// materially informed it. The Principal message remains causal
    /// provenance; these sources preserve the repository evidence used.
    pub fn add_document_provenance(
        &mut self,
        documents: &[DocumentRef],
        recorded_by: ActorId,
        recorded_at_unix_ms: i64,
    ) {
        let document_sources = documents
            .iter()
            .map(|document| Provenance {
                source: ProvenanceSource::Document {
                    document_id: document.id,
                    relative_path: document.relative_path.clone(),
                },
                recorded_by,
                recorded_at_unix_ms,
            })
            .collect::<Vec<_>>();
        for provenance in self
            .requirements
            .iter_mut()
            .map(|record| &mut record.provenance)
            .chain(
                self.decisions
                    .iter_mut()
                    .map(|record| &mut record.provenance),
            )
            .chain(
                self.acceptance_criteria
                    .iter_mut()
                    .map(|record| &mut record.provenance),
            )
        {
            for source in &document_sources {
                if !provenance.contains(source) {
                    provenance.push(source.clone());
                }
            }
        }
    }

    pub fn validate(&self) -> Result<(), DomainValidationError> {
        if self.proposal_revision == 0 {
            return Err(DomainValidationError::ZeroVersion("shaping records"));
        }
        let mut ids = HashSet::new();
        for requirement in &self.requirements {
            requirement.validate()?;
            validate_shaping_record(self.work_item_id, requirement.work_item_id)?;
            if !ids.insert(requirement.id.0) {
                return Err(DomainValidationError::DuplicateIdentity("shaping record"));
            }
        }
        for decision in &self.decisions {
            decision.validate()?;
            validate_shaping_record(self.work_item_id, decision.work_item_id)?;
            if !ids.insert(decision.id.0) {
                return Err(DomainValidationError::DuplicateIdentity("shaping record"));
            }
        }
        for criterion in &self.acceptance_criteria {
            criterion.validate()?;
            validate_shaping_record(self.work_item_id, criterion.work_item_id)?;
            if !ids.insert(criterion.id.0) {
                return Err(DomainValidationError::DuplicateIdentity("shaping record"));
            }
        }
        Ok(())
    }
}

fn normalized_record_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn stable_record_id(work_item_id: WorkItemId, kind: &str, text: &str) -> Uuid {
    let key = format!(
        "{kind}:{}",
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    );
    Uuid::new_v5(&work_item_id.0, key.as_bytes())
}

fn validate_shaping_record(
    expected_work_item_id: WorkItemId,
    actual_work_item_id: WorkItemId,
) -> Result<(), DomainValidationError> {
    if actual_work_item_id != expected_work_item_id {
        return Err(DomainValidationError::DifferentWorkItem("shaping record"));
    }
    Ok(())
}

fn require_provenance(
    record: &'static str,
    provenance: &[Provenance],
) -> Result<(), DomainValidationError> {
    if provenance.is_empty() {
        Err(DomainValidationError::MissingProvenance(record))
    } else {
        Ok(())
    }
}
