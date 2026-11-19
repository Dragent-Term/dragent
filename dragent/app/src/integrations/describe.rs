//! What an agent can do with each connected service, as structured data:
//! the accounts it may use, the operations with their input schemas,
//! identifiers, scope and paging. No credentials: only labels and the
//! values a connection holds that aren't secret (an address, a workspace).

use std::path::Path;

use serde_json::{json, Value};

use super::api::{self, Kind};
use super::items::{abilities, needs_query, LIMIT};
use super::{Auth, Connection, Provider};

/// How a provider names its items and what limits where they are found.
struct Facts {
    /// Examples of an item's `id`, as `search` and `read` print them.
    ids: &'static str,
    /// What the account can reach, and what narrows it.
    scope: &'static str,
    /// `place` for `create`.
    place: &'static str,
    /// Where `url` comes from, when the service gives one.
    links: &'static str,
}

fn facts(provider: Provider) -> Facts {
    let f = |ids, scope, place, links| Facts {
        ids,
        scope,
        place,
        links,
    };
    match provider {
        Provider::Github => f("#12, or the issue's web address", "The repository whose folder --project names (default: the current folder), through the GitHub CLI's own sign-in.", "owner/repo (default: the folder's repository)", "url is the issue's web address"),
        Provider::Gitlab => f("123:45 (project id and issue number), group/app#45, or #45 in the folder's project", "The GitLab server of the connection; --project picks the project whose remote is used.", "a project path such as group/app (default: the folder's project)", "url is the issue's web address"),
        Provider::Forgejo => f("owner/repo#12", "The Forgejo or Gitea server of the connection; --project picks the repository.", "owner/repo", "url is the issue's web address"),
        Provider::Linear => f("ENG-12", "The Linear workspace the key belongs to; issues of every team it can see.", "a team key such as ENG", "url is the issue's web address"),
        Provider::Jira => f("PROJ-12", "The Atlassian site of the connection; the projects the account can browse.", "a project key such as PROJ", "url is the issue's web address"),
        Provider::Plane => f("project/item (as search prints it)", "The Plane workspace of the connection (see scope.config.workspace).", "a project identifier", "url is the work item's web address"),
        Provider::Youtrack => f("PROJ-12", "The YouTrack server of the connection; the projects the token can read.", "a project key", "url is the issue's web address"),
        Provider::Asana => f("the task's gid, or its web address", "The workspaces the personal access token can see.", "a project gid", "url is the task's web address"),
        Provider::Trello => f("the card's short link or id, or its web address", "The boards the token's member belongs to.", "", "url is the card's short link address"),
        Provider::Monday => f("the item's id, or its web address", "The boards the token's user can read.", "", "url is the item's web address"),
        Provider::Notion => f("the page id, or its web address", "Only pages and databases shared with the integration.", "", "url is the page's web address"),
        Provider::Featurebase => f("the post id as search prints it", "The Featurebase organisation the API key belongs to.", "", "url is the post's public address when the service gives one"),
        Provider::Plain => f("th_… thread id", "The Plain workspace the API key belongs to.", "", "url is the thread's address in the Plain app"),
        Provider::Sentry => f("BACKEND-3K (short id) or the numeric issue id", "The organisation in scope.config.organization on the connection's server; projects the token can read.", "", "url is the issue's web address"),
        Provider::Slack => f("channel_id:thread_ts as search prints it", "Messages the user token can search; channels it is a member of or that are public.", "", "url is the message's permalink"),
        Provider::Teams => f("", "", "", ""),
        Provider::Gmail => f("the message id as search prints it", "The signed-in Google account's mailbox. Read only.", "", "url may be empty for mail"),
    }
}

fn category(provider: Provider) -> &'static str {
    provider.category().label()
}

/// A connection's non-secret values (an address, a workspace).
fn config(provider: Provider, connection: Option<&Connection>) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(connection) = connection {
        for field in provider
            .fields()
            .iter()
            .filter(|field| !field.secret && field.id != "email")
        {
            let value = match field.id {
                "url" => connection.base_url(),
                id => connection.credential(id).unwrap_or_default().to_string(),
            };
            if !value.is_empty() {
                out.insert(field.id.to_string(), json!(value));
            }
        }
    }
    Value::Object(out)
}

fn operations(provider: Provider, detail: bool) -> Vec<Value> {
    let can = abilities(provider);
    let keys = [provider.key()];
    let mut list = vec![("search", true, false)];
    list.push(("read", can.read, false));
    list.push(("comment", can.comment, true));
    list.push(("set_status", can.status, true));
    list.push(("create", can.create, true));
    list.push(("api", api::target(provider).is_some(), true));
    list.into_iter()
        .map(|(name, available, writes)| {
            if !detail {
                return json!(name);
            }
            let tool = dragent_host::service_mcp::tool(name);
            let mut schema = dragent_host::service_mcp::schema(name, &keys);
            if name == "search" && needs_query(provider) {
                schema["required"] = json!(["service", "query"]);
            }
            let cli = match name {
                "search" => format!("dragent integrations search {} [WORDS] --json", provider.key()),
                "read" => format!("dragent integrations show {} ID --json", provider.key()),
                "comment" => format!("dragent integrations comment {} ID --body-file FILE --json", provider.key()),
                "set_status" => format!("dragent integrations status {} ID STATUS --json", provider.key()),
                "create" => format!("dragent integrations create {} --title TEXT [--body-file FILE] [--in PLACE] --json", provider.key()),
                _ => format!("dragent integrations api {} METHOD PATH [--query k=v]... [--body-file FILE] --json", provider.key()),
            };
            let access = match name {
                "api" => "read for GET (and read-only queries), write otherwise",
                _ if writes => "write",
                _ => "read",
            };
            json!({
                "name": name,
                "mcp_tool": name,
                "agent_tool": format!("{}{name}", super::tools::AGENT_PREFIX),
                "cli": cli,
                "access": access,
                "available": available,
                "description": tool.map(|t| t.description).unwrap_or_default(),
                "input": schema,
            })
        })
        .collect()
}

fn pagination(provider: Provider) -> Value {
    let api = api::target(provider).map(|t| t.pagination);
    json!({
        "search": {
            "style": "none",
            "page_size": LIMIT,
            "truncated_when": format!("items.len() == {LIMIT}: the result sets \"truncated\": true"),
            "how": "No cursor: narrow the query (labels, state, assignee, dates, project) and search again; say how many pages were read and what was left out.",
        },
        "read": "One item per call: comments are included up to a size limit; the result sets \"truncated\" when the text was cut.",
        "api": api,
    })
}

fn credential_state(provider: Provider, accounts: &[super::Account]) -> Value {
    match provider.unavailable() {
        Some(reason) => json!({"state": "unavailable_in_this_build", "reason": reason}),
        None if accounts.is_empty() => {
            let how = match provider.auth() {
                Auth::GithubCli => "Run gh auth login in a terminal.",
                _ => "The person connects it in Settings → Integrations; agents never handle the key.",
            };
            json!({"state": "not_connected", "how": how})
        }
        None => json!({"state": "connected"}),
    }
}

/// One service. `detail` adds the input schemas and the API entry.
pub fn service(provider: Provider, root: &Path, detail: bool) -> Value {
    let accounts = super::accounts(provider);
    let default = super::connection(provider);
    let facts = facts(provider);
    let mut out = json!({
        "service": provider.key(),
        "name": provider.label(),
        "category": category(provider),
        "items": provider.item(),
        "connected": !accounts.is_empty(),
        "auth": credential_state(provider, &accounts),
        "accounts": accounts,
        "operations": operations(provider, detail),
        "scope": {
            "describes": facts.scope,
            "config": config(provider, default.as_ref()),
            "uses_project_folder": matches!(provider, Provider::Github | Provider::Gitlab | Provider::Forgejo),
            "project_folder": root.to_string_lossy(),
        },
        "identifiers": {"item_id": facts.ids, "use": "Pass the `id` field of a search or read result to read, comment and set_status; `key` is only for people."},
    });
    if detail {
        out["create_place"] = json!(facts.place);
        out["links"] = json!(facts.links);
        out["pagination"] = pagination(provider);
        out["needs_query"] = json!(needs_query(provider));
        if let Some(target) = api::target(provider) {
            out["api"] = json!({
                "kind": match target.kind { Kind::Rest => "rest", Kind::GraphQl => "graphql", Kind::GithubCli => "github_cli" },
                "root": target.base,
                "example": target.example,
                "body": if target.kind == Kind::GraphQl { json!("{\"query\": \"query { … }\", \"variables\": {}}") } else { Value::Null },
                "reads": "GET, and queries that don't mutate; they need no write permission",
                "writes": "any other call: posted under the account, refused in automation runs that may only read",
                "pagination": target.pagination,
            });
        }
    }
    out
}

/// All services (compact), or one in detail.
pub fn describe(only: Option<Provider>, root: &Path) -> Value {
    let providers: Vec<Provider> = Provider::ALL.into_iter().filter(|p| p.offered()).collect();
    let mut out = json!({
        "services": match only {
            Some(provider) => vec![service(provider, root, true)],
            None => providers.iter().map(|p| service(*p, root, false)).collect(),
        },
        "account_selection": {
            "argument": "account (MCP and agent tools) / --account (CLI)",
            "values": "an accounts[].selector of the service: personal, team, local",
            "default": "the account marked default: used when none is named, the same in every entry point",
            "note": "Choosing an account changes whose permissions and whose name the call uses. Name one only when the person asked, or when a default fails with insufficient_permission.",
        },
        "result": {
            "item": ["service", "id", "key", "title", "url", "detail", "text"],
            "search": ["items", "count", "truncated", "service", "account"],
            "write": ["done", "service", "id", "account", "verify"],
        },
        "failures": super::errors::codes_json(),
    });
    if only.is_none() {
        out["next"] = json!("`dragent integrations describe SERVICE --json` adds input schemas, API access, paging and identifiers for one service.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_lists_operations_with_schemas_and_access() {
        let value = service(Provider::Linear, Path::new("/repo"), true);
        assert_eq!(value["service"], "linear");
        let ops = value["operations"].as_array().unwrap();
        let names: Vec<&str> = ops.iter().map(|o| o["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["search", "read", "comment", "set_status", "create", "api"]
        );
        let comment = &ops[2];
        assert_eq!(comment["access"], "write");
        assert_eq!(
            comment["input"]["required"],
            json!(["service", "id", "text"])
        );
        assert!(comment["input"]["properties"]["account"].is_object());
        assert_eq!(ops[0]["access"], "read");
        assert_eq!(value["api"]["kind"], "graphql");
        assert_eq!(value["pagination"]["search"]["page_size"], LIMIT);
        assert!(value["identifiers"]["item_id"]
            .as_str()
            .unwrap()
            .contains("ENG-12"));
    }

    #[test]
    fn what_a_service_cannot_do_is_marked_unavailable() {
        let slack = service(Provider::Slack, Path::new("."), true);
        let ops = slack["operations"].as_array().unwrap();
        let create = ops.iter().find(|o| o["name"] == "create").unwrap();
        assert_eq!(create["available"], false);
        let search = ops.iter().find(|o| o["name"] == "search").unwrap();
        assert_eq!(search["input"]["required"], json!(["service", "query"]));
        assert_eq!(slack["needs_query"], true);
        let gmail = service(Provider::Gmail, Path::new("."), true);
        assert!(gmail.get("api").is_none());
    }

    #[test]
    fn the_overview_is_compact_and_carries_the_failure_codes() {
        let value = describe(None, Path::new("."));
        let first = &value["services"][0];
        assert!(first["operations"][0].is_string());
        assert!(first.get("pagination").is_none());
        let codes: Vec<&str> = value["failures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["code"].as_str().unwrap())
            .collect();
        assert!(
            codes.contains(&"credentials_expired") && codes.contains(&"insufficient_permission")
        );
        let text = value.to_string();
        for secret_word in ["token\":\"", "api_key\":\"", "password"] {
            assert!(!text.contains(secret_word), "{secret_word}");
        }
    }

    #[test]
    fn every_offered_service_has_identifier_and_scope_facts() {
        for provider in Provider::ALL.into_iter().filter(|p| p.offered()) {
            let f = facts(provider);
            assert!(!f.ids.is_empty() && !f.scope.is_empty(), "{provider:?}");
        }
    }
}
