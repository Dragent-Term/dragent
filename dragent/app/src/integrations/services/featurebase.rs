//! Featurebase's API: feature requests and bug reports (its "posts").

use serde_json::Value;

use crate::integrations::items::{branch_for, clip, Item, LIMIT};
use crate::integrations::{http, Connection, Provider};

const API: &str = "https://do.featurebase.app/v2";

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    connection.send(http()?.get(format!("{API}{path}")).query(query))
}

/// Featurebase has no "who am I" for a key: listing one post proves it works.
pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    get(connection, "/posts", &[("limit", "1")])?;
    Ok(None)
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    let limit = LIMIT.to_string();
    let mut params = vec![("limit", limit.as_str()), ("sortBy", "date:desc")];
    if !query.is_empty() {
        params.push(("q", query));
    }
    let data = get(connection, "/posts", &params)?;
    Ok(parse(&data))
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let data = get(connection, "/posts", &[("id", id.trim())])?;
    parse(&data)
        .into_iter()
        .find(|item| item.reference.as_deref() == Some(id.trim()))
        .ok_or_else(|| format!("Featurebase has no post {id}."))
}

fn parse(data: &Value) -> Vec<Item> {
    data["results"]
        .as_array()
        .or_else(|| data["data"].as_array())
        .or_else(|| data.as_array())
        .into_iter()
        .flatten()
        .filter_map(|post| {
            let title = post["title"].as_str()?.to_string();
            let slug = post["slug"].as_str().unwrap_or_default();
            let url = post["postUrl"]
                .as_str()
                .or_else(|| post["url"].as_str())?
                .to_string();
            let tags: Vec<&str> = post["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tag| tag["name"].as_str())
                .collect();
            Some(Item {
                provider: Provider::Featurebase,
                key: String::new(),
                url,
                body: clip(&super::strip_html(
                    post["content"].as_str().unwrap_or_default(),
                )),
                detail: super::joined(&[
                    post["status"]["name"].as_str(),
                    (!tags.is_empty()).then(|| tags.join(", ")).as_deref(),
                ]),
                branch: branch_for("", if slug.is_empty() { &title } else { slug }),
                reference: post["id"].as_str().map(str::to_string),
                title,
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn posts_read_as_text_with_their_status() {
        let items = parse(&json!({"results": [{
            "title": "Add dark mode", "slug": "add-dark-mode-support",
            "postUrl": "https://feedback.example.com/p/add-dark-mode-support",
            "content": "<p>Please &amp; thanks</p>", "status": {"name": "Planned"}, "tags": [{"name": "UI"}]
        }]}));
        assert_eq!(items[0].body, "Please & thanks");
        assert_eq!(items[0].detail.as_deref(), Some("Planned · UI"));
        assert_eq!(items[0].branch, "add-dark-mode-support");
    }
}
