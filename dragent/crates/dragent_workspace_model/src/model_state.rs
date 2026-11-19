//! Small, UI-agnostic domain types for Dragent's engineering workspace.
//!
//! This crate intentionally contains no filesystem, database, model, or UI
//! code. It is the stable vocabulary shared by persistence, the Rust workspace
//! shell, and the future orchestration event stream.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::dependent_task_ids;
use crate::evaluate_coordination_readiness;
use crate::evaluate_scheduler;
use crate::execution_retry_delay_ms;
use crate::ActorRef;
use crate::AgentInstance;
use crate::AgentInstanceId;
use crate::AgentMessage;
use crate::AgentMessageId;
use crate::AgentMessageKind;
use crate::AgentMessageRouting;
use crate::ArtifactRef;
use crate::AttentionItem;
use crate::AttentionStatus;
use crate::CommunicationThreadSummary;
use crate::CoordinationHealth;
use crate::CoordinationReadiness;
use crate::DecisionStatus;
use crate::Evidence;
use crate::ExecutionAttempt;
use crate::ExecutionAttemptId;
use crate::ExecutionAttemptStatus;
use crate::ExecutionLease;
use crate::ExecutionLeaseId;
use crate::Plan;
use crate::PlanTaskId;
use crate::PlanTaskStatus;
use crate::Project;
use crate::ProjectId;
use crate::ProposedTeam;
use crate::RoleDefinition;
use crate::RoleDefinitionId;
use crate::SchedulerIssue;
use crate::ShapingMemoryNote;
use crate::ShapingProposal;
use crate::ShapingRecords;
use crate::ThreadId;
use crate::WorkBrief;
use crate::WorkContract;
use crate::WorkContractId;
use crate::WorkItem;
use crate::WorkItemId;
use crate::WorkStatus;
use crate::WorkTransitionError;
use crate::WorkspaceEvent;
use crate::WorkspaceEventKind;
use crate::WorkspaceEventLog;
use crate::COORDINATION_STALL_GRACE_MS;
use crate::MAX_AUTOMATIC_EXECUTION_ATTEMPTS;
use crate::MAX_LIVE_WORKSPACE_EVENTS;
use crate::MAX_SHAPING_MEMORY_NOTES;

/// A deliberately conservative bound for in-memory semantic history. Durable
/// persistence owns the complete audit trail; the UI only needs a recent tail.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoordinationReadinessIssue {
    MissingShapingProposal,
    OpenDecisionsRemain(usize),
    MissingProposedTeam,
    MissingPlan,
    MissingRoleDefinition(RoleDefinitionId),
    MissingWorkContract(WorkContractId),
    ContractIdentityMismatch(WorkContractId),
    ContractNotApproved(WorkContractId),
    PlanTaskUnassigned(PlanTaskId),
    PlanTaskReferencesUnknownContract {
        task_id: PlanTaskId,
        contract_id: WorkContractId,
    },
    ContractHasNoPlanTask(WorkContractId),
}

/// The bounded in-memory projection consumed by the UI. Durable persistence
/// can serialize the `Project`/`WorkItem` values and append the same events
/// independently; the projection never owns file contents or terminal output.
#[derive(Clone, Debug)]
pub struct WorkspaceState {
    project: Project,
    active_work: Option<WorkItem>,
    events: WorkspaceEventLog,
}

impl WorkspaceState {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            active_work: None,
            events: WorkspaceEventLog::default(),
        }
    }

    /// Rebuild the bounded UI projection from durable metadata. Events for a
    /// different project/work item are rejected at this boundary so switching
    /// folders cannot leak another workspace's activity into the current UI.
    pub fn restore(
        project: Project,
        active_work: Option<WorkItem>,
        events: impl IntoIterator<Item = WorkspaceEvent>,
    ) -> Self {
        let active_work = active_work.filter(|work| work.project_id == project.id);
        let active_work_id = active_work.as_ref().map(|work| work.id);
        let events = events
            .into_iter()
            .filter(|event| {
                event.project_id == project.id
                    && match (active_work_id, event.work_item_id) {
                        (Some(active), Some(event_work)) => active == event_work,
                        (_, None) => true,
                        (None, Some(_)) => false,
                    }
            })
            .collect::<Vec<_>>();
        Self {
            project,
            active_work,
            events: WorkspaceEventLog::restore(events, MAX_LIVE_WORKSPACE_EVENTS),
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn active_work(&self) -> Option<&WorkItem> {
        self.active_work.as_ref()
    }

    pub fn events(&self) -> &WorkspaceEventLog {
        &self.events
    }

    pub fn latest_shaping_proposal(&self) -> Option<&ShapingProposal> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::ShapingProposalUpdated { proposal } => Some(proposal),
                _ => None,
            })
    }

    pub fn latest_shaping_records(&self) -> Option<&ShapingRecords> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::ShapingRecordsUpdated { records } => Some(records),
                _ => None,
            })
    }

    pub fn latest_work_brief(&self) -> Option<&WorkBrief> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::WorkBriefUpdated { work_brief } => Some(work_brief),
                _ => None,
            })
    }

    pub fn latest_proposed_team(&self) -> Option<&ProposedTeam> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::ProposedTeamUpdated { proposed_team } => Some(proposed_team),
                _ => None,
            })
    }

    pub fn role_definition(&self, role_definition_id: RoleDefinitionId) -> Option<&RoleDefinition> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::RoleDefinitionUpdated { role_definition }
                    if role_definition.id == role_definition_id =>
                {
                    Some(role_definition)
                }
                _ => None,
            })
    }

    pub fn work_contract(&self, work_contract_id: WorkContractId) -> Option<&WorkContract> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::WorkContractUpdated { work_contract }
                    if work_contract.id == work_contract_id =>
                {
                    Some(work_contract)
                }
                WorkspaceEventKind::AgentInstanceProvisioned { work_contract, .. }
                    if work_contract.id == work_contract_id =>
                {
                    Some(work_contract)
                }
                WorkspaceEventKind::AgentInstanceActivated { work_contract, .. }
                | WorkspaceEventKind::AgentExecutionReviewResolved { work_contract, .. }
                    if work_contract.id == work_contract_id =>
                {
                    Some(work_contract)
                }
                _ => None,
            })
    }

    pub fn agent_instance(&self, agent_instance_id: AgentInstanceId) -> Option<&AgentInstance> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::AgentInstanceProvisioned { agent_instance, .. }
                    if agent_instance.id == agent_instance_id =>
                {
                    Some(agent_instance)
                }
                WorkspaceEventKind::AgentInstanceActivated { agent_instance, .. }
                | WorkspaceEventKind::ExecutionLeaseRenewed { agent_instance, .. }
                | WorkspaceEventKind::AgentExecutionRetryPrepared { agent_instance, .. }
                    if agent_instance.id == agent_instance_id =>
                {
                    Some(agent_instance)
                }
                WorkspaceEventKind::AgentExecutionStarted { agent_instance, .. }
                    if agent_instance.id == agent_instance_id =>
                {
                    Some(agent_instance)
                }
                WorkspaceEventKind::AgentExecutionFinished { agent_instance, .. }
                | WorkspaceEventKind::AgentExecutionReviewResolved { agent_instance, .. }
                    if agent_instance.id == agent_instance_id =>
                {
                    Some(agent_instance)
                }
                _ => None,
            })
    }

    /// Latest value for every contract in canonical event order. Runtime
    /// binding/activation events replace the earlier editable contract record.
    pub fn work_contracts(&self) -> Vec<&WorkContract> {
        let mut seen = HashSet::new();
        let mut contracts = self
            .events
            .iter()
            .rev()
            .filter_map(|event| {
                let contract = match &event.kind {
                    WorkspaceEventKind::WorkContractUpdated { work_contract }
                    | WorkspaceEventKind::AgentInstanceProvisioned { work_contract, .. }
                    | WorkspaceEventKind::AgentInstanceActivated { work_contract, .. }
                    | WorkspaceEventKind::AgentExecutionReviewResolved { work_contract, .. } => {
                        work_contract
                    }
                    _ => return None,
                };
                seen.insert(contract.id).then_some(contract)
            })
            .collect::<Vec<_>>();
        contracts.reverse();
        contracts
    }

    /// Latest value for every durable runtime-agent identity.
    pub fn agent_instances(&self) -> Vec<&AgentInstance> {
        let mut seen = HashSet::new();
        let mut agents = self
            .events
            .iter()
            .rev()
            .filter_map(|event| {
                let agent = match &event.kind {
                    WorkspaceEventKind::AgentInstanceProvisioned { agent_instance, .. }
                    | WorkspaceEventKind::AgentInstanceActivated { agent_instance, .. }
                    | WorkspaceEventKind::ExecutionLeaseRenewed { agent_instance, .. }
                    | WorkspaceEventKind::AgentExecutionRetryPrepared { agent_instance, .. }
                    | WorkspaceEventKind::AgentExecutionStarted { agent_instance, .. }
                    | WorkspaceEventKind::AgentExecutionFinished { agent_instance, .. }
                    | WorkspaceEventKind::AgentExecutionReviewResolved { agent_instance, .. } => {
                        agent_instance
                    }
                    _ => return None,
                };
                seen.insert(agent.id).then_some(agent)
            })
            .collect::<Vec<_>>();
        agents.reverse();
        agents
    }

    pub fn execution_lease(&self, lease_id: ExecutionLeaseId) -> Option<&ExecutionLease> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::AgentInstanceActivated { lease, .. }
                | WorkspaceEventKind::ExecutionLeaseRenewed { lease, .. }
                | WorkspaceEventKind::AgentExecutionRetryPrepared { lease, .. }
                    if lease.id == lease_id =>
                {
                    Some(lease)
                }
                WorkspaceEventKind::AgentExecutionStarted { lease, .. } if lease.id == lease_id => {
                    Some(lease)
                }
                WorkspaceEventKind::AgentExecutionFinished { lease, .. }
                | WorkspaceEventKind::AgentExecutionReviewResolved { lease, .. }
                    if lease.id == lease_id =>
                {
                    Some(lease)
                }
                _ => None,
            })
    }

    pub fn execution_attempt(&self, attempt_id: ExecutionAttemptId) -> Option<&ExecutionAttempt> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::AgentExecutionStarted { attempt, .. }
                | WorkspaceEventKind::AgentExecutionFinished { attempt, .. }
                | WorkspaceEventKind::AgentExecutionReviewResolved { attempt, .. }
                    if attempt.id == attempt_id =>
                {
                    Some(attempt)
                }
                _ => None,
            })
    }

    /// Latest value for every execution-attempt identity. Finished events
    /// replace their Running predecessor.
    pub fn execution_attempts(&self) -> Vec<&ExecutionAttempt> {
        let mut seen = HashSet::new();
        let mut attempts = self
            .events
            .iter()
            .rev()
            .filter_map(|event| {
                let attempt = match &event.kind {
                    WorkspaceEventKind::AgentExecutionStarted { attempt, .. }
                    | WorkspaceEventKind::AgentExecutionFinished { attempt, .. }
                    | WorkspaceEventKind::AgentExecutionReviewResolved { attempt, .. } => attempt,
                    _ => return None,
                };
                seen.insert(attempt.id).then_some(attempt)
            })
            .collect::<Vec<_>>();
        attempts.reverse();
        attempts
    }

    /// Canonical operational communication in per-work-item event order.
    /// Shaping conversation remains represented by the legacy user/principal
    /// projection variants and is intentionally excluded here.
    pub fn agent_messages(&self) -> Vec<(&AgentMessage, &AgentMessageRouting)> {
        let mut seen = HashSet::new();
        let mut messages = self
            .events
            .iter()
            .rev()
            .filter_map(|event| match &event.kind {
                WorkspaceEventKind::AgentMessagePosted { message, routing }
                | WorkspaceEventKind::AgentMessageDeliveryUpdated { message, routing }
                    if seen.insert(message.id) =>
                {
                    Some((message, routing))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        messages.reverse();
        messages
    }

    /// Every communication thread with at least one message, most recently
    /// active first. Pure projection over `agent_messages()` — see
    /// `CommunicationThreadSummary`'s doc comment for why this is never a
    /// stored field.
    pub fn communication_threads(&self) -> Vec<CommunicationThreadSummary> {
        let principal_id = ActorRef::principal_agent(self.project.id).id;
        let mut by_thread: HashMap<ThreadId, Vec<(&AgentMessage, &AgentMessageRouting)>> =
            HashMap::new();
        for (message, routing) in self.agent_messages() {
            by_thread
                .entry(message.thread_id)
                .or_default()
                .push((message, routing));
        }
        let mut summaries = by_thread
            .into_iter()
            .map(|(thread_id, thread_messages)| {
                let mut seen_participants = HashSet::new();
                let participant_ids = thread_messages
                    .iter()
                    .map(|(message, _)| message.sender_id)
                    .filter(|id| seen_participants.insert(*id))
                    .collect::<Vec<_>>();
                let task_id = thread_messages
                    .iter()
                    .find_map(|(_, routing)| routing.task_id);
                let last_activity_unix_ms = thread_messages
                    .iter()
                    .map(|(message, _)| message.created_at_unix_ms)
                    .max()
                    .unwrap_or(0);
                let answered_ids: HashSet<AgentMessageId> = thread_messages
                    .iter()
                    .filter(|(message, _)| message.sender_id == principal_id)
                    .filter_map(|(message, _)| message.reply_to)
                    .collect();
                let is_open = thread_messages.iter().any(|(message, routing)| {
                    matches!(
                        message.kind,
                        AgentMessageKind::Blocker | AgentMessageKind::DecisionRequest
                    ) && message.sender_id != principal_id
                        && routing.recipient_ids.contains(&principal_id)
                        && !answered_ids.contains(&message.id)
                });
                CommunicationThreadSummary {
                    thread_id,
                    task_id,
                    participant_ids,
                    last_activity_unix_ms,
                    is_open,
                }
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|a, b| b.last_activity_unix_ms.cmp(&a.last_activity_unix_ms));
        summaries
    }

    pub fn evidence(&self) -> Vec<&Evidence> {
        let mut seen = HashSet::new();
        let mut evidence = self
            .events
            .iter()
            .rev()
            .filter_map(|event| match &event.kind {
                WorkspaceEventKind::EvidenceRecorded { evidence } if seen.insert(evidence.id) => {
                    Some(evidence)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        evidence.reverse();
        evidence
    }

    /// Latest value for every durable Attention identity. Reverse traversal
    /// makes resolution updates replace their open predecessor without keeping
    /// a second UI-owned copy of canonical state.
    pub fn attention_items(&self) -> Vec<&AttentionItem> {
        let mut seen = HashSet::new();
        let mut items = self
            .events
            .iter()
            .rev()
            .filter_map(|event| match &event.kind {
                WorkspaceEventKind::AttentionUpdated { attention_item }
                    if seen.insert(attention_item.id) =>
                {
                    Some(attention_item)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        items.reverse();
        items
    }

    pub fn open_attention_items(&self) -> Vec<&AttentionItem> {
        self.attention_items()
            .into_iter()
            .filter(|item| {
                matches!(
                    item.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                )
            })
            .collect()
    }

    pub fn latest_structured_plan(&self) -> Option<&Plan> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                WorkspaceEventKind::StructuredPlanUpdated { plan }
                | WorkspaceEventKind::AgentExecutionRetryPrepared { plan, .. }
                | WorkspaceEventKind::AgentExecutionStarted { plan, .. }
                | WorkspaceEventKind::AgentExecutionFinished { plan, .. }
                | WorkspaceEventKind::AgentExecutionReviewResolved { plan, .. } => Some(plan),
                _ => None,
            })
    }

    pub fn coordination_readiness(&self) -> CoordinationReadiness {
        // Prefer the per-record open count: inline resolutions supersede the
        // proposal snapshot, so a fully resolved record set must unlock
        // coordination without waiting for another shaping turn.
        let open_decisions = self.shaping_records_projection().map(|records| {
            records
                .decisions
                .iter()
                .filter(|decision| decision.status == DecisionStatus::Open)
                .count()
        });
        let Some(team) = self.latest_proposed_team() else {
            return evaluate_coordination_readiness(
                self.latest_shaping_proposal(),
                open_decisions,
                None,
                &HashSet::new(),
                &HashMap::new(),
                self.latest_structured_plan(),
            );
        };
        let role_definition_ids = team
            .roles
            .iter()
            .filter_map(|role| {
                self.role_definition(role.role_definition_id)
                    .map(|definition| definition.id)
            })
            .collect();
        let contracts = team
            .roles
            .iter()
            .filter_map(|role| {
                self.work_contract(role.work_contract_id)
                    .cloned()
                    .map(|contract| (contract.id, contract))
            })
            .collect();
        evaluate_coordination_readiness(
            self.latest_shaping_proposal(),
            open_decisions,
            Some(team),
            &role_definition_ids,
            &contracts,
            self.latest_structured_plan(),
        )
    }

    /// UI-independent coordination health. Coordinate renders this projection;
    /// execution ownership never lives in view state. Never reports idle when
    /// coordination started but nothing is running, waiting, or blocked: that
    /// state is `Stalled` and must surface a concrete next action.
    pub fn coordination_health(
        &self,
        maximum_parallel_tasks: usize,
        now_unix_ms: i64,
    ) -> CoordinationHealth {
        let work = match self.active_work() {
            None => return CoordinationHealth::NotStarted,
            Some(work) => work,
        };
        match work.status {
            WorkStatus::Paused => return CoordinationHealth::Paused,
            WorkStatus::Cancelled => return CoordinationHealth::Cancelled,
            WorkStatus::Completed => return CoordinationHealth::Completed,
            WorkStatus::Draft
            | WorkStatus::Understanding
            | WorkStatus::WaitingForDecision
            | WorkStatus::ReadyToPlan
            | WorkStatus::Planning => return CoordinationHealth::NotStarted,
            _ => {}
        }
        let Some(plan) = self.latest_structured_plan() else {
            return CoordinationHealth::Starting;
        };
        if work.status == WorkStatus::ReadyToExecute {
            return CoordinationHealth::Starting;
        }
        let attempts = self.execution_attempts();
        let running = attempts
            .iter()
            .any(|attempt| attempt.status == ExecutionAttemptStatus::Running);
        if running {
            return CoordinationHealth::Active;
        }
        let contracts = self
            .work_contracts()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let criteria = self
            .shaping_records_projection()
            .map(|records| records.acceptance_criteria)
            .unwrap_or_default();
        let attention = self
            .attention_items()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let projection = evaluate_scheduler(
            plan,
            &contracts,
            &criteria,
            &[],
            &attention,
            maximum_parallel_tasks,
        );
        if !projection.launchable_task_ids.is_empty() {
            return CoordinationHealth::Active;
        }
        // A retry sleeping with budget remaining is Waiting, not Stalled: the
        // controller will re-drive it at its deadline.
        let pending_retry = plan.tasks.iter().any(|task| {
            if task.status != PlanTaskStatus::Failed {
                return false;
            }
            let Some(latest) = attempts
                .iter()
                .filter(|attempt| attempt.task_id == task.id)
                .max_by_key(|attempt| attempt.attempt_number)
            else {
                return false;
            };
            if !matches!(
                latest.status,
                ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut
            ) || latest.attempt_number >= MAX_AUTOMATIC_EXECUTION_ATTEMPTS
            {
                return false;
            }
            let Some(finished_at) = latest.finished_at_unix_ms else {
                return false;
            };
            now_unix_ms
                < finished_at.saturating_add(execution_retry_delay_ms(latest.attempt_number))
        });
        if pending_retry {
            return CoordinationHealth::Waiting;
        }
        let blocked_by_attention = attention.iter().any(|item| {
            matches!(
                item.status,
                AttentionStatus::Open | AttentionStatus::Acknowledged
            )
        });
        let blocked_by_dep = projection.issues.iter().any(|issue| {
            matches!(
                issue,
                SchedulerIssue::DependencyFailed { .. } | SchedulerIssue::AttentionRequired { .. }
            )
        });
        if blocked_by_attention || blocked_by_dep {
            return CoordinationHealth::Blocked;
        }
        if projection.active_task_count > 0 || !projection.ready_task_ids.is_empty() {
            return CoordinationHealth::Waiting;
        }
        // Executing with no running work, nothing launchable, no pending
        // retry, and no blocking attention is indistinguishable from idle.
        // Report Stalled once the grace period elapses so the watchdog fires.
        if work.status == WorkStatus::Executing {
            let last_activity = attempts
                .iter()
                .filter_map(|attempt| {
                    attempt
                        .finished_at_unix_ms
                        .or(Some(attempt.started_at_unix_ms))
                })
                .max()
                .or(Some(work.updated_at_unix_ms));
            if last_activity
                .is_some_and(|at| now_unix_ms.saturating_sub(at) >= COORDINATION_STALL_GRACE_MS)
            {
                return CoordinationHealth::Stalled;
            }
            return CoordinationHealth::Waiting;
        }
        CoordinationHealth::Waiting
    }

    /// Runtime agents assigned to tasks that directly depend on `task_id`.
    /// Used for direct agent-to-agent handoff routing alongside the Principal
    /// copy: dependents observe the typed handoff without a relay.
    pub fn direct_handoff_agent_ids(&self, task_id: PlanTaskId) -> Vec<AgentInstanceId> {
        let Some(plan) = self.latest_structured_plan() else {
            return Vec::new();
        };
        let dependents = dependent_task_ids(plan, task_id);
        let mut ids = plan
            .tasks
            .iter()
            .filter(|task| dependents.contains(&task.id))
            .filter_map(|task| task.assigned_contract_id)
            .filter_map(|contract_id| self.work_contract(contract_id))
            .filter_map(|contract| contract.agent_instance_id)
            .collect::<Vec<_>>();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        ids
    }

    /// Reduce the latest complete shaping snapshot plus subsequent
    /// individually edited records. This returns owned projection data so the
    /// immutable event log remains the source of truth.
    pub fn shaping_records_projection(&self) -> Option<ShapingRecords> {
        let mut projection: Option<ShapingRecords> = None;
        for event in self.events.iter() {
            match &event.kind {
                WorkspaceEventKind::ShapingRecordsUpdated { records } => {
                    projection = Some(records.clone());
                }
                WorkspaceEventKind::RequirementUpdated { requirement } => {
                    if let Some(records) = &mut projection {
                        upsert_by(&mut records.requirements, requirement.clone(), |record| {
                            record.id
                        });
                    }
                }
                WorkspaceEventKind::DecisionUpdated { decision } => {
                    if let Some(records) = &mut projection {
                        upsert_by(&mut records.decisions, decision.clone(), |record| record.id);
                    }
                }
                WorkspaceEventKind::AcceptanceCriterionUpdated {
                    acceptance_criterion,
                } => {
                    if let Some(records) = &mut projection {
                        upsert_by(
                            &mut records.acceptance_criteria,
                            acceptance_criterion.clone(),
                            |record| record.id,
                        );
                    }
                }
                _ => {}
            }
        }
        projection
    }

    /// The Principal's durable work-item brain: every recorded memory note in
    /// arrival order, rotated to the recent tail. Append-only — entries are
    /// never edited or removed by later turns.
    pub fn shaping_memory(&self) -> Vec<ShapingMemoryNote> {
        let mut notes = Vec::new();
        for event in self.events.iter() {
            if let WorkspaceEventKind::ShapingMemoryRecorded { note } = &event.kind {
                if !notes
                    .iter()
                    .any(|existing: &ShapingMemoryNote| existing.id == note.id)
                {
                    notes.push(note.clone());
                }
            }
        }
        if notes.len() > MAX_SHAPING_MEMORY_NOTES {
            notes.drain(..notes.len() - MAX_SHAPING_MEMORY_NOTES);
        }
        notes
    }

    pub fn artifacts(&self) -> Vec<ArtifactRef> {
        let mut artifacts = Vec::new();
        for event in self.events.iter() {
            match &event.kind {
                WorkspaceEventKind::ArtifactUpdated { artifact } => {
                    upsert_by(&mut artifacts, artifact.clone(), |artifact| artifact.id);
                }
                WorkspaceEventKind::ArtifactsReplaced {
                    artifacts: replacement,
                } => {
                    artifacts.clone_from(replacement);
                }
                _ => {}
            }
        }
        artifacts
    }

    pub fn start_work(&mut self, title: impl Into<String>, now_unix_ms: i64) -> &WorkItem {
        self.start_work_with_id(WorkItemId::new(), title, now_unix_ms)
    }

    /// Start or restore a work item with an authority-assigned identity. The UI
    /// fixture uses a deterministic ID today; replay and cloud projections can
    /// supply their canonical ID through the same path later.
    pub fn start_work_with_id(
        &mut self,
        work_item_id: WorkItemId,
        title: impl Into<String>,
        now_unix_ms: i64,
    ) -> &WorkItem {
        let mut work = WorkItem::new(self.project.id, title, now_unix_ms);
        work.id = work_item_id;
        self.events.append(
            self.project.id,
            Some(work.id),
            now_unix_ms,
            WorkspaceEventKind::WorkCreated {
                title: work.title.clone(),
            },
        );
        self.active_work = Some(work);
        self.active_work.as_ref().expect("work was just inserted")
    }

    /// Apply a validated lifecycle transition and emit its semantic event.
    /// Returns `None` when no work item is active; renderers never synthesize a
    /// work item merely because a control was clicked.
    pub fn transition_active_work(
        &mut self,
        next: WorkStatus,
        now_unix_ms: i64,
    ) -> Option<Result<(), WorkTransitionError>> {
        let work = self.active_work.as_mut()?;
        if let Err(error) = work.transition_to(next, now_unix_ms) {
            return Some(Err(error));
        }
        let work_item_id = work.id;
        self.events.append(
            self.project.id,
            Some(work_item_id),
            now_unix_ms,
            WorkspaceEventKind::WorkStatusChanged { status: next },
        );
        Some(Ok(()))
    }

    pub fn record(&mut self, timestamp_unix_ms: i64, kind: WorkspaceEventKind) {
        // Unlike `transition_active_work`, this applies an event the
        // authority already accepted, so it never re-validates the
        // transition — only mirrors the same field mutation onto the local
        // `active_work`. Without this, `active_work().status` stays frozen
        // at whatever `restore()` last loaded: derived views (plan, tasks,
        // attempts) read live from `events` and update correctly, but any
        // render gated on `active_work().status` — e.g. Coordinate's
        // pre-flight checklist vs. execution map — stays stuck on stale
        // status until the process restarts and reprojects from scratch.
        if let WorkspaceEventKind::WorkStatusChanged { status } = &kind {
            if let Some(work) = self.active_work.as_mut() {
                work.status = *status;
                work.updated_at_unix_ms = timestamp_unix_ms;
            }
        }
        let work_item_id = self.active_work.as_ref().map(|work| work.id);
        self.events
            .append(self.project.id, work_item_id, timestamp_unix_ms, kind);
    }
}

fn upsert_by<T, Id: PartialEq>(items: &mut Vec<T>, replacement: T, id: impl Fn(&T) -> Id) {
    let replacement_id = id(&replacement);
    if let Some(existing) = items.iter_mut().find(|item| id(item) == replacement_id) {
        *existing = replacement;
    } else {
        items.push(replacement);
    }
}

impl Default for WorkspaceEventLog {
    fn default() -> Self {
        Self::new(MAX_LIVE_WORKSPACE_EVENTS)
    }
}

impl WorkspaceEventLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity.min(MAX_LIVE_WORKSPACE_EVENTS)),
            next_sequence: 0,
            capacity: capacity.min(MAX_LIVE_WORKSPACE_EVENTS),
        }
    }

    fn restore(mut events: Vec<WorkspaceEvent>, capacity: usize) -> Self {
        let capacity = capacity.min(MAX_LIVE_WORKSPACE_EVENTS);
        events.sort_by_key(|event| (event.sequence, event.timestamp_unix_ms));
        let next_sequence = events
            .last()
            .map(|event| event.sequence.saturating_add(1))
            .unwrap_or(0);
        if events.len() > capacity {
            events.drain(..events.len() - capacity);
        }
        Self {
            events: events.into(),
            next_sequence,
            capacity,
        }
    }

    pub fn append(
        &mut self,
        project_id: ProjectId,
        work_item_id: Option<WorkItemId>,
        timestamp_unix_ms: i64,
        kind: WorkspaceEventKind,
    ) -> Option<WorkspaceEvent> {
        let event = WorkspaceEvent {
            sequence: self.next_sequence,
            timestamp_unix_ms,
            project_id,
            work_item_id,
            kind,
        };
        self.next_sequence = self.next_sequence.saturating_add(1);
        if self.capacity == 0 {
            return Some(event);
        }
        self.events.push_back(event);
        if self.events.len() > self.capacity {
            self.events.pop_front()
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &WorkspaceEvent> {
        self.events.iter()
    }

    pub fn back(&self) -> Option<&WorkspaceEvent> {
        self.events.back()
    }
}
