//! monday.com's GraphQL API, with a personal API token.

use serde_json::{json, Value};

use crate::integrations::items::{branch_for, choose, clip, updated_ms, with_comments, Item, ItemState, StateCategory};
use crate::integrations::{http, Connection, Provider};

fn query(connection: &Connection, query: &str) -> Result<Value, String> {
    query_with(connection, query, json!({}))
}

fn query_with(connection: &Connection, query: &str, variables: Value) -> Result<Value, String> {
    let data = connection.send(
        http()?
            .post("https://api.monday.com/v2")
            .header("API-Version", "2024-10")
            .json(&json!({"query": query, "variables": variables})),
    )?;
    if let Some(message) = data["error_message"].as_str() {
        return Err(format!("monday.com: {message}"));
    }
    Ok(data)
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let data = query(connection, "query { me { name email } }")?;
    let me = &data["data"]["me"];
    Ok(super::named(me["name"].as_str(), me["email"].as_str()))
}

/// Items of the recently used boards; with `term`, the ones whose name
/// contains it.
fn items_query(term: &str) -> String {
    let filter = if term.is_empty() {
        String::new()
    } else {
        // A JSON string is a valid GraphQL string literal.
        let literal = serde_json::to_string(term).unwrap_or_else(|_| "\"\"".into());
        format!(", query_params: {{ rules: [{{ column_id: \"name\", compare_value: [{literal}], operator: contains_text }}] }}")
    };
    format!(
        "query {{ boards(limit: 10, order_by: used_at) {{ name items_page(limit: 15{filter}) {{ items {{ id name url updated_at column_values {{ text column {{ title }} }} updates(limit: 1) {{ text_body }} }} }} }} }}"
    )
}

pub(super) fn search(connection: &Connection, term: &str) -> Result<Vec<Item>, String> {
    let data = query(connection, &items_query(term))?;
    Ok(parse(&data))
}

/// An item's id, from the id or its address (which ends with it).
fn item_id(id: &str) -> Result<String, String> {
    let id = id
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        Ok(id.to_string())
    } else {
        Err(format!(
            "monday.com items are named by their number, not “{id}”."
        ))
    }
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let id = item_id(id)?;
    let data = query_with(
        connection,
        "query Item($ids: [ID!]) { items(ids: $ids) { id name url updated_at board { name } column_values { text column { title } } updates(limit: 25) { text_body creator { name } } } }",
        json!({"ids": [id]}),
    )?;
    let row = &data["data"]["items"][0];
    let title = row["name"]
        .as_str()
        .ok_or_else(|| format!("monday.com has no item {id}."))?;
    // Its columns ("Status: Working on it"), then its updates as comments.
    let columns: Vec<String> = row["column_values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let text = value["text"]
                .as_str()
                .filter(|text| !text.trim().is_empty())?;
            Some(format!(
                "{}: {text}",
                value["column"]["title"].as_str().unwrap_or("Column")
            ))
        })
        .collect();
    // Newest first in monday.com's answer.
    let mut updates: Vec<(String, String)> = row["updates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|update| {
            (
                update["creator"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                update["text_body"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    updates.reverse();
    Ok(Item {
        provider: Provider::Monday,
        key: String::new(),
        title: title.to_string(),
        url: row["url"].as_str().unwrap_or_default().to_string(),
        body: with_comments(&columns.join("\n"), &updates),
        detail: row["board"]["name"].as_str().map(str::to_string),
        branch: branch_for(&format!("item-{id}"), title),
        reference: Some(id),
        state: state_of(&row),
        assignee: assignee_of(&row),
        labels: Vec::new(),
        updated_at_ms: updated_ms(&row["updated_at"]),
    })
}

pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    query_with(
        connection,
        "mutation Update($item: ID!, $body: String!) { create_update(item_id: $item, body: $body) { id } }",
        json!({"item": item_id(id)?, "body": text}),
    )
    .map(|_| ())
}

/// The label of the board's first status column.
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let id = item_id(id)?;
    let data = query_with(
        connection,
        "query Board($ids: [ID!]) { items(ids: $ids) { board { id columns(types: [status]) { id title settings_str } } } }",
        json!({"ids": [id]}),
    )?;
    let board = &data["data"]["items"][0]["board"];
    let board_id = board["id"]
        .as_str()
        .ok_or_else(|| format!("monday.com has no item {id}."))?;
    let column = &board["columns"][0];
    let column_id = column["id"]
        .as_str()
        .ok_or("This item's board has no status column.")?;
    let settings: Value = column["settings_str"]
        .as_str()
        .and_then(|settings| serde_json::from_str(settings).ok())
        .unwrap_or(Value::Null);
    let labels: Vec<(String, String)> = settings["labels"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(index, label)| Some((index.clone(), label.as_str()?.to_string())))
        .filter(|(_, label)| !label.is_empty())
        .collect();
    let (_, label) = choose(status, &labels, "labels of this board's status column")?;
    query_with(
        connection,
        "mutation Move($board: ID!, $item: ID!, $column: String!, $value: String) { change_simple_column_value(board_id: $board, item_id: $item, column_id: $column, value: $value) { id } }",
        json!({"board": board_id, "item": id, "column": column_id, "value": label}),
    )?;
    Ok(label.clone())
}

fn parse(data: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    for board in data["data"]["boards"].as_array().into_iter().flatten() {
        for row in board["items_page"]["items"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let (Some(id), Some(title), Some(url)) = (
                row["id"].as_str(),
                row["name"].as_str(),
                row["url"].as_str(),
            ) else {
                continue;
            };
            items.push(Item {
                provider: Provider::Monday,
                key: String::new(),
                title: title.to_string(),
                url: url.to_string(),
                body: clip(row["updates"][0]["text_body"].as_str().unwrap_or_default()),
                detail: board["name"].as_str().map(str::to_string),
                branch: branch_for(&format!("item-{id}"), title),
                reference: Some(id.to_string()),
                state: state_of(row),
                assignee: assignee_of(row),
                labels: Vec::new(),
                updated_at_ms: updated_ms(&row["updated_at"]),
            });
        }
    }
    items
}

/// Its state from the board's status column, when it names one.
fn state_of(row: &Value) -> Option<ItemState> {
    let status = row["column_values"].as_array().into_iter().flatten().find(|value| {
        value["column"]["title"]
            .as_str()
            .is_some_and(|title| title.eq_ignore_ascii_case("status"))
    })?;
    let text = status["text"]
        .as_str()
        .filter(|text| !text.trim().is_empty())?;
    Some(ItemState::new(StateCategory::from_name(text), text))
}

/// Who holds it, from a people column, when the board names one.
fn assignee_of(row: &Value) -> Option<String> {
    row["column_values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|value| {
            value["text"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty())
                && value["column"]["title"].as_str().is_some_and(|title| {
                    let title = title.to_lowercase();
                    title.contains("person") || title.contains("assignee") || title.contains("owner")
                })
        })
        .filter_map(|value| value["text"].as_str())
        .next()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_escape_the_term_and_items_read_with_their_board() {
        assert!(items_query("say \"hi\"").contains(r#"compare_value: ["say \"hi\""]"#));
        assert!(!items_query("").contains("query_params"));
        let items = parse(
            &json!({"data": {"boards": [{"name": "Roadmap", "items_page": {"items": [
                {"id": "42", "name": "Billing page", "url": "https://acme.monday.com/boards/1/pulses/42", "updates": [{"text_body": "Latest note"}], "updated_at": "2026-10-24T12:00:00Z", "column_values": [{"text": "Working on it", "column": {"title": "Status"}}, {"text": "Ada", "column": {"title": "Person"}}]}
            ]}}]}}),
        );
        assert_eq!(items[0].branch, "item-42-billing-page");
        assert_eq!(
            (items[0].detail.as_deref(), items[0].body.as_str()),
            (Some("Roadmap"), "Latest note")
        );
        assert_eq!(items[0].state, Some(ItemState::new(StateCategory::Started, "Working on it")));
        assert_eq!(items[0].assignee.as_deref(), Some("Ada"));
        assert_eq!(items[0].updated_at_ms, Some(1_792_843_200_000));
    }

    #[test]
    fn statuses_follow_the_status_column() {
        let row = |text: &str| {
            json!({"id": "1", "name": "T", "url": "u", "column_values": [{"text": text, "column": {"title": "Status"}}]})
        };
        assert_eq!(state_of(&row("Done")), Some(ItemState::new(StateCategory::Done, "Done")));
        assert_eq!(
            state_of(&row("")),
            None,
            "an empty status is no state"
        );
        assert_eq!(
            state_of(&json!({"id": "1", "name": "T", "url": "u"})),
            None,
            "no status column is no state"
        );
    }
}
