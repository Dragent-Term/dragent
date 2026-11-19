//! The organisation's Tasks for the phone (Dragent companion).
//!
//! The phone is paired with this desktop, not signed in to the server, so it
//! reaches Tasks through the desktop over the end-to-end encrypted channel:
//! each request runs here with the desktop's own sign-in, in the
//! organisation the phone's channel belongs to, off the socket thread.
//!
//! Methods:
//! - `tasks.list {status?, q?, assignee?}` -> `{prefix, tasks:[…], truncated}`
//! - `tasks.get {task}` -> `{task, comments:[…], activity:[…]}`
//! - `tasks.create {title, description?, status?, priority?, assignee_id?}`
//!   -> `{task}`
//! - `tasks.update {task, title?, description?, status?, priority?,
//!   assignee_id?, labels?, due_at?, project?}` -> `{task}`
//!   (`assignee_id: ""` unassigns, `due_at: null` and `project: ""` clear)
//! - `tasks.delete {task}` -> `{}`; `tasks.restore {task}` -> `{task}`
//! - `tasks.comment {task, body}` -> `{comment}`
//! - `tasks.comment_edit {task, comment, body}` -> `{comment}`
//! - `tasks.comment_delete {task, comment}` -> `{}`
//! - `tasks.link {task, url? | item?, title?}` -> `{task}` (a pull request
//!   address or a `PROVIDER:KEY` tracker item); `tasks.unlink {task, link}`
//! - `tasks.members {}` -> `{members:[{id, name}]}`
//! - `tasks.options {}` -> `{members, labels, projects:[{key, label, path}],
//!   agents:[{key, label}]}`: what the phone's pickers offer
//!
//! Every request names its fields exactly (unknown ones are refused), every
//! text is bounded, and answers are projections: no links' private
//! references, no raw server errors.

use serde_json::{json, Map, Value};

use super::companion_host::Failure;
use super::desktop_catalog::AccountKey;

/// Tasks a list answers with at most.
const MAX_LIST: u32 = 200;
/// Description shown in a list row.
const LIST_DESCRIPTION_CHARS: usize = 280;
/// Description in a task's detail.
const DETAIL_DESCRIPTION_CHARS: usize = 20_000;
/// Comments and history entries in a detail (the newest).
const MAX_COMMENTS: usize = 100;
const MAX_ACTIVITY: usize = 100;
const MAX_TITLE_CHARS: usize = 500;
const MAX_COMMENT_CHARS: usize = 20_000;
const MAX_QUERY_CHARS: usize = 200;

pub(crate) const METHODS: &[&str] = &[
    "tasks.list",
    "tasks.get",
    "tasks.create",
    "tasks.update",
    "tasks.comment",
    "tasks.members",
    "tasks.delete",
    "tasks.restore",
    "tasks.comment_edit",
    "tasks.comment_delete",
    "tasks.link",
    "tasks.unlink",
    "tasks.options",
];

/// Labels one task may carry from the phone.
const MAX_LABELS: usize = 20;
const MAX_LABEL_CHARS: usize = 48;
const MAX_LINKS: usize = 50;
const MAX_URL_CHARS: usize = 2048;

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

fn clip(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        (text.to_string(), false)
    } else {
        (text.chars().take(max).collect(), true)
    }
}

fn person(person: Option<&dragent_tasks::Person>) -> Value {
    person.map_or(Value::Null, |person| json!({"id": person.id, "name": person.display()}))
}

/// A task as the phone sees it in a list (description cut short).
pub(crate) fn task_summary(task: &dragent_tasks::Task) -> Value {
    task_json(task, DETAIL_DESCRIPTION_CHARS)
}

/// A task as the phone sees it.
fn task_json(task: &dragent_tasks::Task, description_chars: usize) -> Value {
    let (description, cut) = clip(&task.description, description_chars);
    json!({
        "id": task.id,
        "key": task.key,
        "title": clip(&task.title, MAX_TITLE_CHARS).0,
        "description": description,
        "description_truncated": cut,
        "status": task.status.key(),
        "priority": serde_json::to_value(task.priority).unwrap_or(Value::Null),
        "assignee": person(task.assignee.as_ref()),
        "creator": person(task.creator.as_ref()),
        "project": task.project.as_ref().map(|project| project.label.clone()),
        "project_key": task.project.as_ref().map(|project| project.key.clone()),
        "labels": task.labels,
        "links": task.links.iter().take(MAX_LINKS).map(link_json).collect::<Vec<_>>(),
        "comment_count": task.comment_count,
        "created_at": task.created_at_ms,
        "updated_at": task.updated_at_ms,
        "due_at": task.due_at_ms,
    })
}

/// A link as the phone sees it: a workspace's name only (its reference is
/// a folder on one machine), a pull request's or item's web address.
fn link_json(link: &dragent_tasks::TaskLink) -> Value {
    let url = link
        .url
        .as_deref()
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .filter(|url| url.len() <= MAX_URL_CHARS)
        .filter(|_| link.kind != dragent_tasks::LinkKind::Workspace);
    // A workspace on this desktop: its folder, so the phone can open it
    // (the phone already sees this desktop's workspace folders). Other
    // machines' folders stay with them.
    let workspace_root = (link.kind == dragent_tasks::LinkKind::Workspace)
        .then(|| dragent_tasks::parse_workspace_reference(&link.reference))
        .flatten()
        .filter(|(device, _)| *device == crate::settings::AppSettings::device_id())
        .map(|(_, root)| clip(root, 4096).0);
    json!({
        "id": link.id,
        "workspace_root": workspace_root,
        "kind": serde_json::to_value(link.kind).unwrap_or(Value::Null),
        "title": clip(&link.title, MAX_TITLE_CHARS).0,
        "url": url,
        "state": link.state.as_deref().map(|state| clip(state, 32).0),
    })
}

fn comment_json(comment: &dragent_tasks::Comment, me: &str) -> Value {
    json!({
        "id": comment.id,
        "author": person(comment.author.as_ref()),
        "by_agent": comment.actor_kind == dragent_tasks::ActorKind::Agent,
        "mine": !me.is_empty()
            && comment.actor_kind != dragent_tasks::ActorKind::Agent
            && comment.author.as_ref().is_some_and(|author| author.id == me),
        "body": clip(&comment.body, MAX_COMMENT_CHARS).0,
        "created_at": comment.created_at_ms,
    })
}

/// A server id (comment, link): plain characters only.
fn plain_id(object: &Map<String, Value>, key: &str) -> Result<String, Failure> {
    let id = text(object, key, 128)?.ok_or(Failure::Invalid)?;
    let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then_some(id).ok_or(Failure::Invalid)
}

/// `labels: [..]`: cleaned the way the desktop cleans them.
fn labels(object: &Map<String, Value>) -> Result<Option<Vec<String>>, Failure> {
    let Some(value) = object.get("labels") else { return Ok(None) };
    let list = value.as_array().filter(|list| list.len() <= MAX_LABELS).ok_or(Failure::Invalid)?;
    let mut out = Vec::new();
    for label in list {
        let label = label
            .as_str()
            .filter(|label| label.chars().count() <= MAX_LABEL_CHARS && !label.contains('\0'))
            .ok_or(Failure::Invalid)?;
        out.push(label.to_string());
    }
    Ok(Some(crate::tasks::client::clean_labels(out)))
}

/// `due_at`: absent leaves it, `null` clears it, else epoch milliseconds.
fn due_at(object: &Map<String, Value>) -> Result<Option<Option<i64>>, Failure> {
    match object.get("due_at") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) => value
            .as_i64()
            .filter(|ms| (0..=253_402_300_799_000).contains(ms))
            .map(|ms| Some(Some(ms)))
            .ok_or(Failure::Invalid),
    }
}

/// `project`: absent leaves it, `""` clears it, else a shared project key
/// (`github.com/acme/app`).
fn project(object: &Map<String, Value>) -> Result<Option<Option<dragent_tasks::ProjectRef>>, Failure> {
    match text(object, "project", 300)? {
        None => Ok(None),
        Some(key) if key.is_empty() => Ok(Some(None)),
        Some(key) => {
            let valid = key.contains('/')
                && !key.starts_with('/')
                && !key.split('/').any(|part| part.is_empty() || part == "." || part == "..")
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./~".contains(&b));
            if !valid {
                return Err(Failure::Invalid);
            }
            crate::tasks::client::resolve_project(&key)
                .map(|project| Some(Some(project)))
                .map_err(|_| Failure::Invalid)
        }
    }
}

fn object(params: &Value, allowed: &[&str]) -> Result<Map<String, Value>, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    Ok(object.clone())
}

fn text(object: &Map<String, Value>, key: &str, max: usize) -> Result<Option<String>, Failure> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= max && !value.contains('\0') => {
            Ok(Some(value.clone()))
        }
        _ => Err(Failure::Invalid),
    }
}

/// A task id or key (`DRA-12`).
fn task_ref(object: &Map<String, Value>) -> Result<String, Failure> {
    let task = text(object, "task", 64)?.ok_or(Failure::Invalid)?;
    let valid = !task.is_empty()
        && task.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then_some(task).ok_or(Failure::Invalid)
}

fn status(object: &Map<String, Value>) -> Result<Option<dragent_tasks::Status>, Failure> {
    text(object, "status", 32)?
        .map(|key| crate::tasks::client::parse_status(&key).map_err(|_| Failure::Invalid))
        .transpose()
}

fn priority(object: &Map<String, Value>) -> Result<Option<dragent_tasks::Priority>, Failure> {
    text(object, "priority", 32)?
        .map(|key| crate::tasks::client::parse_priority(&key).map_err(|_| Failure::Invalid))
        .transpose()
}

fn member_id(object: &Map<String, Value>) -> Result<Option<String>, Failure> {
    let id = text(object, "assignee_id", 128)?;
    match id {
        Some(id) if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') => {
            Err(Failure::Invalid)
        }
        other => Ok(other),
    }
}

/// The organisation to act in: the channel's, and only while this desktop
/// is still signed in to it.
fn organisation(expected: &AccountKey) -> Result<String, Failure> {
    if expected.org.is_empty()
        || expected.org == "local"
        || dragent_paths::organisation_id() != expected.org
    {
        return Err(Failure::Gone);
    }
    let active = crate::tasks::client::active_org().map_err(|_| Failure::Unavailable)?;
    if active != expected.org {
        return Err(Failure::Gone);
    }
    Ok(active)
}

/// A server failure as the phone may see it: never the raw message.
fn unavailable<T>(_: String) -> Result<T, Failure> {
    Err(Failure::Unavailable)
}

/// Check a request on the socket thread and return the work to run off it.
pub(crate) fn job(expected: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let expected = expected.clone();
    match method {
        "tasks.list" => {
            let o = object(params, &["status", "q", "assignee", "label"])?;
            let status = text(&o, "status", 16)?;
            if status.as_deref().is_some_and(|s| !matches!(s, "open" | "closed" | "all")) {
                return Err(Failure::Invalid);
            }
            let q = text(&o, "q", MAX_QUERY_CHARS)?.filter(|q| !q.trim().is_empty());
            // `me`, `none`, or a member's id.
            let assignee = text(&o, "assignee", 128)?;
            if assignee
                .as_deref()
                .is_some_and(|a| a.is_empty() || !a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            {
                return Err(Failure::Invalid);
            }
            let label = text(&o, "label", MAX_LABEL_CHARS)?.map(|label| label.trim().to_lowercase()).filter(|label| !label.is_empty());
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let query = dragent_tasks::ListQuery {
                    status: Some(status.unwrap_or_else(|| "open".into())),
                    assignee,
                    label,
                    q,
                    limit: Some(MAX_LIST),
                    ..Default::default()
                };
                let list = crate::tasks::client::list(&org, &query).or_else(unavailable)?;
                let tasks: Vec<Value> = list
                    .tasks
                    .iter()
                    .take(MAX_LIST as usize)
                    .map(|task| task_json(task, LIST_DESCRIPTION_CHARS))
                    .collect();
                Ok(json!({
                    "prefix": list.prefix,
                    "tasks": tasks,
                    "truncated": list.next_cursor.is_some() || list.tasks.len() > MAX_LIST as usize,
                }))
            }))
        }
        "tasks.get" => {
            let o = object(params, &["task"])?;
            let task = task_ref(&o)?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let detail = crate::tasks::client::detail(&org, &task).or_else(unavailable)?;
                let skip = detail.comments.len().saturating_sub(MAX_COMMENTS);
                let comments: Vec<Value> = detail
                    .comments
                    .iter()
                    .skip(skip)
                    .map(|comment| comment_json(comment, &expected.user_id))
                    .collect();
                let skip = detail.activity.len().saturating_sub(MAX_ACTIVITY);
                let activity: Vec<Value> = detail
                    .activity
                    .iter()
                    .skip(skip)
                    .map(|entry| {
                        // Only simple values: a status or priority key, or
                        // a person's name.
                        let value = |value: &Option<Value>| match value {
                            Some(Value::String(text)) => Value::String(clip(text, 200).0),
                            Some(Value::Object(map)) => map
                                .get("name")
                                .filter(|name| name.as_str().is_some_and(|n| !n.is_empty()))
                                .or_else(|| map.get("email"))
                                .cloned()
                                .unwrap_or(Value::Null),
                            _ => Value::Null,
                        };
                        json!({
                            "id": entry.id,
                            "kind": serde_json::to_value(entry.kind).unwrap_or(Value::Null),
                            "actor": person(entry.actor.as_ref()),
                            "by_agent": entry.actor_kind == dragent_tasks::ActorKind::Agent,
                            "from": value(&entry.from),
                            "to": value(&entry.to),
                            "at": entry.at_ms,
                        })
                    })
                    .collect();
                Ok(json!({
                    "task": task_json(&detail.task, DETAIL_DESCRIPTION_CHARS),
                    "comments": comments,
                    "activity": activity,
                }))
            }))
        }
        "tasks.create" => {
            let o = object(params, &["title", "description", "status", "priority", "assignee_id"])?;
            let title = text(&o, "title", MAX_TITLE_CHARS)?
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty())
                .ok_or(Failure::Invalid)?;
            let new = dragent_tasks::NewTask {
                title,
                description: text(&o, "description", DETAIL_DESCRIPTION_CHARS)?.filter(|d| !d.trim().is_empty()),
                status: status(&o)?,
                priority: priority(&o)?,
                assignee_id: member_id(&o)?.filter(|id| !id.is_empty()),
                ..Default::default()
            };
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let task = crate::tasks::client::create(&org, &new).or_else(unavailable)?;
                Ok(json!({"task": task_json(&task, DETAIL_DESCRIPTION_CHARS)}))
            }))
        }
        "tasks.update" => {
            let o = object(
                params,
                &["task", "title", "description", "status", "priority", "assignee_id", "labels", "due_at", "project"],
            )?;
            let task = task_ref(&o)?;
            let title = match text(&o, "title", MAX_TITLE_CHARS)? {
                Some(title) if title.trim().is_empty() => return Err(Failure::Invalid),
                other => other.map(|title| title.trim().to_string()),
            };
            let patch = dragent_tasks::TaskPatch {
                title,
                description: text(&o, "description", DETAIL_DESCRIPTION_CHARS)?,
                labels: labels(&o)?,
                due_at_ms: due_at(&o)?,
                project: project(&o)?,
                status: status(&o)?,
                priority: priority(&o)?,
                // "" unassigns; absent leaves it.
                assignee_id: if o.contains_key("assignee_id") {
                    Some(member_id(&o)?.filter(|id| !id.is_empty()))
                } else {
                    None
                },
                ..Default::default()
            };
            if patch.is_empty() {
                return Err(Failure::Invalid);
            }
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let task = crate::tasks::client::patch(&org, &task, &patch).or_else(unavailable)?;
                Ok(json!({"task": task_json(&task, DETAIL_DESCRIPTION_CHARS)}))
            }))
        }
        "tasks.comment" => {
            let o = object(params, &["task", "body"])?;
            let task = task_ref(&o)?;
            let body = text(&o, "body", MAX_COMMENT_CHARS)?
                .filter(|body| !body.trim().is_empty())
                .ok_or(Failure::Invalid)?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let comment = crate::tasks::client::add_comment(
                    &org,
                    &task,
                    &dragent_tasks::NewComment { body, by_agent: false },
                )
                .or_else(unavailable)?;
                Ok(json!({"comment": comment_json(&comment, &expected.user_id)}))
            }))
        }
        "tasks.members" => {
            object(params, &[])?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let members = crate::tasks::client::members(&org).or_else(unavailable)?;
                let members: Vec<Value> = members
                    .iter()
                    .take(500)
                    .map(|member| json!({"id": member.id, "name": member.display()}))
                    .collect();
                Ok(json!({"members": members}))
            }))
        }
        "tasks.delete" | "tasks.restore" => {
            let o = object(params, &["task"])?;
            let task = task_ref(&o)?;
            let restore = method == "tasks.restore";
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                if restore {
                    let task = crate::tasks::client::restore(&org, &task).or_else(unavailable)?;
                    Ok(json!({"task": task_json(&task, DETAIL_DESCRIPTION_CHARS)}))
                } else {
                    crate::tasks::client::delete(&org, &task).or_else(unavailable)?;
                    Ok(json!({}))
                }
            }))
        }
        "tasks.comment_edit" => {
            let o = object(params, &["task", "comment", "body"])?;
            let task = task_ref(&o)?;
            let comment = plain_id(&o, "comment")?;
            let body = text(&o, "body", MAX_COMMENT_CHARS)?
                .filter(|body| !body.trim().is_empty())
                .ok_or(Failure::Invalid)?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let comment = crate::tasks::client::edit_comment(&org, &task, &comment, &body).or_else(unavailable)?;
                Ok(json!({"comment": comment_json(&comment, &expected.user_id)}))
            }))
        }
        "tasks.comment_delete" => {
            let o = object(params, &["task", "comment"])?;
            let task = task_ref(&o)?;
            let comment = plain_id(&o, "comment")?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                crate::tasks::client::delete_comment(&org, &task, &comment).or_else(unavailable)?;
                Ok(json!({}))
            }))
        }
        "tasks.link" => {
            let o = object(params, &["task", "url", "item", "title"])?;
            let task = task_ref(&o)?;
            let url = text(&o, "url", MAX_URL_CHARS)?;
            if url.as_deref().is_some_and(|url| !url.trim().is_empty() && !url.trim().starts_with("https://") && !url.trim().starts_with("http://")) {
                return Err(Failure::Invalid);
            }
            let args = json!({
                "url": url,
                "item": text(&o, "item", 200)?,
                "title": text(&o, "title", MAX_TITLE_CHARS)?,
            });
            let link = crate::tasks::client::parse_link(&args, false).map_err(|_| Failure::Invalid)?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                crate::tasks::client::add_link(&org, &task, &link).or_else(unavailable)?;
                let detail = crate::tasks::client::detail(&org, &task).or_else(unavailable)?;
                Ok(json!({"task": task_json(&detail.task, DETAIL_DESCRIPTION_CHARS)}))
            }))
        }
        "tasks.unlink" => {
            let o = object(params, &["task", "link"])?;
            let task = task_ref(&o)?;
            let link = plain_id(&o, "link")?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                crate::tasks::client::delete_link(&org, &task, &link).or_else(unavailable)?;
                let detail = crate::tasks::client::detail(&org, &task).or_else(unavailable)?;
                Ok(json!({"task": task_json(&detail.task, DETAIL_DESCRIPTION_CHARS)}))
            }))
        }
        "tasks.options" => {
            object(params, &[])?;
            Ok(Box::new(move || {
                let org = organisation(&expected)?;
                let members: Vec<Value> = crate::tasks::client::members(&org)
                    .unwrap_or_default()
                    .iter()
                    .take(500)
                    .map(|member| json!({"id": member.id, "name": member.display()}))
                    .collect();
                let labels: Vec<String> = crate::tasks::client::labels(&org)
                    .unwrap_or_default()
                    .into_iter()
                    .take(500)
                    .collect();
                let mut options = super::companion_starts::start_options(&expected)?;
                options["members"] = json!(members);
                options["labels"] = json!(labels);
                Ok(options)
            }))
        }
        _ => Err(Failure::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> AccountKey {
        AccountKey { user_id: "u".into(), org: "org-1".into() }
    }

    #[test]
    fn requests_name_their_fields_exactly() {
        let refused = |method: &str, params: Value| job(&account(), method, &params).is_err();
        assert!(refused("tasks.list", json!({"status": "everything"})));
        assert!(refused("tasks.list", json!({"limit": 5})));
        assert!(refused("tasks.get", json!({})));
        assert!(refused("tasks.get", json!({"task": "../etc"})));
        assert!(refused("tasks.create", json!({"title": "   "})));
        assert!(refused("tasks.create", json!({"title": "x", "status": "sideways"})));
        assert!(refused("tasks.update", json!({"task": "DRA-1"})), "an empty change");
        assert!(refused("tasks.comment", json!({"task": "DRA-1", "body": ""})));
        assert!(refused("tasks.members", json!({"x": 1})));
        assert!(refused("tasks.update", json!({"task": "DRA-1", "title": "  "})), "a blank title");
        assert!(refused("tasks.update", json!({"task": "DRA-1", "labels": "bug"})));
        assert!(refused("tasks.update", json!({"task": "DRA-1", "project": "../x"})));
        assert!(refused("tasks.comment_edit", json!({"task": "DRA-1", "comment": "c/1", "body": "x"})));
        assert!(refused("tasks.link", json!({"task": "DRA-1"})), "nothing to link");
        assert!(refused("tasks.link", json!({"task": "DRA-1", "url": "javascript:alert(1)"})));
        assert!(refused("tasks.purge", json!({"task": "DRA-1"})));
        let ok = |method: &str, params: Value| job(&account(), method, &params).is_ok();
        assert!(ok("tasks.list", json!({"status": "closed", "q": "login"})));
        assert!(ok("tasks.list", json!({"assignee": "none", "label": "bug"})));
        assert!(ok("tasks.list", json!({"assignee": "user-123"})), "a member");
        assert!(refused("tasks.list", json!({"assignee": "a b"})));
        assert!(ok("tasks.get", json!({"task": "DRA-12"})));
        assert!(ok("tasks.create", json!({"title": "Fix it", "priority": "high"})));
        assert!(ok("tasks.update", json!({"task": "DRA-1", "status": "in_progress"})));
        assert!(ok("tasks.update", json!({"task": "DRA-1", "assignee_id": ""})), "unassign");
        assert!(ok("tasks.comment", json!({"task": "DRA-1", "body": "Looks good"})));
        assert!(ok("tasks.members", json!({})));
        assert!(ok("tasks.update", json!({"task": "DRA-1", "title": "New", "labels": ["Bug", "bug"], "due_at": null})));
        assert!(ok("tasks.update", json!({"task": "DRA-1", "project": "github.com/acme/app"})));
        assert!(ok("tasks.update", json!({"task": "DRA-1", "project": ""})), "clear the project");
        assert!(ok("tasks.delete", json!({"task": "DRA-1"})));
        assert!(ok("tasks.restore", json!({"task": "DRA-1"})));
        assert!(ok("tasks.comment_edit", json!({"task": "DRA-1", "comment": "c1", "body": "fixed"})));
        assert!(ok("tasks.comment_delete", json!({"task": "DRA-1", "comment": "c1"})));
        assert!(ok("tasks.link", json!({"task": "DRA-1", "url": "https://github.com/a/b/pull/1"})));
        assert!(ok("tasks.link", json!({"task": "DRA-1", "item": "linear:ENG-12"})));
        assert!(ok("tasks.unlink", json!({"task": "DRA-1", "link": "l1"})));
        assert!(ok("tasks.options", json!({})));
    }

    #[test]
    fn a_task_reaches_the_phone_as_a_bounded_projection() {
        let task = dragent_tasks::Task {
            id: "t1".into(),
            key: "DRA-1".into(),
            number: 1,
            title: "Login".into(),
            description: "x".repeat(LIST_DESCRIPTION_CHARS + 50),
            status: dragent_tasks::Status::InProgress,
            priority: dragent_tasks::Priority::High,
            assignee: Some(dragent_tasks::Person { id: "u1".into(), name: String::new(), email: "a@b.c".into() }),
            creator: None,
            project: None,
            labels: vec!["bug".into()],
            due_at_ms: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            started_at_ms: None,
            completed_at_ms: None,
            links: Vec::new(),
            comment_count: 3,
        };
        let value = task_json(&task, LIST_DESCRIPTION_CHARS);
        assert_eq!(value["status"], "in_progress");
        assert_eq!(value["priority"], "high");
        assert_eq!(value["assignee"]["name"], "a@b.c");
        assert_eq!(value["description"].as_str().unwrap().chars().count(), LIST_DESCRIPTION_CHARS);
        assert_eq!(value["description_truncated"], true);
        assert_eq!(value["links"], json!([]));
    }

    #[test]
    fn another_organisation_is_refused() {
        let other = AccountKey { user_id: "u".into(), org: "not-this-desktop".into() };
        let work = job(&other, "tasks.members", &json!({})).unwrap();
        assert_eq!(work().unwrap_err(), Failure::Gone);
    }
}
