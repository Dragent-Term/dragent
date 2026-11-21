//! File tools: read/write/edit/patch/list/move executors.
//!
//! Moved verbatim; same behavior.

//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use super::tools_background::*;
use super::tools_command::*;
use super::tools_mcp_auth::*;
use super::tools_search::*;
use super::tools_shell::*;
use super::tools_subagent::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;
/// Client-provided tool definitions for the local filesystem suite, sent to the
/// server via `extra_tools` (the same channel MCP tools use) so the model can
/// call them. Only tools the backend does not already provide server-side are
/// listed here — `read_file`/`write_file`/`edit_file`/`list_directory` are
/// already built in. Together they give the agent the full Filesystem-MCP set
/// (read_multiple_files, create_directory, list_directory_with_sizes,
/// directory_tree, move_file, search_files, get_file_info,
/// list_allowed_directories) without spawning an external server process.
pub fn filesystem_tool_defs() -> Vec<(String, String, String)> {
    fn def(name: &str, desc: &str, schema: serde_json::Value) -> (String, String, String) {
        (name.to_string(), desc.to_string(), schema.to_string())
    }
    let s = |d: &str| serde_json::json!({ "type": "string", "description": d });
    vec![
        def(
            "read_multiple_files",
            "Read several files at once. Absolute paths outside the workspace are supported and trigger the user's scoped approval.",
            serde_json::json!({"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"},"description":"Absolute or workspace-relative paths to read."}},"required":["paths"]}),
        ),
        def(
            "create_directory",
            "Create a directory, including any missing parent directories.",
            serde_json::json!({"type":"object","properties":{"path":s("Directory to create.")},"required":["path"]}),
        ),
        def(
            "list_directory_with_sizes",
            "List a directory's immediate entries with each file's byte size.",
            serde_json::json!({"type":"object","properties":{"path":s("Directory to list.")},"required":["path"]}),
        ),
        def(
            "directory_tree",
            "Recursive indented tree of a directory (depth-capped; skips node_modules/target/dotdirs).",
            serde_json::json!({"type":"object","properties":{"path":s("Root directory."),"max_depth":{"type":"integer","description":"Max recursion depth (default 5)."}},"required":["path"]}),
        ),
        def(
            "move_file",
            "Move or rename a file/directory. Fails if the destination already exists.",
            serde_json::json!({"type":"object","properties":{"source":s("Existing path."),"destination":s("New path.")},"required":["source","destination"]}),
        ),
        def(
            "search_files",
            "Recursively find files/dirs whose NAME contains a case-insensitive substring (use `grep` for file contents).",
            serde_json::json!({"type":"object","properties":{"path":s("Directory to search under."),"pattern":s("Case-insensitive substring to match in names."),"exclude":{"type":"array","items":{"type":"string"},"description":"Name substrings to skip."}},"required":["path","pattern"]}),
        ),
        def(
            "get_file_info",
            "Get metadata for a path: type, size, timestamps, and permissions.",
            serde_json::json!({"type":"object","properties":{"path":s("Path to stat.")},"required":["path"]}),
        ),
        def(
            "list_allowed_directories",
            "List the workspace and permanently approved external read directories. Other absolute paths may still be requested; Dragent will ask the user for scoped approval.",
            serde_json::json!({"type":"object","properties":{}}),
        ),
        def(
            "github_create_pull_request",
            "Create a GitHub pull request using the user's local authenticated GitHub CLI (gh). The branch must already be pushed. Always preview the title, body, base, head, and repository with the user before calling this tool.",
            serde_json::json!({
                "type":"object",
                "properties": {
                    "title": s("Pull request title."),
                    "body": s("Pull request description in Markdown."),
                    "base": s("Target branch, usually main or master."),
                    "head": s("Pushed source branch."),
                    "repo": s("Optional owner/repository override; omit to use the current Git remote."),
                    "draft": {"type":"boolean", "description":"Create as a draft pull request."}
                },
                "required":["title","body","base","head"]
            }),
        ),
        def(
            "ask_user",
            "Ask the user to choose from options. This is the ONLY correct way to put \
             a decision to the user — whenever you would otherwise write a list of \
             choices in your message and ask them to pick one, call this tool instead. \
             It renders interactive, clickable options and returns their selection as \
             JSON. Writing the choices as plain text is wrong: it does not render the \
             picker. Use it when genuinely blocked on a decision only the user can make \
             — a fork with no sensible default, missing information you cannot discover \
             from the code, or a preference that changes what you build. You may ask up \
             to 4 questions in one call; they are shown one at a time. Do NOT use it for \
             choices with an obvious default (proceed with a stated assumption instead), \
             for facts you can verify yourself, or merely to ask permission to proceed. \
             After the user answers, CONTINUE the task in the same turn using their \
             choice — take the next concrete action immediately; do not stop or just \
             restate your plan.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "1–4 questions, presented to the user one at a time.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": {
                                    "type": "string",
                                    "description": "Very short label shown as a chip, e.g. \"Auth method\" (≤12 chars)."
                                },
                                "question": {
                                    "type": "string",
                                    "description": "The full question to ask."
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "description": "Allow selecting more than one option (default false)."
                                },
                                "custom": {
                                    "type": "boolean",
                                    "description": "Offer a free-text answer field alongside the options (default true). Set false to force a strict choice."
                                },
                                "options": {
                                    "type": "array",
                                    "description": "2–4 distinct choices.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string", "description": "Short choice text (1–5 words)." },
                                            "description": { "type": "string", "description": "One line on what this choice means / its trade-off." }
                                        },
                                        "required": ["label"]
                                    }
                                }
                            },
                            "required": ["question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        ),
        def(
            "todowrite",
            "Create and maintain a structured task list for the current coding session. Tracks progress, organizes multi-step work, and surfaces status to the user. \
             Use proactively when the task requires 3+ distinct steps, the work is non-trivial, the user gives multiple tasks, or new instructions arrive. \
             Capture todos when you start a task and mark exactly one `in_progress` at a time; mark `completed` only after the work is actually done (including verification). \
             Send the FULL updated list on every call — statuses: pending, in_progress, completed, cancelled; priorities: high, medium, low.",
            serde_json::json!({
                "type":"object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The updated todo list (replaces the previous list).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string", "description": "Brief description of the task." },
                                "status": { "type": "string", "description": "Current status: pending, in_progress, completed, cancelled." },
                                "priority": { "type": "string", "description": "Priority: high, medium, low." }
                            },
                            "required": ["content"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        ),
    ]
}

pub(crate) fn str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .ok_or_else(|| format!("missing required argument: {key}"))
}

// ── read_file ─────────────────────────────────────────────────────────────────
// Supports offset (1-based line number) and limit (max lines).
// Returns line-numbered output with a truncation notice if the file has more lines.

pub(crate) fn exec_read_file(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let resolved = resolve_path(raw_path);
    let path = resolved.to_string_lossy();

    let content = match std::fs::read_to_string(resolved.as_path()) {
        Err(e) => return (format!("Cannot open {path}: {e}"), true),
        Ok(c) => c,
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    // offset is 1-based; default to start of file
    let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(2000) as usize;

    let start = (offset - 1).min(total); // convert to 0-indexed
    let end = (start + limit).min(total);

    let width = total.to_string().len();
    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        out.push_str(&format!(
            "{:width$}\t{}\n",
            start + i + 1,
            line,
            width = width
        ));
    }

    if end < total {
        out.push_str(&format!(
            "\n... ({} more lines, {} total. Use offset={} to read more.)",
            total - end,
            total,
            end + 1
        ));
    }

    // Record the read so the write-guard knows this file has been seen. Note a
    // partial read (offset/limit) still counts — the agent chose what to look
    // at, which is enough to lift the "never read it" block.
    crate::agent::session_facts::mark_read_content(current_pane(), resolved.as_path(), &content);

    (out, false)
}

// ── write_file ────────────────────────────────────────────────────────────────

pub(crate) fn exec_write_file(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let content = match str_arg(args, "content") {
        Ok(c) => c,
        Err(e) => return (e, true),
    };
    let resolved = resolve_path(raw_path);
    let path = resolved.to_string_lossy().into_owned();
    let pane = current_pane();

    // Write-guard: refuse to OVERWRITE an existing file the agent hasn't read
    // this task. This is the structural form of the prompt's "read before
    // write" rule — a model cannot clobber a file whose contents it only
    // imagined. Creating a NEW file is always allowed (nothing to lose).
    if resolved.exists() && !crate::agent::session_facts::was_read(pane, resolved.as_path()) {
        return (
            format!(
                "write_file refused: {path} already exists and has not been read this task. \
                 Read it first (read_file), then write — or use edit_file for a targeted change. \
                 This guard prevents overwriting a file with assumed contents."
            ),
            true,
        );
    }

    if let Some(parent) = resolved.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (format!("Cannot create directories for {path}: {e}"), true);
        }
    }
    // Capture the prior content BEFORE the write so the UI gets a ±diff card
    // like edit_file/apply_patch produce. New file → empty old → all-adds.
    let old_content = std::fs::read_to_string(&resolved).unwrap_or_default();
    match std::fs::write(&resolved, content) {
        Ok(_) => {
            crate::agent::session_facts::mark_edited(pane, resolved.as_path());
            if let Some(cid) = current_call_id() {
                let fd = crate::agent::edit_diff::from_replacement(&path, &old_content, content);
                crate::agent::edit_diff::record(
                    &cid,
                    crate::agent::edit_diff::EditDiff { files: vec![fd] },
                );
            }
            (format!("Written {} bytes to {path}", content.len()), false)
        }
        Err(e) => (format!("Cannot write {path}: {e}"), true),
    }
}

// ── edit_file ─────────────────────────────────────────────────────────────────
// old_str must match exactly once unless replace_all=true.

pub(crate) fn exec_edit_file(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let old_str = match str_arg(args, "old_str") {
        Ok(s) => s,
        Err(e) => return (e, true),
    };
    let new_str = match str_arg(args, "new_str") {
        Ok(s) => s,
        Err(e) => return (e, true),
    };
    let replace_all = args["replace_all"].as_bool().unwrap_or(false);
    let resolved = resolve_path(raw_path);
    let path = resolved.to_string_lossy().into_owned();

    let original = match std::fs::read_to_string(&resolved) {
        Ok(s) => s,
        Err(e) => return (format!("Cannot read {path}: {e}"), true),
    };

    // Normalise CRLF so Windows-edited files match correctly
    let original = original.replace("\r\n", "\n");
    let old_str = old_str.replace("\r\n", "\n");

    let count = original.matches(old_str.as_str()).count();
    if count == 0 {
        return (
            format!(
                "edit_file: old_str not found in {path}. \
                 Check exact whitespace and indentation (copy verbatim from read_file output)."
            ),
            true,
        );
    }
    if count > 1 && !replace_all {
        return (
            format!(
                "edit_file: old_str matches {count} locations in {path}. \
                 Provide more surrounding context to make it unique, or set replace_all=true."
            ),
            true,
        );
    }

    let updated = if replace_all {
        original.replace(old_str.as_str(), new_str)
    } else {
        original.replacen(old_str.as_str(), new_str, 1)
    };

    match std::fs::write(&resolved, &updated) {
        Ok(_) => {
            // edit_file already proved it read the current content (exact
            // old_str match), so this also lifts the write-guard for the file.
            crate::agent::session_facts::mark_edited(current_pane(), resolved.as_path());
            // Capture a ±diff for the UI (old_str → new_str block replacement).
            if let Some(cid) = current_call_id() {
                let fd = crate::agent::edit_diff::from_replacement(&path, &old_str, new_str);
                crate::agent::edit_diff::record(
                    &cid,
                    crate::agent::edit_diff::EditDiff { files: vec![fd] },
                );
            }
            let n = if replace_all { count } else { 1 };
            (format!("Applied {n} replacement(s) in {path}"), false)
        }
        Err(e) => (format!("Cannot write {path}: {e}"), true),
    }
}

// ── batch_edit ────────────────────────────────────────────────────────────────
// Atomic multi-file editing: validate all edits first, then write all files.
// If any validation fails, no files are changed.
// If a write fails mid-batch, already-written files are rolled back.

pub(crate) fn exec_batch_edit(args: &serde_json::Value) -> (String, bool) {
    let edits = match args["edits"].as_array() {
        Some(e) if !e.is_empty() => e,
        _ => return ("'edits' must be a non-empty array".to_string(), true),
    };

    struct EditPlan {
        path: String,
        old_str: String,
        new_str: String,
        replace_all: bool,
    }

    // Parse edit specs
    let mut plans: Vec<EditPlan> = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let raw_path = match edit["path"].as_str() {
            Some(s) => s,
            None => return (format!("edit[{i}]: missing 'path'"), true),
        };
        let old_str = match edit["old_str"].as_str() {
            Some(s) => s,
            None => return (format!("edit[{i}]: missing 'old_str'"), true),
        };
        let new_str = match edit["new_str"].as_str() {
            Some(s) => s,
            None => return (format!("edit[{i}]: missing 'new_str'"), true),
        };
        plans.push(EditPlan {
            path: resolve_path(raw_path).to_string_lossy().into_owned(),
            old_str: old_str.replace("\r\n", "\n"),
            new_str: new_str.to_string(),
            replace_all: edit["replace_all"].as_bool().unwrap_or(false),
        });
    }

    // Phase 1 — read all unique files and apply edits to in-memory copies
    let mut file_contents: HashMap<String, String> = HashMap::new();
    let mut backups: HashMap<String, String> = HashMap::new();

    for plan in &plans {
        if !file_contents.contains_key(&plan.path) {
            match std::fs::read_to_string(&plan.path) {
                Ok(c) => {
                    let c = c.replace("\r\n", "\n");
                    backups.insert(plan.path.clone(), c.clone());
                    file_contents.insert(plan.path.clone(), c);
                }
                Err(e) => return (format!("Cannot read {}: {e}", plan.path), true),
            }
        }

        let content = file_contents.get_mut(&plan.path).unwrap();
        let count = content.matches(plan.old_str.as_str()).count();
        if count == 0 {
            return (
                format!(
                    "batch_edit: old_str not found in {} (edit {}). \
                     No files were changed.",
                    plan.path,
                    plans
                        .iter()
                        .position(|p| std::ptr::eq(p, plan))
                        .unwrap_or(0)
                ),
                true,
            );
        }
        if count > 1 && !plan.replace_all {
            return (
                format!(
                    "batch_edit: old_str matches {count} times in {} — use replace_all or \
                     add more context. No files were changed.",
                    plan.path
                ),
                true,
            );
        }

        *content = if plan.replace_all {
            content.replace(plan.old_str.as_str(), &plan.new_str)
        } else {
            content.replacen(plan.old_str.as_str(), &plan.new_str, 1)
        };
    }

    // Phase 2 — write files; rollback on failure
    let changed_paths: Vec<String> = file_contents
        .keys()
        .filter(|p| backups[*p] != file_contents[*p])
        .cloned()
        .collect();

    let mut written: Vec<String> = Vec::new();
    for path in &changed_paths {
        match std::fs::write(path, &file_contents[path]) {
            Ok(_) => written.push(path.clone()),
            Err(e) => {
                // Roll back already-written files
                for wp in &written {
                    let _ = std::fs::write(wp, &backups[wp]);
                }
                return (
                    format!(
                        "Write failed for {path} (rolled back {} file(s)): {e}",
                        written.len()
                    ),
                    true,
                );
            }
        }
    }

    // batch_edit matched exact old_str per file → the write-guard is satisfied
    // and these edits are unverified until a build/test runs.
    let pane = current_pane();
    for path in &changed_paths {
        crate::agent::session_facts::mark_edited(pane, std::path::Path::new(path));
    }
    // Capture a whole-file diff per changed file for the UI (backup = before,
    // final content = after).
    if let Some(cid) = current_call_id() {
        let files: Vec<crate::agent::edit_diff::FileDiff> = changed_paths
            .iter()
            .map(|p| crate::agent::edit_diff::from_replacement(p, &backups[p], &file_contents[p]))
            .collect();
        crate::agent::edit_diff::record(&cid, crate::agent::edit_diff::EditDiff { files });
    }

    (
        format!(
            "Applied {} edits across {} file(s): {}",
            plans.len(),
            changed_paths.len(),
            changed_paths.join(", ")
        ),
        false,
    )
}

// ── apply_patch ───────────────────────────────────────────────────────────────
// Applies a unified diff (git diff format) to one or more files.

#[derive(Debug)]
pub(crate) enum Change {
    Context(String),
    Remove(String),
    Add(String),
}

pub(crate) struct Hunk {
    pub(crate) orig_start: usize, // 1-based line number in original file
    pub(crate) changes: Vec<Change>,
}

pub(crate) struct FilePatch {
    pub(crate) path: String,
    pub(crate) hunks: Vec<Hunk>,
}

pub(crate) fn exec_apply_patch(args: &serde_json::Value) -> (String, bool) {
    let patch_text = match str_arg(args, "patch") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };

    let patches = match parse_patch(patch_text) {
        Ok(p) => p,
        Err(e) => return (format!("Failed to parse patch: {e}"), true),
    };

    if patches.is_empty() {
        return ("No files found in patch".to_string(), true);
    }
    // Relative paths are the project's, like every other file tool (they
    // used to resolve against the app's own working directory).
    let patches: Vec<FilePatch> = patches
        .into_iter()
        .map(|mut fp| {
            fp.path = resolve_path(&fp.path).to_string_lossy().into_owned();
            fp
        })
        .collect();

    // Read all files and backups before applying
    let mut contents: HashMap<String, Vec<String>> = HashMap::new();
    let mut backups: HashMap<String, Vec<String>> = HashMap::new();
    for fp in &patches {
        match std::fs::read_to_string(&fp.path) {
            Ok(c) => {
                let lines: Vec<String> = c.lines().map(String::from).collect();
                backups.insert(fp.path.clone(), lines.clone());
                contents.insert(fp.path.clone(), lines);
            }
            Err(e) => return (format!("Cannot read {}: {e}", fp.path), true),
        }
    }

    // Apply each file's hunks to in-memory content
    for fp in &patches {
        let file_lines = contents.get_mut(&fp.path).unwrap();
        for hunk in &fp.hunks {
            if let Err(e) = apply_hunk(file_lines, hunk) {
                return (format!("Failed to apply hunk to {}: {e}", fp.path), true);
            }
        }
    }

    // Write all files; rollback on failure
    let mut written: Vec<String> = Vec::new();
    for fp in &patches {
        let new_content = contents[&fp.path].join("\n");
        // Preserve trailing newline if original had one
        let trailing = if backups[&fp.path].last().map(String::as_str) == Some("") {
            "\n"
        } else {
            ""
        };
        match std::fs::write(&fp.path, format!("{new_content}{trailing}")) {
            Ok(_) => written.push(fp.path.clone()),
            Err(e) => {
                for wp in &written {
                    let _ = std::fs::write(wp, backups[wp].join("\n"));
                }
                return (
                    format!(
                        "Write failed for {} (rolled back {}): {e}",
                        fp.path,
                        written.len()
                    ),
                    true,
                );
            }
        }
    }

    // Patch hunks matched real file content (structural grounding, like
    // edit_file), so register the edits for the evidence gate + write-guard.
    let pane = current_pane();
    for fp in &patches {
        crate::agent::session_facts::mark_edited(pane, std::path::Path::new(&fp.path));
    }
    // Capture per-file diffs for the UI (before = backup lines, after = new content).
    if let Some(cid) = current_call_id() {
        let files: Vec<crate::agent::edit_diff::FileDiff> = patches
            .iter()
            .map(|fp| {
                let before = backups[&fp.path].join("\n");
                let after = contents[&fp.path].join("\n");
                crate::agent::edit_diff::from_replacement(&fp.path, &before, &after)
            })
            .collect();
        crate::agent::edit_diff::record(&cid, crate::agent::edit_diff::EditDiff { files });
    }

    (
        format!(
            "Patched {} file(s): {}",
            patches.len(),
            patches
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        false,
    )
}

pub(crate) fn parse_patch(text: &str) -> Result<Vec<FilePatch>, String> {
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut current: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;

    for line in text.lines() {
        if line.starts_with("+++ ") {
            // Save previous file
            if let Some(mut fp) = current.take() {
                if let Some(h) = current_hunk.take() {
                    fp.hunks.push(h);
                }
                patches.push(fp);
            }
            // Strip "b/" prefix from path
            let raw = &line[4..];
            let path = raw.trim_start_matches("b/");
            current = Some(FilePatch {
                path: path.to_string(),
                hunks: Vec::new(),
            });
        } else if line.starts_with("--- ") {
            // skip — we use +++ for the path
        } else if line.starts_with("@@ ") {
            // Save previous hunk
            if let Some(fp) = current.as_mut() {
                if let Some(h) = current_hunk.take() {
                    fp.hunks.push(h);
                }
            }
            // Parse: @@ -orig_start[,orig_count] +new_start[,new_count] @@
            let orig_start = parse_hunk_header(line)?;
            current_hunk = Some(Hunk {
                orig_start,
                changes: Vec::new(),
            });
        } else if let Some(hunk) = current_hunk.as_mut() {
            if let Some(rest) = line.strip_prefix('-') {
                hunk.changes.push(Change::Remove(rest.to_string()));
            } else if let Some(rest) = line.strip_prefix('+') {
                hunk.changes.push(Change::Add(rest.to_string()));
            } else if line.starts_with(' ') {
                hunk.changes.push(Change::Context(line[1..].to_string()));
            } else if line == "\\ No newline at end of file" {
                // ignore
            }
            // Lines not starting with ' ', '-', '+' (e.g. empty) are skipped
        }
    }

    // Flush last file/hunk
    if let Some(mut fp) = current {
        if let Some(h) = current_hunk {
            fp.hunks.push(h);
        }
        patches.push(fp);
    }

    Ok(patches)
}

pub(crate) fn parse_hunk_header(line: &str) -> Result<usize, String> {
    // @@ -start[,count] +start[,count] @@
    let after = line.trim_start_matches('@').trim_start_matches(' ');
    let minus = after
        .strip_prefix('-')
        .ok_or("invalid hunk header: missing -")?;
    let num_str = minus.split(&[',', ' '][..]).next().unwrap_or("1");
    num_str
        .parse::<usize>()
        .map_err(|_| format!("invalid hunk header line number: {num_str}"))
}

pub(crate) fn apply_hunk(file_lines: &mut Vec<String>, hunk: &Hunk) -> Result<(), String> {
    // Collect context+remove lines to locate in file
    let expected: Vec<&str> = hunk
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Context(s) | Change::Remove(s) => Some(s.as_str()),
            Change::Add(_) => None,
        })
        .collect();

    if expected.is_empty() {
        // Pure insertion — insert at orig_start position
        let pos = (hunk.orig_start - 1).min(file_lines.len());
        let additions: Vec<String> = hunk
            .changes
            .iter()
            .filter_map(|c| {
                if let Change::Add(s) = c {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
        for (i, line) in additions.into_iter().enumerate() {
            file_lines.insert(pos + i, line);
        }
        return Ok(());
    }

    // Find the hunk in the file — first try at the hint position, then scan
    let hint = hunk.orig_start.saturating_sub(1); // 0-indexed
    let start_pos = find_context(file_lines, &expected, hint)
        .or_else(|| find_context(file_lines, &expected, 0))
        .ok_or_else(|| {
            format!(
                "context not found in file (expected lines: {:?})",
                &expected[..expected.len().min(3)]
            )
        })?;

    // Rebuild the affected region
    let mut new_lines: Vec<String> = Vec::new();
    for change in &hunk.changes {
        match change {
            Change::Context(s) => new_lines.push(s.clone()),
            Change::Add(s) => new_lines.push(s.clone()),
            Change::Remove(_) => {} // skip removed lines
        }
    }

    let remove_count = expected.len();
    file_lines.splice(start_pos..start_pos + remove_count, new_lines);
    Ok(())
}

/// Find `needle` lines in `haystack` starting at `hint` position. Returns the
/// 0-based start index if found, or None.
pub(crate) fn find_context(haystack: &[String], needle: &[&str], hint: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(hint.min(haystack.len()));
    }
    let len = needle.len();
    // Search forward from hint
    for start in hint..haystack.len().saturating_sub(len - 1) {
        if haystack[start..start + len]
            .iter()
            .zip(needle)
            .all(|(a, b)| a == b)
        {
            return Some(start);
        }
    }
    // Search backward from hint
    for start in (0..hint.min(haystack.len().saturating_sub(len))).rev() {
        if haystack[start..start + len]
            .iter()
            .zip(needle)
            .all(|(a, b)| a == b)
        {
            return Some(start);
        }
    }
    None
}

// ── list_directory ────────────────────────────────────────────────────────────

pub(crate) fn exec_list_directory(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let recursive = args["recursive"].as_bool().unwrap_or(false);
    let resolved = resolve_path(raw_path);
    let path_str = resolved.to_string_lossy().into_owned();

    let mut entries = Vec::new();
    if let Err(e) = collect_dir(&path_str, &path_str, recursive, &mut entries, 0) {
        return (format!("Cannot list {path_str}: {e}"), true);
    }
    (entries.join("\n"), false)
}

pub(crate) fn collect_dir(
    root: &str,
    path: &str,
    recursive: bool,
    out: &mut Vec<String>,
    depth: usize,
) -> std::io::Result<()> {
    if depth > 5 {
        return Ok(());
    }
    let read = std::fs::read_dir(path)?;
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let p = entry.path();
        let name = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .to_string();
        let is_dir = p.is_dir();
        out.push(if is_dir {
            format!("{name}/")
        } else {
            name.clone()
        });
        let basename = p.file_name().unwrap_or_default().to_string_lossy();
        if is_dir
            && recursive
            && !basename.starts_with('.')
            && basename != "node_modules"
            && basename != "target"
        {
            let _ = collect_dir(root, &p.to_string_lossy(), true, out, depth + 1);
        }
    }
    Ok(())
}

// ── Filesystem tools (parity with @modelcontextprotocol/server-filesystem) ─────
// These give the local agent the same file operations the standard Filesystem
// MCP server exposes, without spawning an external process. All paths resolve
// relative to the pane's project cwd via `resolve_path`, so access stays scoped
// to the workspace the same way `read_file`/`write_file` already are.

/// The workspace root the tools are scoped to (the pane's project cwd, or the
/// process cwd as a fallback).
pub(crate) fn workspace_root() -> std::path::PathBuf {
    PROJECT_CWD
        .lock()
        .ok()
        .and_then(|m| m.get(&current_pane()).cloned())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

pub(crate) fn fmt_sys_time(t: std::io::Result<std::time::SystemTime>) -> String {
    match t {
        Ok(st) => chrono::DateTime::<chrono::Local>::from(st)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        Err(_) => "-".to_string(),
    }
}

// ── read_multiple_files ────────────────────────────────────────────────────────
// Read several files in one call. Each file is returned under a header; a failed
// read is reported inline rather than aborting the whole batch.

pub(crate) fn exec_read_multiple_files(args: &serde_json::Value) -> (String, bool) {
    let paths = match args["paths"].as_array() {
        Some(a) if !a.is_empty() => a,
        _ => {
            return (
                "read_multiple_files: 'paths' must be a non-empty array".to_string(),
                true,
            )
        }
    };
    let mut out = String::new();
    for p in paths {
        let Some(raw) = p.as_str() else { continue };
        let resolved = resolve_path(raw);
        let path = resolved.to_string_lossy();
        out.push_str(&format!("==> {path} <==\n"));
        match std::fs::read_to_string(&resolved) {
            Ok(c) => {
                out.push_str(&c);
                if !c.ends_with('\n') {
                    out.push('\n');
                }
                // Register for the write-guard — a file read through THIS tool
                // must count exactly like one read through read_file.
                crate::agent::session_facts::mark_read_content(
                    current_pane(),
                    resolved.as_path(),
                    &c,
                );
            }
            Err(e) => out.push_str(&format!("[error: {e}]\n")),
        }
        out.push('\n');
    }
    (out, false)
}

// ── create_directory ───────────────────────────────────────────────────────────
// Create a directory (and any missing parents). Succeeds silently if it exists.

pub(crate) fn exec_create_directory(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let resolved = resolve_path(raw_path);
    let path = resolved.to_string_lossy().into_owned();
    match std::fs::create_dir_all(&resolved) {
        Ok(_) => (format!("Created directory {path}"), false),
        Err(e) => (format!("Cannot create {path}: {e}"), true),
    }
}

// ── list_directory_with_sizes ──────────────────────────────────────────────────
// Non-recursive listing including each entry's byte size (dirs shown as <DIR>).

pub(crate) fn exec_list_directory_with_sizes(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let resolved = resolve_path(raw_path);
    let path_str = resolved.to_string_lossy().into_owned();

    let read = match std::fs::read_dir(&resolved) {
        Ok(r) => r,
        Err(e) => return (format!("Cannot list {path_str}: {e}"), true),
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());

    let mut out = String::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry.metadata().ok();
        if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
            out.push_str(&format!("{:>12}  {}/\n", "<DIR>", name));
        } else {
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!("{size:>12}  {name}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("(empty)\n");
    }
    (out, false)
}

// ── directory_tree ─────────────────────────────────────────────────────────────
// Recursive indented tree, depth-capped and skipping heavy build dirs so the
// output stays bounded on large repos.

pub(crate) fn exec_directory_tree(args: &serde_json::Value) -> (String, bool) {
    let raw_path = match str_arg(args, "path") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let max_depth = args["max_depth"].as_u64().unwrap_or(5) as usize;
    let resolved = resolve_path(raw_path);
    let path_str = resolved.to_string_lossy().into_owned();

    let mut out = String::new();
    if let Err(e) = build_tree(&resolved, 0, max_depth, &mut out) {
        return (format!("Cannot walk {path_str}: {e}"), true);
    }
    if out.is_empty() {
        out.push_str("(empty)\n");
    }
    (out, false)
}

pub(crate) fn build_tree(
    dir: &std::path::Path,
    depth: usize,
    max_depth: usize,
    out: &mut String,
) -> std::io::Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.path().is_dir();
        out.push_str(&"  ".repeat(depth));
        if is_dir {
            out.push_str(&format!("{name}/\n"));
            if !name.starts_with('.') && name != "node_modules" && name != "target" {
                let _ = build_tree(&entry.path(), depth + 1, max_depth, out);
            }
        } else {
            out.push_str(&format!("{name}\n"));
        }
    }
    Ok(())
}

// ── move_file ──────────────────────────────────────────────────────────────────
// Move or rename a file/directory. Refuses to clobber an existing destination.

pub(crate) fn exec_move_file(args: &serde_json::Value) -> (String, bool) {
    let raw_src = match str_arg(args, "source") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let raw_dst = match str_arg(args, "destination") {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    let src = resolve_path(raw_src);
    let dst = resolve_path(raw_dst);
    let (src_s, dst_s) = (
        src.to_string_lossy().into_owned(),
        dst.to_string_lossy().into_owned(),
    );

    if !src.exists() {
        return (format!("move_file: source does not exist: {src_s}"), true);
    }
    if dst.exists() {
        return (
            format!("move_file: destination already exists: {dst_s}"),
            true,
        );
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(&src, &dst) {
        Ok(_) => (format!("Moved {src_s} → {dst_s}"), false),
        Err(e) => (format!("Cannot move {src_s} → {dst_s}: {e}"), true),
    }
}

// ── search_files ───────────────────────────────────────────────────────────────
// Recursively find files/dirs whose name matches a case-insensitive substring.
// Unlike `grep` (which searches file *contents*), this searches file *names*.
