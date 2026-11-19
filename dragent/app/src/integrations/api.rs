//! A connected service's own API, for what the six item tools don't cover
//! (boards, labels, pull requests, releases, any extra endpoint the
//! service offers). The call goes through the same connection and account
//! as the other tools, so the key is added by Dragent and never seen here;
//! the path is relative to the service's API root, so the key can only ever
//! go to the service.

use std::path::Path;

use reqwest::Method;
use serde_json::{json, Value};

use dragent_host::outcome::{Failure, Status};

use super::{Connection, Provider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Paths under a base address.
    Rest,
    /// One endpoint taking `{"query": …, "variables": …}`.
    GraphQl,
    /// Through the GitHub CLI's own sign-in.
    GithubCli,
}

/// What `api` can reach for a provider, and how it pages.
pub struct Target {
    pub kind: Kind,
    /// The API root, `{server}` standing for a self-hosted address.
    pub base: &'static str,
    pub example: &'static str,
    pub pagination: &'static str,
}

pub fn target(provider: Provider) -> Option<Target> {
    let rest = |base, example, pagination| Target {
        kind: Kind::Rest,
        base,
        example,
        pagination,
    };
    let graph = |base, example, pagination| Target {
        kind: Kind::GraphQl,
        base,
        example,
        pagination,
    };
    Some(match provider {
        Provider::Github => Target { kind: Kind::GithubCli, base: "https://api.github.com", example: "/repos/{owner}/{repo}/pulls", pagination: "query page and per_page (max 100); an empty array is the end" },
        Provider::Gitlab => rest("{server}/api/v4", "/user", "query page and per_page (max 100); an empty array is the end"),
        Provider::Forgejo => rest("{server}/api/v1", "/user", "query page and limit; an empty array is the end"),
        Provider::Linear => graph("https://api.linear.app/graphql", "/graphql", "first and after in the query, with pageInfo { hasNextPage endCursor }"),
        Provider::Jira => rest("{server}", "/rest/api/3/myself", "search/jql: nextPageToken; other lists: startAt and maxResults until isLast"),
        Provider::Plane => rest("{server}/api/v1", "/workspaces/{workspace}/projects/", "query per_page and cursor, from next_cursor while next_page_results is true"),
        Provider::Youtrack => rest("{server}/api", "/users/me", "query $skip and $top; name the fields wanted with fields"),
        Provider::Asana => rest("https://app.asana.com/api/1.0", "/users/me", "query limit and offset, from next_page.offset"),
        Provider::Trello => rest("https://api.trello.com/1", "/members/me/boards", "most lists are unpaged; use limit, before and since where a list takes them"),
        Provider::Monday => graph("https://api.monday.com/v2", "/graphql", "limit and page, or items_page with cursor"),
        Provider::Notion => rest("https://api.notion.com/v1", "/users/me", "start_cursor from next_cursor while has_more is true"),
        Provider::Featurebase => rest("https://do.featurebase.app/v2", "/posts", "query page and limit"),
        Provider::Plain => graph("https://core-api.uk.plain.com/graphql/v1", "/graphql", "first and after in the query, with pageInfo { hasNextPage endCursor }"),
        Provider::Sentry => rest("{server}/api/0", "/organizations/{organization}/issues/", "the next cursor travels in a Link header this path does not return: narrow with query, project and statsPeriod instead"),
        Provider::Slack => rest("https://slack.com/api", "/auth.test", "query cursor, from response_metadata.next_cursor"),
        Provider::Teams | Provider::Gmail => return None,
    })
}

/// The API root for `connection` (a self-hosted address filled in).
fn base(target: &Target, connection: Option<&Connection>) -> String {
    match connection {
        Some(connection) => target.base.replace("{server}", &connection.base_url()),
        None => target.base.to_string(),
    }
}

/// A call parsed from tool arguments.
#[derive(Debug, PartialEq)]
pub struct Call {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
}

impl Call {
    pub fn parse(args: &Value) -> Result<Call, String> {
        let invalid = |code: &str, message: String| {
            Failure::new(Status::InvalidInput, code, message).encode()
        };
        let method = args["method"]
            .as_str()
            .unwrap_or("GET")
            .trim()
            .to_uppercase();
        let method = match method.as_str() {
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" => {
                Method::from_bytes(method.as_bytes()).unwrap_or(Method::GET)
            }
            other => {
                return Err(invalid(
                    "invalid_method",
                    format!("method must be GET, POST, PUT, PATCH or DELETE, not “{other}”"),
                ))
            }
        };
        let path = args["path"]
            .as_str()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| invalid("missing_argument", "missing required argument: path".into()))?;
        let (path, inline) = path
            .split_once('?')
            .map_or((path, None), |(p, q)| (p, Some(q)));
        let bad = !path.starts_with('/')
            || path.starts_with("//")
            || path.contains("://")
            || path.contains('\\')
            || path.contains('@')
            || path.contains('#')
            || path.split('/').any(|part| part == "..");
        if bad {
            return Err(Failure::new(Status::InvalidInput, "api_path_invalid", format!("“{path}” is not a path under the service's API root"))
                .next("Use a path like /user (see api.example in `dragent integrations describe SERVICE`), never a full URL.")
                .encode());
        }
        let mut query: Vec<(String, String)> = inline
            .into_iter()
            .flat_map(|inline| inline.split('&'))
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                pair.split_once('=')
                    .map_or((pair.to_string(), String::new()), |(k, v)| {
                        (k.to_string(), v.to_string())
                    })
            })
            .collect();
        match &args["query"] {
            Value::Object(map) => {
                for (key, value) in map {
                    let value =
                        match value {
                            Value::String(text) => text.clone(),
                            Value::Number(number) => number.to_string(),
                            Value::Bool(flag) => flag.to_string(),
                            Value::Null => continue,
                            _ => return Err(invalid(
                                "invalid_query",
                                format!(
                                    "query value for “{key}” must be text, a number or true/false"
                                ),
                            )),
                        };
                    query.push((key.clone(), value));
                }
            }
            Value::Null => {}
            _ => {
                return Err(invalid(
                    "invalid_query",
                    "query must be an object of names and values".into(),
                ))
            }
        }
        let body = match &args["body"] {
            Value::Null => None,
            Value::String(text) if text.trim().is_empty() => None,
            Value::String(text) => Some(serde_json::from_str(text).map_err(|error| {
                invalid("invalid_body", format!("body is not valid JSON: {error}"))
            })?),
            other => Some(other.clone()),
        };
        Ok(Call {
            method,
            path: path.to_string(),
            query,
            body,
        })
    }
}

/// Whether `call` only reads. GraphQL services take every call as a POST,
/// so their documents are looked at; Slack and Notion read with POST or
/// GET methods whose names say so; everywhere else the HTTP method decides.
pub fn reads_only(provider: Provider, call: &Call) -> bool {
    let Some(target) = target(provider) else {
        return false;
    };
    match target.kind {
        Kind::GraphQl => {
            let document = call
                .body
                .as_ref()
                .and_then(|b| b["query"].as_str())
                .unwrap_or("")
                .to_lowercase();
            let words = |needle: &str| {
                document
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|word| word == needle)
            };
            call.method == Method::POST
                && !document.is_empty()
                && !words("mutation")
                && !words("subscription")
        }
        _ if provider == Provider::Slack => {
            let method = call.path.trim_start_matches('/');
            [
                "auth.test",
                "team.info",
                "users.info",
                "users.list",
                "search.",
                "conversations.history",
                "conversations.replies",
                "conversations.info",
                "conversations.list",
                "reactions.get",
            ]
            .iter()
            .any(|read| method == *read || (read.ends_with('.') && method.starts_with(read)))
        }
        _ if provider == Provider::Notion && call.method == Method::POST => {
            call.path == "/search"
                || (call.path.starts_with("/databases/") && call.path.ends_with("/query"))
        }
        _ => call.method == Method::GET,
    }
}

/// How large an answer travels back before it is cut.
const MAX_BYTES: usize = 200_000;

/// Run `call` against `provider` through `connection` (none for GitHub).
pub fn run(
    provider: Provider,
    connection: Option<&Connection>,
    root: &Path,
    call: &Call,
) -> Result<Value, String> {
    let target = target(provider).ok_or_else(|| {
        Failure::new(
            Status::Unsupported,
            "not_supported",
            format!("Dragent has no API access for {} yet.", provider.label()),
        )
        .encode()
    })?;
    let data = match target.kind {
        Kind::GithubCli => github(root, call)?,
        _ => {
            let connection = connection.ok_or_else(|| {
                format!(
                    "{} isn't connected. Connect it in Dragent under Settings → Integrations.",
                    provider.label()
                )
            })?;
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("Dragent/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|error| format!("Couldn't start a connection: {error}"))?;
            let url = if target.kind == Kind::GraphQl {
                base(&target, Some(connection))
            } else {
                format!("{}{}", base(&target, Some(connection)), call.path)
            };
            let mut request = client.request(call.method.clone(), url);
            if !call.query.is_empty() {
                request = request.query(&call.query);
            }
            if provider == Provider::Notion {
                request = request.header("Notion-Version", "2022-06-28");
            }
            if provider == Provider::Monday {
                request = request.header("API-Version", "2024-10");
            }
            if let Some(body) = &call.body {
                request = request.json(body);
            }
            let data = connection.send(request)?;
            slack_ok(provider, &data)?;
            data
        }
    };
    let text = data.to_string();
    let mut answer = json!({
        "service": provider.key(),
        "method": call.method.as_str(),
        "path": call.path,
        "read_only": reads_only(provider, call),
        "pagination": target.pagination,
    });
    if text.len() > MAX_BYTES {
        let cut: String = text.chars().take(MAX_BYTES).collect();
        answer["status"] = json!("partial");
        answer["truncated"] = json!(true);
        answer["preview"] = json!(cut);
        answer["next"] = json!("The answer was too large. Ask for less: a smaller page size, fewer fields, or a narrower filter.");
    } else {
        answer["data"] = data;
    }
    Ok(answer)
}

/// Slack answers 200 with `ok: false`; say which kind of refusal it was.
fn slack_ok(provider: Provider, data: &Value) -> Result<(), String> {
    if provider != Provider::Slack || data["ok"].as_bool() != Some(false) {
        return Ok(());
    }
    let error = data["error"].as_str().unwrap_or("unknown_error");
    Err(match error {
        "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive" => super::expired_message("Slack", false),
        "missing_scope" => format!(
            "Slack {} because the token is missing a scope ({}). Add it to the app, reinstall it, then reconnect with the new token.",
            super::REFUSED_REQUEST,
            data["needed"].as_str().unwrap_or("unknown")
        ),
        "not_in_channel" | "channel_not_found" | "access_denied" => format!("Slack {}: {error} (the account cannot see this channel). Reconnecting won't fix that.", super::REFUSED_REQUEST),
        other => format!("Slack: {other}"),
    })
}

fn github(root: &Path, call: &Call) -> Result<Value, String> {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut url = reqwest::Url::parse("https://github.invalid").map_err(|e| e.to_string())?;
    url.set_path(&call.path);
    if !call.query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(call.query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let mut command = crate::process_ext::gh_command();
    command.args([
        "api",
        "-X",
        call.method.as_str(),
        path.trim_start_matches('/'),
    ]);
    if call.body.is_some() {
        command.args(["--input", "-"]);
    }
    let mut child = crate::process_ext::NoWindow::no_window(
        command.current_dir(root).env("GH_PROMPT_DISABLED", "1"),
    )
    .stdin(if call.body.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "Install the GitHub CLI (gh) to use GitHub.".to_string(),
        _ => format!("Couldn't run the GitHub CLI: {error}"),
    })?;
    if let (Some(body), Some(mut stdin)) = (&call.body, child.stdin.take()) {
        stdin
            .write_all(body.to_string().as_bytes())
            .map_err(|error| format!("Couldn't hand the text to the GitHub CLI: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("The GitHub CLI didn't finish: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "The GitHub CLI said: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.trim().to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(provider: Provider, args: Value) -> (Call, bool) {
        let call = Call::parse(&args).unwrap();
        let read = reads_only(provider, &call);
        (call, read)
    }

    #[test]
    fn paths_stay_under_the_services_root() {
        for bad in [
            "user",
            "//evil.test/x",
            "https://evil.test/",
            "/a/../b",
            "/x@evil.test",
            "/a\\b",
            "/a#b",
        ] {
            let error = Call::parse(&json!({"path": bad})).unwrap_err();
            assert_eq!(
                Failure::from_error(&error).code,
                "api_path_invalid",
                "{bad}"
            );
        }
        assert_eq!(
            Failure::from_error(&Call::parse(&json!({})).unwrap_err()).code,
            "missing_argument"
        );
        assert_eq!(
            Failure::from_error(
                &Call::parse(&json!({"path": "/x", "method": "TRACE"})).unwrap_err()
            )
            .code,
            "invalid_method"
        );
    }

    #[test]
    fn query_and_body_are_read_from_arguments() {
        let parsed = Call::parse(&json!({"path": "/issues?state=opened", "query": {"page": 2, "labels": "bug"}, "body": "{\"a\":1}"})).unwrap();
        assert_eq!(parsed.method, Method::GET);
        assert_eq!(parsed.path, "/issues");
        assert_eq!(
            parsed.query,
            vec![
                ("state".into(), "opened".into()),
                ("labels".into(), "bug".into()),
                ("page".into(), "2".into())
            ]
        );
        assert_eq!(parsed.body, Some(json!({"a": 1})));
        assert_eq!(
            Failure::from_error(&Call::parse(&json!({"path": "/x", "body": "{nope"})).unwrap_err())
                .code,
            "invalid_body"
        );
    }

    #[test]
    fn only_reads_skip_the_write_gate() {
        assert!(call(Provider::Gitlab, json!({"path": "/user"})).1);
        assert!(
            !call(
                Provider::Gitlab,
                json!({"path": "/issues", "method": "POST", "body": {}})
            )
            .1
        );
        assert!(call(Provider::Linear, json!({"path": "/graphql", "method": "POST", "body": {"query": "query { viewer { id } }"}})).1);
        assert!(!call(Provider::Linear, json!({"path": "/graphql", "method": "POST", "body": {"query": "mutation { issueDelete(id: \"x\") { success } }"}})).1);
        assert!(call(Provider::Linear, json!({"path": "/graphql", "method": "POST", "body": {"query": "query { issues { nodes { mutationCount } } }"}})).1);
        assert!(
            call(
                Provider::Slack,
                json!({"path": "/search.messages", "query": {"query": "x"}})
            )
            .1
        );
        assert!(
            !call(
                Provider::Slack,
                json!({"path": "/chat.postMessage", "method": "POST", "body": {}})
            )
            .1
        );
        assert!(
            call(
                Provider::Notion,
                json!({"path": "/search", "method": "POST", "body": {}})
            )
            .1
        );
        assert!(
            !call(
                Provider::Notion,
                json!({"path": "/pages", "method": "POST", "body": {}})
            )
            .1
        );
        assert!(!reads_only(
            Provider::Gmail,
            &Call::parse(&json!({"path": "/x"})).unwrap()
        ));
    }

    #[test]
    fn every_connectable_service_has_a_documented_api_root() {
        for provider in Provider::ALL {
            match target(provider) {
                Some(t) => assert!(
                    t.base.starts_with("https://") || t.base.starts_with("{server}"),
                    "{provider:?}"
                ),
                None => assert!(matches!(provider, Provider::Teams | Provider::Gmail)),
            }
        }
    }

    #[test]
    fn slack_refusals_are_told_apart() {
        let scope = slack_ok(
            Provider::Slack,
            &json!({"ok": false, "error": "missing_scope", "needed": "chat:write"}),
        )
        .unwrap_err();
        assert_eq!(
            crate::integrations::errors::code_for(&scope, true),
            Some("insufficient_scope")
        );
        let auth = slack_ok(
            Provider::Slack,
            &json!({"ok": false, "error": "token_revoked"}),
        )
        .unwrap_err();
        assert_eq!(
            crate::integrations::errors::code_for(&auth, false),
            Some("credentials_expired")
        );
        assert!(slack_ok(Provider::Slack, &json!({"ok": true})).is_ok());
    }
}
