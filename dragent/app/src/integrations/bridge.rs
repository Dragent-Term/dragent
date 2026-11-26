//! `dragent mcp`: the connected services as an MCP server on stdio, which
//! other coding agents start from their MCP settings (see
//! [`super::agents`]). One JSON-RPC message per line each way; it serves
//! tools only.

use std::path::Path;

use serde_json::{json, Value};

use super::tools::{self, TOOLS};

/// Protocol versions this server speaks; it answers with the client's when
/// it's one of them, else the newest.
#[cfg(test)]
use dragent_host::mcp::VERSIONS;

const INSTRUCTIONS: &str = "Tools for the services the user connected in Dragent: issue trackers, \
code hosts, error monitoring, support inboxes, chat and docs. Start with `services` to see what is \
connected, and `describe` for a service's accounts, operations, input schemas and scope. Comments, status changes and new items are posted under the user's own account where \
their team sees them: only make them when the user asked for it. \
The team_, task_ and msg_ tools coordinate a team of agents: a lead plans the work as tasks for \
workers and follows it with team_wait; a worker reports with task_done, task_block and msg_send. \
The browser_ tools drive Dragent's browser beside your terminal while the person watches; the \
pane_ tools open, show and close the terminals and browsers of your workspace. Use pane_open \
with command to launch a command in a new terminal beside your agent; never replace your own \
session or type a shell command into an agent's REPL. workspace_ and \
agent_ tools create workspaces and follow the agents running in them. The computer_ tools read and control desktop apps through the enabled Computer use settings; snapshots include images and accessibility text.";

/// Serve until the agent closes stdin.
pub fn serve() -> Result<(), String> {
    // Dragent's tools are for agents Dragent started: an entry someone put in
    // an agent's own settings gets nothing outside it.
    if !dragent_host::agent_skills::inside_dragent() {
        return Err(
            "Dragent's tools work only in agents started from Dragent. Start the agent from Dragent to use them.".into(),
        );
    }
    let root = std::env::current_dir().unwrap_or_default();
    dragent_host::mcp::serve_content(INSTRUCTIONS, catalog(), |name, arguments| {
        call(name, arguments, &root, crate::cli::signed_in).map(|output| content(name, &output))
    })
}

fn content(name: &str, output: &str) -> Vec<Value> {
    let (text, attachment) = if name.starts_with("computer_") {
        crate::cli::computer::unpack_agent_result(output)
    } else {
        (output.to_string(), None)
    };
    let mut content = vec![json!({"type":"text", "text":text})];
    if let Some(image) = attachment {
        content.push(json!({"type":"image", "mimeType":"image/png", "data":image["data"]}));
    }
    content
}

fn call(
    name: &str,
    arguments: &Value,
    root: &Path,
    signed_in: fn() -> bool,
) -> Result<String, String> {
    if let Ok(owner) = std::env::var("DRAGENT_MCP_BOUND_USER") {
        if crate::organisation_context::cached_active().is_none_or(|active| {
            active.user_id != owner || active.context != dragent_paths::organisation_id()
        }) {
            return Err(
                "The desktop account or organisation changed; reconnect the SSH machine".into(),
            );
        }
    }
    if !signed_in() { return Err(crate::cli::signed_out()); }
    if crate::cli::team::tool_info(name).is_some() {
        crate::cli::team::call_tool(name, arguments)
    } else if crate::cli::app_tools::has(name) {
        crate::cli::app_tools::call(name, arguments)
    } else if crate::tasks::mcp::has(name) {
        crate::tasks::mcp::call(name, arguments)
    } else { tools::call(name, arguments, root) }
}

#[cfg(test)]
fn answer(message: &Value, root: &Path, signed_in: fn() -> bool) -> Option<Value> {
    dragent_host::mcp::answer(message, INSTRUCTIONS, &catalog(), |name, arguments| call(name, arguments, root, signed_in))
}

pub(crate) fn catalog() -> Vec<Value> {
    let services = TOOLS.iter().map(|tool| {
        json!({
            "name": tool.name,
            "description": tool.description,
            "inputSchema": tools::schema(tool.name),
            "annotations": {
                "readOnlyHint": !tool.writes,
                "destructiveHint": false,
                "openWorldHint": true,
            },
        })
    });
    services
        .chain(crate::tasks::mcp::catalog())
        .chain(dragent_host::mcp::coordination_tools())
        .chain(crate::cli::app_tools::catalog())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(message: Value) -> Option<Value> {
        answer(&message, Path::new("."), || true)
    }

    #[test]
    fn tool_calls_need_a_sign_in() {
        let signed_out = |message: Value| answer(&message, Path::new("."), || false).unwrap();
        // The agent can still start it and see its tools.
        let tools = signed_out(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
        assert!(!tools["result"]["tools"].as_array().unwrap().is_empty());
        for name in ["services", "search", "comment", "team_status", "task_done"] {
            let reply = signed_out(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": {"service": "linear", "id": "ENG-1", "text": "hi"}}}));
            assert_eq!(reply["result"]["isError"], true, "{name}");
            assert_eq!(reply["result"]["structuredContent"]["status"], "needs_auth");
            assert_eq!(reply["result"]["structuredContent"]["message"], crate::cli::SIGN_IN_FIRST);
        }
    }

    #[test]
    fn it_speaks_the_handshake_and_lists_its_tools() {
        let reply = ask(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})).unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(reply["result"]["serverInfo"]["name"], "dragent");
        let newer = ask(json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "2099-01-01"}})).unwrap();
        assert_eq!(newer["result"]["protocolVersion"], VERSIONS[0]);

        assert_eq!(
            ask(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
            None
        );

        let tools = ask(json!({"jsonrpc": "2.0", "id": "a", "method": "tools/list"})).unwrap();
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names[..8],
            [
                "services",
                "search",
                "read",
                "comment",
                "set_status",
                "create",
                "describe",
                "api"
            ]
        );
        assert!(names[8..].iter().all(|name| ["team_", "task_", "msg_"].iter().any(|prefix| name.starts_with(prefix))
            || crate::tasks::mcp::has(name)
            || crate::cli::app_tools::has(name)));
        assert!(names.contains(&"browser_snapshot") && names.contains(&"pane_open"));
        assert!(names.contains(&"team_wait") && names.contains(&"task_done"));
        assert_eq!(tools["id"], "a");
    }

    #[test]
    fn tool_errors_come_back_as_results_and_unknown_calls_as_errors() {
        let reply = ask(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "read", "arguments": {"service": "linear"}}})).unwrap();
        assert_eq!(reply["result"]["isError"], true);
        let failure = &reply["result"]["structuredContent"];
        assert_eq!(failure["status"], "invalid_input");
        assert_eq!(failure["code"], "missing_argument");
        assert_eq!(failure["message"], "missing required argument: id");
        assert_eq!(failure["retry_safe"], true);

        let unknown = ask(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "delete_everything"}})).unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
        let method =
            ask(json!({"jsonrpc": "2.0", "id": 5, "method": "sampling/createMessage"})).unwrap();
        assert_eq!(method["error"]["code"], -32601);
    }
}
