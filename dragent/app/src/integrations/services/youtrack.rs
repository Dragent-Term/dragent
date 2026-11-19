//! YouTrack's REST API, with a permanent token.

use reqwest::Method;
use serde_json::{json, Value};

use crate::integrations::items::{branch_for, clip, updated_ms, with_comments, Draft, Item, ItemState, StateCategory};
use crate::integrations::{http, Connection, Provider};

fn request(
    connection: &Connection,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut builder = http()?
        .request(method, format!("{}/api{path}", connection.base_url()))
        .header("Accept", "application/json")
        .query(query);
    if let Some(body) = body {
        builder = builder.json(body);
    }
    connection.send(builder)
}

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    request(connection, Method::GET, path, query, None)
}

const ISSUE_FIELDS: &str =
    "idReadable,summary,description,project(name),resolved,updated,tags(name),customFields(name,value(fullName,name))";

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let fields = format!("{ISSUE_FIELDS},comments(text,author(fullName),created)");
    let row = get(
        connection,
        &format!("/issues/{}", id.trim()),
        &[("fields", fields.as_str())],
    )?;
    let mut item = parse(&Value::Array(vec![row.clone()]), &connection.base_url())
        .pop()
        .ok_or_else(|| format!("YouTrack has no issue {id}."))?;
    let comments: Vec<(String, String)> = row["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| {
            (
                comment["author"]["fullName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                comment["text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    item.body = with_comments(row["description"].as_str().unwrap_or_default(), &comments);
    Ok(item)
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    request(
        connection,
        Method::POST,
        &format!("/issues/{}/comments", id.trim()),
        &[("fields", "id")],
        Some(&json!({"text": text})),
    )
    .map(|_| ())
}

/// With a command, as people type them: `State {In Progress}`.
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let status = status.replace(['{', '}'], "");
    request(
        connection,
        Method::POST,
        "/commands",
        &[],
        Some(&json!({
            "query": format!("State {{{}}}", status.trim()),
            "issues": [{"idReadable": id.trim()}],
        })),
    )?;
    Ok(status.trim().to_string())
}

/// In the project `draft.place` names (its short name or name), or the
/// only one there is.
pub(super) fn create(connection: &Connection, draft: &Draft) -> Result<Item, String> {
    let projects: Vec<(String, String, String)> = get(
        connection,
        "/admin/projects",
        &[("fields", "id,shortName,name"), ("$top", "200")],
    )?
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(|project| {
        Some((
            project["id"].as_str()?.to_string(),
            project["shortName"].as_str()?.to_string(),
            project["name"].as_str().unwrap_or_default().to_string(),
        ))
    })
    .collect();
    let project = match draft.place.as_deref() {
        Some(place) => projects.iter().find(|(_, short, name)| {
            short.eq_ignore_ascii_case(place) || name.eq_ignore_ascii_case(place)
        }),
        None if projects.len() == 1 => projects.first(),
        None => None,
    }
    .ok_or_else(|| {
        let names: Vec<&str> = projects
            .iter()
            .map(|(_, short, _)| short.as_str())
            .collect();
        format!("Say which YouTrack project it's for: {}.", names.join(", "))
    })?;
    let row = request(
        connection,
        Method::POST,
        "/issues",
        &[("fields", ISSUE_FIELDS)],
        Some(&json!({
            "project": {"id": project.0},
            "summary": draft.title,
            "description": draft.body,
        })),
    )?;
    parse(&Value::Array(vec![row]), &connection.base_url())
        .pop()
        .ok_or_else(|| "YouTrack didn't say which issue it created.".to_string())
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(
        connection,
        "/users/me",
        &[("fields", "login,fullName,email")],
    )?;
    Ok(super::named(
        me["fullName"].as_str(),
        me["email"].as_str().or(me["login"].as_str()),
    ))
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    // YouTrack's own query language passes straight through.
    let query = if query.is_empty() {
        "#Unresolved sort by: updated desc"
    } else {
        query
    };
    let data = get(
        connection,
        "/issues",
        &[("query", query), ("fields", ISSUE_FIELDS), ("$top", "40")],
    )?;
    Ok(parse(&data, &connection.base_url()))
}

fn parse(data: &Value, base: &str) -> Vec<Item> {
    data.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let key = row["idReadable"].as_str()?.to_string();
            let title = row["summary"].as_str()?.to_string();
            let labels: Vec<String> = row["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tag| tag["name"].as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Youtrack,
                url: format!("{base}/issue/{key}"),
                body: clip(row["description"].as_str().unwrap_or_default()),
                detail: row["project"]["name"].as_str().map(str::to_string),
                branch: branch_for(&key, &title),
                reference: None,
                key,
                title,
                state: state_of(row),
                assignee: assignee_of(row),
                labels,
                updated_at_ms: updated_ms(&row["updated"]),
            })
        })
        .collect()
}

/// Resolved or not, named by its State field when there is one.
fn state_of(row: &Value) -> Option<ItemState> {
    let state = field_value(row, "state").and_then(|value| value["name"].as_str());
    match (row["resolved"].as_bool(), state) {
        (_, Some(name)) if !name.trim().is_empty() => {
            let category = if row["resolved"].as_bool() == Some(true) {
                StateCategory::Done
            } else {
                StateCategory::from_name(name)
            };
            Some(ItemState::new(category, name))
        }
        (Some(true), _) => Some(ItemState::done("Resolved")),
        (Some(false), _) => Some(ItemState::open("Open")),
        (None, _) => None,
    }
}

/// Who holds it, from its Assignee field when there is one.
fn assignee_of(row: &Value) -> Option<String> {
    let value = field_value(row, "assignee")?;
    value["fullName"]
        .as_str()
        .or_else(|| value["name"].as_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
}

/// A custom field's value, found by its name.
fn field_value<'a>(row: &'a Value, name: &str) -> Option<&'a Value> {
    row["customFields"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|field| field["name"].as_str().is_some_and(|field| field.eq_ignore_ascii_case(name)))
        .map(|field| &field["value"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn issues_open_on_the_server() {
        let items = parse(
            &json!([{"idReadable": "APP-3", "summary": "Slow sync", "description": null, "project": {"name": "App"}, "resolved": false, "updated": 1792843200000_i64, "tags": [{"name": "perf"}], "customFields": [{"name": "State", "value": {"name": "In Progress"}}, {"name": "Assignee", "value": {"fullName": "Ada"}}]}]),
            "https://acme.youtrack.cloud",
        );
        assert_eq!(items[0].url, "https://acme.youtrack.cloud/issue/APP-3");
        assert_eq!(
            (items[0].branch.as_str(), items[0].detail.as_deref()),
            ("APP-3-slow-sync", Some("App"))
        );
        assert_eq!(items[0].state, Some(ItemState::new(StateCategory::Started, "In Progress")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].labels, vec!["perf"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
    }

    #[test]
    fn resolved_without_a_state_still_maps() {
        let done = parse(
            &json!([{"idReadable": "APP-4", "summary": "T", "resolved": true}]),
            "https://acme.youtrack.cloud",
        );
        assert_eq!(done[0].state, Some(ItemState::done("Resolved")));
        let open = parse(
            &json!([{"idReadable": "APP-5", "summary": "T", "resolved": false}]),
            "https://acme.youtrack.cloud",
        );
        assert_eq!(open[0].state, Some(ItemState::open("Open")));
        let unknown = parse(
            &json!([{"idReadable": "APP-6", "summary": "T"}]),
            "https://acme.youtrack.cloud",
        );
        assert_eq!(unknown[0].state, None);
    }
}
