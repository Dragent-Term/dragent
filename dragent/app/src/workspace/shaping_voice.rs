//! Shaping voice, attachments, and approval methods.
//!
//! Responsible for: voice input, attachments, model pickers,
//! shape approval and role description flows.
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
use super::shape_build::*;
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
    pub(crate) fn receive_shape_context_files(
        &mut self,
        paths: Vec<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        const MAX_ATTACHMENTS: usize = 5;
        for path in paths {
            if self.shape_composer_attachments.len() >= MAX_ATTACHMENTS {
                self.show_error_notice("Input failed", format!(
                    "Maximum {MAX_ATTACHMENTS} attachments per message."
                ), ctx);
                break;
            }
            match crate::agent::attachments::load(&path) {
                Ok(attachment) => self.shape_composer_attachments.push(attachment),
                Err(message) => {
                    log::warn!("[workspace-shape-attach] {message}");
                    self.show_error_notice("Input failed", message, ctx);
                }
            }
        }
        ctx.notify();
    }

    pub(crate) fn open_shape_attachment_picker(&mut self, ctx: &mut ViewContext<Self>) {
        let (tx, rx) = async_channel::bounded::<Vec<PathBuf>>(1);
        ctx.spawn_stream_local(
            rx,
            |view, paths, ctx| view.receive_shape_context_files(paths, ctx),
            |_, _| {},
        );
        crate::agent::attachments::pick_files_async(tx);
    }

    pub(crate) fn clear_shape_voice_state(&mut self) {
        self.shape_voice_ui = crate::voice::VoiceUiState::Idle;
        self.shape_voice_gen = self.shape_voice_gen.wrapping_add(1);
        self.shape_voice_session = None;
        self.shape_voice_fold = Default::default();
        self.shape_voice_rebase_pending = false;
        self.shape_voice_pending_confirm = None;
    }

    pub(crate) fn apply_shape_voice_snapshot(
        &mut self,
        snapshot: &str,
        is_final: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let current = self
            .shape_composer_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let force = std::mem::take(&mut self.shape_voice_rebase_pending);
        let text = if is_final {
            Some(self.shape_voice_fold.apply(&current, snapshot, force))
        } else {
            self.shape_voice_fold.apply_partial(&current, snapshot, force)
        };
        let Some(text) = text else {
            self.shape_composer_notice = Some(format!("Dictating: {snapshot}"));
            return;
        };
        self.shape_composer_text = text.clone();
        ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
            editor.set_text(&text, editor_ctx);
        });
    }

    pub(crate) fn on_shape_voice_event(
        &mut self,
        event: crate::voice::VoiceEvent,
        generation: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        if generation != self.shape_voice_gen {
            return;
        }
        match event {
            crate::voice::VoiceEvent::Live => {
                if self.shape_voice_session.is_some() {
                    self.shape_voice_ui = crate::voice::VoiceUiState::Recording;
                }
            }
            crate::voice::VoiceEvent::Partial(text) => {
                self.apply_shape_voice_snapshot(&text, false, ctx);
            }
            crate::voice::VoiceEvent::Final(text) => {
                // Confirm-before-insert gate (Settings → Voice): hold the
                // finished dictation for explicit confirmation instead of
                // inserting it. The fold (rebase base) is kept so the later
                // confirm still merges cleanly with interim edits.
                match self.shape_voice_fold.final_disposition(text) {
                    crate::voice::FinalDisposition::Insert(final_text) => {
                        self.apply_shape_voice_snapshot(&final_text, true, ctx);
                        self.clear_shape_voice_state();
                    }
                    crate::voice::FinalDisposition::AwaitingConfirm(final_text) => {
                        self.shape_voice_session = None;
                        self.shape_voice_gen = self.shape_voice_gen.wrapping_add(1);
                        self.shape_voice_ui = crate::voice::VoiceUiState::Idle;
                        self.shape_composer_notice = Some(crate::voice::review_notice(&final_text));
                        self.shape_voice_pending_confirm = Some(final_text);
                    }
                }
            }
            crate::voice::VoiceEvent::Error(message) => {
                log::warn!("[workspace-shape-voice] {message}");
                self.show_error_notice("Input failed", message, ctx);
                self.clear_shape_voice_state();
            }
        }
        ctx.notify();
    }

    pub(crate) fn tick_shape_voice_spinner(
        &mut self,
        generation: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.spawn(
            Timer::after(Duration::from_millis(80)),
            move |view, _, ctx| {
                if view.shape_voice_gen != generation
                    || view.shape_voice_ui != crate::voice::VoiceUiState::Connecting
                {
                    return;
                }
                view.shape_voice_spin = (view.shape_voice_spin + 1) % crate::icons::SPINNER_FRAMES;
                view.tick_shape_voice_spinner(generation, ctx);
                ctx.notify();
            },
        );
    }

    pub(crate) fn toggle_shape_voice_input(&mut self, ctx: &mut ViewContext<Self>) {
        use crate::voice::VoiceUiState;
        // A held dictation confirms on the next toggle: insert it now.
        if self.shape_voice_session.is_none() && self.shape_voice_ui == VoiceUiState::Idle {
            if let Some(pending) = self.shape_voice_pending_confirm.take() {
                self.apply_shape_voice_snapshot(&pending, true, ctx);
                self.shape_voice_fold = Default::default();
                self.shape_voice_rebase_pending = false;
                self.shape_composer_notice = None;
                ctx.notify();
                return;
            }
        }
        if let Some(session) = self.shape_voice_session.take() {
            session.stop();
            if self.shape_voice_ui == VoiceUiState::Connecting {
                self.clear_shape_voice_state();
                ctx.notify();
                return;
            }
            let generation = self.shape_voice_gen;
            ctx.spawn(Timer::after(Duration::from_secs(6)), move |view, _, ctx| {
                if view.shape_voice_gen == generation && view.shape_voice_ui != VoiceUiState::Idle {
                    view.clear_shape_voice_state();
                    view.shape_composer_notice = Some("Voice input stopped.".into());
                    ctx.notify();
                }
            });
            return;
        }
        if self.shape_voice_ui != VoiceUiState::Idle {
            return;
        }
        self.shape_voice_gen = self.shape_voice_gen.wrapping_add(1);
        let generation = self.shape_voice_gen;
        let (tx, rx) = async_channel::unbounded();
        match crate::voice::start(tx) {
            Ok(session) => {
                self.shape_voice_session = Some(session);
                self.shape_voice_ui = VoiceUiState::Connecting;
                self.shape_voice_spin = 0;
                self.shape_composer_notice = None;
                let existing = self
                    .shape_composer_editor
                    .read(ctx, |editor, _| editor.get_text().to_string());
                self.shape_voice_fold = crate::voice::fold::VoiceFold::start_with_confirmation(
                    existing, crate::settings::AppSettings::global().voice.confirm_before_insert,
                );
                self.shape_voice_pending_confirm = None;
                self.shape_voice_rebase_pending = false;
                self.tick_shape_voice_spinner(generation, ctx);
                ctx.spawn_stream_local(
                    rx,
                    move |view, event, ctx| view.on_shape_voice_event(event, generation, ctx),
                    |_, _| {},
                );
                ctx.spawn(
                    Timer::after(Duration::from_secs(20)),
                    move |view, _, ctx| {
                        if view.shape_voice_gen == generation
                            && view.shape_voice_ui == VoiceUiState::Connecting
                        {
                            log::warn!(
                                "[workspace-shape-voice] no Live event within 20s — engine never started"
                            );
                            if let Some(session) = view.shape_voice_session.take() {
                                session.stop();
                            }
                            view.clear_shape_voice_state();
                            view.show_error_notice("Input failed", "The dictation engine never connected. Check your network and Dragent's microphone and speech-recognition permissions."
                                    .to_string(), ctx);
                            ctx.notify();
                        }
                    },
                );
                ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
                    editor.request_focus(editor_ctx);
                });
            }
            Err(message) => {
                log::warn!("[workspace-shape-voice] {message}");
                self.show_error_notice("Input failed", message, ctx);
            }
        }
        ctx.notify();
    }

    /// Repaint ticker while a Principal turn is in flight — same role as the
    /// terminal's spinner timers: keeps the shimmer text alive between
    /// stream deltas. Stops itself when the turn ends.
    pub(crate) fn tick_principal_progress(&self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(Duration::from_millis(120)), |view, _, ctx| {
            if view.shape_principal_pending {
                ctx.notify();
                view.tick_principal_progress(ctx);
            }
        });
    }

    /// Enter-to-send for the Principal composer — same as the terminal agent
    /// input: plain Enter submits, Shift+Enter inserts a newline. Uses
    /// `with_always_handle` so the key is observed even after the focused
    /// InputEditor handled it, and yields to the @-mention popup and the open
    /// model picker exactly like the terminal outer handler.
    pub(crate) fn wrap_composer_enter_to_send(
        &self,
        element: Box<dyn Element>,
    ) -> Box<dyn Element> {
        let workspace_view_id = self.workspace_view_id;
        let window_id = self.window_id;
        let composer = self.shape_composer_editor.clone();
        let composer_id = composer.id();
        let model_menu_open = self.shape_model_menu_open;
        EventHandler::new(element)
            .with_always_handle()
            .on_keydown(move |event_ctx, app, keystroke| {
                if keystroke.key != "enter" {
                    return DispatchEventResult::PropagateToParent;
                }
                let focused = app.focused_view_id(window_id);
                if !focused.is_some_and(|id| id == composer_id) {
                    return DispatchEventResult::PropagateToParent;
                }
                if model_menu_open {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::ToggleShapeModelMenu,
                    );
                    return DispatchEventResult::StopPropagation;
                }
                if keystroke.shift {
                    return DispatchEventResult::PropagateToParent;
                }
                let mention_open = composer.read(app, |editor, _| editor.mention_active());
                if mention_open {
                    return DispatchEventResult::PropagateToParent;
                }
                event_ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::SubmitShapeMessage,
                );
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    pub(crate) fn stop_principal_turn(&mut self, ctx: &mut ViewContext<Self>) {
        // A live coordination turn (composer busy during Executing/Paused,
        // not Shaping) has its own pending flag and epoch — same
        // cooperative-cancel shape as Shaping's below, just without a
        // tool-step list or parked question to unwind.
        if self.workspace_principal_coordination_pending {
            self.workspace_principal_coordination_epoch =
                self.workspace_principal_coordination_epoch.wrapping_add(1);
            self.clear_principal_coordination_pending();
            self.shape_composer_notice = Some("Stopped.".into());
            ctx.notify();
            return;
        }
        if !self.shape_principal_pending {
            return;
        }
        // Bump the epoch so any late background ShapeTurnEvents are ignored —
        // same cooperative-cancel pattern the terminal agent uses for Stop.
        self.shape_principal_request_epoch = self.shape_principal_request_epoch.wrapping_add(1);
        self.shape_principal_pending = false;
        // The controller's own cancel_stream closes the turn as "Stopped" and
        // KEEPS everything it produced (terminal parity); dropping the
        // orchestrator's reply channel ends the background turn.
        let work_item_id = self.workspace_work_item_id;
        self.principal_pass_replies.remove(&work_item_id);
        if let Some(agent) = self.work_item_agents.get(&work_item_id).cloned() {
            let controller = agent.read(ctx, |view, _| view.controller().clone());
            ctx.update_model(&controller, |controller, mctx| {
                controller.cancel_stream(mctx)
            });
        }
        self.shape_composer_notice = Some("Stopped.".into());
        ctx.notify();
    }

    pub(crate) fn select_shape_principal_model(
        &mut self,
        model_id: String,
        effort: String,
        ctx: &mut ViewContext<Self>,
    ) {
        // Same persistence path as TerminalAction::SetAgentModel so the
        // Principal composer and the terminal agent stay on one global model.
        let mut settings = crate::settings::AppSettings::load();
        settings.selected_model = model_id;
        settings.selected_effort = match effort.as_str() {
            "low" => crate::settings::AgentEffort::Low,
            "medium" => crate::settings::AgentEffort::Medium,
            "high" => crate::settings::AgentEffort::High,
            _ => crate::settings::AgentEffort::Default,
        };
        settings.save();
        self.shape_model_menu_open = false;
        ctx.notify();
    }

    pub(crate) fn approve_shape(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shape_principal_pending {
            return;
        }
        // Work Item Flow step 3→4: explicit consent unlocks Shape turns, then
        // immediately starts the shaping turn on the confirmed requirements.
        self.shape_approved = true;
        self.shape_force_proposal = true;
        // NOTE: this wording deliberately avoids the is_shape_approval_text
        // phrases, so the resubmission below can never re-trigger approval
        // routing (which lives only in the SubmitShapeMessage handler).
        self.shape_composer_text =
            "The requirements are confirmed and approved. Build the work item shape now from our agreed requirements and scope."
                .into();
        let text = self.shape_composer_text.clone();
        let editor = self.shape_composer_editor.clone();
        ctx.update_view(&editor, move |editor, editor_ctx| {
            editor.set_text(&text, editor_ctx);
        });
        self.submit_shape_message(ctx);
    }

    pub(crate) fn begin_describe_role(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shape_principal_pending {
            return;
        }
        if self.shape_composer_text.trim().is_empty() {
            self.shape_composer_text = "Add a role that ".into();
        }
        let text = self.shape_composer_text.clone();
        let editor = self.shape_composer_editor.clone();
        ctx.update_view(&editor, move |editor, editor_ctx| {
            editor.set_text(&text, editor_ctx);
            editor.request_focus(editor_ctx);
        });
        ctx.notify();
    }

    pub(crate) fn select_proposed_role(
        &mut self,
        work_contract_id: dragent_workspace_model::WorkContractId,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_role_action_pending.is_some()
            || self
                .workspace_state
                .work_contract(work_contract_id)
                .is_none()
        {
            return;
        }
        self.shape_selected_role = Some(work_contract_id);
        self.shape_role_action_error = None;
        ctx.notify();
    }

    pub(crate) fn toggle_shape_role_model_picker(
        &mut self,
        work_contract_id: dragent_workspace_model::WorkContractId,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_record_edit_pending
            || self.shape_role_action_pending.is_some()
            || self
                .workspace_state
                .work_contract(work_contract_id)
                .is_none()
        {
            return;
        }
        self.shape_model_picker_contract = (self.shape_model_picker_contract
            != Some(work_contract_id))
        .then_some(work_contract_id);
        if self.shape_model_picker_contract.is_some() {
            let account = AccountStore::handle(ctx);
            let needs_fetch =
                ctx.read_model(&account, |store, _| store.data.is_none() && !store.loading);
            if needs_fetch {
                ctx.update_model(&account, |store, model_ctx| store.fetch(model_ctx));
            }
        }
        ctx.notify();
    }

    pub(crate) fn select_shape_role_model(
        &mut self,
        work_contract_id: dragent_workspace_model::WorkContractId,
        model_route: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_record_edit_pending || self.shape_role_action_pending.is_some() {
            return;
        }
        let (available_routes, locked) = shape_model_route_catalog(ctx);
        let pins_model = model_route
            .as_deref()
            .is_some_and(|route| dragent_workspace_model::route_agent(route).is_none());
        if locked && pins_model
            || model_route
                .as_ref()
                .is_some_and(|route| !available_routes.iter().any(|model| model.route == *route))
        {
            self.shape_role_action_error = Some(
                "That model is not available for the current account and provider settings".into(),
            );
            self.shape_model_picker_contract = None;
            ctx.notify();
            return;
        }
        self.shape_model_picker_contract = None;
        self.shape_role_action_error = None;
        self.start_shape_record_edit(
            super::ShapeRecordEditTarget::RoleModelRoute(work_contract_id),
            ctx,
        );
        if self.shape_record_edit_target
            != Some(super::ShapeRecordEditTarget::RoleModelRoute(
                work_contract_id,
            ))
        {
            return;
        }
        let value = model_route.unwrap_or_else(|| "Auto".into());
        self.shape_record_edit_text = value.clone();
        let editor = self.shape_record_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_text(&value, editor_ctx);
        });
        self.save_shape_record_edit(ctx);
    }

    pub(crate) fn close_proposed_role(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shape_role_action_pending.is_some() {
            return;
        }
        if self.shape_record_edit_target.is_some_and(|target| {
            matches!(
                target,
                super::ShapeRecordEditTarget::RoleMission(_)
                    | super::ShapeRecordEditTarget::RoleCapabilities(_)
                    | super::ShapeRecordEditTarget::RoleTools(_)
                    | super::ShapeRecordEditTarget::RoleWriteScope(_)
                    | super::ShapeRecordEditTarget::RolePermissions(_)
                    | super::ShapeRecordEditTarget::RoleExpectedOutputs(_)
                    | super::ShapeRecordEditTarget::RoleInputArtifacts(_)
                    | super::ShapeRecordEditTarget::RoleSuccessCriteria(_)
                    | super::ShapeRecordEditTarget::RoleEscalationRules(_)
                    | super::ShapeRecordEditTarget::RoleExecutionLocation(_)
                    | super::ShapeRecordEditTarget::RoleModelRoute(_)
                    | super::ShapeRecordEditTarget::RoleBudget(_)
            )
        }) {
            self.shape_record_edit_target = None;
            self.shape_record_edit_text.clear();
            self.shape_record_edit_error = None;
        }
        self.shape_selected_role = None;
        self.shape_model_picker_contract = None;
        ctx.notify();
    }

    pub(crate) fn approve_proposed_role(
        &mut self,
        work_contract_id: dragent_workspace_model::WorkContractId,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.shape_role_action_pending.is_some() {
            return;
        }
        let Some(mut contract) = self
            .workspace_state
            .work_contract(work_contract_id)
            .cloned()
        else {
            return;
        };
        if !matches!(
            contract.status,
            dragent_workspace_model::WorkContractStatus::Draft
                | dragent_workspace_model::WorkContractStatus::Proposed
        ) {
            return;
        }
        let Some(work) = self.workspace_state.active_work() else {
            return;
        };
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
        let now = chrono::Utc::now().timestamp_millis();
        contract.version = contract.version.saturating_add(1);
        contract.status = dragent_workspace_model::WorkContractStatus::Approved;
        contract.updated_by = actor.id;
        contract.updated_at_unix_ms = now;
        let command = WorkspaceCommandEnvelope {
            command_id: WorkspaceCommandId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            project_id: self.workspace_state.project().id,
            work_item_id: work.id,
            actor_id: actor.id,
            device_id: actor.device_id,
            correlation_id: CorrelationId::new(),
            timestamp_unix_ms: now,
            expected_revision: Some(self.workspace_canonical_revision),
            payload: WorkspaceCommandPayload::UpsertWorkContract {
                work_contract: contract,
            },
        };
        self.shape_role_action_pending = Some(work_contract_id);
        self.shape_role_action_error = None;
        self.shape_role_action_epoch = self.shape_role_action_epoch.wrapping_add(1);
        let epoch = self.shape_role_action_epoch;
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = writer
                .accept_workspace_command(command)
                .map_err(|error| format!("Could not approve this role: {error:#}"));
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                if view.shape_role_action_epoch != epoch
                    || view.shape_role_action_pending != Some(work_contract_id)
                {
                    return;
                }
                view.shape_role_action_pending = None;
                match result {
                    Ok(events) if events.len() == 1 => {
                        let event = events.into_iter().next().unwrap();
                        view.workspace_canonical_revision =
                            view.workspace_canonical_revision.max(event.sequence);
                        let principal_id =
                            ActorRef::principal_agent(view.workspace_state.project().id).id;
                        if let Some(projected) =
                            dragent_session_protocol::workspace::project_envelope_event(
                                event,
                                principal_id,
                            )
                        {
                            view.workspace_state
                                .record(projected.timestamp_unix_ms, projected.kind);
                            view.shape_role_action_error = None;
                            view.shape_coordination_error = None;
                        } else {
                            view.shape_role_action_error =
                                Some("Authority returned an unexpected role event".into());
                        }
                    }
                    Ok(events) => {
                        view.shape_role_action_error = Some(format!(
                            "Authority returned {} role events; expected one",
                            events.len()
                        ));
                    }
                    Err(error) => view.shape_role_action_error = Some(error),
                }
                ctx.notify();
            },
            |_, _| {},
        );
        ctx.notify();
    }
}
