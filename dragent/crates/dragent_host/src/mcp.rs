//! Shared stdio MCP transport for desktop and headless Dragent builds.
//! The caller supplies its tool catalog and authorization/dispatch policy.
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub fn serve(
    instructions: &str,
    tools: Vec<Value>,
    call: impl FnMut(&str, &Value) -> Result<String, String>,
) -> Result<(), String> {
    serve_io(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        instructions,
        &tools,
        call,
    )
}

pub fn serve_io(
    reader: impl BufRead,
    writer: impl Write,
    instructions: &str,
    tools: &[Value],
    mut call: impl FnMut(&str, &Value) -> Result<String, String>,
) -> Result<(), String> {
    serve_io_content(reader, writer, instructions, tools, move |name, args| {
        call(name, args).map(|text| vec![json!({"type":"text", "text":text})])
    })
}

pub fn serve_content(instructions: &str, tools: Vec<Value>, call: impl FnMut(&str, &Value) -> Result<Vec<Value>, String>) -> Result<(), String> {
    serve_io_content(std::io::stdin().lock(), std::io::stdout().lock(), instructions, &tools, call)
}

fn serve_io_content(
    reader: impl BufRead,
    mut writer: impl Write,
    instructions: &str,
    tools: &[Value],
    mut call: impl FnMut(&str, &Value) -> Result<Vec<Value>, String>,
) -> Result<(), String> {
    for line in reader.lines() {
        let line = line.map_err(|error| format!("reading from the agent: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<_> = batch
                    .iter()
                    .filter_map(|message| answer_content(message, instructions, tools, &mut call))
                    .collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(message) => answer_content(&message, instructions, tools, &mut call),
            Err(error) => Some(failure(
                Value::Null,
                -32700,
                &format!("Parse error: {error}"),
            )),
        };
        if let Some(reply) = reply {
            writeln!(writer, "{reply}")
                .map_err(|error| format!("writing to the agent: {error}"))?;
            writer
                .flush()
                .map_err(|error| format!("writing to the agent: {error}"))?;
        }
    }
    Ok(())
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}})
}

/// Notifications and responses have no reply. Tool failures remain MCP tool
/// results, while unknown tools/methods are JSON-RPC errors.
pub fn answer(
    message: &Value,
    instructions: &str,
    tools: &[Value],
    mut call: impl FnMut(&str, &Value) -> Result<String, String>,
) -> Option<Value> {
    answer_content(message, instructions, tools, move |name, args| {
        call(name, args).map(|text| vec![json!({"type":"text", "text":text})])
    })
}

pub fn answer_content(message: &Value, instructions: &str, tools: &[Value], mut call: impl FnMut(&str, &Value) -> Result<Vec<Value>, String>) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str().unwrap_or_default();
    let params = &message["params"];
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS
                .iter()
                .find(|version| **version == asked)
                .unwrap_or(&VERSIONS[0]);
            json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"dragent","title":"Dragent","version":env!("CARGO_PKG_VERSION")},"instructions":instructions})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":tools}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            if !tools.iter().any(|tool| tool["name"].as_str() == Some(name)) {
                return Some(failure(id, -32602, &format!("Unknown tool: {name}")));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call(name, &arguments) {
                Ok(content) => json!({"content": content, "isError": false}),
                Err(error) => {
                    // The same failure object the command line prints: as
                    // text for every client, structured for those reading it.
                    let failure = crate::outcome::Failure::from_error(&error).to_json();
                    let text = serde_json::to_string_pretty(&failure).unwrap_or(error);
                    json!({"content":[{"type":"text", "text":text}],"structuredContent":failure,"isError":true})
                }
            }
        }
        "resources/list" => json!({"resources":[]}),
        "prompts/list" => json!({"prompts":[]}),
        other => return Some(failure(id, -32601, &format!("Method not found: {other}"))),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

/// One catalog for both installations: names, descriptions, schemas and hints.
pub fn coordination_tools() -> Vec<Value> {
    crate::team_commands::TOOLS.iter().map(|tool|json!({
        "name":tool.name,"description":tool.description,"inputSchema":crate::team_commands::schema(tool.name),
        "annotations":{"readOnlyHint":!tool.writes,"destructiveHint":false,"openWorldHint":false}
    })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stdio_preserves_ids_notifications_batches_and_error_types() {
        let input = [
            json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-03-26"}}).to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            "invalid-json".into(),
            json!([
                {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_done","arguments":{"summary":"Done"}}},
                {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"delete_everything"}},
                {"jsonrpc":"2.0","id":3,"method":"tools/list"}
            ]).to_string(),
        ].join("\n");
        let mut output = Vec::new();
        let mut called = Vec::new();
        serve_io(
            std::io::Cursor::new(input),
            &mut output,
            "Coordination",
            &coordination_tools(),
            |name, args| {
                called.push((name.to_string(), args.clone()));
                Err("Workers report only on their own task".into())
            },
        )
        .unwrap();
        let rows: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["id"], "init");
        assert_eq!(rows[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(rows[1]["error"]["code"], -32700);
        assert_eq!(rows[2][0]["result"]["isError"], true);
        assert_eq!(rows[2][1]["error"]["code"], -32602);
        assert_eq!(rows[2][2]["result"]["tools"], json!(coordination_tools()));
        assert_eq!(
            called,
            vec![("task_done".into(), json!({"summary":"Done"}))]
        );
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn content_transport_keeps_images_and_errors_as_mcp_results() {
        let tools = vec![json!({"name":"computer_snapshot"})];
        let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"computer_snapshot"}});
        let reply = answer_content(&request, "", &tools, |_, _| Ok(vec![json!({"type":"text", "text":"tree"}), json!({"type":"image", "mimeType":"image/png", "data":"AA=="})])).unwrap();
        assert_eq!(reply["result"]["content"][1]["type"], "image");
        assert_eq!(reply["result"]["isError"], false);
        let denied = answer_content(&request, "", &tools, |_, _| Err("denied".into())).unwrap();
        assert_eq!(denied["result"]["isError"], true);
        // Every error is the shared failure object, as text and structured.
        let text: Value = serde_json::from_str(denied["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["message"], "denied");
        assert_eq!(denied["result"]["structuredContent"]["status"], "failed");
        let timeout = crate::outcome::Failure::new(crate::outcome::Status::Timeout, "no_answer", "late")
            .request("r1")
            .encode();
        let late = answer_content(&request, "", &tools, |_, _| Err(timeout.clone())).unwrap();
        assert_eq!(late["result"]["structuredContent"]["status"], "timeout");
        assert_eq!(late["result"]["structuredContent"]["request_id"], "r1");
        assert_eq!(late["result"]["structuredContent"]["retry_safe"], false);
    }
}
