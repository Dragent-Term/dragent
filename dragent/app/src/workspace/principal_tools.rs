//! The Principal's control-center tools.
//!
//! The Principal is an architect / principal engineer, not another coding
//! agent: it inspects, delegates, monitors, and decides. These are the tools
//! its conversation turns can call — executed by `WorkspaceView` against
//! canonical workspace state and the live controllers of every worker and
//! reviewer attempt — so "what is the frontend agent doing?", "is anyone
//! blocked?", "retry that task" and "tell the reviewer X" are answered from
//! real state, never from a stale dump pasted into the prompt.
//!
//! Every action goes through the same typed authority commands the UI uses
//! (retry = resolve the Failure attention item, a message = a canonical
//! thread message); nothing bypasses the scheduler.

use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorKind, ActorRef, AgentMessageId, AgentMessageKind,
    AgentMessageRouting, AttentionKind, AttentionResolution, AttentionStatus,
    ExecutionAttemptStatus, MessageDeliveryState, PlanTask, PlanTaskId, PlanTaskStatus, WorkItemId,
    WorkspaceState,
};
use warpui::{ReadModel, UpdateModel, ViewContext};

use crate::agent::pipeline::HostToolDef;

use super::view::WorkspaceView;

fn schema(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Tools the Principal may call in every conversation turn. Inspection
/// tools are non-mutating (they fold into the "Explored" activity group);
/// `message_agent`, `retry_task`, and `record_knowledge` act.
pub(super) fn principal_host_tools() -> Vec<HostToolDef> {
    vec![
        HostToolDef {
            name: "list_agents".into(),
            description: "List every plan task with its assigned role, canonical status, latest attempt, and — for a live agent — what it is doing right now (state, last tool step, open request). Use this first for any question about who is working on what, what is blocked, or what remains.".into(),
            parameters: schema(serde_json::json!({}), &[]),
            mutating: false,
        },
        HostToolDef {
            name: "inspect_agent".into(),
            description: "Detailed activity digest of one agent: its recent tool steps and results, task list, open permission/question, errors, last reply, files written, plus the messages, evidence, and attention tied to its task. `task` matches a task title, role name, or task id.".into(),
            parameters: schema(
                serde_json::json!({
                    "task": { "type": "string", "description": "Task title / role name / task id to inspect" },
                    "steps": { "type": "integer", "description": "How many recent tool steps to include (default 12)" }
                }),
                &["task"],
            ),
            mutating: false,
        },
        HostToolDef {
            name: "list_blockers".into(),
            description: "Open attention items (permission requests, questions, failures, budget, review) and unanswered Blocker/DecisionRequest messages across the work item — what needs a decision, and from whom.".into(),
            parameters: schema(serde_json::json!({}), &[]),
            mutating: false,
        },
        HostToolDef {
            name: "work_status".into(),
            description: "Definition-of-done view: task counts by status, acceptance criteria satisfied/failed/waived/pending with their descriptions, evidence count, and what remains before the work item can complete.".into(),
            parameters: schema(serde_json::json!({}), &[]),
            mutating: false,
        },
        HostToolDef {
            name: "read_knowledge".into(),
            description: "The shared work-item knowledge ledger: decisions, preferences, changes, rejected options, discoveries — recorded by you and by workers. Read it before re-researching anything.".into(),
            parameters: schema(serde_json::json!({}), &[]),
            mutating: false,
        },
        HostToolDef {
            name: "record_knowledge".into(),
            description: "Append one durable note to the shared knowledge ledger so no agent rediscovers it: a decision (with reason), a discovered API/behavior/limitation, a root cause, a handoff, a user answer.".into(),
            parameters: schema(
                serde_json::json!({
                    "kind": { "type": "string", "enum": ["decision", "approval", "preference", "change", "rejected", "note"] },
                    "text": { "type": "string", "description": "One sentence, self-contained" }
                }),
                &["kind", "text"],
            ),
            mutating: true,
        },
        HostToolDef {
            name: "message_agent".into(),
            description: "Send an instruction, correction, or piece of information to a worker or reviewer by task title / role name. A running agent receives it inside its current turn; an idle one gets it with its next attempt. Also recorded on the task's thread.".into(),
            parameters: schema(
                serde_json::json!({
                    "task": { "type": "string" },
                    "text": { "type": "string" }
                }),
                &["task", "text"],
            ),
            mutating: true,
        },
        HostToolDef {
            name: "retry_task".into(),
            description: "Retry a blocked independent review or authorize one more attempt for a failed task. Resolve the cause first with message_agent, record_knowledge or revise_task_instructions.".into(),
            parameters: schema(serde_json::json!({ "task": { "type": "string" } }), &["task"]),
            mutating: true,
        },
        HostToolDef {
            name: "revise_task_instructions".into(),
            description: "Clarify the instructions of an idle or failed task within its already-approved scope. Cannot change permissions, models, budgets, dependencies, acceptance criteria, or active/completed tasks. Use message_agent for a running task; ask the user for authority changes.".into(),
            parameters: schema(serde_json::json!({
                "task": {"type": "string"}, "description": {"type": "string"}
            }), &["task", "description"]),
            mutating: true,
        },
    ]
}

fn task_status_label(status: PlanTaskStatus) -> &'static str {
    match status {
        PlanTaskStatus::Proposed => "proposed (waiting for dependencies or a slot)",
        PlanTaskStatus::Ready => "ready to start",
        PlanTaskStatus::Running => "running",
        PlanTaskStatus::Blocked => "blocked",
        PlanTaskStatus::Reviewing => "awaiting review",
        PlanTaskStatus::Verifying => "verifying",
        PlanTaskStatus::Completed => "completed",
        PlanTaskStatus::Failed => "failed",
        PlanTaskStatus::Cancelled => "cancelled",
    }
}

fn role_name_for_task(state: &WorkspaceState, task: &PlanTask) -> String {
    task.assigned_contract_id
        .and_then(|id| state.work_contract(id))
        .and_then(|contract| state.role_definition(contract.role_definition_id))
        .map(|role| role.name.clone())
        .unwrap_or_else(|| "Unassigned".into())
}

/// Resolve `task` (title / role name / task id / plan key) to a plan task.
fn find_task<'a>(state: &'a WorkspaceState, needle: &str) -> Result<&'a PlanTask, String> {
    let plan = state
        .latest_structured_plan()
        .ok_or_else(|| "There is no plan yet.".to_string())?;
    let needle_lc = needle.trim().to_ascii_lowercase();
    if needle_lc.is_empty() {
        return Err("`task` must name a task title, role, or id.".into());
    }
    if let Some(task) = plan
        .tasks
        .iter()
        .find(|task| task.id.0.to_string().to_ascii_lowercase() == needle_lc)
    {
        return Ok(task);
    }
    let mut matches: Vec<&PlanTask> = plan
        .tasks
        .iter()
        .filter(|task| task.title.to_ascii_lowercase() == needle_lc)
        .collect();
    if matches.is_empty() {
        matches = plan
            .tasks
            .iter()
            .filter(|task| {
                task.title.to_ascii_lowercase().contains(&needle_lc)
                    || role_name_for_task(state, task)
                        .to_ascii_lowercase()
                        .contains(&needle_lc)
            })
            .collect();
    }
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err(format!(
            "No task matches `{needle}`. Tasks: {}",
            plan.tasks
                .iter()
                .map(|task| format!("`{}` ({})", task.title, role_name_for_task(state, task)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => {
            // Several tasks share a role: prefer the one with live/most recent work.
            let by_attempt = matches
                .iter()
                .copied()
                .max_by_key(|task| {
                    state
                        .execution_attempts()
                        .into_iter()
                        .filter(|attempt| attempt.task_id == task.id)
                        .map(|attempt| attempt.started_at_unix_ms)
                        .max()
                        .unwrap_or(0)
                })
                .unwrap();
            Ok(by_attempt)
        }
    }
}

impl WorkspaceView {
    /// Execute one Principal host tool for `work_item_id` and return the
    /// tool result text (`is_error` true for rejections).
    pub(super) fn execute_principal_tool(
        &mut self,
        work_item_id: WorkItemId,
        runtime_task_id: &str,
        call_id: &str,
        name: &str,
        args_json: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Option<(String, bool)> {
        if work_item_id != self.workspace_work_item_id {
            return Some((
                "This tool can only inspect the active work item right now; switch to its tab."
                    .into(),
                true,
            ));
        }
        let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or_default();
        if name == "record_knowledge" {
            return match self.principal_record_knowledge(&args, runtime_task_id, call_id, ctx) {
                Ok(()) => None,
                Err(error) => Some((error, true)),
            };
        }
        if name == "message_agent" {
            return match self.principal_message_agent(&args, runtime_task_id, call_id, ctx) {
                Ok(()) => None,
                Err(error) => Some((error, true)),
            };
        }
        if name == "revise_task_instructions" {
            return match self.principal_revise_task_instructions(
                &args,
                runtime_task_id,
                call_id,
                ctx,
            ) {
                Ok(()) => None,
                Err(error) => Some((error, true)),
            };
        }
        if name == "retry_task" {
            return match self.principal_retry_task(&args, runtime_task_id, call_id, ctx) {
                Ok(Some(result)) => Some((result, false)),
                Ok(None) => None,
                Err(error) => Some((error, true)),
            };
        }
        let result = match name {
            "list_agents" => Ok(self.principal_list_agents(ctx)),
            "inspect_agent" => self.principal_inspect_agent(&args, ctx),
            "list_blockers" => Ok(self.principal_list_blockers()),
            "work_status" => Ok(self.principal_work_status()),
            "read_knowledge" => Ok(self.principal_read_knowledge()),
            other => Err(format!("Unknown Principal tool `{other}`.")),
        };
        match result {
            Ok(text) => Some((text, false)),
            Err(text) => Some((text, true)),
        }
    }

    fn latest_attempt_for(
        &self,
        task_id: PlanTaskId,
    ) -> Option<&dragent_workspace_model::ExecutionAttempt> {
        self.workspace_state
            .execution_attempts()
            .into_iter()
            .filter(|attempt| attempt.task_id == task_id)
            .max_by_key(|attempt| attempt.attempt_number)
    }

    fn live_digest(
        &self,
        attempt_id: dragent_workspace_model::ExecutionAttemptId,
        steps: usize,
        ctx: &ViewContext<Self>,
    ) -> Option<String> {
        let agent = self.attempt_agents.get(&attempt_id)?;
        let controller: warpui::ModelHandle<crate::agent::AgentController> = agent.read(
            ctx,
            |view: &crate::agent::conversation_view::AgentConversationView, _| {
                view.controller().clone()
            },
        );
        Some(
            controller.read(ctx, |controller: &crate::agent::AgentController, _| {
                controller.activity_digest(steps)
            }),
        )
    }

    fn principal_list_agents(&self, ctx: &ViewContext<Self>) -> String {
        let Some(plan) = self.workspace_state.latest_structured_plan() else {
            return "There is no plan yet — nothing has been delegated.".into();
        };
        let mut out = Vec::new();
        for task in &plan.tasks {
            let role = role_name_for_task(&self.workspace_state, task);
            let mut line = format!(
                "• `{}` — role: {} — status: {}",
                task.title,
                role,
                task_status_label(task.status)
            );
            if let Some(attempt) = self.latest_attempt_for(task.id) {
                line.push_str(&format!(
                    " — attempt {} {:?} on {}",
                    attempt.attempt_number, attempt.status, attempt.model_route
                ));
                if attempt.status == ExecutionAttemptStatus::Running {
                    if let Some(digest) = self.live_digest(attempt.id, 2, ctx) {
                        // First line is the live state; the last step follows.
                        let mut lines = digest.lines();
                        if let Some(state) = lines.next() {
                            line.push_str(&format!("\n    live {state}"));
                        }
                        for extra in lines
                            .filter(|l| {
                                l.starts_with("- ")
                                    || l.starts_with("OPEN")
                                    || l.starts_with("ERROR")
                            })
                            .take(3)
                        {
                            line.push_str(&format!("\n    {extra}"));
                        }
                    }
                }
            }
            if !task.depends_on.is_empty() {
                let deps = task
                    .depends_on
                    .iter()
                    .filter_map(|id| plan.tasks.iter().find(|t| t.id == *id))
                    .map(|t| format!("`{}` ({})", t.title, task_status_label(t.status)))
                    .collect::<Vec<_>>()
                    .join(", ");
                line.push_str(&format!("\n    depends on: {deps}"));
            }
            out.push(line);
        }
        let active = self
            .workspace_state
            .execution_attempts()
            .iter()
            .filter(|attempt| attempt.status == ExecutionAttemptStatus::Running)
            .count();
        out.insert(
            0,
            format!("{} task(s), {} running now.", plan.tasks.len(), active),
        );
        out.join("\n")
    }

    fn principal_inspect_agent(
        &self,
        args: &serde_json::Value,
        ctx: &ViewContext<Self>,
    ) -> Result<String, String> {
        let needle = args["task"].as_str().unwrap_or_default();
        let steps = args["steps"].as_u64().unwrap_or(12) as usize;
        let task = find_task(&self.workspace_state, needle)?.clone();
        let role = role_name_for_task(&self.workspace_state, &task);
        let mut out = vec![format!(
            "Task `{}` — role {} — status {}\nDescription: {}",
            task.title,
            role,
            task_status_label(task.status),
            task.description
        )];
        match self.latest_attempt_for(task.id) {
            None => out.push("No execution attempt yet.".into()),
            Some(attempt) => {
                out.push(format!(
                    "Latest attempt {} — {:?} on {}",
                    attempt.attempt_number, attempt.status, attempt.model_route
                ));
                if let Some(summary) = &attempt.change_summary {
                    out.push(format!(
                        "Changed files: {}",
                        summary
                            .files
                            .iter()
                            .map(|f| f.relative_path.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                match self.live_digest(attempt.id, steps, ctx) {
                    Some(digest) => out.push(format!("Agent activity:\n{digest}")),
                    None => out.push("No live transcript for this attempt (ran in a previous session or before controller-run execution).".into()),
                }
            }
        }
        let messages: Vec<String> = self
            .workspace_state
            .agent_messages()
            .into_iter()
            .filter(|(_, routing)| routing.task_id == Some(task.id))
            .map(|(message, _)| {
                format!(
                    "- {:?}: {}",
                    message.kind,
                    message
                        .body
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .collect();
        if !messages.is_empty() {
            out.push(format!("Thread messages:\n{}", messages.join("\n")));
        }
        let evidence: Vec<String> = self
            .workspace_state
            .evidence()
            .into_iter()
            .filter(|evidence| evidence.task_id == Some(task.id))
            .map(|evidence| format!("- {:?}: {}", evidence.kind, evidence.summary))
            .collect();
        if !evidence.is_empty() {
            out.push(format!("Evidence:\n{}", evidence.join("\n")));
        }
        let attention: Vec<String> = self
            .workspace_state
            .attention_items()
            .into_iter()
            .filter(|item| item.task_id == Some(task.id))
            .map(|item| format!("- {:?} [{:?}]: {}", item.kind, item.status, item.summary))
            .collect();
        if !attention.is_empty() {
            out.push(format!("Attention:\n{}", attention.join("\n")));
        }
        Ok(out.join("\n\n"))
    }

    fn principal_list_blockers(&self) -> String {
        let mut out = Vec::new();
        for item in self.workspace_state.attention_items() {
            if !matches!(
                item.status,
                AttentionStatus::Open | AttentionStatus::Acknowledged
            ) {
                continue;
            }
            let task = item
                .task_id
                .and_then(|id| {
                    self.workspace_state
                        .latest_structured_plan()
                        .and_then(|plan| plan.tasks.iter().find(|task| task.id == id))
                })
                .map(|task| format!(" (task `{}`)", task.title))
                .unwrap_or_default();
            out.push(format!("- {:?}{task}: {}", item.kind, item.summary));
        }
        let principal = ActorRef::principal_agent(self.workspace_state.project().id).id;
        for (message, routing) in self.workspace_state.agent_messages() {
            if !matches!(
                message.kind,
                AgentMessageKind::Blocker | AgentMessageKind::DecisionRequest
            ) || !routing.recipient_ids.contains(&principal)
            {
                continue;
            }
            let answered = self
                .workspace_state
                .agent_messages()
                .iter()
                .any(|(reply, _)| reply.reply_to == Some(message.id));
            if answered {
                continue;
            }
            out.push(format!(
                "- unanswered {:?} to you: {}",
                message.kind,
                message
                    .body
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        if out.is_empty() {
            "Nothing is blocked: no open attention items and no unanswered blockers.".into()
        } else {
            out.join("\n")
        }
    }

    fn principal_work_status(&self) -> String {
        let mut out = Vec::new();
        if let Some(work) = self.workspace_state.active_work() {
            out.push(format!("Work item `{}` — {:?}", work.title, work.status));
        }
        if let Some(plan) = self.workspace_state.latest_structured_plan() {
            let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
            for task in &plan.tasks {
                *counts.entry(task_status_label(task.status)).or_default() += 1;
            }
            out.push(format!(
                "Tasks: {}",
                counts
                    .iter()
                    .map(|(k, v)| format!("{v} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let remaining: Vec<String> = plan
                .tasks
                .iter()
                .filter(|task| {
                    !matches!(
                        task.status,
                        PlanTaskStatus::Completed | PlanTaskStatus::Cancelled
                    )
                })
                .map(|task| format!("- `{}` ({})", task.title, task_status_label(task.status)))
                .collect();
            if !remaining.is_empty() {
                out.push(format!("Remaining:\n{}", remaining.join("\n")));
            }
        }
        if let Some(records) = self.workspace_state.shaping_records_projection() {
            let criteria: Vec<String> = records
                .acceptance_criteria
                .iter()
                .map(|criterion| format!("- [{:?}] {}", criterion.status, criterion.description))
                .collect();
            if !criteria.is_empty() {
                out.push(format!("Acceptance criteria:\n{}", criteria.join("\n")));
            }
        }
        out.push(format!(
            "Evidence recorded: {}",
            self.workspace_state.evidence().len()
        ));
        out.join("\n\n")
    }

    pub(super) fn principal_read_knowledge(&self) -> String {
        let notes = self.workspace_state.shaping_memory();
        if notes.is_empty() {
            return "The knowledge ledger is empty.".into();
        }
        notes
            .iter()
            .map(|note| format!("- [{:?}] {}", note.kind, note.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn principal_record_knowledge(
        &mut self,
        args: &serde_json::Value,
        runtime_task_id: &str,
        call_id: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let text = args["text"].as_str().unwrap_or_default().trim().to_string();
        if text.is_empty() {
            return Err("`text` is required.".into());
        }
        let kind = args["kind"].as_str().unwrap_or("note");
        let work_item_id = self.workspace_work_item_id;
        let project_id = self.workspace_state.project().id;
        let principal = ActorRef::principal_agent(project_id);
        let now = chrono::Utc::now().timestamp_millis();
        let note = dragent_workspace_model::ShapingMemoryNote {
            id: dragent_workspace_model::ShapingMemoryNoteId::new(),
            work_item_id,
            kind: dragent_workspace_model::ShapingMemoryKind::parse(kind),
            text: text.clone(),
            recorded_at_unix_ms: now,
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: principal.id,
            device_id: principal.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::RecordShapingMemory { notes: vec![note] },
        };
        self.submit_workspace_runtime_command_with_acknowledgement(
            command,
            runtime_task_id.to_string(),
            call_id.to_string(),
            format!("Recorded [{kind}] {text}"),
            ctx,
        )
    }

    fn principal_message_agent(
        &mut self,
        args: &serde_json::Value,
        runtime_task_id: &str,
        call_id: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let needle = args["task"].as_str().unwrap_or_default();
        let text = args["text"].as_str().unwrap_or_default().trim().to_string();
        if text.is_empty() {
            return Err("`text` is required.".into());
        }
        let task = find_task(&self.workspace_state, needle)?.clone();
        let project_id = self.workspace_state.project().id;
        let principal = ActorRef::principal_agent(project_id);
        let now = chrono::Utc::now().timestamp_millis();
        let attempt = self.latest_attempt_for(task.id).cloned();
        // Canonical record on the task's thread — the next attempt's context
        // includes it, and Coordinate / the agent pane show it.
        let recipients = attempt
            .as_ref()
            .map(|attempt| {
                vec![ActorRef::agent_instance(attempt.agent_instance_id, ActorKind::WorkerAgent).id]
            })
            .unwrap_or_default();
        if recipients.is_empty() {
            return Err("That task has no agent attempt to receive a canonical message.".into());
        }
        let role = role_name_for_task(&self.workspace_state, &task);
        let message = dragent_workspace_model::AgentMessage {
            id: AgentMessageId::new(),
            thread_id: task_coordination_thread_id(task.id),
            work_item_id: task.plan_id_work_item(&self.workspace_state),
            sender_id: principal.id,
            kind: AgentMessageKind::StatusUpdate,
            body: text,
            review_finding: None,
            reply_to: None,
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::LocalOnly,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id: message.work_item_id,
            actor_id: principal.id,
            device_id: principal.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message,
                routing: Some(AgentMessageRouting {
                    recipient_ids: recipients,
                    task_id: Some(task.id),
                    execution_attempt_id: attempt.as_ref().map(|attempt| attempt.id),
                }),
            },
        };
        self.submit_workspace_runtime_command_with_acknowledgement(
            command,
            runtime_task_id.to_string(),
            call_id.to_string(),
            format!("Committed to `{}`'s thread ({role}).", task.title),
            ctx,
        )
    }

    fn principal_revise_task_instructions(
        &mut self,
        args: &serde_json::Value,
        runtime_task_id: &str,
        call_id: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        if self.workspace_launch_pending {
            return Err("Workspace state is being committed; retry this tool shortly.".into());
        }
        let task = find_task(
            &self.workspace_state,
            args["task"].as_str().unwrap_or_default(),
        )?
        .clone();
        let current = self
            .workspace_state
            .latest_structured_plan()
            .ok_or_else(|| "There is no approved plan.".to_string())?;
        let mut plan = current.clone();
        plan.version = plan.version.saturating_add(1);
        plan.updated_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let revised = plan
            .tasks
            .iter_mut()
            .find(|item| item.id == task.id)
            .unwrap();
        revised.description = args["description"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        dragent_workspace_model::validate_principal_plan_refinement(current, &plan)?;
        let principal = ActorRef::principal_agent(self.workspace_state.project().id);
        self.submit_workspace_runtime_command_with_acknowledgement(
            WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: self.workspace_state.project().id,
                work_item_id: plan.work_item_id,
                actor_id: principal.id,
                device_id: None,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: plan.updated_at_unix_ms,
                expected_revision: Some(self.workspace_canonical_revision),
                payload: WorkspaceCommandPayload::UpsertPlan { plan },
            },
            runtime_task_id.to_string(),
            call_id.to_string(),
            format!("Committed instruction refinement for `{}`.", task.title),
            ctx,
        )
    }

    fn principal_retry_task(
        &mut self,
        args: &serde_json::Value,
        runtime_task_id: &str,
        call_id: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<Option<String>, String> {
        let needle = args["task"].as_str().unwrap_or_default();
        let task = find_task(&self.workspace_state, needle)?.clone();
        // A blocked review has succeeded as a provider turn but has not
        // verified its candidate. Let the Principal retry that inspection
        // after resolving the cause, without marking the review accepted.
        if task.status == PlanTaskStatus::Reviewing && task.review_target_task_id.is_some() {
            if self.workspace_launch_pending {
                return Err("Workspace state is being committed; retry this tool shortly.".into());
            }
            let attempt = self
                .latest_attempt_for(task.id)
                .cloned()
                .ok_or_else(|| "No review attempt is available.".to_string())?;
            let blocked = self
                .workspace_state
                .agent_messages()
                .iter()
                .any(|(message, routing)| {
                    routing.execution_attempt_id == Some(attempt.id)
                        && message.review_finding.as_ref().is_some_and(|finding| {
                            finding.verdict == dragent_workspace_model::ReviewVerdict::Blocked
                        })
                });
            if !blocked {
                return Err("Only a blocked review can be explicitly retried; ordinary review decisions are automatic.".into());
            }
            let principal = ActorRef::principal_agent(self.workspace_state.project().id);
            let command = WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id: self.workspace_state.project().id,
                work_item_id: attempt.work_item_id,
                actor_id: principal.id,
                device_id: principal.device_id,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
                expected_revision: Some(self.workspace_canonical_revision),
                payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                    attempt_id: attempt.id,
                    resolution:
                        dragent_workspace_model::ExecutionReviewResolution::RevisionRequested,
                },
            };
            self.submit_workspace_runtime_command_with_acknowledgement(
                command,
                runtime_task_id.to_string(),
                call_id.to_string(),
                format!(
                    "Committed another independent review request for `{}`.",
                    task.title
                ),
                ctx,
            )?;
            return Ok(None);
        }
        if task.status != PlanTaskStatus::Failed {
            return Err(format!(
                "`{}` is {} — only a failed task can be retried.",
                task.title,
                task_status_label(task.status)
            ));
        }
        let attempt = self
            .latest_attempt_for(task.id)
            .cloned()
            .ok_or_else(|| "That task has no attempt to retry.".to_string())?;
        let failure = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| {
                item.kind == AttentionKind::Failure
                    && item.execution_attempt_id == Some(attempt.id)
                    && matches!(
                        item.status,
                        AttentionStatus::Open | AttentionStatus::Acknowledged
                    )
            })
            .cloned();
        match failure {
                Some(item) => {
                    let principal = ActorRef::principal_agent(self.workspace_state.project().id);
                    let command = WorkspaceCommandEnvelope {
                        command_id: WorkspaceCommandId::new(),
                        schema_version: WORKSPACE_PROTOCOL_VERSION,
                        project_id: self.workspace_state.project().id,
                        work_item_id: item.work_item_id,
                        actor_id: principal.id,
                        device_id: principal.device_id,
                        correlation_id: CorrelationId::new(),
                        timestamp_unix_ms: chrono::Utc::now()
                            .timestamp_millis()
                            .max(item.created_at_unix_ms),
                        expected_revision: Some(self.workspace_canonical_revision),
                        payload: WorkspaceCommandPayload::ResolveAttention {
                            attention_item_id: item.id,
                            outcome: AttentionResolution::Answered,
                            resolution: format!(
                                "The Principal authorized one more attempt for `{}`.",
                                task.title
                            ),
                        },
                    };
                    self.submit_workspace_runtime_command_with_acknowledgement(
                        command,
                        runtime_task_id.to_string(),
                        call_id.to_string(),
                        format!("Authorized one more attempt for `{}`.", task.title),
                        ctx,
                    )?;
                    Ok(None)
                }
                None => Ok(Some(format!(
                    "`{}` is still inside its automatic retry window (attempt {} of {}); a new attempt starts on its own.",
                    task.title,
                    attempt.attempt_number,
                    dragent_workspace_model::MAX_AUTOMATIC_EXECUTION_ATTEMPTS
                ))),
            }
    }
}

trait PlanTaskWorkItem {
    fn plan_id_work_item(&self, state: &WorkspaceState) -> WorkItemId;
}

impl PlanTaskWorkItem for PlanTask {
    fn plan_id_work_item(&self, state: &WorkspaceState) -> WorkItemId {
        state
            .active_work()
            .map(|work| work.id)
            .unwrap_or_else(|| WorkItemId(uuid::Uuid::nil()))
    }
}
