//! Plane's REST API, on Plane Cloud or a self-hosted server.

use reqwest::Method;
use serde_json::{json, Value};

use crate::integrations::items::{branch_for, choose, clip, updated_ms, with_comments, Draft, Item, ItemState, StateCategory};
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

/// A request under a project's work items, falling back to the `issues`
/// path servers used before work items were renamed.
fn work_items(
    connection: &Connection,
    method: Method,
    project: &str,
    rest: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let workspace = connection.required("workspace")?.to_string();
    let path =
        |segment: &str| format!("/workspaces/{workspace}/projects/{project}/{segment}/{rest}");
    request(connection, method.clone(), &path("work-items"), &[], body)
        .or_else(|error| request(connection, method, &path("issues"), &[], body).map_err(|_| error))
}

/// A project's id, identifier ("ENG") and name.
fn projects(connection: &Connection) -> Result<Vec<(String, String, String)>, String> {
    let workspace = connection.required("workspace")?.to_string();
    Ok(rows(&get(
        connection,
        &format!("/workspaces/{workspace}/projects/"),
        &[],
    )?)
    .iter()
    .filter_map(|project| {
        Some((
            project["id"].as_str()?.to_string(),
            project["identifier"].as_str()?.to_string(),
            project["name"].as_str().unwrap_or_default().to_string(),
        ))
    })
    .collect())
}

/// The project and work item ids an id names: `project/item` (what
/// searches give) or a key like `ENG-7`.
fn locate(connection: &Connection, id: &str) -> Result<(String, String), String> {
    let id = id.trim();
    if let Some((project, item)) = id.split_once('/') {
        return Ok((project.to_string(), item.to_string()));
    }
    let workspace = connection.required("workspace")?.to_string();
    let found = rows(&get(
        connection,
        &format!("/workspaces/{workspace}/work-items/search/"),
        &[("search", id)],
    )?);
    found
        .iter()
        .find_map(|row| {
            let key = format!(
                "{}-{}",
                row["project__identifier"].as_str()?,
                row["sequence_id"].as_u64()?
            );
            if !key.eq_ignore_ascii_case(id) {
                return None;
            }
            let project = row["project_id"]
                .as_str()
                .or_else(|| row["project"].as_str())?;
            Some((project.to_string(), row["id"].as_str()?.to_string()))
        })
        .ok_or_else(|| format!("Plane has no work item {id}."))
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let (project, item_id) = locate(connection, id)?;
    let row = work_items(
        connection,
        Method::GET,
        &project,
        &format!("{item_id}/"),
        None,
    )?;
    let (identifier, name) = projects(connection)?
        .into_iter()
        .find(|(id, _, _)| *id == project)
        .map(|(_, identifier, name)| (identifier, name))
        .unwrap_or_default();
    let workspace = connection.required("workspace")?.to_string();
    let mut item = item(
        &row,
        &identifier,
        Some(&name),
        &workspace,
        &app_url(connection),
    )
    .ok_or_else(|| format!("Plane has no work item {id}."))?;
    let comments: Vec<(String, String)> = work_items(
        connection,
        Method::GET,
        &project,
        &format!("{item_id}/comments/"),
        None,
    )
    .map(|data| rows(&data))
    .unwrap_or_default()
    .iter()
    .map(|comment| {
        let text = comment["comment_stripped"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                super::strip_html(comment["comment_html"].as_str().unwrap_or_default())
            });
        let author = comment["actor_detail"]["display_name"]
            .as_str()
            .unwrap_or_default();
        (author.to_string(), text)
    })
    .collect();
    item.body = with_comments(&item.body, &comments);
    Ok(item)
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    let (project, item_id) = locate(connection, id)?;
    work_items(
        connection,
        Method::POST,
        &project,
        &format!("{item_id}/comments/"),
        Some(&json!({"comment_html": super::html(text)})),
    )
    .map(|_| ())
}

pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let (project, item_id) = locate(connection, id)?;
    let workspace = connection.required("workspace")?.to_string();
    let states: Vec<(String, String)> = rows(&get(
        connection,
        &format!("/workspaces/{workspace}/projects/{project}/states/"),
        &[],
    )?)
    .iter()
    .filter_map(|state| {
        Some((
            state["id"].as_str()?.to_string(),
            state["name"].as_str()?.to_string(),
        ))
    })
    .collect();
    let (state, name) = choose(status, &states, "states of this project")?;
    work_items(
        connection,
        Method::PATCH,
        &project,
        &format!("{item_id}/"),
        Some(&json!({"state": state})),
    )?;
    Ok(name.clone())
}

/// In the project `draft.place` names (its identifier or name), or the
/// workspace's only project.
pub(super) fn create(connection: &Connection, draft: &Draft) -> Result<Item, String> {
    let projects = projects(connection)?;
    let project = match draft.place.as_deref() {
        Some(place) => projects.iter().find(|(_, identifier, name)| {
            identifier.eq_ignore_ascii_case(place) || name.eq_ignore_ascii_case(place)
        }),
        None if projects.len() == 1 => projects.first(),
        None => None,
    }
    .ok_or_else(|| {
        let identifiers: Vec<&str> = projects
            .iter()
            .map(|(_, identifier, _)| identifier.as_str())
            .collect();
        format!(
            "Say which Plane project it's for: {}.",
            identifiers.join(", ")
        )
    })?;
    let mut body = json!({"name": draft.title});
    if !draft.body.is_empty() {
        body["description_html"] = Value::String(super::html(&draft.body));
    }
    let row = work_items(connection, Method::POST, &project.0, "", Some(&body))?;
    let workspace = connection.required("workspace")?.to_string();
    item(
        &row,
        &project.1,
        Some(&project.2),
        &workspace,
        &app_url(connection),
    )
    .ok_or_else(|| "Plane didn't say which work item it created.".to_string())
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(connection, "/users/me/", &[])?;
    // The workspace has to exist for this key, too.
    let workspace = connection.required("workspace")?.to_string();
    get(
        connection,
        &format!("/workspaces/{workspace}/projects/"),
        &[("per_page", "1")],
    )?;
    Ok(super::named(
        me["display_name"].as_str(),
        me["email"].as_str(),
    ))
}

/// Where people open a work item: Plane Cloud's app, or the self-hosted
/// server itself.
fn app_url(connection: &Connection) -> String {
    let base = connection.base_url();
    if base == "https://api.plane.so" {
        "https://app.plane.so".to_string()
    } else {
        base
    }
}

/// A list response's rows, whatever it wraps them in.
fn rows(data: &Value) -> Vec<Value> {
    data.as_array()
        .or_else(|| data["results"].as_array())
        .or_else(|| data["issues"].as_array())
        .or_else(|| data["work_items"].as_array())
        .cloned()
        .unwrap_or_default()
}

pub(super) fn search(connection: &Connection, term: &str) -> Result<Vec<Item>, String> {
    let workspace = connection.required("workspace")?.to_string();
    let app = app_url(connection);
    if !term.is_empty() {
        let data = get(
            connection,
            &format!("/workspaces/{workspace}/work-items/search/"),
            &[("search", term)],
        )?;
        return Ok(rows(&data)
            .iter()
            .filter_map(|row| {
                let project = row["project__identifier"].as_str()?;
                item(row, project, None, &workspace, &app)
            })
            .collect());
    }
    // Nothing typed: the latest work items of the first few projects.
    let projects = rows(&get(
        connection,
        &format!("/workspaces/{workspace}/projects/"),
        &[],
    )?);
    let mut items = Vec::new();
    for project in projects.iter().take(4) {
        let (Some(id), Some(identifier)) = (project["id"].as_str(), project["identifier"].as_str())
        else {
            continue;
        };
        let path = format!("/workspaces/{workspace}/projects/{id}/work-items/");
        let data = match get(
            connection,
            &path,
            &[("per_page", "15"), ("order_by", "-updated_at")],
        ) {
            Ok(data) => data,
            // Servers from before work items were renamed.
            Err(_) => get(
                connection,
                &format!("/workspaces/{workspace}/projects/{id}/issues/"),
                &[("per_page", "15"), ("order_by", "-updated_at")],
            )?,
        };
        items.extend(
            rows(&data).iter().filter_map(|row| {
                item(row, identifier, project["name"].as_str(), &workspace, &app)
            }),
        );
    }
    Ok(items)
}

fn item(
    row: &Value,
    project: &str,
    project_name: Option<&str>,
    workspace: &str,
    app: &str,
) -> Option<Item> {
    let sequence = row["sequence_id"].as_u64()?;
    let title = row["name"].as_str()?.to_string();
    let key = format!("{project}-{sequence}");
    let state_name = row["state_detail"]["name"]
        .as_str()
        .or_else(|| row["state"]["name"].as_str());
    let labels: Vec<String> = row["label_details"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|label| label["name"].as_str().map(str::to_string))
        .collect();
    let labels = if labels.is_empty() {
        row["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| label.as_str().map(str::to_string))
            .collect()
    } else {
        labels
    };
    Some(Item {
        provider: Provider::Plane,
        url: format!("{app}/{workspace}/browse/{key}/"),
        body: clip(row["description_stripped"].as_str().unwrap_or_default()),
        detail: project_name.map(str::to_string),
        branch: branch_for(&key, &title),
        reference: row["id"].as_str().and_then(|id| {
            let project = row["project_id"]
                .as_str()
                .or_else(|| row["project"].as_str())?;
            Some(format!("{project}/{id}"))
        }),
        key,
        title,
        state: state_name.map(|name| {
            let category = match row["state_detail"]["group"].as_str() {
                Some("backlog") | Some("unstarted") => StateCategory::Open,
                Some("started") => StateCategory::Started,
                Some("completed") => StateCategory::Done,
                Some("cancelled") | Some("canceled") => StateCategory::Canceled,
                _ => StateCategory::from_name(name),
            };
            ItemState::new(category, name)
        }),
        assignee: row["assignee_details"]
            .as_array()
            .and_then(|assignees| assignees.first())
            .and_then(|assignee| assignee["display_name"].as_str())
            .map(str::to_string),
        labels,
        updated_at_ms: updated_ms(&row["updated_at"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn work_items_get_their_project_key_and_address() {
        let found = rows(
            &json!({"issues": [{"id": "1", "name": "Dark mode", "sequence_id": 7, "project__identifier": "ENG", "state_detail": {"name": "In Progress", "group": "started"}, "assignee_details": [{"display_name": "Ada"}], "label_details": [{"name": "ui"}], "updated_at": "2026-10-24T12:00:00.000Z"}]}),
        );
        let first = item(&found[0], "ENG", None, "my-team", "https://app.plane.so").unwrap();
        assert_eq!(first.key, "ENG-7");
        assert_eq!(first.url, "https://app.plane.so/my-team/browse/ENG-7/");
        assert_eq!(first.branch, "ENG-7-dark-mode");
        assert_eq!(first.id(), "ENG-7");
        assert_eq!(first.state, Some(ItemState::new(StateCategory::Started, "In Progress")));
        assert_eq!(first.assignee.as_deref(), Some("Ada"));
        assert_eq!(first.labels, vec!["ui"]);
        assert_eq!(first.updated_at_ms, Some(1_792_843_200_000));
        let row = json!({"id": "i1", "project": "p1", "name": "Docs", "sequence_id": 8});
        assert_eq!(item(&row, "ENG", None, "t", "a").unwrap().id(), "p1/i1");
        assert_eq!(rows(&json!({"results": [1, 2]})).len(), 2);
    }

    #[test]
    fn state_groups_map_to_categories() {
        for (group, name, category) in [
            ("backlog", "Backlog", StateCategory::Open),
            ("unstarted", "Todo", StateCategory::Open),
            ("started", "In Progress", StateCategory::Started),
            ("completed", "Done", StateCategory::Done),
            ("cancelled", "Cancelled", StateCategory::Canceled),
        ] {
            let row = json!({"id": "i", "project": "p", "name": "T", "sequence_id": 1, "state_detail": {"name": name, "group": group}});
            assert_eq!(
                item(&row, "ENG", None, "t", "a").unwrap().state,
                Some(ItemState::new(category, name)),
                "{group}"
            );
        }
    }
}
