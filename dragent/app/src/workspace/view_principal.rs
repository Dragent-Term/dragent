//! Principal coordination drivers.
//!
//! Responsible for: Principal turns, plan actions, escalation, review
//! policy, work-item agents, principal passes, plan approval.
//! Blocks here are impl WorkspaceView: same type, same behavior.

use super::share_sheet;
use super::viewer_bar;
use super::{InputEditor, InputEditorEvent, InputMode, LanguageNavigationCapabilities};
use crate::agent::workspace_execution::{
    next_workspace_launch_action, next_workspace_retry_wait_ms, WorkspaceExecutionRuntime,
    WorkspaceLaunchAction, WorkspaceLaunchPolicy, WorkspacePipelineAction,
    WorkspaceResolvedToolAction, WorkspaceToolRequest,
};
use crate::auth::{AuthManager, AuthManagerEvent};
use crate::realtime::manager::{SessionManager, SessionManagerEvent};
use crate::settings::AccountStore;
use crate::settings::{SettingsEvent, SettingsView};
use crate::setup::UiSettings;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::{
    Theme, DEFAULT_LEFT_PANEL_WIDTH, DEFAULT_RIGHT_PANEL_WIDTH, HEADER_BAR_HEIGHT, MIN_PANEL_WIDTH,
    UI_FONT_SIZE, WORKSPACE_PADDING,
};
use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_session_protocol::{AccessLevel, ParticipantId, Role};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::{
    drag_resize_handle, resizable_state_handle, resizable_state_handle_initialized, Align,
    ChildAnchor, ChildView, ClippedScrollStateHandle, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, DragBarSide, DragResizeAxis, DragResizeElement,
    DragResizeHandle, DropTarget, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    PositionedElementAnchor, PositionedElementOffsetBounds, Resizable, ResizableStateHandle,
    SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, ReadModel, ReadView, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::Cursor;
use warpui_core::platform::FilePickerConfiguration;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::process_ext::NoWindow;
use dragent_editor::{
    char_index_to_lsp_position, lsp_position_to_char_index, LanguageRequest, LanguageRequestKind,
    TextPosition, TextRange,
};
use dragent_language_services::{
    LanguageServerCatalog, LanguageServerKey, LanguageServiceClient, LanguageServiceClientConfig,
    LanguageServiceEvent, SyncedDocument,
};

use super::pane::WorkspacePane;
use super::pane_layout::{
    LayoutNode, PaneLayout, SplitDirection, SplitId, SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT,
    MIN_PANE_WIDTH,
};
use super::project_sidebar::{
    find_git_root, ProjectSidebarModel, ProjectSidebarView, ReviewPrState,
};
// Re-exported so existing `super::view::ReviewShipState` paths keep working
// during the migration; new code should import from `review_pr_service`.
// Re-exported so existing `super::…` and test paths keep working during the
// migration; new code should import from `project_naming`.
// Glob import during migration: shaping helpers moved to `shape_parse`
// verbatim; call sites below are unchanged.
// Glob import during migration: shaping builders moved to `shape_build`.
use super::shape_build::*;
// Glob import during migration: execution delivery helpers moved verbatim.
use super::execution_delivery::*;
// Glob import during migration: language navigation helpers moved verbatim.
use super::language_nav::*;
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_parse::*;
use super::sidebar::color_from_hex;
use super::view_overlays::ExplorerPrompt;
use super::{
    chrome_inset, content_header, header, left_panel, pane_layout, persistence, sidebar,
    split_tab_header, DropSide, PaneDropZone, ProjectFolderIntent, SidebarFocus, WorkItemView,
    WorkspaceAction, DEFAULT_SESSION_TITLE,
};
use dragent_persistence::{SerializedPaneLayout, SerializedSplitDirection};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorKind, ActorRef, AgentMessage as WorkspaceAgentMessage,
    AgentMessageId, AgentMessageKind, AgentMessageRouting, AttentionItem, AttentionItemId,
    AttentionKind, AttentionResolution, AttentionStatus, BuiltInLayout, ChargingAuthorization,
    ChargingMode, CommunicationThread, ExecutionAttemptId, LayoutAxis, LayoutChromeState,
    LayoutCollection, LayoutInstance, LayoutPanelState, LayoutScope, MessageDeliveryState, Project,
    ProjectDocumentIndex, SurfaceDescriptor, SurfaceId, SurfaceKind, SurfaceLayoutNode,
    SurfaceRegistry, ThreadId, WorkItemId, WorkStatus, WorkspaceEvent, WorkspaceEventKind,
    WorkspaceState, MAX_LAYOUT_FOCUS_HISTORY,
};
use left_panel::LeftPanelView;
use persistence::{PersistedSession, WindowChrome, WriterHandles};
use uuid::Uuid;

use super::view::WorkspaceView;
use super::view::PRINCIPAL_ESCALATED_REPLY;

impl WorkspaceView {
    /// Drives the Principal's own side of coordination: whenever a worker's
    /// Blocker or DecisionRequest to the Principal is still unanswered, the
    /// Principal takes a turn — reads the plan, contracts, dependency state,
    /// and this thread's history, then replies and/or escalates. The same
    /// conversation stays running; Start Coordination never ended it. Hooked
    /// into the same reactive call sites `continue_workspace_launch` uses,
    /// and tried before it there so a pending coordination turn always gets
    /// a chance to run first.
    pub(crate) fn continue_principal_coordination(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if self.workspace_principal_coordination_pending {
            let now = chrono::Utc::now().timestamp_millis();
            let started = self
                .workspace_principal_coordination_pending_since
                .unwrap_or(now);
            if now.saturating_sub(started) >= Self::PRINCIPAL_COORDINATION_TURN_TIMEOUT_MS {
                log::warn!(
                    "[workspace] principal coordination turn timed out; clearing stuck flag"
                );
                self.clear_principal_coordination_pending();
            } else {
                return false;
            }
        }
        let Some((target_message, routing)) =
            crate::agent::principal_coordination::next_principal_coordination_message_excluding(
                &self.workspace_state,
                &self.principal_replies_submitted,
            )
        else {
            return false;
        };
        let target_message = target_message.clone();
        let routing = routing.clone();
        let Some(work_title) = self
            .workspace_state
            .active_work()
            .map(|work| work.title.clone())
        else {
            return false;
        };
        // No plan means coordination could not have produced this message;
        // leave it for a human rather than guess at a response.
        let Some(plan) = self.workspace_state.latest_structured_plan().cloned() else {
            return false;
        };
        let contracts: Vec<dragent_workspace_model::WorkContract> = self
            .workspace_state
            .work_contracts()
            .into_iter()
            .cloned()
            .collect();
        // Role names for the prompt's `add_tasks[].assigned_role` /
        // `propose_roles` guidance — `WorkContract` itself has no name, only
        // `role_definition_id`.
        let role_names: HashMap<dragent_workspace_model::WorkContractId, String> = self
            .workspace_state
            .latest_proposed_team()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        self.workspace_state
                            .role_definition(role.role_definition_id)
                            .map(|definition| (role.work_contract_id, definition.name.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // A worker's Blocker/DecisionRequest is always about one task, so its
        // dependency closure is the right, narrow context. A human's live
        // question has no task_id at all (`submit_live_coordination_message`)
        // — for that case the Principal needs to see everything currently
        // running to answer something like "which agent is working right
        // now?", not an empty list.
        let related_attempts: Vec<dragent_workspace_model::ExecutionAttempt> = match routing.task_id
        {
            Some(task_id) => {
                let closure = crate::agent::workspace_execution::task_dependency_closure(
                    &self.workspace_state,
                    task_id,
                );
                self.workspace_state
                    .execution_attempts()
                    .into_iter()
                    .filter(|attempt| closure.contains(&attempt.task_id))
                    .cloned()
                    .collect()
            }
            None => self
                .workspace_state
                .execution_attempts()
                .into_iter()
                .cloned()
                .collect(),
        };
        let thread_history: Vec<WorkspaceAgentMessage> = self
            .workspace_state
            .agent_messages()
            .into_iter()
            .filter(|(message, _)| {
                message.thread_id == target_message.thread_id && message.id != target_message.id
            })
            .map(|(message, _)| message.clone())
            .collect();
        // Same durable-memory tail Shaping feeds every turn: decisions,
        // preferences, and prior changes are why the plan looks the way it
        // does, and coordination decisions must stay consistent with them.
        let shaping_memory: Vec<String> = {
            let memory = self.workspace_state.shaping_memory();
            memory
                .iter()
                .skip(memory.len().saturating_sub(30))
                .map(|note| {
                    format!(
                        "[{}] {}",
                        format!("{:?}", note.kind).to_ascii_lowercase(),
                        note.text
                    )
                })
                .collect()
        };

        self.mark_principal_coordination_pending();
        self.workspace_principal_coordination_epoch =
            self.workspace_principal_coordination_epoch.wrapping_add(1);
        let request_epoch = self.workspace_principal_coordination_epoch;

        let settings = crate::settings::AppSettings::load();
        let model = if !self.account_logged_in_cache {
            crate::settings::FREE_TIER_MODEL_ID.to_string()
        } else if settings.selected_model == crate::settings::AUTO_MODEL_ID
            || settings.selected_model.trim().is_empty()
        {
            crate::settings::FREE_TIER_MODEL_ID.to_string()
        } else {
            settings.selected_model.clone()
        };
        // The coordination turn runs on the work item's own conversation —
        // the same controller and cards the shaping turns use — so the user
        // sees the Principal read, think, and answer, and any failure is an
        // Error card in the conversation rather than a banner. Read tools
        // only: no ask_user/todowrite, the reply must resolve or escalate.
        let (system, prompt) = super::coordination_principal::build_coordination_request(
            &work_title,
            &plan,
            &contracts,
            &role_names,
            &related_attempts.iter().collect::<Vec<_>>(),
            &target_message,
            &thread_history.iter().collect::<Vec<_>>(),
            &shaping_memory,
        );
        let allowed_tools: Vec<String> = super::shape_principal::PRINCIPAL_READ_TOOLS
            .iter()
            .map(|name| name.to_string())
            .collect();
        let work_item_id = self.workspace_work_item_id;
        let local_user = ActorRef::local_user(self.workspace_state.project().id, None).id;
        // Who is asking: the user themself (no author badge) or a worker /
        // reviewer, shown under its role name like a teammate's message.
        let author = if target_message.sender_id == local_user {
            None
        } else {
            let role_name = self
                .workspace_state
                .agent_instances()
                .into_iter()
                .find(|agent| {
                    ActorRef::agent_instance(agent.id, ActorKind::WorkerAgent).id
                        == target_message.sender_id
                        || ActorRef::agent_instance(agent.id, ActorKind::ReviewerAgent).id
                            == target_message.sender_id
                })
                .and_then(|agent| {
                    self.workspace_state
                        .role_definition(agent.role_definition_id)
                })
                .map(|role| role.name.clone())
                .unwrap_or_else(|| "Agent".to_string());
            Some(crate::agent::controller::AgentAuthor {
                display_name: role_name,
                avatar_url: String::new(),
            })
        };
        self.workspace_principal_coordination_activity = Some(match &author {
            None => "your message".to_string(),
            Some(author) => format!(
                "a {} from {}",
                match target_message.kind {
                    AgentMessageKind::Handoff => "handoff",
                    AgentMessageKind::Blocker => "blocker",
                    _ => "request",
                },
                author.display_name
            ),
        });
        let display = target_message.body.clone();
        let agent = self.ensure_work_item_agent(work_item_id, ctx);
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        self.principal_coordination_turns
            .insert(work_item_id, (request_epoch, target_message, routing));
        ctx.update_model(&controller, |controller, mctx| {
            controller.set_allowed_tools(Some(allowed_tools));
            controller.set_tool_call_budget(Some(super::shape_principal::MAX_PRINCIPAL_FINDINGS));
            controller.set_session_history(Vec::new());
            controller.start_streaming_query_with_display(
                prompt,
                display,
                Vec::new(),
                author,
                model,
                Box::new(move || system),
                mctx,
            );
        });
        ctx.update_view(&agent, |agent, _| agent.pin_to_bottom());
        ctx.notify();
        true
    }

    /// Commit the Principal's reply, plan changes, and any escalation in one
    /// transaction. A persisted reply must never outlive lost follow-up actions.
    pub(crate) fn finish_principal_coordination_turn(
        &mut self,
        request_epoch: u64,
        target_message: WorkspaceAgentMessage,
        routing: AgentMessageRouting,
        result: Result<super::coordination_principal::CoordinationTurnResult, String>,
        ctx: &mut ViewContext<Self>,
    ) {
        // A newer turn already started (the stuck-flag guard cleared this
        // one) — this stream's result is stale and must not disturb it.
        if request_epoch != self.workspace_principal_coordination_epoch
            || target_message.work_item_id != self.workspace_work_item_id
        {
            return;
        }
        if self.workspace_launch_pending {
            ctx.spawn(
                Timer::after(Duration::from_millis(50)),
                move |view, _, ctx| {
                    view.finish_principal_coordination_turn(
                        request_epoch,
                        target_message,
                        routing,
                        result,
                        ctx,
                    );
                },
            );
            return;
        }
        self.clear_principal_coordination_pending();
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.shape_coordination_error =
                    Some(format!("Principal coordination turn failed: {error}"));
                ctx.notify();
                return;
            }
        };
        let project_id = self.workspace_state.project().id;
        let principal = ActorRef::principal_agent(project_id);
        let Some(work_item_id) = self.workspace_state.active_work().map(|work| work.id) else {
            return;
        };
        let now = chrono::Utc::now().timestamp_millis();
        let mut recipient_ids: Vec<dragent_workspace_model::ActorId> = routing
            .recipient_ids
            .iter()
            .copied()
            .filter(|id| *id != principal.id)
            .collect();
        if !recipient_ids.contains(&target_message.sender_id) {
            recipient_ids.push(target_message.sender_id);
        }
        if outcome.escalate {
            self.workspace_principal_escalation_queue
                .push_back(AttentionItem {
                    id: AttentionItemId(Uuid::new_v5(
                        &target_message.id.0,
                        b"attention:principal-coordination",
                    )),
                    work_item_id,
                    task_id: routing.task_id,
                    thread_id: Some(target_message.thread_id),
                    execution_attempt_id: routing.execution_attempt_id,
                    tool_call_id: None,
                    kind: outcome.escalation_kind.unwrap_or(AttentionKind::Decision),
                    status: AttentionStatus::Open,
                    summary: outcome.escalation_summary.clone().unwrap_or_else(|| {
                        "The Principal needs your decision to continue coordination.".to_string()
                    }),
                    requested_by: principal.id,
                    assigned_to: vec![ActorRef::local_user(project_id, None).id],
                    created_at_unix_ms: now,
                    resolved_at_unix_ms: None,
                    resolution_kind: None,
                    resolution: None,
                });
        }
        let reply_body = if !outcome.reply_body.trim().is_empty() {
            outcome.reply_body.clone()
        } else if outcome.escalate {
            PRINCIPAL_ESCALATED_REPLY.to_string()
        } else if !outcome.add_tasks.is_empty() || !outcome.propose_roles.is_empty() {
            "Done.".to_string()
        } else {
            // A handoff/status the Principal had nothing to add to still
            // gets an explicit acknowledgement so the thread reads as
            // answered (and the selector stops re-offering it).
            "Acknowledged.".to_string()
        };
        if !self.enqueue_principal_plan_actions(work_item_id, &outcome, now, ctx) {
            // No acknowledgement of actions that could not even be prepared.
            self.workspace_principal_escalation_queue.retain(|item| {
                item.id
                    != AttentionItemId(Uuid::new_v5(
                        &target_message.id.0,
                        b"attention:principal-coordination",
                    ))
            });
            return;
        }
        self.principal_replies_submitted.insert(target_message.id);
        let reply = WorkspaceAgentMessage {
            id: AgentMessageId(Uuid::new_v5(
                &target_message.id.0,
                b"message:principal-coordination-reply",
            )),
            thread_id: target_message.thread_id,
            work_item_id,
            sender_id: principal.id,
            kind: AgentMessageKind::Discussion,
            body: reply_body,
            review_finding: None,
            reply_to: Some(target_message.id),
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::LocalOnly,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId(Uuid::new_v5(
                &target_message.id.0,
                b"command:principal-coordination-reply",
            )),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: principal.id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::PostMessage {
                message: reply,
                routing: Some(AgentMessageRouting {
                    recipient_ids,
                    task_id: routing.task_id,
                    execution_attempt_id: routing.execution_attempt_id,
                }),
            },
        };
        let mut commands = Vec::new();
        for (work_item_id, actor_id, payload) in
            self.workspace_principal_plan_actions_queue.drain(..)
        {
            commands.push(WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id,
                work_item_id,
                actor_id,
                device_id: None,
                correlation_id: command.correlation_id,
                timestamp_unix_ms: now,
                expected_revision: Some(self.workspace_canonical_revision),
                payload,
            });
        }
        for attention_item in self.workspace_principal_escalation_queue.drain(..) {
            commands.push(WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id,
                work_item_id: attention_item.work_item_id,
                actor_id: principal.id,
                device_id: None,
                correlation_id: command.correlation_id,
                timestamp_unix_ms: now,
                expected_revision: Some(self.workspace_canonical_revision),
                payload: WorkspaceCommandPayload::RaiseAttention { attention_item },
            });
        }
        // The reply is last, and all effects are in one SQLite transaction.
        // Either replay sees the entire decision, or the request is unanswered.
        commands.push(command);
        if let Err(error) = self.try_submit_workspace_runtime_commands(commands, None, ctx) {
            self.record_principal_error(error.clone(), ctx);
            self.shape_coordination_error = Some(error);
            ctx.notify();
        }
    }

    /// Turns one coordination turn's `add_tasks`/`propose_roles` into queued
    /// commands (`workspace_principal_plan_actions_queue`, drained one at a
    /// time by `continue_principal_plan_actions`). A role name the model
    /// reuses from an already-existing role is treated as more tasks for
    /// that role rather than a duplicate — defensive, since the prompt
    /// already tells it to do this itself. Any resolution failure (an
    /// unknown role/task reference, a role with no acceptance criteria to
    /// satisfy) surfaces as a coordination error instead of silently
    /// dropping the Principal's action.
    fn enqueue_principal_plan_actions(
        &mut self,
        work_item_id: WorkItemId,
        outcome: &super::coordination_principal::CoordinationTurnResult,
        now: i64,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if outcome.add_tasks.is_empty() && outcome.propose_roles.is_empty() {
            return true;
        }
        let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
            self.shape_coordination_error =
                Some("The Principal tried to add work, but no plan exists yet.".into());
            ctx.notify();
            return false;
        };
        let project_id = self.workspace_state.project().id;
        let principal_id = ActorRef::principal_agent(project_id).id;
        let plan_actor_id = plan.approved_by.unwrap_or(principal_id);
        let acceptance_criteria = self
            .workspace_state
            .shaping_records_projection()
            .map(|records| records.acceptance_criteria)
            .unwrap_or_default();
        let mut role_contract_by_name: HashMap<String, dragent_workspace_model::WorkContractId> =
            self.workspace_state
                .latest_proposed_team()
                .map(|team| {
                    team.roles
                        .iter()
                        .filter_map(|role| {
                            self.workspace_state
                                .role_definition(role.role_definition_id)
                                .map(|definition| {
                                    (
                                        normalize_role_or_task_key(&definition.name),
                                        role.work_contract_id,
                                    )
                                })
                        })
                        .collect()
                })
                .unwrap_or_default();

        let mut all_new_tasks: Vec<dragent_workspace_model::PlanTask> = Vec::new();
        let mut queued: Vec<(
            WorkItemId,
            dragent_workspace_model::ActorId,
            WorkspaceCommandPayload,
        )> = Vec::new();

        for proposal in &outcome.propose_roles {
            let identity = normalize_role_or_task_key(&proposal.role.name);
            if role_contract_by_name.contains_key(&identity) {
                match resolve_principal_plan_task_drafts(
                    &plan,
                    &role_contract_by_name,
                    &acceptance_criteria,
                    &proposal.tasks,
                ) {
                    Ok(tasks) => all_new_tasks.extend(tasks),
                    Err(error) => {
                        self.shape_coordination_error = Some(error);
                        ctx.notify();
                        return false;
                    }
                }
                continue;
            }
            let (role_definition, mut work_contract) = build_proposed_role_records(
                project_id,
                work_item_id,
                proposal.role.clone(),
                principal_id,
                now,
            );
            role_contract_by_name.insert(identity, work_contract.id);
            let role_tasks = match resolve_principal_plan_task_drafts(
                &plan,
                &role_contract_by_name,
                &acceptance_criteria,
                &proposal.tasks,
            ) {
                Ok(tasks) => tasks,
                Err(error) => {
                    self.shape_coordination_error = Some(error);
                    ctx.notify();
                    return false;
                }
            };
            // No dependency anywhere on a task that isn't already Completed
            // — not just within this role's own new tasks — starts
            // immediately, exactly the rule the user asked for. Otherwise it
            // needs the existing one-click Approve, same as a role proposed
            // during Shaping.
            let completed: HashSet<dragent_workspace_model::PlanTaskId> = plan
                .tasks
                .iter()
                .filter(|task| task.status == dragent_workspace_model::PlanTaskStatus::Completed)
                .map(|task| task.id)
                .collect();
            let dependency_free = role_tasks_are_dependency_free(&role_tasks, &completed);
            work_contract.task_ids = role_tasks.iter().map(|task| task.id).collect();
            let task_criteria: Vec<dragent_workspace_model::AcceptanceCriterionId> = role_tasks
                .iter()
                .flat_map(|task| task.acceptance_criteria.iter().copied())
                .collect();
            work_contract.success_criteria = if task_criteria.is_empty() {
                acceptance_criteria
                    .iter()
                    .map(|criterion| criterion.id)
                    .collect()
            } else {
                task_criteria
            };
            if work_contract.success_criteria.is_empty() {
                self.shape_coordination_error = Some(format!(
                    "Cannot propose role '{}': the work item has no acceptance criteria yet for it to satisfy.",
                    role_definition.name
                ));
                ctx.notify();
                return false;
            }
            if dependency_free {
                work_contract.status = dragent_workspace_model::WorkContractStatus::Approved;
            }
            if let Err(error) = role_definition.validate() {
                self.shape_coordination_error = Some(error.to_string());
                ctx.notify();
                return false;
            }
            if let Err(error) = work_contract.validate() {
                self.shape_coordination_error = Some(error.to_string());
                ctx.notify();
                return false;
            }
            let mut proposed_team = self
                .workspace_state
                .latest_proposed_team()
                .cloned()
                .unwrap_or_else(|| dragent_workspace_model::ProposedTeam {
                    id: dragent_workspace_model::TeamId(Uuid::new_v5(
                        &work_item_id.0,
                        b"proposed-team",
                    )),
                    work_item_id,
                    version: 0,
                    roles: Vec::new(),
                    status: dragent_workspace_model::StructuredRecordStatus::Draft,
                    created_by: principal_id,
                    created_at_unix_ms: now,
                    updated_at_unix_ms: now,
                });
            proposed_team.version = proposed_team.version.saturating_add(1);
            proposed_team.updated_at_unix_ms = now;
            proposed_team
                .roles
                .push(dragent_workspace_model::ProposedTeamRole {
                    role_definition_id: role_definition.id,
                    work_contract_id: work_contract.id,
                    rationale: proposal.role.rationale.clone(),
                });
            if let Err(error) = proposed_team.validate() {
                self.shape_coordination_error = Some(error.to_string());
                ctx.notify();
                return false;
            }
            queued.push((
                work_item_id,
                principal_id,
                WorkspaceCommandPayload::UpsertRoleDefinition { role_definition },
            ));
            queued.push((
                work_item_id,
                principal_id,
                WorkspaceCommandPayload::UpsertWorkContract { work_contract },
            ));
            queued.push((
                work_item_id,
                principal_id,
                WorkspaceCommandPayload::UpsertProposedTeam { proposed_team },
            ));
            all_new_tasks.extend(role_tasks);
        }

        match resolve_principal_plan_task_drafts(
            &plan,
            &role_contract_by_name,
            &acceptance_criteria,
            &outcome.add_tasks,
        ) {
            Ok(tasks) => all_new_tasks.extend(tasks),
            Err(error) => {
                self.shape_coordination_error = Some(error);
                ctx.notify();
                return false;
            }
        }

        if !all_new_tasks.is_empty() {
            plan.tasks.extend(all_new_tasks);
            match append_plan_task_payload(&self.workspace_state, plan, now) {
                Ok(payload) => queued.push((work_item_id, plan_actor_id, payload)),
                Err(error) => {
                    self.shape_coordination_error = Some(error);
                    ctx.notify();
                    return false;
                }
            }
        }

        self.workspace_principal_plan_actions_queue.extend(queued);
        true
    }

    /// Drains one queued plan action (role_definition, contract, team, or
    /// plan task-append from `enqueue_principal_plan_actions`) per call —
    /// same single-command-at-a-time shape `continue_principal_escalation`
    /// uses, since `submit_workspace_runtime_command` drops a command
    /// outright while another is already in flight. The envelope is built
    /// fresh here, not when queued, so `expected_revision` reflects
    /// whatever just landed from the previous step in this same sequence.
    pub(crate) fn continue_principal_plan_actions(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if self.workspace_launch_pending {
            return false;
        }
        let Some((work_item_id, actor_id, payload)) =
            self.workspace_principal_plan_actions_queue.pop_front()
        else {
            return false;
        };
        let project_id = self.workspace_state.project().id;
        let now = chrono::Utc::now().timestamp_millis();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload,
        };
        self.submit_workspace_runtime_command(command, ctx);
        true
    }

    /// Drains one queued coordination escalation per call, mirroring
    /// `continue_principal_review_policy`'s queue-drain shape: `RaiseAttention`
    /// shares the single serialized submission path with every other
    /// coordination command, so only one command goes out per reactive tick.
    pub(crate) fn continue_principal_escalation(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if self.workspace_launch_pending {
            return false;
        }
        let Some(attention_item) = self.workspace_principal_escalation_queue.pop_front() else {
            return false;
        };
        let project_id = self.workspace_state.project().id;
        let principal = ActorRef::principal_agent(project_id);
        let now = chrono::Utc::now().timestamp_millis();
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId(Uuid::new_v5(
                &attention_item.id.0,
                b"command:principal-coordination-escalation",
            )),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id: attention_item.work_item_id,
            actor_id: principal.id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::RaiseAttention { attention_item },
        };
        self.submit_workspace_runtime_command(command, ctx);
        true
    }

    pub(crate) fn waive_workspace_criterion(
        &mut self,
        acceptance_criterion_id: dragent_workspace_model::AcceptanceCriterionId,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(mut criterion) =
            self.workspace_state
                .shaping_records_projection()
                .and_then(|records| {
                    records
                        .acceptance_criteria
                        .into_iter()
                        .find(|criterion| criterion.id == acceptance_criterion_id)
                })
        else {
            self.shape_coordination_error =
                Some("This acceptance criterion is no longer available.".into());
            ctx.notify();
            return;
        };
        if !matches!(
            criterion.status,
            dragent_workspace_model::AcceptanceStatus::Pending
                | dragent_workspace_model::AcceptanceStatus::Failed
        ) {
            return;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        criterion.status = dragent_workspace_model::AcceptanceStatus::Waived;
        criterion.waiver_reason =
            Some("Explicitly waived by the workspace owner during final review.".into());
        criterion.waived_by = Some(actor.id);
        criterion.updated_at_unix_ms = now;
        criterion
            .provenance
            .push(dragent_workspace_model::Provenance {
                source: dragent_workspace_model::ProvenanceSource::ManualEdit,
                recorded_by: actor.id,
                recorded_at_unix_ms: now,
            });
        let project_id = self.workspace_state.project().id;
        let Some(work_item_id) = self.workspace_state.active_work().map(|work| work.id) else {
            return;
        };
        self.submit_workspace_runtime_command(
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
                payload: WorkspaceCommandPayload::UpsertAcceptanceCriterion {
                    acceptance_criterion: criterion,
                },
            },
            ctx,
        );
    }

    pub(super) fn resolve_workspace_execution_review_as(
        &mut self,
        attempt_id: ExecutionAttemptId,
        resolution: dragent_workspace_model::ExecutionReviewResolution,
        actor: ActorRef,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.workspace_launch_pending {
            return;
        }
        let Some(attempt) = self.workspace_state.execution_attempt(attempt_id).cloned() else {
            self.shape_coordination_error =
                Some("This execution review is no longer available.".into());
            ctx.notify();
            return;
        };
        if attempt.status != dragent_workspace_model::ExecutionAttemptStatus::Succeeded
            || (attempt.review_resolution.is_some()
                && !(resolution
                    == dragent_workspace_model::ExecutionReviewResolution::RevisionRequested
                    && actor.id == ActorRef::principal_agent(self.workspace_state.project().id).id
                    && crate::agent::workspace_execution::legacy_review_recovery(
                        &self.workspace_state,
                        attempt_id,
                    )))
        {
            self.shape_coordination_error =
                Some("Only completed isolated changes can be reviewed.".into());
            ctx.notify();
            return;
        }
        let review_root = self
            .workspace_execution_runtime
            .prepared_execution_root(attempt_id)
            .map(Path::to_path_buf)
            .zip(
                self.workspace_execution_runtime
                    .prepared_execution_isolation(attempt_id)
                    .cloned(),
            );
        if resolution == dragent_workspace_model::ExecutionReviewResolution::Integrated
            && review_root.is_none()
        {
            self.shape_coordination_error = Some(
                "The isolated review checkout is unavailable. It may already be resolved.".into(),
            );
            ctx.notify();
            return;
        }
        let project_root = self.workspace_state.project().root.clone();
        let review_project = self.workspace_state.project().id;
        let review_work = attempt.work_item_id;
        let review_actor = actor.id;
        let completion_command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: review_project,
            work_item_id: review_work,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::ResolveAgentExecutionReview {
                attempt_id,
                resolution,
            },
        };
        if resolution != dragent_workspace_model::ExecutionReviewResolution::Integrated {
            // Non-Git resolutions already use the ordinary authority path,
            // whose callback updates the originating parked work item after a
            // tab switch. Only Integrated needs the durable Git bridge below.
            self.submit_workspace_runtime_command(completion_command, ctx);
            return;
        }
        let background_command = completion_command.clone();
        self.mark_launch_pending();
        let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
        crate::bg::detach(move || {
            let result = match resolution {
                dragent_workspace_model::ExecutionReviewResolution::Integrated => {
                    let (root, isolation) = review_root.as_ref().expect("checked review root");
                    (|| {
                        #[cfg(not(target_family = "wasm"))]
                        {
                            let path =
                                dragent_persistence::database_path().map_err(|e| e.to_string())?;
                            let mut conn = dragent_persistence::setup_connection(&path)
                                .map_err(|e| e.to_string())?;
                            let fingerprints = dragent_persistence::workspace_store::preflight_execution_acceptance(
                                &mut conn, review_project, review_work, review_actor, attempt_id,
                                resolution, background_command.timestamp_unix_ms,
                            ).map_err(|e| e.to_string())?;
                            let plan = crate::git_status::worktree::prepare_agent_integration_plan(
                                &project_root,
                                root,
                                isolation,
                            )?;
                            if fingerprints
                                .iter()
                                .any(|fingerprint| fingerprint != &plan.candidate_fingerprint)
                            {
                                return Err("Candidate changed after verification; request another review before integration.".into());
                            }
                            let plan_json = serde_json::to_string(&plan).map_err(|error| {
                                format!("Could not encode integration recovery plan: {error}")
                            })?;
                            dragent_persistence::begin_workspace_integration(
                                &mut conn,
                                &dragent_persistence::WorkspaceIntegrationIntent {
                                    attempt_id,
                                    project_id: review_project,
                                    work_item_id: review_work,
                                    command_id: background_command.command_id,
                                    command: background_command.clone(),
                                    integration_plan_json: plan_json,
                                    state: "prepared".into(),
                                    claim_owner: None,
                                    claim_expires_at_unix_ms: None,
                                    created_at_unix_ms: background_command.timestamp_unix_ms,
                                    completed_at_unix_ms: None,
                                    last_error: None,
                                },
                            )
                            .map_err(|error| error.to_string())?;
                            let claim_owner = Uuid::new_v4().to_string();
                            let claimed = dragent_persistence::claim_workspace_integration(
                                &mut conn,
                                attempt_id,
                                &claim_owner,
                                chrono::Utc::now().timestamp_millis(),
                                Self::LAUNCH_REQUEST_TIMEOUT_MS,
                            )
                            .map_err(|error| error.to_string())?;
                            if claimed.is_none() {
                                return Err("Another process is already reconciling this integration; it will resume from the durable intent.".into());
                            }
                            if let Err(error) =
                                crate::git_status::worktree::apply_agent_integration_plan(&plan)
                            {
                                let reason = format!("Git integration stopped safely before canonical completion: {error}");
                                dragent_persistence::block_claimed_workspace_integration(
                                    &mut conn,
                                    attempt_id,
                                    &claim_owner,
                                    &reason,
                                )
                                .map_err(|block_error| {
                                    format!(
                                        "{reason}; could not record blocked state: {block_error}"
                                    )
                                })?;
                                return Err(reason);
                            }
                            if !dragent_persistence::release_workspace_integration_claim(
                                &mut conn,
                                attempt_id,
                                &claim_owner,
                            )
                            .map_err(|error| error.to_string())?
                            {
                                return Err("Integration ownership changed before acknowledgment; recovery will reconcile the durable intent.".into());
                            }
                            return Ok(());
                        }
                        #[cfg(target_family = "wasm")]
                        crate::git_status::worktree::integrate_agent_worktree(
                            &project_root,
                            root,
                            isolation,
                        )
                    })()
                }
                _ => unreachable!("non-Git review resolutions return before background apply"),
            };
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                if view.workspace_work_item_id != review_work {
                    if let Some(parked) = view.parked_workspace_work.get_mut(&review_work) {
                        parked.launch_pending = false;
                        parked.launch_pending_since = None;
                        if let Err(error) = result {
                            parked.record_principal_error(format!(
                                "Could not resolve isolated changes: {error}"
                            ));
                        }
                    }
                    ctx.notify();
                    return;
                }
                view.clear_launch_pending();
                match result {
                    Ok(()) => {
                        view.submit_workspace_runtime_command(completion_command.clone(), ctx);
                        view.refresh_document_tree(ctx);
                    }
                    Err(error) => {
                        view.shape_coordination_error =
                            Some(format!("Could not resolve isolated changes: {error}"));
                    }
                }
                ctx.notify();
            },
            |_, _| {},
        );
    }

    pub(crate) fn continue_principal_review_policy(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if self.workspace_launch_pending {
            return false;
        }
        if let Some((attempt_id, resolution)) =
            crate::agent::workspace_execution::pending_review_decisions(&self.workspace_state)
                .into_iter()
                .next()
        {
            let principal = ActorRef::principal_agent(self.workspace_state.project().id);
            self.resolve_workspace_execution_review_as(attempt_id, resolution, principal, ctx);
            return true;
        }
        false
    }

    /// The work item's agent conversation — the terminal agent's own
    /// `AgentController` + block renderer — created on first use. Its pane
    /// UUID is the work item id, so persistence lands in the same
    /// `agent_blocks` table a terminal pane uses and restores the same way.
    pub(crate) fn ensure_work_item_agent(
        &mut self,
        work_item_id: WorkItemId,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<crate::agent::conversation_view::AgentConversationView> {
        if let Some(existing) = self.work_item_agents.get(&work_item_id) {
            return existing.clone();
        }
        let root = self.work_item_project_root(work_item_id);
        let allowed_roots =
            crate::agent::permissions::AllowedRoots::for_session(&root, Some(root.as_path()));
        let api = crate::agent::default_agent_api();
        let controller = {
            let root = root.clone();
            ctx.add_model(move |mctx| {
                let mut controller =
                    crate::agent::AgentController::with_allowed_roots(api, allowed_roots, root);
                controller.start_background_watcher(mctx);
                controller
            })
        };
        // Same persistence wiring as `TerminalView::new_with_restore`.
        let sender = self.persistence_writer.sender();
        let scope_root = root.clone();
        let restored = self
            .workspace_boot_restored_agent_blocks
            .get(&work_item_id.0)
            .cloned()
            .unwrap_or_default();
        ctx.update_model(&controller, |controller, mctx| {
            controller.set_persistence(work_item_id.0, sender);
            controller.set_run_id(Some(principal_workspace_run_id(work_item_id)));
            controller.set_allowed_tools(Some(super::shape_principal::shaping_allowed_tools()));
            controller.set_tool_call_budget(Some(super::shape_principal::MAX_PRINCIPAL_FINDINGS));
            controller.set_chat_mode(crate::agent::controller::AgentRole::Default, false, true);
            controller.set_display_text_filter(Some(std::sync::Arc::new(
                |raw: &str, is_streaming: bool| {
                    super::shape_principal::display_text(raw, is_streaming)
                },
            )));
            // Architect / principal engineer: it can inspect every agent,
            // message them, retry tasks, and keep the knowledge ledger.
            controller.set_host_tools(super::principal_tools::principal_host_tools());
            if !restored.is_empty() {
                let true_max_sort_order = (|| -> anyhow::Result<Option<i32>> {
                    let org = dragent_persistence::current_org();
                    let _ = dragent_persistence::migrate_pane_to_root(
                        &scope_root,
                        &org,
                        work_item_id.0,
                    );
                    let path = dragent_persistence::db_path_for_read(&scope_root)?;
                    let mut conn = dragent_persistence::setup_connection(&path)?;
                    dragent_persistence::max_sort_order_for_pane(&mut conn, work_item_id.0)
                })()
                .unwrap_or_default();
                controller.restore_blocks_from_sql(restored, true_max_sort_order);
                mctx.emit(crate::agent::AgentControllerEvent::BlocksChanged);
            }
        });
        let theme = self.theme;
        let mono = self.mono_font;
        let agent = ctx.add_typed_action_view(|agent_ctx| {
            crate::agent::conversation_view::AgentConversationView::new(
                controller, theme, mono, agent_ctx,
            )
        });
        ctx.subscribe_to_view(&agent, move |view, _, event, ctx| {
            use crate::agent::conversation_view::AgentConversationEvent;
            match event {
                AgentConversationEvent::BlocksChanged => ctx.notify(),
                AgentConversationEvent::TurnCompleted {
                    assistant_text,
                    tool_calls,
                    ..
                } => {
                    if let Some(reply) = view.principal_pass_replies.remove(&work_item_id) {
                        let _ = reply.send(Ok(super::shape_principal::PrincipalPassOutcome {
                            content: assistant_text.clone(),
                            tool_calls: tool_calls.clone(),
                        }));
                    } else if let Some((epoch, target, routing)) =
                        view.principal_coordination_turns.remove(&work_item_id)
                    {
                        let result = if assistant_text.trim().is_empty() {
                            Err("Principal coordination turn ended without a response".to_string())
                        } else {
                            super::coordination_principal::parse_coordination_response(
                                assistant_text,
                            )
                        };
                        view.finish_principal_coordination_turn(
                            epoch, target, routing, result, ctx,
                        );
                    }
                    ctx.notify();
                }
                AgentConversationEvent::Error { message } => {
                    if let Some(reply) = view.principal_pass_replies.remove(&work_item_id) {
                        let _ = reply.send(Err(message.clone()));
                    } else if let Some((epoch, target, routing)) =
                        view.principal_coordination_turns.remove(&work_item_id)
                    {
                        view.finish_principal_coordination_turn(
                            epoch,
                            target,
                            routing,
                            Err(message.clone()),
                            ctx,
                        );
                    }
                    ctx.notify();
                }
                AgentConversationEvent::HostToolCall {
                    task_id,
                    call_id,
                    name,
                    args_json,
                } => {
                    if let Some((output, is_error)) = view.execute_principal_tool(
                        work_item_id,
                        task_id,
                        call_id,
                        name,
                        args_json,
                        ctx,
                    ) {
                        let task_id = task_id.clone();
                        let call_id = call_id.clone();
                        crate::bg::detach(move || {
                            if let Err(error) = crate::agent::pipeline::post_tool_result(
                                &task_id, &call_id, &output, is_error,
                            ) {
                                log::warn!("[principal-tools] post result {call_id}: {error}");
                            }
                        });
                    }
                    ctx.notify();
                }
                AgentConversationEvent::ConsentRequested { .. }
                | AgentConversationEvent::ApprovalResolved { .. }
                | AgentConversationEvent::QuestionOpened { .. }
                | AgentConversationEvent::QuestionAnswered { .. } => ctx.notify(),
            }
        });
        self.work_item_agents.insert(work_item_id, agent.clone());

        // Principal host-tool results use the same durable local-run checkpoint
        // as worker attempts. If the app stopped after the canonical workspace
        // write and its tool result were saved, resume the provider round now;
        // do not re-execute unresolved mutations.
        self.resume_checkpointed_principal(work_item_id, None, ctx);
        agent
    }

    pub(crate) fn resume_checkpointed_principal(
        &mut self,
        work_item_id: WorkItemId,
        execution_id: Option<&str>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(agent) = self.work_item_agents.get(&work_item_id).cloned() else {
            return;
        };
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        if controller.read(ctx, |controller, _| controller.is_busy()) {
            return;
        }
        let run_id = principal_workspace_run_id(work_item_id);
        if crate::agent::pipeline::local_run_ready_to_resume(&run_id, execution_id) {
            let settings = crate::settings::AppSettings::load();
            let model = if settings.selected_model == crate::settings::AUTO_MODEL_ID
                || settings.selected_model.trim().is_empty()
            {
                crate::settings::FREE_TIER_MODEL_ID.to_string()
            } else {
                settings.selected_model
            };
            ctx.update_model(&controller, |controller, mctx| {
                controller.recover_local_run(run_id, Vec::new(), model, mctx)
            });
        }
    }

    /// Run one Principal provider pass on the work item's controller. The
    /// pass that carries the user's message is a real user turn (their
    /// block, their attachments); every later pass of the same turn is an
    /// internal continuation. The controller streams, executes read tools,
    /// parks on ask_user, renders todowrite, persists — and reports back via
    /// `TurnCompleted`, which answers `reply`.
    pub(crate) fn run_principal_pass(
        &mut self,
        work_item_id: WorkItemId,
        request: super::shape_principal::PrincipalPassRequest,
        user_turn: Option<(String, Vec<crate::agent::attachments::MessageAttachment>)>,
        reply: std::sync::mpsc::Sender<
            Result<super::shape_principal::PrincipalPassOutcome, String>,
        >,
        ctx: &mut ViewContext<Self>,
    ) {
        let agent = self.ensure_work_item_agent(work_item_id, ctx);
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        // The controller refuses to start past its session cap (it pushes a
        // notice block and returns without a turn) — answer the orchestrator
        // instead of leaving it blocked on a reply that would never come.
        if controller.read(ctx, |c, _| c.turn_count())
            >= crate::agent::controller::MAX_SESSION_TURNS
        {
            let _ = reply.send(Err(format!(
                "Session limit reached ({} passes). Start a new work item to continue.",
                crate::agent::controller::MAX_SESSION_TURNS
            )));
            return;
        }
        // A pass still registered here was superseded (Stop / resubmit):
        // dropping its sender unblocks the old orchestrator as "stopped".
        self.principal_pass_replies.insert(work_item_id, reply);
        let super::shape_principal::PrincipalPassRequest {
            model,
            system,
            prompt,
            allowed_tools,
            note,
        } = request;
        ctx.update_model(&controller, |controller, mctx| {
            controller.set_allowed_tools(Some(allowed_tools));
            controller.set_tool_call_budget(Some(super::shape_principal::MAX_PRINCIPAL_FINDINGS));
            // The prompt scaffold already carries the canonical conversation
            // (WorkspaceState events); an empty provider history keeps the
            // model from seeing it twice — same request shape as before.
            controller.set_session_history(Vec::new());
            let system_builder: Box<dyn FnOnce() -> String + Send + 'static> =
                Box::new(move || system);
            match user_turn {
                Some((display_text, attachments)) => controller.start_streaming_query_with_display(
                    prompt,
                    display_text,
                    attachments,
                    None,
                    model,
                    system_builder,
                    mctx,
                ),
                None => controller.start_internal_continuation(
                    prompt,
                    note,
                    model,
                    system_builder,
                    mctx,
                ),
            }
        });
        ctx.update_view(&agent, |agent, _| agent.pin_to_bottom());
        ctx.notify();
    }

    /// Write a user/agent exchange that needed no provider pass (canned
    /// greeting reply) into the work item's conversation.
    pub(crate) fn push_principal_exchange(
        &mut self,
        work_item_id: WorkItemId,
        user_text: String,
        reply_text: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let agent = self.ensure_work_item_agent(work_item_id, ctx);
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        ctx.update_model(&controller, |controller, mctx| {
            controller.push_local_exchange(user_text, reply_text, mctx);
        });
        ctx.update_view(&agent, |agent, _| agent.pin_to_bottom());
    }
}
