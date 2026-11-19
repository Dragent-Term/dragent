//! Shaping interaction methods for WorkspaceView.
//!
//! Responsible for:
//! - shaping record edits (roles, tasks, contracts) and plan revisions
//! - submitting shaping revisions and structural changes
//!
//! Not responsible for:
//! - shaping parse/format (shape_parse)
//! - shaping validation (shape_principal / shape_workspace)
//! - rendering shaping UI (view_render)
//!
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
    pub(crate) fn submit_shape_message(&mut self, ctx: &mut ViewContext<Self>) {
        // A turn the app stopped in the middle of, resumed after a restart:
        // its saved message, not the composer's (which may hold a draft).
        let resume = self.shape_resume_turn.take();
        let body = resume
            .as_ref()
            .map(|turn| turn.display.clone())
            .unwrap_or_else(|| self.shape_composer_text.trim().to_string());
        if (body.is_empty() && self.shape_composer_attachments.is_empty())
            || self.shape_principal_pending
        {
            return;
        }

        let pending_attachments = if resume.is_some() {
            Vec::new()
        } else {
            std::mem::take(&mut self.shape_composer_attachments)
        };
        let attachment_names = pending_attachments
            .iter()
            .map(|attachment| attachment.name().to_string())
            .collect::<Vec<_>>();
        let mut image_attachments = Vec::new();
        let mut text_files = Vec::new();
        for attachment in pending_attachments {
            match attachment {
                crate::agent::attachments::LoadedAttachment::Image(image) => {
                    image_attachments.push(image);
                }
                crate::agent::attachments::LoadedAttachment::Text { name, content, .. } => {
                    text_files.push((name, content));
                }
            }
        }
        let attachment_only = body.is_empty();
        let body = if attachment_only {
            format!(
                "Review the attached file{}: {}",
                if attachment_names.len() == 1 { "" } else { "s" },
                attachment_names.join(", ")
            )
        } else {
            body
        };
        let canonical_message_body = if attachment_only || attachment_names.is_empty() {
            body.clone()
        } else {
            format!("{body}\n\nAttached: {}", attachment_names.join(", "))
        };
        let principal_body = match &resume {
            Some(turn) => turn.body.clone(),
            None => crate::agent::attachments::inline_text_files(&body, &text_files),
        };
        self.shape_composer_notice = None;
        if self.shape_voice_session.is_some() {
            self.shape_voice_rebase_pending = true;
        }

        let active_work = self.workspace_state.active_work().cloned();
        let is_new_work = active_work.is_none();
        let conversation_reply = attachment_names
            .is_empty()
            .then(|| super::shape_principal::conversational_reply(&body))
            .flatten();
        let project_id = self.workspace_state.project().id;
        let work_item_id = active_work
            .as_ref()
            .map(|work| work.id)
            .unwrap_or(self.workspace_work_item_id);
        let initial_status = active_work.as_ref().map(|work| work.status);
        let mut work_title = active_work
            .as_ref()
            .map(|work| work.title.clone())
            .unwrap_or_else(|| {
                if conversation_reply.is_some() {
                    "New work".into()
                } else {
                    dragent_workspace_model::derive_work_item_title(&body)
                }
            });
        if conversation_reply.is_none()
            && active_work
                .as_ref()
                .is_none_or(|work| is_default_work_label(&work.title))
        {
            let inferred_title = dragent_workspace_model::derive_work_item_title(&body);
            self.name_work_tab_from_intent(work_item_id, &inferred_title, ctx);
            work_title = inferred_title;
        }
        let current_shaping_records = self.workspace_state.shaping_records_projection();
        let current_work_brief = self.workspace_state.latest_work_brief().cloned();
        let current_proposal =
            self.workspace_state
                .latest_shaping_proposal()
                .cloned()
                .map(|mut proposal| {
                    if let Some(brief) = &current_work_brief {
                        proposal.outcome = brief.outcome.clone();
                        proposal.plan_summary = brief.summary.clone();
                    }
                    if let Some(records) = &current_shaping_records {
                        proposal.requirements = records
                            .requirements
                            .iter()
                            .map(|record| record.summary.clone())
                            .collect();
                        proposal.open_decisions = records
                            .decisions
                            .iter()
                            .map(|record| record.question.clone())
                            .collect();
                        proposal.acceptance_criteria = records
                            .acceptance_criteria
                            .iter()
                            .map(|record| record.description.clone())
                            .collect();
                    }
                    proposal
                });
        let current_plan = self.workspace_state.latest_structured_plan().cloned();
        let principal_conversation_history = self
            .workspace_state
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                WorkspaceEventKind::UserMessagePosted { body } => {
                    Some(("user".to_string(), body.clone()))
                }
                WorkspaceEventKind::PrincipalMessagePosted { body } => {
                    Some(("principal".to_string(), body.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let current_role_drafts = self
            .workspace_state
            .latest_proposed_team()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        let definition = self
                            .workspace_state
                            .role_definition(role.role_definition_id)?;
                        let contract = self.workspace_state.work_contract(role.work_contract_id)?;
                        Some(super::shape_principal::PrincipalRoleDraft {
                            kind: definition.kind,
                            name: definition.name.clone(),
                            mission: contract.mission.clone(),
                            rationale: role.rationale.clone(),
                            capabilities: contract
                                .capabilities
                                .iter()
                                .map(|capability| capability.capability.clone())
                                .collect(),
                            tools: contract.tools.clone(),
                            write_scope: contract
                                .write_scope
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect(),
                            preferred_model_route: contract.preferred_model_route.clone(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current_team = self.workspace_state.latest_proposed_team().cloned();
        let shared_project_memory = self.shared_project_memory_context(work_item_id);
        let current_role_definitions = current_team
            .as_ref()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        self.workspace_state
                            .role_definition(role.role_definition_id)
                            .cloned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current_work_contracts = current_team
            .as_ref()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        self.workspace_state
                            .work_contract(role.work_contract_id)
                            .cloned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Revision guard rails for the prompt: names of user-approved roles
        // and already-resolved decisions, so the model treats them as
        // sacrosanct instead of rebuilding around them.
        let approved_roles: Vec<String> = current_team
            .as_ref()
            .map(|team| {
                team.roles
                    .iter()
                    .filter_map(|role| {
                        let contract = self.workspace_state.work_contract(role.work_contract_id)?;
                        (contract.status == dragent_workspace_model::WorkContractStatus::Approved)
                            .then(|| {
                                self.workspace_state
                                    .role_definition(role.role_definition_id)
                                    .map(|definition| definition.name.clone())
                            })
                            .flatten()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let resolved_decisions: Vec<String> = current_shaping_records
            .as_ref()
            .map(|records| {
                records
                    .decisions
                    .iter()
                    .filter(|decision| {
                        decision.status == dragent_workspace_model::DecisionStatus::Resolved
                    })
                    .map(|decision| {
                        format!(
                            "{} → {}",
                            decision.question,
                            decision.resolution.as_deref().unwrap_or("")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let resolved_questions: Vec<String> = current_shaping_records
            .as_ref()
            .map(|records| {
                records
                    .decisions
                    .iter()
                    .filter(|decision| {
                        decision.status == dragent_workspace_model::DecisionStatus::Resolved
                    })
                    .map(|decision| decision.question.clone())
                    .collect()
            })
            .unwrap_or_default();
        let shaping_project_root = self.workspace_state.project().root.clone();
        if resume.is_none() {
            self.shape_composer_text.clear();
        }
        self.shape_principal_pending = true;
        self.shape_composer_notice = Some("Preparing Principal workspace context…".into());
        // The conversation itself (turn history, thinking, tool steps, todos,
        // question cards) is the work item's `AgentController` — append-only
        // and SQLite-persisted exactly like a terminal pane. Make sure it
        // exists and follows the tail for the new turn.
        let agent = self.ensure_work_item_agent(work_item_id, ctx);
        ctx.update_view(&agent, |agent, _| agent.pin_to_bottom());
        // The error card is conversation history (terminal flow): a new submit
        // must not remove it. It survives until overwritten by a newer error.
        self.shape_principal_request_epoch = self.shape_principal_request_epoch.wrapping_add(1);
        let request_epoch = self.shape_principal_request_epoch;
        if resume.is_none() {
            let editor = self.shape_composer_editor.clone();
            ctx.update_view(&editor, |editor, editor_ctx| {
                editor.set_text("", editor_ctx);
            });
        }

        let settings = crate::settings::AppSettings::load();
        let primary_model = if !self.account_logged_in_cache {
            crate::settings::FREE_TIER_MODEL_ID.to_string()
        } else if settings.selected_model == crate::settings::AUTO_MODEL_ID {
            String::new()
        } else {
            settings.selected_model.clone()
        };
        let allow_fallback = settings.allow_dragent_fallback;
        let (catalog_routes, _) = shape_model_route_catalog(ctx);
        let mut principal_model_routes = catalog_routes
            .into_iter()
            .map(|model| super::shape_principal::PrincipalModelRoute {
                route: model.route,
                label: model.label,
                detail: model.detail,
            })
            .collect::<Vec<_>>();
        if principal_model_routes.is_empty() {
            let route = if primary_model.is_empty() {
                crate::settings::FREE_TIER_MODEL_ID.to_string()
            } else {
                primary_model.clone()
            };
            principal_model_routes.push(super::shape_principal::PrincipalModelRoute {
                label: route.clone(),
                route,
                detail: "Current account default · available for this workspace".into(),
            });
        } else if !primary_model.is_empty()
            && !principal_model_routes
                .iter()
                .any(|model| model.route == primary_model)
        {
            principal_model_routes.push(super::shape_principal::PrincipalModelRoute {
                label: primary_model.clone(),
                route: primary_model.clone(),
                detail: "Current selected model · available for this workspace".into(),
            });
        }
        let local_user = ActorRef::local_user(project_id, None);
        let principal = ActorRef::principal_agent(project_id);
        let thread = CommunicationThread::principal(
            work_item_id,
            local_user.id,
            principal.id,
            chrono::Utc::now().timestamp_millis(),
        );
        let correlation_id = CorrelationId::new();
        let persistence_writer = self.persistence_writer.clone_for_workspace();
        // Defensive ordering for restored/migrated sessions as well as newly
        // opened folders. Writer events are serialized, so this idempotent
        // project upsert is durably processed before the background
        // CreateWorkItem command is accepted.
        self.persistence_writer
            .save_workspace_project(self.workspace_state.project().clone());
        let canonical_body = canonical_message_body;
        let background_body = principal_body;
        // What the user block shows: their own words (or the attachment
        // stand-in), never the prompt scaffold the model receives.
        let display_body = body.clone();
        let conversation_reply = conversation_reply.map(str::to_string);
        // Read before the background move closure: WorkspaceView is !Send.
        let shape_approved = self.shape_approved;
        let force_proposal = std::mem::replace(&mut self.shape_force_proposal, false);
        // Enduring per-work turn memory (opencode-style: prior tool findings
        // + answered questions travel with every turn; never re-read/re-ask).
        let turn_memory = self.shape_turn_memory.clone();
        // The brain feed: durable memory, oldest first, recent tail only.
        // Survives restarts and tab switches — the canonical record, not the
        // live digest.
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
        // Fresh user requirements invalidate prior completeness: the
        // Principal must re-confirm before Approve Shape unlocks again.
        // Chit-chat (greetings/thanks/farewells) adds no requirements, so it
        // leaves completeness alone — otherwise a "thanks!" after the summary
        // would strand the button disabled with no path back.
        // (ApproveShape sets approval + submits immediately, so its own turn
        // is unaffected — the flag is only read from completed turns.)
        if super::shape_principal::conversational_reply(&body).is_none() {
            self.shape_requirements_complete = false;
        }
        // Saved with the session for as long as the turn runs, so a crash
        // doesn't lose it: the next start resumes it.
        let resume_accepted = resume.as_ref().and_then(|turn| turn.accepted.clone());
        self.shape_interrupted_turn = Some(dragent_persistence::SerializedShapeTurn {
            body: background_body.clone(),
            display: display_body.clone(),
            shape_approved,
            force_proposal,
            accepted: resume_accepted.clone(),
            resumed: resume.is_some(),
        });
        self.save_session(ctx);
        self.persistence_writer.flush();
        let (tx, rx) = async_channel::bounded(6);
        let turn_params = super::shaping_turn::ShapeTurnParams {
            resume_accepted,
            resume_revision: self.workspace_canonical_revision,
            allow_fallback,
            approved_roles,
            background_body,
            canonical_body,
            conversation_reply,
            correlation_id,
            current_plan,
            current_proposal,
            current_role_definitions,
            current_role_drafts,
            current_shaping_records,
            current_team,
            current_work_contracts,
            display_body,
            force_proposal,
            image_attachments,
            initial_status,
            is_new_work,
            local_user,
            persistence_writer,
            primary_model,
            principal,
            principal_conversation_history,
            principal_model_routes,
            project_id,
            resolved_decisions,
            resolved_questions,
            shape_approved,
            shaping_memory,
            shaping_project_root,
            shared_project_memory,
            thread,
            turn_memory,
            work_item_id,
            work_title,
        };
        crate::bg::detach(move || {
            super::shaping_turn::run_shape_turn_blocking(turn_params, tx);
        });
        let mut message_accepted = false;
        ctx.spawn_stream_local(
            rx,
            move |view, event, ctx| {
                // Epoch is parked per work item (like the turn itself) so a
                // submit in another tab never orphans this background turn.
                let target_epoch = if view.workspace_work_item_id == work_item_id {
                    view.shape_principal_request_epoch
                } else {
                    view.parked_workspace_work
                        .get(&work_item_id)
                        .map(|p| p.principal_request_epoch)
                        .unwrap_or(u64::MAX)
                };
                if target_epoch != request_epoch {
                    return;
                }
                // Background turn for a parked tab: keep updating the parked
                // entry (turn UI + canonical projection) instead of dropping
                // the work. Restored untouched on return — terminal-agent
                // per-pane durability for Principal turns.
                if view.workspace_work_item_id != work_item_id {
                    let mut pending_pass = None;
                    let mut pending_exchange = None;
                    if let Some(parked) = view.parked_workspace_work.get_mut(&work_item_id) {
                        match event {
                            ShapeTurnEvent::PrincipalActivity(message) => {
                                parked.composer_notice = Some(message);
                            }
                            ShapeTurnEvent::RequirementsComplete => {
                                parked.requirements_complete = true;
                            }
                            ShapeTurnEvent::PrincipalFindings { digest } => {
                                for line in digest {
                                    super::shape_principal::push_turn_memory(
                                        &mut parked.turn_memory,
                                        line,
                                    );
                                }
                            }
                            ShapeTurnEvent::RunPass {
                                request,
                                user_turn,
                                reply,
                            } => {
                                pending_pass = Some((request, user_turn, reply));
                            }
                            ShapeTurnEvent::WorkCreated(event) => {
                                parked.canonical_revision = event.sequence;
                                if let WorkspaceEventPayload::WorkCreated { title, .. } =
                                    &event.payload
                                {
                                    let display_title = title.clone();
                                    parked.state.start_work_with_id(
                                        work_item_id,
                                        title.clone(),
                                        event.timestamp_unix_ms,
                                    );
                                    view.name_work_tab_from_intent(
                                        work_item_id,
                                        &display_title,
                                        ctx,
                                    );
                                } else {
                                    parked.principal_pending = false;
                                    parked.record_principal_error(
                                        "Local workspace authority returned the wrong creation event"
                                            .into(),
                                    );
                                }
                            }
                            ShapeTurnEvent::StatusAccepted(event) => {
                                parked.canonical_revision =
                                    parked.canonical_revision.max(event.sequence);
                                if let WorkspaceEventPayload::WorkStatusChanged {
                                    current,
                                    ..
                                } = &event.payload
                                {
                                    if !matches!(
                                        parked
                                            .state
                                            .transition_active_work(
                                                *current,
                                                event.timestamp_unix_ms,
                                            ),
                                        Some(Ok(()))
                                    ) {
                                        parked.principal_pending = false;
                                        parked.record_principal_error(
                                            "The accepted work status could not be projected".into(),
                                        );
                                    }
                                } else {
                                    parked.principal_pending = false;
                                    parked.record_principal_error(
                                        "Local workspace authority returned the wrong status event"
                                            .into(),
                                    );
                                }
                            }
                            ShapeTurnEvent::MessageAccepted(event) => {
                                parked.canonical_revision =
                                    parked.canonical_revision.max(event.sequence);
                                if let WorkspaceEventPayload::MessagePosted { message, .. } =
                                    &event.payload
                                {
                                    message_accepted = true;
                                    // A resumed turn answers this message
                                    // instead of posting it again.
                                    if let Some(turn) = parked.interrupted_turn.as_mut() {
                                        turn.accepted = Some(event.clone());
                                    }
                                    parked.state.record(
                                        event.timestamp_unix_ms,
                                        WorkspaceEventKind::UserMessagePosted {
                                            body: message.body.clone(),
                                        },
                                    );
                                } else {
                                    parked.principal_pending = false;
                                    parked.record_principal_error(
                                        "Local workspace authority returned the wrong event".into(),
                                    );
                                }
                            }
                            ShapeTurnEvent::ConversationReplyAccepted { event, streamed } => {
                                parked.principal_pending = false;
                                parked.composer_notice = None;
                                parked.canonical_revision =
                                    parked.canonical_revision.max(event.sequence);
                                if let WorkspaceEventPayload::MessagePosted { message, .. } =
                                    &event.payload
                                {
                                    if !streamed {
                                        pending_exchange =
                                            Some((body.clone(), message.body.clone()));
                                    }
                                    parked.state.record(
                                        event.timestamp_unix_ms,
                                        WorkspaceEventKind::PrincipalMessagePosted {
                                            body: message.body.clone(),
                                        },
                                    );
                                } else {
                                    parked.record_principal_error(
                                        "Local workspace authority returned the wrong Principal reply event"
                                            .into(),
                                    );
                                }
                            }
                            ShapeTurnEvent::PrincipalMemoryRecorded { events } => {
                                for event in events {
                                    parked.canonical_revision = parked
                                        .canonical_revision
                                        .max(event.sequence);
                                    if let WorkspaceEventPayload::ShapingMemoryRecorded {
                                        note,
                                    } = &event.payload
                                    {
                                        parked.state.record(
                                            event.timestamp_unix_ms,
                                            WorkspaceEventKind::ShapingMemoryRecorded {
                                                note: note.clone(),
                                            },
                                        );
                                    }
                                }
                            }
                            ShapeTurnEvent::PrincipalAccepted(result) => {
                                parked.principal_pending = false;
                                parked.composer_notice = None;
                                match result {
                                    Err(error) => {
                                        parked.record_principal_error(
                                            super::shape_principal::user_facing_principal_error(
                                                &error,
                                            ),
                                        );
                                    }
                                    Ok(outcome) => {
                                        if outcome.events.len() != 3 {
                                            parked.record_principal_error(format!(
                                                "Local workspace authority returned {} events for the Principal turn; expected three",
                                                outcome.events.len()
                                            ));
                                        } else {
                                            for event in
                                                outcome.events.into_iter().chain(outcome.team_events)
                                            {
                                                parked.canonical_revision = parked
                                                    .canonical_revision
                                                    .max(event.sequence);
                                                let kind = match event.payload {
                                                    WorkspaceEventPayload::MessagePosted {
                                                        message,
                                                        ..
                                                    } => WorkspaceEventKind::PrincipalMessagePosted {
                                                        body: message.body,
                                                    },
                                                    WorkspaceEventPayload::ShapingProposalUpdated {
                                                        proposal,
                                                    } => WorkspaceEventKind::ShapingProposalUpdated {
                                                        proposal,
                                                    },
                                                    WorkspaceEventPayload::ShapingRecordsUpdated {
                                                        records,
                                                    } => WorkspaceEventKind::ShapingRecordsUpdated {
                                                        records,
                                                    },
                                                    WorkspaceEventPayload::RoleDefinitionUpdated {
                                                        role_definition,
                                                    } => WorkspaceEventKind::RoleDefinitionUpdated {
                                                        role_definition,
                                                    },
                                                    WorkspaceEventPayload::ArtifactUpdated {
                                                        artifact,
                                                    } => WorkspaceEventKind::ArtifactUpdated {
                                                        artifact,
                                                    },
                                                    WorkspaceEventPayload::WorkContractUpdated {
                                                        work_contract,
                                                    } => WorkspaceEventKind::WorkContractUpdated {
                                                        work_contract,
                                                    },
                                                    WorkspaceEventPayload::ProposedTeamUpdated {
                                                        proposed_team,
                                                    } => WorkspaceEventKind::ProposedTeamUpdated {
                                                        proposed_team,
                                                    },
                                                    WorkspaceEventPayload::PlanUpdated { plan } => {
                                                        WorkspaceEventKind::StructuredPlanUpdated {
                                                            plan,
                                                        }
                                                    }
                                                    WorkspaceEventPayload::WorkStatusChanged {
                                                        current,
                                                        ..
                                                    } => {
                                                        // Planning transition parked with the work.
                                                        let _ = parked.state.transition_active_work(
                                                            current,
                                                            event.timestamp_unix_ms,
                                                        );
                                                        continue;
                                                    }
                                                    _ => continue,
                                                };
                                                parked
                                                    .state
                                                    .record(event.timestamp_unix_ms, kind);
                                            }
                                            if let Some(event) = outcome.planning_event {
                                                parked.canonical_revision = parked
                                                    .canonical_revision
                                                    .max(event.sequence);
                                                if let WorkspaceEventPayload::WorkStatusChanged {
                                                    current,
                                                    ..
                                                } = event.payload
                                                {
                                                    let _ = parked.state.transition_active_work(
                                                        current,
                                                        event.timestamp_unix_ms,
                                                    );
                                                }
                                            }
                            if let Some(warning) = outcome.warning {
                                                parked.record_principal_error(warning);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Re-borrowed after the match above has fully released its
                    // mutable borrow of `parked_workspace_work`: host errors
                    // become Error cards in the item's own conversation, and a
                    // requested pass starts on its controller — a parked tab's
                    // turn keeps streaming exactly like a background terminal
                    // pane's would.
                    let queued_errors = view
                        .parked_workspace_work
                        .get_mut(&work_item_id)
                        .map(|parked| std::mem::take(&mut parked.principal_errors))
                        .unwrap_or_default();
                    for error in queued_errors {
                        view.record_principal_error_for(work_item_id, error, ctx);
                    }
                    if let Some((user_text, reply_text)) = pending_exchange.take() {
                        view.push_principal_exchange(work_item_id, user_text, reply_text, ctx);
                    }
                    if let Some((request, user_turn, reply)) = pending_pass.take() {
                        view.run_principal_pass(work_item_id, request, user_turn, reply, ctx);
                    }
                    ctx.notify();
                    return;
                }
                if view
                    .workspace_state
                    .active_work()
                    .is_some_and(|work| work.id != work_item_id)
                {
                    return;
                }
                match event {
                    ShapeTurnEvent::PrincipalActivity(message) => {
                        view.shape_composer_notice = Some(message);
                        ctx.notify();
                    }
                    ShapeTurnEvent::RequirementsComplete => {
                        view.shape_requirements_complete = true;
                        ctx.notify();
                    }
                    ShapeTurnEvent::PrincipalFindings { digest } => {
                        for line in digest {
                            super::shape_principal::push_turn_memory(
                                &mut view.shape_turn_memory,
                                line,
                            );
                        }
                    }
                    ShapeTurnEvent::RunPass {
                        request,
                        user_turn,
                        reply,
                    } => {
                        view.run_principal_pass(work_item_id, request, user_turn, reply, ctx);
                    }
                    ShapeTurnEvent::WorkCreated(event) => {
                        view.workspace_canonical_revision = event.sequence;
                        let WorkspaceEventPayload::WorkCreated { title, .. } = event.payload else {
                            view.shape_principal_pending = false;
                            view.record_principal_error("Local workspace authority returned the wrong creation event".into(), ctx);
                            ctx.notify();
                            return;
                        };
                        let display_title = title.clone();
                        view.workspace_state.start_work_with_id(
                            work_item_id,
                            title,
                            event.timestamp_unix_ms,
                        );
                        view.name_work_tab_from_intent(work_item_id, &display_title, ctx);
                    }
                    ShapeTurnEvent::StatusAccepted(event) => {
                        view.workspace_canonical_revision =
                            view.workspace_canonical_revision.max(event.sequence);
                        let WorkspaceEventPayload::WorkStatusChanged { current, .. } = event.payload
                        else {
                            view.shape_principal_pending = false;
                            view.record_principal_error("Local workspace authority returned the wrong status event".into(), ctx);
                            ctx.notify();
                            return;
                        };
                        if !matches!(
                            view.workspace_state.transition_active_work(
                                current,
                                event.timestamp_unix_ms,
                            ),
                            Some(Ok(()))
                        ) {
                            view.shape_principal_pending = false;
                            view.record_principal_error("The accepted work status could not be projected".into(), ctx);
                        }
                    }
                    ShapeTurnEvent::MessageAccepted(event) => {
                        view.workspace_canonical_revision =
                            view.workspace_canonical_revision.max(event.sequence);
                        // A resumed turn answers this message instead of
                        // posting it again.
                        if let Some(turn) = view.shape_interrupted_turn.as_mut() {
                            turn.accepted = Some(event.clone());
                        }
                        let WorkspaceEventPayload::MessagePosted { message, .. } = event.payload
                        else {
                            view.shape_principal_pending = false;
                            view.record_principal_error("Local workspace authority returned the wrong event".into(), ctx);
                            ctx.notify();
                            return;
                        };
                        message_accepted = true;
                        view.save_session(ctx);
                        // Keep the compact projection during migration, but
                        // only after the canonical transaction has committed.
                        view.apply_workspace_projection_event_at(
                            event.timestamp_unix_ms,
                            WorkspaceEventKind::UserMessagePosted { body: message.body },
                        );
                    }
                    ShapeTurnEvent::ConversationReplyAccepted { event, streamed } => {
                        view.shape_principal_pending = false;
                        view.shape_composer_notice = None;
                        view.workspace_canonical_revision =
                            view.workspace_canonical_revision.max(event.sequence);
                        let WorkspaceEventPayload::MessagePosted { message, .. } = event.payload
                        else {
                            view.record_principal_error("Local workspace authority returned the wrong Principal reply event"
                                    .into(), ctx);
                            ctx.notify();
                            return;
                        };
                        // A streamed turn already left its user/agent blocks;
                        // only the canned greeting reply (no model pass) needs
                        // the exchange written into the conversation.
                        if !streamed {
                            view.push_principal_exchange(
                                work_item_id,
                                body.clone(),
                                message.body.clone(),
                                ctx,
                            );
                        }
                        view.apply_workspace_projection_event_at(
                            event.timestamp_unix_ms,
                            WorkspaceEventKind::PrincipalMessagePosted { body: message.body },
                        );
                    }
                    ShapeTurnEvent::PrincipalMemoryRecorded { events } => {
                        for event in events {
                            view.workspace_canonical_revision = view
                                .workspace_canonical_revision
                                .max(event.sequence);
                            let WorkspaceEventPayload::ShapingMemoryRecorded { note } =
                                &event.payload
                            else {
                                continue;
                            };
                            view.apply_workspace_projection_event_at(
                                event.timestamp_unix_ms,
                                WorkspaceEventKind::ShapingMemoryRecorded { note: note.clone() },
                            );
                        }
                    }
                    ShapeTurnEvent::PrincipalAccepted(Ok(outcome)) => {
                        view.shape_principal_pending = false;
                        view.shape_composer_notice = None;
                        if outcome.events.len() != 3 {
                            view.record_principal_error(
                                format!(
                                    "Local workspace authority returned {} events for the Principal turn; expected three",
                                    outcome.events.len()
                                ),
                                ctx,
                            );
                        } else {
                            for event in outcome.events {
                                view.workspace_canonical_revision =
                                    view.workspace_canonical_revision.max(event.sequence);
                                let kind = match event.payload {
                                    WorkspaceEventPayload::MessagePosted { message, .. } => {
                                        WorkspaceEventKind::PrincipalMessagePosted {
                                            body: message.body,
                                        }
                                    }
                                    WorkspaceEventPayload::ShapingProposalUpdated { proposal } => {
                                        WorkspaceEventKind::ShapingProposalUpdated { proposal }
                                    }
                                    WorkspaceEventPayload::ShapingRecordsUpdated { records } => {
                                        WorkspaceEventKind::ShapingRecordsUpdated { records }
                                    }
                                    _ => {
                                        view.record_principal_error("Local workspace authority returned the wrong Principal event"
                                                .into(), ctx);
                                        continue;
                                    }
                                };
                                view.apply_workspace_projection_event_at(
                                    event.timestamp_unix_ms,
                                    kind,
                                );
                            }
                            for event in outcome.team_events {
                                view.workspace_canonical_revision =
                                    view.workspace_canonical_revision.max(event.sequence);
                                let kind = match event.payload {
                                    WorkspaceEventPayload::RoleDefinitionUpdated {
                                        role_definition,
                                    } => WorkspaceEventKind::RoleDefinitionUpdated {
                                        role_definition,
                                    },
                                    WorkspaceEventPayload::ArtifactUpdated { artifact } => {
                                        WorkspaceEventKind::ArtifactUpdated { artifact }
                                    }
                                    WorkspaceEventPayload::WorkContractUpdated {
                                        work_contract,
                                    } => WorkspaceEventKind::WorkContractUpdated { work_contract },
                                    WorkspaceEventPayload::ProposedTeamUpdated {
                                        proposed_team,
                                    } => WorkspaceEventKind::ProposedTeamUpdated { proposed_team },
                                    WorkspaceEventPayload::PlanUpdated { plan } => {
                                        WorkspaceEventKind::StructuredPlanUpdated { plan }
                                    }
                                    _ => {
                                        view.record_principal_error("Local workspace authority returned the wrong proposed-team event"
                                                .into(), ctx);
                                        continue;
                                    }
                                };
                                view.apply_workspace_projection_event_at(
                                    event.timestamp_unix_ms,
                                    kind,
                                );
                            }
                            if let Some(event) = outcome.planning_event {
                                view.workspace_canonical_revision =
                                    view.workspace_canonical_revision.max(event.sequence);
                                if let WorkspaceEventPayload::WorkStatusChanged {
                                    current,
                                    ..
                                } = event.payload
                                {
                                    if !matches!(
                                        view.workspace_state.transition_active_work(
                                            current,
                                            event.timestamp_unix_ms,
                                        ),
                                        Some(Ok(()))
                                    ) {
                                        view.record_principal_error("The accepted Planning status could not be projected"
                                                .into(), ctx);
                                    }
                                }
                            }
                            if let Some(warning) = outcome.warning {
                                view.record_principal_error(warning, ctx);
                            }
                            view.work_item_view = WorkItemView::Plan;
                            view.work_item_plan_scroll.scroll_to(warpui_core::units::Pixels::zero());
                        }
                    }
                    ShapeTurnEvent::PrincipalAccepted(Err(error)) => {
                        view.shape_principal_pending = false;
                        view.shape_composer_notice = None;
                        log::warn!("[workspace-shape] {error}");
                        view.record_principal_error(super::shape_principal::user_facing_principal_error(&error), ctx);
                        if !message_accepted && view.shape_composer_text.trim().is_empty() {
                            view.shape_composer_text = body.clone();
                            let editor = view.shape_composer_editor.clone();
                            ctx.update_view(&editor, |editor, editor_ctx| {
                                editor.set_text(&body, editor_ctx);
                            });
                        }
                    }
                }
                ctx.notify();
            },
            // The turn ended (answered, failed or stopped): nothing to
            // resume any more.
            move |view, ctx| {
                view.update_interrupted_turn(work_item_id, |turn| *turn = None);
                view.save_session(ctx);
            },
        );
        ctx.notify();
    }

    /// Change the running-turn record of `work_item_id`, whether its tab is
    /// the active one or parked.
    pub(crate) fn update_interrupted_turn(
        &mut self,
        work_item_id: WorkItemId,
        change: impl FnOnce(&mut Option<dragent_persistence::SerializedShapeTurn>),
    ) {
        if work_item_id == self.workspace_work_item_id {
            change(&mut self.shape_interrupted_turn);
        } else if let Some(parked) = self.parked_workspace_work.get_mut(&work_item_id) {
            change(&mut parked.interrupted_turn);
        }
    }

    /// A Principal turn the app stopped in the middle of (a crash, a forced
    /// quit) is resumed once, as a terminal agent's session is. If the app
    /// stopped during that resumed turn too, the message goes back in the
    /// composer for the person to send again instead of looping.
    pub(crate) fn resume_interrupted_shape_turn(&mut self, ctx: &mut ViewContext<Self>) {
        if self.shape_principal_pending {
            return;
        }
        let Some(turn) = self.shape_interrupted_turn.take() else {
            return;
        };
        // The app stopped after the Principal's answer was saved: nothing
        // to redo. Its turns run one at a time, so an answer after the
        // person's last message is this turn's.
        let answered = turn.accepted.is_some() && last_message_answered(self.workspace_state.events().iter());
        if answered {
            self.save_session(ctx);
            return;
        }
        if turn.resumed {
            if self.shape_composer_text.trim().is_empty() {
                self.shape_composer_text = turn.display.clone();
                let editor = self.shape_composer_editor.clone();
                let text = turn.display.clone();
                ctx.update_view(&editor, |editor, editor_ctx| editor.set_text(&text, editor_ctx));
            }
            self.shape_composer_notice = Some(
                "Dragent closed while the Principal was working on this, and again when it resumed. \
                 Your message is back in the box: send it to try again."
                    .into(),
            );
            self.save_session(ctx);
            ctx.notify();
            return;
        }
        self.shape_approved = turn.shape_approved;
        self.shape_force_proposal = turn.force_proposal;
        self.shape_resume_turn = Some(turn);
        self.submit_shape_message(ctx);
    }
}

impl WorkspaceView {
    /// Return an approved plan to Draft through the semantic authority
    /// after an execution-affecting edit (currently: model route changes).
    /// The user must re-approve before coordination. No-op unless the plan
    /// is Approved and work is still in Planning.
    pub(crate) fn reset_approved_plan_to_draft(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(work) = self.workspace_state.active_work().cloned() else {
            return;
        };
        if work.status != WorkStatus::Planning {
            return;
        }
        let Some(mut plan) = self.workspace_state.latest_structured_plan().cloned() else {
            return;
        };
        if plan.status != dragent_workspace_model::PlanStatus::Approved {
            return;
        }
        let now = chrono::Utc::now().timestamp_millis();
        plan.version = plan.version.saturating_add(1);
        plan.status = dragent_workspace_model::PlanStatus::Draft;
        plan.approved_by = None;
        plan.approved_at_unix_ms = None;
        plan.updated_at_unix_ms = now;
        let actor = ActorRef::local_user(self.workspace_state.project().id, None);
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
            payload: WorkspaceCommandPayload::UpsertPlan { plan },
        };
        let writer = self.persistence_writer.clone_for_workspace();
        let (tx, rx) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let result = writer
                .accept_workspace_command(command)
                .map_err(|error| format!("Could not return the plan to Draft: {error:#}"));
            let _ = tx.send_blocking(result);
        });
        ctx.spawn_stream_local(
            rx,
            move |view, result, ctx| {
                match result {
                    Ok(events) if !events.is_empty() => {
                        let principal_id =
                            ActorRef::principal_agent(view.workspace_state.project().id).id;
                        for event in events {
                            view.workspace_canonical_revision =
                                view.workspace_canonical_revision.max(event.sequence);
                            if let Some(projected) =
                                dragent_session_protocol::workspace::project_envelope_event(event, principal_id)
                            {
                                view.workspace_state
                                    .record(projected.timestamp_unix_ms, projected.kind);
                            }
                        }
                        view.shape_recovery_notice = Some(
                            "Model changed — the plan returned to Draft. Re-approve it to coordinate."
                                .into(),
                        );
                    }
                    Ok(_) => {
                        view.shape_coordination_error =
                            Some("The authority returned no plan reset events".into());
                    }
                    Err(error) => view.shape_coordination_error = Some(error),
                }
                ctx.notify();
            },
            |_, _| {},
        );
        ctx.notify();
    }

    /// Validate every planned task's model route against the account tier and
    /// the shared catalog. Auto (None) is always valid — the server resolves
    /// it at execution time.
    pub(crate) fn validate_plan_model_routes(
        &self,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        let Some(plan) = self.workspace_state.latest_structured_plan() else {
            return Err("A structured dependency plan is required before coordination.".into());
        };
        let (catalog, locked) = shape_model_route_catalog(ctx);
        let catalog_routes = catalog
            .iter()
            .map(|model| model.route.clone())
            .collect::<Vec<_>>();
        let tasks = plan
            .tasks
            .iter()
            .map(|task| {
                let route = task
                    .assigned_contract_id
                    .and_then(|contract_id| self.workspace_state.work_contract(contract_id))
                    .and_then(|contract| contract.preferred_model_route.as_deref());
                (task.title.as_str(), route)
            })
            .collect::<Vec<_>>();
        dragent_workspace_model::validate_task_model_routes_for_coordination(
            &tasks,
            &catalog_routes,
            locked,
            Some(crate::settings::FREE_TIER_MODEL_ID),
        )
    }

    /// A coordination request older than this without an authority answer
    /// is treated as lost (background worker died): the flag clears and the
    /// user gets a visible retry instead of a permanently dead button.
    pub(crate) const COORDINATION_REQUEST_TIMEOUT_MS: i64 = 45_000;
    /// Same lost-worker guard for launch requests. Longer: worktree
    /// preparation and change capture legitimately take a while.
    pub(crate) const LAUNCH_REQUEST_TIMEOUT_MS: i64 = 120_000;
}

impl WorkspaceView {}

/// Whether the Principal answered the person's latest message: an answer or
/// a shape after it. Its turns run one at a time, so that answer is the
/// latest turn's.
fn last_message_answered<'a>(
    events: impl DoubleEndedIterator<Item = &'a WorkspaceEvent>,
) -> bool {
    events
        .rev()
        .take_while(|event| !matches!(event.kind, WorkspaceEventKind::UserMessagePosted { .. }))
        .any(|event| {
            matches!(
                event.kind,
                WorkspaceEventKind::PrincipalMessagePosted { .. }
                    | WorkspaceEventKind::ShapingProposalUpdated { .. }
            )
        })
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    fn event(kind: WorkspaceEventKind) -> WorkspaceEvent {
        WorkspaceEvent {
            sequence: 1,
            timestamp_unix_ms: 1,
            project_id: dragent_workspace_model::ProjectId::new(),
            work_item_id: None,
            kind,
        }
    }

    #[test]
    fn only_an_answer_after_the_last_message_counts() {
        let user = |body: &str| event(WorkspaceEventKind::UserMessagePosted { body: body.into() });
        let principal =
            |body: &str| event(WorkspaceEventKind::PrincipalMessagePosted { body: body.into() });
        // The app stopped before the answer: resume it.
        assert!(!last_message_answered(
            [user("hi"), principal("hello"), user("build it")].iter()
        ));
        // The answer was saved before the app stopped: nothing to redo.
        assert!(last_message_answered(
            [user("build it"), principal("Here's the shape")].iter()
        ));
        assert!(!last_message_answered([].iter()));
    }
}
