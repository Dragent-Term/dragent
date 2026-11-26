//! Keeps chosen MCP servers in other coding agents' own config files, so
//! Claude Code, Codex, Cursor, Gemini, Qwen and OpenCode can use the servers
//! set up in Settings → MCP servers, inside Dragent or not.
//!
//! Each server lists the agents it should reach (`sync_agents`). A sync
//! writes it into each of those agents' global MCP config and takes it out
//! again when the server is removed, disabled or the agent is unticked. Only
//! entries Dragent wrote are ever changed: an agent's own server with the
//! same name is left alone and reported. What Dragent wrote is recorded in
//! `mcp-agent-sync.json` beside `mcp.json`.
//!
//! Sign-in: a fixed token travels as an `Authorization` header. Servers
//! signed into through the browser get only their address; each agent signs
//! in to them itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::mcp_config::{McpServerEntry, McpTransport};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyncAgent {
    ClaudeCode,
    Codex,
    Cursor,
    Gemini,
    Qwen,
    OpenCode,
}

impl SyncAgent {
    pub const ALL: [SyncAgent; 6] = [
        SyncAgent::ClaudeCode,
        SyncAgent::Codex,
        SyncAgent::Cursor,
        SyncAgent::Gemini,
        SyncAgent::Qwen,
        SyncAgent::OpenCode,
    ];

    /// The name stored in `mcp.json`.
    pub fn key(self) -> &'static str {
        match self {
            SyncAgent::ClaudeCode => "claude",
            SyncAgent::Codex => "codex",
            SyncAgent::Cursor => "cursor",
            SyncAgent::Gemini => "gemini",
            SyncAgent::Qwen => "qwen",
            SyncAgent::OpenCode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SyncAgent::ClaudeCode => "Claude Code",
            SyncAgent::Codex => "Codex",
            SyncAgent::Cursor => "Cursor",
            SyncAgent::Gemini => "Gemini",
            SyncAgent::Qwen => "Qwen",
            SyncAgent::OpenCode => "OpenCode",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|agent| agent.key() == key)
    }

    /// Whether the agent has been used on this machine: its config folder
    /// exists. Cheap enough to call while drawing.
    pub fn is_present(self, home: &Path) -> bool {
        match self {
            SyncAgent::ClaudeCode => {
                home.join(".claude.json").exists() || home.join(".claude").is_dir()
            }
            SyncAgent::Codex => home.join(".codex").is_dir(),
            SyncAgent::Cursor => home.join(".cursor").is_dir(),
            SyncAgent::Gemini => home.join(".gemini").is_dir(),
            SyncAgent::Qwen => home.join(".qwen").is_dir(),
            SyncAgent::OpenCode => home.join(".config/opencode").is_dir(),
        }
    }

    /// The settings file of this agent that holds its MCP servers.
    pub fn settings_file(self, home: &Path) -> PathBuf {
        self.config_path(home)
    }

    fn config_path(self, home: &Path) -> PathBuf {
        match self {
            SyncAgent::ClaudeCode => home.join(".claude.json"),
            SyncAgent::Codex => home.join(".codex/config.toml"),
            SyncAgent::Cursor => home.join(".cursor/mcp.json"),
            SyncAgent::Gemini => home.join(".gemini/settings.json"),
            SyncAgent::Qwen => home.join(".qwen/settings.json"),
            SyncAgent::OpenCode => {
                let dir = home.join(".config/opencode");
                let jsonc = dir.join("opencode.jsonc");
                if jsonc.exists() && !dir.join("opencode.json").exists() {
                    jsonc
                } else {
                    dir.join("opencode.json")
                }
            }
        }
    }

    /// The top-level key holding the agent's MCP servers.
    fn servers_key(self) -> &'static str {
        match self {
            SyncAgent::Codex => "mcp_servers",
            SyncAgent::OpenCode => "mcp",
            _ => "mcpServers",
        }
    }
}

/// Labels for the agents a server is written into, in a fixed order.
pub fn agent_labels(keys: &[String]) -> Vec<&'static str> {
    SyncAgent::ALL
        .into_iter()
        .filter(|agent| keys.iter().any(|key| key == agent.key()))
        .map(SyncAgent::label)
        .collect()
}

/// What a sync changed and what it couldn't do.
#[derive(Debug, Default)]
pub struct SyncReport {
    /// Agents whose config file was rewritten.
    pub updated: Vec<SyncAgent>,
    pub problems: Vec<String>,
}

impl SyncReport {
    /// One line for the MCP page, or nothing when nothing happened.
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.updated.is_empty() {
            let names: Vec<_> = self.updated.iter().map(|agent| agent.label()).collect();
            parts.push(format!("Updated MCP servers in {}.", names.join(", ")));
        }
        parts.extend(self.problems.iter().cloned());
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    /// Agent key → names of the servers Dragent wrote into that agent.
    #[serde(default)]
    agents: BTreeMap<String, BTreeSet<String>>,
}

pub fn sync(entries: &[McpServerEntry], home: &Path, record_path: &Path) -> SyncReport {
    sync_selected(entries, home, record_path, &SyncAgent::ALL)
}

/// Sync only the agents selected for this launch; ownership of other agents
/// remains untouched. Startup can sync all present agents independently.
pub fn sync_selected(
    entries: &[McpServerEntry],
    home: &Path,
    record_path: &Path,
    selected: &[SyncAgent],
) -> SyncReport {
    sync_paths(entries, home, record_path, selected, &BTreeMap::new())
}

/// The same editor for isolated provider account configuration directories.
pub fn sync_paths(
    entries: &[McpServerEntry],
    home: &Path,
    record_path: &Path,
    selected: &[SyncAgent],
    paths: &BTreeMap<String, std::path::PathBuf>,
) -> SyncReport {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    #[cfg(not(target_family = "wasm"))]
    let _lock = match lock_record(record_path) {
        Ok(lock) => lock,
        Err(error) => {
            return SyncReport {
                problems: vec![format!("Couldn't lock MCP registration: {error}")],
                ..Default::default()
            };
        }
    };

    let mut record: Record = std::fs::read_to_string(record_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut report = SyncReport::default();
    for agent in SyncAgent::ALL
        .into_iter()
        .filter(|agent| selected.contains(agent))
    {
        let wanted: Vec<&McpServerEntry> = entries
            .iter()
            .filter(|entry| entry.enabled && entry.sync_agents.iter().any(|key| key == agent.key()))
            .collect();
        let owned = record.agents.get(agent.key()).cloned().unwrap_or_default();
        if wanted.is_empty() && owned.is_empty() {
            continue;
        }
        let path = paths
            .get(agent.key())
            .cloned()
            .unwrap_or_else(|| agent.config_path(home));
        match apply_to_file(agent, &path, &wanted, &owned) {
            Ok(outcome) => {
                if outcome.owned.is_empty() {
                    record.agents.remove(agent.key());
                } else {
                    record.agents.insert(agent.key().into(), outcome.owned);
                }
                if outcome.changed {
                    report.updated.push(agent);
                }
                for name in outcome.conflicts {
                    report.problems.push(format!(
                        "{} already has its own \"{name}\" server, so Dragent left it alone.",
                        agent.label()
                    ));
                }
            }
            Err(error) => report.problems.push(format!(
                "Couldn't update {} ({}): {error}",
                agent.label(),
                path.display()
            )),
        }
    }
    if let Ok(text) = serde_json::to_string_pretty(&record) {
        if let Some(parent) = record_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = write_replacing(record_path, &text) {
            report
                .problems
                .push(format!("Couldn't save MCP ownership record: {error}"));
        }
    }
    report
}

#[derive(Debug, Default, PartialEq)]
struct Outcome {
    /// Servers Dragent now has in the file.
    owned: BTreeSet<String>,
    changed: bool,
    /// Wanted servers the agent already had under its own entry.
    conflicts: Vec<String>,
}

/// Rewrite one agent's config file, retrying if the agent saves it while
/// Dragent is working so neither side's change is lost.
fn apply_to_file(
    agent: SyncAgent,
    path: &Path,
    wanted: &[&McpServerEntry],
    owned: &BTreeSet<String>,
) -> Result<Outcome, String> {
    // A dotfiles setup may link the file elsewhere; edit the real file.
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for _ in 0..3 {
        let modified = modified_time(&path);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        if text.is_none() && wanted.is_empty() {
            return Ok(Outcome::default());
        }
        let (output, outcome) = match agent {
            SyncAgent::Codex => edit_toml(text.as_deref(), wanted, owned)?,
            _ => edit_json(agent, text.as_deref(), wanted, owned)?,
        };
        if !outcome.changed {
            return Ok(outcome);
        }
        if modified_time(&path) != modified {
            continue;
        }
        write_replacing(&path, &output).map_err(|error| error.to_string())?;
        return Ok(outcome);
    }
    Err("the file kept changing while Dragent was saving it; try again".into())
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(not(target_family = "wasm"))]
fn lock_record(record: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = record.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(record.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

/// Atomically replace configs, retaining existing permissions. New files may
/// contain user-supplied environment variables or bearer tokens: keep private.
fn write_replacing(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("dragent-sync-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Which servers to delete, add or replace, given the names the file has.
fn plan<'a>(
    present: &BTreeSet<String>,
    wanted: &[&'a McpServerEntry],
    owned: &BTreeSet<String>,
) -> (Vec<String>, Vec<&'a McpServerEntry>, Outcome) {
    let wanted_names: BTreeSet<&str> = wanted.iter().map(|entry| entry.name.as_str()).collect();
    let remove: Vec<String> = owned
        .iter()
        .filter(|name| !wanted_names.contains(name.as_str()) && present.contains(*name))
        .cloned()
        .collect();
    let mut write = Vec::new();
    let mut outcome = Outcome::default();
    for entry in wanted {
        if present.contains(&entry.name) && !owned.contains(&entry.name) {
            outcome.conflicts.push(entry.name.clone());
        } else {
            outcome.owned.insert(entry.name.clone());
            write.push(*entry);
        }
    }
    (remove, write, outcome)
}

// ── JSON configs ─────────────────────────────────────────────────────────────

/// Edit the servers section of a JSON config. Every other top-level key keeps
/// its position and its exact original text.
fn edit_json(
    agent: SyncAgent,
    text: Option<&str>,
    wanted: &[&McpServerEntry],
    owned: &BTreeSet<String>,
) -> Result<(String, Outcome), String> {
    let source = text.unwrap_or("").trim();
    let mut top: IndexMap<String, Box<RawValue>> = if source.is_empty() {
        IndexMap::new()
    } else {
        serde_json::from_str(source)
            .map_err(|_| "it isn't plain JSON (comments can't be kept), so it was left as is")?
    };
    let key = agent.servers_key();
    let before: Map<String, Value> = match top.get(key) {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(Value::Object(map)) => map,
            _ => return Err(format!("its \"{key}\" isn't a list of servers")),
        },
        None => Map::new(),
    };
    let present: BTreeSet<String> = before.keys().cloned().collect();
    let (remove, write, mut outcome) = plan(&present, wanted, owned);
    let mut servers = before.clone();
    for name in &remove {
        servers.remove(name);
    }
    for entry in write {
        servers.insert(entry.name.clone(), json_entry(agent, entry));
    }
    outcome.changed = servers != before;
    if !outcome.changed {
        return Ok((text.unwrap_or("").to_string(), outcome));
    }
    let pretty = source.is_empty() || source.contains('\n');
    let section = if pretty {
        // Indent continuation lines to sit one level inside the file.
        serde_json::to_string_pretty(&Value::Object(servers))
            .map_err(|error| error.to_string())?
            .replace('\n', "\n  ")
    } else {
        serde_json::to_string(&Value::Object(servers)).map_err(|error| error.to_string())?
    };
    let section = RawValue::from_string(section).map_err(|error| error.to_string())?;
    top.insert(key.to_string(), section);
    let mut output = if pretty {
        serde_json::to_string_pretty(&top)
    } else {
        serde_json::to_string(&top)
    }
    .map_err(|error| error.to_string())?;
    output.push('\n');
    Ok((output, outcome))
}

/// One server in the shape a JSON-configured agent expects.
fn json_entry(agent: SyncAgent, entry: &McpServerEntry) -> Value {
    let mut out = Map::new();
    match entry.transport {
        McpTransport::Stdio => {
            let env: Map<String, Value> = entry
                .env
                .iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            if agent == SyncAgent::OpenCode {
                let mut command = vec![Value::String(entry.command.clone())];
                command.extend(entry.args.iter().cloned().map(Value::String));
                out.insert("type".into(), json!("local"));
                out.insert("command".into(), Value::Array(command));
                if !env.is_empty() {
                    out.insert("environment".into(), Value::Object(env));
                }
                out.insert("enabled".into(), json!(true));
                return Value::Object(out);
            }
            if agent == SyncAgent::ClaudeCode {
                out.insert("type".into(), json!("stdio"));
            }
            out.insert("command".into(), json!(entry.command));
            if !entry.args.is_empty() {
                out.insert("args".into(), json!(entry.args));
            }
            if !env.is_empty() {
                out.insert("env".into(), Value::Object(env));
            }
        }
        McpTransport::Sse => {
            let legacy = uses_legacy_sse(&entry.command);
            match agent {
                SyncAgent::ClaudeCode => {
                    out.insert("type".into(), json!(if legacy { "sse" } else { "http" }));
                    out.insert("url".into(), json!(entry.command));
                }
                SyncAgent::Gemini | SyncAgent::Qwen if !legacy => {
                    out.insert("httpUrl".into(), json!(entry.command));
                }
                SyncAgent::OpenCode => {
                    out.insert("type".into(), json!("remote"));
                    out.insert("url".into(), json!(entry.command));
                }
                _ => {
                    out.insert("url".into(), json!(entry.command));
                }
            }
            if let Some(headers) = auth_headers(entry) {
                out.insert("headers".into(), headers);
            }
            if agent == SyncAgent::OpenCode {
                out.insert("enabled".into(), json!(true));
            }
        }
    }
    Value::Object(out)
}

/// Endpoints that only speak the older SSE transport end in `/sse`; the
/// rest use streamable HTTP.
fn uses_legacy_sse(url: &str) -> bool {
    url.trim_end_matches('/').ends_with("/sse")
}

fn auth_headers(entry: &McpServerEntry) -> Option<Value> {
    let token = entry.bearer_token.as_deref()?.trim();
    (!token.is_empty()).then(|| json!({ "Authorization": format!("Bearer {token}") }))
}

// ── Codex (TOML) ─────────────────────────────────────────────────────────────

/// Edit `[mcp_servers]` in Codex's config.toml, keeping comments and layout.
fn edit_toml(
    text: Option<&str>,
    wanted: &[&McpServerEntry],
    owned: &BTreeSet<String>,
) -> Result<(String, Outcome), String> {
    let source = text.unwrap_or("");
    let mut doc: toml_edit::DocumentMut = source
        .parse()
        .map_err(|_| "it isn't valid TOML, so it was left as is")?;
    let present: BTreeSet<String> = doc
        .get("mcp_servers")
        .and_then(|item| item.as_table_like())
        .map(|table| table.iter().map(|(name, _)| name.to_string()).collect())
        .unwrap_or_default();
    let (remove, write, mut outcome) = plan(&present, wanted, owned);
    if remove.is_empty() && write.is_empty() {
        return Ok((source.to_string(), outcome));
    }
    if doc.get("mcp_servers").is_none() {
        let mut parent = toml_edit::Table::new();
        parent.set_implicit(true);
        doc.insert("mcp_servers", toml_edit::Item::Table(parent));
    }
    let servers = doc["mcp_servers"]
        .as_table_like_mut()
        .ok_or("its mcp_servers isn't a table")?;
    for name in &remove {
        servers.remove(name);
    }
    for entry in write {
        let rendered = toml_entry(entry);
        let unchanged = servers
            .get(&entry.name)
            .and_then(|item| item.as_table())
            .is_some_and(|existing| existing.to_string() == rendered.to_string());
        if !unchanged {
            servers.insert(&entry.name, toml_edit::Item::Table(rendered));
        }
    }
    let output = doc.to_string();
    outcome.changed = output != source;
    Ok((output, outcome))
}

fn toml_entry(entry: &McpServerEntry) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    match entry.transport {
        McpTransport::Stdio => {
            table.insert("command", toml_edit::value(entry.command.as_str()));
            if !entry.args.is_empty() {
                let args: toml_edit::Array = entry.args.iter().map(String::as_str).collect();
                table.insert("args", toml_edit::value(args));
            }
            if !entry.env.is_empty() {
                let mut env = toml_edit::Table::new();
                for (key, value) in entry.env.iter().collect::<BTreeMap<_, _>>() {
                    env.insert(key, toml_edit::value(value.as_str()));
                }
                table.insert("env", toml_edit::Item::Table(env));
            }
        }
        McpTransport::Sse => {
            table.insert("url", toml_edit::value(entry.command.as_str()));
            if let Some(token) = entry
                .bearer_token
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                let mut headers = toml_edit::Table::new();
                headers.insert("Authorization", toml_edit::value(format!("Bearer {token}")));
                table.insert("http_headers", toml_edit::Item::Table(headers));
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, agents: &[&str]) -> McpServerEntry {
        McpServerEntry {
            name: name.into(),
            command: "npx".into(),
            transport: McpTransport::Stdio,
            args: vec!["-y".into(), "pkg".into()],
            env: [("TOKEN".to_string(), "abc".to_string())]
                .into_iter()
                .collect(),
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: agents.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn remote(name: &str, url: &str, token: Option<&str>, agents: &[&str]) -> McpServerEntry {
        McpServerEntry {
            transport: McpTransport::Sse,
            command: url.into(),
            args: vec![],
            env: Default::default(),
            bearer_token: token.map(str::to_string),
            ..local(name, agents)
        }
    }

    fn owned(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn profile_sync_targets_only_the_selected_account_and_preserves_auth() {
        let home =
            std::env::temp_dir().join(format!("dragent-mcp-profile-{}", uuid::Uuid::new_v4()));
        let profile = home.join("account");
        std::fs::create_dir_all(&profile).unwrap();
        let path = profile.join("config.toml");
        std::fs::write(&path, "model = \"chosen\"\n").unwrap();
        std::fs::write(profile.join("auth.json"), "credential-placeholder").unwrap();
        let entry = local("dragent", &["codex"]);
        let record = profile.join("ownership.json");
        let paths = [("codex".into(), path.clone())].into_iter().collect();
        let report = sync_paths(&[entry], &home, &record, &[SyncAgent::Codex], &paths);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("mcp_servers.dragent")
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("chosen"));
        assert_eq!(
            std::fs::read_to_string(profile.join("auth.json")).unwrap(),
            "credential-placeholder"
        );
        assert!(!home.join(".codex/config.toml").exists());
        let report = sync_paths(&[], &home, &record, &[SyncAgent::Codex], &paths);
        assert!(report.problems.is_empty());
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("mcp_servers.dragent")
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn json_edits_keep_other_keys_in_place_and_text() {
        let file = "{\n  \"zeta\": {\"keep\":   1},\n  \"mcpServers\": {\n    \"mine\": {\"url\": \"https://x\"}\n  },\n  \"alpha\": 2\n}\n";
        let expo = remote("expo", "https://mcp.expo.dev/mcp", None, &["claude"]);
        let (out, outcome) =
            edit_json(SyncAgent::ClaudeCode, Some(file), &[&expo], &owned(&[])).unwrap();
        assert!(outcome.changed);
        assert_eq!(outcome.owned, owned(&["expo"]));
        assert!(out.find("\"zeta\"").unwrap() < out.find("\"mcpServers\"").unwrap());
        assert!(out.find("\"mcpServers\"").unwrap() < out.find("\"alpha\"").unwrap());
        assert!(
            out.contains("{\"keep\":   1}"),
            "untouched values keep their text"
        );
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed["mcpServers"]["expo"],
            json!({"type": "http", "url": "https://mcp.expo.dev/mcp"})
        );
        assert_eq!(parsed["mcpServers"]["mine"], json!({"url": "https://x"}));
    }

    #[test]
    fn an_agents_own_server_is_never_overwritten_or_removed() {
        let file = r#"{"mcpServers": {"github": {"url": "https://theirs"}}}"#;
        let github = remote("github", "https://ours", None, &["cursor"]);
        let (out, outcome) =
            edit_json(SyncAgent::Cursor, Some(file), &[&github], &owned(&[])).unwrap();
        assert_eq!(outcome.conflicts, vec!["github".to_string()]);
        assert!(!outcome.changed);
        assert!(out.contains("https://theirs"));

        let (out, outcome) = edit_json(SyncAgent::Cursor, Some(file), &[], &owned(&[])).unwrap();
        assert!(
            !outcome.changed && out.contains("github"),
            "only Dragent's entries are removed"
        );
    }

    #[test]
    fn dropped_servers_are_removed_and_tokens_become_headers() {
        let file = r#"{"mcpServers": {"old": {"url": "https://old"}, "keep": {"url": "k"}}}"#;
        let api = remote(
            "api",
            "https://api.example.com/sse",
            Some("t0k"),
            &["gemini"],
        );
        let (out, outcome) =
            edit_json(SyncAgent::Gemini, Some(file), &[&api], &owned(&["old"])).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["mcpServers"].get("old").is_none());
        assert!(parsed["mcpServers"].get("keep").is_some());
        assert_eq!(
            parsed["mcpServers"]["api"],
            json!({"url": "https://api.example.com/sse", "headers": {"Authorization": "Bearer t0k"}})
        );
        assert_eq!(outcome.owned, owned(&["api"]));
    }

    #[test]
    fn each_agent_gets_its_own_shape() {
        let stdio = local("fs", &[]);
        let http = remote("web", "https://mcp.example.com/mcp", None, &[]);
        assert_eq!(
            json_entry(SyncAgent::OpenCode, &stdio),
            json!({"type": "local", "command": ["npx", "-y", "pkg"], "environment": {"TOKEN": "abc"}, "enabled": true})
        );
        assert_eq!(
            json_entry(SyncAgent::ClaudeCode, &stdio),
            json!({"type": "stdio", "command": "npx", "args": ["-y", "pkg"], "env": {"TOKEN": "abc"}})
        );
        assert_eq!(
            json_entry(SyncAgent::Qwen, &http),
            json!({"httpUrl": "https://mcp.example.com/mcp"})
        );
        assert_eq!(
            json_entry(SyncAgent::OpenCode, &http),
            json!({"type": "remote", "url": "https://mcp.example.com/mcp", "enabled": true})
        );
    }

    #[test]
    fn json_with_comments_is_left_alone() {
        let file = "{\n  // mine\n  \"mcp\": {}\n}";
        let fs = local("fs", &["opencode"]);
        assert!(edit_json(SyncAgent::OpenCode, Some(file), &[&fs], &owned(&[])).is_err());
    }

    #[test]
    fn codex_toml_keeps_comments_and_other_servers() {
        let file =
            "# my settings\nmodel = \"gpt\"\n\n[mcp_servers.node_repl]\ncommand = \"node\"\n";
        let fs = local("fs", &["codex"]);
        let (out, outcome) = edit_toml(Some(file), &[&fs], &owned(&[])).unwrap();
        assert!(outcome.changed);
        assert!(out.starts_with("# my settings\nmodel = \"gpt\""));
        assert!(out.contains("[mcp_servers.node_repl]"));
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(doc["mcp_servers"]["fs"]["command"].as_str(), Some("npx"));
        assert_eq!(
            doc["mcp_servers"]["fs"]["env"]["TOKEN"].as_str(),
            Some("abc")
        );

        let (again, outcome) = edit_toml(Some(&out), &[&fs], &owned(&["fs"])).unwrap();
        assert!(!outcome.changed, "a second sync changes nothing");
        assert_eq!(again, out);

        let (removed, _) = edit_toml(Some(&out), &[], &owned(&["fs"])).unwrap();
        assert!(!removed.contains("[mcp_servers.fs]"));
        assert!(removed.contains("[mcp_servers.node_repl]"));
    }

    #[test]
    fn sync_writes_records_and_cleans_up_through_files() {
        let root = std::env::temp_dir().join(format!("dragent-mcp-sync-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let record = root.join("dragent/mcp-agent-sync.json");
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(home.join(".cursor/mcp.json"), r#"{"mcpServers": {}}"#).unwrap();

        let expo = remote(
            "expo",
            "https://mcp.expo.dev/mcp",
            None,
            &["cursor", "codex"],
        );
        let report = sync(&[expo.clone()], &home, &record);
        assert_eq!(report.updated, vec![SyncAgent::Codex, SyncAgent::Cursor]);
        assert!(
            std::fs::read_to_string(home.join(".codex/config.toml"))
                .unwrap()
                .contains("[mcp_servers.expo]")
        );

        let disabled = McpServerEntry {
            enabled: false,
            ..expo
        };
        let report = sync(&[disabled], &home, &record);
        assert_eq!(report.updated, vec![SyncAgent::Codex, SyncAgent::Cursor]);
        let cursor: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap())
                .unwrap();
        assert_eq!(cursor["mcpServers"], json!({}));
        assert!(sync(&[], &home, &record).updated.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
