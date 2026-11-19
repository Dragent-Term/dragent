//! Asana's REST API, with a personal access token.

use reqwest::Method;
use serde_json::{json, Value};

use crate::integrations::items::{branch_for, clip, updated_ms, with_comments, Draft, Item, ItemState, LIMIT};
use crate::integrations::{http, Connection, Provider};

const API: &str = "https://app.asana.com/api/1.0";
const TASK_FIELDS: &str = "name,notes,permalink_url,projects.name,completed,assignee.name,tags.name,modified_at";

fn request(
    connection: &Connection,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut builder = http()?.request(method, format!("{API}{path}")).query(query);
    if let Some(body) = body {
        builder = builder.json(body);
    }
    connection.send(builder)
}

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    request(connection, Method::GET, path, query, None)
}

/// A task's gid, from the gid itself or its address (which ends with it).
fn task(id: &str) -> Result<String, String> {
    let gid = id
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/f")
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if !gid.is_empty() && gid.chars().all(|c| c.is_ascii_digit()) {
        Ok(gid.to_string())
    } else {
        Err(format!(
            "Asana tasks are named by their number (gid), not “{id}”."
        ))
    }
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let gid = task(id)?;
    let data = get(
        connection,
        &format!("/tasks/{gid}"),
        &[("opt_fields", TASK_FIELDS)],
    )?;
    let mut item = parse(&json!({"data": [data["data"].clone()]}))
        .pop()
        .ok_or_else(|| format!("Asana has no task {gid}."))?;
    let stories = get(
        connection,
        &format!("/tasks/{gid}/stories"),
        &[("opt_fields", "type,text,created_by.name")],
    )?;
    let comments: Vec<(String, String)> = stories["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|story| story["type"].as_str() == Some("comment"))
        .map(|story| {
            (
                story["created_by"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                story["text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    item.body = with_comments(
        data["data"]["notes"].as_str().unwrap_or_default(),
        &comments,
    );
    Ok(item)
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    let gid = task(id)?;
    request(
        connection,
        Method::POST,
        &format!("/tasks/{gid}/stories"),
        &[],
        Some(&json!({"data": {"text": text}})),
    )
    .map(|_| ())
}

/// Asana tasks are done or not.
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let open =
        super::open_or_closed(status).ok_or("Asana tasks are either complete or incomplete.")?;
    let gid = task(id)?;
    request(
        connection,
        Method::PUT,
        &format!("/tasks/{gid}"),
        &[],
        Some(&json!({"data": {"completed": !open}})),
    )?;
    Ok(if open { "incomplete" } else { "complete" }.to_string())
}

/// Assigned to the user, in their workspace, and in the project
/// `draft.place` names by its gid.
pub(super) fn create(connection: &Connection, draft: &Draft) -> Result<Item, String> {
    let workspace = connection.required("workspace")?.to_string();
    let mut data = json!({
        "name": draft.title,
        "notes": draft.body,
        "workspace": workspace,
        "assignee": "me",
    });
    if let Some(project) = draft.place.as_deref() {
        data["projects"] =
            json!([task(project)
                .map_err(|_| "Give the Asana project by its number (gid).".to_string())?]);
    }
    let created = request(
        connection,
        Method::POST,
        "/tasks",
        &[("opt_fields", TASK_FIELDS)],
        Some(&json!({"data": data})),
    )?;
    parse(&json!({"data": [created["data"].clone()]}))
        .pop()
        .ok_or_else(|| "Asana didn't say which task it created.".to_string())
}

/// Also notes the workspace searches look in (the user's first).
pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(
        connection,
        "/users/me",
        &[("opt_fields", "name,email,workspaces.gid,workspaces.name")],
    )?;
    let me = &me["data"];
    let workspace = me["workspaces"][0]["gid"]
        .as_str()
        .ok_or("This Asana account has no workspace.")?;
    connection
        .credentials
        .insert("workspace".into(), workspace.to_string());
    Ok(super::named(me["name"].as_str(), me["email"].as_str()))
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    let workspace = connection.required("workspace")?.to_string();
    let limit = LIMIT.to_string();
    let data = if query.is_empty() {
        // The user's open tasks.
        get(
            connection,
            "/tasks",
            &[
                ("assignee", "me"),
                ("workspace", workspace.as_str()),
                ("completed_since", "now"),
                ("limit", limit.as_str()),
                ("opt_fields", TASK_FIELDS),
            ],
        )?
    } else {
        // Typeahead works on every plan (the full search API needs Premium).
        get(
            connection,
            &format!("/workspaces/{workspace}/typeahead"),
            &[
                ("resource_type", "task"),
                ("query", query),
                ("count", limit.as_str()),
                ("opt_fields", TASK_FIELDS),
            ],
        )?
    };
    Ok(parse(&data))
}

fn parse(data: &Value) -> Vec<Item> {
    data["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let gid = row["gid"].as_str()?;
            let title = row["name"]
                .as_str()
                .filter(|name| !name.is_empty())?
                .to_string();
            let short = &gid[gid.len().saturating_sub(6)..];
            let labels: Vec<String> = row["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tag| tag["name"].as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Asana,
                key: String::new(),
                url: row["permalink_url"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("https://app.asana.com/0/0/{gid}")),
                body: clip(row["notes"].as_str().unwrap_or_default()),
                detail: row["projects"][0]["name"].as_str().map(str::to_string),
                branch: branch_for(&format!("task-{short}"), &title),
                reference: Some(gid.to_string()),
                title,
                state: row["completed"].as_bool().map(|completed| {
                    if completed {
                        ItemState::done("Completed")
                    } else {
                        ItemState::open("Open")
                    }
                }),
                assignee: row["assignee"]["name"].as_str().map(str::to_string),
                labels,
                updated_at_ms: updated_ms(&row["modified_at"]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tasks_read_with_their_project() {
        let items = parse(&json!({"data": [
            {"gid": "1204567890123", "name": "Ship onboarding", "notes": "Steps", "permalink_url": "https://app.asana.com/0/1/1204567890123", "projects": [{"name": "Growth"}], "completed": false, "assignee": {"name": "Ada"}, "tags": [{"name": "launch"}], "modified_at": "2026-10-24T12:00:00.000Z"},
            {"gid": "1", "name": ""}
        ]}));
        assert_eq!(items.len(), 1, "untitled tasks are skipped");
        assert_eq!(items[0].branch, "task-890123-ship-onboarding");
        assert_eq!(items[0].detail.as_deref(), Some("Growth"));
        assert_eq!(items[0].state, Some(ItemState::open("Open")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].labels, vec!["launch"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
        let done = parse(&json!({"data": [{"gid": "2", "name": "T", "completed": true}]}));
        assert_eq!(done[0].state, Some(ItemState::done("Completed")));
    }
}
