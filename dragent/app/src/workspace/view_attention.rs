//! Workspace attention flows.
//!
//! Responsible for: worker questions, attention items, standing approvals,
//! agent tabs and timelines feeding attention UI.
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

impl WorkspaceView {
    /// Knowledge ledger + skills inventory injected into every worker's
    /// system prompt as trusted context.
    pub(crate) fn worker_trusted_context(&self) -> String {
        let mut sections = Vec::new();
        let notes = self.workspace_state.shaping_memory();
        if !notes.is_empty() {
            sections.push(format!(
                "### Shared knowledge ledger\nRecorded by the Principal and other agents on this work item. Read before researching; record what you learn with `record_knowledge`.\n{}",
                notes
                    .iter()
                    .map(|note| format!("- [{:?}] {}", note.kind, note.text))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        let skills = crate::agent::skill_discovery::skills_prompt_block(
            &self.workspace_state.project().root,
        );
        if !skills.trim().is_empty() {
            sections.push(skills);
        }
        sections.join("\n\n")
    }

    /// A worker's `ask_user` card goes to the Principal first: post the
    /// question as a DecisionRequest on the task thread and park the card on
    /// the reply (`deliver_principal_reply_to_worker` fills it in).
    pub(crate) fn route_worker_question_to_principal(
        &mut self,
        attempt_id: ExecutionAttemptId,
        action_id: Uuid,
        summary: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(attempt) = self.workspace_state.execution_attempt(attempt_id).cloned() else {
            return;
        };
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
        let sender = ActorRef::agent_instance(attempt.agent_instance_id, kind);
        let project_id = self.workspace_state.project().id;
        let principal = ActorRef::principal_agent(project_id).id;
        let now = chrono::Utc::now().timestamp_millis();
        let message = WorkspaceAgentMessage {
            id: AgentMessageId::new(),
            thread_id: task_coordination_thread_id(attempt.task_id),
            work_item_id: attempt.work_item_id,
            sender_id: sender.id,
            kind: AgentMessageKind::DecisionRequest,
            body: summary.to_string(),
            review_finding: None,
            reply_to: None,
            artifact_ids: Vec::new(),
            evidence_ids: Vec::new(),
            delivery_state: MessageDeliveryState::LocalOnly,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let message_id = message.id;
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id: attempt.work_item_id,
            actor_id: sender.id,
            device_id: None,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: None,
            payload: WorkspaceCommandPayload::PostMessage {
                message,
                routing: Some(AgentMessageRouting {
                    recipient_ids: vec![principal],
                    task_id: Some(attempt.task_id),
                    execution_attempt_id: Some(attempt.id),
                }),
            },
        };
        let command_id = command.command_id;
        match self.submit_workspace_runtime_command_retryable(command, ctx) {
            Ok(()) => {
                self.pending_worker_questions.insert(
                    message_id,
                    super::worker_tools::PendingWorkerQuestion::Card {
                        attempt_id,
                        action_id,
                    },
                );
                self.pending_worker_question_submissions
                    .insert(command_id, message_id);
            }
            Err(error) => {
                if let Some(agent) = self.attempt_agents.get(&attempt_id).cloned() {
                    let controller = agent.read(ctx, |view, _| view.controller().clone());
                    ctx.update_model(&controller, |controller, mctx| {
                        controller.answer_user_question_text(
                            action_id,
                            format!("Could not persist the question for the Principal: {error}"),
                            mctx,
                        )
                    });
                }
            }
        }
    }

    /// Mirror a controller's parked approval / question card as a durable
    /// Attention item: the scheduler blocks on it, Coordinate lists it, and
    /// its Open-agent route lands on the card itself.
    pub(crate) fn raise_host_attention(
        &mut self,
        attempt_id: ExecutionAttemptId,
        action_id: Uuid,
        kind: AttentionKind,
        call_id: &str,
        summary: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(launch) = self
            .workspace_execution_runtime
            .mounted_launch(attempt_id)
            .cloned()
        else {
            return;
        };
        // Already raised (e.g. the card was rebuilt by restart recovery while
        // its canonical item stayed open): just re-link the card to it.
        if let Some(existing) = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| {
                item.execution_attempt_id == Some(attempt_id)
                    && item.tool_call_id.as_deref() == Some(call_id)
                    && matches!(
                        item.status,
                        AttentionStatus::Open | AttentionStatus::Acknowledged
                    )
            })
        {
            self.attempt_attention
                .insert((attempt_id, action_id), existing.id);
            ctx.notify();
            return;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let attention_item = AttentionItem {
            id: AttentionItemId(Uuid::new_v5(&launch.attempt.id.0, call_id.as_bytes())),
            work_item_id: launch.work_item_id,
            task_id: Some(launch.attempt.task_id),
            thread_id: Some(task_coordination_thread_id(launch.attempt.task_id)),
            execution_attempt_id: Some(launch.attempt.id),
            tool_call_id: Some(call_id.to_string()),
            kind,
            status: AttentionStatus::Open,
            summary,
            requested_by: launch.actor_id,
            assigned_to: vec![launch.lease.charging.owner_id],
            created_at_unix_ms: now,
            resolved_at_unix_ms: None,
            resolution_kind: None,
            resolution: None,
        };
        self.attempt_attention
            .insert((attempt_id, action_id), attention_item.id);
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: launch.project_id,
            work_item_id: launch.work_item_id,
            actor_id: launch.actor_id,
            device_id: launch.device_id,
            correlation_id: launch.correlation_id,
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::RaiseAttention { attention_item },
        };
        self.submit_workspace_runtime_command(command, ctx);
    }

    /// The card was answered inside the agent's pane: resolve its mirrored
    /// Attention item so the scheduler unblocks and Coordinate clears it.
    pub(crate) fn resolve_host_attention(
        &mut self,
        attempt_id: ExecutionAttemptId,
        action_id: Uuid,
        approved: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(attention_item_id) = self.attempt_attention.remove(&(attempt_id, action_id))
        else {
            return;
        };
        let still_open = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| item.id == attention_item_id)
            .is_some_and(|item| {
                matches!(
                    item.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                )
            });
        if !still_open {
            return;
        }
        let Some(launch) = self
            .workspace_execution_runtime
            .mounted_launch(attempt_id)
            .cloned()
        else {
            return;
        };
        let (outcome, resolution) = if approved {
            (
                AttentionResolution::Approved,
                "Approved in the agent's conversation.",
            )
        } else {
            (
                AttentionResolution::Rejected,
                "Rejected in the agent's conversation.",
            )
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: launch.project_id,
            work_item_id: launch.work_item_id,
            actor_id: launch.lease.charging.owner_id,
            device_id: launch.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::ResolveAttention {
                attention_item_id,
                outcome,
                resolution: resolution.to_string(),
            },
        };
        self.submit_workspace_runtime_command(command, ctx);
    }

    /// An Attention item resolved from Coordinate for a controller-run
    /// attempt: forward the decision to the parked card. Returns true when
    /// the item belonged to a mounted controller.
    pub(crate) fn apply_attention_resolution_to_host(
        &mut self,
        attention_item: &AttentionItem,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(attempt_id) = attention_item.execution_attempt_id else {
            return false;
        };
        let Some(agent) = self.attempt_agents.get(&attempt_id).cloned() else {
            return false;
        };
        let Some(((_, action_id), _)) = self
            .attempt_attention
            .iter()
            .find(|((id, _), item)| *id == attempt_id && **item == attention_item.id)
            .map(|(k, v)| (*k, *v))
        else {
            return false;
        };
        self.attempt_attention.remove(&(attempt_id, action_id));
        let approved = matches!(
            attention_item.resolution_kind,
            Some(AttentionResolution::Approved)
        );
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        ctx.update_model(&controller, |controller, mctx| {
            if attention_item.kind == AttentionKind::Permission {
                if controller.is_streaming() {
                    controller.resolve_streaming_approval(action_id, approved, None, mctx);
                } else if approved {
                    let _ = controller.approve_action(action_id, mctx);
                } else {
                    controller.reject_action(action_id, None, mctx);
                }
            } else if !approved {
                // A question dismissed from Coordinate: retire the card so the
                // stream does not wait forever on an answer nobody will give.
                controller.dismiss_open_question();
                controller.cancel_stream(mctx);
            }
        });
        true
    }

    /// A mounted attempt reached its terminal outcome (host-run controller
    /// turn ended, or a legacy session finished): capture isolated changes,
    /// reject false empty successes, and submit the canonical finish.
    pub(crate) fn finish_mounted_attempt(
        &mut self,
        attempt_id: ExecutionAttemptId,
        completion: crate::agent::workspace_execution::WorkspaceExecutionCompletion,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some((worktree, isolation)) = self
            .workspace_execution_runtime
            .change_capture_request(attempt_id)
        {
            self.mark_launch_pending();
            let captured_at = chrono::Utc::now().timestamp_millis();
            let (tx, rx) = async_channel::bounded(1);
            crate::bg::detach(move || {
                let result = crate::git_status::worktree::capture_agent_change_summary(
                    &worktree,
                    &isolation,
                    captured_at,
                );
                let _ = tx.send_blocking((completion, result));
            });
            ctx.spawn_stream_local(
                rx,
                move |view, (mut completion, result), ctx| {
                    view.clear_launch_pending();
                    match result {
                        Ok(summary) => completion.change_summary = Some(summary),
                        Err(error) => {
                            view.shape_coordination_error = Some(format!(
                                "Could not capture isolated agent changes: {error}"
                            ));
                            completion.outcome = dragent_workspace_model::ExecutionAttemptStatus::Failed;
                            completion.summary = format!(
                                "Execution changes could not be captured safely: {error}"
                            );
                        }
                    }
                    // A writable worker that reports success with zero
                    // captured files is a false success: it would sit
                    // in Reviewing with "0 changed files ready for
                    // integration" and block the DAG. Fail it so the
                    // deterministic retry + Principal attention path
                    // runs instead.
                    if completion.outcome
                        == dragent_workspace_model::ExecutionAttemptStatus::Succeeded
                    {
                        let empty_writable_success = completion
                            .change_summary
                            .as_ref()
                            .is_some_and(|summary| summary.files.is_empty())
                            && view
                                .workspace_state
                                .execution_attempt(attempt_id)
                                .and_then(|attempt| {
                                    view.workspace_state
                                        .latest_structured_plan()?
                                        .tasks
                                        .iter()
                                        .find(|task| task.id == attempt.task_id)?
                                        .assigned_contract_id
                                })
                                .and_then(|contract_id| {
                                    view.workspace_state.work_contract(contract_id)
                                })
                                .is_some_and(|contract| {
                                    !contract.write_scope.is_empty()
                                });
                        if empty_writable_success {
                            completion.outcome =
                                dragent_workspace_model::ExecutionAttemptStatus::Failed;
                            completion.summary = format!(
                                "Blocked: the agent reported success but captured zero file changes in its isolated worktree. {}",
                                completion.summary.trim()
                            );
                        }
                    }
                    match view.workspace_execution_runtime.completion_command(
                        attempt_id,
                        completion,
                        view.workspace_canonical_revision,
                        chrono::Utc::now().timestamp_millis(),
                    ) {
                        Ok(command) => view.submit_workspace_runtime_command(command, ctx),
                        Err(error) => {
                            view.shape_coordination_error = Some(format!(
                                "Could not finish execution: {error}"
                            ));
                        }
                    }
                    ctx.notify();
                },
                |_, _| {},
            );
        } else {
            match self.workspace_execution_runtime.completion_command(
                attempt_id,
                completion,
                self.workspace_canonical_revision,
                chrono::Utc::now().timestamp_millis(),
            ) {
                Ok(command) => self.submit_workspace_runtime_command(command, ctx),
                Err(error) => {
                    self.shape_coordination_error =
                        Some(format!("Could not finish execution: {error}"));
                }
            }
        }
    }

    pub(crate) fn active_workspace_agent_tab(&self) -> Option<&super::editor_tabs::AgentEditorTab> {
        let id = self.workspace_active_agent_tab?;
        if !self.work_item_tab_active || id.work_item_id != self.workspace_work_item_id {
            return None;
        }
        super::editor_tabs::agent_tab_index(&self.workspace_agent_tabs, id)
            .map(|index| &self.workspace_agent_tabs[index])
    }

    pub(crate) fn select_workspace_agent_tab(
        &mut self,
        id: super::action::WorkspaceAgentTabId,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.work_item_tab_open
            || !self.work_item_tab_active
            || id.work_item_id != self.workspace_work_item_id
            || super::editor_tabs::agent_tab_index(&self.workspace_agent_tabs, id).is_none()
        {
            return;
        }
        self.workspace_active_agent_tab = Some(id);
        ctx.notify();
    }

    pub(super) fn resolve_workspace_attention(
        &mut self,
        attention_item_id: AttentionItemId,
        approve: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(attention) = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| item.id == attention_item_id)
            .cloned()
        else {
            self.shape_coordination_error =
                Some("This attention item is no longer available.".into());
            ctx.notify();
            return;
        };
        if !matches!(
            attention.status,
            AttentionStatus::Open | AttentionStatus::Acknowledged
        ) {
            self.shape_coordination_error =
                Some("This attention item has already been resolved.".into());
            ctx.notify();
            return;
        }

        // A manual rejection revokes any standing "always allow" for the
        // same tool: the human changed their mind, so future prompts must
        // surface again instead of auto-approving.
        if !approve && attention.kind == AttentionKind::Permission {
            self.revoke_standing_tool_approval(&attention);
        }
        // Permission items approve or reject a parked tool call. Every other
        // kind (Failure, Budget, Decision, Review, Conflict) carries its next
        // step in the summary; resolving records what the human did without
        // mutating execution state outside typed commands.
        let (outcome, resolution) = if attention.kind == AttentionKind::Permission {
            if approve {
                (
                    AttentionResolution::Approved,
                    "Approved once by the workspace owner.",
                )
            } else {
                (
                    AttentionResolution::Rejected,
                    "Rejected by the workspace owner.",
                )
            }
        } else if approve {
            (
                AttentionResolution::Answered,
                "Marked resolved by the workspace owner after following the listed next step.",
            )
        } else {
            (
                AttentionResolution::Dismissed,
                "Dismissed by the workspace owner.",
            )
        };
        self.submit_attention_resolution(&attention, outcome, resolution.into(), ctx);
    }

    /// Shared validation + canonical submit for every attention resolution
    /// path (rail buttons, floating popover, standing approvals).
    pub(crate) fn submit_attention_resolution(
        &mut self,
        attention: &AttentionItem,
        outcome: AttentionResolution,
        resolution: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let project_id = self.workspace_state.project().id;
        let actor = ActorRef::local_user(project_id, None);
        if !attention.assigned_to.contains(&actor.id) {
            self.shape_coordination_error =
                Some("Only the assigned workspace owner can resolve this request.".into());
            ctx.notify();
            return;
        }
        log::info!(
            "[workspace] resolving attention item_id={:?} outcome={:?}",
            attention.id,
            outcome
        );
        let now = chrono::Utc::now()
            .timestamp_millis()
            .max(attention.created_at_unix_ms);
        self.submit_workspace_runtime_command(
            WorkspaceCommandEnvelope {
                command_id: WorkspaceCommandId::new(),
                schema_version: WORKSPACE_PROTOCOL_VERSION,
                project_id,
                work_item_id: attention.work_item_id,
                actor_id: actor.id,
                device_id: actor.device_id,
                correlation_id: CorrelationId::new(),
                timestamp_unix_ms: now,
                expected_revision: Some(self.workspace_canonical_revision),
                payload: WorkspaceCommandPayload::ResolveAttention {
                    attention_item_id: attention.id,
                    outcome,
                    resolution,
                },
            },
            ctx,
        );
    }

    /// Resolve with the human's own words. Empty responses fall back to the
    /// canned path. Permission items never take this path: only
    /// Approved/Rejected unpark a parked tool call.
    pub(crate) fn resolve_workspace_attention_with_response(
        &mut self,
        attention_item_id: AttentionItemId,
        approve: bool,
        response: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(attention) = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| item.id == attention_item_id)
            .cloned()
        else {
            self.shape_coordination_error =
                Some("This attention item is no longer available.".into());
            ctx.notify();
            return;
        };
        if !matches!(
            attention.status,
            AttentionStatus::Open | AttentionStatus::Acknowledged
        ) {
            self.shape_coordination_error =
                Some("This attention item has already been resolved.".into());
            ctx.notify();
            return;
        }
        let trimmed = response.trim();
        if trimmed.is_empty() || attention.kind == AttentionKind::Permission {
            self.resolve_workspace_attention(attention_item_id, approve, ctx);
            return;
        }
        let (outcome, prefix) = if approve {
            (
                AttentionResolution::Answered,
                "Answered by the workspace owner",
            )
        } else {
            (
                AttentionResolution::Dismissed,
                "Dismissed by the workspace owner",
            )
        };
        self.submit_attention_resolution(&attention, outcome, format!("{prefix}: {trimmed}"), ctx);
    }

    /// Open attention across the active and parked work items, oldest
    /// first, with the context the navbar bell dropdown shows. Canonical
    /// state only — presentation never filters what needs the human.
    pub(crate) fn open_attention_entries(
        &self,
    ) -> Vec<super::execution_workspace::AttentionOverlayEntry> {
        let mut states: Vec<&WorkspaceState> = vec![&self.workspace_state];
        states.extend(
            self.parked_workspace_work
                .values()
                .map(|parked| &parked.state),
        );
        let mut entries = Vec::new();
        for state in states {
            let work_title = state.active_work().map(|work| work.title.clone());
            let plan_tasks = state
                .latest_structured_plan()
                .map(|plan| plan.tasks.clone())
                .unwrap_or_default();
            for item in state.attention_items() {
                if !matches!(
                    item.status,
                    AttentionStatus::Open | AttentionStatus::Acknowledged
                ) {
                    continue;
                }
                // Parked runtimes swap with their work item, so only the
                // active runtime can name a parked call; others show no
                // tool rather than a wrong one.
                let tool_name = self.attention_tool_name(item);
                let task_title = item.task_id.and_then(|task_id| {
                    plan_tasks
                        .iter()
                        .find(|task| task.id == task_id)
                        .map(|task| task.title.clone())
                });
                entries.push(super::execution_workspace::AttentionOverlayEntry {
                    item: item.clone(),
                    tool_name,
                    task_title,
                    work_title: work_title.clone(),
                });
            }
        }
        entries.sort_by_key(|entry| entry.item.created_at_unix_ms);
        entries
    }

    /// Tool name behind a permission attention item, for standing approvals
    /// and popover context.
    pub(crate) fn attention_tool_name(&self, attention: &AttentionItem) -> Option<String> {
        let (attempt_id, call_id) = match (
            attention.execution_attempt_id,
            attention.tool_call_id.as_deref(),
        ) {
            (Some(attempt_id), Some(call_id)) => (attempt_id, call_id),
            _ => return None,
        };
        self.workspace_execution_runtime
            .parked_consent_tool_name(attempt_id, call_id)
    }

    /// Approve once and remember (tool) for this work item so future consent
    /// requests auto-approve through the same canonical resolve path.
    pub(crate) fn always_allow_workspace_tool(
        &mut self,
        attention_item_id: AttentionItemId,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(attention) = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| item.id == attention_item_id)
            .cloned()
        else {
            self.shape_coordination_error =
                Some("This attention item is no longer available.".into());
            ctx.notify();
            return;
        };
        if attention.kind != AttentionKind::Permission {
            self.shape_coordination_error =
                Some("Standing approvals only apply to tool permission requests.".into());
            ctx.notify();
            return;
        }
        let Some(tool_name) = self.attention_tool_name(&attention) else {
            self.shape_coordination_error =
                Some("Could not identify the tool behind this request.".into());
            ctx.notify();
            return;
        };
        self.workspace_standing_tool_approvals
            .insert((attention.work_item_id, tool_name));
        self.resolve_workspace_attention(attention_item_id, true, ctx);
    }

    pub(crate) fn revoke_standing_tool_approval(&mut self, attention: &AttentionItem) {
        let tool_name = self.attention_tool_name(attention);
        if let Some(tool_name) = tool_name {
            self.workspace_standing_tool_approvals
                .remove(&(attention.work_item_id, tool_name));
        }
    }

    /// Switch to the attention item's work item and open its Conversation
    /// screen — the Coordinate screen is read-only now, so this is the only
    /// place a human can act on the request. Used by the navbar bell
    /// dropdown: one click lands on the exact work and surface that needs
    /// the human. The open Blocker/DecisionRequest thread renders expanded
    /// by default there, with its inline accept control right on it.
    pub(crate) fn route_to_workspace_attention(
        &mut self,
        attention_item_id: AttentionItemId,
        ctx: &mut ViewContext<Self>,
    ) {
        let located = self
            .workspace_state
            .attention_items()
            .into_iter()
            .find(|item| item.id == attention_item_id)
            .map(|item| (item.work_item_id, item.task_id, item.execution_attempt_id))
            .or_else(|| {
                self.parked_workspace_work
                    .values()
                    .flat_map(|parked| parked.state.attention_items())
                    .find(|item| item.id == attention_item_id)
                    .map(|item| (item.work_item_id, item.task_id, item.execution_attempt_id))
            });
        let work_item_id = located.map(|(work_item_id, _, _)| work_item_id);
        log::info!(
            "[workspace] routing to attention item_id={:?}",
            attention_item_id
        );
        let Some(work_item_id) = work_item_id else {
            self.shape_coordination_error =
                Some("This attention item is no longer available.".into());
            ctx.notify();
            return;
        };
        let Some(tab_id) = self
            .tabs
            .iter()
            .find(|tab| tab.work_item_id == Some(work_item_id))
            .map(|tab| tab.id)
        else {
            self.shape_coordination_error =
                Some("The work item for this request is not open in a tab.".into());
            ctx.notify();
            return;
        };
        self.attention_bell_open = false;
        self.attention_popover_item = None;
        self.exit_split_unless_contains(tab_id);
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.active_tab_id = tab_id;
        self.restore_document_owner_tab(tab_id, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        // A request raised by a running agent (permission, question) is
        // answered on the card in that agent's own pane; anything else lands
        // in the Principal conversation.
        match located {
            Some((_, task_id, Some(attempt_id))) => {
                self.open_workspace_agent_timeline(task_id, Some(attempt_id), ctx);
            }
            _ => self.open_work_item_tab(WorkItemView::Conversation, ctx),
        }
    }

    /// Auto-approve open permission items covered by a standing approval.
    /// Runs inside the execution poll so each approval still travels the
    /// canonical ResolveAttention command path with full history.
    pub(crate) fn auto_approve_standing_attention(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_standing_tool_approvals.is_empty() {
            return;
        }
        let project_id = self.workspace_state.project().id;
        let owner = ActorRef::local_user(project_id, None).id;
        let due: Vec<AttentionItemId> = self
            .workspace_state
            .attention_items()
            .into_iter()
            .filter(|item| {
                item.kind == AttentionKind::Permission
                    && matches!(
                        item.status,
                        AttentionStatus::Open | AttentionStatus::Acknowledged
                    )
                    && item.assigned_to.contains(&owner)
            })
            .filter(|item| {
                self.attention_tool_name(item).is_some_and(|tool| {
                    self.workspace_standing_tool_approvals
                        .contains(&(item.work_item_id, tool))
                })
            })
            .map(|item| item.id)
            .collect();
        for attention_item_id in due {
            log::debug!("[workspace] auto-approving standing tool approval");
            self.resolve_workspace_attention(attention_item_id, true, ctx);
        }
    }
}
