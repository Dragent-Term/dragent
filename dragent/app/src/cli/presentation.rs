//! Terminal-only presentation. Dispatch still produces plain text/JSON; this
//! layer never decorates machine output, transcripts, or redirected streams.
use serde_json::Value;
use std::io::IsTerminal;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Internal dispatch payload, consumed before printing. Carry full records to
// presentation so layout never needs to reconstruct or discard data from TSV.
const RECORDS: &str = "\u{1e}dragent-records:";

pub(crate) fn records(rows: &[Value], empty: &str) -> Option<String> {
    std::io::stdout().is_terminal().then(|| {
        format!(
            "{RECORDS}{}",
            serde_json::json!({ "rows": rows, "empty": empty })
        )
    })
}

struct Terminal {
    color: bool,
    width: usize,
}

impl Terminal {
    fn new(stderr: bool) -> Self {
        let tty = if stderr {
            std::io::stderr().is_terminal()
        } else {
            std::io::stdout().is_terminal()
        };
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|width: &usize| *width > 0)
            .map(|requested| {
                terminal_width(stderr).map_or(requested, |actual| requested.min(actual))
            })
            .or_else(|| terminal_width(stderr))
            .unwrap_or(88);
        Self {
            color: tty
                && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").as_deref() != Ok("dumb")
                && ansi_supported(stderr),
            width: width.clamp(16, 100),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn muted(&self, text: &str) -> String {
        self.paint("90", text)
    }
    fn accent(&self, text: &str) -> String {
        self.paint("36", text)
    }
    fn strong(&self, text: &str) -> String {
        self.paint("1", text)
    }
    fn banner(&self, title: &str) -> String {
        let brand = format!("{} {}", self.accent("◆"), self.strong("dragent"));
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        let line = if self.width >= 40 {
            format!("{brand}  {}", self.muted(&version))
        } else {
            brand
        };
        let mut out = format!("\n  {line}\n");
        if !title.is_empty() {
            self.paragraph(&mut out, &self.muted(title), 2);
        }
        out.push_str(&format!(
            "  {}\n",
            self.muted(&"─".repeat(self.width.saturating_sub(4).min(48)))
        ));
        out
    }
    fn section(&self, out: &mut String, label: &str) {
        out.push('\n');
        self.paragraph(out, &self.strong(label), 2);
    }
    fn paragraph(&self, out: &mut String, text: &str, indent: usize) {
        let prefix = " ".repeat(indent);
        for paragraph in text.split('\n') {
            for line in wrap(paragraph, self.width.saturating_sub(indent).max(1)) {
                out.push_str(&format!("{prefix}{line}\n"));
            }
        }
    }
    fn card_paragraph(&self, out: &mut String, text: &str, indent: usize) {
        let indent = indent.min(self.width.saturating_sub(8)).max(4);
        let prefix = format!("  {}{}", self.accent("│"), " ".repeat(indent - 3));
        for paragraph in text.split('\n') {
            let plain = clean(paragraph);
            let path = plain.starts_with("\"/")
                || plain.starts_with("\"~")
                || plain.starts_with("\".")
                || plain.starts_with("'/")
                || plain.starts_with("'~")
                || plain.starts_with("'.")
                || plain.starts_with("\"http")
                || plain.starts_with("'http")
                || plain
                    .as_bytes()
                    .get(2..4)
                    .is_some_and(|part| part == b":/" || part == b":\\");
            let lines = if path {
                split_link(&plain, self.width.saturating_sub(indent).max(1))
            } else {
                wrap(paragraph, self.width.saturating_sub(indent).max(1))
            };
            let prefix = if path {
                prefix.clone()
            } else {
                " ".repeat(indent)
            };
            for line in lines {
                out.push_str(&format!("{prefix}{line}\n"));
            }
        }
    }
    fn command(&self, out: &mut String, command: &str, description: &str, column: usize) {
        if self.width >= 78 && command.width() <= column {
            let padding = " ".repeat(column - command.width() + 3);
            let lines = wrap(description, self.width.saturating_sub(column + 7).max(1));
            out.push_str(&format!(
                "    {}{padding}{}\n",
                self.accent(command),
                lines.first().map(String::as_str).unwrap_or("")
            ));
            for line in lines.iter().skip(1) {
                out.push_str(&format!("{}{line}\n", " ".repeat(column + 7)));
            }
        } else {
            for line in wrap(command, self.width.saturating_sub(4).max(1)) {
                out.push_str(&format!("    {}\n", self.accent(&line)));
            }
            self.paragraph(out, description, 6);
        }
    }
}

pub(super) fn output(args: &[String], raw: &str) -> String {
    let tty = std::io::stdout().is_terminal();
    let explicit_text = args.iter().any(|arg| arg == "--text");
    let machine = args.iter().any(|arg| arg == "--json" || arg == "--quiet")
        || (!explicit_text && super::agent_environment());
    let action = subcommand(args);
    let raw_content = matches!(args.first().map(String::as_str), Some("mcp"))
        || matches!(args.first().map(String::as_str), Some("agent" | "agents")) && action == "read"
        || matches!(
            args.first().map(String::as_str),
            Some("automation" | "automations")
        ) && action == "prompt";
    if !tty || machine || raw_content || raw.is_empty() {
        return raw.to_string();
    }
    let terminal = Terminal::new(false);
    if let Some(encoded) = raw.strip_prefix(RECORDS) {
        let mut values = serde_json::Deserializer::from_str(encoded).into_iter::<Value>();
        if let Some(Ok(payload)) = values.next() {
            let rows = payload["rows"].as_array().cloned().unwrap_or_default();
            let mut out = record_list(
                &terminal,
                args,
                &rows,
                payload["empty"].as_str().unwrap_or("Nothing here yet."),
            );
            let note = encoded[values.byte_offset()..].trim();
            if !note.is_empty() {
                terminal.paragraph(&mut out, &clean(note), 2);
            }
            return out;
        }
    }
    // The help payload is an explicit dispatch result, not guessed from user
    // arguments (a prompt or search query can itself contain "--help").
    if raw == super::HELP.trim_end() {
        let root = args.is_empty()
            || matches!(
                args.first().map(String::as_str),
                Some("help" | "--help" | "-h")
            );
        return if root && !args.iter().any(|arg| arg == "--all") {
            root_help(&terminal)
        } else {
            help(&terminal, raw)
        };
    }
    if (action == "help" || args.len() == 1) && raw.contains("\nUsage: dragent ") {
        return help(&terminal, raw);
    }
    // Detail commands return structured data too. Only the human terminal
    // view turns it into labelled sections; --json still returns it verbatim.
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        let rows = match value {
            Value::Array(rows) => rows,
            other => vec![other],
        };
        return record_list(&terminal, args, &rows, "Nothing here yet.");
    }
    let clean = clean(raw);
    let title = title(args);
    let mut out = terminal.banner(title);
    if clean.contains('\t') {
        table(&terminal, &mut out, args, &clean);
    } else {
        out.push('\n');
        let success = matches!(args.first().map(String::as_str), Some("open" | "feedback"))
            || matches!(
                action.as_str(),
                "add"
                    | "create"
                    | "delete"
                    | "start"
                    | "stop"
                    | "send"
                    | "switch"
                    | "use"
                    | "set"
                    | "reset"
                    | "run"
                    | "pause"
                    | "resume"
                    | "update"
                    | "comment"
                    | "move"
            );
        let key_width = clean
            .lines()
            .filter_map(|line| line.split_once(": "))
            .filter(|(key, _)| key.len() < 22)
            .map(|(key, _)| key.width())
            .max()
            .unwrap_or(0);
        for (index, line) in clean.lines().enumerate() {
            if let Some((key, value)) = line.split_once(": ").filter(|(key, _)| key.len() < 22) {
                terminal.paragraph(
                    &mut out,
                    &format!(
                        "{}{}{}",
                        terminal.muted(&format!("{key}:")),
                        " ".repeat(key_width - key.width() + 2),
                        state(&terminal, value)
                    ),
                    2,
                );
            } else {
                let line = if success && index == 0 {
                    format!("{} {line}", terminal.paint("32", "✓"))
                } else {
                    state(&terminal, line)
                };
                terminal.paragraph(&mut out, &line, 2);
            }
        }
    }
    next_steps(&terminal, &mut out, args);
    out
}

/// Whether a failure is also printed as JSON on stdout: `--json`, or an agent
/// or CI running the command without `--text` (`--quiet` prints nothing).
pub(super) fn wants_json(args: &[String]) -> bool {
    !args.iter().any(|arg| arg == "--quiet" || arg == "--text")
        && (args.iter().any(|arg| arg == "--json") || super::agent_environment())
}

pub(super) fn error(args: &[String], message: &str) -> String {
    let failure = dragent_host::outcome::Failure::from_error(message);
    let message = match &failure.next {
        Some(next) => format!("{} {next}", failure.message),
        None => failure.message.clone(),
    };
    let message = message.as_str();
    if !std::io::stderr().is_terminal()
        || args.iter().any(|arg| arg == "--json" || arg == "--quiet")
        || (super::agent_environment() && !args.iter().any(|arg| arg == "--text"))
    {
        return format!("dragent: {message}");
    }
    let terminal = Terminal::new(true);
    let mut out = String::from("\n");
    terminal.paragraph(
        &mut out,
        &format!(
            "{} {}",
            terminal.paint("31;1", "×"),
            terminal.strong("Couldn't complete the command")
        ),
        2,
    );
    out.push('\n');
    terminal.paragraph(&mut out, &clean(message), 2);
    out.push('\n');
    terminal.paragraph(
        &mut out,
        &terminal.muted("Run dragent help to see available commands."),
        2,
    );
    out
}

fn root_help(t: &Terminal) -> String {
    let mut out = t.banner("Your workspace, from the command line.");
    out.push('\n');
    t.paragraph(
        &mut out,
        &format!(
            "{} {}",
            t.muted("Usage:"),
            t.accent("dragent <command> [options]")
        ),
        2,
    );
    let groups: &[(&str, &[(&str, &str)])] = &[
        (
            "Workspaces & projects",
            &[
                (
                    "workspaces list",
                    "Your workspaces, branches and active agents",
                ),
                (
                    "workspaces create [options]",
                    "Start a workspace, with an agent or a terminal",
                ),
                (
                    "workspaces delete PATH",
                    "Close a workspace and remove its worktree",
                ),
                ("projects list", "Browse your projects"),
                (
                    "projects add PATH | URL",
                    "Open a folder or clone a repository",
                ),
                ("open [PATH]", "Open a project or workspace in Dragent"),
            ],
        ),
        (
            "Agents",
            &[
                ("agents list", "See which agents are installed"),
                ("agents running", "See who is working and who needs you"),
                ("agents read AGENT", "Read an agent's terminal"),
                ("agents send AGENT TEXT", "Send a follow-up message"),
                (
                    "agents start [options]",
                    "Start another agent in a workspace",
                ),
                ("agents stop AGENT", "End an agent's session"),
            ],
        ),
        (
            "Desktop apps",
            &[(
                "computer help",
                "Read windows, take screenshots and use desktop apps",
            )],
        ),
        (
            "Automations & integrations",
            &[
                ("automations help", "Schedule agents and manage their runs"),
                ("integrations list", "See your connected services"),
                (
                    "integrations help",
                    "Find, read and update issues and threads",
                ),
                ("mcp", "Expose connected services as an MCP server"),
            ],
        ),
        (
            "Your app",
            &[
                ("status · whoami", "App status and your signed-in account"),
                (
                    "org list · org switch NAME",
                    "Browse or switch organisations",
                ),
                ("machines list", "Browse your SSH machines"),
                ("settings list · theme list", "Personalise Dragent"),
                ("feedback TEXT", "Send feedback to the Dragent team"),
                ("help · version", "Command reference and app version"),
            ],
        ),
    ];
    for (name, commands) in groups {
        t.section(&mut out, name);
        for (command, description) in *commands {
            t.command(&mut out, command, description, 29);
        }
    }
    t.section(&mut out, "Start here");
    t.command(&mut out, "dragent status", "Check your connection", 29);
    t.command(
        &mut out,
        "dragent workspaces help",
        "Workspace and agent launch options",
        29,
    );
    t.command(
        &mut out,
        "dragent help --all",
        "Full command reference and options",
        29,
    );
    t.section(&mut out, "Output");
    for (flag, description) in [
        ("--json", "Machine-readable output"),
        ("--quiet", "IDs and paths only"),
        ("--text", "Human-readable output"),
        ("NO_COLOR=1", "Disable colour"),
    ] {
        t.command(&mut out, flag, description, 16);
    }
    out
}

fn help(t: &Terminal, raw: &str) -> String {
    let mut out = t.banner("");
    for line in clean(raw).lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(usage) = line.strip_prefix("Usage: ") {
            t.section(&mut out, "Usage");
            t.paragraph(&mut out, &t.accent(usage), 4);
        } else if line.starts_with("  ") {
            let trimmed = line.trim();
            if let Some(index) = trimmed.find("  ") {
                t.command(&mut out, &trimmed[..index], trimmed[index..].trim(), 31);
            } else {
                t.paragraph(&mut out, trimmed, 4);
            }
        } else {
            t.section(&mut out, line.trim_end_matches(':'));
        }
    }
    out
}

fn title(args: &[String]) -> &'static str {
    match args.first().map(String::as_str).unwrap_or("") {
        "status" => "App status",
        "whoami" | "org" => "Your account",
        "agents" | "agent" => "Agents",
        "workspaces" | "workspace" => "Workspaces",
        "projects" | "project" | "open" => "Projects",
        "automations" | "automation" => "Automations",
        "integrations" | "integration" => "Connected services",
        "machines" => "Machines",
        "settings" | "theme" => "Preferences",
        "feedback" => "Feedback sent",
        _ => "",
    }
}

fn record_list(t: &Terminal, args: &[String], rows: &[Value], empty: &str) -> String {
    if matches!(
        args.first().map(String::as_str),
        Some("workspace" | "workspaces")
    ) && subcommand(args) == "list"
    {
        return workspace_overview(t, rows, empty);
    }
    let noun = match args.first().map(String::as_str).unwrap_or("") {
        "workspace" | "workspaces" => "workspace",
        "project" | "projects" => "project",
        "agent" | "agents" => "agent",
        "automation" | "automations" if subcommand(args) == "runs" => "run",
        "automation" | "automations" => "automation",
        "machines" => "machine",
        "theme" => "theme",
        "settings" => "setting",
        "org" => "organisation",
        "integrations" | "integration" if subcommand(args) == "list" => "service",
        _ => "result",
    };
    let mut out = t.banner("");
    t.section(&mut out, title(args));
    let summary = format!(
        "{} {}{}",
        rows.len(),
        noun,
        if rows.len() == 1 { "" } else { "s" }
    );
    t.paragraph(&mut out, &t.muted(&summary), 2);
    if noun == "workspace" {
        let agents: Vec<_> = rows
            .iter()
            .filter_map(|row| row["agents"].as_array())
            .flatten()
            .collect();
        let busy = agents
            .iter()
            .filter(|agent| matches!(agent["state"].as_str(), Some("working" | "running")))
            .count();
        let attention = agents
            .iter()
            .filter(|agent| matches!(agent["state"].as_str(), Some("needs_input" | "failed")))
            .count();
        t.paragraph(
            &mut out,
            &format!(
                "{} agent sessions  ·  {} busy  ·  {} need attention",
                agents.len(),
                busy,
                attention
            ),
            2,
        );
    }
    let explanation = match noun {
        "workspace" => "Each workspace has its own branch, folder and agent sessions.",
        "agent" if subcommand(args) == "list" => {
            "Available agent keys. Use a key with --agent when starting work."
        }
        "agent" => {
            "Working = busy. Waiting = ready for your next message. Needs input = approval or an answer is required."
        }
        "project" => "Your projects and the folders they use on disk.",
        "automation" => {
            "Scheduled agents. Review the schedule, target workspace and latest run below."
        }
        "run" => "Recent runs and their recorded progress, replies and errors.",
        "service" => "Connection state and the actions each service allows.",
        "setting" => "Current values, with a description of what each preference controls.",
        "machine" => "Saved remote machines and their associated projects.",
        "organisation" => "Your organisations. Active identifies the one these commands use.",
        "theme" => "Available colour themes. Active identifies your current theme.",
        _ => "Full details from Dragent.",
    };
    t.paragraph(&mut out, explanation, 2);
    if rows.is_empty() {
        out.push('\n');
        t.paragraph(&mut out, &clean(empty), 4);
    }
    for (index, row) in rows.iter().enumerate() {
        let primary = ["name", "title", "key", "agent", "service", "id"]
            .into_iter()
            .find(|key| row[*key].as_str().is_some_and(|value| !value.is_empty()));
        let heading = primary
            .and_then(|key| row[key].as_str())
            .unwrap_or("Details");
        out.push('\n');
        // Preserve the full name; status has its own row. Quoted link values
        // wrap into gutter-aligned rows with their complete target retained.
        let badge = if noun == "workspace" {
            let states: Vec<_> = row["agents"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|agent| agent["state"].as_str())
                .collect();
            if states.contains(&"needs_input") {
                "  [NEEDS INPUT]"
            } else if states.contains(&"failed") {
                "  [AGENT ERROR]"
            } else if states
                .iter()
                .any(|state| matches!(*state, "working" | "running"))
            {
                "  [RUNNING]"
            } else if states.is_empty() {
                "  [NO AGENTS]"
            } else if states.iter().all(|state| *state == "waiting") {
                "  [WAITING]"
            } else {
                "  [STATUS UNKNOWN]"
            }
        } else {
            ""
        };
        let header = clean(heading);
        let lines: Vec<_> = header
            .lines()
            .flat_map(|line| wrap(line, t.width.saturating_sub(10).max(1)))
            .collect();
        for (line, text) in lines.iter().enumerate() {
            let prefix = if line == 0 {
                format!("  {}  ", t.accent(&format!("{:02}", index + 1)))
            } else {
                "      ".to_string()
            };
            out.push_str(&format!("{prefix}{}\n", t.strong(text)));
        }
        if !badge.is_empty() {
            t.card_paragraph(
                &mut out,
                &format!(
                    "{}  {}",
                    t.muted("Workspace status:"),
                    t.strong(badge.trim().trim_matches(['[', ']']))
                ),
                5,
            );
        }
        if let Some(object) = row.as_object() {
            let preferred = [
                "active",
                "connected",
                "enabled",
                "state",
                "status",
                "project",
                "branch",
                "agents",
                "schedule_label",
                "timezone",
                "workspace",
                "path",
                "workspace_path",
                "cwd",
                "id",
            ];
            let mut shown = std::collections::HashSet::new();
            if let Some(key) = primary {
                shown.insert(key);
            }
            for key in preferred {
                if let Some(value) = object.get(key) {
                    if shown.insert(key) {
                        if key == "project"
                            && value
                                .as_str()
                                .is_some_and(|path| std::path::Path::new(path).is_absolute())
                        {
                            let path = value.as_str().unwrap();
                            let name = std::path::Path::new(path)
                                .file_name()
                                .and_then(|part| part.to_str())
                                .unwrap_or(path);
                            record_field(
                                t,
                                &mut out,
                                "project",
                                &Value::String(name.to_string()),
                                0,
                            );
                            record_field(t, &mut out, "project_folder", value, 0);
                        } else {
                            record_field(t, &mut out, key, value, 0);
                        }
                    }
                }
            }
            // Retain every other field too: adding a field to a command must
            // never silently hide it from the human view.
            for (key, value) in object {
                if shown.insert(key) {
                    record_field(t, &mut out, key, value, 0);
                }
            }
        } else {
            record_field(t, &mut out, "value", row, 0);
        }
    }
    command_footer(t, &mut out, noun, rows);
    next_steps(t, &mut out, args);
    out
}

fn shell_arg(value: &str) -> String {
    // PowerShell and POSIX shells both accept single-quoted literal paths,
    // but escape a quote inside them differently.
    #[cfg(windows)]
    {
        format!("'{}'", value.replace('\'', "''"))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn command_footer(t: &Terminal, out: &mut String, noun: &str, rows: &[Value]) {
    let mut commands = Vec::new();
    match noun {
        "workspace" | "project" => {
            // Pick the shortest real path for the example; all paths remain
            // available in Locations. No repeated organisation hashes.
            if let Some(path) = rows
                .iter()
                .filter_map(|row| row["path"].as_str())
                .min_by_key(|path| path.len())
            {
                commands.push((
                    "Open a workspace",
                    format!("dragent open {}", shell_arg(path)),
                ));
            }
            if let Some(id) = rows
                .iter()
                .filter_map(|row| row["agents"].as_array())
                .flatten()
                .find_map(|agent| agent["id"].as_str())
            {
                commands.push((
                    "Read an agent's screen",
                    format!("dragent agents read {}", shell_arg(id)),
                ));
            }
        }
        "agent" => {
            if let Some(id) = rows.iter().find_map(|row| row["id"].as_str()) {
                commands.push((
                    "Read an agent's screen",
                    format!("dragent agents read {}", shell_arg(id)),
                ));
            }
        }
        "automation" => {
            if let Some(id) = rows.iter().find_map(|row| row["id"].as_str()) {
                commands.push((
                    "Inspect recent runs",
                    format!("dragent automations runs {}", shell_arg(id)),
                ));
            }
        }
        _ => {}
    }
    if !commands.is_empty() {
        t.section(out, "Try next");
        for (description, command) in commands {
            t.paragraph(out, &t.muted(description), 4);
            out.push_str(&format!("    {}\n", t.accent(&clean(&command))));
        }
    }
}

/// One-line status for a workspace: a coloured dot plus a short label.
/// Green = busy, yellow = needs you, dim = idle. Keeps the list scannable.
fn workspace_status(t: &Terminal, agents: Option<&Vec<Value>>) -> (String, &'static str) {
    let states: Vec<&str> = agents
        .into_iter()
        .flatten()
        .filter_map(|agent| agent["state"].as_str())
        .collect();
    if states.is_empty() {
        return (t.muted("○"), "idle");
    }
    if states.contains(&"needs_input") {
        return (t.paint("33", "●"), "needs input");
    }
    if states.contains(&"failed") {
        return (t.paint("31", "●"), "attention");
    }
    if states.iter().any(|s| matches!(*s, "working" | "running")) {
        return (t.paint("32", "●"), "active");
    }
    if states.iter().all(|s| *s == "waiting") {
        return (t.paint("33", "●"), "waiting");
    }
    (t.muted("●"), "active")
}

/// Compact agent summary for the meta line: `opencode · running`,
/// or `2 agents · 1 running` for multi-agent workspaces.
fn workspace_agents(agents: Option<&Vec<Value>>) -> String {
    let agents = match agents {
        Some(agents) if !agents.is_empty() => agents,
        _ => return String::new(),
    };
    if agents.len() == 1 {
        let agent = &agents[0];
        let provider = agent["agent"].as_str().unwrap_or("agent");
        let state = agent["state"].as_str().unwrap_or("");
        let label = match state {
            "working" | "running" => "running",
            "waiting" => "waiting",
            "needs_input" => "needs input",
            "failed" => "failed",
            _ => state,
        };
        let id = agent["id"].as_str().unwrap_or("");
        if id.is_empty() {
            return format!("{provider} · {label}");
        }
        return format!("{provider} · {label} {id}");
    }
    let running = agents
        .iter()
        .filter(|a| matches!(a["state"].as_str().unwrap_or(""), "working" | "running"))
        .count();
    if running > 0 {
        format!("{} agents · {} running", agents.len(), running)
    } else {
        format!("{} agents", agents.len())
    }
}

/// Single-line truncation with an ellipsis. Width-aware for CJK/wide chars.
fn truncate_end(text: &str, max: usize) -> String {
    let max = max.max(4);
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let size = ch.width().unwrap_or(0);
        if used + size > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += size;
    }
    format!("{out}…")
}

/// Shorten a filesystem path for inline display: `~` for home, then
/// middle-truncation that keeps the tail (the distinctive part).
fn shorten_path(path: &str, max: usize) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut short = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };
    if short.width() <= max.max(8) {
        return short;
    }
    // Keep the last two components fully; truncate the middle.
    let parts: Vec<&str> = short.split('/').collect();
    if parts.len() > 3 {
        let tail = parts[parts.len() - 2..].join("/");
        let candidate = format!("{}/…/{tail}", parts[0]);
        if candidate.width() <= max {
            return candidate;
        }
        short = format!("…/{tail}");
    }
    truncate_end(&short, max)
}

/// One quiet footer hint instead of the previous multi-section footers.
fn workspace_hint(rows: &[Value], width: usize) -> Option<String> {
    if width < 40 {
        return None;
    }
    let id = rows
        .iter()
        .filter_map(|row| row["agents"].as_array())
        .flatten()
        .find_map(|agent| agent["id"].as_str());
    match id {
        Some(id) => Some(format!("› dragent agents read {id}"),
        ),
        None => Some("› dragent open PATH".to_string()),
    }
}

fn workspace_overview(t: &Terminal, rows: &[Value], empty: &str) -> String {
    // Simple, scannable list: one block per workspace, no location
    // indirection, no banner noise. Paths are shortened inline so the
    // layout stays clean on narrow terminals.
    let mut out = String::new();
    let active = rows
        .iter()
        .filter_map(|row| row["agents"].as_array())
        .flat_map(|agents| agents.iter())
        .filter(|agent| {
            matches!(
                agent["state"].as_str().unwrap_or(""),
                "working" | "running" | "waiting" | "needs_input"
            )
        })
        .count();
    let summary = if rows.is_empty() {
        "Workspaces".to_string()
    } else if active == 0 {
        format!("Workspaces · {}", rows.len())
    } else {
        format!(
            "Workspaces · {} ({} active)",
            rows.len(),
            active
        )
    };
    out.push('\n');
    t.paragraph(&mut out, &t.strong(&summary), 2);
    if rows.is_empty() {
        t.paragraph(&mut out, &t.muted(&clean(empty)), 2);
        return out;
    }
    for row in rows {
        out.push('\n');
        let agents = row["agents"].as_array();
        let (dot, _) = workspace_status(t, agents);
        let name = truncate_end(
            &clean(row["name"].as_str().unwrap_or("Untitled")),
            t.width.saturating_sub(6).clamp(12, 56),
        );
        t.paragraph(&mut out, &format!("{dot} {}", t.strong(&name)), 2);

        let project = row["project"]
            .as_str()
            .and_then(|path| std::path::Path::new(path).file_name())
            .and_then(|name| name.to_str())
            .map(clean)
            .unwrap_or_default();
        let branch = row["branch"].as_str().map(clean).unwrap_or_default();
        let agent_text = workspace_agents(agents);
        let mut meta: Vec<String> = Vec::new();
        if !branch.is_empty() {
            meta.push(truncate_end(&branch, 32));
        }
        if !project.is_empty() {
            meta.push(truncate_end(&project, 32));
        }
        if !agent_text.is_empty() {
            meta.push(agent_text);
        } else {
            meta.push("idle".to_string());
        }
        // Keep the meta line to a single row: truncate the joined text
        // rather than wrapping it into a paragraph.
        let meta_line = truncate_end(
            &meta.join(" · "),
            t.width.saturating_sub(4).max(16),
        );
        t.paragraph(&mut out, &t.muted(&meta_line), 4);

        if let Some(path) = row["path"].as_str() {
            let short = shorten_path(path, t.width.saturating_sub(4).max(16));
            if !short.is_empty() {
                t.paragraph(&mut out, &t.muted(&short), 4);
            }
        }
        // Surface any extra fields compactly instead of dropping them.
        if let Some(fields) = row.as_object() {
            for (key, value) in fields {
                if !matches!(
                    key.as_str(),
                    "name" | "project" | "branch" | "agents" | "path"
                ) {
                    record_field(t, &mut out, key, value, 0);
                }
            }
        }
    }
    // One quiet hint instead of the previous Try-next + Explore-more blocks.
    if let Some(hint) = workspace_hint(rows, t.width) {
        out.push('\n');
        t.paragraph(&mut out, &t.muted(&hint), 2);
    }
    out
}

fn field_label(key: &str) -> String {
    match key {
        "cwd" => "Working directory".into(),
        "id" => "ID".into(),
        "session" => "Session ID".into(),
        "since_ms" => "Activity since (Unix ms)".into(),
        "for" => "Last activity".into(),
        "path" => "Folder".into(),
        "workspace_path" => "Workspace folder".into(),
        "about" => "Description".into(),
        "can" => "Allowed actions".into(),
        "schedule_label" => "Schedule description".into(),
        "next_run_at" => "Next run (Unix ms)".into(),
        _ => {
            let value = key.replace('_', " ").replace('-', " ");
            let mut chars = value.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

fn record_field(t: &Terminal, out: &mut String, key: &str, value: &Value, depth: usize) {
    let indent = 5 + depth * 2;
    let label = if key == "session" && value.is_boolean() {
        "Project-less session".into()
    } else {
        field_label(key)
    };
    let label = format!("{label}:");
    match value {
        Value::Object(fields) => {
            t.card_paragraph(out, &t.muted(&label), indent);
            if fields.is_empty() {
                t.card_paragraph(out, "None", indent + 2);
            }
            for (key, value) in fields {
                record_field(t, out, key, value, depth + 1);
            }
        }
        Value::Array(values) => {
            t.card_paragraph(
                out,
                &t.muted(&format!("{label} {}", values.len())),
                indent,
            );
            if values.is_empty() {
                t.card_paragraph(out, "None", indent + 2);
            }
            for (index, item) in values.iter().enumerate() {
                if key == "agents"
                    && item.as_object().is_some_and(|object| {
                        object
                            .keys()
                            .all(|key| matches!(key.as_str(), "id" | "agent" | "state"))
                    })
                {
                    t.card_paragraph(
                        out,
                        &format!(
                            "{}  {}  {}",
                            t.accent(&clean(item["id"].as_str().unwrap_or("No ID"))),
                            clean(item["agent"].as_str().unwrap_or("Agent")),
                            activity_label(t, item["state"].as_str().unwrap_or("unknown"))
                        ),
                        indent + 2,
                    );
                } else if item.is_object() {
                    t.card_paragraph(out, &t.muted(&format!("#{}", index + 1)), indent + 2);
                    for (key, value) in item.as_object().unwrap() {
                        record_field(t, out, key, value, depth + 2);
                    }
                } else {
                    record_field(t, out, &format!("#{}", index + 1), item, depth + 1);
                }
            }
        }
        _ => {
            let text = match value {
                Value::Null => "Not set".to_string(),
                Value::Bool(true) => t.paint("32", "Yes"),
                Value::Bool(false) => "No".to_string(),
                Value::String(value) if matches!(key, "state" | "status") => {
                    activity_label(t, value)
                }
                Value::String(value) => display_path(&clean(value)),
                Value::Number(number)
                    if key.ends_with("_ms") || key.ends_with("_at") || key == "at" =>
                {
                    number
                        .as_i64()
                        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                        .map(|time| {
                            format!(
                                "{}  (Unix ms: {number})",
                                time.with_timezone(&chrono::Local)
                                    .format("%d %b %Y, %H:%M:%S %Z")
                            )
                        })
                        .unwrap_or_else(|| number.to_string())
                }
                _ => value.to_string(),
            };
            let text = if text.is_empty() {
                "Empty".to_string()
            } else {
                text
            };
            let column = 21usize;
            let linked_value = value
                .as_str()
                .is_some_and(|value| display_path(value) != value);
            let prose = matches!(key, "prompt" | "reply" | "body" | "message" | "error")
                && value.is_string()
                && (text.contains('\n') || clean(&text).width() > 100);
            if !linked_value
                && !prose
                && t.width.saturating_sub(indent) >= column + 20
                && label.width() <= column
            {
                let prefix = format!(
                    "{}{}",
                    t.muted(&label),
                    " ".repeat(column - label.width() + 2)
                );
                let value_indent = indent + column + 2;
                let lines: Vec<_> = text
                    .split('\n')
                    .flat_map(|line| wrap(line, t.width.saturating_sub(value_indent).max(1)))
                    .collect();
                for (index, line) in lines.iter().enumerate() {
                    if index == 0 {
                        t.card_paragraph(out, &format!("{prefix}{line}"), indent);
                    } else {
                        t.card_paragraph(out, line, value_indent);
                    }
                }
            } else {
                t.card_paragraph(out, &t.muted(&label), indent);
                t.card_paragraph(out, &text, indent + 2);
            }
        }
    }
}

fn activity_label(t: &Terminal, value: &str) -> String {
    let (label, color) = match value {
        "running" => ("Running", "32"),
        "working" => ("Working", "32"),
        "waiting" => ("Waiting for your next message", "33"),
        "needs_input" => ("Needs your input", "33"),
        "failed" => ("Failed — review the error", "31"),
        "done" | "completed" => ("Completed", "32"),
        "unknown" => ("Unknown — activity not reported", "90"),
        _ => return state(t, &clean(value)),
    };
    format!("{} {}", t.paint(color, "●"), t.paint(color, label))
}

fn next_steps(t: &Terminal, out: &mut String, args: &[String]) {
    let command = args.first().map(String::as_str).unwrap_or("");
    let suggestions: &[(&str, &str)] = match command {
        "workspace" | "workspaces" => &[
            ("dragent agents running", "See all agent activity"),
            ("dragent workspaces help", "Workspace options and commands"),
        ],
        "project" | "projects" | "open" => &[
            ("dragent workspaces list", "Browse workspaces"),
            ("dragent projects list", "Browse projects"),
        ],
        "agent" | "agents" => &[
            ("dragent agents running", "See agent activity and IDs"),
            ("dragent agents list", "See installed agents"),
        ],
        "automation" | "automations" => &[
            (
                "dragent automations list",
                "Browse schedules and recent activity",
            ),
            (
                "dragent automations help",
                "Scheduling and workspace options",
            ),
        ],
        "status" | "whoami" | "org" => &[
            (
                "dragent projects list",
                "Browse projects in this organisation",
            ),
            ("dragent org list", "See which organisation is active"),
        ],
        "settings" | "theme" => &[
            ("dragent settings list", "See current preferences"),
            ("dragent theme list", "Browse available themes"),
        ],
        "machines" => &[("dragent projects list", "Browse available projects")],
        "integration" | "integrations" => &[
            ("dragent integrations list", "See connection status"),
            ("dragent integrations help", "Connected service commands"),
        ],
        _ => &[("dragent help", "Browse available commands")],
    };
    t.section(out, "Explore more");
    for (command, description) in suggestions {
        t.command(out, command, description, 36);
    }
}

fn subcommand(args: &[String]) -> String {
    super::Flags::parse(args.get(1..).unwrap_or_default())
        .map(|flags| flags.subcommand().to_string())
        .unwrap_or_default()
}

fn headers(args: &[String], count: usize) -> Vec<&'static str> {
    let command = args.first().map(String::as_str).unwrap_or("");
    let action = subcommand(args);
    let columns: &[&str] = match (command, action.as_str()) {
        ("projects" | "project", _) => &["PROJECT", "PATH"],
        ("workspaces" | "workspace", _) => &["WORKSPACE", "BRANCH", "PROJECT", "AGENTS", "PATH"],
        ("agents" | "agent", "list") => &["KEY", "AGENT"],
        ("agents" | "agent", "running" | "status") => &["ID", "AGENT", "ACTIVITY", "WORKSPACE"],
        ("org", _) => &["ORGANISATION", "ROLE", "ID"],
        ("machines", _) => &["MACHINE", "PROJECTS", "ID"],
        ("theme", _) => &["ID", "THEME"],
        ("settings", _) => &["SETTING", "VALUE", "DESCRIPTION"],
        _ => &[],
    };
    if columns.len() == count {
        columns.to_vec()
    } else {
        Vec::new()
    }
}

fn table(t: &Terminal, out: &mut String, args: &[String], raw: &str) {
    let rows: Vec<Vec<String>> = raw
        .lines()
        .filter(|line| line.contains('\t'))
        .map(|line| line.split('\t').map(display_path).collect())
        .collect();
    let count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let labels = headers(args, count);
    let mut widths = vec![0; count];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.width());
        }
    }
    for (i, label) in labels.iter().enumerate() {
        widths[i] = widths[i].max(label.width());
    }
    let wide = widths.iter().sum::<usize>() + count.saturating_sub(1) * 3 + 4 <= t.width;
    out.push('\n');
    if wide {
        if !labels.is_empty() {
            let header = labels
                .iter()
                .enumerate()
                .map(|(i, value)| format!("{value:padding$}", padding = widths[i]))
                .collect::<Vec<_>>()
                .join("   ");
            out.push_str(&format!("  {}\n\n", t.muted(&header)));
        }
        for row in &rows {
            out.push_str("  ");
            for (i, value) in row.iter().enumerate() {
                out.push_str(&if i == 0 {
                    t.strong(value)
                } else {
                    state(t, value)
                });
                if i + 1 < row.len() {
                    out.push_str(&" ".repeat(widths[i] - value.width() + 3));
                }
            }
            out.push('\n');
        }
    } else {
        // Keep long paths, ids and descriptions complete. Compact terminals
        // get records rather than clipped columns or horizontal scrolling.
        for row in &rows {
            for (i, value) in row.iter().enumerate() {
                let prefix = if i == 0 || labels.is_empty() {
                    String::new()
                } else {
                    format!("{}  ", labels[i].to_lowercase())
                };
                let text = format!("{prefix}{value}");
                for line in wrap(&text, t.width.saturating_sub(4).max(1)) {
                    let rendered = if i == 0 {
                        t.strong(&line)
                    } else {
                        state(t, &line)
                    };
                    out.push_str(&format!("  {rendered}\n"));
                }
            }
            out.push('\n');
        }
    }
    for note in raw
        .lines()
        .filter(|line| !line.contains('\t') && !line.is_empty())
    {
        out.push('\n');
        t.paragraph(out, note, 2);
    }
    out.push_str(&format!(
        "\n  {}\n",
        t.muted(&format!(
            "{} {}",
            rows.len(),
            if rows.len() == 1 { "item" } else { "items" }
        ))
    ));
}

fn state(t: &Terminal, value: &str) -> String {
    let primary = value.split(" · ").next().unwrap_or(value);
    let color = match primary.trim().to_ascii_lowercase().as_str() {
        "running" | "working" | "connected" | "done" | "completed" | "enabled" => "32",
        "waiting" | "needs input" | "needs_input" | "needs permission" | "paused" | "offline" => {
            "33"
        }
        "failed" | "stopped with an error" | "error" | "not running" | "disconnected" => "31",
        _ => return value.to_string(),
    };
    t.paint(color, value)
}

/// Strip terminal controls from API/user-provided display data. Raw terminal
/// transcripts deliberately bypass presentation instead.
fn clean(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    let mut escape = false;
                    for ch in chars.by_ref() {
                        if ch == '\x07' || escape && ch == '\\' {
                            break;
                        }
                        escape = ch == '\x1b';
                    }
                }
                _ => {}
            }
        } else if !ch.is_control() || matches!(ch, '\n' | '\t') {
            out.push(ch);
        }
    }
    out
}

/// Quoted path values remain one link even when directory names have spaces.
/// Presentation only: JSON/quiet/redirected output retain their original data.
fn display_path(value: &str) -> String {
    let path = value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("./")
        || value.starts_with("../")
        || value
            .as_bytes()
            .get(1..3)
            .is_some_and(|v| v == b":\\" || v == b":/");
    let path = path || value.starts_with("https://") || value.starts_with("http://");
    if path {
        if !value.contains('"') {
            return format!("\"{value}\"");
        }
        if !value.contains('\'') {
            return format!("'{value}'");
        }
    }
    value.to_string()
}

fn split_link(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let size = ch.width().unwrap_or(0);
        if used + size > width && used > 0 {
            // Default-colour trailing spaces are trimmed by terminal rows.
            // Move them to the next row so path spaces survive reconstruction.
            let row = rows.last_mut().unwrap();
            let end = row.trim_end_matches(' ').len();
            let spaces = row.split_off(end);
            used = spaces.len();
            rows.push(spaces);
        }
        rows.last_mut().unwrap().push(ch);
        used += size;
    }
    rows
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    // Never emit a real newline inside a link. The terminal owns visual
    // wrapping and records continuation rows for hit-testing/highlighting.
    if !crate::terminal::render::block_output::find_terminal_links(&clean(text)).is_empty() {
        return vec![text.to_string()];
    }
    let mut lines = vec![String::new()];
    let mut used = 0;
    let mut escape = false;
    let mut last_space = None;
    for ch in text.chars() {
        if ch == '\x1b' {
            escape = true;
        }
        if escape {
            lines.last_mut().unwrap().push(ch);
            if ch == 'm' {
                escape = false;
            }
            continue;
        }
        let size = ch.width().unwrap_or(0);
        if used + size > width && used > 0 {
            let remaining = if let Some(index) = last_space.take() {
                let tail = lines.last_mut().unwrap().split_off(index);
                tail.trim_start().to_string()
            } else {
                String::new()
            };
            used = clean(&remaining).width();
            lines.push(remaining);
        }
        if ch == ' ' && used == 0 {
            continue;
        }
        if ch == ' ' {
            last_space = Some(lines.last().unwrap().len());
        }
        lines.last_mut().unwrap().push(ch);
        used += size;
    }
    lines
}

#[cfg(unix)]
fn terminal_width(stderr: bool) -> Option<usize> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let fd = if stderr {
        libc::STDERR_FILENO
    } else {
        libc::STDOUT_FILENO
    };
    (unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0)
        .then_some(size.ws_col as usize)
}
#[cfg(not(unix))]
fn terminal_width(_: bool) -> Option<usize> {
    None
}

#[cfg(not(windows))]
fn ansi_supported(_: bool) -> bool {
    true
}
#[cfg(windows)]
fn ansi_supported(stderr: bool) -> bool {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let Ok(handle) = GetStdHandle(if stderr {
            STD_ERROR_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        }) else {
            return false;
        };
        let mut mode = CONSOLE_MODE(0);
        GetConsoleMode(handle, &mut mode).is_ok()
            && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING).is_ok()
    }
}
