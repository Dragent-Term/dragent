//! Agent router: launchable scheduler tasks → concrete run proposals.
//!
//! Pure projection, never execution: for each launchable task it assembles
//! everything dispatch needs except the home and the agent preset (both
//! caller-chosen at `RunAgent` time). The coordinator (UI, automation, or
//! human) confirms proposals and dispatches them; nothing here spawns.
//! Reviewer tasks route like workers — the role name travels in the
//! proposal so the caller can match capability to preset.

use dragent_workspace_model::{
    PlanTask, PlanTaskId, RoleDefinition, WorkContract, WorkContractId, WorkspaceState,
};

/// One launchable task, ready to confirm and dispatch. Doubles as the
/// wire shape (fields are stable ids + assembled text).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteProposal {
    pub task_id: PlanTaskId,
    pub title: String,
    /// Mission + title + description + success criteria, assembled for
    /// the run prompt.
    pub prompt: String,
    pub role_name: Option<String>,
    pub contract_id: Option<WorkContractId>,
    pub model_route: Option<String>,
}

/// Propose runs for every launchable task in the latest projection.
/// Tasks without metadata (unknown ids) are skipped, never guessed.
pub fn route_launchable(state: &WorkspaceState) -> Vec<RouteProposal> {
    let Some(plan) = state.latest_structured_plan() else {
        return Vec::new();
    };
    let Some(projection) = super::scheduler::evaluate_workspace(state) else {
        return Vec::new();
    };
    let tasks: std::collections::HashMap<_, _> =
        plan.tasks.iter().map(|task| (task.id, task)).collect();
    let mut proposals = Vec::new();
    for task_id in &projection.launchable_task_ids {
        let Some(task) = tasks.get(task_id) else {
            continue;
        };
        let contract = task
            .assigned_contract_id
            .and_then(|id| state.work_contract(id));
        let role = contract
            .map(|c| c.role_definition_id)
            .and_then(|id| state.role_definition(id));
        let (prompt, role_name, model_route) = assemble_proposal(task, contract, role);
        proposals.push(RouteProposal {
            task_id: *task_id,
            title: task.title.clone(),
            prompt,
            role_name,
            contract_id: task.assigned_contract_id,
            model_route,
        });
    }
    proposals
}

/// Assemble one proposal's prompt (mission + title + description +
/// criteria) plus role/model attribution. Pure for unit tests.
pub(crate) fn assemble_proposal(
    task: &PlanTask,
    contract: Option<&WorkContract>,
    role: Option<&RoleDefinition>,
) -> (String, Option<String>, Option<String>) {
    let mut prompt = String::new();
    if let Some(contract) = contract {
        prompt.push_str(&contract.mission);
        prompt.push_str("\n\n");
    }
    prompt.push_str(&task.title);
    if !task.description.trim().is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(task.description.trim());
    }
    let criteria: Vec<String> = task
        .acceptance_criteria
        .iter()
        .map(|c| c.0.to_string())
        .collect();
    if !criteria.is_empty() {
        prompt.push_str("\n\nAcceptance criteria:\n");
        for criterion in &criteria {
            prompt.push_str("- ");
            prompt.push_str(criterion);
            prompt.push('\n');
        }
    }
    (
        prompt,
        role.map(|r| r.name.clone()),
        contract.and_then(|c| c.preferred_model_route.clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_workspace_model::{
        Plan, PlanId, PlanStatus, PlanTaskStatus, Project, WorkItemId, WorkspaceState,
    };

    #[test]
    fn no_plan_proposes_nothing() {
        let project = Project::new_for_organisation(&dragent_persistence::current_org(), std::path::PathBuf::from("/tmp/router-test"), 1);
        let state = WorkspaceState::new(project);
        assert!(route_launchable(&state).is_empty());
    }

    fn ready_task(id: PlanTaskId, plan_id: PlanId) -> PlanTask {
        PlanTask {
            id,
            plan_id,
            title: "Migrate widgets".into(),
            description: "Move widgets to the new layout".into(),
            status: PlanTaskStatus::Ready,
            depends_on: Vec::new(),
            review_target_task_id: None,
            acceptance_criteria: Vec::new(),
            assigned_contract_id: None,
        }
    }

    #[test]
    fn assemble_includes_mission_criteria_and_role() {
        let task = ready_task(PlanTaskId::new(), PlanId::new());
        let (prompt, role, model) = assemble_proposal(&task, None, None);
        assert!(prompt.contains("Migrate widgets") && prompt.contains("new layout"));
        assert_eq!((role, model), (None, None));

        let bare = PlanTask {
            title: "T".into(),
            description: "   ".into(),
            ..ready_task(PlanTaskId::new(), PlanId::new())
        };
        let (prompt, _, _) = assemble_proposal(&bare, None, None);
        assert_eq!(prompt, "T");
    }
}
