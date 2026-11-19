//! `dragent integrations`: the services connected in Dragent, for agents
//! and scripts in a shell. The same tools other agents get through
//! `dragent mcp`, with tables for people.

use std::io::Read as _;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::Flags;
use crate::integrations::tools;

pub fn run(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let Some(parsed)=dragent_host::integration_commands::parse(args)? else { return Ok(dragent_host::integration_commands::HELP.into()); };
    let dragent_host::integration_commands::Parsed { tool, arguments, root, .. }=parsed;
    let answer = tools::call(tool, &arguments, &PathBuf::from(&root))?;
    if flags.json {
        return Ok(answer);
    }
    let value: Value = serde_json::from_str(&answer).unwrap_or(Value::Null);
    Ok(match tool {
        "services" => services_table(&flags, &value),
        "search" => {
            let rows = value["items"].as_array().cloned().unwrap_or_default();
            if rows.is_empty() {
                value["note"]
                    .as_str()
                    .unwrap_or("Nothing found.")
                    .to_string()
            } else {
                super::listing(&flags, rows, |row| super::text(row, "id"), item_line, "")
            }
        }
        "read" | "create" if !flags.quiet => super::presentation::records(std::slice::from_ref(&value), "No details returned.")
            .unwrap_or_else(|| item_text(&value)),
        "read" | "create" => item_text(&value),
        "describe" | "api" => answer,
        _ => value["done"].as_str().unwrap_or_default().to_string(),
    })
}

/// `--body` or `--body-file` (`-` for stdin).
fn services_table(flags: &Flags, value: &Value) -> String {
    let rows = value.as_array().cloned().unwrap_or_default();
    super::listing(
        flags,
        rows,
        |row| super::text(row, "service"),
        |row| {
            let can = &row["can"];
            let abilities: Vec<&str> = ["read", "comment", "status", "create"]
                .into_iter()
                .filter(|ability| can[*ability].as_bool() == Some(true))
                .collect();
            let state = if row["connected"].as_bool() == Some(true) {
                match row["account"].as_str() {
                    Some(account) => format!("connected as {account}"),
                    None => "connected".to_string(),
                }
            } else {
                "not connected".to_string()
            };
            format!(
                "{:<16}{:<18}{:<44}{}",
                super::text(row, "service"),
                super::text(row, "name"),
                state,
                if abilities.is_empty() {
                    "search".to_string()
                } else {
                    format!("search, {}", abilities.join(", "))
                }
            )
        },
        "",
    )
}

fn item_line(row: &Value) -> String {
    let key = row["key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .unwrap_or("-");
    let detail = row["detail"]
        .as_str()
        .map(|detail| format!("  ({detail})"))
        .unwrap_or_default();
    format!(
        "{key:<14}{}{detail}\t{}",
        super::text(row, "title"),
        super::text(row, "id")
    )
}

fn item_text(row: &Value) -> String {
    let mut lines = vec![super::text(row, "title")];
    let key = row["key"].as_str().filter(|key| !key.is_empty());
    let facts: Vec<String> = [key, row["detail"].as_str()]
        .into_iter()
        .flatten()
        .map(str::to_string)
        .collect();
    if !facts.is_empty() {
        lines.push(facts.join(" · "));
    }
    lines.push(super::text(row, "url"));
    lines.push(format!("id: {}", super::text(row, "id")));
    if let Some(text) = row["text"].as_str().filter(|text| !text.trim().is_empty()) {
        lines.push(String::new());
        lines.push(text.trim().to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn commands_say_what_they_are_missing() {
        assert!(run(&args(&["help"]))
            .unwrap()
            .contains("dragent integrations"));
        assert_eq!(
            run(&args(&["show", "linear"])).unwrap_err(),
            "say which item (its id from `search`)"
        );
        assert_eq!(
            run(&args(&["comment", "linear", "ENG-1"])).unwrap_err(),
            "write the comment with --body or --body-file"
        );
        assert_eq!(
            run(&args(&["status", "linear", "ENG-1"])).unwrap_err(),
            "say which status to move it to"
        );
        assert!(run(&args(&["frobnicate"]))
            .unwrap_err()
            .contains("unknown command"));
    }

    #[test]
    fn items_print_for_people() {
        let row = json!({"service": "linear", "id": "ENG-1", "key": "ENG-1", "title": "Retry uploads", "url": "https://linear.app/a/issue/ENG-1", "detail": "Engineering · Todo", "text": "Body"});
        assert_eq!(
            item_text(&row),
            "Retry uploads\nENG-1 · Engineering · Todo\nhttps://linear.app/a/issue/ENG-1\nid: ENG-1\n\nBody"
        );
        assert_eq!(
            item_line(&row),
            "ENG-1         Retry uploads  (Engineering · Todo)\tENG-1"
        );
    }
}
