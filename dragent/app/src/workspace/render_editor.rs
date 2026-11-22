//! Editor and split-tree rendering for the workspace.
//!
//! Responsible for: editor center/pane rendering, split-tree rendering,
//! tab panes, document-terminal groups, drop zones and dividers.
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
    DragResizeHandle, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
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
    split_tab_header, ProjectFolderIntent, SidebarFocus, WorkItemView, WorkspaceAction,
    DEFAULT_SESSION_TITLE,
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
use super::view::ACTIVE_PANE_ACCENT_ALPHA;

impl WorkspaceView {
    pub(crate) fn render_editor_center(
        &self,
        theme: &Theme,
        ui: FamilyId,
        app: &AppContext,
        header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
        show_work_item: bool,
    ) -> Box<dyn Element> {
        let document = if show_work_item {
            let account = crate::settings::AccountStore::handle(app);
            let (account_display_name, account_avatar_url) =
                app.read_model(&account, |store, _| {
                    store
                        .data
                        .as_ref()
                        .map(|d| (d.user.display_name.clone(), d.user.avatar_url.clone()))
                        .unwrap_or_default()
                });
            let mut messages = self
                .workspace_state
                .events()
                .iter()
                .filter_map(|event| match &event.kind {
                    WorkspaceEventKind::UserMessagePosted { body } => {
                        Some(super::shape_workspace::ConversationMessage {
                            actor: "You".into(),
                            time: chrono::DateTime::from_timestamp_millis(event.timestamp_unix_ms)
                                .map(|time| time.format("%-I:%M %p").to_string())
                                .unwrap_or_default(),
                            body: body.clone(),
                            principal: false,
                            display_name: account_display_name.clone(),
                            avatar_url: account_avatar_url.clone(),
                        })
                    }
                    WorkspaceEventKind::PrincipalMessagePosted { body } => {
                        Some(super::shape_workspace::ConversationMessage {
                            actor: "Principal Agent".into(),
                            time: chrono::DateTime::from_timestamp_millis(event.timestamp_unix_ms)
                                .map(|time| time.format("%-I:%M %p").to_string())
                                .unwrap_or_default(),
                            body: body.clone(),
                            principal: true,
                            display_name: String::new(),
                            avatar_url: String::new(),
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let title = self.work_item_display_title();
            let status = self
                .workspace_state
                .active_work()
                .map(|work| work.status)
                .unwrap_or(WorkStatus::Draft);
            let coordination_ready = self
                .workspace_state
                .coordination_readiness()
                .is_approvable()
                && status == WorkStatus::Planning
                // Coordinated work re-enters through the same button:
                // the handler navigates (and resumes launch when still
                // pending) instead of re-approving. Without this, live
                // work shows a permanently disabled button with green
                // checks.
                || status.has_started_coordination();
            // Readiness checklist for the Plan view: a disabled Start
            // Coordination must always explain what remains to do.
            let readiness = self.workspace_state.coordination_readiness();
            let open_decisions: usize = readiness
                .issues
                .iter()
                .filter_map(|issue| match issue {
                    dragent_workspace_model::CoordinationReadinessIssue::OpenDecisionsRemain(n) => {
                        Some(*n)
                    }
                    _ => None,
                })
                .sum();
            let unassigned_tasks = readiness
                .issues
                .iter()
                .filter(|issue| {
                    matches!(
                        issue,
                        dragent_workspace_model::CoordinationReadinessIssue::PlanTaskUnassigned(_)
                            | dragent_workspace_model::CoordinationReadinessIssue::PlanTaskReferencesUnknownContract { .. }
                    )
                })
                .count();
            let roles_without_tasks = readiness
                .issues
                .iter()
                .filter(|issue| {
                    matches!(
                        issue,
                        dragent_workspace_model::CoordinationReadinessIssue::ContractHasNoPlanTask(
                            _
                        )
                    )
                })
                .count();
            let team_data_broken = readiness.issues.iter().any(|issue| {
                matches!(
                    issue,
                    dragent_workspace_model::CoordinationReadinessIssue::MissingRoleDefinition(_)
                        | dragent_workspace_model::CoordinationReadinessIssue::MissingWorkContract(_)
                        | dragent_workspace_model::CoordinationReadinessIssue::ContractIdentityMismatch(_)
                )
            });
            let contracts_ready =
                readiness.role_count > 0 && readiness.approved_role_count == readiness.role_count;
            let readiness_checks = vec![
                super::work_item_surface::ReadinessCheck {
                    label: if self.workspace_state.latest_shaping_proposal().is_some() {
                        "Shaped proposal".to_string()
                    } else {
                        "Shaped proposal — shape the work item with the Principal first".to_string()
                    },
                    done: self.workspace_state.latest_shaping_proposal().is_some(),
                },
                super::work_item_surface::ReadinessCheck {
                    label: if open_decisions == 0 {
                        "No open shaping decisions".to_string()
                    } else {
                        format!(
                            "Resolve {} open shaping decision{} — press Resolve on each one below, or answer them with the Principal",
                            open_decisions,
                            if open_decisions == 1 { "" } else { "s" },
                        )
                    },
                    done: open_decisions == 0,
                },
                super::work_item_surface::ReadinessCheck {
                    label: if readiness.role_count > 0 {
                        format!(
                            "Proposed team ({} role{})",
                            readiness.role_count,
                            if readiness.role_count == 1 { "" } else { "s" },
                        )
                    } else {
                        "Proposed team — ask the Principal to propose one".to_string()
                    },
                    done: readiness.role_count > 0,
                },
                super::work_item_surface::ReadinessCheck {
                    label: if contracts_ready {
                        format!(
                            "Role contracts approved ({}/{})",
                            readiness.approved_role_count, readiness.role_count
                        )
                    } else {
                        format!(
                            "Approve role contracts ({}/{} approved) — press Approve on each role in the Team section below",
                            readiness.approved_role_count, readiness.role_count
                        )
                    },
                    done: contracts_ready,
                },
                super::work_item_surface::ReadinessCheck {
                    label: if readiness.plan_present {
                        "Structured plan".to_string()
                    } else {
                        "Structured plan — waiting for the plan".to_string()
                    },
                    done: readiness.plan_present,
                },
                super::work_item_surface::ReadinessCheck {
                    label: if unassigned_tasks == 0 {
                        "Every plan task assigned".to_string()
                    } else {
                        "Assign every plan task to a role contract".to_string()
                    },
                    done: unassigned_tasks == 0,
                },
                super::work_item_surface::ReadinessCheck {
                    label: if roles_without_tasks == 0 {
                        "Every role has a task".to_string()
                    } else {
                        "Give every role at least one plan task".to_string()
                    },
                    done: roles_without_tasks == 0,
                },
                super::work_item_surface::ReadinessCheck {
                    label: if team_data_broken {
                        "Team data incomplete — ask the Principal to re-propose the team"
                            .to_string()
                    } else {
                        "Team data consistent".to_string()
                    },
                    done: !team_data_broken,
                },
            ];
            let work_brief = self.workspace_state.latest_work_brief().cloned();
            let records = self.workspace_state.shaping_records_projection();
            let artifacts = self.workspace_state.artifacts();
            let project_memory = self.project_memory_views(self.workspace_work_item_id);
            let knowledge_ledger = self.workspace_state.shaping_memory();
            let plan = self.workspace_state.latest_structured_plan().cloned();
            let proposed_roles = self.shaping_proposed_role_views(&artifacts, records.as_ref());
            // Same catalog source as the Shape role cards — never a
            // second list. Computed here too so the Plan task cards can
            // offer the identical per-task model selector.
            let (shape_model_routes, shape_model_picker_locked) = shape_model_route_catalog(app);
            let coordinate_active = self.work_item_view == WorkItemView::Coordinate;
            let changes_active = self.work_item_view == WorkItemView::Changes;
            let review_active = self.work_item_view == WorkItemView::Review;
            let agent_tab_active = self.work_item_view == WorkItemView::Agent;
            let conversation_active = self.work_item_view == WorkItemView::Conversation;
            let attempts = (coordinate_active || changes_active || agent_tab_active)
                .then(|| {
                    self.workspace_state
                        .execution_attempts()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            // Conversation now needs these too: coordination threads (a
            // worker's Blocker, the Principal's reply, an open request)
            // render there, not on the read-only Coordinate screen.
            let agent_messages = (coordinate_active || agent_tab_active || conversation_active)
                .then(|| {
                    self.workspace_state
                        .agent_messages()
                        .into_iter()
                        .map(|(message, routing)| (message.clone(), routing.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let communication_threads = conversation_active
                .then(|| self.workspace_state.communication_threads())
                .unwrap_or_default();
            let evidence = (coordinate_active || review_active || agent_tab_active)
                .then(|| {
                    self.workspace_state
                        .evidence()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let attention =
                (coordinate_active || review_active || agent_tab_active || conversation_active)
                    .then(|| {
                        self.workspace_state
                            .attention_items()
                            .into_iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
            let checkpoints = (coordinate_active || agent_tab_active)
                .then(|| self.workspace_execution_runtime.checkpoint_summaries())
                .unwrap_or_default();
            let completion_report = review_active
                .then(|| {
                    plan.as_ref().and_then(|plan| {
                        records.as_ref().map(|records| {
                            dragent_workspace_model::build_completion_report(
                                plan,
                                &records.acceptance_criteria,
                                &evidence,
                                &attention,
                            )
                        })
                    })
                })
                .flatten();
            let window_size = app
                .windows()
                .platform_window(self.window_id)
                .map(|window| window.size());
            let left_width = self
                .left_panel_visible
                .then(|| self.left_panel_state.lock().ok().map(|state| state.size()))
                .flatten()
                .unwrap_or(0.0);
            let right_width = self
                .right_panel_visible
                .then(|| self.right_panel_state.lock().ok().map(|state| state.size()))
                .flatten()
                .unwrap_or(0.0);
            let dock_height = self
                .document_terminal_visible
                .then(|| {
                    self.document_terminal_state
                        .lock()
                        .ok()
                        .map(|state| state.size())
                })
                .flatten()
                .unwrap_or(0.0);
            // Keep Coordinate's dense map on finite dimensions. The host
            // clips this estimate to the actual editor pane, while the
            // shared work canvas exposes any remaining overflow.
            let coordinate_viewport_width = window_size
                .map(|size| size.x() - left_width - right_width - (WORKSPACE_PADDING * 2.0))
                .unwrap_or(1_142.0)
                .max(1.0);
            let coordinate_viewport_height = window_size
                .map(|size| {
                    size.y() - HEADER_BAR_HEIGHT - dock_height - (WORKSPACE_PADDING * 2.0) - 112.0
                })
                .unwrap_or(720.0)
                .max(1.0);
            let (shape_models, shape_selected_label, shape_model_locked) =
                shape_composer_model_options(app);
            let agent_tab = self.active_workspace_agent_tab();
            let selected_agent_task = agent_tab.map(|tab| tab.id.task_id);
            let selected_agent_attempt =
                agent_tab.and_then(|tab| tab.id.attempt_id).or_else(|| {
                    selected_agent_task.and_then(|task_id| {
                        attempts
                            .iter()
                            .filter(|attempt| attempt.task_id == task_id)
                            .max_by_key(|attempt| attempt.attempt_number)
                            .map(|attempt| attempt.id)
                    })
                });
            let agent_activity = selected_agent_attempt
                .map(|id| self.workspace_execution_runtime.activity(id))
                .unwrap_or_default();
            let work = super::work_item_surface::render(
                super::work_item_surface::WorkItemSurfaceProps {
                    title: &title,
                    project_scope: &self.workspace_state.project().display_name,
                    status,
                    active_view: if agent_tab.is_some() {
                        WorkItemView::Agent
                    } else {
                        self.work_item_view
                    },
                    mono: self.mono_font,
                    agent_view: self
                        .work_item_agents
                        .get(&self.workspace_work_item_id)
                        .cloned(),
                    proposal: self.workspace_state.latest_shaping_proposal(),
                    work_brief: work_brief.as_ref(),
                    records: records.as_ref(),
                    artifacts: &artifacts,
                    project_memory: &project_memory,
                    knowledge: &knowledge_ledger,
                    plan: plan.as_ref(),
                    proposed_roles: &proposed_roles,
                    composer: self.shape_composer_editor.clone(),
                    composer_has_text: !self.shape_composer_text.trim().is_empty(),
                    composer_attachments: &self.shape_composer_attachments,
                    composer_notice: if self.shape_principal_pending {
                        None
                    } else {
                        self.shape_composer_notice.as_deref()
                    },
                    voice_state: self.shape_voice_ui,
                    voice_spin: self.shape_voice_spin,
                    // Shaping-only (gates the Approve Shape row); the
                    // composer's own busy indicator uses `composer_busy`,
                    // which also covers a live coordination turn.
                    principal_pending: self.shape_principal_pending,
                    // A coordination turn does NOT lock the composer:
                    // messages queue canonically and are answered next.
                    composer_busy: self.shape_principal_pending,
                    principal_activity: self.workspace_principal_coordination_pending.then(|| {
                        format!(
                            "Principal is working on {} — you can still send; it answers next.",
                            self.workspace_principal_coordination_activity
                                .as_deref()
                                .unwrap_or("an agent request")
                        )
                    }),
                    shape_approved: self.shape_approved,
                    requirements_complete: self.shape_requirements_complete,
                    has_proposal: self.workspace_state.latest_shaping_proposal().is_some(),
                    shape_model_menu_open: self.shape_model_menu_open,
                    shape_models,
                    shape_selected_label,
                    shape_model_locked,
                    coordination_ready,
                    coordination_pending: self.shape_coordination_pending
                        || self.workspace_launch_pending,
                    coordination_error: None,
                    recovery_notice: self.shape_recovery_notice.as_deref(),
                    record_edit_target: self.shape_record_edit_target.as_ref(),
                    remove_armed: self.shape_remove_armed,
                    record_editor: self.shape_record_editor.clone(),
                    record_edit_has_text: !self.shape_record_edit_text.trim().is_empty(),
                    record_edit_pending: self.shape_record_edit_pending,
                    record_edit_error: None,
                    role_action_pending: self.shape_role_action_pending,
                    role_action_error: None,
                    task_model_routes: &shape_model_routes,
                    task_model_picker_contract: self.shape_model_picker_contract,
                    task_model_picker_locked: shape_model_picker_locked,
                    view_tabs_scroll: self.work_item_view_tabs_scroll.clone(),
                    vertical_scroll: if self.work_item_view == WorkItemView::Conversation {
                        self.work_item_conversation_scroll.clone()
                    } else {
                        self.work_item_plan_scroll.clone()
                    },
                    coordinate_viewport_width,
                    coordinate_viewport_height,
                    coordinate_horizontal_scroll: self
                        .work_item_coordinate_horizontal_scroll
                        .clone(),
                    coordinate_vertical_scroll: self.work_item_coordinate_vertical_scroll.clone(),
                    activity_scroll: self.coordinate_activity_scroll.clone(),
                    activity_dock_state: self.coordinate_activity_dock_state.clone(),
                    active_execution_count: self.workspace_execution_runtime.active_count(),
                    attempts: &attempts,
                    agent_messages: &agent_messages,
                    communication_threads: &communication_threads,
                    evidence: &evidence,
                    attention: &attention,
                    checkpoints: &checkpoints,
                    checkpoint_notice: self.workspace_checkpoint_notice.as_deref(),
                    execution_notice: None,
                    changes_scroll: self.work_item_changes_scroll.clone(),
                    review_horizontal_scroll: self.work_item_review_horizontal_scroll.clone(),
                    review_vertical_scroll: self.work_item_review_vertical_scroll.clone(),
                    completion_report: completion_report.as_ref(),
                    readiness_checks: &readiness_checks,
                    selected_agent_task,
                    selected_agent_attempt,
                    agent_activity: &agent_activity,
                    attempt_agent_view: selected_agent_attempt
                        .and_then(|id| self.attempt_agents.get(&id).cloned()),
                    #[cfg(not(target_family = "wasm"))]
                    cli_worker: selected_agent_attempt.and_then(|id| self.cli_worker_summary(id)),
                    #[cfg(target_family = "wasm")]
                    cli_worker: None,
                    agent_timeline_expanded: agent_tab.is_some_and(|tab| tab.expanded),
                    agent_timeline_scroll: agent_tab
                        .map(|tab| tab.scroll.clone())
                        .unwrap_or_else(|| self.work_item_agent_timeline_scroll.clone()),
                },
                &theme,
                ui,
                self.workspace_view_id,
                header_mouse_state,
            );
            // Enter-to-send on the Principal composer, like terminal input.
            let work = if agent_tab.is_some() {
                work
            } else {
                self.wrap_composer_enter_to_send(work)
            };
            self.project_sidebar_view.read(app, |view, _| {
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(Expanded::new(1.0, work).finish())
                    .finish()
            })
        } else {
            self.project_sidebar_view.read(app, |view, _| {
                view.render_document_workspace(app, &theme, ui, self.mono_font)
            })
        };
        // ProjectSidebarView contains a nested editor ChildView with
        // unbounded positioned gutter/caret layers. A flex constraint sizes
        // it but does not clip those layers, so they previously painted over
        // the docked terminal. Clip the complete document surface at the
        // editor/terminal split boundary; this is the actual occlusion gate.
        let document = Clipped::new(document).finish();
        if self.document_terminal_visible {
            let terminal_group = self.render_document_terminal_group(app, &theme);
            let terminal_group = Container::new(terminal_group)
                .with_border(Border::top(2.0).with_border_color(theme.accent))
                .finish();
            if self.document_terminal_maximized {
                terminal_group
            } else {
                let terminal_panel =
                    Resizable::new(self.document_terminal_state.clone(), terminal_group)
                        .with_dragbar_side(DragBarSide::Top)
                        .with_initial_fraction(0.30)
                        .with_dragbar_color(theme.accent.into())
                        .on_resize(|ctx, _| ctx.notify())
                        .with_bounds_callback(Box::new(|window_size| {
                            let min_height = 140.0;
                            let max_height = (window_size.y() * 0.6).max(min_height);
                            (min_height, max_height)
                        }))
                        .finish();
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(Expanded::new(1.0, document).finish())
                    .with_child(terminal_panel)
                    .finish()
            }
        } else {
            document
        }
    }

    /// Editor layout pane: the shared document surface in a split tile.
    /// The header is the tab bar: scrollable file tabs with the docked-terminal
    /// button pinned right. The terminal belongs to the editor surface rather
    /// than creating another top-level workspace pane.
    pub(crate) fn render_editor_pane(
        &self,
        theme: &Theme,
        ui: FamilyId,
        app: &AppContext,
        header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    ) -> Box<dyn Element> {
        let workspace_view_id = self.workspace_view_id;
        let is_active = self.editor_pane_focused;
        let show_active_frame = is_active
            && self
                .split_tree
                .as_ref()
                .is_some_and(|layout| layout.pane_count() > 1);
        let tabs_bar = self.project_sidebar_view.read(app, |view, _| {
            view.render_workspace_tabs(
                self.editor_header_tabs(app),
                self.document_tabs_scroll.clone(),
                theme,
                self.mono_font,
            )
        });

        let button = |mouse: MouseStateHandle, icon: AppIcon, action: WorkspaceAction| {
            Hoverable::new(mouse, move |state| {
                Container::new(render_icon(
                    icon,
                    if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    },
                    ICON_SIZE_MD,
                ))
                .with_uniform_padding(4.0)
                .with_background_color(if state.is_hovered() {
                    theme.surface_2()
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
            })
            .finish()
        };
        // Always the stacked icon (it shows and hides the terminal under
        // the editor), in the accent colour while that terminal is open.
        let terminal_open = self.document_terminal_visible;
        let terminal_btn = Hoverable::new(header_mouse_state("editor_terminal"), move |state| {
            Container::new(render_icon(
                AppIcon::SplitVertical,
                if terminal_open {
                    theme.accent
                } else if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                ICON_SIZE_MD,
            ))
            .with_uniform_padding(4.0)
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx
                .dispatch_typed_action_for(workspace_view_id, WorkspaceAction::ToggleDocumentTerminal);
        })
        .finish();

        // Tab bar: the scrollable strip takes the remaining width and clips
        // underneath the pinned docked-terminal button on the right. Shared
        // `nav_header_bar` style with min-height 0: the bar shrink-fits the
        // 33px file cards (bar = cards + 1px divider = 34, same as the
        // terminal navbar), so no empty space sits below the cards.
        let header = super::ide_chrome::nav_header_bar(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(Expanded::new(1.0, tabs_bar).finish())
                .with_child(terminal_btn)
                .finish(),
            theme,
            0.0,
            8.0,
            0.0,
        );

        let body = Shrinkable::new(
            1.0,
            self.render_editor_center(
                theme,
                ui,
                app,
                header_mouse_state,
                self.work_item_tab_active,
            ),
        )
        .finish();
        let column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header)
            .with_child(body)
            .finish();

        let column = Clipped::new(column).finish();
        let column = if show_active_frame {
            let accent = ColorU::new(
                theme.accent.r,
                theme.accent.g,
                theme.accent.b,
                ACTIVE_PANE_ACCENT_ALPHA,
            );
            Container::new(column)
                .with_border(Border::all(2.0).with_border_color(accent))
                .finish()
        } else {
            column
        };

        // Editor content consumes clicks natively; always_handle keeps
        // pane-focus working everywhere inside the tile.
        let pane = EventHandler::new(column)
            .with_always_handle()
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::FocusSplitPane {
                        tab_id: EDITOR_PANE_ID,
                    },
                );
                DispatchEventResult::StopPropagation
            })
            .finish();
        pane
    }

    pub(crate) fn render_split_tab_pane(
        &self,
        tab_id: usize,
        theme: &Theme,
        ui: FamilyId,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let workspace_view_id = self.workspace_view_id;
        let tab = self
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .expect("split tab must exist");
        let is_active = tab_id == self.active_tab_id && !self.editor_pane_focused;
        let show_active_frame = is_active
            && self
                .split_tree
                .as_ref()
                .is_some_and(|layout| layout.pane_count() > 1);
        let is_work_item_pane =
            tab.work_item_id == Some(self.workspace_work_item_id) && self.work_item_tab_open;
        // Full header identity: `Agent · [login] · [live work context]`.
        // Login only when multiple identities exist (never for Dragent);
        // work context only while the pane is not idle, updating live.
        let agent_chip = self.full_pane_chip(tab, app);
        // Effective status folds terminal signals + Dragent runtime
        // (principal / pipeline) so internal, external CLI and
        // Dragent-terminal agents all update the navbar together.
        let pane_status = self.effective_pane_status(tab, app);
        // Canonical lifecycle text — covers internal, external CLI and
        // Dragent-terminal agents (not just `agent_id`/`active_ai_tool`).
        // Idle hides so plain terminals stay clean; any active lifecycle
        // shows with its status color + glow in the header.
        let runtime_status: Option<&str> = match pane_status {
            super::worktree_status::WorktreeAgentStatus::Working { .. } => Some("Running"),
            super::worktree_status::WorktreeAgentStatus::Permission => Some("Needs input"),
            super::worktree_status::WorktreeAgentStatus::Failed => Some("Failed"),
            super::worktree_status::WorktreeAgentStatus::Review => Some("Review"),
            super::worktree_status::WorktreeAgentStatus::Idle => None,
        };
        let header_title = if is_work_item_pane {
            format!("Work: {}", self.work_item_display_title())
        } else {
            agent_chip.label
        };
        let header_status = if is_work_item_pane {
            &super::worktree_status::WorktreeAgentStatus::Idle
        } else {
            &pane_status
        };
        let icon_tint = if matches!(
            header_status,
            super::worktree_status::WorktreeAgentStatus::Idle
        ) {
            theme.main_text()
        } else {
            header_status.color(theme)
        };

        let close_state = Self::split_mouse_state(&self.split_close_mouse_states, tab_id);
        let split_state = self
            .split_action_mouse_states
            .borrow_mut()
            .entry(format!("split_{tab_id}"))
            .or_insert_with(MouseStateHandle::default)
            .clone();
        // The pane's split button offers the opposite of its parent split
        // (panes in a side-by-side group split stacked and vice versa), so
        // layouts alternate as they nest. A standalone pane has no parent,
        // so its first split opens side-by-side horizontally.
        let split_direction = self
            .split_tree
            .as_ref()
            .and_then(|layout| layout.parent_direction(tab_id))
            .map(|parent| match parent {
                SplitDirection::Horizontal => SplitDirection::Vertical,
                SplitDirection::Vertical => SplitDirection::Horizontal,
            })
            .unwrap_or(SplitDirection::Horizontal);

        let share_state = self
            .split_action_mouse_states
            .borrow_mut()
            .entry(format!("split_share_{tab_id}"))
            .or_insert_with(MouseStateHandle::default)
            .clone();
        let header = split_tab_header::render_split_tab_header(
            ui,
            theme,
            agent_chip.icon.render(icon_tint, crate::icons::ICON_SIZE_MD),
            runtime_status,
            header_status,
            &header_title,
            workspace_view_id,
            tab_id,
            is_active,
            close_state,
            split_state,
            share_state,
            split_direction,
            !is_work_item_pane,
        );

        let body = if is_work_item_pane {
            let work_mouse_states = &self.header_mouse_states;
            let work_mouse_state = move |key: &'static str| {
                work_mouse_states
                    .borrow_mut()
                    .entry(format!("work-pane-{key}"))
                    .or_insert_with(MouseStateHandle::default)
                    .clone()
            };
            self.render_editor_center(theme, ui, app, work_mouse_state, true)
        } else {
            self.render_tab_pane(&tab.pane, app)
        };
        let body = Shrinkable::new(1.0, body).finish();

        // A browser's own bar carries the pane actions; a second header row
        // would stack two bars.
        let column = if tab.pane.is_browser() {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(body)
                .finish()
        } else {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(header)
                .with_child(body)
                .finish()
        };

        // Clip the pane at its own bounds: terminal/editor child views carry
        // unbounded positioned layers that otherwise paint over the neighbor
        // pane while a divider drag squeezes them. Drop zones wrap outside
        // this clip so their indicators are never cut off.
        // Split nodes own the neutral shared separator. Only the focused pane
        // receives its own accent frame.
        let column = Clipped::new(column).finish();
        let column = if show_active_frame {
            let accent = ColorU::new(
                theme.accent.r,
                theme.accent.g,
                theme.accent.b,
                ACTIVE_PANE_ACCENT_ALPHA,
            );
            Container::new(column)
                .with_border(Border::all(2.0).with_border_color(accent))
                .finish()
        } else {
            column
        };

        // `always_handle`: terminal content and inputs consume clicks, so
        // without this only chrome clicks would ever reach us. Focusing on
        // any click is safe — refocusing the active pane is a no-op, and
        // interacting with a pane's own buttons should focus it anyway.
        let pane = EventHandler::new(column)
            .with_always_handle()
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::FocusSplitPane { tab_id },
                );
                DispatchEventResult::StopPropagation
            })
            .finish();
        pane
    }

    /// Render one split-tree node. `Split` nodes reuse the flat group's
    /// row/column arrangement per level, so divider drag, bounds, and
    /// separators behave identically at every nesting depth.
    /// `SavePosition` tag for a split container, so divider drags can
    /// read the split's measured extent from the position cache.
    pub(crate) fn split_position_tag(split_id: SplitId) -> String {
        format!("pane_split_{split_id}")
    }

    /// Recursive weight renderer: every child flexes by its tree share, so
    /// pixels derive top-down every frame. No pixel state, no measurement
    /// dependence — window resizes and sidebar collapses rescale ratios
    /// automatically. Horizontal splits read left-to-right, vertical
    /// top-to-bottom.
    pub(crate) fn render_split_node(
        &self,
        node: &LayoutNode,
        theme: &Theme,
        ui: FamilyId,
        app: &AppContext,
        header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    ) -> Box<dyn Element> {
        match node {
            LayoutNode::Pane { id } if *id == EDITOR_PANE_ID => {
                self.render_editor_pane(theme, ui, app, header_mouse_state)
            }
            LayoutNode::Pane { id } => self.render_split_tab_pane(*id, theme, ui, app),
            LayoutNode::Split {
                id,
                direction,
                children,
                ..
            } => {
                if children.is_empty() {
                    return Empty::new().finish();
                }
                let shares = node.shares();
                let horizontal = matches!(direction, SplitDirection::Horizontal);
                let mut row = if horizontal {
                    Flex::row()
                } else {
                    Flex::column()
                }
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max);
                for (i, (child, share)) in children.iter().zip(shares.iter()).enumerate() {
                    let pane_el = self.render_split_node(child, theme, ui, app, header_mouse_state);
                    row = row.with_child(Expanded::new(*share, pane_el).finish());
                    if i + 1 < children.len() {
                        row = row.with_child(self.render_weight_divider(*id, i, *direction, theme));
                    }
                }
                SavePosition::new(row.finish(), &Self::split_position_tag(*id)).finish()
            }
        }
    }

    /// One divider between children `idx`/`idx+1` of a split: the existing
    /// separator visual as the grab surface. Drags convert pixel deltas to
    /// weight shifts against the split's measured extent (read live from
    /// the position cache, so bounds are always this node's own).
    pub(crate) fn render_weight_divider(
        &self,
        split_id: SplitId,
        divider_idx: usize,
        direction: SplitDirection,
        theme: &Theme,
    ) -> Box<dyn Element> {
        let handle = self
            .split_drag_handles
            .borrow_mut()
            .entry((split_id, divider_idx))
            .or_insert_with(drag_resize_handle)
            .clone();
        let (separator, axis) = match direction {
            SplitDirection::Horizontal => (
                self.vertical_split_grab_area(theme),
                DragResizeAxis::Horizontal,
            ),
            SplitDirection::Vertical => (
                self.horizontal_split_grab_area(theme),
                DragResizeAxis::Vertical,
            ),
        };
        let horizontal = matches!(direction, SplitDirection::Horizontal);
        let workspace_view_id = self.workspace_view_id;
        let tag = Self::split_position_tag(split_id);
        DragResizeElement::with_axis(
            handle,
            separator,
            axis,
            move |delta_px, event_ctx, _app| {
                let extent = event_ctx
                    .element_position_by_id(&tag)
                    .map(|rect| {
                        let size = rect.size();
                        if horizontal {
                            size.x()
                        } else {
                            size.y()
                        }
                    })
                    .unwrap_or(0.0);
                if extent > 0.0 {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::SplitDividerDragged {
                            split_id,
                            divider_idx,
                            delta_px,
                            extent_px: extent,
                        },
                    );
                }
            },
            None,
        )
        .finish()
    }

    pub(crate) fn render_split_tree(
        &self,
        root: &LayoutNode,
        theme: &Theme,
        ui: FamilyId,
        app: &AppContext,
        header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    ) -> Box<dyn Element> {
        self.render_split_node(root, theme, ui, app, header_mouse_state)
    }

    pub(crate) fn render_tab_pane(
        &self,
        pane: &WorkspacePane,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        match pane {
            WorkspacePane::Terminal(p) => Shrinkable::new(1.0, ChildView::new(p).finish()).finish(),
            WorkspacePane::Settings(s) => Shrinkable::new(1.0, ChildView::new(s).finish()).finish(),
            WorkspacePane::Browser(b) => Shrinkable::new(1.0, ChildView::new(b).finish()).finish(),
        }
    }

    pub(crate) fn render_document_terminal_group(
        &self,
        app: &AppContext,
        theme: &Theme,
    ) -> Box<dyn Element> {
        let active = self.active_document_terminal();
        let body = match active {
            Some(active) => {
                let active_body = self.render_tab_pane(&active.pane, app);
                let split = self
                    .document_terminal_split_id
                    .filter(|id| Some(*id) != self.document_terminal_active_id)
                    .and_then(|id| {
                        self.document_terminals.iter().find(|tab| {
                            tab.id == id
                                && self
                                    .document_owner_root
                                    .as_ref()
                                    .is_some_and(|root| tab.owner_root == *root)
                        })
                    });
                if let Some(split) = split {
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_child(
                            Expanded::new(1.0, self.render_tab_pane(&split.pane, app)).finish(),
                        )
                        .with_child(self.vertical_split_grab_area(theme))
                        .with_child(Expanded::new(1.0, active_body).finish())
                        .finish()
                } else {
                    active_body
                }
            }
            None => Empty::new().finish(),
        };

        let header_tabs = self
            .document_terminals
            .iter()
            .filter(|tab| {
                self.document_owner_root
                    .as_ref()
                    .is_some_and(|root| tab.owner_root == *root)
            })
            .map(|tab| {
                let mut label = tab.pane.display_title(app);
                if label.trim().is_empty() || label == DEFAULT_SESSION_TITLE {
                    label = tab
                        .pane
                        .cwd(app)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .filter(|name| !name.is_empty())
                        .unwrap_or("Terminal")
                        .to_string();
                }
                super::docked_terminal::HeaderTab {
                    id: tab.id,
                    label,
                    active: self.document_terminal_active_id == Some(tab.id),
                }
            })
            .collect();
        let header = super::docked_terminal::render_header(
            header_tabs,
            self.document_terminal_maximized,
            theme,
            self.ui_font,
            self.workspace_view_id,
            &self.header_mouse_states,
            self.document_terminal_tabs_scroll.clone(),
        );

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header)
            .with_child(Expanded::new(1.0, body).finish())
            .finish()
    }
}
