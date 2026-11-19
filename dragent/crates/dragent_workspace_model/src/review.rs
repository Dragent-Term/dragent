//! Deterministic definition-of-done and traceability projection.
//!
//! Review UI, local authority, cloud workers, and companion clients should all
//! explain completion from the same canonical records. This module contains no
//! renderer, filesystem access, model inference, or persistence policy.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    AcceptanceCriterion, AcceptanceCriterionId, AcceptanceStatus, AttentionItem, AttentionStatus,
    Evidence, EvidenceId, Plan, PlanTaskId, PlanTaskStatus, WorkItemId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletionRiskLevel {
    Clear,
    NeedsEvidence,
    Warning,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CriterionTrace {
    pub criterion_id: AcceptanceCriterionId,
    pub description: String,
    pub status: AcceptanceStatus,
    pub task_ids: Vec<PlanTaskId>,
    pub evidence_ids: Vec<EvidenceId>,
    pub evidence_summaries: Vec<String>,
    pub waiver_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CompletionReport {
    pub work_item_id: WorkItemId,
    pub completed_tasks: usize,
    pub total_tasks: usize,
    pub satisfied_criteria: usize,
    pub waived_criteria: usize,
    pub unresolved_criteria: usize,
    pub open_attention: usize,
    pub evidence_count: usize,
    pub risk: CompletionRiskLevel,
    pub ready_to_complete: bool,
    pub summary: String,
    pub criteria: Vec<CriterionTrace>,
}

pub fn build_completion_report(
    plan: &Plan,
    criteria: &[AcceptanceCriterion],
    evidence: &[Evidence],
    attention: &[AttentionItem],
) -> CompletionReport {
    let evidence_by_id = evidence
        .iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let mut traces = criteria
        .iter()
        .filter(|criterion| criterion.work_item_id == plan.work_item_id)
        .map(|criterion| {
            let mut task_ids = plan
                .tasks
                .iter()
                .filter(|task| task.acceptance_criteria.contains(&criterion.id))
                .map(|task| task.id)
                .collect::<Vec<_>>();
            task_ids.sort_by_key(|id| id.0);
            CriterionTrace {
                criterion_id: criterion.id,
                description: criterion.description.clone(),
                status: criterion.status,
                task_ids,
                evidence_ids: criterion.evidence_ids.clone(),
                evidence_summaries: criterion
                    .evidence_ids
                    .iter()
                    .filter_map(|id| evidence_by_id.get(id).map(|item| item.summary.clone()))
                    .collect(),
                waiver_reason: criterion.waiver_reason.clone(),
            }
        })
        .collect::<Vec<_>>();
    traces.sort_by_key(|trace| trace.criterion_id.0);

    let completed_tasks = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let satisfied_criteria = traces
        .iter()
        .filter(|trace| trace.status == AcceptanceStatus::Satisfied)
        .count();
    let waived_criteria = traces
        .iter()
        .filter(|trace| trace.status == AcceptanceStatus::Waived)
        .count();
    let unresolved_criteria = traces.len() - satisfied_criteria - waived_criteria;
    let open_attention = attention
        .iter()
        .filter(|item| {
            item.work_item_id == plan.work_item_id
                && matches!(
                    item.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                )
        })
        .count();
    let failed_task = plan.tasks.iter().any(|task| {
        matches!(
            task.status,
            PlanTaskStatus::Failed | PlanTaskStatus::Cancelled
        )
    });
    let failed_criterion = traces
        .iter()
        .any(|trace| trace.status == AcceptanceStatus::Failed);
    let ready_to_complete = !plan.tasks.is_empty()
        && completed_tasks == plan.tasks.len()
        && !traces.is_empty()
        && unresolved_criteria == 0
        && open_attention == 0
        && !failed_task;
    let risk = if open_attention > 0 || failed_task || failed_criterion {
        CompletionRiskLevel::Blocked
    } else if unresolved_criteria > 0 || completed_tasks != plan.tasks.len() {
        CompletionRiskLevel::NeedsEvidence
    } else if waived_criteria > 0 {
        CompletionRiskLevel::Warning
    } else {
        CompletionRiskLevel::Clear
    };
    let summary = if ready_to_complete {
        if waived_criteria == 0 {
            format!(
                "Definition of done passed: {completed_tasks}/{} tasks and {satisfied_criteria}/{} criteria verified.",
                plan.tasks.len(),
                traces.len()
            )
        } else {
            format!(
                "Definition of done passed with {waived_criteria} recorded waiver{}.",
                if waived_criteria == 1 { "" } else { "s" }
            )
        }
    } else {
        format!(
            "Verification pending: {completed_tasks}/{} tasks complete, {unresolved_criteria} criterion{} unresolved, {open_attention} attention item{} open.",
            plan.tasks.len(),
            if unresolved_criteria == 1 { "" } else { "s" },
            if open_attention == 1 { "" } else { "s" }
        )
    };

    CompletionReport {
        work_item_id: plan.work_item_id,
        completed_tasks,
        total_tasks: plan.tasks.len(),
        satisfied_criteria,
        waived_criteria,
        unresolved_criteria,
        open_attention,
        evidence_count: evidence
            .iter()
            .filter(|item| item.work_item_id == plan.work_item_id)
            .count(),
        risk,
        ready_to_complete,
        summary,
        criteria: traces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActorId, EvidenceKind, PlanId, PlanStatus, PlanTask, Provenance, ProvenanceSource,
    };

    #[test]
    fn report_is_traceable_and_fails_closed_until_evidence_is_accepted() {
        let work_item_id = WorkItemId::new();
        let plan_id = PlanId::new();
        let task_id = PlanTaskId::new();
        let criterion_id = AcceptanceCriterionId::new();
        let actor_id = ActorId::new();
        let mut plan = Plan {
            id: plan_id,
            work_item_id,
            version: 1,
            status: PlanStatus::Approved,
            tasks: vec![PlanTask {
                id: task_id,
                plan_id,
                title: "Verify behavior".into(),
                description: "Run the accepted verification".into(),
                status: PlanTaskStatus::Verifying,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: vec![criterion_id],
                assigned_contract_id: None,
            }],
            approved_by: Some(actor_id),
            approved_at_unix_ms: Some(1),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let evidence = Evidence {
            id: EvidenceId::new(),
            work_item_id,
            task_id: Some(task_id),
            kind: EvidenceKind::Test,
            summary: "Focused verification passed".into(),
            source: "cargo test -p feature".into(),
            produced_by: actor_id,
            created_at_unix_ms: 2,
        };
        let mut criterion = AcceptanceCriterion {
            id: criterion_id,
            work_item_id,
            description: "The feature behaves correctly".into(),
            status: AcceptanceStatus::Pending,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: vec![Provenance {
                source: ProvenanceSource::ManualEdit,
                recorded_by: actor_id,
                recorded_at_unix_ms: 1,
            }],
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let pending = build_completion_report(&plan, &[criterion.clone()], &[], &[]);
        assert_eq!(pending.risk, CompletionRiskLevel::NeedsEvidence);
        assert!(!pending.ready_to_complete);

        plan.tasks[0].status = PlanTaskStatus::Completed;
        criterion.status = AcceptanceStatus::Satisfied;
        criterion.evidence_ids.push(evidence.id);
        let complete = build_completion_report(&plan, &[criterion], &[evidence], &[]);
        assert_eq!(complete.risk, CompletionRiskLevel::Clear);
        assert!(complete.ready_to_complete);
        assert_eq!(complete.criteria[0].task_ids, vec![task_id]);
        assert_eq!(complete.criteria[0].evidence_summaries.len(), 1);
    }
}
