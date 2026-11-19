//! Workspace execution launch drivers.
//!
//! Responsible for: launch policy, scheduling launches, execution polls,
//! mounting attempts (fresh + recovered), finishing host attempts.
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
use super::view_state::AttemptStart;

impl WorkspaceView {
    pub(crate) fn local_workspace_launch_policy(
        &self,
        ctx: &ViewContext<Self>,
    ) -> Result<WorkspaceLaunchPolicy, String> {
        let settings = crate::settings::AppSettings::load();
        let account = AccountStore::handle(ctx);
        let billing = ctx.read_model(&account, |store, _| {
            store
                .data
                .as_ref()
                .map(|data| crate::agent::workspace_execution::LaunchBilling {
                    account_known: true,
                    locked_to_free: data.usage.model_locked_to_free
                        || data.user.tier.eq_ignore_ascii_case("free"),
                    has_premium_funds: data.usage.paid.balance_mc > 0
                        || (data.usage.allow_credit_usage && data.usage.deposit_balance_mc > 0),
                    active_model_ids: data
                        .models
                        .iter()
                        .filter(|model| model.is_active)
                        .map(|model| model.id.clone())
                        .collect(),
                })
        });
        // Route and charging follow the user's billing reality (see
        // resolve_execution_route): BYOK credentials win with no silent
        // fallback, free accounts get the platform free-tier route, paid
        // accounts get their selected catalog model, and per-role model
        // pickers still override all of this at schedule time.
        let (route, mode) = crate::agent::workspace_execution::resolve_execution_route(
            &settings.selected_model,
            matches!(settings.agent_backend, crate::settings::AgentBackend::Mock),
            billing.as_ref(),
            crate::settings::FREE_TIER_MODEL_ID,
            |model| settings.has_user_credential_for_model(model),
        )?;
        Ok(WorkspaceLaunchPolicy {
            effective_model_route: route,
            charging: ChargingAuthorization {
                owner_id: ActorRef::local_user(self.workspace_state.project().id, None).id,
                mode,
                billing_operation_id: None,
            },
            // Independent roles may run concurrently only after activation
            // has acquired non-overlapping contract write scopes. The launch
            // queue remains revision-serialized, while provider sessions may
            // overlap safely.
            maximum_parallel_tasks: 3,
            lease_duration_ms: 10 * 60 * 1_000,
        })
    }

    pub(crate) fn continue_workspace_launch(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_launch_pending {
            let now = chrono::Utc::now().timestamp_millis();
            let started = self.workspace_launch_pending_since.unwrap_or(now);
            if now.saturating_sub(started) >= Self::LAUNCH_REQUEST_TIMEOUT_MS {
                log::warn!("[workspace] launch request timed out; clearing stuck flag");
                self.clear_launch_pending();
                self.shape_coordination_error = Some(
                    "An execution request timed out without an answer. Driving the scheduler again — no work was cancelled.".into(),
                );
            } else {
                return;
            }
        }
        let now = chrono::Utc::now().timestamp_millis();
        let project_id = self.workspace_state.project().id;
        let actor = ActorRef::local_user(project_id, None);
        if let Some(command) = crate::agent::workspace_execution::next_expired_execution_command(
            &self.workspace_state,
            actor.id,
            actor.device_id,
            self.workspace_canonical_revision,
            now,
        ) {
            self.submit_workspace_runtime_command(command, ctx);
            return;
        }
        let policy = match self.local_workspace_launch_policy(ctx) {
            Ok(policy) => policy,
            Err(message) => {
                // Billing is a gate, not a cancellation: park execution at a
                // safe checkpoint (nothing running is stopped) and raise a
                // durable Budget attention with a concrete next step. Stable
                // runtime identities mean restoring billing resumes without
                // duplicate charges.
                self.shape_coordination_error = Some(message.clone());
                let settings = crate::settings::AppSettings::load();
                let route = settings.selected_model.trim();
                let route = if route.is_empty() || route == crate::settings::AUTO_MODEL_ID {
                    crate::settings::FREE_TIER_MODEL_ID
                } else {
                    route
                };
                if let Some(action) = crate::agent::workspace_execution::billing_park_attention(
                    &self.workspace_state,
                    route,
                    now,
                ) {
                    let command =
                        action.into_command_payload(self.workspace_state.project().id, None);
                    let envelope = WorkspaceCommandEnvelope {
                        command_id: WorkspaceCommandId::new(),
                        schema_version: WORKSPACE_PROTOCOL_VERSION,
                        project_id: self.workspace_state.project().id,
                        work_item_id: match self.workspace_state.active_work() {
                            Some(work) => work.id,
                            None => {
                                ctx.notify();
                                return;
                            }
                        },
                        actor_id: actor.id,
                        device_id: actor.device_id,
                        correlation_id: CorrelationId::new(),
                        timestamp_unix_ms: now,
                        expected_revision: Some(self.workspace_canonical_revision),
                        payload: command,
                    };
                    self.submit_workspace_runtime_command(envelope, ctx);
                }
                ctx.notify();
                return;
            }
        };
        let action = match next_workspace_launch_action(&self.workspace_state, &policy, now) {
            Ok(Some(action)) => action,
            Ok(None) => {
                if let Some(wait_ms) = next_workspace_retry_wait_ms(&self.workspace_state, now) {
                    self.schedule_workspace_retry_wakeup(wait_ms, ctx);
                }
                return;
            }
            Err(error) => {
                self.shape_coordination_error = Some(format!("Could not schedule work: {error}"));
                ctx.notify();
                return;
            }
        };
        // Concurrency caps apply to new provider sessions only. Renewals,
        // retries, readiness marks, and failure escalation are housekeeping
        // that must run even at capacity; otherwise a Running lease can never
        // renew once 3 tasks are active and always times out.
        let launches_new_session = matches!(
            action,
            WorkspaceLaunchAction::ProvisionAgent { .. }
                | WorkspaceLaunchAction::ActivateAgent { .. }
                | WorkspaceLaunchAction::StartExecution { .. }
        );
        if launches_new_session
            && self.workspace_execution_runtime.active_count() >= policy.maximum_parallel_tasks
        {
            return;
        }
        if let WorkspaceLaunchAction::StartExecution {
            attempt_id,
            task_id,
            ..
        } = &action
        {
            let is_review_task = self
                .workspace_state
                .latest_structured_plan()
                .and_then(|plan| plan.tasks.iter().find(|task| task.id == *task_id))
                .is_some_and(|task| task.review_target_task_id.is_some());
            if is_review_task
                && self
                    .workspace_execution_runtime
                    .prepared_execution_root(*attempt_id)
                    .is_none()
            {
                if let Some(candidate) = crate::agent::workspace_execution::review_candidate_attempt(
                    &self.workspace_state,
                    *task_id,
                ) {
                    let candidate_root = self
                        .workspace_execution_runtime
                        .prepared_execution_root(candidate.id)
                        .map(Path::to_path_buf);
                    let base_revision = candidate
                        .isolation
                        .as_ref()
                        .and_then(|isolation| isolation.base_revision.clone());
                    if let (Some(root), Some(base_revision)) = (candidate_root, base_revision) {
                        self.workspace_execution_runtime
                            .record_review_candidate_root(
                                *attempt_id,
                                candidate.id,
                                root,
                                base_revision,
                            );
                    } else {
                        self.shape_coordination_error = Some(
                            "The exact candidate checkout required by this review task is unavailable."
                                .into(),
                        );
                        ctx.notify();
                        return;
                    }
                } else {
                    self.shape_coordination_error = Some(
                        "The approved review target has no unresolved candidate to inspect.".into(),
                    );
                    ctx.notify();
                    return;
                }
            }
            let writable = self
                .workspace_state
                .latest_structured_plan()
                .and_then(|plan| plan.tasks.iter().find(|task| task.id == *task_id))
                .and_then(|task| task.assigned_contract_id)
                .and_then(|contract_id| self.workspace_state.work_contract(contract_id))
                .is_some_and(|contract| !contract.write_scope.is_empty());
            if writable
                && self
                    .workspace_execution_runtime
                    .prepared_execution_root(*attempt_id)
                    .is_none()
            {
                self.workspace_execution_runtime
                    .adopt_revision_root(*task_id, *attempt_id);
            }
            if writable
                && self
                    .workspace_execution_runtime
                    .prepared_execution_root(*attempt_id)
                    .is_none()
            {
                let attempt_id = *attempt_id;
                let repository_root = self.workspace_state.project().root.clone();
                self.mark_launch_pending();
                let (tx, rx) = async_channel::bounded(1);
                crate::bg::detach(move || {
                    let result = crate::git_status::worktree::prepare_agent_worktree(
                        &repository_root,
                        project_id.0,
                        attempt_id.0,
                    )
                    .map(|worktree| (worktree.path, worktree.base_head));
                    let _ = tx.send_blocking(result);
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, ctx| {
                        view.clear_launch_pending();
                        match result {
                            Ok((root, base_revision)) => {
                                view.workspace_execution_runtime
                                    .record_prepared_execution_root(
                                        attempt_id,
                                        root,
                                        base_revision,
                                    );
                                view.continue_workspace_launch(ctx);
                            }
                            Err(error) => {
                                view.shape_coordination_error = Some(format!(
                                    "Could not prepare isolated agent checkout: {error}"
                                ));
                            }
                        }
                        ctx.notify();
                    },
                    |_, _| {},
                );
                return;
            }
        }
        let Some(work_item_id) = self.workspace_state.active_work().map(|work| work.id) else {
            return;
        };
        let prepared_isolation = match &action {
            WorkspaceLaunchAction::StartExecution { attempt_id, .. } => self
                .workspace_execution_runtime
                .prepared_execution_isolation(*attempt_id)
                .cloned(),
            _ => None,
        };
        // RaiseAttention requires attention_item.requested_by == actor_id
        // (validated in dragent_session_protocol). EscalateFailure always
        // builds its attention_item as requested_by the Principal, so the
        // envelope must be attributed to the Principal too, or persistence
        // rejects it with PayloadActorMismatch — silently, since nothing
        // retries a rejected command on its own.
        let command_actor_id = match &action {
            WorkspaceLaunchAction::EscalateFailure { .. } => {
                ActorRef::principal_agent(project_id).id
            }
            _ => actor.id,
        };
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id,
            work_item_id,
            actor_id: command_actor_id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: action.into_command_payload(project_id, prepared_isolation),
        };
        self.mark_launch_pending();
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = writer
                .accept_workspace_command(command)
                .map_err(|error| format!("Could not advance workspace execution: {error:#}"));
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            |view, result, ctx| {
                view.clear_launch_pending();
                match result {
                    Ok(events) if !events.is_empty() => {
                        // StartAgentExecution legitimately returns two events
                        // on a work item's first start: WorkStatusChanged
                        // (ReadyToExecute -> Executing) bundled with
                        // AgentExecutionStarted in the same atomic commit
                        // (dragent_persistence workspace_store.rs). Apply
                        // every returned event rather than assuming exactly
                        // one, or the whole launch silently dead-ends here on
                        // every work item's very first task.
                        let principal_id =
                            ActorRef::principal_agent(view.workspace_state.project().id).id;
                        let mut start_event = None;
                        for event in events {
                            view.workspace_canonical_revision =
                                view.workspace_canonical_revision.max(event.sequence);
                            if matches!(
                                event.payload,
                                WorkspaceEventPayload::AgentExecutionStarted { .. }
                            ) {
                                start_event = Some(event.clone());
                            }
                            if let Some(projected) =
                                dragent_session_protocol::workspace::project_envelope_event(
                                    event,
                                    principal_id,
                                )
                            {
                                view.workspace_state
                                    .record(projected.timestamp_unix_ms, projected.kind);
                            }
                        }
                        if let Some(start_event) = start_event {
                            let contract_id = match &start_event.payload {
                                WorkspaceEventPayload::AgentExecutionStarted {
                                    attempt, ..
                                } => attempt.contract_id,
                                _ => unreachable!(),
                            };
                            if let Some(contract) =
                                view.workspace_state.work_contract(contract_id).cloned()
                            {
                                let root = view.workspace_state.project().root.clone();
                                let task_id = match &start_event.payload {
                                    WorkspaceEventPayload::AgentExecutionStarted {
                                        attempt,
                                        ..
                                    } => attempt.task_id,
                                    _ => unreachable!(),
                                };
                                let communication =
                                    crate::agent::workspace_execution::task_communication_context(
                                        &view.workspace_state,
                                        task_id,
                                    );
                                // Shared knowledge ledger + skills travel with
                                // every worker so nothing is rediscovered.
                                let knowledge = view.worker_trusted_context();
                                match view
                                    .workspace_execution_runtime
                                    .mount_committed_start_for_host(
                                        &start_event,
                                        &contract,
                                        root,
                                        &knowledge,
                                        &communication,
                                    ) {
                                    Ok(Some(request)) => {
                                        // A role pinned to an agent CLI works in a
                                        // terminal; everything else (reviews
                                        // included) on Dragent's agent.
                                        #[cfg(not(target_family = "wasm"))]
                                        match super::cli_workers::worker_agent(&request) {
                                            Some(agent) => view.mount_attempt_terminal(request, agent, ctx),
                                            None => view.mount_attempt_agent(request, ctx),
                                        }
                                        #[cfg(target_family = "wasm")]
                                        view.mount_attempt_agent(request, ctx);
                                        view.schedule_workspace_execution_poll(ctx);
                                        // The start is canonical before the
                                        // next independent task advances.
                                        view.continue_workspace_launch(ctx);
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        view.shape_coordination_error = Some(format!(
                                            "Execution committed but could not be mounted: {error}"
                                        ));
                                    }
                                }
                            }
                        } else {
                            view.continue_workspace_launch(ctx);
                        }
                    }
                    Ok(_empty) => {
                        view.shape_coordination_error = Some(
                            "Authority returned no execution events; expected at least one".into(),
                        );
                    }
                    Err(error) => view.shape_coordination_error = Some(error),
                }
                ctx.notify();
            },
            |_, _| {},
        );
    }

    pub(crate) fn schedule_workspace_execution_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_execution_poll_scheduled
            || self.workspace_execution_runtime.active_count() == 0
        {
            return;
        }
        self.workspace_execution_poll_scheduled = true;
        let expected_work_item_id = self.workspace_work_item_id;
        ctx.spawn(
            Timer::after(Duration::from_millis(50)),
            move |view, _, ctx| {
                if view.workspace_work_item_id != expected_work_item_id {
                    if let Some(parked) = view.parked_workspace_work.get_mut(&expected_work_item_id)
                    {
                        parked.execution_poll_scheduled = false;
                    }
                    return;
                }
                view.workspace_execution_poll_scheduled = false;
                view.poll_workspace_execution(ctx);
            },
        );
    }

    pub(crate) fn schedule_workspace_retry_wakeup(
        &mut self,
        wait_ms: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.workspace_retry_wakeup_scheduled {
            return;
        }
        self.workspace_retry_wakeup_scheduled = true;
        let expected_work_item_id = self.workspace_work_item_id;
        ctx.spawn(
            Timer::after(Duration::from_millis(wait_ms)),
            move |view, _, ctx| {
                if view.workspace_work_item_id != expected_work_item_id {
                    if let Some(parked) = view.parked_workspace_work.get_mut(&expected_work_item_id)
                    {
                        parked.retry_wakeup_scheduled = false;
                    }
                    return;
                }
                view.workspace_retry_wakeup_scheduled = false;
                view.continue_workspace_launch(ctx);
            },
        );
    }

    pub(crate) fn poll_workspace_execution(&mut self, ctx: &mut ViewContext<Self>) {
        if self.workspace_launch_pending {
            // The poll loop is the only driver guaranteed to run while work
            // is active: recover a lost launch worker here, not just in the
            // event-driven paths, or a stuck flag halts all execution.
            let now = chrono::Utc::now().timestamp_millis();
            let started = self.workspace_launch_pending_since.unwrap_or(now);
            if now.saturating_sub(started) >= Self::LAUNCH_REQUEST_TIMEOUT_MS {
                log::warn!("[workspace] launch request timed out in poll; clearing stuck flag");
                self.clear_launch_pending();
                self.shape_coordination_error = Some(
                    "An execution request timed out without an answer. Driving the scheduler again — no work was cancelled.".into(),
                );
                ctx.notify();
                self.continue_workspace_launch(ctx);
            }
            self.schedule_workspace_execution_poll(ctx);
            return;
        }
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(command) = self
            .workspace_execution_runtime
            .next_lease_renewal_command(self.workspace_canonical_revision, now)
        {
            self.submit_workspace_runtime_command(command, ctx);
            self.schedule_workspace_execution_poll(ctx);
            return;
        }
        // Tool execution, consent and completion are driven by each attempt's
        // `AgentController` (see `mount_attempt_agent`); this loop only keeps
        // leases alive and applies standing approvals.
        self.auto_approve_standing_attention(ctx);
        self.schedule_workspace_execution_poll(ctx);
        ctx.notify();
    }

    /// Run a committed execution attempt on the terminal agent's own engine.
    /// The Work Contract is injected, not re-implemented: provider-visible
    /// tools = the contract's allowlist, every tool call passes the contract
    /// policy (`classify_tool_call`), the cwd is the attempt's isolated
    /// checkout, the run id is the attempt's stable billing/recovery identity,
    /// and the transcript persists under the attempt id like a terminal pane.
    pub(crate) fn mount_attempt_agent(
        &mut self,
        request: crate::agent::workspace_execution::WorkspacePipelineRequest,
        ctx: &mut ViewContext<Self>,
    ) {
        self.mount_attempt_agent_with(request, AttemptStart::Fresh, ctx);
    }

    /// Continue the attempts restart recovery proved safe to resume: rebuild
    /// each one's controller from its persisted transcript and resume its run
    /// from the durable checkpoint (parked approval cards come back).
    pub(crate) fn mount_recovered_attempts(&mut self, ctx: &mut ViewContext<Self>) {
        // Tasks on agent CLIs: re-adopt the agent's terminal.
        #[cfg(not(target_family = "wasm"))]
        for attempt_id in self.workspace_execution_runtime.take_recovered_cli_attempts() {
            let Some(request) = self
                .workspace_execution_runtime
                .mounted_request(attempt_id)
                .cloned()
            else {
                continue;
            };
            if let Some(agent) = super::cli_workers::worker_agent(&request) {
                self.readopt_cli_worker(request, agent, ctx);
            }
        }
        for recovered in self
            .workspace_execution_runtime
            .take_recovered_host_attempts()
        {
            let Some(request) = self
                .workspace_execution_runtime
                .mounted_request(recovered.attempt_id)
                .cloned()
            else {
                continue;
            };
            // The system prompt is not re-sent: the run's checkpoint already
            // holds the provider request it was interrupted in.
            self.mount_attempt_agent_with(
                request,
                AttemptStart::Recover {
                    run_id: recovered.runtime_task_id,
                    parked: recovered
                        .parked
                        .into_iter()
                        .map(|call| crate::agent::controller::RecoveredParkedCall {
                            call_id: call.call_id,
                            tool_name: call.tool_name,
                            args_json: call.args_json,
                            reason: call.reason,
                        })
                        .collect(),
                },
                ctx,
            );
        }
    }

    pub(crate) fn mount_attempt_agent_with(
        &mut self,
        request: crate::agent::workspace_execution::WorkspacePipelineRequest,
        start: AttemptStart,
        ctx: &mut ViewContext<Self>,
    ) {
        let autonomy = crate::settings::AppSettings::global().agent.clone();
        let request = request.with_auto_approval(
            autonomy.approval_mode() == crate::settings::ApprovalMode::Auto,
        );
        let launch = request.launch().clone();
        let attempt_id = launch.attempt.id;
        let root = request.project_root().to_path_buf();
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
        let sender = self.persistence_writer.sender();
        let policy_request = request.clone();
        let mut system_prompt = request.system_prompt().to_string();
        if let Some(candidate_id) = launch
            .attempt
            .isolation
            .as_ref()
            .and_then(|i| i.source_attempt_id)
        {
            if let Some(candidate) = self.workspace_state.execution_attempt(candidate_id) {
                if let Some(task) = self
                    .workspace_state
                    .latest_structured_plan()
                    .and_then(|plan| plan.tasks.iter().find(|task| task.id == candidate.task_id))
                {
                    crate::agent::host_verification::bind(
                        crate::agent::host_verification::VerificationScope {
                            work_item_id: launch.attempt.work_item_id,
                            attempt_id,
                            candidate_attempt_id: candidate_id,
                            criteria: task.acceptance_criteria.clone(),
                            root: root.clone(),
                        },
                    );
                    system_prompt.push_str(&format!(
                        "\n\nHost verification: run the actual checks for candidate {} using run_command with acceptance_criteria: an array of the criterion UUIDs covered by that command. Allowed criterion IDs: {}. The host records command, directory, exit code and bounded output. Wait for completion before reporting pass. A pass verdict alone cannot complete the candidate. Do not claim coverage merely because a command exits zero. If a criterion cannot be verified by command, ask_principal for explicit human confirmation/waiver rather than inventing proof. Failed or unfinished checks block acceptance; report blocked/needs_revision instead of pass.",
                        candidate_id.0, serde_json::to_string(&task.acceptance_criteria).unwrap_or_default()
                    ));
                }
            }
        }
        let task_prompt = request.task_prompt().to_string();
        let display = format!("{}\n\n{}", launch.task.title, launch.task.description);
        // A role pinned to an agent CLI is reviewed by Dragent's agent: its
        // checks run through host verification. It takes the usual route.
        let model_route = if dragent_workspace_model::route_agent(&launch.attempt.model_route).is_some() {
            self.local_workspace_launch_policy(ctx)
                .map(|policy| policy.effective_model_route)
                .unwrap_or_default()
        } else {
            launch.attempt.model_route.clone()
        };
        let tools = launch.contract.tools.clone();
        let read_only = launch.contract.write_scope.is_empty();
        let restored_blocks = self
            .workspace_boot_restored_agent_blocks
            .get(&attempt_id.0)
            .cloned()
            .filter(|blocks| !blocks.is_empty());
        ctx.update_model(&controller, |controller, mctx| {
            controller.set_persistence(attempt_id.0, sender);
            controller.set_allowed_tools(Some(tools));
            controller.set_run_id(Some(crate::agent::workspace_execution::workspace_run_id(
                attempt_id,
            )));
            controller.set_tool_policy(Some(std::sync::Arc::new(move |event| {
                policy_request.classify_tool_call(event)
            })));
            controller.set_chat_mode(
                crate::agent::controller::AgentRole::Default,
                false,
                read_only,
            );
            controller.set_host_tools(super::worker_tools::worker_host_tools());
            controller.set_session_history(Vec::new());
            match start {
                AttemptStart::Fresh => controller.start_streaming_query_with_display(
                    task_prompt,
                    display,
                    Vec::new(),
                    None,
                    model_route,
                    Box::new(move || system_prompt),
                    mctx,
                ),
                AttemptStart::Recover { run_id, parked } => {
                    if let Some(blocks) = restored_blocks {
                        let scope_root = root.clone();
                        let true_max_sort_order = (|| -> anyhow::Result<Option<i32>> {
                            let org = dragent_persistence::current_org();
                            let _ = dragent_persistence::migrate_pane_to_root(
                                &scope_root,
                                &org,
                                attempt_id.0,
                            );
                            let path = dragent_persistence::db_path_for_read(&scope_root)?;
                            let mut conn = dragent_persistence::setup_connection(&path)?;
                            dragent_persistence::max_sort_order_for_pane(&mut conn, attempt_id.0)
                        })()
                        .unwrap_or_default();
                        controller.restore_blocks_from_sql(blocks, true_max_sort_order);
                    }
                    controller.recover_local_run(run_id, parked, model_route, mctx);
                }
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
                AgentConversationEvent::TurnCompleted { assistant_text, .. } => {
                    let summary = if assistant_text.trim().is_empty() {
                        "The agent finished without a summary.".to_string()
                    } else {
                        assistant_text.clone()
                    };
                    view.finish_host_attempt(
                        attempt_id,
                        crate::agent::workspace_execution::WorkspaceExecutionCompletion::succeeded(
                            summary,
                        ),
                        ctx,
                    );
                }
                AgentConversationEvent::Error { message } => {
                    view.finish_host_attempt(
                        attempt_id,
                        crate::agent::workspace_execution::WorkspaceExecutionCompletion::failed(
                            message.clone(),
                        ),
                        ctx,
                    );
                }
                AgentConversationEvent::ConsentRequested {
                    action_id,
                    call_id,
                    tool_name,
                    args_json,
                    reason,
                } => {
                    let request = crate::agent::workspace_execution::WorkspaceToolRequest {
                        runtime_task_id: String::new(),
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        args_json: args_json.clone(),
                        reason: reason.clone(),
                    };
                    view.raise_host_attention(
                        attempt_id,
                        *action_id,
                        AttentionKind::Permission,
                        &call_id.clone(),
                        request.consent_summary(),
                        ctx,
                    );
                }
                AgentConversationEvent::QuestionOpened {
                    action_id, summary, ..
                } => {
                    // Principal first: the question becomes a DecisionRequest
                    // to the Principal, whose reply fills the card. It
                    // escalates to the human itself when it must.
                    view.route_worker_question_to_principal(attempt_id, *action_id, summary, ctx);
                }
                AgentConversationEvent::ApprovalResolved {
                    action_id,
                    approved,
                } => {
                    view.resolve_host_attention(attempt_id, *action_id, *approved, ctx);
                }
                AgentConversationEvent::QuestionAnswered { action_id } => {
                    view.resolve_host_attention(attempt_id, *action_id, true, ctx);
                }
                AgentConversationEvent::HostToolCall {
                    task_id,
                    call_id,
                    name,
                    args_json,
                } => {
                    // `ask_principal` answers later (None); everything else now.
                    if let Some((output, is_error)) =
                        view.execute_worker_tool(attempt_id, task_id, call_id, name, args_json, ctx)
                    {
                        let task_id = task_id.clone();
                        let call_id = call_id.clone();
                        crate::bg::detach(move || {
                            if let Err(error) = crate::agent::pipeline::post_tool_result(
                                &task_id, &call_id, &output, is_error,
                            ) {
                                log::warn!("[worker-tools] post result {call_id}: {error}");
                            }
                        });
                    }
                    ctx.notify();
                }
            }
        });
        self.attempt_agents.insert(attempt_id, agent);
        ctx.notify();
    }

    /// Terminal outcome reported by an attempt's controller. Ignored once the
    /// attempt is no longer mounted (cancelled/paused by authority first).
    pub(crate) fn finish_host_attempt(
        &mut self,
        attempt_id: ExecutionAttemptId,
        completion: crate::agent::workspace_execution::WorkspaceExecutionCompletion,
        ctx: &mut ViewContext<Self>,
    ) {
        if self
            .workspace_execution_runtime
            .mounted_launch(attempt_id)
            .is_none()
        {
            return;
        }
        #[cfg(not(target_family = "wasm"))]
        if completion.outcome == dragent_workspace_model::ExecutionAttemptStatus::Succeeded {
            let candidate = self
                .workspace_state
                .execution_attempt(attempt_id)
                .and_then(|a| a.isolation.as_ref())
                .and_then(|i| i.source_attempt_id);
            if let Some(candidate) = candidate {
                if crate::agent::workspace_execution::parse_structured_review_finding(
                    &completion.summary,
                    candidate,
                )
                .is_ok_and(|f| f.verdict == dragent_workspace_model::ReviewVerdict::Pass)
                {
                    let waived = self
                        .workspace_state
                        .shaping_records_projection()
                        .map(|r| {
                            r.acceptance_criteria
                                .iter()
                                .filter(|c| {
                                    c.status == dragent_workspace_model::AcceptanceStatus::Waived
                                })
                                .map(|c| c.id)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let (tx, rx) = async_channel::bounded(1);
                    crate::bg::detach(move || {
                        let mut completion = completion;
                        if let Err(error) =
                            crate::agent::host_verification::validate_review(attempt_id.0, &waived)
                        {
                            // Keep the successful agent transcript, but route unsupported
                            // Pass through the existing blocked-review/Principal loop.
                            completion.summary = serde_json::json!({
                                "verdict": "blocked", "summary": format!("Host verification blocked acceptance: {error}"),
                                "findings": []
                            }).to_string();
                        }
                        let _ = tx.send_blocking(completion);
                    });
                    ctx.spawn_stream_local(
                        rx,
                        move |view, completion, ctx| {
                            if view
                                .workspace_execution_runtime
                                .mounted_launch(attempt_id)
                                .is_some()
                            {
                                view.finish_mounted_attempt(attempt_id, completion, ctx);
                            }
                        },
                        |_, _| {},
                    );
                    return;
                }
            }
        }
        self.finish_mounted_attempt(attempt_id, completion, ctx);
    }
}
