//! Trello's REST API, with a Power-Up API key and a token for it.

use reqwest::Method;
use serde_json::Value;

use crate::integrations::items::{branch_for, choose, clip, updated_ms, with_comments, Item, ItemState, StateCategory, LIMIT};
use crate::integrations::{http, Connection, Provider};

const API: &str = "https://api.trello.com/1";
const CARD_FIELDS: &str = "name,desc,shortLink,url,idShort,closed,dateLastActivity,labels";

fn request(
    connection: &Connection,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
) -> Result<Value, String> {
    connection.send(http()?.request(method, format!("{API}{path}")).query(query))
}

fn get(connection: &Connection, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    request(connection, Method::GET, path, query)
}

/// A card's short link or id, from either or from its address
/// (`https://trello.com/c/<short link>/…`).
fn card(id: &str) -> Result<String, String> {
    let id = id.trim();
    let id = match id.split_once("/c/") {
        Some((_, rest)) => rest.split('/').next().unwrap_or_default(),
        None => id,
    };
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id.to_string())
    } else {
        Err(format!(
            "Trello cards are named by their short link (like aBcD1234), not “{id}”."
        ))
    }
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let card_id = card(id)?;
    let fields = format!("{CARD_FIELDS},idBoard");
    let row = get(
        connection,
        &format!("/cards/{card_id}"),
        &[
            ("fields", fields.as_str()),
            ("board", "true"),
            ("board_fields", "name"),
            ("list", "true"),
            ("list_fields", "name"),
            ("members", "true"),
            ("member_fields", "fullName"),
            ("actions", "commentCard"),
            ("actions_limit", "50"),
        ],
    )?;
    let mut item = parse(&Value::Array(vec![row.clone()]))
        .pop()
        .ok_or_else(|| format!("Trello has no card {card_id}."))?;
    // Newest first in Trello's answer.
    let mut comments: Vec<(String, String)> = row["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|action| {
            (
                action["memberCreator"]["fullName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                action["data"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    comments.reverse();
    item.body = with_comments(row["desc"].as_str().unwrap_or_default(), &comments);
    item.detail = super::joined(&[row["board"]["name"].as_str(), row["list"]["name"].as_str()]);
    Ok(item)
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    request(
        connection,
        Method::POST,
        &format!("/cards/{}/actions/comments", card(id)?),
        &[("text", text)],
    )
    .map(|_| ())
}

/// To another list on its board (a card's status is its list), or
/// archived.
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let card_id = card(id)?;
    if matches!(
        status.trim().to_lowercase().as_str(),
        "archived" | "archive" | "closed"
    ) {
        request(
            connection,
            Method::PUT,
            &format!("/cards/{card_id}"),
            &[("closed", "true")],
        )?;
        return Ok("archived".into());
    }
    let board = get(
        connection,
        &format!("/cards/{card_id}"),
        &[("fields", "idBoard")],
    )?;
    let board = board["idBoard"]
        .as_str()
        .ok_or("Trello didn't say which board the card is on.")?;
    let lists: Vec<(String, String)> = get(
        connection,
        &format!("/boards/{board}/lists"),
        &[("fields", "name"), ("filter", "open")],
    )?
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(|list| {
        Some((
            list["id"].as_str()?.to_string(),
            list["name"].as_str()?.to_string(),
        ))
    })
    .collect();
    let (list, name) = choose(status, &lists, "lists on this card's board")?;
    request(
        connection,
        Method::PUT,
        &format!("/cards/{card_id}"),
        &[("idList", list.as_str())],
    )?;
    Ok(name.clone())
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(
        connection,
        "/members/me",
        &[("fields", "fullName,username")],
    )?;
    let username = me["username"].as_str().map(|name| format!("@{name}"));
    Ok(super::named(me["fullName"].as_str(), username.as_deref()))
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    if query.is_empty() {
        // The user's own open cards.
        let data = get(
            connection,
            "/members/me/cards",
            &[
                ("filter", "open"),
                ("fields", CARD_FIELDS),
                ("board", "true"),
                ("board_fields", "name"),
            ],
        )?;
        return Ok(parse(&data));
    }
    let limit = LIMIT.to_string();
    let data = get(
        connection,
        "/search",
        &[
            ("query", query),
            ("modelTypes", "cards"),
            ("cards_limit", limit.as_str()),
            ("card_fields", CARD_FIELDS),
            ("card_board", "true"),
            ("board_fields", "name"),
        ],
    )?;
    Ok(parse(&data["cards"]))
}

fn parse(cards: &Value) -> Vec<Item> {
    cards
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|card| {
            let short_link = card["shortLink"].as_str()?;
            let title = card["name"].as_str()?.to_string();
            let labels: Vec<String> = card["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| {
                    label["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .or_else(|| label["color"].as_str())
                        .map(str::to_string)
                })
                .collect();
            Some(Item {
                provider: Provider::Trello,
                key: card["idShort"]
                    .as_u64()
                    .map(|number| format!("#{number}"))
                    .unwrap_or_default(),
                url: card["url"].as_str()?.to_string(),
                body: clip(card["desc"].as_str().unwrap_or_default()),
                detail: card["board"]["name"].as_str().map(str::to_string),
                branch: branch_for(&short_link.to_lowercase(), &title),
                reference: Some(short_link.to_string()),
                title,
                state: state_of(card),
                assignee: card["members"]
                    .as_array()
                    .and_then(|members| members.first())
                    .and_then(|member| member["fullName"].as_str())
                    .map(str::to_string),
                labels,
                updated_at_ms: updated_ms(&card["dateLastActivity"]),
            })
        })
        .collect()
}

/// Archived or not, otherwise named by its list when the card says which.
fn state_of(card: &Value) -> Option<ItemState> {
    if card["closed"].as_bool() == Some(true) {
        return Some(ItemState::done("Closed"));
    }
    match card["list"]["name"]
        .as_str()
        .or_else(|| card["idList"].as_str())
    {
        Some(list) if !list.trim().is_empty() => Some(ItemState::new(
            StateCategory::from_name(list),
            list,
        )),
        _ if card["closed"].is_null() => None,
        _ => Some(ItemState::open("Open")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cards_read_with_their_board() {
        let items = parse(&json!([
            {"name": "Fix login bug", "desc": "x", "shortLink": "aBcD1234", "url": "https://trello.com/c/aBcD1234/1-fix-login-bug", "idShort": 1, "board": {"name": "Sprint"}, "closed": false, "list": {"name": "Doing"}, "labels": [{"name": "bug", "color": "red"}], "members": [{"fullName": "Ada"}], "dateLastActivity": "2026-10-24T12:00:00.000Z"}
        ]));
        assert_eq!(items[0].key, "#1");
        assert_eq!(items[0].branch, "abcd1234-fix-login-bug");
        assert_eq!(items[0].detail.as_deref(), Some("Sprint"));
        assert_eq!(items[0].id(), "aBcD1234");
        assert_eq!(items[0].state, Some(ItemState::new(StateCategory::Started, "Doing")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].labels, vec!["bug"]);
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
        assert_eq!(
            card("https://trello.com/c/aBcD1234/1-fix").unwrap(),
            "aBcD1234"
        );
        assert!(card("not a card").is_err());
        let archived = parse(&json!([{"name": "T", "shortLink": "x", "url": "u", "closed": true}]));
        assert_eq!(archived[0].state, Some(ItemState::done("Closed")));
    }
}
