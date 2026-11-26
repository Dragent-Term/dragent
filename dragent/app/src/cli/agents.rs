//! `dragent agents …`: the agents Dragent started, what each is doing, and
//! talking to them. Read from Dragent's agent records and its terminal
//! service, so everything but `start` works while the app is closed.

use std::path::Path;

use dragent_host::agent_sessions::{self, AgentSession};
use dragent_host::protocol::TerminalState;
use dragent_host::HostClient;

use super::{absolute, agent_key, ask_app, listing, model, pretty, requests, saved, text, Flags};

/// What an agent is doing, from its last hook report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Working,
    NeedsInput,
    Waiting,
    Failed,
    /// Alive, but it doesn't report what it's doing.
    Running,
    /// Hasn't started: it waits for memory.
    WaitingForMemory,
    /// Stopped for holding too much memory.
    Paused,
    /// Dragent's terminal service isn't reachable to check.
    Unknown,
}

impl State {
    pub(super) fn key(self) -> &'static str {
        match self {
            State::Working => "working",
            State::NeedsInput => "needs_input",
            State::Waiting => "waiting",
            State::Failed => "failed",
            State::Running => "running",
            State::WaitingForMemory => "waiting_for_memory",
            State::Paused => "paused",
            State::Unknown => "unknown",
        }
    }

    pub(super) fn label_for(key: &str) -> &'static str {
        match key {
            "working" => "working",
            "needs_input" => "needs your input",
            "waiting" => "waiting for you",
            "failed" => "stopped with an error",
            "running" => "running",
            "waiting_for_memory" => "waiting for memory",
            "paused" => "paused for its memory",
            _ => "unknown",
        }
    }

    fn from_activity(event: &str) -> State {
        match event {
            "Start" => State::Working,
            "PermissionRequest" => State::NeedsInput,
            "Failed" => State::Failed,
            _ => State::Waiting,
        }
    }
}

#[derive(Debug)]
pub(super) struct Running {
    pub(super) session: AgentSession,
    pub(super) state: State,
    /// When it last reported, in Unix milliseconds.
    pub(super) since_ms: Option<u64>,
}

impl Running {
    /// The id the commands take: the session's first eight characters,
    /// enough to tell agents apart.
    pub(super) fn id(&self) -> String {
        self.session.pane_uuid.to_string()[..8].to_string()
    }
}

/// The terminal service, if it's running. Never started from here.
fn service() -> Option<HostClient> {
    HostClient::connect_existing().ok()
}

/// Agents this organisation's records call active, minus those whose
/// terminal has ended.
pub(super) fn running_now() -> Vec<Running> {
    running_with(service().as_ref())
}

fn running_with(client: Option<&HostClient>) -> Vec<Running> {
    let Ok(entries) = std::fs::read_dir(dragent_paths::organisation_dir().join("agent-sessions")) else {
        return Vec::new();
    };
    let paused: Vec<String> = client
        .and_then(|client| client.agent_resources().ok())
        .map(|found| found.paused.into_iter().map(|agent| agent.pane).collect())
        .unwrap_or_default();
    let mut agents: Vec<Running> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<uuid::Uuid>().ok())
        .filter_map(agent_sessions::load)
        .filter(|session| session.active)
        .filter_map(|session| {
            let alive = client.map(|client| {
                client
                    .terminal(&session.workspace_id, &session.terminal_id)
                    .is_ok_and(|terminal| terminal.state == TerminalState::Running)
            });
            if alive == Some(false) {
                return None;
            }
            let activity = agent_sessions::load_activity(session.pane_uuid);
            let state = match (&activity, alive) {
                _ if agent_sessions::is_waiting(session.pane_uuid) => State::WaitingForMemory,
                _ if paused.contains(&session.pane_uuid.to_string()) => State::Paused,
                (_, None) => State::Unknown,
                (Some(activity), _) => State::from_activity(&activity.event),
                (None, _) => State::Running,
            };
            Some(Running {
                since_ms: activity.map(|activity| activity.at_ms),
                state,
                session,
            })
        })
        .collect();
    agents.sort_by(|a, b| a.session.cwd.cmp(&b.session.cwd));
    agents
}

/// `AGENT`: an id (or its start), or a workspace path with one agent in it.
fn find<'a>(agents: &'a [Running], reference: &str) -> Result<&'a Running, String> {
    let path = absolute(reference);
    let matches: Vec<&Running> = if path.is_dir() {
        agents
            .iter()
            .filter(|agent| {
                let cwd = &agent.session.cwd;
                cwd.starts_with(&path) || cwd.canonicalize().is_ok_and(|cwd| cwd.starts_with(&path))
            })
            .collect()
    } else {
        let reference = reference.to_lowercase();
        if reference.len() < 4 {
            return Err("give at least the first four characters of the agent's id".into());
        }
        agents
            .iter()
            .filter(|agent| agent.session.pane_uuid.to_string().starts_with(&reference))
            .collect()
    };
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no running agent matches “{reference}” (see `dragent agents running`)"
        )),
        several => Err(format!(
            "“{reference}” matches {} agents: {}. Use an id",
            several.len(),
            several
                .iter()
                .map(|agent| format!("{} ({})", agent.id(), agent.session.provider))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn reference(flags: &Flags) -> Result<&str, String> {
    flags
        .argument(0)
        .ok_or_else(|| format!("which agent? (`dragent agents {} AGENT`)", flags.subcommand()))
}

pub(super) fn run(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    match flags.subcommand() {
        "list" => Ok(super::installed_agents(&flags)),
        "accounts" => super::agent_accounts::run(&flags),
        "running" | "status" => running_command(&flags),
        "read" => read(&flags),
        "send" => send(&flags),
        "stop" => stop(&flags),
        "resume" => resume(&flags),
        "start-anyway" => start_anyway(&flags),
        "start" | "new" => start(&flags),
        other => Err(format!(
            "unknown agents command “{other}” (use accounts, list, running, read, send, stop, resume, start-anyway or start)"
        )),
    }
}

fn running_command(flags: &Flags) -> Result<String, String> {
    let saved = saved()?;
    let project = flags.value("project").map(absolute);
    let workspace = flags.value("workspace").map(absolute);
    let now = now_ms();
    let rows = running_now()
        .iter()
        .filter_map(|agent| {
            let home = saved.workspace_containing(&agent.session.cwd);
            let root = home.map(|workspace| workspace.root.clone());
            if workspace.as_ref().is_some_and(|wanted| root.as_ref() != Some(wanted)) {
                return None;
            }
            let agent_project = root.as_deref().and_then(|root| saved.project_of(root));
            if project.is_some() && agent_project != project {
                return None;
            }
            Some(serde_json::json!({
                "id": agent.id(),
                "session": agent.session.pane_uuid.to_string(),
                "agent": agent.session.provider,
                "account": agent.session.account,
                "state": agent.state.key(),
                "since_ms": agent.since_ms,
                "for": agent.since_ms.map(|since| ago(now, since)),
                "workspace": home.map(|workspace| workspace.title.clone()),
                "workspace_path": root,
                "cwd": agent.session.cwd,
            }))
        })
        .collect();
    let unchecked = service().is_none();
    let mut output = listing(
        flags,
        rows,
        |row| text(row, "id"),
        |row| {
            format!(
                "{}\t{}\t{}{}\t{}",
                text(row, "id"),
                text(row, "agent"),
                State::label_for(&text(row, "state")),
                row["for"]
                    .as_str()
                    .map(|ago| format!(" · {ago} ago"))
                    .unwrap_or_default(),
                row["workspace"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| text(row, "cwd"))
            )
        },
        "No agents are running.",
    );
    if unchecked && !flags.json && !flags.quiet {
        output.push_str("\n(Dragent's terminal service isn't running, so these couldn't be checked.)");
    }
    Ok(output)
}

fn read(flags: &Flags) -> Result<String, String> {
    let agents = running_now();
    let agent = find(&agents, reference(flags)?)?;
    let lines = match flags.value("lines") {
        Some(lines) => lines.parse::<usize>().map_err(|_| "--lines takes a number".to_string())?,
        None => 60,
    };
    let client = service().ok_or("Dragent's terminal service isn't running")?;
    let screen = screen_text(&client, &agent.session, lines)?;
    if flags.json {
        return Ok(pretty(serde_json::json!({
            "id": agent.id(),
            "agent": agent.session.provider,
            "text": screen,
        })));
    }
    Ok(screen)
}

/// The last `lines` rows of an agent's screen as plain text: what a person
/// looking at its pane sees now.
pub(super) fn screen_text(client: &HostClient, session: &AgentSession, lines: usize) -> Result<String, String> {
    let screen = client.read_terminal_screen(&session.workspace_id, &session.terminal_id)?;
    let mut rows: Vec<String> = screen
        .lines
        .iter()
        .map(|row| {
            row.spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    while rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    let start = rows.len().saturating_sub(lines);
    Ok(rows[start..].join("\n"))
}

/// Typed into the session as a paste, then Enter (unless `--no-enter`),
/// the way a person would send a message.
fn send(flags: &Flags) -> Result<String, String> {
    let agents = running_now();
    let agent = find(&agents, reference(flags)?)?;
    let message = match flags.value("text") {
        Some(message) => message.to_string(),
        None => flags.positional.iter().skip(2).cloned().collect::<Vec<_>>().join(" "),
    };
    if message.trim().is_empty() {
        return Err("what should it say? (`dragent agents send AGENT TEXT`)".into());
    }
    let client = service().ok_or("Dragent's terminal service isn't running")?;
    let session = &agent.session;
    if agent_sessions::is_waiting(session.pane_uuid) {
        return Err(format!(
            "{id} hasn't started: it waits for memory. Send it later, or `dragent agents start-anyway {id}` first.",
            id = agent.id()
        ));
    }
    type_into_session(&client, session, &message, !flags.on("no-enter"))?;
    if flags.json {
        return Ok(pretty(serde_json::json!({ "id": agent.id(), "sent": true })));
    }
    if flags.quiet {
        return Ok(agent.id());
    }
    Ok(format!("Sent to {} ({})", session.provider, agent.id()))
}

/// Type `message` into the agent in `session` as a paste, then (with
/// `enter`) submit it. Enter goes in a separate write once the paste has
/// landed: sent together, some agents (Codex) keep the message in their
/// input box instead of starting a turn.
pub(crate) fn type_into_session(
    client: &HostClient,
    session: &AgentSession,
    message: &str,
    enter: bool,
) -> Result<(), String> {
    let message: String = message
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect();
    let mut paste = b"\x1b[200~".to_vec();
    paste.extend_from_slice(message.as_bytes());
    paste.extend_from_slice(b"\x1b[201~");
    client.send_terminal_input(&session.workspace_id, &session.terminal_id, &paste)?;
    if enter {
        std::thread::sleep(std::time::Duration::from_millis(250));
        client.send_terminal_input(&session.workspace_id, &session.terminal_id, b"\r")?;
    }
    Ok(())
}

fn stop(flags: &Flags) -> Result<String, String> {
    let agents = running_now();
    let agent = find(&agents, reference(flags)?)?;
    // A tab showing the agent would bring a closed terminal back as a lost
    // session, so the app closes the tab, as the user would.
    let tab_closed = super::instance::app_running()
        && ask_app(
            requests::Request::StopAgent {
                pane: agent.session.pane_uuid.to_string(),
            },
            false,
        )
        .is_ok_and(|response| {
            response.data.as_ref().and_then(|data| data["tab_closed"].as_bool()) == Some(true)
        });
    if !tab_closed {
        let client = service().ok_or("Dragent's terminal service isn't running")?;
        client.close_terminal(&agent.session.workspace_id, &agent.session.terminal_id)?;
    }
    if flags.json {
        return Ok(pretty(serde_json::json!({ "id": agent.id(), "stopped": true })));
    }
    if flags.quiet {
        return Ok(agent.id());
    }
    Ok(format!("Stopped {} ({})", agent.session.provider, agent.id()))
}

/// `dragent agents resume`: let an agent paused for its memory run again.
fn resume(flags: &Flags) -> Result<String, String> {
    let agents = running_now();
    let agent = find(&agents, reference(flags)?)?;
    let client = service().ok_or("Dragent's terminal service isn't running")?;
    client.resume_agent(&agent.session.pane_uuid.to_string())?;
    if flags.json {
        return Ok(pretty(serde_json::json!({ "id": agent.id(), "resumed": true })));
    }
    if flags.quiet {
        return Ok(agent.id());
    }
    Ok(format!("Resumed {} ({})", agent.session.provider, agent.id()))
}

/// `dragent agents start-anyway`: start one that waits for memory now.
fn start_anyway(flags: &Flags) -> Result<String, String> {
    let agents = running_now();
    let agent = find(&agents, reference(flags)?)?;
    if !agent_sessions::is_waiting(agent.session.pane_uuid) {
        return Err(format!("{} isn't waiting for memory.", agent.id()));
    }
    let client = service().ok_or("Dragent's terminal service isn't running")?;
    client.start_anyway(&agent.session.pane_uuid.to_string())?;
    if flags.json {
        return Ok(pretty(serde_json::json!({ "id": agent.id(), "started": true })));
    }
    if flags.quiet {
        return Ok(agent.id());
    }
    Ok(format!("Starting {} ({})", agent.session.provider, agent.id()))
}

/// Most of an agent's screen a handover carries.
const HANDOVER_CHARS: usize = 30_000;

fn start(flags: &Flags) -> Result<String, String> {
    let workspace = absolute(
        flags
            .value("workspace")
            .ok_or("which workspace? (--workspace PATH, see `dragent workspaces list`)")?,
    );
    let saved = saved()?;
    if !saved.workspaces.iter().any(|known| known.root == workspace) {
        return Err(format!(
            "{} isn't a workspace in Dragent (see `dragent workspaces list`)",
            workspace.display()
        ));
    }
    let agent = flags.value("agent").map(agent_key).transpose()?;
    let model = flags.value("model").map(str::to_string);
    if let (Some(model), Some(key)) = (&model, &agent) {
        model::check(key, model)?;
    }
    let mut prompt = flags.prompt()?;
    if let Some(from) = flags.value("from") {
        prompt = handover(from, &prompt)?;
    }
    if prompt.trim().is_empty() {
        return Err("what should it do? (--prompt TEXT or --prompt-file FILE)".into());
    }
    let response = ask_app(
        requests::Request::StartAgent {
            workspace,
            agent,
            prompt,
            model,
            account: flags.value("account").map(dragent_host::agent_accounts::Selection::parse).transpose()?.unwrap_or_default(),
        },
        false,
    )?;
    if flags.json {
        return Ok(pretty(serde_json::to_value(&response).unwrap_or_default()));
    }
    let id = response.agent_id.as_deref().map(|id| &id[..8.min(id.len())]).unwrap_or("-");
    if flags.quiet {
        return Ok(id.to_string());
    }
    Ok(format!(
        "Started {} ({id}) in {}",
        response.agent.as_deref().unwrap_or("the agent"),
        response.title.as_deref().unwrap_or("the workspace")
    ))
}

/// `prompt` preceded by the end of another agent's session, so a new agent
/// can carry on from it.
fn handover(from: &str, prompt: &str) -> Result<String, String> {
    let agents = running_now();
    let source = find(&agents, from)?;
    let client = service().ok_or("Dragent's terminal service isn't running")?;
    let screen = screen_text(&client, &source.session, 400)?;
    let screen = screen.trim();
    let start = screen
        .char_indices()
        .rev()
        .nth(HANDOVER_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let task = if prompt.trim().is_empty() {
        "Carry on with this work.".to_string()
    } else {
        prompt.to_string()
    };
    Ok(format!(
        "Another agent ({} in {}) was working on this. The end of its session:\n\n```\n{}\n```\n\n{task}",
        source.session.provider,
        source.session.cwd.display(),
        &screen[start..],
    ))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// `3m`, `2h`, `5d`.
fn ago(now: u64, then: u64) -> String {
    let minutes = now.saturating_sub(then) / 60_000;
    match minutes {
        0 => "<1m".into(),
        1..=59 => format!("{minutes}m"),
        60..=1439 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_reports_read_as_states() {
        assert_eq!(State::from_activity("Start"), State::Working);
        assert_eq!(State::from_activity("PermissionRequest"), State::NeedsInput);
        assert_eq!(State::from_activity("Stop"), State::Waiting);
        assert_eq!(
            State::from_activity(dragent_host::agent_sessions::ACTIVITY_QUIET),
            State::Waiting
        );
        assert_eq!(State::from_activity("Failed"), State::Failed);
        assert_eq!(State::label_for(State::NeedsInput.key()), "needs your input");
        assert_eq!(ago(10 * 60_000, 0), "10m");
        assert_eq!(ago(3 * 3_600_000, 0), "3h");
    }

    fn agent(pane: &str, cwd: &str) -> Running {
        Running {
            session: serde_json::from_value(serde_json::json!({
                "pane_uuid": pane,
                "generation": "00000000-0000-0000-0000-000000000001",
                "provider": "claude",
                "cwd": cwd,
                "executable": "claude",
                "resume_options": null,
                "conversation_id": null,
                "active": true,
                "workspace_id": "w",
                "terminal_id": "t",
            }))
            .unwrap(),
            state: State::Running,
            since_ms: None,
        }
    }

    #[test]
    fn agents_are_found_by_id_start_or_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        let two = dir.path().join("two");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        let agents = [
            agent("aaaa1111-0000-0000-0000-000000000000", &one.display().to_string()),
            agent("bbbb2222-0000-0000-0000-000000000000", &two.display().to_string()),
            agent("bbbb3333-0000-0000-0000-000000000000", &two.display().to_string()),
        ];
        assert_eq!(find(&agents, "aaaa").unwrap().id(), "aaaa1111");
        assert_eq!(find(&agents, &one.display().to_string()).unwrap().id(), "aaaa1111");
        assert!(find(&agents, "bbbb").unwrap_err().contains("matches 2 agents"));
        assert!(find(&agents, &two.display().to_string()).is_err());
        assert!(find(&agents, "cccc").unwrap_err().contains("no running agent"));
        assert!(find(&agents, "aa").is_err(), "too short to be safe");
    }
}
