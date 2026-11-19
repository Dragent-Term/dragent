//! Jira Cloud's REST API, with an account email and API token.

use reqwest::Method;
use serde_json::{json, Value};

use crate::integrations::items::{branch_for, choose, clip, updated_ms, with_comments, Draft, Item, ItemState, StateCategory, LIMIT};
use crate::integrations::{http, Connection, Provider};

fn request(
    connection: &Connection,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut builder = http()?
        .request(method, format!("{}{path}", connection.base_url()))
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

fn post(connection: &Connection, path: &str, body: &Value) -> Result<Value, String> {
    request(connection, Method::POST, path, &[], Some(body))
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(connection, "/rest/api/3/myself", &[])?;
    Ok(super::named(
        me["displayName"].as_str(),
        me["emailAddress"].as_str(),
    ))
}

/// The JQL for a search box: an issue key finds that issue, words search
/// text, and nothing lists what's assigned to the user and still open.
fn jql(query: &str) -> String {
    let is_key = query.split_once('-').is_some_and(|(project, number)| {
        !project.is_empty()
            && project
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && project
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    });
    if query.is_empty() {
        "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC".to_string()
    } else if is_key {
        format!("key = \"{}\"", query.to_uppercase())
    } else {
        let escaped = query.replace('\\', "\\\\").replace('"', "\\\"");
        format!("text ~ \"{escaped}\" ORDER BY updated DESC")
    }
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    let jql = jql(query);
    let limit = LIMIT.to_string();
    let data = get(
        connection,
        "/rest/api/3/search/jql",
        &[
            ("jql", jql.as_str()),
            ("fields", "summary,description,status,project,assignee,labels,updated"),
            ("maxResults", limit.as_str()),
        ],
    )?;
    Ok(parse(&data, &connection.base_url()))
}

/// An issue key, upper-cased: Jira's keys are.
fn key(id: &str) -> String {
    id.trim().to_uppercase()
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let key = key(id);
    let row = get(
        connection,
        &format!("/rest/api/3/issue/{key}"),
            &[("fields", "summary,description,status,project,assignee,labels,updated,comment")],
    )?;
    let mut item = parse(&json!({"issues": [row.clone()]}), &connection.base_url())
        .pop()
        .ok_or_else(|| format!("Jira has no issue {key}."))?;
    let comments: Vec<(String, String)> = row["fields"]["comment"]["comments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|comment| {
            (
                comment["author"]["displayName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                document_text(&comment["body"]),
            )
        })
        .collect();
    item.body = with_comments(&document_text(&row["fields"]["description"]), &comments);
    Ok(item)
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    post(
        connection,
        &format!("/rest/api/3/issue/{}/comment", key(id)),
        &json!({"body": document(text)}),
    )
    .map(|_| ())
}

/// Through one of the issue's transitions, named by itself or by the
/// status it leads to.
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let path = format!("/rest/api/3/issue/{}/transitions", key(id));
    let data = get(connection, &path, &[])?;
    let transitions: Vec<(String, String, String)> = data["transitions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|transition| {
            let id = transition["id"].as_str()?.to_string();
            let name = transition["name"].as_str()?.to_string();
            let to = transition["to"]["name"]
                .as_str()
                .unwrap_or(&name)
                .to_string();
            Some((id, name, to))
        })
        .collect();
    let by_status: Vec<(String, String)> = transitions
        .iter()
        .map(|(id, _, to)| (id.clone(), to.clone()))
        .collect();
    let by_name: Vec<(String, String)> = transitions
        .iter()
        .map(|(id, name, _)| (id.clone(), name.clone()))
        .collect();
    let chosen = choose(status, &by_status, "statuses this issue can move to")
        .or_else(|error| choose(status, &by_name, "transitions").map_err(|_| error))?
        .0
        .clone();
    post(connection, &path, &json!({"transition": {"id": chosen}}))?;
    Ok(transitions
        .iter()
        .find(|(id, _, _)| *id == chosen)
        .map(|(_, _, to)| to.clone())
        .unwrap_or_default())
}

/// In the project `draft.place` names (its key), as a Task when the
/// project has them, else its first kind of issue.
pub(super) fn create(connection: &Connection, draft: &Draft) -> Result<Item, String> {
    let project = draft
        .place
        .as_deref()
        .map(key)
        .ok_or("Say which Jira project it's for (its key, like OPS).")?;
    let kinds = get(
        connection,
        &format!("/rest/api/3/issue/createmeta/{project}/issuetypes"),
        &[],
    )
    .ok();
    let kinds: Vec<(String, String)> = kinds
        .iter()
        .flat_map(|data| {
            data["issueTypes"]
                .as_array()
                .or_else(|| data["values"].as_array())
                .into_iter()
                .flatten()
        })
        .filter(|kind| kind["subtask"].as_bool() != Some(true))
        .filter_map(|kind| {
            Some((
                kind["id"].as_str()?.to_string(),
                kind["name"].as_str()?.to_string(),
            ))
        })
        .collect();
    let kind = kinds
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case("task"))
        .or_else(|| kinds.first())
        .map(|(id, _)| json!({"id": id}))
        .unwrap_or_else(|| json!({"name": "Task"}));
    let mut fields = json!({
        "project": {"key": project},
        "summary": draft.title,
        "issuetype": kind,
    });
    if !draft.body.is_empty() {
        fields["description"] = document(&draft.body);
    }
    let created = post(connection, "/rest/api/3/issue", &json!({"fields": fields}))?;
    let new_key = created["key"]
        .as_str()
        .ok_or("Jira didn't say which issue it created.")?;
    read(connection, new_key)
}

/// Text as an Atlassian document: a paragraph per blank-line-separated
/// block, its lines kept.
fn document(text: &str) -> Value {
    let paragraphs: Vec<Value> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| {
            let mut content = Vec::new();
            for (index, line) in paragraph.lines().enumerate() {
                if index > 0 {
                    content.push(json!({"type": "hardBreak"}));
                }
                if !line.is_empty() {
                    content.push(json!({"type": "text", "text": line}));
                }
            }
            json!({"type": "paragraph", "content": content})
        })
        .collect();
    json!({"type": "doc", "version": 1, "content": paragraphs})
}

fn parse(data: &Value, site: &str) -> Vec<Item> {
    data["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let key = row["key"].as_str()?.to_string();
            let fields = &row["fields"];
            let title = fields["summary"].as_str()?.to_string();
            let labels: Vec<String> = fields["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label.as_str().map(str::to_string))
                .collect();
            Some(Item {
                provider: Provider::Jira,
                url: format!("{site}/browse/{key}"),
                body: clip(&document_text(&fields["description"])),
                detail: super::joined(&[
                    fields["project"]["name"].as_str(),
                    fields["status"]["name"].as_str(),
                ]),
                branch: branch_for(&key, &title),
                reference: None,
                key,
                title,
                state: fields["status"]["name"].as_str().map(|name| {
                    let category = match fields["status"]["statusCategory"]["key"].as_str() {
                        Some("new") => StateCategory::Open,
                        Some("indeterminate") => StateCategory::Started,
                        Some("done") => StateCategory::Done,
                        _ => StateCategory::from_name(name),
                    };
                    ItemState::new(category, name)
                }),
                assignee: fields["assignee"]["displayName"]
                    .as_str()
                    .map(str::to_string),
                labels,
                updated_at_ms: updated_ms(&fields["updated"]),
            })
        })
        .collect()
}

/// Plain text of an Atlassian document (Jira's rich-text JSON).
pub(super) fn document_text(node: &Value) -> String {
    fn walk(node: &Value, out: &mut String) {
        match node["type"].as_str() {
            Some("text") => out.push_str(node["text"].as_str().unwrap_or_default()),
            Some("hardBreak") => out.push('\n'),
            _ => {}
        }
        for child in node["content"].as_array().into_iter().flatten() {
            walk(child, out);
        }
        if matches!(
            node["type"].as_str(),
            Some("paragraph" | "heading" | "listItem" | "codeBlock" | "blockquote" | "rule")
        ) && !out.ends_with('\n')
        {
            out.push('\n');
        }
    }
    let mut out = String::new();
    walk(node, &mut out);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn searches_by_key_words_or_assignment() {
        assert_eq!(
            jql(""),
            "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC"
        );
        assert_eq!(jql("proj-12"), "key = \"PROJ-12\"");
        assert_eq!(
            jql("say \"hi\""),
            "text ~ \"say \\\"hi\\\"\" ORDER BY updated DESC"
        );
        assert_eq!(
            jql("well-known"),
            "text ~ \"well-known\" ORDER BY updated DESC"
        );
    }

    #[test]
    fn documents_read_as_plain_text() {
        let description = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "First line"}, {"type": "hardBreak"}, {"type": "text", "text": "second"}]},
                {"type": "bulletList", "content": [{"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "point"}]}]}]}
            ]
        });
        assert_eq!(document_text(&description), "First line\nsecond\npoint");
        assert_eq!(document_text(&Value::Null), "");
        let items = parse(
            &json!({"issues": [{"key": "OPS-4", "fields": {"summary": "Rotate keys", "description": description, "status": {"name": "In Progress", "statusCategory": {"key": "indeterminate"}}, "project": {"name": "Ops"}, "assignee": {"displayName": "Ada"}, "labels": ["security"], "updated": "2026-10-24T12:00:00.000+0000"}}]}),
            "https://acme.atlassian.net",
        );
        assert_eq!(items[0].url, "https://acme.atlassian.net/browse/OPS-4");
        assert_eq!(items[0].branch, "OPS-4-rotate-keys");
        assert_eq!(items[0].detail.as_deref(), Some("Ops · In Progress"));
        assert_eq!(items[0].state, Some(ItemState::new(StateCategory::Started, "In Progress")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].labels, vec!["security"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
    }

    #[test]
    fn comments_go_as_documents_that_read_back_the_same() {
        let text = "Fixed in 2.1\nDeployed\n\nThanks";
        let doc = document(text);
        assert_eq!(doc["content"].as_array().unwrap().len(), 2);
        assert_eq!(document_text(&doc), text.replace("\n\n", "\n"));
    }
}
