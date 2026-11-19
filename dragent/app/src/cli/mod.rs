//! The `dragent` command line, for people and agents in a terminal.
//!
//! Launchers call the app binary with [`CLI_ARG`] first, so a command never
//! starts the app itself (a second copy of the app would sign the first one
//! out). Two are installed: `~/.dragent/bin/dragent`, for any terminal, works
//! in the organisation the app last had open; one per organisation is first
//! on the PATH of Dragent's own terminals, so they act in the window's
//! organisation.

mod account;
mod agent_accounts;
pub mod agent_skill;
mod agents;
pub(crate) mod app_tools;
mod browser;
mod capabilities;
pub(crate) mod computer;
pub(crate) mod doctor;
mod idempotency;
pub mod inbox;
pub mod instance;
mod integrations;
mod issues;
pub mod model;
mod panes;
pub(crate) mod plugins;
pub(crate) mod presentation;
pub(crate) mod scope;
pub mod requests;
pub mod settings;
pub(crate) mod setup;
pub mod shell_setup;
#[cfg(test)]
mod skill_evals;
pub(crate) mod standup;
pub(crate) mod team;
mod work;
pub(crate) use agents::type_into_session;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

static EXPLICIT_ORGANISATION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static LIVE_ORGANISATION: std::sync::Mutex<
    Option<crate::organisation_context::ActiveOrganisation>,
> = std::sync::Mutex::new(None);

pub(crate) fn live_organisation() -> Option<crate::organisation_context::ActiveOrganisation> {
    LIVE_ORGANISATION.lock().ok()?.clone()
}

use crate::workspace::PromptAgent;

/// First argument from a launcher: run a command, never the app.
pub const CLI_ARG: &str = "--dragent-cli";

/// Commands also accepted without [`CLI_ARG`], when the binary is run
/// directly.
pub(crate) const COMMANDS: [&str; 36] = [
    "capabilities",
    "requests",
    "computer",
    "open",
    "work",
    "team",
    "teams",
    "tasks",
    "task",
    "issues",
    "msg",
    "message",
    "integrations",
    "integration",
    "mcp",
    "status",
    "standup",
    "doctor",
    "setup",
    "plugins",
    "automations",
    "automation",
    "workspaces",
    "workspace",
    "projects",
    "project",
    "agents",
    "agent",
    "org",
    "whoami",
    "machines",
    "settings",
    "theme",
    "feedback",
    "help",
    "version",
];

const HELP: &str = "\
Dragent from the command line.

Usage: dragent <command> [options]

Workspaces and projects
  workspaces list [--project PATH]      Workspaces in the sidebar, with branch and agents
  workspaces create [options]           A new workspace with an agent working in it
  workspaces delete PATH [--force]      Close a workspace and remove its worktree
  projects list                         Projects open in Dragent
  projects add PATH | URL [--into DIR]  Open a folder as a project, or clone a repository
  open [PATH]                           Show a folder in Dragent (starts Dragent if needed)

Agents
  agents accounts list [--agent KEY]    Provider accounts on this machine
  agents accounts capabilities          Verified isolation support by agent
  agents accounts add --agent KEY --name NAME
  agents accounts login ID --text       Native sign-in on this machine
  agents accounts status ID             Check native sign-in
  agents accounts rename ID --name NAME
  agents accounts remove ID             Remove metadata; retain native history
  agents accounts default --agent KEY --account ID|system|inherit [--project PATH]
  agents list                           Agents Dragent can start on this computer
  agents running [--project PATH]       Agents at work, and what each is doing
  agents read AGENT [--lines N]         An agent's terminal, as text
  agents send AGENT TEXT [--no-enter]   Type a message into an agent's session
  agents stop AGENT                     End an agent's session
  agents start --workspace PATH [--agent KEY] (--prompt TEXT | --prompt-file FILE)
               [--model NAME] [--from AGENT] [--account ID|system|inherit]
                                        Another agent in an existing workspace
  AGENT is an id from `agents running` (its first characters are enough) or a
  workspace path with one agent in it.

Browser (the browser pane beside this terminal; the person watches)
  browser open URL | snapshot | click REF | type REF TEXT
                                        Open pages, read them, act on them (`dragent browser help`)
  open-url URL                          Open a link as a click in Dragent would (Dragent's
                                        browser for web pages); Dragent terminals' BROWSER

Computer (desktop apps on this machine)
  computer permissions | apps | snapshot APP | click APP ELEMENT
                                        Read windows and act (`dragent computer help`)

Panes (the tabs and panes of this terminal's workspace; the person watches)
  panes list | open [--browser] [--right|--down] [COMMAND] | focus ID | close ID
                                        Arrange them (`dragent panes help`)

Automations
  automations ...                       Agents on a schedule (`dragent automations help`)

Teams (a lead agent and its workers)
  team create GOAL [--agents claude,codex]  Start a team; an agent that runs this leads it
  team list | team show [TEAM] | team stop [TEAM]
   tasks add | list | show | edit | done | block | wait ...
                                         The plan, and reporting on it (`dragent team help`)
   issues list | show | create | update | comment | link | delete | restore ...
                                         The organisation's task list (`dragent issues help`)
  msg send TO TEXT | msg reply ID TEXT | msg inbox
                                        The team's mailbox: lead, user, or a task's id
  standup [--project PATH] [--since WHEN] [--limit N] [--setup]
                                        Read-only recap across workspaces, agents, teams, runs
  doctor [--check ID,ID] [--project PATH]
                                        Diagnose Dragent layer by layer (`dragent doctor --help`)
  setup check|init|launch|status|stop|cleanup
                                        Repository setup for workspaces (`.dragent/workspace.json`)
  plugins inspect|install|list|status|check|update|disable|enable|accounts|remove|sync
                                        Packages of skills and MCP servers (`dragent plugins help`)

Connected services (Settings → Integrations)
  integrations list                     Linear, Jira, Sentry, Slack, …: connected or not
  integrations search SERVICE [WORDS]   Find issues, tasks, errors, threads (`dragent integrations help`)
  integrations describe | show | comment | status | create | api ...
  mcp                                   The same as an MCP server on stdio, for other agents

Account and app
  capabilities                          What this build supports: commands, tool schemas,
                                        skills, platform limits and the result contract (JSON)
  requests show ID                      Whether the app finished a request a command stopped
                                        waiting for, and what it answered
  status                                Whether Dragent is running, and the organisation used
  whoami                                The signed-in user and organisation
  org list | org switch NAME            Your organisations, and switching between them
  machines list | show ID               SSH machines added to Dragent: status, projects,
                                        what runs there
  settings list | get KEY | set KEY VALUE | reset KEY
  theme list | theme set ID
  feedback TEXT                         Send feedback to the Dragent team
  help | version

Options for workspaces create
  --project PATH       The project (default: the one you're in, or your only one)
  --agent KEY          claude, codex, gemini, dragent, ... (`dragent agents list`)
  --prompt TEXT | --prompt-file FILE
                       What the agent should do (no agent and no prompt: a terminal)
  --name NAME          The workspace's name in the sidebar
  --branch NAME        Name for the new branch (default: made from the prompt)
  --base-branch NAME   Branch to start from (default: the project's main branch)
  --pr NUMBER          Start from a pull request's head
  --local              Use the project's own checkout instead of a new worktree
  --command CMD        Run a shell command in the workspace once it's open
  --tag NAME           File it in a sidebar folder (repeatable)
  --model NAME         The agent's model

Output: tables for people, JSON for scripts and agents (--json, on by default
when an agent or CI runs the command; --text to turn it off). --quiet prints
only ids and paths. Failures in JSON mode are an object with status, code,
message, next and retry_safe, and each status has its own exit code
(`dragent capabilities --json` lists them). --idempotency-key KEY runs a command
at most once per key: a repeat replays the first result.
";

/// Every word `dispatch` accepts as a command, aliases included.
const KNOWN_COMMANDS: &[&str] = &[
    "help",
    "--help",
    "-h",
    "version",
    "--version",
    "-V",
    "capabilities",
    "requests",
    "request",
    "automations",
    "automation",
    "integrations",
    "integration",
    "work",
    "browser",
    "computer",
    "panes",
    "pane",
    "open-url",
    "team",
    "teams",
    "tasks",
    "task",
    "issues",
    "msg",
    "message",
    "mcp",
    "open",
    "status",
    "workspaces",
    "workspace",
    "projects",
    "project",
    "agents",
    "agent",
    "org",
    "whoami",
    "machines",
    "feedback",
    "settings",
    "theme",
    "standup",
    "doctor",
    "setup",
    "plugins",
    "plugin",
];

/// Run a command if this process was started as one. `None` starts the app.
pub fn run_if_requested() -> Option<i32> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let explicit_org = raw
        .iter()
        .any(|arg| arg.starts_with(&format!("{}=", dragent_paths::ORGANISATION_ARG)));
    EXPLICIT_ORGANISATION.store(explicit_org, std::sync::atomic::Ordering::Relaxed);
    let mut args = dragent_paths::take_organisation_arg(raw);
    if args.first().map(String::as_str) == Some(CLI_ARG) {
        args.remove(0);
    } else if !args
        .first()
        .is_some_and(|arg| COMMANDS.contains(&arg.as_str()))
    {
        return None;
    }
    // The app binary has no console of its own on Windows (it's a GUI
    // program); write to the terminal that ran the command.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    if !explicit_org {
        // The organisation the app last had selected.
        crate::organisation_context::restore_at_launch();
    }
    Some(match dispatch(&args) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{}", presentation::output(&args, &output));
            }
            0
        }
        Err(error) => {
            let failure = Failure::from_error(&error);
            if presentation::wants_json(&args) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&failure.to_json()).unwrap_or_default()
                );
            }
            eprintln!("{}", presentation::error(&args, &error));
            failure.status.exit_code().max(1)
        }
    })
}

/// Why a command was refused while signed out.
pub(crate) const SIGN_IN_FIRST: &str =
    "You're not signed in to Dragent. Open Dragent and sign in, \
then run this again: the command line and the MCP server only work while you're signed in.";

use dragent_host::outcome::{Failure, Status};

/// [`SIGN_IN_FIRST`] as a structured failure.
pub(crate) fn signed_out() -> String {
    Failure::new(Status::NeedsAuth, "signed_out", SIGN_IN_FIRST)
        .next("Ask the person to open Dragent and sign in, then run the command again.")
        .encode()
}

/// The app took request `id` but hadn't answered when the caller stopped
/// waiting: it may still finish.
fn no_answer(id: &str) -> String {
    Failure::new(Status::Timeout, "no_answer", "Dragent didn't answer in time")
        .request(id)
        .next(format!(
            "It may still finish: `dragent requests show {id}` says whether it did and what it answered. Check that before running the command again."
        ))
        .encode()
}

/// The app's refusal or error for request `id`, keeping its status and code.
fn app_failure(response: requests::Response, id: &str) -> String {
    match response.failure {
        Some(failure) => failure.request(id).encode(),
        None => Failure::new(
            Status::Failed,
            "app_error",
            response
                .error
                .unwrap_or_else(|| "Dragent couldn't do it".into()),
        )
        .request(id)
        .encode(),
    }
}

/// Prefer the app's live identity. When no app answers (including older
/// versions), retain the saved-session check for offline commands.
pub(crate) fn signed_in() -> bool {
    if let Some(identity) = live_identity() {
        return identity["signed_in"].as_bool() == Some(true);
    }
    crate::auth::has_saved_session()
        && crate::organisation_context::cached_active()
            .is_some_and(|active| active.context == dragent_paths::organisation_id())
}

/// Ask the app that owns the session, rather than trusting a stale cache or
/// requiring this CLI binary to have access to the app's credential store.
/// This never exchanges, copies, or returns a token.
pub(super) fn live_identity() -> Option<serde_json::Value> {
    *LIVE_ORGANISATION.lock().ok()? = None;
    if !instance::app_running() {
        return None;
    }
    let explicit = EXPLICIT_ORGANISATION.load(std::sync::atomic::Ordering::Relaxed);
    let folder = if explicit {
        requests::scoped_identity_folder(&dragent_paths::organisation_id())
    } else {
        requests::identity_folder()
    };
    let id = requests::submit(&folder, requests::Request::Whoami).ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(response) = requests::take_response(&folder, &id) {
            let data = response.data.filter(|_| response.ok)?;
            let context = data["context"].as_str()?;
            if explicit {
                // An existing terminal is bound to its organisation. Never
                // silently move it into another account's organisation.
                if context != dragent_paths::organisation_id() {
                    return Some(serde_json::json!({ "signed_in": false }));
                }
            } else {
                dragent_paths::select_organisation(context);
            }
            *LIVE_ORGANISATION.lock().ok()? =
                Some(crate::organisation_context::ActiveOrganisation {
                    user_id: data["user_id"].as_str()?.to_string(),
                    organisation_id: data["organisation_id"].as_str()?.to_string(),
                    name: data["organisation"].as_str()?.to_string(),
                    context: context.to_string(),
                });
            return Some(data);
        }
        if Instant::now() >= deadline {
            requests::withdraw(&folder, &id);
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Commands that work signed out: help, and saying who and what is here.
/// `mcp` always starts, and refuses each tool call instead (see
/// `integrations::bridge`), so the agent can say why.
fn needs_sign_in(command: &str, rest: &[String]) -> bool {
    let help = rest.first().is_some_and(|arg| {
        matches!(arg.as_str(), "help" | "--help" | "-h")
            || (command == "computer" && arg == "guide")
    });
    // `feedback --draft` only renders the draft; it never contacts the app.
    let draft_preview = command == "feedback"
        && rest.iter().any(|arg| arg == "--draft" || arg.starts_with("--draft="));
    // The next-run preview only works out times from a schedule and a zone.
    let schedule_preview = matches!(command, "automations" | "automation")
        && rest.first().is_some_and(|arg| arg == "preview");
    !help
        && !draft_preview
        && !schedule_preview
        && !matches!(
            command,
            "help"
                | "--help"
                | "-h"
                | "version"
                | "--version"
                | "-V"
                | "capabilities"
                | "requests"
                | "status"
                | "standup"
                | "doctor"
                | "setup"
                | "plugins"
                | "plugin"
                | "whoami"
                | "mcp"
                | "open-url"
        )
}

pub(crate) fn dispatch(args: &[String]) -> Result<String, String> {
    let mut args = args.to_vec();
    match idempotency::take_key(&mut args)? {
        Some(key) => idempotency::run(&key, &args, || dispatch_command(&args)),
        None => dispatch_command(&args),
    }
}

fn dispatch_command(args: &[String]) -> Result<String, String> {
    let (command, rest) = match args.split_first() {
        Some((command, rest)) => (command.as_str(), rest),
        None => ("help", &[][..]),
    };
    // A mistyped command is the caller's to fix, signed in or not.
    if !KNOWN_COMMANDS.contains(&command) {
        return Err(Failure::new(
            Status::InvalidInput,
            "unknown_command",
            format!("unknown command “{command}”. Run `dragent help` to see the commands."),
        )
        .next("`dragent capabilities --json` lists every command this build has.")
        .encode());
    }
    if needs_sign_in(command, rest) && !signed_in() {
        return Err(signed_out());
    }
    match command {
        "help" | "--help" | "-h" => Ok(HELP.trim_end().to_string()),
        "version" | "--version" | "-V" => Ok(format!("dragent {}", env!("CARGO_PKG_VERSION"))),
        "capabilities" => capabilities::run(rest),
        "requests" | "request" => request_status(rest),
        "automations" | "automation" => {
            let mut rest = rest.to_vec();
            // The automations commands have their own options; give them
            // the same JSON default.
            if agent_environment() && !rest.iter().any(|arg| arg == "--text" || arg == "--json") {
                rest.push("--json".into());
            }
            rest.retain(|arg| arg != "--text");
            crate::automations::cli::run(&rest).map_err(|error| match Failure::decode(&error) {
                Some(_) => error,
                None => format!("automations: {error}"),
            })
        }
        "integrations" | "integration" => integrations::run(rest),
        "work" => work::run(rest),
        "browser" => browser::run(rest),
        "computer" => computer::run(rest),
        "panes" | "pane" => panes::run(rest),
        "open-url" => open_url(rest),
        "team" | "teams" => team::run("team", rest),
        "tasks" | "task" => team::run("tasks", rest),
        "issues" => issues::run(rest),
        "msg" | "message" => team::run("msg", rest),
        "mcp" => crate::integrations::bridge::serve().map(|()| String::new()),
        "open" => open(rest),
        "status" => status(rest),
        "standup" => standup::run(rest),
        "doctor" => doctor::run(rest),
        "setup" => setup::run(rest),
        "plugins" | "plugin" => plugins::run(rest),
        "workspaces" | "workspace" => workspaces(rest),
        "projects" | "project" => projects(rest),
        "agents" | "agent" => agents::run(rest),
        "org" => account::org(rest),
        "whoami" => account::whoami(rest),
        "machines" => account::machines(rest),
        "feedback" => account::feedback(rest),
        "settings" => settings::run(rest),
        "theme" => settings::theme(rest),
        other => Err(Failure::new(
            Status::InvalidInput,
            "unknown_command",
            format!("unknown command “{other}”. Run `dragent help` to see the commands."),
        )
        .next("`dragent capabilities --json` lists every command this build has.")
        .encode()),
    }
}

// ── Options and output ───────────────────────────────────────────────────────

/// Options that take no value.
const SWITCHES: [&str; 14] = [
    "json",
    "text",
    "quiet",
    "force",
    "no-enter",
    "local",
    "all",
    "peek",
    "keep-uncommitted",
    "approve",
    "changes",
    "qa",
    "draft",
    "clear-assignee",
];

struct Flags {
    positional: Vec<String>,
    values: Vec<(String, String)>,
    switches: Vec<String>,
    json: bool,
    quiet: bool,
}

impl Flags {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut flags = Flags {
            positional: Vec::new(),
            values: Vec::new(),
            switches: Vec::new(),
            json: false,
            quiet: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let Some(name) = arg.strip_prefix("--") else {
                flags.positional.push(arg.clone());
                continue;
            };
            if let Some((name, value)) = name.split_once('=') {
                flags.values.push((name.to_string(), value.to_string()));
            } else if SWITCHES.contains(&name) {
                flags.switches.push(name.to_string());
            } else if name == "help" {
                flags.positional.insert(0, "help".into());
            } else {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                flags.values.push((name.to_string(), value.clone()));
            }
        }
        // Asked-for output wins over the automatic JSON.
        flags.json =
            flags.on("json") || (!flags.on("text") && !flags.on("quiet") && agent_environment());
        flags.quiet = flags.on("quiet");
        Ok(flags)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }

    fn on(&self, name: &str) -> bool {
        self.switches.iter().any(|switch| switch == name)
    }

    fn subcommand(&self) -> &str {
        self.positional
            .first()
            .map(String::as_str)
            .unwrap_or("list")
    }

    /// The `index`th word after the subcommand.
    fn argument(&self, index: usize) -> Option<&str> {
        self.positional.get(index + 1).map(String::as_str)
    }

    /// `--prompt` or `--prompt-file`.
    fn prompt(&self) -> Result<String, String> {
        match (self.value("prompt"), self.value("prompt-file")) {
            (Some(_), Some(_)) => Err("use --prompt or --prompt-file, not both".into()),
            (Some(text), None) => Ok(text.to_string()),
            (None, Some(file)) => std::fs::read_to_string(absolute(file))
                .map_err(|error| format!("couldn't read {file}: {error}")),
            (None, None) => Ok(String::new()),
        }
    }
}

/// An agent or CI is running the command: they read JSON more reliably
/// than tables. Each agent marks the commands it runs with its own
/// variable.
pub(crate) fn agent_environment() -> bool {
    let set = |name: &str| {
        std::env::var_os(name)
            .is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    };
    [
        "CI",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "GEMINI_CLI",
        "QWEN_CODE",
        "DRAGENT_JSON",
    ]
    .iter()
    .any(|name| set(name))
}

/// Rows as JSON, as their ids (`--quiet`), or as tab-separated lines.
fn listing(
    flags: &Flags,
    rows: Vec<serde_json::Value>,
    id: impl Fn(&serde_json::Value) -> String,
    line: impl Fn(&serde_json::Value) -> String,
    empty: &str,
) -> String {
    if flags.json {
        return pretty(serde_json::Value::Array(rows));
    }
    if flags.quiet {
        return rows.iter().map(id).collect::<Vec<_>>().join("\n");
    }
    if let Some(output) = presentation::records(&rows, empty) {
        return output;
    }
    if rows.is_empty() {
        return empty.to_string();
    }
    rows.iter().map(line).collect::<Vec<_>>().join("\n")
}

fn text(value: &serde_json::Value, key: &str) -> String {
    match &value[key] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => "-".into(),
        other => other.to_string(),
    }
}

fn absolute(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    path.canonicalize().unwrap_or(path)
}

fn pretty(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// Validate an agent key from `--agent`.
fn agent_key(key: &str) -> Result<String, String> {
    crate::automations::executor::prompt_agent(key)
        .map(|agent| agent.command_key().to_string())
        .ok_or_else(|| format!("unknown agent “{key}” (see `dragent agents list`)"))
}

// ── Talking to the app ───────────────────────────────────────────────────────

/// The app takes requests within a second or two while it's open.
const CLAIM_WAIT: Duration = Duration::from_secs(10);
/// A just-started app takes its first request once its window is ready.
const STARTUP_WAIT: Duration = requests::UNCLAIMED_TTL;
/// Making a worktree and naming it can take a while in a large repository.
const ANSWER_WAIT: Duration = Duration::from_secs(10 * 60);

/// Hand `request` to the open app and wait up to `wait` (default: the
/// usual long wait) for its answer. `None` when it's still working on it
/// after `wait`. Never starts the app.
pub(crate) fn ask_app_within(
    request: requests::Request,
    wait: Option<Duration>,
) -> Result<Option<requests::Response>, String> {
    let folder = requests::folder();
    let id = requests::submit(&folder, request)?;
    match wait_for_answer_within(&folder, &id, CLAIM_WAIT, wait.unwrap_or(ANSWER_WAIT)) {
        Ok(Some(response)) if response.ok => Ok(Some(response)),
        Ok(Some(response)) => Err(app_failure(response, &id)),
        Ok(None) if wait.is_some() => Ok(None),
        Ok(None) => Err(no_answer(&id)),
        Err(error) => Err(error),
    }
}

/// Hand `request` to the open app and wait up to `wait` for its answer; no
/// answer in time is a timeout naming the request, to check before retrying.
pub(crate) fn ask_app_waiting(
    request: requests::Request,
    wait: Duration,
) -> Result<requests::Response, String> {
    let folder = requests::folder();
    let id = requests::submit(&folder, request)?;
    match wait_for_answer_within(&folder, &id, CLAIM_WAIT, wait)? {
        Some(response) if response.ok => Ok(response),
        Some(response) => Err(app_failure(response, &id)),
        None => Err(no_answer(&id)),
    }
}

/// Hand `request` to the open app and wait for its answer; with `start`,
/// start the app first if it isn't running.
pub(crate) fn ask_app(
    request: requests::Request,
    start: bool,
) -> Result<requests::Response, String> {
    let folder = requests::folder();
    let id = requests::submit(&folder, request)?;
    let claim_wait = if start && !instance::app_running() {
        if let Err(error) = instance::launch_app() {
            requests::withdraw(&folder, &id);
            return Err(Failure::new(Status::Unavailable, "app_start_failed", error).encode());
        }
        STARTUP_WAIT
    } else {
        CLAIM_WAIT
    };
    let response = wait_for_answer(&folder, &id, claim_wait)?;
    if response.ok {
        Ok(response)
    } else {
        Err(app_failure(response, &id))
    }
}

fn wait_for_answer(
    folder: &Path,
    id: &str,
    claim_wait: Duration,
) -> Result<requests::Response, String> {
    wait_for_answer_within(folder, id, claim_wait, ANSWER_WAIT)?.ok_or_else(|| no_answer(id))
}

/// `None` once `answer_wait` passes after the app took the request.
fn wait_for_answer_within(
    folder: &Path,
    id: &str,
    claim_wait: Duration,
    answer_wait: Duration,
) -> Result<Option<requests::Response>, String> {
    let started = Instant::now();
    while !requests::claimed(folder, id) {
        if !folder.join(format!("{id}.json")).exists() && !requests::claimed(folder, id) {
            // Taken by something that neither claimed nor answered it.
            return Err(Failure::new(
                Status::Unsupported,
                "request_unreadable",
                "Dragent couldn't read the request. Update Dragent and try again.",
            )
            .next("Update the Dragent app so it matches this command line.")
            .encode());
        }
        if started.elapsed() > claim_wait && requests::withdraw(folder, id) {
            return Err(if instance::app_running() {
                Failure::new(
                    Status::Unavailable,
                    "request_not_taken",
                    "Dragent didn't take the request: it may be open in another organisation, or have no window open.",
                )
                .next("Compare `dragent status --json` with the organisation you expect; don't switch organisations without asking.")
            } else {
                Failure::new(Status::Unavailable, "app_not_open", "Dragent isn't open. Open it, then run this again.")
                    .next("`dragent open PATH` starts it, if the request needs the app.")
            }
            .encode());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let started = Instant::now();
    loop {
        if let Some(response) = requests::take_response(folder, id) {
            return Ok(Some(response));
        }
        if started.elapsed() > answer_wait {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ── Saved state ──────────────────────────────────────────────────────────────

/// What Dragent saved: the sidebar's projects and workspaces.
struct Saved {
    projects: Vec<PathBuf>,
    workspaces: Vec<dragent_persistence::SerializedWorkspace>,
}

fn saved() -> Result<Saved, String> {
    let path = dragent_persistence::database_path().map_err(|error| format!("{error:#}"))?;
    if !path.exists() {
        return Ok(Saved {
            projects: Vec::new(),
            workspaces: Vec::new(),
        });
    }
    let unreadable = |error: anyhow::Error| format!("couldn't read Dragent's data: {error:#}");
    let mut conn = dragent_persistence::establish_readonly(&path).map_err(unreadable)?;
    let session = dragent_persistence::load_document_session(&mut conn).map_err(unreadable)?;
    let catalogue = dragent_persistence::workspace_catalogue::load_local_workspaces(&mut conn)
        .map_err(unreadable)?;
    let sessions = crate::workspace::project_naming::automation_sessions_root();
    let mut projects: Vec<PathBuf> = Vec::new();
    for root in session.open_project_roots {
        if root.is_dir() && !root.starts_with(&sessions) && !projects.contains(&root) {
            projects.push(root);
        }
    }
    // The sidebar's workspaces, then checkouts only the catalogue knows.
    let mut workspaces: Vec<dragent_persistence::SerializedWorkspace> = Vec::new();
    for workspace in session.workspaces.into_iter().chain(catalogue) {
        if workspace.root.is_dir() && !workspaces.iter().any(|known| known.root == workspace.root) {
            workspaces.push(workspace);
        }
    }
    Ok(Saved {
        projects,
        workspaces,
    })
}

impl Saved {
    /// The sidebar workspace `path` is in (the deepest one).
    fn workspace_containing(
        &self,
        path: &Path,
    ) -> Option<&dragent_persistence::SerializedWorkspace> {
        self.workspaces
            .iter()
            .filter(|workspace| path.starts_with(&workspace.root))
            .max_by_key(|workspace| workspace.root.components().count())
    }

    /// The project a workspace belongs to; `None` for session folders.
    fn project_of(&self, root: &Path) -> Option<PathBuf> {
        let sessions = crate::workspace::project_naming::automation_sessions_root();
        (!root.starts_with(&sessions)).then(|| {
            crate::workspace::project_naming::sidebar_project_root(
                &root.to_path_buf(),
                &self.projects,
            )
        })
    }
}

/// Projects open in Dragent's sidebar (not its own session folders).
fn open_projects() -> Result<Vec<PathBuf>, String> {
    saved().map(|saved| saved.projects)
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

// ── Projects ─────────────────────────────────────────────────────────────────

fn projects(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    match flags.subcommand() {
        "list" => {
            let rows = open_projects()?
                .iter()
                .map(|root| {
                    serde_json::json!({
                        "name": project_name(root),
                        "path": root,
                        "machine": scope::machine_of(root),
                    })
                })
                .collect();
            Ok(listing(
                &flags,
                rows,
                |row| text(row, "path"),
                |row| format!("{}\t{}", text(row, "name"), text(row, "path")),
                "No projects are open in Dragent.",
            ))
        }
        "add" => add_project(&flags),
        other => Err(format!(
            "unknown projects command “{other}” (use list or add)"
        )),
    }
}

/// A folder, or a repository URL to clone first (into `--into`, else the
/// current folder, like `git clone`).
fn add_project(flags: &Flags) -> Result<String, String> {
    let source = flags
        .argument(0)
        .ok_or("usage: dragent projects add PATH | URL [--into DIR]")?;
    let path = if looks_like_url(source) {
        let name = crate::workspace::project_naming::clone_target_name(source)
            .ok_or_else(|| format!("“{source}” doesn't look like a repository URL"))?;
        let parent = absolute(flags.value("into").unwrap_or("."));
        let target = parent.join(&name);
        if target.exists() {
            return Err(format!("{} already exists", target.display()));
        }
        // Git's own progress goes to this terminal.
        let status = std::process::Command::new("git")
            .args(["clone", "--", source])
            .arg(&target)
            .status()
            .map_err(|error| format!("couldn't run git: {error}"))?;
        if !status.success() {
            return Err("git clone failed".into());
        }
        target
    } else {
        let path = absolute(source);
        if !path.is_dir() {
            return Err(format!("{} isn't a folder", path.display()));
        }
        path
    };
    let response = ask_app(requests::Request::Open { path: path.clone() }, true)?;
    if flags.json {
        return Ok(pretty(serde_json::to_value(&response).unwrap_or_default()));
    }
    if flags.quiet {
        return Ok(path.display().to_string());
    }
    Ok(format!("Added {} to Dragent", path.display()))
}

fn looks_like_url(source: &str) -> bool {
    source.contains("://")
        || (source.contains('@') && source.contains(':') && !Path::new(source).exists())
}

// ── Workspaces ───────────────────────────────────────────────────────────────

fn workspaces(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    match flags.subcommand() {
        "list" => list_workspaces(&flags),
        "create" | "new" => create_workspace(&flags),
        "delete" | "remove" => delete_workspace(&flags),
        "help" => Ok(HELP.trim_end().to_string()),
        other => Err(format!(
            "unknown workspaces command “{other}” (use list, create or delete)"
        )),
    }
}

fn list_workspaces(flags: &Flags) -> Result<String, String> {
    let saved = saved()?;
    let filter = flags.value("project").map(absolute);
    let running = agents::running_now();
    let rows: Vec<serde_json::Value> = saved
        .workspaces
        .iter()
        .filter_map(|workspace| {
            let project = saved.project_of(&workspace.root);
            if filter.is_some() && project != filter {
                return None;
            }
            // A remote workspace's local folder is only a stand-in: its
            // branch is on the machine, not readable here.
            let branch = project
                .as_ref()
                .filter(|_| !scope::is_remote(&workspace.root))
                .and_then(|_| crate::workspace::project_naming::local_branch_name(&workspace.root));
            let agents: Vec<serde_json::Value> = running
                .iter()
                .filter(|agent| saved.workspace_containing(&agent.session.cwd).map(|w| &w.root) == Some(&workspace.root))
                .map(|agent| serde_json::json!({ "id": agent.id(), "agent": agent.session.provider, "state": agent.state.key() }))
                .collect();
            Some(serde_json::json!({
                "name": workspace.title,
                "path": workspace.root,
                "machine": scope::machine_of(&workspace.root),
                "branch": branch,
                "project": project,
                "agents": agents,
            }))
        })
        .collect();
    Ok(listing(
        flags,
        rows,
        |row| text(row, "path"),
        |row| {
            let project = row["project"]
                .as_str()
                .map(|project| project_name(Path::new(project)))
                .unwrap_or_else(|| "Sessions".into());
            let agents = row["agents"]
                .as_array()
                .map(|agents| {
                    agents
                        .iter()
                        .map(|agent| {
                            format!(
                                "{}: {}",
                                text(agent, "agent"),
                                agents::State::label_for(&text(agent, "state"))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|agents| !agents.is_empty())
                .unwrap_or_else(|| "-".into());
            format!(
                "{}\t{}\t{project}\t{agents}\t{}",
                text(row, "name"),
                text(row, "branch"),
                text(row, "path")
            )
        },
        "No workspaces.",
    ))
}

/// The project a workspace is for: `--project`, else the open project the
/// current folder is in, else the only open project.
fn chosen_project(flags: &Flags) -> Result<PathBuf, String> {
    chosen_project_at(flags.value("project"))
}

/// [`chosen_project`] from a `--project` value.
fn chosen_project_at(project: Option<&str>) -> Result<PathBuf, String> {
    if let Some(project) = project {
        let project = absolute(project);
        if !project.is_dir() {
            return Err(format!("{} isn't a folder", project.display()));
        }
        return Ok(project);
    }
    let projects = open_projects()?;
    let here = std::env::current_dir()
        .ok()
        .map(|dir| dir.canonicalize().unwrap_or(dir));
    if let Some(project) = here.and_then(|here| {
        projects
            .iter()
            .filter(|root| here.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned()
    }) {
        return Ok(project);
    }
    match projects.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err("pass --project PATH (see `dragent projects list`)".into()),
    }
}

fn create_workspace(flags: &Flags) -> Result<String, String> {
    let project = chosen_project(flags)?;
    let local = flags.on("local");
    if !local && crate::workspace::project_sidebar::detect::find_git_root(&project).is_none() {
        return Err(format!(
            "{} isn't a git repository; use --local to work in its folder directly",
            project.display()
        ));
    }
    let pull_request = match flags.value("pr") {
        Some(number) => Some(
            number
                .trim_start_matches('#')
                .parse::<u32>()
                .map_err(|_| format!("--pr takes a pull request number, not “{number}”"))?,
        ),
        None => None,
    };
    if local && (pull_request.is_some() || flags.value("branch").is_some()) {
        return Err("--local uses the project's checkout as it is: drop --pr and --branch".into());
    }
    let agent = flags.value("agent").map(agent_key).transpose()?;
    let model = flags.value("model").map(str::to_string);
    if let (Some(model), Some(key)) = (&model, &agent) {
        model::check(key, model)?;
    }
    let response = ask_app(
        requests::Request::CreateWorkspace {
            project,
            agent,
            prompt: flags.prompt()?,
            branch: flags.value("branch").map(str::to_string),
            base_branch: flags.value("base-branch").map(str::to_string),
            name: flags.value("name").map(str::to_string),
            pull_request,
            local,
            command: flags.value("command").map(str::to_string),
            tags: flags.all("tag"),
            model,
            background: false,
        },
        false,
    )?;
    if flags.json {
        return Ok(pretty(serde_json::to_value(&response).unwrap_or_default()));
    }
    let path = response
        .workspace
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    if flags.quiet {
        return Ok(path);
    }
    Ok(format!(
        "Created workspace “{}”{} at {path}{}",
        response.title.as_deref().unwrap_or("-"),
        response
            .branch
            .as_deref()
            .map(|branch| format!(" on branch {branch}"))
            .unwrap_or_default(),
        match (&response.agent, &response.agent_id) {
            (Some(agent), Some(id)) => format!(
                ", with {agent} working in it (agent {})",
                &id[..8.min(id.len())]
            ),
            (Some(agent), None) => format!(", with {agent} working in it"),
            _ => String::new(),
        },
    ))
}

/// Close a workspace and remove its worktree. Refuses when that would lose
/// work, unless `--force`.
fn delete_workspace(flags: &Flags) -> Result<String, String> {
    let path = absolute(
        flags
            .argument(0)
            .ok_or("usage: dragent workspaces delete PATH [--force]")?,
    );
    let saved = saved()?;
    let workspace = saved
        .workspaces
        .iter()
        .find(|workspace| workspace.root == path)
        .ok_or_else(|| {
            format!(
                "{} isn't a workspace in Dragent (see `dragent workspaces list`)",
                path.display()
            )
        })?;
    let main_checkout = saved.projects.contains(&workspace.root);
    if !flags.on("force") {
        let blockers = delete_blockers(&workspace.root, main_checkout);
        if !blockers.agents.is_empty() {
            return Err(format!(
                "agents are still working in it: {}. Stop them, or add --force",
                blockers.agents.join(", ")
            ));
        }
        let changes = blockers.changes;
        if changes > 0 {
            return Err(format!(
                "it has {changes} uncommitted change{} that removing the worktree would lose. Commit them, or add --force",
                if changes == 1 { "" } else { "s" }
            ));
        }
    }
    let response = ask_app(
        requests::Request::DeleteWorkspace { path: path.clone() },
        false,
    )?;
    if flags.json {
        return Ok(pretty(serde_json::to_value(&response).unwrap_or_default()));
    }
    if flags.quiet {
        return Ok(path.display().to_string());
    }
    Ok(if main_checkout {
        format!(
            "Closed {} in Dragent (it's the project's own checkout, so its files stay)",
            workspace.title
        )
    } else {
        format!("Deleted workspace {} and its worktree", workspace.title)
    })
}

/// What deleting a workspace would cut short or lose: agents still working
/// in it and, for a worktree (not the project's own checkout, whose files
/// stay), its uncommitted changes. The command line and a paired phone
/// both ask this before deleting.
pub(crate) struct DeleteBlockers {
    /// `provider (id)` of each agent still working there.
    pub agents: Vec<String>,
    /// Changed or untracked files the removal would lose.
    pub changes: usize,
}

pub(crate) fn delete_blockers(root: &Path, main_checkout: bool) -> DeleteBlockers {
    let agents = agents::running_now()
        .iter()
        .filter(|agent| agent.session.cwd.starts_with(root))
        .map(|agent| format!("{} ({})", agent.session.provider, agent.id()))
        .collect();
    let changes = if main_checkout { 0 } else { uncommitted_changes(root) };
    DeleteBlockers { agents, changes }
}

/// Changed or untracked files in a checkout (0 if git can't tell).
fn uncommitted_changes(root: &Path) -> usize {
    let mut command = std::process::Command::new("git");
    command.args(["status", "--porcelain"]).current_dir(root);
    if let Some(ceilings) = dragent_host::git_ceiling_env() {
        command.env("GIT_CEILING_DIRECTORIES", ceilings);
    }
    command
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().count())
        .unwrap_or(0)
}

// ── Open and status ──────────────────────────────────────────────────────────

fn open(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let path = absolute(flags.positional.first().map(String::as_str).unwrap_or("."));
    if !path.is_dir() {
        return Err(format!("{} isn't a folder", path.display()));
    }
    let response = ask_app(requests::Request::Open { path: path.clone() }, true)?;
    if flags.json {
        return Ok(pretty(serde_json::to_value(&response).unwrap_or_default()));
    }
    if flags.quiet {
        return Ok(path.display().to_string());
    }
    Ok(format!("Opened {} in Dragent", path.display()))
}

/// `dragent open-url URL`, the `BROWSER` of Dragent's terminals: the open
/// app opens the link as a click in it would (a web page in Dragent's
/// browser); without the app, the system opens it, so no link is lost.
fn open_url(args: &[String]) -> Result<String, String> {
    let url = args
        .iter()
        .map(|arg| arg.trim())
        .find(|arg| !arg.is_empty())
        .ok_or("usage: dragent open-url URL")?;
    let opened = instance::app_running()
        && ask_app(
            requests::Request::OpenLink {
                url: url.to_string(),
            },
            false,
        )
        .is_ok();
    if !opened {
        system_open(url)?;
    }
    Ok(String::new())
}

/// The system's handler for `url`: its browser for a web page.
fn system_open(url: &str) -> Result<(), String> {
    use crate::process_ext::NoWindow as _;
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // Not `cmd /c start`: cmd would split the address at `&`.
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(url)
        .no_window()
        .status()
        .map_err(|error| format!("Couldn't open {url}: {error}"))?;
    Ok(())
}

/// `requests show ID`: where a request to the app stands, for a command that
/// stopped waiting (a timeout names the id).
fn request_status(args: &[String]) -> Result<String, String> {
    let usage = || {
        Failure::new(
            Status::InvalidInput,
            "usage",
            "usage: dragent requests show REQUEST_ID",
        )
        .next("A timed-out command's failure has the id in request_id.")
        .encode()
    };
    let words: Vec<&String> = args.iter().filter(|arg| !arg.starts_with("--")).collect();
    let (Some("show"), Some(id)) = (words.first().map(|w| w.as_str()), words.get(1)) else {
        return Err(usage());
    };
    let report = match requests::inspect(&requests::folder(), id) {
        requests::State::Queued => serde_json::json!({
            "request_id": id, "state": "queued",
            "message": "Written, but the app hasn't taken it yet."
        }),
        requests::State::InProgress => serde_json::json!({
            "request_id": id, "state": "in_progress",
            "message": "The app took it and hasn't answered yet."
        }),
        requests::State::Done(response) => {
            let mut done =
                serde_json::json!({ "request_id": id, "state": "done", "ok": response.ok });
            if response.ok {
                done["response"] = serde_json::to_value(&response).unwrap_or_default();
            } else {
                done["failure"] = Failure::from_error(&app_failure(response, id)).to_json();
            }
            done
        }
        requests::State::Unknown => serde_json::json!({
            "request_id": id, "state": "unknown",
            "message": "No such request here: it was never made, its answer was already collected, or it expired (answers are kept a day)."
        }),
    };
    Ok(pretty(report))
}

fn status(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let running = instance::app_running();
    let live = live_identity();
    let organisation = live
        .as_ref()
        .and_then(|data| data["organisation"].as_str().map(str::to_string))
        .or_else(|| {
            crate::organisation_context::cached_active()
                .map(|active| active.name)
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(dragent_paths::organisation_id);
    let machine = dragent_host::scope::this_machine();
    let remote_machines = dragent_host::machine_registry::MachineRegistry::load()
        .map(|registry| registry.machines.len())
        .unwrap_or(0);
    if flags.json {
        return Ok(pretty(serde_json::json!({
            "running": running,
            "organisation": organisation,
            "organisation_id": dragent_paths::organisation_id(),
            "version": env!("CARGO_PKG_VERSION"),
            "machine": machine,
            "remote_machines": remote_machines,
        })));
    }
    Ok(format!(
        "App: {}\nVersion: {}\nOrganisation: {organisation}\nMachine: {} (this computer){}",
        if running { "running" } else { "not running" },
        env!("CARGO_PKG_VERSION"),
        machine["name"].as_str().unwrap_or_default(),
        if remote_machines > 0 {
            format!("\nRemote machines: {remote_machines} (`dragent machines list`)")
        } else {
            String::new()
        },
    ))
}

/// `agents list`: the agents this computer can start.
fn installed_agents(flags: &Flags) -> String {
    let installed = crate::workspace::detect_prompt_agents();
    let rows = PromptAgent::ALL
        .into_iter()
        .filter(|agent| *agent == PromptAgent::Dragent || installed.contains(agent))
        .map(|agent| {
            serde_json::json!({
                "key": agent.command_key(),
                "name": agent.label(),
                "model_option": model::flag(agent).is_some(),
            })
        })
        .collect();
    listing(
        flags,
        rows,
        |row| text(row, "key"),
        |row| format!("{}\t{}", text(row, "key"), text(row, "name")),
        "No agents found.",
    )
}

// ── Launchers ────────────────────────────────────────────────────────────────

/// Write both launchers for this executable; returns the organisation's.
pub fn install_launchers() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("current_exe: {error}"))?;
    let organisation = dragent_paths::organisation_id();
    let launcher = write_launchers(
        &dragent_paths::organisation_dir().join("bin"),
        &exe,
        Some(&organisation),
    )?;
    if let Err(error) = write_launchers(&user_bin_dir(), &exe, None) {
        log::warn!("[cli] {error}");
    }
    Ok(launcher)
}

/// Where the launcher for any terminal lives.
pub fn user_bin_dir() -> PathBuf {
    dragent_paths::home().join("bin")
}

/// Git Bash on Windows runs the POSIX launcher, kept in its own folder so
/// PowerShell and cmd never see a file named plain `dragent`.
pub const POSIX_SUBDIR: &str = "posix";

/// This platform's launchers in `dir`; returns the one its shells run.
fn write_launchers(dir: &Path, exe: &Path, organisation: Option<&str>) -> Result<PathBuf, String> {
    let organisation = organisation.map(|id| format!("{}={id}", dragent_paths::ORGANISATION_ARG));
    #[cfg(windows)]
    {
        let posix_exe = dragent_shell_hooks::posix_path(exe);
        write_script(
            &dir.join(POSIX_SUBDIR).join("dragent"),
            &posix_script(&posix_exe, organisation.as_deref()),
        )?;
        write_script(
            &dir.join(POSIX_SUBDIR).join(OPEN_LAUNCHER),
            &posix_launcher(&posix_exe, organisation.as_deref(), OPEN_COMMAND),
        )?;
        let launcher = dir.join("dragent.cmd");
        write_script(
            &launcher,
            &cmd_script(&exe.display().to_string(), organisation.as_deref()),
        )?;
        write_script(
            &dir.join(format!("{OPEN_LAUNCHER}.cmd")),
            &cmd_launcher(
                &exe.display().to_string(),
                organisation.as_deref(),
                OPEN_COMMAND,
            ),
        )?;
        Ok(launcher)
    }
    #[cfg(not(windows))]
    {
        let launcher = dir.join("dragent");
        write_script(
            &launcher,
            &posix_script(&exe.display().to_string(), organisation.as_deref()),
        )?;
        write_script(
            &dir.join(OPEN_LAUNCHER),
            &posix_launcher(
                &exe.display().to_string(),
                organisation.as_deref(),
                OPEN_COMMAND,
            ),
        )?;
        Ok(launcher)
    }
}

/// The launcher Dragent's terminals use as `BROWSER`: `dragent open-url`.
pub const OPEN_LAUNCHER: &str = "dragent-open";
const OPEN_COMMAND: &str = "open-url";

/// `sh` launcher: the app binary in command mode, with the arguments.
fn posix_script(exe: &str, organisation: Option<&str>) -> String {
    posix_launcher(exe, organisation, "")
}

/// A `sh` launcher running `command` (empty: whatever the arguments say).
fn posix_launcher(exe: &str, organisation: Option<&str>, command: &str) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let organisation = organisation
        .map(|organisation| format!(" {}", quote(organisation)))
        .unwrap_or_default();
    let command = if command.is_empty() {
        String::new()
    } else {
        format!(" {command}")
    };
    format!(
        "#!/bin/sh\nexec {}{organisation} {CLI_ARG}{command} \"$@\"\n",
        quote(exe)
    )
}

/// `cmd` launcher; its exit code is the command's, so scripts can check it.
fn cmd_script(exe: &str, organisation: Option<&str>) -> String {
    cmd_launcher(exe, organisation, "")
}

/// A `cmd` launcher running `command` (empty: whatever the arguments say).
fn cmd_launcher(exe: &str, organisation: Option<&str>, command: &str) -> String {
    let organisation = organisation
        .map(|organisation| format!(" \"{organisation}\""))
        .unwrap_or_default();
    let command = if command.is_empty() {
        String::new()
    } else {
        format!(" {command}")
    };
    format!(
        "@echo off\r\n\"{exe}\"{organisation} {CLI_ARG}{command} %*\r\nexit /b %ERRORLEVEL%\r\n"
    )
}

fn write_script(path: &Path, script: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    if std::fs::read_to_string(path).ok().as_deref() != Some(script) {
        std::fs::write(path, script)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn only_help_and_identity_work_signed_out() {
        for open in ["help", "--help", "version", "-V", "status", "whoami", "mcp"] {
            assert!(!needs_sign_in(open, &[]), "{open}");
        }
        assert!(!needs_sign_in("automations", &args(&["help"])));
        assert!(!needs_sign_in("integrations", &args(&["--help"])));
        for (command, rest) in [
            ("integrations", vec!["search", "linear"]),
            (
                "integrations",
                vec!["comment", "linear", "ENG-1", "--body", "hi"],
            ),
            ("workspaces", vec!["create", "--prompt", "x"]),
            ("agents", vec!["send", "abc", "rm -rf"]),
            ("agents", vec!["read", "abc"]),
            ("team", vec!["create", "Ship invites"]),
            ("tasks", vec!["done", "--summary", "x"]),
            ("msg", vec!["send", "lead", "hi"]),
            ("automations", vec!["list"]),
            ("projects", vec!["list"]),
            ("settings", vec!["set", "theme", "dark"]),
            ("machines", vec!["list"]),
            ("open", vec![]),
            ("feedback", vec!["hi"]),
        ] {
            assert!(needs_sign_in(command, &args(&rest)), "{command} {rest:?}");
        }
    }

    #[test]
    fn options_parse_with_values_switches_and_equals() {
        let flags = Flags::parse(&args(&[
            "create",
            "--agent",
            "codex",
            "--prompt=fix the tests",
            "--json",
        ]))
        .unwrap();
        assert_eq!(flags.subcommand(), "create");
        assert_eq!(flags.value("agent"), Some("codex"));
        assert_eq!(flags.value("prompt"), Some("fix the tests"));
        assert!(flags.json);
        assert!(Flags::parse(&args(&["create", "--agent"])).is_err());
    }

    #[test]
    fn help_and_unknown_commands() {
        assert!(dispatch(&[]).unwrap().contains("Usage: dragent"));
        assert!(dispatch(&args(&["version"]))
            .unwrap()
            .starts_with("dragent "));
        assert!(dispatch(&args(&["frobnicate"]))
            .unwrap_err()
            .contains("dragent help"));
    }

    #[test]
    fn launchers_run_commands_never_the_app() {
        assert_eq!(
            posix_script("/Applications/Dragent's App/dragent", Some("--dragent-organisation=org-1")),
            "#!/bin/sh\nexec '/Applications/Dragent'\\''s App/dragent' '--dragent-organisation=org-1' --dragent-cli \"$@\"\n"
        );
        assert_eq!(
            posix_script("/c/Program Files/Dragent/dragent.exe", None),
            "#!/bin/sh\nexec '/c/Program Files/Dragent/dragent.exe' --dragent-cli \"$@\"\n"
        );
        assert_eq!(
            cmd_script(r"C:\Program Files\Dragent\dragent.exe", Some("--dragent-organisation=org-1")),
            "@echo off\r\n\"C:\\Program Files\\Dragent\\dragent.exe\" \"--dragent-organisation=org-1\" --dragent-cli %*\r\nexit /b %ERRORLEVEL%\r\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_posix_launcher_passes_arguments_through() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = write_launchers(dir.path(), Path::new("/bin/echo"), Some("o")).unwrap();
        let output = std::process::Command::new(&launcher)
            .args(["workspaces", "list", "two words"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "--dragent-organisation=o --dragent-cli workspaces list two words\n"
        );
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn every_command_is_known_and_a_typo_is_invalid_input() {
        for command in COMMANDS {
            assert!(
                KNOWN_COMMANDS.contains(&command),
                "{command} missing from KNOWN_COMMANDS"
            );
        }
        let failure = Failure::from_error(&dispatch(&["nosuchthing".to_string()]).unwrap_err());
        assert_eq!(
            (failure.status, failure.code.as_str()),
            (Status::InvalidInput, "unknown_command")
        );
        let usage =
            Failure::from_error(&request_status(&["show".into(), "--json".into()]).unwrap_err());
        assert_eq!(usage.status, Status::InvalidInput);
        let unknown = request_status(&[
            "show".into(),
            uuid::Uuid::new_v4().to_string(),
            "--json".into(),
        ])
        .unwrap();
        assert!(unknown.contains("\"unknown\""));
    }
}
