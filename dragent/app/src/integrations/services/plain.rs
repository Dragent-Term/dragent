//! Plain's GraphQL API: customer support threads.

use serde_json::{json, Value};

use crate::integrations::items::{branch_for, clip, Item};
use crate::integrations::{http, Connection, Provider};

const API: &str = "https://core-api.uk.plain.com/graphql/v1";

fn query(connection: &Connection, query: &str, variables: Value) -> Result<Value, String> {
    connection.send(
        http()?
            .post(API)
            .json(&json!({"query": query, "variables": variables})),
    )
}

/// Also notes the workspace id thread addresses need.
pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let data = query(connection, "query { myWorkspace { id name } }", json!({}))?;
    let workspace = &data["data"]["myWorkspace"];
    let id = workspace["id"]
        .as_str()
        .ok_or("Plain didn't say which workspace the key belongs to.")?;
    connection
        .credentials
        .insert("workspace".into(), id.to_string());
    Ok(workspace["name"].as_str().map(str::to_string))
}

const THREAD: &str = "id title ref previewText status customer { fullName }";

pub(super) fn search(connection: &Connection, term: &str) -> Result<Vec<Item>, String> {
    let workspace = connection.required("workspace")?.to_string();
    let threads: Vec<Value> = if term.is_empty() {
        // Threads waiting on the team.
        let data = query(
            connection,
            &format!("query {{ threads(first: 25, filters: {{ statuses: [TODO] }}) {{ edges {{ node {{ {THREAD} }} }} }} }}"),
            json!({}),
        )?;
        edges(&data["data"]["threads"])
            .into_iter()
            .cloned()
            .collect()
    } else {
        let data = query(
            connection,
            &format!("query Search($term: String!) {{ searchThreads(searchQuery: {{ term: $term }}, first: 25) {{ edges {{ node {{ thread {{ {THREAD} }} }} }} }} }}"),
            json!({"term": term}),
        )?;
        edges(&data["data"]["searchThreads"])
            .into_iter()
            .map(|node| node["thread"].clone())
            .collect()
    };
    Ok(threads
        .iter()
        .filter_map(|thread| item(thread, &workspace))
        .collect())
}

pub(super) fn read(connection: &Connection, id: &str) -> Result<Item, String> {
    let workspace = connection.required("workspace")?.to_string();
    let data = query(
        connection,
        &format!("query Thread($id: ID!) {{ thread(threadId: $id) {{ {THREAD} }} }}"),
        json!({"id": id.trim()}),
    )?;
    item(&data["data"]["thread"], &workspace).ok_or_else(|| format!("Plain has no thread {id}."))
}

/// A mutation's own error, which Plain reports beside its result.
fn mutation_error(result: &Value) -> Result<(), String> {
    match result["error"]["message"].as_str() {
        Some(message) => Err(format!("Plain: {message}")),
        None => Ok(()),
    }
}

/// An internal note on the thread (not a reply to the customer).
pub(super) fn comment(connection: &Connection, id: &str, text: &str) -> Result<(), String> {
    let data = query(
        connection,
        "query Thread($id: ID!) { thread(threadId: $id) { customer { id } } }",
        json!({"id": id.trim()}),
    )?;
    let customer = data["data"]["thread"]["customer"]["id"]
        .as_str()
        .ok_or_else(|| format!("Plain has no thread {id}."))?;
    let data = query(
        connection,
        "mutation Note($input: CreateNoteInput!) { createNote(input: $input) { note { id } error { message } } }",
        json!({"input": {"customerId": customer, "threadId": id.trim(), "text": text, "markdown": text}}),
    )?;
    mutation_error(&data["data"]["createNote"])
}

/// Done, or back to todo.
pub(super) fn set_status(
    connection: &Connection,
    id: &str,
    status: &str,
) -> Result<String, String> {
    let open =
        super::open_or_closed(status).ok_or("Plain threads can be marked done, or todo again.")?;
    let (mutation, input) = if open {
        ("markThreadAsTodo", "MarkThreadAsTodoInput")
    } else {
        ("markThreadAsDone", "MarkThreadAsDoneInput")
    };
    let data = query(
        connection,
        &format!("mutation Mark($input: {input}!) {{ {mutation}(input: $input) {{ thread {{ status }} error {{ message }} }} }}"),
        json!({"input": {"threadId": id.trim()}}),
    )?;
    let result = &data["data"][mutation];
    mutation_error(result)?;
    Ok(result["thread"]["status"]
        .as_str()
        .map(|status| status.to_lowercase())
        .unwrap_or_else(|| if open { "todo" } else { "done" }.to_string()))
}

fn edges(connection: &Value) -> Vec<&Value> {
    connection["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|edge| &edge["node"])
        .collect()
}

fn item(thread: &Value, workspace: &str) -> Option<Item> {
    let id = thread["id"].as_str()?;
    let title = thread["title"]
        .as_str()
        .filter(|t| !t.is_empty())?
        .to_string();
    let key = thread["ref"].as_str().unwrap_or_default().to_string();
    let status = thread["status"]
        .as_str()
        .map(|status| status.to_lowercase().replace('_', " "));
    Some(Item {
        provider: Provider::Plain,
        url: format!("https://app.plain.com/workspace/{workspace}/thread/{id}"),
        body: clip(thread["previewText"].as_str().unwrap_or_default()),
        detail: super::joined(&[thread["customer"]["fullName"].as_str(), status.as_deref()]),
        branch: branch_for(&key.to_lowercase(), &title),
        reference: Some(id.to_string()),
        key,
        title,
        state: None,
        assignee: None,
        labels: Vec::new(),
        updated_at_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_read_with_their_customer() {
        let thread = json!({"id": "th_1", "title": "Can't export CSV", "ref": "T-88", "previewText": "Hi, the export…", "status": "TODO", "customer": {"fullName": "Grace Hopper"}});
        let item = item(&thread, "w_1").unwrap();
        assert_eq!(item.url, "https://app.plain.com/workspace/w_1/thread/th_1");
        assert_eq!(item.detail.as_deref(), Some("Grace Hopper · todo"));
        assert_eq!(item.branch, "t-88-can-t-export-csv");
    }
}
