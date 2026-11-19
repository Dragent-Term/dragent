//! Forgejo and Gitea servers (Codeberg included): the same REST API.

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
        .request(method, format!("{}/api/v1{path}", connection.base_url()))
        .query(query);
    if let Some(body) = body {
        builder = builder.json(body);
    }
    connection.send(builder)
}

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    request(connection, Method::GET, path, query, None)
}

/// `owner/repo` of `root`'s repository, when it lives on this server.
fn own_repository(connection: &Connection, root: &Path) -> Option<String> {
    let base = connection.base_url();
    super::origin(root).and_then(|remote| super::repository(&remote, super::host(&base)))
}

/// The repository and issue number an id names: `owner/repo#12` (what
/// searches give) or `#12` in the project's own repository.
fn locate(connection: &Connection, root: &Path, id: &str) -> Result<(String, u64), String> {
    let id = id.trim();
    let (repository, number) = match id.rsplit_once('#') {
        Some((repository, number)) if !repository.is_empty() => {
            (Some(repository.to_string()), number)
        }
        _ => (None, id.trim_start_matches('#')),
    };
    let number: u64 = number
        .trim()
        .parse()
        .map_err(|_| format!("Issues here look like owner/repo#12 or #12, not “{id}”."))?;
    let repository = match repository {
        Some(repository) => repository,
        None => own_repository(connection, root).ok_or(
            "This folder's repository isn't on that server: give the issue as owner/repo#12.",
        )?,
    };
    Ok((repository, number))
}

pub(super) fn read(connection: &Connection, root: &Path, id: &str) -> Result<Item, String> {
    let (repository, number) = locate(connection, root, id)?;
    let path = format!("/repos/{repository}/issues/{number}");
    let row = get(connection, &path, &[])?;
    let mut item = parse(&Value::Array(vec![row.clone()]), Some(&repository))
        .pop()
        .ok_or_else(|| format!("There's no issue {id}."))?;
    let comments: Vec<(String, String)> = get(connection, &format!("{path}/comments"), &[])?
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| {
            let user = &comment["user"];
            let name = user["full_name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .or_else(|| user["login"].as_str())
                .unwrap_or_default();
            (
                name.to_string(),
                comment["body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    item.body = with_comments(row["body"].as_str().unwrap_or_default(), &comments);
    item.detail = super::joined(&[item.detail.as_deref(), row["state"].as_str()]);
    Ok(item)
}

pub(super) fn comment(
    connection: &Connection,
    root: &Path,
    id: &str,
    text: &str,
) -> Result<(), String> {
    let (repository, number) = locate(connection, root, id)?;
    request(
        connection,
        Method::POST,
        &format!("/repos/{repository}/issues/{number}/comments"),
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
    let open = super::open_or_closed(status).ok_or("Issues here are either open or closed.")?;
    let (repository, number) = locate(connection, root, id)?;
    let state = if open { "open" } else { "closed" };
    request(
        connection,
        Method::PATCH,
        &format!("/repos/{repository}/issues/{number}"),
        &[],
        Some(&json!({"state": state})),
    )?;
    Ok(state.to_string())
}

/// In the repository `draft.place` names (`owner/repo`), else `root`'s own.
pub(super) fn create(connection: &Connection, root: &Path, draft: &Draft) -> Result<Item, String> {
    let repository = draft
        .place
        .clone()
        .or_else(|| own_repository(connection, root))
        .ok_or("Say which repository it's for (owner/repo).")?;
    let row = request(
        connection,
        Method::POST,
        &format!("/repos/{repository}/issues"),
        &[],
        Some(&json!({"title": draft.title, "body": draft.body})),
    )?;
    parse(&Value::Array(vec![row]), Some(&repository))
        .pop()
        .ok_or_else(|| "The server didn't say which issue it created.".to_string())
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(connection, "/user", &[])?;
    let login = me["login"].as_str().map(|login| format!("@{login}"));
    Ok(super::named(me["full_name"].as_str(), login.as_deref()))
}

pub(super) fn search(
    connection: &Connection,
    root: &Path,
    query: &str,
) -> Result<Vec<Item>, String> {
    let limit = LIMIT.to_string();
    let mut params = vec![
        ("state", "open"),
        ("type", "issues"),
        ("limit", limit.as_str()),
    ];
    if !query.is_empty() {
        params.push(("q", query));
    }
    let repository = own_repository(connection, root);
    let data = match &repository {
        Some(repository) => get(connection, &format!("/repos/{repository}/issues"), &params)?,
        None => {
            // Across the user's repositories: the ones assigned to them
            // when nothing is typed.
            if query.is_empty() {
                params.push(("assigned", "true"));
            }
            get(connection, "/repos/issues/search", &params)?
        }
    };
    Ok(parse(&data, repository.as_deref()))
}

fn parse(data: &Value, repository: Option<&str>) -> Vec<Item> {
    data.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let number = row["number"].as_u64()?;
            let title = row["title"].as_str()?.to_string();
            let repository = row["repository"]["full_name"].as_str().or(repository);
            let labels: Vec<String> = row["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label["name"].as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Forgejo,
                key: format!("#{number}"),
                url: row["html_url"].as_str()?.to_string(),
                body: clip(row["body"].as_str().unwrap_or_default()),
                detail: repository.map(str::to_string),
                branch: branch_for(&number.to_string(), &title),
                reference: repository.map(|repository| format!("{repository}#{number}")),
                title,
                state: row["state"].as_str().map(|state| match state {
                    "open" => ItemState::open(state_label(state)),
                    "closed" => ItemState::done(state_label(state)),
                    other => ItemState::new(StateCategory::from_name(other), state_label(other)),
                }),
                assignee: (!row["assignee"].is_null())
                    .then_some(&row["assignee"])
                    .and_then(|assignee| {
                        assignee["full_name"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .or_else(|| assignee["login"].as_str())
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
    fn issues_read_with_their_repository() {
        let data = json!([{"number": 5, "title": "Fix CI", "html_url": "https://codeberg.org/a/b/issues/5", "body": "x", "repository": {"full_name": "a/b"}, "state": "open", "labels": [{"name": "bug"}], "assignee": {"login": "ada", "full_name": ""}, "updated_at": "2026-10-24T12:00:00Z"}]);
        let items = parse(&data, None);
        assert_eq!(
            (items[0].key.as_str(), items[0].detail.as_deref()),
            ("#5", Some("a/b"))
        );
        assert_eq!(items[0].state, Some(ItemState::open("Open")));
        assert_eq!(items[0].assignee.as_deref(), Some("ada"));
        assert_eq!(items[0].labels, vec!["bug"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
        let items = parse(
            &json!([{"number": 6, "title": "T", "html_url": "u", "state": "closed"}]),
            Some("own/repo"),
        );
        assert_eq!(items[0].detail.as_deref(), Some("own/repo"));
        assert_eq!(items[0].state, Some(ItemState::done("Closed")));
        assert_eq!(items[0].id(), "own/repo#6");
    }
}
