//! Settings → Machines: a searchable list of saved SSH machines, an "Add SSH
//! Connection" dialog, and a per-machine page with System / Workspaces /
//! Sessions / Agents tabs. Connection state lives in [`MachinesStore`];
//! adding a project happens in the workspace's Add Project dialog, never here.
use super::machines_store::{MachineConnection, MachinesStore, RuntimeCommand};
use super::tool_catalog::{self, JobKind, ToolJob};
use dragent_host::development::ToolPin;
use super::{SettingsEvent, SettingsView};
use crate::{
    theme::Theme,
    workspace::{InputEditor, InputEditorEvent, InputMode},
};
use dragent_host::remote::{Authentication, ResolvedSsh, SshHost};
use std::collections::HashMap;
use std::time::Instant;
use warpui::fonts::FamilyId;
use warpui::elements::ClippedScrollStateHandle;
use warpui::{ReadView, UpdateModel, UpdateView, ViewContext, ViewHandle};

mod detail;
mod form;
mod list;
mod tool_sheet;

pub(crate) use detail::path_within;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineTab {
    System,
    Workspaces,
    Sessions,
    Agents,
}

#[derive(Clone, Debug)]
pub enum MachineAction {
    ShowList,
    Open(String),
    Tab(MachineTab),
    Add,
    Edit(String),
    Connect(String),
    /// Trust the host key the machine presented, then connect.
    TrustHostKey(String),
    Disconnect(String),
    ToggleMenu,
    CloseMenus,
    StartRename,
    CommitRename,
    CancelRename,
    RequestDelete,
    ConfirmDelete,
    CancelConfirm,
    ToggleRuntimeMenu,
    Runtime(RuntimeCommand),
    CloseForm,
    FormMode { config: bool },
    ToggleHostMenu,
    PickHost(String),
    Resolve,
    ToggleAuthMenu,
    Auth(Authentication),
    ToggleAdvanced,
    ToggleForward,
    Test,
    /// Trust the host key the connection test found, then test again.
    TrustTestedHostKey,
    Submit,
    ToggleTestLogs,
    AddProject(String),
    OpenProject(String),
    RemoveProject(String),
    AttachSession(String),
    CloseSession(String),
    /// Install with the recommended method.
    InstallTool(String),
    /// Install with the tool's `methods()[index]`.
    InstallWith { tool: String, method: usize },
    UpdateTool(String),
    UninstallTool(String),
    /// Every missing system tool, in one go.
    InstallAllTools,
    /// Open the tool sheet: status, copies, install methods.
    ShowTool(String),
    CloseTool,
    /// Use this copy (`None`: the first one on the PATH).
    UseCopy { tool: String, path: Option<String> },
    /// Use the command typed in the tool sheet.
    UseToolCommand(String),
    CheckTools,
    OpenToolDocs(String),
    AddAgent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MachinePage {
    List,
    Detail(String),
}

/// OpenSSH resolution of an SSH-config alias (the "Resolve" step that turns
/// a short alias into a concrete host before connecting).
#[derive(Clone, Debug)]
pub(crate) enum Resolution {
    Idle,
    Resolving,
    Resolved { alias: String, host: ResolvedSsh },
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) enum TestState {
    Idle,
    Testing,
    Passed { latency_ms: u128 },
    Failed { message: String, logs: Vec<String> },
    /// Reached a machine whose host key nothing trusts yet.
    NewHostKey(dragent_host::remote::UnknownHostKey),
}

pub(crate) struct MachineForm {
    pub id: Option<String>,
    pub config_mode: bool,
    pub auth: Authentication,
    pub auth_menu_open: bool,
    pub host_menu_open: bool,
    pub advanced: bool,
    pub forward: bool,
    pub resolution: Resolution,
    pub test: TestState,
    pub show_logs: bool,
    pub saving: bool,
    pub save_error: Option<String>,
    pub generation: u64,
    /// The dialog body's scroll position.
    pub scroll: ClippedScrollStateHandle,
}

impl MachineForm {
    /// Manual mode is always ready; SSH-config mode once its alias resolved.
    pub fn ready(&self, alias: &str) -> bool {
        !self.config_mode
            || matches!(&self.resolution, Resolution::Resolved { alias: resolved, .. } if resolved == alias)
    }
}

pub(crate) enum Confirmation {
    Delete(String),
    DeleteBlocked,
}

pub(crate) struct MachinesState {
    pub page: MachinePage,
    pub tab: MachineTab,
    pub form: Option<MachineForm>,
    pub menu_open: bool,
    pub runtime_menu_open: bool,
    pub renaming: bool,
    pub confirm: Option<Confirmation>,
    pub confirm_scroll: ClippedScrollStateHandle,
    pub fields: HashMap<&'static str, ViewHandle<InputEditor>>,
    pub query: String,
    pub notice: Option<String>,
    /// The tool whose sheet is open (status, copies, install methods).
    pub tool_sheet: Option<String>,
    pub tool_sheet_scroll: ClippedScrollStateHandle,
    sequence: u64,
}

const FIELDS: &[(&str, &str)] = &[
    ("search", "Search machines…"),
    ("name", "Staging server"),
    ("alias", "Choose a host or type a hostname"),
    ("host", "203.0.113.10"),
    ("port", "22"),
    ("user", "ubuntu"),
    ("key", "~/.ssh/id_ed25519"),
    ("passphrase", "Key passphrase"),
    ("password", "Password"),
    ("jump", "bastion or user@bastion:2222"),
    ("config", "~/.ssh/config"),
    ("rename", "Machine name"),
    ("agent_name", "Agent name"),
    ("agent_command", "e.g. aider --model sonnet"),
    ("tool_command", "e.g. npx -y @openai/codex"),
];

impl MachinesState {
    pub fn new(ctx: &mut ViewContext<SettingsView>, ui: FamilyId, theme: Theme) -> Self {
        let mut fields = HashMap::new();
        for (key, placeholder) in FIELDS {
            let large = *key == "rename";
            let handle = ctx.add_typed_action_view(|inner| {
                let mut editor = InputEditor::new(
                    placeholder,
                    ui,
                    crate::theme::text_size(if large { 16.0 } else { 13.0 }),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Plain {
                        key: format!("machines_{key}"),
                    },
                    inner.focus_handle(),
                );
                if matches!(*key, "passphrase" | "password") {
                    editor.set_secure(true, inner);
                }
                editor
            });
            match *key {
                "search" => ctx.subscribe_to_view(&handle, |view, _, event, ctx| {
                    if let InputEditorEvent::Changed { text, .. } = event {
                        view.machines.query = text.clone();
                        ctx.notify();
                    }
                }),
                // Editing the alias invalidates its resolution and reopens
                // the suggestions, exactly like a combobox.
                "alias" => ctx.subscribe_to_view(&handle, |view, _, event, ctx| {
                    if let InputEditorEvent::Changed { text, .. } = event {
                        if let Some(form) = view.machines.form.as_mut() {
                            let resolved = matches!(&form.resolution, Resolution::Resolved { alias, .. } if alias == text);
                            if !resolved {
                                form.resolution = Resolution::Idle;
                                form.test = TestState::Idle;
                            }
                            form.host_menu_open = !text.is_empty() || form.host_menu_open;
                        }
                        ctx.notify();
                    }
                }),
                "host" | "user" | "port" | "key" => {
                    ctx.subscribe_to_view(&handle, |view, _, event, ctx| {
                        if let InputEditorEvent::Changed { .. } = event {
                            if let Some(form) = view.machines.form.as_mut() {
                                form.test = TestState::Idle;
                            }
                            ctx.notify();
                        }
                    })
                }
                _ => {}
            }
            fields.insert(*key, handle);
        }
        Self {
            page: MachinePage::List,
            tab: MachineTab::System,
            form: None,
            menu_open: false,
            runtime_menu_open: false,
            renaming: false,
            confirm: None,
            confirm_scroll: ClippedScrollStateHandle::new(),
            fields,
            query: String::new(),
            notice: None,
            tool_sheet: None,
            tool_sheet_scroll: ClippedScrollStateHandle::new(),
            sequence: 0,
        }
    }

    pub fn value(&self, key: &str, ctx: &impl ReadView) -> String {
        self.fields[key].read(ctx, |e, _| e.get_text().trim().to_string())
    }

    fn set(&self, key: &str, value: &str, ctx: &mut ViewContext<SettingsView>) {
        let editor = self.fields[key].clone();
        ctx.update_view(&editor, |e, c| e.set_text(value, c));
    }

    fn focus(&self, key: &str, ctx: &mut ViewContext<SettingsView>) {
        let editor = self.fields[key].clone();
        ctx.focus(&editor);
    }

    /// The SSH destination described by the open form.
    fn draft_host(&self, ctx: &ViewContext<SettingsView>) -> Result<SshHost, String> {
        let form = self.form.as_ref().ok_or("Open the machine form first")?;
        let (host, user, port) = if form.config_mode {
            let alias = self.value("alias", ctx);
            if alias.is_empty() {
                return Err("Choose a host from your SSH config, or enter a hostname.".into());
            }
            (alias, None, None)
        } else {
            let host = self.value("host", ctx);
            if host.is_empty() {
                return Err("Enter the machine's host.".into());
            }
            let user = self.value("user", ctx);
            let port = self.value("port", ctx);
            let port = if port.is_empty() {
                22
            } else {
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or("Port must be between 1 and 65535.")?
            };
            (host, (!user.is_empty()).then_some(user), Some(port))
        };
        let optional_path = |key: &str| {
            let value = self.value(key, ctx);
            (!value.is_empty()).then(|| dragent_host::remote::expand_key_path(&value))
        };
        let jump = self.value("jump", ctx);
        let connection = SshHost {
            name: self.value("name", ctx),
            host,
            user,
            port,
            identity_file: (form.auth == Authentication::Key)
                .then(|| optional_path("key"))
                .flatten(),
            proxy_jump: (!form.config_mode && !jump.is_empty()).then_some(jump),
            config_file: optional_path("config"),
            authentication: form.auth.clone(),
            forward_agent: !form.config_mode && form.forward,
        };
        connection.validate()?;
        Ok(connection)
    }

    fn secret(&self, ctx: &ViewContext<SettingsView>) -> Option<String> {
        let form = self.form.as_ref()?;
        let key = match form.auth {
            Authentication::Key => "passphrase",
            Authentication::Password => "password",
            Authentication::Agent => return None,
        };
        let secret = self.fields[key].read(ctx, |e, _| e.get_text().to_string());
        (!secret.is_empty()).then_some(secret)
    }
}

impl SettingsView {
    pub(crate) fn machines_store(&self, app: &warpui::AppContext) -> warpui::ModelHandle<MachinesStore> {
        use warpui::SingletonEntity;
        MachinesStore::handle(app)
    }

    /// The machine whose page is open.
    pub(crate) fn machine_id(&self) -> Option<String> {
        match &self.machines.page {
            MachinePage::Detail(id) => Some(id.clone()),
            MachinePage::List => None,
        }
    }

    /// Read the open machine's connection.
    pub(crate) fn with_machine<R>(
        &self,
        app: &warpui::AppContext,
        read: impl FnOnce(&MachineConnection) -> Option<R>,
    ) -> Option<R> {
        let id = self.machine_id()?;
        self.machines_store(app).read(app, |store, _| store.connection(&id).and_then(read))
    }

    fn machine_os(&self, app: &warpui::AppContext) -> Option<tool_catalog::Os> {
        self.with_machine(app, |connection| Some(connection.os()))
    }

    fn run_tool_job(&mut self, job: ToolJob, ctx: &mut ViewContext<Self>) {
        if let Some(machine_id) = self.machine_id() {
            ctx.emit(SettingsEvent::RunToolJob { machine_id, job });
        }
    }

    fn pin_tool(&mut self, tool: &str, pin: Option<ToolPin>, ctx: &mut ViewContext<Self>) {
        if let Some(id) = self.machine_id() {
            let store = self.machines_store(ctx);
            ctx.update_model(&store, |s, c| s.pin_tool(&id, tool, pin, c));
        }
    }

    pub(crate) fn sync_machine_inputs(&mut self, ctx: &mut ViewContext<Self>) {
        let theme = self.theme();
        let font = self.ui_font;
        for (key, editor) in self.machines.fields.iter() {
            let size = if matches!(*key, "name" | "rename") { 18.0 } else { 13.0 };
            ctx.update_view(editor, |editor, ctx| {
                editor.set_theme(theme, ctx);
                editor.set_font(font, crate::theme::scaled(size), ctx);
                editor.set_menu_font(font, ctx);
            });
        }
    }

    /// Entering Settings → Machines re-reads saved machines from disk.
    pub(crate) fn refresh_machines(&mut self, ctx: &mut ViewContext<Self>) {
        #[cfg(target_os = "windows")]
        {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(super::task_settings::refresh_wsl_distros());
            });
            ctx.spawn(async move { rx.recv().await }, |_, _, ctx| ctx.notify());
        }
        let store = self.machines_store(ctx);
        ctx.update_model(&store, |store, ctx| store.reload(ctx));
        if let MachinePage::Detail(id) = &self.machines.page {
            let id = id.clone();
            ctx.update_model(&store, |store, ctx| store.refresh(&id, ctx));
        }
        ctx.notify();
    }

    /// Open "Add SSH Connection" for a destination typed elsewhere.
    pub(crate) fn prefill_machine(
        &mut self,
        destination: Option<&crate::workspace::ssh::SshDestination>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.handle_machine_action(&MachineAction::Add, ctx);
        let Some(destination) = destination else {
            return;
        };
        if destination.user.is_none() && destination.port.is_none() {
            self.machines.set("alias", &destination.host, ctx);
            self.resolve_alias(ctx);
        } else {
            if let Some(form) = self.machines.form.as_mut() {
                form.config_mode = false;
            }
            self.machines.set("host", &destination.host, ctx);
            self.machines
                .set("user", destination.user.as_deref().unwrap_or_default(), ctx);
            if let Some(port) = destination.port {
                self.machines.set("port", &port.to_string(), ctx);
            }
        }
        ctx.notify();
    }

    /// Jump straight to one machine's page (from a project or the sidebar).
    pub(crate) fn open_machine(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        self.handle_machine_action(&MachineAction::Open(id.to_string()), ctx);
    }

    pub(crate) fn handle_machine_action(
        &mut self,
        action: &MachineAction,
        ctx: &mut ViewContext<Self>,
    ) {
        use MachineAction::*;
        let store = self.machines_store(ctx);
        if !matches!(action, ToggleMenu | ToggleRuntimeMenu | ToggleHostMenu | ToggleAuthMenu) {
            self.machines.menu_open = false;
            self.machines.runtime_menu_open = false;
        }
        match action {
            ShowList => {
                self.machines.page = MachinePage::List;
                self.machines.renaming = false;
                self.machines.notice = None;
                self.machines.tool_sheet = None;
                ctx.update_model(&store, |store, _| store.set_watched(None));
            }
            Open(id) => {
                self.machines.page = MachinePage::Detail(id.clone());
                self.machines.tool_sheet = None;
                let watched = id.clone();
                ctx.update_model(&store, |store, _| store.set_watched(Some(watched)));
                self.machines.tab = MachineTab::System;
                self.machines.renaming = false;
                self.machines.notice = None;
                self.sync_host_override_editors(&id, ctx);
                let id = id.clone();
                ctx.update_model(&store, |store, ctx| store.refresh(&id, ctx));
            }
            Tab(tab) => {
                self.machines.tab = *tab;
                if let MachinePage::Detail(id) = &self.machines.page {
                    let id = id.clone();
                    ctx.update_model(&store, |store, ctx| store.refresh(&id, ctx));
                }
            }
            Add | Edit(_) => self.open_machine_form(action, ctx),
            Connect(id) => {
                let id = id.clone();
                ctx.update_model(&store, |store, ctx| store.recover(&id, ctx));
            }
            TrustHostKey(id) => {
                let id = id.clone();
                ctx.update_model(&store, |store, ctx| store.trust_host_key(&id, ctx));
            }
            Disconnect(id) => {
                let id = id.clone();
                ctx.update_model(&store, |store, ctx| store.disconnect(&id, ctx));
                ctx.emit(SettingsEvent::MachinesChanged);
            }
            ToggleMenu => self.machines.menu_open = !self.machines.menu_open,
            ToggleRuntimeMenu => {
                self.machines.runtime_menu_open = !self.machines.runtime_menu_open
            }
            CloseMenus => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.auth_menu_open = false;
                    form.host_menu_open = false;
                }
            }
            StartRename => {
                if let MachinePage::Detail(id) = &self.machines.page {
                    let name = store.read(ctx, |s, _| s.machine(id).map(|m| m.name()));
                    if let Some(name) = name {
                        self.machines.renaming = true;
                        self.machines.set("rename", &name, ctx);
                        self.machines.focus("rename", ctx);
                    }
                }
            }
            CancelRename => self.machines.renaming = false,
            CommitRename => self.commit_rename(ctx),
            RequestDelete => {
                if let MachinePage::Detail(id) = &self.machines.page {
                    let used = store.read(ctx, |s, _| !s.projects_for(id).is_empty());
                    self.machines.confirm = Some(if used {
                        Confirmation::DeleteBlocked
                    } else {
                        Confirmation::Delete(id.clone())
                    });
                }
            }
            CancelConfirm => self.machines.confirm = None,
            ConfirmDelete => self.delete_machine(ctx),
            Runtime(command) => {
                if let MachinePage::Detail(id) = &self.machines.page {
                    let (id, command) = (id.clone(), *command);
                    ctx.update_model(&store, |store, ctx| {
                        store.runtime_command(&id, command, ctx)
                    });
                }
            }
            CloseForm => {
                self.machines.sequence += 1;
                self.machines.form = None;
                self.machines.set("passphrase", "", ctx);
                self.machines.set("password", "", ctx);
            }
            FormMode { config } => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.config_mode = *config;
                    form.test = TestState::Idle;
                    form.host_menu_open = false;
                }
            }
            ToggleHostMenu => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.host_menu_open = !form.host_menu_open;
                    form.auth_menu_open = false;
                }
            }
            PickHost(alias) => {
                self.machines.set("alias", alias, ctx);
                if let Some(form) = self.machines.form.as_mut() {
                    form.host_menu_open = false;
                }
                self.resolve_alias(ctx);
            }
            Resolve => self.resolve_alias(ctx),
            ToggleAuthMenu => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.auth_menu_open = !form.auth_menu_open;
                    form.host_menu_open = false;
                }
            }
            Auth(auth) => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.auth = auth.clone();
                    form.auth_menu_open = false;
                    form.test = TestState::Idle;
                }
            }
            ToggleAdvanced => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.advanced = !form.advanced;
                }
            }
            ToggleForward => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.forward = !form.forward;
                }
            }
            ToggleTestLogs => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.show_logs = !form.show_logs;
                }
            }
            Test => self.test_machine_form(ctx),
            TrustTestedHostKey => self.trust_tested_host_key(ctx),
            Submit => self.submit_machine_form(ctx),
            AddProject(machine_id) => ctx.emit(SettingsEvent::AddRemoteProject(machine_id.clone())),
            OpenProject(project_id) => {
                ctx.emit(SettingsEvent::OpenRemoteProject(project_id.clone()))
            }
            RemoveProject(project_id) => {
                ctx.emit(SettingsEvent::RemoveRemoteProject(project_id.clone()))
            }
            AttachSession(session_id) => {
                if let MachinePage::Detail(id) = &self.machines.page {
                    let session = store.read(ctx, |s, _| {
                        s.connection(id)
                            .and_then(|c| c.sessions.iter().find(|s| &s.id == session_id).cloned())
                    });
                    if let Some(session) = session {
                        ctx.emit(SettingsEvent::AttachRemoteSession {
                            machine_id: id.clone(),
                            session,
                        });
                    }
                }
            }
            CloseSession(session_id) => self.close_remote_session(session_id, ctx),
            InstallTool(tool) => {
                let method = self.machine_os(ctx).and_then(|os| tool_catalog::recommended(tool, os));
                if let Some(method) = method {
                    self.run_tool_job(ToolJob::new(JobKind::Install, vec![tool.clone()], method.install, method.may_elevate), ctx);
                }
            }
            InstallWith { tool, method } => {
                let method = self
                    .machine_os(ctx)
                    .and_then(|os| tool_catalog::methods(tool, os).into_iter().nth(*method));
                if let Some(method) = method {
                    self.run_tool_job(ToolJob::new(JobKind::Install, vec![tool.clone()], method.install, method.may_elevate), ctx);
                }
            }
            UpdateTool(tool) | UninstallTool(tool) => {
                let update = matches!(action, UpdateTool(_));
                let command = self.with_machine(ctx, |connection| {
                    let report = connection.reports.get(tool)?;
                    if update {
                        tool_catalog::update_command(tool, report, connection.os())
                    } else {
                        tool_catalog::uninstall_command(tool, report, connection.os())
                    }
                });
                if let Some((command, may_elevate)) = command {
                    let kind = if update { JobKind::Update } else { JobKind::Uninstall };
                    self.run_tool_job(ToolJob::new(kind, vec![tool.clone()], command, may_elevate), ctx);
                }
            }
            InstallAllTools => {
                let job = self.with_machine(ctx, |connection| {
                    let os = connection.os();
                    let missing: Vec<String> = connection
                        .environment
                        .as_ref()?
                        .tools
                        .iter()
                        .filter(|(tool, found)| {
                            !found && tool_catalog::SYSTEM_TOOLS.contains(&tool.as_str()) && tool_catalog::can_install(tool, os)
                        })
                        .map(|(tool, _)| tool.clone())
                        .collect();
                    let names: Vec<&str> = missing.iter().map(String::as_str).collect();
                    let command = tool_catalog::install_all(&names, os)?;
                    let elevate = os == tool_catalog::Os::Linux;
                    Some(ToolJob::new(JobKind::Install, missing, command, elevate))
                });
                if let Some(job) = job {
                    self.run_tool_job(job, ctx);
                }
            }
            ShowTool(tool) => {
                self.machines.tool_sheet = Some(tool.clone());
                let pinned = self.with_machine(ctx, |connection| {
                    match connection.reports.get(tool)?.pin.as_ref()? {
                        ToolPin::Command(command) => Some(command.clone()),
                        ToolPin::Path(_) => None,
                    }
                });
                self.machines.set("tool_command", &pinned.unwrap_or_default(), ctx);
                if let Some(id) = self.machine_id() {
                    let store = self.machines_store(ctx);
                    ctx.update_model(&store, |s, c| {
                        if let Some(connection) = s.connection_mut(&id) {
                            connection.tool_error = None;
                        }
                        s.probe(&id, c)
                    });
                }
            }
            CloseTool => self.machines.tool_sheet = None,
            UseCopy { tool, path } => {
                let pin = path.clone().map(ToolPin::Path);
                self.pin_tool(tool, pin, ctx);
            }
            UseToolCommand(tool) => {
                let command = self.machines.value("tool_command", ctx).trim().to_string();
                let pin = (!command.is_empty()).then_some(ToolPin::Command(command));
                self.pin_tool(tool, pin, ctx);
            }
            CheckTools => {
                if let Some(id) = self.machine_id() {
                    let store = self.machines_store(ctx);
                    ctx.update_model(&store, |s, c| s.probe(&id, c));
                }
            }
            OpenToolDocs(tool) => {
                if let Some(url) = tool_catalog::docs(tool) {
                    ctx.dispatch_typed_action_deferred(crate::workspace::WorkspaceAction::OpenExternalUrl { url: url });
                }
            }
            AddAgent => self.add_remote_agent(ctx),
        }
        ctx.notify();
    }

    fn open_machine_form(&mut self, action: &MachineAction, ctx: &mut ViewContext<Self>) {
        let store = self.machines_store(ctx);
        let machine = match action {
            MachineAction::Edit(id) => store.read(ctx, |s, _| s.machine(id).cloned()),
            _ => None,
        };
        let host = machine.as_ref().map(|m| &m.connection);
        for (key, _) in FIELDS {
            if !matches!(*key, "search" | "rename" | "agent_name" | "agent_command") {
                self.machines.set(key, "", ctx);
            }
        }
        // A destination without user/port is an SSH-config alias.
        let config_mode = host.is_none_or(|h| h.user.is_none() && h.port.is_none());
        if let Some(host) = host {
            let path = |p: &Option<std::path::PathBuf>| {
                p.as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            for (key, value) in [
                ("name", host.name.clone()),
                ("alias", if config_mode { host.host.clone() } else { String::new() }),
                ("host", if config_mode { String::new() } else { host.host.clone() }),
                ("user", host.user.clone().unwrap_or_default()),
                ("port", host.port.map(|p| p.to_string()).unwrap_or_default()),
                ("key", path(&host.identity_file)),
                ("jump", host.proxy_jump.clone().unwrap_or_default()),
                ("config", path(&host.config_file)),
            ] {
                self.machines.set(key, &value, ctx);
            }
        }
        self.machines.sequence += 1;
        self.machines.form = Some(MachineForm {
            id: machine.as_ref().map(|m| m.id.clone()),
            config_mode,
            auth: host
                .map(|h| h.authentication.clone())
                .unwrap_or(Authentication::Key),
            auth_menu_open: false,
            host_menu_open: false,
            advanced: host.is_some_and(|h| h.proxy_jump.is_some() || h.config_file.is_some()),
            forward: host.is_some_and(|h| h.forward_agent),
            resolution: Resolution::Idle,
            test: TestState::Idle,
            show_logs: false,
            saving: false,
            save_error: None,
            generation: self.machines.sequence,
            scroll: ClippedScrollStateHandle::new(),
        });
        if config_mode && host.is_some() {
            self.resolve_alias(ctx);
        }
        self.machines.focus("name", ctx);
    }

    fn resolve_alias(&mut self, ctx: &mut ViewContext<Self>) {
        let alias = self.machines.value("alias", ctx);
        let config = self.machines.value("config", ctx);
        let Some(form) = self.machines.form.as_mut() else {
            return;
        };
        if alias.is_empty() {
            form.resolution = Resolution::Failed("Choose a host first.".into());
            return;
        }
        form.resolution = Resolution::Resolving;
        form.host_menu_open = false;
        let generation = form.generation;
        let probe = SshHost {
            host: alias.clone(),
            name: String::new(),
            proxy_jump: None,
            user: None,
            port: None,
            identity_file: None,
            config_file: (!config.is_empty()).then(|| dragent_host::remote::expand_key_path(&config)),
            forward_agent: false,
            authentication: Authentication::Agent,
        };
        ctx.spawn(
            async move { probe.validate().and_then(|_| probe.resolve()) },
            move |view, result, ctx| {
                if !view
                    .machines
                    .form
                    .as_ref()
                    .is_some_and(|f| f.generation == generation)
                {
                    return;
                }
                let resolution = match result {
                    Ok(host) => {
                        // The key OpenSSH would use becomes the field's hint.
                        if let Some(key) = host.identity_file.clone() {
                            let editor = view.machines.fields["key"].clone();
                            ctx.update_view(&editor, |e, c| e.set_placeholder(&key, c));
                        }
                        Resolution::Resolved { alias, host }
                    }
                    Err(e) => Resolution::Failed(e),
                };
                if let Some(form) = view.machines.form.as_mut() {
                    form.resolution = resolution;
                }
                ctx.notify();
            },
        );
    }

    fn test_machine_form(&mut self, ctx: &mut ViewContext<Self>) {
        let host = match self.machines.draft_host(ctx) {
            Ok(host) => host,
            Err(e) => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.test = TestState::Failed {
                        message: e,
                        logs: vec![],
                    };
                }
                return;
            }
        };
        let secret = self.machines.secret(ctx);
        let Some(form) = self.machines.form.as_mut() else {
            return;
        };
        form.test = TestState::Testing;
        form.show_logs = false;
        let generation = form.generation;
        ctx.spawn(
            async move {
                let started = Instant::now();
                super::machines_store::reach(&host, secret).map(|_| started.elapsed().as_millis())
            },
            move |view, result, ctx| {
                let Some(form) = view
                    .machines
                    .form
                    .as_mut()
                    .filter(|f| f.generation == generation)
                else {
                    return;
                };
                form.test = match result {
                    Ok(latency_ms) => TestState::Passed { latency_ms },
                    Err(super::machines_store::Unreached::NewHostKey(key)) => TestState::NewHostKey(key),
                    Err(super::machines_store::Unreached::Failed(e)) => {
                        let logs: Vec<String> = e.lines().map(str::to_string).collect();
                        let message = logs
                            .iter()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .cloned()
                            .unwrap_or_else(|| "Connection failed".into());
                        TestState::Failed { message, logs }
                    }
                };
                ctx.notify();
            },
        );
    }

    fn trust_tested_host_key(&mut self, ctx: &mut ViewContext<Self>) {
        let Ok(host) = self.machines.draft_host(ctx) else {
            return;
        };
        let Some(form) = self.machines.form.as_mut() else {
            return;
        };
        let TestState::NewHostKey(key) = std::mem::replace(&mut form.test, TestState::Testing) else {
            return;
        };
        let generation = form.generation;
        ctx.spawn(
            async move { host.trust_host_key(&key) },
            move |view, result, ctx| {
                if !view.machines.form.as_ref().is_some_and(|f| f.generation == generation) {
                    return;
                }
                match result {
                    Ok(()) => view.test_machine_form(ctx),
                    Err(e) => {
                        if let Some(form) = view.machines.form.as_mut() {
                            form.test = TestState::Failed {
                                message: format!("Couldn't save the machine's host key: {e}"),
                                logs: vec![],
                            };
                        }
                        ctx.notify();
                    }
                }
            },
        );
    }

    fn submit_machine_form(&mut self, ctx: &mut ViewContext<Self>) {
        let alias = self.machines.value("alias", ctx);
        let Some(form) = self.machines.form.as_ref() else {
            return;
        };
        if form.saving {
            return;
        }
        // SSH-config mode: the primary button resolves before it saves.
        if !form.ready(&alias) {
            self.resolve_alias(ctx);
            return;
        }
        let host = match self.machines.draft_host(ctx) {
            Ok(host) => host,
            Err(e) => {
                if let Some(form) = self.machines.form.as_mut() {
                    form.save_error = Some(e);
                }
                return;
            }
        };
        let secret = self.machines.secret(ctx);
        let Some(form) = self.machines.form.as_mut() else {
            return;
        };
        form.saving = true;
        form.save_error = None;
        let id = form.id.clone();
        let generation = form.generation;
        let store = self.machines_store(ctx);
        let previous = id
            .as_ref()
            .and_then(|id| store.read(ctx, |s, _| s.machine(id).map(|m| m.connection.clone())));
        ctx.spawn(
            async move {
                // Saved credentials stay on this device, in the OS keychain.
                if let Some(secret) = &secret {
                    host.remember_credential(secret)?;
                }
                dragent_host::machine_registry::MachineRegistry::save_machine(id.as_deref(), host)
            },
            move |view, result, ctx| {
                let Some(form) = view
                    .machines
                    .form
                    .as_mut()
                    .filter(|f| f.generation == generation)
                else {
                    return;
                };
                form.saving = false;
                match result {
                    Ok(machine) => {
                        let store = view.machines_store(ctx);
                        let transport_changed = previous
                            .as_ref()
                            .is_some_and(|old| !same_machine_transport(old, &machine.connection));
                        ctx.update_model(&store, |store, ctx| {
                            if transport_changed {
                                store.forget_connection(&machine.id, ctx);
                            }
                            store.reload(ctx);
                        });
                        view.machines.form = None;
                        view.machines.set("passphrase", "", ctx);
                        view.machines.set("password", "", ctx);
                        if previous.is_none() {
                            view.machines.page = MachinePage::Detail(machine.id.clone());
                            view.machines.tab = MachineTab::System;
                        }
                        ctx.emit(SettingsEvent::MachinesChanged);
                    }
                    Err(e) => form.save_error = Some(e),
                }
                ctx.notify();
            },
        );
    }

    fn commit_rename(&mut self, ctx: &mut ViewContext<Self>) {
        let MachinePage::Detail(id) = self.machines.page.clone() else {
            return;
        };
        self.machines.renaming = false;
        let name = self.machines.value("rename", ctx);
        let store = self.machines_store(ctx);
        let Some(mut host) = store.read(ctx, |s, _| s.machine(&id).map(|m| m.connection.clone()))
        else {
            return;
        };
        if name.is_empty() || name == host.name {
            return;
        }
        host.name = name;
        match dragent_host::machine_registry::MachineRegistry::save_machine(Some(&id), host) {
            Ok(_) => {
                ctx.update_model(&store, |store, ctx| store.reload(ctx));
                ctx.emit(SettingsEvent::MachinesChanged);
            }
            Err(e) => self.machines.notice = Some(format!("Failed to rename machine: {e}")),
        }
    }

    fn delete_machine(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(Confirmation::Delete(id)) = self.machines.confirm.take() else {
            return;
        };
        let store = self.machines_store(ctx);
        let Some(machine) = store.read(ctx, |s, _| s.machine(&id).cloned()) else {
            return;
        };
        ctx.spawn(
            async move {
                dragent_host::machine_registry::MachineRegistry::update_at(
                    &dragent_host::machine_registry::MachineRegistry::path(),
                    |registry| {
                        machine.connection.forget_credentials()?;
                        registry.remove_machine(&machine.id)
                    },
                )
            },
            move |view, result, ctx| {
                let store = view.machines_store(ctx);
                match result {
                    Ok(()) => {
                        ctx.update_model(&store, |store, ctx| {
                            store.forget_connection(&id, ctx);
                            store.reload(ctx);
                        });
                        view.machines.page = MachinePage::List;
                        ctx.emit(SettingsEvent::MachinesChanged);
                    }
                    Err(e) => {
                        view.machines.notice = Some(format!("Failed to delete SSH connection: {e}"))
                    }
                }
                ctx.notify();
            },
        );
    }

    fn close_remote_session(&mut self, session_id: &str, ctx: &mut ViewContext<Self>) {
        let MachinePage::Detail(id) = self.machines.page.clone() else {
            return;
        };
        let store = self.machines_store(ctx);
        let found = store.read(ctx, |s, _| {
            let connection = s.connection(&id)?;
            let session = connection.sessions.iter().find(|s| s.id == session_id)?.clone();
            Some((connection.client.clone()?, session))
        });
        let Some((client, session)) = found else {
            return;
        };
        ctx.spawn(
            async move { client.close_terminal(&session.workspace_id, &session.id) },
            move |view, result, ctx| {
                if let Err(e) = result {
                    view.machines.notice = Some(e);
                }
                let store = view.machines_store(ctx);
                ctx.update_model(&store, |store, ctx| store.refresh(&id, ctx));
                ctx.notify();
            },
        );
    }

    fn add_remote_agent(&mut self, ctx: &mut ViewContext<Self>) {
        let MachinePage::Detail(id) = self.machines.page.clone() else {
            return;
        };
        let name = self.machines.value("agent_name", ctx);
        let command = self.machines.value("agent_command", ctx);
        if name.is_empty() || command.is_empty() {
            self.machines.notice = Some("Enter an agent name and its executable.".into());
            return;
        }
        let store = self.machines_store(ctx);
        let Some(client) = store.read(ctx, |s, _| s.ready_client(&id)) else {
            return;
        };
        ctx.spawn(
            async move {
                client.add_agent_config(
                    &name,
                    &command,
                    vec![],
                    dragent_host::PromptTransport::Argv,
                    vec![],
                    Default::default(),
                )
            },
            move |view, result, ctx| {
                match result {
                    Ok(_) => {
                        view.machines.notice = None;
                        view.machines.set("agent_name", "", ctx);
                        view.machines.set("agent_command", "", ctx);
                    }
                    Err(e) => view.machines.notice = Some(e),
                }
                let store = view.machines_store(ctx);
                ctx.update_model(&store, |store, ctx| store.refresh(&id, ctx));
                ctx.notify();
            },
        );
    }

    pub(crate) fn render_machines(&self, theme: &Theme, app: &warpui::AppContext) -> Box<dyn warpui::elements::Element> {
        let store = self.machines_store(app);
        let store = store.as_ref(app);
        let page = match &self.machines.page {
            MachinePage::Detail(id) if store.machine(id).is_some() => {
                self.render_machine_detail(theme, store, id)
            }
            _ => self.render_machine_list(theme, store),
        };
        if !self.machines.menu_open && !self.machines.runtime_menu_open {
            return page;
        }
        // An open menu closes on the next click anywhere on the page.
        use warpui::elements::Element as _;
        let view_id = self.view_id;
        warpui::elements::EventHandler::new(page)
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(
                    view_id,
                    super::SettingsAction::Machine(MachineAction::CloseMenus),
                );
                warpui::elements::DispatchEventResult::PropagateToParent
            })
            .finish()
    }

    /// Dialogs layered over the whole settings surface.
    pub(crate) fn render_machine_modal(
        &self,
        theme: &Theme,
        app: &warpui::AppContext,
    ) -> Option<Box<dyn warpui::elements::Element>> {
        if let Some(confirm) = &self.machines.confirm {
            let store = self.machines_store(app);
            return Some(self.render_machine_confirm(theme, store.as_ref(app), confirm));
        }
        if let (Some(tool), Some(id)) = (&self.machines.tool_sheet, self.machine_id()) {
            let store = self.machines_store(app);
            let sheet = store.read(app, |store, _| {
                store.connection(&id).map(|connection| self.render_tool_sheet(theme, connection, tool))
            });
            if sheet.is_some() {
                return sheet;
            }
        }
        let form = self.machines.form.as_ref()?;
        Some(self.render_machine_form(theme, form, app))
    }
}

/// True when two saved destinations reach the same machine the same way;
/// the display name alone may differ.
pub(crate) fn same_machine_transport(a: &SshHost, b: &SshHost) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.name.clear();
    b.name.clear();
    a == b
}
