//! Workspace pipeline requests, sessions, and tool disposition.
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

#[cfg(test)]
pub(crate) fn write_scopes_overlap(left: &[PathBuf], right: &[PathBuf]) -> bool {
    left.iter().any(|left_scope| {
        right.iter().any(|right_scope| {
            left_scope.starts_with(right_scope) || right_scope.starts_with(left_scope)
        })
    })
}

pub(crate) fn stable_agent_instance_id(contract_id: WorkContractId) -> AgentInstanceId {
    AgentInstanceId(uuid::Uuid::new_v5(
        &contract_id.0,
        b"workspace-agent-instance:v1",
    ))
}

pub(crate) fn stable_initial_lease_id(agent_instance_id: AgentInstanceId) -> ExecutionLeaseId {
    ExecutionLeaseId(uuid::Uuid::new_v5(
        &agent_instance_id.0,
        b"workspace-execution-lease:1",
    ))
}

pub(crate) fn stable_retry_lease_id(
    agent_instance_id: AgentInstanceId,
    attempt_number: u32,
) -> ExecutionLeaseId {
    ExecutionLeaseId(uuid::Uuid::new_v5(
        &agent_instance_id.0,
        format!("workspace-execution-lease:{attempt_number}").as_bytes(),
    ))
}

pub(crate) fn stable_attempt_id(task_id: PlanTaskId, attempt_number: u32) -> ExecutionAttemptId {
    ExecutionAttemptId(uuid::Uuid::new_v5(
        &task_id.0,
        format!("workspace-execution-attempt:{attempt_number}").as_bytes(),
    ))
}

pub(crate) fn checkpoint_message_id(attempt_id: ExecutionAttemptId) -> usize {
    let bytes = attempt_id.0.as_bytes();
    let mut value = [0_u8; std::mem::size_of::<usize>()];
    let width = value.len();
    value.copy_from_slice(&bytes[..width]);
    usize::from_le_bytes(value)
}

pub fn workspace_run_id(attempt_id: ExecutionAttemptId) -> String {
    format!("local-workspace-{}", attempt_id.0.simple())
}

pub(crate) fn checkpoint_summary(
    attempt_id: ExecutionAttemptId,
    task_title: &str,
    store: &crate::agent::checkpoint_store::CheckpointStore,
    message_id: usize,
) -> Option<WorkspaceCheckpointSummary> {
    let files = store.preview_stats(message_id)?;
    let (added_lines, deleted_lines) = files.iter().fold((0_u32, 0_u32), |total, entry| {
        (
            total.0.saturating_add(entry.1),
            total.1.saturating_add(entry.2),
        )
    });
    Some(WorkspaceCheckpointSummary {
        attempt_id,
        task_title: task_title.to_string(),
        changed_files: files.len(),
        added_lines,
        deleted_lines,
    })
}

/// Concrete input for the existing desktop-owned provider/tool stream. Tool
/// calls are emitted through `PipelineEvent`; the workspace host remains
/// responsible for contract permission checks and consent before returning a
/// local tool result.
#[derive(Clone, Debug)]
pub struct WorkspacePipelineRequest {
    pub launch: WorkspaceExecutionLaunch,
    pub project_root: PathBuf,
    pub system_prompt: String,
    pub task_prompt: String,
    /// The user's Auto approval mode, captured by the host at mount time
    /// (never read from global settings inside the classifier, which stays
    /// pure and testable). See `classify_tool_call`.
    pub auto_approve_contract_asks: bool,
}

/// How a tool call from a workspace-bound turn should be handled — single
/// definition in `dragent_agent::pipeline` shared by the engine core,
// the Host, and the UI.
pub use dragent_agent::pipeline::WorkspaceToolDisposition;

impl WorkspacePipelineRequest {
    pub fn new(
        launch: WorkspaceExecutionLaunch,
        project_root: impl Into<PathBuf>,
        trusted_project_context: &str,
        task_communication_context: &str,
    ) -> Result<Self, WorkspaceExecutionAdapterError> {
        let project_root = project_root.into();
        if !project_root.is_absolute() {
            return Err(WorkspaceExecutionAdapterError::InvalidProjectRoot);
        }
        let system_prompt = execution_system_prompt(
            &launch,
            &project_root,
            trusted_project_context,
            task_communication_context,
        );
        let task_prompt = format!(
            "Execute this approved plan task to completion.\n\nTask: {}\n\n{}",
            launch.task.title, launch.task.description
        );
        Ok(Self {
            launch,
            project_root,
            system_prompt,
            task_prompt,
            auto_approve_contract_asks: false,
        })
    }

    /// Apply the user's Auto approval mode: contract permissions marked
    /// "ask" run without a live decision (scope and high-risk rules still
    /// apply).
    pub fn with_auto_approval(mut self, auto: bool) -> Self {
        self.auto_approve_contract_asks = auto;
        self
    }

    /// Start the existing desktop-owned stream with the Work Contract's exact
    /// provider-visible tool allowlist. This function blocks and therefore must
    /// be called from a background worker. The receiver/host executes or parks
    /// emitted tool calls; this method never grants consent itself.
    pub fn run(
        &self,
        tx: mpsc::Sender<
            Result<crate::agent::pipeline::PipelineEvent, crate::agent::pipeline::StreamError>,
        >,
    ) -> Result<(), crate::agent::pipeline::StreamError> {
        let runtime_id = self.launch.attempt.id.0;
        crate::agent::local_tools::set_project_cwd(&self.project_root, runtime_id);
        crate::agent::pipeline::start_scoped_task_stream_with_run_id(
            &workspace_run_id(self.launch.attempt.id),
            &self.launch.attempt.model_route,
            &self.system_prompt,
            &self.task_prompt,
            &[],
            "worker",
            &self.launch.contract.tools,
            tx,
        )
    }

    /// Register this attempt WITHOUT starting a provider stream here: the
    /// host runs it on an `AgentController` (the terminal agent's own engine)
    /// and reports the outcome back through `completion_command`. The
    /// detached session still carries the launch identity every later
    /// authority command (pause, cancel, finish, lease renewal, consent) is
    /// built from, so the runtime's bookkeeping is unchanged.
    pub fn mount_detached(self) -> WorkspacePipelineSession {
        let (tx, rx) = mpsc::channel();
        WorkspacePipelineSession {
            request: self,
            sender: tx,
            receiver: rx,
            runtime_task_id: None,
            accepted_tool_calls: HashSet::new(),
            parked_tool_calls: HashMap::new(),
            checkpoints: crate::agent::checkpoint_store::CheckpointStore::default(),
            checkpoint_message_id: 0,
            response_text: String::new(),
            terminal: false,
        }
    }

    pub fn launch(&self) -> &WorkspaceExecutionLaunch {
        &self.launch
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn task_prompt(&self) -> &str {
        &self.task_prompt
    }

    /// Launch on the shared background executor and return the UI-independent
    /// stream state machine that owns this attempt's provider events.
    pub fn spawn(self) -> WorkspacePipelineSession {
        let (tx, rx) = mpsc::channel();
        let session_sender = tx.clone();
        let background_request = self.clone();
        crate::bg::detach(move || {
            if let Err(error) = background_request.run(tx.clone()) {
                let _ = tx.send(Err(error));
            }
        });
        WorkspacePipelineSession {
            request: self,
            sender: session_sender,
            receiver: rx,
            runtime_task_id: None,
            accepted_tool_calls: HashSet::new(),
            parked_tool_calls: HashMap::new(),
            checkpoints: crate::agent::checkpoint_store::CheckpointStore::default(),
            checkpoint_message_id: 0,
            response_text: String::new(),
            terminal: false,
        }
    }

    /// Enforce the Work Contract again when a streamed tool call reaches the
    /// desktop. Provider filtering is the first boundary; this is the defense
    /// in depth boundary before local execution.
    pub fn classify_tool_call(
        &self,
        event: &crate::agent::pipeline::PipelineEvent,
    ) -> WorkspaceToolDisposition {
        use crate::agent::pipeline::AgentEventKind;

        if event.kind != AgentEventKind::ToolCall {
            return WorkspaceToolDisposition::Denied {
                reason: "Only a complete tool-call event can be executed.".into(),
            };
        }
        let name = event.tool_name.as_deref().unwrap_or_default();
        if name.is_empty() || !self.launch.contract.tools.iter().any(|tool| tool == name) {
            return WorkspaceToolDisposition::Denied {
                reason: format!("Tool `{name}` is not granted by this Work Contract."),
            };
        }
        if event.server_executed.unwrap_or(false) {
            return WorkspaceToolDisposition::ObserveServerResult;
        }
        if name == "ask_user" {
            return WorkspaceToolDisposition::NeedsConsent {
                reason: "The agent needs an answer from the user.".into(),
            };
        }

        let args = event.tool_args.as_deref().unwrap_or("{}");
        let command = (name == "run_command")
            .then(|| crate::agent::approval_policy::command_from_args(args))
            .flatten();
        // Checked before any contract-permission lookup: PR creation,
        // database writes, file deletion, and shell commands that are
        // destructive beyond this task always need a live decision, no
        // matter how broadly Shaping pre-authorized this role's tools.
        if crate::agent::approval_policy::is_always_high_risk(name, command.as_deref()) {
            return WorkspaceToolDisposition::NeedsConsent {
                reason: format!(
                    "`{name}` always requires a live decision, regardless of contract pre-authorization."
                ),
            };
        }
        if crate::agent::approval_policy::is_mutating_tool(name, command.as_deref()) {
            let has_unbounded_effect = matches!(
                name,
                "run_command" | "repl" | "spawn_subagent" | "github_create_pull_request"
            );
            if has_unbounded_effect {
                let permission_name = match name {
                    "run_command" | "repl" => "command_execute",
                    "spawn_subagent" => "agent_delegate",
                    "github_create_pull_request" => "external_write",
                    _ => unreachable!(),
                };
                let explicitly_authorized = self.launch.contract.permissions.iter().any(|grant| {
                    (grant.permission == name || grant.permission == permission_name)
                        && !grant.requires_approval
                });
                if !explicitly_authorized {
                    return WorkspaceToolDisposition::NeedsConsent {
                        reason: format!(
                            "`{name}` can perform effects that cannot be proven to stay inside the contract write scope."
                        ),
                    };
                }
            } else {
                let targets = crate::agent::checkpoint_store::write_target_paths(
                    name,
                    args,
                    &self.project_root,
                );
                if targets.is_empty() {
                    return WorkspaceToolDisposition::Denied {
                        reason: format!("The write targets for `{name}` could not be verified."),
                    };
                }
                if let Some(target) = targets
                    .iter()
                    .find(|target| !self.write_target_is_granted(target))
                {
                    return WorkspaceToolDisposition::Denied {
                        reason: format!(
                            "Write target `{}` is outside the Work Contract scope.",
                            target.display()
                        ),
                    };
                }
            }
        }

        let runtime_id = self.launch.attempt.id.0;
        crate::agent::local_tools::set_project_cwd(&self.project_root, runtime_id);
        if !crate::agent::local_tools::external_read_paths_for_tool(name, args, runtime_id)
            .is_empty()
        {
            return WorkspaceToolDisposition::NeedsConsent {
                reason: "The tool wants to read outside the project root.".into(),
            };
        }

        let is_mutating = crate::agent::approval_policy::is_mutating_tool(name, command.as_deref());
        let contract_permission = self.launch.contract.permissions.iter().find(|grant| {
            grant.permission == name
                || (is_mutating && grant.permission == "project_write")
                || (!is_mutating && grant.permission == "project_read")
        });
        let mut requires_consent = contract_permission
            .map(|grant| grant.requires_approval)
            .unwrap_or_else(|| event.requires_approval.unwrap_or(false));
        // The user's Auto approval mode applies to workers exactly as it does
        // to the terminal agent: an in-scope, non-high-risk operation the
        // contract merely marks "ask" runs autonomously. Out-of-scope writes
        // (denied above), always-high-risk tools, and out-of-root reads still
        // stop — those are the cases that genuinely need a person.
        if requires_consent && self.auto_approve_contract_asks {
            requires_consent = false;
        }
        if requires_consent {
            return WorkspaceToolDisposition::NeedsConsent {
                reason: format!("The Work Contract requires approval before `{name}` runs."),
            };
        }
        WorkspaceToolDisposition::Execute
    }

    pub(crate) fn write_target_is_granted(&self, target: &Path) -> bool {
        let target = normalize_path(target);
        self.launch.contract.write_scope.iter().any(|scope| {
            let scope = normalize_path(&self.project_root.join(scope));
            target.starts_with(scope)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceToolRequest {
    pub runtime_task_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub args_json: String,
    pub reason: String,
}

impl WorkspaceToolRequest {
    /// Human-facing text for a parked consent request — the single source
    /// both the durable `AttentionItem.summary` and the companion
    /// conversation message build from, so the two never drift. Mirrors the
    /// terminal agent's own live approval card
    /// (`build_approval_request` in `agent_block_element.rs`), which shows
    /// the actual command alongside the reason rather than the reason alone.
    pub fn consent_summary(&self) -> String {
        let reason = self.reason.trim();
        let command = crate::agent::approval_policy::command_from_args(&self.args_json);
        match (reason.is_empty(), command) {
            (true, Some(command)) => {
                format!("Approval required to run:\n$ {command}")
            }
            (true, None) => format!("Approval required before `{}` can run.", self.tool_name),
            (false, Some(command)) => format!("{reason}\n$ {command}"),
            (false, None) => reason.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspacePipelineAction {
    RuntimeIdentified {
        runtime_task_id: String,
    },
    TextDelta {
        text: String,
    },
    Thinking {
        text: String,
    },
    ExecuteTool(WorkspaceToolRequest),
    ConsentRequired(WorkspaceToolRequest),
    ServerToolStarted(WorkspaceToolRequest),
    ToolRejected {
        request: WorkspaceToolRequest,
        reason: String,
    },
    ToolFinished {
        call_id: String,
        is_error: bool,
        duration_ms: Option<i64>,
    },
    Finished(WorkspaceExecutionCompletion),
}

/// One active provider stream. It contains no WarpUI handles and can therefore
/// be polled by Coordinate, a background service, or a future cloud projection
/// without recreating an AgentController or terminal pane.
pub struct WorkspacePipelineSession {
    pub(crate) request: WorkspacePipelineRequest,
    pub(crate) sender: mpsc::Sender<
        Result<crate::agent::pipeline::PipelineEvent, crate::agent::pipeline::StreamError>,
    >,
    pub(crate) receiver: mpsc::Receiver<
        Result<crate::agent::pipeline::PipelineEvent, crate::agent::pipeline::StreamError>,
    >,
    pub(crate) runtime_task_id: Option<String>,
    pub(crate) accepted_tool_calls: HashSet<String>,
    pub(crate) parked_tool_calls: HashMap<String, WorkspaceToolRequest>,
    pub(crate) checkpoints: crate::agent::checkpoint_store::CheckpointStore,
    pub(crate) checkpoint_message_id: usize,
    pub(crate) response_text: String,
    pub(crate) terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceExecutionActivity {
    pub sequence: u64,
    pub kind: String,
    pub text: String,
}

#[derive(Default)]
pub(crate) struct WorkspaceExecutionActivityLog {
    pub(crate) entries: VecDeque<WorkspaceExecutionActivity>,
    pub(crate) tool_names: VecDeque<(String, String)>,
    pub(crate) sequence: u64,
}

impl WorkspaceExecutionActivityLog {
    pub(crate) fn record(&mut self, action: &WorkspacePipelineAction) {
        let (kind, text) = match action {
            WorkspacePipelineAction::Thinking { text } => {
                if text.is_empty() {
                    return;
                }
                // Raw reasoning content never reaches the activity feed — only
                // the fact that the agent is thinking does. One indicator per
                // burst, not one per delta chunk.
                if self
                    .entries
                    .back()
                    .is_some_and(|entry| entry.kind == "thinking")
                {
                    return;
                }
                ("thinking", "Thinking…".to_string())
            }
            WorkspacePipelineAction::TextDelta { text } => {
                if text.is_empty() {
                    return;
                }
                if let Some(previous) = self
                    .entries
                    .back_mut()
                    .filter(|entry| entry.kind == "narration")
                {
                    previous.text = bounded_activity_text(&format!("{}{text}", previous.text));
                    return;
                }
                ("narration", text.clone())
            }
            WorkspacePipelineAction::RuntimeIdentified { runtime_task_id } => {
                ("runtime", format!("Runtime identified: {runtime_task_id}"))
            }
            WorkspacePipelineAction::ExecuteTool(request)
            | WorkspacePipelineAction::ServerToolStarted(request)
            | WorkspacePipelineAction::ConsentRequired(request) => {
                let name = bounded_activity_text(&request.tool_name);
                self.tool_names
                    .push_back((request.call_id.clone(), name.clone()));
                if self.tool_names.len() > MAX_EXECUTION_ACTIVITY {
                    self.tool_names.pop_front();
                }
                if matches!(action, WorkspacePipelineAction::ConsentRequired(_)) {
                    ("consent", format!("Approval required: {name}"))
                } else {
                    ("tool", format!("Dispatched: {name}"))
                }
            }
            WorkspacePipelineAction::ToolRejected { request, .. } => {
                ("tool", format!("Rejected: {}", request.tool_name))
            }
            WorkspacePipelineAction::ToolFinished {
                call_id,
                is_error,
                duration_ms,
            } => {
                let name = self
                    .tool_names
                    .iter()
                    .position(|(id, _)| id == call_id)
                    .and_then(|index| self.tool_names.remove(index))
                    .map(|(_, name)| name)
                    .unwrap_or_else(|| "Tool".into());
                let outcome = if *is_error { "Failed" } else { "Completed" };
                let mut text = format!("{outcome}: {name}");
                if let Some(ms) = duration_ms {
                    text.push_str(&format!(" ({ms}ms)"));
                }
                ("tool", text)
            }
            WorkspacePipelineAction::Finished(completion) => {
                self.tool_names.clear();
                (
                    "finished",
                    format!("Execution finished: {:?}", completion.outcome),
                )
            }
        };
        self.sequence += 1;
        self.entries.push_back(WorkspaceExecutionActivity {
            sequence: self.sequence,
            kind: kind.into(),
            text: bounded_activity_text(&text),
        });
        if self.entries.len() > MAX_EXECUTION_ACTIVITY {
            self.entries.pop_front();
        }
    }
}

pub(crate) fn bounded_activity_text(text: &str) -> String {
    let start = text
        .char_indices()
        .rev()
        .nth(MAX_EXECUTION_ACTIVITY_TEXT_CHARS - 1)
        .map(|(index, _)| index)
        .unwrap_or(0);
    text[start..].to_string()
}

impl WorkspacePipelineSession {
    pub fn attempt_id(&self) -> ExecutionAttemptId {
        self.request.launch.attempt.id
    }

    pub(crate) fn bind_checkpoints(&mut self) {
        self.checkpoint_message_id = checkpoint_message_id(self.request.launch.attempt.id);
        self.checkpoints.bind(
            self.request.launch.attempt.id.0,
            self.request.launch.work_item_id.0,
        );
    }

    pub(crate) fn prepare_tool_checkpoint(&mut self, request: &WorkspaceToolRequest) {
        if request.tool_name == "run_command" || request.tool_name == "repl" {
            self.checkpoints.register_shell_command(
                &request.call_id,
                self.checkpoint_message_id,
                &self.request.project_root,
            );
            return;
        }
        for path in crate::agent::checkpoint_store::write_target_paths(
            &request.tool_name,
            &request.args_json,
            &self.request.project_root,
        ) {
            self.checkpoints.capture(self.checkpoint_message_id, &path);
        }
    }

    pub(crate) fn finish_tool_checkpoint(&mut self, request: &WorkspaceToolRequest) {
        if request.tool_name == "run_command" || request.tool_name == "repl" {
            let _ = self.checkpoints.ingest_shell_command(&request.call_id);
        }
    }

    pub fn try_next(&mut self) -> Option<WorkspacePipelineAction> {
        if self.terminal {
            return None;
        }
        match self.receiver.try_recv() {
            Ok(event) => Some(self.ingest(event)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.terminal = true;
                Some(WorkspacePipelineAction::Finished(
                    WorkspaceExecutionCompletion::failed(
                        "The agent stream ended without a terminal result.",
                    ),
                ))
            }
        }
    }

    pub fn ingest(
        &mut self,
        event: Result<crate::agent::pipeline::PipelineEvent, crate::agent::pipeline::StreamError>,
    ) -> WorkspacePipelineAction {
        use crate::agent::pipeline::{AgentEventKind, StreamError};

        let event = match event {
            Ok(event) => event,
            Err(error) => {
                self.terminal = true;
                let summary = error.to_string();
                let completion = if matches!(error, StreamError::Timeout(_)) {
                    WorkspaceExecutionCompletion::timed_out(summary)
                } else {
                    WorkspaceExecutionCompletion::failed(summary)
                };
                return WorkspacePipelineAction::Finished(completion);
            }
        };
        if let Some(runtime_task_id) = event
            .error
            .as_deref()
            .and_then(|value| value.strip_prefix("__task_id__"))
        {
            self.runtime_task_id = Some(runtime_task_id.to_string());
            return WorkspacePipelineAction::RuntimeIdentified {
                runtime_task_id: runtime_task_id.to_string(),
            };
        }
        match event.kind.clone() {
            AgentEventKind::Thinking => WorkspacePipelineAction::Thinking {
                text: event.text.unwrap_or_default(),
            },
            AgentEventKind::TextDelta => {
                let text = event.text.unwrap_or_default();
                self.response_text.push_str(&text);
                WorkspacePipelineAction::TextDelta { text }
            }
            AgentEventKind::ToolCall => self.tool_action(&event),
            AgentEventKind::ToolEnd => WorkspacePipelineAction::ToolFinished {
                call_id: event.tool_id.unwrap_or_default(),
                is_error: event.tool_is_error.unwrap_or(false),
                duration_ms: event.duration_ms,
            },
            AgentEventKind::Done => {
                self.terminal = true;
                let summary = if self.response_text.trim().is_empty() {
                    format!("Completed task: {}", self.request.launch.task.title)
                } else {
                    self.response_text.trim().to_string()
                };
                // A model that reports a blocker in its final text must not be
                // recorded as Succeeded: that false success moves the task to
                // Reviewing with zero changed files and deadlocks the DAG
                // (0/5 tasks, 0 active agents, "No blockers").
                if completion_reports_blocker(&summary) {
                    WorkspacePipelineAction::Finished(WorkspaceExecutionCompletion::failed(summary))
                } else {
                    WorkspacePipelineAction::Finished(WorkspaceExecutionCompletion::succeeded(
                        summary,
                    ))
                }
            }
            AgentEventKind::Error => {
                self.terminal = true;
                WorkspacePipelineAction::Finished(WorkspaceExecutionCompletion::failed(
                    event
                        .error
                        .unwrap_or_else(|| "The agent reported an error.".into()),
                ))
            }
            AgentEventKind::ToolCallStarted | AgentEventKind::StepDone => {
                WorkspacePipelineAction::Thinking {
                    text: event
                        .args_summary
                        .or(event.assistant_content)
                        .unwrap_or_default(),
                }
            }
        }
    }

    pub(crate) fn tool_action(
        &mut self,
        event: &crate::agent::pipeline::PipelineEvent,
    ) -> WorkspacePipelineAction {
        let call_id = event.tool_id.clone().unwrap_or_default();
        let tool_name = event.tool_name.clone().unwrap_or_default();
        let request = WorkspaceToolRequest {
            runtime_task_id: self.runtime_task_id.clone().unwrap_or_default(),
            call_id: call_id.clone(),
            tool_name,
            args_json: event.tool_args.clone().unwrap_or_else(|| "{}".into()),
            reason: event.args_summary.clone().unwrap_or_default(),
        };
        if request.runtime_task_id.is_empty() || call_id.is_empty() {
            self.terminal = true;
            return WorkspacePipelineAction::Finished(WorkspaceExecutionCompletion::failed(
                "The agent emitted a tool call without runtime identity.",
            ));
        }
        if !self.accepted_tool_calls.insert(call_id) {
            return WorkspacePipelineAction::ToolRejected {
                request,
                reason: "Duplicate tool call ignored; this call ID was already processed.".into(),
            };
        }
        match self.request.classify_tool_call(event) {
            WorkspaceToolDisposition::Execute => WorkspacePipelineAction::ExecuteTool(request),
            WorkspaceToolDisposition::ObserveServerResult => {
                WorkspacePipelineAction::ServerToolStarted(request)
            }
            WorkspaceToolDisposition::NeedsConsent { reason } => {
                let mut request = request;
                request.reason = reason;
                self.parked_tool_calls
                    .insert(request.call_id.clone(), request.clone());
                WorkspacePipelineAction::ConsentRequired(request)
            }
            WorkspaceToolDisposition::Denied { reason } => {
                WorkspacePipelineAction::ToolRejected { request, reason }
            }
        }
    }

    /// Resolve an Execute/Consent/Reject action. The result re-enters the
    /// existing local checkpoint loop; tool code is never run by this state
    /// machine itself.
    pub fn post_tool_result(
        &self,
        request: &WorkspaceToolRequest,
        output: &str,
        is_error: bool,
    ) -> Result<(), crate::agent::pipeline::StreamError> {
        crate::agent::pipeline::post_tool_result(
            &request.runtime_task_id,
            &request.call_id,
            output,
            is_error,
        )
    }

    /// Convert a parked tool request into durable workspace authority. The
    /// stable attention identity makes reconstruction deterministic, while the
    /// command identity remains unique for normal retry-safe command handling.
    pub fn consent_command(
        &self,
        request: &WorkspaceToolRequest,
        current_revision: u64,
        timestamp_unix_ms: i64,
    ) -> Result<WorkspaceCommandEnvelope, WorkspaceExecutionAdapterError> {
        if request.call_id.trim().is_empty()
            || request.runtime_task_id.trim().is_empty()
            || !self.accepted_tool_calls.contains(&request.call_id)
            || self.runtime_task_id.as_deref() != Some(request.runtime_task_id.as_str())
        {
            return Err(WorkspaceExecutionAdapterError::InvalidConsentRequest);
        }
        let launch = &self.request.launch;
        let attention_item = AttentionItem {
            id: AttentionItemId(uuid::Uuid::new_v5(
                &launch.attempt.id.0,
                request.call_id.as_bytes(),
            )),
            work_item_id: launch.work_item_id,
            task_id: Some(launch.attempt.task_id),
            thread_id: Some(task_coordination_thread_id(launch.attempt.task_id)),
            execution_attempt_id: Some(launch.attempt.id),
            tool_call_id: Some(request.call_id.clone()),
            kind: AttentionKind::Permission,
            status: AttentionStatus::Open,
            summary: request.consent_summary(),
            requested_by: launch.actor_id,
            assigned_to: vec![launch.lease.charging.owner_id],
            created_at_unix_ms: timestamp_unix_ms,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
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
            payload: WorkspaceCommandPayload::RaiseAttention { attention_item },
        })
    }

    pub fn cancel(&mut self) -> WorkspaceExecutionCompletion {
        if !self.terminal {
            if let Some(runtime_task_id) = &self.runtime_task_id {
                let _ = crate::agent::pipeline::cancel_local_task(runtime_task_id);
            }
            self.terminal = true;
        }
        WorkspaceExecutionCompletion::cancelled("Execution cancelled by the workspace authority.")
    }
}
