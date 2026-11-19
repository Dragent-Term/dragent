//! Linear's GraphQL API, with a personal API key.

use serde_json::{json, Value};

use crate::integrations::items::{branch_for, choose, clip, updated_ms, with_comments, Draft, Item, ItemState, StateCategory, LIMIT};
use crate::integrations::{http, Connection, Provider};

fn query(connection: &Connection, query: &str, variables: Value) -> Result<Value, String> {
    connection.send(
        http()?
            .post("https://api.linear.app/graphql")
            .json(&json!({"query": query, "variables": variables})),
    )
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let data = query(connection, "query { viewer { name email } }", json!({}))?;
    let viewer = &data["data"]["viewer"];
    Ok(super::named(
        viewer["name"].as_str(),
        viewer["email"].as_str(),
    ))
}

const FIELDS: &str = "identifier title description url branchName updatedAt team { name } state { type name } assignee { name } labels { nodes { name } }";

pub(super) fn search(connection: &Connection, term: &str) -> Result<Vec<Item>, String> {
    let (field, data) = if term.is_empty() {
        // Open issues, most recently touched first.
        let open = "filter: { state: { type: { nin: [\"completed\", \"canceled\"] } } }";
        let data = query(
            connection,
            &format!("query Issues {{ issues(first: {LIMIT}, orderBy: updatedAt, {open}) {{ nodes {{ {FIELDS} }} }} }}"),
            json!({}),
        )?;
        ("issues", data)
    } else {
        let data = query(
            connection,
            &format!("query Search($term: String!) {{ searchIssues(term: $term, first: {LIMIT}) {{ nodes {{ {FIELDS} }} }} }}"),
            json!({"term": term}),
        )?;
        ("searchIssues", data)
    };
    Ok(parse(&data["data"][field]["nodes"]))
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let data = query(
        connection,
        &format!("query Issue($id: String!) {{ issue(id: $id) {{ identifier title description url branchName team {{ name }} state {{ name }} comments(first: 50) {{ nodes {{ body createdAt user {{ name }} }} }} }} }}"),
        json!({"id": id}),
    )?;
    let issue = &data["data"]["issue"];
    let mut item = parse(&Value::Array(vec![issue.clone()]))
        .pop()
        .ok_or_else(|| format!("Linear has no issue {id}."))?;
    let mut comments: Vec<(&str, String, String)> = issue["comments"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| {
            (
                comment["createdAt"].as_str().unwrap_or_default(),
                comment["user"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                comment["body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    comments.sort_by(|a, b| a.0.cmp(b.0));
    let comments: Vec<(String, String)> = comments
        .into_iter()
        .map(|(_, author, text)| (author, text))
        .collect();
    item.body = with_comments(issue["description"].as_str().unwrap_or_default(), &comments);
    Ok(item)
}

/// Linear reports a mutation's outcome in `success`.
fn succeeded(result: &Value, what: &str) -> Result<(), String> {
    if result["success"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(format!("Linear didn't {what}."))
    }
}

/// The issue's own id, which mutations take, and its team's statuses.
fn issue_and_states(
    connection: &Connection,
    id: &str,
) -> Result<(String, Vec<(String, String)>), String> {
    let data = query(
        connection,
        "query Issue($id: String!) { issue(id: $id) { id team { states { nodes { id name } } } } }",
        json!({"id": id}),
    )?;
    let issue = &data["data"]["issue"];
    let uuid = issue["id"]
        .as_str()
        .ok_or_else(|| format!("Linear has no issue {id}."))?
        .to_string();
    let states = issue["team"]["states"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|state| {
            Some((
                state["id"].as_str()?.to_string(),
                state["name"].as_str()?.to_string(),
            ))
        })
        .collect();
    Ok((uuid, states))
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    let (issue, _) = issue_and_states(connection, id)?;
    let data = query(
        connection,
        "mutation Comment($input: CommentCreateInput!) { commentCreate(input: $input) { success } }",
        json!({"input": {"issueId": issue, "body": text}}),
    )?;
    succeeded(&data["data"]["commentCreate"], "add the comment")
}

pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let (issue, states) = issue_and_states(connection, id)?;
    let (state, name) = choose(status, &states, "statuses of this issue's team")?;
    let data = query(
        connection,
        "mutation Move($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }",
        json!({"id": issue, "input": {"stateId": state}}),
    )?;
    succeeded(&data["data"]["issueUpdate"], "change the status")?;
    Ok(name.clone())
}

/// In the team `draft.place` names (its key or name), or the account's
/// only team.
pub(super) fn create(connection: &Connection, draft: &Draft) -> Result<Item, String> {
    let data = query(
        connection,
        "query { teams { nodes { id key name } } }",
        json!({}),
    )?;
    let teams: Vec<(String, String, String)> = data["data"]["teams"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|team| {
            Some((
                team["id"].as_str()?.to_string(),
                team["key"].as_str()?.to_string(),
                team["name"].as_str()?.to_string(),
            ))
        })
        .collect();
    let team = match draft.place.as_deref() {
        Some(place) => teams.iter().find(|(_, key, name)| {
            key.eq_ignore_ascii_case(place) || name.eq_ignore_ascii_case(place)
        }),
        None if teams.len() == 1 => teams.first(),
        None => None,
    }
    .ok_or_else(|| {
        let keys: Vec<&str> = teams.iter().map(|(_, key, _)| key.as_str()).collect();
        format!("Say which Linear team it's for: {}.", keys.join(", "))
    })?;
    let data = query(
        connection,
        &format!("mutation Create($input: IssueCreateInput!) {{ issueCreate(input: $input) {{ success issue {{ {FIELDS} }} }} }}"),
        json!({"input": {"teamId": team.0, "title": draft.title, "description": draft.body}}),
    )?;
    let created = &data["data"]["issueCreate"];
    succeeded(created, "create the issue")?;
    parse(&Value::Array(vec![created["issue"].clone()]))
        .pop()
        .ok_or_else(|| "Linear didn't say which issue it created.".to_string())
}

fn parse(nodes: &Value) -> Vec<Item> {
    nodes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let key = row["identifier"].as_str()?.to_string();
            let title = row["title"].as_str()?.to_string();
            let branch = row["branchName"]
                .as_str()
                .filter(|branch| !branch.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| branch_for(&key.to_lowercase(), &title));
            let labels: Vec<String> = row["labels"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label["name"].as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Linear,
                url: row["url"].as_str()?.to_string(),
                body: clip(row["description"].as_str().unwrap_or_default()),
                detail: super::joined(&[
                    row["team"]["name"].as_str(),
                    row["state"]["name"].as_str(),
                ]),
                reference: None,
                key,
                title,
                branch,
                state: row["state"]["name"].as_str().map(|name| {
                    let category = match row["state"]["type"].as_str() {
                        Some("backlog") | Some("unstarted") => StateCategory::Open,
                        Some("started") => StateCategory::Started,
                        Some("completed") => StateCategory::Done,
                        Some("canceled") => StateCategory::Canceled,
                        _ => StateCategory::from_name(name),
                    };
                    ItemState::new(category, name)
                }),
                assignee: row["assignee"]["name"].as_str().map(str::to_string),
                labels,
                updated_at_ms: updated_ms(&row["updatedAt"]),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_keep_linears_branch() {
        let items = parse(&json!([
            {"identifier": "ENG-12", "title": "Retry uploads", "url": "https://linear.app/a/issue/ENG-12", "description": "Body", "branchName": "ada/eng-12-retry-uploads", "team": {"name": "Engineering"}, "state": {"type": "started", "name": "In Progress"}, "assignee": {"name": "Ada"}, "labels": {"nodes": [{"name": "backend"}]}, "updatedAt": "2026-10-24T12:00:00.000Z"},
            {"identifier": "ENG-13", "title": "No branch", "url": "https://linear.app/a/issue/ENG-13", "branchName": ""}
        ]));
        assert_eq!(items[0].branch, "ada/eng-12-retry-uploads");
        assert_eq!(items[0].detail.as_deref(), Some("Engineering · In Progress"));
        assert_eq!(items[0].state, Some(ItemState::new(StateCategory::Started, "In Progress")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].labels, vec!["backend"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
        assert_eq!(items[1].branch, "eng-13-no-branch");
        assert_eq!(items[1].state, None);
    }

    #[test]
    fn state_types_map_to_categories() {
        for (kind, name, category) in [
            ("backlog", "Backlog", StateCategory::Open),
            ("unstarted", "Todo", StateCategory::Open),
            ("started", "In Progress", StateCategory::Started),
            ("completed", "Done", StateCategory::Done),
            ("canceled", "Canceled", StateCategory::Canceled),
        ] {
            let items = parse(&json!([{"identifier": "ENG-1", "title": "T", "url": "u", "state": {"type": kind, "name": name}}]));
            assert_eq!(items[0].state, Some(ItemState::new(category, name)), "{kind}");
        }
    }
}
