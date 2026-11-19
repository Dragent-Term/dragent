//! Durable, metadata-only storage for the engineering workspace projection.

use crate::load_canonical_workspace_events;
use crate::store_agent::load_scheduler_authority_state;
use crate::store_agent::validate_execution_start_isolation;
use crate::store_projects::load_workspace_work_item;
use crate::workspace_store::DEFAULT_MAX_PARALLEL_TASKS;
use crate::workspace_store::MAX_CANONICAL_REPLAY_EVENTS;
use crate::workspace_store::MAX_EXECUTION_LEASE_MS;
use anyhow::{Context, Result};
use dragent_session_protocol::workspace::WorkspaceEventPayload;
use dragent_workspace_model::{
    build_completion_report, evaluate_coordination_readiness, evaluate_scheduler,
    validate_scheduler_transition, AcceptanceCriterion, AcceptanceCriterionId, AcceptanceStatus,
    ActorRef, AgentInstance, AgentInstanceId, AgentInstanceStatus, AttentionItem, AttentionItemId,
    AttentionKind, AttentionResolution, AttentionStatus, Decision, DecisionId, DecisionStatus,
    Evidence, EvidenceId, EvidenceKind, ExecutionAttempt, ExecutionAttemptId,
    ExecutionAttemptStatus, ExecutionIsolation, ExecutionIsolationKind, ExecutionLease,
    ExecutionLeaseId, ExecutionLeaseStatus, ExecutionReviewResolution, Plan, PlanTaskId,
    PlanTaskStatus, ProjectId, ProposedTeam, Provenance, ProvenanceSource, RoleDefinitionId,
    WorkContract, WorkContractId, WorkContractStatus, WorkItemId, WorkStatus,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_agent_execution(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    attempt_id: ExecutionAttemptId,
    task_id: PlanTaskId,
    agent_instance_id: AgentInstanceId,
    lease_id: ExecutionLeaseId,
    isolation: &ExecutionIsolation,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentInstance, ExecutionLease, ExecutionAttempt, Plan)> {
    let work = load_workspace_work_item(conn, project_id, work_item_id)?
        .context("cannot start execution for a work item that does not exist")?;
    if !matches!(
        work.status,
        WorkStatus::ReadyToExecute | WorkStatus::Executing
    ) {
        anyhow::bail!(
            "agent execution requires ready or executing work; current status is {:?}",
            work.status
        );
    }
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!("agent execution requires a canonical snapshot after {replay_limit} events");
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut agents = HashMap::<AgentInstanceId, AgentInstance>::new();
    let mut leases = HashMap::<ExecutionLeaseId, ExecutionLease>::new();
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut attempt_ids = HashSet::<ExecutionAttemptId>::new();
    let mut attempts = HashMap::<ExecutionAttemptId, ExecutionAttempt>::new();
    let mut prior_task_attempts = 0_u32;
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
            } => {
                if attempt.task_id == task_id {
                    prior_task_attempts = prior_task_attempts.max(attempt.attempt_number);
                }
                attempt_ids.insert(attempt.id);
                attempts.insert(attempt.id, attempt);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance,
                lease,
                attempt,
                ..
            } => {
                if attempt.task_id == task_id {
                    prior_task_attempts = prior_task_attempts.max(attempt.attempt_number);
                }
                attempt_ids.insert(attempt.id);
                attempts.insert(attempt.id, attempt);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            WorkspaceEventPayload::AgentExecutionReviewResolved {
                agent_instance,
                work_contract,
                lease,
                attempt,
                ..
            } => {
                if attempt.task_id == task_id {
                    prior_task_attempts = prior_task_attempts.max(attempt.attempt_number);
                }
                attempt_ids.insert(attempt.id);
                attempts.insert(attempt.id, attempt);
                contracts.insert(work_contract.id, work_contract);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            _ => {}
        }
    }
    if attempt_ids.contains(&attempt_id) {
        anyhow::bail!("execution attempt identity is already in use for this work item");
    }
    let mut agent_instance = agents
        .remove(&agent_instance_id)
        .context("execution start requires an activated agent instance")?;
    let lease = leases
        .remove(&lease_id)
        .context("execution start requires the agent's canonical lease")?;
    if agent_instance.status != AgentInstanceStatus::Ready
        || agent_instance.active_lease_id != Some(lease.id)
    {
        anyhow::bail!("execution start requires a ready leased agent");
    }
    if lease.status != ExecutionLeaseStatus::Active
        || lease.agent_instance_id != agent_instance.id
        || lease.contract_id != agent_instance.contract_id
        || lease.expires_at_unix_ms <= timestamp_unix_ms
    {
        anyhow::bail!("execution start requires the agent's active, unexpired lease");
    }
    isolation
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid execution isolation: {error}"))?;
    let contract = contracts
        .get(&agent_instance.contract_id)
        .context("execution start requires its canonical work contract")?;
    let (plan, previous) = transition_plan_task(
        conn,
        project_id,
        work_item_id,
        task_id,
        PlanTaskStatus::Running,
        timestamp_unix_ms,
        current_revision,
    )?;
    if previous != PlanTaskStatus::Ready {
        anyhow::bail!("only a ready task can begin an execution attempt");
    }
    let task = plan
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .context("execution start references an unknown task")?;
    if task.assigned_contract_id != Some(agent_instance.contract_id) {
        anyhow::bail!("agent contract does not own the requested task");
    }
    validate_execution_start_isolation(task, contract, isolation, attempts.values(), &contracts)?;

    agent_instance.status = AgentInstanceStatus::Running;
    agent_instance
        .started_at_unix_ms
        .get_or_insert(timestamp_unix_ms);
    agent_instance.updated_at_unix_ms = timestamp_unix_ms;
    agent_instance
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid running agent: {error}"))?;
    let attempt = ExecutionAttempt {
        id: attempt_id,
        work_item_id,
        task_id,
        agent_instance_id: agent_instance.id,
        contract_id: agent_instance.contract_id,
        lease_id: lease.id,
        attempt_number: prior_task_attempts
            .checked_add(1)
            .context("execution attempt number exhausted")?,
        status: ExecutionAttemptStatus::Running,
        model_route: agent_instance.model_route.clone(),
        isolation: Some(isolation.clone()),
        change_summary: None,
        review_resolution: None,
        started_at_unix_ms: timestamp_unix_ms,
        finished_at_unix_ms: None,
        updated_at_unix_ms: timestamp_unix_ms,
    };
    attempt
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid execution attempt: {error}"))?;
    Ok((agent_instance, lease, attempt, plan))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_agent_execution(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    attempt_id: ExecutionAttemptId,
    outcome: ExecutionAttemptStatus,
    summary: &str,
    change_summary: Option<&dragent_workspace_model::ExecutionChangeSummary>,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentInstance, ExecutionLease, ExecutionAttempt, Plan)> {
    if outcome == ExecutionAttemptStatus::Running {
        anyhow::bail!("execution finish requires a terminal outcome");
    }
    if summary.trim().is_empty() {
        anyhow::bail!("execution finish requires an outcome summary");
    }
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!("execution finish requires a canonical snapshot after {replay_limit} events");
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut agents = HashMap::<AgentInstanceId, AgentInstance>::new();
    let mut leases = HashMap::<ExecutionLeaseId, ExecutionLease>::new();
    let mut attempts = HashMap::<ExecutionAttemptId, ExecutionAttempt>::new();
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
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
            } => {
                attempts.insert(attempt.id, attempt);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            WorkspaceEventPayload::AgentExecutionFinished {
                agent_instance,
                lease,
                attempt,
                ..
            } => {
                attempts.insert(attempt.id, attempt);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
            }
            _ => {}
        }
    }
    let mut attempt = attempts
        .remove(&attempt_id)
        .context("execution finish requires a canonical running attempt")?;
    if attempt.status != ExecutionAttemptStatus::Running {
        anyhow::bail!("execution attempt is already terminal");
    }
    if outcome == ExecutionAttemptStatus::Succeeded
        && contracts
            .get(&attempt.contract_id)
            .is_some_and(|contract| !contract.write_scope.is_empty())
        && change_summary.is_none()
    {
        anyhow::bail!("successful writable execution requires an attributable change summary");
    }
    let mut agent_instance = agents
        .remove(&attempt.agent_instance_id)
        .context("execution finish requires its running agent")?;
    let mut lease = leases
        .remove(&attempt.lease_id)
        .context("execution finish requires its execution lease")?;
    if lease.charging.owner_id != actor_id {
        anyhow::bail!("only the charging owner can finish an execution attempt");
    }
    if agent_instance.status != AgentInstanceStatus::Running
        || agent_instance.active_lease_id != Some(lease.id)
        || lease.status != ExecutionLeaseStatus::Active
    {
        anyhow::bail!("execution finish state is not active and coherent");
    }
    let target_task_status = match outcome {
        ExecutionAttemptStatus::Succeeded => PlanTaskStatus::Reviewing,
        ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut => PlanTaskStatus::Failed,
        ExecutionAttemptStatus::Cancelled => PlanTaskStatus::Cancelled,
        ExecutionAttemptStatus::Paused => PlanTaskStatus::Ready,
        ExecutionAttemptStatus::Running => unreachable!(),
    };
    let (plan, previous) = transition_plan_task(
        conn,
        project_id,
        work_item_id,
        attempt.task_id,
        target_task_status,
        timestamp_unix_ms,
        current_revision,
    )?;
    if previous != PlanTaskStatus::Running {
        anyhow::bail!("only a running task can finish an execution attempt");
    }
    attempt.status = outcome;
    if let Some(change_summary) = change_summary {
        let isolation = attempt
            .isolation
            .as_ref()
            .context("execution changes require canonical isolation")?;
        change_summary
            .validate(isolation)
            .map_err(|error| anyhow::anyhow!("invalid execution changes: {error}"))?;
        attempt.change_summary = Some(change_summary.clone());
    }
    attempt.finished_at_unix_ms = Some(timestamp_unix_ms);
    attempt.updated_at_unix_ms = timestamp_unix_ms;
    match outcome {
        ExecutionAttemptStatus::Succeeded => {
            agent_instance.status = AgentInstanceStatus::Reviewing;
        }
        ExecutionAttemptStatus::Failed => {
            agent_instance.status = AgentInstanceStatus::Failed;
            agent_instance.active_lease_id = None;
            lease.status = ExecutionLeaseStatus::Released;
        }
        ExecutionAttemptStatus::Cancelled => {
            agent_instance.status = AgentInstanceStatus::Cancelled;
            agent_instance.active_lease_id = None;
            lease.status = ExecutionLeaseStatus::Cancelled;
        }
        ExecutionAttemptStatus::TimedOut => {
            agent_instance.status = AgentInstanceStatus::Failed;
            agent_instance.active_lease_id = None;
            lease.status = ExecutionLeaseStatus::Expired;
        }
        ExecutionAttemptStatus::Paused => {
            agent_instance.status = AgentInstanceStatus::Ready;
        }
        ExecutionAttemptStatus::Running => unreachable!(),
    }
    agent_instance.updated_at_unix_ms = timestamp_unix_ms;
    lease.updated_at_unix_ms = timestamp_unix_ms;
    attempt
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid finished attempt: {error}"))?;
    agent_instance
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid finished agent: {error}"))?;
    lease
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid finished lease: {error}"))?;
    Ok((agent_instance, lease, attempt, plan))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_agent_execution_review(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    attempt_id: ExecutionAttemptId,
    resolution: ExecutionReviewResolution,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(
    AgentInstance,
    WorkContract,
    ExecutionLease,
    ExecutionAttempt,
    Plan,
)> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "execution review resolution requires a canonical snapshot after {replay_limit} events"
        );
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut agents = HashMap::<AgentInstanceId, AgentInstance>::new();
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut leases = HashMap::<ExecutionLeaseId, ExecutionLease>::new();
    let mut attempts = HashMap::<ExecutionAttemptId, ExecutionAttempt>::new();
    let mut resolved = HashSet::<ExecutionAttemptId>::new();
    let mut latest_attempt = HashMap::new();
    let mut findings = HashMap::new();
    let mut resolution_actors = HashMap::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::MessagePosted { message, routing } => {
                if let (Some(finding), Some(id)) = (
                    message.review_finding,
                    routing.and_then(|r| r.execution_attempt_id),
                ) {
                    findings.insert(id, finding);
                }
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
                latest_attempt.insert(attempt.task_id, attempt.id);
                agents.insert(agent_instance.id, agent_instance);
                leases.insert(lease.id, lease);
                attempts.insert(attempt.id, attempt);
            }
            WorkspaceEventPayload::AgentExecutionReviewResolved {
                agent_instance,
                work_contract,
                lease,
                attempt,
                ..
            } => {
                resolved.insert(attempt.id);
                resolution_actors.insert(attempt.id, event.actor_id);
                agents.insert(agent_instance.id, agent_instance);
                contracts.insert(work_contract.id, work_contract);
                leases.insert(lease.id, lease);
                attempts.insert(attempt.id, attempt);
            }
            _ => {}
        }
    }
    let mut attempt = attempts
        .remove(&attempt_id)
        .context("execution review requires a canonical attempt")?;
    if attempt.status != ExecutionAttemptStatus::Succeeded {
        anyhow::bail!("review resolution requires a successful execution");
    }
    let mut agent = agents
        .remove(&attempt.agent_instance_id)
        .context("execution review agent is unavailable")?;
    let mut lease = leases
        .remove(&attempt.lease_id)
        .context("execution review lease is unavailable")?;
    let mut contract = contracts
        .remove(&attempt.contract_id)
        .context("execution review contract is unavailable")?;
    if resolution == ExecutionReviewResolution::Integrated
        && (contract.write_scope.is_empty() || attempt.change_summary.is_none())
    {
        anyhow::bail!("integration requires attributable changes from a writable contract");
    }
    // Never integrate an empty worktree as success: it would mark the task
    // Verifying→Completed with zero evidence and stall the DAG behind a
    // "0 changed files ready for integration" card. The user must request a
    // revision or discard instead.
    if resolution == ExecutionReviewResolution::Integrated
        && attempt
            .change_summary
            .as_ref()
            .is_some_and(|summary| summary.files.is_empty())
    {
        anyhow::bail!(
            "integration requires at least one captured file change; request a revision or discard this attempt"
        );
    }
    if resolution == ExecutionReviewResolution::Accepted
        && (!contract.write_scope.is_empty() || attempt.change_summary.is_some())
    {
        anyhow::bail!("accepting a result requires a read-only execution");
    }
    let principal_id = ActorRef::principal_agent(project_id).id;
    let (mut recovery_plan, _, _, _, _) =
        load_scheduler_authority_state(conn, project_id, work_item_id, current_revision)?;
    let recovery = resolution == ExecutionReviewResolution::RevisionRequested
        && actor_id == principal_id
        && resolution_actors.get(&attempt_id) == Some(&principal_id)
        && latest_attempt.get(&attempt.task_id) == Some(&attempt_id)
        && findings.get(&attempt_id).is_some_and(|finding| {
            attempts
                .get(&finding.candidate_attempt_id)
                .is_some_and(|candidate| {
                    recovery_plan
                        .tasks
                        .iter()
                        .find(|t| t.id == attempt.task_id)
                        .is_some_and(|task| {
                            dragent_workspace_model::legacy_review_needs_rearm(
                                &attempt,
                                candidate,
                                &contract,
                                task,
                                finding,
                                principal_id,
                            )
                        })
                })
        });
    if resolved.contains(&attempt_id) && !recovery {
        anyhow::bail!("execution review is already resolved");
    }
    if recovery {
        let work = load_workspace_work_item(conn, project_id, work_item_id)?
            .context("review recovery requires a work item")?;
        if !matches!(
            work.status,
            WorkStatus::ReadyToExecute
                | WorkStatus::Executing
                | WorkStatus::Reviewing
                | WorkStatus::Verifying
                | WorkStatus::Blocked
        ) || agent.status != AgentInstanceStatus::Completed
            || agent.active_lease_id.is_some()
            || lease.status != ExecutionLeaseStatus::Released
        {
            anyhow::bail!(
                "legacy review recovery requires inactive completed reviewer in active work"
            );
        }
        agent.status = AgentInstanceStatus::Reviewing;
        agent.active_lease_id = Some(lease.id);
        lease.status = ExecutionLeaseStatus::Active;
        lease.expires_at_unix_ms = timestamp_unix_ms.saturating_add(MAX_EXECUTION_LEASE_MS);
    }
    if (lease.charging.owner_id != actor_id && principal_id != actor_id)
        || agent.status != AgentInstanceStatus::Reviewing
        || agent.active_lease_id != Some(lease.id)
        || lease.status != ExecutionLeaseStatus::Active
    {
        anyhow::bail!("execution review requires its charging owner and active review lease");
    }
    let target = match resolution {
        ExecutionReviewResolution::Accepted => PlanTaskStatus::Verifying,
        ExecutionReviewResolution::Integrated => PlanTaskStatus::Verifying,
        ExecutionReviewResolution::RevisionRequested => PlanTaskStatus::Ready,
        ExecutionReviewResolution::Discarded => PlanTaskStatus::Cancelled,
    };
    let (plan, previous) = if recovery {
        recovery_plan
            .tasks
            .iter_mut()
            .find(|t| t.id == attempt.task_id)
            .unwrap()
            .status = target;
        recovery_plan.version = recovery_plan.version.saturating_add(1);
        recovery_plan.updated_at_unix_ms = timestamp_unix_ms;
        recovery_plan.validate().map_err(|e| anyhow::anyhow!(e))?;
        (recovery_plan, PlanTaskStatus::Reviewing)
    } else {
        transition_plan_task(
            conn,
            project_id,
            work_item_id,
            attempt.task_id,
            target,
            timestamp_unix_ms,
            current_revision,
        )?
    };
    if previous != PlanTaskStatus::Reviewing {
        anyhow::bail!("execution review requires a reviewing plan task");
    }
    let accepted = matches!(
        resolution,
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated
    );
    let contract_has_remaining_work = accepted
        && contract.task_ids.iter().any(|task_id| {
            *task_id != attempt.task_id
                && plan.tasks.iter().any(|task| {
                    task.id == *task_id
                        && !matches!(
                            task.status,
                            PlanTaskStatus::Completed | PlanTaskStatus::Cancelled
                        )
                })
        });
    agent.status = match resolution {
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated
            if contract_has_remaining_work =>
        {
            AgentInstanceStatus::Ready
        }
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated => {
            AgentInstanceStatus::Completed
        }
        ExecutionReviewResolution::RevisionRequested => AgentInstanceStatus::Ready,
        ExecutionReviewResolution::Discarded => AgentInstanceStatus::Cancelled,
    };
    if resolution != ExecutionReviewResolution::RevisionRequested && !contract_has_remaining_work {
        agent.active_lease_id = None;
    }
    agent.updated_at_unix_ms = timestamp_unix_ms;
    lease.status = match resolution {
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated
            if contract_has_remaining_work =>
        {
            ExecutionLeaseStatus::Active
        }
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated => {
            ExecutionLeaseStatus::Released
        }
        ExecutionReviewResolution::RevisionRequested => ExecutionLeaseStatus::Active,
        ExecutionReviewResolution::Discarded => ExecutionLeaseStatus::Cancelled,
    };
    lease.updated_at_unix_ms = timestamp_unix_ms;
    contract.status = match resolution {
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated
            if contract_has_remaining_work =>
        {
            WorkContractStatus::Active
        }
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated => {
            WorkContractStatus::Completed
        }
        ExecutionReviewResolution::RevisionRequested => WorkContractStatus::Active,
        ExecutionReviewResolution::Discarded => WorkContractStatus::Revoked,
    };
    contract.version = contract.version.saturating_add(1);
    contract.updated_by = actor_id;
    contract.updated_at_unix_ms = timestamp_unix_ms;
    attempt.review_resolution = Some(resolution);
    attempt.updated_at_unix_ms = timestamp_unix_ms;
    agent.validate().map_err(|error| anyhow::anyhow!(error))?;
    lease.validate().map_err(|error| anyhow::anyhow!(error))?;
    contract
        .validate()
        .map_err(|error| anyhow::anyhow!(error))?;
    attempt.validate().map_err(|error| anyhow::anyhow!(error))?;
    Ok((agent, contract, lease, attempt, plan))
}

/// Convert an explicitly accepted execution review into the canonical proof
/// required by definition of done. This runs inside the same SQLite authority
/// transaction as the review resolution, so replay can never observe a task as
/// completed without its evidence and criterion adjudication.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_accepted_task(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    accepted_by: dragent_workspace_model::ActorId,
    attempt: &ExecutionAttempt,
    completed_contract: &WorkContract,
    reviewed_plan: &Plan,
    _agent_instance_id: AgentInstanceId,
    resolution: ExecutionReviewResolution,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(Vec<Evidence>, Vec<AcceptanceCriterion>, Plan, bool)> {
    if !matches!(
        resolution,
        ExecutionReviewResolution::Accepted | ExecutionReviewResolution::Integrated
    ) {
        anyhow::bail!("only accepted execution reviews can produce completion evidence");
    }
    let (_, mut contracts, mut criteria, mut existing_evidence, attention) =
        load_scheduler_authority_state(conn, project_id, work_item_id, current_revision)?;
    let task = reviewed_plan
        .tasks
        .iter()
        .find(|task| task.id == attempt.task_id)
        .context("accepted execution references an unknown task")?;
    if task.status != PlanTaskStatus::Verifying || task.acceptance_criteria.is_empty() {
        anyhow::bail!("accepted execution must enter verification with explicit criteria");
    }

    // An automatic acceptance must be backed by a passing independent review
    // of this exact attempt, not merely by the existence of a file diff. Human
    // acceptance remains an explicit user confirmation, never a claimed test.
    let principal = ActorRef::principal_agent(project_id).id;
    let verification =
        if accepted_by == principal {
            Some(
            load_canonical_workspace_events(
                conn,
                project_id,
                work_item_id,
                0,
                MAX_CANONICAL_REPLAY_EVENTS as usize,
            )?
            .into_iter()
            .rev()
            .find_map(|event| {
                let WorkspaceEventPayload::MessagePosted { message, routing } = event.payload
                else {
                    return None;
                };
                let finding = message.review_finding.as_ref()?;
                let matches_attempt = match resolution {
                    ExecutionReviewResolution::Integrated => {
                        finding.candidate_attempt_id == attempt.id
                    }
                    ExecutionReviewResolution::Accepted => {
                        routing
                            .as_ref()
                            .and_then(|routing| routing.execution_attempt_id)
                            == Some(attempt.id)
                    }
                    _ => false,
                };
                if matches_attempt
                    && finding.verdict == dragent_workspace_model::ReviewVerdict::Pass
                    && !finding.findings.iter().any(|item| {
                        matches!(
                            item.severity,
                            dragent_workspace_model::ReviewFindingSeverity::Error
                                | dragent_workspace_model::ReviewFindingSeverity::Critical
                        )
                    })
                {
                    Some((message.id, finding.summary.clone(), finding.candidate_attempt_id,
                        routing.as_ref().and_then(|r| r.execution_attempt_id)))
                } else {
                    None
                }
            })
            .context(
                "automatic acceptance requires a passing independent review for this exact attempt",
            )?,
        )
        } else {
            None
        };
    let mut command_evidence = Vec::new();
    let mut criterion_commands: HashMap<AcceptanceCriterionId, Vec<EvidenceId>> = HashMap::new();
    if let Some((_, _, candidate_id, reviewer_id)) = &verification {
        let reviewer_id = reviewer_id.context("passing review must identify its execution")?;
        let receipts = crate::host_verification::load(conn, work_item_id, *candidate_id)?;
        let events = load_canonical_workspace_events(
            conn,
            project_id,
            work_item_id,
            0,
            MAX_CANONICAL_REPLAY_EVENTS as usize,
        )?;
        let reviewer = events
            .iter()
            .rev()
            .find_map(|event| match &event.payload {
                WorkspaceEventPayload::AgentExecutionFinished { attempt, .. }
                | WorkspaceEventPayload::AgentExecutionReviewResolved { attempt, .. }
                    if attempt.id == reviewer_id =>
                {
                    Some(attempt)
                }
                _ => None,
            })
            .context("passing review execution is unavailable")?;
        anyhow::ensure!(
            reviewer.status == ExecutionAttemptStatus::Succeeded
                && reviewer
                    .isolation
                    .as_ref()
                    .is_some_and(|i| i.kind == ExecutionIsolationKind::ReviewCandidate
                        && i.source_attempt_id == Some(*candidate_id)),
            "passing review must inspect this exact candidate"
        );
        let candidate_task_id = events
            .iter()
            .rev()
            .find_map(|event| match &event.payload {
                WorkspaceEventPayload::AgentExecutionFinished { attempt, .. }
                    if attempt.id == *candidate_id =>
                {
                    Some(attempt.task_id)
                }
                _ => None,
            })
            .context("passing review candidate is unavailable")?;
        let candidate_task = reviewed_plan
            .tasks
            .iter()
            .find(|t| t.id == candidate_task_id)
            .context("passing review candidate task is unavailable")?;
        for criterion_id in &candidate_task.acceptance_criteria {
            if criteria
                .iter()
                .any(|c| c.id == *criterion_id && c.status == AcceptanceStatus::Waived)
            {
                continue;
            }
            let checks =
                crate::host_verification::passing_receipts(&receipts, reviewer_id, *criterion_id)?;
            anyhow::ensure!(
                checks
                    .iter()
                    .all(|r| r.started_at_unix_ms >= reviewer.started_at_unix_ms
                        && r.finished_at_unix_ms.is_some_and(|finished| reviewer
                            .finished_at_unix_ms
                            .is_some_and(|end| finished <= end))),
                "verification must complete during the reviewing execution"
            );
            if resolution != ExecutionReviewResolution::Integrated {
                continue;
            }
            for receipt in checks {
                let id = EvidenceId(Uuid::new_v5(&attempt.id.0, receipt.id.as_bytes()));
                criterion_commands
                    .entry(*criterion_id)
                    .or_default()
                    .push(id);
                if command_evidence.iter().any(|e: &Evidence| e.id == id) {
                    continue;
                }
                command_evidence.push(Evidence {
                    id,
                    work_item_id,
                    task_id: Some(attempt.task_id),
                    kind: EvidenceKind::Test,
                    summary: format!(
                        "Host executed `{}` in {}: exit 0.\n{}",
                        receipt.command, receipt.cwd, receipt.output
                    ),
                    source: format!("host-command://{}", receipt.id),
                    produced_by: accepted_by,
                    created_at_unix_ms: timestamp_unix_ms,
                });
            }
        }
    }
    let change_summary = match resolution {
        ExecutionReviewResolution::Integrated => {
            let changes = attempt
                .change_summary
                .as_ref()
                .context("integrated execution has no attributable changes")?;
            let (added, deleted) = changes.files.iter().fold((0_u64, 0_u64), |totals, file| {
                (
                    totals.0.saturating_add(file.added_lines.unwrap_or(0)),
                    totals.1.saturating_add(file.deleted_lines.unwrap_or(0)),
                )
            });
            format!(
                "Accepted and integrated {} changed file{} (+{added}, -{deleted}) for {}.",
                changes.files.len(),
                if changes.files.len() == 1 { "" } else { "s" },
                task.title
            )
        }
        ExecutionReviewResolution::Accepted => {
            format!("Accepted reviewed execution result for {}.", task.title)
        }
        _ => unreachable!(),
    };
    let (kind, summary, source) = match verification {
        Some((message_id, review_summary, _, _)) => (
            EvidenceKind::Review,
            format!("{change_summary} Independent review: {review_summary}"),
            format!("agent-review://message/{}", message_id.0),
        ),
        None => (
            EvidenceKind::UserConfirmation,
            format!("Explicitly accepted by the workspace owner. {change_summary}"),
            format!("agent-execution://attempt/{}", attempt.id.0),
        ),
    };
    let evidence = Evidence {
        id: EvidenceId(Uuid::new_v5(&attempt.id.0, b"evidence:accepted-execution")),
        work_item_id,
        task_id: Some(attempt.task_id),
        kind,
        summary,
        source,
        produced_by: accepted_by,
        created_at_unix_ms: timestamp_unix_ms,
    };
    evidence
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid accepted execution evidence: {error}"))?;
    if existing_evidence.iter().any(|item| item.id == evidence.id) {
        anyhow::bail!("accepted execution evidence is already canonical");
    }

    let task_criterion_ids = task
        .acceptance_criteria
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut updated_criteria = Vec::with_capacity(task_criterion_ids.len());
    for criterion in criteria
        .iter_mut()
        .filter(|criterion| task_criterion_ids.contains(&criterion.id))
    {
        if criterion.status == AcceptanceStatus::Failed {
            anyhow::bail!(
                "failed acceptance criterion must be resolved or explicitly waived before accepting execution"
            );
        }
        if criterion.status != AcceptanceStatus::Waived {
            criterion.status = AcceptanceStatus::Satisfied;
            for id in criterion_commands.get(&criterion.id).into_iter().flatten() {
                if !criterion.evidence_ids.contains(id) {
                    criterion.evidence_ids.push(*id);
                }
            }
            if !criterion.evidence_ids.contains(&evidence.id) {
                criterion.evidence_ids.push(evidence.id);
            }
            criterion.waiver_reason = None;
            criterion.waived_by = None;
            criterion.updated_at_unix_ms = timestamp_unix_ms;
            criterion.provenance.push(Provenance {
                source: ProvenanceSource::Evidence {
                    evidence_id: evidence.id,
                },
                recorded_by: accepted_by,
                recorded_at_unix_ms: timestamp_unix_ms,
            });
            criterion.validate().map_err(|error| {
                anyhow::anyhow!("accepted execution produced an invalid criterion: {error}")
            })?;
        }
        updated_criteria.push(criterion.clone());
    }
    if updated_criteria.len() != task_criterion_ids.len() {
        anyhow::bail!("accepted execution is missing a canonical acceptance criterion");
    }

    let mut completed_plan = reviewed_plan.clone();
    let completed_task = completed_plan
        .tasks
        .iter_mut()
        .find(|task| task.id == attempt.task_id)
        .context("accepted execution task disappeared during verification")?;
    completed_task.status = PlanTaskStatus::Completed;
    completed_plan.version = completed_plan.version.saturating_add(1);
    completed_plan.updated_at_unix_ms = timestamp_unix_ms;
    completed_plan
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid evidence-completed plan: {error}"))?;

    contracts.retain(|contract| contract.id != completed_contract.id);
    contracts.push(completed_contract.clone());
    command_evidence.push(evidence);
    existing_evidence.extend(command_evidence.clone());
    let projection = evaluate_scheduler(
        &completed_plan,
        &contracts,
        &criteria,
        &existing_evidence,
        &attention,
        DEFAULT_MAX_PARALLEL_TASKS,
    );
    let task_complete = projection
        .tasks
        .iter()
        .find(|task| task.task_id == attempt.task_id)
        .is_some_and(|task| task.completion_gate_satisfied);
    if !task_complete {
        anyhow::bail!("accepted execution did not satisfy its definition of done");
    }
    let report =
        build_completion_report(&completed_plan, &criteria, &existing_evidence, &attention);
    if projection.work_complete != report.ready_to_complete {
        anyhow::bail!("scheduler and completion report disagree on definition of done");
    }
    Ok((
        command_evidence,
        updated_criteria,
        completed_plan,
        projection.work_complete,
    ))
}

pub(crate) fn raise_attention(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    attention_item: &AttentionItem,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<AttentionItem> {
    if attention_item.work_item_id != work_item_id {
        anyhow::bail!("attention belongs to a different work item");
    }
    if attention_item.status != AttentionStatus::Open
        || attention_item.created_at_unix_ms != timestamp_unix_ms
    {
        anyhow::bail!("new attention must be open and use the command timestamp");
    }
    attention_item
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid attention request: {error}"))?;

    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "attention requests require a canonical snapshot after {replay_limit} events"
        );
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut attempts = HashMap::<ExecutionAttemptId, ExecutionAttempt>::new();
    let mut attentions = HashMap::<AttentionItemId, AttentionItem>::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::AgentExecutionStarted { attempt, .. }
            | WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } => {
                attempts.insert(attempt.id, attempt);
            }
            WorkspaceEventPayload::AttentionUpdated { attention_item } => {
                attentions.insert(attention_item.id, attention_item);
            }
            _ => {}
        }
    }
    if attentions.contains_key(&attention_item.id) {
        anyhow::bail!("attention identity is already in use for this work item");
    }
    if attention_item.kind == AttentionKind::Permission {
        let attempt_id = attention_item
            .execution_attempt_id
            .context("permission attention requires an execution attempt")?;
        let attempt = attempts
            .get(&attempt_id)
            .context("permission attention requires a canonical execution attempt")?;
        if attempt.status != ExecutionAttemptStatus::Running
            || attention_item.task_id != Some(attempt.task_id)
        {
            anyhow::bail!("permission attention must target its running execution task");
        }
        let tool_call_id = attention_item
            .tool_call_id
            .as_deref()
            .context("permission attention requires a tool call")?;
        if attentions.values().any(|existing| {
            existing.kind == AttentionKind::Permission
                && matches!(
                    existing.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                )
                && existing.execution_attempt_id == Some(attempt_id)
                && existing.tool_call_id.as_deref() == Some(tool_call_id)
        }) {
            anyhow::bail!("an open permission attention already exists for this tool call");
        }
    }
    Ok(attention_item.clone())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_attention(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    attention_item_id: AttentionItemId,
    outcome: AttentionResolution,
    resolution: &str,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<AttentionItem> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "attention resolution requires a canonical snapshot after {replay_limit} events"
        );
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut attention = None;
    for event in events {
        if let WorkspaceEventPayload::AttentionUpdated { attention_item } = event.payload {
            if attention_item.id == attention_item_id {
                attention = Some(attention_item);
            }
        }
    }
    let mut attention = attention.context("attention item does not exist")?;
    if !matches!(
        attention.status,
        AttentionStatus::Open | AttentionStatus::Acknowledged
    ) {
        anyhow::bail!("attention item is already terminal");
    }
    if !attention.assigned_to.contains(&actor_id) {
        anyhow::bail!("only an assigned actor can resolve this attention item");
    }
    attention.status = if outcome == AttentionResolution::Dismissed {
        AttentionStatus::Dismissed
    } else {
        AttentionStatus::Resolved
    };
    attention.resolved_at_unix_ms = Some(timestamp_unix_ms);
    attention.resolution_kind = Some(outcome);
    attention.resolution = Some(resolution.trim().to_string());
    attention
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid attention resolution: {error}"))?;
    Ok(attention)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn provision_agent_instance(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    actor_id: dragent_workspace_model::ActorId,
    agent_instance_id: AgentInstanceId,
    contract_id: WorkContractId,
    model_route: &str,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(AgentInstance, WorkContract)> {
    let work = load_workspace_work_item(conn, project_id, work_item_id)?
        .context("cannot provision an agent for a work item that does not exist")?;
    if work.status != WorkStatus::ReadyToExecute {
        anyhow::bail!(
            "agent provisioning requires ready-to-execute work; current status is {:?}",
            work.status
        );
    }
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "agent provisioning requires a canonical snapshot after {replay_limit} events"
        );
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut agent_instance_ids = HashSet::<AgentInstanceId>::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::AgentInstanceProvisioned {
                agent_instance,
                work_contract,
            } => {
                agent_instance_ids.insert(agent_instance.id);
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::AgentInstanceActivated {
                agent_instance,
                work_contract,
                ..
            } => {
                agent_instance_ids.insert(agent_instance.id);
                contracts.insert(work_contract.id, work_contract);
            }
            _ => {}
        }
    }
    if agent_instance_ids.contains(&agent_instance_id) {
        anyhow::bail!("agent instance identity is already in use for this work item");
    }
    let mut work_contract = contracts
        .remove(&contract_id)
        .context("agent provisioning requires a canonical work contract")?;
    if work_contract.status != WorkContractStatus::Approved {
        anyhow::bail!(
            "agent provisioning requires an approved contract; current status is {:?}",
            work_contract.status
        );
    }
    if work_contract.agent_instance_id.is_some() {
        anyhow::bail!("approved work contract already has a provisioned agent instance");
    }

    let agent_instance = AgentInstance {
        id: agent_instance_id,
        work_item_id,
        role_definition_id: work_contract.role_definition_id,
        contract_id: work_contract.id,
        status: AgentInstanceStatus::Pending,
        execution_location: work_contract.execution_location,
        model_route: model_route.trim().to_string(),
        active_lease_id: None,
        started_at_unix_ms: None,
        updated_at_unix_ms: timestamp_unix_ms,
    };
    agent_instance
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid provisioned agent: {error}"))?;
    work_contract.agent_instance_id = Some(agent_instance.id);
    work_contract.version = work_contract
        .version
        .checked_add(1)
        .context("work contract version exhausted")?;
    work_contract.updated_by = actor_id;
    work_contract.updated_at_unix_ms = timestamp_unix_ms;
    work_contract
        .validate()
        .map_err(|error| anyhow::anyhow!("invalid provisioned contract binding: {error}"))?;
    Ok((agent_instance, work_contract))
}

pub(crate) fn transition_plan_task(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    task_id: PlanTaskId,
    target: PlanTaskStatus,
    timestamp_unix_ms: i64,
    current_revision: u64,
) -> Result<(Plan, PlanTaskStatus)> {
    let work = load_workspace_work_item(conn, project_id, work_item_id)?
        .context("cannot transition a task for a work item that does not exist")?;
    if !matches!(
        work.status,
        WorkStatus::ReadyToExecute
            | WorkStatus::Executing
            | WorkStatus::Reviewing
            | WorkStatus::Verifying
            | WorkStatus::Blocked
    ) {
        anyhow::bail!(
            "task transitions require coordinated work; current status is {:?}",
            work.status
        );
    }
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!("task scheduling requires a canonical snapshot after {replay_limit} events");
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut plan = None;
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut criteria = HashMap::<AcceptanceCriterionId, AcceptanceCriterion>::new();
    let mut evidence = HashMap::<EvidenceId, Evidence>::new();
    let mut attention = HashMap::<AttentionItemId, AttentionItem>::new();
    for event in events {
        match event.payload {
            WorkspaceEventPayload::PlanUpdated { plan: updated_plan }
            | WorkspaceEventPayload::PlanTaskStatusChanged {
                plan: updated_plan, ..
            }
            | WorkspaceEventPayload::AgentExecutionStarted {
                plan: updated_plan, ..
            }
            | WorkspaceEventPayload::AgentExecutionFinished {
                plan: updated_plan, ..
            }
            | WorkspaceEventPayload::AgentExecutionReviewResolved {
                plan: updated_plan, ..
            }
            | WorkspaceEventPayload::AgentExecutionRetryPrepared {
                plan: updated_plan, ..
            } => plan = Some(updated_plan),
            WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::AgentInstanceProvisioned { work_contract, .. }
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
            WorkspaceEventPayload::EvidenceRecorded { evidence: recorded } => {
                evidence.insert(recorded.id, recorded);
            }
            WorkspaceEventPayload::AttentionUpdated { attention_item } => {
                attention.insert(attention_item.id, attention_item);
            }
            _ => {}
        }
    }
    let mut plan = plan.context("task transition requires a canonical plan")?;
    let projection = evaluate_scheduler(
        &plan,
        &contracts.into_values().collect::<Vec<_>>(),
        &criteria.into_values().collect::<Vec<_>>(),
        &evidence.into_values().collect::<Vec<_>>(),
        &attention.into_values().collect::<Vec<_>>(),
        DEFAULT_MAX_PARALLEL_TASKS,
    );
    validate_scheduler_transition(&plan, &projection, task_id, target)
        .map_err(|error| anyhow::anyhow!("scheduler rejected task transition: {error}"))?;
    let task = plan
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .context("scheduler accepted an unknown task")?;
    let previous = task.status;
    task.status = target;
    plan.version = plan.version.saturating_add(1);
    plan.updated_at_unix_ms = timestamp_unix_ms;
    plan.validate()
        .map_err(|error| anyhow::anyhow!("scheduler produced an invalid plan: {error}"))?;
    Ok((plan, previous))
}

pub(crate) fn ensure_coordination_is_ready(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    current_revision: u64,
) -> Result<()> {
    let replay_limit = usize::try_from(MAX_CANONICAL_REPLAY_EVENTS).unwrap_or(usize::MAX);
    if current_revision > replay_limit as u64 {
        anyhow::bail!(
            "coordination readiness requires a canonical snapshot after {replay_limit} events"
        );
    }
    let events = load_canonical_workspace_events(conn, project_id, work_item_id, 0, replay_limit)?;
    let mut shaping_proposal = None;
    // Per-record decision state supersedes the proposal snapshot: inline
    // resolutions must unlock coordination here exactly as they do in the UI
    // readiness checklist (single source of truth for the gate).
    let mut decisions: Option<HashMap<DecisionId, Decision>> = None;
    let mut proposed_team: Option<ProposedTeam> = None;
    let mut role_definition_ids = HashSet::<RoleDefinitionId>::new();
    let mut contracts = HashMap::<WorkContractId, WorkContract>::new();
    let mut plan = None;
    for event in events {
        match event.payload {
            WorkspaceEventPayload::ShapingProposalUpdated { proposal } => {
                shaping_proposal = Some(proposal);
            }
            WorkspaceEventPayload::ShapingRecordsUpdated { records } => {
                decisions = Some(
                    records
                        .decisions
                        .into_iter()
                        .map(|decision| (decision.id, decision))
                        .collect(),
                );
            }
            WorkspaceEventPayload::DecisionUpdated { decision } => {
                decisions
                    .get_or_insert_with(HashMap::new)
                    .insert(decision.id, decision);
            }
            WorkspaceEventPayload::RoleDefinitionUpdated { role_definition } => {
                role_definition_ids.insert(role_definition.id);
            }
            WorkspaceEventPayload::WorkContractUpdated { work_contract } => {
                contracts.insert(work_contract.id, work_contract);
            }
            WorkspaceEventPayload::ProposedTeamUpdated {
                proposed_team: team,
            } => {
                proposed_team = Some(team);
            }
            WorkspaceEventPayload::PlanUpdated { plan: updated_plan } => {
                plan = Some(updated_plan);
            }
            _ => {}
        }
    }
    let open_decisions = decisions.as_ref().map(|decisions| {
        decisions
            .values()
            .filter(|decision| decision.status == DecisionStatus::Open)
            .count()
    });
    let readiness = evaluate_coordination_readiness(
        shaping_proposal.as_ref(),
        open_decisions,
        proposed_team.as_ref(),
        &role_definition_ids,
        &contracts,
        plan.as_ref(),
    );
    if !readiness.is_ready() {
        anyhow::bail!(
            "coordination is not ready: {}/{} role contracts approved; {} blocking issue(s)",
            readiness.approved_role_count,
            readiness.role_count,
            readiness.issues.len()
        );
    }
    Ok(())
}
