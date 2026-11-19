//! Add Project dialog: where the project lives (this Mac or a saved
//! machine), how to start it (existing folder, new repository, clone), and
//! the folder. Remote locations browse the machine's folders in a folder
//! picker. Local projects reuse the existing open / create / clone flows.
use super::project_naming::{clone_target_name, default_projects_dir, valid_project_name};
use super::{InputEditor, InputEditorEvent, InputMode, ProjectFolderIntent, WorkspaceAction};
use super::view::WorkspaceView;
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Segment, Size, Tone, Variant};
use crate::process_ext::NoWindow;
use crate::settings::machines_store::{endpoint_label, MachineStatus, MachinesStore};
use crate::shared_input::{shared_input, SharedInputIcon, SharedInputOpts};
use crate::theme::Theme;
use dragent_host::development::{DevelopmentRequest as Request, DevelopmentResponse as Response, FileEntry};
use std::path::PathBuf;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, EventHandler, Expanded, Flex, MainAxisSize,
    ParentElement, ScrollbarWidth,
};
use warpui::{ReadView, SingletonEntity, UpdateModel, UpdateView, ViewContext, ViewHandle};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::{Cursor, FilePickerConfiguration};
use warpui_core::scene::{Border, CornerRadius, Radius};

const DIALOG_WIDTH: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddProjectMode {
    Pick,
    Create,
    Clone,
}

#[derive(Clone, Debug)]
pub enum BrowserAction {
    Navigate(String),
    Select(String),
    Back,
    Forward,
    Go,
    NewFolder,
    CommitFolder,
    CancelFolder,
    Confirm,
    Cancel,
}

#[derive(Clone, Debug)]
pub enum AddProjectAction {
    Open { machine: Option<String> },
    Close,
    ToggleLocationMenu,
    CloseMenus,
    SelectLocal,
    SelectMachine(String),
    ManageMachines,
    Connect,
    /// Trust the host key the chosen machine presented, then connect.
    TrustHostKey,
    Mode(AddProjectMode),
    ChooseDirectory,
    LocalDirectory(PathBuf),
    Browser(BrowserAction),
    ToggleInitGit,
    Submit,
}

#[derive(Clone, Debug)]
pub(crate) enum Inspection {
    Idle,
    Checking,
    Ready { root: String, git: bool },
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) enum Listing {
    Loading,
    Failed(String),
    Ready(Vec<FileEntry>),
}

pub(crate) struct DirectoryBrowser {
    pub path: String,
    pub history: Vec<String>,
    pub cursor: usize,
    pub listing: Listing,
    pub selected: Option<String>,
    pub creating_folder: bool,
    pub generation: u64,
    /// The folder list's scroll position.
    pub scroll: ClippedScrollStateHandle,
    /// The dialog body's scroll position.
    pub dialog_scroll: ClippedScrollStateHandle,
}

pub(crate) struct AddProjectEditors {
    pub name: ViewHandle<InputEditor>,
    pub url: ViewHandle<InputEditor>,
    pub folder: ViewHandle<InputEditor>,
    pub search: ViewHandle<InputEditor>,
    pub location: ViewHandle<InputEditor>,
    pub new_folder: ViewHandle<InputEditor>,
}

pub(crate) struct AddProjectDialog {
    pub machine: Option<String>,
    pub mode: AddProjectMode,
    /// Picked directory (Pick) or parent directory (Create / Clone).
    pub path: String,
    pub location_menu_open: bool,
    pub inspection: Inspection,
    pub init_git: bool,
    pub busy: Option<&'static str>,
    pub error: Option<String>,
    pub browser: Option<DirectoryBrowser>,
    /// Browse or submit once the selected machine finishes connecting.
    pub after_connect: Option<AddProjectAction>,
    pub generation: u64,
    pub editors: AddProjectEditors,
    pub search_text: String,
    /// The dialog body's scroll position.
    pub scroll: ClippedScrollStateHandle,
}

fn act(action: AddProjectAction) -> WorkspaceAction {
    WorkspaceAction::AddProject(action)
}

fn browse(action: BrowserAction) -> WorkspaceAction {
    act(AddProjectAction::Browser(action))
}

/// Remote paths are POSIX strings owned by the machine, never local paths.
fn join(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn parent_of(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((parent, _)) => parent.into(),
    }
}

fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

impl WorkspaceView {
    pub(crate) fn mouse(&self, key: &str) -> MouseStateHandle {
        self.new_tab_picker_mouse_states
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    fn add_project_editor(&self, placeholder: &str, key: &str, size: f32, ctx: &mut ViewContext<Self>) -> ViewHandle<InputEditor> {
        let (ui, theme) = (self.ui_font, self.theme);
        let placeholder = placeholder.to_string();
        let key = format!("add_project_{key}");
        ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                &placeholder,
                ui,
                crate::theme::text_size(size),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain { key },
                inner.focus_handle(),
            )
        })
    }

    pub(crate) fn handle_add_project_action(&mut self, action: &AddProjectAction, ctx: &mut ViewContext<Self>) {
        use AddProjectAction::*;
        if let Open { machine } = action {
            self.open_add_project(machine.clone(), ctx);
            ctx.notify();
            return;
        }
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        if !matches!(action, ToggleLocationMenu) {
            dialog.location_menu_open = false;
        }
        match action {
            Open { .. } => {}
            Close => {
                if dialog.busy.is_none() && !self.clone_busy {
                    self.add_project = None;
                }
            }
            ToggleLocationMenu => dialog.location_menu_open = !dialog.location_menu_open,
            CloseMenus => {}
            SelectLocal => {
                if dialog.machine.is_some() {
                    dialog.machine = None;
                    dialog.path = if dialog.mode == AddProjectMode::Pick {
                        String::new()
                    } else {
                        default_projects_dir().to_string_lossy().into_owned()
                    };
                    dialog.inspection = Inspection::Idle;
                    dialog.error = None;
                    dialog.browser = None;
                    dialog.after_connect = None;
                }
            }
            SelectMachine(id) => {
                let id = id.clone();
                dialog.machine = Some(id.clone());
                dialog.inspection = Inspection::Idle;
                dialog.error = None;
                dialog.browser = None;
                dialog.after_connect = None;
                dialog.path = String::new();
                self.ensure_add_project_machine(ctx);
                self.reset_remote_default_path(ctx);
            }
            ManageMachines => {
                self.add_project = None;
                self.show_machines(None, ctx);
            }
            Connect => {
                if let Some(id) = dialog.machine.clone() {
                    let store = MachinesStore::handle(ctx);
                    ctx.update_model(&store, |s, c| s.recover(&id, c));
                }
            }
            TrustHostKey => {
                if let Some(id) = dialog.machine.clone() {
                    let store = MachinesStore::handle(ctx);
                    ctx.update_model(&store, |s, c| s.trust_host_key(&id, c));
                }
            }
            Mode(mode) => {
                let local = dialog.machine.is_none();
                dialog.mode = *mode;
                dialog.error = None;
                dialog.inspection = Inspection::Idle;
                dialog.path = String::new();
                if *mode != AddProjectMode::Pick {
                    if local {
                        dialog.path = default_projects_dir().to_string_lossy().into_owned();
                    } else {
                        self.reset_remote_default_path(ctx);
                    }
                }
            }
            ChooseDirectory => self.choose_add_project_directory(ctx),
            LocalDirectory(path) => {
                dialog.path = path.to_string_lossy().into_owned();
                dialog.error = None;
                let generated = basename(&dialog.path);
                let name = dialog.editors.name.clone();
                ctx.update_view(&name, |e, c| e.set_placeholder(&generated, c));
            }
            Browser(browser_action) => self.handle_browser_action(browser_action, ctx),
            ToggleInitGit => dialog.init_git = !dialog.init_git,
            Submit => self.submit_add_project(ctx),
        }
        ctx.notify();
    }

    /// "Connect remote host" and saved SSH hosts on the start screen: Add
    /// Project on that machine, or Settings → Machines to save it first.
    pub(crate) fn add_remote_project(
        &mut self,
        destination: Option<super::ssh::SshDestination>,
        ctx: &mut ViewContext<Self>,
    ) {
        let store = MachinesStore::handle(ctx);
        ctx.update_model(&store, |s, c| s.reload(c));
        let machine = store.read(ctx, |s, _| {
            // Prefer a machine that works now, then one not yet tried; one
            // that just failed is never picked over either.
            let mut machines: Vec<_> = s.registry.machines.iter().collect();
            machines.sort_by_key(|m| {
                let rank = match s.status(&m.id) {
                    MachineStatus::Connected => 0,
                    MachineStatus::Connecting => 1,
                    MachineStatus::Idle => 2,
                    MachineStatus::Error => 3,
                };
                (rank, m.name().to_lowercase())
            });
            match &destination {
                Some(destination) => machines
                    .into_iter()
                    .find(|m| m.connection.label() == destination.display() || m.connection.host == destination.host)
                    .map(|m| m.id.clone()),
                None => {
                    // The default host for new projects wins
                    // when it still exists; otherwise the healthiest machine.
                    let ids: Vec<String> =
                        machines.iter().map(|m| m.id.clone()).collect();
                    if let Some(preferred) =
                        crate::settings::settings_tasks_hosts::default_machine_id(&ids)
                    {
                        // Only honor the preference if that machine is in the
                        // health-sorted list (it always is when it exists).
                        if machines.iter().any(|m| &m.id == &preferred) {
                            return Some(preferred);
                        }
                    }
                    machines.first().map(|m| m.id.clone())
                }
            }
        });
        match machine {
            Some(machine) => self.handle_add_project_action(&AddProjectAction::Open { machine: Some(machine) }, ctx),
            None => {
                self.show_machines(None, ctx);
                let settings = self.settings_view.clone();
                ctx.update_view(&settings, |view, ctx| view.prefill_machine(destination.as_ref(), ctx));
            }
        }
    }

    fn open_add_project(&mut self, machine: Option<String>, ctx: &mut ViewContext<Self>) {
        self.add_project_menu = super::view::AddProjectMenu::Closed;
        self.close_settings(ctx);
        let editors = AddProjectEditors {
            name: self.add_project_editor("Defaults to the folder name", "name", 13.0, ctx),
            url: self.add_project_editor("Enter a repository URL", "url", 13.0, ctx),
            folder: self.add_project_editor("Enter a repository name", "folder", 13.0, ctx),
            search: self.add_project_editor("Search", "search", 12.5, ctx),
            location: self.add_project_editor("Absolute path, or ~/projects", "location", 12.5, ctx),
            new_folder: self.add_project_editor("Untitled folder", "new_folder", 12.5, ctx),
        };
        ctx.subscribe_to_view(&editors.search, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                if let Some(dialog) = view.add_project.as_mut() {
                    dialog.search_text = text.to_lowercase();
                }
                ctx.notify();
            }
        });
        ctx.subscribe_to_view(&editors.url, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                if let Some(dialog) = view.add_project.as_ref() {
                    let generated = clone_target_name(text).unwrap_or_default();
                    let name = dialog.editors.name.clone();
                    ctx.update_view(&name, |e, c| {
                        e.set_placeholder(if generated.is_empty() { "Project name" } else { &generated }, c)
                    });
                }
                ctx.notify();
            }
        });
        ctx.subscribe_to_view(&editors.folder, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                if let Some(dialog) = view.add_project.as_ref() {
                    let name = dialog.editors.name.clone();
                    let text = text.trim().to_string();
                    ctx.update_view(&name, |e, c| {
                        e.set_placeholder(if text.is_empty() { "Project name" } else { &text }, c)
                    });
                }
                ctx.notify();
            }
        });
        let focus = editors.name.clone();
        self.add_project = Some(AddProjectDialog {
            machine: machine.clone(),
            mode: AddProjectMode::Pick,
            path: String::new(),
            location_menu_open: false,
            inspection: Inspection::Idle,
            init_git: true,
            busy: None,
            error: None,
            browser: None,
            after_connect: None,
            generation: 0,
            editors,
            search_text: String::new(),
            scroll: ClippedScrollStateHandle::new(),
        });
        if machine.is_some() {
            self.ensure_add_project_machine(ctx);
        }
        ctx.focus(&focus);
    }

    /// Connect the selected machine if needed; the dialog resumes when the
    /// store reports it ready (see `on_machines_changed`).
    fn ensure_add_project_machine(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(id) = self.add_project.as_ref().and_then(|d| d.machine.clone()) else {
            return;
        };
        let store = MachinesStore::handle(ctx);
        let status = store.read(ctx, |s, _| s.status(&id));
        if matches!(status, MachineStatus::Idle | MachineStatus::Error) {
            ctx.update_model(&store, |s, c| s.connect(&id, c));
        }
    }

    fn reset_remote_default_path(&mut self, ctx: &mut ViewContext<Self>) {
        let store = MachinesStore::handle(ctx);
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        let Some(id) = dialog.machine.clone() else {
            return;
        };
        if dialog.mode == AddProjectMode::Pick {
            return;
        }
        let home = store.read(ctx, |s, _| {
            s.connection(&id)
                .and_then(|c| c.environment.as_ref())
                .map(|e| e.home.clone())
        });
        // Per-host worktree location wins; else the machine home.
        dialog.path = crate::settings::settings_tasks_hosts::worktree_base_for(
            &id,
            home.as_deref(),
        );
    }

    /// Called on every machine store change while the dialog is open.
    pub(crate) fn add_project_machines_changed(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        let Some(id) = dialog.machine.clone() else {
            return;
        };
        let store = MachinesStore::handle(ctx);
        let (ready, status, error, host_key_pending, home) = store.read(ctx, |s, _| {
            (
                s.ready_client(&id).is_some(),
                s.status(&id),
                s.connection(&id).and_then(|c| c.error.clone()),
                // The machine strip asks about an untrusted host key itself.
                s.connection(&id).is_some_and(|c| c.host_key.is_some()),
                s.connection(&id)
                    .and_then(|c| c.environment.as_ref())
                    .map(|e| e.home.clone()),
            )
        });
        if !ready {
            // A pending browse/submit gives up when the connection fails.
            if status == MachineStatus::Error && dialog.after_connect.take().is_some() {
                dialog.busy = None;
                dialog.error = Some(error.unwrap_or_else(|| "Could not connect to the machine.".into()))
                    .filter(|_| !host_key_pending);
                ctx.notify();
            }
            return;
        }
        if dialog.path.is_empty() && dialog.mode != AddProjectMode::Pick {
            dialog.path = crate::settings::settings_tasks_hosts::worktree_base_for(
                &id,
                home.as_deref(),
            );
        }
        if let Some(next) = dialog.after_connect.take() {
            dialog.busy = None;
            self.handle_add_project_action(&next, ctx);
        }
        ctx.notify();
    }

    fn choose_add_project_directory(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        if dialog.machine.is_none() {
            ctx.open_file_picker(
                move |result, ctx| {
                    if let Ok(paths) = result {
                        if let Some(path) = paths.into_iter().next() {
                            ctx.dispatch_typed_action_deferred(act(AddProjectAction::LocalDirectory(
                                PathBuf::from(path),
                            )));
                        }
                    }
                },
                FilePickerConfiguration::new().folders_only(),
            );
            return;
        }
        let id = dialog.machine.clone().unwrap_or_default();
        let store = MachinesStore::handle(ctx);
        let (ready, home) = store.read(ctx, |s, _| {
            (
                s.ready_client(&id).is_some(),
                s.connection(&id)
                    .and_then(|c| c.environment.as_ref())
                    .map(|e| e.home.clone()),
            )
        });
        if !ready {
            dialog.after_connect = Some(AddProjectAction::ChooseDirectory);
            self.ensure_add_project_machine(ctx);
            return;
        }
        let start = if dialog.path.is_empty() {
            home.unwrap_or_else(|| "~".into())
        } else {
            dialog.path.clone()
        };
        dialog.browser = Some(DirectoryBrowser {
            path: start.clone(),
            history: vec![start.clone()],
            cursor: 0,
            listing: Listing::Loading,
            selected: (dialog.mode == AddProjectMode::Pick && !dialog.path.is_empty())
                .then(|| dialog.path.clone()),
            creating_folder: false,
            generation: 0,
            scroll: ClippedScrollStateHandle::new(),
            dialog_scroll: ClippedScrollStateHandle::new(),
        });
        let search = dialog.editors.search.clone();
        ctx.update_view(&search, |e, c| e.set_text("", c));
        self.load_browser(start, false, ctx);
    }

    /// List `path` on the machine. `record` pushes it onto back/forward history.
    fn load_browser(&mut self, path: String, record: bool, ctx: &mut ViewContext<Self>) {
        let store = MachinesStore::handle(ctx);
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        let Some(client) = dialog
            .machine
            .as_ref()
            .and_then(|id| store.read(ctx, |s, _| s.ready_client(id)))
        else {
            return;
        };
        let Some(browser) = dialog.browser.as_mut() else {
            return;
        };
        if record {
            browser.history.truncate(browser.cursor + 1);
            browser.history.push(path.clone());
            browser.cursor = browser.history.len() - 1;
        }
        browser.path = path.clone();
        browser.listing = Listing::Loading;
        browser.creating_folder = false;
        browser.generation += 1;
        let generation = browser.generation;
        let location = dialog.editors.location.clone();
        let search = dialog.editors.search.clone();
        ctx.update_view(&location, |e, c| e.set_text(&path, c));
        ctx.update_view(&search, |e, c| e.set_text("", c));
        ctx.spawn(
            async move {
                // OpenProject resolves "~" and symlinks to the canonical folder.
                let Response::Project { root, .. } = client.development(Request::OpenProject { path })? else {
                    return Err("Could not resolve this folder".to_string());
                };
                let Response::Entries(entries) = client.development(Request::List {
                    root: root.clone(),
                    path: String::new(),
                })?
                else {
                    return Err("Could not list this folder".into());
                };
                Ok((root, entries))
            },
            move |view, result, ctx| {
                let Some(browser) = view
                    .add_project
                    .as_mut()
                    .and_then(|d| d.browser.as_mut())
                    .filter(|b| b.generation == generation)
                else {
                    return;
                };
                match result {
                    Ok((root, entries)) => {
                        if let Some(slot) = browser.history.get_mut(browser.cursor) {
                            *slot = root.clone();
                        }
                        browser.path = root.clone();
                        browser.listing = Listing::Ready(entries);
                        if let Some(location) = view.add_project.as_ref().map(|d| d.editors.location.clone()) {
                            ctx.update_view(&location, |e, c| e.set_text(&root, c));
                        }
                    }
                    Err(e) => browser.listing = Listing::Failed(e),
                }
                ctx.notify();
            },
        );
    }

    fn handle_browser_action(&mut self, action: &BrowserAction, ctx: &mut ViewContext<Self>) {
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        let Some(browser) = dialog.browser.as_mut() else {
            return;
        };
        match action {
            BrowserAction::Navigate(path) => {
                let path = path.clone();
                browser.selected = None;
                self.load_browser(path, true, ctx);
            }
            BrowserAction::Select(path) => {
                browser.selected = if browser.selected.as_deref() == Some(path) {
                    None
                } else {
                    Some(path.clone())
                };
            }
            BrowserAction::Back | BrowserAction::Forward => {
                let next = if matches!(action, BrowserAction::Back) {
                    browser.cursor.checked_sub(1)
                } else {
                    Some(browser.cursor + 1).filter(|c| *c < browser.history.len())
                };
                if let Some(cursor) = next {
                    browser.cursor = cursor;
                    browser.selected = None;
                    let path = browser.history[cursor].clone();
                    self.load_browser(path, false, ctx);
                }
            }
            BrowserAction::Go => {
                let path = dialog.editors.location.read(ctx, |e, _| e.get_text().trim().to_string());
                if path.is_empty() || !(path.starts_with('/') || path.starts_with('~')) {
                    browser.listing = Listing::Failed("Enter an absolute path, such as /home/you/projects or ~/projects.".into());
                } else {
                    browser.selected = None;
                    self.load_browser(path, true, ctx);
                }
            }
            BrowserAction::NewFolder => {
                browser.creating_folder = true;
                let editor = dialog.editors.new_folder.clone();
                ctx.update_view(&editor, |e, c| e.set_text("", c));
                ctx.focus(&editor);
            }
            BrowserAction::CancelFolder => browser.creating_folder = false,
            BrowserAction::CommitFolder => self.create_browser_folder(ctx),
            BrowserAction::Cancel => dialog.browser = None,
            BrowserAction::Confirm => {
                let chosen = if dialog.mode == AddProjectMode::Pick {
                    browser.selected.clone().unwrap_or_else(|| browser.path.clone())
                } else {
                    browser.selected.clone().unwrap_or_else(|| browser.path.clone())
                };
                dialog.browser = None;
                dialog.path = chosen.clone();
                dialog.error = None;
                if dialog.mode == AddProjectMode::Pick {
                    let name = dialog.editors.name.clone();
                    let generated = basename(&chosen);
                    ctx.update_view(&name, |e, c| e.set_placeholder(&generated, c));
                    self.inspect_remote_directory(ctx);
                }
            }
        }
    }

    fn create_browser_folder(&mut self, ctx: &mut ViewContext<Self>) {
        let store = MachinesStore::handle(ctx);
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        let name = dialog.editors.new_folder.read(ctx, |e, _| e.get_text().trim().to_string());
        let Some(browser) = dialog.browser.as_mut() else {
            return;
        };
        browser.creating_folder = false;
        if !valid_project_name(&name) {
            browser.listing = Listing::Failed("Folder names cannot contain path separators.".into());
            return;
        }
        let Some(client) = dialog
            .machine
            .as_ref()
            .and_then(|id| store.read(ctx, |s, _| s.ready_client(id)))
        else {
            return;
        };
        let parent = browser.path.clone();
        ctx.spawn(
            async move {
                client.development(Request::Mkdir {
                    root: parent.clone(),
                    path: name.clone(),
                })?;
                Ok::<_, String>(join(&parent, &name))
            },
            |view, result, ctx| {
                match result {
                    Ok(created) => {
                        let parent = parent_of(&created);
                        if let Some(browser) = view.add_project.as_mut().and_then(|d| d.browser.as_mut()) {
                            browser.selected = Some(created);
                        }
                        view.load_browser(parent, false, ctx);
                        if let Some(browser) = view.add_project.as_mut().and_then(|d| d.browser.as_mut()) {
                            browser.creating_folder = false;
                        }
                    }
                    Err(e) => {
                        if let Some(browser) = view.add_project.as_mut().and_then(|d| d.browser.as_mut()) {
                            browser.listing = Listing::Failed(format!("Could not create folder: {e}"));
                        }
                    }
                }
                ctx.notify();
            },
        );
    }

    fn inspect_remote_directory(&mut self, ctx: &mut ViewContext<Self>) {
        let store = MachinesStore::handle(ctx);
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        let Some(client) = dialog
            .machine
            .as_ref()
            .and_then(|id| store.read(ctx, |s, _| s.ready_client(id)))
        else {
            return;
        };
        dialog.generation += 1;
        let generation = dialog.generation;
        dialog.inspection = Inspection::Checking;
        let path = dialog.path.clone();
        ctx.spawn(
            async move {
                match client.development(Request::OpenProject { path })? {
                    Response::Project { root, git } => Ok((root, git)),
                    _ => Err("Could not inspect this directory.".to_string()),
                }
            },
            move |view, result, ctx| {
                let Some(dialog) = view.add_project.as_mut().filter(|d| d.generation == generation) else {
                    return;
                };
                dialog.inspection = match result {
                    Ok((root, git)) => Inspection::Ready { root, git },
                    Err(e) => Inspection::Failed(e),
                };
                ctx.notify();
            },
        );
    }

    fn submit_add_project(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(dialog) = self.add_project.as_mut() else {
            return;
        };
        if dialog.busy.is_some() || self.clone_busy {
            return;
        }
        let name = dialog.editors.name.read(ctx, |e, _| e.get_text().trim().to_string());
        let url = dialog.editors.url.read(ctx, |e, _| e.get_text().trim().to_string());
        let folder = dialog.editors.folder.read(ctx, |e, _| e.get_text().trim().to_string());
        let mode = dialog.mode;
        let path = dialog.path.clone();
        dialog.error = None;

        let validation = match mode {
            AddProjectMode::Pick if path.is_empty() => Some("Choose a directory."),
            AddProjectMode::Create if !valid_project_name(&folder) => {
                Some("Enter a repository name without path separators.")
            }
            AddProjectMode::Clone if clone_target_name(&url).is_none() => {
                Some("Enter a valid HTTPS or Git SSH repository URL.")
            }
            AddProjectMode::Create | AddProjectMode::Clone if path.is_empty() => {
                Some("Choose a project directory.")
            }
            _ => None,
        };
        if let Some(message) = validation {
            dialog.error = Some(message.into());
            return;
        }

        let Some(machine_id) = dialog.machine.clone() else {
            self.submit_local_project(mode, PathBuf::from(&path), folder, url, ctx);
            return;
        };

        let store = MachinesStore::handle(ctx);
        let Some(client) = store.read(ctx, |s, _| s.ready_client(&machine_id)) else {
            dialog.after_connect = Some(AddProjectAction::Submit);
            dialog.busy = Some("Connecting…");
            self.ensure_add_project_machine(ctx);
            return;
        };
        let init_git = dialog.init_git;
        dialog.busy = Some(match mode {
            AddProjectMode::Clone => "Cloning…",
            _ => "Creating…",
        });
        let display_name = (!name.is_empty()).then_some(name);
        ctx.spawn(
            async move {
                let root = match mode {
                    AddProjectMode::Pick => {
                        let Response::Project { root, git } =
                            client.development(Request::OpenProject { path })?
                        else {
                            return Err("Could not open this directory.".to_string());
                        };
                        if !git && init_git {
                            client.development(Request::Init { path: root.clone() })?;
                        }
                        root
                    }
                    AddProjectMode::Create => {
                        client.development(Request::Mkdir {
                            root: path.clone(),
                            path: folder.clone(),
                        })?;
                        let Response::Project { root, .. } =
                            client.development(Request::Init { path: join(&path, &folder) })?
                        else {
                            return Err("Could not initialize the repository.".into());
                        };
                        root
                    }
                    AddProjectMode::Clone => {
                        let name = clone_target_name(&url).unwrap_or_default();
                        let Response::Project { root, .. } = client.development(Request::Clone {
                            url,
                            parent: path,
                            name,
                        })?
                        else {
                            return Err("Could not clone the repository.".into());
                        };
                        root
                    }
                };
                dragent_host::machine_registry::MachineRegistry::update_at(
                    &dragent_host::machine_registry::MachineRegistry::path(),
                    |registry| registry.add_named_project(&machine_id, root, display_name),
                )
            },
            |view, result, ctx| {
                match result {
                    Ok(project) => {
                        view.add_project = None;
                        let store = MachinesStore::handle(ctx);
                        ctx.update_model(&store, |s, c| s.reload(c));
                        view.open_remote_project(&project.id, ctx);
                    }
                    Err(e) => {
                        if let Some(dialog) = view.add_project.as_mut() {
                            dialog.busy = None;
                            dialog.error = Some(e);
                        }
                    }
                }
                ctx.notify();
            },
        );
    }

    fn submit_local_project(
        &mut self,
        mode: AddProjectMode,
        path: PathBuf,
        folder: String,
        url: String,
        ctx: &mut ViewContext<Self>,
    ) {
        match mode {
            AddProjectMode::Pick => {
                self.add_project = None;
                self.receive_project_folder(path, ProjectFolderIntent::OpenFolder, ctx);
            }
            AddProjectMode::Create => {
                let dir = path.join(&folder);
                let result = if dir.exists() {
                    Err(format!("{} already exists.", dir.display()))
                } else {
                    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))
                };
                if let Err(e) = result {
                    if let Some(dialog) = self.add_project.as_mut() {
                        dialog.error = Some(e);
                    }
                    return;
                }
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
                self.add_project = None;
                self.receive_project_folder(dir, ProjectFolderIntent::OpenFolder, ctx);
            }
            AddProjectMode::Clone => {
                // The existing clone flow reports progress through
                // `clone_busy` / `clone_url_error`, which this dialog shows.
                self.clone_url_error = None;
                self.pending_clone_repository = Some(url);
                self.handle_projects_action(&WorkspaceAction::CloneDestinationSelected { parent: path }, ctx);
            }
        }
    }

    // ── Rendering ──────────────────────────────────────────────────────────

    pub(crate) fn render_add_project(&self, theme: &Theme, app: &warpui::AppContext) -> Option<Box<dyn Element>> {
        let dialog = self.add_project.as_ref()?;
        let store = MachinesStore::handle(app);
        let store = store.as_ref(app);
        if let Some(browser) = &dialog.browser {
            return Some(self.render_directory_browser(theme, dialog, browser, store));
        }
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let busy = dialog.busy.or(self.clone_busy.then_some("Cloning…"));
        let error = dialog.error.clone().or_else(|| {
            (dialog.machine.is_none() && dialog.mode == AddProjectMode::Clone)
                .then(|| self.clone_url_error.clone())
                .flatten()
        });
        let remote = dialog.machine.is_some();

        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(20.0)
            .with_child(mui::field(ui, theme, "Location", self.location_selector(theme, dialog, store), None));

        if let Some(machine) = dialog.machine.as_ref() {
            if let Some(strip) = self.machine_strip(theme, store, machine) {
                body = body.with_child(strip);
            }
        }

        let segment = |mode: AddProjectMode, icon: AppIcon, label: &str| Segment {
            hover: self.mouse(&format!("add_project:mode:{mode:?}")),
            icon: Some(icon),
            label: label.into(),
            selected: dialog.mode == mode,
            action: act(AddProjectAction::Mode(mode)),
        };
        body = body.with_child(mui::field(
            ui,
            theme,
            "Start from",
            mui::segmented(
                theme,
                ui,
                id,
                vec![
                    segment(AddProjectMode::Pick, AppIcon::FolderOpen, "Existing folder"),
                    segment(AddProjectMode::Create, AppIcon::Plus, "New repository"),
                    segment(AddProjectMode::Clone, AppIcon::Download, "Clone"),
                ],
                true,
            ),
            None,
        ));

        let input = |editor: &ViewHandle<InputEditor>| {
            shared_input(
                theme,
                editor,
                SharedInputOpts {
                    height: Some(mui::CONTROL_HEIGHT),
                    radius: mui::RADIUS_CONTROL,
                    pad_left: 11.0,
                    ..Default::default()
                },
            )
        };
        let where_hint = if remote { "On the machine." } else { "On this Mac." };
        match dialog.mode {
            AddProjectMode::Pick => {
                body = body.with_child(mui::field(ui, theme, "Folder", self.directory_field(theme, dialog), None));
                match &dialog.inspection {
                    Inspection::Checking => {
                        body = body.with_child(mui::notice(theme, ui, Tone::Neutral, "Checking the folder…", None, None))
                    }
                    Inspection::Failed(message) => {
                        body = body.with_child(mui::notice(
                            theme,
                            ui,
                            Tone::Error,
                            "Couldn't open this folder",
                            Some(message.clone()),
                            None,
                        ))
                    }
                    Inspection::Ready { git: false, .. } => {
                        body = body.with_child(mui::sunken(
                            theme,
                            mui::settings_row(
                                ui,
                                theme,
                                "Initialize a Git repository",
                                Vec::new(),
                                Some(mui::text(
                                    ui,
                                    "This folder isn't a Git repository yet. Worktrees and Changes need one.",
                                    12.0,
                                    theme.sub_text(),
                                )),
                                Some(mui::switch(
                                    theme,
                                    id,
                                    self.mouse("add_project:init_git"),
                                    dialog.init_git,
                                    act(AddProjectAction::ToggleInitGit),
                                )),
                            ),
                        ))
                    }
                    _ => {}
                }
            }
            AddProjectMode::Create => {
                body = body
                    .with_child(mui::field(ui, theme, "Repository name", input(&dialog.editors.folder), None))
                    .with_child(mui::field(
                        ui,
                        theme,
                        "Create in",
                        self.directory_field(theme, dialog),
                        Some(where_hint),
                    ));
            }
            AddProjectMode::Clone => {
                body = body
                    .with_child(mui::field(
                        ui,
                        theme,
                        "Repository URL",
                        input(&dialog.editors.url),
                        Some("HTTPS or SSH, such as git@github.com:org/repo.git"),
                    ))
                    .with_child(mui::field(
                        ui,
                        theme,
                        "Clone into",
                        self.directory_field(theme, dialog),
                        Some(where_hint),
                    ));
            }
        }
        if remote {
            body = body.with_child(mui::field(
                ui,
                theme,
                "Project name",
                input(&dialog.editors.name),
                Some("Optional."),
            ));
        }
        if let Some(error) = error {
            body = body.with_child(mui::notice(theme, ui, Tone::Error, "Couldn't add the project", Some(error), None));
        }

        let primary = busy.unwrap_or(match dialog.mode {
            AddProjectMode::Pick => "Add project",
            AddProjectMode::Create => "Create repository",
            AddProjectMode::Clone => "Clone repository",
        });
        let footer = mui::dialog_footer(
            None,
            vec![
                mui::button(
                    theme,
                    ui,
                    id,
                    self.mouse("add_project:cancel"),
                    None,
                    "Cancel",
                    Variant::Secondary,
                    Size::Md,
                    busy.is_none(),
                    act(AddProjectAction::Close),
                ),
                mui::button(
                    theme,
                    ui,
                    id,
                    self.mouse("add_project:submit"),
                    None,
                    primary,
                    Variant::Primary,
                    Size::Md,
                    busy.is_none() && !matches!(dialog.inspection, Inspection::Checking | Inspection::Failed(_)),
                    act(AddProjectAction::Submit),
                ),
            ],
        );
        let body = EventHandler::new(body.finish())
            .on_keydown(move |ctx, _, keystroke| match keystroke.key.as_str() {
                "enter" => {
                    ctx.dispatch_typed_action_for(id, act(AddProjectAction::Submit));
                    DispatchEventResult::StopPropagation
                }
                "escape" => {
                    ctx.dispatch_typed_action_for(id, act(AddProjectAction::Close));
                    DispatchEventResult::StopPropagation
                }
                _ => DispatchEventResult::PropagateToParent,
            })
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, act(AddProjectAction::CloseMenus));
                DispatchEventResult::PropagateToParent
            })
            .finish();
        Some(mui::dialog(
            theme,
            ui,
            id,
            self.mouse("add_project:close"),
            &dialog.scroll,
            mui::dialog_title(
                theme,
                ui,
                AppIcon::FolderPlus,
                "Add project",
                Some("Open a folder, start a repository, or clone one."),
            ),
            body,
            Some(footer),
            DIALOG_WIDTH,
            busy.is_none().then(|| act(AddProjectAction::Close)),
        ))
    }

    /// Where the project lives: this Mac or one saved machine, as a select.
    fn location_selector(&self, theme: &Theme, dialog: &AddProjectDialog, store: &MachinesStore) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let selected = dialog.machine.as_ref().and_then(|m| store.machine(m));
        let theme_c = *theme;
        let (label, detail) = match selected {
            Some(machine) => (machine.name(), endpoint_label(machine)),
            None => ("This Mac".to_string(), "Projects on this computer".to_string()),
        };
        let status = dialog.machine.as_ref().map(|m| store.status(m));
        let open = dialog.location_menu_open;
        let trigger = Hoverable::new(self.mouse("add_project:location"), move |state| {
            let leading = match status {
                Some(status) => mui::machine_avatar(&theme_c, status, 24.0),
                None => mui::icon_tile(&theme_c, AppIcon::Monitor, 24.0, None),
            };
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(leading)
                .with_child(
                    Expanded::new(
                        1.0,
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(mui::medium(ui, label.clone(), 13.0, theme_c.main_text()))
                            .with_child(Expanded::new(1.0, mui::line(ui, detail.clone(), 12.0, theme_c.sub_text())).finish())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(render_icon(
                    if open { AppIcon::ChevronUp } else { AppIcon::ChevronDown },
                    theme_c.sub_text(),
                    13.0,
                ))
                .finish();
            Container::new(
                ConstrainedBox::new(Align::new(row).left().finish())
                    .with_height(mui::CONTROL_HEIGHT + 6.0)
                    .finish(),
            )
            .with_background_color(if state.is_hovered() { theme_c.surface_2() } else { theme_c.surface_1() })
            .with_border(Border::all(1.0).with_border_color(if open { theme_c.accent } else { theme_c.border() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CONTROL)))
            .with_padding_left(7.0)
            .with_padding_right(12.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, act(AddProjectAction::ToggleLocationMenu)))
        .finish();
        let menu = dialog.location_menu_open.then(|| {
            let mut rows = vec![mui::menu_row(
                theme,
                ui,
                id,
                self.mouse("add_project:local"),
                Some(render_icon(AppIcon::Monitor, theme.sub_text(), 15.0)),
                "This Mac",
                Some("Projects on this computer"),
                dialog.machine.is_none(),
                false,
                act(AddProjectAction::SelectLocal),
            )];
            rows.push(mui::menu_separator(theme));
            rows.push(mui::menu_heading(ui, theme, "Remote machines"));
            let mut machines: Vec<_> = store.registry.machines.iter().collect();
            machines.sort_by_key(|m| m.name().to_lowercase());
            if machines.is_empty() {
                rows.push(
                    Container::new(mui::text(ui, "No machines yet.", 12.0, theme.sub_text()))
                        .with_padding_left(10.0)
                        .with_padding_top(4.0)
                        .with_padding_bottom(6.0)
                        .finish(),
                );
            }
            for machine in machines {
                let status = store.status(&machine.id);
                let detail = format!("{}  ·  {}", status_word(status), endpoint_label(machine));
                rows.push(mui::menu_row(
                    theme,
                    ui,
                    id,
                    self.mouse(&format!("add_project:machine:{}", machine.id)),
                    Some(mui::status_icon(theme, status, 14.0)),
                    &machine.name(),
                    Some(&detail),
                    dialog.machine.as_deref() == Some(machine.id.as_str()),
                    false,
                    act(AddProjectAction::SelectMachine(machine.id.clone())),
                ));
            }
            rows.push(mui::menu_separator(theme));
            rows.push(mui::menu_row(
                theme,
                ui,
                id,
                self.mouse("add_project:manage"),
                Some(render_icon(AppIcon::Settings, theme.sub_text(), 15.0)),
                "Add or manage machines…",
                None,
                false,
                false,
                act(AddProjectAction::ManageMachines),
            ));
            mui::menu_panel(theme, rows, DIALOG_WIDTH - 2.0 * mui::DIALOG_PAD)
        });
        mui::with_menu(trigger, menu, false)
    }

    /// The chosen machine's connection state, while it isn't ready.
    fn machine_strip(&self, theme: &Theme, store: &MachinesStore, machine: &str) -> Option<Box<dyn Element>> {
        let ui = self.ui_font;
        let status = store.status(machine);
        if status == MachineStatus::Connected {
            return None;
        }
        let name = store.machine(machine).map(|m| m.name()).unwrap_or_default();
        if let Some(key) = store.connection(machine).and_then(|c| c.host_key.as_ref()) {
            return Some(mui::host_key_notice(
                theme,
                ui,
                &name,
                key,
                mui::button(
                    theme,
                    ui,
                    self.workspace_view_id,
                    self.mouse("add_project:trust_key"),
                    None,
                    "Trust and connect",
                    Variant::Primary,
                    Size::Xs,
                    true,
                    act(AddProjectAction::TrustHostKey),
                ),
            ));
        }
        let error = store.connection(machine).and_then(|c| c.error.clone());
        let connect = |label: &str| {
            mui::button(
                theme,
                ui,
                self.workspace_view_id,
                self.mouse("add_project:connect"),
                Some(AppIcon::Plug),
                label,
                Variant::Secondary,
                Size::Xs,
                true,
                act(AddProjectAction::Connect),
            )
        };
        Some(match status {
            MachineStatus::Connecting => mui::notice(
                theme,
                ui,
                Tone::Info,
                &format!("Connecting to {name}…"),
                Some("The first connection installs Dragent's runtime and can take a minute.".into()),
                None,
            ),
            MachineStatus::Error => mui::notice(
                theme,
                ui,
                Tone::Error,
                &format!("Couldn't reach {name}"),
                error,
                Some(connect("Try again")),
            ),
            _ => mui::notice(
                theme,
                ui,
                Tone::Neutral,
                &format!("{name} isn't connected"),
                Some("Connect to browse its folders.".into()),
                Some(connect("Connect")),
            ),
        })
    }

    /// A field showing the chosen folder, opening the picker on click.
    fn directory_field(&self, theme: &Theme, dialog: &AddProjectDialog) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let theme = *theme;
        let path = dialog.path.clone();
        let remote = dialog.machine.is_some();
        Hoverable::new(self.mouse("add_project:directory"), move |state| {
            let empty = path.is_empty();
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(render_icon(AppIcon::Folder, theme.sub_text(), 14.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        mui::line(
                            ui,
                            if empty {
                                if remote { "Choose a folder on the machine" } else { "Choose a folder" }.to_string()
                            } else {
                                path.clone()
                            },
                            13.0,
                            if empty { theme.disabled_text() } else { theme.main_text() },
                        ),
                    )
                    .finish(),
                )
                .with_child(
                    Container::new(mui::medium(ui, "Browse", 12.0, theme.main_text()))
                        .with_background_color(if state.is_hovered() { theme.surface_3() } else { theme.surface_2() })
                        .with_border(Border::all(1.0).with_border_color(theme.outline()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                        .with_padding_left(10.0)
                        .with_padding_right(10.0)
                        .with_padding_top(4.0)
                        .with_padding_bottom(4.0)
                        .finish(),
                )
                .finish();
            Container::new(
                ConstrainedBox::new(Align::new(row).left().finish())
                    .with_height(mui::CONTROL_HEIGHT)
                    .finish(),
            )
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(if state.is_hovered() { theme.border() } else { theme.outline() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CONTROL)))
            .with_padding_left(11.0)
            .with_padding_right(4.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, act(AddProjectAction::ChooseDirectory)))
        .finish()
    }

    /// The remote folder picker: a path field, then a panel with history
    /// buttons, clickable breadcrumbs and search above the folder list.
    fn render_directory_browser(
        &self,
        theme: &Theme,
        dialog: &AddProjectDialog,
        browser: &DirectoryBrowser,
        store: &MachinesStore,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let input = |editor: &ViewHandle<InputEditor>, icon: AppIcon, height: f32, trailing: Option<Box<dyn Element>>| {
            shared_input(
                theme,
                editor,
                SharedInputOpts {
                    leading_icon: Some(SharedInputIcon { icon, size: 13.0 }),
                    height: Some(height),
                    radius: 7.0,
                    pad_left: 10.0,
                    pad_right: if trailing.is_some() { 3.0 } else { 10.0 },
                    trailing,
                    ..Default::default()
                },
            )
        };
        let go = EventHandler::new(input(
            &dialog.editors.location,
            AppIcon::Folder,
            mui::CONTROL_HEIGHT,
            Some(mui::button(
                theme,
                ui,
                id,
                self.mouse("browser:go"),
                None,
                "Go",
                Variant::Ghost,
                Size::Xs,
                true,
                browse(BrowserAction::Go),
            )),
        ))
        .on_keydown(move |ctx, _, keystroke| {
            if keystroke.key == "enter" {
                ctx.dispatch_typed_action_for(id, browse(BrowserAction::Go));
                return DispatchEventResult::StopPropagation;
            }
            DispatchEventResult::PropagateToParent
        })
        .finish();

        let toolbar = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(2.0)
                .with_child(mui::button(theme, ui, id, self.mouse("browser:back"), Some(AppIcon::ChevronLeft), "", Variant::Ghost, Size::Xs, browser.cursor > 0, browse(BrowserAction::Back)))
                .with_child(mui::button(theme, ui, id, self.mouse("browser:forward"), Some(AppIcon::ChevronRight), "", Variant::Ghost, Size::Xs, browser.cursor + 1 < browser.history.len(), browse(BrowserAction::Forward)))
                .with_child(
                    Expanded::new(
                        1.0,
                        Container::new(mui::line(
                            ui,
                            if browser.path == "/" { "/".to_string() } else { basename(&browser.path) },
                            13.5,
                            theme.main_text(),
                        ))
                        .with_padding_left(8.0)
                        .with_padding_right(8.0)
                        .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(input(&dialog.editors.search, AppIcon::Search, mui::CONTROL_HEIGHT_SM, None))
                        .with_width(160.0)
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish();

        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(1.0);
        if browser.creating_folder {
            list = list.with_child(
                Container::new(
                    EventHandler::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(Expanded::new(1.0, input(&dialog.editors.new_folder, AppIcon::FolderPlus, mui::CONTROL_HEIGHT_SM, None)).finish())
                            .with_child(mui::button(theme, ui, id, self.mouse("browser:create"), None, "Create", Variant::Primary, Size::Xs, true, browse(BrowserAction::CommitFolder)))
                            .with_child(mui::button(theme, ui, id, self.mouse("browser:create-cancel"), Some(AppIcon::XClose), "", Variant::Ghost, Size::Xs, true, browse(BrowserAction::CancelFolder)))
                            .finish(),
                    )
                    .on_keydown(move |ctx, _, keystroke| match keystroke.key.as_str() {
                        "enter" => {
                            ctx.dispatch_typed_action_for(id, browse(BrowserAction::CommitFolder));
                            DispatchEventResult::StopPropagation
                        }
                        "escape" => {
                            ctx.dispatch_typed_action_for(id, browse(BrowserAction::CancelFolder));
                            DispatchEventResult::StopPropagation
                        }
                        _ => DispatchEventResult::PropagateToParent,
                    })
                    .finish(),
                )
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_bottom(4.0)
                .finish(),
            );
        }
        let mut folders = 0;
        match &browser.listing {
            Listing::Loading => list = list.with_child(state_row(theme, ui, AppIcon::RefreshCw, "Loading…", false)),
            Listing::Failed(message) => list = list.with_child(state_row(theme, ui, AppIcon::CircleX, message, true)),
            Listing::Ready(entries) => {
                folders = entries.iter().filter(|e| e.directory).count();
                let visible: Vec<&FileEntry> = entries
                    .iter()
                    .filter(|e| dialog.search_text.is_empty() || e.path.to_lowercase().contains(&dialog.search_text))
                    .collect();
                if entries.is_empty() && !browser.creating_folder {
                    list = list.with_child(state_row(theme, ui, AppIcon::Folder, "This folder is empty", false));
                } else if visible.is_empty() && !browser.creating_folder {
                    list = list.with_child(state_row(theme, ui, AppIcon::Search, "No matches", false));
                }
                for entry in visible {
                    list = list.with_child(self.browser_row(theme, browser, entry));
                }
            }
        }
        let list = ConstrainedBox::new(
            ClippedScrollable::vertical(
                browser.scroll.clone(),
                Container::new(list.finish()).with_uniform_padding(6.0).finish(),
                ScrollbarWidth::Auto,
                theme.disabled_text().into(),
                theme.sub_text().into(),
                ColorU::transparent_black().into(),
            )
            .finish(),
        )
        .with_height(300.0)
        .finish();

        let status = match browser.selected.as_deref() {
            Some(selected) => format!("{} selected", basename(selected)),
            None => mui::count(folders, "folder", "folders"),
        };
        // Finder-style path bar: clickable ancestors, and what's selected.
        let status_bar = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.0)
                .with_child(Expanded::new(1.0, self.breadcrumbs(theme, &browser.path)).finish())
                .with_child(mui::text(ui, status, 11.5, theme.sub_text()))
                .finish(),
        )
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .with_padding_left(8.0)
        .with_padding_right(12.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish();

        let panel = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(toolbar)
                .with_child(list)
                .with_child(status_bar)
                .finish(),
        )
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .finish();

        let body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(go)
            .with_child(panel)
            .finish();
        let confirm_label = match (&browser.selected, dialog.mode) {
            (Some(selected), _) => format!("Choose “{}”", basename(selected)),
            (None, AddProjectMode::Pick) => "Use this folder".to_string(),
            (None, _) => "Choose folder".to_string(),
        };
        let footer = mui::dialog_footer(
            Some(mui::button(theme, ui, id, self.mouse("browser:new-folder"), Some(AppIcon::FolderPlus), "New folder", Variant::Secondary, Size::Md, !browser.creating_folder, browse(BrowserAction::NewFolder))),
            vec![
                mui::button(theme, ui, id, self.mouse("browser:cancel"), None, "Back", Variant::Secondary, Size::Md, true, browse(BrowserAction::Cancel)),
                mui::button(
                    theme,
                    ui,
                    id,
                    self.mouse("browser:confirm"),
                    None,
                    &confirm_label,
                    Variant::Primary,
                    Size::Md,
                    matches!(browser.listing, Listing::Ready(_)),
                    browse(BrowserAction::Confirm),
                ),
            ],
        );
        let machine = dialog
            .machine
            .as_ref()
            .and_then(|m| store.machine(m))
            .map(|m| format!("On {}  ·  {}", m.name(), endpoint_label(m)))
            .unwrap_or_default();
        mui::dialog(
            theme,
            ui,
            id,
            self.mouse("browser:close"),
            &browser.dialog_scroll,
            mui::dialog_title(theme, ui, AppIcon::FolderOpen, "Choose a folder", Some(&machine)),
            body,
            Some(footer),
            DIALOG_WIDTH,
            Some(browse(BrowserAction::Cancel)),
        )
    }

    fn browser_row(&self, theme: &Theme, browser: &DirectoryBrowser, entry: &FileEntry) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let theme = *theme;
        let name = entry.path.clone();
        let full = join(&browser.path, &name);
        let directory = entry.directory;
        let hidden = name.starts_with('.');
        let selected = browser.selected.as_deref() == Some(full.as_str());
        let row = Hoverable::new(self.mouse(&format!("browser:row:{full}")), move |state| {
            let (icon, icon_color, color) = if !directory {
                (AppIcon::File, theme.disabled_text(), theme.disabled_text())
            } else if selected {
                (AppIcon::FolderOpen, theme.accent, theme.main_text())
            } else {
                (AppIcon::Folder, if hidden { theme.disabled_text() } else { theme.accent }, if hidden { theme.sub_text() } else { theme.main_text() })
            };
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(mui::sized(render_icon(icon, icon_color, 15.0), 18.0, 18.0))
                .with_child(Expanded::new(1.0, if selected {
                    mui::medium(ui, name.clone(), 13.0, color)
                } else {
                    mui::line(ui, name.clone(), 13.0, color)
                }).finish());
            if directory && (selected || state.is_hovered()) {
                row = row.with_child(render_icon(AppIcon::ChevronRight, theme.sub_text(), 12.0));
            }
            Container::new(row.finish())
                .with_background_color(if selected {
                    theme.accent_overlay()
                } else if directory && state.is_hovered() {
                    theme.sidebar_row_hover()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(8.0)
                .with_padding_right(10.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish()
        });
        if !directory {
            return row.finish();
        }
        let select = full.clone();
        row.with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, browse(BrowserAction::Select(select.clone()))))
            .on_double_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, browse(BrowserAction::Navigate(full.clone()))))
            .finish()
    }

    /// Clickable path segments. Deep paths keep the root and the last two,
    /// and long names are shortened, so the bar never outgrows the dialog.
    fn breadcrumbs(&self, theme: &Theme, path: &str) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(3.0);
        let mut current = String::new();
        let mut parts = vec![("/".to_string(), "/".to_string())];
        for part in path.split('/').filter(|p| !p.is_empty()) {
            current = format!("{current}/{part}");
            parts.push((part.to_string(), current.clone()));
        }
        let last = parts.len() - 1;
        let elided = parts.len() > 4;
        for (index, (label, target)) in parts.into_iter().enumerate() {
            let label = if label.chars().count() > 22 {
                format!("{}…", label.chars().take(21).collect::<String>())
            } else {
                label
            };
            if elided && index > 0 && index < last - 1 {
                if index == 1 {
                    row = row
                        .with_child(render_icon(AppIcon::ChevronRight, theme.disabled_text(), 10.0))
                        .with_child(mui::text(ui, "…", 12.0, theme.disabled_text()));
                }
                continue;
            }
            if index > 0 {
                row = row.with_child(render_icon(AppIcon::ChevronRight, theme.disabled_text(), 10.0));
            }
            let theme_c = *theme;
            let is_last = index == last;
            row = row.with_child(
                Hoverable::new(self.mouse(&format!("browser:crumb:{target}")), move |state| {
                    Container::new(if is_last {
                        mui::medium(ui, label.clone(), 11.5, theme_c.main_text())
                    } else {
                        mui::text(ui, label.clone(), 11.5, if state.is_hovered() { theme_c.main_text() } else { theme_c.sub_text() })
                    })
                    .with_background_color(if state.is_hovered() && !is_last { theme_c.surface_2() } else { ColorU::transparent_black() })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .with_padding_left(4.0)
                    .with_padding_right(4.0)
                    .with_padding_top(2.0)
                    .with_padding_bottom(2.0)
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, browse(BrowserAction::Navigate(target.clone()))))
                .finish(),
            );
        }
        row.finish()
    }
}

/// "Connected", "Not connected"… for menu detail lines.
fn status_word(status: MachineStatus) -> &'static str {
    match status {
        MachineStatus::Connected => "Connected",
        MachineStatus::Connecting => "Connecting",
        MachineStatus::Error => "Unreachable",
        MachineStatus::Idle => "Not connected",
    }
}

fn state_row(theme: &Theme, ui: warpui::fonts::FamilyId, icon: AppIcon, message: &str, error: bool) -> Box<dyn Element> {
    let color = if error { theme.ui_red() } else { theme.sub_text() };
    Container::new(
        Align::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(10.0)
                .with_child(render_icon(icon, if error { color } else { theme.disabled_text() }, 20.0))
                .with_child(mui::text(ui, message, 12.5, color))
                .finish(),
        )
        .finish(),
    )
    .with_padding_top(64.0)
    .with_padding_bottom(64.0)
    .finish()
}

#[cfg(test)]
mod tests {
    use super::{basename, join, parent_of};

    #[test]
    fn remote_paths_join_and_split_as_posix() {
        assert_eq!(join("/home/dev", "app"), "/home/dev/app");
        assert_eq!(join("/", "srv"), "/srv");
        assert_eq!(parent_of("/home/dev/app"), "/home/dev");
        assert_eq!(parent_of("/srv"), "/");
        assert_eq!(basename("/home/dev/app/"), "app");
    }
}
