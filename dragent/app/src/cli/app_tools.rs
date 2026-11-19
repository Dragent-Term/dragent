//! The `dragent mcp` tools that act in the open app: the browser beside the
//! agent's terminal, the panes of its workspace, the workspaces themselves
//! and the agents running in them. Each is a `dragent` command underneath
//! (`browser`, `panes`, `workspaces`, `agents`), so an agent gets the answer
//! a person at the terminal would.

use serde_json::{json, Value};

/// One tool: its name, what it does, its arguments as
/// `(name, kind, description, required)`, and whether it changes anything.
struct Tool {
    name: &'static str,
    description: &'static str,
    arguments: &'static [(&'static str, &'static str, &'static str, bool)],
    writes: bool,
    destructive: bool,
}

const TOOLS: &[Tool] = &[
    // ── Browser ─────────────────────────────────────────────────────────────
    // Every browser tool also takes `pane`: a browser's id from browser_list.
    Tool {
        name: "browser_list",
        description: "Dragent's browser panes: id, address, title, whether each is on screen, and which one the other browser tools use by default. Pass an id as `pane` to work in another.",
        arguments: &[],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_open",
        description: "Open an address in Dragent's browser beside your terminal, opening a browser pane if there isn't one (localhost:3000 works); `new` opens a fresh pane even if one exists. The person sees the page.",
        arguments: &[
            ("url", "string", "Address or search text", true),
            ("new", "boolean", "Open a fresh browser pane instead of reusing one", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_status",
        description: "The browser's address, title, whether the page is still loading, and what it last downloaded or opened in a new tab.",
        arguments: &[("pane", "integer", "A browser's id from browser_list", false)],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_snapshot",
        description: "The page as text with numbered elements, like [e3] button \"Sign in\" (with states such as disabled, checked, invalid), plus open dialogs, alerts, frames from other sites and drawn content. References never repeat: one from an older snapshot fails as stale_ref.",
        arguments: &[("pane", "integer", "A browser's id from browser_list", false)],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_click",
        description: "Click an element from the latest browser_snapshot. The answer says if a dialog appeared.",
        arguments: &[
            ("ref", "string", "Element reference such as e3", true),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_hover",
        description: "Move the pointer over an element (to open a menu or tooltip), then snapshot again.",
        arguments: &[
            ("ref", "string", "Element reference such as e3", true),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_type",
        description: "Replace a field's text (or choose a select option by its text); with submit, send its form. The answer includes the field's validation message if it's invalid.",
        arguments: &[
            ("ref", "string", "Element reference such as e3", true),
            ("text", "string", "Text to put in the field", true),
            ("submit", "boolean", "Send the form after typing", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_press",
        description: "Press a key in the focused element: Enter, Escape, Tab, ArrowDown, Control+a…",
        arguments: &[
            ("key", "string", "Key name, with modifiers joined by +", true),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_scroll",
        description: "Scroll the page down, up, to the top or bottom, or bring an element (e3) into view.",
        arguments: &[
            ("to", "string", "down, up, top, bottom or an element reference", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_wait",
        description: "Wait until every condition holds, checking a few times a second (default 10 s, at most 60): text appears, text is gone, a CSS selector is visible, the address contains something, an element is gone, or the page is idle (loaded, no requests running, nothing changing). Use it instead of sleeping; on timeout it says what was still missing.",
        arguments: &[
            ("text", "string", "Text that must appear", false),
            ("gone", "string", "Text that must disappear", false),
            ("selector", "string", "A CSS selector that must match something visible", false),
            ("url", "string", "Part of the address", false),
            ("ref_gone", "string", "An element (e3) that must disappear", false),
            ("idle", "boolean", "Loaded, no requests running, nothing changing", false),
            ("timeout", "number", "Seconds, 0.5 to 60", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_dialog",
        description: "How to answer the next alert, confirm or prompt the page opens (unanswered ones are dismissed). Set it, then do what opens the dialog.",
        arguments: &[
            ("answer", "string", "accept or dismiss", true),
            ("text", "string", "For a prompt: the text to answer with", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_eval",
        description: "Run JavaScript in the page and return the result; with await, wait for a promise it returns.",
        arguments: &[
            ("script", "string", "JavaScript to run", true),
            ("await", "boolean", "Wait for a returned promise to settle (up to 30 s)", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_console",
        description: "What the page logged: console messages, errors, failed requests and dialogs; errors keeps only errors and failed requests; clear empties the log afterwards.",
        arguments: &[
            ("errors", "boolean", "Only errors and failed requests", false),
            ("clear", "boolean", "Empty the log after reading it", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_screenshot",
        description: "Save a PNG of the page and return where it is.",
        arguments: &[
            ("path", "string", "Where to save it (relative to your directory)", false),
            ("pane", "integer", "A browser's id from browser_list", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "browser_back",
        description: "Go back a page.",
        arguments: &[("pane", "integer", "A browser's id from browser_list", false)],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_forward",
        description: "Go forward a page.",
        arguments: &[("pane", "integer", "A browser's id from browser_list", false)],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "browser_reload",
        description: "Reload the page.",
        arguments: &[("pane", "integer", "A browser's id from browser_list", false)],
        writes: true,
        destructive: false,
    },
    // ── Panes ───────────────────────────────────────────────────────────────
    Tool {
        name: "panes_list",
        description: "The workspace tabs of the window your terminal is in, their panes (terminals, browsers, settings) and split layout, as JSON. Your own pane is marked \"you\"; \"visible\" panes are on screen; \"active\" has the keyboard.",
        arguments: &[],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "pane_open",
        description: "Open a new terminal or browser. A terminal command defaults to a split on the right of your pane (or target), keeping your agent session visible and untouched. Without a command, opens a sidebar tab. Set place to choose another side or explicitly request a tab. The keyboard stays where the person has it unless focus is true. Give a label so you can find the pane again after a timeout. Returns the new pane's id and the layout.",
        arguments: &[
            ("kind", "string", "terminal (default) or browser", false),
            ("place", "string", "right by default for terminal commands, otherwise tab; also left, down or up", false),
            ("target", "integer", "Pane to split beside (default: yours)", false),
            ("command", "string", "Command to run in a new terminal", false),
            ("url", "string", "Address for a new browser", false),
            ("label", "string", "Name shown on the pane", false),
            ("focus", "boolean", "Give the new pane the keyboard", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "pane_move",
        description: "Show an existing tab's session beside a pane that's on screen, or move a pane within the split. Nothing restarts.",
        arguments: &[
            ("id", "integer", "Pane to move", true),
            ("target", "integer", "Pane to place it beside (default: yours)", false),
            ("place", "string", "right (default), left, down or up", false),
            ("focus", "boolean", "Give the moved pane the keyboard", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "pane_unsplit",
        description: "Take a pane out of the split into its own tab; its session keeps running.",
        arguments: &[("id", "integer", "Pane id from panes_list", true)],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "pane_swap",
        description: "Exchange the places of two panes in the split on screen.",
        arguments: &[
            ("id", "integer", "One pane", true),
            ("with", "integer", "The other pane", true),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "pane_resize",
        description: "Give a pane a share (0.1–0.9) of the split it's in; its neighbours keep their proportions.",
        arguments: &[
            ("id", "integer", "Pane id from panes_list", true),
            ("share", "number", "Its part of the split, 0.1 to 0.9", true),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "panes_equalize",
        description: "Even out every split on screen in your workspace tab.",
        arguments: &[],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "pane_focus",
        description: "Show a pane by its id from panes_list and give it the keyboard.",
        arguments: &[("id", "integer", "Pane id from panes_list", true)],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "pane_close",
        description: "Close a pane by its id from panes_list, ending its session. When something runs there, the person is asked first (needs_approval). You can't close your own pane.",
        arguments: &[("id", "integer", "Pane id from panes_list", true)],
        writes: true,
        destructive: true,
    },
    // ── Workspaces ──────────────────────────────────────────────────────────
    Tool {
        name: "workspaces_list",
        description: "The workspaces in Dragent (project, branch, path), as JSON.",
        arguments: &[("project", "string", "Only this project's (its folder)", false)],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "workspace_create",
        description: "A new workspace for a project: a worktree on a new branch (or the project's own checkout with local), optionally with an agent started on a prompt, or a command run in it.",
        arguments: &[
            ("project", "string", "The project's folder (defaults to the one you're in)", false),
            ("prompt", "string", "What the agent should do", false),
            ("agent", "string", "Agent to start: claude, codex, gemini…", false),
            ("branch", "string", "Branch name for the worktree", false),
            ("name", "string", "The workspace's name", false),
            ("command", "string", "Command to run in its first terminal", false),
            ("local", "boolean", "Use the project's checkout instead of a worktree", false),
        ],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "workspace_open",
        description: "Show a folder in Dragent: its workspace if the sidebar has one, else the folder as a project.",
        arguments: &[("path", "string", "Folder to show", true)],
        writes: true,
        destructive: false,
    },
    Tool {
        name: "workspace_delete",
        description: "Close a workspace and remove its worktree. Refused while it has uncommitted changes or running agents.",
        arguments: &[("path", "string", "The workspace's folder", true)],
        writes: true,
        destructive: true,
    },
    // ── Agents ──────────────────────────────────────────────────────────────
    Tool {
        name: "agents_list",
        description: "The agents running in Dragent's terminals (id, agent, workspace, state), as JSON.",
        arguments: &[
            ("project", "string", "Only this project's agents (its folder)", false),
            ("workspace", "string", "Only this workspace's agents (its folder)", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "agent_read",
        description: "What a running agent's terminal shows now.",
        arguments: &[
            ("id", "string", "Agent id from agents_list (its first characters are enough)", true),
            ("lines", "integer", "How many of the last lines", false),
        ],
        writes: false,
        destructive: false,
    },
    Tool {
        name: "agent_send",
        description: "Type a message into a running agent's terminal and press Enter, as a person would.",
        arguments: &[
            ("id", "string", "Agent id from agents_list", true),
            ("text", "string", "The message", true),
        ],
        writes: true,
        destructive: false,
    },
];

/// The tools as MCP describes them.
pub(crate) fn catalog() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for (name, kind, description, needed) in tool.arguments {
                properties.insert(
                    name.to_string(),
                    json!({ "type": kind, "description": description }),
                );
                if *needed {
                    required.push(*name);
                }
            }
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": { "type": "object", "properties": properties, "required": required },
                "annotations": {
                    "readOnlyHint": !tool.writes,
                    "destructiveHint": tool.destructive,
                    "openWorldHint": false,
                },
            })
        })
        .chain(super::computer::catalog())
        .collect()
}

/// Whether `name` is one of these tools.
pub(crate) fn has(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
        || super::computer::catalog()
            .iter()
            .any(|tool| tool["name"] == name)
}

/// CLI and MCP command launches share the same placement default. An explicit
/// tab or side always wins; a command must stay visible beside its caller.
pub(crate) fn pane_open_place(arguments: &Value) -> &str {
    let text = |key: &str| arguments[key].as_str().map(str::trim).filter(|s| !s.is_empty());
    if let Some(place) = text("place") {
        return place;
    }
    let terminal = text("kind").unwrap_or("terminal") == "terminal";
    if terminal && text("command").is_some() {
        "right"
    } else {
        "tab"
    }
}

/// Run a tool: the `dragent` command it stands for.
pub(crate) fn call(name: &str, arguments: &Value) -> Result<String, String> {
    if let Some(command) = name.strip_prefix("computer_") {
        return super::computer::call_tool(command, arguments.clone());
    }
    if let Some(command) = name.strip_prefix("browser_") {
        return super::browser::call_tool(command, arguments.clone());
    }
    match name {
        "panes_list" => super::panes::call_tool("list", json!({})),
        "pane_open" => super::panes::call_tool("open", arguments.clone()),
        "pane_move" => super::panes::call_tool("move", arguments.clone()),
        "pane_unsplit" => super::panes::call_tool("unsplit", arguments.clone()),
        "pane_swap" => super::panes::call_tool("swap", arguments.clone()),
        "pane_resize" => super::panes::call_tool("resize", arguments.clone()),
        "panes_equalize" => super::panes::call_tool("equalize", json!({})),
        "pane_focus" => super::panes::call_tool("focus", arguments.clone()),
        "pane_close" => super::panes::call_tool("close", arguments.clone()),
        _ => super::dispatch(&command_line(name, arguments)?),
    }
}

/// The `dragent` command line a workspace or agent tool stands for.
fn command_line(name: &str, arguments: &Value) -> Result<Vec<String>, String> {
    let text = |key: &str| -> Option<String> {
        match &arguments[key] {
            Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        }
    };
    let needed = |key: &str| text(key).ok_or_else(|| format!("{name} needs {key}"));
    let flag = |key: &str| text(key).map(|value| format!("--{key}={value}"));
    let words = |list: &[&str]| list.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    let mut line = match name {
        "workspaces_list" => {
            let mut line = words(&["workspaces", "list"]);
            line.extend(flag("project"));
            line
        }
        "workspace_create" => {
            let mut line = words(&["workspaces", "create"]);
            for key in ["project", "prompt", "agent", "branch", "name", "command"] {
                line.extend(flag(key));
            }
            if arguments["local"].as_bool() == Some(true) {
                line.push("--local".into());
            }
            line
        }
        "workspace_open" => vec!["open".into(), needed("path")?],
        "workspace_delete" => vec!["workspaces".into(), "delete".into(), needed("path")?],
        "agents_list" => {
            let mut line = words(&["agents", "running"]);
            line.extend(flag("project"));
            line.extend(flag("workspace"));
            line
        }
        "agent_read" => {
            let mut line = vec!["agents".into(), "read".into(), needed("id")?];
            line.extend(flag("lines"));
            line
        }
        "agent_send" => vec![
            "agents".into(),
            "send".into(),
            needed("id")?,
            format!("--text={}", needed("text")?),
        ],
        other => return Err(format!("Unknown tool “{other}”.")),
    };
    line.push("--json".into());
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_terminal_commands_default_to_a_visible_right_split() {
        for arguments in [
            json!({"command": "npm run dev"}),
            json!({"kind": "terminal", "command": "cargo test", "target": 7}),
            json!({"command": "npm run dev", "place": null}),
            json!({"kind": " terminal ", "command": "npm run dev"}),
        ] {
            assert_eq!(pane_open_place(&arguments), "right");
        }
        for place in ["tab", "left", "down", "up", "right"] {
            assert_eq!(
                pane_open_place(&json!({"command": "cargo test", "place": place})),
                place
            );
        }
        assert_eq!(pane_open_place(&json!({"command": "cargo test", "place": " tab "})), "tab");
        for arguments in [
            json!({}),
            json!({"command": "  "}),
            json!({"kind": "browser", "url": "localhost:3000"}),
        ] {
            assert_eq!(pane_open_place(&arguments), "tab");
        }
    }

    #[test]
    fn every_tool_is_described_with_its_arguments() {
        let catalog = catalog();
        assert_eq!(
            catalog.len(),
            TOOLS.len() + crate::cli::computer::catalog().len()
        );
        let open = catalog
            .iter()
            .find(|tool| tool["name"] == "browser_open")
            .unwrap();
        assert_eq!(open["inputSchema"]["required"], json!(["url"]));
        let close = catalog.iter().find(|tool| tool["name"] == "pane_close").unwrap();
        assert_eq!(close["annotations"]["destructiveHint"], json!(true));
        let list = catalog.iter().find(|tool| tool["name"] == "panes_list").unwrap();
        assert_eq!(list["annotations"]["readOnlyHint"], json!(true));
    }

    #[test]
    fn workspace_and_agent_tools_are_their_commands() {
        assert_eq!(
            command_line(
                "workspace_create",
                &json!({ "project": "/code/app", "prompt": "-v flag", "local": true })
            )
            .unwrap(),
            ["workspaces", "create", "--project=/code/app", "--prompt=-v flag", "--local", "--json"]
        );
        assert_eq!(
            command_line("agent_send", &json!({ "id": "ab12", "text": "keep going" })).unwrap(),
            ["agents", "send", "ab12", "--text=keep going", "--json"]
        );
        assert_eq!(
            command_line("agent_read", &json!({ "id": "ab12", "lines": 40 })).unwrap(),
            ["agents", "read", "ab12", "--lines=40", "--json"]
        );
        assert!(command_line("workspace_open", &json!({})).is_err());
        assert!(command_line("nope", &json!({})).is_err());
    }

    #[test]
    fn names_do_not_clash_with_the_other_bridge_tools() {
        for tool in TOOLS {
            assert!(
                dragent_host::mcp::coordination_tools()
                    .iter()
                    .all(|other| other["name"] != tool.name),
                "{}",
                tool.name
            );
        }
    }
}
