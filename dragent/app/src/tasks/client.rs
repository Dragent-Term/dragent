//! One blocking call per Tasks route (`dragent_tasks/API.md`), through the
//! signed-in API client. Errors are plain sentences for people; the command
//! line and MCP tools wrap them into the result contract themselves.

use dragent_tasks::{
    task_path, tasks_path, Comment, LinkKind, ListQuery, ListResponse, NewComment, NewLink,
    NewTask, Person, Priority, ProjectRef, Status, Task, TaskDetail, TaskLink, TaskPatch,
};
use serde_json::json;

/// Read a JSON answer into `T`, refusing an unexpected shape rather than
/// guessing.
fn read<T: serde::de::DeserializeOwned>(path: &str, value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value)
        .map_err(|error| format!("Unexpected tasks response from {path}: {error}"))
}

/// The organisation CLI and MCP calls act in: the live one, else what the
/// app last cached.
pub(crate) fn active_org() -> Result<String, String> {
    crate::cli::live_organisation()
        .or_else(crate::organisation_context::cached_active)
        .map(|active| active.organisation_id)
        .filter(|id| !id.is_empty())
        .ok_or_else(crate::cli::signed_out)
}

/// Whether the command runs inside an agent: its changes are marked
/// `by_agent`, so the task's history shows who really made them.
pub(crate) fn by_agent() -> bool {
    crate::tasks::by_agent()
}

// ── Shared argument parsing (the command line and MCP tools) ────────────────

fn invalid_input(code: &str, message: String, next: &str) -> String {
    dragent_host::outcome::Failure::new(dragent_host::outcome::Status::InvalidInput, code, message)
        .next(next)
        .encode()
}

fn invalid_target(code: &str, message: String, next: &str) -> String {
    dragent_host::outcome::Failure::new(dragent_host::outcome::Status::InvalidTarget, code, message)
        .next(next)
        .encode()
}

/// A status key as people write it.
pub(crate) fn parse_status(key: &str) -> Result<Status, String> {
    Status::from_key(key).ok_or_else(|| {
        invalid_input(
            "unknown_status",
            format!("Unknown status “{key}”."),
            "Use backlog, todo, in_progress, in_review, done or canceled.",
        )
    })
}

/// A priority as people write it.
pub(crate) fn parse_priority(key: &str) -> Result<Priority, String> {
    Priority::from_key(key).ok_or_else(|| {
        invalid_input(
            "unknown_priority",
            format!("Unknown priority “{key}”."),
            "Use urgent, high, medium, low or none.",
        )
    })
}

/// An assignee argument: `me` is the signed-in user, an email names a
/// member, anything else is already a member id (the server refuses
/// strangers).
pub(crate) fn resolve_assignee(org: &str, assignee: &str) -> Result<String, String> {
    if assignee == "me" {
        let id = crate::cli::live_organisation()
            .or_else(crate::organisation_context::cached_active)
            .map(|active| active.user_id)
            .unwrap_or_default();
        return (!id.is_empty()).then_some(id).ok_or_else(|| {
            invalid_input(
                "unknown_user",
                "Couldn't tell who --assignee me is.".into(),
                "Open Dragent and sign in, then run this again.",
            )
        });
    }
    if assignee.contains('@') {
        let wanted = assignee.to_ascii_lowercase();
        let hit = members(org)?
            .into_iter()
            .find(|member| member.email.to_ascii_lowercase() == wanted)
            .map(|member| member.id);
        return hit.ok_or_else(|| {
            invalid_target(
                "unknown_member",
                format!("Nobody in the organisation signs in as {assignee}."),
                "List members in Dragent under Settings → Organisation, or pass their user id.",
            )
        });
    }
    Ok(assignee.to_string())
}

/// A project argument: `here` names this folder's repository (through its
/// git remote, so every checkout of it shares the key); anything else is
/// already a project key, read as its last segment.
pub(crate) fn resolve_project(project: &str) -> Result<ProjectRef, String> {
    if project == "here" {
        let root = std::env::current_dir().unwrap_or_default();
        let remote = git_remote(&root).ok_or_else(|| {
            invalid_input(
                "no_remote",
                format!(
                    "{} has no git remote, so it has no shared project name.",
                    root.display()
                ),
                "Pass --project KEY, or run this in a checkout with a remote.",
            )
        })?;
        let key = dragent_tasks::project_key(&remote).ok_or_else(|| {
            invalid_input(
                "no_remote",
                format!("Couldn't make a project name from the remote “{remote}”."),
                "Pass --project KEY.",
            )
        })?;
        return Ok(ProjectRef {
            label: root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| key.clone()),
            key,
        });
    }
    Ok(ProjectRef {
        label: project.rsplit('/').next().unwrap_or(project).to_string(),
        key: project.to_string(),
    })
}

/// The `origin` remote of the repository `dir` is in, if any.
fn git_remote(dir: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|remote| !remote.is_empty())
}

/// Labels from repeated `--label` flags or a JSON array: trimmed, lowercase,
/// deduplicated, in first-seen order (the server enforces the count).
pub(crate) fn clean_labels(labels: Vec<String>) -> Vec<String> {
    let mut cleaned = Vec::new();
    for label in labels {
        let label = label.trim().to_ascii_lowercase();
        if !label.is_empty() && !cleaned.contains(&label) {
            cleaned.push(label);
        }
    }
    cleaned
}

fn string_list(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A `NewTask` from MCP-style arguments (`title` required).
pub(crate) fn parse_new(args: &serde_json::Value, by_agent: bool) -> Result<NewTask, String> {
    let title = args["title"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| {
            invalid_input(
                "missing_title",
                "A task needs a title.".into(),
                "Say what needs doing in a sentence.",
            )
        })?;
    let status = match args["status"]
        .as_str()
        .filter(|status| !status.trim().is_empty())
    {
        Some(status) => Some(parse_status(status)?),
        None => None,
    };
    let priority = match args["priority"]
        .as_str()
        .filter(|priority| !priority.trim().is_empty())
    {
        Some(priority) => Some(parse_priority(priority)?),
        None => None,
    };
    let project = match args["project"]
        .as_str()
        .filter(|project| !project.trim().is_empty())
    {
        Some(project) => Some(resolve_project(project)?),
        None => None,
    };
    let labels = clean_labels(string_list(&args["labels"]));
    Ok(NewTask {
        title: title.to_string(),
        description: args["description"]
            .as_str()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(str::to_string),
        status,
        priority,
        assignee_id: args["assignee"]
            .as_str()
            .map(str::trim)
            .filter(|assignee| !assignee.is_empty())
            .map(str::to_string),
        project,
        labels: (!labels.is_empty()).then_some(labels),
        due_at_ms: None,
        by_agent,
    })
}

/// A `TaskPatch` from MCP-style arguments: only what's given changes.
pub(crate) fn parse_patch(args: &serde_json::Value, by_agent: bool) -> Result<TaskPatch, String> {
    let mut patch = TaskPatch {
        by_agent,
        ..Default::default()
    };
    if let Some(title) = args["title"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        patch.title = Some(title.to_string());
    }
    if let Some(description) = args["description"].as_str() {
        patch.description = Some(description.to_string());
    }
    if let Some(status) = args["status"]
        .as_str()
        .filter(|status| !status.trim().is_empty())
    {
        patch.status = Some(parse_status(status)?);
    }
    if let Some(priority) = args["priority"]
        .as_str()
        .filter(|priority| !priority.trim().is_empty())
    {
        patch.priority = Some(parse_priority(priority)?);
    }
    if args["clear_assignee"].as_bool() == Some(true) {
        patch.assignee_id = Some(None);
    } else if let Some(assignee) = args["assignee"]
        .as_str()
        .map(str::trim)
        .filter(|assignee| !assignee.is_empty())
    {
        patch.assignee_id = Some(Some(assignee.to_string()));
    }
    if let Some(project) = args["project"]
        .as_str()
        .filter(|project| !project.trim().is_empty())
    {
        patch.project = Some(Some(resolve_project(project)?));
    }
    if args["labels"].is_array() {
        patch.labels = Some(clean_labels(string_list(&args["labels"])));
    }
    Ok(patch)
}

/// A `NewLink` from MCP-style arguments: a pull request address or a
/// `PROVIDER:KEY` tracker item.
pub(crate) fn parse_link(args: &serde_json::Value, by_agent: bool) -> Result<NewLink, String> {
    let (kind, reference) = match (
        args["url"]
            .as_str()
            .map(str::trim)
            .filter(|url| !url.is_empty()),
        args["item"]
            .as_str()
            .map(str::trim)
            .filter(|item| !item.is_empty()),
    ) {
        (Some(_), Some(_)) => {
            return Err(invalid_input(
                "link_conflict",
                "Link a pull request or a tracker item, not both.".into(),
                "Pass --pr URL or --item PROVIDER:KEY.",
            ));
        }
        (Some(url), None) => (LinkKind::PullRequest, url.to_string()),
        (None, Some(item)) => {
            let (provider, key) = item
                .split_once(':')
                .map(|(provider, key)| (provider.trim(), key.trim()))
                .filter(|(provider, key)| !provider.is_empty() && !key.is_empty())
                .ok_or_else(|| {
                    invalid_input(
                        "bad_item",
                        format!("“{item}” isn't PROVIDER:KEY."),
                        "Write it like linear:ENG-12.",
                    )
                })?;
            (LinkKind::Item, dragent_tasks::item_reference(provider, key))
        }
        (None, None) => {
            return Err(invalid_input(
                "missing_link",
                "Nothing to link.".into(),
                "Pass --pr URL or --item PROVIDER:KEY.",
            ));
        }
    };
    Ok(NewLink {
        kind,
        title: args["title"]
            .as_str()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| reference.clone()),
        url: args["url"]
            .as_str()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string),
        reference,
        state: None,
        by_agent,
    })
}
/// The members of `org`, for assignee pickers. The endpoint answers a bare
/// array (`server/internal/organization/handler.go`); `{"members": [...]}` is
/// accepted too. It carries no names, so people read as their email.
pub(crate) fn members(org: &str) -> Result<Vec<Person>, String> {
    let path = format!("/api/v1/organizations/{org}/members");
    let value = crate::api::API_CLIENT
        .get(&path, 20)
        .map_err(|error| error.user_message())?;
    let list = match value {
        serde_json::Value::Array(_) => value,
        serde_json::Value::Object(mut map) => map.remove("members").unwrap_or_default(),
        _ => serde_json::Value::Null,
    };
    if list.is_null() {
        return Ok(Vec::new());
    }
    #[derive(serde::Deserialize)]
    struct Member {
        user_id: String,
        #[serde(default)]
        email: String,
    }
    let members: Vec<Member> = serde_json::from_value(list)
        .map_err(|error| format!("Unexpected members response: {error}"))?;
    Ok(members
        .into_iter()
        .map(|member| Person {
            id: member.user_id,
            name: String::new(),
            email: member.email,
        })
        .collect())
}

/// A page of the organisation's tasks.
pub(crate) fn list(org: &str, query: &ListQuery) -> Result<ListResponse, String> {
    let path = format!("{}{}", tasks_path(org), query.query_string());
    let value = crate::api::API_CLIENT
        .get(&path, 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// A new task.
pub(crate) fn create(org: &str, new: &NewTask) -> Result<Task, String> {
    let path = tasks_path(org);
    let body = serde_json::to_value(new).map_err(|error| error.to_string())?;
    let value = crate::api::API_CLIENT
        .post(&path, &body, 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// A task with its comments and history, by id or key (`DRA-12`).
pub(crate) fn detail(org: &str, id_or_key: &str) -> Result<TaskDetail, String> {
    let path = task_path(org, id_or_key);
    let value = crate::api::API_CLIENT
        .get(&path, 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Change only the fields `patch` carries.
pub(crate) fn patch(org: &str, id_or_key: &str, patch: &TaskPatch) -> Result<Task, String> {
    let path = task_path(org, id_or_key);
    let body = serde_json::to_value(patch).map_err(|error| error.to_string())?;
    let value = crate::api::API_CLIENT
        .patch(&path, &body, 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Soft-delete a task; it leaves every list until restored.
pub(crate) fn delete(org: &str, id_or_key: &str) -> Result<(), String> {
    let path = task_path(org, id_or_key);
    crate::api::API_CLIENT
        .delete(&path, 20)
        .map_err(|error| error.user_message())?;
    Ok(())
}

/// Bring back a deleted task.
pub(crate) fn restore(org: &str, id_or_key: &str) -> Result<Task, String> {
    let path = format!("{}/restore", task_path(org, id_or_key));
    let value = crate::api::API_CLIENT
        .post(&path, &json!({}), 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Comment on a task.
pub(crate) fn add_comment(
    org: &str,
    id_or_key: &str,
    comment: &NewComment,
) -> Result<Comment, String> {
    let path = format!("{}/comments", task_path(org, id_or_key));
    let body = serde_json::to_value(comment).map_err(|error| error.to_string())?;
    let value = crate::api::API_CLIENT
        .post(&path, &body, 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Change the body of one's own comment.
pub(crate) fn edit_comment(
    org: &str,
    id_or_key: &str,
    comment_id: &str,
    body: &str,
) -> Result<Comment, String> {
    let path = format!("{}/comments/{comment_id}", task_path(org, id_or_key));
    let value = crate::api::API_CLIENT
        .patch(&path, &json!({ "body": body }), 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Delete a comment (one's own, or any as an admin).
pub(crate) fn delete_comment(org: &str, id_or_key: &str, comment_id: &str) -> Result<(), String> {
    let path = format!("{}/comments/{comment_id}", task_path(org, id_or_key));
    crate::api::API_CLIENT
        .delete(&path, 20)
        .map_err(|error| error.user_message())?;
    Ok(())
}

/// Link a workspace, pull request or tracker item. Posting the same kind +
/// reference again updates that link instead of adding another.
pub(crate) fn add_link(org: &str, id_or_key: &str, link: &NewLink) -> Result<TaskLink, String> {
    let path = format!("{}/links", task_path(org, id_or_key));
    let body = serde_json::to_value(link).map_err(|error| error.to_string())?;
    let value = crate::api::API_CLIENT
        .post(&path, &body, 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Change a link's title, address or state; only what's given changes.
pub(crate) fn patch_link(
    org: &str,
    id_or_key: &str,
    link_id: &str,
    title: Option<&str>,
    url: Option<&str>,
    state: Option<&str>,
) -> Result<TaskLink, String> {
    let path = format!("{}/links/{link_id}", task_path(org, id_or_key));
    let mut body = serde_json::Map::new();
    if let Some(title) = title {
        body.insert("title".into(), json!(title));
    }
    if let Some(url) = url {
        body.insert("url".into(), json!(url));
    }
    if let Some(state) = state {
        body.insert("state".into(), json!(state));
    }
    let value = crate::api::API_CLIENT
        .patch(&path, &serde_json::Value::Object(body), 20)
        .map_err(|error| error.user_message())?;
    read(&path, value)
}

/// Remove a link from a task.
pub(crate) fn delete_link(org: &str, id_or_key: &str, link_id: &str) -> Result<(), String> {
    let path = format!("{}/links/{link_id}", task_path(org, id_or_key));
    crate::api::API_CLIENT
        .delete(&path, 20)
        .map_err(|error| error.user_message())?;
    Ok(())
}

/// Every label in use in the organisation, sorted.
pub(crate) fn labels(org: &str) -> Result<Vec<String>, String> {
    let path = format!("/api/v1/organizations/{org}/task-labels");
    let value = crate::api::API_CLIENT
        .get(&path, 20)
        .map_err(|error| error.user_message())?;
    let labels = value.get("labels").cloned().unwrap_or_default();
    serde_json::from_value(labels).map_err(|error| format!("Unexpected labels response: {error}"))
}
