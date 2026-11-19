//! Service tool definitions shared by both MCP installations. Credentials
//! and network calls belong to the signed-in desktop, never this catalog.
use serde_json::{Value, json};
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// Changes something in a service, under the user's account.
    pub writes: bool,
}

pub const TOOLS: [Tool; 8] = [
    Tool {
        name: "services",
        description: "List the services connected in Dragent (Linear, Jira, GitHub, Sentry, Slack, Notion, …) and what can be done with each: read, comment, change status, create. Check this before the other tools.",
        writes: false,
    },
    Tool {
        name: "search",
        description: "Find items in a connected service: issues, tasks, cards, errors, support threads, messages, pages. Without a query, lists recent, open or assigned ones where the service can. Each result has the `id` the other tools take.",
        writes: false,
    },
    Tool {
        name: "read",
        description: "Read one item in full: its description and comments (for Sentry, the latest stack trace; for Slack, the whole thread; for Notion, the page).",
        writes: false,
    },
    Tool {
        name: "comment",
        description: "Add a comment to an item. It is posted under the user's own account, where their team sees it: only comment when the user asked for it or clearly expects it, and keep it to what they'd sign.",
        writes: true,
    },
    Tool {
        name: "set_status",
        description: "Move an item to another status: Done, In Progress, closed, resolved, a list on a board. The service's own names are matched loosely; when nothing matches, the error lists them. Only when the user asked for it or clearly expects it.",
        writes: true,
    },
    Tool {
        name: "create",
        description: "File a new item (issue, task, work item) in a service, under the user's account. Only when the user asked for it. `place` says where: a Linear team key, a Jira or YouTrack project key, a GitLab project path, an owner/repo.",
        writes: true,
    },
    Tool {
        name: "describe",
        description: "Start here when unsure what a service offers: its connected accounts (which is the default), operations with input schemas, read or write access, identifier formats, scope, paging and the failure codes. With `service`, full detail for that one; without, a compact overview of all. Never returns credentials.",
        writes: false,
    },
    Tool {
        name: "api",
        description: "Call a connected service's own API for what the other tools don't cover (boards, labels, pull requests, releases, extra endpoints, paging by the service's cursor). `path` is relative to the API root (see `describe`); the key is added by Dragent. GET and read-only queries just read; any other call is posted under the user's account: only when they asked for it.",
        writes: true,
    },
];

pub fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// The JSON Schema of `name`'s arguments.
pub fn schema(name: &str, keys: &[&str]) -> Value {
    let mut service = json!({
        "type": "string",
        "enum": keys,
        "description": "The service, by its key (see the services tool)."
    });
    if keys.is_empty() {
        service.as_object_mut().unwrap().remove("enum");
    }
    let id = json!({"type": "string", "description": "The item's id, as search and read give it (ENG-12, #42, 123:45, a card's short link, …)."});
    let project = json!({"type": "string", "description": "For GitHub, GitLab and Forgejo: the project folder whose repository to use. Defaults to the current folder."});
    let account = json!({"type": "string", "description": "Which connected account to use: a selector from `describe` (personal, team, local). Leave out for the default; name one only when asked or when the default is refused."});
    let object = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required});
    match name {
        "services" => object(json!({}), &[]),
        "search" => object(
            json!({"service": service, "query": {"type": "string", "description": "Words to search for, or the service's own query syntax. Leave out to list recent or assigned items."}, "project": project, "account": account}),
            &["service"],
        ),
        "read" => object(
            json!({"service": service, "id": id, "project": project, "account": account}),
            &["service", "id"],
        ),
        "comment" => object(
            json!({"service": service, "id": id, "text": {"type": "string", "description": "The comment, in Markdown where the service supports it."}, "project": project, "account": account}),
            &["service", "id", "text"],
        ),
        "set_status" => object(
            json!({"service": service, "id": id, "status": {"type": "string", "description": "The status to move it to, as the service names it (Done, In Progress, closed, resolved…)."}, "project": project, "account": account}),
            &["service", "id", "status"],
        ),
        "create" => object(
            json!({"service": service, "title": {"type": "string"}, "body": {"type": "string", "description": "The description, in Markdown where the service supports it."}, "place": {"type": "string", "description": "Where it goes: a Linear team key, a Jira or YouTrack project key, a Plane project identifier, a GitLab project path, an owner/repo, an Asana project gid. Leave out when there's only one, or for the project's own repository."}, "project": project, "account": account}),
            &["service", "title"],
        ),
        "describe" => object(
            json!({"service": {"type": "string", "description": "One service's key for full detail; leave out for an overview of all."}, "project": project}),
            &[],
        ),
        "api" => object(
            json!({"service": service, "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"], "description": "Defaults to GET. GraphQL services take POST."}, "path": {"type": "string", "description": "Relative to the service's API root, as `describe` shows (api.example). GraphQL services: /graphql."}, "query": {"type": "object", "description": "Query parameters, e.g. page, per_page, cursor.", "additionalProperties": {"type": ["string", "number", "boolean"]}}, "body": {"description": "JSON body; GraphQL: {\"query\": \"…\", \"variables\": {}}."}, "project": project, "account": account}),
            &["service", "path"],
        ),
        _ => object(json!({}), &[]),
    }
}
pub fn catalog(keys: &[&str]) -> Vec<Value> {
    TOOLS.iter().map(|tool|json!({"name":tool.name,"description":tool.description,"inputSchema":schema(tool.name,keys),"annotations":{"readOnlyHint":!tool.writes,"destructiveHint":false,"openWorldHint":true}})).collect()
}
