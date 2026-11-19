//! Pure, deterministic scheduling projection for an approved work plan.
//!
//! This module never invokes a model, starts a process, or mutates canonical
//! state. It derives which tasks may advance so local and cloud authorities can
//! make the same decision from the same records.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    AcceptanceCriterion, AcceptanceCriterionId, AcceptanceStatus, AttentionItem, AttentionStatus,
    Evidence, EvidenceId, Plan, PlanId, PlanStatus, PlanTaskId, PlanTaskStatus, WorkContract,
    WorkContractId, WorkContractStatus,
};

pub const MAX_AUTOMATIC_EXECUTION_ATTEMPTS: u32 = 3;

/// Deterministic exponential delay shared by local and cloud authorities.
pub fn execution_retry_delay_ms(attempt_number: u32) -> i64 {
    let exponent = attempt_number.saturating_sub(1).min(4);
    5_000_i64.saturating_mul(1_i64 << exponent)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerTaskReadiness {
    Ready,
    WaitingOnDependencies,
    Blocked,
    Active,
    VerificationRequired,
    Completed,
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SchedulerTaskProjection {
    pub task_id: PlanTaskId,
    pub canonical_status: PlanTaskStatus,
    pub readiness: SchedulerTaskReadiness,
    pub waiting_on: Vec<PlanTaskId>,
    pub blocked_by: Vec<PlanTaskId>,
    pub completion_gate_satisfied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SchedulerProjection {
    pub plan_id: PlanId,
    pub plan_version: u64,
    pub tasks: Vec<SchedulerTaskProjection>,
    /// All tasks whose deterministic prerequisites are currently satisfied.
    pub ready_task_ids: Vec<PlanTaskId>,
    /// Ready tasks that fit into the configured concurrency capacity.
    pub launchable_task_ids: Vec<PlanTaskId>,
    /// Capacity-selected tasks whose contract is bound to an active runtime.
    pub runtime_ready_task_ids: Vec<PlanTaskId>,
    pub active_task_count: usize,
    pub completed_task_count: usize,
    pub work_complete: bool,
    pub issues: Vec<SchedulerIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SchedulerIssue {
    PlanNotApproved,
    ZeroConcurrency,
    TaskHasNoContract {
        task_id: PlanTaskId,
    },
    MissingContract {
        task_id: PlanTaskId,
        contract_id: WorkContractId,
    },
    ContractNotAuthorized {
        task_id: PlanTaskId,
        contract_id: WorkContractId,
    },
    AttentionRequired {
        task_id: Option<PlanTaskId>,
    },
    DependencyFailed {
        task_id: PlanTaskId,
        dependency_ids: Vec<PlanTaskId>,
    },
    MissingAcceptanceCriterion {
        task_id: PlanTaskId,
        criterion_id: AcceptanceCriterionId,
    },
    CompletedTaskMissingEvidence {
        task_id: PlanTaskId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerTransitionError {
    StaleProjection,
    UnknownTask(PlanTaskId),
    InvalidTransition {
        task_id: PlanTaskId,
        from: PlanTaskStatus,
        to: PlanTaskStatus,
    },
    TaskNotReady(PlanTaskId),
    ConcurrencyUnavailable(PlanTaskId),
    CompletionEvidenceRequired(PlanTaskId),
}

impl std::fmt::Display for SchedulerTransitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for SchedulerTransitionError {}

/// The Principal may clarify an idle task, but cannot use that permission to
/// change approval, authority, dependencies, criteria, or lifecycle state.
pub fn validate_principal_plan_refinement(current: &Plan, proposed: &Plan) -> Result<(), String> {
    if current.status != PlanStatus::Approved
        || proposed.version != current.version.saturating_add(1)
        || proposed.updated_at_unix_ms < current.updated_at_unix_ms
        || proposed.tasks.len() != current.tasks.len()
    {
        return Err("Principal refinement requires the next version of an approved plan".into());
    }
    let mut expected = current.clone();
    expected.version = proposed.version;
    expected.updated_at_unix_ms = proposed.updated_at_unix_ms;
    for (old, new) in expected.tasks.iter_mut().zip(&proposed.tasks) {
        if old.title != new.title || old.description != new.description {
            if !matches!(
                old.status,
                PlanTaskStatus::Proposed
                    | PlanTaskStatus::Ready
                    | PlanTaskStatus::Blocked
                    | PlanTaskStatus::Failed
            ) {
                return Err("Cannot revise instructions for an active or finished task".into());
            }
            if new.title.trim().is_empty() || new.description.trim().is_empty() {
                return Err("Task instructions and title must not be empty".into());
            }
            old.title = new.title.clone();
            old.description = new.description.clone();
        }
    }
    if expected != *proposed {
        return Err("Principal refinement cannot change scope, approvals, criteria, dependencies, or task state".into());
    }
    Ok(())
}

/// Validate one canonical task transition against a previously evaluated
/// projection. The caller remains responsible for appending the event and
/// incrementing the Plan version atomically.
pub fn validate_scheduler_transition(
    plan: &Plan,
    projection: &SchedulerProjection,
    task_id: PlanTaskId,
    target: PlanTaskStatus,
) -> Result<(), SchedulerTransitionError> {
    if projection.plan_id != plan.id || projection.plan_version != plan.version {
        return Err(SchedulerTransitionError::StaleProjection);
    }
    let task = plan
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or(SchedulerTransitionError::UnknownTask(task_id))?;
    let task_projection = projection
        .tasks
        .iter()
        .find(|task| task.task_id == task_id)
        .ok_or(SchedulerTransitionError::StaleProjection)?;
    if task.status == target {
        return Ok(());
    }

    let edge_allowed = match (task.status, target) {
        (PlanTaskStatus::Proposed | PlanTaskStatus::Blocked, PlanTaskStatus::Ready) => true,
        (PlanTaskStatus::Ready, PlanTaskStatus::Running) => true,
        (PlanTaskStatus::Running, PlanTaskStatus::Reviewing | PlanTaskStatus::Verifying) => true,
        (PlanTaskStatus::Running, PlanTaskStatus::Ready) => true,
        (PlanTaskStatus::Failed, PlanTaskStatus::Ready) => true,
        (
            PlanTaskStatus::Reviewing,
            PlanTaskStatus::Ready | PlanTaskStatus::Running | PlanTaskStatus::Verifying,
        ) => true,
        (PlanTaskStatus::Verifying, PlanTaskStatus::Running) => true,
        (PlanTaskStatus::Reviewing | PlanTaskStatus::Verifying, PlanTaskStatus::Completed) => true,
        (
            PlanTaskStatus::Proposed
            | PlanTaskStatus::Ready
            | PlanTaskStatus::Running
            | PlanTaskStatus::Reviewing
            | PlanTaskStatus::Verifying,
            PlanTaskStatus::Blocked | PlanTaskStatus::Cancelled,
        ) => true,
        (
            PlanTaskStatus::Running
            | PlanTaskStatus::Reviewing
            | PlanTaskStatus::Verifying
            | PlanTaskStatus::Blocked,
            PlanTaskStatus::Failed,
        ) => true,
        _ => false,
    };
    if !edge_allowed {
        return Err(SchedulerTransitionError::InvalidTransition {
            task_id,
            from: task.status,
            to: target,
        });
    }
    if target == PlanTaskStatus::Ready
        && !matches!(
            task.status,
            PlanTaskStatus::Running | PlanTaskStatus::Reviewing | PlanTaskStatus::Failed
        )
        && task_projection.readiness != SchedulerTaskReadiness::Ready
    {
        return Err(SchedulerTransitionError::TaskNotReady(task_id));
    }
    if target == PlanTaskStatus::Running
        && task.status == PlanTaskStatus::Ready
        && !projection.runtime_ready_task_ids.contains(&task_id)
    {
        return Err(SchedulerTransitionError::ConcurrencyUnavailable(task_id));
    }
    if target == PlanTaskStatus::Completed && !task_projection.completion_gate_satisfied {
        return Err(SchedulerTransitionError::CompletionEvidenceRequired(
            task_id,
        ));
    }
    Ok(())
}

pub fn evaluate_scheduler(
    plan: &Plan,
    contracts: &[WorkContract],
    acceptance_criteria: &[AcceptanceCriterion],
    evidence: &[Evidence],
    attention: &[AttentionItem],
    maximum_parallel_tasks: usize,
) -> SchedulerProjection {
    let contract_by_id = contracts
        .iter()
        .map(|contract| (contract.id, contract))
        .collect::<HashMap<_, _>>();
    let criterion_by_id = acceptance_criteria
        .iter()
        .map(|criterion| (criterion.id, criterion))
        .collect::<HashMap<_, _>>();
    let evidence_by_id = evidence
        .iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<EvidenceId, _>>();
    let status_by_task = plan
        .tasks
        .iter()
        .map(|task| (task.id, task.status))
        .collect::<HashMap<_, _>>();
    let globally_blocked_by_attention = attention.iter().any(|item| {
        item.task_id.is_none()
            && matches!(
                item.status,
                AttentionStatus::Open | AttentionStatus::Acknowledged
            )
    });
    let task_attention = attention
        .iter()
        .filter(|item| {
            item.task_id.is_some()
                && matches!(
                    item.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                )
        })
        .filter_map(|item| item.task_id)
        .collect::<HashSet<_>>();

    let mut issues = Vec::new();
    if plan.status != PlanStatus::Approved {
        issues.push(SchedulerIssue::PlanNotApproved);
    }
    if maximum_parallel_tasks == 0 {
        issues.push(SchedulerIssue::ZeroConcurrency);
    }
    if globally_blocked_by_attention {
        issues.push(SchedulerIssue::AttentionRequired { task_id: None });
    }

    let active_task_count = plan
        .tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                PlanTaskStatus::Running | PlanTaskStatus::Reviewing | PlanTaskStatus::Verifying
            )
        })
        .count();
    let mut ready_task_ids = Vec::new();
    let mut task_projections = Vec::with_capacity(plan.tasks.len());

    for task in &plan.tasks {
        let waiting_on = task
            .depends_on
            .iter()
            .copied()
            .filter(|dependency| {
                let status = status_by_task.get(dependency);
                !matches!(status, Some(PlanTaskStatus::Completed))
                    && !(task.review_target_task_id == Some(*dependency)
                        && matches!(status, Some(PlanTaskStatus::Reviewing)))
            })
            .collect::<Vec<_>>();
        let blocked_by = task
            .depends_on
            .iter()
            .copied()
            .filter(|dependency| {
                matches!(
                    status_by_task.get(dependency),
                    Some(PlanTaskStatus::Failed | PlanTaskStatus::Cancelled)
                )
            })
            .collect::<Vec<_>>();
        if !blocked_by.is_empty() {
            issues.push(SchedulerIssue::DependencyFailed {
                task_id: task.id,
                dependency_ids: blocked_by.clone(),
            });
        }

        let contract_authorized = match task.assigned_contract_id {
            None => {
                issues.push(SchedulerIssue::TaskHasNoContract { task_id: task.id });
                false
            }
            Some(contract_id) => match contract_by_id.get(&contract_id) {
                None => {
                    issues.push(SchedulerIssue::MissingContract {
                        task_id: task.id,
                        contract_id,
                    });
                    false
                }
                Some(contract)
                    if contract.work_item_id == plan.work_item_id
                        && matches!(
                            contract.status,
                            WorkContractStatus::Approved
                                | WorkContractStatus::Active
                                | WorkContractStatus::Completed
                        ) =>
                {
                    true
                }
                Some(_) => {
                    issues.push(SchedulerIssue::ContractNotAuthorized {
                        task_id: task.id,
                        contract_id,
                    });
                    false
                }
            },
        };

        let (completion_gate_satisfied, criterion_missing) = task_completion_gate(
            plan,
            task.id,
            &task.acceptance_criteria,
            &criterion_by_id,
            &evidence_by_id,
            evidence,
        );
        for criterion_id in criterion_missing {
            issues.push(SchedulerIssue::MissingAcceptanceCriterion {
                task_id: task.id,
                criterion_id,
            });
        }
        if task.status == PlanTaskStatus::Completed && !completion_gate_satisfied {
            issues.push(SchedulerIssue::CompletedTaskMissingEvidence { task_id: task.id });
        }

        let attention_required = globally_blocked_by_attention || task_attention.contains(&task.id);
        if task_attention.contains(&task.id) {
            issues.push(SchedulerIssue::AttentionRequired {
                task_id: Some(task.id),
            });
        }
        let prerequisites_satisfied = plan.status == PlanStatus::Approved
            && contract_authorized
            && waiting_on.is_empty()
            && !attention_required;
        let readiness = match task.status {
            PlanTaskStatus::Running | PlanTaskStatus::Reviewing | PlanTaskStatus::Verifying => {
                SchedulerTaskReadiness::Active
            }
            PlanTaskStatus::Completed if completion_gate_satisfied => {
                SchedulerTaskReadiness::Completed
            }
            PlanTaskStatus::Completed => SchedulerTaskReadiness::VerificationRequired,
            PlanTaskStatus::Failed | PlanTaskStatus::Cancelled => SchedulerTaskReadiness::Terminal,
            _ if !blocked_by.is_empty() || !contract_authorized || attention_required => {
                SchedulerTaskReadiness::Blocked
            }
            _ if prerequisites_satisfied => {
                ready_task_ids.push(task.id);
                SchedulerTaskReadiness::Ready
            }
            _ => SchedulerTaskReadiness::WaitingOnDependencies,
        };
        task_projections.push(SchedulerTaskProjection {
            task_id: task.id,
            canonical_status: task.status,
            readiness,
            waiting_on,
            blocked_by,
            completion_gate_satisfied,
        });
    }

    // Plan vector order is explicitly presentation-only. UUID ordering keeps
    // local and cloud launch selection deterministic for the same plan.
    ready_task_ids.sort_by_key(|task_id| task_id.0);
    let available_slots = maximum_parallel_tasks.saturating_sub(active_task_count);
    let launchable_task_ids = ready_task_ids
        .iter()
        .copied()
        .take(available_slots)
        .collect::<Vec<_>>();
    let runtime_ready_task_ids = launchable_task_ids
        .iter()
        .copied()
        .filter(|task_id| {
            let Some(task) = plan.tasks.iter().find(|task| task.id == *task_id) else {
                return false;
            };
            let Some(contract_id) = task.assigned_contract_id else {
                return false;
            };
            contract_by_id.get(&contract_id).is_some_and(|contract| {
                contract.status == WorkContractStatus::Active
                    && contract.agent_instance_id.is_some()
            })
        })
        .collect();
    let completed_task_count = task_projections
        .iter()
        .filter(|task| task.readiness == SchedulerTaskReadiness::Completed)
        .count();
    let work_complete = !task_projections.is_empty()
        && completed_task_count == task_projections.len()
        && issues
            .iter()
            .all(|issue| matches!(issue, SchedulerIssue::ZeroConcurrency));

    SchedulerProjection {
        plan_id: plan.id,
        plan_version: plan.version,
        tasks: task_projections,
        ready_task_ids,
        launchable_task_ids,
        runtime_ready_task_ids,
        active_task_count,
        completed_task_count,
        work_complete,
        issues,
    }
}

fn task_completion_gate(
    plan: &Plan,
    task_id: PlanTaskId,
    criterion_ids: &[AcceptanceCriterionId],
    criteria: &HashMap<AcceptanceCriterionId, &AcceptanceCriterion>,
    evidence_by_id: &HashMap<EvidenceId, &Evidence>,
    evidence: &[Evidence],
) -> (bool, Vec<AcceptanceCriterionId>) {
    let mut missing = Vec::new();
    let mut all_waived = !criterion_ids.is_empty();
    let criteria_satisfied = !criterion_ids.is_empty()
        && criterion_ids.iter().all(|criterion_id| {
            let Some(criterion) = criteria.get(criterion_id) else {
                missing.push(*criterion_id);
                return false;
            };
            if criterion.work_item_id != plan.work_item_id {
                return false;
            }
            match criterion.status {
                AcceptanceStatus::Waived => true,
                AcceptanceStatus::Satisfied => {
                    all_waived = false;
                    criterion.evidence_ids.iter().any(|evidence_id| {
                        evidence_by_id
                            .get(evidence_id)
                            .is_some_and(|item| item.work_item_id == plan.work_item_id)
                    })
                }
                _ => {
                    all_waived = false;
                    false
                }
            }
        });
    let task_has_evidence = evidence
        .iter()
        .any(|item| item.work_item_id == plan.work_item_id && item.task_id == Some(task_id));
    (
        criteria_satisfied && (all_waived || task_has_evidence),
        missing,
    )
}

/// UI-independent coordination health. Coordinate renders this; it never
/// derives execution ownership from view state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationHealth {
    NotStarted,
    Starting,
    Active,
    Waiting,
    Blocked,
    Stalled,
    Completed,
    Paused,
    Cancelled,
}

/// Grace period before an Executing work item with zero runtime activity is
/// reported as stalled. Covers "start committed but no provider session came
/// up": the watchdog must fire instead of leaving the UI looking idle.
pub const COORDINATION_STALL_GRACE_MS: i64 = 30_000;

/// Deterministic Principal decision for a worker blocker. Infrastructure
/// failures retry automatically; contract/capability failures escalate with a
/// concrete fix instead of burning retries. Prose never mutates state: the
/// scheduler applies the returned decision through typed commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerDecision {
    /// Retry the same task (attempt budget permitting).
    Retry,
    /// The work contract is wrong (tools, scope, permissions). Escalate to
    /// the Principal/human with a contract fix; do not retry blindly.
    EscalateContract,
    /// Unknown failure. Retry while budget remains, else escalate.
    Escalate,
}

pub fn principal_decision_for_blocker(summary: &str, attempt_number: u32) -> BlockerDecision {
    let normalized = summary.to_lowercase();
    const CONTRACT_MARKERS: &[&str] = &[
        "no file-editing",
        "no tool",
        "tools are unavailable",
        "tool is currently available",
        "tool is not currently available",
        "unavailable in this session",
        "outside the work contract",
        "outside the write scope",
        "missing permission",
        "missing authority",
        "requires approval",
        "requires consent",
    ];
    if CONTRACT_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return BlockerDecision::EscalateContract;
    }
    const INFRA_MARKERS: &[&str] = &[
        "lease expired",
        "timed out",
        "timeout",
        "stream ended",
        "cancelled by the workspace authority",
    ];
    if INFRA_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        if attempt_number < MAX_AUTOMATIC_EXECUTION_ATTEMPTS {
            return BlockerDecision::Retry;
        }
        return BlockerDecision::Escalate;
    }
    if attempt_number < MAX_AUTOMATIC_EXECUTION_ATTEMPTS {
        BlockerDecision::Retry
    } else {
        BlockerDecision::Escalate
    }
}

/// Tasks that directly depend on `task_id`, either through `depends_on` or as
/// a review target. Their agents receive worker handoffs directly, without
/// waiting on the Principal to relay.
pub fn dependent_task_ids(plan: &Plan, task_id: PlanTaskId) -> Vec<PlanTaskId> {
    let mut dependents = plan
        .tasks
        .iter()
        .filter(|task| {
            task.depends_on.contains(&task_id) || task.review_target_task_id == Some(task_id)
        })
        .map(|task| task.id)
        .collect::<Vec<_>>();
    dependents.sort_by_key(|id| id.0);
    dependents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActorId, EvidenceKind, ExecutionLocation, PlanTask, RoleDefinitionId, WorkBudget,
        WorkItemId,
    };

    struct Fixture {
        plan: Plan,
        contracts: Vec<WorkContract>,
        criteria: Vec<AcceptanceCriterion>,
        evidence: Vec<Evidence>,
    }

    #[test]
    fn principal_refinement_preserves_approval_and_execution_boundaries() {
        let current = fixture().plan;
        let mut revised = current.clone();
        revised.version += 1;
        revised.updated_at_unix_ms += 1;
        revised.tasks[0].description =
            "Handle an empty response within the approved API contract".into();
        assert!(validate_principal_plan_refinement(&current, &revised).is_ok());
        let mut unsafe_plan = revised.clone();
        unsafe_plan.tasks[0].acceptance_criteria.clear();
        assert!(validate_principal_plan_refinement(&current, &unsafe_plan).is_err());
        unsafe_plan = revised.clone();
        unsafe_plan.approved_by = Some(ActorId::new());
        assert!(validate_principal_plan_refinement(&current, &unsafe_plan).is_err());
        for status in [
            PlanTaskStatus::Running,
            PlanTaskStatus::Reviewing,
            PlanTaskStatus::Verifying,
            PlanTaskStatus::Completed,
            PlanTaskStatus::Cancelled,
        ] {
            let mut busy = current.clone();
            busy.tasks[0].status = status;
            revised.tasks[0].status = status;
            assert!(validate_principal_plan_refinement(&busy, &revised).is_err());
        }
    }

    fn fixture() -> Fixture {
        let work_item_id = WorkItemId::new();
        let actor_id = ActorId::new();
        let plan_id = PlanId::new();
        let first_task_id = PlanTaskId::new();
        let second_task_id = PlanTaskId::new();
        let first_contract_id = WorkContractId::new();
        let second_contract_id = WorkContractId::new();
        let first_criterion_id = AcceptanceCriterionId::new();
        let second_criterion_id = AcceptanceCriterionId::new();
        let contract = |id, task_id, criterion_id| WorkContract {
            id,
            work_item_id,
            role_definition_id: RoleDefinitionId::new(),
            version: 1,
            agent_instance_id: None,
            status: WorkContractStatus::Approved,
            mission: "Complete assigned work".into(),
            task_ids: vec![task_id],
            input_artifact_ids: Vec::new(),
            expected_outputs: vec!["Evidence".into()],
            success_criteria: vec![criterion_id],
            write_scope: Vec::new(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            permissions: Vec::new(),
            budget: WorkBudget::default(),
            escalation_rules: Vec::new(),
            execution_location: ExecutionLocation::Local,
            preferred_model_route: None,
            created_by: actor_id,
            updated_by: actor_id,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let criterion = |id| AcceptanceCriterion {
            id,
            work_item_id,
            description: "Observable behavior".into(),
            status: AcceptanceStatus::Pending,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: vec![crate::Provenance {
                source: crate::ProvenanceSource::ManualEdit,
                recorded_by: actor_id,
                recorded_at_unix_ms: 1,
            }],
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        Fixture {
            plan: Plan {
                id: plan_id,
                work_item_id,
                version: 1,
                status: PlanStatus::Approved,
                tasks: vec![
                    PlanTask {
                        id: first_task_id,
                        plan_id,
                        title: "Implement".into(),
                        description: "Implement the feature".into(),
                        status: PlanTaskStatus::Proposed,
                        depends_on: Vec::new(),
                        review_target_task_id: None,
                        acceptance_criteria: vec![first_criterion_id],
                        assigned_contract_id: Some(first_contract_id),
                    },
                    PlanTask {
                        id: second_task_id,
                        plan_id,
                        title: "Review".into(),
                        description: "Review the implementation".into(),
                        status: PlanTaskStatus::Proposed,
                        depends_on: vec![first_task_id],
                        review_target_task_id: Some(first_task_id),
                        acceptance_criteria: vec![second_criterion_id],
                        assigned_contract_id: Some(second_contract_id),
                    },
                ],
                approved_by: Some(actor_id),
                approved_at_unix_ms: Some(1),
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            },
            contracts: vec![
                contract(first_contract_id, first_task_id, first_criterion_id),
                contract(second_contract_id, second_task_id, second_criterion_id),
            ],
            criteria: vec![
                criterion(first_criterion_id),
                criterion(second_criterion_id),
            ],
            evidence: Vec::new(),
        }
    }

    #[test]
    fn only_dependency_ready_tasks_fit_into_deterministic_capacity() {
        let fixture = fixture();
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            1,
        );
        assert_eq!(projection.ready_task_ids, [fixture.plan.tasks[0].id]);
        assert_eq!(projection.launchable_task_ids, projection.ready_task_ids);
        assert_eq!(
            projection.tasks[1].readiness,
            SchedulerTaskReadiness::WaitingOnDependencies
        );
    }

    #[test]
    fn evidence_backed_completion_unlocks_the_dependent_task() {
        let mut fixture = fixture();
        let completed_task_id = fixture.plan.tasks[0].id;
        let evidence = Evidence {
            id: EvidenceId::new(),
            work_item_id: fixture.plan.work_item_id,
            task_id: Some(completed_task_id),
            kind: EvidenceKind::Test,
            summary: "Tests passed".into(),
            source: "cargo test".into(),
            produced_by: ActorId::new(),
            created_at_unix_ms: 2,
        };
        fixture.plan.tasks[0].status = PlanTaskStatus::Completed;
        fixture.criteria[0].status = AcceptanceStatus::Satisfied;
        fixture.criteria[0].evidence_ids = vec![evidence.id];
        fixture.evidence.push(evidence);

        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            2,
        );
        assert_eq!(projection.completed_task_count, 1);
        assert_eq!(projection.ready_task_ids, [fixture.plan.tasks[1].id]);
        assert!(!projection.work_complete);
    }

    #[test]
    fn explicit_review_task_can_inspect_an_unresolved_candidate() {
        let mut fixture = fixture();
        fixture.plan.tasks[0].status = PlanTaskStatus::Reviewing;

        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            2,
        );

        assert_eq!(projection.ready_task_ids, [fixture.plan.tasks[1].id]);
        assert!(projection.tasks[1].waiting_on.is_empty());
    }

    #[test]
    fn failed_dependencies_and_open_attention_block_automatic_launch() {
        let mut fixture = fixture();
        fixture.plan.tasks[0].status = PlanTaskStatus::Failed;
        let attention = AttentionItem {
            id: crate::AttentionItemId::new(),
            work_item_id: fixture.plan.work_item_id,
            task_id: Some(fixture.plan.tasks[1].id),
            thread_id: None,
            execution_attempt_id: Some(crate::ExecutionAttemptId::new()),
            tool_call_id: Some("call-1".into()),
            kind: crate::AttentionKind::Permission,
            status: AttentionStatus::Open,
            summary: "Permission required".into(),
            requested_by: ActorId::new(),
            assigned_to: vec![ActorId::new()],
            created_at_unix_ms: 2,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[attention],
            2,
        );
        assert!(projection.launchable_task_ids.is_empty());
        assert_eq!(
            projection.tasks[1].readiness,
            SchedulerTaskReadiness::Blocked
        );
    }

    #[test]
    fn completed_status_without_accepted_evidence_does_not_complete_work() {
        let mut fixture = fixture();
        for task in &mut fixture.plan.tasks {
            task.status = PlanTaskStatus::Completed;
        }
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            2,
        );
        assert_eq!(projection.completed_task_count, 0);
        assert!(!projection.work_complete);
        assert!(projection
            .issues
            .iter()
            .any(|issue| matches!(issue, SchedulerIssue::CompletedTaskMissingEvidence { .. })));
    }

    #[test]
    fn transition_guard_prevents_skips_and_completion_without_evidence() {
        let mut fixture = fixture();
        let root_task_id = fixture.plan.tasks[0].id;
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            1,
        );
        assert!(validate_scheduler_transition(
            &fixture.plan,
            &projection,
            root_task_id,
            PlanTaskStatus::Ready,
        )
        .is_ok());
        assert!(matches!(
            validate_scheduler_transition(
                &fixture.plan,
                &projection,
                root_task_id,
                PlanTaskStatus::Running,
            ),
            Err(SchedulerTransitionError::InvalidTransition { .. })
        ));

        fixture.plan.tasks[0].status = PlanTaskStatus::Verifying;
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            1,
        );
        assert_eq!(
            validate_scheduler_transition(
                &fixture.plan,
                &projection,
                root_task_id,
                PlanTaskStatus::Completed,
            ),
            Err(SchedulerTransitionError::CompletionEvidenceRequired(
                root_task_id
            ))
        );
    }

    #[test]
    fn independent_tasks_start_in_parallel_within_capacity() {
        let mut fixture = fixture();
        // Detach the second task so both are independently schedulable.
        fixture.plan.tasks[1].depends_on.clear();
        fixture.plan.tasks[1].review_target_task_id = None;
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            2,
        );
        assert_eq!(projection.ready_task_ids.len(), 2);
        assert_eq!(projection.launchable_task_ids.len(), 2);
        // Capacity 1 still launches exactly one, deterministically ordered.
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            1,
        );
        assert_eq!(projection.launchable_task_ids.len(), 1);
    }

    #[test]
    fn reviewer_waits_for_worker_completion() {
        // Fixture review task depends on the worker and targets it: only the
        // worker is ready while the worker is merely proposed.
        let fixture = fixture();
        let projection = evaluate_scheduler(
            &fixture.plan,
            &fixture.contracts,
            &fixture.criteria,
            &fixture.evidence,
            &[],
            2,
        );
        assert_eq!(projection.ready_task_ids, [fixture.plan.tasks[0].id]);
        assert_eq!(
            projection.tasks[1].readiness,
            SchedulerTaskReadiness::WaitingOnDependencies
        );
    }

    #[test]
    fn blocker_decisions_route_retry_vs_contract_escalation() {
        assert_eq!(
            principal_decision_for_blocker(
                "Execution lease expired before attempt 1 completed.",
                1
            ),
            BlockerDecision::Retry
        );
        assert_eq!(
            principal_decision_for_blocker(
                "Execution lease expired before attempt 3 completed.",
                3
            ),
            BlockerDecision::Escalate
        );
        assert_eq!(
            principal_decision_for_blocker(
                "Blocked: no file-editing tool is currently available in this session.",
                1
            ),
            BlockerDecision::EscalateContract
        );
        assert_eq!(
            principal_decision_for_blocker("Write target is outside the Work Contract scope.", 2),
            BlockerDecision::EscalateContract
        );
        // Prose never mutates state: unknown failures retry within budget.
        assert_eq!(
            principal_decision_for_blocker("Something unexpected happened.", 1),
            BlockerDecision::Retry
        );
        assert_eq!(
            principal_decision_for_blocker("Something unexpected happened.", 3),
            BlockerDecision::Escalate
        );
    }

    #[test]
    fn dependents_cover_depends_on_and_review_targets() {
        let fixture = fixture();
        let dependents = dependent_task_ids(&fixture.plan, fixture.plan.tasks[0].id);
        assert_eq!(dependents, [fixture.plan.tasks[1].id]);
        let dependents = dependent_task_ids(&fixture.plan, fixture.plan.tasks[1].id);
        assert!(dependents.is_empty());
    }
}
