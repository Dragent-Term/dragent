//! Workspace tool execution and acknowledgements.
//!
//! Responsible for: dispatching tools, consent outcomes, checkpoints,
//! reviews, delivery acknowledgements and parking.
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
use super::view_state::PendingWorkspaceToolAcknowledgement;

impl WorkspaceView {
    pub(crate) fn execute_workspace_tool(
        &mut self,
        attempt_id: ExecutionAttemptId,
        request: WorkspaceToolRequest,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Err(error) = self
            .workspace_execution_runtime
            .prepare_tool_checkpoint(attempt_id, &request)
        {
            self.reject_workspace_tool(
                Some(attempt_id),
                request,
                format!("Could not create a pre-write checkpoint: {error}"),
            );
            return;
        }

        let pane_id = attempt_id.0;
        let worker_request = request.clone();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let (output, is_error) = crate::agent::local_tools::execute_with_call_id(
                &worker_request.call_id,
                &worker_request.tool_name,
                &worker_request.args_json,
                pane_id,
            );
            let _ = tx.send_blocking((output, is_error));
        });
        ctx.spawn_stream_local(
            rx,
            move |view, (output, is_error), ctx| {
                // Shell/REPL checkpoint discovery completes on the worker.
                // Attach its preimages durably before allowing the model to
                // continue from the tool result.
                view.workspace_execution_runtime
                    .finish_tool_checkpoint(attempt_id, &request);
                if let Some(delivery) = view
                    .workspace_execution_runtime
                    .tool_result_delivery(attempt_id, &request)
                {
                    crate::bg::detach(move || {
                        if let Err(error) = delivery.deliver(&output, is_error) {
                            log::warn!("[workspace-execution] post tool result: {error}");
                        }
                    });
                } else {
                    view.shape_coordination_error = Some(
                        "The execution session ended before its tool result could be delivered."
                            .into(),
                    );
                }
                view.schedule_workspace_execution_poll(ctx);
                ctx.notify();
            },
            |_, _| {},
        );
    }

    pub(crate) fn reject_workspace_tool(
        &self,
        attempt_id: Option<ExecutionAttemptId>,
        request: WorkspaceToolRequest,
        reason: String,
    ) {
        let delivery = attempt_id.and_then(|attempt_id| {
            self.workspace_execution_runtime
                .tool_result_delivery(attempt_id, &request)
        });
        crate::bg::detach(move || {
            let result = if let Some(delivery) = delivery {
                delivery.deliver(&reason, true)
            } else {
                crate::agent::pipeline::post_tool_result(
                    &request.runtime_task_id,
                    &request.call_id,
                    &reason,
                    true,
                )
            };
            if let Err(error) = result {
                log::warn!("[workspace-execution] reject tool result: {error}");
            }
        });
    }

    pub(crate) fn apply_resolved_workspace_tool(
        &mut self,
        action: WorkspaceResolvedToolAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceResolvedToolAction::Execute {
                attempt_id,
                request,
            } => self.execute_workspace_tool(attempt_id, request, ctx),
            WorkspaceResolvedToolAction::Reject {
                attempt_id,
                request,
                reason,
            } => self.reject_workspace_tool(Some(attempt_id), request, reason),
        }
    }

    /// Open the inspectable runtime agent tab. Read-only navigation over
    /// stable identities: resolves the explicit attempt when it exists in
    /// canonical state, otherwise the latest attempt for the task, otherwise
    /// the task itself with no history yet. Touches no execution state — the
    /// tab renders the durable event log and subscribes to live updates
    /// through the normal poll loop.
    pub(crate) fn open_workspace_agent_timeline(
        &mut self,
        task_id: Option<dragent_workspace_model::PlanTaskId>,
        attempt_id: Option<dragent_workspace_model::ExecutionAttemptId>,
        ctx: &mut ViewContext<Self>,
    ) {
        let resolved_attempt = if let Some(id) = attempt_id {
            let Some(attempt) = self.workspace_state.execution_attempt(id) else {
                return;
            };
            if task_id.is_some_and(|task| task != attempt.task_id) {
                return;
            }
            Some(attempt)
        } else {
            task_id.and_then(|task| {
                self.workspace_state
                    .execution_attempts()
                    .into_iter()
                    .filter(|attempt| attempt.task_id == task)
                    .max_by_key(|attempt| attempt.attempt_number)
            })
        };
        let Some(task_id) = resolved_attempt.map(|attempt| attempt.task_id).or(task_id) else {
            return;
        };
        if resolved_attempt.is_none()
            && !self
                .workspace_state
                .latest_structured_plan()
                .is_some_and(|plan| plan.tasks.iter().any(|task| task.id == task_id))
        {
            return;
        }
        let id = super::action::WorkspaceAgentTabId {
            work_item_id: self.workspace_work_item_id,
            task_id,
            attempt_id: resolved_attempt.map(|attempt| attempt.id),
        };
        if super::editor_tabs::agent_tab_index(&self.workspace_agent_tabs, id).is_none() {
            self.workspace_agent_tabs
                .push(super::editor_tabs::AgentEditorTab {
                    id,
                    expanded: false,
                    scroll: ClippedScrollStateHandle::new(),
                });
        }
        if let Some(attempt_id) = id.attempt_id {
            self.ensure_attempt_agent_history(attempt_id, ctx);
        }
        self.select_workspace_agent_tab(id, ctx);
    }

    /// An attempt that ran in an earlier session has no live controller, but
    /// its transcript persisted under the attempt id exactly like a terminal
    /// pane's. Rebuild a read-only conversation from SQLite so the Agent
    /// pane shows what that agent did, not an empty timeline.
    pub(crate) fn ensure_attempt_agent_history(
        &mut self,
        attempt_id: ExecutionAttemptId,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.attempt_agents.contains_key(&attempt_id) {
            return;
        }
        let Some(restored) = self
            .workspace_boot_restored_agent_blocks
            .get(&attempt_id.0)
            .cloned()
            .filter(|blocks| !blocks.is_empty())
        else {
            return;
        };
        let root = self.workspace_state.project().root.clone();
        let allowed_roots =
            crate::agent::permissions::AllowedRoots::for_session(&root, Some(root.as_path()));
        let api = crate::agent::default_agent_api();
        let controller = ctx.add_model(move |_| {
            crate::agent::AgentController::with_allowed_roots(api, allowed_roots, root)
        });
        let sender = self.persistence_writer.sender();
        let scope_root = self.workspace_state.project().root.clone();
        ctx.update_model(&controller, |controller, mctx| {
            controller.set_persistence(attempt_id.0, sender);
            let true_max_sort_order = (|| -> anyhow::Result<Option<i32>> {
                let org = dragent_persistence::current_org();
                let _ = dragent_persistence::migrate_pane_to_root(&scope_root, &org, attempt_id.0);
                let path = dragent_persistence::db_path_for_read(&scope_root)?;
                let mut conn = dragent_persistence::setup_connection(&path)?;
                dragent_persistence::max_sort_order_for_pane(&mut conn, attempt_id.0)
            })()
            .unwrap_or_default();
            controller.restore_blocks_from_sql(restored, true_max_sort_order);
            mctx.emit(crate::agent::AgentControllerEvent::BlocksChanged);
        });
        let theme = self.theme;
        let mono = self.mono_font;
        let agent = ctx.add_typed_action_view(|agent_ctx| {
            crate::agent::conversation_view::AgentConversationView::new(
                controller, theme, mono, agent_ctx,
            )
        });
        ctx.subscribe_to_view(&agent, |_, _, _, ctx| ctx.notify());
        self.attempt_agents.insert(attempt_id, agent);
    }

    pub(crate) fn restore_workspace_checkpoint(
        &mut self,
        attempt_id: ExecutionAttemptId,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(summary) = self
            .workspace_execution_runtime
            .restore_checkpoint(attempt_id)
        else {
            self.workspace_checkpoint_notice =
                Some("This restore point is no longer available.".into());
            ctx.notify();
            return;
        };
        self.workspace_checkpoint_notice = Some(summary);
        self.refresh_document_tree(ctx);

        // A clean open document can safely adopt the restored disk contents.
        // Dirty user buffers remain authoritative and are never overwritten by
        // checkpoint restoration UI refresh.
        let root = self.document_index_root.clone();
        let sidebar_model = self.project_sidebar_model.clone();
        let reload_path = sidebar_model.read(ctx, |model, _| {
            model
                .buffer
                .as_ref()
                .filter(|buffer| !buffer.is_dirty())
                .map(|buffer| buffer.document().relative_path.clone())
        });
        if let Some(relative_path) = reload_path {
            ctx.spawn(
                async move { super::project_documents::read_preview(&root, &relative_path) },
                move |view, preview, ctx| {
                    let text = preview.text.clone();
                    let reloaded = ctx.update_model(&sidebar_model, |model, model_ctx| {
                        model.reload_clean_document(&preview, model_ctx)
                    });
                    if reloaded {
                        ctx.update_view(&view.project_sidebar_view, |sidebar, sidebar_ctx| {
                            sidebar.reload_saved_document_text(&text, sidebar_ctx);
                        });
                    }
                    ctx.notify();
                },
            );
        } else {
            ctx.notify();
        }
    }

    pub(crate) fn resolve_workspace_execution_review(
        &mut self,
        attempt_id: ExecutionAttemptId,
        resolution: dragent_workspace_model::ExecutionReviewResolution,
        ctx: &mut ViewContext<Self>,
    ) {
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        self.resolve_workspace_execution_review_as(attempt_id, resolution, actor, ctx);
    }

    pub(crate) fn finish_workspace_tool_acknowledgements(
        &mut self,
        command_ids: &[WorkspaceCommandId],
        outcome: &Result<(), String>,
        ctx: &mut ViewContext<Self>,
    ) {
        for command_id in command_ids {
            let Some(pending) = self
                .pending_workspace_tool_acknowledgements
                .get_mut(command_id)
            else {
                continue;
            };
            pending.resolved = Some(match outcome {
                Ok(()) => (pending.success.clone(), false),
                Err(error) => (error.clone(), true),
            });
        }
        self.deliver_pending_workspace_tool_acknowledgements(ctx);
    }

    pub(crate) fn finish_worker_question_submissions(
        &mut self,
        command_ids: &[WorkspaceCommandId],
        outcome: &Result<(), String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let failed = take_failed_worker_question_submissions(
            command_ids,
            outcome.is_ok(),
            &mut self.pending_worker_question_submissions,
        );
        if let Err(error) = outcome {
            for message_id in failed {
                self.fail_pending_worker_question(message_id, error.clone(), ctx);
            }
        }
    }

    pub(crate) fn deliver_pending_workspace_tool_acknowledgements(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        let ready = self
            .pending_workspace_tool_acknowledgements
            .iter_mut()
            .filter_map(|(command_id, pending)| {
                let (output, is_error) = pending.resolved.clone()?;
                if pending.delivery_in_flight {
                    return None;
                }
                pending.delivery_in_flight = true;
                Some((
                    *command_id,
                    pending.runtime_task_id.clone(),
                    pending.runtime_execution_id.clone(),
                    pending.runtime_round,
                    pending.call_id.clone(),
                    output,
                    is_error,
                    pending.durable,
                ))
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            return;
        }
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(ready.len());
        crate::bg::detach(move || {
            for (
                command_id,
                runtime_task_id,
                runtime_execution_id,
                runtime_round,
                call_id,
                output,
                is_error,
                durable,
            ) in ready
            {
                let result = crate::agent::pipeline::persist_local_tool_result(
                    &runtime_task_id,
                    &runtime_execution_id,
                    runtime_round,
                    &call_id,
                    &output,
                    is_error,
                )
                .and_then(|()| {
                    crate::agent::pipeline::post_tool_result_for_execution(
                        &runtime_task_id,
                        &runtime_execution_id,
                        runtime_round,
                        &call_id,
                        &output,
                        is_error,
                    )
                })
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    if durable {
                        writer
                            .mark_workspace_tool_outcome_delivered(
                                command_id,
                                runtime_execution_id.clone(),
                                call_id.clone(),
                                chrono::Utc::now().timestamp_millis(),
                            )
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                });
                let _ = tx.send_blocking((command_id, call_id, result));
            }
        });
        ctx.spawn_stream_local(
            rx,
            |view, (command_id, call_id, result), ctx| {
                record_workspace_tool_delivery_result(
                    &mut view.pending_workspace_tool_acknowledgements,
                    command_id,
                    result.is_ok(),
                );
                match result {
                    Ok(()) => {}
                    Err(error) => {
                        // Outbox replay may have made a previously unresolved
                        // checkpoint recoverable after its controller mounted.
                        // Start that idle controller before the next delivery
                        // attempt; never replay the accepted mutation.
                        if error.contains("the matching local execution is not active") {
                            let resume = view.pending_workspace_tool_acknowledgements
                                .get(&command_id)
                                .and_then(|pending| {
                                    view.work_item_agents.keys().find(|work_id| {
                                        principal_workspace_run_id(**work_id) == pending.runtime_task_id
                                    }).map(|work_id| (*work_id, pending.runtime_execution_id.clone()))
                                });
                            if let Some((work_id, execution_id)) = resume {
                                view.resume_checkpointed_principal(work_id, Some(&execution_id), ctx);
                            }
                        }
                        let attempts = view
                            .pending_workspace_tool_acknowledgements
                            .get_mut(&command_id)
                            .map(|pending| {
                                pending.delivery_attempts =
                                    pending.delivery_attempts.saturating_add(1);
                                pending.delivery_attempts
                            })
                            .unwrap_or_default();
                        view.shape_coordination_error = Some(format!(
                            "Saved the workspace change, but its agent acknowledgement is still pending: {error}"
                        ));
                        log::warn!("[workspace-tools] deliver durable result {call_id}: {error}");
                        if attempts >= 5
                            || error.contains("expired local execution")
                            || error.contains("older provider round")
                            || error.contains("No such file")
                        {
                            view.park_workspace_tool_acknowledgement(
                                command_id,
                                format!("delivery parked after {attempts} attempt(s): {error}"),
                                ctx,
                            );
                        } else {
                            ctx.spawn(
                                Timer::after(Duration::from_secs(1)),
                                |view, _, ctx| {
                                    view.deliver_pending_workspace_tool_acknowledgements(ctx)
                                },
                            );
                        }
                    }
                }
                ctx.notify();
            },
            |_, _| {},
        );
    }

    pub(super) fn queue_workspace_tool_delivery(
        &mut self,
        runtime_task_id: String,
        call_id: String,
        output: String,
        is_error: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let identity =
            match crate::agent::pipeline::local_tool_call_identity(&runtime_task_id, &call_id) {
                Ok(identity) => identity,
                Err(error) => {
                    self.shape_coordination_error = Some(format!(
                        "Could not identify the provider turn for tool result `{call_id}`: {error}"
                    ));
                    ctx.notify();
                    return;
                }
            };
        self.pending_workspace_tool_acknowledgements.insert(
            WorkspaceCommandId::new(),
            PendingWorkspaceToolAcknowledgement {
                runtime_task_id,
                runtime_execution_id: identity.execution_id,
                runtime_round: identity.round,
                call_id,
                success: output.clone(),
                resolved: Some((output, is_error)),
                delivery_in_flight: false,
                durable: false,
                delivery_attempts: 0,
            },
        );
        self.deliver_pending_workspace_tool_acknowledgements(ctx);
    }

    pub(crate) fn park_workspace_tool_acknowledgement(
        &mut self,
        command_id: WorkspaceCommandId,
        reason: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(pending) = self
            .pending_workspace_tool_acknowledgements
            .remove(&command_id)
        else {
            return;
        };
        if !pending.durable {
            ctx.notify();
            return;
        }
        let writer = self.persistence_writer.clone_for_workspace();
        crate::bg::detach(move || {
            if let Err(error) =
                writer.park_workspace_tool_outcome(command_id, pending.runtime_execution_id, reason)
            {
                log::warn!("[workspace-tools] park acknowledgement: {error:#}");
            }
        });
        ctx.notify();
    }

    pub(super) fn reconcile_workspace_tool_outcomes(&mut self, ctx: &mut ViewContext<Self>) {
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = writer
                .load_pending_workspace_tool_outcomes()
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            |view, result, ctx| match result {
                Ok(outcomes) => {
                    for outcome in outcomes {
                        // Non-active work items are mounted lazily. They
                        // also need a controller to consume recovered results.
                        if outcome.runtime_run_id
                            == principal_workspace_run_id(outcome.work_item_id)
                            && (outcome.work_item_id == view.workspace_work_item_id
                                || view
                                    .parked_workspace_work
                                    .contains_key(&outcome.work_item_id))
                        {
                            view.ensure_work_item_agent(outcome.work_item_id, ctx);
                        }
                        let (Some(output), Some(is_error)) =
                            (outcome.output.clone(), outcome.is_error)
                        else {
                            let valid_wait = crate::agent::pipeline::local_tool_call_identity(
                                &outcome.runtime_run_id,
                                &outcome.provider_call_id,
                            )
                            .is_ok_and(|identity| {
                                identity.execution_id == outcome.runtime_execution_id
                                    && identity.round == outcome.runtime_round
                            });
                            if !valid_wait {
                                let writer = view.persistence_writer.clone_for_workspace();
                                crate::bg::detach(move || {
                                    if let Err(error) = writer.park_workspace_tool_outcome(
                                        outcome.command_id,
                                        outcome.runtime_execution_id,
                                        "waiting provider call is no longer recoverable".into(),
                                    ) {
                                        log::warn!(
                                            "[workspace-tools] park unresolved outcome: {error:#}"
                                        );
                                    }
                                });
                            }
                            continue;
                        };
                        view.pending_workspace_tool_acknowledgements
                            .entry(outcome.command_id)
                            .or_insert(PendingWorkspaceToolAcknowledgement {
                                runtime_task_id: outcome.runtime_run_id,
                                runtime_execution_id: outcome.runtime_execution_id,
                                runtime_round: outcome.runtime_round,
                                call_id: outcome.provider_call_id,
                                success: output.clone(),
                                resolved: Some((output, is_error)),
                                delivery_in_flight: false,
                                durable: true,
                                delivery_attempts: 0,
                            });
                    }
                    view.deliver_pending_workspace_tool_acknowledgements(ctx);
                }
                Err(error) => {
                    log::warn!("[workspace-tools] reconcile outcomes: {error}");
                }
            },
            |_, _| {},
        );
    }
}

impl WorkspaceView {}
