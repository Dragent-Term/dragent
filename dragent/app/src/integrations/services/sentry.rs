//! Sentry's API, on sentry.io or a self-hosted server: unresolved issues,
//! and the latest event's stack trace for the one picked.

use reqwest::Method;
use serde_json::{json, Value};

use crate::integrations::items::{branch_for, clip, Item, LIMIT};
use crate::integrations::{http, Connection, Provider};

fn request(
    connection: &Connection,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let organization = connection.required("organization")?;
    let mut builder = http()?
        .request(
            method,
            format!(
                "{}/api/0/organizations/{organization}{path}",
                connection.base_url()
            ),
        )
        .query(query);
    if let Some(body) = body {
        builder = builder.json(body);
    }
    connection.send(builder)
}

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    request(connection, Method::GET, path, query, None)
}

/// The issue's numeric id, from it or from its short id (`BACKEND-3K`).
fn issue_id(connection: &Connection, id: &str) -> Result<String, String> {
    let id = id.trim();
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        return Ok(id.to_string());
    }
    let found = get(
        connection,
        &format!("/shortids/{}/", id.to_uppercase()),
        &[],
    )?;
    found["groupId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| found["group"]["id"].as_str().map(str::to_string))
        .ok_or_else(|| format!("Sentry has no issue {id}."))
}

pub(super) fn read(connection: &mut Connection, id: &str) -> Result<Item, String> {
    let issue = issue_id(connection, id)?;
    let row = get(connection, &format!("/issues/{issue}/"), &[])?;
    let mut item = parse(&Value::Array(vec![row]))
        .pop()
        .ok_or_else(|| format!("Sentry has no issue {id}."))?;
    if let Ok(trace) = details(connection, &issue) {
        if !trace.is_empty() {
            item.body = trace;
        }
    }
    Ok(item)
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    let issue = issue_id(connection, id)?;
    request(
        connection,
        Method::POST,
        &format!("/issues/{issue}/comments/"),
        &[],
        Some(&json!({"text": text})),
    )
    .map(|_| ())
}

/// Resolved, unresolved or ignored (archived).
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let wanted = status.trim().to_lowercase();
    let status = match wanted.as_str() {
        "ignored" | "ignore" | "archived" | "archive" | "muted" => "ignored",
        _ => match super::open_or_closed(&wanted) {
            Some(true) => "unresolved",
            Some(false) => "resolved",
            None => return Err("Sentry issues can be resolved, unresolved or ignored.".into()),
        },
    };
    let issue = issue_id(connection, id)?;
    request(
        connection,
        Method::PUT,
        &format!("/issues/{issue}/"),
        &[],
        Some(&json!({"status": status})),
    )?;
    Ok(status.to_string())
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let organization = get(connection, "/", &[])?;
    Ok(organization["name"].as_str().map(str::to_string))
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    let query = format!("is:unresolved {query}").trim().to_string();
    let limit = LIMIT.to_string();
    let data = get(
        connection,
        "/issues/",
        &[
            ("query", query.as_str()),
            ("limit", limit.as_str()),
            ("statsPeriod", "14d"),
            ("sort", "date"),
        ],
    )?;
    Ok(parse(&data))
}

fn parse(data: &Value) -> Vec<Item> {
    data.as_array()
        .into_iter()
        .flatten()
        .filter_map(|issue| {
            let id = issue["id"].as_str()?;
            let key = issue["shortId"].as_str()?.to_string();
            let title = issue["title"].as_str()?.to_string();
            let events = issue["count"]
                .as_str()
                .map(|count| format!("{count} events"));
            let mut body = vec![title.clone()];
            if let Some(culprit) = issue["culprit"].as_str().filter(|c| !c.is_empty()) {
                body.push(format!("In {culprit}"));
            }
            if let Some(value) = issue["metadata"]["value"]
                .as_str()
                .filter(|v| !v.is_empty())
            {
                body.push(value.to_string());
            }
            Some(Item {
                provider: Provider::Sentry,
                url: issue["permalink"].as_str()?.to_string(),
                body: clip(&body.join("\n")),
                detail: super::joined(&[
                    issue["project"]["slug"].as_str(),
                    issue["level"].as_str(),
                    events.as_deref(),
                ]),
                branch: branch_for(&key.to_lowercase(), &title),
                reference: Some(id.to_string()),
                key,
                title,
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
            })
        })
        .collect()
}

/// The issue's latest event: the error, where it was thrown, and the
/// innermost frames of its stack trace.
pub(super) fn details(connection: &mut Connection, issue: &str) -> Result<String, String> {
    let event = get(connection, &format!("/issues/{issue}/events/latest/"), &[])?;
    Ok(event_text(&event))
}

fn event_text(event: &Value) -> String {
    let mut out = Vec::new();
    if let Some(title) = event["title"].as_str() {
        out.push(title.to_string());
    }
    for entry in event["entries"].as_array().into_iter().flatten() {
        if entry["type"].as_str() != Some("exception") {
            continue;
        }
        for exception in entry["data"]["values"].as_array().into_iter().flatten() {
            out.push(format!(
                "\n{}: {}",
                exception["type"].as_str().unwrap_or("Error"),
                exception["value"].as_str().unwrap_or_default()
            ));
            let frames: Vec<&Value> = exception["stacktrace"]["frames"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            // Innermost last in Sentry's order: show the last frames first.
            for frame in frames.iter().rev().take(15) {
                let file = frame["filename"]
                    .as_str()
                    .or_else(|| frame["absPath"].as_str())
                    .unwrap_or("?");
                let line = frame["lineNo"]
                    .as_u64()
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default();
                let function = frame["function"].as_str().unwrap_or("?");
                let own = if frame["inApp"].as_bool() == Some(true) {
                    ""
                } else {
                    "  (library)"
                };
                out.push(format!("  at {function} ({file}{line}){own}"));
            }
        }
    }
    out.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn issues_and_their_stack_traces_read_for_the_agent() {
        let items = parse(&json!([{
            "id": "42", "shortId": "BACKEND-3K", "title": "TypeError: x is undefined",
            "culprit": "app/billing.js in charge", "permalink": "https://acme.sentry.io/issues/42/",
            "metadata": {"value": "x is undefined"}, "project": {"slug": "backend"}, "level": "error", "count": "128"
        }]));
        assert_eq!(items[0].branch, "backend-3k-typeerror-x-is-undefined");
        assert_eq!(
            items[0].detail.as_deref(),
            Some("backend · error · 128 events")
        );
        assert!(items[0].body.contains("In app/billing.js in charge"));
        let event = json!({"title": "TypeError", "entries": [{"type": "exception", "data": {"values": [{
            "type": "TypeError", "value": "x is undefined",
            "stacktrace": {"frames": [
                {"filename": "node_modules/lib.js", "lineNo": 3, "function": "run", "inApp": false},
                {"filename": "app/billing.js", "lineNo": 88, "function": "charge", "inApp": true}
            ]}
        }]}}]});
        let text = event_text(&event);
        assert!(text.contains("TypeError: x is undefined"));
        let charge = text.find("at charge (app/billing.js:88)").unwrap();
        let library = text
            .find("at run (node_modules/lib.js:3)  (library)")
            .unwrap();
        assert!(charge < library, "innermost frame first");
    }
}
