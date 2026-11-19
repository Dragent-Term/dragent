//! Small, UI-agnostic domain types for Dragent's engineering workspace.
//!
//! This crate intentionally contains no filesystem, database, model, or UI
//! code. It is the stable vocabulary shared by persistence, the Rust workspace
//! shell, and the future orchestration event stream.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::CoordinationReadinessIssue;
use crate::Plan;
use crate::PlanStatus;
use crate::ProposedTeam;
use crate::RoleDefinitionId;
use crate::ShapingProposal;
use crate::WorkContract;
use crate::WorkContractId;
use crate::WorkContractStatus;
use crate::WorkspaceEvent;

/// A deliberately conservative bound for in-memory semantic history. Durable
/// persistence owns the complete audit trail; the UI only needs a recent tail.

/// A bounded in-memory tail. The persistence layer may retain the full event
/// history, but renderers never accumulate an unbounded Vec of events.
#[derive(Clone, Debug)]
pub struct WorkspaceEventLog {
    pub(crate) events: VecDeque<WorkspaceEvent>,
    pub(crate) next_sequence: u64,
    pub(crate) capacity: usize,
}

/// Deterministic readiness projection used before a shaped work item can enter
/// coordination. This is intentionally about reviewed authority, not runtime
/// availability: a ready team still has no agent instances until a scheduler
/// handles a separate launch command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoordinationReadiness {
    pub role_count: usize,
    pub approved_role_count: usize,
    pub plan_present: bool,
    pub plan_approved: bool,
    pub issues: Vec<CoordinationReadinessIssue>,
}

impl CoordinationReadiness {
    pub fn is_approvable(&self) -> bool {
        self.role_count > 0 && self.plan_present && self.issues.is_empty()
    }

    pub fn is_ready(&self) -> bool {
        self.is_approvable() && self.plan_approved
    }
}

/// `open_decisions` carries the fresher per-record open-decision count when
/// the caller replays shaping records (inline resolutions supersede the
/// proposal snapshot). `None` falls back to the proposal's own list.
pub fn evaluate_coordination_readiness(
    shaping_proposal: Option<&ShapingProposal>,
    open_decisions: Option<usize>,
    team: Option<&ProposedTeam>,
    role_definition_ids: &HashSet<RoleDefinitionId>,
    contracts: &HashMap<WorkContractId, WorkContract>,
    plan: Option<&Plan>,
) -> CoordinationReadiness {
    let mut readiness = CoordinationReadiness::default();
    match shaping_proposal {
        None => readiness
            .issues
            .push(CoordinationReadinessIssue::MissingShapingProposal),
        Some(proposal) => {
            let open = open_decisions.unwrap_or(proposal.open_decisions.len());
            if open > 0 {
                readiness
                    .issues
                    .push(CoordinationReadinessIssue::OpenDecisionsRemain(open));
            }
        }
    }
    let Some(team) = team else {
        readiness
            .issues
            .push(CoordinationReadinessIssue::MissingProposedTeam);
        return readiness;
    };
    readiness.role_count = team.roles.len();
    for role in &team.roles {
        if !role_definition_ids.contains(&role.role_definition_id) {
            readiness
                .issues
                .push(CoordinationReadinessIssue::MissingRoleDefinition(
                    role.role_definition_id,
                ));
        }
        let Some(contract) = contracts.get(&role.work_contract_id) else {
            readiness
                .issues
                .push(CoordinationReadinessIssue::MissingWorkContract(
                    role.work_contract_id,
                ));
            continue;
        };
        if contract.work_item_id != team.work_item_id
            || contract.role_definition_id != role.role_definition_id
        {
            readiness
                .issues
                .push(CoordinationReadinessIssue::ContractIdentityMismatch(
                    contract.id,
                ));
        } else if matches!(
            contract.status,
            // Active and Completed contracts necessarily passed approval:
            // provisioning requires Approved status, so anything beyond it
            // satisfies the gate. Only Draft/Proposed/Revoked block.
            WorkContractStatus::Approved
                | WorkContractStatus::Active
                | WorkContractStatus::Completed
        ) {
            readiness.approved_role_count += 1;
        } else {
            readiness
                .issues
                .push(CoordinationReadinessIssue::ContractNotApproved(contract.id));
        }
    }
    let Some(plan) = plan else {
        readiness
            .issues
            .push(CoordinationReadinessIssue::MissingPlan);
        return readiness;
    };
    readiness.plan_present = true;
    readiness.plan_approved = plan.status == PlanStatus::Approved;
    let team_contract_ids = team
        .roles
        .iter()
        .map(|role| role.work_contract_id)
        .collect::<HashSet<_>>();
    let mut assigned_contract_ids = HashSet::new();
    for task in &plan.tasks {
        let Some(contract_id) = task.assigned_contract_id else {
            readiness
                .issues
                .push(CoordinationReadinessIssue::PlanTaskUnassigned(task.id));
            continue;
        };
        if !team_contract_ids.contains(&contract_id) {
            readiness.issues.push(
                CoordinationReadinessIssue::PlanTaskReferencesUnknownContract {
                    task_id: task.id,
                    contract_id,
                },
            );
        } else {
            assigned_contract_ids.insert(contract_id);
        }
    }
    for contract_id in team_contract_ids {
        if !assigned_contract_ids.contains(&contract_id) {
            readiness
                .issues
                .push(CoordinationReadinessIssue::ContractHasNoPlanTask(
                    contract_id,
                ));
        }
    }
    readiness
}
