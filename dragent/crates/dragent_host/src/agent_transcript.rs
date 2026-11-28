//! A CLI agent's conversation, read from the transcript the agent itself
//! writes, so a phone can show it as a conversation rather than a terminal.
//!
//! Claude Code keeps one JSONL file per conversation under its config folder
//! (`projects/<folder named after the cwd>/<id>.jsonl`); Codex keeps one per
//! session under `sessions/<yyyy>/<mm>/<dd>/rollout-…-<id>.jsonl`. An account
//! profile moves that folder (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`). Read-only.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::agent_sessions::AgentSession;

/// How much of the end of a transcript is read.
const TAIL_BYTES: u64 = 768 * 1024;
/// Most rows returned.
pub const MAX_ROWS: usize = 200;
/// Longest text kept per row.
const MAX_TEXT: usize = 8 * 1024;
/// Days of Codex session folders searched.
const CODEX_DAYS: usize = 120;

/// One step of the conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    /// Stable within the transcript (its position from the start of the tail).
    pub id: String,
    /// `user`, `assistant` or `tool`.
    pub role: &'static str,
    pub text: String,
    /// A tool step's name (`Bash`, `Edit`, `exec`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// A tool's result reported an error.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub error: bool,
    /// When the agent wrote it, in Unix milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    /// A tool step with no result yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
    /// The file a tool step read or changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Lines a file change added and removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// The agent's id for a tool call, to pair it with its result.
    #[serde(skip)]
    call: Option<String>,
}

impl Row {
    /// A row with only its id, role (`user`, `assistant` or `tool`) and text.
    pub fn new(id: String, role: &'static str, text: String) -> Self {
        Row { id, role, text, tool: None, error: false, at: None, running: false, path: None, added: None, removed: None, call: None }
    }
}

/// A row's `timestamp` (RFC 3339, as both agents write it) in Unix milliseconds.
fn timestamp(value: &serde_json::Value) -> Option<i64> {
    rfc3339_millis(value.get("timestamp")?.as_str()?)
}

/// `2026-11-14T10:00:02.500Z` (or with a `±hh:mm` offset) in Unix milliseconds.
fn rfc3339_millis(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &text[19..];
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        let padded = format!("{:0<3}", &fraction[..digits.min(3)]);
        millis = padded.parse::<i64>().ok()?;
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 => {
            let sign = match rest.as_bytes()[0] { b'+' => 1, b'-' => -1, _ => return None };
            sign * (rest.get(1..3)?.parse::<i64>().ok()? * 60 + rest.get(4..6)?.parse::<i64>().ok()?)
        }
        _ => return None,
    };
    // Days since 1970-01-01 for a proleptic Gregorian date.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hour) * 60 + minute - offset) * 60_000 + second * 1000 + millis)
}

fn line_count(text: &str) -> u32 {
    if text.is_empty() { 0 } else { text.lines().count() as u32 }
}

/// What a tool call changed or read: its file and, for a change, the lines
/// added and removed.
fn tool_effect(name: &str, input: &serde_json::Value) -> (Option<String>, Option<(u32, u32)>) {
    let field = |key: &str| input.get(key).and_then(|v| v.as_str());
    let any = |keys: &[&str]| keys.iter().find_map(|key| field(key));
    let path = any(&["file_path", "filePath", "absolute_path", "path", "notebook_path"]).map(str::to_string);
    let old = any(&["old_string", "oldString", "old_str"]);
    let new = any(&["new_string", "newString", "new_str"]);
    let written = any(&["content", "file_text", "contents"]);
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "apply_patch" | "applypatch" | "patch") {
        let patch = input.as_str().or_else(|| any(&["input", "patch", "patchText"])).unwrap_or("");
        return patch_effect(patch);
    }
    let counts = match lower.as_str() {
        "multiedit" => input.get("edits").and_then(|e| e.as_array()).map(|edits| {
            edits.iter().fold((0, 0), |(a, r), edit| {
                let text = |keys: &[&str]| keys.iter().find_map(|k| edit.get(*k).and_then(|v| v.as_str())).unwrap_or("");
                (a + line_count(text(&["new_string", "newString"])), r + line_count(text(&["old_string", "oldString"])))
            })
        }),
        "write" | "write_file" | "create" | "create_file" => written.map(|text| (line_count(text), 0)),
        _ if old.is_some() || new.is_some() => Some((line_count(new.unwrap_or("")), line_count(old.unwrap_or("")))),
        _ => None,
    };
    (path, counts)
}

/// The first file a patch (`*** Update File: …` format) touches and the
/// lines it adds and removes.
fn patch_effect(patch: &str) -> (Option<String>, Option<(u32, u32)>) {
    let mut path = None;
    let (mut added, mut removed) = (0, 0);
    for line in patch.lines() {
        if let Some(rest) = ["*** Update File: ", "*** Add File: ", "*** Delete File: "].iter().find_map(|p| line.strip_prefix(p)) {
            path.get_or_insert_with(|| rest.trim().to_string());
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    let counts = (path.is_some()).then_some((added, removed));
    (path, counts)
}

fn tool_row(id: String, name: String, input: &serde_json::Value, at: Option<i64>, call: Option<String>) -> Row {
    let (path, counts) = tool_effect(&name, input);
    let text = if matches!(name.to_ascii_lowercase().as_str(), "apply_patch" | "applypatch") { path.clone().unwrap_or_else(|| "patch".into()) } else { tool_summary(&name, input) };
    Row {
        tool: Some(name),
        at,
        running: call.is_some(),
        path,
        added: counts.map(|c| c.0),
        removed: counts.map(|c| c.1),
        call,
        ..Row::new(id, "tool", text)
    }
}

/// Marks the tool call `call` as answered (and failed, when `error`).
fn settle(rows: &mut [Row], call: Option<&str>, error: bool) {
    let found = match call {
        Some(call) => rows.iter_mut().rev().find(|row| row.call.as_deref() == Some(call)),
        None => rows.iter_mut().rev().find(|row| row.role == "tool"),
    };
    if let Some(row) = found {
        row.running = false;
        row.error |= error;
    }
}

/// Agents whose transcripts can be read.
pub fn supported(provider: &str) -> bool {
    matches!(provider, "claude" | "codex" | "opencode" | "mimo" | "gemini" | "qwen" | "copilot" | "cursor-agent")
}

/// `HOME`, else the account's home folder: Windows apps started from the
/// Start menu or taskbar have no `HOME`, and without this fallback no
/// transcript was found there.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// The agent's own folder for this session: its account profile's, else the
/// standard one.
fn agent_home(record: &AgentSession) -> Option<PathBuf> {
    if let Some(binding) = &record.account {
        if let crate::agent_accounts::Selection::Profile(id) = binding.selection {
            return Some(crate::agent_accounts::root().join(id.to_string()));
        }
    }
    match record.provider.as_str() {
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude"))),
        "codex" => std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".codex"))),
        "opencode" => data_home().map(|d| d.join("opencode")),
        "mimo" => data_home().map(|d| d.join("mimocode")),
        "gemini" => home().map(|h| h.join(".gemini")),
        "qwen" => home().map(|h| h.join(".qwen")),
        "cursor-agent" => home().map(|h| h.join(".cursor")),
        "copilot" => std::env::var_os("COPILOT_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".copilot"))),
        _ => None,
    }
}

/// `$XDG_DATA_HOME`, else `~/.local/share` (where the OpenCode family keeps its data on every platform).
pub(crate) fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(".local").join("share")))
}

/// Claude Code's folder name for a working directory. It names the folder
/// after the plain path it runs in, so a canonical Windows path (`\\?\C:\…`)
/// is read without its prefix.
pub fn claude_project_folder(cwd: &Path) -> String {
    dunce::simplified(cwd)
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Claude Code's conversation `id`: its file under the agent's folder
/// (`CLAUDE_CONFIG_DIR`, else `~/.claude`), in whichever project folder
/// holds it. `None` when it was never saved.
pub fn claude_conversation_file(id: &str) -> Option<PathBuf> {
    if !safe_id(id) {
        return None;
    }
    let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude")))?;
    sorted_dirs(&base.join("projects"))
        .into_iter()
        .map(|project| project.join(format!("{id}.jsonl")))
        .find(|path| path.is_file())
}

/// Where Claude Code has to run to continue the conversation in `file`.
/// It files conversations under a folder named after the path it was
/// started from, and looks only there, so the same directory reached another
/// way (through a symlink such as `~/.superset` → `~/.paradev`, or resolved)
/// finds nothing. Tries `cwd` as given, resolved, and through each symlink in
/// the home folder, and returns the spelling whose folder holds `file`.
pub fn claude_resume_dir(file: &Path, cwd: &Path) -> Option<PathBuf> {
    let folder = file.parent()?.file_name()?.to_string_lossy().into_owned();
    let mut candidates = vec![cwd.to_path_buf()];
    if let Ok(real) = cwd.canonicalize() {
        candidates.push(real.clone());
        if let Some(home) = home() {
            for entry in std::fs::read_dir(&home).ok().into_iter().flatten().filter_map(Result::ok) {
                let link = entry.path();
                if !entry.file_type().is_ok_and(|t| t.is_symlink()) {
                    continue;
                }
                let Ok(target) = link.canonicalize() else { continue };
                if let Ok(rest) = real.strip_prefix(&target) {
                    candidates.push(link.join(rest));
                }
            }
        }
    }
    candidates.into_iter().find(|candidate| claude_project_folder(candidate) == folder)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The transcript file of this session's conversation, if it can be found.
pub fn locate(record: &AgentSession) -> Option<PathBuf> {
    let id = record.conversation_id.as_deref().filter(|id| safe_id(id))?;
    let base = agent_home(record)?;
    match record.provider.as_str() {
        "claude" => {
            let projects = base.join("projects");
            let direct = projects.join(claude_project_folder(&record.cwd)).join(format!("{id}.jsonl"));
            if direct.is_file() {
                return Some(direct);
            }
            // The folder was named after another spelling of the cwd.
            std::fs::read_dir(&projects)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(format!("{id}.jsonl")))
                .find(|path| path.is_file())
        }
        "codex" => {
            let sessions = base.join("sessions");
            let suffix = format!("-{id}.jsonl");
            let mut days: Vec<PathBuf> = Vec::new();
            for year in sorted_dirs(&sessions) {
                for month in sorted_dirs(&year) {
                    days.extend(sorted_dirs(&month));
                }
            }
            days.sort();
            days.reverse();
            days.into_iter().take(CODEX_DAYS).find_map(|day| {
                std::fs::read_dir(day).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix))
                })
            })
        }
        // One database for every conversation; the id picks the session.
        "opencode" => Some(base.join("opencode.db")).filter(|p| p.is_file()),
        "mimo" => Some(base.join("mimocode.db")).filter(|p| p.is_file()),
        "copilot" => Some(base.join("session-state").join(id).join("events.jsonl")).filter(|p| p.is_file()),
        // `projects/<folder named after the cwd>/agent-transcripts/<id>/<id>.jsonl`.
        "cursor-agent" => {
            let file = format!("{id}.jsonl");
            sorted_dirs(&base.join("projects"))
                .into_iter()
                .map(|project| project.join("agent-transcripts").join(id).join(&file))
                .find(|path| path.is_file())
        }
        // `tmp/<project>/chats/session-<time>-<id prefix>.jsonl`, confirmed by its first line.
        "gemini" | "qwen" => {
            let prefix: String = id.chars().take(8).collect();
            sorted_dirs(&base.join("tmp")).into_iter().find_map(|project| {
                std::fs::read_dir(project.join("chats")).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|path| {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    name.starts_with("session-") && name.contains(&prefix) && gemini_session_id(path).as_deref() == Some(id)
                })
            })
        }
        _ => None,
    }
}

/// The `sessionId` a Gemini-family chat file declares first.
fn gemini_session_id(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).take(256 * 1024).read_line(&mut first).ok()?;
    let value: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
    value.get("sessionId").and_then(|v| v.as_str()).map(str::to_string)
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.len() <= MAX_TEXT {
        return text.to_string();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// The last lines of a file (whole lines only).
fn tail_lines(path: &Path) -> std::io::Result<Vec<String>> {
    lines_of(path, Extent::Recent)
}

/// How much of a conversation to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// Its end only (sent to a phone, over the network).
    Recent,
    /// All of it (shown on this machine, read straight from disk).
    Full,
}

fn lines_of(path: &Path, extent: Extent) -> std::io::Result<Vec<String>> {
    if extent == Extent::Full {
        let text = std::fs::read(path)?;
        return Ok(String::from_utf8_lossy(&text).lines().map(str::to_string).collect());
    }
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // Cut mid-line.
    }
    Ok(lines)
}

/// A one-line summary of a tool call's input.
fn tool_summary(name: &str, input: &serde_json::Value) -> String {
    let field = |key: &str| {
        let value = input.get(key)?;
        value.as_str().map(str::to_string).or_else(|| {
            // Codex passes a command as its argv.
            let parts: Vec<&str> = value.as_array()?.iter().filter_map(|v| v.as_str()).collect();
            let parts = match parts.as_slice() {
                ["bash" | "sh" | "zsh", "-lc" | "-c", rest @ ..] => rest.to_vec(),
                _ => parts,
            };
            (!parts.is_empty()).then(|| parts.join(" "))
        })
    };
    let text = field("command")
        .or_else(|| field("cmd"))
        .or_else(|| field("file_path").or_else(|| field("filePath")).or_else(|| field("absolute_path")).or_else(|| field("path")))
        .or_else(|| field("pattern"))
        .or_else(|| field("glob_pattern"))
        .or_else(|| field("search_term"))
        .or_else(|| field("description"))
        .or_else(|| field("url"))
        .or_else(|| field("query"))
        .or_else(|| input.as_str().map(str::to_string))
        .unwrap_or_else(|| name.to_string());
    let line = text.lines().next().unwrap_or_default().trim().to_string();
    clip(&line)
}

/// Text the person typed, or `None` for what the agent injected itself.
fn person_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("<command-")
        || trimmed.starts_with("<local-command")
        || trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<session_context>")
        || trimmed.starts_with("<user_instructions>")
        || trimmed.starts_with("Caveat: ")
    {
        return None;
    }
    Some(clip(trimmed))
}

fn add(rows: &mut Vec<Row>, index: usize, role: &'static str, text: String, at: Option<i64>) {
    let id = format!("{index}.{}", rows.len());
    rows.push(Row { at, ..Row::new(id, role, text) });
}

fn parse_claude(lines: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        if !matches!(kind, "user" | "assistant")
            || value.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
            || value.get("isMeta").and_then(|v| v.as_bool()) == Some(true)
        {
            continue;
        }
        let at = timestamp(&value);
        let Some(content) = value.get("message").and_then(|m| m.get("content")) else { continue };
        if let Some(text) = content.as_str() {
            if kind == "user" {
                if let Some(text) = person_text(text) {
                    add(&mut rows, index, "user", text, at);
                }
            } else if !text.trim().is_empty() {
                add(&mut rows, index, "assistant", clip(text), at);
            }
            continue;
        }
        for block in content.as_array().into_iter().flatten() {
            match block.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
                "text" => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or_default();
                    if kind == "user" {
                        if let Some(text) = person_text(text) {
                            add(&mut rows, index, "user", text, at);
                        }
                    } else if !text.trim().is_empty() {
                        add(&mut rows, index, "assistant", clip(text), at);
                    }
                }
                "tool_use" => {
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("Tool").to_string();
                    let input = block.get("input").cloned().unwrap_or_default();
                    let call = block.get("id").and_then(|v| v.as_str()).map(str::to_string);
                    let id = format!("{index}.{}", rows.len());
                    rows.push(tool_row(id, name, &input, at, call));
                }
                "tool_result" => {
                    // The step it answers is done, and failed when it says so.
                    let error = block.get("is_error").and_then(|v| v.as_bool()) == Some(true);
                    settle(&mut rows, block.get("tool_use_id").and_then(|v| v.as_str()), error);
                }
                _ => {}
            }
        }
    }
    rows
}

fn parse_codex(lines: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if value.get("type").and_then(|v| v.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else { continue };
        let id = format!("{index}");
        let at = timestamp(&value);
        match payload.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
            "message" => {
                let role = match payload.get("role").and_then(|v| v.as_str()) {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    _ => continue,
                };
                let text: String = payload
                    .get("content")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if role == "user" { person_text(&text) } else { Some(clip(&text)).filter(|t| !t.is_empty()) };
                if let Some(text) = text {
                    rows.push(Row { at, ..Row::new(id, role, text) });
                }
            }
            "custom_tool_call" | "function_call" | "local_shell_call" => {
                let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                let input = payload
                    .get("input")
                    .cloned()
                    .or_else(|| {
                        payload
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .and_then(|a| serde_json::from_str(a).ok())
                    })
                    .unwrap_or_default();
                let call = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
                rows.push(tool_row(id, name, &input, at, call));
            }
            "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                let output = payload.get("output");
                let text = output.and_then(|o| o.as_str()).map(str::to_string).unwrap_or_default();
                // Shell results report their exit code in the output.
                let failed = output
                    .and_then(|o| o.get("metadata"))
                    .and_then(|m| m.get("exit_code"))
                    .and_then(|c| c.as_i64())
                    .or_else(|| {
                        serde_json::from_str::<serde_json::Value>(&text)
                            .ok()
                            .and_then(|v| v.get("metadata")?.get("exit_code")?.as_i64())
                    })
                    .is_some_and(|code| code != 0);
                settle(&mut rows, payload.get("call_id").and_then(|v| v.as_str()), failed);
            }
            _ => {}
        }
    }
    rows
}

/// The text of a message's content: a string, or the `text` of its parts.
fn content_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.as_str().or_else(|| part.get("text").and_then(|t| t.as_str())))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// What the person asked, out of a Cursor message: the `<user_query>` it
/// wraps, else the text itself.
fn cursor_query(text: &str) -> Option<String> {
    if let Some(start) = text.find("<user_query>") {
        let rest = &text[start + "<user_query>".len()..];
        let end = rest.find("</user_query>").unwrap_or(rest.len());
        return person_text(&rest[..end]);
    }
    if text.trim_start().starts_with('<') {
        return None; // Context Cursor attached, not something typed.
    }
    person_text(text)
}

/// Cursor's CLI agent: `projects/<folder>/agent-transcripts/<id>/<id>.jsonl`,
/// one message per line with text and `tool_use` parts.
fn parse_cursor(lines: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let role = value.get("role").and_then(|v| v.as_str()).unwrap_or_default();
        let Some(content) = value.get("message").and_then(|m| m.get("content")) else { continue };
        let parts: Vec<serde_json::Value> = match content {
            serde_json::Value::Array(parts) => parts.clone(),
            serde_json::Value::String(text) => vec![serde_json::json!({"type": "text", "text": text})],
            _ => continue,
        };
        for part in parts {
            match part.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
                "text" => {
                    let text = part.get("text").and_then(|v| v.as_str()).unwrap_or_default();
                    if role == "user" {
                        if let Some(text) = cursor_query(text) {
                            add(&mut rows, index, "user", text, None);
                        }
                    } else if role == "assistant" && !text.trim().is_empty() {
                        add(&mut rows, index, "assistant", clip(text), None);
                    }
                }
                "tool_use" if role == "assistant" => {
                    let name = part.get("name").and_then(|v| v.as_str()).unwrap_or("Tool").to_string();
                    let input = part.get("input").cloned().unwrap_or_default();
                    let id = format!("{index}.{}", rows.len());
                    rows.push(tool_row(id, name, &input, None, None));
                }
                _ => {}
            }
        }
    }
    rows
}

/// GitHub Copilot CLI: `session-state/<id>/events.jsonl`, one event per line.
pub(crate) fn parse_copilot(lines: &[String]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let at = timestamp(&value);
        let Some(data) = value.get("data") else { continue };
        let id = format!("{index}.{}", rows.len());
        let call = |data: &serde_json::Value| data.get("toolCallId").and_then(|v| v.as_str()).map(str::to_string);
        let arguments = |value: Option<&serde_json::Value>| match value {
            Some(serde_json::Value::String(text)) => serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.clone())),
            Some(other) => other.clone(),
            None => serde_json::Value::Null,
        };
        match value.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
            "user.message" => {
                if let Some(text) = person_text(&content_text(data.get("content"))) {
                    rows.push(Row { at, ..Row::new(id, "user", text) });
                }
            }
            "assistant.message" => {
                let text = content_text(data.get("content"));
                if !text.trim().is_empty() {
                    rows.push(Row { at, ..Row::new(id, "assistant", clip(&text)) });
                }
                for request in data.get("toolRequests").and_then(|r| r.as_array()).into_iter().flatten() {
                    let call_id = call(request);
                    if call_id.is_some() && rows.iter().any(|row| row.call == call_id) {
                        continue;
                    }
                    let name = request.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                    let input = arguments(request.get("arguments"));
                    let id = format!("{index}.{}", rows.len());
                    rows.push(tool_row(id, name, &input, at, call_id));
                }
            }
            "tool.execution_start" => {
                let call_id = call(data);
                if call_id.is_some() && rows.iter().any(|row| row.call == call_id) {
                    continue;
                }
                let name = data.get("toolName").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                let input = arguments(data.get("arguments"));
                rows.push(tool_row(id, name, &input, at, call_id));
            }
            "tool.execution_complete" => {
                let failed = data.get("success").and_then(|v| v.as_bool()) == Some(false);
                settle(&mut rows, data.get("toolCallId").and_then(|v| v.as_str()), failed);
            }
            _ => {}
        }
    }
    rows
}

/// Gemini CLI (and Qwen Code): `tmp/<project>/chats/session-….jsonl`. Each
/// message is a line (a later line with the same id updates it); a `$set`
/// line can replace the whole list. Older versions wrote one JSON object
/// with a `messages` array.
pub(crate) fn parse_gemini(text: &str) -> Vec<Row> {
    let mut order: Vec<String> = Vec::new();
    let mut messages: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    let mut put = |message: &serde_json::Value, order: &mut Vec<String>| {
        let Some(id) = message.get("id").and_then(|v| v.as_str()) else { return };
        if !messages.contains_key(id) {
            order.push(id.to_string());
        }
        messages.insert(id.to_string(), message.clone());
    };
    let whole: Option<serde_json::Value> = serde_json::from_str(text).ok();
    if let Some(list) = whole.as_ref().and_then(|v| v.get("messages")).and_then(|m| m.as_array()) {
        for message in list {
            put(message, &mut order);
        }
    } else {
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if let Some(list) = value.get("$set").and_then(|s| s.get("messages")).and_then(|m| m.as_array()) {
                order.clear();
                for message in list {
                    put(message, &mut order);
                }
            } else if value.get("type").is_some() {
                put(&value, &mut order);
            }
        }
    }
    drop(put);
    let mut rows = Vec::new();
    for (index, id) in order.iter().enumerate() {
        let Some(message) = messages.get(id) else { continue };
        let at = timestamp(message);
        let text = content_text(message.get("content"));
        match message.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
            "user" => {
                if let Some(text) = person_text(&text) {
                    add(&mut rows, index, "user", text, at);
                }
            }
            "gemini" | "qwen" | "model" | "assistant" => {
                if !text.trim().is_empty() {
                    add(&mut rows, index, "assistant", clip(&text), at);
                }
                for call in message.get("toolCalls").and_then(|c| c.as_array()).into_iter().flatten() {
                    let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                    let input = call.get("args").cloned().unwrap_or_default();
                    let status = call.get("status").and_then(|v| v.as_str()).unwrap_or("success");
                    let id = format!("{index}.{}", rows.len());
                    let mut row = tool_row(id, name, &input, timestamp(call).or(at), None);
                    row.running = !matches!(status, "success" | "error" | "cancelled" | "canceled");
                    row.error = matches!(status, "error");
                    rows.push(row);
                }
            }
            _ => {}
        }
    }
    rows
}

/// The OpenCode family keeps every conversation in one SQLite database:
/// `message` rows (role, time) and their `part` rows (text, tool calls).
fn read_opencode(path: &Path, session: &str) -> std::io::Result<Vec<Row>> {
    read_opencode_with(path, session, Extent::Recent)
}

fn read_opencode_with(path: &Path, session: &str, extent: Extent) -> std::io::Result<Vec<Row>> {
    use diesel::prelude::*;
    use diesel::sql_types::Text;

    #[derive(QueryableByName)]
    struct Item {
        #[diesel(sql_type = Text)]
        message: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
        part: Option<String>,
    }

    let unavailable = |e: &dyn std::fmt::Display| std::io::Error::other(e.to_string());
    let location = uri_path(path);
    // Read-only: the agent owns this database and may be writing to it.
    let mut connection = diesel::SqliteConnection::establish(&format!("file:{location}?mode=ro")).map_err(|e| unavailable(&e))?;
    let items: Vec<Item> = diesel::sql_query(
        "SELECT m.data AS message, p.data AS part FROM \
           (SELECT id, data, time_created FROM message WHERE session_id = ? ORDER BY time_created DESC, id DESC LIMIT ?) m \
         LEFT JOIN part p ON p.message_id = m.id \
         ORDER BY m.time_created, m.id, p.id",
    )
    .bind::<Text, _>(session)
    // -1: no limit.
    .bind::<diesel::sql_types::BigInt, _>(if extent == Extent::Full { -1 } else { 400 })
    .load(&mut connection)
    .map_err(|e| unavailable(&e))?;
    Ok(parse_opencode(items.iter().map(|item| (item.message.as_str(), item.part.as_deref()))))
}

/// `(message data, part data)` pairs, in order, as rows.
fn parse_opencode<'a>(items: impl Iterator<Item = (&'a str, Option<&'a str>)>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, (message, part)) in items.enumerate() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(message) else { continue };
        let Some(part) = part.and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok()) else { continue };
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or_default();
        let at = message.get("time").and_then(|t| t.get("created")).and_then(|v| v.as_i64());
        match part.get("type").and_then(|v| v.as_str()).unwrap_or_default() {
            "text" if part.get("synthetic").and_then(|v| v.as_bool()) != Some(true) => {
                let text = part.get("text").and_then(|v| v.as_str()).unwrap_or_default();
                if role == "user" {
                    if let Some(text) = person_text(text) {
                        add(&mut rows, index, "user", text, at);
                    }
                } else if role == "assistant" && !text.trim().is_empty() {
                    add(&mut rows, index, "assistant", clip(text), at);
                }
            }
            "tool" if role == "assistant" => {
                let name = part.get("tool").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                let state = part.get("state");
                let input = state.and_then(|s| s.get("input")).cloned().unwrap_or_default();
                let status = state.and_then(|s| s.get("status")).and_then(|v| v.as_str()).unwrap_or("completed");
                let started = state.and_then(|s| s.get("time")).and_then(|t| t.get("start")).and_then(|v| v.as_i64());
                let id = format!("{index}.{}", rows.len());
                let mut row = tool_row(id, name, &input, started.or(at), None);
                row.running = matches!(status, "pending" | "running");
                row.error = status == "error";
                rows.push(row);
            }
            _ => {}
        }
    }
    rows
}

/// Whether [`read_latest`] can find `provider`'s conversation without a
/// session record (an agent started by hand in a terminal).
pub fn reads_latest(provider: &str) -> bool {
    matches!(provider, "claude" | "opencode" | "mimo" | "cursor-agent")
}

/// Cursor's folder name for a working directory: every run of characters
/// that isn't a letter or digit becomes one `-`.
fn cursor_project_folder(cwd: &Path) -> String {
    let mut out = String::new();
    for c in cwd.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The newest file in `dir` whose name passes `keep`.
fn newest_file(dir: &Path, keep: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| keep(p))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max_by_key(|(at, _)| *at)
        .map(|(_, p)| p)
}

/// `provider`'s newest conversation in folder `cwd`, for an agent started
/// by hand (no session record says which conversation it is in).
pub fn read_latest(provider: &str, cwd: &Path) -> std::io::Result<Vec<Row>> {
    let missing = || std::io::Error::new(std::io::ErrorKind::NotFound, "no conversation here");
    let home = home().ok_or_else(missing)?;
    let mut rows = match provider {
        "claude" => {
            let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
            let dir = base.join("projects").join(claude_project_folder(cwd));
            let file = newest_file(&dir, |p| p.extension().is_some_and(|e| e == "jsonl")).ok_or_else(missing)?;
            parse_claude(&tail_lines(&file)?)
        }
        "cursor-agent" => {
            let dir = home.join(".cursor").join("projects").join(cursor_project_folder(cwd)).join("agent-transcripts");
            let chat = newest_file(&dir, |p| p.is_dir()).ok_or_else(missing)?;
            let file = newest_file(&chat, |p| p.extension().is_some_and(|e| e == "jsonl")).ok_or_else(missing)?;
            parse_cursor(&tail_lines(&file)?)
        }
        "opencode" | "mimo" => {
            let (folder, name) = if provider == "opencode" { ("opencode", "opencode.db") } else { ("mimocode", "mimocode.db") };
            let db = data_home().ok_or_else(missing)?.join(folder).join(name);
            if !db.is_file() {
                return Err(missing());
            }
            let session = latest_opencode_session(&db, cwd)?.ok_or_else(missing)?;
            read_opencode(&db, &session)?
        }
        _ => return Err(missing()),
    };
    if rows.len() > MAX_ROWS {
        rows.drain(..rows.len() - MAX_ROWS);
    }
    Ok(rows)
}

/// The OpenCode-family session last used in `cwd` (top-level, not a subagent's).
fn latest_opencode_session(db: &Path, cwd: &Path) -> std::io::Result<Option<String>> {
    use diesel::prelude::*;
    use diesel::sql_types::Text;

    #[derive(QueryableByName)]
    struct Session {
        #[diesel(sql_type = Text)]
        id: String,
    }
    let unavailable = |e: &dyn std::fmt::Display| std::io::Error::other(e.to_string());
    let mut connection = diesel::SqliteConnection::establish(&format!("file:{}?mode=ro", uri_path(db))).map_err(|e| unavailable(&e))?;
    let found: Vec<Session> = diesel::sql_query(
        "SELECT id FROM session WHERE directory = ? AND parent_id IS NULL ORDER BY time_updated DESC LIMIT 1",
    )
    // OpenCode records the plain path it ran in, never `\\?\C:\…`.
    .bind::<Text, _>(dunce::simplified(cwd).to_string_lossy().to_string())
    .load(&mut connection)
    .map_err(|e| unavailable(&e))?;
    Ok(found.into_iter().next().map(|s| s.id))
}

/// A path as the `file:` URI SQLite reads (spaces and URI marks escaped).
pub(crate) fn uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '?' => "%3f".to_string(),
            '#' => "%23".to_string(),
            '%' => "%25".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// The conversation's last steps, oldest first. `path` is what [`locate`]
/// found for `record`.
pub fn read(path: &Path, record: &AgentSession) -> std::io::Result<Vec<Row>> {
    let id = record.conversation_id.as_deref().filter(|id| safe_id(id));
    read_rows(path, &record.provider, id, Extent::Recent)
}

/// Where a conversation is: its file (or database) and, for a database, the
/// session in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub provider: String,
    pub session: Option<String>,
}

/// The conversation `record` points at.
pub fn source(record: &AgentSession) -> Option<Source> {
    let path = locate(record)?;
    let session = record.conversation_id.clone().filter(|id| safe_id(id));
    Some(Source { path, provider: record.provider.clone(), session })
}

/// `provider`'s newest conversation in folder `cwd`, for an agent started by
/// hand (see [`read_latest`]).
pub fn latest_source(provider: &str, cwd: &Path) -> Option<Source> {
    let home = home()?;
    let (path, session) = match provider {
        "claude" => {
            let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
            let dir = base.join("projects").join(claude_project_folder(cwd));
            (newest_file(&dir, |p| p.extension().is_some_and(|e| e == "jsonl"))?, None)
        }
        "cursor-agent" => {
            let dir = home.join(".cursor").join("projects").join(cursor_project_folder(cwd)).join("agent-transcripts");
            let chat = newest_file(&dir, |p| p.is_dir())?;
            (newest_file(&chat, |p| p.extension().is_some_and(|e| e == "jsonl"))?, None)
        }
        "opencode" | "mimo" => {
            let (folder, name) = if provider == "opencode" { ("opencode", "opencode.db") } else { ("mimocode", "mimocode.db") };
            let db = data_home()?.join(folder).join(name);
            let session = latest_opencode_session(&db, cwd).ok().flatten()?;
            (db, Some(session))
        }
        _ => return None,
    };
    Some(Source { path, provider: provider.to_string(), session })
}

/// Read `source`'s conversation, all of it or its end.
pub fn read_source(source: &Source, extent: Extent) -> std::io::Result<Vec<Row>> {
    read_rows(&source.path, &source.provider, source.session.as_deref(), extent)
}

/// What changes whenever the conversation does: its file's size and time
/// (a database's write-ahead log included). Reading again is pointless while
/// this stays the same.
pub fn signature(source: &Source) -> Option<(u64, std::time::SystemTime)> {
    let stamp = |path: &Path| std::fs::metadata(path).ok().and_then(|m| Some((m.len(), m.modified().ok()?)));
    let (mut len, mut at) = stamp(&source.path)?;
    let mut wal = source.path.clone().into_os_string();
    wal.push("-wal");
    if let Some((wal_len, wal_at)) = stamp(Path::new(&wal)) {
        len = len.wrapping_add(wal_len);
        at = at.max(wal_at);
    }
    Some((len, at))
}

fn read_rows(path: &Path, provider: &str, session: Option<&str>, extent: Extent) -> std::io::Result<Vec<Row>> {
    let mut rows = match provider {
        "claude" => parse_claude(&lines_of(path, extent)?),
        "codex" => parse_codex(&lines_of(path, extent)?),
        "copilot" => parse_copilot(&lines_of(path, extent)?),
        "cursor-agent" => parse_cursor(&lines_of(path, extent)?),
        "gemini" | "qwen" => parse_gemini(&std::fs::read_to_string(path)?),
        "opencode" | "mimo" => read_opencode_with(path, session.unwrap_or_default(), extent)?,
        _ => Vec::new(),
    };
    if extent == Extent::Recent && rows.len() > MAX_ROWS {
        rows.drain(..rows.len() - MAX_ROWS);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn claude_transcripts_read_as_a_conversation() {
        let rows = parse_claude(&lines(concat!(
            r#"{"type":"custom-title","customTitle":"x"}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"Fix the login bug"}}"#, "\n",
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Looking at auth.ts."}]}}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"npm test\nmore"}}]}}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","is_error":true,"content":"fail"}]}}"#, "\n",
            r#"{"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":"subagent"}}"#, "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Fixed."}]}}"#,
        )));
        let view: Vec<(&str, &str, Option<&str>, bool)> =
            rows.iter().map(|r| (r.role, r.text.as_str(), r.tool.as_deref(), r.error)).collect();
        assert_eq!(view, vec![
            ("user", "Fix the login bug", None, false),
            ("assistant", "Looking at auth.ts.", None, false),
            ("tool", "npm test", Some("Bash"), true),
            ("assistant", "Fixed.", None, false),
        ]);
    }

    #[test]
    fn codex_sessions_read_as_a_conversation() {
        let rows = parse_codex(&lines(concat!(
            r#"{"type":"session_meta","payload":{"id":"x"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"}]}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Add a test"}]}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"cargo test -p app"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":\"ls\"}"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Added."}]}}"#,
        )));
        let view: Vec<(&str, &str)> = rows.iter().map(|r| (r.role, r.text.as_str())).collect();
        assert_eq!(view, vec![("user", "Add a test"), ("tool", "cargo test -p app"), ("tool", "ls"), ("assistant", "Added.")]);
    }

    #[test]
    fn steps_carry_their_time_file_counts_and_whether_they_finished() {
        let rows = parse_claude(&lines(concat!(
            r#"{"type":"user","timestamp":"2026-11-14T10:00:00.000Z","message":{"role":"user","content":"Fix it"}}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-11-14T10:00:02.500Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"a","name":"Edit","input":{"file_path":"/x/src/auth.ts","old_string":"one","new_string":"one\ntwo\nthree"}},{"type":"tool_use","id":"b","name":"Bash","input":{"command":"npm test"}}]}}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"ok"}]}}"#,
        )));
        assert_eq!(rows[0].at, Some(1_794_650_400_000));
        let edit = &rows[1];
        assert_eq!((edit.path.as_deref(), edit.added, edit.removed, edit.running), (Some("/x/src/auth.ts"), Some(3), Some(1), false));
        assert_eq!(edit.at, Some(1_794_650_402_500));
        let bash = &rows[2];
        assert!(bash.running && bash.path.is_none() && bash.added.is_none());

        let codex = parse_codex(&lines(concat!(
            r#"{"timestamp":"2026-11-14T10:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"p","input":"*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n+more\n*** End Patch"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"s","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"p","output":"done"}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"s","output":"{\"output\":\"fail\",\"metadata\":{\"exit_code\":101}}"}}"#,
        )));
        let view: Vec<_> = codex.iter().map(|r| (r.text.as_str(), r.path.as_deref(), r.added, r.removed, r.running, r.error)).collect();
        assert_eq!(view, vec![
            ("src/a.rs", Some("src/a.rs"), Some(2), Some(1), false, false),
            ("cargo test", None, None, None, false, true),
        ]);
    }

    #[test]
    fn opencode_conversations_read_from_messages_and_parts() {
        let user = r#"{"role":"user","time":{"created":1000}}"#;
        let bot = r#"{"role":"assistant","time":{"created":2000}}"#;
        let rows = parse_opencode(vec![
            (user, Some(r#"{"type":"text","text":"Fix the build"}"#)),
            (user, Some(r#"{"type":"text","synthetic":true,"text":"injected"}"#)),
            (bot, Some(r#"{"type":"step-start"}"#)),
            (bot, Some(r#"{"type":"reasoning","text":""}"#)),
            (bot, Some(r#"{"type":"text","text":"Looking."}"#)),
            (bot, Some(r#"{"type":"tool","tool":"edit","state":{"status":"completed","input":{"filePath":"/x/a.ts","oldString":"a","newString":"a\nb"},"time":{"start":2500}}}"#)),
            (bot, Some(r#"{"type":"tool","tool":"bash","state":{"status":"running","input":{"command":"npm test"}}}"#)),
            (bot, Some(r#"{"type":"tool","tool":"read","state":{"status":"error","input":{"filePath":"/x/b.ts"}}}"#)),
            (bot, None),
        ].into_iter());
        let view: Vec<_> = rows.iter().map(|r| (r.role, r.text.as_str(), r.tool.as_deref(), r.running, r.error, r.added, r.at)).collect();
        assert_eq!(view, vec![
            ("user", "Fix the build", None, false, false, None, Some(1000)),
            ("assistant", "Looking.", None, false, false, None, Some(2000)),
            ("tool", "/x/a.ts", Some("edit"), false, false, Some(2), Some(2500)),
            ("tool", "npm test", Some("bash"), true, false, None, Some(2000)),
            ("tool", "/x/b.ts", Some("read"), false, true, None, Some(2000)),
        ]);
    }

    #[test]
    fn copilot_events_read_as_a_conversation() {
        let rows = parse_copilot(&lines(concat!(
            r#"{"type":"session.start","data":{},"timestamp":"2026-11-14T10:00:00Z"}"#, "\n",
            r#"{"type":"user.message","data":{"content":"Add tests"},"timestamp":"2026-11-14T10:00:01Z"}"#, "\n",
            r#"{"type":"assistant.message","data":{"content":"On it.","toolRequests":[{"toolCallId":"t1","name":"bash","arguments":{"command":"cargo test"}}]},"timestamp":"2026-11-14T10:00:02Z"}"#, "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"t1","toolName":"bash","arguments":{"command":"cargo test"}}}"#, "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"t2","toolName":"edit","arguments":{"path":"src/a.rs","old_str":"x","new_str":"y\nz"}}}"#, "\n",
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"t1","success":false}}"#,
        )));
        let view: Vec<_> = rows.iter().map(|r| (r.role, r.text.as_str(), r.running, r.error, r.added)).collect();
        assert_eq!(view, vec![
            ("user", "Add tests", false, false, None),
            ("assistant", "On it.", false, false, None),
            ("tool", "cargo test", false, true, None),
            ("tool", "src/a.rs", true, false, Some(2)),
        ]);
    }

    #[test]
    fn gemini_chats_read_with_updates_by_id() {
        let rows = parse_gemini(concat!(
            r#"{"sessionId":"s","projectHash":"p","startTime":"2026-11-14T10:00:00Z","kind":"main"}"#, "\n",
            r#"{"id":"u0","timestamp":"2026-11-14T10:00:00Z","type":"user","content":[{"text":"<session_context>\nsetup"}]}"#, "\n",
            r#"{"id":"u1","timestamp":"2026-11-14T10:00:01Z","type":"user","content":[{"text":"Rename the module"}]}"#, "\n",
            r#"{"id":"g1","timestamp":"2026-11-14T10:00:02Z","type":"gemini","content":"","toolCalls":[{"id":"c","name":"run_shell_command","args":{"command":"ls"},"status":"executing"}]}"#, "\n",
            r#"{"id":"g1","timestamp":"2026-11-14T10:00:02Z","type":"gemini","content":"Done.","toolCalls":[{"id":"c","name":"run_shell_command","args":{"command":"ls"},"status":"success"}]}"#, "\n",
            r#"{"$set":{"lastUpdated":"2026-11-14T10:00:03Z"}}"#,
        ));
        let view: Vec<_> = rows.iter().map(|r| (r.role, r.text.as_str(), r.running)).collect();
        assert_eq!(view, vec![("user", "Rename the module", false), ("assistant", "Done.", false), ("tool", "ls", false)]);
    }

    /// Reads this machine's real OpenCode and Gemini history; run by hand.
    #[test]
    #[ignore]
    fn reads_local_history() {
        let home = home().unwrap();
        let db = home.join(".local/share/opencode/opencode.db");
        if db.is_file() {
            let output = std::process::Command::new("sqlite3")
                .args(["-readonly", db.to_str().unwrap(), "select id from session order by time_updated desc limit 1"])
                .output()
                .unwrap();
            let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let rows = read_opencode(&db, &id).unwrap();
            eprintln!("opencode {id}: {} rows", rows.len());
            let output = std::process::Command::new("sqlite3")
                .args(["-readonly", db.to_str().unwrap(), "select directory from session where parent_id is null order by time_updated desc limit 1"])
                .output()
                .unwrap();
            let folder = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let latest = read_latest("opencode", Path::new(&folder)).unwrap();
            eprintln!("opencode latest in {folder}: {} rows", latest.len());
            assert!(!latest.is_empty());
            for row in rows.iter().rev().take(8).rev() {
                eprintln!("  {} {:?} {:.70} run={} err={} +{:?}", row.role, row.tool, row.text.replace('\n', " "), row.running, row.error, row.added);
            }
            assert!(!rows.is_empty());
        }
        for path in sorted_dirs(&home.join(".cursor/projects")).into_iter().flat_map(|p| sorted_dirs(&p.join("agent-transcripts"))).take(400) {
            let Some(id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            let file = path.join(format!("{id}.jsonl"));
            if let Ok(lines) = tail_lines(&file) {
                let rows = parse_cursor(&lines);
                if rows.len() > 40 {
                    eprintln!("cursor {id}: {} rows", rows.len());
                    for row in rows.iter().rev().take(6).rev() {
                        eprintln!("  {} {:?} {:.70} +{:?}", row.role, row.tool, row.text.replace('\n', " "), row.added);
                    }
                    break;
                }
            }
        }
        for project in sorted_dirs(&home.join(".gemini/tmp")) {
            for path in std::fs::read_dir(project.join("chats")).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()) {
                let rows = parse_gemini(&std::fs::read_to_string(&path).unwrap());
                eprintln!("gemini {}: {} rows; id {:?}", path.display(), rows.len(), gemini_session_id(&path));
            }
        }
    }

    #[test]
    fn cursor_transcripts_read_as_a_conversation() {
        let rows = parse_cursor(&lines(concat!(
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<attached_files>x</attached_files>"},{"type":"text","text":"<timestamp>now</timestamp>\n<user_query>\nFix the header\n</user_query>"}]}}"#, "\n",
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","name":"StrReplace","input":{"path":"src/h.tsx","old_string":"a","new_string":"b\nc"}},{"type":"tool_use","name":"Shell","input":{"description":"Run tests","command":"npm test"}},{"type":"tool_use","name":"ApplyPatch","input":"*** Begin Patch\n*** Update File: src/x.ts\n-a\n+b\n*** End Patch"}]}}"#,
        )));
        let view: Vec<_> = rows.iter().map(|r| (r.role, r.text.as_str(), r.added, r.removed)).collect();
        assert_eq!(view, vec![
            ("user", "Fix the header", None, None),
            ("assistant", "Looking.", None, None),
            ("tool", "src/h.tsx", Some(2), Some(1)),
            ("tool", "npm test", None, None),
            ("tool", "src/x.ts", Some(1), Some(1)),
        ]);
    }

    #[test]
    fn this_machine_reads_the_whole_conversation_and_a_phone_its_end() {
        let dir = std::env::temp_dir().join(format!("transcript-extent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long.jsonl");
        let mut text = String::new();
        for n in 0..(MAX_ROWS + 50) {
            text.push_str(&format!(
                "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"message {n}\"}}}}\n"
            ));
        }
        std::fs::write(&path, text).unwrap();
        let source = Source { path: path.clone(), provider: "claude".into(), session: None };
        let full = read_source(&source, Extent::Full).unwrap();
        let recent = read_source(&source, Extent::Recent).unwrap();
        assert_eq!(full.len(), MAX_ROWS + 50);
        assert_eq!(full[0].text, "message 0");
        assert_eq!(recent.len(), MAX_ROWS);
        assert_eq!(recent.last().unwrap().text, format!("message {}", MAX_ROWS + 49));
        assert!(signature(&source).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn claude_resumes_from_the_spelling_its_folder_was_named_after() {
        // A project reached through a symlink in the home folder, its
        // conversation filed under the link's spelling.
        let home = std::env::temp_dir().join(format!("claude-resume-{}", std::process::id()));
        let real = home.join(".paradev").join("work");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(home.join(".paradev"), home.join(".superset")).ok();
        let linked = home.join(".superset").join("work");
        let projects = home.join(".claude").join("projects").join(claude_project_folder(&linked));
        std::fs::create_dir_all(&projects).unwrap();
        let file = projects.join("abc.jsonl");
        std::fs::write(&file, "{}").unwrap();
        let saved = std::env::var_os("HOME");
        // SAFETY: tests touching HOME in this module run one at a time below.
        unsafe { std::env::set_var("HOME", &home) };
        let found = claude_resume_dir(&file, &real);
        match saved {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        std::fs::remove_dir_all(&home).ok();
        let found = found.expect("the link's spelling");
        assert_eq!(claude_project_folder(&found), claude_project_folder(&linked));
    }

    /// This machine: `PROBE_ID=<claude conversation> PROBE_CWD=<tab folder>
    /// cargo test -p dragent_host -- --ignored claude_resume_probe --nocapture`
    #[test]
    #[ignore]
    fn claude_resume_probe() {
        let (Some(id), Some(cwd)) = (std::env::var_os("PROBE_ID"), std::env::var_os("PROBE_CWD")) else { return };
        let file = claude_conversation_file(&id.to_string_lossy()).expect("conversation file");
        let dir = claude_resume_dir(&file, Path::new(&cwd));
        eprintln!("file: {}\nrun in: {:?}", file.display(), dir);
        assert!(dir.is_some());
    }

    #[test]
    #[cfg(windows)]
    fn a_canonical_windows_path_names_the_folder_claude_made() {
        assert_eq!(
            claude_project_folder(Path::new(r"\\?\C:\Users\me\dragent-clean\dragent")),
            "C--Users-me-dragent-clean-dragent"
        );
        assert_eq!(
            claude_project_folder(Path::new(r"C:\Users\me\dragent-clean\dragent")),
            "C--Users-me-dragent-clean-dragent"
        );
    }

    #[test]
    #[cfg(windows)]
    fn transcripts_are_found_without_home() {
        // Windows apps started from the Start menu have no HOME.
        let saved = std::env::var_os("HOME");
        // SAFETY: the only test in this module touching HOME on Windows.
        unsafe { std::env::remove_var("HOME") };
        let found = home();
        if let Some(h) = saved {
            unsafe { std::env::set_var("HOME", h) };
        }
        assert_eq!(found, dirs::home_dir());
        assert!(found.is_some());
    }

    #[test]
    fn cursor_folders_are_named_after_the_cwd() {
        assert_eq!(cursor_project_folder(Path::new("/Users/me/.dragent/worktrees/Learning/hip-runner")), "Users-me-dragent-worktrees-Learning-hip-runner");
        assert_eq!(cursor_project_folder(Path::new("/private/tmp/x/-Users-me/abc")), "private-tmp-x-Users-me-abc");
    }

    #[test]
    fn claude_folders_are_named_after_the_cwd() {
        assert_eq!(
            claude_project_folder(Path::new("/Users/me/.dragent/worktrees/Terminal agent/app")),
            "-Users-me--dragent-worktrees-Terminal-agent-app"
        );
        assert!(!safe_id("../x") && !safe_id("") && safe_id("01a1c729-4a5c"));
    }
}

