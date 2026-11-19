//! Gmail, read-only, through Google's API with a browser sign-in.

use base64::Engine as _;
use serde_json::Value;

use crate::integrations::items::{slug, Item};
use crate::integrations::oauth::{access_token, GOOGLE};
use crate::integrations::{http, read_json, Connection, Provider};

const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// Messages a search fetches the subject of (one request each).
const MESSAGES: &str = "15";

fn get(connection: &mut Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let token = access_token(connection, &GOOGLE)?;
    let response = http()?
        .get(format!("{API}{path}"))
        .bearer_auth(token)
        .query(query)
        .send();
    read_json(Provider::Gmail, response)
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let profile = get(connection, "/profile", &[])?;
    Ok(profile["emailAddress"].as_str().map(str::to_string))
}

pub(super) fn search(connection: &mut Connection, query: &str) -> Result<Vec<Item>, String> {
    // Gmail's own search syntax passes straight through.
    let query = if query.is_empty() { "in:inbox" } else { query };
    let list = get(
        connection,
        "/messages",
        &[("q", query), ("maxResults", MESSAGES)],
    )?;
    let mut items = Vec::new();
    for message in list["messages"].as_array().into_iter().flatten() {
        let Some(id) = message["id"].as_str() else {
            continue;
        };
        let data = get(
            connection,
            &format!("/messages/{id}"),
            &[
                ("format", "metadata"),
                ("metadataHeaders", "Subject"),
                ("metadataHeaders", "From"),
            ],
        )?;
        items.extend(item(&data));
    }
    Ok(items)
}

fn header<'a>(message: &'a Value, name: &str) -> Option<&'a str> {
    message["payload"]["headers"]
        .as_array()?
        .iter()
        .find(|header| {
            header["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })?["value"]
        .as_str()
}

fn item(message: &Value) -> Option<Item> {
    let id = message["id"].as_str()?;
    let thread = message["threadId"].as_str().unwrap_or(id);
    let title = header(message, "Subject")
        .filter(|s| !s.is_empty())
        .unwrap_or("(no subject)")
        .to_string();
    // "Ada Lovelace <ada@example.com>" → "Ada Lovelace".
    let from = header(message, "From").map(|from| {
        from.split('<')
            .next()
            .unwrap_or(from)
            .trim()
            .trim_matches('"')
            .to_string()
    });
    Some(Item {
        provider: Provider::Gmail,
        key: String::new(),
        url: format!("https://mail.google.com/mail/u/0/#all/{thread}"),
        body: message["snippet"].as_str().unwrap_or_default().to_string(),
        detail: from.filter(|from| !from.is_empty()),
        branch: format!("email-{}", slug(&title, 40))
            .trim_end_matches('-')
            .to_string(),
        reference: Some(id.to_string()),
        title,
        state: None,
        assignee: None,
        labels: Vec::new(),
        updated_at_ms: None,
    })
}

pub(super) fn read(connection: &mut Connection, id: &str) -> Result<Item, String> {
    let message = get(
        connection,
        &format!("/messages/{}", id.trim()),
        &[("format", "full")],
    )?;
    let mut item = item(&message).ok_or_else(|| format!("Gmail has no message {id}."))?;
    if let Some(text) = plain_text(&message["payload"]) {
        item.body = text;
    }
    Ok(item)
}

/// The message's plain text (its first text/plain part).
pub(super) fn details(connection: &mut Connection, id: &str) -> Result<String, String> {
    let message = get(
        connection,
        &format!("/messages/{id}"),
        &[("format", "full")],
    )?;
    Ok(plain_text(&message["payload"]).unwrap_or_default())
}

fn plain_text(part: &Value) -> Option<String> {
    if part["mimeType"].as_str() == Some("text/plain") {
        let data = part["body"]["data"].as_str()?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(data.trim_end_matches('='))
            .ok()?;
        return Some(String::from_utf8_lossy(&bytes).trim().to_string());
    }
    part["parts"].as_array()?.iter().find_map(plain_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn emails_read_with_their_sender_and_text() {
        let message = json!({"id": "m1", "threadId": "t1", "snippet": "Hi team…", "payload": {"headers": [
            {"name": "Subject", "value": "Invoice export broken"},
            {"name": "From", "value": "\"Ada Lovelace\" <ada@example.com>"}
        ]}});
        let item = item(&message).unwrap();
        assert_eq!(item.url, "https://mail.google.com/mail/u/0/#all/t1");
        assert_eq!(item.detail.as_deref(), Some("Ada Lovelace"));
        assert_eq!(item.branch, "email-invoice-export-broken");
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("Line one\nLine two");
        let full = json!({"mimeType": "multipart/alternative", "parts": [
            {"mimeType": "text/html", "body": {"data": "PGI-"}},
            {"mimeType": "text/plain", "body": {"data": body}}
        ]});
        assert_eq!(plain_text(&full).as_deref(), Some("Line one\nLine two"));
    }
}
