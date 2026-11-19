//! `dragent capabilities`: what this build supports, in one machine-readable
//! answer, so agents and skills discover commands, tool schemas and limits
//! instead of guessing from memory or an older build's documentation.
//!
//! Everything here is read from the same sources the commands use: the
//! command table and help, the MCP tool catalog (`dragent mcp`), the bundled
//! skill catalog and the platform. It never contacts the app or a service,
//! so it works signed out and while Dragent isn't running.

use serde_json::{json, Value};

/// Command families: (command, what it covers, how to get its full help,
/// whether it needs the app running).
const COMMANDS: &[(&str, &str, &str, bool)] = &[
    (
        "capabilities",
        "This report",
        "dragent capabilities --json",
        false,
    ),
    (
        "requests",
        "Whether the app finished a request a command stopped waiting for",
        "dragent help",
        false,
    ),
    (
        "status",
        "Whether Dragent is running, its version and organisation",
        "dragent help",
        false,
    ),
    (
        "standup",
        "Read-only recap across workspaces, agents, teams and runs",
        "dragent standup --help",
        false,
    ),
    (
        "doctor",
        "Layer-by-layer diagnostics: launcher, app, sign-in, agents, skills",
        "dragent doctor --help",
        false,
    ),
    (
        "setup",
        "Repository setup for workspaces: check, init, launch, status, stop, cleanup",
        "dragent setup --help",
        false,
    ),
    (
        "plugins",
        "Packages of skills and MCP servers: inspect, install, status, check, update, remove",
        "dragent plugins help",
        false,
    ),
    (
        "whoami",
        "The signed-in user and organisation",
        "dragent help",
        false,
    ),
    (
        "org",
        "List and switch organisations",
        "dragent help",
        false,
    ),
    (
        "workspaces",
        "Workspaces in the sidebar: list, create, delete",
        "dragent help",
        true,
    ),
    (
        "projects",
        "Projects open in Dragent: list, add",
        "dragent help",
        true,
    ),
    ("open", "Show a folder in Dragent", "dragent help", false),
    (
        "agents",
        "Installed agents, running sessions, provider accounts",
        "dragent help",
        true,
    ),
    (
        "browser",
        "The embedded browser pane: open, snapshot, act",
        "dragent browser help",
        true,
    ),
    (
        "open-url",
        "Open a link the way a click in Dragent would",
        "dragent help",
        true,
    ),
    (
        "computer",
        "Desktop apps on this machine through accessibility",
        "dragent computer help",
        true,
    ),
    (
        "panes",
        "Tabs and panes of the caller's workspace",
        "dragent panes help",
        true,
    ),
    (
        "automations",
        "Scheduled and watch-triggered agent runs",
        "dragent automations help",
        false,
    ),
    ("team", "Lead/worker teams", "dragent team help", true),
    (
        "tasks",
        "A team's tasks, dependencies and results",
        "dragent team help",
        true,
    ),
    ("msg", "A team's mailbox", "dragent team help", true),
    (
        "issues",
        "The organisation's task list: list, show, create, update, comment, link, delete, restore",
        "dragent issues help",
        false,
    ),
    ("work", "Work items", "dragent work help", true),
    (
        "integrations",
        "Connected services: list, describe, search, show, comment, status, create, api",
        "dragent integrations help",
        false,
    ),
    (
        "mcp",
        "All of the tools above as an MCP server on stdio",
        "dragent mcp (stdio)",
        false,
    ),
    (
        "machines",
        "SSH machines added to Dragent",
        "dragent help",
        false,
    ),
    (
        "settings",
        "Read and change settings: list, get, set, reset",
        "dragent settings list",
        false,
    ),
    ("theme", "List and set the theme", "dragent help", false),
    (
        "feedback",
        "Send feedback to the Dragent team",
        "dragent help",
        false,
    ),
];

pub fn run(args: &[String]) -> Result<String, String> {
    let json = !args.iter().any(|arg| arg == "--text");
    let report = report();
    if json {
        return serde_json::to_string_pretty(&report).map_err(|error| error.to_string());
    }
    Ok(text(&report))
}

pub(crate) fn report() -> Value {
    let commands: Vec<Value> = COMMANDS
        .iter()
        .map(|(name, summary, help, needs_app)| {
            json!({
                "name": name,
                "summary": summary,
                "help": help,
                "needs_app": needs_app,
                "needs_sign_in": super::needs_sign_in(name, &[]),
            })
        })
        .collect();
    let tools: Vec<Value> = crate::integrations::bridge::catalog();
    let skills: Vec<Value> = dragent_host::agent_skills::BUNDLED_SKILLS
        .iter()
        .map(|skill| {
            json!({
                "name": skill.name,
                "files": skill.files.iter().map(|file| file.path).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "schema": 1,
        "dragent_version": env!("CARGO_PKG_VERSION"),
        "skills_version": dragent_host::agent_skills::BUNDLE_VERSION,
        "platform": platform(),
        "commands": commands,
        "mcp_tools": tools,
        "skills": {
            "bundled": skills,
            "shared_folders_installed": super::agent_skill::enabled(),
            "desktop_guidance_installed": super::agent_skill::computer_enabled(),
            "native_location": dragent_host::agent_skills::bundle_root()
                .join(dragent_host::agent_skills::BUNDLE_VERSION),
        },
        "results": dragent_host::outcome::contract(),
        "result_codes": {
            "computer": dragent_computer::errors::CODES,
        },
        "output": "JSON by default when an agent or CI runs a command; --text for people.",
    })
}

fn platform() -> Value {
    let os = std::env::consts::OS;
    let display = if cfg!(target_os = "linux") {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "wayland"
        } else if std::env::var_os("DISPLAY").is_some() {
            "x11"
        } else {
            "none"
        }
    } else {
        "native"
    };
    let mut limits = Vec::new();
    if display == "none" {
        limits.push("No desktop display: browser panes, panes and computer use need the Dragent app on a desktop.");
    }
    if display == "wayland" {
        limits.push("Wayland: computer use clicks and typing need ydotool; screenshots need grim.");
    }
    json!({
        "os": os,
        "arch": std::env::consts::ARCH,
        "display": display,
        "computer_use": {
            "backend": match os {
                "macos" => "accessibility (AX) + Quartz events",
                "windows" => "UI Automation + SendInput",
                "linux" => "AT-SPI + xdotool/ydotool",
                _ => "unsupported",
            },
            "check": "dragent computer permissions",
        },
        "limits": limits,
    })
}

fn text(report: &Value) -> String {
    let mut out = format!(
        "Dragent {} · skills {} · {} {}\n\nCommands:\n",
        report["dragent_version"].as_str().unwrap_or_default(),
        report["skills_version"].as_str().unwrap_or_default(),
        report["platform"]["os"].as_str().unwrap_or_default(),
        report["platform"]["arch"].as_str().unwrap_or_default(),
    );
    for command in report["commands"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  {:<14} {}  ({})\n",
            command["name"].as_str().unwrap_or_default(),
            command["summary"].as_str().unwrap_or_default(),
            command["help"].as_str().unwrap_or_default(),
        ));
    }
    out.push_str(&format!(
        "\nMCP tools: {} (`dragent capabilities --json` for their schemas)\nSkills: {}\n",
        report["mcp_tools"].as_array().map_or(0, Vec::len),
        report["skills"]["bundled"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|skill| skill["name"].as_str())
            .collect::<Vec<_>>()
            .join(", "),
    ));
    for limit in report["platform"]["limits"]
        .as_array()
        .into_iter()
        .flatten()
    {
        out.push_str(&format!("Limit: {}\n", limit.as_str().unwrap_or_default()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_matches_the_commands_tools_and_skills_this_build_has() {
        let report = report();
        // Every listed command is one the dispatcher really accepts.
        for command in report["commands"].as_array().unwrap() {
            let name = command["name"].as_str().unwrap();
            assert!(
                super::super::COMMANDS.contains(&name)
                    || matches!(name, "open-url" | "panes" | "browser"),
                "{name} isn't a command"
            );
        }
        // Every family the dispatcher accepts is described.
        for name in super::super::COMMANDS {
            let family = match name {
                "teams" => "team",
                "task" => "tasks",
                "message" => "msg",
                "integration" => "integrations",
                "automation" => "automations",
                "workspace" => "workspaces",
                "project" => "projects",
                "agent" => "agents",
                "help" | "version" => continue,
                other => other,
            };
            assert!(
                COMMANDS.iter().any(|(command, ..)| *command == family),
                "{name} missing from the capability report"
            );
        }
        let tools = report["mcp_tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        for tool in tools {
            assert!(tool["name"].is_string() && tool["inputSchema"].is_object());
        }
        assert_eq!(
            report["skills"]["bundled"].as_array().unwrap().len(),
            dragent_host::agent_skills::BUNDLED_SKILLS.len()
        );
        assert!(!report["result_codes"]["computer"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            report["results"]["statuses"].as_array().unwrap().len(),
            dragent_host::outcome::Status::ALL.len()
        );
        assert!(text(&report).contains("Commands:"));
    }

    #[test]
    fn help_pointers_name_commands_that_answer_help() {
        // Families whose own parser accepts `help`; the rest point at the
        // top-level help or another working command.
        let handles_help = [
            "browser",
            "computer",
            "panes",
            "automations",
            "team",
            "work",
            "integrations",
            "plugins",
            "issues",
        ];
        for (name, _, help, _) in COMMANDS {
            if let Some(family) = help
                .strip_prefix("dragent ")
                .and_then(|rest| rest.strip_suffix(" help"))
            {
                assert!(
                    handles_help.contains(&family),
                    "`{help}` for {name} isn't a real help command"
                );
            }
        }
    }

    #[test]
    fn discovery_works_signed_out() {
        assert!(!super::super::needs_sign_in("capabilities", &[]));
    }
}
