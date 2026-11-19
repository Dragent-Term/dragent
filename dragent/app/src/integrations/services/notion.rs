//! Notion's API, with an internal integration's secret. It sees only the
//! pages shared with that integration.

use serde_json::{json, Value};

use crate::integrations::items::{branch_for, Item, LIMIT};
use crate::integrations::{http, Connection, Provider};

const API: &str = "https://api.notion.com/v1";
const VERSION: &str = "2022-06-28";

fn request(
    connection: &Connection,
    builder: reqwest::blocking::RequestBuilder,
) -> Result<Value, String> {
    connection.send(builder.header("Notion-Version", VERSION))
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = request(connection, http()?.get(format!("{API}/users/me")))?;
    Ok(super::named(
        me["bot"]["workspace_name"].as_str(),
        me["name"].as_str(),
    ))
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    let body = json!({
        "query": query,
        "filter": {"property": "object", "value": "page"},
        "sort": {"direction": "descending", "timestamp": "last_edited_time"},
        "page_size": LIMIT,
    });
    let data = request(
        connection,
        http()?.post(format!("{API}/search")).json(&body),
    )?;
    Ok(parse(&data))
}

/// A page's title (its `title` property), and its unique id ("TASK-12")
/// when its database gives pages one.
fn title_and_key(properties: &Value) -> (String, String) {
    let mut title = String::new();
    let mut key = String::new();
    for property in properties
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, property)| property)
    {
        match property["type"].as_str() {
            Some("title") => title = plain_text(&property["title"]),
            Some("unique_id") => {
                if let Some(number) = property["unique_id"]["number"].as_u64() {
                    key = match property["unique_id"]["prefix"]
                        .as_str()
                        .filter(|p| !p.is_empty())
                    {
                        Some(prefix) => format!("{prefix}-{number}"),
                        None => number.to_string(),
                    };
                }
            }
            _ => {}
        }
    }
    (title, key)
}

fn plain_text(rich_text: &Value) -> String {
    rich_text
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["plain_text"].as_str())
        .collect()
}

fn parse(data: &Value) -> Vec<Item> {
    data["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let id = page["id"].as_str()?;
            let (title, key) = title_and_key(&page["properties"]);
            // Untitled pages are noise in a picker.
            let title = title.trim().to_string();
            if title.is_empty() {
                return None;
            }
            Some(Item {
                provider: Provider::Notion,
                branch: branch_for(&key, &title),
                key,
                url: page["url"].as_str()?.to_string(),
                body: String::new(),
                detail: None,
                reference: Some(id.to_string()),
                title,
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
            })
        })
        .collect()
}

/// A page id from the id or the page's address (which ends with it,
/// dashes left out).
fn page(id: &str) -> String {
    let id = id.trim().split(['?', '#']).next().unwrap_or_default();
    let last = id
        .trim_end_matches('/')
        .rsplit(['/', '-'])
        .next()
        .unwrap_or(id);
    if last.len() == 32 && last.chars().all(|c| c.is_ascii_hexdigit()) {
        last.to_string()
    } else {
        id.to_string()
    }
}

pub(super) fn read(connection: &mut Connection, id: &str) -> Result<Item, String> {
    let page = page(id);
    let data = request(connection, http()?.get(format!("{API}/pages/{page}")))?;
    let mut item = parse(&json!({"results": [data]}))
        .pop()
        .ok_or_else(|| format!("Notion has no page {id} shared with Dragent's integration."))?;
    item.body = details(connection, &page)?;
    Ok(item)
}

/// On the page, as the integration (which needs the "insert comments"
/// capability).
pub(super) fn comment(connection: &mut Connection, id: &str, text: &str) -> Result<(), String> {
    // Rich text runs hold at most 2000 characters each.
    let characters: Vec<char> = text.chars().collect();
    let runs: Vec<Value> = characters
        .chunks(2000)
        .map(|chunk| json!({"type": "text", "text": {"content": chunk.iter().collect::<String>()}}))
        .collect();
    request(
        connection,
        http()?.post(format!("{API}/comments")).json(&json!({
            "parent": {"page_id": page(id)},
            "rich_text": runs,
        })),
    )
    .map(|_| ())
}

/// The page's text, block by block (one level deep).
pub(super) fn details(connection: &mut Connection, page: &str) -> Result<String, String> {
    let data = request(
        connection,
        http()?
            .get(format!("{API}/blocks/{page}/children"))
            .query(&[("page_size", "100")]),
    )?;
    Ok(blocks_text(&data))
}

fn blocks_text(data: &Value) -> String {
    let mut lines = Vec::new();
    for block in data["results"].as_array().into_iter().flatten() {
        let Some(kind) = block["type"].as_str() else {
            continue;
        };
        let text = plain_text(&block[kind]["rich_text"]);
        let line = match kind {
            "heading_1" => format!("# {text}"),
            "heading_2" => format!("## {text}"),
            "heading_3" => format!("### {text}"),
            "bulleted_list_item" | "toggle" => format!("- {text}"),
            "numbered_list_item" => format!("1. {text}"),
            "to_do" => {
                let done = block["to_do"]["checked"].as_bool().unwrap_or(false);
                format!("- [{}] {text}", if done { "x" } else { " " })
            }
            "quote" | "callout" => format!("> {text}"),
            "code" => format!("```\n{text}\n```"),
            "divider" => "---".to_string(),
            _ => text,
        };
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_read_with_their_title_and_id() {
        let items = parse(&json!({"results": [
            {"id": "p1", "url": "https://www.notion.so/Spec-p1", "properties": {
                "Name": {"type": "title", "title": [{"plain_text": "Billing "}, {"plain_text": "spec"}]},
                "ID": {"type": "unique_id", "unique_id": {"prefix": "TASK", "number": 12}}
            }},
            {"id": "p2", "url": "u", "properties": {"title": {"type": "title", "title": []}}}
        ]}));
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].key.as_str(), items[0].title.as_str()),
            ("TASK-12", "Billing spec")
        );
        assert_eq!(items[0].branch, "TASK-12-billing-spec");
        assert_eq!(items[0].reference.as_deref(), Some("p1"));
        assert_eq!(
            page("https://www.notion.so/acme/Billing-spec-0123456789abcdef0123456789abcdef?pvs=4"),
            "0123456789abcdef0123456789abcdef"
        );
        assert_eq!(page("p1"), "p1");
    }

    #[test]
    fn blocks_read_as_markdown() {
        let data = json!({"results": [
            {"type": "heading_2", "heading_2": {"rich_text": [{"plain_text": "Goal"}]}},
            {"type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "Make it fast."}]}},
            {"type": "to_do", "to_do": {"checked": true, "rich_text": [{"plain_text": "Profile"}]}}
        ]});
        assert_eq!(blocks_text(&data), "## Goal\nMake it fast.\n- [x] Profile");
    }
}
