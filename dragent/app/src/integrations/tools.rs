//! Connected services as agent tools, one definition for every way agents
//! reach them: Dragent's own agent (prefixed `integrations_`), other
//! agents through the `dragent mcp` bridge, and `dragent integrations` in
//! a shell.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::items::{self, Draft, Item};
use super::{Provider, Status};

pub use dragent_host::service_mcp::{Tool, TOOLS, tool};
pub fn schema(name: &str) -> Value {
    let keys:Vec<_>=Provider::ALL.iter().filter(|provider|provider.offered()).map(|provider|provider.key()).collect();
    dragent_host::service_mcp::schema(name,&keys)
}


/// A service named by its key or its name, in any case.
pub fn provider(name: &str) -> Option<Provider> {
    let name = name.trim().to_lowercase();
    let name = match name.as_str() {
        "teams" | "msteams" | "ms_teams" => "microsoft_teams",
        "gitea" | "codeberg" => "forgejo",
        "monday.com" => "monday",
        other => other,
    };
    Provider::ALL
        .into_iter()
        .filter(|provider| provider.offered())
        .find(|provider| provider.key() == name || provider.label().to_lowercase() == name)
}

/// Every service, connected or not, with what can be done with it.
/// Blocking (GitHub asks the GitHub CLI).
pub fn services() -> Value {
    let rows: Vec<Value> = Provider::ALL
        .into_iter()
        .filter(|provider| provider.offered())
        .map(|provider| {
            let (connected, account, note) = match super::status(provider) {
                Status::Connected { account, .. } => (true, account, None),
                Status::NotConnected => (
                    false,
                    None,
                    Some("Not connected: the user can connect it in Dragent under Settings → Integrations.".to_string()),
                ),
                Status::Unavailable(reason) => (false, None, Some(reason)),
            };
            json!({
                "service": provider.key(),
                "name": provider.label(),
                "items": provider.item(),
                "connected": connected,
                "account": account,
                "can": items::abilities(provider),
                "note": note,
            })
        })
        .collect();
    Value::Array(rows)
}

/// An item for an agent. `full` keeps all of its text; a search result's
/// is cut to a preview.
pub fn item_json(item: &Item, full: bool) -> Value {
    let body = if full {
        item.body.clone()
    } else {
        preview(&item.body, 400)
    };
    json!({
        "service": item.provider.key(),
        "id": item.id(),
        "key": item.key,
        "title": item.title,
        "url": item.url,
        "detail": item.detail,
        "branch": item.branch,
        "text": body,
    })
}

fn preview(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

fn text_argument<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args[name]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            dragent_host::outcome::Failure::new(
                dragent_host::outcome::Status::InvalidInput,
                "missing_argument",
                format!("missing required argument: {name}"),
            )
            .encode()
        })
}

/// Whether this call changes something in the service. `api` calls are
/// writes unless the call only reads (see [`super::api::reads_only`]).
fn writes(name: &str, args: &Value, provider: Option<Provider>) -> bool {
    if name == "api" {
        return match (provider, super::api::Call::parse(args)) {
            (Some(provider), Ok(call)) => !super::api::reads_only(provider, &call),
            _ => false,
        };
    }
    tool(name).is_some_and(|tool| tool.writes)
}

fn failure(
    status: dragent_host::outcome::Status,
    code: &str,
    message: String,
    next: &str,
) -> String {
    dragent_host::outcome::Failure::new(status, code, message)
        .next(next)
        .encode()
}

/// The account the call will use, for results: no secrets.
fn using(provider: Provider) -> Value {
    if provider == Provider::Github {
        return json!({"selector": "github_cli"});
    }
    match super::connection(provider) {
        Some(connection) => {
            let selector = match connection.source {
                super::Source::Local | super::Source::Environment => "local",
                super::Source::Account => "personal",
                super::Source::GithubCli => "github_cli",
                super::Source::Team { .. } => "team",
            };
            json!({"selector": selector, "label": connection.account})
        }
        None => Value::Null,
    }
}

/// Fail early, and say what exists, when the named account is not one of
/// the provider's.
fn check_account(provider: Provider, selector: &str) -> Result<(), String> {
    let known = if provider == Provider::Github {
        matches!(selector, "github_cli" | "default")
    } else {
        super::connection_for(provider, selector).is_some()
    };
    if known {
        return Ok(());
    }
    let available: Vec<String> = if provider == Provider::Github {
        vec!["github_cli".into()]
    } else {
        super::accounts(provider).into_iter().map(|account| account.selector).collect()
    };
    Err(failure(
        dragent_host::outcome::Status::InvalidInput,
        "unknown_account",
        format!(
            "{} has no connected account “{selector}”. Available: {}.",
            provider.label(),
            if available.is_empty() { "none".to_string() } else { available.join(", ") }
        ),
        "Use a selector from `dragent integrations describe SERVICE` (accounts[].selector), or leave the account out for the default.",
    ))
}

/// Run tool `name` with `args`; `default_root` is the folder code hosts
/// use when the call names none. The answer is JSON text. Blocking.
/// Failures are [`dragent_host::outcome::Failure`]s: a rejected key, a
/// refused request and a write that got no answer each have their own code
/// (see [`super::errors`]).
pub fn call(name: &str, args: &Value, default_root: &Path) -> Result<String, String> {
    let provider_named = args["service"].as_str().and_then(provider);
    let write = writes(name, args, provider_named);
    run(name, args, default_root).map_err(|error| super::errors::classify(&error, write))
}

fn run(name: &str, args: &Value, default_root: &Path) -> Result<String, String> {
    if name == "services" {
        return Ok(pretty(&services()));
    }
    let root: PathBuf = args["project"]
        .as_str()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_root.to_path_buf());
    if name == "describe" {
        let only = match args["service"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            Some(service) => Some(unknown_service(service, provider(service))?),
            None => None,
        };
        return Ok(pretty(&super::describe::describe(only, &root)));
    }
    let service = text_argument(args, "service")?;
    let provider = unknown_service(service, provider(service))?;
    let account = args["account"].as_str().map(str::trim).filter(|a| !a.is_empty());
    super::with_account(account, || {
        if let Some(selector) = account {
            check_account(provider, selector)?;
        }
        dispatch(name, args, &root, provider)
    })
}

fn unknown_service(service: &str, found: Option<Provider>) -> Result<Provider, String> {
    found.ok_or_else(|| {
        failure(
            dragent_host::outcome::Status::InvalidInput,
            "unknown_service",
            format!("Unknown service “{service}”. Use the services tool to see them."),
            "Use a service key from `dragent integrations list`.",
        )
    })
}

fn dispatch(name: &str, args: &Value, root: &Path, provider: Provider) -> Result<String, String> {
    // Outside the app (the command line, the MCP bridge), a connection
    // kept on Dragent's server is the app's to use.
    let on_server = !super::remote::in_app()
        && super::connection(provider).is_some_and(|connection| connection.remote.is_some());
    if on_server {
        return through_the_app(name, args, root, provider);
    }
    let account = using(provider);
    let answer = match name {
        "search" => {
            let query = args["query"].as_str().unwrap_or_default();
            let found = items::search(provider, root, query)?;
            let rows: Vec<Value> = found.iter().map(|item| item_json(item, false)).collect();
            if rows.is_empty() {
                json!({"items": [], "count": 0, "truncated": false, "service": provider.key(), "account": account, "note": if items::needs_query(provider) { "Nothing found. This service only finds items for words: give a query." } else { "Nothing found." }})
            } else {
                let truncated = rows.len() >= items::LIMIT;
                let mut answer = json!({"items": rows, "count": rows.len(), "truncated": truncated, "service": provider.key(), "account": account});
                if truncated {
                    answer["next"] = json!(format!("This is a full page of {} with no further cursor. Narrow the query (state, label, assignee, date, project) and search again, and report that more may exist.", items::LIMIT));
                }
                answer
            }
        }
        "read" => {
            let item = items::get(provider, root, text_argument(args, "id")?)?;
            let mut answer = item_json(&item, true);
            answer["account"] = account;
            answer["truncated"] = json!(item.body.chars().count() >= items::BODY_CHARS);
            answer
        }
        "comment" => {
            let id = text_argument(args, "id")?;
            items::comment(provider, root, id, text_argument(args, "text")?)?;
            json!({"done": format!("Commented on {} {id}.", provider.label()), "service": provider.key(), "id": id, "account": account, "verify": format!("dragent integrations show {} {id}", provider.key())})
        }
        "set_status" => {
            let id = text_argument(args, "id")?;
            let status = items::set_status(provider, root, id, text_argument(args, "status")?)?;
            json!({"done": format!("{} {id} is now {status}.", provider.label()), "status": status, "service": provider.key(), "id": id, "account": account, "verify": format!("dragent integrations show {} {id}", provider.key())})
        }
        "create" => {
            let draft = Draft {
                title: text_argument(args, "title")?.to_string(),
                body: args["body"].as_str().unwrap_or_default().to_string(),
                place: args["place"].as_str().map(str::to_string),
            };
            let mut answer = item_json(&items::create(provider, root, &draft)?, true);
            answer["account"] = account;
            answer
        }
        "api" => {
            let call = super::api::Call::parse(args)?;
            let reads = super::api::reads_only(provider, &call);
            if !reads {
                super::policy::check_writes()?;
            }
            let connection = super::connection(provider);
            let mut answer = super::api::run(provider, connection.as_ref(), root, &call)?;
            answer["account"] = account;
            if !reads {
                answer["verify"] = json!("Read the changed resource back with a GET before repeating this call.");
            }
            answer
        }
        other => return Err(format!("Unknown tool: {other}")),
    };
    Ok(pretty(&answer))
}

/// What Dragent's own agent calls these tools.
pub const AGENT_PREFIX: &str = "integrations_";

/// The tools as Dragent's own agent is told about them: (name,
/// description, schema JSON). None while nothing but GitHub is connected
/// (it has its own tools), so they cost nothing then.
pub fn agent_tool_defs() -> Vec<(String, String, String)> {
    if !super::agents::any_connected() {
        return Vec::new();
    }
    TOOLS
        .iter()
        .map(|tool| {
            (
                format!("{AGENT_PREFIX}{}", tool.name),
                tool.description.to_string(),
                schema(tool.name).to_string(),
            )
        })
        .collect()
}

/// Whether Dragent's agent tool `name` posts to a service.
pub fn agent_tool_writes(name: &str) -> bool {
    name.strip_prefix(AGENT_PREFIX)
        .and_then(tool)
        .is_some_and(|tool| tool.writes)
}

/// Hand the call to the open app, which talks to Dragent's server. Whether
/// this process may post is decided here, where its automation is known.
fn through_the_app(
    name: &str,
    args: &Value,
    root: &Path,
    provider: Provider,
) -> Result<String, String> {
    if writes(name, args, Some(provider)) {
        super::policy::check_writes()?;
    }
    if !crate::cli::instance::app_running() {
        return Err(format!(
            "{} is connected through Dragent's server: open the Dragent app to use it here.",
            provider.label()
        ));
    }
    let response = crate::cli::ask_app(
        crate::cli::requests::Request::IntegrationTool {
            tool: name.to_string(),
            arguments: args.clone(),
            root: root.to_path_buf(),
        },
        false,
    )?;
    response
        .data
        .as_ref()
        .and_then(|data| data["output"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "Dragent didn't answer.".to_string())
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_schema_naming_what_it_needs() {
        for tool in &TOOLS {
            let schema = schema(tool.name);
            assert_eq!(schema["type"], "object", "{}", tool.name);
            for required in schema["required"].as_array().unwrap() {
                let field = required.as_str().unwrap();
                assert!(
                    schema["properties"][field].is_object(),
                    "{} {field}",
                    tool.name
                );
            }
        }
        assert!(tool("comment").unwrap().writes && !tool("read").unwrap().writes);
    }

    #[test]
    fn services_are_named_loosely() {
        assert_eq!(provider("Linear"), Some(Provider::Linear));
        assert_eq!(provider("monday.com"), Some(Provider::Monday));
        // Shown only once this build can sign in to it.
        let teams = Provider::Teams.offered().then_some(Provider::Teams);
        assert_eq!(provider("teams"), teams);
        assert_eq!(provider("Microsoft Teams"), teams);
        assert_eq!(provider("gitea"), Some(Provider::Forgejo));
        assert_eq!(provider("bitbucket"), None);
    }

    #[test]
    fn calls_check_their_arguments_before_any_request() {
        let root = Path::new(".");
        let missing = dragent_host::outcome::Failure::from_error(
            &call("read", &json!({"service": "linear"}), root).unwrap_err(),
        );
        assert_eq!(missing.status, dragent_host::outcome::Status::InvalidInput);
        assert_eq!(missing.message, "missing required argument: id");
        assert!(
            call("read", &json!({"service": "bitbucket", "id": "1"}), root)
                .unwrap_err()
                .contains("Unknown service")
        );
        assert!(
            call("create", &json!({"service": "notion", "title": "x"}), root)
                .unwrap_err()
                .contains("can't create")
        );
    }

    #[test]
    fn a_named_account_that_does_not_exist_is_refused_before_any_request() {
        let failure = dragent_host::outcome::Failure::from_error(
            &call("search", &json!({"service": "linear", "account": "team"}), Path::new(".")).unwrap_err(),
        );
        assert_eq!(failure.status, dragent_host::outcome::Status::InvalidInput);
        assert_eq!(failure.code, "unknown_account");
        assert!(failure.message.contains("Available: none"));
        let github = dragent_host::outcome::Failure::from_error(
            &call("read", &json!({"service": "github", "id": "#1", "account": "team"}), Path::new(".")).unwrap_err(),
        );
        assert_eq!(github.code, "unknown_account");
        assert!(github.message.contains("github_cli"));
    }

    #[test]
    fn every_service_tool_takes_an_account_and_describe_is_read_only() {
        for name in ["search", "read", "comment", "set_status", "create", "api"] {
            assert!(schema(name)["properties"]["account"].is_object(), "{name}");
        }
        assert!(!tool("describe").unwrap().writes && tool("api").unwrap().writes);
        let unknown = dragent_host::outcome::Failure::from_error(
            &call("describe", &json!({"service": "bitbucket"}), Path::new(".")).unwrap_err(),
        );
        assert_eq!(unknown.code, "unknown_service");
        let answer: Value = serde_json::from_str(&call("describe", &json!({"service": "Linear"}), Path::new(".")).unwrap()).unwrap();
        assert_eq!(answer["services"][0]["service"], "linear");
        assert!(answer["failures"].as_array().unwrap().len() > 10);
    }

    #[test]
    fn api_calls_are_checked_and_gated_like_the_other_tools() {
        let bad = dragent_host::outcome::Failure::from_error(
            &call("api", &json!({"service": "gitlab", "path": "https://evil.test/x"}), Path::new(".")).unwrap_err(),
        );
        assert_eq!(bad.code, "api_path_invalid");
        let get = json!({"service": "gitlab", "path": "/user"});
        assert!(!writes("api", &get, provider("gitlab")));
        let post = json!({"service": "gitlab", "path": "/issues", "method": "POST"});
        assert!(writes("api", &post, provider("gitlab")));
        assert!(writes("comment", &json!({}), None) && !writes("search", &json!({}), None));
        // Not connected: said so, with the code that asks the person to connect it.
        let none = dragent_host::outcome::Failure::from_error(&call("api", &get, Path::new(".")).unwrap_err());
        assert_eq!(none.code, "not_connected");
    }
}
