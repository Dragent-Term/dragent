//! Worker / reviewer coordination tools.
//!
//! Every attempt runs on an `AgentController` with these host tools so an
//! agent can talk to the Principal and to its peers WITHOUT the user acting
//! as the relay: `ask_principal` (blocks until the Principal — or, if it
//! escalates, the human — answers), `report_finding`, `handoff_to`,
//! `message_agent`, `read_knowledge`, `record_knowledge`.
//!
//! Everything is a canonical typed message or knowledge note on the work
//! item, so the Principal sees it through its own tools and Coordinate /
//! the agent panes show it. Communication never grants capability.

use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorId, ActorKind, ActorRef, AgentMessage, AgentMessageId,
    AgentMessageKind, AgentMessageRouting, ExecutionAttemptId, ExecutionAttemptStatus,
    MessageDeliveryState, PlanTaskId,
};
use uuid::Uuid;
use warpui::{ReadModel, UpdateModel, ViewContext};

use crate::agent::pipeline::HostToolDef;

use super::view::WorkspaceView;

fn schema(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": properties, "required": required })
}

pub(super) fn worker_host_tools() -> Vec<HostToolDef> {
    vec![
        HostToolDef {
            name: "ask_principal".into(),
            description: "Ask the Principal (your coordinator) for missing information, a decision, or authority. Blocks until the answer arrives — the Principal answers from the requirements, plan, other agents' state and shared knowledge, and escalates to the human itself only when it must. Use this instead of asking the user.".into(),
            parameters: schema(
                serde_json::json!({
                    "question": { "type": "string", "description": "The precise question, with the context needed to answer it" }
                }),
                &["question"],
            ),
            mutating: true,
        },
        HostToolDef {
            name: "report_finding".into(),
            description: "Report something to the Principal that affects the plan or other agents: an API behavior, a limitation, a root cause, a decision you had to make. Non-blocking.".into(),
            parameters: schema(
                serde_json::json!({ "text": { "type": "string" }, "record_as_knowledge": { "type": "boolean", "description": "Also write it to the shared knowledge ledger (default true)" } }),
                &["text"],
            ),
            mutating: true,
        },
        HostToolDef {
            name: "handoff_to".into(),
            description: "Hand a concrete artifact or responsibility to another role (by role name) with everything they need to continue. The Principal sees every handoff.".into(),
            parameters: schema(
                serde_json::json!({ "role": { "type": "string" }, "text": { "type": "string" } }),
                &["role", "text"],
            ),
            mutating: true,
        },
        HostToolDef {
            name: "message_agent".into(),
            description: "Send information directly to another role (by role name) when it affects their work — e.g. 'the auth endpoint now returns 204, not 200'. Delivered live if that agent is between steps, otherwise with its next turn.".into(),
            parameters: schema(
                serde_json::json!({ "role": { "type": "string" }, "text": { "type": "string" } }),
                &["role", "text"],
            ),
            mutating: true,
        },
        HostToolDef {
            name: "read_knowledge".into(),
            description: "Read the shared work-item knowledge ledger (decisions, discoveries, root causes, limitations, answered questions). Read it before researching something another agent may already know.".into(),
            parameters: schema(serde_json::json!({}), &[]),
            mutating: false,
        },
        HostToolDef {
            name: "record_knowledge".into(),
            description: "Append one durable note to the shared knowledge ledger so no agent rediscovers it.".into(),
            parameters: schema(
                serde_json::json!({
                    "kind": { "type": "string", "enum": ["decision", "approval", "preference", "change", "rejected", "note"] },
                    "text": { "type": "string" }
                }),
                &["kind", "text"],
            ),
            mutating: true,
        },
    ]
}

/// A worker question parked on the Principal's answer.
#[derive(Clone, Debug)]
pub(super) enum PendingWorkerQuestion {
    /// `ask_principal` tool call: answer = tool result.
    Tool {
        attempt_id: ExecutionAttemptId,
        runtime_task_id: String,
        call_id: String,
    },
    /// The agent's own `ask_user` card: answer = fill the card.
    Card {
        attempt_id: ExecutionAttemptId,
        action_id: Uuid,
    },
    /// `dragent work ask` from an agent CLI working on a task: answer =
    /// the command's reply (see `cli_workers`).
    #[cfg(not(target_family = "wasm"))]
    Cli {
        attempt_id: ExecutionAttemptId,
        request_id: String,
        asked_at_ms: i64,
    },
}

impl WorkspaceView {
    pub(super) fn attempt_actor(&self, attempt_id: ExecutionAttemptId) -> Option<(ActorRef, PlanTaskId)> {
        let attempt = self.workspace_state.execution_attempt(attempt_id)?;
        let kind = self
            .workspace_state
            .agent_instances()
            .into_iter()
            .find(|agent| agent.id == attempt.agent_instance_id)
            .and_then(|agent| {
                self.workspace_state
                    .role_definition(agent.role_definition_id)
            })
            .map(|role| match role.kind {
                dragent_workspace_model::AgentRoleKind::Reviewer => ActorKind::ReviewerAgent,
                _ => ActorKind::WorkerAgent,
            })
            .unwrap_or(ActorKind::WorkerAgent);
        Some((
            ActorRef::agent_instance(attempt.agent_instance_id, kind),
            attempt.task_id,
        ))
    }

    fn post_agent_message(
        &mut self,
        sender: &ActorRef,
        task_id: PlanTaskId,
        attempt_id: ExecutionAttemptId,
        kind: AgentMessageKind,
        recipients: Vec<ActorId>,
        body: String,
        ctx: &mut ViewContext<Self>,
    ) -> Result<AgentMessageId, String> {
        let (id, command) =
            self.agent_message_command(sender, task_id, attempt_id, kind, recipients, body);
        self.submit_workspace_runtime_command_retryable(command, ctx)?;
        Ok(id)
    }

    pub(super) fn agent_message_command(
        &self,
        sender: &ActorRef,
        task_id: PlanTaskId,
        attempt_id: ExecutionAttemptId,
        kind: AgentMessageKind,
        recipients: Vec<ActorId>,
        body: String,
    ) -> (AgentMessageId, WorkspaceCommandEnvelope) {
        let project_id = self.workspace_state.project().id;
        let work_item_id = self.workspace_work_item_id;
        let now = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage {
            id: AgentMessageId::new(),
            thread_id: task_coordination_thread_id(task_id),
            work_item_id,
            sender_id: sender.id,
            kind,
            body,
            review_finding: None,
            reply_to: None,
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::LocalOnly,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let id = message.id;
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: sender.id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message,
                routing: Some(AgentMessageRouting {
                    recipient_ids: recipients,
                    task_id: Some(task_id),
                    execution_attempt_id: Some(attempt_id),
                }),
            },
        };
        (id, command)
    }

    /// Resolve a role name to its live/most recent attempt (and the role's
    /// agent actor) for peer messaging and handoffs.
    pub(super) fn find_role_attempt(
        &self,
        role_needle: &str,
    ) -> Result<(ActorId, dragent_workspace_model::ExecutionAttempt), String> {
        let needle = role_needle.trim().to_ascii_lowercase();
        let mut candidates: Vec<(String, dragent_workspace_model::ExecutionAttempt)> = Vec::new();
        for attempt in self.workspace_state.execution_attempts() {
            let Some(agent) = self
                .workspace_state
                .agent_instances()
                .into_iter()
                .find(|agent| agent.id == attempt.agent_instance_id)
            else {
                continue;
            };
            let Some(role) = self
                .workspace_state
                .role_definition(agent.role_definition_id)
            else {
                continue;
            };
            if role.name.to_ascii_lowercase().contains(&needle) {
                candidates.push((role.name.clone(), attempt.clone()));
            }
        }
        let best = candidates
            .into_iter()
            .max_by_key(|(_, attempt)| {
                (
                    attempt.status == ExecutionAttemptStatus::Running,
                    attempt.started_at_unix_ms,
                )
            })
            .ok_or_else(|| {
                let roles = self
                    .workspace_state
                    .latest_proposed_team()
                    .map(|team| {
                        team.roles
                            .iter()
                            .filter_map(|role| {
                                self.workspace_state
                                    .role_definition(role.role_definition_id)
                            })
                            .map(|role| role.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                format!("No agent with a role matching `{role_needle}` has run yet. Roles: {roles}")
            })?;
        let kind = self
            .workspace_state
            .agent_instances()
            .into_iter()
            .find(|agent| agent.id == best.1.agent_instance_id)
            .and_then(|agent| {
                self.workspace_state
                    .role_definition(agent.role_definition_id)
            })
            .map(|role| match role.kind {
                dragent_workspace_model::AgentRoleKind::Reviewer => ActorKind::ReviewerAgent,
                _ => ActorKind::WorkerAgent,
            })
            .unwrap_or(ActorKind::WorkerAgent);
        Ok((
            ActorRef::agent_instance(best.1.agent_instance_id, kind).id,
            best.1,
        ))
    }

    /// Deliver `text` into a peer attempt's controller when it is mounted and
    /// idle between turns; otherwise it waits on the thread.
    fn deliver_live_to_attempt(
        &mut self,
        attempt: &dragent_workspace_model::ExecutionAttempt,
        from: &str,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if attempt.status != ExecutionAttemptStatus::Running {
            return false;
        }
        let Some((agent, request)) = self.attempt_agents.get(&attempt.id).cloned().zip(
            self.workspace_execution_runtime
                .mounted_request(attempt.id)
                .cloned(),
        ) else {
            return false;
        };
        let controller: warpui::ModelHandle<crate::agent::AgentController> = agent.read(
            ctx,
            |view: &crate::agent::conversation_view::AgentConversationView, _| {
                view.controller().clone()
            },
        );
        if controller.read(ctx, |c: &crate::agent::AgentController, _| c.is_busy()) {
            return false;
        }
        let system = request.system_prompt().to_string();
        let model = request.launch().attempt.model_route.clone();
        let query = format!("Message from {from}. Act on it within your Work Contract:\n\n{text}");
        ctx.update_model(&controller, |c, mctx| {
            c.start_internal_continuation(
                query,
                Some(format!("Message from {from}")),
                model,
                Box::new(move || system),
                mctx,
            );
        });
        true
    }

    /// Execute one worker host tool. `ask_principal` returns `None`: its
    /// result is posted later, when the Principal's reply lands.
    pub(super) fn execute_worker_tool(
        &mut self,
        attempt_id: ExecutionAttemptId,
        runtime_task_id: &str,
        call_id: &str,
        name: &str,
        args_json: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Option<(String, bool)> {
        let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or_default();
        let Some((sender, task_id)) = self.attempt_actor(attempt_id) else {
            return Some(("This attempt is no longer active.".into(), true));
        };
        let principal = ActorRef::principal_agent(self.workspace_state.project().id).id;
        let text = args["text"].as_str().unwrap_or_default().trim().to_string();
        match name {
            "ask_principal" => {
                let question = args["question"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if question.is_empty() {
                    return Some(("`question` is required.".into(), true));
                }
                // A DecisionRequest to the Principal triggers its coordination
                // turn; the reply is routed back to this call by message id.
                let (message_id, command) = self.agent_message_command(
                    &sender,
                    task_id,
                    attempt_id,
                    AgentMessageKind::DecisionRequest,
                    vec![principal],
                    question,
                );
                let command_id = command.command_id;
                match self.submit_workspace_runtime_question_with_acknowledgement(
                    command,
                    message_id,
                    runtime_task_id.to_string(),
                    call_id.to_string(),
                    ctx,
                ) {
                    Ok(()) => {}
                    Err(error) => return Some((error, true)),
                }
                self.pending_worker_questions.insert(
                    message_id,
                    PendingWorkerQuestion::Tool {
                        attempt_id,
                        runtime_task_id: runtime_task_id.to_string(),
                        call_id: call_id.to_string(),
                    },
                );
                self.pending_worker_question_submissions
                    .insert(command_id, message_id);
                None
            }
            "report_finding" => {
                if text.is_empty() {
                    return Some(("`text` is required.".into(), true));
                }
                let (_, message_command) = self.agent_message_command(
                    &sender,
                    task_id,
                    attempt_id,
                    AgentMessageKind::StatusUpdate,
                    vec![principal],
                    format!("Finding: {text}"),
                );
                let mut commands = vec![message_command];
                let records_knowledge = args["record_as_knowledge"].as_bool().unwrap_or(true);
                if records_knowledge {
                    commands.push(self.knowledge_note_command(&sender, "note", &text));
                }
                let success = if records_knowledge {
                    "Finding and knowledge note were committed atomically."
                } else {
                    "Finding was committed."
                };
                match self.submit_workspace_runtime_commands_with_acknowledgement(
                    commands,
                    runtime_task_id.to_string(),
                    call_id.to_string(),
                    success.into(),
                    ctx,
                ) {
                    Ok(()) => None,
                    Err(error) => Some((error, true)),
                }
            }
            "handoff_to" | "message_agent" => {
                let role = args["role"].as_str().unwrap_or_default();
                if text.is_empty() {
                    return Some(("`text` is required.".into(), true));
                }
                let (target_actor, _target_attempt) = match self.find_role_attempt(role) {
                    Ok(found) => found,
                    Err(error) => return Some((error, true)),
                };
                let (kind, body) = if name == "handoff_to" {
                    (AgentMessageKind::Handoff, format!("Handoff: {text}"))
                } else {
                    (AgentMessageKind::StatusUpdate, text.clone())
                };
                let (_, command) = self.agent_message_command(
                    &sender,
                    task_id,
                    attempt_id,
                    kind,
                    vec![target_actor, principal],
                    body,
                );
                let success = format!(
                    "Committed for `{role}` on its task thread; it will read the message from canonical state."
                );
                match self.submit_workspace_runtime_command_with_acknowledgement(
                    command,
                    runtime_task_id.to_string(),
                    call_id.to_string(),
                    success,
                    ctx,
                ) {
                    Ok(()) => None,
                    Err(error) => Some((error, true)),
                }
            }
            "read_knowledge" => Some((self.principal_read_knowledge(), false)),
            "record_knowledge" => {
                if text.is_empty() {
                    return Some(("`text` is required.".into(), true));
                }
                let kind = args["kind"].as_str().unwrap_or("note").to_string();
                match self.record_knowledge_note_with_acknowledgement(
                    &sender,
                    &kind,
                    &text,
                    runtime_task_id,
                    call_id,
                    ctx,
                ) {
                    Ok(()) => None,
                    Err(error) => Some((error, true)),
                }
            }
            other => Some((
                format!("Tool `{other}` is not available to this agent."),
                true,
            )),
        }
    }

    pub(super) fn record_knowledge_note(
        &mut self,
        actor: &ActorRef,
        kind: &str,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let command = self.knowledge_note_command(actor, kind, text);
        self.submit_workspace_runtime_command(command, ctx);
    }

    pub(super) fn knowledge_note_command(
        &self,
        actor: &ActorRef,
        kind: &str,
        text: &str,
    ) -> WorkspaceCommandEnvelope {
        let project_id = self.workspace_state.project().id;
        let work_item_id = self.workspace_work_item_id;
        let now = chrono::Utc::now().timestamp_millis();
        let note = dragent_workspace_model::ShapingMemoryNote {
            id: dragent_workspace_model::ShapingMemoryNoteId::new(),
            work_item_id,
            kind: dragent_workspace_model::ShapingMemoryKind::parse(kind),
            text: text.to_string(),
            recorded_at_unix_ms: now,
        };
        WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::RecordShapingMemory { notes: vec![note] },
        }
    }

    fn record_knowledge_note_with_acknowledgement(
        &mut self,
        actor: &ActorRef,
        kind: &str,
        text: &str,
        runtime_task_id: &str,
        call_id: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let command = self.knowledge_note_command(actor, kind, text);
        self.submit_workspace_runtime_command_with_acknowledgement(
            command,
            runtime_task_id.to_string(),
            call_id.to_string(),
            format!("Recorded [{kind}] {text}"),
            ctx,
        )
    }

    /// A Principal reply landed on a task thread: if it answers a parked
    /// worker question, deliver it — as the `ask_principal` tool result or
    /// as the answer to the agent's own question card.
    pub(super) fn deliver_principal_reply_to_worker(
        &mut self,
        reply: &AgentMessage,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(target) = reply.reply_to else {
            return;
        };
        let Some(pending) = self.pending_worker_questions.remove(&target) else {
            return;
        };
        let answer = reply.body.clone();
        match pending {
            PendingWorkerQuestion::Tool {
                runtime_task_id: _,
                call_id: _,
                ..
            } => {
                // The Principal reply transaction resolves the durable outbox
                // row for this DecisionRequest. Reconciliation delivers that
                // exact outcome; do not create a second in-memory completion.
                self.reconcile_workspace_tool_outcomes(ctx);
            }
            PendingWorkerQuestion::Card {
                attempt_id,
                action_id,
            } => {
                if let Some(agent) = self.attempt_agents.get(&attempt_id).cloned() {
                    let controller: warpui::ModelHandle<crate::agent::AgentController> = agent
                        .read(
                            ctx,
                            |view: &crate::agent::conversation_view::AgentConversationView, _| {
                                view.controller().clone()
                            },
                        );
                    ctx.update_model(&controller, |c, mctx| {
                        c.answer_user_question_text(action_id, answer, mctx);
                    });
                }
            }
            #[cfg(not(target_family = "wasm"))]
            PendingWorkerQuestion::Cli {
                attempt_id,
                request_id,
                asked_at_ms,
            } => self.answer_cli_worker_question(attempt_id, &request_id, asked_at_ms, reply, ctx),
        }
    }

    pub(super) fn fail_pending_worker_question(
        &mut self,
        message_id: AgentMessageId,
        error: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(pending) = self.pending_worker_questions.remove(&message_id) else {
            return;
        };
        let error = format!("Could not persist the question for the Principal: {error}");
        match pending {
            PendingWorkerQuestion::Tool {
                runtime_task_id,
                call_id,
                ..
            } => self.queue_workspace_tool_delivery(runtime_task_id, call_id, error, true, ctx),
            PendingWorkerQuestion::Card {
                attempt_id,
                action_id,
            } => {
                if let Some(agent) = self.attempt_agents.get(&attempt_id).cloned() {
                    let controller = agent.read(ctx, |view, _| view.controller().clone());
                    ctx.update_model(&controller, |controller, mctx| {
                        controller.answer_user_question_text(action_id, error, mctx)
                    });
                }
            }
            #[cfg(not(target_family = "wasm"))]
            PendingWorkerQuestion::Cli { request_id, .. } => crate::cli::inbox::CliInbox::respond(
                &request_id,
                &crate::cli::requests::Response::failed(error),
            ),
        }
    }
}
