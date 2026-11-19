//! Workspace execution launch planning: routes, actions, retries.
//!
//! Moved verbatim; same behavior.

//! Durable workspace-to-agent execution boundary.
//!
//! The workspace authority commits an `AgentExecutionStarted` event before a
//! provider or tool loop is launched. This module turns that committed fact
//! into an immutable launch request and turns the runner's terminal result
//! back into a revision-guarded workspace command. It deliberately owns no UI
//! state and never starts work while canonical history is merely being replayed.

use super::execution_pipeline::*;
use super::execution_review::*;
use super::execution_runtime::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    evaluate_scheduler, execution_retry_delay_ms, task_coordination_thread_id, ActorId, ActorKind,
    ActorRef, AgentInstance, AgentInstanceId, AgentInstanceStatus, AgentMessageKind, AttentionItem,
    AttentionItemId, AttentionKind, AttentionResolution, AttentionStatus, ChargingAuthorization,
    CoordinationHealth, DeviceId, ExecutionAttempt, ExecutionAttemptId, ExecutionAttemptStatus,
    ExecutionChangeSummary, ExecutionIsolation, ExecutionLease, ExecutionLeaseId,
    ExecutionReviewResolution, PlanTask, PlanTaskId, PlanTaskStatus, ProjectId, ReviewFindingItem,
    ReviewVerdict, StructuredReviewFinding, WorkContract, WorkContractId, WorkContractStatus,
    WorkItemId, WorkStatus, WorkspaceState, MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
};
use thiserror::Error;

pub(crate) const LEASE_RENEWAL_LEAD_MS: i64 = 2 * 60 * 1_000;
pub(crate) const LEASE_RENEWAL_DURATION_MS: i64 = 10 * 60 * 1_000;
pub(crate) const MAX_EXECUTION_ACTIVITY: usize = 256;
pub(crate) const MAX_EXECUTION_ACTIVITY_TEXT_CHARS: usize = 4_000;
pub(crate) const MAX_SESSION_EVENTS_PER_POLL: usize = 64;

pub fn execution_completion_message_kind(
    state: &WorkspaceState,
    agent: &AgentInstance,
    status: ExecutionAttemptStatus,
) -> Option<(AgentMessageKind, ActorKind)> {
    let role_kind = state
        .role_definition(agent.role_definition_id)
        .map(|role| role.kind)
        .unwrap_or_default();
    let message_kind = match status {
        ExecutionAttemptStatus::Succeeded => match role_kind {
            dragent_workspace_model::AgentRoleKind::Worker => AgentMessageKind::Handoff,
            dragent_workspace_model::AgentRoleKind::Reviewer => AgentMessageKind::ReviewFinding,
        },
        ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut => {
            AgentMessageKind::Blocker
        }
        _ => return None,
    };
    Some((message_kind, role_kind.actor_kind()))
}

/// Detect final responses that describe a blocker while the provider still
/// emitted `Done`. Without this, "Blocked: no file-editing tool is available"
/// is stored as `Succeeded`, routed as a `Handoff`, and stalls review with
/// zero changed files.
pub(crate) fn completion_reports_blocker(summary: &str) -> bool {
    let normalized = summary.to_lowercase();
    const MARKERS: &[&str] = &[
        "blocked",
        "blocker",
        "could not safely",
        "could not complete",
        "couldn't complete",
        "unable to complete",
        "did not complete",
        "was not completed",
        "no file-editing",
        "no tool",
        "tools are unavailable",
        "tool is currently available",
        "tool is not currently available",
        "unavailable in this session",
        "lease expired",
        "missing authority",
        "missing permission",
        "requires approval",
        "requires consent",
    ];
    MARKERS.iter().any(|marker| normalized.contains(marker))
}

/// Watchdog: coordination started but the projection is stalled (no running
/// work, nothing launchable, no pending retry, no blocking attention, grace
/// period elapsed). Returns a durable Failure attention action with a
/// concrete next step. Never leaves the system looking idle.
pub fn next_stall_attention(
    state: &WorkspaceState,
    maximum_parallel_tasks: usize,
    now_unix_ms: i64,
) -> Option<WorkspaceLaunchAction> {
    if state.coordination_health(maximum_parallel_tasks, now_unix_ms) != CoordinationHealth::Stalled
    {
        return None;
    }
    let work = state.active_work()?;
    let attention_id = AttentionItemId(uuid::Uuid::new_v5(&work.id.0, b"coordination-stall:v1"));
    if state
        .attention_items()
        .iter()
        .any(|attention| attention.id == attention_id)
    {
        return None;
    }
    let principal = ActorRef::principal_agent(state.project().id).id;
    let owner = ActorRef::local_user(state.project().id, None).id;
    Some(WorkspaceLaunchAction::EscalateFailure {
        attention_item: AttentionItem {
            id: attention_id,
            work_item_id: work.id,
            task_id: None,
            thread_id: None,
            execution_attempt_id: None,
            tool_call_id: None,
            kind: AttentionKind::Failure,
            status: AttentionStatus::Open,
            summary: "Coordination stalled: work is Executing but no agent is running, nothing is scheduled, and no blocker explains it. Next step: check provider/model status and resume; or cancel the work if it should not continue.".into(),
            requested_by: principal,
            assigned_to: vec![owner],
            created_at_unix_ms: now_unix_ms,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        },
    })
}

/// Billing park: execution cannot start on the selected route without
/// credentials, but nothing is cancelled. Returns a durable Budget attention
/// with a concrete resolution action; leases, attempts, and stable runtime
/// identities are preserved so restoring billing resumes without duplicates.
pub fn billing_park_attention(
    state: &WorkspaceState,
    route: &str,
    now_unix_ms: i64,
) -> Option<WorkspaceLaunchAction> {
    let work = state.active_work()?;
    let attention_id = AttentionItemId(uuid::Uuid::new_v5(
        &work.id.0,
        b"coordination-billing-park:v1",
    ));
    if state
        .attention_items()
        .iter()
        .any(|attention| attention.id == attention_id)
    {
        return None;
    }
    let principal = ActorRef::principal_agent(state.project().id).id;
    let owner = ActorRef::local_user(state.project().id, None).id;
    Some(WorkspaceLaunchAction::EscalateFailure {
        attention_item: AttentionItem {
            id: attention_id,
            work_item_id: work.id,
            task_id: None,
            thread_id: None,
            execution_attempt_id: None,
            tool_call_id: None,
            kind: AttentionKind::Budget,
            status: AttentionStatus::Open,
            summary: format!(
                "Billing paused execution for `{route}`: no credential or preflight covers this route. Execution is parked at a safe checkpoint — nothing was cancelled. Next step: add the provider key or switch to the free-tier model in Settings → Models, then coordination resumes with the same agent identities (no duplicate charges)."
            ),
            requested_by: principal,
            assigned_to: vec![owner],
            created_at_unix_ms: now_unix_ms,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        },
    })
}

/// Direct agent-to-agent recipients for a worker handoff: the Principal copy
/// is always kept, and agents on dependent tasks observe the same typed
/// message without a relay. Messaging never grants capabilities: the work
/// contract gate stays authoritative.
pub fn handoff_recipient_ids(
    state: &WorkspaceState,
    task_id: PlanTaskId,
    attempt_id: ExecutionAttemptId,
) -> Vec<ActorId> {
    let principal = ActorRef::principal_agent(state.project().id).id;
    let mut recipients = vec![principal];
    for agent_id in state.direct_handoff_agent_ids(task_id) {
        let agent_actor =
            ActorRef::agent_instance(agent_id, dragent_workspace_model::ActorKind::WorkerAgent).id;
        if !recipients.contains(&agent_actor) {
            recipients.push(agent_actor);
        }
    }
    // Deterministic order keeps command ids stable across replays. The
    // attempt id breaks ties only; it never invents a recipient.
    recipients.sort_by_key(|id| (id.0, attempt_id.0));
    recipients.dedup();
    // Principal stays first for stable snapshots.
    recipients.sort_by_key(|id| (*id != principal, id.0));
    recipients.dedup();
    recipients
}

/// Account-derived billing facts for launch route resolution. Plain data so
/// the resolver stays pure and unit-testable; the view builds it from the
/// account store.
#[derive(Clone, Debug, Default)]
pub struct LaunchBilling {
    /// Server account data is loaded (as opposed to logged-out or still
    /// loading). Platform billing is impossible without it.
    pub account_known: bool,
    /// Server restricts this account to the free-tier model.
    pub locked_to_free: bool,
    /// The account can spend premium funds (subscription balance or
    /// consented purchased credits).
    pub has_premium_funds: bool,
    /// Active server catalog model ids the plan may access.
    pub active_model_ids: Vec<String>,
}

/// Resolve the execution model route and charging mode from the user's
/// billing reality — never by silent fallback:
///
/// - BYOK credential for the requested route → that route on the user's key.
///   No fallback is applied here at all; per-request fallback stays governed
///   by the user's fallback toggle in the request headers.
/// - Free/locked account without premium funds → the platform free-tier
///   route on platform credits (what the server's auto default resolves to).
/// - Paid account, explicit route → that route on platform credits, but only
///   when it is in the active catalog; otherwise an error naming what is
///   available instead of a silent substitution.
/// - Paid account, AUTO/empty route → the free-tier default. The scheduler
///   already prefers each contract's user-overridable model route, so this
///   only fills gaps; per-role pickers remain the real control.
/// - Logged-out cloud use without a credential → error: nobody to bill.
pub fn resolve_execution_route(
    selected_model: &str,
    backend_is_mock: bool,
    billing: Option<&LaunchBilling>,
    free_tier_model_id: &str,
    has_credential: impl Fn(&str) -> bool,
) -> Result<(String, dragent_workspace_model::ChargingMode), String> {
    use dragent_workspace_model::ChargingMode;
    let selected = selected_model.trim();
    if backend_is_mock {
        return Ok((free_tier_model_id.to_string(), ChargingMode::LocalCompute));
    }
    if !selected.is_empty()
        && selected != "auto"
        && (selected.starts_with("ollama/") || selected.starts_with("lmstudio/"))
    {
        return Ok((selected.to_string(), ChargingMode::LocalCompute));
    }
    if !selected.is_empty() && selected != "auto" && has_credential(selected) {
        return Ok((selected.to_string(), ChargingMode::BringYourOwnKey));
    }
    let Some(billing) = billing else {
        return Err(
            "The plan is approved. Sign in or add a provider key before agents can start.".into(),
        );
    };
    if billing.locked_to_free || !billing.has_premium_funds {
        return Ok((
            free_tier_model_id.to_string(),
            ChargingMode::PlatformCredits,
        ));
    }
    if selected.is_empty() || selected == "auto" {
        return Ok((
            free_tier_model_id.to_string(),
            ChargingMode::PlatformCredits,
        ));
    }
    if !billing.active_model_ids.is_empty()
        && !billing.active_model_ids.iter().any(|id| id == selected)
    {
        return Err(format!(
            "The plan is approved. Model '{selected}' is not available on your plan. Available: {}. Pick one in Settings → Models or per role.",
            billing.active_model_ids.join(", ")
        ));
    }
    Ok((selected.to_string(), ChargingMode::PlatformCredits))
}

pub(crate) fn recoverable_checkout_attempt_id(
    state: &WorkspaceState,
    target: &ExecutionAttempt,
) -> Option<ExecutionAttemptId> {
    state
        .execution_attempts()
        .into_iter()
        .filter(|candidate| {
            candidate.task_id == target.task_id
                && candidate.attempt_number <= target.attempt_number
                && (candidate.id == target.id
                    || candidate.review_resolution
                        == Some(
                            dragent_workspace_model::ExecutionReviewResolution::RevisionRequested,
                        ))
        })
        .filter(|candidate| {
            crate::git_status::worktree::agent_worktree_path(state.project().id.0, candidate.id.0)
                .exists()
        })
        .max_by_key(|candidate| candidate.attempt_number)
        .map(|candidate| candidate.id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DispatchState {
    Unclaimed,
    Claimed,
    Terminal,
}

/// Everything a model/tool adapter needs from the canonical execution start.
/// The worktree/project path and generated prompt are intentionally supplied by
/// the caller: neither is durable semantic workspace state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceExecutionLaunch {
    pub project_id: ProjectId,
    pub work_item_id: WorkItemId,
    pub actor_id: ActorId,
    pub device_id: Option<DeviceId>,
    pub correlation_id: CorrelationId,
    pub start_revision: u64,
    pub agent_instance: AgentInstance,
    pub lease: ExecutionLease,
    pub attempt: ExecutionAttempt,
    pub task: PlanTask,
    pub contract: WorkContract,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceExecutionCompletion {
    pub outcome: ExecutionAttemptStatus,
    pub summary: String,
    pub change_summary: Option<ExecutionChangeSummary>,
}

impl WorkspaceExecutionCompletion {
    pub fn succeeded(summary: impl Into<String>) -> Self {
        Self {
            outcome: ExecutionAttemptStatus::Succeeded,
            summary: summary.into(),
            change_summary: None,
        }
    }

    pub fn failed(summary: impl Into<String>) -> Self {
        Self {
            outcome: ExecutionAttemptStatus::Failed,
            summary: summary.into(),
            change_summary: None,
        }
    }

    pub fn cancelled(summary: impl Into<String>) -> Self {
        Self {
            outcome: ExecutionAttemptStatus::Cancelled,
            summary: summary.into(),
            change_summary: None,
        }
    }

    pub fn timed_out(summary: impl Into<String>) -> Self {
        Self {
            outcome: ExecutionAttemptStatus::TimedOut,
            summary: summary.into(),
            change_summary: None,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkspaceExecutionAdapterError {
    #[error("workspace event is not an execution start")]
    NotExecutionStart,
    #[error("invalid canonical execution event: {0}")]
    InvalidEvent(String),
    #[error("execution start references a missing plan task")]
    MissingTask,
    #[error("execution start and work contract do not describe the same assignment")]
    ContractMismatch,
    #[error("execution completion must be terminal")]
    NonTerminalCompletion,
    #[error("execution completion requires a summary")]
    EmptyCompletionSummary,
    #[error("workspace execution requires an absolute project root")]
    InvalidProjectRoot,
    #[error("consent request does not belong to this execution attempt")]
    InvalidConsentRequest,
    #[error("workspace launch model route must be resolved before activation")]
    UnresolvedModelRoute,
    #[error("workspace launch lease duration must be between one millisecond and fifteen minutes")]
    InvalidLeaseDuration,
    #[error("workspace launch charging authorization is invalid: {0}")]
    InvalidChargingAuthorization(String),
}

/// Authority already resolved provider/billing ownership before this policy is
/// constructed. In particular, `auto` is an intent and is never accepted as a
/// durable effective route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceLaunchPolicy {
    pub effective_model_route: String,
    pub charging: ChargingAuthorization,
    pub maximum_parallel_tasks: usize,
    pub lease_duration_ms: i64,
}

impl WorkspaceLaunchPolicy {
    pub fn validate(&self) -> Result<(), WorkspaceExecutionAdapterError> {
        let route = self.effective_model_route.trim();
        if route.is_empty() || route.eq_ignore_ascii_case("auto") {
            return Err(WorkspaceExecutionAdapterError::UnresolvedModelRoute);
        }
        if !(1..=15 * 60 * 1_000).contains(&self.lease_duration_ms) {
            return Err(WorkspaceExecutionAdapterError::InvalidLeaseDuration);
        }
        self.charging.validate().map_err(|error| {
            WorkspaceExecutionAdapterError::InvalidChargingAuthorization(error.to_string())
        })
    }
}

/// One resumable canonical mutation selected from the current projection. The
/// host accepts exactly one action, projects its resulting event, and asks
/// again. This makes a crash between readiness, provisioning, activation, and
/// start recoverable without duplicating runtime identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceLaunchAction {
    MarkTaskReady {
        task_id: PlanTaskId,
    },
    ProvisionAgent {
        task_id: PlanTaskId,
        contract_id: WorkContractId,
        agent_instance_id: AgentInstanceId,
        model_route: String,
    },
    ActivateAgent {
        task_id: PlanTaskId,
        agent_instance_id: AgentInstanceId,
        lease_id: ExecutionLeaseId,
        effective_model_route: String,
        charging: ChargingAuthorization,
        lease_expires_at_unix_ms: i64,
    },
    RenewLease {
        lease_id: ExecutionLeaseId,
        lease_expires_at_unix_ms: i64,
    },
    PrepareRetry {
        failed_attempt_id: ExecutionAttemptId,
        lease_id: ExecutionLeaseId,
        lease_expires_at_unix_ms: i64,
    },
    EscalateFailure {
        attention_item: AttentionItem,
    },
    StartExecution {
        task_id: PlanTaskId,
        agent_instance_id: AgentInstanceId,
        lease_id: ExecutionLeaseId,
        attempt_id: ExecutionAttemptId,
    },
}

impl WorkspaceLaunchAction {
    pub fn into_command_payload(
        self,
        project_id: ProjectId,
        prepared_isolation: Option<ExecutionIsolation>,
    ) -> WorkspaceCommandPayload {
        match self {
            Self::MarkTaskReady { task_id } => WorkspaceCommandPayload::TransitionPlanTask {
                task_id,
                status: PlanTaskStatus::Ready,
            },
            Self::ProvisionAgent {
                contract_id,
                agent_instance_id,
                model_route,
                ..
            } => WorkspaceCommandPayload::ProvisionAgentInstance {
                agent_instance_id,
                contract_id,
                model_route,
            },
            Self::ActivateAgent {
                agent_instance_id,
                lease_id,
                effective_model_route,
                charging,
                lease_expires_at_unix_ms,
                ..
            } => WorkspaceCommandPayload::ActivateAgentInstance {
                agent_instance_id,
                lease_id,
                effective_model_route,
                charging,
                lease_expires_at_unix_ms,
            },
            Self::RenewLease {
                lease_id,
                lease_expires_at_unix_ms,
            } => WorkspaceCommandPayload::RenewExecutionLease {
                lease_id,
                lease_expires_at_unix_ms,
            },
            Self::PrepareRetry {
                failed_attempt_id,
                lease_id,
                lease_expires_at_unix_ms,
            } => WorkspaceCommandPayload::PrepareAgentExecutionRetry {
                failed_attempt_id,
                lease_id,
                lease_expires_at_unix_ms,
            },
            Self::EscalateFailure { attention_item } => {
                WorkspaceCommandPayload::RaiseAttention { attention_item }
            }
            Self::StartExecution {
                task_id,
                agent_instance_id,
                lease_id,
                attempt_id,
            } => WorkspaceCommandPayload::StartAgentExecution {
                attempt_id,
                task_id,
                agent_instance_id,
                lease_id,
                isolation: prepared_isolation
                    .unwrap_or_else(|| ExecutionIsolation::shared(project_id)),
            },
        }
    }
}

/// Select the next scheduler-owned launch mutation. This function has no I/O,
/// starts no provider, and never mutates the projection.
pub fn next_workspace_launch_action(
    state: &WorkspaceState,
    policy: &WorkspaceLaunchPolicy,
    now_unix_ms: i64,
) -> Result<Option<WorkspaceLaunchAction>, WorkspaceExecutionAdapterError> {
    policy.validate()?;
    if !matches!(
        state.active_work().map(|work| work.status),
        Some(WorkStatus::ReadyToExecute | WorkStatus::Executing)
    ) {
        return Ok(None);
    }
    let Some(plan) = state.latest_structured_plan() else {
        return Ok(None);
    };
    let contracts = state.work_contracts();
    let records = state.shaping_records_projection();
    let criteria = records
        .as_ref()
        .map(|records| records.acceptance_criteria.as_slice())
        .unwrap_or_default();
    let attention = state.attention_items();
    let agents = state.agent_instances();
    let attempts = state.execution_attempts();

    // Exhausted automatic failures are not silently abandoned. The Principal
    // receives a stable, durable failure attention item and decides whether to
    // revise the plan, change the contract, or ask the user for authority.
    for task in &plan.tasks {
        if task.status != PlanTaskStatus::Failed {
            continue;
        }
        let Some(attempt) = attempts
            .iter()
            .copied()
            .filter(|attempt| attempt.task_id == task.id)
            .max_by_key(|attempt| attempt.attempt_number)
        else {
            continue;
        };
        if !matches!(
            attempt.status,
            ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut
        ) || attempt.attempt_number < MAX_AUTOMATIC_EXECUTION_ATTEMPTS
        {
            continue;
        }
        // The Principal, not a hardcoded rule, decides what an exhausted
        // retry means — unless it has genuinely gone silent. Defer to
        // whatever already-posted Blocker this attempt's completion raised
        // (`execution_completion_message_kind`/`handoff_recipient_ids`
        // always route one to the Principal) as long as it's still fresh;
        // past the timeout, escalate straight to the human exactly as before
        // so coordination can never go silently stuck forever.
        if let Some((message, _)) =
            crate::agent::principal_coordination::principal_coordination_message_for_attempt(
                state, attempt.id,
            )
        {
            let age_ms = now_unix_ms.saturating_sub(message.created_at_unix_ms);
            if age_ms
                < crate::agent::principal_coordination::PRINCIPAL_COORDINATION_ESCALATION_TIMEOUT_MS
            {
                continue;
            }
        }
        let attention_id = AttentionItemId(uuid::Uuid::new_v5(
            &attempt.id.0,
            b"execution-retry-exhausted:v1",
        ));
        if state
            .attention_items()
            .iter()
            .any(|attention| attention.id == attention_id)
        {
            continue;
        }
        let principal = dragent_workspace_model::ActorRef::principal_agent(state.project().id).id;
        let owner = dragent_workspace_model::ActorRef::local_user(state.project().id, None).id;
        return Ok(Some(WorkspaceLaunchAction::EscalateFailure {
            attention_item: AttentionItem {
                id: attention_id,
                work_item_id: attempt.work_item_id,
                task_id: Some(attempt.task_id),
                // Same thread every message about this task already shares —
                // without this, the escalation is invisible in the unified
                // Conversation scrollback (it only correlates to a thread by
                // this id) and only reachable through the bell popover.
                thread_id: Some(dragent_workspace_model::task_coordination_thread_id(
                    attempt.task_id,
                )),
                execution_attempt_id: Some(attempt.id),
                tool_call_id: None,
                kind: AttentionKind::Failure,
                status: AttentionStatus::Open,
                summary: format!(
                    "`{}` failed after {} automatic attempts. Resolving this gives it one more attempt — fix whatever blocked it first (e.g. re-authenticate, adjust the contract), then resolve.",
                    task.title, attempt.attempt_number
                ),
                requested_by: principal,
                // Both may authorize the extra attempt: the Principal through
                // its coordination turn, or the human directly with Retry —
                // otherwise an exhausted task has no human-reachable way on.
                assigned_to: vec![principal, owner],
                created_at_unix_ms: now_unix_ms,
                resolved_at_unix_ms: None,
                resolution_kind: None,
                resolution: None,
            },
        }));
    }

    // Failed and timed-out tasks are deliberately re-armed through authority
    // before the ordinary scheduler can launch them again. Selecting by plan
    // order and latest attempt number keeps recovery deterministic across
    // process restarts. Pause and cancellation are intentionally excluded.
    for task in &plan.tasks {
        if task.status != PlanTaskStatus::Failed {
            continue;
        }
        let Some(attempt) = attempts
            .iter()
            .copied()
            .filter(|attempt| attempt.task_id == task.id)
            .max_by_key(|attempt| attempt.attempt_number)
        else {
            continue;
        };
        if !matches!(
            attempt.status,
            ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut
        ) {
            continue;
        }
        if attempt.attempt_number >= MAX_AUTOMATIC_EXECUTION_ATTEMPTS {
            // Exhausted, but the human explicitly resolved (not merely
            // dismissed) the escalation this exact attempt raised — that
            // resolution is the human's "go ahead, try again," mediated
            // through the same conversation every other coordination
            // decision already goes through, not a separate bypass. One
            // resolution buys exactly one more attempt: a fresh exhaustion
            // raises, and needs, its own new resolution.
            let retry_authorized = attention.iter().any(|item| {
                item.execution_attempt_id == Some(attempt.id)
                    && item.kind == AttentionKind::Failure
                    && item.status == AttentionStatus::Resolved
                    && item.resolution_kind != Some(AttentionResolution::Dismissed)
            });
            if !retry_authorized {
                continue;
            }
        }
        let Some(finished_at) = attempt.finished_at_unix_ms else {
            continue;
        };
        if now_unix_ms
            < finished_at.saturating_add(execution_retry_delay_ms(attempt.attempt_number))
        {
            continue;
        }
        let Some(contract_id) = task.assigned_contract_id else {
            continue;
        };
        let Some(contract) = contracts
            .iter()
            .copied()
            .find(|item| item.id == contract_id && item.status == WorkContractStatus::Active)
        else {
            continue;
        };
        let Some(agent_id) = contract.agent_instance_id else {
            continue;
        };
        let Some(agent) = agents.iter().copied().find(|item| {
            item.id == agent_id
                && item.status == AgentInstanceStatus::Failed
                && item.active_lease_id.is_none()
        }) else {
            continue;
        };
        return Ok(Some(WorkspaceLaunchAction::PrepareRetry {
            failed_attempt_id: attempt.id,
            lease_id: stable_retry_lease_id(agent.id, attempt.attempt_number + 1),
            lease_expires_at_unix_ms: now_unix_ms + policy.lease_duration_ms,
        }));
    }

    // Running attempts must renew their lease before it expires. The previous
    // code only renewed `Ready` agents waiting to start, so any task running
    // longer than one lease window (10 min) always timed out even while the
    // provider was still making progress.
    {
        let mut running = attempts
            .iter()
            .copied()
            .filter(|attempt| attempt.status == ExecutionAttemptStatus::Running)
            .collect::<Vec<_>>();
        running.sort_by_key(|attempt| (attempt.started_at_unix_ms, attempt.id.0));
        for attempt in running {
            let Some(lease) = state.execution_lease(attempt.lease_id) else {
                continue;
            };
            if lease.status != dragent_workspace_model::ExecutionLeaseStatus::Active {
                continue;
            }
            if lease.expires_at_unix_ms.saturating_sub(now_unix_ms) <= LEASE_RENEWAL_LEAD_MS {
                return Ok(Some(WorkspaceLaunchAction::RenewLease {
                    lease_id: lease.id,
                    lease_expires_at_unix_ms: now_unix_ms + policy.lease_duration_ms,
                }));
            }
        }
    }

    // Watchdog runs before the scheduler projection: a stalled projection
    // has nothing launchable by definition, so this never shadows real work.
    // It guarantees coordination can never look idle without an explicit,
    // actionable attention item.
    if let Some(stall) = next_stall_attention(state, policy.maximum_parallel_tasks, now_unix_ms) {
        return Ok(Some(stall));
    }

    let projection = evaluate_scheduler(
        plan,
        &contracts
            .iter()
            .map(|contract| (*contract).clone())
            .collect::<Vec<_>>(),
        criteria,
        &[],
        &attention
            .iter()
            .map(|item| (*item).clone())
            .collect::<Vec<_>>(),
        policy.maximum_parallel_tasks,
    );
    for task_id in projection.launchable_task_ids {
        let Some(task) = plan.tasks.iter().find(|task| task.id == task_id) else {
            continue;
        };
        if task.status != PlanTaskStatus::Ready {
            return Ok(Some(WorkspaceLaunchAction::MarkTaskReady { task_id }));
        }
        let Some(contract_id) = task.assigned_contract_id else {
            continue;
        };
        let Some(contract) = contracts
            .iter()
            .copied()
            .find(|item| item.id == contract_id)
        else {
            continue;
        };
        // A writable contract with no tools can never make progress: the
        // worker prompt renders `Allowed tools: (none)` and the model
        // correctly refuses to edit. Surface this as durable Attention for
        // the Principal instead of launching an agent that immediately
        // reports "no file-editing tool is available" as a false success.
        if !contract.write_scope.is_empty() && contract.tools.is_empty() {
            let attention_id = AttentionItemId(uuid::Uuid::new_v5(
                &contract.id.0,
                b"execution-contract-missing-tools:v1",
            ));
            if !state
                .attention_items()
                .iter()
                .any(|attention| attention.id == attention_id)
            {
                let principal =
                    dragent_workspace_model::ActorRef::principal_agent(state.project().id).id;
                let owner =
                    dragent_workspace_model::ActorRef::local_user(state.project().id, None).id;
                return Ok(Some(WorkspaceLaunchAction::EscalateFailure {
                    attention_item: AttentionItem {
                        id: attention_id,
                        work_item_id: contract.work_item_id,
                        task_id: Some(task.id),
                        thread_id: None,
                        execution_attempt_id: None,
                        tool_call_id: None,
                        kind: AttentionKind::Failure,
                        status: AttentionStatus::Open,
                        summary: format!(
                            "`{}` has a write scope but its work contract grants no tools. Next step: add file-editing tools to the contract in Shape, or narrow the scope, then coordination retries automatically.",
                            task.title
                        ),
                        requested_by: principal,
                        assigned_to: vec![owner],
                        created_at_unix_ms: now_unix_ms,
                        resolved_at_unix_ms: None,
                        resolution_kind: None,
                        resolution: None,
                    },
                }));
            }
            continue;
        }
        let contract_model_route = contract
            .preferred_model_route
            .as_deref()
            .unwrap_or(&policy.effective_model_route)
            .to_string();
        let Some(agent_instance_id) = contract.agent_instance_id else {
            return Ok(Some(WorkspaceLaunchAction::ProvisionAgent {
                task_id,
                contract_id,
                agent_instance_id: stable_agent_instance_id(contract_id),
                model_route: contract_model_route,
            }));
        };
        let Some(agent) = agents
            .iter()
            .copied()
            .find(|agent| agent.id == agent_instance_id)
        else {
            continue;
        };
        if agent.status == AgentInstanceStatus::Pending {
            return Ok(Some(WorkspaceLaunchAction::ActivateAgent {
                task_id,
                agent_instance_id,
                lease_id: stable_initial_lease_id(agent_instance_id),
                effective_model_route: contract_model_route,
                charging: policy.charging.clone(),
                lease_expires_at_unix_ms: now_unix_ms + policy.lease_duration_ms,
            }));
        }
        if agent.status == AgentInstanceStatus::Ready
            && contract.status == WorkContractStatus::Active
        {
            let Some(lease_id) = agent.active_lease_id else {
                continue;
            };
            let Some(lease) = state.execution_lease(lease_id) else {
                continue;
            };
            if lease.status != dragent_workspace_model::ExecutionLeaseStatus::Active {
                continue;
            }
            if lease.expires_at_unix_ms.saturating_sub(now_unix_ms) <= LEASE_RENEWAL_LEAD_MS {
                return Ok(Some(WorkspaceLaunchAction::RenewLease {
                    lease_id,
                    lease_expires_at_unix_ms: now_unix_ms + policy.lease_duration_ms,
                }));
            }
            let attempt_number = attempts
                .iter()
                .filter(|attempt| attempt.task_id == task_id)
                .map(|attempt| attempt.attempt_number)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            return Ok(Some(WorkspaceLaunchAction::StartExecution {
                task_id,
                agent_instance_id,
                lease_id,
                attempt_id: stable_attempt_id(task_id, attempt_number),
            }));
        }
    }
    Ok(None)
}

/// Convert the first canonically-running attempt whose lease has elapsed into
/// a revision-guarded timeout command. Lease expiry is authority state, not a
/// process-local timer: the accepted finish event is what releases ownership
/// and allows the deterministic retry policy to advance after a restart.
pub fn next_expired_execution_command(
    state: &WorkspaceState,
    actor_id: ActorId,
    device_id: Option<DeviceId>,
    current_revision: u64,
    now_unix_ms: i64,
) -> Option<WorkspaceCommandEnvelope> {
    let work_item_id = state.active_work()?.id;
    let mut attempts = state
        .execution_attempts()
        .into_iter()
        .filter(|attempt| attempt.status == ExecutionAttemptStatus::Running)
        .collect::<Vec<_>>();
    attempts.sort_by_key(|attempt| (attempt.started_at_unix_ms, attempt.id.0));
    let attempt = attempts.into_iter().find(|attempt| {
        state
            .execution_lease(attempt.lease_id)
            .is_some_and(|lease| {
                lease.status == dragent_workspace_model::ExecutionLeaseStatus::Active
                    && lease.charging.owner_id == actor_id
                    && lease.expires_at_unix_ms <= now_unix_ms
            })
    })?;
    Some(WorkspaceCommandEnvelope {
        command_id: WorkspaceCommandId::new(),
        schema_version: WORKSPACE_PROTOCOL_VERSION,
        project_id: state.project().id,
        work_item_id,
        actor_id,
        device_id,
        correlation_id: CorrelationId::new(),
        timestamp_unix_ms: now_unix_ms,
        expected_revision: Some(current_revision),
        payload: WorkspaceCommandPayload::FinishAgentExecution {
            attempt_id: attempt.id,
            outcome: ExecutionAttemptStatus::TimedOut,
            summary: format!(
                "Execution lease expired before attempt {} completed. You don't need to act: the scheduler retries automatically (up to {} attempts); if retries are exhausted the Principal raises an Attention request for review.",
                attempt.attempt_number, MAX_AUTOMATIC_EXECUTION_ATTEMPTS,
            ),
            change_summary: None,
        },
    })
}

/// Return the nearest deterministic retry deadline that has not elapsed yet.
/// The UI host uses this only to wake the authority planner; it grants no
/// execution permission and performs no mutation itself.
pub fn next_workspace_retry_wait_ms(state: &WorkspaceState, now_unix_ms: i64) -> Option<u64> {
    if state.active_work().map(|work| work.status) != Some(WorkStatus::Executing) {
        return None;
    }
    let plan = state.latest_structured_plan()?;
    let attempts = state.execution_attempts();
    plan.tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Failed)
        .filter_map(|task| {
            attempts
                .iter()
                .copied()
                .filter(|attempt| attempt.task_id == task.id)
                .max_by_key(|attempt| attempt.attempt_number)
        })
        .filter(|attempt| {
            matches!(
                attempt.status,
                ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut
            ) && attempt.attempt_number < MAX_AUTOMATIC_EXECUTION_ATTEMPTS
        })
        .filter_map(|attempt| {
            let deadline = attempt
                .finished_at_unix_ms?
                .saturating_add(execution_retry_delay_ms(attempt.attempt_number));
            (deadline > now_unix_ms).then_some(deadline.saturating_sub(now_unix_ms) as u64)
        })
        .min()
}
