//! Workspace execution control.
//!
//! Responsible for: cancel/pause, runtime submissions, live coordination
//! messages, pending flags, integration reconciliation.
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
use super::view::PRINCIPAL_ESCALATED_REPLY;
use super::view_state::{PendingWorkspaceToolAcknowledgement, WorkspaceIntegrationReconciliation};

impl WorkspaceView {
    pub(crate) fn cancel_workspace_execution(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_launch_pending {
            return;
        }
        let Some(attempt_id) = self
            .workspace_execution_runtime
            .active_attempt_ids()
            .into_iter()
            .next()
        else {
            self.shape_coordination_error = Some("There is no active execution to cancel.".into());
            ctx.notify();
            return;
        };
        match self.workspace_execution_runtime.cancellation_command(
            attempt_id,
            self.workspace_canonical_revision,
            chrono::Utc::now().timestamp_millis(),
        ) {
            Ok(command) => self.submit_workspace_runtime_command(command, ctx),
            Err(error) => {
                self.shape_coordination_error =
                    Some(format!("Could not cancel execution: {error}"));
                ctx.notify();
            }
        }
    }

    pub(crate) fn toggle_workspace_execution_pause(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_launch_pending {
            return;
        }
        if self
            .workspace_state
            .active_work()
            .is_some_and(|work| work.status == WorkStatus::Paused)
        {
            let project_id = self.workspace_state.project().id;
            let Some(work_item_id) = self.workspace_state.active_work().map(|work| work.id) else {
                return;
            };
            let actor = ActorRef::local_user(project_id, None);
            self.submit_workspace_runtime_command(
                WorkspaceCommandEnvelope {
                    command_id: WorkspaceCommandId::new(),
                    schema_version: WORKSPACE_PROTOCOL_VERSION,
                    project_id,
                    work_item_id,
                    actor_id: actor.id,
                    device_id: actor.device_id,
                    correlation_id: CorrelationId::new(),
                    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
                    expected_revision: Some(self.workspace_canonical_revision),
                    payload: WorkspaceCommandPayload::TransitionWork {
                        status: WorkStatus::Executing,
                    },
                },
                ctx,
            );
            return;
        }
        let Some(attempt_id) = self
            .workspace_execution_runtime
            .active_attempt_ids()
            .into_iter()
            .next()
        else {
            self.shape_coordination_error = Some("There is no active execution to pause.".into());
            ctx.notify();
            return;
        };
        match self.workspace_execution_runtime.pause_command(
            attempt_id,
            self.workspace_canonical_revision,
            chrono::Utc::now().timestamp_millis(),
        ) {
            Ok(command) => self.submit_workspace_runtime_command(command, ctx),
            Err(error) => {
                self.shape_coordination_error = Some(format!("Could not pause execution: {error}"));
                ctx.notify();
            }
        }
    }

    pub(super) fn submit_workspace_runtime_command(
        &mut self,
        command: WorkspaceCommandEnvelope,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Err(error) = self.try_submit_workspace_runtime_commands(vec![command], None, ctx) {
            self.record_principal_error(error.clone(), ctx);
            self.shape_coordination_error = Some(error);
            ctx.notify();
        }
    }

    /// Retryable form for callers whose own protocol result must reflect
    /// whether the command was actually handed to the persistence authority.
    pub(super) fn submit_workspace_runtime_command_retryable(
        &mut self,
        command: WorkspaceCommandEnvelope,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        self.try_submit_workspace_runtime_commands(vec![command], None, ctx)
    }

    pub(super) fn submit_workspace_runtime_commands_retryable(
        &mut self,
        commands: Vec<WorkspaceCommandEnvelope>,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        self.try_submit_workspace_runtime_commands(commands, None, ctx)
    }

    /// Submit a command for a host tool and answer that tool only after the
    /// canonical transaction returns. A concurrent mutation is rejected as
    /// explicitly retryable; it is never silently dropped or implicitly
    /// rebased onto state the caller did not inspect.
    pub(super) fn submit_workspace_runtime_command_with_acknowledgement(
        &mut self,
        command: WorkspaceCommandEnvelope,
        runtime_task_id: String,
        call_id: String,
        success: String,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let command_id = command.command_id;
        let identity = crate::agent::pipeline::local_tool_call_identity(&runtime_task_id, &call_id)
            .map_err(|error| error.to_string())?;
        let durable_outcome = dragent_persistence::WorkspaceToolOutcome {
            command_id,
            project_id: command.project_id,
            work_item_id: command.work_item_id,
            runtime_run_id: runtime_task_id.clone(),
            runtime_execution_id: identity.execution_id.clone(),
            runtime_round: identity.round,
            provider_call_id: call_id.clone(),
            output: Some(success.clone()),
            is_error: Some(false),
            source_message_id: None,
            accepted_at_unix_ms: command.timestamp_unix_ms,
            delivered_at_unix_ms: None,
            terminal_reason: None,
        };
        self.pending_workspace_tool_acknowledgements.insert(
            command_id,
            PendingWorkspaceToolAcknowledgement {
                runtime_task_id,
                runtime_execution_id: identity.execution_id,
                runtime_round: identity.round,
                call_id,
                success,
                resolved: None,
                delivery_in_flight: false,
                durable: true,
                delivery_attempts: 0,
            },
        );
        if let Err(error) =
            self.try_submit_workspace_runtime_commands(vec![command], Some(durable_outcome), ctx)
        {
            self.pending_workspace_tool_acknowledgements
                .remove(&command_id);
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn submit_workspace_runtime_commands_with_acknowledgement(
        &mut self,
        commands: Vec<WorkspaceCommandEnvelope>,
        runtime_task_id: String,
        call_id: String,
        success: String,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let first = commands
            .first()
            .ok_or_else(|| "workspace command batch is empty".to_string())?;
        let command_id = first.command_id;
        let identity = crate::agent::pipeline::local_tool_call_identity(&runtime_task_id, &call_id)
            .map_err(|error| error.to_string())?;
        let durable_outcome = dragent_persistence::WorkspaceToolOutcome {
            command_id,
            project_id: first.project_id,
            work_item_id: first.work_item_id,
            runtime_run_id: runtime_task_id.clone(),
            runtime_execution_id: identity.execution_id.clone(),
            runtime_round: identity.round,
            provider_call_id: call_id.clone(),
            output: Some(success.clone()),
            is_error: Some(false),
            source_message_id: None,
            accepted_at_unix_ms: first.timestamp_unix_ms,
            delivered_at_unix_ms: None,
            terminal_reason: None,
        };
        self.pending_workspace_tool_acknowledgements.insert(
            command_id,
            PendingWorkspaceToolAcknowledgement {
                runtime_task_id,
                runtime_execution_id: identity.execution_id,
                runtime_round: identity.round,
                call_id,
                success,
                resolved: None,
                delivery_in_flight: false,
                durable: true,
                delivery_attempts: 0,
            },
        );
        if let Err(error) =
            self.try_submit_workspace_runtime_commands(commands, Some(durable_outcome), ctx)
        {
            self.pending_workspace_tool_acknowledgements
                .remove(&command_id);
            return Err(error);
        }
        Ok(())
    }

    /// Persist a DecisionRequest and the waiting provider call together. Its
    /// outbox row has no result until a Principal reply referencing the message
    /// commits; that reply transaction fills the durable outcome atomically.
    pub(super) fn submit_workspace_runtime_question_with_acknowledgement(
        &mut self,
        command: WorkspaceCommandEnvelope,
        message_id: AgentMessageId,
        runtime_task_id: String,
        call_id: String,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let command_id = command.command_id;
        let identity = crate::agent::pipeline::local_tool_call_identity(&runtime_task_id, &call_id)
            .map_err(|error| error.to_string())?;
        let durable_outcome = dragent_persistence::WorkspaceToolOutcome {
            command_id,
            project_id: command.project_id,
            work_item_id: command.work_item_id,
            runtime_run_id: runtime_task_id,
            runtime_execution_id: identity.execution_id,
            runtime_round: identity.round,
            provider_call_id: call_id,
            output: None,
            is_error: None,
            source_message_id: Some(message_id.0),
            accepted_at_unix_ms: command.timestamp_unix_ms,
            delivered_at_unix_ms: None,
            terminal_reason: None,
        };
        self.try_submit_workspace_runtime_commands(vec![command], Some(durable_outcome), ctx)
    }

    pub(crate) fn approve_plan_and_coordinate(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shape_coordination_pending {
            let now = chrono::Utc::now().timestamp_millis();
            let started = self.shape_coordination_pending_since.unwrap_or(now);
            if now.saturating_sub(started) >= Self::COORDINATION_REQUEST_TIMEOUT_MS {
                log::warn!("[workspace] coordination request timed out; clearing stuck flag");
                self.clear_coordination_pending();
                self.shape_coordination_error = Some(
                    "The coordination request timed out without an answer. It is safe to press Start Coordination again.".into(),
                );
                ctx.notify();
                return;
            }
            // Genuinely in flight: the button renders its Working state.
            // Notify so the state paints even if this frame would not.
            log::debug!("[workspace] coordination request already in flight; ignoring duplicate");
            ctx.notify();
            return;
        }
        let Some(work) = self.workspace_state.active_work().cloned() else {
            self.shape_coordination_error = Some("Create and shape a work item first.".into());
            ctx.notify();
            return;
        };
        let readiness = self.workspace_state.coordination_readiness();
        if !readiness.is_approvable() {
            let remaining = readiness
                .role_count
                .saturating_sub(readiness.approved_role_count);
            self.shape_coordination_error = Some(if readiness.role_count == 0 {
                "A proposed team is required before coordination.".into()
            } else if remaining == 0 {
                "Resolve the remaining shaping decisions or plan issues first.".into()
            } else {
                format!("Approve the remaining {remaining} role contract(s) first.")
            });
            ctx.notify();
            return;
        }
        // Every task needs a valid model route before coordination: Auto
        // is always valid (server-resolved), anything else must fit the
        // account tier and the shared catalog.
        if let Err(message) = self.validate_plan_model_routes(ctx) {
            self.shape_coordination_error = Some(message);
            ctx.notify();
            return;
        }
        if work.status == WorkStatus::ReadyToExecute {
            self.shape_coordination_error = None;
            self.shape_recovery_notice = None;
            self.open_work_item_tab(WorkItemView::Coordinate, ctx);
            self.continue_workspace_launch(ctx);
            return;
        }
        if work.status.has_started_coordination() {
            // Already running: nothing to approve or launch, just navigate
            // to the execution view.
            self.open_work_item_tab(WorkItemView::Coordinate, ctx);
            return;
        }
        if work.status != WorkStatus::Planning {
            self.shape_coordination_error = Some(format!(
                "Work must be in Planning before coordination (currently {:?}).",
                work.status
            ));
            ctx.notify();
            return;
        }

        let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
            self.shape_coordination_error =
                Some("A structured dependency plan is required before coordination.".into());
            ctx.notify();
            return;
        };
        if plan.status == dragent_workspace_model::PlanStatus::Superseded {
            self.shape_coordination_error =
                Some("The current dependency plan was superseded and must be reshaped.".into());
            ctx.notify();
            return;
        }

        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        let project_id = self.workspace_state.project().id;
        let work_item_id = work.id;
        let correlation_id = CorrelationId::new();
        let timestamp_unix_ms = chrono::Utc::now().timestamp_millis();
        let starting_revision = self.workspace_canonical_revision;
        let approve_plan = plan.status != dragent_workspace_model::PlanStatus::Approved;
        if approve_plan {
            plan.version = plan.version.saturating_add(1);
            plan.status = dragent_workspace_model::PlanStatus::Approved;
            plan.approved_by = Some(actor.id);
            plan.approved_at_unix_ms = Some(timestamp_unix_ms);
            plan.updated_at_unix_ms = timestamp_unix_ms;
        }
        self.mark_coordination_pending();
        self.shape_coordination_error = None;
        self.shape_coordination_epoch = self.shape_coordination_epoch.wrapping_add(1);
        let epoch = self.shape_coordination_epoch;
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = (|| {
                let mut revision = starting_revision;
                let mut events = Vec::new();
                if approve_plan {
                    let mut accepted =
                        writer.accept_workspace_command(WorkspaceCommandEnvelope {
                            command_id: WorkspaceCommandId::new(),
                            schema_version: WORKSPACE_PROTOCOL_VERSION,
                            project_id,
                            work_item_id,
                            actor_id: actor.id,
                            device_id: actor.device_id,
                            correlation_id,
                            timestamp_unix_ms,
                            expected_revision: Some(revision),
                            payload: WorkspaceCommandPayload::UpsertPlan { plan },
                        })?;
                    // The authority returns the events it committed; project
                    // every one rather than demanding an exact count, so a
                    // future extra event can't fail an approved plan with a
                    // confusing count mismatch. Empty is still an error.
                    if accepted.is_empty() {
                        anyhow::bail!("authority returned no plan approval events; expected one");
                    }
                    revision = accepted
                        .last()
                        .map(|event| event.sequence)
                        .unwrap_or(revision);
                    events.append(&mut accepted);
                }
                let mut accepted = writer.accept_workspace_command(WorkspaceCommandEnvelope {
                    command_id: WorkspaceCommandId::new(),
                    schema_version: WORKSPACE_PROTOCOL_VERSION,
                    project_id,
                    work_item_id,
                    actor_id: actor.id,
                    device_id: actor.device_id,
                    correlation_id,
                    timestamp_unix_ms,
                    expected_revision: Some(revision),
                    payload: WorkspaceCommandPayload::TransitionWork {
                        status: WorkStatus::ReadyToExecute,
                    },
                })?;
                // Project every committed event (see above): only an empty
                // outcome is an error.
                if accepted.is_empty() {
                    anyhow::bail!("authority returned no coordination events; expected one");
                }
                events.append(&mut accepted);
                Ok(events)
            })()
            .map_err(|error: anyhow::Error| {
                format!("Could not approve the plan and prepare coordination: {error:#}")
            });
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                if view.shape_coordination_epoch != epoch {
                    log::debug!("[workspace] ignoring stale coordination response");
                    return;
                }
                view.clear_coordination_pending();
                match result {
                    Ok(events) if !events.is_empty() => {
                        let principal_id =
                            ActorRef::principal_agent(view.workspace_state.project().id).id;
                        let mut projection_failed = false;
                        for event in events {
                            view.workspace_canonical_revision =
                                view.workspace_canonical_revision.max(event.sequence);
                            let Some(projected) =
                                dragent_session_protocol::workspace::project_envelope_event(
                                    event,
                                    principal_id,
                                )
                            else {
                                projection_failed = true;
                                break;
                            };
                            view.workspace_state
                                .record(projected.timestamp_unix_ms, projected.kind);
                        }
                        if !projection_failed {
                            view.shape_coordination_error = None;
                            view.shape_recovery_notice = None;
                            view.open_work_item_tab(WorkItemView::Coordinate, ctx);
                            view.continue_workspace_launch(ctx);
                        } else {
                            view.shape_coordination_error =
                                Some("Authority returned an unexpected coordination event".into());
                        }
                    }
                    Ok(events) => {
                        view.shape_coordination_error = Some(format!(
                            "Authority returned {} coordination events; expected one",
                            events.len()
                        ));
                    }
                    Err(error) => view.shape_coordination_error = Some(error),
                }
                ctx.notify();
            },
            |_, _| {},
        );
        ctx.notify();
    }

    pub(crate) fn try_submit_workspace_runtime_commands(
        &mut self,
        commands: Vec<WorkspaceCommandEnvelope>,
        tool_outcome: Option<dragent_persistence::WorkspaceToolOutcome>,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let command_work_item_ids = commands
            .iter()
            .map(|command| command.work_item_id)
            .collect::<Vec<_>>();
        let submitted_work_item_id = validate_runtime_submission_scope(
            self.workspace_launch_pending,
            self.workspace_work_item_id,
            &command_work_item_ids,
        )?;
        let command_ids = commands
            .iter()
            .map(|command| command.command_id)
            .collect::<Vec<_>>();
        self.mark_launch_pending();
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = writer
                .accept_workspace_commands_with_tool_outcome(commands, tool_outcome)
                .map_err(|error| format!("Could not persist execution state: {error:#}"));
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                let acknowledgement_outcome = result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| error.clone());
                view.finish_workspace_tool_acknowledgements(
                    &command_ids,
                    &acknowledgement_outcome,
                    ctx,
                );
                view.finish_worker_question_submissions(
                    &command_ids,
                    &acknowledgement_outcome,
                    ctx,
                );
                if acknowledgement_outcome.is_ok() {
                    view.reconcile_workspace_tool_outcomes(ctx);
                }

                // A tab switch may park the originating work item while its
                // SQLite transaction is in flight. Never apply those events to
                // the newly active item. Its bounded projection is updated in
                // place and ordinary activation reconciliation resumes driving
                // any execution when the user returns.
                if view.workspace_work_item_id != submitted_work_item_id {
                    let parked_principal_replies = result
                        .as_ref()
                        .map(|events| principal_replies_for_delivery(events))
                        .unwrap_or_default();
                    for message in parked_principal_replies {
                        view.deliver_principal_reply_to_worker(&message, ctx);
                    }
                    if let Some(parked) = view
                        .parked_workspace_work
                        .get_mut(&submitted_work_item_id)
                    {
                        parked.launch_pending = false;
                        parked.launch_pending_since = None;
                        match result {
                            Ok(events) => {
                                let principal_id =
                                    ActorRef::principal_agent(parked.state.project().id).id;
                                for event in events {
                                    parked.canonical_revision =
                                        parked.canonical_revision.max(event.sequence);
                                    if let Some(projected) =
                                        dragent_session_protocol::workspace::project_envelope_event(event, principal_id)
                                    {
                                        parked
                                            .state
                                            .record(projected.timestamp_unix_ms, projected.kind);
                                    }
                                }
                            }
                            Err(error) => parked.record_principal_error(error),
                        }
                    }
                    ctx.notify();
                    return;
                }
                view.clear_launch_pending();
                match result {
                    Ok(events) => {
                        let principal_id =
                            ActorRef::principal_agent(view.workspace_state.project().id).id;
                        let mut resolved_tools = Vec::new();
                        let mut resolved_review_roots = Vec::new();
                        let mut execution_communication = None;
                        let mut principal_coordination_resolution_reply = None;
                        for event in events {
                            view.workspace_canonical_revision =
                                view.workspace_canonical_revision.max(event.sequence);
                            if let WorkspaceEventPayload::AttentionUpdated { attention_item } =
                                &event.payload
                            {
                                if attention_item.status == AttentionStatus::Resolved
                                    && !view.apply_attention_resolution_to_host(attention_item, ctx)
                                {
                                    if let Some(action) = view
                                        .workspace_execution_runtime
                                        .observe_attention_resolution(&event)
                                    {
                                        resolved_tools.push(action);
                                    }
                                }
                            }
                            // A Principal reply on a task thread may answer a
                            // parked worker question (tool call or card);
                            // an escalation placeholder waits for the human.
                            if let WorkspaceEventPayload::MessagePosted { message, routing } =
                                &event.payload
                            {
                                if message.sender_id == principal_id
                                    && message.body != PRINCIPAL_ESCALATED_REPLY
                                {
                                    view.deliver_principal_reply_to_worker(message, ctx);
                                }
                                #[cfg(not(target_family = "wasm"))]
                                if message.body != PRINCIPAL_ESCALATED_REPLY {
                                    view.forward_to_cli_worker(message, routing.as_ref(), ctx);
                                }
                                #[cfg(target_family = "wasm")]
                                let _ = routing;
                            }
                            view.workspace_execution_runtime
                                .observe_lease_renewed(&event);
                            if let WorkspaceEventPayload::AgentExecutionFinished { attempt, .. } =
                                &event.payload
                            {
                                if matches!(
                                    attempt.status,
                                    dragent_workspace_model::ExecutionAttemptStatus::Cancelled
                                        | dragent_workspace_model::ExecutionAttemptStatus::Paused
                                        | dragent_workspace_model::ExecutionAttemptStatus::TimedOut
                                ) {
                                    #[cfg(not(target_family = "wasm"))]
                                    view.release_cli_worker(attempt.id, ctx);
                                    if let Some(agent) = view.attempt_agents.get(&attempt.id).cloned() {
                                        let controller =
                                            agent.read(ctx, |view, _| view.controller().clone());
                                        ctx.update_model(&controller, |controller, mctx| {
                                            controller.cancel_stream(mctx)
                                        });
                                    }
                                }
                            }
                            view.workspace_execution_runtime.observe_finished(&event);
                            // Closes the human-resolution loop: without this,
                            // Phase 0's "Mark resolved" button is a dead end
                            // again — nothing turns a resolved AttentionItem
                            // back into something the blocked worker's next
                            // attempt ever sees. Rejections and answers both
                            // resolve to `Resolved` (only a plain Dismiss
                            // does not), so both carry the human's decision
                            // back into the thread.
                            if let WorkspaceEventPayload::AttentionUpdated { attention_item } =
                                &event.payload
                            {
                                if attention_item.status == AttentionStatus::Resolved
                                    && attention_item.requested_by == principal_id
                                {
                                    if let Some(thread_id) = attention_item.thread_id {
                                        let reply_to = view
                                            .workspace_state
                                            .agent_messages()
                                            .into_iter()
                                            .filter(|(message, _)| {
                                                message.thread_id == thread_id
                                                    && matches!(
                                                        message.kind,
                                                        AgentMessageKind::Blocker
                                                            | AgentMessageKind::DecisionRequest
                                                    )
                                            })
                                            .max_by_key(|(message, _)| message.created_at_unix_ms)
                                            .map(|(message, _)| message.id);
                                        let recipient_ids = attention_item
                                            .task_id
                                            .zip(attention_item.execution_attempt_id)
                                            .map(|(task_id, attempt_id)| {
                                                crate::agent::workspace_execution::handoff_recipient_ids(
                                                    &view.workspace_state,
                                                    task_id,
                                                    attempt_id,
                                                )
                                            })
                                            .unwrap_or_default();
                                        let now = event.timestamp_unix_ms;
                                        let body = attention_item
                                            .resolution
                                            .clone()
                                            .filter(|text| !text.trim().is_empty())
                                            .unwrap_or_else(|| {
                                                "The user resolved this without additional detail."
                                                    .to_string()
                                            });
                                        principal_coordination_resolution_reply =
                                            Some(WorkspaceCommandEnvelope {
                                                command_id: WorkspaceCommandId(Uuid::new_v5(
                                                    &attention_item.id.0,
                                                    b"command:principal-coordination-resolution",
                                                )),
                                                schema_version: WORKSPACE_PROTOCOL_VERSION,
                                                project_id: event.project_id,
                                                work_item_id: event.work_item_id,
                                                actor_id: principal_id,
                                                device_id: None,
                                                correlation_id: event.correlation_id,
                                                timestamp_unix_ms: now,
                                                expected_revision: None,
                                                payload: WorkspaceCommandPayload::PostMessage {
                                                    message: WorkspaceAgentMessage {
                                                        id: AgentMessageId(Uuid::new_v5(
                                                            &attention_item.id.0,
                                                            b"message:principal-coordination-resolution",
                                                        )),
                                                        thread_id,
                                                        work_item_id: event.work_item_id,
                                                        sender_id: principal_id,
                                                        kind: AgentMessageKind::Approval,
                                                        body,
                                                        review_finding: None,
                                                        reply_to,
                                                        artifact_ids: Vec::new(),
                                                        evidence_ids: Vec::new(),
                                                        delivery_state: MessageDeliveryState::LocalOnly,
                                                        created_at_unix_ms: now,
                                                        updated_at_unix_ms: now,
                                                    },
                                                    routing: Some(AgentMessageRouting {
                                                        recipient_ids,
                                                        task_id: attention_item.task_id,
                                                        execution_attempt_id: attention_item
                                                            .execution_attempt_id,
                                                    }),
                                                },
                                            });
                                    }
                                }
                            }
                            if let WorkspaceEventPayload::AgentExecutionFinished {
                                agent_instance,
                                attempt,
                                summary,
                                ..
                            } = &event.payload
                            {
                                if let Some((mut kind, actor_kind)) = crate::agent::workspace_execution::execution_completion_message_kind(
                                    &view.workspace_state,
                                    agent_instance,
                                    attempt.status,
                                ) {
                                    let sender = ActorRef::agent_instance(
                                        agent_instance.id,
                                        actor_kind,
                                    );
                                    let timestamp = event.timestamp_unix_ms;
                                    let mut message_body = summary.clone();
                                    let review_finding = if kind == AgentMessageKind::ReviewFinding {
                                        match attempt
                                            .isolation
                                            .as_ref()
                                            .and_then(|isolation| isolation.source_attempt_id)
                                            .ok_or_else(|| "review execution has no assigned candidate".to_string())
                                            .and_then(|candidate_id| {
                                                crate::agent::workspace_execution::parse_structured_review_finding(
                                                    summary,
                                                    candidate_id,
                                                )
                                            }) {
                                            Ok(finding) => {
                                                message_body = finding.summary.clone();
                                                Some(finding)
                                            }
                                            Err(error) => {
                                                kind = AgentMessageKind::Blocker;
                                                message_body = error;
                                                None
                                            }
                                        }
                                    } else {
                                        None
                                    };
                                    let message_tag = match kind {
                                        AgentMessageKind::Handoff => {
                                            b"message:execution-handoff".as_slice()
                                        }
                                        AgentMessageKind::ReviewFinding => {
                                            b"message:execution-review-finding".as_slice()
                                        }
                                        _ => b"message:execution-blocker".as_slice(),
                                    };
                                    execution_communication = Some(WorkspaceCommandEnvelope {
                                        command_id: WorkspaceCommandId(Uuid::new_v5(
                                            &attempt.id.0,
                                            b"command:execution-communication",
                                        )),
                                        schema_version: WORKSPACE_PROTOCOL_VERSION,
                                        project_id: event.project_id,
                                        work_item_id: event.work_item_id,
                                        actor_id: sender.id,
                                        device_id: None,
                                        correlation_id: event.correlation_id,
                                        timestamp_unix_ms: timestamp,
                                        expected_revision: None,
                                        payload: WorkspaceCommandPayload::PostMessage {
                                            message: WorkspaceAgentMessage {
                                                id: AgentMessageId(Uuid::new_v5(
                                                    &attempt.id.0,
                                                    message_tag,
                                                )),
                                                thread_id: task_coordination_thread_id(attempt.task_id),
                                                work_item_id: event.work_item_id,
                                                sender_id: sender.id,
                                                kind,
                                                body: message_body,
                                                review_finding,
                                                reply_to: None,
                                                artifact_ids: Vec::new(),
                                                evidence_ids: Vec::new(),
                                                delivery_state: MessageDeliveryState::LocalOnly,
                                                created_at_unix_ms: timestamp,
                                                updated_at_unix_ms: timestamp,
                                            },
                                            routing: Some(AgentMessageRouting {
                                                recipient_ids: crate::agent::workspace_execution::handoff_recipient_ids(
                                                    &view.workspace_state,
                                                    attempt.task_id,
                                                    attempt.id,
                                                ),
                                                task_id: Some(attempt.task_id),
                                                execution_attempt_id: Some(attempt.id),
                                            }),
                                        },
                                    });
                                }
                            }
                            if let WorkspaceEventPayload::AgentExecutionReviewResolved {
                                agent_instance,
                                attempt,
                                resolution,
                                ..
                            } = &event.payload
                            {
                                if let Some(root) = view
                                    .workspace_execution_runtime
                                    .prepared_execution_root(attempt.id)
                                    .map(Path::to_path_buf)
                                {
                                    resolved_review_roots.push((
                                        attempt.id,
                                        attempt.task_id,
                                        *resolution,
                                        root,
                                    ));
                                }
                                let communication = match resolution {
                                    dragent_workspace_model::ExecutionReviewResolution::Accepted => Some((
                                        AgentMessageKind::Approval,
                                        "The read-only result was accepted. Continue with dependent work.",
                                        b"command:execution-review-accepted".as_slice(),
                                        b"message:execution-review-accepted".as_slice(),
                                    )),
                                    dragent_workspace_model::ExecutionReviewResolution::Integrated => Some((
                                        AgentMessageKind::Approval,
                                        "The change set was approved and integrated. Continue with dependent work.",
                                        b"command:execution-review-approval".as_slice(),
                                        b"message:execution-review-approval".as_slice(),
                                    )),
                                    dragent_workspace_model::ExecutionReviewResolution::RevisionRequested => Some((
                                        AgentMessageKind::RevisionRequest,
                                        "Revise this task using the latest canonical review findings, then return a new attributable change set.",
                                        b"command:execution-review-revision".as_slice(),
                                        b"message:execution-review-revision".as_slice(),
                                    )),
                                    dragent_workspace_model::ExecutionReviewResolution::Discarded => None,
                                };
                                if let Some((kind, body, command_tag, message_tag)) = communication {
                                    let handoff_id = AgentMessageId(Uuid::new_v5(
                                        &attempt.id.0,
                                        b"message:execution-handoff",
                                    ));
                                    let reply_to = view
                                        .workspace_state
                                        .agent_messages()
                                        .iter()
                                        .any(|(message, _)| message.id == handoff_id)
                                        .then_some(handoff_id);
                                    if kind != AgentMessageKind::Approval || reply_to.is_some() {
                                        let recipient = ActorRef::agent_instance(
                                            agent_instance.id,
                                            ActorKind::WorkerAgent,
                                        );
                                        execution_communication = Some(WorkspaceCommandEnvelope {
                                        command_id: WorkspaceCommandId(Uuid::new_v5(
                                            &attempt.id.0,
                                            command_tag,
                                        )),
                                        schema_version: WORKSPACE_PROTOCOL_VERSION,
                                        project_id: event.project_id,
                                        work_item_id: event.work_item_id,
                                        actor_id: event.actor_id,
                                        device_id: event.device_id,
                                        correlation_id: event.correlation_id,
                                        timestamp_unix_ms: event.timestamp_unix_ms,
                                        expected_revision: None,
                                        payload: WorkspaceCommandPayload::PostMessage {
                                            message: WorkspaceAgentMessage {
                                                id: AgentMessageId(Uuid::new_v5(
                                                    &attempt.id.0,
                                                    message_tag,
                                                )),
                                                thread_id: task_coordination_thread_id(attempt.task_id),
                                                work_item_id: event.work_item_id,
                                                sender_id: event.actor_id,
                                                kind,
                                                body: body.into(),
                                                review_finding: None,
                                                reply_to,
                                                artifact_ids: Vec::new(),
                                                evidence_ids: Vec::new(),
                                                delivery_state: MessageDeliveryState::LocalOnly,
                                                created_at_unix_ms: event.timestamp_unix_ms,
                                                updated_at_unix_ms: event.timestamp_unix_ms,
                                            },
                                            routing: Some(AgentMessageRouting {
                                                recipient_ids: vec![recipient.id],
                                                task_id: Some(attempt.task_id),
                                                execution_attempt_id: Some(attempt.id),
                                            }),
                                        },
                                        });
                                    }
                                }
                            }
                            if let Some(projected) = dragent_session_protocol::workspace::project_envelope_event(event, principal_id)
                            {
                                view.workspace_state
                                    .record(projected.timestamp_unix_ms, projected.kind);
                            }
                        }
                        for action in resolved_tools {
                            view.apply_resolved_workspace_tool(action, ctx);
                        }
                        for (attempt_id, task_id, resolution, root) in resolved_review_roots {
                            if resolution
                                == dragent_workspace_model::ExecutionReviewResolution::RevisionRequested
                            {
                                view.workspace_execution_runtime
                                    .retain_revision_root(task_id, attempt_id);
                                continue;
                            }
                            if view
                                .workspace_execution_runtime
                                .retire_execution_root(attempt_id)
                            {
                                let project_root = view.workspace_state.project().root.clone();
                                crate::bg::detach(move || {
                                    if let Err(error) =
                                        crate::git_status::worktree::discard_agent_worktree(
                                            &project_root,
                                            &root,
                                        )
                                    {
                                        log::warn!(
                                            "[workspace-execution] cleanup review worktree: {error}"
                                        );
                                    }
                                });
                            }
                        }
                        if let Some(command) = execution_communication {
                            view.submit_workspace_runtime_command(command, ctx);
                            ctx.notify();
                            return;
                        }
                        if let Some(command) = principal_coordination_resolution_reply {
                            view.submit_workspace_runtime_command(command, ctx);
                            ctx.notify();
                            return;
                        }
                        if view.continue_principal_review_policy(ctx) {
                            ctx.notify();
                            return;
                        }
                        if view.continue_principal_escalation(ctx) {
                            ctx.notify();
                            return;
                        }
                        if view.continue_principal_plan_actions(ctx) {
                            ctx.notify();
                            return;
                        }
                        if view.continue_principal_coordination(ctx) {
                            ctx.notify();
                            return;
                        }
                        if view.workspace_state.active_work().is_some_and(|work| {
                            matches!(
                                work.status,
                                WorkStatus::Executing | WorkStatus::ReadyToExecute
                            )
                        }) {
                            view.continue_workspace_launch(ctx);
                        }
                        let work_completed = view
                            .workspace_state
                            .active_work()
                            .is_some_and(|work| work.status == WorkStatus::Completed);
                        if view.agentic_workspace_enabled && work_completed {
                            if view.work_item_tab_active
                                && view.work_item_view == WorkItemView::Coordinate
                            {
                                view.work_item_view = WorkItemView::Review;
                                view.save_session(ctx);
                            } else if view.active_builtin_layout() == BuiltInLayout::Coordinate {
                                view.select_workspace_layout(BuiltInLayout::Review, ctx);
                            }
                        }
                    }
                    Err(error) => {
                        // Show it where the user is looking (the conversation),
                        // not only in Coordinate's notice strip.
                        view.record_principal_error(error.clone(), ctx);
                        view.shape_coordination_error = Some(error);
                    }
                }
                view.schedule_workspace_execution_poll(ctx);
                ctx.notify();
            },
            |_, _| {},
        );
        Ok(())
    }

    /// Send a message into the live coordination conversation while workers
    /// are running — the composer's other path, `submit_shape_message`,
    /// unconditionally reruns the full Shaping pass and has no idea
    /// coordination has started; `SubmitShapeMessage`'s dispatch gate routes
    /// here instead once `WorkStatus` is past Shaping. Reuses the same
    /// composer state (`shape_composer_text`) since it's the same visual
    /// input box.
    ///
    /// This posts exactly one message and does nothing else: no plan/contract
    /// touched, no attempt paused or interrupted.
    /// `next_principal_coordination_message` (`principal_coordination.rs`)
    /// already treats any unanswered `DecisionRequest` addressed to the
    /// Principal the same regardless of sender, so posting this is enough for
    /// `continue_principal_coordination` to pick it up on the next reactive
    /// tick — no new selector logic needed. The thread is
    /// `work_item_coordination_thread_id`, not any one task's thread: a
    /// status question like "which agent is working right now?" isn't about
    /// a single task, and `DecisionRequest` is the one operational message
    /// kind allowed to omit `routing.task_id` for exactly this reason (see
    /// `validate_message_routing` in `dragent_session_protocol`).
    pub(crate) fn submit_live_coordination_message(&mut self, ctx: &mut ViewContext<Self>) {
        let body = self.shape_composer_text.trim().to_string();
        if body.is_empty() {
            return;
        }
        let Some(work) = self.workspace_state.active_work().cloned() else {
            return;
        };
        let project_id = self.workspace_state.project().id;
        let actor = ActorRef::local_user(project_id, None);
        let principal_id = ActorRef::principal_agent(project_id).id;
        let now = chrono::Utc::now().timestamp_millis();
        self.shape_composer_text.clear();
        // Clear the editor too, not just its mirror — otherwise the sent
        // text stays in the box and the next Enter re-sends it.
        let editor = self.shape_composer_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_text("", editor_ctx);
        });
        let message = WorkspaceAgentMessage {
            id: AgentMessageId::new(),
            thread_id: dragent_workspace_model::work_item_coordination_thread_id(work.id),
            work_item_id: work.id,
            sender_id: actor.id,
            // Plain chat, not a decision request: the Principal decides what
            // it is. Posting is append-safe, so this works while a
            // coordination turn is running — it's answered right after.
            kind: AgentMessageKind::Discussion,
            body,
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
            work_item_id: work.id,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::PostMessage {
                message,
                routing: Some(AgentMessageRouting {
                    recipient_ids: vec![principal_id],
                    task_id: None,
                    execution_attempt_id: None,
                }),
            },
        };
        if self.workspace_principal_coordination_pending {
            self.shape_composer_notice =
                Some("Queued — the Principal answers as soon as its current turn finishes.".into());
        }
        self.submit_workspace_runtime_command(command, ctx);
    }
}

impl WorkspaceView {
    pub(crate) fn mark_coordination_pending(&mut self) {
        self.shape_coordination_pending = true;
        self.shape_coordination_pending_since = Some(chrono::Utc::now().timestamp_millis());
    }

    pub(crate) fn clear_coordination_pending(&mut self) {
        self.shape_coordination_pending = false;
        self.shape_coordination_pending_since = None;
    }

    pub(crate) fn mark_launch_pending(&mut self) {
        self.workspace_launch_pending = true;
        self.workspace_launch_pending_since = Some(chrono::Utc::now().timestamp_millis());
    }

    pub(crate) fn clear_launch_pending(&mut self) {
        self.workspace_launch_pending = false;
        self.workspace_launch_pending_since = None;
    }

    /// Same lost-worker guard for a Principal-coordination turn: a real
    /// provider call with read-only tool use legitimately takes a while.
    pub(crate) const PRINCIPAL_COORDINATION_TURN_TIMEOUT_MS: i64 = 120_000;

    pub(crate) fn mark_principal_coordination_pending(&mut self) {
        self.workspace_principal_coordination_pending = true;
        self.workspace_principal_coordination_pending_since =
            Some(chrono::Utc::now().timestamp_millis());
    }

    pub(crate) fn clear_principal_coordination_pending(&mut self) {
        self.workspace_principal_coordination_pending = false;
        self.workspace_principal_coordination_pending_since = None;
        self.workspace_principal_coordination_activity = None;
        if self
            .shape_composer_notice
            .as_deref()
            .is_some_and(|notice| notice.starts_with("Queued — "))
        {
            self.shape_composer_notice = None;
        }
    }

    /// Record a Shaping-turn error anchored to whichever message currently
    /// exists — see `shape_message_count` and
    /// `ParkedWorkspaceWork::record_principal_error`'s sibling doc comment
    /// for why this can't just be `.push(error)`.
    /// If the active work item's Principal is parked on an `ask_user`
    /// question, feed the composer text to it and clear the composer.
    /// Returns false when no question is open (the text starts a turn).
    pub(crate) fn answer_open_principal_question_from_composer(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let text = self.shape_composer_text.trim().to_string();
        if text.is_empty() {
            return false;
        }
        let Some(agent) = self
            .work_item_agents
            .get(&self.workspace_work_item_id)
            .cloned()
        else {
            return false;
        };
        let controller = agent.read(ctx, |view, _| view.controller().clone());
        let Some(action_id) = controller.read(ctx, |c, _| c.first_pending_question_id()) else {
            return false;
        };
        ctx.update_model(&controller, |controller, mctx| {
            controller.answer_user_question_from_input(action_id, text, mctx);
        });
        super::shape_principal::push_turn_memory(
            &mut self.shape_turn_memory,
            format!("Answered: {}", self.shape_composer_text.trim()),
        );
        self.shape_composer_text.clear();
        let editor = self.shape_composer_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_text("", editor_ctx)
        });
        ctx.notify();
        true
    }

    /// Resume an integration that crossed the Git/SQLite crash boundary. Only
    /// the active work item is driven here; other work items reconcile when
    /// their tab becomes active. Target paths must be wholly in the prepared
    /// or wholly in the reviewed post-apply state. Mixed/foreign bytes stop in
    /// a durable blocked state and are never overwritten or re-applied.
    pub(crate) fn reconcile_workspace_integrations(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_launch_pending {
            return;
        }
        let project_id = self.workspace_state.project().id;
        let work_item_id = self.workspace_work_item_id;
        self.mark_launch_pending();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = (|| -> Result<WorkspaceIntegrationReconciliation, String> {
                let path =
                    dragent_persistence::database_path().map_err(|error| error.to_string())?;
                let mut conn = dragent_persistence::setup_connection(&path)
                    .map_err(|error| error.to_string())?;
                let Some(intent) =
                    dragent_persistence::load_pending_workspace_integration_intents(&mut conn)
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .find(|intent| {
                            intent.project_id == project_id && intent.work_item_id == work_item_id
                        })
                else {
                    return Ok(WorkspaceIntegrationReconciliation::None);
                };
                if intent.state == "blocked" {
                    return Err(intent.last_error.clone().unwrap_or_else(|| {
                        "Git integration requires manual reconciliation.".into()
                    }));
                }
                let claim_owner = Uuid::new_v4().to_string();
                let now = chrono::Utc::now().timestamp_millis();
                let Some(claimed) = dragent_persistence::claim_workspace_integration(
                    &mut conn,
                    intent.attempt_id,
                    &claim_owner,
                    now,
                    Self::LAUNCH_REQUEST_TIMEOUT_MS,
                )
                .map_err(|error| error.to_string())?
                else {
                    let retry_at =
                        dragent_persistence::load_pending_workspace_integration_intents(&mut conn)
                            .map_err(|error| error.to_string())?
                            .into_iter()
                            .find(|pending| pending.attempt_id == intent.attempt_id)
                            .and_then(|pending| pending.claim_expires_at_unix_ms)
                            .unwrap_or_else(|| now.saturating_add(Self::LAUNCH_REQUEST_TIMEOUT_MS));
                    return Ok(WorkspaceIntegrationReconciliation::BusyUntil(retry_at));
                };
                let recovery = (|| -> Result<(), String> {
                    let plan: crate::git_status::worktree::AgentIntegrationPlan =
                        serde_json::from_str(&claimed.integration_plan_json).map_err(|error| {
                            format!("Could not decode integration recovery plan: {error}")
                        })?;
                    match crate::git_status::worktree::classify_agent_integration(&plan)? {
                        crate::git_status::worktree::IntegrationPlanState::Before => {
                            crate::git_status::worktree::apply_agent_integration_plan(&plan)
                        }
                        crate::git_status::worktree::IntegrationPlanState::Applied => Ok(()),
                        crate::git_status::worktree::IntegrationPlanState::Ambiguous => Err(
                            "Integration target paths changed after the durable intent; manual reconciliation is required."
                                .into(),
                        ),
                    }
                })();
                if let Err(reason) = recovery {
                    dragent_persistence::block_claimed_workspace_integration(
                        &mut conn,
                        claimed.attempt_id,
                        &claim_owner,
                        &reason,
                    )
                    .map_err(|error| {
                        format!("{reason}; could not record blocked state: {error}")
                    })?;
                    return Err(reason);
                }
                if !dragent_persistence::release_workspace_integration_claim(
                    &mut conn,
                    claimed.attempt_id,
                    &claim_owner,
                )
                .map_err(|error| error.to_string())?
                {
                    return Err("Integration ownership changed during recovery; the current owner will continue reconciliation.".into());
                }
                Ok(WorkspaceIntegrationReconciliation::Ready(claimed))
            })();
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                let origin_is_active = view.workspace_work_item_id == work_item_id;
                if origin_is_active {
                    view.clear_launch_pending();
                } else if let Some(parked) = view.parked_workspace_work.get_mut(&work_item_id) {
                    parked.launch_pending = false;
                    parked.launch_pending_since = None;
                }
                match result {
                    Ok(WorkspaceIntegrationReconciliation::Ready(intent)) => {
                        // Recovery is scoped to the work item captured above.
                        // If the user changed tabs, leave the exact command in
                        // its durable prepared state; activation will resume it.
                        if origin_is_active {
                            view.submit_workspace_runtime_command(intent.command, ctx);
                            view.refresh_document_tree(ctx);
                        }
                    }
                    Ok(WorkspaceIntegrationReconciliation::BusyUntil(retry_at)) => {
                        if origin_is_active {
                            let wait_ms = retry_at
                                .saturating_sub(chrono::Utc::now().timestamp_millis())
                                .max(1) as u64;
                            ctx.spawn(
                                Timer::after(Duration::from_millis(wait_ms)),
                                move |view, _, ctx| {
                                    if view.workspace_work_item_id == work_item_id {
                                        view.reconcile_workspace_integrations(ctx);
                                    }
                                },
                            );
                        }
                    }
                    Ok(WorkspaceIntegrationReconciliation::None) => {}
                    Err(error) => {
                        let error =
                            format!("Could not safely recover reviewed Git integration: {error}");
                        if origin_is_active {
                            view.shape_coordination_error = Some(error);
                        } else if let Some(parked) =
                            view.parked_workspace_work.get_mut(&work_item_id)
                        {
                            parked.record_principal_error(error);
                        }
                    }
                }
                ctx.notify();
            },
            |_, _| {},
        );
    }
}
