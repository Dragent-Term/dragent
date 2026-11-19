//! GitLab's REST API, on gitlab.com or a self-managed server.

use std::path::Path;

use reqwest::Method;
use serde_json::{json, Value};

use crate::integrations::items::{branch_for, clip, state_label, updated_ms, with_comments, Draft, Item, ItemState, StateCategory, LIMIT};
use crate::integrations::{http, Connection, Provider};

fn request(
    connection: &Connection,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut builder = http()?
        .request(method, format!("{}/api/v4{path}", connection.base_url()))
        .query(query);
    if let Some(body) = body {
        builder = builder.json(body);
    }
    connection.send(builder)
}

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    request(connection, Method::GET, path, query, None)
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(connection, "/user", &[])?;
    let username = me["username"].as_str().map(|name| format!("@{name}"));
    Ok(super::named(me["name"].as_str(), username.as_deref()))
}

/// The path of the project `root`'s repository, when it lives on this
/// GitLab.
fn own_project(connection: &Connection, root: &Path) -> Option<String> {
    let base = connection.base_url();
    super::origin(root).and_then(|remote| super::repository(&remote, super::host(&base)))
}

pub(super) fn search(
    connection: &Connection,
    root: &Path,
    query: &str,
) -> Result<Vec<Item>, String> {
    let limit = LIMIT.to_string();
    let mut params = vec![
        ("state", "opened"),
        ("order_by", "updated_at"),
        ("per_page", limit.as_str()),
    ];
    if !query.is_empty() {
        params.push(("search", query));
    }
    // The project's own issues when it lives on this GitLab, else the
    // user's assigned ones.
    let data = match own_project(connection, root) {
        Some(project) => {
            let encoded = project.replace('/', "%2F");
            get(connection, &format!("/projects/{encoded}/issues"), &params)?
        }
        None => {
            params.push(("scope", "assigned_to_me"));
            get(connection, "/issues", &params)?
        }
    };
    Ok(parse(&data))
}

/// The project (its id or encoded path) and issue number an id names:
/// `123:45` (a project id and issue, what searches give), `group/app#45`,
/// or `#45` in the project's own repository.
fn locate(connection: &Connection, root: &Path, id: &str) -> Result<(String, u64), String> {
    let id = id.trim();
    let invalid = || format!("GitLab issues look like 123:45, group/app#45 or #45, not “{id}”.");
    let (project, number) = match id.split_once(':').or_else(|| id.rsplit_once('#')) {
        Some((project, number)) if !project.is_empty() => (Some(project.to_string()), number),
        _ => (None, id.trim_start_matches('#')),
    };
    let number: u64 = number.trim().parse().map_err(|_| invalid())?;
    let project = match project {
        Some(project) => project,
        None => own_project(connection, root).ok_or(
            "This folder's repository isn't on that GitLab: give the issue as group/app#45.",
        )?,
    };
    Ok((project.replace('/', "%2F"), number))
}

pub(super) fn read(connection: &Connection, root: &Path, id: &str) -> Result<Item, String> {
    let (project, number) = locate(connection, root, id)?;
    let path = format!("/projects/{project}/issues/{number}");
    let row = get(connection, &path, &[])?;
    let mut item = parse(&Value::Array(vec![row.clone()]))
        .pop()
        .ok_or_else(|| format!("GitLab has no issue {id}."))?;
    let notes = get(
        connection,
        &format!("{path}/notes"),
        &[
            ("sort", "asc"),
            ("order_by", "created_at"),
            ("per_page", "50"),
        ],
    )?;
    let comments: Vec<(String, String)> = notes
        .as_array()
        .into_iter()
        .flatten()
        // Not "changed the milestone" and the like.
        .filter(|note| note["system"].as_bool() != Some(true))
        .map(|note| {
            (
                note["author"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                note["body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    item.body = with_comments(row["description"].as_str().unwrap_or_default(), &comments);
    let state = row["state"].as_str();
    item.detail = super::joined(&[item.detail.as_deref(), state]);
    Ok(item)
}

pub(super) fn comment(
    connection: &Connection,
    root: &Path,
    id: &str,
    text: &str,
) -> Result<(), String> {
    let (project, number) = locate(connection, root, id)?;
    request(
        connection,
        Method::POST,
        &format!("/projects/{project}/issues/{number}/notes"),
        &[],
        Some(&json!({"body": text})),
    )
    .map(|_| ())
}

pub(super) fn set_status(
    connection: &Connection,
    root: &Path,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let open = super::open_or_closed(status).ok_or("GitLab issues are either open or closed.")?;
    let (project, number) = locate(connection, root, id)?;
    let row = request(
        connection,
        Method::PUT,
        &format!("/projects/{project}/issues/{number}"),
        &[],
        Some(&json!({"state_event": if open { "reopen" } else { "close" }})),
    )?;
    Ok(row["state"]
        .as_str()
        .unwrap_or(if open { "opened" } else { "closed" })
        .to_string())
}

/// In the project `draft.place` names (its path), else `root`'s own.
pub(super) fn create(connection: &Connection, root: &Path, draft: &Draft) -> Result<Item, String> {
    let project = draft
        .place
        .clone()
        .or_else(|| own_project(connection, root))
        .ok_or("Say which GitLab project it's for (its path, like group/app).")?
        .replace('/', "%2F");
    let row = request(
        connection,
        Method::POST,
        &format!("/projects/{project}/issues"),
        &[],
        Some(&json!({"title": draft.title, "description": draft.body})),
    )?;
    parse(&Value::Array(vec![row]))
        .pop()
        .ok_or_else(|| "GitLab didn't say which issue it created.".to_string())
}

fn parse(data: &Value) -> Vec<Item> {
    data.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let iid = row["iid"].as_u64()?;
            let title = row["title"].as_str()?.to_string();
            let project = row["references"]["full"]
                .as_str()
                .and_then(|full| full.rsplit_once('#'))
                .map(|(project, _)| project);
            let labels: Vec<String> = row["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label.as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Gitlab,
                key: row["references"]["short"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("#{iid}")),
                url: row["web_url"].as_str()?.to_string(),
                body: clip(row["description"].as_str().unwrap_or_default()),
                detail: project.map(str::to_string),
                branch: branch_for(&iid.to_string(), &title),
                reference: row["project_id"]
                    .as_u64()
                    .map(|project| format!("{project}:{iid}")),
                title,
                state: row["state"].as_str().map(|state| match state {
                    "opened" => ItemState::open(state_label(state)),
                    "closed" => ItemState::done(state_label(state)),
                    other => ItemState::new(StateCategory::from_name(other), state_label(other)),
                }),
                assignee: row["assignees"]
                    .as_array()
                    .and_then(|assignees| assignees.first())
                    .or_else(|| {
                        (!row["assignee"].is_null()).then_some(&row["assignee"])
                    })
                    .and_then(|assignee| {
                        assignee["name"]
                            .as_str()
                            .or_else(|| assignee["username"].as_str())
                    })
                    .map(str::to_string),
                labels,
                updated_at_ms: updated_ms(&row["updated_at"]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn issues_read_with_their_project() {
        let items = parse(&json!([
            {"iid": 3, "title": "Docs", "web_url": "https://gitlab.com/g/p/-/issues/3", "description": null, "references": {"short": "#3", "full": "g/p#3"}, "state": "opened", "labels": ["docs"], "assignees": [{"name": "Ada"}], "updated_at": "2026-10-24T12:00:00.000Z"}
        ]));
        assert_eq!(
            (
                items[0].key.as_str(),
                items[0].branch.as_str(),
                items[0].body.as_str()
            ),
            ("#3", "3-docs", "")
        );
        assert_eq!(items[0].detail.as_deref(), Some("g/p"));
        assert_eq!(items[0].reference, None);
        assert_eq!(items[0].state, Some(ItemState::open("Opened")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].labels, vec!["docs"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
        let items = parse(&json!([{"iid": 4, "project_id": 77, "title": "T", "web_url": "u", "state": "closed"}]));
        assert_eq!(items[0].id(), "77:4");
        assert_eq!(items[0].state, Some(ItemState::done("Closed")));
    }
}
