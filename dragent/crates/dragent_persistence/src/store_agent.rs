//! Durable, metadata-only storage for the engineering workspace projection.

use crate::load_canonical_workspace_events;
use crate::store_execution::transition_plan_task;
use crate::store_projects::load_workspace_work_item;
use crate::workspace_store::DEFAULT_MAX_PARALLEL_TASKS;
use crate::workspace_store::MAX_CANONICAL_REPLAY_EVENTS;
use crate::workspace_store::MAX_EXECUTION_LEASE_MS;
use anyhow::{Context, Result};
use dragent_session_protocol::workspace::WorkspaceEventPayload;
use dragent_workspace_model::{
    build_completion_report, evaluate_scheduler, execution_retry_delay_ms, AcceptanceCriterion,
    AcceptanceCriterionId, AcceptanceStatus, ActorRef, AgentInstance, AgentInstanceId,
    AgentInstanceStatus, AgentMessage, AgentMessageId, AgentMessageKind, AgentMessageRouting,
    AttentionItem, AttentionItemId, ChargingAuthorization, Evidence, EvidenceId, ExecutionAttempt,
    ExecutionAttemptId, ExecutionAttemptStatus, ExecutionIsolation, ExecutionIsolationKind,
    ExecutionLease, ExecutionLeaseId, ExecutionLeaseStatus, MessageDeliveryState, Plan, PlanTask,
    PlanTaskId, PlanTaskStatus, ProjectId, WorkContract, WorkContractId, WorkContractStatus,
    WorkItemId, WorkStatus, MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
};
use std::collections::{HashMap, HashSet};

#[allow(clippy::too_many_arguments)]
pub(crate) fn activate_agent_instance(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    agent_instance_id: AgentInstanceId,
    lease_id: ExecutionLeaseId,
    effective_model_route: &str,
    charging: &ChargingAuthorization,
    timestamp_unix_ms: i64,
    lease_expires_at_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentInstance, WorkContract, ExecutionLease)> {
    let work = load_workspace_work_item(conn, project_id, work_item_id)?
        .context("cannot activate an agent for a work item that does not exist")?;
    if !matches!(
        work.status,
        WorkStatus::ReadyToExecute | WorkStatus::Executing
    ) {
        anyhow::bail!(
            "agent activation requires ready-to-execute or executing work; current status is {:?}",
            work.status
        );
    }
    if charging.owner_id != actor_id {
        anyhow::bail!("charging owner must match the activating actor");
    }
    charging
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid charging authorization: {error}"))?;
    // Platform-credit leases are pay-as-you-go: the provider enforces
    // balance per call (exhaustion fails the attempt and raises attention),
    // while the lease window and contract budgets bound exposure. Owner
    // attribution above is what ties spend to an account.
    let effective_model_route = effective_model_route.trim();
    if effective_model_route.is_empty() || effective_model_route.eq_ignore_ascii_case("auto") {
        anyhow::bail!("agent activation requires a resolved effective model route");
    }
    let latest_expiry = timestamp_unix_ms
        .checked_add(MAX_EXECUTION_LEASE_MS)
        .context("execution lease timestamp overflow")?;
    if lease_expires_at_unix_ms <= timestamp_unix_ms || lease_expires_at_unix_ms > latest_expiry {
        anyhow::bail!("execution lease must be positive and no longer than 15 minutes");
    }

    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!("agent activation requires a canonical snapshot after {replay_limit} events");
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut agents = HashMap::<AgentInstanceId, AgentInstance>::new();
    let mut lease_ids = HashSet::<ExecutionLeaseId>::new();
    let mut leases = HashMap::<ExecutionLeaseId, ExecutionLease>::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::AgentInstanceProvisioned {
                agent_instance,
                work_contract,
            } => {
                agents.insert(agent_instance.id, agent_instance);
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::AgentInstanceActivated {
                agent_instance,
                work_contract,
                lease,
            } => {
                lease_ids.insert(lease.id);
                agents.insert(agent_instance.id, agent_instance);
                contracts.insert(work_contract.id, work_contract);
                leases.insert(lease.id, lease);
            }
            WorkspaceEventPayload::ExecutionLeaseRenewed {
                agent_instance,
                lease,
            }
            | WorkspaceEventPayload::AgentExecutionRetryPrepared {
                agent_instance,
                lease,
                ..
            }
            | WorkspaceEventPayload::AgentExecutionStarted {
                agent_instance,
                lease,
                ..
            }
            | WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance,
                lease,
                ..
            } => {
                lease_ids.insert(lease.id);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            _ => {}
        }
    }
    if lease_ids.contains(&lease_id) {
        anyhow::bail!("execution lease identity is already in use for this work item");
    }
    let mut agent_instance = agents
        .remove(&agent_instance_id)
        .context("agent activation requires a provisioned agent instance")?;
    if agent_instance.status != AgentInstanceStatus::Pending
        || agent_instance.active_lease_id.is_some()
        || agent_instance.started_at_unix_ms.is_some()
    {
        anyhow::bail!("only a pending, unleased agent instance can be activated");
    }
    let mut work_contract = contracts
        .remove(&agent_instance.contract_id)
        .context("agent activation requires the bound canonical work contract")?;
    if work_contract.status != WorkContractStatus::Approved
        || work_contract.agent_instance_id != Some(agent_instance.id)
        || work_contract.role_definition_id != agent_instance.role_definition_id
    {
        anyhow::bail!("agent activation requires its matching approved contract binding");
    }
    let lease = ExecutionLease {
        id: lease_id,
        work_item_id,
        agent_instance_id: agent_instance.id,
        contract_id: work_contract.id,
        status: ExecutionLeaseStatus::Active,
        charging: charging.clone(),
        acquired_at_unix_ms: timestamp_unix_ms,
        expires_at_unix_ms: lease_expires_at_unix_ms,
        updated_at_unix_ms: timestamp_unix_ms,
    };
    lease
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid execution lease: {error}"))?;
    agent_instance.status = AgentInstanceStatus::Ready;
    agent_instance.model_route = effective_model_route.to_string();
    agent_instance.active_lease_id = Some(lease.id);
    agent_instance.updated_at_unix_ms = timestamp_unix_ms;
    agent_instance
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid activated agent: {error}"))?;
    work_contract.status = WorkContractStatus::Active;
    work_contract.version = work_contract
        .version
        .checked_add(1)
        .context("work contract version exhausted")?;
    work_contract.updated_by = actor_id;
    work_contract.updated_at_unix_ms = timestamp_unix_ms;
    work_contract
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid activated contract: {error}"))?;
    Ok((agent_instance, work_contract, lease))
}

/// An execution lease protects the immutable write scope of its Active work
/// contract. Empty scopes are read-only; otherwise hierarchical paths overlap
/// when either is an ancestor of the other. This is deliberately conservative:
/// uncertain paths are serialized rather than allowing two local agents to
/// write the same project tree.
fn write_scopes_overlap(left: &[std::path::PathBuf], right: &[std::path::PathBuf]) -> bool {
    left.iter().any(|left_scope| {
        right.iter().any(|right_scope| {
            left_scope.starts_with(right_scope) || right_scope.starts_with(left_scope)
        })
    })
}

type SchedulerAuthorityState = (
    Plan,
    Vec<WorkContract>,
    Vec<AcceptanceCriterion>,
    Vec<Evidence>,
    Vec<AttentionItem>,
);

pub(crate) fn load_scheduler_authority_state(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    current_revision: u64,
) -> Result<SchedulerAuthorityState> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "evidence validation requires a canonical snapshot after {replay_limit} events"
        );
    }
    let mut plan = None;
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut criteria = HashMap::<AcceptanceCriterionId, AcceptanceCriterion>::new();
    let mut evidence = HashMap::<EvidenceId, Evidence>::new();
    let mut attention = HashMap::<AttentionItemId, AttentionItem>::new();
    for event in load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)? {
        match event.payload {
            WorkspaceEventPayload::PlanUpdated { plan: updated }
            | WorkspaceEventPayload::PlanTaskStatusChanged { plan: updated, .. }
            | WorkspaceEventPayload::AgentExecutionStarted { plan: updated, .. }
            | WorkspaceEventPayload::AgentExecutionFinished { plan: updated, .. }
            | WorkspaceEventPayload::AgentExecutionRetryPrepared { plan: updated, .. } => {
                plan = Some(updated)
            }
            WorkspaceEventPayload::AgentExecutionReviewResolved {
                plan: updated,
                work_contract,
                ..
            } => {
                plan = Some(updated);
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::WorkContractUpdated { work_contract }
            | WorkspaceEventPayload::AgentInstanceProvisioned { work_contract, .. }
            | WorkspaceEventPayload::AgentInstanceActivated { work_contract, .. } => {
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::ShapingRecordsUpdated { records } => {
                criteria = records
                    .acceptance_criteria
                    .into_iter()
                    .map(|criterion| (criterion.id, criterion))
                    .collect();
            }
            WorkspaceEventPayload::AcceptanceCriterionUpdated {
                acceptance_criterion,
            } => {
                criteria.insert(acceptance_criterion.id, acceptance_criterion);
            }
            WorkspaceEventPayload::EvidenceRecorded { evidence: item } => {
                evidence.insert(item.id, item);
            }
            WorkspaceEventPayload::AttentionUpdated { attention_item } => {
                attention.insert(attention_item.id, attention_item);
            }
            _ => {}
        }
    }
    Ok((
        plan.context("evidence validation requires a canonical plan")?,
        contracts.into_values().collect(),
        criteria.into_values().collect(),
        evidence.into_values().collect(),
        attention.into_values().collect(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_evidence_authority(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    evidence: &Evidence,
    actor_id: dragent_workspace_model::ActorId,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<()> {
    if evidence.work_item_id != work_item_id
        || evidence.produced_by != actor_id
        || evidence.created_at_unix_ms != timestamp_unix_ms
    {
        anyhow::bail!("evidence must preserve command scope, producer, and timestamp");
    }
    let (plan, _, _, existing, _) =
        load_scheduler_authority_state(conn, project_id, work_item_id, current_revision)?;
    if existing.iter().any(|item| item.id == evidence.id) {
        anyhow::bail!("evidence identity is already in use");
    }
    if evidence
        .task_id
        .is_some_and(|task_id| !plan.tasks.iter().any(|task| task.id == task_id))
    {
        anyhow::bail!("evidence references an unknown plan task");
    }
    Ok(())
}

pub(crate) fn validate_acceptance_criterion_evidence(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    criterion: &AcceptanceCriterion,
    actor_id: dragent_workspace_model::ActorId,
    current_revision: u64,
) -> Result<()> {
    let (plan, _, criteria, evidence, _) =
        load_scheduler_authority_state(conn, project_id, work_item_id, current_revision)?;
    if !criteria.iter().any(|current| current.id == criterion.id) {
        anyhow::bail!("acceptance status can only update a canonical criterion");
    }
    match criterion.status {
        AcceptanceStatus::Satisfied => {
            let cited = criterion
                .evidence_ids
                .iter()
                .filter_map(|id| evidence.iter().find(|item| item.id == *id))
                .collect::<Vec<_>>();
            if cited.len() != criterion.evidence_ids.len()
                || cited.iter().any(|item| {
                    item.work_item_id != work_item_id
                        || item.task_id.is_some_and(|task_id| {
                            !plan.tasks.iter().any(|task| {
                                task.id == task_id
                                    && task.acceptance_criteria.contains(&criterion.id)
                            })
                        })
                })
            {
                anyhow::bail!(
                    "satisfied acceptance criteria require canonical evidence from a covering task"
                );
            }
        }
        AcceptanceStatus::Waived => {
            let local_user = ActorRef::local_user(project_id, None).id;
            if actor_id != local_user || criterion.waived_by != Some(local_user) {
                anyhow::bail!("only the local user may record an acceptance waiver");
            }
        }
        AcceptanceStatus::Pending | AcceptanceStatus::Failed => {}
    }
    Ok(())
}

pub(crate) fn ensure_work_completion_ready(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    current_revision: u64,
) -> Result<()> {
    let (plan, contracts, criteria, evidence, attention) =
        load_scheduler_authority_state(conn, project_id, work_item_id, current_revision)?;
    let projection = evaluate_scheduler(
        &plan,
        &contracts,
        &criteria,
        &evidence,
        &attention,
        DEFAULT_MAX_PARALLEL_TASKS,
    );
    if !projection.work_complete {
        anyhow::bail!(
            "work cannot complete until every task and acceptance criterion has accepted evidence or an attributable waiver"
        );
    }
    let report = build_completion_report(&plan, &criteria, &evidence, &attention);
    if !report.ready_to_complete {
        anyhow::bail!("work completion report is not ready: {}", report.summary);
    }
    Ok(())
}

pub(crate) fn validate_execution_start_isolation<'a>(
    task: &PlanTask,
    contract: &WorkContract,
    isolation: &ExecutionIsolation,
    attempts: impl IntoIterator<Item = &'a ExecutionAttempt>,
    contracts: &HashMap<WorkContractId, WorkContract>,
) -> Result<()> {
    if contract.write_scope.is_empty() {
        if let Some(target_task_id) = task.review_target_task_id {
            if isolation.kind != ExecutionIsolationKind::ReviewCandidate {
                anyhow::bail!("review execution must use its candidate isolation");
            }
            let source_attempt_id = isolation
                .source_attempt_id
                .context("review candidate isolation requires a source attempt")?;
            let source = attempts
                .into_iter()
                .find(|attempt| attempt.id == source_attempt_id)
                .context("review candidate source attempt is unavailable")?;
            if source.task_id != target_task_id
                || source.status != ExecutionAttemptStatus::Succeeded
                || source.review_resolution.is_some()
                || source.change_summary.is_none()
                || source
                    .isolation
                    .as_ref()
                    .and_then(|candidate| candidate.base_revision.as_deref())
                    != isolation.base_revision.as_deref()
            {
                anyhow::bail!("review candidate does not match the approved unresolved target");
            }
        } else if isolation.kind != ExecutionIsolationKind::SharedProject {
            anyhow::bail!("ordinary read-only execution must use shared project isolation");
        }
        return Ok(());
    }
    if task.review_target_task_id.is_some() {
        anyhow::bail!("review tasks cannot receive a writable contract");
    }
    if isolation.kind != ExecutionIsolationKind::GitWorktree {
        anyhow::bail!("writable execution requires a Git worktree isolation");
    }
    for active_attempt in attempts
        .into_iter()
        .filter(|attempt| attempt.status == ExecutionAttemptStatus::Running)
    {
        let Some(active_contract) = contracts.get(&active_attempt.contract_id) else {
            continue;
        };
        if !write_scopes_overlap(&contract.write_scope, &active_contract.write_scope) {
            continue;
        }
        let active_isolation = active_attempt.isolation.as_ref().context(
            "an overlapping legacy execution has no isolation identity; wait for it to finish",
        )?;
        if active_isolation.kind != ExecutionIsolationKind::GitWorktree
            || isolation.id == active_isolation.id
        {
            anyhow::bail!(
                "overlapping writable executions require distinct Git worktree isolations"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_agent_message_authority(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    message: &AgentMessage,
    routing: &AgentMessageRouting,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<()> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "agent communication requires a canonical snapshot after {replay_limit} events"
        );
    }
    if message.created_at_unix_ms != timestamp_unix_ms
        || message.updated_at_unix_ms != timestamp_unix_ms
    {
        anyhow::bail!("agent communication must use the authority command timestamp");
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut known_actors = HashSet::from([
        ActorRef::local_user(project_id, None).id,
        ActorRef::principal_agent(project_id).id,
    ]);
    let mut known_tasks = HashSet::new();
    let mut attempts = HashMap::<ExecutionAttemptId, ExecutionAttempt>::new();
    let mut messages = HashMap::new();
    let mut agent_roles = HashMap::new();
    let mut role_kinds = HashMap::new();
    for event in events {
        match &event.payload {
            WorkspaceEventPayload::AgentInstanceProvisioned { agent_instance, .. }
            | WorkspaceEventPayload::AgentInstanceActivated { agent_instance, .. }
            | WorkspaceEventPayload::ExecutionLeaseRenewed { agent_instance, .. }
            | WorkspaceEventPayload::AgentExecutionRetryPrepared { agent_instance, .. }
            | WorkspaceEventPayload::AgentExecutionStarted { agent_instance, .. }
            | WorkspaceEventPayload::AgentExecutionFinished { agent_instance, .. }
            | WorkspaceEventPayload::AgentExecutionReviewResolved { agent_instance, .. } => {
                let actor_id = ActorRef::agent_instance(
                    agent_instance.id,
                    dragent_workspace_model::ActorKind::WorkerAgent,
                )
                .id;
                known_actors.insert(actor_id);
                agent_roles.insert(actor_id, agent_instance.role_definition_id);
            }
            WorkspaceEventPayload::RoleDefinitionUpdated { role_definition } => {
                role_kinds.insert(role_definition.id, role_definition.kind);
            }
            WorkspaceEventPayload::PlanUpdated { plan }
            | WorkspaceEventPayload::PlanTaskStatusChanged { plan, .. } => {
                known_tasks.extend(plan.tasks.iter().map(|task| task.id));
            }
            WorkspaceEventPayload::MessagePosted { message, .. }
            | WorkspaceEventPayload::MessageDeliveryUpdated { message, .. } => {
                messages.insert(message.id, message.clone());
            }
            _ => {}
        }
        match &event.payload {
            WorkspaceEventPayload::AgentExecutionStarted { attempt, .. }
            | WorkspaceEventPayload::AgentExecutionFinished { attempt, .. }
            | WorkspaceEventPayload::AgentExecutionReviewResolved { attempt, .. } => {
                attempts.insert(attempt.id, attempt.clone());
            }
            _ => {}
        }
    }
    if !known_actors.contains(&message.sender_id)
        || routing
            .recipient_ids
            .iter()
            .any(|recipient| !known_actors.contains(recipient))
    {
        anyhow::bail!("agent communication references an unknown workspace actor");
    }
    if message.kind == AgentMessageKind::ReviewFinding
        && agent_roles
            .get(&message.sender_id)
            .and_then(|role_id| role_kinds.get(role_id))
            != Some(&dragent_workspace_model::AgentRoleKind::Reviewer)
    {
        anyhow::bail!("only an explicitly designated reviewer can post a review finding");
    }
    // Same rule as the wire protocol's `validate_message_routing`: only
    // operational kinds are about exactly one task, EXCEPT `DecisionRequest`,
    // which is also how a human speaks at work-item level while agents run.
    // `Discussion`/`StatusUpdate` (the Principal answering that human, a
    // worker's progress note) are not operational and may omit the task.
    let operational = matches!(
        message.kind,
        AgentMessageKind::Handoff
            | AgentMessageKind::Blocker
            | AgentMessageKind::DecisionRequest
            | AgentMessageKind::ReviewFinding
            | AgentMessageKind::RevisionRequest
            | AgentMessageKind::Approval
    );
    let task_id = match routing.task_id {
        Some(task_id) => Some(task_id),
        None if !operational || message.kind == AgentMessageKind::DecisionRequest => None,
        None => anyhow::bail!("operational agent communication requires a task"),
    };
    if let Some(task_id) = task_id {
        if !known_tasks.contains(&task_id) {
            anyhow::bail!("agent communication references an unknown plan task");
        }
    }
    if let Some(attempt_id) = routing.execution_attempt_id {
        let task_id =
            task_id.context("agent communication with an execution attempt requires its task")?;
        let attempt = attempts
            .get(&attempt_id)
            .context("agent communication references an unknown execution attempt")?;
        if attempt.task_id != task_id {
            anyhow::bail!("agent communication attempt belongs to a different task");
        }
        if let Some(finding) = &message.review_finding {
            if attempt
                .isolation
                .as_ref()
                .and_then(|isolation| isolation.source_attempt_id)
                != Some(finding.candidate_attempt_id)
            {
                anyhow::bail!("review finding does not describe the assigned candidate attempt");
            }
        }
    }
    if messages.contains_key(&message.id) {
        anyhow::bail!("agent message identity is already in use");
    }
    if let Some(reply_to) = message.reply_to {
        let parent = messages
            .get(&reply_to)
            .context("agent message reply target is unavailable")?;
        if parent.thread_id != message.thread_id {
            anyhow::bail!("agent message reply must remain in its original thread");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_agent_message_delivery(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    message_id: AgentMessageId,
    target: MessageDeliveryState,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentMessage, AgentMessageRouting)> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "message delivery update requires a canonical snapshot after {replay_limit} events"
        );
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut current = None;
    for event in events {
        match event.payload {
            WorkspaceEventPayload::MessagePosted {
                message,
                routing: Some(routing),
            }
            | WorkspaceEventPayload::MessageDeliveryUpdated { message, routing }
                if message.id == message_id =>
            {
                current = Some((message, routing))
            }
            _ => {}
        }
    }
    let (mut message, routing) = current.context("message delivery target is unavailable")?;
    let authorized = match target {
        MessageDeliveryState::Delivered | MessageDeliveryState::Failed => {
            actor_id == message.sender_id || routing.recipient_ids.contains(&actor_id)
        }
        MessageDeliveryState::Read => routing.recipient_ids.contains(&actor_id),
        MessageDeliveryState::LocalOnly | MessageDeliveryState::Pending => false,
    };
    if !authorized {
        anyhow::bail!("message delivery update is not authorized for this actor");
    }
    let valid_transition = matches!(
        (message.delivery_state, target),
        (
            MessageDeliveryState::Pending,
            MessageDeliveryState::Delivered
        ) | (MessageDeliveryState::Pending, MessageDeliveryState::Failed)
            | (MessageDeliveryState::Delivered, MessageDeliveryState::Read)
            | (
                MessageDeliveryState::Delivered,
                MessageDeliveryState::Failed
            )
    );
    if !valid_transition {
        anyhow::bail!("message delivery state must advance pending → delivered → read, or fail");
    }
    message.delivery_state = target;
    message.updated_at_unix_ms = timestamp_unix_ms;
    Ok((message, routing))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_execution_retry(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    failed_attempt_id: ExecutionAttemptId,
    lease_id: ExecutionLeaseId,
    timestamp_unix_ms: i64,
    lease_expires_at_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentInstance, ExecutionLease, Plan)> {
    let work = load_workspace_work_item(conn, project_id, work_item_id)?
        .context("cannot retry execution for a missing work item")?;
    if work.status != WorkStatus::Executing {
        anyhow::bail!("automatic retry requires an executing work item");
    }
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!("execution retry requires a canonical snapshot after {replay_limit} events");
    }
    let latest_expiry = timestamp_unix_ms
        .checked_add(MAX_EXECUTION_LEASE_MS)
        .context("execution retry lease timestamp overflow")?;
    if lease_expires_at_unix_ms <= timestamp_unix_ms || lease_expires_at_unix_ms > latest_expiry {
        anyhow::bail!("retry lease must be positive and no longer than 15 minutes");
    }

    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut agents = HashMap::<AgentInstanceId, AgentInstance>::new();
    let mut leases = HashMap::<ExecutionLeaseId, ExecutionLease>::new();
    let mut attempts = HashMap::<ExecutionAttemptId, ExecutionAttempt>::new();
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut latest_attempt_by_task = HashMap::<PlanTaskId, ExecutionAttemptId>::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::AgentInstanceProvisioned {
                agent_instance,
                work_contract,
            } => {
                agents.insert(agent_instance.id, agent_instance);
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::AgentInstanceActivated {
                agent_instance,
                lease,
                work_contract,
            } => {
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::ExecutionLeaseRenewed {
                agent_instance,
                lease,
            }
            | WorkspaceEventPayload::AgentExecutionRetryPrepared {
                agent_instance,
                lease,
                ..
            } => {
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            WorkspaceEventPayload::AgentExecutionStarted {
                agent_instance,
                lease,
                attempt,
                ..
            }
            | WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance,
                lease,
                attempt,
                ..
            } => {
                latest_attempt_by_task.insert(attempt.task_id, attempt.id);
                attempts.insert(attempt.id, attempt);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            _ => {}
        }
    }
    if leases.contains_key(&lease_id) {
        anyhow::bail!("retry lease identity is already in use");
    }
    let failed_attempt = attempts
        .remove(&failed_attempt_id)
        .context("retry requires a canonical failed attempt")?;
    if latest_attempt_by_task.get(&failed_attempt.task_id) != Some(&failed_attempt.id) {
        anyhow::bail!("only the latest task attempt can be retried");
    }
    if !matches!(
        failed_attempt.status,
        ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut
    ) || failed_attempt.attempt_number >= MAX_AUTOMATIC_EXECUTION_ATTEMPTS
    {
        anyhow::bail!("execution attempt is not eligible for automatic retry");
    }
    let retry_at = failed_attempt
        .finished_at_unix_ms
        .context("failed execution attempt has no finish timestamp")?
        .saturating_add(execution_retry_delay_ms(failed_attempt.attempt_number));
    if timestamp_unix_ms < retry_at {
        anyhow::bail!("execution retry backoff has not elapsed");
    }
    let previous_lease = leases
        .get(&failed_attempt.lease_id)
        .context("failed execution lease is unavailable")?;
    if previous_lease.charging.owner_id != actor_id
        || !matches!(
            previous_lease.status,
            ExecutionLeaseStatus::Released | ExecutionLeaseStatus::Expired
        )
    {
        anyhow::bail!("retry requires the charging owner and a terminal prior lease");
    }
    let mut agent_instance = agents
        .remove(&failed_attempt.agent_instance_id)
        .context("failed execution agent is unavailable")?;
    if agent_instance.status != AgentInstanceStatus::Failed
        || agent_instance.active_lease_id.is_some()
    {
        anyhow::bail!("retry requires a failed unleased agent");
    }
    let (plan, previous) = transition_plan_task(
        conn,
        project_id,
        work_item_id,
        failed_attempt.task_id,
        PlanTaskStatus::Ready,
        timestamp_unix_ms,
        current_revision,
    )?;
    if previous != PlanTaskStatus::Failed {
        anyhow::bail!("retry requires a failed plan task");
    }
    let lease = ExecutionLease {
        id: lease_id,
        work_item_id,
        agent_instance_id: agent_instance.id,
        contract_id: agent_instance.contract_id,
        status: ExecutionLeaseStatus::Active,
        charging: previous_lease.charging.clone(),
        acquired_at_unix_ms: timestamp_unix_ms,
        expires_at_unix_ms: lease_expires_at_unix_ms,
        updated_at_unix_ms: timestamp_unix_ms,
    };
    agent_instance.status = AgentInstanceStatus::Ready;
    agent_instance.active_lease_id = Some(lease.id);
    agent_instance.updated_at_unix_ms = timestamp_unix_ms;
    lease
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid retry lease: {error}"))?;
    agent_instance
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid retry agent: {error}"))?;
    Ok((agent_instance, lease, plan))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn renew_execution_lease(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    lease_id: ExecutionLeaseId,
    timestamp_unix_ms: i64,
    lease_expires_at_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentInstance, ExecutionLease)> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!("lease renewal requires a canonical snapshot after {replay_limit} events");
    }
    let latest_expiry = timestamp_unix_ms
        .checked_add(MAX_EXECUTION_LEASE_MS)
        .context("execution lease timestamp overflow")?;
    if lease_expires_at_unix_ms <= timestamp_unix_ms || lease_expires_at_unix_ms > latest_expiry {
        anyhow::bail!("renewed execution lease must be positive and no longer than 15 minutes");
    }

    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut agents = HashMap::<AgentInstanceId, AgentInstance>::new();
    let mut leases = HashMap::<ExecutionLeaseId, ExecutionLease>::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::AgentInstanceProvisioned { agent_instance, .. } => {
                agents.insert(agent_instance.id, agent_instance);
            }
            WorkspaceEventPayload::AgentInstanceActivated {
                agent_instance,
                lease,
                ..
            }
            | WorkspaceEventPayload::ExecutionLeaseRenewed {
                agent_instance,
                lease,
            }
            | WorkspaceEventPayload::AgentExecutionRetryPrepared {
                agent_instance,
                lease,
                ..
            }
            | WorkspaceEventPayload::AgentExecutionStarted {
                agent_instance,
                lease,
                ..
            }
            | WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance,
                lease,
                ..
            } => {
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            _ => {}
        }
    }
    let mut lease = leases
        .remove(&lease_id)
        .context("execution lease does not exist")?;
    let agent_instance = agents
        .remove(&lease.agent_instance_id)
        .context("execution lease agent does not exist")?;
    if lease.status != ExecutionLeaseStatus::Active
        || agent_instance.active_lease_id != Some(lease.id)
        || !matches!(
            agent_instance.status,
            AgentInstanceStatus::Ready
                | AgentInstanceStatus::Running
                | AgentInstanceStatus::Waiting
                | AgentInstanceStatus::Blocked
                | AgentInstanceStatus::Reviewing
        )
    {
        anyhow::bail!("only an active lease bound to a live agent can be renewed");
    }
    if lease.expires_at_unix_ms <= timestamp_unix_ms
        && agent_instance.status != AgentInstanceStatus::Ready
    {
        anyhow::bail!("an expired lease for an executing agent cannot be renewed");
    }
    if lease.charging.owner_id != actor_id {
        anyhow::bail!("only the charging owner can renew an execution lease");
    }
    if lease_expires_at_unix_ms <= lease.expires_at_unix_ms {
        anyhow::bail!("lease renewal must extend the current expiry");
    }
    lease.expires_at_unix_ms = lease_expires_at_unix_ms;
    lease.updated_at_unix_ms = timestamp_unix_ms;
    lease
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid renewed execution lease: {error}"))?;
    Ok((agent_instance, lease))
}
