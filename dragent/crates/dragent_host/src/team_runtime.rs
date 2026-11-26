//! Headless coordination transport. All commands, Git I/O and PTYs stay on
//! this host; a desktop connection is only a viewer/controller.
use crate::team::{TeamRequest, TeamResponse};
use crate::team_commands::{Caller, Services};
use crate::{HostClient, agent_sessions, cli_requests as requests};
use serde_json::{Value, json};
use std::path::Path;

pub fn call(tool: &str, args: &Value, caller: &Caller) -> Result<Value, String> {
    let client = HostClient::connect_existing()?;
    if tool == "host_mcp_configure" {
        if caller.agent {
            return Err("Only the person configures agent tools".into());
        }
        let enabled = args["enabled"]
            .as_bool()
            .ok_or("Choose whether agent tools are enabled")?;
        let report = crate::remote_mcp::configure(enabled)?;
        return Ok(
            json!({"updated":report.updated.iter().map(|agent|agent.key()).collect::<Vec<_>>(),"problems":report.problems}),
        );
    }
    if tool == "host_preflight" {
        let selected: Vec<String> = args["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        crate::remote_mcp::register(&selected)?;
        for agent in args["agents"].as_array().into_iter().flatten() {
            executable(agent.as_str().ok_or("Invalid agent")?)?;
        }
        return Ok(json!({"ready": true}));
    }
    if tool == "host_sessions" {
        let mut rows = Vec::new();
        for pane in args["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let Ok(id) = pane.parse() else { continue };
            if let Some(record) = agent_sessions::load(id) {
                rows.push(json!({"pane":pane,"record":record,"activity":agent_sessions::load_activity(id)}));
            }
        }
        return Ok(json!(rows));
    }
    let mut runtime = Runtime {
        client,
        pending: Vec::new(),
    };
    if tool == "host_start_lead" {
        if caller.agent {
            return Err("Only the person can replace a coordination lead.".into());
        }
        let team = args["team"].as_str().ok_or("Choose a team")?;
        let TeamResponse::Team(current) = runtime
            .client
            .team(TeamRequest::Get { team: team.into() })?
        else {
            return Err("Team unavailable".into());
        };
        if !matches!(
            current.status,
            crate::team::TeamStatus::Planning | crate::team::TeamStatus::Running
        ) {
            return Err("This team is finished".into());
        }
        if let Some(pane) = current.lead.as_ref().and_then(|lead| lead.pane.as_deref()) {
            if runtime.alive(pane) {
                return Err("The team's lead is still running.".into());
            }
        }
        let agent = args["agent"].as_str().ok_or("Choose a lead agent")?;
        let model = args["model"].as_str().map(str::to_owned);
        let cwd = current
            .lead
            .as_ref()
            .and_then(|lead| lead.pane.as_deref())
            .and_then(|pane| pane.parse().ok())
            .and_then(agent_sessions::load)
            .map(|record| record.cwd)
            .filter(|cwd| cwd.is_dir())
            .unwrap_or_else(|| current.project_root.clone().into());
        let response = runtime.launch(requests::Request::StartAgent {
            workspace: cwd,
            agent: Some(agent.into()),
            prompt: crate::team_commands::takeover_prompt(&current),
            model: model.clone(),
            account: Default::default(),
        })?;
        let pane = response.agent_id.as_deref().ok_or("Missing lead session")?;
        runtime.client.team(TeamRequest::SetLead {
            team: team.into(),
            lead: crate::team::Member {
                pane: Some(pane.into()),
                terminal: None,
                agent: Some(agent.into()),
                ..Default::default()
            },
            model,
        })?;
        runtime.launch_confirmed(pane)?;
        return serde_json::to_value(response).map_err(|e| e.to_string());
    }
    crate::team_commands::call_with(tool, args, caller, &mut runtime)
}

/// Teams this host is starting tasks for right now.
fn starting() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static STARTING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    STARTING.get_or_init(Default::default)
}

/// Start `team`'s due tasks off the courier's thread, one batch at a time.
pub fn start_due_in_background(team: String, due: Vec<String>) {
    if !starting()
        .lock()
        .map(|mut teams| teams.insert(team.clone()))
        .unwrap_or(false)
    {
        return;
    }
    std::thread::spawn(move || {
        match HostClient::connect_existing() {
            Ok(client) => {
                let mut runtime = Runtime {
                    client,
                    pending: Vec::new(),
                };
                let outcome = crate::team_commands::start_due(&team, &due, &mut runtime);
                log::info!("[team] {team} started on its own: {outcome}");
            }
            Err(error) => log::warn!("[team] couldn't start {team}'s ready tasks: {error}"),
        }
        if let Ok(mut teams) = starting().lock() {
            teams.remove(&team);
        }
    });
}

struct Runtime {
    client: HostClient,
    pending: Vec<(String, String, crate::protocol::TerminalSession)>,
}
fn session(pane: &str) -> Result<agent_sessions::AgentSession, String> {
    pane.parse()
        .ok()
        .and_then(agent_sessions::load)
        .filter(|record| record.active)
        .ok_or_else(|| "Its agent isn't running any more.".into())
}
fn executable(key: &str) -> Result<(String, Vec<String>), String> {
    let key = crate::team_command_support::agent_key(key)?;
    if key == "dragent" {
        return Err("Choose a terminal agent to coordinate on an SSH machine.".into());
    }
    if crate::agent_recovery::builtin(&key).is_none() {
        return Err(format!(
            "{key} doesn't yet support tracked coordination on an SSH machine. Choose Claude Code, Codex, OpenCode, Gemini or another agent with session tracking."
        ));
    }
    let words = crate::host_tools::launch_words(&key).unwrap_or_else(|| vec![key.clone()]);
    let command = &words[0];
    if words.len() > 1 && crate::agent_recovery::executable_name(command) != key {
        return Err(format!(
            "Choose {key}'s executable path on this machine for coordination; package-runner launch commands cannot preserve its session identity."
        ));
    }
    let found = if Path::new(command).is_absolute() {
        Path::new(command)
            .is_file()
            .then(|| Path::new(command).to_path_buf())
    } else {
        crate::host_tools::find_in(command, &crate::host_tools::search_dirs())
    };
    let found = found.ok_or_else(||format!("{key} isn't installed on this machine. Install and sign in to it before starting coordination."))?;
    Ok((found.to_string_lossy().into_owned(), words[1..].to_vec()))
}
impl Runtime {
    fn launch(&mut self, request: requests::Request) -> Result<requests::Response, String> {
        let (root, agent, prompt, model, branch, base, create) = match request {
            requests::Request::CreateWorkspace {
                project,
                agent,
                prompt,
                model,
                branch,
                base_branch,
                ..
            } => (project, agent, prompt, model, branch, base_branch, true),
            requests::Request::StartAgent {
                workspace,
                agent,
                prompt,
                model,
                ..
            } => (workspace, agent, prompt, model, None, None, false),
            _ => return Err("This request isn't a coordination worker launch.".into()),
        };
        let agent = agent.ok_or("Choose a worker agent")?;
        let (program, mut words) = executable(&agent)?;
        if let Some(model) = model {
            crate::team_command_support::check_model(&agent, &model)?;
            words.extend(["--model".into(), model]);
        }
        // No approval-bypass flags: workers use the machine's own agent login
        // and normal permissions. Prompts are quoted once, never shell code.
        let recovery = crate::agent_recovery::builtin(&agent)
            .ok_or("This agent has no remote session tracking")?;
        if recovery.positionals != crate::agent_recovery::Positionals::InteractivePrompt {
            words.push(
                recovery
                    .prompt_flags
                    .first()
                    .ok_or("This agent has no interactive prompt option")?
                    .clone(),
            );
        }
        words.push(prompt);
        let root = root
            .canonicalize()
            .map_err(|e| format!("Project unavailable: {e}"))?;
        let workspace = self.client.create_workspace(&root.to_string_lossy())?;
        let (cwd, branch) = if create {
            let branch = branch.ok_or("A worker needs a branch name")?;
            let stem = crate::worktrees::sanitize_branch(&branch);
            let mut branch = stem.clone();
            for suffix in 0..100 {
                let probe = crate::host_tools::command("git")
                    .arg("-C")
                    .arg(&root)
                    .args([
                        "show-ref",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{branch}"),
                    ])
                    .status()
                    .map_err(|e| e.to_string())?;
                if probe.code() == Some(1) {
                    break;
                }
                if !probe.success() || suffix == 99 {
                    return Err("Couldn't choose a free worker branch".into());
                }
                branch = format!("{stem}-{}", suffix + 2);
            }
            let revision = format!("{}^{{commit}}", base.as_deref().unwrap_or("HEAD"));
            let resolved = crate::host_tools::command("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "--verify", "--end-of-options", &revision])
                .output()
                .map_err(|e| e.to_string())?;
            if !resolved.status.success() {
                return Err(String::from_utf8_lossy(&resolved.stderr).trim().into());
            }
            let base = String::from_utf8_lossy(&resolved.stdout).trim().to_string();
            let directory = dragent_paths::organisation_dir()
                .join("team-worktrees")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(directory.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut git = crate::host_tools::command("git");
            git.arg("-C")
                .arg(&root)
                .args(["worktree", "add", "-b", &branch])
                .arg(&directory);
            git.arg(&base);
            let output = git
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().into());
            }
            (directory, Some(branch))
        } else {
            let output = crate::host_tools::command("git")
                .arg("-C")
                .arg(&root)
                .args(["branch", "--show-current"])
                .output()
                .map_err(|e| e.to_string())?;
            (
                root.clone(),
                Some(String::from_utf8_lossy(&output.stdout).trim().into()),
            )
        };
        let home = self.client.ensure_terminal_home(
            &workspace,
            &root.to_string_lossy(),
            &cwd.to_string_lossy(),
        )?;
        let shell = crate::host_tools::login_shell()
            .to_string_lossy()
            .into_owned();
        let pane = uuid::Uuid::new_v4();
        agent_sessions::stage_custom_agent(pane, &agent, &program, &recovery)?;
        let quote = |s: &str| agent_sessions::quote(s, &shell);
        let command = std::iter::once(program.as_str())
            .chain(words.iter().map(String::as_str))
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ");
        let command = agent_sessions::wrap_command(pane, &shell, &command);
        let terminal = self.client.create_terminal_with_shell(
            &workspace,
            &home.id,
            None,
            Some(&cwd.to_string_lossy()),
            120,
            40,
            Some(&shell),
        )?;
        // Return the reserved terminal first; the engine persists membership
        // before launch_confirmed lets its CLI start issuing team commands.
        self.pending.push((pane.to_string(), command, terminal));
        Ok(requests::Response {
            ok: true,
            workspace: Some(cwd),
            branch,
            agent: Some(agent),
            agent_id: Some(pane.to_string()),
            ..Default::default()
        })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        for (_, _, terminal) in self.pending.drain(..) {
            let _ = self
                .client
                .close_terminal(&terminal.workspace_id, &terminal.id);
        }
    }
}
impl Services for Runtime {
    fn team(&mut self, request: TeamRequest) -> Result<TeamResponse, String> {
        self.client.team(request)
    }
    fn app_open(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn app(&mut self, request: requests::Request) -> Result<requests::Response, String> {
        self.launch(request)
    }
    fn launch_confirmed(&mut self, pane: &str) -> Result<(), String> {
        let index = self
            .pending
            .iter()
            .position(|entry| entry.0 == pane)
            .ok_or("No pending worker launch")?;
        let (_, command, terminal) = self.pending.remove(index);
        if let Err(error) = self.client.send_terminal_input(
            &terminal.workspace_id,
            &terminal.id,
            format!("{command}\r").as_bytes(),
        ) {
            let _ = self
                .client
                .close_terminal(&terminal.workspace_id, &terminal.id);
            return Err(error);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while session(pane).is_err() {
            if std::time::Instant::now() >= deadline {
                let _ = self
                    .client
                    .close_terminal(&terminal.workspace_id, &terminal.id);
                return Err("The remote agent launch wasn't acknowledged. Open its machine to check its CLI and sign-in.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        Ok(())
    }
    fn alive(&mut self, pane: &str) -> bool {
        session(pane).is_ok_and(|s| {
            self.client
                .terminal(&s.workspace_id, &s.terminal_id)
                .is_ok_and(|t| t.state == crate::protocol::TerminalState::Running)
        })
    }
    fn screen(&mut self, pane: &str, lines: usize) -> Result<String, String> {
        let s = session(pane)?;
        let screen = self
            .client
            .read_terminal_screen(&s.workspace_id, &s.terminal_id)?;
        let mut rows: Vec<_> = screen
            .lines
            .iter()
            .map(|r| {
                r.spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        Ok(rows[rows.len().saturating_sub(lines)..].join("\n"))
    }
    fn keys(&mut self, pane: &str, keys: &[Vec<u8>]) -> Result<(), String> {
        let s = session(pane)?;
        for key in keys {
            self.client
                .send_terminal_input(&s.workspace_id, &s.terminal_id, key)?;
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        Ok(())
    }
    fn type_text(&mut self, pane: &str, text: &str) -> Result<(), String> {
        let s = session(pane)?;
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        self.client.send_terminal_input(
            &s.workspace_id,
            &s.terminal_id,
            format!("\x1b[200~{clean}\x1b[201~").as_bytes(),
        )?;
        std::thread::sleep(std::time::Duration::from_millis(250));
        self.client
            .send_terminal_input(&s.workspace_id, &s.terminal_id, b"\r")
    }
    fn uncommitted(&mut self, folder: &str) -> Result<Vec<String>, String> {
        Ok(self
            .git(folder, &["status", "--porcelain"])?
            .lines()
            .map(|line| line.get(3..).unwrap_or(line).to_string())
            .collect())
    }
    fn git(&mut self, folder: &str, args: &[&str]) -> Result<String, String> {
        self.git_env(folder, args, &[])
    }
    fn git_env(
        &mut self,
        folder: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<String, String> {
        let out = crate::host_tools::command("git")
            .arg("-C")
            .arg(folder)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .envs(env.iter().copied())
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().into())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().into())
        }
    }
}

/// The remote service uses its own CLI; never replace the desktop launcher.
pub fn install_cli() -> Result<(), String> {
    if std::env::var_os("DRAGENT_REMOTE_HOST_BINARY").is_none() {
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin = dragent_paths::organisation_dir().join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let context = format!(
        "export DRAGENT_HOME={}\nexport DRAGENT_HOST_ENDPOINT_FILE={}\n",
        crate::remote::quote(&dragent_paths::home().to_string_lossy()),
        crate::remote::quote(&crate::discovery::endpoint_path().to_string_lossy())
    );
    let executable = crate::remote::quote(&executable.to_string_lossy());
    let org = crate::remote::quote(&dragent_paths::organisation_arg());
    let script = format!(
        "#!/bin/sh\n{context}case \"$1\" in\n  --dragent-agent-run|--dragent-agent-session-hook) exec {executable} {org} \"$@\" ;;\n  *) exec {executable} {org} --dragent-cli \"$@\" ;;\nesac\n"
    );
    let path = bin.join("dragent");
    let temp = bin.join(format!(".dragent-{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp, script).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    crate::remote_mcp::register(&[])?;
    Ok(())
}
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args().collect();
    let start = args.iter().position(|a| a == "--dragent-cli")?;
    let command = args.get(start + 1).map(String::as_str).unwrap_or("help");
    if command == "mcp" {
        if !crate::agent_skills::inside_dragent() {
            eprintln!("Dragent's tools work only in agents started from Dragent. Start the agent from Dragent to use them.");
            return Some(1);
        }
        let result = crate::mcp::serve(
            "Dragent coordination on this machine. The team_, task_ and msg_ tools share the same team store and permissions as dragent team, tasks and msg. Leads plan tasks; workers report results and send messages to their lead. Read team_guide before coordinating. Connected-service tools use the signed-in desktop while it is connected; credentials remain there.",
            HostClient::connect_existing()
                .and_then(|client| {
                    client.coordination(
                        "host_services_catalog",
                        json!({}),
                        serde_json::to_value(Caller::person()).unwrap(),
                    )
                })
                .ok()
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_else(|| crate::service_mcp::catalog(&[]))
                .into_iter()
                .chain(crate::mcp::coordination_tools())
                .collect(),
            |name, arguments| {
                if !crate::remote_mcp::enabled() {
                    return Err("Agent tools are disabled in Dragent settings".into());
                }
                if name == "team_guide" {
                    return Ok(crate::team_commands::LEAD_PLAYBOOK.into());
                }
                if crate::service_mcp::tool(name).is_some() {
                    let value = HostClient::connect_existing()?.coordination(
                        "host_services_call",
                        json!({"name":name,"arguments":arguments}),
                        serde_json::to_value(Caller::here(true)).unwrap(),
                    )?;
                    return value["text"]
                        .as_str()
                        .map(str::to_string)
                        .ok_or("The desktop returned no service answer".into());
                }
                call(name, arguments, &Caller::here(true))
                    .map(|value| serde_json::to_string_pretty(&value).unwrap_or_default())
            },
        );
        return Some(match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        });
    }
    let result = match command {
        "integrations" | "integration" => (|| {
            let Some(parsed)=crate::integration_commands::parse(&args[start+2..])? else {return Ok(crate::integration_commands::HELP.into());};
            let mut arguments=parsed.arguments; arguments["project"]=json!(parsed.root);
            let value=HostClient::connect_existing()?.coordination("host_services_call",json!({"name":parsed.tool,"arguments":arguments}),serde_json::to_value(Caller::here(false)).unwrap())?;
            let text=value["text"].as_str().ok_or("The desktop returned no service answer")?.to_string();
            if parsed.quiet {
                let data:Value=serde_json::from_str(&text).map_err(|e|e.to_string())?;
                let rows=data.as_array().or_else(||data["items"].as_array());
                return Ok(match rows { Some(rows)=>rows.iter().filter_map(|row|row["id"].as_str().or_else(||row["service"].as_str())).collect::<Vec<_>>().join("\n"),None=>data["id"].as_str().unwrap_or_default().to_string() });
            }
            Ok(text)
        })(),
        "team" | "teams" => crate::team_commands::run_with("team", &args[start + 2..], call),
        "tasks" | "task" => crate::team_commands::run_with("tasks", &args[start + 2..], call),
        "msg" | "message" => crate::team_commands::run_with("msg", &args[start + 2..], call),
        "help" | "--help" | "-h" => Ok(format!(
            "Dragent on {} (a machine Dragent reaches over SSH)\n\n  dragent team help\n  dragent tasks help\n  dragent msg help\n  dragent integrations help\n  dragent status               This machine and whether the desktop is reachable\n  dragent capabilities --json  What this machine offers\n  dragent mcp                  Serve the same coordination tools over MCP (stdio)\n\nBrowser panes, pane layout, desktop control, workspaces, automations and setup belong to the desktop running the Dragent window.",
            crate::scope::machine_name()
        )),
        "version" | "--version" | "-V" => Ok(format!("dragent {}", env!("CARGO_PKG_VERSION"))),
        "status" => Ok(serde_json::to_string_pretty(&remote_status()).unwrap_or_default()),
        "capabilities" => Ok(serde_json::to_string_pretty(&remote_capabilities()).unwrap_or_default()),
        _ if DESKTOP_ONLY.contains(&command) => Err(crate::outcome::Failure::new(
            crate::outcome::Status::Unsupported,
            "not_on_this_machine",
            format!(
                "`dragent {command}` acts in the Dragent window on the desktop computer; this is {}, a machine Dragent reaches over SSH, which has no window.",
                crate::scope::machine_name()
            ),
        )
        .next("Ask the person to do it from the desktop, or use what this machine offers: `dragent capabilities --json`.")
        .encode()),
        _ => Err(crate::outcome::Failure::new(
            crate::outcome::Status::InvalidInput,
            "unknown_command",
            format!("Unknown command: {command}."),
        )
        .next("`dragent help` lists the commands on this machine.")
        .encode()),
    };
    let json = args.iter().any(|arg| arg == "--json");
    Some(match result {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => match crate::outcome::Failure::decode(&error) {
            Some(failure) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&failure.to_json()).unwrap_or_default());
                }
                eprintln!("dragent: {}", failure.message);
                if let Some(next) = &failure.next {
                    eprintln!("{next}");
                }
                failure.status.exit_code().max(1)
            }
            None => {
                eprintln!("{error}");
                1
            }
        },
    })
}

/// Commands of the desktop's `dragent` that act in its window or its own
/// state, and so don't exist on a machine reached over SSH.
const DESKTOP_ONLY: &[&str] = &[
    "browser", "panes", "pane", "computer", "open", "open-url", "workspaces", "workspace",
    "projects", "project", "agents", "agent", "automations", "automation", "setup", "standup",
    "doctor", "settings", "theme", "feedback", "requests", "request", "machines", "work", "org",
    "whoami",
];

/// This machine, and whether the desktop is reachable for connected
/// services.
fn remote_status() -> Value {
    let desktop = HostClient::connect_existing()
        .and_then(|client| {
            client.coordination(
                "host_services_catalog",
                json!({}),
                serde_json::to_value(Caller::person()).unwrap_or_default(),
            )
        })
        .is_ok();
    json!({
        "machine": crate::scope::this_machine(),
        "version": env!("CARGO_PKG_VERSION"),
        "desktop_reachable": desktop,
        "note": if desktop {
            "The desktop is connected: connected-service commands go through it."
        } else {
            "The desktop isn't reachable now: team commands work here; connected services wait for it."
        },
    })
}

/// What `dragent` offers on this machine, for agents to check before
/// relying on a command.
fn remote_capabilities() -> Value {
    json!({
        "machine": crate::scope::this_machine(),
        "commands": ["team", "tasks", "msg", "integrations", "mcp", "status", "capabilities", "help", "version"],
        "desktop_only": DESKTOP_ONLY,
        "provides": crate::scope::ssh_machine_provides(),
        "results": crate::outcome::contract(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_machine_says_what_it_offers_and_what_belongs_to_the_desktop() {
        let offered = remote_capabilities();
        let commands: Vec<&str> = offered["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for command in ["team", "tasks", "msg", "integrations", "status", "capabilities"] {
            assert!(commands.contains(&command), "{command}");
        }
        for command in ["browser", "panes", "computer", "workspaces", "automations", "setup"] {
            assert!(DESKTOP_ONLY.contains(&command), "{command}");
            assert!(!commands.contains(&command), "{command}");
        }
        assert!(offered["machine"]["name"].as_str().is_some_and(|name| !name.is_empty()));
        assert!(offered["results"].is_object());
    }
}

