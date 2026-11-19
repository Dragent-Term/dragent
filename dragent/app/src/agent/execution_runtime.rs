//! Workspace execution runtime: attempts, leases, recovery.
//!
//! Moved verbatim; same behavior.

//! Durable workspace-to-agent execution boundary.
//!
//! The workspace authority commits an `AgentExecutionStarted` event before a
//! provider or tool loop is launched. This module turns that committed fact
//! into an immutable launch request and turns the runner's terminal result
//! back into a revision-guarded workspace command. It deliberately owns no UI
//! state and never starts work while canonical history is merely being replayed.

use super::execution_launch::*;
use super::execution_pipeline::*;
use super::execution_review::*;
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

#[derive(Default)]
pub struct WorkspaceExecutionRuntime {
    pub(crate) adapter: WorkspaceExecutionAdapter,
    pub(crate) sessions: HashMap<ExecutionAttemptId, WorkspacePipelineSession>,
    pub(crate) activity: HashMap<ExecutionAttemptId, WorkspaceExecutionActivityLog>,
    pub(crate) prepared_execution_roots: HashMap<ExecutionAttemptId, PreparedExecutionRoot>,
    pub(crate) revision_execution_roots: HashMap<PlanTaskId, PreparedExecutionRoot>,
    pub(crate) checkpoint_archive: HashMap<ExecutionAttemptId, ArchivedWorkspaceCheckpoint>,
    pub(crate) replayed_attempts: HashSet<ExecutionAttemptId>,
    /// Attempts restart recovery re-mounted for the host to continue on an
    /// `AgentController` (`take_recovered_host_attempts`).
    pub(crate) recovered_host_attempts: Vec<RecoveredHostAttempt>,
    /// Interrupted attempts run by an agent CLI in its own terminal
    /// (`take_recovered_cli_attempts`). Terminals outlive the app, so the
    /// agent is usually still at work.
    pub(crate) recovered_cli_attempts: Vec<ExecutionAttemptId>,
}

/// An interrupted attempt that recovery proved safe to continue: the host
/// rebuilds its controller from the persisted transcript, then resumes the
/// run from its checkpoint (`AgentController::recover_local_run`).
#[derive(Clone, Debug)]
pub struct RecoveredHostAttempt {
    pub attempt_id: ExecutionAttemptId,
    pub runtime_task_id: String,
    /// Tool calls still waiting on consent when the app stopped, each with
    /// the summary of its open canonical Permission attention item.
    pub parked: Vec<WorkspaceToolRequest>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedExecutionRoot {
    pub(crate) path: PathBuf,
    pub(crate) isolation: ExecutionIsolation,
    pub(crate) owns_checkout: bool,
}

/// Resolve the exact unresolved writable attempt named by a review task. The
/// review target is part of the approved plan, so this never guesses from
/// titles, role names, or whichever attempt happened to finish last globally.
pub fn review_candidate_attempt(
    state: &WorkspaceState,
    review_task_id: PlanTaskId,
) -> Option<ExecutionAttempt> {
    let target_task_id = state
        .latest_structured_plan()?
        .tasks
        .iter()
        .find(|task| task.id == review_task_id)?
        .review_target_task_id?;
    state
        .execution_attempts()
        .into_iter()
        .filter(|attempt| {
            attempt.task_id == target_task_id
                && attempt.status == ExecutionAttemptStatus::Succeeded
                && attempt.review_resolution.is_none()
                && attempt.change_summary.is_some()
        })
        .max_by_key(|attempt| attempt.attempt_number)
        .cloned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceResolvedToolAction {
    Execute {
        attempt_id: ExecutionAttemptId,
        request: WorkspaceToolRequest,
    },
    Reject {
        attempt_id: ExecutionAttemptId,
        request: WorkspaceToolRequest,
        reason: String,
    },
}

#[derive(Clone)]
pub struct WorkspaceToolResultDelivery {
    pub(crate) request: WorkspaceToolRequest,
    pub(crate) sender: mpsc::Sender<
        Result<crate::agent::pipeline::PipelineEvent, crate::agent::pipeline::StreamError>,
    >,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCheckpointSummary {
    pub attempt_id: ExecutionAttemptId,
    pub task_title: String,
    pub changed_files: usize,
    pub added_lines: u32,
    pub deleted_lines: u32,
}

pub(crate) struct ArchivedWorkspaceCheckpoint {
    pub(crate) task_title: String,
    pub(crate) store: crate::agent::checkpoint_store::CheckpointStore,
}

impl WorkspaceToolResultDelivery {
    pub fn deliver(
        self,
        output: &str,
        is_error: bool,
    ) -> Result<(), crate::agent::pipeline::StreamError> {
        crate::agent::pipeline::deliver_tool_result_with_recovery(
            &self.request.runtime_task_id,
            &self.request.call_id,
            output,
            is_error,
            self.sender,
        )
    }
}

impl WorkspaceExecutionRuntime {
    pub fn restore_history<'a>(
        &mut self,
        events: impl IntoIterator<Item = &'a WorkspaceEventEnvelope>,
    ) {
        self.sessions.clear();
        self.activity.clear();
        self.prepared_execution_roots.clear();
        self.revision_execution_roots.clear();
        self.checkpoint_archive.clear();
        self.replayed_attempts.clear();
        let events = events.into_iter().collect::<Vec<_>>();
        for event in &events {
            let WorkspaceEventPayload::AgentExecutionStarted { attempt, plan, .. } = &event.payload
            else {
                continue;
            };
            let mut store = crate::agent::checkpoint_store::CheckpointStore::default();
            store.bind(attempt.id.0, event.work_item_id.0);
            let task_title = plan
                .tasks
                .iter()
                .find(|task| task.id == attempt.task_id)
                .map(|task| task.title.clone())
                .unwrap_or_else(|| "Workspace task".into());
            self.checkpoint_archive.insert(
                attempt.id,
                ArchivedWorkspaceCheckpoint { task_title, store },
            );
        }
        self.replayed_attempts
            .extend(events.iter().filter_map(|event| match &event.payload {
                WorkspaceEventPayload::AgentExecutionStarted { attempt, .. }
                | WorkspaceEventPayload::AgentExecutionFinished { attempt, .. }
                | WorkspaceEventPayload::AgentExecutionReviewResolved { attempt, .. } => {
                    Some(attempt.id)
                }
                _ => None,
            }));
        self.adapter.restore_history(events);
    }

    /// Reconstruct a stopped local session only at a provably safe tool-wait
    /// boundary. Every unresolved provider call must correspond to an open,
    /// canonical Permission attention item; otherwise recovery remains inert.
    pub fn recover_interrupted_local_sessions<'a>(
        &mut self,
        events: impl IntoIterator<Item = &'a WorkspaceEventEnvelope>,
        state: &WorkspaceState,
        project_root: &Path,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        for attempt in state.execution_attempts().into_iter().filter(|attempt| {
            attempt.status == ExecutionAttemptStatus::Succeeded
                && attempt.change_summary.is_some()
                && attempt.review_resolution.is_none()
        }) {
            let Some(expected) = attempt.isolation.as_ref() else {
                continue;
            };
            if expected.kind != dragent_workspace_model::ExecutionIsolationKind::GitWorktree {
                continue;
            }
            let Some(checkout_attempt_id) = recoverable_checkout_attempt_id(state, attempt) else {
                continue;
            };
            match crate::git_status::worktree::prepare_agent_worktree(
                project_root,
                state.project().id.0,
                checkout_attempt_id.0,
            ) {
                Ok(worktree) => {
                    let actual =
                        ExecutionIsolation::git_worktree(attempt.id, worktree.base_head.clone());
                    if &actual == expected {
                        self.prepared_execution_roots.insert(
                            attempt.id,
                            PreparedExecutionRoot {
                                path: worktree.path,
                                isolation: actual,
                                owns_checkout: true,
                            },
                        );
                    } else {
                        warnings.push(format!(
                            "Review checkout for {:?} no longer matches its canonical isolation",
                            attempt.id
                        ));
                    }
                }
                Err(error) => warnings.push(format!(
                    "Review checkout for {:?} could not be restored: {error}",
                    attempt.id
                )),
            }
        }
        let mut latest_attempts = HashMap::<PlanTaskId, &ExecutionAttempt>::new();
        for attempt in state.execution_attempts() {
            latest_attempts
                .entry(attempt.task_id)
                .and_modify(|current| {
                    if attempt.attempt_number > current.attempt_number {
                        *current = attempt;
                    }
                })
                .or_insert(attempt);
        }
        for attempt in latest_attempts.into_values().filter(|attempt| {
            attempt.review_resolution
                == Some(dragent_workspace_model::ExecutionReviewResolution::RevisionRequested)
        }) {
            let Some(expected) = attempt.isolation.as_ref() else {
                continue;
            };
            if expected.kind != dragent_workspace_model::ExecutionIsolationKind::GitWorktree {
                continue;
            }
            let Some(checkout_attempt_id) = recoverable_checkout_attempt_id(state, attempt) else {
                warnings.push(format!(
                    "Revision checkout for {:?} is unavailable; the next attempt will restart from the current project baseline",
                    attempt.id
                ));
                continue;
            };
            match crate::git_status::worktree::prepare_agent_worktree(
                project_root,
                state.project().id.0,
                checkout_attempt_id.0,
            ) {
                Ok(worktree) => {
                    let actual =
                        ExecutionIsolation::git_worktree(attempt.id, worktree.base_head.clone());
                    if &actual == expected {
                        self.revision_execution_roots.insert(
                            attempt.task_id,
                            PreparedExecutionRoot {
                                path: worktree.path,
                                isolation: actual,
                                owns_checkout: true,
                            },
                        );
                    } else {
                        warnings.push(format!(
                            "Revision checkout for {:?} no longer matches its canonical isolation",
                            attempt.id
                        ));
                    }
                }
                Err(error) => warnings.push(format!(
                    "Revision checkout for {:?} could not be restored: {error}",
                    attempt.id
                )),
            }
        }
        warnings.extend(self.recover_interrupted_local_sessions_with(
            events,
            state,
            project_root,
            chrono::Utc::now().timestamp_millis(),
            |run_id| crate::agent::pipeline::inspect_local_run_recovery(run_id),
        ));
        warnings
    }

    pub(crate) fn recover_interrupted_local_sessions_with<'a>(
        &mut self,
        events: impl IntoIterator<Item = &'a WorkspaceEventEnvelope>,
        state: &WorkspaceState,
        project_root: &Path,
        now_unix_ms: i64,
        mut recover_pending: impl FnMut(
            &str,
        )
            -> Result<crate::agent::pipeline::LocalRunRecoveryState, String>,
    ) -> Vec<String> {
        let events = events.into_iter().collect::<Vec<_>>();
        let terminal = events
            .iter()
            .filter_map(|event| match &event.payload {
                WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } => Some(attempt.id),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let starts = events
            .iter()
            .filter_map(|event| match &event.payload {
                WorkspaceEventPayload::AgentExecutionStarted { attempt, .. }
                    if !terminal.contains(&attempt.id) =>
                {
                    Some((attempt.id, *event))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        let mut attention_by_attempt: HashMap<ExecutionAttemptId, Vec<&AttentionItem>> =
            HashMap::new();
        for attention in state.attention_items().into_iter().filter(|attention| {
            attention.kind == AttentionKind::Permission
                && matches!(
                    attention.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                )
        }) {
            if let Some(attempt_id) = attention.execution_attempt_id {
                attention_by_attempt
                    .entry(attempt_id)
                    .or_default()
                    .push(attention);
            }
        }

        let mut warnings = Vec::new();
        for (attempt_id, start) in starts {
            let Some(attempt) = state.execution_attempt(attempt_id) else {
                warnings.push(format!(
                    "Execution recovery skipped for {attempt_id:?}: canonical attempt is unavailable"
                ));
                continue;
            };
            let Some(lease) = state.execution_lease(attempt.lease_id) else {
                warnings.push(format!(
                    "Execution recovery skipped for {attempt_id:?}: execution lease is unavailable"
                ));
                continue;
            };
            if lease.status != dragent_workspace_model::ExecutionLeaseStatus::Active
                || lease.expires_at_unix_ms <= now_unix_ms
            {
                warnings.push(format!(
                    "Execution recovery skipped for {attempt_id:?}: execution lease is no longer active"
                ));
                continue;
            }
            let runtime_task_id = workspace_run_id(attempt_id);
            // An agent CLI keeps no provider checkpoint here: it works in its
            // own terminal, which outlives the app. Re-adopt it as it is.
            let cli_agent = dragent_workspace_model::route_agent(&attempt.model_route).is_some()
                && !attempt.isolation.as_ref().is_some_and(|isolation| {
                    isolation.source_attempt_id.is_some()
                        || isolation.kind
                            == dragent_workspace_model::ExecutionIsolationKind::ReviewCandidate
                });
            let recovery = if cli_agent {
                crate::agent::pipeline::LocalRunRecoveryState::ResumeProvider
            } else {
                match recover_pending(&runtime_task_id) {
                    Ok(recovery) => recovery,
                    Err(error) => {
                        warnings.push(format!(
                            "Execution recovery skipped for {attempt_id:?}: {error}"
                        ));
                        continue;
                    }
                }
            };
            let attention_items = attention_by_attempt.remove(&attempt_id).unwrap_or_default();
            let pending = match recovery {
                crate::agent::pipeline::LocalRunRecoveryState::ResumeProvider => Vec::new(),
                crate::agent::pipeline::LocalRunRecoveryState::AwaitingTools(pending) => pending,
            };
            let attention_ids = attention_items
                .iter()
                .filter_map(|attention| attention.tool_call_id.clone())
                .collect::<HashSet<_>>();
            let pending_ids = pending
                .iter()
                .map(|call| call.call_id.clone())
                .collect::<HashSet<_>>();
            if pending_ids != attention_ids {
                warnings.push(format!(
                    "Execution recovery skipped for {attempt_id:?}: unresolved local calls do not exactly match canonical Attention"
                ));
                continue;
            }
            let contract_id = match &start.payload {
                WorkspaceEventPayload::AgentExecutionStarted { attempt, .. } => attempt.contract_id,
                _ => unreachable!(),
            };
            let Some(contract) = state.work_contract(contract_id) else {
                warnings.push(format!(
                    "Execution recovery skipped for {attempt_id:?}: Work Contract is unavailable"
                ));
                continue;
            };
            let execution_root = if contract.write_scope.is_empty() {
                match attempt.isolation.as_ref() {
                    Some(isolation)
                        if isolation.kind
                            == dragent_workspace_model::ExecutionIsolationKind::ReviewCandidate =>
                    {
                        let Some(source_attempt_id) = isolation.source_attempt_id else {
                            warnings.push(format!(
                                "Execution recovery skipped for {attempt_id:?}: review source is unavailable"
                            ));
                            continue;
                        };
                        let Some(root) = self
                            .prepared_execution_roots
                            .get(&source_attempt_id)
                            .map(|prepared| prepared.path.clone())
                        else {
                            warnings.push(format!(
                                "Execution recovery skipped for {attempt_id:?}: candidate checkout is unavailable"
                            ));
                            continue;
                        };
                        root
                    }
                    _ => project_root.to_path_buf(),
                }
            } else {
                let checkout_attempt_id =
                    recoverable_checkout_attempt_id(state, attempt).unwrap_or(attempt_id);
                match crate::git_status::worktree::prepare_agent_worktree(
                    project_root,
                    start.project_id.0,
                    checkout_attempt_id.0,
                ) {
                    Ok(worktree) => {
                        let Some(expected) = attempt.isolation.as_ref() else {
                            warnings.push(format!(
                                "Execution recovery skipped for {attempt_id:?}: canonical isolation is unavailable"
                            ));
                            continue;
                        };
                        let actual = ExecutionIsolation::git_worktree(
                            attempt_id,
                            worktree.base_head.clone(),
                        );
                        if expected != &actual {
                            warnings.push(format!(
                                "Execution recovery skipped for {attempt_id:?}: isolated checkout identity or base revision changed"
                            ));
                            continue;
                        }
                        self.prepared_execution_roots.insert(
                            attempt_id,
                            PreparedExecutionRoot {
                                path: worktree.path.clone(),
                                isolation: actual,
                                owns_checkout: true,
                            },
                        );
                        worktree.path
                    }
                    Err(error) => {
                        warnings.push(format!(
                            "Execution recovery skipped for {attempt_id:?}: isolated checkout is unavailable: {error}"
                        ));
                        continue;
                    }
                }
            };
            let mut recovery_adapter = WorkspaceExecutionAdapter::default();
            let launch = match recovery_adapter.claim_started(start, contract) {
                Ok(Some(launch)) => launch,
                Ok(None) => continue,
                Err(error) => {
                    warnings.push(format!(
                        "Execution recovery skipped for {attempt_id:?}: {error}"
                    ));
                    continue;
                }
            };
            let communication = task_communication_context(state, attempt.task_id);
            let request =
                match WorkspacePipelineRequest::new(launch, &execution_root, "", &communication) {
                    Ok(request) => request,
                    Err(error) => {
                        warnings.push(format!(
                            "Execution recovery skipped for {attempt_id:?}: {error}"
                        ));
                        continue;
                    }
                };
            let parked = pending
                .into_iter()
                .map(|call| {
                    let reason = attention_items
                        .iter()
                        .find(|attention| {
                            attention.tool_call_id.as_deref() == Some(call.call_id.as_str())
                        })
                        .map(|attention| attention.summary.clone())
                        .unwrap_or_default();
                    WorkspaceToolRequest {
                        runtime_task_id: runtime_task_id.clone(),
                        call_id: call.call_id,
                        tool_name: call.tool_name,
                        args_json: call.args_json,
                        reason,
                    }
                })
                .collect();
            // Mounted detached: the host's controller drives the provider, or
            // for an agent CLI, the agent in its terminal does.
            let mut session = request.mount_detached();
            if cli_agent {
                self.recovered_cli_attempts.push(attempt_id);
            } else {
                session.runtime_task_id = Some(runtime_task_id.clone());
                session.bind_checkpoints();
                self.recovered_host_attempts.push(RecoveredHostAttempt {
                    attempt_id,
                    runtime_task_id,
                    parked,
                });
            }
            if contract.write_scope.is_empty() {
                let isolation = attempt
                    .isolation
                    .clone()
                    .unwrap_or_else(|| ExecutionIsolation::shared(start.project_id));
                self.prepared_execution_roots.insert(
                    attempt_id,
                    PreparedExecutionRoot {
                        path: execution_root.clone(),
                        isolation,
                        owns_checkout: false,
                    },
                );
            }
            self.sessions.insert(attempt_id, session);
        }
        for attempt_id in attention_by_attempt.into_keys() {
            warnings.push(format!(
                "Execution recovery skipped for {attempt_id:?}: execution start is unavailable"
            ));
        }
        warnings
    }

    /// Attempts `recover_interrupted_local_sessions` re-mounted, for the host
    /// to continue on their controllers. Drained on first call.
    pub fn take_recovered_host_attempts(&mut self) -> Vec<RecoveredHostAttempt> {
        std::mem::take(&mut self.recovered_host_attempts)
    }

    /// Attempts on agent CLIs that recovery re-mounted, for the host to
    /// re-adopt their agent's terminal. Drained on first call.
    pub fn take_recovered_cli_attempts(&mut self) -> Vec<ExecutionAttemptId> {
        std::mem::take(&mut self.recovered_cli_attempts)
    }

    pub fn mount_committed_start(
        &mut self,
        event: &WorkspaceEventEnvelope,
        contract: &WorkContract,
        project_root: impl Into<PathBuf>,
        trusted_project_context: &str,
        task_communication_context: &str,
    ) -> Result<bool, WorkspaceExecutionAdapterError> {
        let Some(request) = self.prepare_committed_start(
            event,
            contract,
            project_root.into(),
            trusted_project_context,
            task_communication_context,
        )?
        else {
            return Ok(false);
        };
        let attempt_id = request.launch.attempt.id;
        let mut session = request.spawn();
        session.bind_checkpoints();
        self.sessions.insert(attempt_id, session);
        Ok(true)
    }

    /// `mount_committed_start` for a host that runs the attempt on an
    /// `AgentController`: registers the attempt (identity, checkpoints, lease
    /// bookkeeping) and hands back the prepared prompts/root/contract for the
    /// controller to start. `None` when the start was already claimed.
    pub fn mount_committed_start_for_host(
        &mut self,
        event: &WorkspaceEventEnvelope,
        contract: &WorkContract,
        project_root: impl Into<PathBuf>,
        trusted_project_context: &str,
        task_communication_context: &str,
    ) -> Result<Option<WorkspacePipelineRequest>, WorkspaceExecutionAdapterError> {
        let Some(request) = self.prepare_committed_start(
            event,
            contract,
            project_root.into(),
            trusted_project_context,
            task_communication_context,
        )?
        else {
            return Ok(None);
        };
        let attempt_id = request.launch.attempt.id;
        let mut session = request.clone().mount_detached();
        session.bind_checkpoints();
        self.sessions.insert(attempt_id, session);
        Ok(Some(request))
    }

    /// Identity of a mounted attempt (host-run or session-run).
    pub fn mounted_launch(
        &self,
        attempt_id: ExecutionAttemptId,
    ) -> Option<&WorkspaceExecutionLaunch> {
        self.sessions
            .get(&attempt_id)
            .map(|session| &session.request.launch)
    }

    /// The prepared request (prompts, root, contract) of a mounted attempt —
    /// what a host needs to continue that agent's turn with its own context.
    pub fn mounted_request(
        &self,
        attempt_id: ExecutionAttemptId,
    ) -> Option<&WorkspacePipelineRequest> {
        self.sessions
            .get(&attempt_id)
            .map(|session| &session.request)
    }

    pub(crate) fn prepare_committed_start(
        &mut self,
        event: &WorkspaceEventEnvelope,
        contract: &WorkContract,
        project_root: PathBuf,
        trusted_project_context: &str,
        task_communication_context: &str,
    ) -> Result<Option<WorkspacePipelineRequest>, WorkspaceExecutionAdapterError> {
        if matches!(
            &event.payload,
            WorkspaceEventPayload::AgentExecutionStarted { attempt, .. }
                if self.replayed_attempts.contains(&attempt.id)
        ) {
            return Ok(None);
        }
        let Some(launch) = self.adapter.claim_started(event, contract)? else {
            return Ok(None);
        };
        let attempt_id = launch.attempt.id;
        let request = (|| {
            let execution_root = if contract.write_scope.is_empty() {
                self.prepared_execution_roots
                    .get(&launch.attempt.id)
                    .filter(|prepared| {
                        prepared.isolation.kind
                            == dragent_workspace_model::ExecutionIsolationKind::ReviewCandidate
                    })
                    .map(|prepared| prepared.path.clone())
                    .unwrap_or(project_root)
            } else {
                self.prepared_execution_roots
                    .get(&launch.attempt.id)
                    .map(|prepared| prepared.path.clone())
                    .ok_or_else(|| {
                        WorkspaceExecutionAdapterError::InvalidEvent(
                            "writable execution start has no prepared isolated checkout".into(),
                        )
                    })?
            };
            WorkspacePipelineRequest::new(
                launch,
                execution_root,
                trusted_project_context,
                task_communication_context,
            )
        })();
        match request {
            Ok(request) => Ok(Some(request)),
            Err(error) => {
                self.adapter
                    .attempts
                    .insert(attempt_id, DispatchState::Unclaimed);
                Err(error)
            }
        }
    }

    pub fn prepared_execution_root(&self, attempt_id: ExecutionAttemptId) -> Option<&Path> {
        self.prepared_execution_roots
            .get(&attempt_id)
            .map(|prepared| prepared.path.as_path())
    }

    pub fn prepared_execution_isolation(
        &self,
        attempt_id: ExecutionAttemptId,
    ) -> Option<&ExecutionIsolation> {
        self.prepared_execution_roots
            .get(&attempt_id)
            .map(|prepared| &prepared.isolation)
    }

    pub fn change_capture_request(
        &self,
        attempt_id: ExecutionAttemptId,
    ) -> Option<(PathBuf, ExecutionIsolation)> {
        let prepared = self.prepared_execution_roots.get(&attempt_id)?;
        (prepared.isolation.kind == dragent_workspace_model::ExecutionIsolationKind::GitWorktree)
            .then(|| (prepared.path.clone(), prepared.isolation.clone()))
    }

    pub fn record_prepared_execution_root(
        &mut self,
        attempt_id: ExecutionAttemptId,
        root: PathBuf,
        base_revision: String,
    ) {
        self.prepared_execution_roots.insert(
            attempt_id,
            PreparedExecutionRoot {
                path: root,
                isolation: ExecutionIsolation::git_worktree(attempt_id, base_revision),
                owns_checkout: true,
            },
        );
    }

    pub fn record_review_candidate_root(
        &mut self,
        attempt_id: ExecutionAttemptId,
        source_attempt_id: ExecutionAttemptId,
        root: PathBuf,
        base_revision: String,
    ) {
        self.prepared_execution_roots.insert(
            attempt_id,
            PreparedExecutionRoot {
                path: root,
                isolation: ExecutionIsolation::review_candidate(
                    attempt_id,
                    source_attempt_id,
                    base_revision,
                ),
                owns_checkout: false,
            },
        );
    }

    /// Preserve a reviewed checkout as the starting filesystem for the next
    /// attempt of the same task. The old attempt remains immutable in
    /// canonical history; only process-local ownership of the checkout moves.
    pub fn retain_revision_root(
        &mut self,
        task_id: PlanTaskId,
        attempt_id: ExecutionAttemptId,
    ) -> bool {
        let Some(prepared) = self.prepared_execution_roots.remove(&attempt_id) else {
            return false;
        };
        self.revision_execution_roots.insert(task_id, prepared);
        true
    }

    /// Rebind a retained revision checkout to a newly scheduled attempt. A
    /// task never has two active owners of this path at the same time.
    pub fn adopt_revision_root(
        &mut self,
        task_id: PlanTaskId,
        attempt_id: ExecutionAttemptId,
    ) -> bool {
        let Some(mut prepared) = self.revision_execution_roots.remove(&task_id) else {
            return false;
        };
        let Some(base_revision) = prepared.isolation.base_revision.clone() else {
            self.revision_execution_roots.insert(task_id, prepared);
            return false;
        };
        prepared.isolation = ExecutionIsolation::git_worktree(attempt_id, base_revision);
        self.prepared_execution_roots.insert(attempt_id, prepared);
        true
    }

    pub fn retire_execution_root(&mut self, attempt_id: ExecutionAttemptId) -> bool {
        self.prepared_execution_roots
            .remove(&attempt_id)
            .is_some_and(|prepared| prepared.owns_checkout)
    }

    pub fn active_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn active_attempt_ids(&self) -> Vec<ExecutionAttemptId> {
        let mut attempt_ids = self.sessions.keys().copied().collect::<Vec<_>>();
        attempt_ids.sort_by_key(|attempt_id| attempt_id.0);
        attempt_ids
    }

    /// Build durable cancellation intent without stopping the provider first.
    /// `observe_finished` performs the process-local cancellation only after
    /// authority accepts and projects the terminal event.
    pub fn cancellation_command(
        &self,
        attempt_id: ExecutionAttemptId,
        current_revision: u64,
        timestamp_unix_ms: i64,
    ) -> Result<WorkspaceCommandEnvelope, WorkspaceExecutionAdapterError> {
        let session = self
            .sessions
            .get(&attempt_id)
            .ok_or(WorkspaceExecutionAdapterError::NotExecutionStart)?;
        let launch = &session.request.launch;
        Ok(WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: launch.project_id,
            work_item_id: launch.work_item_id,
            actor_id: launch.actor_id,
            device_id: launch.device_id,
            correlation_id: launch.correlation_id,
            timestamp_unix_ms,
            expected_revision: Some(current_revision),
            payload: WorkspaceCommandPayload::CancelAgentExecution {
                attempt_id,
                reason: "Execution cancelled by the workspace owner.".into(),
            },
        })
    }

    pub fn pause_command(
        &self,
        attempt_id: ExecutionAttemptId,
        current_revision: u64,
        timestamp_unix_ms: i64,
    ) -> Result<WorkspaceCommandEnvelope, WorkspaceExecutionAdapterError> {
        let session = self
            .sessions
            .get(&attempt_id)
            .ok_or(WorkspaceExecutionAdapterError::NotExecutionStart)?;
        let launch = &session.request.launch;
        Ok(WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: launch.project_id,
            work_item_id: launch.work_item_id,
            actor_id: launch.actor_id,
            device_id: launch.device_id,
            correlation_id: launch.correlation_id,
            timestamp_unix_ms,
            expected_revision: Some(current_revision),
            payload: WorkspaceCommandPayload::PauseAgentExecution {
                attempt_id,
                reason: "Execution paused by the workspace owner.".into(),
            },
        })
    }

    /// Renew the first lease approaching expiry. The returned command is
    /// revision guarded; the session keeps using its current lease until the
    /// accepted renewal event is observed.
    pub fn next_lease_renewal_command(
        &self,
        current_revision: u64,
        timestamp_unix_ms: i64,
    ) -> Option<WorkspaceCommandEnvelope> {
        let mut sessions = self.sessions.values().collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.request.launch.attempt.id.0);
        let session = sessions.into_iter().find(|session| {
            let lease = &session.request.launch.lease;
            lease.status == dragent_workspace_model::ExecutionLeaseStatus::Active
                && lease.expires_at_unix_ms > timestamp_unix_ms
                && lease.expires_at_unix_ms.saturating_sub(timestamp_unix_ms)
                    <= LEASE_RENEWAL_LEAD_MS
        })?;
        let launch = &session.request.launch;
        Some(WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: launch.project_id,
            work_item_id: launch.work_item_id,
            actor_id: launch.lease.charging.owner_id,
            device_id: launch.device_id,
            correlation_id: launch.correlation_id,
            timestamp_unix_ms,
            expected_revision: Some(current_revision),
            payload: WorkspaceCommandPayload::RenewExecutionLease {
                lease_id: launch.lease.id,
                lease_expires_at_unix_ms: timestamp_unix_ms + LEASE_RENEWAL_DURATION_MS,
            },
        })
    }

    pub fn observe_lease_renewed(&mut self, event: &WorkspaceEventEnvelope) {
        let WorkspaceEventPayload::ExecutionLeaseRenewed { lease, .. } = &event.payload else {
            return;
        };
        for session in self.sessions.values_mut() {
            if session.request.launch.lease.id == lease.id {
                session.request.launch.lease = lease.clone();
            }
        }
    }

    pub fn checkpoint_summaries(&self) -> Vec<WorkspaceCheckpointSummary> {
        let mut summaries = Vec::new();
        for (attempt_id, session) in &self.sessions {
            if let Some(summary) = checkpoint_summary(
                *attempt_id,
                &session.request.launch.task.title,
                &session.checkpoints,
                session.checkpoint_message_id,
            ) {
                summaries.push(summary);
            }
        }
        for (attempt_id, archived) in &self.checkpoint_archive {
            if self.sessions.contains_key(attempt_id) {
                continue;
            }
            if let Some(summary) = checkpoint_summary(
                *attempt_id,
                &archived.task_title,
                &archived.store,
                checkpoint_message_id(*attempt_id),
            ) {
                summaries.push(summary);
            }
        }
        summaries.sort_by(|left, right| left.task_title.cmp(&right.task_title));
        summaries
    }

    pub fn restore_checkpoint(&self, attempt_id: ExecutionAttemptId) -> Option<String> {
        if let Some(session) = self.sessions.get(&attempt_id) {
            return session.checkpoints.restore(session.checkpoint_message_id);
        }
        self.checkpoint_archive
            .get(&attempt_id)?
            .store
            .restore(checkpoint_message_id(attempt_id))
    }

    pub fn activity(&self, attempt_id: ExecutionAttemptId) -> Vec<WorkspaceExecutionActivity> {
        self.activity
            .get(&attempt_id)
            .map(|log| log.entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn try_next_actions(&mut self) -> Vec<(ExecutionAttemptId, WorkspacePipelineAction)> {
        let mut actions = Vec::new();
        for attempt_id in self.active_attempt_ids() {
            let session = self.sessions.get_mut(&attempt_id).unwrap();
            for _ in 0..MAX_SESSION_EVENTS_PER_POLL {
                let Some(action) = session.try_next() else {
                    break;
                };
                self.activity.entry(attempt_id).or_default().record(&action);
                let requires_authority = matches!(
                    action,
                    WorkspacePipelineAction::ConsentRequired(_)
                        | WorkspacePipelineAction::Finished(_)
                );
                actions.push((attempt_id, action));
                if requires_authority {
                    return actions;
                }
            }
        }
        actions
    }

    pub fn consent_command(
        &self,
        attempt_id: ExecutionAttemptId,
        request: &WorkspaceToolRequest,
        current_revision: u64,
        timestamp_unix_ms: i64,
    ) -> Result<WorkspaceCommandEnvelope, WorkspaceExecutionAdapterError> {
        self.sessions
            .get(&attempt_id)
            .ok_or(WorkspaceExecutionAdapterError::InvalidConsentRequest)?
            .consent_command(request, current_revision, timestamp_unix_ms)
    }

    /// Tool name behind a parked consent request, for standing approvals
    /// and popover context. Reads runtime session state only.
    pub fn parked_consent_tool_name(
        &self,
        attempt_id: ExecutionAttemptId,
        call_id: &str,
    ) -> Option<String> {
        self.sessions
            .get(&attempt_id)?
            .parked_tool_calls
            .get(call_id)
            .map(|request| request.tool_name.clone())
    }

    pub fn park_tool_for_consent(
        &mut self,
        attempt_id: ExecutionAttemptId,
        request: WorkspaceToolRequest,
    ) -> Result<(), WorkspaceExecutionAdapterError> {
        let session = self
            .sessions
            .get_mut(&attempt_id)
            .ok_or(WorkspaceExecutionAdapterError::InvalidConsentRequest)?;
        if !session.accepted_tool_calls.contains(&request.call_id) {
            return Err(WorkspaceExecutionAdapterError::InvalidConsentRequest);
        }
        session
            .parked_tool_calls
            .insert(request.call_id.clone(), request);
        Ok(())
    }

    pub fn observe_attention_resolution(
        &mut self,
        event: &WorkspaceEventEnvelope,
    ) -> Option<WorkspaceResolvedToolAction> {
        let WorkspaceEventPayload::AttentionUpdated { attention_item } = &event.payload else {
            return None;
        };
        if attention_item.kind != AttentionKind::Permission
            || attention_item.status != AttentionStatus::Resolved
        {
            return None;
        }
        let attempt_id = attention_item.execution_attempt_id?;
        let call_id = attention_item.tool_call_id.as_deref()?;
        let request = self
            .sessions
            .get_mut(&attempt_id)?
            .parked_tool_calls
            .remove(call_id)?;
        match attention_item.resolution_kind? {
            AttentionResolution::Approved => Some(WorkspaceResolvedToolAction::Execute {
                attempt_id,
                request,
            }),
            AttentionResolution::Rejected => Some(WorkspaceResolvedToolAction::Reject {
                attempt_id,
                request,
                reason: attention_item
                    .resolution
                    .clone()
                    .unwrap_or_else(|| "The user rejected this operation.".into()),
            }),
            AttentionResolution::Answered | AttentionResolution::Dismissed => None,
        }
    }

    pub fn prepare_tool_checkpoint(
        &mut self,
        attempt_id: ExecutionAttemptId,
        request: &WorkspaceToolRequest,
    ) -> Result<(), WorkspaceExecutionAdapterError> {
        self.sessions
            .get_mut(&attempt_id)
            .ok_or(WorkspaceExecutionAdapterError::InvalidConsentRequest)?
            .prepare_tool_checkpoint(request);
        Ok(())
    }

    pub fn finish_tool_checkpoint(
        &mut self,
        attempt_id: ExecutionAttemptId,
        request: &WorkspaceToolRequest,
    ) {
        if let Some(session) = self.sessions.get_mut(&attempt_id) {
            session.finish_tool_checkpoint(request);
        }
    }

    pub fn tool_result_delivery(
        &self,
        attempt_id: ExecutionAttemptId,
        request: &WorkspaceToolRequest,
    ) -> Option<WorkspaceToolResultDelivery> {
        let session = self.sessions.get(&attempt_id)?;
        Some(WorkspaceToolResultDelivery {
            request: request.clone(),
            sender: session.sender.clone(),
        })
    }

    pub fn completion_command(
        &self,
        attempt_id: ExecutionAttemptId,
        completion: WorkspaceExecutionCompletion,
        current_revision: u64,
        timestamp_unix_ms: i64,
    ) -> Result<WorkspaceCommandEnvelope, WorkspaceExecutionAdapterError> {
        let session = self
            .sessions
            .get(&attempt_id)
            .ok_or(WorkspaceExecutionAdapterError::NotExecutionStart)?;
        self.adapter.finish_command(
            &session.request.launch,
            completion,
            current_revision,
            timestamp_unix_ms,
        )
    }

    pub fn post_tool_result(
        &self,
        attempt_id: ExecutionAttemptId,
        request: &WorkspaceToolRequest,
        output: &str,
        is_error: bool,
    ) -> Result<(), crate::agent::pipeline::StreamError> {
        self.sessions
            .get(&attempt_id)
            .ok_or_else(|| {
                crate::agent::pipeline::StreamError::Network(
                    "workspace execution session is no longer active".into(),
                )
            })?
            .post_tool_result(request, output, is_error)
    }

    pub fn observe_finished(&mut self, event: &WorkspaceEventEnvelope) {
        if let WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } = &event.payload {
            if let Some(mut session) = self.sessions.remove(&attempt.id) {
                if matches!(
                    attempt.status,
                    ExecutionAttemptStatus::Cancelled | ExecutionAttemptStatus::Paused
                ) {
                    let _ = session.cancel();
                }
                self.checkpoint_archive.insert(
                    attempt.id,
                    ArchivedWorkspaceCheckpoint {
                        task_title: session.request.launch.task.title.clone(),
                        store: session.checkpoints,
                    },
                );
            }
            self.replayed_attempts.remove(&attempt.id);
        }
        self.adapter.observe_finished(event);
    }
}
