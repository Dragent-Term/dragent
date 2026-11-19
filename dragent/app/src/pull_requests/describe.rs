//! "Describe with AI": a pull request description written from what the
//! source branch adds over the base (`Operation::Changes`), following the
//! team's template when one is chosen. It goes through the same agent call
//! and model as AI commit messages.

use serde_json::Value;

/// What the model is given: the template to fill, the title, anything the
/// person already wrote, and the branch's commits, diffstat and diff.
pub(super) fn prompt(template: Option<&str>, title: &str, notes: &str, changes: &Value) -> String {
    let mut prompt = String::new();
    match template {
        Some(template) => {
            prompt.push_str("Fill in this pull request template for the changes below. Keep its headings, order and checklists. Follow any instructions in its HTML comments, then remove those comments. Tick a checkbox only when the changes clearly satisfy it. Leave a section that the changes give nothing for with a short \"N/A\".\n\n<template>\n");
            prompt.push_str(template.trim());
            prompt.push_str("\n</template>\n\n");
        }
        None => prompt.push_str("Write a pull request description for the changes below: a short summary, then the notable changes as a list, then anything a reviewer should check.\n\n"),
    }
    if !title.trim().is_empty() {
        prompt.push_str(&format!("Title: {}\n\n", title.trim()));
    }
    if !notes.trim().is_empty() {
        prompt.push_str("The author's notes, to keep and build on:\n<notes>\n");
        prompt.push_str(notes.trim());
        prompt.push_str("\n</notes>\n\n");
    }
    let commits: Vec<&str> = changes["commits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    prompt.push_str(&format!("Commits ({}), oldest first:\n", commits.len()));
    for commit in commits {
        prompt.push_str("- ");
        prompt.push_str(&commit.replace('\n', "\n  "));
        prompt.push('\n');
    }
    prompt.push_str("\nFiles changed:\n");
    prompt.push_str(changes["stat"].as_str().unwrap_or(""));
    prompt.push_str("\n\nDiff");
    if changes["truncated"] == true {
        prompt.push_str(" (cut short; describe the rest from the commits and file list)");
    }
    prompt.push_str(":\n");
    prompt.push_str(changes["diff"].as_str().unwrap_or(""));
    prompt.push_str("\n\nOnly state what the commits and diff show: don't invent tests, issue numbers, screenshots or results. Output only the Markdown description.");
    prompt
}

/// Ask the model; `Err` says why it couldn't.
#[cfg(not(target_family = "wasm"))]
pub(super) fn generate(prompt: String) -> Result<String, String> {
    use crate::agent::api::types::{AgentMessage, AgentStepRequest};
    let selected = crate::settings::AppSettings::load().selected_model;
    let request = AgentStepRequest {
        model_name: crate::settings::git_consumers::commit_ai_model(&selected),
        system: "You write clear, accurate pull request descriptions from a branch's commits and diff, in the team's own template when there is one.".to_string(),
        messages: vec![AgentMessage::user(&prompt)],
        tools: vec![],
        refresh_token: None,
        access_token: None,
    };
    let reply = crate::agent::default_agent_api()
        .agent_step(request)
        .map_err(|error| format!("Couldn't write a description: {error}"))?;
    let description = clean(&reply.content);
    if description.is_empty() {
        Err("The model returned an empty description. Try again.".into())
    } else {
        Ok(description)
    }
}

/// The description without a fence the model may wrap it in.
fn clean(reply: &str) -> String {
    let reply = reply.trim();
    let Some(inner) = reply.strip_prefix("```") else {
        return reply.to_string();
    };
    let inner = inner.split_once('\n').map_or("", |(_, rest)| rest);
    inner.strip_suffix("```").unwrap_or(inner).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_template_and_the_changes_reach_the_model() {
        let changes = json!({
            "commits": ["Hash invitation tokens\n\nStore only the hash."],
            "stat": " src/invitations.ts | 12 ++++++----",
            "diff": "+const tokenHash = hash(token);",
            "truncated": true,
        });
        let prompt = prompt(Some("## Summary\n<!-- what and why -->\n## Testing"), "Secure invitations", "Closes #117", &changes);
        assert!(prompt.contains("<template>\n## Summary"));
        assert!(prompt.contains("Title: Secure invitations"));
        assert!(prompt.contains("<notes>\nCloses #117"));
        assert!(prompt.contains("- Hash invitation tokens\n  \n  Store only the hash."));
        assert!(prompt.contains("src/invitations.ts"));
        assert!(prompt.contains("cut short"));
        let plain = super::prompt(None, "", "", &changes);
        assert!(!plain.contains("<template>") && !plain.contains("<notes>"));
    }

    #[test]
    fn fences_around_the_reply_are_dropped() {
        assert_eq!(clean("```markdown\n## Summary\nDone\n```"), "## Summary\nDone");
        assert_eq!(clean("## Summary"), "## Summary");
    }
}
