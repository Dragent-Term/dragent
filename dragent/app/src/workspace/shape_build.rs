//! Shaping record builders for the workspace.
//!
//! Responsible for: building plan revisions, proposed teams/roles,
//! model catalogs, memory submission, contract comparison.
//! Pure construction from validated inputs; no View dependencies.

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
use super::shape_parse::{move_plan_task, normalize_role_or_task_key, remove_plan_task};

pub(crate) fn build_plan_revision_payload(
    workspace_state: &WorkspaceState,
    mut plan: dragent_workspace_model::Plan,
    actor_id: dragent_workspace_model::ActorId,
    now: i64,
) -> Result<WorkspaceCommandPayload, String> {
    let team = workspace_state
        .latest_proposed_team()
        .ok_or_else(|| "A proposed team is required".to_string())?;
    let roles = team
        .roles
        .iter()
        .filter_map(|role| {
            workspace_state
                .role_definition(role.role_definition_id)
                .map(|definition| (definition.name.clone(), role.work_contract_id))
        })
        .collect::<Vec<_>>();

    plan.version = plan.version.saturating_add(1);
    plan.status = dragent_workspace_model::PlanStatus::Draft;
    plan.approved_by = None;
    plan.approved_at_unix_ms = None;
    plan.updated_at_unix_ms = now;
    for task in &mut plan.tasks {
        task.status = dragent_workspace_model::PlanTaskStatus::Proposed;
    }
    plan.validate()
        .map_err(|error| format!("Invalid plan revision: {error}"))?;
    if let Some(records) = workspace_state.shaping_records_projection() {
        if let Some(uncovered) = records.acceptance_criteria.iter().find(|criterion| {
            !plan
                .tasks
                .iter()
                .any(|task| task.acceptance_criteria.contains(&criterion.id))
        }) {
            return Err(format!(
                "Acceptance criterion '{}' must remain covered by at least one task",
                uncovered.description
            ));
        }
    }

    let mut work_contracts = team
        .roles
        .iter()
        .filter_map(|role| workspace_state.work_contract(role.work_contract_id))
        .cloned()
        .collect::<Vec<_>>();
    if work_contracts.len() != team.roles.len() {
        return Err("A proposed role contract is no longer available".into());
    }
    for contract in &mut work_contracts {
        contract.task_ids.clear();
        contract.success_criteria.clear();
    }
    for task in &plan.tasks {
        let contract_id = task
            .assigned_contract_id
            .ok_or_else(|| format!("Task '{}' has no assigned role", task.title))?;
        let contract = work_contracts
            .iter_mut()
            .find(|contract| contract.id == contract_id)
            .ok_or_else(|| format!("Task '{}' references an unknown role", task.title))?;
        contract.task_ids.push(task.id);
        for criterion_id in &task.acceptance_criteria {
            if !contract.success_criteria.contains(criterion_id) {
                contract.success_criteria.push(*criterion_id);
            }
        }
    }
    for contract in &mut work_contracts {
        if contract.task_ids.is_empty() {
            let role_name = roles
                .iter()
                .find(|(_, id)| *id == contract.id)
                .map(|(name, _)| name.as_str())
                .unwrap_or("A proposed role");
            return Err(format!("{role_name} must retain at least one plan task"));
        }
        contract.version = contract.version.saturating_add(1);
        contract.status = dragent_workspace_model::WorkContractStatus::Proposed;
        contract.agent_instance_id = None;
        contract.updated_by = actor_id;
        contract.updated_at_unix_ms = now;
    }
    Ok(WorkspaceCommandPayload::RevisePlan {
        plan,
        work_contracts,
    })
}

/// Non-destructive counterpart to `build_plan_revision_payload`, used once
/// coordination has started. That function force-resets `plan.status` to
/// `Draft`, every task's `status` back to `Proposed`, and every contract's
/// `status`/`agent_instance_id` — correct pre-coordination (nothing has run
/// yet), but it would silently freeze every in-flight agent if used mid-run:
/// `evaluate_scheduler` refuses to launch anything while `plan.status !=
/// Approved`, and `UpsertWorkContract` validation rejects a contract that
/// still has `agent_instance_id` set. This variant touches only the plan —
/// no `WorkContract` needs to change for a new task to become launchable,
/// since `evaluate_scheduler`'s readiness check only reads
/// `task.assigned_contract_id`/`contract.status`, never `contract.task_ids`.
/// `plan` must already contain every edit (same convention as
/// `build_plan_revision_payload`); this only bumps the version, validates,
/// and leaves `status`/`approved_by`/`approved_at_unix_ms` and every other
/// task's `status` untouched.
pub(crate) fn append_plan_task_payload(
    workspace_state: &WorkspaceState,
    mut plan: dragent_workspace_model::Plan,
    now: i64,
) -> Result<WorkspaceCommandPayload, String> {
    plan.version = plan.version.saturating_add(1);
    plan.updated_at_unix_ms = now;
    plan.validate()
        .map_err(|error| format!("Invalid plan revision: {error}"))?;
    if let Some(records) = workspace_state.shaping_records_projection() {
        if let Some(uncovered) = records.acceptance_criteria.iter().find(|criterion| {
            !plan
                .tasks
                .iter()
                .any(|task| task.acceptance_criteria.contains(&criterion.id))
        }) {
            return Err(format!(
                "Acceptance criterion '{}' must remain covered by at least one task",
                uncovered.description
            ));
        }
    }
    Ok(WorkspaceCommandPayload::UpsertPlan { plan })
}

/// Resolve the Principal's plan-task drafts (from a live coordination turn's
/// `add_tasks`, or a `propose_roles` entry's own tasks) into real `PlanTask`s
/// — same key/dependency/acceptance-criteria resolution `proposed_team_records`
/// uses during Shaping, trimmed for the always-new case: these tasks never
/// existed before, so there's no prior revision's identity to preserve.
pub(crate) fn resolve_principal_plan_task_drafts(
    plan: &dragent_workspace_model::Plan,
    role_contract_by_name: &HashMap<String, dragent_workspace_model::WorkContractId>,
    acceptance_criteria: &[dragent_workspace_model::AcceptanceCriterion],
    drafts: &[super::shape_principal::PrincipalPlanTaskDraft],
) -> Result<Vec<dragent_workspace_model::PlanTask>, String> {
    let existing_by_title: HashMap<String, dragent_workspace_model::PlanTaskId> = plan
        .tasks
        .iter()
        .map(|task| (normalize_role_or_task_key(&task.title), task.id))
        .collect();
    let new_by_key: HashMap<String, dragent_workspace_model::PlanTaskId> = drafts
        .iter()
        .map(|draft| {
            (
                draft.key.clone(),
                dragent_workspace_model::PlanTaskId(Uuid::new_v5(&plan.id.0, draft.key.as_bytes())),
            )
        })
        .collect();
    let resolve_ref = |reference: &str| -> Result<dragent_workspace_model::PlanTaskId, String> {
        new_by_key
            .get(reference)
            .copied()
            .or_else(|| {
                existing_by_title
                    .get(&normalize_role_or_task_key(reference))
                    .copied()
            })
            .ok_or_else(|| format!("Unknown plan task reference '{reference}'"))
    };
    let criteria_by_description: HashMap<&str, dragent_workspace_model::AcceptanceCriterionId> =
        acceptance_criteria
            .iter()
            .map(|criterion| (criterion.description.as_str(), criterion.id))
            .collect();
    drafts
        .iter()
        .map(|draft| {
            let contract_id = role_contract_by_name
                .get(&normalize_role_or_task_key(&draft.assigned_role))
                .copied()
                .ok_or_else(|| format!("Unknown assigned role '{}'", draft.assigned_role))?;
            let depends_on = draft
                .depends_on
                .iter()
                .map(|reference| resolve_ref(reference))
                .collect::<Result<Vec<_>, _>>()?;
            let review_target_task_id = draft
                .review_target
                .as_deref()
                .map(resolve_ref)
                .transpose()?;
            let acceptance_criteria_ids = draft
                .acceptance_criteria
                .iter()
                .filter_map(|description| {
                    criteria_by_description.get(description.as_str()).copied()
                })
                .collect();
            Ok(dragent_workspace_model::PlanTask {
                id: new_by_key[&draft.key],
                plan_id: plan.id,
                title: draft.title.clone(),
                description: draft.description.clone(),
                status: dragent_workspace_model::PlanTaskStatus::Proposed,
                depends_on,
                review_target_task_id,
                acceptance_criteria: acceptance_criteria_ids,
                assigned_contract_id: Some(contract_id),
            })
        })
        .collect()
}

/// The user's own rule for auto-spin-up: a role proposed mid-coordination
/// starts immediately with no human approval click only if none of its
/// tasks depend on anything not already `Completed` — anywhere in the plan,
/// not just among its own new tasks. A role with such a dependency still
/// needs the existing one-click Approve, same as a role proposed during
/// Shaping.
pub(crate) fn role_tasks_are_dependency_free(
    tasks: &[dragent_workspace_model::PlanTask],
    completed: &HashSet<dragent_workspace_model::PlanTaskId>,
) -> bool {
    tasks
        .iter()
        .all(|task| task.depends_on.iter().all(|dep| completed.contains(dep)))
}

/// Construct a brand-new role's `RoleDefinition` + `WorkContract` from the
/// Principal's draft — same deterministic-id and permission/capability shape
/// `proposed_team_records` builds during Shaping, trimmed to exactly one new
/// role with no prior revision to carry over (`version: 1`,
/// `status: Proposed` — the caller decides whether to promote it to
/// `Approved` per the dependency-free auto-start rule). `success_criteria`
/// is left empty; the caller fills it in once the role's own tasks are
/// resolved, since `WorkContract::validate()` requires at least one.
pub(crate) fn build_proposed_role_records(
    project_id: dragent_workspace_model::ProjectId,
    work_item_id: WorkItemId,
    role: super::shape_principal::PrincipalRoleDraft,
    principal_id: dragent_workspace_model::ActorId,
    now_unix_ms: i64,
) -> (
    dragent_workspace_model::RoleDefinition,
    dragent_workspace_model::WorkContract,
) {
    let identity = normalize_role_or_task_key(&role.name);
    let role_definition_id = dragent_workspace_model::RoleDefinitionId(Uuid::new_v5(
        &project_id.0,
        format!("role-definition:{identity}").as_bytes(),
    ));
    let work_contract_id = dragent_workspace_model::WorkContractId(Uuid::new_v5(
        &work_item_id.0,
        format!("work-contract:{}", role_definition_id.0).as_bytes(),
    ));
    let role_definition = dragent_workspace_model::RoleDefinition {
        id: role_definition_id,
        scope: dragent_workspace_model::RoleDefinitionScope::Project(project_id),
        kind: role.kind,
        name: role.name.clone(),
        mission: role.mission.clone(),
        capabilities: role
            .capabilities
            .iter()
            .map(
                |capability| dragent_workspace_model::CapabilityRequirement {
                    capability: capability.clone(),
                    reason: role.rationale.clone(),
                    required: true,
                },
            )
            .collect(),
        preferred_tools: role.tools.clone(),
        created_by: principal_id,
        created_at_unix_ms: now_unix_ms,
        updated_at_unix_ms: now_unix_ms,
    };
    let permissions = if role.write_scope.is_empty() {
        vec![dragent_workspace_model::PermissionGrant {
            permission: "project_read".into(),
            scope: None,
            requires_approval: false,
        }]
    } else {
        let mut permissions = vec![dragent_workspace_model::PermissionGrant {
            permission: "project_read".into(),
            scope: None,
            requires_approval: false,
        }];
        permissions.extend(role.write_scope.iter().map(|scope| {
            dragent_workspace_model::PermissionGrant {
                permission: "project_write".into(),
                scope: Some(scope.clone()),
                requires_approval: true,
            }
        }));
        permissions
    };
    let write_scope = role.write_scope.iter().map(PathBuf::from).collect();
    let work_contract = dragent_workspace_model::WorkContract {
        id: work_contract_id,
        work_item_id,
        role_definition_id,
        version: 1,
        agent_instance_id: None,
        status: dragent_workspace_model::WorkContractStatus::Proposed,
        mission: role.mission,
        task_ids: Vec::new(),
        input_artifact_ids: Vec::new(),
        expected_outputs: vec!["Evidence-backed completion report".into()],
        success_criteria: Vec::new(),
        write_scope,
        capabilities: role
            .capabilities
            .into_iter()
            .map(|capability| dragent_workspace_model::CapabilityGrant {
                capability,
                scope: None,
            })
            .collect(),
        tools: role.tools,
        permissions,
        budget: dragent_workspace_model::WorkBudget::default(),
        escalation_rules: vec!["Escalate blockers and scope changes to the Principal".into()],
        execution_location: dragent_workspace_model::ExecutionLocation::Local,
        preferred_model_route: role.preferred_model_route,
        created_by: principal_id,
        updated_by: principal_id,
        created_at_unix_ms: now_unix_ms,
        updated_at_unix_ms: now_unix_ms,
    };
    (role_definition, work_contract)
}

pub(crate) fn shape_model_route_catalog(
    app: &AppContext,
) -> (Vec<super::shape_workspace::ModelRouteOption>, bool) {
    let settings = crate::settings::AppSettings::global();
    let account = crate::settings::AccountStore::handle(app);
    let (mut models, restricted_to_free) = app.read_model(&account, |store, _| {
        store.data.as_ref().map_or_else(
            || (Vec::new(), false),
            |data| {
                (
                    data.models
                        .iter()
                        .filter(|model| model.is_active)
                        .cloned()
                        .collect::<Vec<_>>(),
                    data.usage.model_locked_to_free || data.user.tier.eq_ignore_ascii_case("free"),
                )
            },
        )
    });
    for custom in &settings.custom_models {
        models.push(crate::settings::AccountModelInfo {
            id: custom.model_id.clone(),
            name: custom.name.clone(),
            provider: "custom".into(),
            context_window: 128_000,
            min_tier: "free".into(),
            is_active: true,
        });
    }

    let has_byok = settings.has_any_user_credential();
    if crate::api::is_local_mode() {
        models = crate::settings::local_models::local_mode_models(&settings);
    } else if has_byok {
        models.retain(|model| settings.has_user_credential_for_model(&model.id));
    }
    let locked = restricted_to_free && !has_byok;
    // Agent CLIs work on the person's own account with them, on any tier.
    #[cfg(not(target_family = "wasm"))]
    let agents = super::cli_workers::agent_route_options();
    #[cfg(target_family = "wasm")]
    let agents = Vec::new();
    if locked {
        return (agents, true);
    }

    let mut seen = HashSet::new();
    let mut routes: Vec<_> = models
        .into_iter()
        .filter(|model| seen.insert(model.id.clone()))
        .map(|model| super::shape_workspace::ModelRouteOption {
            route: model.id,
            label: model.name,
            detail: format!(
                "{} · {}K context · {} tier",
                model.provider,
                (model.context_window.max(0) as u64) / 1_000,
                model.min_tier
            ),
        })
        .collect();
    routes.extend(agents);
    (routes, false)
}

pub(crate) fn shape_composer_model_options(
    app: &AppContext,
) -> (
    Vec<super::work_item_surface::ShapeComposerModelRow>,
    String,
    bool,
) {
    use super::work_item_surface::ShapeComposerModelRow;
    let settings = crate::settings::AppSettings::global();
    let account = crate::settings::AccountStore::handle(app);
    let mut models: Vec<(String, String)> = app.read_model(&account, |store, _| {
        store
            .data
            .as_ref()
            .map(|d| {
                d.models
                    .iter()
                    .filter(|m| m.is_active)
                    .map(|m| (m.id.clone(), m.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    });
    for cm in &settings.custom_models {
        models.push((cm.model_id.clone(), cm.name.clone()));
    }
    models.insert(
        0,
        (
            crate::settings::AUTO_MODEL_ID.to_string(),
            "Auto".to_string(),
        ),
    );
    let has_byok = settings.has_any_user_credential();
    if crate::api::is_local_mode() {
        models = crate::settings::local_models::local_mode_choices(&settings);
    } else if has_byok {
        models.retain(|(id, _)| {
            id == crate::settings::AUTO_MODEL_ID || settings.has_user_credential_for_model(id)
        });
    }
    let (restricted, _credits_offered, _is_free_tier, _free_model_name) =
        app.read_model(&account, |store, _| {
            store
                .data
                .as_ref()
                .map(|d| {
                    let free = d
                        .models
                        .iter()
                        .find(|m| m.min_tier == "free")
                        .map(|m| m.name.clone());
                    (
                        d.usage.model_locked_to_free || d.user.tier.eq_ignore_ascii_case("free"),
                        d.usage.deposit_balance_mc > 0 && !d.usage.allow_credit_usage,
                        d.user.tier == "free",
                        free,
                    )
                })
                .unwrap_or((false, false, false, None))
        });
    let locked = restricted && !has_byok;
    let selected_id = settings.selected_model.clone();
    let selected_effort = settings.selected_effort;
    let expanded: Vec<(String, String, crate::settings::AgentEffort)> = models
        .into_iter()
        .flat_map(|(id, name)| {
            crate::settings::AgentEffort::rows_for(&id)
                .iter()
                .map(|e| {
                    let label = match e.wire_value() {
                        None => name.clone(),
                        Some(level) => format!("{name} ({level})"),
                    };
                    (id.clone(), label, *e)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let display = expanded
        .iter()
        .find(|(id, _, e)| *id == selected_id && *e == selected_effort)
        .map(|(_, name, _)| name.clone())
        .unwrap_or_else(|| selected_id.clone());
    let rows = expanded
        .into_iter()
        .map(|(id, label, effort)| {
            let selected = id == selected_id
                && (id == crate::settings::AUTO_MODEL_ID || effort == selected_effort);
            ShapeComposerModelRow {
                id,
                label,
                effort_wire: effort.wire_value().unwrap_or("auto").to_string(),
                selected,
            }
        })
        .collect();
    (rows, display, locked)
}

/// Best-effort recording of one turn's memory notes into the canonical
/// brain. Never fails the turn: notes are already normalized and bounded by
/// the parser, and authority rejection only logs — the next turn still works
/// (the live digest covers the gap). Returns the committed events so the
/// caller can project them and advance its revision.
#[allow(clippy::too_many_arguments)]
pub(crate) fn submit_principal_memory(
    writer: &WriterHandles,
    project_id: dragent_workspace_model::ProjectId,
    work_item_id: WorkItemId,
    principal: &dragent_workspace_model::ActorRef,
    expected_revision: u64,
    notes: Vec<super::shape_principal::PrincipalMemoryNote>,
) -> Option<Vec<WorkspaceEventEnvelope>> {
    if notes.is_empty() {
        return None;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let memory = notes
        .into_iter()
        .map(|note| dragent_workspace_model::ShapingMemoryNote {
            id: dragent_workspace_model::ShapingMemoryNoteId::new(),
            work_item_id,
            kind: dragent_workspace_model::ShapingMemoryKind::parse(&note.kind),
            text: note.text,
            recorded_at_unix_ms: now,
        })
        .collect::<Vec<_>>();
    let command = WorkspaceCommandEnvelope {
        command_id: WorkspaceCommandId::new(),
        schema_version: WORKSPACE_PROTOCOL_VERSION,
        project_id,
        work_item_id,
        actor_id: principal.id,
        device_id: principal.device_id,
        correlation_id: CorrelationId::new(),
        timestamp_unix_ms: now,
        expected_revision: Some(expected_revision),
        payload: WorkspaceCommandPayload::RecordShapingMemory { notes: memory },
    };
    match writer.accept_workspace_command(command) {
        Ok(events) => Some(events),
        Err(error) => {
            log::warn!("[workspace-shape] could not record Principal memory: {error:#}");
            None
        }
    }
}

/// Substantive contract equality for approval carry-over across shaping
/// revisions. Identity/version/timestamps are excluded by design: the
/// revision always bumps the version and restamps, while approval follows
/// the terms. Role-definition kind/name are covered transitively — the
/// contract id itself derives from the role name, so a renamed role is a
/// different contract and never matches.
pub(crate) fn contract_terms_unchanged(
    revised: &dragent_workspace_model::WorkContract,
    current: &dragent_workspace_model::WorkContract,
) -> bool {
    revised.work_item_id == current.work_item_id
        && revised.role_definition_id == current.role_definition_id
        && revised.agent_instance_id == current.agent_instance_id
        && revised.mission == current.mission
        && revised.task_ids == current.task_ids
        && revised.input_artifact_ids == current.input_artifact_ids
        && revised.expected_outputs == current.expected_outputs
        && revised.success_criteria == current.success_criteria
        && revised.write_scope == current.write_scope
        && revised.capabilities == current.capabilities
        && revised.tools == current.tools
        && revised.permissions == current.permissions
        && revised.budget == current.budget
        && revised.escalation_rules == current.escalation_rules
        && revised.execution_location == current.execution_location
        && revised.preferred_model_route == current.preferred_model_route
}

pub(crate) fn proposed_team_records(
    project_id: dragent_workspace_model::ProjectId,
    work_item_id: WorkItemId,
    revision: u64,
    current_plan: Option<&dragent_workspace_model::Plan>,
    current_team: Option<&dragent_workspace_model::ProposedTeam>,
    current_role_definitions: &[dragent_workspace_model::RoleDefinition],
    current_work_contracts: &[dragent_workspace_model::WorkContract],
    roles: Vec<super::shape_principal::PrincipalRoleDraft>,
    plan_tasks: Vec<super::shape_principal::PrincipalPlanTaskDraft>,
    acceptance_criteria: &[dragent_workspace_model::AcceptanceCriterion],
    input_artifact_ids: &[dragent_workspace_model::ArtifactId],
    principal_id: dragent_workspace_model::ActorId,
    now_unix_ms: i64,
) -> Result<
    (
        Vec<dragent_workspace_model::RoleDefinition>,
        Vec<dragent_workspace_model::WorkContract>,
        dragent_workspace_model::ProposedTeam,
        dragent_workspace_model::Plan,
    ),
    String,
> {
    let mut role_definitions = Vec::with_capacity(roles.len());
    let mut work_contracts = Vec::with_capacity(roles.len());
    let mut proposed_roles = Vec::with_capacity(roles.len());
    let mut contract_by_role = HashMap::new();
    let all_success_criteria = acceptance_criteria
        .iter()
        .map(|criterion| criterion.id)
        .collect::<Vec<_>>();
    for role in roles {
        let identity = role
            .name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let role_definition_id = dragent_workspace_model::RoleDefinitionId(Uuid::new_v5(
            &project_id.0,
            format!("role-definition:{identity}").as_bytes(),
        ));
        let work_contract_id = dragent_workspace_model::WorkContractId(Uuid::new_v5(
            &work_item_id.0,
            format!("work-contract:{}", role_definition_id.0).as_bytes(),
        ));
        if contract_by_role
            .insert(identity, work_contract_id)
            .is_some()
        {
            return Err("Proposed role names must be unique".into());
        }
        let mut role_definition = dragent_workspace_model::RoleDefinition {
            id: role_definition_id,
            scope: dragent_workspace_model::RoleDefinitionScope::Project(project_id),
            kind: role.kind,
            name: role.name,
            mission: role.mission.clone(),
            capabilities: role
                .capabilities
                .iter()
                .map(
                    |capability| dragent_workspace_model::CapabilityRequirement {
                        capability: capability.clone(),
                        reason: role.rationale.clone(),
                        required: true,
                    },
                )
                .collect(),
            preferred_tools: role.tools.clone(),
            created_by: principal_id,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        };
        if let Some(current) = current_role_definitions
            .iter()
            .find(|current| current.id == role_definition_id)
        {
            role_definition.created_by = current.created_by;
            role_definition.created_at_unix_ms = current.created_at_unix_ms;
        }
        role_definition
            .validate()
            .map_err(|error| error.to_string())?;
        let permissions = if role.write_scope.is_empty() {
            vec![dragent_workspace_model::PermissionGrant {
                permission: "project_read".into(),
                scope: None,
                requires_approval: false,
            }]
        } else {
            let mut permissions = vec![dragent_workspace_model::PermissionGrant {
                permission: "project_read".into(),
                scope: None,
                requires_approval: false,
            }];
            permissions.extend(role.write_scope.iter().map(|scope| {
                dragent_workspace_model::PermissionGrant {
                    permission: "project_write".into(),
                    scope: Some(scope.clone()),
                    requires_approval: true,
                }
            }));
            permissions
        };
        let write_scope = role.write_scope.into_iter().map(PathBuf::from).collect();
        let mut work_contract = dragent_workspace_model::WorkContract {
            id: work_contract_id,
            work_item_id,
            role_definition_id,
            version: 1,
            agent_instance_id: None,
            status: dragent_workspace_model::WorkContractStatus::Proposed,
            mission: role.mission,
            task_ids: Vec::new(),
            input_artifact_ids: input_artifact_ids.to_vec(),
            expected_outputs: vec!["Evidence-backed completion report".into()],
            success_criteria: all_success_criteria.clone(),
            write_scope,
            capabilities: role
                .capabilities
                .into_iter()
                .map(|capability| dragent_workspace_model::CapabilityGrant {
                    capability,
                    scope: None,
                })
                .collect(),
            tools: role.tools,
            permissions,
            budget: dragent_workspace_model::WorkBudget::default(),
            escalation_rules: vec!["Escalate blockers and scope changes to the Principal".into()],
            execution_location: dragent_workspace_model::ExecutionLocation::Local,
            preferred_model_route: role.preferred_model_route,
            created_by: principal_id,
            updated_by: principal_id,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        };
        if let Some(current) = current_work_contracts
            .iter()
            .find(|current| current.id == work_contract_id)
        {
            work_contract.version = current.version.saturating_add(1);
            work_contract.expected_outputs = current.expected_outputs.clone();
            work_contract.budget = current.budget.clone();
            work_contract.escalation_rules = current.escalation_rules.clone();
            work_contract.execution_location = current.execution_location;
            work_contract.created_by = current.created_by;
            work_contract.created_at_unix_ms = current.created_at_unix_ms;
        }
        proposed_roles.push(dragent_workspace_model::ProposedTeamRole {
            role_definition_id,
            work_contract_id,
            rationale: role.rationale,
        });
        role_definitions.push(role_definition);
        work_contracts.push(work_contract);
    }
    let mut proposed_team = dragent_workspace_model::ProposedTeam {
        id: dragent_workspace_model::TeamId(Uuid::new_v5(&work_item_id.0, b"proposed-team")),
        work_item_id,
        version: revision,
        roles: proposed_roles,
        status: dragent_workspace_model::StructuredRecordStatus::Draft,
        created_by: principal_id,
        created_at_unix_ms: now_unix_ms,
        updated_at_unix_ms: now_unix_ms,
    };
    if let Some(current) = current_team.filter(|team| team.id == proposed_team.id) {
        proposed_team.version = current.version.saturating_add(1);
        proposed_team.created_by = current.created_by;
        proposed_team.created_at_unix_ms = current.created_at_unix_ms;
    }
    proposed_team
        .validate()
        .map_err(|error| error.to_string())?;

    let plan_id = dragent_workspace_model::PlanId(Uuid::new_v5(&work_item_id.0, b"plan"));
    let current_task_ids_by_title = current_plan
        .map(|plan| {
            plan.tasks
                .iter()
                .map(|task| {
                    (
                        task.title
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .to_lowercase(),
                        task.id,
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let task_ids = plan_tasks
        .iter()
        .map(|task| {
            let normalized_title = task
                .title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            (
                task.key.clone(),
                current_plan
                    .and_then(|plan| {
                        Uuid::parse_str(&task.key).ok().and_then(|id| {
                            plan.tasks
                                .iter()
                                .any(|current| current.id.0 == id)
                                .then_some(dragent_workspace_model::PlanTaskId(id))
                        })
                    })
                    .or_else(|| current_task_ids_by_title.get(&normalized_title).copied())
                    .unwrap_or_else(|| {
                        dragent_workspace_model::PlanTaskId(Uuid::new_v5(
                            &plan_id.0,
                            task.key.as_bytes(),
                        ))
                    }),
            )
        })
        .collect::<HashMap<_, _>>();
    let criteria_by_description = acceptance_criteria
        .iter()
        .map(|criterion| (criterion.description.as_str(), criterion.id))
        .collect::<HashMap<_, _>>();
    let mut contract_tasks = HashMap::<
        dragent_workspace_model::WorkContractId,
        Vec<dragent_workspace_model::PlanTaskId>,
    >::new();
    let mut contract_criteria = HashMap::<
        dragent_workspace_model::WorkContractId,
        Vec<dragent_workspace_model::AcceptanceCriterionId>,
    >::new();
    let mut tasks = Vec::with_capacity(plan_tasks.len());
    for task in plan_tasks {
        let task_id = task_ids[&task.key];
        let role_identity = task
            .assigned_role
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let contract_id = contract_by_role
            .get(&role_identity)
            .copied()
            .ok_or_else(|| format!("Unknown assigned role '{}'", task.assigned_role))?;
        let depends_on = task
            .depends_on
            .iter()
            .map(|dependency| {
                task_ids
                    .get(dependency)
                    .copied()
                    .ok_or_else(|| format!("Unknown plan dependency '{dependency}'"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let review_target_task_id = task
            .review_target
            .as_ref()
            .map(|target| {
                task_ids
                    .get(target)
                    .copied()
                    .ok_or_else(|| format!("Unknown review target '{target}'"))
            })
            .transpose()?;
        let criterion_ids = task
            .acceptance_criteria
            .iter()
            .map(|criterion| {
                criteria_by_description
                    .get(criterion.as_str())
                    .copied()
                    .ok_or_else(|| format!("Unknown acceptance criterion '{criterion}'"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        contract_tasks.entry(contract_id).or_default().push(task_id);
        for criterion_id in &criterion_ids {
            let criteria = contract_criteria.entry(contract_id).or_default();
            if !criteria.contains(criterion_id) {
                criteria.push(*criterion_id);
            }
        }
        tasks.push(dragent_workspace_model::PlanTask {
            id: task_id,
            plan_id,
            title: task.title,
            description: task.description,
            status: dragent_workspace_model::PlanTaskStatus::Proposed,
            depends_on,
            review_target_task_id,
            acceptance_criteria: criterion_ids,
            assigned_contract_id: Some(contract_id),
        });
    }
    for contract in &mut work_contracts {
        contract.task_ids = contract_tasks.remove(&contract.id).unwrap_or_default();
        contract.success_criteria = contract_criteria.remove(&contract.id).unwrap_or_default();
        if contract.task_ids.is_empty() {
            return Err(format!(
                "Role contract {} has no assigned plan task",
                contract.id.0
            ));
        }
        // Iterative refinement must not wipe approvals: when the revision
        // leaves a contract's terms untouched, its approval (and approver)
        // carries over, so the user only re-approves roles that actually
        // changed. Any substantive change still resets to Proposed, exactly
        // like a manual edit does.
        if let Some(current) = current_work_contracts
            .iter()
            .find(|current| current.id == contract.id)
        {
            if contract_terms_unchanged(contract, current) {
                contract.status = current.status;
                contract.updated_by = current.updated_by;
            }
        }
        contract.validate().map_err(|error| error.to_string())?;
    }
    let mut plan = dragent_workspace_model::Plan {
        id: plan_id,
        work_item_id,
        version: current_plan
            .map(|plan| plan.version.saturating_add(1))
            .unwrap_or(1),
        status: dragent_workspace_model::PlanStatus::Draft,
        tasks,
        approved_by: None,
        approved_at_unix_ms: None,
        created_at_unix_ms: now_unix_ms,
        updated_at_unix_ms: now_unix_ms,
    };
    if let Some(current) = current_plan.filter(|current| current.id == plan.id) {
        plan.created_at_unix_ms = current.created_at_unix_ms;
    }
    plan.validate().map_err(|error| error.to_string())?;
    Ok((role_definitions, work_contracts, proposed_team, plan))
}

#[cfg(test)]
mod shape_contract_tests {
    use super::super::shape_parse::{
        format_plan_task_edit, format_role_permissions, format_shape_budget, move_plan_task,
        normalize_role_or_task_key, parse_execution_location, parse_input_artifacts,
        parse_plan_task_edit, parse_preferred_model_route, parse_role_permissions,
        parse_shape_budget, parse_success_criteria, remove_plan_task,
    };
    use super::{
        append_plan_task_payload, build_proposed_role_records, resolve_principal_plan_task_drafts,
        role_tasks_are_dependency_free,
    };
    use crate::workspace::ShapePlanTaskMove;
    use dragent_session_protocol::workspace::WorkspaceCommandPayload;
    use dragent_workspace_model::{
        AcceptanceCriterion, AcceptanceCriterionId, AcceptanceStatus, ActorId, ArtifactId,
        ArtifactKind, ArtifactRef, ExecutionLocation, Plan, PlanId, PlanStatus, PlanTask,
        PlanTaskId, PlanTaskStatus, Project, ProjectId, WorkBudget, WorkContractId, WorkItemId,
        WorkspaceState,
    };
    use std::path::PathBuf;

    #[test]
    fn revision_preserves_approval_for_unchanged_contracts() {
        use super::{contract_terms_unchanged, proposed_team_records};
        use crate::workspace::shape_principal::{PrincipalPlanTaskDraft, PrincipalRoleDraft};
        use dragent_workspace_model::{ActorId, AgentRoleKind, ProjectId, WorkContractStatus};

        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let principal = ActorId::new();
        let criterion = AcceptanceCriterion {
            id: AcceptanceCriterionId::new(),
            work_item_id,
            description: "Done".into(),
            status: AcceptanceStatus::Pending,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: Vec::new(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let criteria = std::slice::from_ref(&criterion);
        let role = || PrincipalRoleDraft {
            kind: AgentRoleKind::Worker,
            name: "Builder".into(),
            mission: "Build it".into(),
            rationale: "Owns building".into(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            write_scope: Vec::new(),
            preferred_model_route: None,
        };
        let task = || PrincipalPlanTaskDraft {
            key: "build".into(),
            title: "Build it".into(),
            description: "Build the thing".into(),
            depends_on: Vec::new(),
            review_target: None,
            assigned_role: "Builder".into(),
            acceptance_criteria: vec!["Done".into()],
        };
        let (defs, contracts, team, plan) = proposed_team_records(
            project_id,
            work_item_id,
            1,
            None,
            None,
            &[],
            &[],
            vec![role()],
            vec![task()],
            criteria,
            &[],
            principal,
            10,
        )
        .unwrap();
        assert_eq!(contracts[0].status, WorkContractStatus::Proposed);
        assert!(contract_terms_unchanged(&contracts[0], &contracts[0]));

        // Approve, then revise with identical terms: approval carries over.
        let mut approved = contracts[0].clone();
        approved.status = WorkContractStatus::Approved;
        let (_, contracts, _, _) = proposed_team_records(
            project_id,
            work_item_id,
            2,
            Some(&plan),
            Some(&team),
            &defs,
            &[approved.clone()],
            vec![role()],
            vec![task()],
            criteria,
            &[],
            principal,
            20,
        )
        .unwrap();
        assert_eq!(contracts[0].version, 2);
        assert_eq!(contracts[0].status, WorkContractStatus::Approved);

        // A changed mission resets to Proposed, like a manual edit does.
        let mut changed = role();
        changed.mission = "Build it differently".into();
        let (_, contracts, _, _) = proposed_team_records(
            project_id,
            work_item_id,
            2,
            Some(&plan),
            Some(&team),
            &defs,
            &[approved],
            vec![changed],
            vec![task()],
            criteria,
            &[],
            principal,
            30,
        )
        .unwrap();
        assert_eq!(contracts[0].status, WorkContractStatus::Proposed);
    }

    #[test]
    fn append_plan_task_payload_never_touches_plan_status_or_other_tasks() {
        // Regression: build_plan_revision_payload force-resets plan.status to
        // Draft and every task's status to Proposed — correct pre-coordination,
        // but it would silently freeze the scheduler (evaluate_scheduler refuses
        // to launch anything while plan.status != Approved) and orphan an
        // already-running task if used mid-run. append_plan_task_payload must
        // leave both alone.
        let project = Project {
            organisation_id: "local".into(),
            id: ProjectId::new(),
            root: PathBuf::from("/workspace/append-plan-task-test"),
            display_name: "test".into(),
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        let state = WorkspaceState::new(project);
        let plan_id = PlanId::new();
        let running_task_id = PlanTaskId::new();
        let new_task_id = PlanTaskId::new();
        let plan = Plan {
            id: plan_id,
            work_item_id: WorkItemId::new(),
            version: 3,
            status: PlanStatus::Approved,
            approved_by: Some(ActorId::new()),
            approved_at_unix_ms: Some(1),
            created_at_unix_ms: 0,
            updated_at_unix_ms: 1,
            tasks: vec![
                PlanTask {
                    id: running_task_id,
                    plan_id,
                    title: "Already running".into(),
                    description: String::new(),
                    status: PlanTaskStatus::Running,
                    depends_on: Vec::new(),
                    review_target_task_id: None,
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: Some(WorkContractId::new()),
                },
                PlanTask {
                    id: new_task_id,
                    plan_id,
                    title: "New task".into(),
                    description: String::new(),
                    status: PlanTaskStatus::Proposed,
                    depends_on: Vec::new(),
                    review_target_task_id: None,
                    acceptance_criteria: Vec::new(),
                    assigned_contract_id: None,
                },
            ],
        };

        let payload = append_plan_task_payload(&state, plan.clone(), 99).unwrap();
        let WorkspaceCommandPayload::UpsertPlan { plan: revised } = payload else {
            panic!("append_plan_task_payload must produce UpsertPlan")
        };

        assert_eq!(revised.status, PlanStatus::Approved);
        assert_eq!(revised.approved_by, plan.approved_by);
        assert_eq!(revised.approved_at_unix_ms, plan.approved_at_unix_ms);
        assert_eq!(revised.version, plan.version + 1);
        assert_eq!(
            revised
                .tasks
                .iter()
                .find(|task| task.id == running_task_id)
                .expect("running task still present")
                .status,
            PlanTaskStatus::Running
        );
        assert!(revised.tasks.iter().any(|task| task.id == new_task_id));
    }

    #[test]
    fn resolve_principal_plan_task_drafts_resolves_sibling_and_existing_task_references() {
        use crate::workspace::shape_principal::PrincipalPlanTaskDraft;
        use std::collections::HashMap;

        let plan_id = PlanId::new();
        let existing_task_id = PlanTaskId::new();
        let plan = Plan {
            id: plan_id,
            work_item_id: WorkItemId::new(),
            version: 1,
            status: PlanStatus::Approved,
            approved_by: Some(ActorId::new()),
            approved_at_unix_ms: Some(1),
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            tasks: vec![PlanTask {
                id: existing_task_id,
                plan_id,
                title: "Design schema".into(),
                description: String::new(),
                status: PlanTaskStatus::Completed,
                depends_on: Vec::new(),
                review_target_task_id: None,
                acceptance_criteria: Vec::new(),
                assigned_contract_id: None,
            }],
        };
        let contract_id = WorkContractId::new();
        let mut role_contract_by_name = HashMap::new();
        role_contract_by_name.insert("backend".to_string(), contract_id);
        let done_id = AcceptanceCriterionId::new();
        let criteria = vec![AcceptanceCriterion {
            id: done_id,
            work_item_id: plan.work_item_id,
            description: "Done".into(),
            status: AcceptanceStatus::Pending,
            evidence_ids: Vec::new(),
            waiver_reason: None,
            waived_by: None,
            provenance: Vec::new(),
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        }];
        let drafts = vec![
            PrincipalPlanTaskDraft {
                key: "task-a".into(),
                title: "Write migration".into(),
                description: "Add the column".into(),
                depends_on: vec!["Design schema".into()],
                review_target: None,
                assigned_role: "Backend".into(),
                acceptance_criteria: vec!["Done".into()],
            },
            PrincipalPlanTaskDraft {
                key: "task-b".into(),
                title: "Write tests".into(),
                description: "Cover the migration".into(),
                depends_on: vec!["task-a".into()],
                review_target: None,
                assigned_role: "Backend".into(),
                acceptance_criteria: Vec::new(),
            },
        ];

        let resolved =
            resolve_principal_plan_task_drafts(&plan, &role_contract_by_name, &criteria, &drafts)
                .expect("resolves");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].depends_on, vec![existing_task_id]);
        assert_eq!(resolved[0].acceptance_criteria, vec![done_id]);
        assert_eq!(resolved[0].assigned_contract_id, Some(contract_id));
        assert_eq!(resolved[1].depends_on, vec![resolved[0].id]);
        assert_eq!(resolved[1].assigned_contract_id, Some(contract_id));
    }

    #[test]
    fn resolve_principal_plan_task_drafts_rejects_an_unknown_role() {
        use crate::workspace::shape_principal::PrincipalPlanTaskDraft;
        use std::collections::HashMap;

        let plan_id = PlanId::new();
        let plan = Plan {
            id: plan_id,
            work_item_id: WorkItemId::new(),
            version: 1,
            status: PlanStatus::Approved,
            approved_by: None,
            approved_at_unix_ms: None,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
            tasks: Vec::new(),
        };
        let drafts = vec![PrincipalPlanTaskDraft {
            key: "task-a".into(),
            title: "Do something".into(),
            description: String::new(),
            depends_on: Vec::new(),
            review_target: None,
            assigned_role: "Nonexistent Role".into(),
            acceptance_criteria: Vec::new(),
        }];
        assert!(resolve_principal_plan_task_drafts(&plan, &HashMap::new(), &[], &drafts).is_err());
    }

    #[test]
    fn role_tasks_are_dependency_free_requires_every_dependency_completed() {
        let plan_id = PlanId::new();
        let dep_id = PlanTaskId::new();
        let task = |depends_on: Vec<PlanTaskId>| PlanTask {
            id: PlanTaskId::new(),
            plan_id,
            title: "t".into(),
            description: String::new(),
            status: PlanTaskStatus::Proposed,
            depends_on,
            review_target_task_id: None,
            acceptance_criteria: Vec::new(),
            assigned_contract_id: None,
        };
        assert!(role_tasks_are_dependency_free(
            &[task(Vec::new())],
            &std::collections::HashSet::new()
        ));
        assert!(!role_tasks_are_dependency_free(
            &[task(vec![dep_id])],
            &std::collections::HashSet::new()
        ));
        assert!(role_tasks_are_dependency_free(
            &[task(vec![dep_id])],
            &std::collections::HashSet::from([dep_id])
        ));
    }

    #[test]
    fn build_proposed_role_records_leaves_success_criteria_for_the_caller() {
        use crate::workspace::shape_principal::PrincipalRoleDraft;
        use dragent_workspace_model::{AgentRoleKind, WorkContractStatus};

        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let principal_id = ActorId::new();
        let draft = PrincipalRoleDraft {
            kind: AgentRoleKind::Worker,
            name: "Database Migrator".into(),
            mission: "Own the schema migration".into(),
            rationale: "No one else is scoped for it".into(),
            capabilities: Vec::new(),
            tools: Vec::new(),
            write_scope: vec!["migrations/".into()],
            preferred_model_route: None,
        };
        let (role_definition, work_contract) = build_proposed_role_records(
            project_id,
            work_item_id,
            draft,
            principal_id,
            1_700_000_000_000,
        );
        assert_eq!(role_definition.name, "Database Migrator");
        assert_eq!(role_definition.kind, AgentRoleKind::Worker);
        assert_eq!(work_contract.mission, "Own the schema migration");
        assert_eq!(work_contract.status, WorkContractStatus::Proposed);
        assert_eq!(work_contract.role_definition_id, role_definition.id);
        // The caller must fill this in before submitting — WorkContract::
        // validate() rejects an empty list.
        assert!(work_contract.success_criteria.is_empty());
    }
}
