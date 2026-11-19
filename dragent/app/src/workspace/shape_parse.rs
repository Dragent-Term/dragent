//! Shaping parse/format helpers for the workspace.
//!
//! Responsible for:
//! - parsing user-entered shaping fields (budgets, artifacts, criteria,
//!   roles, plan tasks, model routes, memory notes) into canonical model types
//! - formatting canonical shaping records back into editable text
//! - building plan-revision payloads and proposed team/contract records
//! - pure plan-task list operations (move/remove/append)
//!
//! Not responsible for:
//! - rendering shaping UI (shape_principal / shape_workspace / view)
//! - submitting commands or driving coordination turns (WorkspaceView)
//!
//! All functions here are free functions with no View dependencies.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    ActorRef, Evidence, Plan, PlanTask, Project, RoleDefinition, WorkContract, WorkItemId,
    WorkStatus, WorkspaceState,
};
use uuid::Uuid;
use warpui::{AppContext, GetSingletonModelHandle, ReadModel, SingletonEntity};

use super::persistence::WriterHandles;

pub(crate) enum ShapeTurnEvent {
    WorkCreated(WorkspaceEventEnvelope),
    StatusAccepted(WorkspaceEventEnvelope),
    MessageAccepted(WorkspaceEventEnvelope),
    /// A conversational Principal reply was accepted by authority.
    /// `streamed` is true when a model pass already rendered it in the
    /// conversation (nothing to draw); false for the canned greeting reply,
    /// which never ran a pass and must be written as a user/agent exchange.
    ConversationReplyAccepted {
        event: WorkspaceEventEnvelope,
        streamed: bool,
    },
    PrincipalActivity(String),
    /// A Gather turn returned requirements_complete: the Principal declares
    /// requirements fully gathered with a confirmation summary presented.
    RequirementsComplete,
    /// Successful findings of a finished pass ("tool: summary" lines) for
    /// the enduring turn-memory digest future turns are prompted with.
    PrincipalFindings {
        digest: Vec<String>,
    },
    /// The background orchestrator wants one provider pass run through the
    /// work item's `AgentController` (see `run_principal_pass`). `user_turn`
    /// is `Some` only for the pass that carries the user's own message — it
    /// becomes their visible block, with any image attachments; later passes
    /// are internal continuations of the same turn. `reply` receives the
    /// pass's raw result; dropping it (Stop, tab closed) ends the turn.
    RunPass {
        request: super::shape_principal::PrincipalPassRequest,
        user_turn: Option<(String, Vec<crate::agent::attachments::MessageAttachment>)>,
        reply:
            std::sync::mpsc::Sender<Result<super::shape_principal::PrincipalPassOutcome, String>>,
    },
    /// Canonical brain entries recorded after a successful turn. Projected
    /// like any other event so the very next turn can already read them.
    PrincipalMemoryRecorded {
        events: Vec<WorkspaceEventEnvelope>,
    },
    PrincipalAccepted(Result<ShapePrincipalOutcome, String>),
}

pub(crate) struct ShapePrincipalOutcome {
    pub(crate) events: Vec<WorkspaceEventEnvelope>,
    pub(crate) team_events: Vec<WorkspaceEventEnvelope>,
    pub(crate) planning_event: Option<WorkspaceEventEnvelope>,
    pub(crate) warning: Option<String>,
}

pub(crate) fn split_shape_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    for item in value
        .lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .take(32)
    {
        let item = item.split_whitespace().collect::<Vec<_>>().join(" ");
        if !items.contains(&item) {
            items.push(item);
        }
    }
    items
}

pub(crate) fn format_shape_budget(budget: &dragent_workspace_model::WorkBudget) -> String {
    let value =
        |value: Option<u64>| value.map_or_else(|| "unset".into(), |value| value.to_string());
    [
        format!("input_tokens={}", value(budget.maximum_input_tokens)),
        format!("output_tokens={}", value(budget.maximum_output_tokens)),
        format!("cost_micros={}", value(budget.maximum_cost_micros)),
        format!("duration_ms={}", value(budget.maximum_duration_ms)),
        format!("tool_calls={}", value(budget.maximum_tool_calls)),
    ]
    .join("\n")
}

pub(crate) fn parse_shape_budget(
    value: &str,
) -> Result<dragent_workspace_model::WorkBudget, String> {
    let mut budget = dragent_workspace_model::WorkBudget::default();
    let mut seen = HashSet::new();
    for entry in value
        .lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((key, raw_value)) = entry.split_once('=') else {
            return Err(format!("Budget entry '{entry}' must use name=value"));
        };
        let key = key.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        if !seen.insert(key.clone()) {
            return Err(format!("Budget field '{key}' appears more than once"));
        }
        let raw_value = raw_value.trim();
        let parsed = if raw_value.is_empty()
            || matches!(raw_value.to_ascii_lowercase().as_str(), "unset" | "none")
        {
            None
        } else {
            let normalized = raw_value.replace('_', "");
            let number = normalized.parse::<u64>().map_err(|_| {
                format!("Budget field '{key}' must be a positive whole number or unset")
            })?;
            if number == 0 {
                return Err(format!(
                    "Budget field '{key}' must be greater than zero or unset"
                ));
            }
            Some(number)
        };
        match key.as_str() {
            "input" | "input_tokens" | "maximum_input_tokens" => {
                budget.maximum_input_tokens = parsed;
            }
            "output" | "output_tokens" | "maximum_output_tokens" => {
                budget.maximum_output_tokens = parsed;
            }
            "cost" | "cost_micros" | "maximum_cost_micros" => {
                budget.maximum_cost_micros = parsed;
            }
            "duration" | "duration_ms" | "maximum_duration_ms" => {
                budget.maximum_duration_ms = parsed;
            }
            "tools" | "tool_calls" | "maximum_tool_calls" => {
                budget.maximum_tool_calls = parsed;
            }
            _ => return Err(format!("Unknown budget field '{key}'")),
        }
    }
    Ok(budget)
}

pub(crate) fn parse_execution_location(
    value: &str,
) -> Result<dragent_workspace_model::ExecutionLocation, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "local" => Ok(dragent_workspace_model::ExecutionLocation::Local),
        "cloud" => Ok(dragent_workspace_model::ExecutionLocation::Cloud),
        "hybrid" => Ok(dragent_workspace_model::ExecutionLocation::Hybrid),
        _ => Err("Execution location must be Local, Cloud, or Hybrid".into()),
    }
}

pub(crate) fn parse_preferred_model_route(value: &str) -> Result<Option<String>, String> {
    let route = value.trim();
    if route.is_empty() || route.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if route.len() > 200
        || route
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("Model route must be Auto or a model ID without whitespace".into());
    }
    Ok(Some(route.to_string()))
}

pub(crate) fn parse_shape_artifact(
    value: &str,
) -> Result<(PathBuf, dragent_workspace_model::ArtifactKind), String> {
    let mut path = None;
    let mut kind = dragent_workspace_model::ArtifactKind::Other;
    for line in value.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            return Err("Repository artifact entries must use path= and kind=".into());
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "path" => path = Some(PathBuf::from(value.trim())),
            "kind" => {
                kind = match value
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '_', '-'], "")
                    .as_str()
                {
                    "productbrief" => dragent_workspace_model::ArtifactKind::ProductBrief,
                    "requirements" => dragent_workspace_model::ArtifactKind::Requirements,
                    "decision" => dragent_workspace_model::ArtifactKind::Decision,
                    "technicalplan" => dragent_workspace_model::ArtifactKind::TechnicalPlan,
                    "testplan" => dragent_workspace_model::ArtifactKind::TestPlan,
                    "review" => dragent_workspace_model::ArtifactKind::Review,
                    "design" => dragent_workspace_model::ArtifactKind::Design,
                    "other" => dragent_workspace_model::ArtifactKind::Other,
                    _ => return Err("Unknown artifact kind".into()),
                };
            }
            other => return Err(format!("Unknown repository artifact field '{other}'")),
        }
    }
    let path = path
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "Repository artifact path is required".to_string())?;
    let candidate = dragent_workspace_model::ArtifactRef {
        id: dragent_workspace_model::ArtifactId::new(),
        work_item_id: dragent_workspace_model::WorkItemId::new(),
        relative_path: path.clone(),
        kind,
    };
    candidate.validate().map_err(|error| error.to_string())?;
    Ok((path, kind))
}

pub(crate) fn parse_success_criteria(
    value: &str,
    criteria: &[dragent_workspace_model::AcceptanceCriterion],
) -> Result<Vec<dragent_workspace_model::AcceptanceCriterionId>, String> {
    let requested = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return Err("At least one acceptance criterion must be assigned".into());
    }

    let mut resolved = Vec::with_capacity(requested.len());
    for description in requested {
        let matches = criteria
            .iter()
            .filter(|criterion| criterion.description.eq_ignore_ascii_case(description))
            .collect::<Vec<_>>();
        let criterion = match matches.as_slice() {
            [criterion] => *criterion,
            [] => return Err(format!("Unknown acceptance criterion: {description}")),
            _ => return Err(format!("Ambiguous acceptance criterion: {description}")),
        };
        if !resolved.contains(&criterion.id) {
            resolved.push(criterion.id);
        }
    }
    Ok(resolved)
}

pub(crate) fn parse_input_artifacts(
    value: &str,
    artifacts: &[dragent_workspace_model::ArtifactRef],
) -> Result<Vec<dragent_workspace_model::ArtifactId>, String> {
    let requested = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let mut resolved = Vec::with_capacity(requested.len());
    for path in requested {
        let matches = artifacts
            .iter()
            .filter(|artifact| artifact.relative_path.to_string_lossy() == path)
            .collect::<Vec<_>>();
        let artifact = match matches.as_slice() {
            [artifact] => *artifact,
            [] => return Err(format!("Unknown repository artifact: {path}")),
            _ => return Err(format!("Ambiguous repository artifact: {path}")),
        };
        if !resolved.contains(&artifact.id) {
            resolved.push(artifact.id);
        }
    }
    Ok(resolved)
}

pub(crate) fn format_plan_task_edit(
    task: &dragent_workspace_model::PlanTask,
    plan: &dragent_workspace_model::Plan,
    roles: &[(String, dragent_workspace_model::WorkContractId)],
    criteria: &[dragent_workspace_model::AcceptanceCriterion],
) -> String {
    let mut lines = vec![
        format!("title={}", task.title),
        format!("description={}", task.description),
        format!(
            "role={}",
            task.assigned_contract_id
                .and_then(|id| roles.iter().find(|(_, contract_id)| *contract_id == id))
                .map(|(name, _)| name.as_str())
                .unwrap_or_default()
        ),
    ];
    lines.extend(task.depends_on.iter().filter_map(|id| {
        plan.tasks
            .iter()
            .find(|candidate| candidate.id == *id)
            .map(|dependency| format!("dependency={}", dependency.title))
    }));
    if let Some(target_id) = task.review_target_task_id {
        if let Some(target) = plan
            .tasks
            .iter()
            .find(|candidate| candidate.id == target_id)
        {
            lines.push(format!("review_target={}", target.title));
        }
    }
    lines.extend(task.acceptance_criteria.iter().filter_map(|id| {
        criteria
            .iter()
            .find(|criterion| criterion.id == *id)
            .map(|criterion| format!("criterion={}", criterion.description))
    }));
    lines.join("\n")
}

pub(crate) fn parse_plan_task_edit(
    value: &str,
    task_id: dragent_workspace_model::PlanTaskId,
    plan: &dragent_workspace_model::Plan,
    roles: &[(String, dragent_workspace_model::WorkContractId)],
    criteria: &[dragent_workspace_model::AcceptanceCriterion],
) -> Result<dragent_workspace_model::PlanTask, String> {
    let Some(mut task) = plan.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return Err("The plan task is no longer available".into());
    };
    let mut title = None;
    let mut description = None;
    let mut role = None;
    let mut dependencies = Vec::new();
    let mut review_target = None;
    let mut criterion_ids = Vec::new();
    for line in value.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((field, raw_value)) = line.split_once('=') else {
            return Err(format!("Plan task line must use field=value: {line}"));
        };
        let field = field.trim().to_ascii_lowercase();
        let raw_value = raw_value.trim();
        match field.as_str() {
            "title" => {
                if title.replace(raw_value.to_string()).is_some() {
                    return Err("title may only appear once".into());
                }
            }
            "description" => {
                if description.replace(raw_value.to_string()).is_some() {
                    return Err("description may only appear once".into());
                }
            }
            "role" => {
                if role.replace(raw_value.to_string()).is_some() {
                    return Err("role may only appear once".into());
                }
            }
            "dependency" => {
                let matches = plan
                    .tasks
                    .iter()
                    .filter(|candidate| candidate.title.eq_ignore_ascii_case(raw_value))
                    .collect::<Vec<_>>();
                let dependency = match matches.as_slice() {
                    [dependency] if dependency.id != task_id => *dependency,
                    [..] if matches.iter().any(|dependency| dependency.id == task_id) => {
                        return Err("A task cannot depend on itself".into());
                    }
                    [] => return Err(format!("Unknown dependency task: {raw_value}")),
                    _ => return Err(format!("Ambiguous dependency task: {raw_value}")),
                };
                if !dependencies.contains(&dependency.id) {
                    dependencies.push(dependency.id);
                }
            }
            "review_target" => {
                if review_target.is_some() {
                    return Err("review_target may only appear once".into());
                }
                let matches = plan
                    .tasks
                    .iter()
                    .filter(|candidate| candidate.title.eq_ignore_ascii_case(raw_value))
                    .collect::<Vec<_>>();
                review_target = Some(match matches.as_slice() {
                    [target] if target.id != task_id => target.id,
                    [..] if matches.iter().any(|target| target.id == task_id) => {
                        return Err("A task cannot review itself".into());
                    }
                    [] => return Err(format!("Unknown review target task: {raw_value}")),
                    _ => return Err(format!("Ambiguous review target task: {raw_value}")),
                });
            }
            "criterion" => {
                let matches = criteria
                    .iter()
                    .filter(|criterion| criterion.description.eq_ignore_ascii_case(raw_value))
                    .collect::<Vec<_>>();
                let criterion = match matches.as_slice() {
                    [criterion] => *criterion,
                    [] => return Err(format!("Unknown acceptance criterion: {raw_value}")),
                    _ => return Err(format!("Ambiguous acceptance criterion: {raw_value}")),
                };
                if !criterion_ids.contains(&criterion.id) {
                    criterion_ids.push(criterion.id);
                }
            }
            _ => return Err(format!("Unsupported plan task field: {field}")),
        }
    }
    let title = title.ok_or_else(|| "title is required".to_string())?;
    if title.trim().is_empty() {
        return Err("title must not be empty".into());
    }
    if plan
        .tasks
        .iter()
        .any(|candidate| candidate.id != task_id && candidate.title.eq_ignore_ascii_case(&title))
    {
        return Err(format!("Another plan task is already named: {title}"));
    }
    let role = role.ok_or_else(|| "role is required".to_string())?;
    let role_matches = roles
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(&role))
        .collect::<Vec<_>>();
    let assigned_contract_id = match role_matches.as_slice() {
        [(_, contract_id)] => *contract_id,
        [] => return Err(format!("Unknown proposed role: {role}")),
        _ => return Err(format!("Ambiguous proposed role: {role}")),
    };
    if criterion_ids.is_empty() {
        return Err("At least one criterion= entry is required".into());
    }
    task.title = title;
    task.description = description.unwrap_or_default();
    task.depends_on = dependencies;
    if review_target.is_some_and(|target| !task.depends_on.contains(&target)) {
        return Err("review_target must also be listed as a dependency".into());
    }
    task.review_target_task_id = review_target;
    task.acceptance_criteria = criterion_ids;
    task.assigned_contract_id = Some(assigned_contract_id);
    task.status = dragent_workspace_model::PlanTaskStatus::Proposed;
    Ok(task)
}

pub(crate) fn move_plan_task(
    plan: &mut dragent_workspace_model::Plan,
    task_id: dragent_workspace_model::PlanTaskId,
    direction: super::ShapePlanTaskMove,
) -> bool {
    let Some(index) = plan.tasks.iter().position(|task| task.id == task_id) else {
        return false;
    };
    let target = match direction {
        super::ShapePlanTaskMove::Up => index.checked_sub(1),
        super::ShapePlanTaskMove::Down => (index + 1 < plan.tasks.len()).then_some(index + 1),
    };
    let Some(target) = target else {
        return false;
    };
    plan.tasks.swap(index, target);
    true
}

pub(crate) fn remove_plan_task(
    plan: &mut dragent_workspace_model::Plan,
    task_id: dragent_workspace_model::PlanTaskId,
) -> Result<(), String> {
    if plan.tasks.len() <= 1 {
        return Err("A plan must retain at least one task".into());
    }
    let previous_len = plan.tasks.len();
    plan.tasks.retain(|task| task.id != task_id);
    if plan.tasks.len() == previous_len {
        return Err("The plan task is no longer available".into());
    }
    for task in &mut plan.tasks {
        task.depends_on.retain(|dependency| *dependency != task_id);
        if task.review_target_task_id == Some(task_id) {
            task.review_target_task_id = None;
        }
    }
    Ok(())
}

/// True once coordination has actually started — the point past which
/// `build_plan_revision_payload`'s full reset becomes destructive instead of
/// harmless. Everything before `Executing` is still Shaping: nothing has run,
/// so resetting plan/task/contract status to draft is exactly what should
/// happen when the human revises the plan there.
pub(crate) fn work_status_is_post_coordination(status: WorkStatus) -> bool {
    !matches!(
        status,
        WorkStatus::Draft
            | WorkStatus::Understanding
            | WorkStatus::WaitingForDecision
            | WorkStatus::ReadyToPlan
            | WorkStatus::Planning
            | WorkStatus::ReadyToExecute
    )
}

pub(crate) fn normalize_role_or_task_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(crate) fn format_role_permissions(
    permissions: &[dragent_workspace_model::PermissionGrant],
) -> String {
    let mut seen = HashSet::new();
    permissions
        .iter()
        .filter(|permission| {
            seen.insert((permission.permission.clone(), permission.requires_approval))
        })
        .map(|permission| {
            format!(
                "{}={}",
                permission.permission,
                if permission.requires_approval {
                    "ask"
                } else {
                    "allow"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn parse_role_permissions(
    value: &str,
    write_scope: &[PathBuf],
) -> Result<Vec<dragent_workspace_model::PermissionGrant>, String> {
    const BOUNDED_PERMISSIONS: [&str; 5] = [
        "project_read",
        "project_write",
        "command_execute",
        "agent_delegate",
        "external_write",
    ];
    let mut policies = HashMap::<String, bool>::new();
    for entry in value
        .lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((permission, policy)) = entry.split_once('=') else {
            return Err(format!(
                "Permission '{entry}' must use permission=ask or allow"
            ));
        };
        let permission = permission.trim().to_ascii_lowercase();
        if !BOUNDED_PERMISSIONS.contains(&permission.as_str()) {
            return Err(format!("Unsupported permission: {permission}"));
        }
        let requires_approval = match policy.trim().to_ascii_lowercase().as_str() {
            "ask" | "approval" | "require_approval" => true,
            "allow" | "automatic" => false,
            _ => return Err(format!("Permission '{permission}' must be ask or allow")),
        };
        if policies
            .insert(permission.clone(), requires_approval)
            .is_some()
        {
            return Err(format!("Permission '{permission}' appears more than once"));
        }
    }
    if policies.get("project_read") != Some(&false) {
        return Err("project_read=allow is required for every role".into());
    }
    if write_scope.is_empty() {
        if policies.contains_key("project_write") {
            return Err("project_write requires at least one validated write scope".into());
        }
    } else if !policies.contains_key("project_write") {
        return Err("A role with write scope must declare project_write=ask or allow".into());
    }

    let mut grants = Vec::new();
    grants.push(dragent_workspace_model::PermissionGrant {
        permission: "project_read".into(),
        scope: None,
        requires_approval: false,
    });
    if let Some(requires_approval) = policies.remove("project_write") {
        grants.extend(
            write_scope
                .iter()
                .map(|scope| dragent_workspace_model::PermissionGrant {
                    permission: "project_write".into(),
                    scope: Some(scope.to_string_lossy().into_owned()),
                    requires_approval,
                }),
        );
    }
    for permission in ["command_execute", "agent_delegate", "external_write"] {
        if let Some(requires_approval) = policies.remove(permission) {
            grants.push(dragent_workspace_model::PermissionGrant {
                permission: permission.into(),
                scope: None,
                requires_approval,
            });
        }
    }
    Ok(grants)
}

#[cfg(test)]
mod shape_parse_tests {
    use super::{
        format_plan_task_edit, format_role_permissions, format_shape_budget, move_plan_task,
        normalize_role_or_task_key, parse_execution_location, parse_input_artifacts,
        parse_plan_task_edit, parse_preferred_model_route, parse_role_permissions,
        parse_shape_budget, parse_success_criteria, remove_plan_task,
    };
    use crate::workspace::ShapePlanTaskMove;
    use dragent_workspace_model::{
        AcceptanceCriterion, AcceptanceCriterionId, AcceptanceStatus, ActorId, ArtifactId,
        ArtifactKind, ArtifactRef, ExecutionLocation, Plan, PlanId, PlanStatus, PlanTask,
        PlanTaskId, PlanTaskStatus, Project, ProjectId, WorkBudget, WorkContractId, WorkItemId,
        WorkspaceState,
    };
    use std::path::PathBuf;

    #[test]
    fn budget_editor_round_trips_all_contract_limits() {
        let budget = WorkBudget {
            maximum_input_tokens: Some(12_000),
            maximum_output_tokens: Some(4_000),
            maximum_cost_micros: Some(2_500_000),
            maximum_duration_ms: Some(900_000),
            maximum_tool_calls: Some(80),
        };

        assert_eq!(
            parse_shape_budget(&format_shape_budget(&budget)),
            Ok(budget)
        );
    }

    #[test]
    fn budget_editor_accepts_aliases_and_explicit_unset_values() {
        let budget = parse_shape_budget(
            "input=10_000, output_tokens=2000\ncost=unset\nduration=60000\ntools=25",
        )
        .unwrap();

        assert_eq!(budget.maximum_input_tokens, Some(10_000));
        assert_eq!(budget.maximum_output_tokens, Some(2_000));
        assert_eq!(budget.maximum_cost_micros, None);
        assert_eq!(budget.maximum_duration_ms, Some(60_000));
        assert_eq!(budget.maximum_tool_calls, Some(25));
    }

    #[test]
    fn budget_editor_rejects_ambiguous_or_invalid_limits() {
        assert!(parse_shape_budget("input=100\ninput=200").is_err());
        assert!(parse_shape_budget("surprises=3").is_err());
        assert!(parse_shape_budget("tools=0").is_err());
        assert!(parse_shape_budget("duration=tomorrow").is_err());
    }

    #[test]
    fn execution_location_editor_accepts_only_domain_locations() {
        assert_eq!(
            parse_execution_location(" local "),
            Ok(ExecutionLocation::Local)
        );
        assert_eq!(
            parse_execution_location("CLOUD"),
            Ok(ExecutionLocation::Cloud)
        );
        assert_eq!(
            parse_execution_location("Hybrid"),
            Ok(ExecutionLocation::Hybrid)
        );
        assert!(parse_execution_location("automatic").is_err());
    }

    #[test]
    fn model_route_editor_distinguishes_auto_from_an_explicit_route() {
        assert_eq!(parse_preferred_model_route("Auto"), Ok(None));
        assert_eq!(parse_preferred_model_route(""), Ok(None));
        assert_eq!(
            parse_preferred_model_route("anthropic/claude-review"),
            Ok(Some("anthropic/claude-review".into()))
        );
        assert!(parse_preferred_model_route("not a model").is_err());
    }

    #[test]
    fn success_criteria_editor_resolves_only_current_canonical_records() {
        let work_item_id = WorkItemId::new();
        let first_id = AcceptanceCriterionId::new();
        let second_id = AcceptanceCriterionId::new();
        let criterion = |id, description: &str| AcceptanceCriterion {
            id,
            work_item_id,
            description: description.into(),
            status: AcceptanceStatus::Pending,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: Vec::new(),
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        let criteria = vec![
            criterion(first_id, "API tests pass"),
            criterion(second_id, "Review has no blockers"),
        ];

        assert_eq!(
            parse_success_criteria("API tests pass\nreview has no blockers", &criteria),
            Ok(vec![first_id, second_id])
        );
        assert_eq!(
            parse_success_criteria("API tests pass\nAPI tests pass", &criteria),
            Ok(vec![first_id])
        );
        assert!(parse_success_criteria("", &criteria).is_err());
        assert!(parse_success_criteria("Invent a new criterion", &criteria).is_err());
    }

    #[test]
    fn input_artifact_editor_resolves_only_registered_repository_paths() {
        let work_item_id = WorkItemId::new();
        let first_id = ArtifactId::new();
        let second_id = ArtifactId::new();
        let artifacts = vec![
            ArtifactRef {
                id: first_id,
                work_item_id,
                relative_path: PathBuf::from("docs/requirements.md"),
                kind: ArtifactKind::Requirements,
            },
            ArtifactRef {
                id: second_id,
                work_item_id,
                relative_path: PathBuf::from("src/api.rs"),
                kind: ArtifactKind::Other,
            },
        ];

        assert_eq!(
            parse_input_artifacts("docs/requirements.md\nsrc/api.rs", &artifacts),
            Ok(vec![first_id, second_id])
        );
        assert_eq!(parse_input_artifacts("", &artifacts), Ok(Vec::new()));
        assert!(parse_input_artifacts("../outside.md", &artifacts).is_err());
        assert!(parse_input_artifacts("docs/missing.md", &artifacts).is_err());
    }

    #[test]
    fn permission_editor_is_bounded_by_scope_and_explicit_consent_policy() {
        let scopes = vec![PathBuf::from("app"), PathBuf::from("crates/core")];
        let permissions = parse_role_permissions(
            "project_read=allow\nproject_write=allow\ncommand_execute=ask",
            &scopes,
        )
        .unwrap();
        assert_eq!(permissions.len(), 4);
        assert_eq!(permissions[1].scope.as_deref(), Some("app"));
        assert!(!permissions[1].requires_approval);
        assert!(permissions[3].requires_approval);
        assert_eq!(
            parse_role_permissions(&format_role_permissions(&permissions), &scopes),
            Ok(permissions)
        );

        assert!(parse_role_permissions("project_read=allow", &scopes).is_err());
        assert!(parse_role_permissions("project_read=allow\nproject_write=ask", &[]).is_err());
        assert!(parse_role_permissions("secrets=allow", &[]).is_err());
        assert!(parse_role_permissions("project_read=ask", &[]).is_err());
    }

    #[test]
    fn plan_task_editor_round_trips_typed_links_and_rejects_unknown_values() {
        let work_item_id = WorkItemId::new();
        let plan_id = PlanId::new();
        let implement_id = PlanTaskId::new();
        let review_id = PlanTaskId::new();
        let implement_contract = WorkContractId::new();
        let review_contract = WorkContractId::new();
        let criterion_id = AcceptanceCriterionId::new();
        let criterion = AcceptanceCriterion {
            id: criterion_id,
            work_item_id,
            description: "Review evidence is accepted".into(),
            status: AcceptanceStatus::Pending,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: Vec::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let plan = Plan {
            id: plan_id,
            work_item_id,
            version: 3,
            status: PlanStatus::Approved,
            tasks: vec![
                PlanTask {
                    id: implement_id,
                    plan_id,
                    title: "Implement feature".into(),
                    description: "Build the bounded change".into(),
                    status: PlanTaskStatus::Proposed,
                    depends_on: Vec::new(),
                    review_target_task_id: None,
                    acceptance_criteria: vec![criterion_id],
                    assigned_contract_id: Some(implement_contract),
                },
                PlanTask {
                    id: review_id,
                    plan_id,
                    title: "Review feature".into(),
                    description: "Check the implementation".into(),
                    status: PlanTaskStatus::Proposed,
                    depends_on: vec![implement_id],
                    review_target_task_id: Some(implement_id),
                    acceptance_criteria: vec![criterion_id],
                    assigned_contract_id: Some(review_contract),
                },
            ],
            approved_by: Some(dragent_workspace_model::ActorId::new()),
            approved_at_unix_ms: Some(2),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 2,
        };
        let roles = vec![
            ("Developer".into(), implement_contract),
            ("Reviewer".into(), review_contract),
        ];
        let criteria = vec![criterion];
        let formatted = format_plan_task_edit(&plan.tasks[1], &plan, &roles, &criteria);
        let parsed = parse_plan_task_edit(&formatted, review_id, &plan, &roles, &criteria).unwrap();
        assert_eq!(parsed, plan.tasks[1]);

        assert!(parse_plan_task_edit(
            "title=Review feature\ndescription=Review\nrole=Unknown\ncriterion=Review evidence is accepted",
            review_id,
            &plan,
            &roles,
            &criteria,
        )
        .is_err());
        assert!(parse_plan_task_edit(
            "title=Review feature\ndescription=Review\nrole=Reviewer\ndependency=Review feature\ncriterion=Review evidence is accepted",
            review_id,
            &plan,
            &roles,
            &criteria,
        )
        .is_err());

        let mut reordered = plan.clone();
        assert!(move_plan_task(
            &mut reordered,
            review_id,
            ShapePlanTaskMove::Up
        ));
        assert_eq!(reordered.tasks[0].id, review_id);
        assert!(!move_plan_task(
            &mut reordered,
            review_id,
            ShapePlanTaskMove::Up
        ));

        let mut reduced = plan;
        remove_plan_task(&mut reduced, implement_id).unwrap();
        assert_eq!(reduced.tasks.len(), 1);
        assert!(reduced.tasks[0].depends_on.is_empty());
        assert!(remove_plan_task(&mut reduced, review_id).is_err());
    }

    #[test]
    fn normalize_role_or_task_key_folds_case_and_whitespace() {
        assert_eq!(
            normalize_role_or_task_key("  Backend   Engineer "),
            normalize_role_or_task_key("backend engineer")
        );
    }
}
