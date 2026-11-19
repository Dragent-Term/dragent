//! Process-wide machine connections. Settings → Machines, the Add Project
//! dialog and the sidebar all read this one store, so a machine's status never
//! disagrees between them. SSH reachability and the workspace runtime are
//! tracked separately ("SSH Connection" vs "Workspace Runtime").
use crate::workspace::PromptAgent;
use super::tool_catalog::{self, Os};
use dragent_host::development::{Elevation, ToolPin, ToolReport, ToolStatus, ToolsReport};
use dragent_host::host_tools::{self, SYSTEM_TOOLS};
use dragent_host::remote::{host_key_problem, HostKeyProblem, SshHost, UnknownHostKey};
use dragent_host::{
    development::{DevelopmentRequest as Request, DevelopmentResponse as Response},
    machine_registry::{Machine, MachineRegistry, RemoteProject},
    AgentConfig, HostClient, TerminalSession,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// How often the open machine page re-checks the machine's tools.
const PROBE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Unknown,
    Starting,
    Running,
    Stopped,
    Failed,
}

/// Four-way machine status, used by every status dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineStatus {
    Connected,
    Connecting,
    Error,
    Idle,
}

impl MachineStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Connecting => "Connecting…",
            Self::Error => "Connection issue",
            Self::Idle => "Not connected",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCommand {
    Start,
    Restart,
    Shutdown,
    Update,
}

#[derive(Clone, Debug, Default)]
pub struct Environment {
    pub home: String,
    pub shell: String,
    pub os: String,
    pub arch: String,
    pub tools: Vec<(String, bool)>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub cpu_percent: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub uptime_seconds: u64,
}

pub struct MachineConnection {
    pub ssh: SshState,
    pub runtime: RuntimeState,
    pub client: Option<HostClient>,
    /// Last failure, shown under whichever row (SSH or runtime) failed.
    pub error: Option<String>,
    /// The machine's host key, when SSH refused it because nothing trusts it
    /// yet: shown for the person to confirm before connecting.
    pub host_key: Option<UnknownHostKey>,
    pub environment: Option<Environment>,
    pub metrics: Option<Metrics>,
    pub version: Option<u32>,
    pub started_at_unix_ms: Option<u64>,
    pub agents: Vec<AgentConfig>,
    pub sessions: Vec<TerminalSession>,
    /// A runtime command in flight, for its button label.
    pub pending: Option<RuntimeCommand>,
    /// Executables already looked up, so refreshes only ask about new ones
    /// (an agent added on the machine).
    asked_tools: Vec<String>,
    /// Status, version and copies of each tool, from runtimes that report
    /// them (older ones only say installed or not, in `environment.tools`).
    pub reports: HashMap<String, ToolReport>,
    /// How installs on the machine get administrator rights.
    pub elevation: Option<Elevation>,
    /// The last copy choice the machine refused, shown in the tool sheet.
    pub tool_error: Option<String>,
    last_probe: Option<Instant>,
    generation: u64,
    refreshing: bool,
}

impl Default for MachineConnection {
    fn default() -> Self {
        Self {
            ssh: SshState::Disconnected,
            runtime: RuntimeState::Unknown,
            client: None,
            error: None,
            host_key: None,
            environment: None,
            metrics: None,
            version: None,
            started_at_unix_ms: None,
            agents: Vec::new(),
            sessions: Vec::new(),
            pending: None,
            asked_tools: Vec::new(),
            reports: HashMap::new(),
            elevation: None,
            tool_error: None,
            last_probe: None,
            generation: 0,
            refreshing: false,
        }
    }
}

impl MachineConnection {
    /// Take in a fresh report on some tools.
    pub fn apply_report(&mut self, report: ToolsReport) {
        if let Some(environment) = self.environment.as_mut() {
            merge_tools(
                &mut environment.tools,
                report
                    .tools
                    .iter()
                    .map(|tool| (tool.name.clone(), tool.status != ToolStatus::Missing))
                    .collect(),
            );
        }
        for tool in report.tools {
            self.reports.insert(tool.name.clone(), tool);
        }
        self.elevation = Some(report.elevation);
    }

    /// The machine's OS, for building install commands.
    pub fn os(&self) -> Os {
        Os::of(self.environment.as_ref().map(|e| e.os.as_str()).unwrap_or("linux"))
    }

    pub fn status(&self) -> MachineStatus {
        match (self.ssh, self.runtime) {
            (SshState::Connecting, _)
            | (SshState::Connected, RuntimeState::Starting | RuntimeState::Unknown) => {
                MachineStatus::Connecting
            }
            (SshState::Connected, RuntimeState::Running) => MachineStatus::Connected,
            (SshState::Failed, _) | (SshState::Connected, RuntimeState::Failed) => {
                MachineStatus::Error
            }
            // Nothing is connecting once the runtime stops: it died (an
            // error to retry) or was shut down on purpose (connect again).
            (SshState::Connected, RuntimeState::Stopped) if self.error.is_some() => {
                MachineStatus::Error
            }
            (SshState::Connected, RuntimeState::Stopped) | (SshState::Disconnected, _) => {
                MachineStatus::Idle
            }
        }
    }

    /// The SSH transport is up (or coming up); drives "Recently used".
    pub fn active(&self) -> bool {
        matches!(self.ssh, SshState::Connected | SshState::Connecting)
    }

    pub fn ready(&self) -> bool {
        self.ssh == SshState::Connected && self.runtime == RuntimeState::Running
    }
}

pub enum MachinesEvent {
    Changed,
}

pub struct MachinesStore {
    pub registry: MachineRegistry,
    pub load_error: Option<String>,
    connections: HashMap<String, MachineConnection>,
    sequence: u64,
    poll_armed: bool,
    /// The machine whose page is open; its tools are re-checked regularly.
    watched: Option<String>,
}

impl Entity for MachinesStore {
    type Event = MachinesEvent;
}
impl SingletonEntity for MachinesStore {}

impl MachinesStore {
    /// In-memory machines for the UI demo, with no registry reads or SSH connections.
    pub(crate) fn preview(_ctx: &mut ModelContext<Self>) -> Self {
        Self { registry: MachineRegistry::default(), load_error: None, connections: HashMap::new(),
            sequence: 0, poll_armed: false, watched: None }
    }

    pub(crate) fn set_preview_connection(&mut self, id: String, connection: MachineConnection) {
        self.connections.insert(id, connection);
    }

    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        let (registry, load_error) = match MachineRegistry::load() {
            Ok(registry) => (registry, None),
            Err(e) => (MachineRegistry::default(), Some(e)),
        };
        Self {
            registry,
            load_error,
            connections: HashMap::new(),
            sequence: 0,
            poll_armed: false,
            watched: None,
        }
    }

    /// Re-read the registry (another window or a save may have changed it).
    pub fn reload(&mut self, ctx: &mut ModelContext<Self>) {
        match MachineRegistry::load() {
            Ok(registry) => {
                self.registry = registry;
                self.load_error = None;
            }
            Err(e) => self.load_error = Some(e),
        }
        let ids: Vec<String> = self.registry.machines.iter().map(|m| m.id.clone()).collect();
        let removed: Vec<String> = self
            .connections
            .keys()
            .filter(|id| !ids.contains(id))
            .cloned()
            .collect();
        for id in removed {
            if let Some(connection) = self.connections.remove(&id) {
                suspend(connection.client);
            }
        }
        ctx.emit(MachinesEvent::Changed);
    }

    pub fn machine(&self, id: &str) -> Option<&Machine> {
        self.registry.machines.iter().find(|m| m.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&RemoteProject> {
        self.registry.projects.iter().find(|p| p.id == id)
    }

    pub fn projects_for(&self, machine_id: &str) -> Vec<&RemoteProject> {
        self.registry
            .projects
            .iter()
            .filter(|p| p.machine_id == machine_id)
            .collect()
    }

    pub fn connection_mut(&mut self, id: &str) -> Option<&mut MachineConnection> {
        self.connections.get_mut(id)
    }

    pub fn connection(&self, id: &str) -> Option<&MachineConnection> {
        self.connections.get(id)
    }

    pub fn status(&self, id: &str) -> MachineStatus {
        self.connections
            .get(id)
            .map(MachineConnection::status)
            .unwrap_or(MachineStatus::Idle)
    }

    pub fn ready_client(&self, id: &str) -> Option<HostClient> {
        self.connections
            .get(id)
            .filter(|c| c.ready())
            .and_then(|c| c.client.clone())
    }

    /// Connect over SSH, then provision/start the workspace runtime. A second
    /// call while one is in flight is ignored.
    pub fn connect(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        let Some(machine) = self.machine(id).cloned() else {
            return;
        };
        self.sequence += 1;
        let generation = self.sequence;
        let connection = self.connections.entry(id.to_string()).or_default();
        if connection.ssh == SshState::Connecting || connection.runtime == RuntimeState::Starting {
            return;
        }
        connection.generation = generation;
        connection.refreshing = false;
        connection.ssh = SshState::Connecting;
        connection.runtime = RuntimeState::Unknown;
        connection.error = None;
        connection.host_key = None;
        ctx.emit(MachinesEvent::Changed);
        let id = id.to_string();
        let name = machine.name();
        let host = machine.connection;
        ctx.spawn(async move { reach(&host, None).map(|_| host) }, move |store, result, ctx| {
            let Some(connection) = store.current(&id, generation) else {
                return;
            };
            match result {
                Err(Unreached::NewHostKey(key)) => {
                    connection.ssh = SshState::Failed;
                    connection.error = Some(format!(
                        "{name} hasn't been confirmed yet. Check its host key and trust it to connect."
                    ));
                    connection.host_key = Some(key);
                    ctx.emit(MachinesEvent::Changed);
                }
                Err(Unreached::Failed(e)) => {
                    connection.ssh = SshState::Failed;
                    connection.error = Some(e);
                    ctx.emit(MachinesEvent::Changed);
                }
                Ok(host) => {
                    connection.ssh = SshState::Connected;
                    connection.runtime = RuntimeState::Starting;
                    ctx.emit(MachinesEvent::Changed);
                    ctx.spawn(
                        async move {
                            let client = HostClient::connect_remote_explicit(host, None)?;
                            let mut environment = client.development(Request::Environment)?;
                            let info = client.host_info()?;
                            // Every agent Dragent knows, and the machine's own.
                            let agents = client.list_agent_configs().unwrap_or_default();
                            let asked = tool_names(&agents);
                            let mut report = None;
                            if let Response::Environment { tools, .. } = &mut environment {
                                let (found, full) = probe_tools(&client, &asked, std::mem::take(tools));
                                *tools = found;
                                report = full;
                            }
                            Ok::<_, String>((client, environment, info, asked, report))
                        },
                        move |store, result, ctx| {
                            let Some(connection) = store.current(&id, generation) else {
                                return;
                            };
                            match result {
                                Ok((
                                    client,
                                    Response::Environment {
                                        home,
                                        shell,
                                        os,
                                        arch,
                                        tools,
                                    },
                                    info,
                                    asked,
                                    report,
                                )) => {
                                    connection.asked_tools = asked;
                                    connection.last_probe = Some(Instant::now());
                                    if let Some(report) = report {
                                        connection.apply_report(report);
                                    }
                                    crate::remote_fs::connect(&id, client.clone(), home.clone(), shell.clone(), name);
                                    connection.client = Some(client);
                                    connection.runtime = RuntimeState::Running;
                                    connection.version = Some(info.version);
                                    connection.started_at_unix_ms = Some(info.started_at_unix_ms);
                                    connection.environment = Some(Environment {
                                        home,
                                        shell,
                                        os,
                                        arch,
                                        tools,
                                    });
                                    store.refresh(&id, ctx);
                                    store.arm_poll(ctx);
                                }
                                Ok(_) => {
                                    connection.runtime = RuntimeState::Failed;
                                    connection.error =
                                        Some("The workspace runtime returned an invalid environment.".into());
                                }
                                Err(e) => {
                                    connection.runtime = RuntimeState::Failed;
                                    connection.error = Some(e);
                                }
                            }
                            ctx.emit(MachinesEvent::Changed);
                        },
                    );
                }
            }
        });
    }

    /// User intent: stop using the machine. Remote processes keep running,
    /// and background calls must not silently reconnect it.
    pub fn disconnect(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        self.sequence += 1;
        let connection = self.connections.entry(id.to_string()).or_default();
        connection.generation = self.sequence;
        connection.ssh = SshState::Disconnected;
        connection.runtime = RuntimeState::Unknown;
        connection.error = None;
        connection.refreshing = false;
        connection.pending = None;
        connection.metrics = None;
        suspend(connection.client.take());
        crate::remote_fs::disconnect(id);
        ctx.emit(MachinesEvent::Changed);
    }

    /// Drop a live connection after its SSH settings changed, so the next
    /// connect uses the new destination and credentials.
    pub fn forget_connection(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        if let Some(connection) = self.connections.remove(id) {
            suspend(connection.client);
        }
        crate::remote_fs::disconnect(id);
        ctx.emit(MachinesEvent::Changed);
    }

    /// An explicit Connect / Try again: start the runtime when SSH is up but
    /// the runtime stopped or died (reconnecting would only re-probe the dead
    /// endpoint), otherwise connect from scratch. Background recovery never
    /// calls this, so an outage is never silently replaced.
    /// Trust the host key the machine presented (after the person compared
    /// its fingerprint), then connect.
    pub fn trust_host_key(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        let Some(host) = self.machine(id).map(|m| m.connection.clone()) else {
            return;
        };
        let Some(key) = self.connections.get_mut(id).and_then(|c| c.host_key.take()) else {
            return;
        };
        let id = id.to_string();
        ctx.spawn(async move { host.trust_host_key(&key) }, move |store, result, ctx| match result {
            Ok(()) => store.connect(&id, ctx),
            Err(e) => {
                if let Some(connection) = store.connections.get_mut(&id) {
                    connection.ssh = SshState::Failed;
                    connection.error = Some(format!("Couldn't save the machine's host key: {e}"));
                    ctx.emit(MachinesEvent::Changed);
                }
            }
        });
    }

    pub fn recover(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        let restart = self.connections.get(id).is_some_and(|c| {
            c.ssh == SshState::Connected
                && c.client.is_some()
                && matches!(c.runtime, RuntimeState::Stopped | RuntimeState::Failed)
        });
        if restart {
            self.runtime_command(id, RuntimeCommand::Start, ctx);
        } else {
            self.connect(id, ctx);
        }
    }

    pub fn runtime_command(
        &mut self,
        id: &str,
        command: RuntimeCommand,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(connection) = self.connections.get_mut(id) else {
            return;
        };
        let Some(client) = connection.client.clone() else {
            return;
        };
        if connection.pending.is_some() {
            return;
        }
        self.sequence += 1;
        let generation = self.sequence;
        connection.generation = generation;
        connection.refreshing = false;
        connection.pending = Some(command);
        connection.error = None;
        if command != RuntimeCommand::Shutdown {
            connection.runtime = RuntimeState::Starting;
        }
        ctx.emit(MachinesEvent::Changed);
        let id = id.to_string();
        ctx.spawn(
            async move {
                let remote = client
                    .remote_connection()
                    .cloned()
                    .ok_or("Not an SSH connection")?;
                remote.resume();
                if command != RuntimeCommand::Start && client.ping() {
                    client.shutdown()?;
                }
                if command == RuntimeCommand::Shutdown {
                    return Ok(None);
                }
                remote.start_or_update(command == RuntimeCommand::Update)?;
                Ok::<_, String>(Some(client.host_info()?))
            },
            move |store, result, ctx| {
                let Some(connection) = store.current(&id, generation) else {
                    return;
                };
                connection.pending = None;
                match result {
                    Ok(Some(info)) => {
                        connection.runtime = RuntimeState::Running;
                        connection.version = Some(info.version);
                        connection.started_at_unix_ms = Some(info.started_at_unix_ms);
                        if let (Some(client), Some(env)) =
                            (connection.client.clone(), connection.environment.clone())
                        {
                            let name = store.machine(&id).map(|m| m.name()).unwrap_or_default();
                            crate::remote_fs::connect(&id, client, env.home, env.shell, name);
                        }
                        store.refresh(&id, ctx);
                    }
                    Ok(None) => {
                        connection.runtime = RuntimeState::Stopped;
                        connection.metrics = None;
                        connection.sessions.clear();
                        crate::remote_fs::disconnect(&id);
                    }
                    Err(e) => {
                        connection.runtime = RuntimeState::Failed;
                        connection.error = Some(e);
                    }
                }
                ctx.emit(MachinesEvent::Changed);
            },
        );
    }

    /// The machine whose page is open (`None` when none is): only its tools
    /// are re-checked in the background.
    pub fn set_watched(&mut self, id: Option<String>) {
        self.watched = id;
    }

    /// A fresh report on some of a connected machine's tools.
    pub fn apply_tools_report(&mut self, id: &str, report: ToolsReport, ctx: &mut ModelContext<Self>) {
        if let Some(connection) = self.connections.get_mut(id) {
            connection.apply_report(report);
            ctx.emit(MachinesEvent::Changed);
        }
    }

    /// Check the machine's tools again now ("Check again").
    pub fn probe(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        // Probe-skipped targets never re-probe (Hosts settings).
        if !super::settings_tasks_hosts::probe_allowed_for(id) {
            return;
        }
        let Some(connection) = self.connections.get_mut(id) else {
            return;
        };
        let Some(client) = connection.client.clone() else {
            return;
        };
        let names = connection.asked_tools.clone();
        connection.last_probe = Some(Instant::now());
        let generation = connection.generation;
        let id = id.to_string();
        ctx.spawn(
            async move { probe_tools(&client, &names, Vec::new()) },
            move |store, (tools, report), ctx| {
                let Some(connection) = store.current(&id, generation) else {
                    return;
                };
                if let Some(environment) = connection.environment.as_mut() {
                    merge_tools(&mut environment.tools, tools);
                }
                if let Some(report) = report {
                    connection.apply_report(report);
                }
                ctx.emit(MachinesEvent::Changed);
            },
        );
    }

    /// Use this copy of `tool` on the machine (`None`: the first on the PATH).
    pub fn pin_tool(&mut self, id: &str, tool: &str, pin: Option<ToolPin>, ctx: &mut ModelContext<Self>) {
        let Some(connection) = self.connections.get_mut(id) else {
            return;
        };
        let Some(client) = connection.client.clone() else {
            return;
        };
        connection.tool_error = None;
        let generation = connection.generation;
        let id = id.to_string();
        let tool = tool.to_string();
        ctx.spawn(
            async move {
                match client.development(Request::PinTool { name: tool.clone(), pin }) {
                    Ok(_) => Ok(probe_tools(&client, &[tool], Vec::new()).1),
                    Err(error) if error.contains("read:") || error.contains("unexpected") => Err(
                        "This machine's workspace runtime is too old to choose copies. Update it from the Workspace runtime menu."
                            .to_string(),
                    ),
                    Err(error) => Err(error),
                }
            },
            move |store, result, ctx| {
                let Some(connection) = store.current(&id, generation) else {
                    return;
                };
                match result {
                    Ok(Some(report)) => connection.apply_report(report),
                    Ok(None) => {}
                    Err(error) => connection.tool_error = Some(error),
                }
                ctx.emit(MachinesEvent::Changed);
            },
        );
    }

    /// A fresh look at some of a connected machine's tools (after an install).
    pub fn set_tools(&mut self, id: &str, tools: Vec<(String, bool)>, ctx: &mut ModelContext<Self>) {
        if let Some(environment) = self
            .connections
            .get_mut(id)
            .and_then(|connection| connection.environment.as_mut())
        {
            merge_tools(&mut environment.tools, tools);
            ctx.emit(MachinesEvent::Changed);
        }
    }

    /// Health, resource usage, agents and sessions for a connected machine.
    /// Only a failed health probe changes the runtime state.
    pub fn refresh(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        let Some(connection) = self.connections.get_mut(id) else {
            return;
        };
        if connection.refreshing
            || connection.pending.is_some()
            || connection.ssh != SshState::Connected
            || !matches!(connection.runtime, RuntimeState::Running | RuntimeState::Failed)
        {
            return;
        }
        let Some(client) = connection.client.clone() else {
            return;
        };
        connection.refreshing = true;
        let generation = connection.generation;
        let asked = connection.asked_tools.clone();
        // The open machine page re-checks every tool now and then: versions
        // move, and tools get installed outside Dragent.
        let due = self.watched.as_deref() == Some(id)
            && connection.last_probe.is_none_or(|at| at.elapsed() >= PROBE_INTERVAL);
        if due {
            connection.last_probe = Some(Instant::now());
        }
        let id = id.to_string();
        ctx.spawn(
            async move {
                let info = client.host_info()?;
                let metrics = client.development(Request::Metrics).ok();
                let agents = client.list_agent_configs().ok();
                // Agents added on the machine since the last lookup, or
                // everything when a re-check is due.
                let mut names: Vec<String> = if due { asked.clone() } else { Vec::new() };
                for name in agents.as_deref().map(tool_names).unwrap_or_default() {
                    if !asked.contains(&name) && !names.contains(&name) {
                        names.push(name);
                    }
                }
                let found = (!names.is_empty()).then(|| {
                    let (tools, report) = probe_tools(&client, &names, Vec::new());
                    (names, tools, report)
                });
                let sessions = client
                    .list_workspaces()
                    .and_then(|ids| {
                        ids.into_iter().try_fold(Vec::new(), |mut all, id| {
                            all.extend(client.list_terminals(&id)?);
                            Ok(all)
                        })
                    })
                    .ok();
                Ok::<_, String>((info, metrics, agents, sessions, found))
            },
            move |store, result, ctx| {
                let name = store.machine(&id).map(|m| m.name()).unwrap_or_default();
                let Some(connection) = store.current(&id, generation) else {
                    return;
                };
                connection.refreshing = false;
                match result {
                    Ok((info, metrics, agents, sessions, found)) => {
                        if let Some((names, tools, report)) = found {
                            for name in names {
                                if !connection.asked_tools.contains(&name) {
                                    connection.asked_tools.push(name);
                                }
                            }
                            if let Some(environment) = connection.environment.as_mut() {
                                merge_tools(&mut environment.tools, tools);
                            }
                            if let Some(report) = report {
                                connection.apply_report(report);
                            }
                        }
                        connection.runtime = RuntimeState::Running;
                        connection.error = None;
                        // A recovered runtime makes the machine's paths
                        // reachable again.
                        if !crate::remote_fs::is_connected(&id) {
                            if let (Some(client), Some(env)) =
                                (connection.client.clone(), connection.environment.as_ref())
                            {
                                crate::remote_fs::connect(
                                    &id,
                                    client,
                                    env.home.clone(),
                                    env.shell.clone(),
                                    name,
                                );
                            }
                        }
                        connection.version = Some(info.version);
                        connection.started_at_unix_ms = Some(info.started_at_unix_ms);
                        if let Some(Response::Metrics {
                            cpu_percent,
                            memory_used,
                            memory_total,
                            uptime_seconds,
                            disk_used,
                            disk_total,
                        }) = metrics
                        {
                            connection.metrics = Some(Metrics {
                                cpu_percent,
                                memory_used,
                                memory_total,
                                disk_used,
                                disk_total,
                                uptime_seconds,
                            });
                        }
                        if let Some(agents) = agents {
                            connection.agents = agents;
                        }
                        if let Some(mut sessions) = sessions {
                            sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at_unix_ms));
                            connection.sessions = sessions;
                        }
                    }
                    Err(e) => {
                        if e.contains("Machine disconnected") {
                            connection.ssh = SshState::Disconnected;
                            connection.runtime = RuntimeState::Unknown;
                        } else {
                            connection.runtime = RuntimeState::Stopped;
                            connection.error = Some(e);
                        }
                        connection.metrics = None;
                        crate::remote_fs::disconnect(&id);
                    }
                }
                ctx.emit(MachinesEvent::Changed);
            },
        );
    }

    fn arm_poll(&mut self, ctx: &mut ModelContext<Self>) {
        if self.poll_armed {
            return;
        }
        self.poll_armed = true;
        ctx.spawn(Timer::after(POLL_INTERVAL), |store, _, ctx| {
            store.poll_armed = false;
            let ids: Vec<String> = store
                .connections
                .iter()
                .filter(|(_, c)| c.ssh == SshState::Connected)
                .map(|(id, _)| id.clone())
                .collect();
            if ids.is_empty() {
                return;
            }
            for id in ids {
                store.refresh(&id, ctx);
            }
            store.arm_poll(ctx);
        });
    }

    fn current(&mut self, id: &str, generation: u64) -> Option<&mut MachineConnection> {
        self.connections
            .get_mut(id)
            .filter(|c| c.generation == generation)
    }
}

/// Why a machine couldn't be reached over SSH.
pub(crate) enum Unreached {
    /// Nothing trusts the machine's host key yet: the person confirms it.
    NewHostKey(UnknownHostKey),
    Failed(String),
}

/// Reach the machine over SSH (`SshHost::test`). A host key nothing trusts
/// yet comes back for the person to confirm rather than as an error.
pub(crate) fn reach(host: &SshHost, secret: Option<String>) -> Result<String, Unreached> {
    let error = match host.test(secret) {
        Ok(out) => return Ok(out),
        Err(error) => error,
    };
    match host_key_problem(&error) {
        Some(HostKeyProblem::Unknown) => match host.unknown_host_key() {
            Ok(Some(key)) => Err(Unreached::NewHostKey(key)),
            Ok(None) => Err(Unreached::Failed(error)),
            Err(probe) => Err(Unreached::Failed(probe)),
        },
        Some(HostKeyProblem::Changed) => {
            let target = match host.resolve() {
                Ok(resolved) if resolved.port != 22 => {
                    format!("'[{}]:{}'", resolved.hostname, resolved.port)
                }
                Ok(resolved) => resolved.hostname,
                Err(_) => host.host.clone(),
            };
            Err(Unreached::Failed(format!(
                "The machine's host key isn't the one you trusted before. That happens when it's \
                 reinstalled, but it can also mean someone is intercepting the connection. If you \
                 expected it, remove the old key with `ssh-keygen -R {target}`, then connect again."
            )))
        }
        None => Err(Unreached::Failed(error)),
    }
}

fn suspend(client: Option<HostClient>) {
    if let Some(remote) = client.and_then(|c| c.remote_connection().cloned()) {
        crate::bg::detach(move || remote.suspend());
    }
}

/// "203.0.113.10", "ubuntu@203.0.113.10" or "ubuntu@203.0.113.10:2222",
/// the row subtitle shown under each machine name.
pub fn endpoint_label(machine: &Machine) -> String {
    let host = &machine.connection;
    let target = match &host.user {
        Some(user) => format!("{user}@{}", host.host),
        None => host.host.clone(),
    };
    match host.port {
        Some(port) if port != 22 => format!("{target}:{port}"),
        _ => target,
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Executables the machine page asks about: system tools, every agent
/// Dragent can start, and the commands of agents added on the machine.
pub fn tool_names(agents: &[AgentConfig]) -> Vec<String> {
    let mut names: Vec<String> = SYSTEM_TOOLS.iter().map(|tool| tool.to_string()).collect();
    let mut add = |name: &str| {
        if host_tools::valid_name(name) && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    };
    for agent in PromptAgent::ALL {
        if let Some(cli) = agent.cli() {
            add(cli);
        }
    }
    for agent in agents {
        if let Some(executable) = agent.command.split_whitespace().next() {
            add(executable);
        }
    }
    names
}

/// Report on `names`: (installed or not, the full report). Runtimes older
/// than `ProbeTools` answer only whether each is installed.
pub fn probe_tools(
    client: &HostClient,
    names: &[String],
    fallback: Vec<(String, bool)>,
) -> (Vec<(String, bool)>, Option<ToolsReport>) {
    let queries = names.iter().map(|name| tool_catalog::query(name)).collect();
    match client.development(Request::ProbeTools { tools: queries }) {
        Ok(Response::ToolsReport(report)) => (
            report
                .tools
                .iter()
                .map(|tool| (tool.name.clone(), tool.status != ToolStatus::Missing))
                .collect(),
            Some(report),
        ),
        _ => (lookup_tools(client, names.to_vec(), fallback), None),
    }
}

/// Which of `names` the machine has. Runtimes older than `Which` answer
/// only Environment's tools: `fallback`.
pub fn lookup_tools(
    client: &HostClient,
    names: Vec<String>,
    fallback: Vec<(String, bool)>,
) -> Vec<(String, bool)> {
    match client.development(Request::Which { tools: names }) {
        Ok(Response::Tools(tools)) => tools,
        _ => fallback,
    }
}

/// Replace or add each looked-up tool, keeping the others.
fn merge_tools(tools: &mut Vec<(String, bool)>, found: Vec<(String, bool)>) {
    for (name, installed) in found {
        match tools.iter_mut().find(|(known, _)| *known == name) {
            Some(entry) => entry.1 = installed,
            None => tools.push((name, installed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_lookups_cover_agents_and_merge() {
        let names = tool_names(&[]);
        for expected in ["git", "rg", "claude", "codex", "gemini", "opencode", "cursor-agent"] {
            assert!(names.iter().any(|n| n == expected), "{expected}");
        }
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "no duplicates");
        let mut tools = vec![("git".to_string(), true), ("rg".to_string(), false)];
        merge_tools(&mut tools, vec![("rg".into(), true), ("aider".into(), false)]);
        assert_eq!(tools, vec![("git".into(), true), ("rg".into(), true), ("aider".into(), false)]);
    }

    fn connection(ssh: SshState, runtime: RuntimeState) -> MachineConnection {
        MachineConnection {
            ssh,
            runtime,
            ..Default::default()
        }
    }

    #[test]
    fn status_separates_ssh_from_runtime() {
        use MachineStatus::*;
        use RuntimeState as R;
        use SshState as S;
        assert_eq!(connection(S::Disconnected, R::Unknown).status(), Idle);
        assert_eq!(connection(S::Connecting, R::Unknown).status(), Connecting);
        assert_eq!(connection(S::Connected, R::Starting).status(), Connecting);
        assert_eq!(connection(S::Connected, R::Running).status(), Connected);
        assert_eq!(connection(S::Connected, R::Failed).status(), Error);
        assert_eq!(connection(S::Failed, R::Unknown).status(), Error);
        assert_eq!(connection(S::Connected, R::Unknown).status(), Connecting);
        // A stopped runtime never reads as "connecting".
        assert_eq!(connection(S::Connected, R::Stopped).status(), Idle);
        let mut died = connection(S::Connected, R::Stopped);
        died.error = Some("runtime exited".into());
        assert_eq!(died.status(), Error);
        assert!(connection(S::Connected, R::Stopped).active());
        assert!(!connection(S::Failed, R::Unknown).active());
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(20 * 1024 * 1024), "20 MB");
        assert_eq!(format_bytes(8 * 1024 * 1024 * 1024), "8.0 GB");
    }
}
