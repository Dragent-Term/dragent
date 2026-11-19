//! `dragent issues`: the organisation's task list, for people and agents
//! in a terminal. (`dragent tasks` is the agent-coordination plan and is
//! left alone.)
//!
//! Server-backed like the other server commands: it needs sign-in (the
//! dispatcher enforces that), and answers JSON in agent/JSON mode, tables
//! for people. Changes made inside an agent are marked `by_agent`, so the
//! task's history shows who really made them.

use dragent_host::outcome::{Failure, Status as OutcomeStatus};
use dragent_tasks::{ListQuery, NewComment, Priority, Status, Task};
use serde_json::{json, Value};

use super::{pretty, Flags};
use crate::tasks::client;

const HELP: &str = "\
The organisation's task list: what needs doing, who has it, where each stands.

Usage: dragent issues <command>

  list [--status open|closed|all|KEYS] [--assignee me|none|EMAIL]
       [--label L] [--project here|KEY] [--search WORDS]
                                         Tasks, most recently updated first
  show KEY                               The task, its comments and history
  create TITLE [--description TEXT | --description-file PATH]
         [--status S] [--priority P] [--assignee EMAIL|me]
         [--label L]... [--project here|KEY]
                                         File a new task
  update KEY [--title T] [--status S] [--priority P]
         [--assignee EMAIL|me] [--clear-assignee] [--label L]...
         [--description TEXT | --description-file PATH]
                                         Change only what is given
  comment KEY TEXT                       Comment on a task
  link KEY (--pr URL | --item PROVIDER:KEY) [--title T]
                                         Link a pull request or tracker item
  delete KEY                             Delete a task (restorable for 30 days)
  restore KEY                            Bring back a deleted task

KEY is a task key (DRA-12) or id. Statuses: backlog, todo, in_progress,
in_review, done, canceled. Priorities: urgent, high, medium, low, none.";

pub fn run(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let json_out = flags.json || super::presentation::wants_json(args);
    let value = match flags.subcommand() {
        "help" => return Ok(HELP.into()),
        "list" | "ls" => list(&flags)?,
        "show" | "get" => show(&flags)?,
        "create" | "new" => create(&flags)?,
        "update" | "edit" => update(&flags)?,
        "comment" => comment(&flags)?,
        "link" => link(&flags)?,
        "delete" | "remove" => delete(&flags)?,
        "restore" => restore(&flags)?,
        other => {
            return Err(invalid_input(
                "unknown_command",
                format!("unknown issues command “{other}”. Run `dragent issues help`."),
            ));
        }
    };
    Ok(if json_out {
        pretty(value)
    } else {
        text(&flags, &value)
    })
}

fn invalid_input(code: &str, message: String) -> String {
    Failure::new(OutcomeStatus::InvalidInput, code, message)
        .next("`dragent issues help` shows the commands.")
        .encode()
}

fn invalid_target(code: &str, message: String) -> String {
    Failure::new(OutcomeStatus::InvalidTarget, code, message)
        .next("`dragent issues list` shows the current keys.")
        .encode()
}

/// A server answer that isn't one of the above: a refusal or a dropped
/// connection. A missing task reads as a stale key, so agents look it up
/// again instead of retrying blindly.
fn request_error(message: String) -> String {
    if message.to_ascii_lowercase().contains("not found") {
        return invalid_target("no_such_task", message);
    }
    Failure::new(OutcomeStatus::Failed, "request_failed", message)
        .next("Check your connection and try again.")
        .encode()
}

fn key_argument(flags: &Flags) -> Result<String, String> {
    flags.argument(0).map(str::to_string).ok_or_else(|| {
        invalid_input(
            "missing_argument",
            "name the task by its key (DRA-12) or id".into(),
        )
    })
}

/// `--description`, `--description-file` (`-` for stdin), or empty.
fn description(flags: &Flags) -> Result<Option<String>, String> {
    match (flags.value("description"), flags.value("description-file")) {
        (Some(_), Some(_)) => Err(invalid_input(
            "conflicting_arguments",
            "use --description or --description-file, not both".into(),
        )),
        (Some(text), None) => Ok(Some(text.to_string())),
        (None, Some("-")) => {
            let mut text = String::new();
            use std::io::Read as _;
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("couldn't read stdin: {error}"))?;
            Ok(Some(text))
        }
        (None, Some(file)) => std::fs::read_to_string(super::absolute(file))
            .map(Some)
            .map_err(|error| format!("couldn't read {file}: {error}")),
        (None, None) => Ok(None),
    }
}

fn assignee_name(task: &Task) -> String {
    task.assignee
        .as_ref()
        .map(|assignee| assignee.display().to_string())
        .unwrap_or_else(|| "-".into())
}

fn task_line(task: &Task) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}",
        task.key,
        task.status.label(),
        task.priority.label(),
        assignee_name(task),
        task.title,
    )
}

fn task_row(task: &Task) -> Value {
    json!({
        "key": task.key,
        "title": task.title,
        "status": task.status.key(),
        "priority": task.priority.key(),
        "assignee": task.assignee,
        "labels": task.labels,
        "project": task.project,
        "updated_at_ms": task.updated_at_ms,
    })
}

// ── Commands ─────────────────────────────────────────────────────────────────

fn list(flags: &Flags) -> Result<Value, String> {
    let org = client::active_org()?;
    let status = match flags.value("status").map(str::trim) {
        None | Some("") | Some("open") => None,
        Some("closed") => Some("closed".to_string()),
        Some("all") => Some("all".to_string()),
        Some(keys) => {
            for key in keys.split(',') {
                if Status::from_key(key).is_none() {
                    return Err(invalid_input(
                        "unknown_status",
                        format!("Unknown status “{key}”."),
                    ));
                }
            }
            Some(keys.to_string())
        }
    };
    let assignee = match flags.value("assignee").map(str::trim) {
        None | Some("") => None,
        Some("me") | Some("none") => flags.value("assignee").map(str::to_string),
        Some(assignee) => Some(client::resolve_assignee(&org, assignee).map_err(request_error)?),
    };
    let project = match flags.value("project").map(str::trim) {
        None | Some("") => None,
        Some("here") => Some(client::resolve_project("here").map_err(request_error)?.key),
        Some(project) => Some(project.to_string()),
    };
    let mut query = ListQuery {
        status,
        assignee,
        label: flags
            .value("label")
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_string),
        project,
        q: flags
            .value("search")
            .map(str::trim)
            .filter(|search| !search.is_empty())
            .map(str::to_string),
        ..Default::default()
    };
    let mut tasks = Vec::new();
    let mut prefix = String::new();
    loop {
        let answer = client::list(&org, &query).map_err(request_error)?;
        if !answer.prefix.is_empty() {
            prefix = answer.prefix;
        }
        tasks.extend(answer.tasks);
        match answer.next_cursor {
            Some(cursor) if !cursor.is_empty() => query.cursor = Some(cursor),
            _ => break,
        }
    }
    if flags.json {
        return Ok(json!({ "tasks": tasks, "prefix": prefix }));
    }
    Ok(json!({
        "rows": tasks.iter().map(task_row).collect::<Vec<_>>(),
        "empty": "No tasks match.",
        "prefix": prefix,
    }))
}

fn show(flags: &Flags) -> Result<Value, String> {
    let key = key_argument(flags)?;
    let org = client::active_org()?;
    let detail = client::detail(&org, &key).map_err(request_error)?;
    Ok(json!({
        "task": detail.task,
        "comments": detail.comments,
        "activity": detail.activity,
    }))
}

fn create(flags: &Flags) -> Result<Value, String> {
    let title = flags
        .positional
        .get(1..)
        .map(|words| words.join(" ").trim().to_string())
        .filter(|title| !title.is_empty())
        .ok_or_else(|| {
            invalid_input(
                "missing_title",
                "say what needs doing: `dragent issues create TITLE`".into(),
            )
        })?;
    let org = client::active_org()?;
    let mut args = json!({ "title": title });
    if let Some(description) = description(flags)? {
        args["description"] = json!(description);
    }
    for (flag, key) in [
        ("status", "status"),
        ("priority", "priority"),
        ("project", "project"),
    ] {
        if let Some(value) = flags.value(flag) {
            args[key] = json!(value);
        }
    }
    if let Some(assignee) = flags.value("assignee") {
        args["assignee"] =
            json!(client::resolve_assignee(&org, assignee.trim()).map_err(request_error)?);
    }
    let labels = client::clean_labels(flags.all("label"));
    if !labels.is_empty() {
        args["labels"] = json!(labels);
    }
    // Status, priority and project are validated (and `here` resolved)
    // through the shared parsing, so the command line and MCP agree.
    let new = client::parse_new(&args, client::by_agent())?;
    let task = client::create(&org, &new).map_err(request_error)?;
    Ok(json!(task))
}

fn update(flags: &Flags) -> Result<Value, String> {
    let key = key_argument(flags)?;
    // Refuse an empty change before any request, so scripts fail fast.
    if flags.value("title").is_none()
        && flags.value("status").is_none()
        && flags.value("priority").is_none()
        && flags.value("assignee").is_none()
        && !flags.on("clear-assignee")
        && flags.all("label").is_empty()
        && flags.value("description").is_none()
        && flags.value("description-file").is_none()
        && flags.value("project").is_none()
    {
        return Err(invalid_input("nothing_to_change", "say what to change: --title, --status, --priority, --assignee, --label or --description".into()));
    }
    let org = client::active_org()?;
    let mut args = json!({});
    for (flag, key) in [
        ("title", "title"),
        ("status", "status"),
        ("priority", "priority"),
        ("project", "project"),
    ] {
        if let Some(value) = flags.value(flag) {
            args[key] = json!(value);
        }
    }
    if flags.on("clear-assignee") {
        args["clear_assignee"] = json!(true);
    } else if let Some(assignee) = flags.value("assignee") {
        args["assignee"] =
            json!(client::resolve_assignee(&org, assignee.trim()).map_err(request_error)?);
    }
    if let Some(description) = description(flags)? {
        args["description"] = json!(description);
    }
    let labels = flags.all("label");
    if !labels.is_empty() {
        args["labels"] = json!(client::clean_labels(labels));
    }
    let patch = client::parse_patch(&args, client::by_agent())?;
    if patch.is_empty() {
        return Err(invalid_input("nothing_to_change", "say what to change: --title, --status, --priority, --assignee, --label or --description".into()));
    }
    let task = client::patch(&org, &key, &patch).map_err(request_error)?;
    Ok(json!(task))
}

fn comment(flags: &Flags) -> Result<Value, String> {
    let key = key_argument(flags)?;
    let body = flags
        .positional
        .get(2..)
        .map(|words| words.join(" ").trim().to_string())
        .filter(|body| !body.is_empty())
        .ok_or_else(|| {
            invalid_input(
                "missing_comment",
                "say it: `dragent issues comment KEY TEXT`".into(),
            )
        })?;
    let org = client::active_org()?;
    let made = client::add_comment(
        &org,
        &key,
        &NewComment {
            body,
            by_agent: client::by_agent(),
        },
    )
    .map_err(request_error)?;
    Ok(json!(made))
}

fn link(flags: &Flags) -> Result<Value, String> {
    let key = key_argument(flags)?;
    // Refuse a missing or conflicting link before any request.
    let has_pr = flags.value("pr").is_some_and(|url| !url.trim().is_empty());
    let has_item = flags
        .value("item")
        .is_some_and(|item| !item.trim().is_empty());
    if has_pr && has_item {
        return Err(invalid_input(
            "link_conflict",
            "Link a pull request or a tracker item, not both.".into(),
        ));
    }
    if !has_pr && !has_item {
        return Err(invalid_input("missing_link", "Nothing to link.".into()));
    }
    let org = client::active_org()?;
    let mut args = json!({});
    if let Some(url) = flags.value("pr") {
        args["url"] = json!(url);
    }
    if let Some(item) = flags.value("item") {
        args["item"] = json!(item);
    }
    if let Some(title) = flags.value("title") {
        args["title"] = json!(title);
    }
    let link = client::parse_link(&args, client::by_agent())?;
    let made = client::add_link(&org, &key, &link).map_err(request_error)?;
    Ok(json!(made))
}

fn delete(flags: &Flags) -> Result<Value, String> {
    let key = key_argument(flags)?;
    let org = client::active_org()?;
    client::delete(&org, &key).map_err(request_error)?;
    Ok(json!({ "deleted": key }))
}

fn restore(flags: &Flags) -> Result<Value, String> {
    let key = key_argument(flags)?;
    let org = client::active_org()?;
    let task = client::restore(&org, &key).map_err(request_error)?;
    Ok(json!(task))
}

/// Readable text for people; agents get the JSON.
fn text(flags: &Flags, value: &Value) -> String {
    if flags.quiet {
        if let Some(rows) = value["rows"].as_array() {
            return rows
                .iter()
                .filter_map(|row| row["key"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
        }
        return value["key"]
            .as_str()
            .or_else(|| value["deleted"].as_str())
            .unwrap_or_default()
            .to_string();
    }
    if let Some(rows) = value["rows"].as_array() {
        if rows.is_empty() {
            return "No tasks match.".into();
        }
        if let Some(output) = super::presentation::records(rows, "No tasks match.") {
            return output;
        }
        return rows
            .iter()
            .map(|row| {
                format!(
                    "{}\t{}\t{}\t{}\t{}",
                    super::text(row, "key"),
                    row["status"].as_str().unwrap_or("-"),
                    row["priority"].as_str().unwrap_or("-"),
                    row["assignee"]
                        .as_object()
                        .and_then(|assignee| assignee["name"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .or_else(|| assignee["email"].as_str()))
                        .unwrap_or("-"),
                    super::text(row, "title"),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(task) = value.get("task") {
        return show_text(task, value);
    }
    if value.get("deleted").is_some() {
        return format!(
            "Deleted {}. It can be restored within 30 days.",
            value["deleted"].as_str().unwrap_or_default()
        );
    }
    if value.get("body").is_some() {
        return format!(
            "Commented on {}.",
            value["task_id"].as_str().unwrap_or_default()
        );
    }
    if value.get("reference").is_some() {
        return format!("Linked “{}”.", value["title"].as_str().unwrap_or_default());
    }
    // A task (create, update, restore).
    if value.get("key").is_some() {
        let status = value["status"].as_str().unwrap_or_default();
        return format!(
            "{}: {} [{}]",
            value["key"].as_str().unwrap_or_default(),
            value["title"].as_str().unwrap_or_default(),
            status
        );
    }
    pretty(value.clone())
}

fn show_text(task: &Value, value: &Value) -> String {
    let mut out = format!(
        "{}: {}\n[{}] {}",
        task["key"].as_str().unwrap_or_default(),
        task["title"].as_str().unwrap_or_default(),
        task["status"].as_str().unwrap_or_default(),
        task["priority"].as_str().unwrap_or_default(),
    );
    if let Some(description) = task["description"]
        .as_str()
        .filter(|description| !description.trim().is_empty())
    {
        out.push_str(&format!("\n\n{description}"));
    }
    let comments = value["comments"].as_array().cloned().unwrap_or_default();
    if !comments.is_empty() {
        out.push_str(&format!("\n\nComments ({})", comments.len()));
        for comment in &comments {
            out.push_str(&format!(
                "\n- {}: {}",
                comment["author"]
                    .as_object()
                    .and_then(|author| author["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .or_else(|| author["email"].as_str()))
                    .unwrap_or("Someone"),
                comment["body"].as_str().unwrap_or_default(),
            ));
        }
    }
    let activity = value["activity"].as_array().cloned().unwrap_or_default();
    if !activity.is_empty() {
        out.push_str(&format!(
            "\n\nHistory ({activity_len})",
            activity_len = activity.len()
        ));
        for entry in activity.iter().take(10) {
            out.push_str(&format!(
                "\n- {}",
                entry["kind"].as_str().unwrap_or_default()
            ));
        }
        if activity.len() > 10 {
            out.push_str(&format!("\n- …and {} more", activity.len() - 10));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn help_names_every_command() {
        assert!(HELP.contains("Usage: dragent issues <command>"));
        for command in [
            "list", "show", "create", "update", "comment", "link", "delete", "restore",
        ] {
            assert!(HELP.contains(&format!("\n  {command} ")), "{command}");
        }
    }

    #[test]
    fn unknown_commands_and_missing_keys_are_refused_before_any_request() {
        let failure = Failure::from_error(&run(&args(&["fly"])).unwrap_err());
        assert_eq!(failure.status, OutcomeStatus::InvalidInput);
        let missing = Failure::from_error(&run(&args(&["show"])).unwrap_err());
        assert_eq!(missing.code, "missing_argument");
        let nothing = Failure::from_error(&run(&args(&["update", "DRA-1"])).unwrap_err());
        assert_eq!(nothing.code, "nothing_to_change");
    }

    #[test]
    fn bad_statuses_and_conflicting_links_are_refused_before_any_request() {
        let status = Failure::from_error(&run(&args(&["list", "--status", "bogus"])).unwrap_err());
        assert_eq!(status.code, "unknown_status");
        let link = Failure::from_error(
            &run(&args(&[
                "link",
                "DRA-1",
                "--pr",
                "https://example.test/pr/1",
                "--item",
                "linear:ENG-1",
            ]))
            .unwrap_err(),
        );
        assert_eq!(link.code, "link_conflict");
        let missing = Failure::from_error(&run(&args(&["link", "DRA-1"])).unwrap_err());
        assert_eq!(missing.code, "missing_link");
    }

    #[test]
    fn statuses_priorities_and_labels_parse_the_way_people_write_them() {
        assert_eq!(
            client::parse_status("In progress").unwrap(),
            Status::InProgress
        );
        assert!(client::parse_status("bogus").is_err());
        assert_eq!(client::parse_priority("2").unwrap(), Priority::High);
        assert_eq!(
            client::clean_labels(vec![" Bug ".into(), "bug".into(), "UI".into()]),
            vec!["bug".to_string(), "ui".to_string()]
        );
        let link = client::parse_link(&json!({"item": "linear:ENG-12"}), false).unwrap();
        assert_eq!(link.reference, "linear:ENG-12");
        assert_eq!(link.title, "linear:ENG-12");
        assert!(client::parse_link(&json!({}), false).is_err());
    }
}
