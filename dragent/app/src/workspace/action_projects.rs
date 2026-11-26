//! Workspace project flows actions.
//!
//! Handles the corresponding WorkspaceAction variants. Called from the
//! dispatcher (view_actions); bodies moved verbatim.
//!
//! Blocks here are impl WorkspaceView: same type, same behavior.

//! Workspace action dispatch for WorkspaceView.
//!
//! Responsible for:
//! - translating WorkspaceAction intents into domain operations
//! - all action handlers (panes, tabs, terminal, agent, editor, git, sharing)
//!
//! Not responsible for:
//! - rendering (view_render / view_overlays)
//! - defining actions (action.rs)
//! - layout tree algorithms (pane_layout)
//!
//! The block here is the TypedActionView implementation for WorkspaceView:
//! same type, same behavior, organized by responsibility.
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
use std::process::Command;
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
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label, local_branch_name,
    parse_worktree_list, should_replace_project_tab_label, valid_project_name, worktree_dir,
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

use super::language_nav::{parse_text_edits_for_path, DefinitionHoverResolution};
use super::view::WorkspaceView;
use super::view::{
    AddProjectMenu, ExplorerClipboardEntry, ExplorerClipboardMode, FolderGitBulkOp,
    FolderGitFileOp, LanguageDocumentSyncState, LanguageRequestPurpose,
};
use super::view_tabs::{apply_sidebar_visibility, refresh_sidebar_projects};

enum PromptStartFailure {
    AgentUnavailable,
    Other(String),
}

impl From<String> for PromptStartFailure {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl WorkspaceView {
    pub(crate) fn handle_projects_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::OpenProjectWorkspacePrompt { root } => {
                if !root.is_dir() {
                    return;
                }
                self.prompt_menu = None;
                self.new_tab_picker_open = false;
                self.project_start_error = None;
                self.prompt_text.clear();
                self.prompt_attachments.clear();
                self.prompt_links.clear();
                self.prompt_task = None;
                let editor = self.prompt_editor.clone();
                ctx.update_view(&editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx)
                });
                self.receive_project_folder(root.clone(), ProjectFolderIntent::OpenFolder, ctx);
            }
            WorkspaceAction::OpenFolder => {
                self.prompt_menu = None;
                self.choose_project_folder(ProjectFolderIntent::OpenFolder, ctx);
            }
            WorkspaceAction::ShowProjectClone => {
                let _ = self.begin_clone_from_url(String::new(), ctx);
            }
            WorkspaceAction::ShowProjectSsh => self.add_remote_project(None, ctx),
            WorkspaceAction::ShowMachines => self.show_machines(None, ctx),
            WorkspaceAction::CloseProjectStartForm => {
                self.project_start_busy = false;
                self.project_start_mode = None;
                self.project_start_text.clear();
                self.project_start_error = None;
                let editor = self.project_start_editor.clone();
                ctx.update_view(&editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx)
                });
                ctx.notify();
            }
            WorkspaceAction::SubmitProjectStartForm => {
                let value = self.project_start_text.trim().to_string();
                match self.project_start_mode {
                    Some(super::project_start::ProjectStartMode::Clone) => {
                        let _ = self.begin_clone_from_url(value, ctx);
                    }
                    Some(super::project_start::ProjectStartMode::Ssh) => {
                        match super::ssh::SshDestination::parse(&value) {
                            Ok(destination) => self.connect_ssh_destination(destination, ctx),
                            Err(error) => {
                                self.project_start_error = Some(error);
                                ctx.notify();
                            }
                        }
                    }
                    None => {}
                }
            }
            WorkspaceAction::ConnectSshDestination { destination } => {
                let target = destination.display();
                if !crate::settings::settings_tasks_hosts::routing_enabled_for(&target)
                    || !crate::settings::settings_tasks_hosts::probe_allowed_for(&target)
                {
                    self.open_new_workspace_tab(
                        Some(&format!("SSH: {target}")),
                        Some(&destination.shell_command()),
                        ctx,
                    );
                } else {
                    self.add_remote_project(Some(destination.clone()), ctx);
                }
            }
            WorkspaceAction::ConnectGitHub => {
                self.open_explicit_terminal(
                    Some("GitHub".into()),
                    Some("gh auth login".into()),
                    ctx,
                );
            }
            WorkspaceAction::OpenRecentProject { root } => {
                self.prompt_menu = None;
                self.receive_project_folder(root.clone(), ProjectFolderIntent::OpenFolder, ctx);
            }
            WorkspaceAction::CreateNewProject => {
                ctx.open_file_picker(
                    move |result, ctx| match result {
                        Ok(paths) => {
                            if let Some(path) = paths.into_iter().next() {
                                let parent = PathBuf::from(path);
                                let mut counter = 0;
                                let dir = loop {
                                    let name = if counter == 0 {
                                        "Untitled project".to_string()
                                    } else {
                                        format!("Untitled project {}", counter + 1)
                                    };
                                    let candidate = parent.join(&name);
                                    if !candidate.exists() {
                                        break candidate;
                                    }
                                    counter += 1;
                                };
                                match std::fs::create_dir_all(&dir) {
                                    Ok(()) => {
                                        ctx.dispatch_typed_action_deferred(
                                            WorkspaceAction::ProjectFolderSelected {
                                                root: dir,
                                                intent: ProjectFolderIntent::OpenFolder,
                                            },
                                        );
                                    }
                                    Err(error) => {
                                        log::warn!(
                                            "[workspace-create] could not create {}: {error:#}",
                                            dir.display()
                                        );
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!("[workspace-create] destination picker: {error}");
                        }
                    },
                    FilePickerConfiguration::new().folders_only(),
                );
                ctx.notify();
            }
            WorkspaceAction::ShowAddProjectCloneForm => {
                let _ = self.begin_clone_from_url(String::new(), ctx);
            }
            WorkspaceAction::BrowseCloneLocation => {
                if self.clone_busy {
                    return;
                }
                ctx.open_file_picker(
                    move |result, ctx| {
                        if let Ok(paths) = result {
                            if let Some(path) = paths.into_iter().next() {
                                ctx.dispatch_typed_action_deferred(
                                    WorkspaceAction::CloneLocationSelected {
                                        parent: PathBuf::from(path),
                                    },
                                );
                            }
                        }
                    },
                    FilePickerConfiguration::new().folders_only(),
                );
            }
            WorkspaceAction::CloneLocationSelected { parent } => {
                if self.clone_busy {
                    return;
                }
                let value = parent.to_string_lossy().into_owned();
                self.clone_location_text = value.clone();
                ctx.update_view(&self.clone_location_editor, |editor, editor_ctx| {
                    editor.set_text(&value, editor_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::SubmitAddProjectClone => {
                if self.clone_busy {
                    return;
                }
                let url = ctx.update_view(&self.clone_url_editor, |editor, _| {
                    editor.get_text().trim().to_string()
                });
                let location = ctx.update_view(&self.clone_location_editor, |editor, _| {
                    editor.get_text().trim().to_string()
                });
                if clone_target_name(&url).is_none() {
                    self.clone_url_error =
                        Some("Enter a valid HTTPS or Git SSH repository URL.".into());
                } else if location.is_empty() {
                    self.clone_url_error = Some("Choose a project directory.".into());
                } else {
                    let parent = if location == "~" {
                        dragent_paths::home()
                    } else if let Some(relative) = location.strip_prefix("~/") {
                        dragent_paths::home().join(relative)
                    } else {
                        PathBuf::from(location)
                    };
                    if !parent.is_absolute() {
                        self.clone_url_error =
                            Some("Use an absolute project directory path.".into());
                    } else {
                        self.pending_clone_repository = Some(url);
                        self.handle_projects_action(
                            &WorkspaceAction::CloneDestinationSelected { parent },
                            ctx,
                        );
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::OpenCreateProject => {
                self.prompt_menu = None;
                self.add_project_menu = AddProjectMenu::Closed;
                self.create_project_open = true;
                self.create_project_location = default_projects_dir();
                self.create_project_error = None;
                let location_text = self.create_project_location.to_string_lossy().into_owned();
                let name_editor = self.create_project_name_editor.clone();
                let location_editor = self.create_project_location_editor.clone();
                ctx.update_view(&name_editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                    editor_ctx.focus_self();
                });
                ctx.update_view(&location_editor, |editor, editor_ctx| {
                    editor.set_text(&location_text, editor_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::CloseCreateProject => {
                self.create_project_open = false;
                self.create_project_error = None;
                ctx.notify();
            }
            WorkspaceAction::BrowseCreateProjectLocation => {
                ctx.open_file_picker(
                    move |result, ctx| match result {
                        Ok(paths) => {
                            if let Some(path) = paths.into_iter().next() {
                                ctx.dispatch_typed_action_deferred(
                                    WorkspaceAction::CreateProjectLocationSelected {
                                        location: PathBuf::from(path),
                                    },
                                );
                            }
                        }
                        Err(error) => {
                            log::warn!("[workspace-create] location picker: {error}");
                        }
                    },
                    FilePickerConfiguration::new().folders_only(),
                );
                ctx.notify();
            }
            WorkspaceAction::CreateProjectLocationSelected { location } => {
                self.create_project_location = location.clone();
                let text = location.to_string_lossy().into_owned();
                let editor = self.create_project_location_editor.clone();
                ctx.update_view(&editor, |editor, editor_ctx| {
                    editor.set_text(&text, editor_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::SubmitCreateProject => {
                let name = ctx.update_view(&self.create_project_name_editor, |editor, _| {
                    editor.get_text().trim().to_string()
                });
                let location_text = ctx
                    .update_view(&self.create_project_location_editor, |editor, _| {
                        editor.get_text().trim().to_string()
                    });
                if !valid_project_name(&name) {
                    self.create_project_error =
                        Some("Enter a project name without path separators.".to_string());
                    ctx.notify();
                    return;
                }
                let location = if location_text.is_empty() {
                    default_projects_dir()
                } else {
                    PathBuf::from(location_text)
                };
                let dir = location.join(&name);
                if dir.exists() {
                    self.create_project_error = Some(format!("{} already exists.", dir.display()));
                    ctx.notify();
                    return;
                }
                if let Err(error) = std::fs::create_dir_all(&dir) {
                    self.create_project_error =
                        Some(format!("Could not create {}: {error}", dir.display()));
                    ctx.notify();
                    return;
                }
                self.create_project_location = location;
                self.create_project_open = false;
                self.create_project_error = None;
                // Initialize the repository in the background: the project
                // opens immediately, git finishes a moment later.
                let init_dir = dir.clone();
                ctx.spawn(
                    async move {
                        crate::git_cmd::GitCommand::new()
                            .arg("init")
                            .current_dir(&init_dir)
                            .no_window()
                            .output()
                            .map_err(|error| format!("Could not start git init: {error}"))
                    },
                    |_, _, _| {},
                );
                self.receive_project_folder(dir, ProjectFolderIntent::OpenFolder, ctx);
                ctx.notify();
            }
            WorkspaceAction::RefreshWorktreeDetails { root } => {
                self.refresh_worktree_git_status(root.clone(), ctx);
                self.spawn_worktree_pr_refresh(root.clone(), ctx);
            }
            WorkspaceAction::RenameWorktreeBranch { root, branch } => {
                if self.worktree_busy { return; }
                self.worktree_base_root = Some(root.clone());
                self.worktree_start_branch = Some(branch.clone());
                self.worktree_rename_branch = true;
                self.worktree_prompt_open = true;
                self.worktree_error = None;
                ctx.update_view(&self.worktree_branch_editor, |editor, editor_ctx| {
                    editor.set_text(branch, editor_ctx);
                    editor.select_all(editor_ctx);
                    editor_ctx.focus_self();
                });
                ctx.notify();
            }
            WorkspaceAction::ShowNewWorktreeForm => {
                if self.worktree_busy { return; }
                self.worktree_rename_branch = false;
                let Some(base) = self.selected_project_root().filter(|root| root.is_dir()) else {
                    return;
                };
                let naming_source = self
                    .prompt_links
                    .first()
                    .map(|link| link.title.trim())
                    .filter(|title| !title.is_empty())
                    .or_else(|| {
                        let prompt = self.prompt_text.trim();
                        (!prompt.is_empty()).then_some(prompt)
                    });
                let issue_branch = self.prompt_links.iter().find_map(|link| link.branch.clone());
                let prefill = if let Some(branch) = issue_branch {
                    branch
                } else if let Some(source) = naming_source {
                    let suffix = uuid::Uuid::new_v4().simple().to_string();
                    super::project_naming::workspace_branch_from_prompt(source, &suffix[..8])
                } else {
                    let source = self
                        .worktree_start_branch
                        .clone()
                        .or_else(|| local_branch_name(&base));
                    source
                        .map(|branch| format!("{branch}-worktree"))
                        .unwrap_or_else(|| "new-worktree".to_string())
                };
                self.worktree_base_root = Some(base);
                self.worktree_prompt_open = true;
                self.worktree_busy = false;
                self.worktree_error = None;
                let editor = self.worktree_branch_editor.clone();
                ctx.update_view(&editor, |editor, editor_ctx| {
                    editor.set_text(&prefill, editor_ctx);
                    editor_ctx.focus_self();
                });
                ctx.notify();
            }
            WorkspaceAction::CloseNewWorktree => {
                if self.worktree_busy { return; }
                self.worktree_rename_branch = false;
                self.worktree_prompt_open = false;
                self.worktree_start_branch = None;
                self.worktree_error = None;
                ctx.notify();
            }
            WorkspaceAction::ConfirmGitInit => {
                let Some(root) = self.pending_git_init_root.take() else {
                    return;
                };
                ctx.spawn(
                    async move {
                        crate::git_cmd::GitCommand::new()
                            .arg("init")
                            .current_dir(&root)
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .no_window()
                            .output()
                            .map_err(|error| format!("Could not start git init: {error}"))?;
                        Ok::<_, String>(root)
                    },
                    |view, result: Result<PathBuf, String>, ctx| match result {
                        Ok(root) => {
                            view.receive_project_folder(root, ProjectFolderIntent::OpenFolder, ctx);
                        }
                        Err(error) => {
                            log::warn!("[workspace-folder] git init failed: {error}");
                            ctx.notify();
                        }
                    },
                );
                ctx.notify();
            }
            WorkspaceAction::DismissGitInit => {
                self.pending_git_init_root = None;
                ctx.notify();
            }
            WorkspaceAction::OpenPromptMenu { menu } => {
                self.prompt_menu = Some(*menu);
                if *menu == super::action::PromptMenu::LeadModel {
                    self.prompt_model_tab = crate::teams::models::ModelTab::Agent(self.prompt_agent);
                }
                self.prompt_menu_scroll = ClippedScrollStateHandle::new();
                self.prompt_device_remote_open = false;
                match menu {
                    super::action::PromptMenu::Branch => {
                        self.prompt_branch_search.clear();
                        ctx.update_view(&self.prompt_branch_search_editor, |editor, editor_ctx| {
                            editor.set_text("", editor_ctx);
                            editor_ctx.focus_self();
                        });
                        self.refresh_prompt_branches(ctx);
                    }
                    super::action::PromptMenu::Links(_) => self.refresh_prompt_links(ctx),
                    super::action::PromptMenu::Agent | super::action::PromptMenu::Team | super::action::PromptMenu::LeadModel => {
                        self.prompt_agent_search.clear();
                        ctx.update_view(&self.prompt_agent_search_editor, |editor, editor_ctx| {
                            editor.set_placeholder(if *menu == super::action::PromptMenu::LeadModel { "Search models" } else { "Search agents…" }, editor_ctx);
                            editor.set_text("", editor_ctx);
                            editor_ctx.focus_self();
                        });
                        self.refresh_prompt_agents(ctx);
                    }
                    _ => {}
                }
                ctx.notify();
            }
            WorkspaceAction::ClosePromptMenus => {
                self.prompt_menu = None;
                self.prompt_link_generation = self.prompt_link_generation.wrapping_add(1);
                self.prompt_device_remote_open = false;
                ctx.notify();
            }
            WorkspaceAction::ClearPromptAgentSearch => {
                self.prompt_agent_search.clear();
                ctx.update_view(&self.prompt_agent_search_editor, |editor, editor_ctx| {
                    editor.set_text("", editor_ctx);
                });
                ctx.notify();
            }
            WorkspaceAction::SelectPromptAgent { agent } => {
                // Disabled agents stay unselectable even if a stale menu row
                // dispatches the action.
                if crate::settings::AppSettings::global()
                    .agent
                    .is_agent_disabled(agent.command_key())
                {
                    ctx.notify();
                    return;
                }
                if self.prompt_agent != *agent { self.prompt_lead_model = None; }
                self.prompt_agent = *agent;
                self.prompt_menu = None;
                ctx.notify();
            }
            WorkspaceAction::BrowsePromptModelTab { tab } => {
                self.prompt_model_tab = *tab;
                self.prompt_agent_search.clear();
                self.prompt_menu_scroll = ClippedScrollStateHandle::new();
                ctx.update_view(&self.prompt_agent_search_editor, |editor, ctx| { editor.set_text("", ctx); ctx.focus_self(); });
                ctx.notify();
            }
            WorkspaceAction::PickPromptAgentModel { agent, model } => {
                self.prompt_agent = *agent;
                self.prompt_lead_model = model.clone();
                self.prompt_menu = None;
                ctx.notify();
            }
            WorkspaceAction::TogglePromptModelFavorite { favorite } => {
                crate::teams::models::toggle_favorite(favorite.clone());
                ctx.update_view(&self.prompt_agent_search_editor, |_, ctx| ctx.focus_self());
                ctx.notify();
            }
            WorkspaceAction::SetPromptLeadModel { model } => {
                self.prompt_lead_model = model.clone();
                self.prompt_menu = None;
                ctx.notify();
            }
            WorkspaceAction::TogglePromptTeam => {
                self.prompt_team = !self.prompt_team;
                self.project_start_error = None;
                ctx.notify();
            }
            WorkspaceAction::SetPromptTeamCost { cost } => {
                self.prompt_team_cost = *cost;
                self.prompt_team = true;
                ctx.notify();
            }
            WorkspaceAction::TogglePromptTeamAgent { agent } => {
                match self.prompt_team_agents.iter().position(|chosen| chosen == agent) {
                    Some(index) => {
                        self.prompt_team_agents.remove(index);
                    }
                    None => {
                        self.prompt_team_agents.push(*agent);
                        // Choosing who's on the team means working as one.
                        self.prompt_team = true;
                    }
                }
                ctx.notify();
            }
            WorkspaceAction::OpenPromptAgentInstall { agent } => {
                // A remote project installs the agent on its machine.
                match self.machine_agents(self.prompt_context_root().as_deref(), ctx) {
                    Some(machine) => {
                        self.prompt_menu = None;
                        self.install_agent_on_machine(&machine, *agent, ctx);
                    }
                    None => self.open_prompt_agent_install(*agent, ctx),
                }
            }
            WorkspaceAction::OpenCustomAgentInstall { agent_id } => {
                self.open_custom_agent_install(agent_id.clone(), ctx);
            }
            WorkspaceAction::FocusPopupTerminal => {
                if let Some(terminal) = self.prompt_install_terminal.clone() {
                    // Inside it already (an editor of its own, say): leave it.
                    if !terminal.read(ctx, |view, app| view.pane_has_focus(app)) {
                        ctx.focus(&terminal);
                    }
                }
            }
            WorkspaceAction::ClosePromptAgentInstall => {
                self.close_prompt_agent_install(ctx);
            }
            WorkspaceAction::RetryRemoteToolElevated => {
                self.retry_remote_tool_job_elevated(ctx);
            }
            WorkspaceAction::CheckPromptAgentInstall => {
                self.check_prompt_agent_install(ctx);
            }
            WorkspaceAction::OpenPromptAttachmentPicker => {
                self.prompt_menu = None;
                self.prompt_link_generation = self.prompt_link_generation.wrapping_add(1);
                self.prompt_notice = None;
                let (tx, rx) = async_channel::bounded::<Vec<PathBuf>>(1);
                ctx.spawn_stream_local(
                    rx,
                    |view, paths, ctx| view.receive_prompt_context_files(paths, ctx),
                    |_, _| {},
                );
                crate::agent::attachments::pick_files_async(tx);
            }
            WorkspaceAction::AttachPromptFiles { paths } => {
                self.receive_prompt_context_files(paths.clone(), ctx);
            }
            WorkspaceAction::OpenPromptLinkKind { kind } => {
                self.prompt_menu = Some(super::action::PromptMenu::Links(*kind));
                // Opens on each service's own default list (open items). A
                // prefilled query would be searched as literal words.
                let initial = String::new();
                self.prompt_link_query = initial.clone();
                self.prompt_link_results.clear();
                self.prompt_link_error = None;
                // Nothing to search until the tracker is connected (the menu
                // links to Settings → Integrations instead).
                let focus_search = *kind == super::project_links::LinkKind::PullRequest
                    || crate::integrations::is_connected(kind.provider());
                ctx.update_view(&self.prompt_link_search_editor, |editor, editor_ctx| {
                    editor.set_text(&initial, editor_ctx);
                    if focus_search {
                        editor_ctx.focus_self();
                    }
                });
                self.refresh_prompt_links(ctx);
                ctx.notify();
            }
            WorkspaceAction::SelectPromptLink { index } => {
                if let Some(link) = self.prompt_link_results.get(*index).cloned() {
                    if link.kind == super::project_links::LinkKind::PullRequest {
                        self.prompt_links.retain(|item| item.kind != link.kind);
                    }
                    if !self.prompt_links.iter().any(|item| item.url == link.url) {
                        // Searches bring a summary: fetch the whole item
                        // for the agent, its comments included (a Notion
                        // page's blocks, a Slack thread, a stack trace).
                        if let Some(item) = link
                            .item()
                            .filter(|item| crate::integrations::items::abilities(item.provider).read)
                        {
                            let url = link.url.clone();
                            let root = self.selected_project_root().unwrap_or_default();
                            ctx.spawn(
                                async move {
                                    tokio::task::spawn_blocking(move || {
                                        crate::integrations::items::get(item.provider, &root, item.id())
                                            .map(|full| Some(full.body))
                                            .or_else(|error| {
                                                crate::integrations::items::details(&item).map_err(|_| error)
                                            })
                                    })
                                    .await
                                    .unwrap_or_else(|error| Err(error.to_string()))
                                },
                                move |view, result, ctx| match result {
                                    Ok(Some(body)) if !body.is_empty() => {
                                        if let Some(link) = view.prompt_links.iter_mut().find(|link| link.url == url) {
                                            link.body = body;
                                            view.save_session(ctx);
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(error) => log::warn!("[workspace] fetching a linked item: {error}"),
                                },
                            );
                        }
                        self.prompt_links.push(link);
                    }
                    self.prompt_menu = None;
                    self.prompt_link_generation = self.prompt_link_generation.wrapping_add(1);
                    self.save_session(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::RemovePromptLink { index } => {
                if *index < self.prompt_links.len() {
                    self.prompt_links.remove(*index);
                    self.save_session(ctx);
                }
                ctx.notify();
            }
            WorkspaceAction::ClearPromptTask => {
                // Leaving out the task leaves out the ones queued after it.
                self.prompt_task = None;
                self.task_start_queue.clear();
                ctx.notify();
            }
            WorkspaceAction::RemovePromptAttachment { index } => {
                if *index < self.prompt_attachments.len() {
                    self.prompt_attachments.remove(*index);
                }
                ctx.notify();
            }
            WorkspaceAction::TogglePromptVoiceInput => {
                self.toggle_prompt_voice_input(ctx);
            }
            WorkspaceAction::TogglePromptRemote => {
                self.prompt_device_remote_open = !self.prompt_device_remote_open;
                ctx.notify();
            }
            WorkspaceAction::ConnectPromptSsh => {
                self.prompt_menu = None;
                self.prompt_device_remote_open = false;
                ctx.dispatch_typed_action_deferred(WorkspaceAction::ShowProjectSsh);
            }
            WorkspaceAction::OpenNewWorktreeFromPrompt => {
                if self.worktree_busy || !self.prompt_worktree_available() { return; }
                self.worktree_rename_branch = false;
                self.prompt_menu = None;
                self.worktree_start_branch = None;
                self.prompt_selected_worktree = None;
                ctx.notify();
            }
            WorkspaceAction::SelectPromptCurrentCheckout => {
                self.prompt_menu = None;
                self.prompt_selected_worktree = self.selected_project_root();
                self.prompt_branch = self
                    .prompt_selected_worktree
                    .as_ref()
                    .and_then(|root| local_branch_name(root));
                self.worktree_start_branch = None;
                ctx.notify();
            }
            WorkspaceAction::CancelReviewSetup => {
                #[cfg(not(target_family = "wasm"))]
                self.cancel_review_setup(ctx);
            }
            WorkspaceAction::SubmitPrompt => {
                #[cfg(not(target_family = "wasm"))]
                if self.review_setup.is_some() { self.start_review_setup(ctx); return; }
                if self.project_start_busy {
                    return;
                }
                let prompt_root = self
                    .prompt_selected_worktree
                    .clone()
                    .or_else(|| self.selected_project_root());
                let machine = self.machine_agents(prompt_root.as_deref(), ctx);
                if self.prompt_agent != super::action::PromptAgent::Dragent {
                    // A project on a machine starts the agent installed there.
                    if let Some(machine) = &machine {
                        let cli = self.prompt_agent.cli().unwrap_or_default();
                        let label = self.prompt_agent.label();
                        if let Some(problem) = self.machine_agent_problem(machine, cli, label, ctx) {
                            self.project_start_error = Some(problem);
                            ctx.notify();
                            return;
                        }
                    }
                }
                let check_local_agent = machine.is_none()
                    && self.prompt_agent != super::action::PromptAgent::Dragent;
                let prompt_text = self.prompt_text.trim().to_string();
                if prompt_text.is_empty()
                    && self.prompt_attachments.is_empty()
                    && self.prompt_links.is_empty()
                {
                    // Nothing to work on, whichever agent is picked.
                    return;
                }
                // A team: the picked agent leads; its kind and the chosen
                // helpers do the work.
                let team_roster: Option<Vec<String>> = if self.prompt_team {
                    if let Some(problem) = super::team_prompt::team_problem(
                        self.prompt_agent,
                        &prompt_text,
                        machine.is_some(),
                    ) {
                        self.project_start_error = Some(problem);
                        ctx.notify();
                        return;
                    }
                    Some(super::team_prompt::roster(self.prompt_agent, &self.prompt_team_agents))
                } else {
                    None
                };
                let team_project = self.selected_project_root();
                let team_cost = self.prompt_team_cost;
                let workspace_name = self
                    .prompt_links
                    .iter()
                    .find(|link| link.kind == super::project_links::LinkKind::PullRequest)
                    .and_then(|link| {
                        super::project_naming::workspace_title_from_prompt(&format!(
                            "{}: {}",
                            link.identifier, link.title
                        ))
                    })
                    .or_else(|| super::project_naming::workspace_title_from_prompt(&prompt_text))
                    .or_else(|| {
                        self.prompt_links.first().and_then(|link| {
                            super::project_naming::workspace_title_from_prompt(&format!(
                                "{}: {}",
                                link.identifier, link.title
                            ))
                        })
                    });
                let prompt =
                    super::project_links::prompt_with_links(&prompt_text, &self.prompt_links);
                let naming_prompt = self
                    .prompt_links
                    .first()
                    .map(|link| link.title.trim().to_string())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| prompt_text.clone());
                let naming_agent = self.prompt_agent;
                // An issue names the work itself: its tracker's branch, and
                // its key and title.
                let issue_names = self.prompt_links.iter().find_map(|link| {
                    let branch = link.branch.clone()?;
                    let title = super::project_naming::workspace_title_from_prompt(&format!(
                        "{}: {}",
                        link.identifier, link.title
                    ))?;
                    Some(super::project_workspace_creation::WorkspaceNames { title, branch })
                });
                // A task being started names it the same way: its key and
                // title, on the task's branch.
                let started_task = self.prompt_task.clone();
                let issue_names = issue_names.or_else(|| {
                    let task = started_task.as_ref()?;
                    let title = super::project_naming::workspace_title_from_prompt(&format!(
                        "{}: {}",
                        task.key, task.title
                    ))?;
                    Some(super::project_workspace_creation::WorkspaceNames {
                        title,
                        branch: dragent_tasks::branch_name(&task.key, &task.title),
                    })
                });
                let linked_issues: Vec<_> = self
                    .prompt_links
                    .iter()
                    .filter_map(super::project_links::PromptLink::linked_issue)
                    .collect();
                // Without a commit to branch from there is no worktree to
                // create: the work starts in the project's own folder.
                let selected_checkout = self.prompt_selected_worktree.is_some()
                    || !self.prompt_worktree_available();
                // Only a new worktree is the issue's own workspace.
                let record_issues = !selected_checkout && !linked_issues.is_empty();
                let base_branch = self.worktree_start_branch.clone();
                let Some(project_root) = self
                    .prompt_selected_worktree
                    .clone()
                    .or_else(|| self.selected_project_root())
                    .filter(|root| root.is_dir())
                else {
                    self.project_start_error = Some("Choose a project before sending.".into());
                    ctx.notify();
                    return;
                };
                self.project_start_busy = true;
                self.project_start_error = None;
                self.prompt_menu = None;
                ctx.notify();
                let team_title = prompt_text.clone();
                let agent_names_it = issue_names.is_none() && !selected_checkout;
                let naming_prompt_for_start = naming_prompt.clone();
                ctx.spawn(
                    async move {
                        // Executable discovery probes the login shell and npm.
                        // Run it after marking busy, off the UI thread, so the
                        // send button can repaint immediately.
                        if check_local_agent
                            && !tokio::task::spawn_blocking(move || {
                                prompt_agent_interactive_command(naming_agent).is_some()
                            })
                            .await
                            .map_err(|error| format!("Couldn't check the agent: {error}"))?
                        {
                            return Err(PromptStartFailure::AgentUnavailable);
                        }
                        // The team first: without the background service
                        // there's no team to lead, and nothing is created.
                        let team = match team_roster {
                            Some(roster) => {
                                let project = team_project.unwrap_or_else(|| project_root.clone());
                                let team = tokio::task::spawn_blocking(move || {
                                    crate::cli::team::create_for_app(&team_title, &project, &roster, team_cost)
                                })
                                .await
                                .map_err(|error| format!("Couldn't start the team: {error}"))?
                                .map_err(|error| format!("Couldn't start the team: {error}"))?;
                                Some(team)
                            }
                            None => None,
                        };
                        if selected_checkout {
                            let branch = local_branch_name(&project_root)
                                .unwrap_or_else(|| "local branch".into());
                            Ok((project_root, branch, None, team))
                        } else {
                            // The agent starts now, under a readable name; it
                            // names the work properly once it has started.
                            let names = async {
                                issue_names.unwrap_or_else(|| {
                                    super::project_workspace_creation::provisional_names(
                                        &naming_prompt_for_start,
                                    )
                                })
                            };
                            let base = project_root.clone();
                            let start = tokio::task::spawn_blocking(move || {
                                super::project_workspace_creation::latest_start_point(
                                    &base,
                                    base_branch.as_deref(),
                                )
                            });
                            let (names, start) = tokio::join!(names, start);
                            let start = start.map_err(|error| {
                                format!("Could not prepare worktree: {error}")
                            })??;
                            let root = project_root.clone();
                            let branch_name = names.branch.clone();
                            let (path, branch) = tokio::task::spawn_blocking(move || {
                                super::project_workspace_creation::create_worktree(
                                    &root,
                                    &branch_name,
                                    &start,
                                )
                            })
                            .await
                            .map_err(|error| format!("Could not create worktree: {error}"))??;
                            super::workspace_setup::maybe_init_worktree(&path);
                            Ok((path, branch, Some(names.title), team))
                        }
                    },
                    move |view,
                          result: Result<
                        (PathBuf, String, Option<String>, Option<dragent_host::team::Team>),
                        PromptStartFailure,
                    >,
                          ctx| {
                        view.project_start_busy = false;
                        match result {
                            Ok((root, branch, generated_title, team)) => {
                                // A new worktree remembers the issues it was
                                // started from (its header shows them).
                                if record_issues {
                                    if let Err(error) =
                                        crate::integrations::links::record(&root, linked_issues)
                                    {
                                        log::warn!("[workspace] {error}");
                                    }
                                }
                                // A coding agent that leads gets the request with
                                // its team and how to lead it; Dragent's own agent
                                // just the request (it's told the rest in its
                                // instructions while it leads).
                                let (prompt, lead_of) = match team {
                                    Some(team) if view.prompt_agent == super::action::PromptAgent::Dragent => {
                                        (prompt, Some(team.id))
                                    }
                                    Some(team) => (crate::cli::team::lead_prompt(&crate::team_source::native_team(&team), &prompt), Some(team.id)),
                                    None => (prompt, None),
                                };
                                #[cfg(not(target_family = "wasm"))]
                                if let Some(task) = started_task {
                                    view.prompt_task = None;
                                    view.link_started_task(task, &root, &branch, ctx);
                                }
                                let title = generated_title.or(workspace_name);
                                let naming = agent_names_it.then(|| {
                                    (root.clone(), branch.clone(), title.clone(), lead_of.clone())
                                });
                                // A team just started: show its Board, where
                                // the lead's planning shows, not its terminal.
                                let board = lead_of.clone();
                                view.finish_project_prompt_workspace(
                                    root,
                                    branch,
                                    prompt,
                                    title,
                                    lead_of,
                                    ctx,
                                );
                                if let Some((root, branch, title, team)) = naming {
                                    view.name_workspace_with_agent(
                                        root,
                                        branch,
                                        title,
                                        naming_prompt,
                                        naming_agent,
                                        team,
                                        ctx,
                                    );
                                }
                                #[cfg(not(target_family = "wasm"))]
                                if let Some(team) = board {
                                    view.open_team_board(team, ctx);
                                }
                                #[cfg(not(target_family = "wasm"))]
                                view.start_next_queued_task(ctx);
                            }
                            Err(error) => {
                                view.project_start_error = Some(match error {
                                    PromptStartFailure::AgentUnavailable => {
                                        view.prompt_installed_agents.remove(&naming_agent);
                                        format!(
                                            "{} is not installed. Select it again to open the installer.",
                                            naming_agent.label()
                                        )
                                    }
                                    PromptStartFailure::Other(message) => message,
                                });
                                ctx.notify();
                            }
                        }
                    },
                );
            }
            WorkspaceAction::OpenPromptWorktree { path } => {
                self.prompt_menu = None;
                self.prompt_selected_worktree = path.join(".git").exists().then(|| path.clone());
                self.prompt_branch = local_branch_name(path);
                ctx.notify();
            }
            WorkspaceAction::CreatePromptWorktreeFromBranch { branch } => {
                self.prompt_menu = None;
                if !self.prompt_worktree_available() { ctx.notify(); return; }
                self.worktree_start_branch = Some(branch.clone());
                self.prompt_selected_worktree = None;
                self.prompt_branch = Some(branch.clone());
                ctx.notify();
            }
            WorkspaceAction::OpenDragentRepo => {
                self.open_link("https://github.com/Dragent-Term/Dragent", ctx);
            }
            WorkspaceAction::SubmitNewWorktree => {
                if self.worktree_busy {
                    return;
                }
                let branch = ctx.update_view(&self.worktree_branch_editor, |editor, _| {
                    editor.get_text().trim().to_string()
                });
                if branch.is_empty() {
                    self.worktree_error = Some("Enter a branch name.".to_string());
                    ctx.notify();
                    return;
                }
                if let Err(error) =
                    crate::terminal::view::terminal_helpers::validate_branch_name(&branch)
                {
                    self.worktree_error = Some(error);
                    ctx.notify();
                    return;
                }
                if self.worktree_rename_branch {
                    let Some(root) = self.worktree_base_root.clone() else { return; };
                    let expected = self.worktree_start_branch.clone();
                    self.worktree_busy = true;
                    self.worktree_error = None;
                    ctx.notify();
                    ctx.spawn(async move {
                        let result = (|| -> Result<(), String> {
                            let current = local_branch_name(&root).ok_or("This checkout has no branch to rename.")?;
                            if Some(&current) != expected.as_ref() {
                                return Err("The checkout's branch changed. Close this dialog and try again.".into());
                            }
                            let output = crate::git_cmd::GitCommand::new()
                                .args(["branch", "-m", "--", &branch])
                                .current_dir(&root).output().map_err(|error| error.to_string())?;
                            if !output.status.success() {
                                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
                            }
                            Ok(())
                        })();
                        (root, expected, branch, result)
                    }, |view, (root, previous, branch, result), ctx| {
                        view.worktree_busy = false;
                        match result {
                            Ok(()) => {
                                view.worktree_prompt_open = false;
                                view.worktree_rename_branch = false;
                                // Recorded now, so the status refresh below
                                // doesn't announce the same change again.
                                let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
                                for key in [&root, &canonical] {
                                    if let Some(known) = view.worktree_git_cache.get_mut(key) {
                                        known.branch = Some(branch.clone());
                                    }
                                }
                                view.on_branch_changed(&root, previous.as_deref(), Some(&branch), ctx);
                                view.refresh_worktree_git_status(root.clone(), ctx);
                                view.refresh_document_tree(ctx);
                            }
                            Err(error) => view.worktree_error = Some(error),
                        }
                        ctx.notify();
                    });
                    return;
                }
                let Some(base) = self.worktree_base_root.clone().filter(|root| root.is_dir())
                else {
                    self.worktree_error =
                        Some("The project folder is no longer available.".to_string());
                    ctx.notify();
                    return;
                };
                let target = worktree_dir(&base, &branch);
                if target.exists() {
                    self.worktree_error = Some(format!(
                        "{} already exists. Choose another branch name.",
                        target.display()
                    ));
                    ctx.notify();
                    return;
                }
                self.worktree_busy = true;
                self.worktree_error = None;
                let start_branch = self.worktree_start_branch.clone();
                ctx.notify();
                ctx.spawn(
                    async move {
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent).map_err(|error| {
                                format!("Could not create worktree root: {error}")
                            })?;
                        }
                        let start = super::project_workspace_creation::latest_start_point(
                            &base,
                            start_branch.as_deref(),
                        )?;
                        let mut command = crate::git_cmd::GitCommand::new();
                        command
                            .args(["worktree", "add", "--no-track", "-b", &branch, "--"])
                            .arg(&target);
                        command.arg(&start);
                        let output = command
                            .current_dir(&base)
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .no_window()
                            .output()
                            .map_err(|error| format!("Could not start git worktree: {error}"))?;
                        if output.status.success() {
                            Ok(target)
                        } else {
                            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                            Err(if detail.is_empty() {
                                format!("git worktree exited with {}", output.status)
                            } else {
                                detail
                            })
                        }
                    },
                    |view, result: Result<PathBuf, String>, ctx| {
                        view.worktree_busy = false;
                        match result {
                            Ok(root) => {
                                view.worktree_prompt_open = false;
                                view.worktree_start_branch = None;
                                view.worktree_error = None;
                                view.prompt_branch = local_branch_name(&root);
                                view.prompt_selected_worktree = Some(root);
                                ctx.notify();
                            }
                            Err(error) => {
                                view.worktree_error = Some(error);
                                ctx.notify();
                            }
                        }
                    },
                );
            }
            WorkspaceAction::CloneRepositoryFromUrl { url } => {
                if self.begin_clone_from_url(url.clone(), ctx).is_err() {
                    return;
                }
                ctx.notify();
            }
            WorkspaceAction::CloneDestinationSelected { parent } => {
                if self.clone_busy {
                    return;
                }
                let Some(repository) = self.pending_clone_repository.take() else {
                    return;
                };
                let Some(name) = clone_target_name(&repository) else {
                    return;
                };
                let target = parent.join(name);
                let parent = parent.clone();
                self.clone_busy = true;
                self.clone_url_error = None;
                ctx.focus_self();
                ctx.spawn(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            std::fs::create_dir_all(&parent)
                                .map_err(|error| format!("Could not create project directory: {error}"))?;
                            // Reserve the destination atomically; never overwrite an existing folder.
                            std::fs::create_dir(&target).map_err(|error| {
                                format!("Could not create {}: {error}. Choose another directory.", target.display())
                            })?;
                            let result = crate::git_cmd::GitCommand::new()
                                .current_dir(&parent)
                                .env("GIT_TERMINAL_PROMPT", "0")
                                .arg("clone").arg("--").arg(&repository).arg(&target).output()
                                .map_err(|error| format!("Could not start git clone: {error}"))
                                .and_then(|output| {
                                    if output.status.success() { Ok(()) } else {
                                        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                                        Err(if detail.is_empty() { format!("git clone exited with {}", output.status) } else { detail })
                                    }
                                });
                            if let Err(error) = result {
                                // This directory was created by this operation, so a retry starts cleanly.
                                if let Err(cleanup) = std::fs::remove_dir_all(&target) {
                                    if target.exists() {
                                        return Err(format!("{error}\nCould not remove the incomplete clone: {cleanup}"));
                                    }
                                }
                                return Err(error);
                            }
                            Ok(target)
                        }).await.map_err(|error| format!("Cloning failed: {error}"))?
                    },
                    |view, result, ctx| {
                        view.clone_busy = false;
                        match result {
                            Ok(root) => {
                                view.add_project_menu = AddProjectMenu::Closed;
                                view.add_project = None;
                                view.receive_project_folder(root, ProjectFolderIntent::OpenFolder, ctx);
                            }
                            Err(error) => view.clone_url_error = Some(error),
                        }
                        ctx.notify();
                    },
                );
                ctx.notify();
            }
            WorkspaceAction::ProjectFolderSelected { root, intent } => {
                self.receive_project_folder(root.clone(), *intent, ctx);
            }
            WorkspaceAction::LaunchProjectWork => {
                self.launch_project_work(ctx);
            }
            WorkspaceAction::LaunchProjectDragent => {
                self.handle_action(&WorkspaceAction::NewAgentTab, ctx);
            }
            WorkspaceAction::LaunchProjectCodex => {
                self.launch_project_cli("Codex", Some("codex"), ctx);
            }
            WorkspaceAction::LaunchProjectClaude => {
                self.launch_project_cli("Claude", Some("claude"), ctx);
            }
            WorkspaceAction::LaunchProjectOpenCode => {
                self.launch_project_cli("OpenCode", Some("opencode"), ctx);
            }
            WorkspaceAction::LaunchProjectTerminal => {
                self.launch_project_cli("Terminal", None, ctx);
            }
            WorkspaceAction::BrowseProjectFiles => {
                self.project_start_visible = false;
                self.project_start_mode = None;
                self.project_start_error = None;
                self.right_panel_visible = true;
                self.right_panel_user_closed = false;
                ctx.update_view(&self.project_sidebar_view, |sidebar, sidebar_ctx| {
                    sidebar.set_sidebar_tab(super::project_sidebar::SidebarTab::Files, sidebar_ctx);
                });
                refresh_sidebar_projects(self, ctx);
                self.sync_left_panel(ctx);
                self.save_session(ctx);
                ctx.notify();
            }
            _ => {}
        }
    }
}

/// Resolve the executable used by the project composer. Other terminal apps
/// can put placeholder agent wrappers on PATH, so `command -v` alone does not
/// establish that the agent itself is installed.
///
/// Resolution order (non-Windows): the user's login-shell PATH (fresh probe,
/// so installs that edited shell profiles are picked up without a restart),
/// then this process's PATH, then existence-gated fallbacks (npm's global
/// bin resolved dynamically via `npm config get prefix`, plus `~/.local/bin`).
/// The same order backs detection, install verification, and launch, so an
/// agent reported as installed is executable from the terminals Dragent
/// spawns (which source the same login profile via `-l`).
pub(super) fn prompt_agent_command(binary: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use crate::process_ext::NoWindow;
        // This process's PATH dates from launch; an installer that just ran
        // only updated the registry copy.
        let mut command = Command::new("where");
        if let Some(path) = dragent_pty::fresh_windows_path() {
            command.env("PATH", path);
        }
        let output = crate::process_ext::probe_output(
            command.arg(binary).no_window(),
            // Generous: a busy machine must not report an installed agent as missing.
            Duration::from_secs(5),
        )
            .ok()
            .filter(|output| output.status.success())?;
        output
            .stdout
            .split(|byte| *byte == b'\n' || *byte == b'\r')
            .filter_map(|line| std::str::from_utf8(line).ok())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .find(|path| path.is_file() && !is_external_agent_wrapper(path))
            .and_then(|path| {
                path.to_str()
                    .map(crate::terminal::view::terminal_helpers::shell_quote)
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        prompt_agent_command_in(binary, &agent_search_dirs())
    }
}

/// Resolve `binary` against a precomputed [`agent_search_dirs`] set. Detection
/// passes share one set so a full refresh costs one shell probe, not one per
/// agent.
#[cfg(not(target_os = "windows"))]
pub(super) fn prompt_agent_command_in(binary: &str, dirs: &[PathBuf]) -> Option<String> {
    find_executable_in(binary, dirs)
        .and_then(|path| path.to_str().map(str::to_string))
        .map(|path| crate::terminal::view::terminal_helpers::shell_quote(&path))
}

/// Windows has no shared dir set (`where` owns resolution); the precomputed
/// argument is ignored so call sites stay uniform across platforms.
#[cfg(target_os = "windows")]
pub(super) fn prompt_agent_command_in(binary: &str, _dirs: &[PathBuf]) -> Option<String> {
    prompt_agent_command(binary)
}

/// Full ordered search set for one detection pass. Empty on Windows.
#[cfg(not(target_os = "windows"))]
pub(super) fn agent_search_dirs() -> Vec<PathBuf> {
    let mut dirs = agent_path_dirs();
    for dir in agent_fallback_dirs() {
        push_unique_dir(&mut dirs, dir);
    }
    dirs
}

/// Windows resolution lives in `where`; there is no shared dir set.
#[cfg(target_os = "windows")]
pub(super) fn agent_search_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// Login-shell PATH first, then this process's PATH, deduplicated.
#[cfg(not(target_os = "windows"))]
fn agent_path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(login_path) = login_shell_path() {
        for dir in std::env::split_paths(&login_path) {
            push_unique_dir(&mut dirs, dir);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            push_unique_dir(&mut dirs, dir);
        }
    }
    dirs
}

/// Existence-gated fallbacks for package-manager bins that may never reach a
/// shell profile. Resolved dynamically per pass — never hard-coded per agent.
#[cfg(not(target_os = "windows"))]
fn agent_fallback_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(bin) = npm_global_bin_dir() {
        push_unique_dir(&mut dirs, bin);
    }
    if let Some(home) = dirs::home_dir() {
        let local_bin = home.join(".local/bin");
        if local_bin.is_dir() {
            push_unique_dir(&mut dirs, local_bin);
        }
    }
    dirs
}

#[cfg(not(target_os = "windows"))]
fn push_unique_dir(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if dir.is_absolute() && !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// The user's login shell when it speaks `-l -c` (sources `~/.zprofile` and
/// friends, where package managers put their bins); plain `sh` otherwise so
/// bash-profile users and minimal environments still resolve.
#[cfg(not(target_os = "windows"))]
fn login_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        let base = shell
            .rsplit('/')
            .next()
            .unwrap_or(&shell)
            .to_ascii_lowercase();
        if base == "zsh" || base == "bash" {
            return shell;
        }
    }
    "sh".to_string()
}

/// Fresh login-shell PATH. Runs per detection pass (not per agent, not
/// cached across passes) so profile edits from a just-finished installer are
/// visible immediately, and long-lived host/daemon processes never serve a
/// stale copy here.
#[cfg(not(target_os = "windows"))]
fn login_shell_path() -> Option<std::ffi::OsString> {
    crate::process_ext::probe_output(
        Command::new(login_shell()).args(["-l", "-c", "printf %s \"$PATH\""]),
        Duration::from_secs(2),
    )
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            std::ffi::OsString::from(String::from_utf8_lossy(&output.stdout).into_owned())
        })
}

/// npm's global `bin` directory (`<prefix>/bin`), when npm exists and the
/// directory does. Covers every `npm install -g` agent whose prefix never
/// reached a shell profile.
#[cfg(not(target_os = "windows"))]
fn npm_global_bin_dir() -> Option<PathBuf> {
    let output = crate::process_ext::probe_output(
        Command::new("npm").args(["config", "get", "prefix"]),
        Duration::from_secs(2),
    )
        .ok()
        .filter(|output| output.status.success())?;
    let prefix = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if prefix.is_empty() {
        return None;
    }
    let bin = PathBuf::from(prefix).join("bin");
    bin.is_dir().then_some(bin)
}

/// Error for an installer that exited 0 while its binary still does not
/// resolve. Names the expected executable and what was searched so the user
/// can fix PATH instead of re-running a successful install.
fn not_on_path_message(binary: &str) -> String {
    format!(
        "The agent executable '{binary}' was not found in Dragent's PATH \
         (login shell, app environment, npm global bin). If the installer printed \
         a custom location, add it to your shell profile, then choose Check again."
    )
}

#[cfg(not(target_os = "windows"))]
fn find_executable_in(binary: &str, dirs: &[PathBuf]) -> Option<PathBuf> {    use std::os::unix::fs::PermissionsExt;

    dirs.iter()
        .map(|dir| dir.join(binary))
        .find(|candidate| {
            let Ok(metadata) = candidate.metadata() else {
                return false;
            };
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                return false;
            }
            // Another agent launcher puts shim scripts ahead of the real
            // agent binaries. They are useful inside that launcher, but
            // running one from Dragent only produces a misleading
            // "not found" error when the underlying CLI is absent. Skip
            // these shims and keep looking for the actual executable
            // later in PATH.
            !is_external_agent_wrapper(candidate)
        })
}

/// Skip launcher shims that re-exec the real binary: PATH entries inside a
/// `<name>-<hex>/bin`-style shadow directory, or tiny scripts whose head
/// contains a re-exec marker. Without this, probing PATH for agent CLIs
/// picks up the wrapper instead of the real binary later in PATH.
fn is_external_agent_wrapper(path: &Path) -> bool {
    let Some(parent) = path.parent().and_then(Path::to_str) else {
        return false;
    };
    let normalized = parent.trim_end_matches(['/', '\\']);
    // Session-host shim layout: a dotted `<name>-<id>/bin` dir shadowed early
    // in PATH. Real installs live in stable locations (`/usr/local/bin`,
    // `~/.local/bin`, …), never in a hex-suffixed shadow dir.
    if normalized.ends_with("/bin") || normalized.ends_with("\\bin") {
        let dir_name = normalized
            .rsplit(['/', '\\'])
            .nth(1)
            .unwrap_or("")
            .trim_start_matches('.');
        if dir_name.contains('-')
            && dir_name
                .rsplit('-')
                .next()
                .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return true;
        }
    }
    std::fs::read(path)
        .ok()
        .map(|bytes| {
            let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
            prefix.contains("agent-wrapper") || prefix.contains("find_real_binary")
        })
        .unwrap_or(false)
}

pub(crate) fn prompt_agent_launch_command(agent: super::action::PromptAgent) -> Option<String> {
    if agent == super::action::PromptAgent::Rovo {
        prompt_agent_command("rovodev").or_else(|| prompt_agent_command("acli"))
    } else {
        agent.cli().and_then(prompt_agent_command)
    }
}

/// [`prompt_agent_launch_command`] against a precomputed [`agent_search_dirs`]
/// set. Detection passes share one set (one shell probe, not one per agent);
/// one-shot checks use the uncached variant above.
pub(crate) fn prompt_agent_launch_command_in(
    agent: super::action::PromptAgent,
    dirs: &[PathBuf],
) -> Option<String> {
    if agent == super::action::PromptAgent::Rovo {
        prompt_agent_command_in("rovodev", dirs)
            .or_else(|| prompt_agent_command_in("acli", dirs))
    } else {
        agent.cli().and_then(|cli| prompt_agent_command_in(cli, dirs))
    }
}

/// Pure defaults: safe to use while rendering settings, without PATH probes.
pub(crate) fn default_agent_command_config(
    agent: super::action::PromptAgent,
) -> crate::setup::settings::AgentCommandConfig {
    let executable = agent.cli().unwrap_or("dragent");
    let (args, stdin) = match agent {
        super::action::PromptAgent::Dragent => ("", false),
        super::action::PromptAgent::Claude => ("--dangerously-skip-permissions", false),
        super::action::PromptAgent::Codex => (
            "--dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust --",
            false,
        ),
        super::action::PromptAgent::Gemini => ("--approval-mode=auto_edit", false),
        super::action::PromptAgent::OpenCode => ("--prompt", false),
        super::action::PromptAgent::Amp => ("", true),
        super::action::PromptAgent::Copilot => ("--allow-tool=write -i", false),
        super::action::PromptAgent::Antigravity => ("--mode accept-edits -i", false),
        super::action::PromptAgent::Cursor => ("", false),
        super::action::PromptAgent::Droid => ("", false),
        super::action::PromptAgent::Grok => ("--always-approve", false),
        super::action::PromptAgent::Hermes => ("chat --yolo -q", false),
        super::action::PromptAgent::Kimi => ("-p", false),
        super::action::PromptAgent::Kiro => ("chat --trust-all-tools", false),
        super::action::PromptAgent::Mistral => ("--trust --auto-approve", false),
        super::action::PromptAgent::Devin => ("--permission-mode dangerous --", false),
        super::action::PromptAgent::Auggie => ("--allow-indexing", false),
        super::action::PromptAgent::Autohand => ("--unrestricted -p", false),
        super::action::PromptAgent::Charm => ("run", false),
        super::action::PromptAgent::Cline => ("--yolo", false),
        super::action::PromptAgent::CodeBuddy => ("--dangerously-skip-permissions", false),
        super::action::PromptAgent::Codebuff => ("", false),
        super::action::PromptAgent::CommandCode => ("--trust --skip-onboarding --yolo", false),
        super::action::PromptAgent::Continue => ("--auto", false),
        super::action::PromptAgent::Freebuff => ("", false),
        // Goose has a distinct non-interactive prompt subcommand. Keeping
        // the prompt after `-t`.
        super::action::PromptAgent::Goose => ("run -s -t", false),
        super::action::PromptAgent::Jules => ("", false),
        super::action::PromptAgent::Junie => ("--task", false),
        super::action::PromptAgent::Kilocode => ("--auto --prompt", false),
        super::action::PromptAgent::Letta => ("--yolo", false),
        super::action::PromptAgent::MiMoCode => ("--prompt", false),
        super::action::PromptAgent::Muse => ("", false),
        super::action::PromptAgent::OhMyPi | super::action::PromptAgent::Pi => ("", false),
        super::action::PromptAgent::PrimeAgent => ("", false),
        super::action::PromptAgent::Qoder => ("--yolo", false),
        super::action::PromptAgent::Qwen => ("--approval-mode=yolo -i", false),
        super::action::PromptAgent::Rovo => ("--yolo", false),
        super::action::PromptAgent::Zero => ("--skip-permissions-unsafe", false),
    };
    let default_command = if args.is_empty() {
        executable.to_string()
    } else {
        format!("{executable} {args}")
    };
    use super::action::PromptAgent;
    let launch_args = match agent {
        PromptAgent::Claude => "--dangerously-skip-permissions",
        PromptAgent::Codex => {
            "--dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust"
        }
        PromptAgent::Gemini => "--approval-mode=auto_edit",
        PromptAgent::Copilot => "--allow-tool=write",
        PromptAgent::Antigravity => "--mode accept-edits",
        PromptAgent::Hermes => "chat --yolo",
        PromptAgent::Kiro => "chat --trust-all-tools",
        PromptAgent::Mistral => "--trust --auto-approve",
        PromptAgent::Devin => "--permission-mode dangerous",
        PromptAgent::Grok => "--always-approve",
        _ => "",
    };
    let suffix = match agent {
        PromptAgent::Kimi => "; kimi --auto --continue",
        PromptAgent::Hermes => "; hermes chat --yolo -c",
        _ => "",
    };
    crate::setup::settings::AgentCommandConfig {
        command: if launch_args.is_empty() {
            executable.to_string()
        } else {
            format!("{executable} {launch_args}")
        },
        prompt_command: default_command,
        prompt_suffix: suffix.to_string(),
        prompt_stdin: stdin,
    }
}

pub(crate) fn agent_command_config(
    agent: super::action::PromptAgent,
    settings: &crate::setup::UiSettings,
) -> crate::setup::settings::AgentCommandConfig {
    if let Some(config) = settings.agent_command_configs.get(agent.command_key()) {
        return apply_agent_autonomy_overlays(agent, config.clone());
    }
    let mut config = default_agent_command_config(agent);
    if let Some(legacy) = settings.agent_commands.get(agent.command_key()) {
        if !legacy.trim().is_empty() {
            config.prompt_command = legacy.clone();
        }
    }
    apply_agent_autonomy_overlays(agent, config)
}

/// Agents visible in pickers and launch paths: the full catalog minus the
/// user's disabled list.
pub(crate) fn agent_picker_agents() -> Vec<super::action::PromptAgent> {
    let autonomy = crate::settings::AppSettings::global().agent.clone();
    super::action::PromptAgent::ALL
        .into_iter()
        .filter(|agent| !autonomy.is_agent_disabled(agent.command_key()))
        .collect()
}

/// Look up a [`PromptAgent`] by its stable command key.
pub(crate) fn prompt_agent_for_key(key: &str) -> Option<super::action::PromptAgent> {
    super::action::PromptAgent::ALL
        .into_iter()
        .find(|agent| agent.command_key() == key)
}

/// The agent new tabs and runs start with: the configured default when it
/// is still enabled, else Claude (the historical default) when enabled,
/// else the first enabled agent.
pub(crate) fn default_prompt_agent() -> super::action::PromptAgent {
    use super::action::PromptAgent;
    let autonomy = crate::settings::AppSettings::global().agent.clone();
    if let Some(key) = autonomy.default_agent.as_deref() {
        if let Some(agent) = prompt_agent_for_key(key) {
            if !autonomy.is_agent_disabled(key) {
                return agent;
            }
        }
    }
    if !autonomy.is_agent_disabled(PromptAgent::Claude.command_key()) {
        return PromptAgent::Claude;
    }
    agent_picker_agents()
        .into_iter()
        .find(|agent| *agent != PromptAgent::Dragent)
        .unwrap_or(PromptAgent::Dragent)
}

/// Whether the untrusted-workspace prompt is enforced. When false, folders
/// open as if trusted.
pub(crate) fn workspace_trust_enforced() -> bool {
    crate::settings::AppSettings::global()
        .agent
        .workspace_trust_enabled
}

/// Layer the Agents settings over a launch config: Codex terminal-server
/// isolation (strip the sandbox bypass flags when isolation is off so Codex
/// runs sandboxed).
fn apply_agent_autonomy_overlays(
    agent: super::action::PromptAgent,
    mut config: crate::setup::settings::AgentCommandConfig,
) -> crate::setup::settings::AgentCommandConfig {
    let autonomy = crate::settings::AppSettings::global().agent.clone();
    if !autonomy.terminal_server_isolation && agent == super::action::PromptAgent::Codex {
        for flag in [
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
        ] {
            config.command = config.command.replace(flag, "");
            config.prompt_command = config.prompt_command.replace(flag, "");
        }
        config.command = config.command.split_whitespace().collect::<Vec<_>>().join(" ");
        config.prompt_command = config
            .prompt_command
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    config
}

pub(crate) fn prompt_agent_interactive_command(
    agent: super::action::PromptAgent,
) -> Option<crate::setup::settings::AgentCommandConfig> {
    if agent == super::action::PromptAgent::Dragent {
        return None;
    }
    // Disabled agents (Settings → Agent → General) never launch.
    if crate::settings::AppSettings::global()
        .agent
        .is_agent_disabled(agent.command_key())
    {
        return None;
    }
    let settings = crate::setup::UiSettings::load();
    let customized = settings
        .agent_command_configs
        .contains_key(agent.command_key())
        || settings.agent_commands.contains_key(agent.command_key());
    if !customized {
        prompt_agent_launch_command(agent)?;
    }
    let config = agent_command_config(agent, &settings);
    config.validate().ok()?;
    Some(config)
}

pub(crate) fn detect_prompt_agents() -> HashSet<super::action::PromptAgent> {
    let mut installed = HashSet::new();
    let settings = crate::setup::UiSettings::load();
    // One shared search set for the whole pass: a single login-shell probe
    // instead of one subprocess per agent.
    let dirs = agent_search_dirs();
    let available = |agent: super::action::PromptAgent| {
        // Custom commands may be aliases, shell functions, or another executable.
        // The configured command is authoritative; do not gate it on the builtin binary.
        let customized = settings
            .agent_command_configs
            .contains_key(agent.command_key())
            || settings.agent_commands.contains_key(agent.command_key());
        agent == super::action::PromptAgent::Dragent
            || if customized {
                agent_command_config(agent, &settings).validate().is_ok()
            } else {
                prompt_agent_launch_command_in(agent, &dirs).is_some()
            }
    };
    // Each lookup can cost a subprocess (`where` on Windows); probing them
    // one after another made a pass take as long as all of them combined.
    const PARALLEL_LOOKUPS: usize = 8;
    for batch in super::action::PromptAgent::ALL.chunks(PARALLEL_LOOKUPS) {
        std::thread::scope(|scope| {
            let lookups: Vec<_> = batch
                .iter()
                .map(|&agent| (agent, scope.spawn(move || available(agent))))
                .collect();
            for (agent, lookup) in lookups {
                if lookup.join().unwrap_or(false) {
                    installed.insert(agent);
                }
            }
        });
    }
    installed
}

impl WorkspaceView {
    pub(crate) fn refresh_prompt_agents(&mut self, ctx: &mut ViewContext<Self>) {
        // A pass can outlast the gap between refresh triggers (tab switches,
        // opening the rail) on a slow machine. Restarting it would discard
        // every result and leave the rail "checking…" indefinitely, so a
        // request made meanwhile runs once after the current pass instead.
        if self.prompt_agent_refresh_in_flight {
            self.prompt_agent_refresh_pending = true;
            return;
        }
        self.start_prompt_agent_refresh(true, ctx);
    }

    fn start_prompt_agent_refresh(&mut self, show_checking: bool, ctx: &mut ViewContext<Self>) {
        self.prompt_agent_refresh_in_flight = true;
        self.prompt_agent_check_generation = self.prompt_agent_check_generation.wrapping_add(1);
        let generation = self.prompt_agent_check_generation;
        if show_checking {
            self.prompt_agent_checking = true;
        }
        ctx.spawn(
            async {
                let settings = crate::setup::UiSettings::load();
                (
                    detect_prompt_agents(),
                    super::agent_definition::detect_custom_agents(&settings),
                    settings.custom_agents,
                )
            },
            move |view, (installed, custom_installed, customs), ctx| {
                view.prompt_agent_refresh_in_flight = false;
                // An install check started meanwhile owns the newer result.
                if view.prompt_agent_check_generation == generation {
                    // Reflect binaries that can actually launch; a placeholder on PATH
                    // must not keep an unavailable agent selectable.
                    #[cfg(not(target_family = "wasm"))]
                    super::cli_workers::remember_installed_agents(&installed);
                    view.prompt_installed_agents = installed;
                    view.custom_installed_agents = custom_installed;
                    view.cached_custom_agents = customs;
                    view.prompt_agent_checking = false;
                    #[cfg(not(target_family = "wasm"))]
                    view.sync_automation_agents(ctx);
                    ctx.notify();
                }
                // The follow-up keeps showing these results while it runs.
                if std::mem::take(&mut view.prompt_agent_refresh_pending) {
                    view.start_prompt_agent_refresh(false, ctx);
                }
            },
        );
    }

    fn open_prompt_agent_install(
        &mut self,
        agent: super::action::PromptAgent,
        ctx: &mut ViewContext<Self>,
    ) {
        let settings = crate::setup::UiSettings::load();
        if settings
            .agent_command_configs
            .contains_key(agent.command_key())
            && agent_command_config(agent, &settings).validate().is_ok()
        {
            self.prompt_agent = agent;
            self.prompt_menu = None;
            ctx.notify();
            return;
        }
        let Some(command) = agent.install_command() else {
            self.close_prompt_agent_install(ctx);
            self.prompt_install_error = Some(format!(
                "{0} does not publish an automatic installer. Install its CLI, then choose Check again.",
                agent.label()
            ));
            self.prompt_install_agent = Some(agent);
            self.prompt_menu = None;
            ctx.notify();
            return;
        };
        self.close_prompt_agent_install(ctx);
        self.prompt_menu = None;
        let (terminal, generation) = self.create_installer_terminal(None, command, ctx);
        // Set after the helper: it closes any previous installer, which
        // clears the subject being replaced.
        self.prompt_install_agent = Some(agent);
        let model = terminal.read(ctx, |view, _| view.terminal_model());
        ctx.subscribe_to_model(&model, move |view, _, event, ctx| {
            if view.prompt_install_generation != generation {
                return;
            }
            if let TerminalEvent::BlockCompleted { context } = event {
                if context.success() {
                    // Success means the agent resolves through the same PATH
                    // detection uses — not merely that the installer exited 0.
                    // The probe is fresh (new login-shell subprocess), so
                    // profile edits from the install are visible immediately.
                    if let Some(agent) = view.prompt_install_agent
                        .filter(|agent| prompt_agent_launch_command(*agent).is_some())
                    {
                        view.prompt_installed_agents.insert(agent);
                        // Reconcile every surface through the startup
                        // mechanism, then close: rail, menus, and customs all
                        // settle on one verified state with no restart.
                        view.refresh_prompt_agents(ctx);
                        view.close_prompt_agent_install(ctx);
                    } else {
                        view.prompt_install_error = Some(not_on_path_message(
                            view.prompt_install_agent
                                .and_then(|agent| agent.cli())
                                .unwrap_or("the agent binary"),
                        ));
                        ctx.notify();
                    }
                } else {
                    view.prompt_install_error = Some(format!(
                        "The installer exited with {}. Fix the terminal output and try again.",
                        context.exit_code.map_or_else(|| "an unknown status".to_string(), |code| code.to_string())
                    ));
                    ctx.notify();
                }
            }
        });
        ctx.notify();
    }

    /// Install a user-provided custom agent through the same embedded-
    /// terminal modal as built-ins. Verification re-probes customs and the
    /// success path lands in `custom_installed_agents`, so the rail flips
    /// to launchable with no restart.
    fn open_custom_agent_install(&mut self, agent_id: String, ctx: &mut ViewContext<Self>) {
        use super::agent_definition::{custom_id, detect_custom_agents};
        let settings = crate::setup::UiSettings::load();
        let Some(custom) = settings
            .custom_agents
            .iter()
            .find(|custom| custom_id(&custom.id) == agent_id)
            .cloned()
        else {
            return;
        };
        let command = custom
            .install_command
            .clone()
            .filter(|command| !command.trim().is_empty());
        let Some(command) = command else {
            self.close_prompt_agent_install(ctx);
            self.custom_install_agent = Some(custom.clone());
            self.prompt_install_error = Some(format!(
                "{} does not publish an automatic installer. Install its command, then choose Check again.",
                custom.name
            ));
            self.prompt_menu = None;
            ctx.notify();
            return;
        };
        self.close_prompt_agent_install(ctx);
        self.prompt_menu = None;
        let (terminal, generation) = self.create_installer_terminal(None, &command, ctx);
        // Set after the helper: it closes any previous installer, which
        // clears the subject being replaced.
        self.custom_install_agent = Some(custom);
        let model = terminal.read(ctx, |view, _| view.terminal_model());
        ctx.subscribe_to_model(&model, move |view, _, event, ctx| {
            if view.prompt_install_generation != generation {
                return;
            }
            if let TerminalEvent::BlockCompleted { context } = event {
                if context.success() {
                    let settings = crate::setup::UiSettings::load();
                    let installed = detect_custom_agents(&settings);
                    let done = view
                        .custom_install_agent
                        .as_ref()
                        .is_some_and(|custom| installed.contains(&custom_id(&custom.id)));
                    if done {
                        view.custom_installed_agents = installed;
                        view.cached_custom_agents = settings.custom_agents;
                        view.close_prompt_agent_install(ctx);
                    } else {
                        view.prompt_install_error = Some(not_on_path_message(
                            view.custom_install_agent
                                .as_ref()
                                .and_then(|custom| {
                                    custom.cli.clone().or_else(|| {
                                        custom
                                            .launch
                                            .command
                                            .split_whitespace()
                                            .next()
                                            .map(str::to_string)
                                    })
                                })
                                .as_deref()
                                .unwrap_or("the agent command"),
                        ));
                        ctx.notify();
                    }
                } else {
                    view.prompt_install_error = Some(format!(
                        "The installer exited with {}. Fix the terminal output and try again.",
                        context.exit_code.map_or_else(|| "an unknown status".to_string(), |code| code.to_string())
                    ));
                    ctx.notify();
                }
            }
        });
        ctx.notify();
    }

    /// Shared embedded-installer terminal: closes any previous installer,
    /// opens a fresh auxiliary pane, and submits the install command.
    /// Callers set their install subject first and subscribe verification.
    /// The install sheet's embedded terminal, running `command` in `cwd`
    /// (home by default). A remote stand-in folder puts it on that machine.
    pub(super) fn create_installer_terminal(
        &mut self,
        cwd: Option<PathBuf>,
        command: &str,
        ctx: &mut ViewContext<Self>,
    ) -> (ViewHandle<TerminalView>, u64) {
        self.close_prompt_agent_install(ctx);
        self.prompt_menu = None;
        self.prompt_install_error = None;
        self.prompt_install_generation = self.prompt_install_generation.wrapping_add(1);
        let generation = self.prompt_install_generation;
        let cwd = cwd.unwrap_or_else(|| {
            dirs::home_dir()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(std::env::temp_dir)
        });
        let remote = crate::remote_fs::resolve(&cwd).is_some();
        #[cfg(not(test))]
        let terminal = match self.create_host_auxiliary_pane(cwd, ctx) {
            super::pane::WorkspacePane::Terminal(terminal) => terminal,
            super::pane::WorkspacePane::Settings(_) | super::pane::WorkspacePane::Browser(_) => {
                unreachable!("installer terminal")
            }
        };
        #[cfg(test)]
        let terminal =
            ctx.add_typed_action_view(|terminal_ctx| TerminalView::new(cwd, None, terminal_ctx));
        self.prompt_install_terminal = Some(terminal.clone());
        let theme = self.theme;
        ctx.update_view(&terminal, |view, terminal_ctx| {
            view.set_theme(theme, terminal_ctx);
            // Whatever this terminal opens goes to the system browser.
            let shell = view
                .launch_shell
                .clone()
                .unwrap_or_else(|| dragent_pty::resolve_shell(None));
            view.use_system_browser(popup_shell_setup(&shell, remote));
            view.submit_shell_command(command, terminal_ctx);
        });
        (terminal, generation)
    }

    /// Run a page's interactive command without changing the active workspace,
    /// saving a tab, or navigating away from the page. Remote aliases preserve
    /// the owning machine through the existing auxiliary terminal runtime.
    pub(crate) fn prompt_context_root(&self) -> Option<PathBuf> {
        #[cfg(not(target_family="wasm"))]
        if let Some(review) = &self.review_setup { return Some(review.target.root.clone()); }
        self.active_project_root()
    }

    pub(crate) fn open_page_command_terminal(
        &mut self,
        title: &str,
        cwd: Option<PathBuf>,
        command: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let location = cwd.as_deref().and_then(crate::remote_fs::resolve)
            .map(|remote| format!("SSH machine {}", remote.machine_id))
            .unwrap_or_else(|| "this computer".into());
        if let Some(root) = &cwd { crate::remote_fs::ensure_stand_in(root); }
        let (terminal, generation) = self.create_installer_terminal(cwd, command, ctx);
        self.page_command = Some((title.into(), format!(
            "Running on {location}. Answer prompts here, then close this window to return to the page."
        )));
        let model = terminal.read(ctx, |view, _| view.terminal_model());
        ctx.subscribe_to_model(&model, move |view, _, event, ctx| {
            if view.prompt_install_generation != generation || view.page_command.is_none() { return; }
            if let TerminalEvent::BlockCompleted { context } = event {
                #[cfg(not(target_family = "wasm"))]
                if context.success() && view.pull_requests_open {
                    ctx.update_view(&view.pull_requests_view, |page,c| page.handle_action(&crate::pull_requests::Action::Refresh,c));
                }
            }
        });
        ctx.focus(&terminal);
        ctx.notify();
    }

    fn check_prompt_agent_install(&mut self, ctx: &mut ViewContext<Self>) {
        use super::agent_definition::{custom_id, detect_custom_agents};
        if self.remote_tool_install.is_some() {
            self.check_remote_tool_install(ctx);
            return;
        }
        // Customs resolve through their own subject first; builtins keep
        // the established path. Either may be absent (stale button).
        if let Some(custom) = self.custom_install_agent.clone() {
            self.prompt_agent_check_generation = self.prompt_agent_check_generation.wrapping_add(1);
            let generation = self.prompt_agent_check_generation;
            let install_generation = self.prompt_install_generation;
            self.prompt_agent_checking = true;
            ctx.spawn(
                async {
                    let settings = crate::setup::UiSettings::load();
                    (
                        detect_custom_agents(&settings),
                        settings.custom_agents,
                    )
                },
                move |view, (installed, customs), ctx| {
                    if view.prompt_agent_check_generation != generation
                        || view.prompt_install_generation != install_generation
                    {
                        return;
                    }
                    view.custom_installed_agents = installed;
                    view.cached_custom_agents = customs;
                    view.prompt_agent_checking = false;
                    if view
                        .custom_installed_agents
                        .contains(&custom_id(&custom.id))
                    {
                        view.close_prompt_agent_install(ctx);
                    } else {
                        view.prompt_install_error = Some(format!(
                            "{} is still not available on PATH. Finish installation in the terminal, then choose Check again.",
                            custom.name
                        ));
                        ctx.notify();
                    }
                },
            );
            return;
        }
        let Some(agent) = self.prompt_install_agent else {
            self.refresh_prompt_agents(ctx);
            return;
        };
        self.prompt_agent_check_generation = self.prompt_agent_check_generation.wrapping_add(1);
        let generation = self.prompt_agent_check_generation;
        let install_generation = self.prompt_install_generation;
        self.prompt_agent_checking = true;
        ctx.spawn(async { detect_prompt_agents() }, move |view, installed, ctx| {
            if view.prompt_agent_check_generation != generation
                || view.prompt_install_generation != install_generation
            {
                return;
            }
            #[cfg(not(target_family = "wasm"))]
            super::cli_workers::remember_installed_agents(&installed);
            view.prompt_installed_agents = installed;
            view.prompt_agent_checking = false;
            #[cfg(not(target_family = "wasm"))]
            view.sync_automation_agents(ctx);
            if view.prompt_installed_agents.contains(&agent) {
                view.close_prompt_agent_install(ctx);
            } else {
                view.prompt_install_error = Some(not_on_path_message(
                    agent.cli().unwrap_or(agent.command_key()),
                ));
                ctx.notify();
            }
        });
    }

    pub(super) fn close_prompt_agent_install(&mut self, ctx: &mut ViewContext<Self>) {
        self.prompt_install_generation = self.prompt_install_generation.wrapping_add(1);
        if let Some(terminal) = self.prompt_install_terminal.take() {
            self.close_host_pane_session(terminal.id());
            ctx.update_view(&terminal, |view, _| view.shutdown());
        }
        let was_page_command = self.page_command.take().is_some();
        let signing_in = self.account_sign_in.take().is_some();
        self.prompt_install_agent = None;
        self.custom_install_agent = None;
        self.remote_tool_install = None;
        self.remote_tool_checking = false;
        self.prompt_install_error = None;
        self.prompt_agent_checking = false;
        if was_page_command {
            if self.settings_open { ctx.focus(&self.settings_view); }
            else { ctx.focus_self(); }
            #[cfg(not(target_family = "wasm"))]
            if self.pull_requests_open {
                ctx.update_view(&self.pull_requests_view, |page,c| page.handle_action(&crate::pull_requests::Action::Refresh,c));
                ctx.focus(&self.pull_requests_view);
            }
        }
        if signing_in {
            ctx.update_view(&self.settings_view, |page, c| page.account_sign_in_closed(c));
        }
        ctx.notify();
    }

    /// The pop-up terminal just opened for an account sign-in: say so in its
    /// heading, and report back to Settings when it closes.
    pub(super) fn describe_account_sign_in(&mut self, provider: &str, ctx: &mut ViewContext<Self>) {
        let Some((_, description)) = &mut self.page_command else {
            // It never opened (the machine went away).
            ctx.update_view(&self.settings_view, |page, c| page.account_sign_in_closed(c));
            return;
        };
        *description = "Finish in your browser. If it shows a code, paste it here. This closes when you're signed in.".into();
        self.account_sign_in = super::PromptAgent::ALL
            .into_iter()
            .find(|agent| agent.command_key() == provider);
        ctx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolver backing detection, install verification, and launch must
    /// agree: a binary in the search set resolves, a missing one does not —
    /// without spawning a login shell (explicit dirs, hermetic).
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn agent_command_in_resolves_explicit_dirs() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("dragent-test-probe-xyz");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let non_executable = temp.path().join("dragent-test-blocked-xyz");
        std::fs::write(&non_executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o644)).unwrap();
        let dirs = vec![temp.path().to_path_buf()];

        assert!(prompt_agent_command_in("dragent-test-probe-xyz", &dirs).is_some());
        assert!(prompt_agent_command_in("dragent-test-blocked-xyz", &dirs).is_none());
        assert!(prompt_agent_command_in("dragent-test-missing-xyz", &dirs).is_none());
        // The search set itself never contains relative entries.
        let mut set = Vec::new();
        push_unique_dir(&mut set, PathBuf::from("relative"));
        push_unique_dir(&mut set, temp.path().to_path_buf());
        push_unique_dir(&mut set, temp.path().to_path_buf());
        assert_eq!(set, vec![temp.path().to_path_buf()]);
    }

    /// Failure messages name the expected executable so a successful install
    /// into an unsearched location is actionable instead of a dead end.
    #[test]
    fn not_on_path_message_names_the_binary() {
        let message = not_on_path_message("mimo");
        assert!(message.contains("mimo"), "unexpected message: {message}");
    }
}

impl WorkspaceView {
    fn clear_prompt_voice_state(&mut self) {
        self.prompt_voice_ui = crate::voice::VoiceUiState::Idle;
        self.prompt_voice_gen = self.prompt_voice_gen.wrapping_add(1);
        self.prompt_voice_session = None;
        self.prompt_voice_fold = Default::default();
        self.prompt_voice_pending_confirm = None;
    }

    fn apply_prompt_voice_snapshot(&mut self, text: &str, is_final: bool, ctx: &mut ViewContext<Self>) {
        let current = self
            .prompt_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let updated = if is_final {
            Some(self.prompt_voice_fold.apply(&current, text, false))
        } else {
            self.prompt_voice_fold.apply_partial(&current, text, false)
        };
        let Some(updated) = updated else {
            self.prompt_notice = Some(format!("Dictating: {text}"));
            return;
        };
        self.prompt_text = updated.clone();
        ctx.update_view(&self.prompt_editor, |editor, editor_ctx| {
            editor.set_text(&updated, editor_ctx);
        });
    }

    fn on_prompt_voice_event(
        &mut self,
        event: crate::voice::VoiceEvent,
        generation: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        if generation != self.prompt_voice_gen {
            return;
        }
        match event {
            crate::voice::VoiceEvent::Live => {
                self.prompt_voice_ui = crate::voice::VoiceUiState::Recording;
            }
            crate::voice::VoiceEvent::Partial(text) => self.apply_prompt_voice_snapshot(&text, false, ctx),
            crate::voice::VoiceEvent::Final(text) => {
                // Confirm-before-insert gate (Settings → Voice): hold the
                // finished dictation for explicit confirmation instead of
                // inserting it. The fold (rebase base) is kept so the later
                // confirm still merges cleanly with interim edits.
                match self.prompt_voice_fold.final_disposition(text) {
                    crate::voice::FinalDisposition::Insert(final_text) => {
                        self.apply_prompt_voice_snapshot(&final_text, true, ctx);
                        self.clear_prompt_voice_state();
                    }
                    crate::voice::FinalDisposition::AwaitingConfirm(final_text) => {
                        self.prompt_voice_session = None;
                        self.prompt_voice_gen = self.prompt_voice_gen.wrapping_add(1);
                        self.prompt_voice_ui = crate::voice::VoiceUiState::Idle;
                        self.prompt_notice = Some(crate::voice::review_notice(&final_text));
                        self.prompt_voice_pending_confirm = Some(final_text);
                    }
                }
            }
            crate::voice::VoiceEvent::Error(message) => {
                self.prompt_notice = Some(message);
                self.clear_prompt_voice_state();
            }
        }
        ctx.notify();
    }

    fn tick_prompt_voice_spinner(&mut self, generation: u64, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            Timer::after(Duration::from_millis(80)),
            move |view, _, ctx| {
                if view.prompt_voice_gen != generation
                    || view.prompt_voice_ui != crate::voice::VoiceUiState::Connecting
                {
                    return;
                }
                view.prompt_voice_spin =
                    (view.prompt_voice_spin + 1) % crate::icons::SPINNER_FRAMES;
                view.tick_prompt_voice_spinner(generation, ctx);
                ctx.notify();
            },
        );
    }

    fn toggle_prompt_voice_input(&mut self, ctx: &mut ViewContext<Self>) {
        use crate::voice::VoiceUiState;
        // A held dictation confirms on the next toggle: insert it now.
        if self.prompt_voice_session.is_none() && self.prompt_voice_ui == VoiceUiState::Idle {
            if let Some(pending) = self.prompt_voice_pending_confirm.take() {
                self.apply_prompt_voice_snapshot(&pending, true, ctx);
                self.prompt_voice_fold = Default::default();
                self.prompt_notice = None;
                ctx.notify();
                return;
            }
        }
        if let Some(session) = self.prompt_voice_session.take() {
            session.stop();
            if self.prompt_voice_ui == VoiceUiState::Connecting {
                self.clear_prompt_voice_state();
            } else {
                let generation = self.prompt_voice_gen;
                ctx.spawn(Timer::after(Duration::from_secs(6)), move |view, _, ctx| {
                    if view.prompt_voice_gen == generation
                        && view.prompt_voice_ui != VoiceUiState::Idle
                    {
                        view.clear_prompt_voice_state();
                        ctx.notify();
                    }
                });
            }
            ctx.notify();
            return;
        }
        if self.prompt_voice_ui != VoiceUiState::Idle {
            return;
        }
        let (tx, rx) = async_channel::unbounded();
        match crate::voice::start(tx) {
            Ok(session) => {
                self.prompt_voice_gen = self.prompt_voice_gen.wrapping_add(1);
                let generation = self.prompt_voice_gen;
                self.prompt_voice_session = Some(session);
                self.prompt_voice_ui = VoiceUiState::Connecting;
                self.prompt_voice_spin = 0;
                self.prompt_notice = None;
                let existing = self
                    .prompt_editor
                    .read(ctx, |editor, _| editor.get_text().to_string());
                self.prompt_voice_fold = crate::voice::fold::VoiceFold::start_with_confirmation(
                    existing, crate::settings::AppSettings::global().voice.confirm_before_insert,
                );
                self.prompt_voice_pending_confirm = None;
                self.tick_prompt_voice_spinner(generation, ctx);
                ctx.spawn_stream_local(
                    rx,
                    move |view, event, ctx| view.on_prompt_voice_event(event, generation, ctx),
                    |_, _| {},
                );
                ctx.spawn(
                    Timer::after(Duration::from_secs(20)),
                    move |view, _, ctx| {
                        if view.prompt_voice_gen == generation
                            && view.prompt_voice_ui == VoiceUiState::Connecting
                        {
                            if let Some(session) = view.prompt_voice_session.take() {
                                session.stop();
                            }
                            view.prompt_notice =
                                Some("The dictation engine did not connect.".into());
                            view.clear_prompt_voice_state();
                            ctx.notify();
                        }
                    },
                );
            }
            Err(message) => self.prompt_notice = Some(message),
        }
        ctx.notify();
    }
}

/// The shell statement giving the pop-up terminal over a page the system
/// browser as `BROWSER`, for a terminal running `shell`. A terminal on a
/// remote machine keeps that machine's own: its system browser isn't the
/// person's (links it shows still open in theirs).
fn popup_shell_setup(shell: &str, remote: bool) -> Option<String> {
    if remote {
        return None;
    }
    #[cfg(not(target_family = "wasm"))]
    {
        dragent_host::terminals::system_browser_entry(
            dragent_shell_hooks::ShellKind::from_program(shell),
            std::env::consts::OS,
        )
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = shell;
        None
    }
}

#[cfg(test)]
mod popup_terminal_tests {
    use super::popup_shell_setup;

    #[test]
    fn the_popup_terminal_opens_pages_in_the_system_browser() {
        let setup = popup_shell_setup("/bin/zsh", false).expect("zsh gets a statement");
        assert!(!setup.contains("dragent"), "{setup}");
        if cfg!(target_os = "macos") {
            assert_eq!(setup, "export BROWSER=open; ");
        } else if cfg!(windows) {
            assert_eq!(setup, "unset BROWSER; ");
        } else {
            assert_eq!(setup, "export BROWSER=xdg-open; ");
        }
        assert!(popup_shell_setup("pwsh", false).is_some());
        // A remote machine keeps its own BROWSER.
        assert_eq!(popup_shell_setup("/bin/zsh", true), None);
    }
}
