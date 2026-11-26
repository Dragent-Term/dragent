//! Post-construction initialization for WorkspaceView.
//!
//! Responsible for: work-item agent ensure, recovery mounts, dock terminal
//! restoration, subscriptions, polls, and automation bridge wiring that run
//! after the view value exists.
//!
//! Not responsible for: building the initial value (view_build::build).
//!
//! Moved verbatim; the `view` parameter is the value under construction.

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
use super::view_state::{DocumentTerminalTab, RemotePointer};
use super::view_tabs::{
    apply_sidebar_visibility, refresh_sidebar_projects, subscribe_terminal_pane,
    subscribe_workspace_pane,
};

impl WorkspaceView {
    pub(crate) fn complete_construction(
        mut view: &mut WorkspaceView,
        ctx: &mut ViewContext<WorkspaceView>,
        restored_document: &dragent_persistence::SerializedDocumentSession,
        document_root: std::path::PathBuf,
        layout_migration_pending: bool,
        workspace_work_item_id: dragent_workspace_model::WorkItemId,
        theme: crate::theme::Theme,
        ui_font: warpui::fonts::FamilyId,
        restored_blocks: dragent_persistence::RestoredBlockLists,
        restored_agent_blocks: dragent_persistence::RestoredAgentBlockLists,
        document_tree_watch_rx: async_channel::Receiver<super::project_documents::TreeWatcherEvent>,
    ) {
        let restore_document_root = document_root.clone();
        view.ensure_work_item_agent(workspace_work_item_id, ctx);
        view.mount_recovered_attempts(ctx);
        // A Principal turn the app stopped in the middle of resumes, like an
        // agent's session — once sign-in has had a moment to restore.
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(4)),
            |view, _, ctx| view.resume_interrupted_shape_turn(ctx),
        );
        view.reconcile_workspace_tool_outcomes(ctx);
        view.reconcile_workspace_integrations(ctx);
        view.poll_agent_usage(ctx);
        view.refresh_community(ctx);

        for dock in restored_document.dock_terminals.clone() {
            let serialized_cwd = match &dock.pane {
                SerializedPaneLayout::Single { cwd, .. } => cwd.clone(),
                SerializedPaneLayout::Split { .. } => continue,
            };
            let owner_root = if dock.owner_root.as_os_str().is_empty() {
                document_root.clone()
            } else {
                dock.owner_root.clone()
            };
            let pane = if dock.borrowed {
                // Older sessions could point the dock at a top-level terminal.
                // Materialize a separate terminal during restore so resizing
                // either placement cannot relayout the other one.
                let cwd = if serialized_cwd.is_dir() {
                    serialized_cwd
                } else {
                    owner_root.clone()
                };
                #[cfg(not(test))]
                let pane = view.create_host_auxiliary_pane(cwd, ctx);
                #[cfg(test)]
                let pane = WorkspacePane::spawn_new(ctx, theme, ui_font, cwd);
                pane
            } else {
                let (pane, reattach) = Self::reconstruct_pane(
                    &dock.pane,
                    &restored_blocks,
                    &restored_agent_blocks,
                    theme,
                    ui_font,
                    dock.host_binding.as_ref(),
                    ctx,
                );
                if let Some(mut reattach) = reattach {
                    reattach.project_root = Some(owner_root.clone());
                    let registry_id = view.next_tab_id;
                    view.next_tab_id += 1;
                    view.attach_restored_host_tabs(ctx, vec![(registry_id, reattach)]);
                }
                pane
            };
            if let WorkspacePane::Terminal(terminal) = &pane {
                subscribe_terminal_pane(
                    None,
                    terminal,
                    view.persistence_writer.sender(),
                    false,
                    ctx,
                );
            }
            view.register_surface(&pane, ctx);
            view.document_terminals.push(DocumentTerminalTab {
                id: dock.id,
                surface_id: pane.surface_id(ctx),
                pane,
                borrowed: false,
                owner_root,
            });
        }
        if view.document_center_open && view.document_terminal_visible {
            view.set_document_terminal_backdrops(true, ctx);
        }

        let writer = view.persistence_writer.sender();
        let pane_titles: Vec<(EntityId, String)> = view
            .tabs
            .iter()
            .map(|tab| (tab.pane.id(), tab.pane.display_title(ctx)))
            .collect();
        let parked_panes = view
            .parked_workspace_tabs
            .iter()
            .flat_map(|workspace_tab| workspace_tab.panes.iter())
            .map(|tab| (tab.id, tab.pane.clone()))
            .collect::<Vec<_>>();
        for tab in &view.tabs {
            subscribe_workspace_pane(tab.id, &tab.pane, writer.clone(), ctx);
        }
        for (tab_id, pane) in &parked_panes {
            subscribe_workspace_pane(*tab_id, pane, writer.clone(), ctx);
        }
        for (pane_id, title) in pane_titles {
            view.apply_pane_title(pane_id, title);
        }

        view.sync_all_pane_colors(ctx);
        view.sync_pane_activity(ctx);
        view.sync_left_panel(ctx);
        // Restored document metadata is normally dormant while the project
        // tree loads. A shutdown snapshot whose editor was visible is opened
        // below, after all panes and models are registered.

        // The centre document is drawn by this view from the sidebar's state
        // (the diff comment card among it), so its changes repaint here.
        let project_sidebar_view = view.project_sidebar_view.clone();
        ctx.subscribe_to_view(&project_sidebar_view, |_, _, _, ctx| ctx.notify());
        // Agent states the sidebar learns from hooks reach the phone's
        // catalog, which is collected as this view draws.
        let left_panel_view = view.left_panel_view.clone();
        ctx.subscribe_to_view(&left_panel_view, |_, _, _, ctx| ctx.notify());
        let shape_composer_editor = view.shape_composer_editor.clone();
        ctx.subscribe_to_view(&shape_composer_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.shape_composer_text = text.clone();
                ctx.notify();
            }
        });
        let attention_response_editor = view.attention_response_editor.clone();
        ctx.subscribe_to_view(&attention_response_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.attention_response_text = text.clone();
                ctx.notify();
            }
        });
        let shape_record_editor = view.shape_record_editor.clone();
        ctx.subscribe_to_view(&shape_record_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.shape_record_edit_text = text.clone();
                ctx.notify();
            }
        });
        let project_start_editor = view.project_start_editor.clone();
        ctx.subscribe_to_view(&project_start_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.project_start_text = text.clone();
                view.project_start_error = None;
                ctx.notify();
            }
        });
        let clone_url_editor = view.clone_url_editor.clone();
        ctx.subscribe_to_view(&clone_url_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.clone_url_text = text.clone();
                if !view.clone_busy {
                    view.clone_url_error = None;
                }
                ctx.notify();
            }
        });
        let clone_location_editor = view.clone_location_editor.clone();
        ctx.subscribe_to_view(&clone_location_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.clone_location_text = text.clone();
                if !view.clone_busy {
                    view.clone_url_error = None;
                }
                ctx.notify();
            }
        });
        #[cfg(not(target_family = "wasm"))]
        {
            let editor = view.review_instructions_editor.clone();
            ctx.subscribe_to_view(&editor, |view, _, event, ctx| {
                if let InputEditorEvent::Changed { text, .. } = event {
                    view.review_instructions = text.clone();
                    ctx.notify();
                }
            });
        }
        let prompt_editor = view.prompt_editor.clone();
        ctx.subscribe_to_view(&prompt_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.prompt_text = text.clone();
                view.save_session(ctx);
                ctx.notify();
            }
        });
        let link_search_editor = view.prompt_link_search_editor.clone();
        ctx.subscribe_to_view(&link_search_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.prompt_link_query = text.clone();
                view.refresh_prompt_links(ctx);
            }
        });
        let branch_search_editor = view.prompt_branch_search_editor.clone();
        ctx.subscribe_to_view(&branch_search_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.prompt_branch_search = text.clone();
                view.prompt_menu_scroll = ClippedScrollStateHandle::new();
                ctx.notify();
            }
        });
        let past_sessions_search_editor = view.past_sessions_search_editor.clone();
        ctx.subscribe_to_view(&past_sessions_search_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.past_sessions_search = text.clone();
                view.past_sessions_scroll = Default::default();
                ctx.notify();
            }
        });
        let agent_search_editor = view.prompt_agent_search_editor.clone();
        ctx.subscribe_to_view(&agent_search_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.prompt_agent_search = text.clone();
                view.prompt_menu_scroll = ClippedScrollStateHandle::new();
                ctx.notify();
            }
        });

        ctx.spawn_stream_local(
            document_tree_watch_rx,
            |view, event, ctx| view.refresh_document_tree_from_watcher(event, ctx),
            |_, _| {},
        );

        // Seed sidebar from all open tabs.
        refresh_sidebar_projects(&mut view, ctx);
        apply_sidebar_visibility(&mut view, ctx);
        // The updater runs off-thread. Keep every open workspace header
        // responsive to its state without requiring Settings → Check for
        // Updates or another unrelated UI event to trigger a repaint.
        view.schedule_update_poll(ctx);
        view.refresh_review_ship_state(ctx);
        view.refresh_review_pr_state(ctx);

        ctx.subscribe_to_model(&view.project_sidebar_model, |view, _, event, ctx| {
            use super::project_sidebar::SidebarEvent;
            if matches!(event, SidebarEvent::Changed) {
                apply_sidebar_visibility(view, ctx);
                if let Some(preview_path) = view.document_preview_path.as_ref() {
                    let promoted = view.project_sidebar_model.read(ctx, |model, _| {
                        model.document_is_dirty(&view.document_index_root, preview_path)
                    });
                    if promoted {
                        log::debug!(
                            "[editor-tab] promoted dirty preview path={}",
                            preview_path.display()
                        );
                        view.document_preview_path = None;
                    }
                }
                view.synchronize_language_document(ctx);
                let psv = view.project_sidebar_view.clone();
                ctx.update_view(&psv, |_, pane_ctx| pane_ctx.notify());
                view.document_backup_epoch = view.document_backup_epoch.wrapping_add(1);
                let epoch = view.document_backup_epoch;
                ctx.spawn(
                    Timer::after(Duration::from_millis(500)),
                    move |view, _, ctx| {
                        if view.document_backup_epoch != epoch {
                            return;
                        }
                        // Recovery is secondary work. Snapshot drafts only
                        // after the editing burst settles; cloning every open
                        // draft synchronously for every character made the
                        // debounce below ineffective at protecting input.
                        let (active_path, copies) =
                            view.project_sidebar_model.read(ctx, |model, _| {
                                (model.active_relative_path(), model.working_copies())
                            });
                        if let Some(active_path) = active_path {
                            if !copies.iter().any(|copy| copy.relative_path == active_path) {
                                view.persistence_writer.stage_working_copy_delete(
                                    view.document_index_root.clone(),
                                    active_path,
                                );
                            }
                        }
                        for copy in copies {
                            log::debug!(
                                "[editor-recovery] staged path={} revision={} bytes={}",
                                copy.relative_path.display(),
                                copy.revision,
                                copy.draft_text.len()
                            );
                            view.persistence_writer.stage_working_copy(copy);
                        }
                        view.persistence_writer.flush_staged_working_copies();
                        view.save_session(ctx);
                    },
                );
                ctx.notify();
            }
        });

        // Shell hooks report the real PWD shortly after spawn — refresh once cwd settles.
        ctx.spawn(
            Timer::after(Duration::from_millis(300)),
            move |view, _, ctx| {
                refresh_sidebar_projects(view, ctx);
                apply_sidebar_visibility(view, ctx);
                let psv = view.project_sidebar_view.clone();
                ctx.update_view(&psv, |_, c| c.notify());
                ctx.notify();
            },
        );

        // Refresh the active explorer immediately using the same bounded
        // background path as Refresh Explorer. Include restored expanded folders;
        // a recursive startup scan skips lazy directories and can leave those
        // folders visibly empty despite their saved "expanded" state.
        view.refresh_document_tree(ctx);

        chrome_inset::sync_native_window_chrome(ctx.windows(), ctx.window_id());
        view.sync_window_title(ctx);

        // One machine store feeds Settings, Add Project and remote projects.
        let machines =
            <crate::settings::machines_store::MachinesStore as warpui::SingletonEntity>::handle(
                ctx,
            );
        ctx.subscribe_to_model(&machines, |ws, _, _event, ctx| {
            ws.remote_machines_changed(ctx)
        });
        // Remote projects restored from the last session reconnect their
        // machines, so their files and terminals come back like local ones.
        let restored_machines: std::collections::BTreeSet<String> = view
            .open_project_roots
            .iter()
            .chain(view.workspaces.iter().map(|workspace| &workspace.root))
            .filter_map(|root| crate::remote_fs::resolve(root).map(|remote| remote.machine_id))
            .collect();
        for machine in restored_machines {
            ctx.update_model(&machines, |store, ctx| store.connect(&machine, ctx));
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let page = view.automations_view.clone();
            ctx.subscribe_to_view(&page, |ws, _, event, ctx| match event {
                crate::automations::AutomationsEvent::OpenRun(run) => {
                    ws.open_automation_run(run.clone(), ctx);
                }
                crate::automations::AutomationsEvent::CreateWithAgent(agent, project) => {
                    ws.create_automation_with_agent(*agent, project.clone(), ctx);
                }
                crate::automations::AutomationsEvent::OpenBilling => {
                    ws.close_automations(ctx);
                    ws.handle_action(&WorkspaceAction::OpenBillingFromHeader, ctx);
                }
                crate::automations::AutomationsEvent::InstallAgent(agent) => {
                    ws.handle_action(
                        &WorkspaceAction::OpenPromptAgentInstall { agent: *agent },
                        ctx,
                    );
                }
                crate::automations::AutomationsEvent::RefreshAgents => {
                    ws.refresh_prompt_agents(ctx);
                    ws.sync_automation_agents(ctx);
                }
            });
            // The Teams page points at agents' tabs; the sidebar counts
            // what its leads left for the person.
            let prs = view.pull_requests_view.clone();
            ctx.subscribe_to_view(&prs, |ws, _, event, ctx| {
                ws.on_pull_request_event(event, ctx)
            });
            let teams = view.teams_view.clone();
            ctx.subscribe_to_view(&teams, |ws, _, event, ctx| match event {
                crate::teams::TeamsEvent::OpenPane { pane, workspace } => {
                    ws.close_teams(ctx);
                    ws.open_team_pane(pane, workspace.clone(), ctx);
                }
                crate::teams::TeamsEvent::NewLead {
                    team,
                    agent,
                    model,
                    cwd,
                    prompt,
                } => {
                    ws.start_new_lead(team.clone(), agent, model.clone(), cwd.clone(), prompt, ctx);
                }
                crate::teams::TeamsEvent::CleanUp { project, remove } => {
                    ws.clean_up_team(project.clone(), remove.clone(), false, ctx);
                }
                crate::teams::TeamsEvent::Delete {
                    project,
                    remove,
                    workers,
                } => {
                    ws.delete_team_work(project.clone(), remove.clone(), workers, ctx);
                }
            });
            let tasks = view.tasks_view.clone();
            ctx.subscribe_to_view(&tasks, |ws, _, event, ctx| ws.on_tasks_event(event, ctx));
            view.poll_teams_badge(ctx);
            // Started runs go into the sidebar; Dragent-agent runs start in
            // a pane; the nav badge follows failures.
            let runtime = crate::automations::AutomationRuntime::handle(ctx);
            ctx.subscribe_to_model(&runtime, |ws, _, event, ctx| {
                ws.on_automation_event(event, ctx);
            });
            // `dragent workspaces create` and other command-line requests.
            let inbox = crate::cli::inbox::CliInbox::handle(ctx);
            ctx.subscribe_to_model(&inbox, |ws, _, event, ctx| match event {
                crate::cli::inbox::CliInboxEvent::Request(id) => ws.on_cli_request(id, ctx),
            });
            ctx.update_model(&inbox, |inbox, _| inbox.window_ready());
            // A paired phone's "+ New tab" in one of this window's workspaces.
            view.listen_for_companion_tabs(ctx);
            // Work a paired phone starts: a task, a new coordination.
            view.listen_for_companion_starts(ctx);
            // Idle agents are put to sleep to free memory.
            view.start_agent_sleep(ctx);
            // What Dragent took back out of other agents, said once.
            #[cfg(not(target_family = "wasm"))]
            ctx.spawn(warpui::r#async::Timer::after(std::time::Duration::from_secs(4)), |view, _, ctx| {
                if let Some(note) = crate::cli::agent_skill::take_cleanup_note() {
                    view.show_error_notice(
                        "Your other agents",
                        format!(
                            "{note} Dragent now changes your other agents only when you ask: agents you start from Dragent still get its skills and tools for that session. To use a skill outside Dragent too, install it in Settings → Skills."
                        ),
                        ctx,
                    );
                }
            });
            // Sleeping agents a team needs are woken on request.
            view.start_wake_requests(ctx);
            // Agents that wait for memory, or are paused, are shown.
            view.start_agent_resources(ctx);
            // Work item tasks agent CLIs were still on when the app closed.
            ctx.spawn(
                warpui::r#async::Timer::after(std::time::Duration::from_secs(3)),
                |ws, _, ctx| ws.schedule_cli_worker_poll(ctx),
            );
            // Runs that started before this window existed.
            ctx.spawn(
                warpui::r#async::Timer::after(std::time::Duration::from_secs(3)),
                |ws, _, ctx| ws.adopt_pending_automation_runs(ctx),
            );
        }

        let settings_handle = view.settings_view.clone();
        ctx.subscribe_to_view(&settings_handle, |ws, _, event, ctx| {
            match event {
                SettingsEvent::AgentAccountError(error) => {
                    ws.show_error_notice("Agent account", error.clone(), ctx)
                }
                SettingsEvent::AgentAccountLogin(id) => {
                    match dragent_host::agent_sessions::helper_executable() {
                        Ok(helper) => {
                            let shell = dragent_pty::resolve_shell(None);
                            let command = format!(
                                "{} --dragent-agent-account-login {}",
                                dragent_host::agent_sessions::quote(
                                    &helper.to_string_lossy(),
                                    &shell
                                ),
                                id
                            );
                            ws.open_page_command_terminal(
                                "Agent account sign-in",
                                None,
                                &command,
                                ctx,
                            );
                        }
                        Err(error) => ws.show_error_notice("Agent account", error, ctx),
                    }
                }
                SettingsEvent::AgentAccountSignIn {
                    machine_id,
                    provider,
                    title,
                    command,
                } => {
                    // Settings never routes to a terminal tab: sign-in runs
                    // in the embedded terminal over Settings.
                    match machine_id {
                        None => ws.open_page_command_terminal(title, None, command, ctx),
                        Some(id) => ws.open_account_sign_in_terminal(id, title, command, ctx),
                    }
                    ws.describe_account_sign_in(provider, ctx);
                }
                SettingsEvent::AgentAccountSignedIn => {
                    // Already over, so closing reports nothing back.
                    if ws.account_sign_in.take().is_some() {
                        ws.close_prompt_agent_install(ctx);
                    }
                }
                SettingsEvent::Project(action) => ws.handle_projects_settings(action, ctx),
                SettingsEvent::MachinesChanged => ws.remote_machines_changed(ctx),
                SettingsEvent::AddRemoteProject(machine) => {
                    ws.handle_add_project_action(
                        &super::add_project::AddProjectAction::Open {
                            machine: Some(machine.clone()),
                        },
                        ctx,
                    );
                }
                SettingsEvent::OpenRemoteProject(id) => ws.open_remote_project(id, ctx),
                SettingsEvent::RemoveRemoteProject(id) => {
                    ws.remove_remote_project(id, ctx);
                    ctx.notify();
                }
                SettingsEvent::AttachRemoteSession {
                    machine_id,
                    session,
                } => {
                    ws.handle_remote_action(
                        &super::remote_project::RemoteAction::Attach {
                            machine_id: machine_id.clone(),
                            session: session.clone(),
                        },
                        ctx,
                    );
                }
                SettingsEvent::RunToolJob { machine_id, job } => {
                    ws.handle_remote_action(
                        &super::remote_project::RemoteAction::RunToolJob {
                            machine_id: machine_id.clone(),
                            job: job.clone(),
                        },
                        ctx,
                    );
                }
                SettingsEvent::Close => {
                    ws.close_settings(ctx);
                }
                SettingsEvent::OpenSkill(path) => {
                    let global = crate::agent::skill_discovery::global_skills_dir();
                    let root = if path.starts_with(&global) {
                        global
                    } else {
                        crate::agent::skill_discovery::resolve_project_root(path)
                    };
                    if let Ok(relative) = path.strip_prefix(&root) {
                        let relative = relative.to_path_buf();
                        ws.close_settings(ctx);
                        ws.document_center_open = true;
                        ws.document_tab_id = Some(ws.active_tab_id);
                        ws.document_owner_root = Some(root);
                        ws.open_document_path(relative, false, ctx);
                    }
                }
                SettingsEvent::ThemeChanged(theme_id) => {
                    let new_theme = Theme::from_id(theme_id);
                    ws.theme = new_theme;

                    let lp = ws.left_panel_view.clone();
                    ctx.update_view(&lp, |lp_view, lp_ctx| {
                        lp_view.set_theme(new_theme, lp_ctx);
                    });

                    let psv = ws.project_sidebar_view.clone();
                    ctx.update_view(&psv, |ps_view, ps_ctx| {
                        ps_view.set_theme(new_theme, ps_ctx);
                    });

                    for agent in ws
                        .work_item_agents
                        .values()
                        .chain(ws.attempt_agents.values())
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        ctx.update_view(&agent, |agent, agent_ctx| {
                            agent.set_theme(new_theme, agent_ctx);
                        });
                    }

                    let shape_composer_editor = ws.shape_composer_editor.clone();
                    ctx.update_view(&shape_composer_editor, |editor, editor_ctx| {
                        editor.set_theme(new_theme, editor_ctx);
                    });
                    let shape_record_editor = ws.shape_record_editor.clone();
                    ctx.update_view(&shape_record_editor, |editor, editor_ctx| {
                        editor.set_theme(new_theme, editor_ctx);
                    });
                    let prs = ws.pull_requests_view.clone();
                    ctx.update_view(&prs, |page, c| page.set_theme(new_theme, c));
                    let teams = ws.teams_view.clone();
                    ctx.update_view(&teams, |page, page_ctx| {
                        page.set_theme(new_theme, page_ctx);
                    });
                    let tasks = ws.tasks_view.clone();
                    ctx.update_view(&tasks, |page, page_ctx| page.set_theme(new_theme, page_ctx));

                    ws.sync_all_pane_colors(ctx);
                    ws.schedule_agent_theme_restarts(ctx);
                    ctx.notify();
                }
                SettingsEvent::EditorSettingsChanged => {
                    log::info!(
                        "[settings] EditorSettingsChanged received — syncing all terminal panes"
                    );
                    // Point the interface and code font roles at the new
                    // families and weight; every view keeps its role ids, so
                    // repainting all windows is enough for their text.
                    let font_cache = ctx.get_singleton_model_handle::<FontCache>();
                    let new_ui_font = ctx.update_model(&font_cache, |cache, _| {
                        crate::fonts::apply_typography(cache);
                        crate::workspace::prewarm::load_ui_font(cache)
                    });
                    ws.ui_font = new_ui_font;
                    let sv = ws.settings_view.clone();
                    ctx.update_view(&sv, |view, vctx| view.apply_typography(new_ui_font, vctx));
                    ws.apply_editor_settings_all(ctx);
                    // Terminals and editors in other windows re-measure too.
                    let this_window = ctx.window_id();
                    let other_workspaces = ctx
                        .window_ids()
                        .filter(|window| *window != this_window)
                        .filter_map(|window| ctx.root_view::<crate::root_view::RootView>(window))
                        .filter_map(|root| root.read(ctx, |root, _| root.workspace()))
                        .collect::<Vec<_>>();
                    for workspace in other_workspaces {
                        ctx.update_view(&workspace, |other, other_ctx| {
                            other.apply_editor_settings_all(other_ctx);
                            other_ctx.notify();
                        });
                    }
                    ctx.invalidate_all_views();
                    ctx.notify();
                }
                SettingsEvent::AppSettingsChanged => {
                    // Retry deferred language startup immediately when the trust
                    // policy changes, including while Settings is open.
                    ws.synchronize_language_document(ctx);
                    let enabled = crate::workspace_features::agentic_workspace_enabled(
                        crate::settings::AppSettings::global()
                            .agent
                            .agentic_workspace_enabled,
                    );
                    let persisted_layout = ws
                        .layout_collection
                        .active()
                        .and_then(|layout| layout.built_in)
                        .unwrap_or(BuiltInLayout::Develop);
                    ws.agentic_workspace_enabled = enabled;
                    // Shortcuts: re-register overrides live so a
                    // rebind/reset in Settings → Keyboard applies without a
                    // restart, and reconcile the keep-awake lock.
                    super::sync_shortcut_bindings(ctx);
                    crate::settings::shortcut_settings::sync_wake_lock(
                        &crate::settings::AppSettings::global().dev_permissions,
                    );
                    if !enabled && persisted_layout != BuiltInLayout::Develop {
                        ws.select_workspace_layout(BuiltInLayout::Develop, ctx);
                    } else {
                        ctx.notify();
                    }
                }
            }
        });

        let auth = AuthManager::handle(ctx);
        view.account_logged_in_cache = ctx.read_model(&auth, |a, _| a.is_logged_in());
        ctx.subscribe_to_model(&auth, |ws, _, event, ctx| {
            if matches!(event, AuthManagerEvent::AuthFailed { .. }) && ws.login_prompt_open {
                // The sign-in window says what went wrong.
                ctx.notify();
            }
            if matches!(
                event,
                AuthManagerEvent::AuthComplete
                    | AuthManagerEvent::SessionRestored
                    | AuthManagerEvent::LoggedOut
                    | AuthManagerEvent::SessionExpired
            ) {
                #[cfg(not(target_family = "wasm"))]
                if matches!(
                    event,
                    AuthManagerEvent::LoggedOut | AuthManagerEvent::SessionExpired
                ) {
                    ws.pull_requests_open = false;
                    ctx.update_view(&ws.pull_requests_view, |page, c| page.reset(c));
                }
                let auth = AuthManager::handle(ctx);
                ws.account_logged_in_cache = ctx.read_model(&auth, |a, _| a.is_logged_in());
                if ws.account_logged_in_cache {
                    // A sign-in started from the prompt finished.
                    ws.login_prompt_open = false;
                }
                let sessions = SessionManager::handle(ctx);
                let authenticated = ws.account_logged_in_cache;
                ctx.update_model(&sessions, |manager, mctx| {
                    manager.set_forwarding_authenticated(mctx, authenticated)
                });
                ctx.notify();
            }
        });

        let session_mgr = SessionManager::handle(ctx);
        let authenticated = view.account_logged_in_cache;
        ctx.update_model(&session_mgr, |manager, mctx| {
            manager.set_forwarding_authenticated(mctx, authenticated)
        });
        ctx.subscribe_to_model(&session_mgr, |ws, mgr, event, ctx| match event {
            SessionManagerEvent::Changed => {
                ws.refresh_share_snapshot(ctx);
                let present: std::collections::HashSet<ParticipantId> = mgr
                    .read(ctx, |manager, _| {
                        manager.participants().iter().map(|p| p.id).collect()
                    });
                ws.remote_pointers.retain(|id, _| present.contains(id));
                ws.sync_left_panel(ctx); // reflect share state in the tab menu
                                         // Keep the desktop mirror's control + size-authority flags in sync
                                         // with the session: a viewer without edit is read-only, and only a
                                         // host-account controller may drive the host's terminal size.
                if let Some(h) = ws.mirror_pane.clone() {
                    let sm = crate::realtime::manager::SessionManager::handle(ctx);
                    let (can_edit, is_host_account) = ctx.read_model(&sm, |m, _| {
                        (m.viewer_can_edit(), m.viewer_is_host_account())
                    });
                    ctx.update_view(&h, |tv, tctx| {
                        tv.set_remote_read_only(!can_edit, tctx);
                        tv.set_remote_is_host_account(is_host_account, tctx);
                    });
                }
                ctx.notify();
            }
            SessionManagerEvent::Ended(_reason) => {
                if let Some(mirror_tab_id) = ws.mirror_tab_id {
                    // A viewer tab is no longer usable after the remote session
                    // ends. Remove it immediately so the workspace falls back to
                    // another tab (or creates a fresh terminal) instead of leaving
                    // the desktop trapped behind a full-window ended card.
                    ws.shared_session_ended = None;
                    ws.mirror_tab_id = None;
                    ws.mirror_pane = None;
                    ws.handle_action(
                        &WorkspaceAction::CloseTabConfirmed {
                            tab_id: mirror_tab_id,
                        },
                        ctx,
                    );
                }
                // Session ended (incl. inactivity auto-end) — clear share modals.
                ws.share_open = false;
                ws.stop_confirm_open = false;
                ws.idle_warning_open = false;
                ws.remote_pointers.clear();
                // Host: stop following a (now-gone) controller's size — resume
                // sizing the shared pane's PTY from the host's own window.
                for tab in &ws.tabs {
                    if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                        let h = h.clone();
                        ctx.update_view(&h, |tv, tctx| tv.clear_remote_resize(tctx));
                    }
                }
                ws.refresh_share_snapshot(ctx);
                ws.sync_left_panel(ctx); // "Stop session" → "Share session"
                ctx.notify();
            }
            SessionManagerEvent::Error(_) => ctx.notify(),
            SessionManagerEvent::ForwardCreated { .. } => {
                ws.show_ports_notice("Port forward ready", ctx);
            }
            SessionManagerEvent::ForwardOpFinished { notice, error } => {
                ws.show_ports_notice(error.clone().unwrap_or_else(|| notice.clone()), ctx);
            }
            SessionManagerEvent::MirrorOutput { bytes } => {
                if let Some(h) = ws.mirror_pane.clone() {
                    let bytes = bytes.clone();
                    ctx.update_view(&h, |tv, tctx| tv.feed_remote_output(&bytes, tctx));
                }
            }
            // The host's own block rendering — authoritative, applied AFTER the
            // raw feed so it overwrites whatever the re-parse produced.
            SessionManagerEvent::MirrorBlock { delta } => {
                if let Some(h) = ws.mirror_pane.clone() {
                    let delta = delta.clone();
                    ctx.update_view(&h, |tv, tctx| tv.apply_remote_block_delta(&delta, tctx));
                }
            }
            SessionManagerEvent::MirrorAgentBlocks { blocks } => {
                if let Some(h) = ws.mirror_pane.clone() {
                    let blocks = blocks.clone();
                    ctx.update_view(&h, |tv, tctx| tv.apply_remote_agent_blocks(blocks, tctx));
                }
            }
            SessionManagerEvent::ShareOpFinished { op, error } => {
                use crate::workspace::share_sheet::ShareOp;
                let msg = match (op, error.as_ref()) {
                    (ShareOp::LinkAccess, None) => "Link updated",
                    (ShareOp::Invite, None) => "Invite sent",
                    (ShareOp::LinkAccess, Some(_)) => "Couldn't update the link",
                    (ShareOp::Invite, Some(_)) => "Couldn't send the invite",
                };
                ws.show_share_notice(msg, ctx);
                ws.refresh_share_snapshot(ctx);
                ctx.notify();
            }
            SessionManagerEvent::JoinRequested { session, secret } => {
                ws.join_session(session.clone(), secret.clone(), ctx);
            }
            SessionManagerEvent::AccessRequested {
                participant,
                display_name,
            } => {
                ws.pending_access_request = Some((participant.0.to_string(), display_name.clone()));
                ctx.notify();
            }
            SessionManagerEvent::RunInput { bytes } => {
                // Host: a controller's forwarded input — write it to the shared
                // pane's PTY (server only forwards it for Edit participants).
                if let Some(target) = mgr.read(ctx, |m, _| m.shared_pane()) {
                    let bytes = bytes.clone();
                    if let Some(tab) = ws.tabs.iter().find(|t| t.pane.pane_uuid(ctx) == target) {
                        if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                            h.read(ctx, |tv, _| tv.write_pty_bytes(&bytes));
                        }
                    }
                }
            }
            SessionManagerEvent::RunAgentQuery {
                request_id: _,
                from,
                query,
            } => {
                let author = mgr.read(ctx, |manager, _| {
                    manager
                        .participants()
                        .iter()
                        .find(|participant| participant.id == *from)
                        .map(|participant| crate::agent::controller::AgentAuthor {
                            display_name: if participant.display_name.is_empty() {
                                "Guest".to_string()
                            } else {
                                participant.display_name.clone()
                            },
                            avatar_url: participant.avatar_url.clone(),
                        })
                });
                if let Some(target) = mgr.read(ctx, |m, _| m.shared_pane()) {
                    if let Some(tab) = ws.tabs.iter().find(|t| t.pane.pane_uuid(ctx) == target) {
                        if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                            let h = h.clone();
                            let query = query.clone();
                            ctx.update_view(&h, |tv, tctx| {
                                tv.submit_shared_agent_query(query, author, tctx)
                            });
                        }
                    }
                }
            }
            SessionManagerEvent::StopAgent => {
                if let Some(target) = mgr.read(ctx, |m, _| m.shared_pane()) {
                    if let Some(tab) = ws.tabs.iter().find(|t| t.pane.pane_uuid(ctx) == target) {
                        if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                            let h = h.clone();
                            ctx.update_view(&h, |tv, tctx| tv.stop_shared_agent(tctx));
                        }
                    }
                }
            }
            SessionManagerEvent::RunResize { rows, cols } => {
                // Host: a controller asked the shared PTY to match their window —
                // resize it so a TUI re-renders to fill their view.
                let (rows, cols) = (*rows, *cols);
                if let Some(target) = mgr.read(ctx, |m, _| m.shared_pane()) {
                    if let Some(tab) = ws.tabs.iter().find(|t| t.pane.pane_uuid(ctx) == target) {
                        if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                            let h = h.clone();
                            ctx.update_view(&h, |tv, tctx| {
                                tv.apply_remote_resize(rows, cols, tctx)
                            });
                        }
                    }
                }
            }
            SessionManagerEvent::ApplyControlSurface { from, surface } => {
                // Host: a controller's live input/scroll surface — reflect it on
                // the shared pane's own editors + scroll (server only forwards it
                // for Edit participants).
                let surface = surface.clone();
                let author = mgr.read(ctx, |manager, _| {
                    manager
                        .participants()
                        .iter()
                        .find(|participant| participant.id == *from)
                        .map(|participant| crate::agent::controller::AgentAuthor {
                            display_name: if participant.display_name.is_empty() {
                                "Guest".to_string()
                            } else {
                                participant.display_name.clone()
                            },
                            avatar_url: participant.avatar_url.clone(),
                        })
                });
                if let Some(target) = mgr.read(ctx, |m, _| m.shared_pane()) {
                    if let Some(tab) = ws.tabs.iter().find(|t| t.pane.pane_uuid(ctx) == target) {
                        if let super::pane::WorkspacePane::Terminal(h) = &tab.pane {
                            let h = h.clone();
                            let author = author.clone();
                            ctx.update_view(&h, |tv, tctx| {
                                tv.apply_control_surface(&surface, author.as_ref(), tctx)
                            });
                        }
                    }
                }
            }
            SessionManagerEvent::RemotePointerMoved { from, position } => {
                let author = mgr.read(ctx, |manager, _| {
                    manager
                        .participants()
                        .iter()
                        .find(|participant| participant.id == *from)
                        .map(|participant| crate::agent::controller::AgentAuthor {
                            display_name: if participant.display_name.is_empty() {
                                "Guest".to_string()
                            } else {
                                participant.display_name.clone()
                            },
                            avatar_url: participant.avatar_url.clone(),
                        })
                });
                if let Some(author) = author {
                    ws.remote_pointers.insert(
                        *from,
                        RemotePointer {
                            display_name: author.display_name.clone(),
                            avatar_url: author.avatar_url.clone(),
                            position: *position,
                        },
                    );
                    ctx.notify();
                }
            }
            // Host: the session has been idle for 10m — prompt to keep or end it.
            SessionManagerEvent::IdleWarning => {
                ws.idle_warning_open = true;
                ctx.notify();
            }
            // Activity resumed (or the host chose keep) — dismiss the prompt.
            SessionManagerEvent::IdleWarningCleared => {
                if ws.idle_warning_open {
                    ws.idle_warning_open = false;
                    ctx.notify();
                }
            }
            // RunCommand / ApprovalRequested are the terminal's concern.
            _ => {}
        });

        view.persistence_writer
            .save_workspace_project(view.workspace_state.project().clone());
        if let Some(work_item) = view.workspace_state.active_work().cloned() {
            view.persistence_writer.save_workspace_work_item(work_item);
        }
        if !view.project_start_visible
            && (restored_document.center_open || view.editor_pane_in_layout())
            && (restored_document.owner_tab_id == Some(view.active_tab_id)
                || view.editor_pane_in_layout())
        {
            if let Some(path) = view
                .document_requested_path
                .clone()
                .or_else(|| restored_document.active_relative_path.clone())
            {
                let root = view
                    .document_owner_root
                    .clone()
                    .unwrap_or(restore_document_root);
                view.restore_saved_document(root, path, ctx);
            }
        }
        if layout_migration_pending || restored_document.center_open {
            // Save only after the restore plan has mounted the editor. Saving
            // before this point records the startup defaults (editor closed,
            // no editor pane) and destroys the previous visual session on the
            // next launch.
            view.save_session(ctx);
        }
        if view.settings_open {
            let settings = view.settings_view.clone();
            ctx.update_view(&settings, |view, settings_ctx| view.on_opened(settings_ctx));
            ctx.focus(&settings);
        }
        // Recovered provider sessions already own their background stream.
        // Reattach Coordinate's lightweight poller so progress and terminal
        // events appear without requiring an unrelated click or layout change.
        view.schedule_workspace_execution_poll(ctx);
        // Canonical execution continues after restart even when no provider
        // session was safe to reattach. This first pass reconciles expired
        // leases before considering retries or additional task launches.
        if view.workspace_state.active_work().is_some_and(|work| {
            matches!(
                work.status,
                WorkStatus::Executing | WorkStatus::ReadyToExecute | WorkStatus::Paused
            )
        }) {
            ctx.spawn(Timer::after(Duration::from_millis(1)), |view, _, ctx| {
                if view.continue_principal_review_policy(ctx) {
                    return;
                }
                if view.continue_principal_escalation(ctx) {
                    return;
                }
                if view.continue_principal_plan_actions(ctx) {
                    return;
                }
                if view.continue_principal_coordination(ctx) {
                    return;
                }
                view.continue_workspace_launch(ctx);
            });
        }
        if let Some(bridge) = crate::automation::claim_workspace_bridge() {
            view.workspace_automation_state = Some(bridge.state);
            ctx.spawn_stream_local(
                bridge.command_rx,
                |view, command, ctx| {
                    use crate::automation::WorkspaceAutomationCommand;
                    match command {
                        WorkspaceAutomationCommand::Shape(text) => {
                            if !view.agentic_workspace_enabled {
                                view.record_principal_error(
                                    "Enable Agentic Workspace before submitting Shape work.".into(),
                                    ctx,
                                );
                                ctx.notify();
                                return;
                            }
                            view.select_workspace_layout(BuiltInLayout::Shape, ctx);
                            view.shape_composer_text = text.clone();
                            let editor = view.shape_composer_editor.clone();
                            ctx.update_view(&editor, |editor, editor_ctx| {
                                editor.set_text(&text, editor_ctx)
                            });
                            view.submit_shape_message(ctx);
                        }
                        WorkspaceAutomationCommand::Approve => {
                            view.approve_plan_and_coordinate(ctx);
                        }
                        WorkspaceAutomationCommand::SelectLayout(layout) => {
                            let layout = match layout.as_str() {
                                "shape" => Some(BuiltInLayout::Shape),
                                "coordinate" => Some(BuiltInLayout::Coordinate),
                                "develop" => Some(BuiltInLayout::Develop),
                                "review" => Some(BuiltInLayout::Review),
                                _ => None,
                            };
                            if let Some(layout) = layout {
                                view.select_workspace_layout(layout, ctx);
                            } else {
                                view.record_principal_error(
                                    "Unknown automated workspace layout.".into(),
                                    ctx,
                                );
                                ctx.notify();
                            }
                        }
                        WorkspaceAutomationCommand::OpenBrowser { target, url } => {
                            let url = url.and_then(|url| {
                                dragent_workspace_model::BrowserUrl::from_address_or_search(&url)
                                    .ok()
                            });
                            let action = match target.as_str() {
                                "side" => WorkspaceAction::SplitActiveTabWithBrowser,
                                "pane" => WorkspaceAction::NewBrowserTab { url: url.clone() },
                                _ => WorkspaceAction::NewBrowserWorkspaceTab,
                            };
                            view.handle_action(&action, ctx);
                            if let (Some(url), false) = (url, target == "pane") {
                                view.navigate_active_browser(url, ctx);
                            }
                        }
                        WorkspaceAutomationCommand::OpenLink(url) => view.open_link(&url, ctx),
                        WorkspaceAutomationCommand::NavigateBrowser(url) => {
                            if let Ok(url) =
                                dragent_workspace_model::BrowserUrl::from_address_or_search(&url)
                            {
                                view.navigate_active_browser(url, ctx);
                            }
                        }
                        WorkspaceAutomationCommand::BrowserAction(action) => {
                            let action = match action.as_str() {
                                "back" => Some(crate::browser::BrowserAction::Back),
                                "forward" => Some(crate::browser::BrowserAction::Forward),
                                "reload" => Some(crate::browser::BrowserAction::ReloadOrStop),
                                "devtools" => Some(crate::browser::BrowserAction::DevTools),
                                "split" => Some(crate::browser::BrowserAction::SplitPane),
                                "close" => Some(crate::browser::BrowserAction::ClosePane),
                                "menu" => Some(crate::browser::BrowserAction::ToggleMenu),
                                "design" => Some(crate::browser::BrowserAction::ToggleDesign),
                                "design-send" => Some(crate::browser::BrowserAction::SendDesign),
                                "design-draw" => Some(crate::browser::BrowserAction::DesignDraw),
                                "design-agents" => {
                                    Some(crate::browser::BrowserAction::ToggleDesignMenu)
                                }
                                other => other.strip_prefix("design-target:").map(|key| {
                                    crate::browser::BrowserAction::SelectDesignTarget(key.into())
                                }),
                            };
                            if let Some(action) = action {
                                view.dispatch_active_browser_action(action, ctx);
                            }
                        }
                        WorkspaceAutomationCommand::OpenProject(root) => {
                            view.handle_action(
                                &WorkspaceAction::ProjectFolderSelected {
                                    root,
                                    intent: super::ProjectFolderIntent::OpenFolder,
                                },
                                ctx,
                            );
                            view.handle_action(&WorkspaceAction::LaunchProjectTerminal, ctx);
                        }
                        WorkspaceAutomationCommand::Commit { message, push } => {
                            view.handle_action(
                                &WorkspaceAction::CommitGitStaged { message, push },
                                ctx,
                            );
                        }
                        WorkspaceAutomationCommand::GitSync { pull } => {
                            let action = if pull {
                                WorkspaceAction::PullActiveGitBranch
                            } else {
                                WorkspaceAction::PushActiveGitBranch
                            };
                            view.handle_action(&action, ctx);
                        }
                        WorkspaceAutomationCommand::GitPanel(reply) => {
                            let panel = view.git_panel_state.borrow();
                            let active = view.tabs.iter().find(|tab| tab.id == view.active_tab_id);
                            let _ = reply.send(serde_json::json!({
                                "active_is_browser": active.is_some_and(|tab| tab.pane.is_browser()),
                                "terminal": panel.terminal_view_id.is_some(),
                                "git_model": panel.git_model.is_some(),
                                "cwd": panel.cwd.as_ref().map(|cwd| cwd.display().to_string()),
                            }));
                        }
                        WorkspaceAutomationCommand::OpenAgent(agent_id) => {
                            view.handle_action(
                                &WorkspaceAction::OpenAgentInNewWorkspaceTab { agent_id },
                                ctx,
                            );
                        }
                        WorkspaceAutomationCommand::StartPrompt { agent, prompt, project } => {
                            if let Some(agent) = crate::automations::executor::prompt_agent(&agent) {
                                if !view.open_project_roots.contains(&project) {
                                    view.open_project_roots.push(project.clone());
                                }
                                view.selected_project_root = Some(project);
                                view.prompt_agent = agent;
                                view.prompt_text = prompt;
                                view.project_start_error = None;
                                view.handle_action(&WorkspaceAction::SubmitPrompt, ctx);
                                if let Some(error) = &view.project_start_error {
                                    log::warn!("[automation] start prompt: {error}");
                                }
                            }
                        }
                        WorkspaceAutomationCommand::SelectTheme(theme) => {
                            let settings = view.settings_view.clone();
                            ctx.update_view(&settings, |settings, settings_ctx| {
                                settings.handle_theme(
                                    &crate::settings::SettingsAction::SelectTheme(theme),
                                    settings_ctx,
                                );
                            });
                        }
                        WorkspaceAutomationCommand::AgentPanes(reply) => {
                            let _ = reply.send(view.automation_agent_panes(ctx));
                        }
                        WorkspaceAutomationCommand::ToggleNewTabMenu => {
                            view.handle_action(&WorkspaceAction::ToggleNewTabPicker, ctx);
                        }
                        WorkspaceAutomationCommand::BrowserTool {
                            pane,
                            tool,
                            arguments,
                            reply,
                        } => {
                            let reply = std::sync::Mutex::new(reply);
                            view.run_browser_tool(
                                pane.as_deref(),
                                None,
                                &tool,
                                &arguments,
                                Box::new(move |result| {
                                    if let Ok(reply) = reply.lock() {
                                        let _ = reply.send(result);
                                    }
                                }),
                                ctx,
                            );
                        }
                        WorkspaceAutomationCommand::Browsers(reply) => {
                            let _ = reply.send(view.browser_automation_snapshot(ctx));
                        }
                        WorkspaceAutomationCommand::DiffComment { action, reply } => {
                            let _ = reply.send(view.diff_comment_automation(&action, ctx));
                        }
                    }
                },
                |_, _| {},
            );
        }
    }
}
