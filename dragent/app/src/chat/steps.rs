//! How a terminal agent's tool call reads as a step line: `Edit` on a file
//! becomes "Edited auth.ts" with its file as a chip, a shell `rg` becomes
//! "Searched …", a plain command is shown as itself in a monospace face.
//! Covers the tool names of every agent whose conversation the desktop can
//! read (Claude Code, Codex, OpenCode, MiMo Code, Gemini, Qwen Code,
//! Copilot, Cursor).

use super::layout::{first_line, Entry, Step, StepKind};
use dragent_host::agent_transcript::Row;

fn base_name(path: &str) -> String {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).last().unwrap_or(path).to_string()
}

fn any_of(name: &str, names: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    names.iter().any(|n| *n == lower)
}

const SHELL: &[&str] = &[
    "bash", "shell", "exec", "exec_command", "local_shell", "command", "terminal", "run_command", "run_terminal_cmd",
    "run_shell_command",
];

/// A shell command that only read, searched or listed reads as that.
fn shell_intent(command: &str) -> Option<(&'static str, &'static str, StepKind, String, bool)> {
    if command.contains(['|', '>', '<', ';', '&']) {
        return None;
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    let program = words.first()?.rsplit('/').next().unwrap_or_default();
    let last = words.last().copied().unwrap_or_default();
    match program {
        "cat" | "head" | "tail" | "less" | "bat" if words.len() > 1 => Some(("Reading", "Read", StepKind::Read, base_name(last), true)),
        "sed" if words.get(1) == Some(&"-n") => Some(("Reading", "Read", StepKind::Read, base_name(last), true)),
        "rg" | "grep" | "ag" | "ack" => {
            let query = words.iter().skip(1).find(|w| !w.starts_with('-')).copied().unwrap_or_default();
            Some(("Searching", "Searched", StepKind::Search, query.trim_matches(['"', '\'']).to_string(), false))
        }
        "ls" | "tree" | "find" | "fd" => {
            let path = words.iter().skip(1).find(|w| !w.starts_with('-')).copied().unwrap_or(".");
            Some(("Listing", "Listed", StepKind::Search, path.to_string(), false))
        }
        _ => None,
    }
}

/// A transcript tool row as a step. `live`: the agent is working, so a call
/// without its result yet is still running (otherwise it was abandoned).
pub fn step_from_row(row: &Row, live: bool) -> Step {
    let name = row.tool.clone().unwrap_or_else(|| "Tool".into());
    let running = row.running && live;
    let pick = |now: &str, done: &str| Some(if running { now } else { done }.to_string());
    let base = Step {
        id: row.id.clone(),
        kind: StepKind::Other,
        verb: None,
        file: None,
        label: String::new(),
        mono: false,
        detail: None,
        added: row.added,
        removed: row.removed,
        running,
        failed: row.error,
        started_at: row.at,
    };
    let target = row.path.clone().unwrap_or_else(|| row.text.clone());
    if any_of(&name, &["delete", "delete_file", "remove_file"]) && !target.is_empty() {
        return Step { kind: StepKind::Delete, verb: pick("Deleting", "Deleted"), file: Some(base_name(&target)), detail: Some(target), ..base };
    }
    if any_of(
        &name,
        &[
            "edit", "multiedit", "write", "write_file", "replace", "patch", "apply_patch", "applypatch", "strreplace", "notebookedit",
            "str_replace_editor", "str_replace_based_edit_tool", "create", "create_file",
        ],
    ) && !target.is_empty()
    {
        let created = any_of(&name, &["write", "write_file", "create", "create_file"]);
        let verb = if created { pick("Writing", "Wrote") } else { pick("Editing", "Edited") };
        return Step { kind: StepKind::Edit, verb, file: Some(base_name(&target)), detail: Some(target), ..base };
    }
    if any_of(&name, &["read", "readfile", "view", "read_file", "read_many_files", "open_file"]) {
        return Step { kind: StepKind::Read, verb: pick("Reading", "Read"), file: Some(base_name(&target)), detail: Some(target), ..base };
    }
    if any_of(&name, &["grep", "rg", "glob", "search", "search_files", "search_file_content", "codebase_search", "semanticsearch", "find"]) {
        return Step { kind: StepKind::Search, verb: pick("Searching", "Searched"), label: row.text.clone(), ..base };
    }
    if any_of(&name, &["ls", "list_dir", "list", "list_directory"]) {
        return Step { kind: StepKind::Search, verb: pick("Listing", "Listed"), label: row.text.clone(), ..base };
    }
    if any_of(&name, &["webfetch", "web_fetch", "fetch", "fetch_url"]) {
        return Step { kind: StepKind::Fetch, verb: pick("Fetching", "Fetched"), label: row.text.clone(), ..base };
    }
    if any_of(&name, &["websearch", "web_search", "google_web_search"]) {
        return Step { kind: StepKind::Fetch, verb: pick("Searching the web", "Searched the web"), label: row.text.clone(), ..base };
    }
    if any_of(&name, &["task", "agent"]) {
        let label = if row.text.is_empty() { "Agent".into() } else { row.text.clone() };
        return Step { kind: StepKind::Agent, label, ..base };
    }
    if any_of(&name, &["todowrite", "update_plan"]) {
        let label = if running { "Updating the plan" } else { "Updated the plan" };
        return Step { kind: StepKind::Plan, label: label.into(), ..base };
    }
    if any_of(&name, SHELL) {
        if let Some((now, done, kind, subject, file)) = shell_intent(&row.text) {
            return Step {
                kind,
                verb: pick(now, done),
                file: file.then(|| subject.clone()),
                label: if file { String::new() } else { subject },
                detail: Some(format!("$ {}", row.text)),
                ..base
            };
        }
        let detail = (row.text.chars().count() > 60).then(|| format!("$ {}", row.text));
        return Step { kind: StepKind::Execute, verb: running.then(|| "Running".into()), label: row.text.clone(), mono: true, detail, ..base };
    }
    let label = if !row.text.is_empty() && row.text != name { format!("{name} {}", row.text) } else { name };
    Step { label, ..base }
}

/// A terminal agent's transcript rows as conversation entries.
pub fn entries_from_rows(rows: &[Row], live: bool) -> Vec<Entry<()>> {
    rows.iter()
        .filter_map(|row| match row.role {
            "user" => Some(Entry::You { id: row.id.clone(), text: row.text.clone(), at: row.at, note: None }),
            "tool" => Some(Entry::Step { step: step_from_row(row, live), at: row.at }),
            "assistant" if !row.text.trim().is_empty() => {
                Some(Entry::Reply { id: row.id.clone(), text: row.text.clone(), at: row.at, streaming: false })
            }
            _ => None,
        })
        .collect()
}

/// A step's text in a few words, for an accessibility label or a tooltip.
pub fn step_words(step: &Step) -> String {
    let mut words = Vec::new();
    if let Some(verb) = &step.verb {
        words.push(verb.clone());
    }
    if let Some(file) = &step.file {
        words.push(file.clone());
    }
    if !step.label.is_empty() {
        words.push(first_line(&step.label, 160));
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(tool: &str, text: &str, path: Option<&str>) -> Row {
        let mut row = Row::new("1".into(), "tool", text.into());
        row.tool = Some(tool.into());
        row.path = path.map(str::to_string);
        row
    }

    fn view(step: &Step) -> (StepKind, Option<&str>, Option<&str>, &str, bool) {
        (step.kind, step.verb.as_deref(), step.file.as_deref(), step.label.as_str(), step.mono)
    }

    #[test]
    fn tools_read_as_what_they_did() {
        assert_eq!(view(&step_from_row(&row("Edit", "/x/src/auth.ts", Some("/x/src/auth.ts")), false)), (StepKind::Edit, Some("Edited"), Some("auth.ts"), "", false));
        assert_eq!(view(&step_from_row(&row("Bash", "cargo test -p app", None), false)), (StepKind::Execute, None, None, "cargo test -p app", true));
        assert_eq!(view(&step_from_row(&row("Bash", "rg -n useTheme src", None), false)), (StepKind::Search, Some("Searched"), None, "useTheme", false));
        assert_eq!(view(&step_from_row(&row("exec_command", "cat src/main.rs", None), false)), (StepKind::Read, Some("Read"), Some("main.rs"), "", false));
        assert_eq!(view(&step_from_row(&row("write_file", "/x/new.ts", Some("/x/new.ts")), false)), (StepKind::Edit, Some("Wrote"), Some("new.ts"), "", false));
        assert_eq!(view(&step_from_row(&row("StrReplace", "src/h.tsx", Some("src/h.tsx")), false)), (StepKind::Edit, Some("Edited"), Some("h.tsx"), "", false));
        assert_eq!(view(&step_from_row(&row("Delete", "old.ts", Some("src/old.ts")), false)), (StepKind::Delete, Some("Deleted"), Some("old.ts"), "", false));
        assert_eq!(view(&step_from_row(&row("run_shell_command", "npm test", None), false)), (StepKind::Execute, None, None, "npm test", true));
    }
}
