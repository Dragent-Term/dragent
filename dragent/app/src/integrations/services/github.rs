//! GitHub, through the GitHub CLI (its sign-in, its rate limits).

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use serde_json::Value;

use crate::integrations::items::{branch_for, clip, updated_ms, with_comments, Draft, Item, ItemState, StateCategory};
use crate::integrations::{Provider, Source, Status};
use crate::process_ext::NoWindow;

pub(in crate::integrations) fn cli_status() -> Status {
    let output = crate::process_ext::gh_command()
        .args(["api", "user", "--jq", ".login"])
        .env("GH_PROMPT_DISABLED", "1")
        .no_window()
        .output();
    match output {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Status::Unavailable(
            "Install the GitHub CLI (gh) to link GitHub issues and pull requests.".into(),
        ),
        Err(error) => Status::Unavailable(format!("Couldn't run the GitHub CLI: {error}")),
        Ok(output) if output.status.success() => Status::Connected {
            account: Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
                .filter(|login| !login.is_empty()),
            source: Source::GithubCli,
        },
        Ok(output) => {
            let detail = String::from_utf8_lossy(&output.stderr);
            if detail.contains("auth login") || detail.contains("not logged") {
                Status::Unavailable(
                    "The GitHub CLI isn't signed in. Run gh auth login in a terminal.".into(),
                )
            } else {
                Status::Unavailable(format!("The GitHub CLI said: {}", detail.trim()))
            }
        }
    }
}

pub(in crate::integrations) fn search(root: &Path, query: &str) -> Result<Vec<Item>, String> {
    let mut command = crate::process_ext::gh_command();
    command.args([
        "issue",
        "list",
        "--limit",
        "40",
        "--state",
        "open",
        "--json",
        "number,title,url,body,labels,state,assignees,updatedAt",
    ]);
    if !query.is_empty() {
        command.args(["--search", query]);
    }
    let output = command
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .no_window()
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                "Install the GitHub CLI (gh) to search GitHub issues.".to_string()
            }
            _ => format!("Couldn't run the GitHub CLI: {error}"),
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("GitHub search failed: {}", detail.trim()));
    }
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("GitHub returned invalid data: {error}"))?;
    Ok(parse(&rows))
}

/// Run the GitHub CLI in `root` (its repository), feeding `input` on
/// stdin; what it printed.
fn gh(root: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut child = crate::process_ext::gh_command()
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .no_window()
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                "Install the GitHub CLI (gh) to use GitHub issues.".to_string()
            }
            _ => format!("Couldn't run the GitHub CLI: {error}"),
        })?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| format!("Couldn't hand the text to the GitHub CLI: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("The GitHub CLI didn't finish: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("The GitHub CLI said: {}", detail.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// An issue's number from "#12" or "12", or its address as given.
fn issue(id: &str) -> Result<String, String> {
    let id = id.trim();
    let number = id.trim_start_matches('#');
    if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
        Ok(number.to_string())
    } else if id.starts_with("https://") {
        Ok(id.to_string())
    } else {
        Err(format!("GitHub issues are numbers like #12, not “{id}”."))
    }
}

pub(in crate::integrations) fn read(root: &Path, id: &str) -> Result<Item, String> {
    let output = gh(
        root,
        &[
            "issue",
            "view",
            &issue(id)?,
            "--json",
            "number,title,url,body,labels,state,assignees,updatedAt,comments",
        ],
        None,
    )?;
    let row: Value = serde_json::from_str(&output)
        .map_err(|error| format!("GitHub returned invalid data: {error}"))?;
    let mut item = parse(std::slice::from_ref(&row))
        .pop()
        .ok_or_else(|| format!("GitHub has no issue {id}."))?;
    let comments: Vec<(String, String)> = row["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| {
            (
                comment["author"]["login"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                comment["body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    item.body = with_comments(row["body"].as_str().unwrap_or_default(), &comments);
    let state = row["state"].as_str().map(str::to_lowercase);
    item.detail = super::joined(&[state.as_deref(), item.detail.as_deref()]);
    Ok(item)
}

pub(in crate::integrations) fn comment(root: &Path, id: &str, text: &str) -> Result<(), String> {
    gh(
        root,
        &["issue", "comment", &issue(id)?, "--body-file", "-"],
        Some(text),
    )
    .map(|_| ())
}

pub(in crate::integrations) fn set_status(
    root: &Path,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let open = super::open_or_closed(status).ok_or("GitHub issues are either open or closed.")?;
    let number = issue(id)?;
    if open {
        gh(root, &["issue", "reopen", &number], None)?;
        Ok("open".into())
    } else {
        gh(root, &["issue", "close", &number], None)?;
        Ok("closed".into())
    }
}

pub(in crate::integrations) fn create(root: &Path, draft: &Draft) -> Result<Item, String> {
    let mut args = vec![
        "issue",
        "create",
        "--title",
        draft.title.as_str(),
        "--body-file",
        "-",
    ];
    if let Some(repository) = draft.place.as_deref() {
        args.extend(["--repo", repository]);
    }
    let output = gh(root, &args, Some(&draft.body))?;
    // It prints the new issue's address last.
    let url = output
        .lines()
        .rev()
        .find(|line| line.starts_with("https://"))
        .ok_or("GitHub didn't say where the new issue is.")?
        .trim()
        .to_string();
    let number = url.rsplit('/').next().unwrap_or_default().to_string();
    Ok(Item {
        provider: Provider::Github,
        key: format!("#{number}"),
        branch: branch_for(&number, &draft.title),
        title: draft.title.clone(),
        body: draft.body.clone(),
        detail: Some("open".into()),
        reference: None,
        url,
        state: Some(ItemState::open("Open")),
        assignee: None,
        labels: Vec::new(),
        updated_at_ms: None,
    })
}

fn parse(rows: &[Value]) -> Vec<Item> {
    rows.iter()
        .filter_map(|row| {
            let number = row["number"].as_u64()?;
            let title = row["title"].as_str()?.to_string();
            let labels: Vec<String> = row["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label["name"].as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Github,
                key: format!("#{number}"),
                branch: branch_for(&number.to_string(), &title),
                url: row["url"].as_str()?.to_string(),
                body: clip(row["body"].as_str().unwrap_or_default()),
                detail: (!labels.is_empty()).then(|| labels.join(", ")),
                reference: None,
                title,
                state: state_of(row),
                assignee: row["assignees"]
                    .as_array()
                    .and_then(|assignees| assignees.first())
                    .and_then(|assignee| assignee["login"].as_str())
                    .map(str::to_string),
                labels,
                updated_at_ms: updated_ms(&row["updatedAt"]),
            })
        })
        .collect()
}

/// Open or closed from what the CLI reports: issues send `state`, pull
/// requests also send whether they're a draft or already merged.
fn state_of(row: &Value) -> Option<ItemState> {
    if row["mergedAt"].as_str().is_some_and(|merged| !merged.is_empty())
        || row["merged"].as_bool() == Some(true)
    {
        return Some(ItemState::done("Merged"));
    }
    if row["isDraft"].as_bool() == Some(true) {
        return Some(ItemState::started("Draft"));
    }
    match row["state"].as_str() {
        Some("OPEN") => Some(ItemState::open("Open")),
        Some("CLOSED") => Some(ItemState::done("Closed")),
        Some("MERGED") => Some(ItemState::done("Merged")),
        Some(other) => Some(ItemState::new(StateCategory::from_name(other), other)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn issues_read_with_their_labels() {
        let items = parse(&[
            json!({"number": 9, "title": "Crash on start", "url": "https://github.com/a/b/issues/9", "body": "Steps", "labels": [{"name": "bug"}], "state": "OPEN", "assignees": [{"login": "ada"}], "updatedAt": "2026-10-24T12:00:00Z"}),
        ]);
        assert_eq!(
            (items[0].key.as_str(), items[0].branch.as_str()),
            ("#9", "9-crash-on-start")
        );
        assert_eq!(items[0].detail.as_deref(), Some("bug"));
        assert_eq!(items[0].state, Some(ItemState::open("Open")));
        assert_eq!(items[0].assignee.as_deref(), Some("ada"));
        assert_eq!(items[0].labels, vec!["bug"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
    }

    #[test]
    fn states_cover_drafts_and_merges() {
        let open = parse(&[json!({"number": 1, "title": "T", "url": "u", "state": "OPEN"})]);
        assert_eq!(open[0].state, Some(ItemState::open("Open")));
        let draft = parse(&[json!({"number": 2, "title": "T", "url": "u", "state": "OPEN", "isDraft": true})]);
        assert_eq!(draft[0].state, Some(ItemState::started("Draft")));
        let merged = parse(&[json!({"number": 3, "title": "T", "url": "u", "state": "MERGED", "mergedAt": "2026-10-24T12:00:00Z"})]);
        assert_eq!(merged[0].state, Some(ItemState::done("Merged")));
        let closed = parse(&[json!({"number": 4, "title": "T", "url": "u", "state": "CLOSED"})]);
        assert_eq!(closed[0].state, Some(ItemState::done("Closed")));
    }
}
