//! Durable, metadata-only storage for the engineering workspace projection.

use crate::load_canonical_workspace_events;
use crate::load_pending_workspace_integration_intents;
use crate::save_workspace_work_item;
use crate::store_agent::activate_agent_instance;
use crate::store_agent::ensure_work_completion_ready;
use crate::store_agent::load_scheduler_authority_state;
use crate::store_agent::prepare_agent_execution_retry;
use crate::store_agent::renew_execution_lease;
use crate::store_agent::update_agent_message_delivery;
use crate::store_agent::validate_acceptance_criterion_evidence;
use crate::store_agent::validate_agent_message_authority;
use crate::store_agent::validate_evidence_authority;
use crate::store_execution::ensure_coordination_is_ready;
use crate::store_execution::finalize_accepted_task;
use crate::store_execution::finish_agent_execution;
use crate::store_execution::provision_agent_instance;
use crate::store_execution::raise_attention;
use crate::store_execution::resolve_agent_execution_review;
use crate::store_execution::resolve_attention;
use crate::store_execution::start_agent_execution;
use crate::store_execution::transition_plan_task;
use crate::store_integration::insert_canonical_outcome;
use crate::store_integration::load_workspace_integration_intent;
use crate::store_integration::mark_workspace_integration_completed;
use crate::store_integration::next_canonical_sequence;
use crate::store_outcomes::insert_compact_projection_events;
use crate::store_outcomes::insert_workspace_tool_outcome;
use crate::store_outcomes::load_command_outcome;
use crate::store_outcomes::resolve_workspace_tool_outcomes_from_command;
use crate::store_projects::load_workspace_work_item;
use crate::workspace_store::MAX_CANONICAL_REPLAY_EVENTS;
use crate::WorkspaceToolOutcome;
use anyhow::{Context, Result};
use diesel::prelude::*;
use dragent_session_protocol::workspace::{
    WorkspaceCommandEnvelope, WorkspaceCommandPayload, WorkspaceEventEnvelope, WorkspaceEventId,
    WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    AcceptanceCriterionId, AcceptanceStatus, ActorRef, EvidenceId, ExecutionAttemptId,
    ExecutionAttemptStatus, ExecutionReviewResolution, Plan, PlanTaskStatus, ProjectId,
    RoleDefinition, RoleDefinitionId, ShapingRecords, WorkContract, WorkContractId,
    WorkContractStatus, WorkItem, WorkItemId, WorkStatus, WorkspaceEventKind,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Read-only preflight before touching the main checkout. Acceptance repeats
/// these checks inside its authority transaction after filesystem integration.
pub fn preflight_execution_acceptance(
    conn: &mut diesel::sqlite::SqliteConnection,
    project: ProjectId,
    work: WorkItemId,
    actor: dragent_workspace_model::ActorId,
    id: ExecutionAttemptId,
    resolution: ExecutionReviewResolution,
    now: i64,
) -> Result<Vec<String>> {
    let revision = next_canonical_sequence(conn, work)?.saturating_sub(1);
    let (agent, contract, _, attempt, plan) =
        resolve_agent_execution_review(conn, project, work, actor, id, resolution, now, revision)?;
    let (evidence, _, _, _) = finalize_accepted_task(
        conn, project, work, actor, &attempt, &contract, &plan, agent.id, resolution, now, revision,
    )?;
    let receipts = crate::host_verification::load(conn, work, id)?;
    Ok(receipts
        .into_iter()
        .filter(|r| {
            evidence
                .iter()
                .any(|e| e.source == format!("host-command://{}", r.id))
        })
        .filter_map(|r| r.snapshot_after)
        .collect())
}

/// Commit one coordination decision and all of its effects together. A crash
/// must never leave an answered message whose requested plan changes were lost.
/// Each command still goes through the ordinary semantic authority.
pub fn accept_local_workspace_commands(
    conn: &mut diesel::sqlite::SqliteConnection,
    commands: &[WorkspaceCommandEnvelope],
) -> Result<Vec<WorkspaceEventEnvelope>> {
    accept_local_workspace_commands_with_tool_outcome(conn, commands, None)
}

/// Accept a workspace command batch and its provider-delivery intent in one
/// SQLite transaction. Recovery can therefore observe either neither effect or
/// both the canonical mutation and the exact tool call/outcome that owns it.
pub fn accept_local_workspace_commands_with_tool_outcome(
    conn: &mut diesel::sqlite::SqliteConnection,
    commands: &[WorkspaceCommandEnvelope],
    tool_outcome: Option<&WorkspaceToolOutcome>,
) -> Result<Vec<WorkspaceEventEnvelope>> {
    let first = commands
        .first()
        .context("workspace command batch is empty")?;
    if commands.len() > 64
        || commands.iter().any(|command| {
            command.project_id != first.project_id || command.work_item_id != first.work_item_id
        })
    {
        anyhow::bail!("workspace command batch must contain at most 64 commands for one work item");
    }
    if let Some(outcome) = tool_outcome {
        if outcome.project_id != first.project_id
            || outcome.work_item_id != first.work_item_id
            || !commands
                .iter()
                .any(|command| command.command_id == outcome.command_id)
        {
            anyhow::bail!("workspace tool outcome must match a command in the accepted batch");
        }
        if outcome.runtime_run_id.trim().is_empty()
            || outcome.runtime_execution_id.trim().is_empty()
            || outcome.provider_call_id.trim().is_empty()
            || outcome.runtime_round == 0
            || outcome.output.is_some() != outcome.is_error.is_some()
        {
            anyhow::bail!("workspace tool outcome identity or result is incomplete");
        }
    }
    conn.transaction::<_, anyhow::Error, _>(|conn| {
        let mut events: Vec<WorkspaceEventEnvelope> = Vec::new();
        for command in commands {
            let mut command = command.clone();
            if let Some(previous) = events.last() {
                command.expected_revision = Some(previous.sequence);
            }
            events.extend(accept_local_workspace_command(conn, &command)?);
            resolve_workspace_tool_outcomes_from_command(conn, &command)?;
        }
        if let Some(outcome) = tool_outcome {
            insert_workspace_tool_outcome(conn, outcome)?;
        }
        Ok(events)
    })
}

/// Atomically accept an append-safe message under local workspace authority.
/// Identical command retries return the original event; command-id collisions
/// with different content are rejected.
pub fn accept_local_workspace_command(
    conn: &mut diesel::sqlite::SqliteConnection,
    command: &WorkspaceCommandEnvelope,
) -> Result<Vec<WorkspaceEventEnvelope>> {
    command
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid workspace command: {error}"))?;
    conn.transaction::<_, anyhow::Error, _>(|conn| {
        if let Some(events) = load_command_outcome(conn, command)? {
            return Ok(events);
        }

        // A prepared Git integration is a canonical revision fence for its
        // work item. Without this, another accepted command can advance the
        // revision after the intent is committed but before its Git apply is
        // acknowledged, guaranteeing a database failure after filesystem
        // mutation. Only the exact journaled completion may cross the fence.
        if let Some(fenced) = load_pending_workspace_integration_intents(conn)?
            .into_iter()
            .find(|intent| intent.work_item_id == command.work_item_id)
        {
            anyhow::ensure!(
                fenced.command == *command && fenced.state == "prepared",
                "workspace integration is pending canonical completion for this work item"
            );
        }

        let integration_attempt = match &command.payload {
            WorkspaceCommandPayload::ResolveAgentExecutionReview {
                attempt_id,
                resolution: ExecutionReviewResolution::Integrated,
            } => {
                let intent = load_workspace_integration_intent(conn, *attempt_id)?
                    .context("integrated completion requires a durable Git integration intent")?;
                anyhow::ensure!(
                    intent.command_id == command.command_id
                        && intent.project_id == command.project_id
                        && intent.work_item_id == command.work_item_id
                        && intent.command == *command
                        && intent.state == "prepared",
                    "integrated completion does not match its prepared Git integration intent"
                );
                Some(*attempt_id)
            }
            _ => None,
        };

        let first_sequence = next_canonical_sequence(conn, command.work_item_id)?;
        if command.payload.requires_expected_revision() {
            let expected = command
                .expected_revision
                .context("strict workspace command requires expected_revision")?;
            let current = first_sequence.saturating_sub(1);
            if expected != current {
                anyhow::bail!(
                    "workspace revision conflict: expected {expected}, current {current}"
                );
            }
        }
        if let WorkspaceCommandPayload::UpsertPlan { plan } = &command.payload {
            if command.actor_id == ActorRef::principal_agent(command.project_id).id
                && plan.approved_by != Some(command.actor_id)
            {
                let (current, _, _, _, _) = load_scheduler_authority_state(
                    conn, command.project_id, command.work_item_id, first_sequence.saturating_sub(1),
                )?;
                dragent_workspace_model::validate_principal_plan_refinement(&current, plan)
                    .map_err(anyhow::Error::msg)?;
            }
        }
        if let WorkspaceCommandPayload::UpsertWorkContract { work_contract } = &command.payload {
            if !work_contract.input_artifact_ids.is_empty() {
                let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS)
                    .unwrap_or(usize::MAX);
                if first_sequence.saturating_sub(1) > replay_limit as u64 {
                    anyhow::bail!(
                        "work contract artifact validation requires a canonical snapshot after {replay_limit} events"
                    );
                }
                let mut known_artifacts = HashSet::new();
                for event in load_canonical_workspace_events(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    0,
                    replay_limit,
                )? {
                    match event.payload {
                        WorkspaceEventPayload::ArtifactUpdated { artifact } => {
                            known_artifacts.insert(artifact.id);
                        }
                        WorkspaceEventPayload::ArtifactsReplaced { artifacts } => {
                            known_artifacts = artifacts
                                .into_iter()
                                .map(|artifact| artifact.id)
                                .collect();
                        }
                        _ => {}
                    }
                }
                if let Some(unknown) = work_contract
                    .input_artifact_ids
                    .iter()
                    .find(|artifact_id| !known_artifacts.contains(artifact_id))
                {
                    anyhow::bail!("work contract references unknown input artifact {unknown:?}");
                }
            }
            let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
            let definition = load_canonical_workspace_events(
                conn,
                command.project_id,
                command.work_item_id,
                0,
                replay_limit,
            )?
            .into_iter()
            .filter_map(|event| match event.payload {
                WorkspaceEventPayload::RoleDefinitionUpdated { role_definition }
                    if role_definition.id == work_contract.role_definition_id =>
                {
                    Some(role_definition)
                }
                _ => None,
            })
            .last();
            if definition.is_some_and(|definition| {
                definition.kind == dragent_workspace_model::AgentRoleKind::Reviewer
                    && !work_contract.write_scope.is_empty()
            })
            {
                anyhow::bail!("reviewer work contracts must remain read-only");
            }
        }
        if let WorkspaceCommandPayload::UpsertRoleDefinition { role_definition } = &command.payload
        {
            if role_definition.kind == dragent_workspace_model::AgentRoleKind::Reviewer {
                let replay_limit =
                    usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
                let has_writable_contract = load_canonical_workspace_events(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    0,
                    replay_limit,
                )?
                .into_iter()
                .filter_map(|event| match event.payload {
                    WorkspaceEventPayload::WorkContractUpdated { work_contract }
                        if work_contract.role_definition_id == role_definition.id =>
                    {
                        Some(work_contract)
                    }
                    _ => None,
                })
                .last()
                .is_some_and(|contract| !contract.write_scope.is_empty());
                if has_writable_contract {
                    anyhow::bail!(
                        "a role with a writable contract cannot become an independent reviewer"
                    );
                }
            }
        }
        if let WorkspaceCommandPayload::RecordEvidence { evidence } = &command.payload {
            validate_evidence_authority(
                conn,
                command.project_id,
                command.work_item_id,
                evidence,
                command.actor_id,
                command.timestamp_unix_ms,
                first_sequence.saturating_sub(1),
            )?;
        }
        if let WorkspaceCommandPayload::RecordShapingMemory { notes } = &command.payload {
            for note in notes {
                if note.work_item_id != command.work_item_id
                    || note.recorded_at_unix_ms != command.timestamp_unix_ms
                {
                    anyhow::bail!(
                        "shaping memory notes must preserve command scope and timestamp"
                    );
                }
            }
        }
        if let WorkspaceCommandPayload::UpsertAcceptanceCriterion {
            acceptance_criterion,
        } = &command.payload
        {
            if matches!(
                acceptance_criterion.status,
                AcceptanceStatus::Satisfied | AcceptanceStatus::Waived
            ) {
                validate_acceptance_criterion_evidence(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    acceptance_criterion,
                    command.actor_id,
                    first_sequence.saturating_sub(1),
                )?;
            }
        }
        if let WorkspaceCommandPayload::RevisePlan {
            plan,
            work_contracts,
        } = &command.payload
        {
            let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
            let definitions = load_canonical_workspace_events(
                conn,
                command.project_id,
                command.work_item_id,
                0,
                replay_limit,
            )?
            .into_iter()
            .filter_map(|event| match event.payload {
                WorkspaceEventPayload::RoleDefinitionUpdated { role_definition } => {
                    Some((role_definition.id, role_definition))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
            if work_contracts.iter().any(|contract| {
                !contract.write_scope.is_empty()
                    && definitions
                        .get(&contract.role_definition_id)
                        .is_some_and(|definition| {
                            definition.kind == dragent_workspace_model::AgentRoleKind::Reviewer
                        })
            }) {
                anyhow::bail!("reviewer work contracts must remain read-only");
            }
            let contracts = work_contracts
                .iter()
                .map(|contract| (contract.id, contract))
                .collect::<HashMap<_, _>>();
            for task in &plan.tasks {
                let role_kind = task
                    .assigned_contract_id
                    .and_then(|id| contracts.get(&id).copied())
                    .and_then(|contract| definitions.get(&contract.role_definition_id))
                    .map(|definition| definition.kind)
                    .unwrap_or_default();
                if (role_kind == dragent_workspace_model::AgentRoleKind::Reviewer)
                    != task.review_target_task_id.is_some()
                {
                    anyhow::bail!(
                        "reviewer tasks require exactly one explicit review target and worker tasks cannot declare one"
                    );
                }
            }
        }
        if let WorkspaceCommandPayload::CompleteShapingTurn {
            proposal,
            role_definitions,
            work_contracts,
            proposed_team: Some(proposed_team),
            plan: Some(plan),
            ..
        } = &command.payload
        {
            let work = load_workspace_work_item(
                conn,
                command.project_id,
                command.work_item_id,
            )?
            .context("cannot complete shaping for a missing work item")?;
            if !matches!(work.status, WorkStatus::Understanding | WorkStatus::Planning) {
                anyhow::bail!("shaping can only be superseded while work is being understood or planned");
            }
            let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
            if first_sequence.saturating_sub(1) > replay_limit as u64 {
                anyhow::bail!(
                    "shaping supersession validation requires a canonical snapshot after {replay_limit} events"
                );
            }
            let mut current_plan = None;
            let mut current_team = None;
            let mut current_proposal_revision = None;
            let mut current_contracts = HashMap::<WorkContractId, WorkContract>::new();
            let mut current_definitions = HashMap::<RoleDefinitionId, RoleDefinition>::new();
            for event in load_canonical_workspace_events(
                conn,
                command.project_id,
                command.work_item_id,
                0,
                replay_limit,
            )? {
                match event.payload {
                    WorkspaceEventPayload::PlanUpdated { plan } => current_plan = Some(plan),
                    WorkspaceEventPayload::PlanTaskStatusChanged { plan, .. } => {
                        current_plan = Some(plan)
                    }
                    WorkspaceEventPayload::ProposedTeamUpdated { proposed_team } => {
                        current_team = Some(proposed_team)
                    }
                    WorkspaceEventPayload::ShapingProposalUpdated { proposal } => {
                        current_proposal_revision = Some(proposal.revision)
                    }
                    WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                        current_contracts.insert(work_contract.id, work_contract);
                    }
                    WorkspaceEventPayload::RoleDefinitionUpdated { role_definition } => {
                        current_definitions.insert(role_definition.id, role_definition);
                    }
                    _ => {}
                }
            }
            if let Some(current_revision) = current_proposal_revision {
                if proposal.revision != current_revision.saturating_add(1) {
                    anyhow::bail!("a shaping supersession must increment the proposal revision once");
                }
            } else if proposal.revision != 1 {
                anyhow::bail!("the first shaping proposal must start at revision one");
            }
            if let Some(current) = current_plan {
                if current.tasks.iter().any(|task| task.status != PlanTaskStatus::Proposed) {
                    anyhow::bail!("shaping cannot be superseded after task execution has started");
                }
                if plan.id != current.id
                    || plan.version != current.version.saturating_add(1)
                    || plan.created_at_unix_ms != current.created_at_unix_ms
                {
                    anyhow::bail!("a shaping supersession must preserve plan identity and increment its version once");
                }
            } else if plan.version != 1 {
                anyhow::bail!("the first shaped plan must start at version one");
            }
            if let Some(current) = current_team {
                if proposed_team.id != current.id
                    || proposed_team.version != current.version.saturating_add(1)
                    || proposed_team.created_at_unix_ms != current.created_at_unix_ms
                {
                    anyhow::bail!("a shaping supersession must preserve team identity and increment its version once");
                }
            } else if proposed_team.version != 1 {
                anyhow::bail!("the first proposed team must start at version one");
            }
            for contract in work_contracts {
                if let Some(current) = current_contracts.get(&contract.id) {
                    if contract.version != current.version.saturating_add(1)
                        || contract.created_by != current.created_by
                        || contract.created_at_unix_ms != current.created_at_unix_ms
                    {
                        anyhow::bail!("a retained shaping contract must preserve identity and increment its version once");
                    }
                } else if contract.version != 1 {
                    anyhow::bail!("a new shaping contract must start at version one");
                }
            }
            for definition in role_definitions {
                if let Some(current) = current_definitions.get(&definition.id) {
                    if definition.created_by != current.created_by
                        || definition.created_at_unix_ms != current.created_at_unix_ms
                    {
                        anyhow::bail!("a retained role definition must preserve its creation attribution");
                    }
                }
            }
            if plan.updated_at_unix_ms != command.timestamp_unix_ms
                || proposed_team.updated_at_unix_ms != command.timestamp_unix_ms
                || work_contracts
                    .iter()
                    .any(|contract| contract.updated_at_unix_ms != command.timestamp_unix_ms)
                || role_definitions
                    .iter()
                    .any(|definition| definition.updated_at_unix_ms != command.timestamp_unix_ms)
            {
                anyhow::bail!("shaping supersession records must use the command timestamp");
            }
        }
        if matches!(
            command.payload,
            WorkspaceCommandPayload::ReplaceShapingRecords { .. }
                | WorkspaceCommandPayload::ReplaceArtifacts { .. }
        ) {
            let work = load_workspace_work_item(conn, command.project_id, command.work_item_id)?
                .context("cannot revise shaping data for a missing work item")?;
            if !matches!(
                work.status,
                WorkStatus::Draft | WorkStatus::Understanding | WorkStatus::Planning
            ) {
                anyhow::bail!("shaping data can only be revised while work is being understood or planned");
            }
            let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
            if first_sequence.saturating_sub(1) > replay_limit as u64 {
                anyhow::bail!("shaping data revision requires a canonical snapshot after {replay_limit} events");
            }
            let events = load_canonical_workspace_events(
                conn,
                command.project_id,
                command.work_item_id,
                0,
                replay_limit,
            )?;
            let mut current_records: Option<ShapingRecords> = None;
            let mut current_artifacts = HashMap::new();
            let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
            let mut plan: Option<Plan> = None;
            for event in events {
                match event.payload {
                    WorkspaceEventPayload::ShapingRecordsUpdated { records } => {
                        current_records = Some(records);
                    }
                    WorkspaceEventPayload::RequirementUpdated { requirement } => {
                        if let Some(records) = &mut current_records {
                            if let Some(existing) = records.requirements.iter_mut().find(|item| item.id == requirement.id) {
                                *existing = requirement;
                            } else {
                                records.requirements.push(requirement);
                            }
                        }
                    }
                    WorkspaceEventPayload::DecisionUpdated { decision } => {
                        if let Some(records) = &mut current_records {
                            if let Some(existing) = records.decisions.iter_mut().find(|item| item.id == decision.id) {
                                *existing = decision;
                            } else {
                                records.decisions.push(decision);
                            }
                        }
                    }
                    WorkspaceEventPayload::AcceptanceCriterionUpdated { acceptance_criterion } => {
                        if let Some(records) = &mut current_records {
                            if let Some(existing) = records.acceptance_criteria.iter_mut().find(|item| item.id == acceptance_criterion.id) {
                                *existing = acceptance_criterion;
                            } else {
                                records.acceptance_criteria.push(acceptance_criterion);
                            }
                        }
                    }
                    WorkspaceEventPayload::ArtifactUpdated { artifact } => {
                        current_artifacts.insert(artifact.id, artifact);
                    }
                    WorkspaceEventPayload::ArtifactsReplaced { artifacts } => {
                        current_artifacts = artifacts.into_iter().map(|artifact| (artifact.id, artifact)).collect();
                    }
                    WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                        contracts.insert(work_contract.id, work_contract);
                    }
                    WorkspaceEventPayload::PlanUpdated { plan: updated }
                    | WorkspaceEventPayload::PlanTaskStatusChanged { plan: updated, .. } => {
                        plan = Some(updated);
                    }
                    _ => {}
                }
            }
            match &command.payload {
                WorkspaceCommandPayload::ReplaceShapingRecords { records } => {
                    let current = current_records.context("cannot replace shaping records before a proposal exists")?;
                    if records.proposal_revision != current.proposal_revision {
                        anyhow::bail!("manual shaping edits must preserve the proposal revision");
                    }
                    let retained = records.acceptance_criteria.iter().map(|criterion| criterion.id).collect::<HashSet<_>>();
                    if let Some(reference) = plan.as_ref().and_then(|plan| plan.tasks.iter().flat_map(|task| task.acceptance_criteria.iter()).find(|id| !retained.contains(id))) {
                        anyhow::bail!("acceptance criterion {reference:?} is still referenced by the dependency plan");
                    }
                    if let Some(reference) = contracts.values().flat_map(|contract| contract.success_criteria.iter()).find(|id| !retained.contains(id)) {
                        anyhow::bail!("acceptance criterion {reference:?} is still referenced by a work contract");
                    }
                }
                WorkspaceCommandPayload::ReplaceArtifacts { artifacts } => {
                    if artifacts.iter().any(|artifact| artifact.work_item_id != command.work_item_id) {
                        anyhow::bail!("replacement artifact belongs to another work item");
                    }
                    let retained = artifacts.iter().map(|artifact| artifact.id).collect::<HashSet<_>>();
                    if let Some(reference) = contracts.values().flat_map(|contract| contract.input_artifact_ids.iter()).find(|id| !retained.contains(id)) {
                        anyhow::bail!("repository artifact {reference:?} is still referenced by a work contract");
                    }
                }
                _ => unreachable!(),
            }
        }
        if let WorkspaceCommandPayload::RevisePlan {
            plan,
            work_contracts,
        } = &command.payload
        {
            let work = load_workspace_work_item(
                conn,
                command.project_id,
                command.work_item_id,
            )?
            .context("cannot revise a plan for a work item that does not exist")?;
            if work.status != WorkStatus::Planning {
                anyhow::bail!("plans can only be revised while work is in Planning");
            }
            let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
            if first_sequence.saturating_sub(1) > replay_limit as u64 {
                anyhow::bail!(
                    "plan revision validation requires a canonical snapshot after {replay_limit} events"
                );
            }
            let mut current_plan = None;
            let mut current_team = None;
            let mut current_contracts = HashMap::<WorkContractId, WorkContract>::new();
            let mut current_criteria = HashSet::<AcceptanceCriterionId>::new();
            for event in load_canonical_workspace_events(
                conn,
                command.project_id,
                command.work_item_id,
                0,
                replay_limit,
            )? {
                match event.payload {
                    WorkspaceEventPayload::PlanUpdated { plan } => current_plan = Some(plan),
                    WorkspaceEventPayload::PlanTaskStatusChanged { plan, .. } => {
                        current_plan = Some(plan)
                    }
                    WorkspaceEventPayload::ProposedTeamUpdated { proposed_team } => {
                        current_team = Some(proposed_team)
                    }
                    WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                        current_contracts.insert(work_contract.id, work_contract);
                    }
                    WorkspaceEventPayload::AcceptanceCriterionUpdated {
                        acceptance_criterion,
                    } => {
                        current_criteria.insert(acceptance_criterion.id);
                    }
                    _ => {}
                }
            }
            let previous = current_plan.context("cannot revise a missing canonical plan")?;
            if plan.id != previous.id || plan.version != previous.version.saturating_add(1) {
                anyhow::bail!("plan revision must preserve identity and increment its version once");
            }
            if plan.created_at_unix_ms != previous.created_at_unix_ms
                || plan.updated_at_unix_ms != command.timestamp_unix_ms
            {
                anyhow::bail!("plan revision must preserve creation time and use the command timestamp");
            }
            if previous
                .tasks
                .iter()
                .any(|task| task.status != PlanTaskStatus::Proposed)
            {
                anyhow::bail!("a plan cannot be reshaped after task execution has started");
            }
            let covered_criteria = plan
                .tasks
                .iter()
                .flat_map(|task| task.acceptance_criteria.iter().copied())
                .collect::<HashSet<_>>();
            if !current_criteria.is_subset(&covered_criteria) {
                anyhow::bail!("every canonical acceptance criterion must remain covered by the revised plan");
            }
            let team = current_team.context("cannot revise a plan without a proposed team")?;
            let expected_contract_ids = team
                .roles
                .iter()
                .map(|role| role.work_contract_id)
                .collect::<HashSet<_>>();
            let revised_contract_ids = work_contracts
                .iter()
                .map(|contract| contract.id)
                .collect::<HashSet<_>>();
            if expected_contract_ids != revised_contract_ids {
                anyhow::bail!("plan revision must include every current proposed-team contract");
            }
            for revised in work_contracts {
                let current = current_contracts
                    .get(&revised.id)
                    .context("plan revision references a missing canonical work contract")?;
                let mut expected = current.clone();
                expected.version = current.version.saturating_add(1);
                expected.status = WorkContractStatus::Proposed;
                expected.agent_instance_id = None;
                expected.task_ids = revised.task_ids.clone();
                expected.success_criteria = revised.success_criteria.clone();
                expected.updated_by = command.actor_id;
                expected.updated_at_unix_ms = command.timestamp_unix_ms;
                if *revised != expected {
                    anyhow::bail!(
                        "plan revision may only update contract task/criterion links and approval metadata"
                    );
                }
            }
        }
        let mut work_item_update = None;
        let (payloads, compact_kinds) = match &command.payload {
            WorkspaceCommandPayload::CreateWorkItem {
                title,
                execution_location,
            } => {
                if first_sequence != 1 {
                    anyhow::bail!("work item already has canonical history");
                }
                let work = WorkItem {
                    id: command.work_item_id,
                    project_id: command.project_id,
                    title: title.trim().to_string(),
                    status: WorkStatus::Draft,
                    created_at_unix_ms: command.timestamp_unix_ms,
                    updated_at_unix_ms: command.timestamp_unix_ms,
                };
                work_item_update = Some(work);
                (
                    vec![WorkspaceEventPayload::WorkCreated {
                        title: title.trim().to_string(),
                        execution_location: *execution_location,
                    }],
                    vec![WorkspaceEventKind::WorkCreated {
                        title: title.trim().to_string(),
                    }],
                )
            }
            WorkspaceCommandPayload::TransitionWork { status } => {
                if *status == WorkStatus::ReadyToExecute {
                    ensure_coordination_is_ready(
                        conn,
                        command.project_id,
                        command.work_item_id,
                        first_sequence.saturating_sub(1),
                    )?;
                }
                if *status == WorkStatus::Completed {
                    ensure_work_completion_ready(
                        conn,
                        command.project_id,
                        command.work_item_id,
                        first_sequence.saturating_sub(1),
                    )?;
                }
                let mut work =
                    load_workspace_work_item(conn, command.project_id, command.work_item_id)?
                        .context("cannot transition a work item that does not exist")?;
                let previous = work.status;
                work.transition_to(*status, command.timestamp_unix_ms)
                    .map_err(|error| anyhow::anyhow!(error))?;
                work_item_update = Some(work);
                (
                    vec![WorkspaceEventPayload::WorkStatusChanged {
                        previous,
                        current: *status,
                    }],
                    vec![WorkspaceEventKind::WorkStatusChanged { status: *status }],
                )
            }
            WorkspaceCommandPayload::TransitionPlanTask { task_id, status } => {
                if *status == PlanTaskStatus::Running {
                    anyhow::bail!("running tasks must be started through start_agent_execution");
                }
                let (plan, previous) = transition_plan_task(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    *task_id,
                    *status,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                if previous == PlanTaskStatus::Running {
                    anyhow::bail!(
                        "running tasks must be terminalized through finish_agent_execution"
                    );
                }
                (
                    vec![WorkspaceEventPayload::PlanTaskStatusChanged {
                        plan: plan.clone(),
                        task_id: *task_id,
                        previous,
                        current: *status,
                    }],
                    vec![WorkspaceEventKind::StructuredPlanUpdated { plan }],
                )
            }
            WorkspaceCommandPayload::ProvisionAgentInstance {
                agent_instance_id,
                contract_id,
                model_route,
            } => {
                let (agent_instance, work_contract) = provision_agent_instance(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *agent_instance_id,
                    *contract_id,
                    model_route,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AgentInstanceProvisioned {
                        agent_instance: agent_instance.clone(),
                        work_contract: work_contract.clone(),
                    }],
                    vec![WorkspaceEventKind::AgentInstanceProvisioned {
                        agent_instance,
                        work_contract,
                    }],
                )
            }
            WorkspaceCommandPayload::ActivateAgentInstance {
                agent_instance_id,
                lease_id,
                effective_model_route,
                charging,
                lease_expires_at_unix_ms,
            } => {
                let (agent_instance, work_contract, lease) = activate_agent_instance(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *agent_instance_id,
                    *lease_id,
                    effective_model_route,
                    charging,
                    command.timestamp_unix_ms,
                    *lease_expires_at_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AgentInstanceActivated {
                        agent_instance: agent_instance.clone(),
                        work_contract: work_contract.clone(),
                        lease: lease.clone(),
                    }],
                    vec![WorkspaceEventKind::AgentInstanceActivated {
                        agent_instance,
                        work_contract,
                        lease,
                    }],
                )
            }
            WorkspaceCommandPayload::RenewExecutionLease {
                lease_id,
                lease_expires_at_unix_ms,
            } => {
                let (agent_instance, lease) = renew_execution_lease(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *lease_id,
                    command.timestamp_unix_ms,
                    *lease_expires_at_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::ExecutionLeaseRenewed {
                        agent_instance: agent_instance.clone(),
                        lease: lease.clone(),
                    }],
                    vec![WorkspaceEventKind::ExecutionLeaseRenewed {
                        agent_instance,
                        lease,
                    }],
                )
            }
            WorkspaceCommandPayload::StartAgentExecution {
                attempt_id,
                task_id,
                agent_instance_id,
                lease_id,
                isolation,
            } => {
                let (agent_instance, lease, attempt, plan) = start_agent_execution(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    *attempt_id,
                    *task_id,
                    *agent_instance_id,
                    *lease_id,
                    isolation,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                let mut payloads = Vec::with_capacity(2);
                let mut compact_kinds = Vec::with_capacity(2);
                let mut work =
                    load_workspace_work_item(conn, command.project_id, command.work_item_id)?
                        .context("cannot start execution for a missing work item")?;
                if work.status == WorkStatus::ReadyToExecute {
                    let previous = work.status;
                    work.transition_to(WorkStatus::Executing, command.timestamp_unix_ms)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    work_item_update = Some(work);
                    payloads.push(WorkspaceEventPayload::WorkStatusChanged {
                        previous,
                        current: WorkStatus::Executing,
                    });
                    compact_kinds.push(WorkspaceEventKind::WorkStatusChanged {
                        status: WorkStatus::Executing,
                    });
                }
                payloads.push(WorkspaceEventPayload::AgentExecutionStarted {
                    agent_instance: agent_instance.clone(),
                    lease: lease.clone(),
                    attempt: attempt.clone(),
                    plan: plan.clone(),
                });
                compact_kinds.push(WorkspaceEventKind::AgentExecutionStarted {
                    agent_instance,
                    lease,
                    attempt,
                    plan,
                });
                (payloads, compact_kinds)
            }
            WorkspaceCommandPayload::FinishAgentExecution {
                attempt_id,
                outcome,
                summary,
                change_summary,
            } => {
                let (agent_instance, lease, attempt, plan) = finish_agent_execution(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *attempt_id,
                    *outcome,
                    summary,
                    change_summary.as_ref(),
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AgentExecutionFinished {
                        agent_instance: agent_instance.clone(),
                        lease: lease.clone(),
                        attempt: attempt.clone(),
                        plan: plan.clone(),
                        summary: summary.trim().to_string(),
                    }],
                    vec![WorkspaceEventKind::AgentExecutionFinished {
                        agent_instance,
                        lease,
                        attempt,
                        plan,
                        summary: summary.trim().to_string(),
                    }],
                )
            }
            WorkspaceCommandPayload::ResolveAgentExecutionReview {
                attempt_id,
                resolution,
            } => {
                let recovery_state = if *resolution == ExecutionReviewResolution::RevisionRequested {
                    Some(load_scheduler_authority_state(conn, command.project_id, command.work_item_id, first_sequence.saturating_sub(1))?)
                } else { None };
                let (agent_instance, work_contract, lease, attempt, mut plan) =
                    resolve_agent_execution_review(
                        conn,
                        command.project_id,
                        command.work_item_id,
                        command.actor_id,
                        *attempt_id,
                        *resolution,
                        command.timestamp_unix_ms,
                        first_sequence.saturating_sub(1),
                    )?;
                let agent_instance_id = agent_instance.id;
                let mut payloads = vec![WorkspaceEventPayload::AgentExecutionReviewResolved {
                        agent_instance: agent_instance.clone(),
                        work_contract: work_contract.clone(),
                        lease: lease.clone(),
                        attempt: attempt.clone(),
                        plan: plan.clone(),
                        resolution: *resolution,
                    }];
                let mut compact_kinds = vec![WorkspaceEventKind::AgentExecutionReviewResolved {
                        agent_instance,
                        work_contract: work_contract.clone(),
                        lease,
                        attempt: attempt.clone(),
                        plan: plan.clone(),
                        resolution: *resolution,
                    }];

                if let Some((previous_plan, _, criteria, _, _)) = recovery_state {
                    if previous_plan.tasks.iter().any(|t| t.id == attempt.task_id && t.status == PlanTaskStatus::Completed) {
                        let stale_id = EvidenceId(Uuid::new_v5(&attempt.id.0, b"evidence:accepted-execution"));
                        for mut criterion in criteria.into_iter().filter(|c| c.evidence_ids.contains(&stale_id)
                            && c.status != AcceptanceStatus::Waived) {
                            criterion.evidence_ids.retain(|id| *id != stale_id);
                            criterion.status = AcceptanceStatus::Pending;
                            criterion.updated_at_unix_ms = command.timestamp_unix_ms;
                            payloads.push(WorkspaceEventPayload::AcceptanceCriterionUpdated { acceptance_criterion: criterion.clone() });
                            compact_kinds.push(WorkspaceEventKind::AcceptanceCriterionUpdated { acceptance_criterion: criterion });
                        }
                    }
                }

                if matches!(
                    resolution,
                    ExecutionReviewResolution::Accepted
                        | ExecutionReviewResolution::Integrated
                ) {
                    let (evidence, updated_criteria, completed_plan, work_complete) =
                        finalize_accepted_task(
                            conn,
                            command.project_id,
                            command.work_item_id,
                            command.actor_id,
                            &attempt,
                            &work_contract,
                            &plan,
                            agent_instance_id,
                            *resolution,
                            command.timestamp_unix_ms,
                            first_sequence.saturating_sub(1),
                        )?;
                    plan = completed_plan;
                    for evidence in evidence {
                        payloads.push(WorkspaceEventPayload::EvidenceRecorded { evidence: evidence.clone() });
                        compact_kinds.push(WorkspaceEventKind::EvidenceRecorded { evidence });
                    }
                    for criterion in updated_criteria {
                        payloads.push(WorkspaceEventPayload::AcceptanceCriterionUpdated {
                            acceptance_criterion: criterion.clone(),
                        });
                        compact_kinds.push(WorkspaceEventKind::AcceptanceCriterionUpdated {
                            acceptance_criterion: criterion,
                        });
                    }
                    payloads.push(WorkspaceEventPayload::PlanTaskStatusChanged {
                        task_id: attempt.task_id,
                        previous: PlanTaskStatus::Verifying,
                        current: PlanTaskStatus::Completed,
                        plan: plan.clone(),
                    });
                    compact_kinds.push(WorkspaceEventKind::StructuredPlanUpdated {
                        plan: plan.clone(),
                    });

                    if work_complete {
                        let mut work = load_workspace_work_item(
                            conn,
                            command.project_id,
                            command.work_item_id,
                        )?
                        .context("accepted verification belongs to a missing work item")?;
                        for target in [
                            WorkStatus::Reviewing,
                            WorkStatus::Verifying,
                            WorkStatus::Completed,
                        ] {
                            if work.status == target {
                                continue;
                            }
                            if work.status.can_transition_to(target) {
                                let previous = work.status;
                                work.transition_to(target, command.timestamp_unix_ms)
                                    .map_err(|error| anyhow::anyhow!(error))?;
                                payloads.push(WorkspaceEventPayload::WorkStatusChanged {
                                    previous,
                                    current: target,
                                });
                                compact_kinds
                                    .push(WorkspaceEventKind::WorkStatusChanged { status: target });
                            }
                        }
                        if work.status != WorkStatus::Completed {
                            anyhow::bail!(
                                "verified work could not reach Completed from its current lifecycle"
                            );
                        }
                        work_item_update = Some(work);
                    }
                }
                (payloads, compact_kinds)
            }
            WorkspaceCommandPayload::CancelAgentExecution { attempt_id, reason } => {
                let (agent_instance, lease, attempt, plan) = finish_agent_execution(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *attempt_id,
                    ExecutionAttemptStatus::Cancelled,
                    reason,
                    None,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AgentExecutionFinished {
                        agent_instance: agent_instance.clone(),
                        lease: lease.clone(),
                        attempt: attempt.clone(),
                        plan: plan.clone(),
                        summary: reason.trim().to_string(),
                    }],
                    vec![WorkspaceEventKind::AgentExecutionFinished {
                        agent_instance,
                        lease,
                        attempt,
                        plan,
                        summary: reason.trim().to_string(),
                    }],
                )
            }
            WorkspaceCommandPayload::PauseAgentExecution { attempt_id, reason } => {
                let (agent_instance, lease, attempt, plan) = finish_agent_execution(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *attempt_id,
                    ExecutionAttemptStatus::Paused,
                    reason,
                    None,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                let mut work =
                    load_workspace_work_item(conn, command.project_id, command.work_item_id)?
                        .context("cannot pause execution for a missing work item")?;
                let previous = work.status;
                work.transition_to(WorkStatus::Paused, command.timestamp_unix_ms)
                    .map_err(|error| anyhow::anyhow!(error))?;
                work_item_update = Some(work);
                (
                    vec![
                        WorkspaceEventPayload::WorkStatusChanged {
                            previous,
                            current: WorkStatus::Paused,
                        },
                        WorkspaceEventPayload::AgentExecutionFinished {
                            agent_instance: agent_instance.clone(),
                            lease: lease.clone(),
                            attempt: attempt.clone(),
                            plan: plan.clone(),
                            summary: reason.trim().to_string(),
                        },
                    ],
                    vec![
                        WorkspaceEventKind::WorkStatusChanged {
                            status: WorkStatus::Paused,
                        },
                        WorkspaceEventKind::AgentExecutionFinished {
                            agent_instance,
                            lease,
                            attempt,
                            plan,
                            summary: reason.trim().to_string(),
                        },
                    ],
                )
            }
            WorkspaceCommandPayload::PrepareAgentExecutionRetry {
                failed_attempt_id,
                lease_id,
                lease_expires_at_unix_ms,
            } => {
                let (agent_instance, lease, plan) = prepare_agent_execution_retry(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *failed_attempt_id,
                    *lease_id,
                    command.timestamp_unix_ms,
                    *lease_expires_at_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AgentExecutionRetryPrepared {
                        agent_instance: agent_instance.clone(),
                        lease: lease.clone(),
                        plan: plan.clone(),
                    }],
                    vec![WorkspaceEventKind::AgentExecutionRetryPrepared {
                        agent_instance,
                        lease,
                        plan,
                    }],
                )
            }
            WorkspaceCommandPayload::RaiseAttention { attention_item } => {
                let attention_item = raise_attention(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    attention_item,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AttentionUpdated {
                        attention_item: attention_item.clone(),
                    }],
                    vec![WorkspaceEventKind::PermissionRequired {
                        summary: attention_item.summary,
                    }],
                )
            }
            WorkspaceCommandPayload::ResolveAttention {
                attention_item_id,
                outcome,
                resolution,
            } => {
                let attention_item = resolve_attention(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *attention_item_id,
                    *outcome,
                    resolution,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::AttentionUpdated { attention_item }],
                    Vec::new(),
                )
            }
            WorkspaceCommandPayload::PostMessage { message, routing } => {
                if let Some(routing) = routing {
                    validate_agent_message_authority(
                        conn,
                        command.project_id,
                        command.work_item_id,
                        message,
                        routing,
                        command.timestamp_unix_ms,
                        first_sequence.saturating_sub(1),
                    )?;
                }
                (
                    vec![WorkspaceEventPayload::MessagePosted {
                        message: message.clone(),
                        routing: routing.clone(),
                    }],
                    vec![if let Some(routing) = routing {
                        WorkspaceEventKind::AgentMessagePosted {
                            message: message.clone(),
                            routing: routing.clone(),
                        }
                    } else {
                        WorkspaceEventKind::UserMessagePosted {
                            body: message.body.clone(),
                        }
                    }],
                )
            }
            WorkspaceCommandPayload::UpdateMessageDelivery {
                message_id,
                delivery_state,
            } => {
                let (message, routing) = update_agent_message_delivery(
                    conn,
                    command.project_id,
                    command.work_item_id,
                    command.actor_id,
                    *message_id,
                    *delivery_state,
                    command.timestamp_unix_ms,
                    first_sequence.saturating_sub(1),
                )?;
                (
                    vec![WorkspaceEventPayload::MessageDeliveryUpdated {
                        message: message.clone(),
                        routing: routing.clone(),
                    }],
                    vec![WorkspaceEventKind::AgentMessageDeliveryUpdated {
                        message,
                        routing,
                    }],
                )
            }
            WorkspaceCommandPayload::RecordEvidence { evidence } => (
                vec![WorkspaceEventPayload::EvidenceRecorded {
                    evidence: evidence.clone(),
                }],
                vec![WorkspaceEventKind::EvidenceRecorded {
                    evidence: evidence.clone(),
                }],
            ),
            WorkspaceCommandPayload::RecordShapingMemory { notes } => (
                notes
                    .iter()
                    .cloned()
                    .map(|note| WorkspaceEventPayload::ShapingMemoryRecorded { note })
                    .collect(),
                notes
                    .iter()
                    .cloned()
                    .map(|note| WorkspaceEventKind::ShapingMemoryRecorded { note })
                    .collect(),
            ),
            WorkspaceCommandPayload::CompleteShapingTurn {
                message,
                proposal,
                records,
                artifacts,
                role_definitions,
                work_contracts,
                proposed_team,
                plan,
            } => {
                let mut payloads = vec![
                    WorkspaceEventPayload::MessagePosted {
                        message: message.clone(),
                        routing: None,
                    },
                    WorkspaceEventPayload::ShapingProposalUpdated {
                        proposal: proposal.clone(),
                    },
                    WorkspaceEventPayload::ShapingRecordsUpdated {
                        records: records.clone(),
                    },
                ];
                payloads.extend(
                    artifacts
                        .iter()
                        .cloned()
                        .map(|artifact| WorkspaceEventPayload::ArtifactUpdated { artifact }),
                );
                payloads.extend(role_definitions.iter().cloned().map(|role_definition| {
                    WorkspaceEventPayload::RoleDefinitionUpdated { role_definition }
                }));
                payloads.extend(work_contracts.iter().cloned().map(|work_contract| {
                    WorkspaceEventPayload::WorkContractUpdated { work_contract }
                }));
                if let Some(plan) = plan {
                    payloads.push(WorkspaceEventPayload::PlanUpdated { plan: plan.clone() });
                }
                if let Some(proposed_team) = proposed_team {
                    payloads.push(WorkspaceEventPayload::ProposedTeamUpdated {
                        proposed_team: proposed_team.clone(),
                    });
                }
                let mut kinds = vec![
                    WorkspaceEventKind::PrincipalMessagePosted {
                        body: message.body.clone(),
                    },
                    WorkspaceEventKind::ShapingProposalUpdated {
                        proposal: proposal.clone(),
                    },
                    WorkspaceEventKind::ShapingRecordsUpdated {
                        records: records.clone(),
                    },
                ];
                kinds.extend(artifacts.iter().cloned().map(|artifact| {
                    WorkspaceEventKind::ArtifactUpdated { artifact }
                }));
                kinds.extend(role_definitions.iter().cloned().map(|role_definition| {
                    WorkspaceEventKind::RoleDefinitionUpdated { role_definition }
                }));
                kinds.extend(work_contracts.iter().cloned().map(|work_contract| {
                    WorkspaceEventKind::WorkContractUpdated { work_contract }
                }));
                if let Some(plan) = plan {
                    kinds.push(WorkspaceEventKind::StructuredPlanUpdated { plan: plan.clone() });
                }
                if let Some(proposed_team) = proposed_team {
                    kinds.push(WorkspaceEventKind::ProposedTeamUpdated {
                        proposed_team: proposed_team.clone(),
                    });
                }
                (payloads, kinds)
            }
            WorkspaceCommandPayload::UpsertRequirement { requirement } => (
                vec![WorkspaceEventPayload::RequirementUpdated {
                    requirement: requirement.clone(),
                }],
                vec![WorkspaceEventKind::RequirementUpdated {
                    requirement: requirement.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertWorkBrief { work_brief } => (
                vec![WorkspaceEventPayload::WorkBriefUpdated {
                    work_brief: work_brief.clone(),
                }],
                vec![WorkspaceEventKind::WorkBriefUpdated {
                    work_brief: work_brief.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertDecision { decision } => (
                vec![WorkspaceEventPayload::DecisionUpdated {
                    decision: decision.clone(),
                }],
                vec![WorkspaceEventKind::DecisionUpdated {
                    decision: decision.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertAcceptanceCriterion {
                acceptance_criterion,
            } => (
                vec![WorkspaceEventPayload::AcceptanceCriterionUpdated {
                    acceptance_criterion: acceptance_criterion.clone(),
                }],
                vec![WorkspaceEventKind::AcceptanceCriterionUpdated {
                    acceptance_criterion: acceptance_criterion.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertArtifact { artifact } => (
                vec![WorkspaceEventPayload::ArtifactUpdated {
                    artifact: artifact.clone(),
                }],
                vec![WorkspaceEventKind::ArtifactUpdated {
                    artifact: artifact.clone(),
                }],
            ),
            WorkspaceCommandPayload::ReplaceShapingRecords { records } => (
                vec![WorkspaceEventPayload::ShapingRecordsUpdated {
                    records: records.clone(),
                }],
                vec![WorkspaceEventKind::ShapingRecordsUpdated {
                    records: records.clone(),
                }],
            ),
            WorkspaceCommandPayload::ReplaceArtifacts { artifacts } => (
                vec![WorkspaceEventPayload::ArtifactsReplaced {
                    artifacts: artifacts.clone(),
                }],
                vec![WorkspaceEventKind::ArtifactsReplaced {
                    artifacts: artifacts.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertRoleDefinition { role_definition } => (
                vec![WorkspaceEventPayload::RoleDefinitionUpdated {
                    role_definition: role_definition.clone(),
                }],
                vec![WorkspaceEventKind::RoleDefinitionUpdated {
                    role_definition: role_definition.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertWorkContract { work_contract } => (
                vec![WorkspaceEventPayload::WorkContractUpdated {
                    work_contract: work_contract.clone(),
                }],
                vec![WorkspaceEventKind::WorkContractUpdated {
                    work_contract: work_contract.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertProposedTeam { proposed_team } => (
                vec![WorkspaceEventPayload::ProposedTeamUpdated {
                    proposed_team: proposed_team.clone(),
                }],
                vec![WorkspaceEventKind::ProposedTeamUpdated {
                    proposed_team: proposed_team.clone(),
                }],
            ),
            WorkspaceCommandPayload::UpsertPlan { plan } => (
                vec![WorkspaceEventPayload::PlanUpdated { plan: plan.clone() }],
                vec![WorkspaceEventKind::StructuredPlanUpdated { plan: plan.clone() }],
            ),
            WorkspaceCommandPayload::RevisePlan {
                plan,
                work_contracts,
            } => {
                let mut payloads = work_contracts
                    .iter()
                    .cloned()
                    .map(|work_contract| WorkspaceEventPayload::WorkContractUpdated {
                        work_contract,
                    })
                    .collect::<Vec<_>>();
                payloads.push(WorkspaceEventPayload::PlanUpdated { plan: plan.clone() });
                let mut compact_kinds = work_contracts
                    .iter()
                    .cloned()
                    .map(|work_contract| WorkspaceEventKind::WorkContractUpdated {
                        work_contract,
                    })
                    .collect::<Vec<_>>();
                compact_kinds.push(WorkspaceEventKind::StructuredPlanUpdated {
                    plan: plan.clone(),
                });
                (payloads, compact_kinds)
            }
            _ => anyhow::bail!("local workspace authority does not accept this command yet"),
        };
        if let Some(work) = &work_item_update {
            save_workspace_work_item(conn, work)?;
        }
        let events = payloads
            .into_iter()
            .enumerate()
            .map(|(offset, payload)| {
                let offset = u64::try_from(offset).context("canonical event batch overflow")?;
                let sequence = first_sequence
                    .checked_add(offset)
                    .context("canonical workspace sequence exhausted")?;
                let task_id = match &payload {
                    WorkspaceEventPayload::PlanTaskStatusChanged { task_id, .. } => Some(*task_id),
                    WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } => {
                        Some(attempt.task_id)
                    }
                    WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } => {
                        Some(attempt.task_id)
                    }
                    WorkspaceEventPayload::AgentExecutionReviewResolved { attempt, .. } => {
                        Some(attempt.task_id)
                    }
                    WorkspaceEventPayload::MessagePosted {
                        routing: Some(routing),
                        ..
                    } => routing.task_id,
                    WorkspaceEventPayload::MessageDeliveryUpdated { routing, .. } => {
                        routing.task_id
                    }
                    WorkspaceEventPayload::EvidenceRecorded { evidence } => evidence.task_id,
                    WorkspaceEventPayload::AttentionUpdated { attention_item } => {
                        attention_item.task_id
                    }
                    WorkspaceEventPayload::CheckpointRecorded { checkpoint } => checkpoint.task_id,
                    _ => None,
                };
                let event = WorkspaceEventEnvelope {
                    event_id: WorkspaceEventId::new(),
                    schema_version: WORKSPACE_PROTOCOL_VERSION,
                    sequence,
                    project_id: command.project_id,
                    work_item_id: command.work_item_id,
                    actor_id: command.actor_id,
                    device_id: command.device_id,
                    task_id,
                    correlation_id: command.correlation_id,
                    causation_id: Some(command.command_id),
                    timestamp_unix_ms: command.timestamp_unix_ms,
                    payload,
                };
                event.validate().map_err(|error| {
                    anyhow::anyhow!("invalid canonical workspace event: {error}")
                })?;
                Ok(event)
            })
            .collect::<Result<Vec<_>>>()?;
        insert_canonical_outcome(conn, command, &events)?;
        insert_compact_projection_events(conn, command, &compact_kinds)?;
        if let Some(attempt_id) = integration_attempt {
            mark_workspace_integration_completed(
                conn,
                attempt_id,
                command.command_id,
                command.timestamp_unix_ms,
            )?;
        }
        Ok(events)
    })
}
