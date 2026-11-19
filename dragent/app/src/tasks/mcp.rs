//! The organisation's task list as MCP tools (`dragent mcp`): the same
//! operations as `dragent issues`, for agents that reach Dragent through the
//! MCP server. Reads never ask; the writing tools post under the user's
//! account, so they go through the same gates as the other writing service
//! tools (see `crate::integrations::policy` and the approval policy lists).

use dragent_tasks::{ListQuery, NewComment, NewLink};
use serde_json::{json, Value};

/// One tool: its name, what it does, its arguments as
/// `(name, kind, description, required)`, and whether it changes anything.
struct Tool {
    name: &'static str,
    description: &'static str,
    arguments: &'static [(&'static str, &'static str, &'static str, bool)],
    writes: bool,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "issues_list",
        description: "The organisation's task list: what needs doing, who has it, where each stands. Filter by status (open, closed, all or status keys), assignee (me, none or a member id), label, project or search words.",
        arguments: &[
            ("status", "string", "open (default), closed, all, or status keys joined by commas", false),
            ("assignee", "string", "me, none, or a member's user id", false),
            ("label", "string", "Only tasks carrying that label", false),
            ("project", "string", "Only tasks of that project", false),
            ("search", "string", "Words in the key, title or description", false),
        ],
        writes: false,
    },
    Tool {
        name: "issues_show",
        description: "One task in full: its description, comments and history, by its key (DRA-12) or id.",
        arguments: &[("key", "string", "The task's key or id", true)],
        writes: false,
    },
    Tool {
        name: "issues_create",
        description: "File a new task in the organisation's list, under the user's account. Only when the user asked for it or clearly expects it.",
        arguments: &[
            ("title", "string", "What needs doing", true),
            ("description", "string", "Markdown details", false),
            ("status", "string", "A status key (backlog, todo, in_progress, in_review, done, canceled)", false),
            ("priority", "string", "urgent, high, medium, low or none", false),
            ("assignee", "string", "A member's user id", false),
            ("project", "string", "A project key", false),
            ("labels", "array", "Labels (at most 20)", false),
        ],
        writes: true,
    },
    Tool {
        name: "issues_update",
        description: "Change a task: its title, status, priority, assignee, labels, description or project. Only what is given changes. Only when the user asked for it or clearly expects it.",
        arguments: &[
            ("key", "string", "The task's key or id", true),
            ("title", "string", "The new title", false),
            ("description", "string", "Markdown details", false),
            ("status", "string", "A status key", false),
            ("priority", "string", "urgent, high, medium, low or none", false),
            ("assignee", "string", "A member's user id", false),
            ("clear_assignee", "boolean", "Unassign the task", false),
            ("project", "string", "A project key", false),
            ("labels", "array", "Labels, replacing the task's", false),
        ],
        writes: true,
    },
    Tool {
        name: "issues_comment",
        description: "Comment on a task, under the user's account. Only when the user asked for it or clearly expects it, and keep it to what they'd sign.",
        arguments: &[
            ("key", "string", "The task's key or id", true),
            ("body", "string", "The comment, in Markdown", true),
        ],
        writes: true,
    },
    Tool {
        name: "issues_link",
        description: "Link a pull request (its address) or a tracker item (PROVIDER:KEY) to a task. Only when the user asked for it or clearly expects it.",
        arguments: &[
            ("key", "string", "The task's key or id", true),
            ("url", "string", "A pull request's address", false),
            ("item", "string", "A tracker item as PROVIDER:KEY (linear:ENG-12)", false),
            ("title", "string", "What the link reads as", false),
        ],
        writes: true,
    },
];

/// Whether `name` is one of these tools.
pub(crate) fn has(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

/// Whether `name` changes something in the organisation's list.
pub(crate) fn writes(name: &str) -> bool {
    TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .is_some_and(|tool| tool.writes)
}

/// The tools as MCP describes them.
pub(crate) fn catalog() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for (name, kind, description, needed) in tool.arguments {
                let schema = if *kind == "array" {
                    json!({ "type": kind, "items": { "type": "string" }, "description": description })
                } else {
                    json!({ "type": kind, "description": description })
                };
                properties.insert(name.to_string(), schema);
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
                    "destructiveHint": false,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

fn missing(name: &str) -> String {
    dragent_host::outcome::Failure::new(
        dragent_host::outcome::Status::InvalidInput,
        "missing_argument",
        format!("missing required argument: {name}"),
    )
    .encode()
}

fn text_argument<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args[name]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing(name))
}

fn optional(args: &Value, name: &str) -> Option<String> {
    args[name]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Run tool `name` with `args`. The answer is JSON text; failures are
/// [`dragent_host::outcome::Failure`]s. Blocking.
pub(crate) fn call(name: &str, arguments: &Value) -> Result<String, String> {
    let tool = TOOLS.iter().find(|tool| tool.name == name);
    let Some(tool) = tool else {
        return Err(format!("Unknown tool: {name}"));
    };
    if tool.writes {
        crate::integrations::policy::check_writes()?;
    }
    // MCP callers are agents acting for the person.
    let by_agent = true;
    let org = || super::client::active_org();
    let answer = match name {
        "issues_list" => {
            let org = org()?;
            let query = ListQuery {
                status: optional(arguments, "status"),
                assignee: optional(arguments, "assignee"),
                label: optional(arguments, "label"),
                project: optional(arguments, "project"),
                q: optional(arguments, "search"),
                ..Default::default()
            };
            let mut cursor: Option<String> = None;
            let mut tasks = Vec::new();
            let mut prefix = String::new();
            loop {
                let mut page = query.clone();
                page.cursor = cursor;
                let answer = super::client::list(&org, &page)?;
                if !answer.prefix.is_empty() {
                    prefix = answer.prefix;
                }
                tasks.extend(answer.tasks);
                cursor = answer.next_cursor.filter(|cursor| !cursor.is_empty());
                if cursor.is_none() {
                    break;
                }
            }
            json!({ "tasks": tasks, "prefix": prefix })
        }
        "issues_show" => {
            let key = text_argument(arguments, "key")?;
            json!(super::client::detail(&org()?, key)?)
        }
        "issues_create" => {
            let new = super::client::parse_new(arguments, by_agent)?;
            json!(super::client::create(&org()?, &new)?)
        }
        "issues_update" => {
            let key = text_argument(arguments, "key")?.to_string();
            let patch = super::client::parse_patch(arguments, by_agent)?;
            if patch.is_empty() && arguments["clear_assignee"].as_bool() != Some(true) {
                return Err(missing("at least one field to change"));
            }
            json!(super::client::patch(&org()?, &key, &patch)?)
        }
        "issues_comment" => {
            let key = text_argument(arguments, "key")?;
            let body = text_argument(arguments, "body")?;
            let comment = NewComment {
                body: body.to_string(),
                by_agent,
            };
            json!(super::client::add_comment(&org()?, key, &comment)?)
        }
        "issues_link" => {
            let key = text_argument(arguments, "key")?.to_string();
            let link = super::client::parse_link(arguments, by_agent)?;
            json!(super::client::add_link(&org()?, &key, &link)?)
        }
        _ => return Err(format!("Unknown tool: {name}")),
    };
    Ok(serde_json::to_string_pretty(&answer).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_described_with_its_arguments() {
        assert_eq!(TOOLS.len(), 6);
        let catalog = catalog();
        assert_eq!(catalog.len(), TOOLS.len());
        for tool in TOOLS.iter() {
            let described = catalog
                .iter()
                .find(|entry| entry["name"] == tool.name)
                .unwrap();
            assert_eq!(
                described["annotations"]["readOnlyHint"],
                json!(!tool.writes),
                "{}",
                tool.name
            );
            for required in described["inputSchema"]["required"].as_array().unwrap() {
                let field = required.as_str().unwrap();
                assert!(
                    described["inputSchema"]["properties"][field].is_object(),
                    "{} {field}",
                    tool.name
                );
            }
        }
        assert!(
            writes("issues_create")
                && writes("issues_update")
                && writes("issues_comment")
                && writes("issues_link")
        );
        assert!(!writes("issues_list") && !writes("issues_show") && !writes("nope"));
        assert!(has("issues_list") && !has("comment"));
    }

    #[test]
    fn calls_check_their_arguments_before_any_request() {
        for (name, args) in [
            ("issues_show", json!({})),
            ("issues_comment", json!({"key": "DRA-1"})),
            ("issues_link", json!({"key": "DRA-1"})),
            ("nope", json!({})),
        ] {
            assert!(call(name, &args).is_err(), "{name}");
        }
    }
}
