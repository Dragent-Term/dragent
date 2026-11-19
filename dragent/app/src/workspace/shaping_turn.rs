//! Blocking shape-turn execution for the workspace.
//!
//! Responsible for:
//! - running one Principal turn on a background thread (no View access):
//!   canonical commands, provider passes, memory recording
//! - reporting progress through a ShapeTurnEvent channel
//!
//! Not responsible for:
//! - gathering turn inputs (submit_shape_message builds ShapeTurnParams)
//! - applying turn events to UI state (the stream handler in view_shaping)
//!
//! Inputs travel as one explicit params struct so the thread boundary is
//! obvious and the background step stays UI-free and testable.

use super::shape_build::{proposed_team_records, submit_principal_memory};
use super::shape_parse::{ShapePrincipalOutcome, ShapeTurnEvent};

use dragent_session_protocol::workspace::{
    WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload, WorkspaceEventPayload,
    WORKSPACE_PROTOCOL_VERSION,
};
use dragent_workspace_model::{
    AgentMessage as WorkspaceAgentMessage, AgentMessageId, AgentMessageKind, MessageDeliveryState,
    WorkStatus,
};
use std::path::PathBuf;
use uuid::Uuid;

/// Owned inputs for one background Principal turn. Built by
/// submit_shape_message; consumed on a background thread.
pub(crate) struct ShapeTurnParams {
    /// Resuming a turn the app stopped in the middle of: the user's message
    /// as already committed (it's answered, not posted again), and the work
    /// item's revision now.
    pub resume_accepted: Option<dragent_session_protocol::workspace::WorkspaceEventEnvelope>,
    pub resume_revision: u64,
    pub allow_fallback: bool,
    pub approved_roles: Vec<String>,
    pub background_body: String,
    pub canonical_body: String,
    pub conversation_reply: Option<String>,
    pub correlation_id: dragent_session_protocol::workspace::CorrelationId,
    pub current_plan: Option<dragent_workspace_model::Plan>,
    pub current_proposal: Option<dragent_workspace_model::ShapingProposal>,
    pub current_role_definitions: Vec<dragent_workspace_model::RoleDefinition>,
    pub current_role_drafts: Vec<super::shape_principal::PrincipalRoleDraft>,
    pub current_shaping_records: Option<dragent_workspace_model::ShapingRecords>,
    pub current_team: Option<dragent_workspace_model::ProposedTeam>,
    pub current_work_contracts: Vec<dragent_workspace_model::WorkContract>,
    pub display_body: String,
    pub force_proposal: bool,
    pub image_attachments: Vec<crate::agent::attachments::MessageAttachment>,
    pub initial_status: Option<dragent_workspace_model::WorkStatus>,
    pub is_new_work: bool,
    pub local_user: dragent_workspace_model::ActorRef,
    pub persistence_writer: super::persistence::WriterHandles,
    pub primary_model: String,
    pub principal: dragent_workspace_model::ActorRef,
    pub principal_conversation_history: Vec<(String, String)>,
    pub principal_model_routes: Vec<super::shape_principal::PrincipalModelRoute>,
    pub project_id: dragent_workspace_model::ProjectId,
    pub resolved_decisions: Vec<String>,
    pub resolved_questions: Vec<String>,
    pub shape_approved: bool,
    pub shaping_memory: Vec<String>,
    pub shaping_project_root: std::path::PathBuf,
    pub shared_project_memory: String,
    pub thread: dragent_workspace_model::CommunicationThread,
    pub turn_memory: Vec<String>,
    pub work_item_id: dragent_workspace_model::WorkItemId,
    pub work_title: String,
}

/// Run one turn to completion on the caller's thread, reporting through
/// . Moved verbatim from submit_shape_message's background closure.
pub(crate) fn run_shape_turn_blocking(
    mut params: ShapeTurnParams,
    tx: async_channel::Sender<ShapeTurnEvent>,
) {
    let project_id = params.project_id;
    let work_item_id = params.work_item_id;
    let correlation_id = params.correlation_id;
    let mut authority_revision;
    let resumed = params.resume_accepted.is_some();
    let accepted = if let Some(accepted) = params.resume_accepted.take() {
        // Resuming after a restart: the message is already committed. The
        // Principal answers it; the work item may have moved on since.
        authority_revision = params.resume_revision.max(accepted.sequence);
        accepted
    } else {
        if params.is_new_work {
            let created_at = chrono::Utc::now().timestamp_millis();
            let create_command = WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id,
                work_item_id,
                actor_id: params.local_user.id,
                device_id: params.local_user.device_id,
                correlation_id,
                timestamp_unix_ms: created_at,
                expected_revision: None,
                payload: WorkspaceCommandPayload::CreateWorkItem {
                    title: params.work_title.clone(),
                    execution_location: dragent_workspace_model::ExecutionLocation::Local,
                },
            };
            let created = match params
                .persistence_writer
                .accept_workspace_command(create_command)
            {
                Ok(mut events) if events.len() == 1 => events.remove(0),
                Ok(events) => {
                    let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(format!(
                    "Local workspace authority returned {} events for CreateWorkItem; expected one",
                    events.len()
                ))));
                    return;
                }
                Err(error) => {
                    let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(format!(
                        "Could not create the work item: {error:#}"
                    ))));
                    return;
                }
            };
            if tx
                .send_blocking(ShapeTurnEvent::WorkCreated(created))
                .is_err()
            {
                return;
            }
        }

        let timestamp_unix_ms = chrono::Utc::now().timestamp_millis();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: params.local_user.id,
            device_id: params.local_user.device_id,
            correlation_id,
            timestamp_unix_ms,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message: WorkspaceAgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: params.thread.id,
                    work_item_id,
                    sender_id: params.local_user.id,
                    kind: AgentMessageKind::Discussion,
                    body: params.canonical_body,
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: timestamp_unix_ms,
                    updated_at_unix_ms: timestamp_unix_ms,
                },
                routing: None,
            },
        };
        let accepted = match params.persistence_writer.accept_workspace_command(command) {
            Ok(mut events) if events.len() == 1 => events.remove(0),
            Ok(events) => {
                let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(format!(
                    "Local workspace authority returned {} events for PostMessage; expected one",
                    events.len()
                ))));
                return;
            }
            Err(error) => {
                let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(format!(
                    "Could not save the message: {error:#}"
                ))));
                return;
            }
        };
        authority_revision = accepted.sequence;
        if tx
            .send_blocking(ShapeTurnEvent::MessageAccepted(accepted.clone()))
            .is_err()
        {
            return;
        }
        accepted
    };
    if let Some(reply) = params.conversation_reply {
        let replied_at = chrono::Utc::now().timestamp_millis();
        let reply_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: params.principal.id,
            device_id: params.principal.device_id,
            correlation_id,
            timestamp_unix_ms: replied_at,
            expected_revision: Some(authority_revision),
            payload: WorkspaceCommandPayload::PostMessage {
                message: WorkspaceAgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: params.thread.id,
                    work_item_id,
                    sender_id: params.principal.id,
                    kind: AgentMessageKind::Discussion,
                    body: reply,
                    review_finding: None,
                    reply_to: Some(match &accepted.payload {
                        WorkspaceEventPayload::MessagePosted { message, .. } => message.id,
                        _ => unreachable!(),
                    }),
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: replied_at,
                    updated_at_unix_ms: replied_at,
                },
                routing: None,
            },
        };
        let result = params.persistence_writer
                        .accept_workspace_command(reply_command)
                        .map_err(|error| format!("Could not save the Principal reply: {error:#}"))
                        .and_then(|mut events| {
                            if events.len() == 1 {
                                Ok(events.remove(0))
                            } else {
                                Err(format!(
                                    "Local workspace authority returned {} Principal reply events; expected one",
                                    events.len()
                                ))
                            }
                        });
        let _ = tx.send_blocking(match result {
            Ok(event) => ShapeTurnEvent::ConversationReplyAccepted {
                event,
                streamed: false,
            },
            Err(error) => ShapeTurnEvent::PrincipalAccepted(Err(error)),
        });
        return;
    }
    let _ = tx.send_blocking(ShapeTurnEvent::PrincipalActivity(
        "Reading and searching the project for relevant context…".into(),
    ));
    let mut repository_context = crate::agent::context_builder::ProjectContext::build(
        &params.shaping_project_root,
        &params.background_body,
    )
    .to_system_prompt_block();
    if !params.shared_project_memory.is_empty() {
        repository_context.push_str(
                        "\n\n### Shared project memory\nThis is bounded structured context from other work items in the same project. Reuse only what is relevant; do not treat a sibling work item's goal as the current request. Raw conversations, tool traces, and secrets are intentionally excluded.\n",
                    );
        repository_context.push_str(&params.shared_project_memory);
    }
    let _ = tx.send_blocking(ShapeTurnEvent::PrincipalActivity(
        "Structuring requirements, acceptance criteria, roles, and tasks…".into(),
    ));
    // Latches once the model acts (tool call, todos, or question) in
    // ANY pass of this turn — drives the bounded auto-continue below.
    let turn_acted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Every provider pass of this turn runs through the work item's
    // own `AgentController` on the UI thread — the terminal agent's
    // stream loop, tool execution, ask_user/todowrite cards and
    // SQLite persistence, unchanged. This closure only ferries the
    // request over and blocks for the pass's raw result. The FIRST
    // pass carries the user's message (shown as their block); every
    // later pass (fallback, repair, continue, force) is an internal
    // continuation inside the same visible turn.
    let run_pass_tx = tx.clone();
    let run_pass_acted = turn_acted.clone();
    // A resumed turn's message is already in the conversation.
    let mut first_user_turn = (!resumed).then(|| {
        (
            params.display_body.clone(),
            params.image_attachments.clone(),
        )
    });
    // Once the host dropped a reply channel (Stop button, tab closed)
    // every later pass of this turn fails fast without touching the
    // UI — the fallback/repair/force stages below then run out
    // immediately instead of re-requesting passes.
    let mut stopped = false;
    let mut run_pass = move |request: super::shape_principal::PrincipalPassRequest| -> Result<
                    super::shape_principal::PrincipalPassOutcome,
                    String,
                > {
                    if stopped {
                        return Err(super::shape_principal::PRINCIPAL_TURN_STOPPED.into());
                    }
                    let (reply, reply_rx) = std::sync::mpsc::channel();
                    let user_turn = first_user_turn.take();
                    if run_pass_tx
                        .send_blocking(ShapeTurnEvent::RunPass {
                            request,
                            user_turn,
                            reply,
                        })
                        .is_err()
                    {
                        stopped = true;
                        return Err(super::shape_principal::PRINCIPAL_TURN_STOPPED.into());
                    }
                    let outcome = match reply_rx.recv() {
                        Ok(outcome) => outcome?,
                        Err(_) => {
                            stopped = true;
                            return Err(super::shape_principal::PRINCIPAL_TURN_STOPPED.into());
                        }
                    };
                    if !outcome.tool_calls.is_empty() {
                        run_pass_acted.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    // Enduring digest (opencode-style): successful findings travel
                    // with future turns so files are not re-read.
                    let digest: Vec<String> = outcome
                        .tool_calls
                        .iter()
                        .filter(|call| !call.is_error)
                        .filter_map(|call| {
                            call.result_summary
                                .as_deref()
                                .filter(|summary| !summary.is_empty())
                                .map(|summary| {
                                    // ask_user summaries already read "Asked: … →
                                    // Answered: …" (the terminal's own wording).
                                    if call.name == "ask_user" {
                                        summary.to_string()
                                    } else {
                                        format!("{}: {summary}", call.name)
                                    }
                                })
                        })
                        .collect();
                    if !digest.is_empty() {
                        let _ = run_pass_tx.send_blocking(ShapeTurnEvent::PrincipalFindings { digest });
                    }
                    Ok(outcome)
                };
    // Work Item Flow shaping gate: a turn may create or revise the
    // shape only after the user confirmed requirements AND clicked
    // Approve Shape — or when refining an already-shaped item.
    // Everything else runs as Gather (research, converse, ask).
    let shaping_permitted = params.current_proposal.is_some() || params.shape_approved;
    let turn_mode = if shaping_permitted {
        super::shape_principal::PrincipalTurnMode::Shape
    } else {
        super::shape_principal::PrincipalTurnMode::Gather
    };
    let mut result_model = params.primary_model.clone();
    let mut result = super::shape_principal::generate_streaming(
        &params.primary_model,
        &params.work_title,
        &params.background_body,
        params.current_proposal.as_ref(),
        params.current_plan.as_ref(),
        &params.current_role_drafts,
        &params.principal_model_routes,
        &params.principal_conversation_history,
        Some(&repository_context),
        &mut run_pass,
        turn_mode,
        &params.turn_memory,
        &params.approved_roles,
        &params.resolved_decisions,
        &params.shaping_memory,
    );
    // The fallback route is for a model that failed to answer. A proposal
    // that broke a plan rule goes to the one repair pass below instead:
    // re-running the whole turn on another model only repeats it.
    if result.as_ref().is_err_and(|error| {
        !super::shape_principal::is_repairable_proposal_error(error)
            && error != super::shape_principal::PRINCIPAL_TURN_STOPPED
    }) && params.allow_fallback
        && params.primary_model != crate::settings::FREE_TIER_MODEL_ID
    {
        log::warn!("[workspace-shape] primary Principal route failed; retrying free-tier route");
        let _ = tx.send_blocking(ShapeTurnEvent::PrincipalActivity(
            "The selected model failed; retrying with the fallback route…".into(),
        ));
        result_model = crate::settings::FREE_TIER_MODEL_ID.to_string();
        result = super::shape_principal::generate_streaming(
            crate::settings::FREE_TIER_MODEL_ID,
            &params.work_title,
            &params.background_body,
            params.current_proposal.as_ref(),
            params.current_plan.as_ref(),
            &params.current_role_drafts,
            &params.principal_model_routes,
            &params.principal_conversation_history,
            Some(&repository_context),
            &mut run_pass,
            turn_mode,
            &params.turn_memory,
            &params.approved_roles,
            &params.resolved_decisions,
            &params.shaping_memory,
        );
    }
    // Continuity guard: a revision that silently drops approved
    // roles or resolved decisions becomes a repairable error so the
    // repair pass below restores them instead of committing the loss.
    if let Ok(proposal) = &result {
        if let Err(continuity) = super::shape_principal::check_revision_continuity(
            proposal,
            &params.background_body,
            params.current_proposal.is_some(),
            &params.approved_roles,
            &params.resolved_questions,
        ) {
            log::warn!("[workspace-shape] {continuity}; retrying one corrected turn");
            result = Err(continuity);
        }
    }
    if result
        .as_ref()
        .is_err_and(|error| super::shape_principal::is_repairable_proposal_error(error))
    {
        let validation_error = result.as_ref().unwrap_err().clone();
        log::warn!(
                        "[workspace-shape] Principal returned an inconsistent proposal; retrying one corrected turn: {validation_error}"
                    );
        let _ = tx.send_blocking(ShapeTurnEvent::PrincipalActivity(
            "Validating the proposed plan and correcting an inconsistent response…".into(),
        ));
        let repair_message = format!(
                        "The user's original message is: {}\n\nThis is an internal response-repair instruction, not additional user intent and not approval to create work. Reclassify the turn using only the original message and prior conversation. The previous response was rejected by validation: {validation_error}. If the correct turn is conversation, return an inert conversation response. If it is proposal, return one corrected complete proposal; every plan task acceptance_criteria entry must copy the exact full text of an item in the top-level acceptance_criteria array, never a key, slug, summary, or paraphrase. The repair must preserve the prior approved roles, resolved decisions, and unaffected tasks — fix only what validation rejected.",
                        serde_json::to_string(&params.background_body).unwrap_or_else(|_| "\"\"".to_string())
                    );
        result = super::shape_principal::generate_streaming(
            &result_model,
            &params.work_title,
            &repair_message,
            params.current_proposal.as_ref(),
            params.current_plan.as_ref(),
            &params.current_role_drafts,
            &params.principal_model_routes,
            &params.principal_conversation_history,
            Some(&repository_context),
            &mut run_pass,
            turn_mode,
            &params.turn_memory,
            &params.approved_roles,
            &params.resolved_decisions,
            &params.shaping_memory,
        );
    }
    // Bounded auto-continue — the terminal-style agentic loop: if the
    // model ended the turn with pure promises and took no action
    // (no tool call, todos, or question), run ONE more pass with an
    // explicit act-now instruction instead of posting another
    // "I will research…" message. Greetings stay exempt; a second
    // idle pass is accepted as-is so this can never loop forever.
    let acted = turn_acted.load(std::sync::atomic::Ordering::Relaxed);
    // Approval-driven turns must not end as chat: up to two firm
    // extra passes until proposal JSON arrives. Otherwise a model
    // that todos-and-chats would be accepted without ever shaping.
    let mut force_left = if params.force_proposal { 2 } else { 0 };
    if force_left == 0
        && result.as_ref().is_ok_and(|result| {
            matches!(
                result.turn_kind,
                super::shape_principal::PrincipalTurnKind::Conversation
            ) && !result.requirements_complete
        })
        && !acted
        && super::shape_principal::conversational_reply(&params.background_body).is_none()
    {
        let _ = tx.send_blocking(ShapeTurnEvent::PrincipalActivity(
            "Continuing the research with tools…".into(),
        ));
        // New agent pass: keep the previous response visible and let
        // this pass stream as its own segment beneath it.
        let mut cont_history = params.principal_conversation_history.clone();
        if let Ok(result) = &result {
            if !result.assistant_message.trim().is_empty() {
                cont_history.push(("assistant".to_string(), result.assistant_message.clone()));
            }
        }
        let cont_body = format!(
                        "Continue working on my request now: {}. Do not reply with text only — in this response you MUST call tools (read_file, list_directory, or grep to gather evidence) and write the summary only after tool results arrive. If you already hold enough evidence, return the shaping JSON instead.",
                        params.background_body.trim()
                    );
        result = super::shape_principal::generate_streaming(
            &result_model,
            &params.work_title,
            &cont_body,
            params.current_proposal.as_ref(),
            params.current_plan.as_ref(),
            &params.current_role_drafts,
            &params.principal_model_routes,
            &cont_history,
            Some(&repository_context),
            &mut run_pass,
            turn_mode,
            &params.turn_memory,
            &params.approved_roles,
            &params.resolved_decisions,
            &params.shaping_memory,
        );
    }
    // Shape-forcing loop: an approval-driven turn that keeps answering
    // in chat (even with tool calls) is not done — drive it with a
    // firm instruction until proposal JSON arrives or the budget (two
    // extra passes) runs out. Bounded, so this can never loop forever.
    while force_left > 0
        && result.as_ref().is_ok_and(|result| {
            matches!(
                result.turn_kind,
                super::shape_principal::PrincipalTurnKind::Conversation
            )
        })
    {
        force_left -= 1;
        let _ = tx.send_blocking(ShapeTurnEvent::PrincipalActivity(
            "Creating the shape from the confirmed requirements…".into(),
        ));
        // New agent pass: keep the previous response visible and let
        // this pass stream as its own segment beneath it.
        let mut force_history = params.principal_conversation_history.clone();
        if let Ok(result) = &result {
            if !result.assistant_message.trim().is_empty() {
                force_history.push(("assistant".to_string(), result.assistant_message.clone()));
            }
        }
        let force_body = format!(
                        "Return ONLY the complete proposal JSON now — no introductory text, no promises, no questions unless genuinely blocking. Base it on the confirmed requirements and the tool evidence already gathered; reuse prior findings instead of repeating finished research. Preserve approved roles, resolved decisions, and unaffected tasks exactly — extend the shape, do not rebuild it. If one or two more read calls are needed to ground a field, make them first, then return the JSON in the same response. Original request: {}.",
                        params.background_body.trim()
                    );
        result = super::shape_principal::generate_streaming(
            &result_model,
            &params.work_title,
            &force_body,
            params.current_proposal.as_ref(),
            params.current_plan.as_ref(),
            &params.current_role_drafts,
            &params.principal_model_routes,
            &force_history,
            Some(&repository_context),
            &mut run_pass,
            turn_mode,
            &params.turn_memory,
            &params.approved_roles,
            &params.resolved_decisions,
            &params.shaping_memory,
        );
    }
    // Requirements-complete signal: a Gather turn presenting its
    // confirmation summary unlocks Approve Shape (user still clicks).
    if matches!(turn_mode, super::shape_principal::PrincipalTurnMode::Gather)
        && result.as_ref().is_ok_and(|result| {
            matches!(
                result.turn_kind,
                super::shape_principal::PrincipalTurnKind::Conversation
            ) && result.requirements_complete
        })
    {
        let _ = tx.send_blocking(ShapeTurnEvent::RequirementsComplete);
    }
    // Fail-closed shaping gate: without approval (and no existing
    // proposal) a proposal-shaped response can never become
    // canonical — it is downgraded to inert conversation, so shaping
    // cannot begin merely because the model emitted JSON.
    if !shaping_permitted {
        if let Ok(result) = &mut result {
            if !matches!(
                result.turn_kind,
                super::shape_principal::PrincipalTurnKind::Conversation
            ) {
                super::shape_principal::downgrade_to_conversation(result);
            }
        }
    }
    if let Ok(result) = &result {
        if result.turn_kind == super::shape_principal::PrincipalTurnKind::Conversation {
            let replied_at = chrono::Utc::now().timestamp_millis();
            let WorkspaceEventPayload::MessagePosted {
                message: user_message,
                ..
            } = &accepted.payload
            else {
                let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(
                    "Local workspace authority returned the wrong event".into(),
                )));
                return;
            };
            let memory_notes = result.memory_notes.clone();
            let reply_command = WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id,
                work_item_id,
                actor_id: params.principal.id,
                device_id: params.principal.device_id,
                correlation_id,
                timestamp_unix_ms: replied_at,
                expected_revision: Some(authority_revision),
                payload: WorkspaceCommandPayload::PostMessage {
                    message: WorkspaceAgentMessage {
                        id: AgentMessageId::new(),
                        thread_id: params.thread.id,
                        work_item_id,
                        sender_id: params.principal.id,
                        kind: AgentMessageKind::Discussion,
                        body: result.assistant_message.clone(),
                        review_finding: None,
                        reply_to: Some(user_message.id),
                        artifact_ids: Vec::new(),
                        evidence_ids: Vec::new(),
                        delivery_state: MessageDeliveryState::LocalOnly,
                        created_at_unix_ms: replied_at,
                        updated_at_unix_ms: replied_at,
                    },
                    routing: None,
                },
            };
            let reply = params.persistence_writer
                            .accept_workspace_command(reply_command)
                            .map_err(|error| format!("Could not save the Principal reply: {error:#}"))
                            .and_then(|mut events| {
                                if events.len() == 1 {
                                    Ok(events.remove(0))
                                } else {
                                    Err(format!(
                                        "Local workspace authority returned {} Principal reply events; expected one",
                                        events.len()
                                    ))
                                }
                            });
            match reply {
                Ok(event) => {
                    let reply_sequence = event.sequence;
                    let memory = submit_principal_memory(
                        &params.persistence_writer,
                        project_id,
                        work_item_id,
                        &params.principal,
                        reply_sequence,
                        memory_notes,
                    );
                    let _ = tx.send_blocking(ShapeTurnEvent::ConversationReplyAccepted {
                        event,
                        streamed: true,
                    });
                    if let Some(events) = memory {
                        let _ =
                            tx.send_blocking(ShapeTurnEvent::PrincipalMemoryRecorded { events });
                    }
                }
                Err(error) => {
                    let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(error)));
                }
            }
            return;
        }
    }

    // A conversation remains Draft. Enter Understanding only after a
    // provider has returned a structurally valid proposal, immediately
    // before committing that proposal to canonical state.
    if result.is_ok() && (params.is_new_work || params.initial_status == Some(WorkStatus::Draft)) {
        let transition_at = chrono::Utc::now().timestamp_millis();
        let transition_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: params.principal.id,
            device_id: params.principal.device_id,
            correlation_id,
            timestamp_unix_ms: transition_at,
            expected_revision: Some(authority_revision),
            payload: WorkspaceCommandPayload::TransitionWork {
                status: WorkStatus::Understanding,
            },
        };
        let transitioned = match params
            .persistence_writer
            .accept_workspace_command(transition_command)
        {
            Ok(mut events) if events.len() == 1 => events.remove(0),
            Ok(events) => {
                let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(format!(
                    "Local workspace authority returned {} events for TransitionWork; expected one",
                    events.len()
                ))));
                return;
            }
            Err(error) => {
                let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(Err(format!(
                    "Could not begin shaping the work item: {error:#}"
                ))));
                return;
            }
        };
        authority_revision = transitioned.sequence;
        if tx
            .send_blocking(ShapeTurnEvent::StatusAccepted(transitioned))
            .is_err()
        {
            return;
        }
    }
    let mut memory_events = None;
    let result = result
                    .and_then(|ok| {
                        // Final continuity check after all retries: a revision
                        // that still drops approved content must not commit.
                        super::shape_principal::check_revision_continuity(
                            &ok,
                            &params.background_body,
                            params.current_proposal.is_some(),
                            &params.approved_roles,
                            &params.resolved_questions,
                        )
                        .map(|()| ok)
                    })
                    .and_then(|result| {
                    let memory_notes = result.memory_notes;
                    let proposed_role_drafts = result.proposed_roles;
                    let proposed_plan_tasks = result.plan_tasks;
                    let revision = params.current_proposal
                        .as_ref()
                        .map(|proposal| proposal.revision.saturating_add(1))
                        .unwrap_or(1);
                    let proposal = dragent_workspace_model::ShapingProposal {
                        revision,
                        outcome: result.outcome,
                        requirements: result.requirements,
                        open_decisions: result.open_decisions,
                        acceptance_criteria: result.acceptance_criteria,
                        plan_summary: result.plan_summary,
                    };
                    proposal.validate().map_err(str::to_string)?;
                    let WorkspaceEventPayload::MessagePosted {
                        message: user_message,
                        ..
                    } = &accepted.payload
                    else {
                        return Err("Local workspace authority returned the wrong event".into());
                    };
                    let completed_at = chrono::Utc::now().timestamp_millis();
                    let principal_message_id = AgentMessageId::new();
                    let mut records = dragent_workspace_model::ShapingRecords::supersede_from_proposal(
                        params.current_shaping_records.as_ref(),
                        work_item_id,
                        &proposal,
                        params.thread.id,
                        principal_message_id,
                        params.principal.id,
                        completed_at,
                    );
                    let repository_documents = result
                        .repository_references
                        .iter()
                        .filter_map(|relative_path| {
                            dragent_workspace_model::DocumentRef::from_relative_path(
                                PathBuf::from(relative_path),
                            )
                        })
                        .collect::<Vec<_>>();
                    records.add_document_provenance(
                        &repository_documents,
                        params.principal.id,
                        completed_at,
                    );
                    let artifacts = repository_documents
                        .iter()
                        .map(|document| {
                            let path = document.relative_path.to_string_lossy();
                            let normalized = path.to_ascii_lowercase();
                            let kind = if normalized.contains("requirement") {
                                dragent_workspace_model::ArtifactKind::Requirements
                            } else if normalized.contains("decision") {
                                dragent_workspace_model::ArtifactKind::Decision
                            } else if normalized.contains("test") {
                                dragent_workspace_model::ArtifactKind::TestPlan
                            } else if normalized.contains("review") {
                                dragent_workspace_model::ArtifactKind::Review
                            } else if normalized.contains("design") {
                                dragent_workspace_model::ArtifactKind::Design
                            } else if normalized.contains("plan") {
                                dragent_workspace_model::ArtifactKind::TechnicalPlan
                            } else {
                                dragent_workspace_model::ArtifactKind::Other
                            };
                            dragent_workspace_model::ArtifactRef {
                                id: dragent_workspace_model::ArtifactId(Uuid::new_v5(
                                    &work_item_id.0,
                                    format!("artifact:{path}").as_bytes(),
                                )),
                                work_item_id,
                                relative_path: document.relative_path.clone(),
                                kind,
                            }
                        })
                        .collect::<Vec<_>>();
                    let input_artifact_ids = artifacts
                        .iter()
                        .map(|artifact| artifact.id)
                        .collect::<Vec<_>>();
                    let (role_definitions, work_contracts, proposed_team, plan) = proposed_team_records(
                        project_id,
                        work_item_id,
                        revision,
                        params.current_plan.as_ref(),
                        params.current_team.as_ref(),
                        &params.current_role_definitions,
                        &params.current_work_contracts,
                        proposed_role_drafts,
                        proposed_plan_tasks,
                        &records.acceptance_criteria,
                        &input_artifact_ids,
                        params.principal.id,
                        completed_at,
                    )?;
                    let principal_command = WorkspaceCommandEnvelope {
                        command_id: WorkspaceCommandId::new(),
                        schema_version: WORKSPACE_PROTOCOL_VERSION,
                        project_id,
                        work_item_id,
                        actor_id: params.principal.id,
                        device_id: params.principal.device_id,
                        correlation_id,
                        timestamp_unix_ms: completed_at,
                        expected_revision: Some(authority_revision),
                        payload: WorkspaceCommandPayload::CompleteShapingTurn {
                            message: WorkspaceAgentMessage {
                                id: principal_message_id,
                                thread_id: params.thread.id,
                                work_item_id,
                                sender_id: params.principal.id,
                                kind: AgentMessageKind::StatusUpdate,
                                body: result.assistant_message,
                                review_finding: None,
                                reply_to: Some(user_message.id),
                                artifact_ids: Vec::new(),
                                evidence_ids: Vec::new(),
                                delivery_state: MessageDeliveryState::LocalOnly,
                                created_at_unix_ms: completed_at,
                                updated_at_unix_ms: completed_at,
                            },
                            proposal,
                            records,
                            artifacts: artifacts.clone(),
                            role_definitions: role_definitions.clone(),
                            work_contracts: work_contracts.clone(),
                            proposed_team: Some(proposed_team.clone()),
                            plan: Some(plan.clone()),
                        },
                    };
                    let mut events = params.persistence_writer
                        .accept_workspace_command(principal_command)
                        .map_err(|error| format!("Could not save the Principal response: {error:#}"))?;
                    authority_revision = events
                        .last()
                        .map(|event| event.sequence)
                        .unwrap_or(authority_revision);
                    let team_events = events.split_off(3);
                    let team_warning: Option<String> = None;
                    let should_enter_planning = params.is_new_work
                        || matches!(params.initial_status, Some(WorkStatus::Draft | WorkStatus::Understanding));
                    let (planning_event, planning_warning) = if should_enter_planning {
                        let transition_command = WorkspaceCommandEnvelope {
                            command_id: WorkspaceCommandId::new(),
                            schema_version: WORKSPACE_PROTOCOL_VERSION,
                            project_id,
                            work_item_id,
                            actor_id: params.principal.id,
                            device_id: params.principal.device_id,
                            correlation_id,
                            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
                            expected_revision: Some(authority_revision),
                            payload: WorkspaceCommandPayload::TransitionWork {
                                status: WorkStatus::Planning,
                            },
                        };
                        match params.persistence_writer.accept_workspace_command(transition_command) {
                            Ok(mut status_events) if status_events.len() == 1 => {
                                (Some(status_events.remove(0)), None)
                            }
                            Ok(status_events) => (
                                None,
                                Some(format!(
                                    "The plan was saved, but authority returned {} planning events; expected one",
                                    status_events.len()
                                )),
                            ),
                            Err(error) => (
                                None,
                                Some(format!(
                                    "The plan was saved, but its lifecycle could not enter Planning: {error:#}"
                                )),
                            ),
                        }
                    } else {
                        (None, None)
                    };
                    let warning = match (team_warning, planning_warning) {
                        (Some(team), Some(planning)) => Some(format!("{team} {planning}")),
                        (Some(warning), None) | (None, Some(warning)) => Some(warning),
                        (None, None) => None,
                    };
                    // Brain entries land after the turn's own events so the next
                    // turn reads them in arrival order. Best-effort: failure only
                    // logs inside the helper, never fails this turn.
                    memory_events = submit_principal_memory(
                        &params.persistence_writer,
                        project_id,
                        work_item_id,
                        &params.principal,
                        authority_revision,
                        memory_notes,
                    );
                    if let Some(ref events) = memory_events {
                        authority_revision = events
                            .last()
                            .map(|event| event.sequence)
                            .unwrap_or(authority_revision);
                    }
                    Ok(ShapePrincipalOutcome {
                        events,
                        team_events,
                        planning_event,
                        warning,
                    })
                });
    let _ = tx.send_blocking(ShapeTurnEvent::PrincipalAccepted(result));
    if let Some(events) = memory_events {
        let _ = tx.send_blocking(ShapeTurnEvent::PrincipalMemoryRecorded { events });
    }
}
