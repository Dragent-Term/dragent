//! Project flow methods for WorkspaceView.
//!
//! Responsible for:
//! - opening work-item tabs and naming work tabs from intent
//! - project folder selection, cloning, SSH destinations
//! - launching project CLIs and work sessions
//! - pristine-work detection and project memory views
//!
//! Not responsible for:
//! - rendering project UI (project_sidebar, view_render)
//! - persistence of projects (workspace_store)
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

use super::project_naming::{bounded_project_memory_text, default_pane_cwd};
use super::view::WorkspaceView;
use super::view::{PaneTab, NEW_WORK_LABEL};
use super::view_tabs::{apply_sidebar_visibility, refresh_sidebar_projects};

impl WorkspaceView {
    /// The worktree already runs under a name made from the request. Ask an
    /// agent for a proper one and adopt it: on the branch while that's safe, and
    /// as the title unless the person has renamed it by then.
    pub(crate) fn name_workspace_with_agent(
        &mut self,
        root: PathBuf,
        branch: String,
        shown_title: Option<String>,
        prompt: String,
        agent: super::action::PromptAgent,
        team: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let worktree = root.clone();
        ctx.spawn(
            async move {
                let names =
                    super::project_workspace_creation::agent_names(&prompt, agent).await?;
                let title = names.title.clone();
                let desired = crate::settings::git_consumers::final_branch_name(&names.branch);
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(reason) = super::project_workspace_creation::rename_worktree_branch(
                        &worktree, &branch, &desired,
                    ) {
                        log::info!("[workspace] keeping branch name: {reason}");
                    }
                    if let Some(team) = team {
                        let renamed = dragent_host::HostClient::ensure_host().and_then(|client| {
                            client.team(dragent_host::team::TeamRequest::SetTitle {
                                team,
                                title: names.title,
                            })
                        });
                        if let Err(error) = renamed {
                            log::info!("[workspace] keeping the team's title: {error}");
                        }
                    }
                })
                .await;
                Some(title)
            },
            move |view, title, ctx| {
                let Some(title) = title else {
                    return;
                };
                let root = root.canonicalize().unwrap_or(root);
                // A linked worktree's HEAD lives in the main repository's `.git`, outside
                // what a pane's file watcher sees, so a rename has to be announced.
                view.on_branch_changed(&root, None, None, ctx);
                let Some(workspace) = view
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.root == root)
                else {
                    return;
                };
                let renamed_by_person = shown_title
                    .as_deref()
                    .is_some_and(|shown| workspace.title != shown);
                if renamed_by_person {
                    return;
                }
                workspace.title = title;
                let record = workspace.clone();
                view.persistence_writer
                    .send(dragent_persistence::ModelEvent::RecordLocalWorkspace(record));
                view.sync_window_title(ctx);
                view.save_session(ctx);
                ctx.notify();
            },
        );
    }

    pub(crate) fn finish_project_prompt_workspace(
        &mut self,
        root: PathBuf,
        branch: String,
        prompt: String,
        workspace_name: Option<String>,
        // The team the agent starts leading.
        lead_of: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let agent = self.prompt_agent;
        let lead_model = self.prompt_lead_model.clone().or_else(||
            (lead_of.is_some() && agent == super::action::PromptAgent::Dragent)
                .then(|| crate::settings::AppSettings::load().selected_model));
        let machine = self.machine_agents(Some(&root), ctx);
        let remote_team_lead = lead_of.is_some() && machine.is_some();
        // A shell on another machine can't read a prompt file written here.
        let local = machine.is_none();
        let launch_command = if agent == super::action::PromptAgent::Dragent {
            None
        } else if let Some(machine) = &machine {
            // On a machine: the user's launch settings, running the copy
            // chosen there.
            if let Some(problem) = self.machine_agent_problem(machine, agent.cli().unwrap_or_default(), agent.label(), ctx) {
                self.project_start_error = Some(problem);
                ctx.notify();
                return;
            }
            Some(super::action_projects::agent_command_config(agent, &crate::setup::UiSettings::load()))
        } else {
            let Some(command) = super::action_projects::prompt_agent_interactive_command(agent)
            else {
                self.prompt_installed_agents.remove(&agent);
                self.project_start_error = Some(format!(
                    "{} is not installed. Select it again to open the installer.",
                    agent.label()
                ));
                ctx.notify();
                return;
            };
            Some(command)
        };
        let mut launch_command = launch_command;
        if let (Some(config), Some(model)) = (&mut launch_command, &lead_model) {
            let result = crate::cli::model::apply(&config.command, agent, model)
                .and_then(|command| crate::cli::model::apply(&config.prompt_command, agent, model)
                    .map(|prompt_command| (command, prompt_command)));
            match result {
                Ok((command, prompt_command)) => { config.command = command; config.prompt_command = prompt_command; }
                Err(error) => { self.project_start_error = Some(error); ctx.notify(); return; }
            }
        }
        if let (Some(machine), Some(config)) = (&machine, &mut launch_command) {
            config.command = machine.launch_line(&config.command);
            config.prompt_command = machine.launch_line(&config.prompt_command);
        }
        // A new worktree starts an independent tab/layout collection. Park
        // the previous surface before adding the new workspace's first pane.
        if self.begin_fresh_surface(ctx).is_none() {
            return;
        }
        let workspace_name = workspace_name.unwrap_or_else(|| branch.clone());
        self.assign_surface_to_workspace(root.clone(), workspace_name.clone());
        #[cfg(not(target_family="wasm"))]
        let attachments = if self.review_setup.is_some() { Vec::new() } else { std::mem::take(&mut self.prompt_attachments) };
        #[cfg(target_family="wasm")]
        let attachments = std::mem::take(&mut self.prompt_attachments);
        let file_paths = attachments
            .iter()
            .filter_map(|file| file.any_path())
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        if let Some(session) = self.prompt_voice_session.take() {
            session.stop();
        }
        self.prompt_voice_ui = crate::voice::VoiceUiState::Idle;
        self.prompt_voice_gen = self.prompt_voice_gen.wrapping_add(1);

        let tab = if agent == super::action::PromptAgent::Dragent {
            self.create_agent_tab_at(ctx, Some(root.clone()))
        } else {
            self.create_tab_with_cwd(
                ctx,
                Some(branch.clone()),
                None,
                root.clone(),
                Some(root.clone()),
            )
        };
        let tab_id = tab.id;
        let terminal = match &tab.pane {
            WorkspacePane::Terminal(terminal) => Some(terminal.clone()),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        };
        // Register the lead and its model before delivering its first prompt.
        let team_lead = lead_of.map(|team| (team, tab.pane.pane_uuid(ctx).to_string()));
        if team_lead.is_some() { self.prompt_team = false; }
        let mut tab = tab;
        tab.custom_label = Some(workspace_name);
        // Bind the session to the launched agent's registry identity so
        // tabs, rail, and input editor resolve it without re-detection —
        // and so restores keep it after restart.
        tab.pane
            .set_agent_id(ctx, Some(agent.command_key().to_string()));
        self.tabs.push(tab);
        self.active_tab_id = tab_id;
        self.split_tree = None;
        self.document_center_open = false;
        self.document_terminal_visible = false;
        self.project_start_visible = false;
        self.project_start_error = None;
        self.right_panel_visible = true;
        self.right_panel_user_closed = false;
        self.switch_workspace_work_for_tab(tab_id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
        self.update_active_pane(ctx);
        if let Some(terminal) = terminal {
            let selected_model = lead_model.clone();
            let submit = move |terminal: &mut TerminalView, terminal_ctx: &mut ViewContext<TerminalView>| {
                match agent {
                    super::action::PromptAgent::Dragent => {
                        terminal.agent_model_override = selected_model;
                        terminal.pending_attachments = attachments;
                        terminal.submit_agent_query(prompt, terminal_ctx);
                    }
                    _ => {
                        if let Some(config) = launch_command.as_ref() {
                            let message = if file_paths.is_empty() { prompt } else {
                                format!("{prompt}\n\nAttached files:\n{}", file_paths.join("\n"))
                            };
                            let shell = terminal.launch_shell.as_deref().unwrap_or("/bin/sh");
                            let command = if message.trim().is_empty() { Ok(config.command.trim().to_string()) }
                                else { config.build_prompt_command(&message, shell, local) };
                            match command {
                                Ok(command) => {
                                    let command = if remote_team_lead {
                                        dragent_host::agent_sessions::wrap_remote_provider_command(terminal.pane_uuid, shell, &command, agent.command_key())
                                    } else { command };
                                    terminal.submit_shell_command(&command, terminal_ctx)
                                },
                                Err(error) => log::error!("Cannot launch {}: {error}", agent.label()),
                            }
                        }
                    }
                }
            };
            if let Some((team, pane)) = team_lead {
                ctx.spawn(
                    async move { crate::cli::team::set_lead(&team, &pane, agent.command_key(), lead_model.as_deref()) },
                    move |view, registered, ctx| match registered {
                        Ok(()) => ctx.update_view(&terminal, submit),
                        Err(error) => view.show_error_notice("Couldn't start coordination lead", error, ctx),
                    },
                );
            } else { ctx.update_view(&terminal, submit); }
        }
        // A review has its own editor; leave an unrelated project draft intact.
        #[cfg(not(target_family = "wasm"))]
        let keep_project_draft = self.review_setup.is_some();
        #[cfg(target_family = "wasm")]
        let keep_project_draft = false;
        if !keep_project_draft {
        // The new workspace is visible and the context has been handed to its
        // agent. Clear the project composer now, so returning to it starts fresh.
        self.prompt_text.clear();
        self.prompt_links.clear();
        self.prompt_notice = None;
        self.prompt_menu = None;
        self.prompt_link_query.clear();
        self.prompt_link_results.clear();
        self.prompt_link_error = None;
        self.prompt_link_generation = self.prompt_link_generation.wrapping_add(1);
        self.prompt_attachment_scroll = ClippedScrollStateHandle::new();
        self.prompt_selected_worktree = None;
        self.prompt_branch = self
            .selected_project_root()
            .and_then(|root| super::project_naming::local_branch_name(&root));
        let editor = self.prompt_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_text("", editor_ctx)
        });
        }
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        self.persistence_writer.flush();
        ctx.notify();
    }

    pub(crate) fn open_work_item_tab(&mut self, view: WorkItemView, ctx: &mut ViewContext<Self>) {
        self.workspace_active_agent_tab = None;
        let work_item_id = self.workspace_work_item_id;
        self.ensure_work_item_agent(work_item_id, ctx);
        if self.active_builtin_layout() != BuiltInLayout::Develop {
            self.select_workspace_layout(BuiltInLayout::Develop, ctx);
        }
        self.work_item_tab_open = true;
        self.work_item_tab_active = true;
        self.work_item_view = view;
        // Work is a first-class workspace pane, like a terminal session. It
        // must not mount through the editor's reserved pane or automatically
        // split the current terminal/editor surface.
        self.project_start_visible = false;
        self.split_tree = None;
        self.editor_pane_focused = false;
        self.document_center_open = false;
        self.document_restore_pending = false;
        self.document_tab_id = Some(self.active_tab_id);
        self.document_owner_root = self
            .active_project_root()
            .or_else(|| Some(self.document_index_root.clone()));
        self.layout_menu_open = false;
        self.prepare_active_tab_for_work();
        self.save_session(ctx);
        self.sync_left_panel(ctx);
        self.sync_window_title(ctx);
        ctx.notify();
    }

    pub(crate) fn selected_project_root(&self) -> Option<PathBuf> {
        self.selected_project_root
            .as_ref()
            .filter(|root| self.open_project_roots.contains(root))
            .cloned()
            .or_else(|| self.open_project_roots.last().cloned())
    }

    /// Whether the project prompt can start work in a new worktree. When it
    /// can't, the prompt starts in the project's own folder whatever is
    /// selected.
    pub(crate) fn prompt_worktree_available(&self) -> bool {
        self.selected_project_root()
            .is_some_and(|root| super::project_naming::worktree_available(&root))
    }

    pub(crate) fn active_project_root(&self) -> Option<PathBuf> {
        if self.project_start_visible {
            return self.selected_project_root().filter(|root| root.is_dir());
        }
        if self.settings_open {
            return self
                .settings_project_root
                .clone()
                .or_else(|| self.selected_project_root())
                .filter(|root| root.is_dir());
        }
        // A standalone terminal has no project association. Recently opened
        // folders must not turn it into a project workspace.
        if !self.document_center_open {
            if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
                if !tab.pane.is_settings() {
                    return tab.project_root.clone().filter(|root| root.is_dir());
                }
            }
        }
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .and_then(|tab| tab.project_root.clone())
            .or_else(|| {
                self.document_center_open
                    .then(|| self.document_owner_root.clone())
                    .flatten()
            })
            .or_else(|| self.selected_project_root())
            .filter(|root| root.is_dir())
    }

    pub(crate) fn active_review_git_root(&self) -> Option<PathBuf> {
        self.active_project_root()
            .and_then(|root| find_git_root(&root))
    }

    /// An Open Folder tab begins as a project container. Once its work surface
    /// is opened, replace only the generated folder label; explicit user names
    /// remain untouched.
    pub(crate) fn prepare_active_tab_for_work(&mut self) {
        if self.workspace_state.active_work().is_some() {
            return;
        }
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == self.active_tab_id)
        else {
            return;
        };
        let generated_folder_label = tab
            .project_root
            .as_ref()
            .and_then(|root| root.file_name())
            .and_then(|name| name.to_str());
        let should_replace =
            should_replace_project_tab_label(tab.custom_label.as_deref(), generated_folder_label);
        if should_replace {
            tab.custom_label = Some(NEW_WORK_LABEL.to_string());
        }
    }

    /// Build a small, deterministic memory view shared by work items under the
    /// same project root. Only structured summaries and resolved decisions are
    /// shared; conversations and runtime/tool data remain work-item private.
    pub(crate) fn shared_project_memory_context(&self, current_work_item_id: WorkItemId) -> String {
        let entries = self.project_memory_views(current_work_item_id);
        if entries.is_empty() {
            String::new()
        } else {
            serde_json::to_string_pretty(&entries).unwrap_or_default()
        }
    }

    pub(crate) fn project_memory_views(
        &self,
        current_work_item_id: WorkItemId,
    ) -> Vec<super::work_item_surface::ProjectMemoryView> {
        let project_id = self.workspace_state.project().id;
        let mut entries = self
            .parked_workspace_work
            .values()
            .filter(|parked| parked.state.project().id == project_id)
            .filter_map(|parked| {
                let work = parked.state.active_work()?;
                if work.id == current_work_item_id {
                    return None;
                }
                let brief = parked.state.latest_work_brief();
                let resolved_decisions = parked
                    .state
                    .shaping_records_projection()
                    .map(|records| {
                        records
                            .decisions
                            .into_iter()
                            .filter(|decision| {
                                decision.status == dragent_workspace_model::DecisionStatus::Resolved
                            })
                            .take(4)
                            .filter_map(|decision| {
                                decision.resolution.map(|resolution| {
                                    (
                                        bounded_project_memory_text(&decision.question, 200),
                                        bounded_project_memory_text(&resolution, 300),
                                        decision.rationale.map(|rationale| {
                                            bounded_project_memory_text(&rationale, 300)
                                        }),
                                    )
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if brief.is_none() && resolved_decisions.is_empty() {
                    return None;
                }
                Some((
                    work.updated_at_unix_ms,
                    super::work_item_surface::ProjectMemoryView {
                        work_item_id: work.id,
                        title: work.title.clone(),
                        status: work.status,
                        outcome: brief
                            .map(|brief| bounded_project_memory_text(&brief.outcome, 600)),
                        summary: brief
                            .map(|brief| bounded_project_memory_text(&brief.summary, 600)),
                        resolved_decisions,
                    },
                ))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|(updated_at, _)| std::cmp::Reverse(*updated_at));
        entries.truncate(4);
        entries.into_iter().map(|(_, entry)| entry).collect()
    }

    pub(crate) fn work_item_display_title(&self) -> String {
        if let Some(label) = self
            .tabs
            .iter()
            .find(|tab| tab.work_item_id == Some(self.workspace_work_item_id))
            .and_then(|tab| tab.custom_label.as_deref())
            .filter(|label| !is_default_work_label(label))
        {
            return label.to_string();
        }
        if let Some(work) = self.workspace_state.active_work() {
            if !is_default_work_label(&work.title) {
                return work.title.clone();
            }
        }
        NEW_WORK_LABEL.to_string()
    }

    /// Give a draft work tab a useful name once the conversation contains a
    /// concrete request. Explicit user labels are never overwritten.
    pub(crate) fn name_work_tab_from_intent(
        &mut self,
        work_item_id: WorkItemId,
        title: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if is_default_work_label(title) {
            return;
        }
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.work_item_id == Some(work_item_id))
        else {
            return;
        };
        if tab
            .custom_label
            .as_deref()
            .is_some_and(|label| !is_default_work_label(label))
        {
            return;
        }
        tab.custom_label = Some(title.to_string());
        self.sync_left_panel(ctx);
        self.sync_window_title(ctx);
        self.save_session(ctx);
    }

    pub(crate) fn choose_project_folder(
        &mut self,
        intent: ProjectFolderIntent,
        ctx: &mut ViewContext<Self>,
    ) {
        self.new_tab_picker_open = false;
        ctx.open_file_picker(
            move |result, ctx| match result {
                Ok(paths) => {
                    if let Some(path) = paths.into_iter().next() {
                        ctx.dispatch_typed_action_deferred(
                            WorkspaceAction::ProjectFolderSelected {
                                root: PathBuf::from(path),
                                intent,
                            },
                        );
                    }
                }
                Err(error) => log::warn!("[workspace-folder] picker failed: {error}"),
            },
            FilePickerConfiguration::new().folders_only(),
        );
        ctx.notify();
    }

    pub(crate) fn open_explicit_terminal(
        &mut self,
        label: Option<String>,
        command: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.open_page_command_terminal(
            label.as_deref().unwrap_or("Terminal"),
            None,
            command.as_deref().unwrap_or(""),
            ctx,
        );
    }

    /// Start an external coding agent through the existing terminal runtime.
    /// This deliberately does not introduce a host/daemon abstraction: the
    /// terminal's normal AI-tool detection takes over after the command starts.
    pub(crate) fn launch_project_cli(
        &mut self,
        label: &'static str,
        binary: Option<&'static str>,
        ctx: &mut ViewContext<Self>,
    ) {
        let machine = self.machine_agents(self.selected_project_root().as_deref(), ctx);
        if let (Some(machine), Some(binary)) = (&machine, binary) {
            if let Some(problem) = self.machine_agent_problem(machine, binary, label, ctx) {
                self.project_start_error = Some(problem);
                ctx.notify();
                return;
            }
        }
        let command = binary.and_then(|binary| {
            let agent = super::action::PromptAgent::ALL
                .into_iter()
                .find(|agent| agent.cli() == Some(binary))?;
            let settings = crate::setup::UiSettings::load();
            if let Some(machine) = &machine {
                let config = super::action_projects::agent_command_config(agent, &settings);
                config.validate().ok()?;
                return Some(machine.launch_line(config.command.trim()));
            }
            // Probing starts a login shell and npm on this UI thread; an agent
            // the background detection pass already found skips it.
            if !settings
                .agent_command_configs
                .contains_key(agent.command_key())
                && !settings.agent_commands.contains_key(agent.command_key())
                && !self.prompt_installed_agents.contains(&agent)
            {
                super::action_projects::prompt_agent_launch_command(agent)?;
            }
            let config = super::action_projects::agent_command_config(agent, &settings);
            config.validate().ok()?;
            Some(config.command.trim().to_string())
        });
        if binary.is_some() && command.is_none() {
            self.project_start_error = Some(format!(
                "{label} is not installed. Select it from the agent menu to install it."
            ));
            ctx.notify();
            return;
        }
        let project_root = self.selected_project_root().filter(|root| root.is_dir());
        let cwd = project_root.clone().unwrap_or_else(default_pane_cwd);
        let new_tab =
            self.create_tab_with_cwd(ctx, Some(label.to_string()), None, cwd, project_root);
        let new_tab_id = new_tab.id;
        // Bind launched-agent panes to their registry identity; plain
        // terminal launches keep `None` (detection fallback).
        if let Some(binary) = binary {
            if let Some(agent) = super::action::PromptAgent::ALL
                .into_iter()
                .find(|agent| agent.cli() == Some(binary))
            {
                new_tab
                    .pane
                    .set_agent_id(ctx, Some(agent.command_key().to_string()));
            }
        }
        let terminal = match &new_tab.pane {
            WorkspacePane::Terminal(terminal) => Some(terminal.clone()),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        };
        self.tabs.push(new_tab);
        self.active_tab_id = new_tab_id;
        self.split_tree = None;
        self.project_start_visible = false;
        self.switch_workspace_work_for_tab(new_tab_id, ctx);
        self.set_active_terminal_sticky_headers(true, ctx);
        self.update_active_pane(ctx);
        if let (Some(terminal), Some(command)) = (terminal, command.as_deref()) {
            ctx.update_view(&terminal, |terminal, terminal_ctx| {
                terminal.submit_shell_command(command, terminal_ctx);
            });
        }
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    /// Open the current Work Item UI using the existing in-process workspace
    /// model. A backing project tab is created only because today's Work Item
    /// surface is owned by a workspace tab; it does not start a command.
    pub(crate) fn launch_project_work(&mut self, ctx: &mut ViewContext<Self>) {
        let project_root = self.selected_project_root().filter(|root| root.is_dir());
        let active_is_work_pane = self.work_item_tab_open
            && self
                .tabs
                .iter()
                .find(|tab| tab.id == self.active_tab_id)
                .is_some_and(|tab| {
                    tab.project_root == project_root
                        && tab.work_item_id == Some(self.workspace_work_item_id)
                });
        if !active_is_work_pane {
            let cwd = project_root.clone().unwrap_or_else(default_pane_cwd);
            let new_tab = self.create_tab_with_cwd(
                ctx,
                Some(NEW_WORK_LABEL.to_string()),
                None,
                cwd,
                project_root,
            );
            self.active_tab_id = new_tab.id;
            self.tabs.push(new_tab);
            self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        }
        self.project_start_visible = false;
        self.open_work_item_tab(WorkItemView::Conversation, ctx);
        ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
            editor.request_focus(editor_ctx);
        });
    }

    /// All clone entry points open the same editable URL and destination form.
    pub(crate) fn begin_clone_from_url(
        &mut self,
        url: String,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        if self.clone_busy {
            return Ok(());
        }
        self.prompt_menu = None;
        self.new_tab_picker_open = false;
        self.project_context_menu_root = None;
        self.create_project_open = false;
        self.project_start_mode = None;
        self.add_project_menu = super::view::AddProjectMenu::CloneForm;
        self.clone_url_error = None;
        let location = default_projects_dir().to_string_lossy().into_owned();
        self.clone_url_text = url.clone();
        self.clone_location_text = location.clone();
        ctx.update_view(&self.clone_location_editor, |editor, editor_ctx| {
            editor.set_text(&location, editor_ctx);
        });
        ctx.update_view(&self.clone_url_editor, |editor, editor_ctx| {
            editor.set_text(&url, editor_ctx);
            editor_ctx.focus_self();
        });
        ctx.notify();
        Ok(())
    }

    pub(crate) fn refresh_prompt_branches(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(root)=self.selected_project_root().filter(|root|root.is_dir()) else{return;};
        let requested=root.clone();
        ctx.spawn(async move {
            crate::terminal::view::terminal_helpers::list_local_branches(&requested)
        },move |view,branches,ctx| {
            if view.selected_project_root().as_ref()!=Some(&root){return;}
            // Refresh choices only. Opening the menu is not a selection.
            view.prompt_branches=branches;
            ctx.notify();
        });
    }

    pub(crate) fn refresh_prompt_links(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(super::action::PromptMenu::Links(kind)) = self.prompt_menu else {
            return;
        };
        self.prompt_link_generation = self.prompt_link_generation.wrapping_add(1);
        let generation = self.prompt_link_generation;
        self.prompt_link_results.clear();
        self.prompt_link_error = None;
        if kind != super::project_links::LinkKind::PullRequest
            && !crate::integrations::is_connected(kind.provider())
        {
            self.prompt_link_loading = false;
            ctx.notify();
            return;
        }
        let Some(root) = self.selected_project_root() else {
            self.prompt_link_loading = false;
            self.prompt_link_error = Some("Choose a project first.".into());
            ctx.notify();
            return;
        };
        let query = self.prompt_link_query.clone();
        self.prompt_link_loading = true;
        ctx.spawn(
            async move {
                Timer::after(Duration::from_millis(220)).await;
                (root, query)
            },
            move |view, (root, query), ctx| {
                if view.prompt_link_generation != generation {
                    return;
                }
                ctx.spawn(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            super::project_links::search(kind, &root, &query)
                        })
                        .await
                        .map_err(|error| format!("Search could not finish: {error}"))?
                    },
                    move |view,
                          result: Result<Vec<super::project_links::PromptLink>, String>,
                          ctx| {
                        if view.prompt_link_generation != generation {
                            return;
                        }
                        view.prompt_link_loading = false;
                        match result {
                            Ok(links) => view.prompt_link_results = links,
                            Err(error) => view.prompt_link_error = Some(error),
                        }
                        ctx.notify();
                    },
                );
            },
        );
        ctx.notify();
    }

    pub(crate) fn refresh_prompt_worktrees(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(root) = self.selected_project_root().filter(|root| root.is_dir()) else {
            return;
        };
        let requested=root.clone();
        ctx.spawn(
            async move {
                let output = crate::git_cmd::GitCommand::new()
                    .args(["worktree", "list", "--porcelain"])
                    .current_dir(&requested)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .no_window()
                    .output()
                    .ok()
                    .filter(|output| output.status.success())?;
                Some(
                    super::project_naming::parse_worktree_list(&String::from_utf8_lossy(
                        &output.stdout,
                    ))
                    .into_iter()
                    .map(|(path, branch)| super::action::PromptWorktree { path, branch })
                    .collect::<Vec<_>>()
                )
            },
            move |view, result, ctx| {
                if view.selected_project_root().as_ref()!=Some(&root){return;}
                if let Some(worktrees) = result {
                    view.prompt_worktrees = worktrees;
                    ctx.notify();
                }
            },
        );
    }

    /// Open an interactive remote shell for an already-parsed SSH destination.
    /// The command is generated from typed fields only — raw user input never
    /// reaches the shell — and the destination is recorded in recents.
    /// When the target skips workspace routing (Hosts settings), fall back to
    /// a plain SSH terminal instead of the workspace-runtime path.
    pub(crate) fn connect_ssh_destination(
        &mut self,
        destination: super::ssh::SshDestination,
        ctx: &mut ViewContext<Self>,
    ) {
        let target = destination.display();
        if !crate::settings::settings_tasks_hosts::routing_enabled_for(&target)
            || !crate::settings::settings_tasks_hosts::probe_allowed_for(&target)
        {
            self.open_new_workspace_tab(
                Some(&format!("SSH: {target}")),
                Some(&destination.shell_command()),
                ctx,
            );
            return;
        }
        self.add_remote_project(Some(destination), ctx);
    }

    pub(crate) fn add_project_mouse(
        states: &RefCell<HashMap<String, MouseStateHandle>>,
        key: &str,
    ) -> MouseStateHandle {
        states
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(MouseStateHandle::default)
            .clone()
    }

    pub(crate) fn primary_button(
        theme: &Theme,
        ui_font: FamilyId,
        workspace_view_id: EntityId,
        mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
        key: &str,
        label: &'static str,
        action: WorkspaceAction,
    ) -> Box<dyn Element> {
        let theme_owned = *theme;
        Hoverable::new(Self::add_project_mouse(mouse_states, key), move |state| {
            Container::new(
                Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                    .with_color(if state.is_hovered() {
                        theme_owned.background
                    } else {
                        theme_owned.main_text()
                    })
                    .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme_owned.accent
            } else {
                theme_owned.surface_3()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
        })
        .finish()
    }

    pub(crate) fn receive_project_folder(
        &mut self,
        root: PathBuf,
        intent: ProjectFolderIntent,
        ctx: &mut ViewContext<Self>,
    ) {
        // A remote project folder: the machine validated it when the project
        // was added (and offered to initialise Git), and adding it is the
        // user's trust decision. Open it like any other folder.
        if crate::remote_fs::is_remote(&root) {
            crate::remote_fs::ensure_stand_in(&root);
            self.workspace_trust.trust(&root);
            self.continue_project_folder_intent(root, intent, ctx);
            self.refresh_review_ship_state(ctx);
            return;
        }
        let root = root.canonicalize().unwrap_or(root);
        if !root.is_dir() {
            log::warn!(
                "[workspace-folder] selected path is not a directory: {}",
                root.display()
            );
            return;
        }
        // A project must be a git checkout for branches, worktrees, and
        // source control. Offer to initialize instead of refusing outright.
        // Standalone terminals stay git-free; only folder opens gate here.
        if intent == ProjectFolderIntent::OpenFolder
            && !crate::console::dir_context::is_git_repo(&root)
        {
            self.pending_git_init_root = Some(root);
            ctx.notify();
            return;
        }
        if !self.workspace_trust.is_trusted(&root)
            && super::action_projects::workspace_trust_enforced()
        {
            self.pending_workspace_trust_document = None;
            self.pending_project_folder_intent = Some(intent);
            self.pending_workspace_trust = Some(root);
            ctx.notify();
            return;
        }
        self.continue_project_folder_intent(root, intent, ctx);
        self.refresh_review_ship_state(ctx);
    }

    /// Find a tab on `root` whose work is pristine: no work item yet, or a
    /// Draft work item with no work-scoped activity beyond its creation.
    pub(crate) fn pristine_work_tab_for_root(&self, root: &Path) -> Option<usize> {
        let pristine_tab_id = |tab: &PaneTab| {
            if tab.project_root.as_deref() != Some(root) {
                return None;
            }
            let work_item_id = tab.work_item_id?;
            let pristine = if work_item_id == self.workspace_work_item_id {
                Self::is_pristine_work_state(&self.workspace_state)
            } else {
                self.parked_workspace_work
                    .get(&work_item_id)
                    .is_some_and(|parked| Self::is_pristine_work_state(&parked.state))
            };
            pristine.then_some(tab.id)
        };

        // Prefer the work already in front of the user. If an older build left
        // more than one empty draft behind, resume the newest remaining tab.
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .and_then(pristine_tab_id)
            .or_else(|| self.tabs.iter().rev().find_map(pristine_tab_id))
    }

    /// Project-level terminal/UI events do not start a work item. A Draft is
    /// reusable until its first work-scoped event after creation; that includes
    /// the first user/Principal message, shaping, planning, file, or execution
    /// activity.
    pub(crate) fn is_pristine_work_state(state: &WorkspaceState) -> bool {
        let Some(work) = state.active_work() else {
            return true;
        };
        work.status == WorkStatus::Draft
            && !state.events().iter().any(|event| {
                event.work_item_id == Some(work.id)
                    && !matches!(&event.kind, WorkspaceEventKind::WorkCreated { .. })
            })
    }

    pub(crate) fn continue_project_folder_intent(
        &mut self,
        root: PathBuf,
        intent: ProjectFolderIntent,
        ctx: &mut ViewContext<Self>,
    ) {
        self.selected_project_root = Some(root.clone());
        if !self.open_project_roots.contains(&root) {
            self.open_project_roots.push(root.clone());
        }
        // New Work on a folder that already holds one untouched Draft must
        // resume it instead of stacking another blank tab. Work that started
        // (non-Draft status or any Principal conversation) falls through and
        // creates a new tab as before.
        if intent == ProjectFolderIntent::NewWork {
            if let Some(tab_id) = self.pristine_work_tab_for_root(&root) {
                self.new_tab_picker_open = false;
                self.exit_split_unless_contains(tab_id);
                self.switch_workspace_work_for_tab(tab_id, ctx);
                self.active_tab_id = tab_id;
                self.restore_document_owner_tab(tab_id, ctx);
                self.open_work_item_tab(WorkItemView::Conversation, ctx);
                self.set_active_terminal_sticky_headers(true, ctx);
                self.update_active_pane(ctx);
                self.refresh_all_tab_titles(ctx);
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                ctx.update_view(&self.shape_composer_editor, |editor, editor_ctx| {
                    editor.request_focus(editor_ctx);
                });
                ctx.notify();
                return;
            }
        }
        let label = match intent {
            ProjectFolderIntent::OpenFolder => "Develop".to_string(),
            ProjectFolderIntent::NewWork => NEW_WORK_LABEL.to_string(),
        };
        // Opening a folder enters the selected-project launcher. It does not
        // create a terminal, Work Item, or editor surface. The file explorer
        // remains hidden until Browse files (or a real surface) is chosen.
        // Existing tabs keep their own cwd untouched; future terminals inherit
        // this folder via `create_tab`.
        if intent == ProjectFolderIntent::OpenFolder {
            self.project_start_visible = true;
            self.project_start_mode = None;
            self.project_start_error = None;
            // Do NOT pre-set `document_index_root` here: `refresh_sidebar_projects`
            // owns the root transition (clear + rescan). Pre-setting it makes
            // the `root != document_index_root` check below false, so the new
            // folder's tree is never discovered and the explorer keeps showing
            // the stale root.
            self.workspace_trust.touch_recent(&root);
            self.project_start_recent_projects = self.workspace_trust.recent_roots();
            self.left_panel_visible = true;
            // The project prompt masks the file sidebar while preserving its
            // prior visibility for a workspace the user selects afterward.
            // The checkout is the worktree: record its branch for the prompt
            // screen's worktree/branch chips and tab titles.
            self.prompt_branch = super::project_naming::local_branch_name(&root);
            // A project starts in its own folder; a worktree is the user's
            // choice from there.
            self.prompt_selected_worktree = Some(root.clone());
            // Opening a folder replaces the center surface (matching the
            // previous terminal-landing behavior): the explorer below must
            // switch roots instead of early-returning on editor ownership.
            self.document_center_open = false;
            // No active tab yet (or tabs belong to other roots), so
            // `update_active_pane` would early-return — refresh the
            // explorer explicitly for the new root.
            refresh_sidebar_projects(self, ctx);
            apply_sidebar_visibility(self, ctx);
            self.refresh_all_tab_titles(ctx);
            self.sync_left_panel(ctx);
            // The sidebar view caches its search/git context root from tab
            // switches; with no tab switch happening, point it at the new
            // folder explicitly so Files and Source Control populate.
            let sidebar_view = self.project_sidebar_view.clone();
            let context_root = Some(root.clone());
            ctx.update_view(&sidebar_view, |sidebar, sidebar_ctx| {
                sidebar.refresh_workspace_context(context_root, sidebar_ctx);
            });
            self.refresh_all_tab_titles(ctx);
            self.sync_left_panel(ctx);
            self.workspace_active_agent_tab = None;
            self.work_item_tab_open = false;
            self.work_item_tab_active = false;
            ctx.update_view(&self.prompt_editor, |editor, editor_ctx| {
                editor.request_focus(editor_ctx);
            });
            self.save_session(ctx);
            self.persistence_writer.flush();
            self.maybe_resume_pending_start(ctx);
            ctx.notify();
            return;
        }
        let new_tab =
            self.create_tab_with_cwd(ctx, Some(label), None, root.clone(), Some(root.clone()));
        self.active_tab_id = new_tab.id;
        self.split_tree = None;
        self.tabs.push(new_tab);
        self.project_start_visible = false;
        self.project_start_mode = None;
        self.project_start_error = None;
        self.workspace_trust.touch_recent(&root);
        self.project_start_recent_projects = self.workspace_trust.recent_roots();
        self.switch_workspace_work_for_tab(self.active_tab_id, ctx);
        self.left_panel_visible = true;
        self.right_panel_visible = true;
        self.right_panel_user_closed = false;
        self.set_active_terminal_sticky_headers(true, ctx);
        self.update_active_pane(ctx);
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        // New Work is an explicit request for its own workspace pane. The
        // terminal remains the current runtime owner, but the pane presents
        // the Work Item surface instead of appearing as an editor tab.
        self.workspace_active_agent_tab = None;
        self.document_center_open = false;
        self.document_terminal_visible = false;
        self.document_terminal_maximized = false;
        self.set_document_terminal_backdrops(false, ctx);
        self.sync_pane_activity(ctx);
        self.open_work_item_tab(WorkItemView::Conversation, ctx);
        self.save_session(ctx);
        self.maybe_resume_pending_start(ctx);
        ctx.notify();
    }
}

impl WorkspaceView {
    pub(crate) fn select_workspace_layout(
        &mut self,
        layout: BuiltInLayout,
        ctx: &mut ViewContext<Self>,
    ) {
        if layout == BuiltInLayout::Shape && self.agentic_workspace_enabled {
            self.open_work_item_tab(WorkItemView::Conversation, ctx);
            return;
        }
        if !matches!(
            layout,
            BuiltInLayout::Shape
                | BuiltInLayout::Coordinate
                | BuiltInLayout::Develop
                | BuiltInLayout::Review
        ) {
            return;
        }
        if layout != BuiltInLayout::Develop && !self.agentic_workspace_enabled {
            self.layout_menu_open = false;
            log::info!(
                "[agentic-workspace] ignored {:?} layout selection while experimental flag is disabled",
                layout
            );
            ctx.notify();
            return;
        }

        // Snapshot Develop before changing the active presentation. This only
        // records placement metadata; every registered view/process remains
        // alive and is rendered again when Develop is selected.
        self.layout_collection = self.capture_layout_collection(ctx);
        let result = match layout {
            BuiltInLayout::Shape => self
                .layout_collection
                .instance_for(
                    LayoutScope::WorkItem {
                        project_id: self.workspace_state.project().id,
                        work_item_id: self.workspace_work_item_id,
                    },
                    BuiltInLayout::Shape,
                )
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.shape_layout_instance()),
            BuiltInLayout::Coordinate => self
                .layout_collection
                .instance_for(
                    LayoutScope::WorkItem {
                        project_id: self.workspace_state.project().id,
                        work_item_id: self.workspace_work_item_id,
                    },
                    BuiltInLayout::Coordinate,
                )
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.coordinate_layout_instance()),
            BuiltInLayout::Review => self
                .layout_collection
                .instance_for(
                    LayoutScope::WorkItem {
                        project_id: self.workspace_state.project().id,
                        work_item_id: self.workspace_work_item_id,
                    },
                    BuiltInLayout::Review,
                )
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.review_layout_instance()),
            BuiltInLayout::Develop => self
                .layout_collection
                .instance_for(
                    LayoutScope::Project {
                        project_id: self.workspace_state.project().id,
                    },
                    BuiltInLayout::Develop,
                )
                .cloned()
                .ok_or(dragent_workspace_model::LayoutValidationError::UnknownActiveLayout),
            _ => return,
        };
        match result.and_then(|instance| self.layout_collection.upsert_and_activate(instance)) {
            Ok(()) => {
                self.layout_menu_open = false;
                self.save_session(ctx);
                ctx.notify();
            }
            Err(error) => {
                log::error!("[workspace-layout] could not switch to {layout:?}: {error}");
            }
        }
    }
}
