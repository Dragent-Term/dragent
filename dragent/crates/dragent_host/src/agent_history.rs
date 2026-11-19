//! Earlier agent conversations started in a folder, for picking one up
//! again: Claude Code, Codex, Gemini, OpenCode, MiMo, Copilot and Cursor.
//! Each agent keeps its conversations on disk under its own home (Claude
//! Code and Codex per account profile when one is used); this reads only
//! what it needs for a title and never changes anything. An agent whose
//! history isn't kept in a form read here simply has no past sessions.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Bytes read from the start of a conversation to find its title.
const HEAD_BYTES: u64 = 256 * 1024;
/// …and from its end, where later titles are written.
const TAIL_BYTES: u64 = 64 * 1024;
/// Codex keeps every folder's conversations together: at most this many of
/// the newest are opened to find the folder's.
const CODEX_SCAN: usize = 600;
const TITLE_CHARS: usize = 120;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PastSession {
    /// The agent's command: "claude", "codex", "gemini", "opencode", "mimo",
    /// "copilot" or "cursor-agent".
    pub provider: String,
    /// The agent's own conversation id.
    pub id: String,
    /// What it was about: its summary, else the first thing the person asked.
    pub title: String,
    /// The folder it ran in, where it has to resume.
    pub cwd: PathBuf,
    /// Unix seconds of its last change.
    pub updated_at: i64,
    /// "system", or the account profile it belongs to.
    pub account: String,
    pub account_label: String,
    /// The files and folders that are this conversation (none for an agent
    /// that keeps every conversation in one database).
    pub files: Vec<PathBuf>,
    /// Bytes it takes on disk; `None` until measured (a database's take a
    /// moment, see [`measure`]).
    pub size: Option<u64>,
}

impl PastSession {
    /// The command line that continues it in `cwd`.
    pub fn resume_command(&self) -> String {
        match self.provider.as_str() {
            "codex" => format!("codex resume {}", self.id),
            "gemini" => format!("gemini --resume {}", self.id),
            "opencode" => format!("opencode --session {}", self.id),
            "mimo" => format!("mimo --session {}", self.id),
            "copilot" => format!("copilot --resume {}", self.id),
            "cursor-agent" => format!("cursor-agent --resume {}", self.id),
            _ => format!("claude --resume {}", self.id),
        }
    }
}

fn modified(path: &Path) -> i64 {
    path.metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn head_lines(path: &Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    BufReader::new(file.take(HEAD_BYTES)).lines().map_while(Result::ok).collect()
}

/// A title from what the person typed: one line, no markup-wrapped
/// commands, clipped.
fn title_from(text: &str) -> Option<String> {
    let text = text.trim();
    // Commands, attached context and system notes arrive wrapped in tags.
    if text.is_empty() || text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped: String = line.chars().take(TITLE_CHARS).collect();
    Some(if line.chars().count() > TITLE_CHARS { format!("{clipped}…") } else { clipped })
}

fn same_folder(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// The folders an account keeps its data in: (account key, label, home).
fn homes(provider: &str) -> Vec<(String, String, PathBuf)> {
    let system = match provider {
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".claude"))),
        _ => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex"))),
    };
    let mut homes: Vec<_> = system
        .map(|home| ("system".to_string(), "System login".to_string(), home))
        .into_iter()
        .collect();
    let registry = crate::agent_accounts::Registry::load().unwrap_or_default();
    homes.extend(
        registry
            .profiles
            .iter()
            .filter(|profile| profile.provider == provider)
            .map(|profile| {
                (
                    profile.id.to_string(),
                    profile.label.clone(),
                    crate::agent_accounts::root().join(profile.id.to_string()),
                )
            }),
    );
    homes
}

// ── Claude Code ──────────────────────────────────────────────────────────────

/// The last `TAIL_BYTES` of a file, as lines (the first, likely partial,
/// line dropped).
fn tail_lines(path: &Path) -> Vec<String> {
    use std::io::{Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return Vec::new() };
    let length = file.metadata().map_or(0, |meta| meta.len());
    if length <= HEAD_BYTES {
        return Vec::new();
    }
    if file.seek(SeekFrom::Start(length - TAIL_BYTES)).is_err() {
        return Vec::new();
    }
    let mut text = String::new();
    let _ = file.take(TAIL_BYTES).read_to_string(&mut text);
    text.lines().skip(1).map(str::to_string).collect()
}

/// One Claude Code conversation file: its title and the folder it ran in.
/// A name the person gave it comes first, then Claude Code's own title, then
/// a summary, then the first thing the person asked.
fn claude_entry(path: &Path) -> Option<(String, Option<PathBuf>)> {
    let (mut named, mut titled, mut summary, mut asked, mut cwd) = (None, None, None, None, None);
    for line in head_lines(path).into_iter().chain(tail_lines(path)) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        if cwd.is_none() {
            cwd = value.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        }
        let text = |key: &str| value.get(key).and_then(Value::as_str).and_then(title_from);
        match value.get("type").and_then(Value::as_str) {
            // Renamed or retitled later: the latest one wins.
            Some("custom-title") => named = text("customTitle").or(named),
            Some("ai-title") => titled = text("aiTitle").or(titled),
            Some("summary") if summary.is_none() => summary = text("summary"),
            Some("user") if asked.is_none() && value.get("isMeta") != Some(&Value::Bool(true)) => {
                let content = value.pointer("/message/content");
                asked = match content {
                    Some(Value::String(text)) => title_from(text),
                    Some(Value::Array(parts)) => parts
                        .iter()
                        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                        .find_map(|part| part.get("text").and_then(Value::as_str).and_then(title_from)),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    // A conversation nobody typed into is not worth resuming.
    asked.as_ref()?;
    Some((named.or(titled).or(summary).or(asked)?, cwd))
}

fn claude_sessions(folder: &Path, out: &mut Vec<PastSession>) {
    let spellings: Vec<PathBuf> = std::iter::once(folder.to_path_buf())
        .chain(folder.canonicalize().ok())
        .collect();
    for (account, label, home) in homes("claude") {
        let mut seen = std::collections::HashSet::new();
        for spelling in &spellings {
            let dir = home
                .join("projects")
                .join(crate::agent_transcript::claude_project_folder(spelling));
            if !seen.insert(dir.clone()) {
                continue;
            }
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "jsonl") {
                    continue;
                }
                let Some(id) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else {
                    continue;
                };
                let Some((title, cwd)) = claude_entry(&path) else { continue };
                out.push(PastSession {
                    provider: "claude".into(),
                    id,
                    title,
                    // Claude Code looks for it under the spelling it was started from.
                    cwd: cwd.unwrap_or_else(|| spelling.clone()),
                    updated_at: modified(&path),
                    account: account.clone(),
                    account_label: label.clone(),
                    files: [path.clone(), path.with_extension("")].into_iter().filter(|p| p.exists()).collect(),
                    size: None,
                });
            }
        }
    }
}

// ── Codex ────────────────────────────────────────────────────────────────────

/// One Codex conversation file: (id, folder, title).
fn codex_entry(path: &Path) -> Option<(String, PathBuf, String)> {
    let lines = head_lines(path);
    let meta: Value = serde_json::from_str(lines.first()?).ok()?;
    let payload = meta.get("payload")?;
    let id = payload.get("id")?.as_str()?.to_string();
    let cwd = PathBuf::from(payload.get("cwd")?.as_str()?);
    let title = lines.iter().skip(1).find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let payload = value.get("payload")?;
        (payload.get("type").and_then(Value::as_str) == Some("user_message"))
            .then(|| payload.get("message").and_then(Value::as_str).and_then(title_from))
            .flatten()
    })?;
    Some((id, cwd, title))
}

fn codex_files(home: &Path, limit: usize) -> Vec<PathBuf> {
    fn newest_dirs(path: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs.reverse();
        dirs
    }
    let mut files = Vec::new();
    for year in newest_dirs(&home.join("sessions")) {
        for month in newest_dirs(&year) {
            for day in newest_dirs(&month) {
                let mut found: Vec<PathBuf> = std::fs::read_dir(&day)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                    .collect();
                found.sort();
                found.reverse();
                files.extend(found);
                if files.len() >= limit {
                    files.truncate(limit);
                    return files;
                }
            }
        }
    }
    files
}

fn codex_sessions(folder: &Path, out: &mut Vec<PastSession>) {
    for (account, label, home) in homes("codex") {
        for path in codex_files(&home, CODEX_SCAN) {
            let Some((id, cwd, title)) = codex_entry(&path) else { continue };
            if !same_folder(&cwd, folder) {
                continue;
            }
            out.push(PastSession {
                provider: "codex".into(),
                id,
                title,
                cwd,
                updated_at: modified(&path),
                account: account.clone(),
                account_label: label.clone(),
                files: vec![path.clone()],
                size: None,
            });
        }
    }
}

// ── Gemini CLI ───────────────────────────────────────────────────────────────

/// An id an agent wrote is passed to a shell: letters, digits, `-` and `_` only.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn first_request(rows: Vec<crate::agent_transcript::Row>) -> Option<String> {
    rows.into_iter().filter(|row| row.role == "user").find_map(|row| title_from(&row.text))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).or_else(dirs::home_dir)
}

/// Gemini keeps a folder per project under `tmp/`, naming the project it
/// belongs to in `.project_root`; conversations are `chats/session-*.jsonl`,
/// whose first line holds the id.
fn gemini_sessions(folder: &Path, out: &mut Vec<PastSession>) {
    let Some(base) = home_dir().map(|home| home.join(".gemini").join("tmp")) else { return };
    let Ok(projects) = std::fs::read_dir(&base) else { return };
    for project in projects.filter_map(Result::ok).map(|entry| entry.path()) {
        let Ok(root) = std::fs::read_to_string(project.join(".project_root")) else { continue };
        if !same_folder(Path::new(root.trim()), folder) {
            continue;
        }
        let Ok(chats) = std::fs::read_dir(project.join("chats")) else { continue };
        for path in chats.filter_map(Result::ok).map(|entry| entry.path()) {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if !name.starts_with("session-") || !name.ends_with(".jsonl") {
                continue;
            }
            let lines = head_lines(&path);
            let Some(head) = lines.first().and_then(|line| serde_json::from_str::<Value>(line).ok()) else { continue };
            // Sub-agents keep chats of their own; only the person's are listed.
            if head.get("kind").and_then(Value::as_str).is_some_and(|kind| kind != "main") {
                continue;
            }
            let Some(id) = head.get("sessionId").and_then(Value::as_str).filter(|id| plain_id(id)) else { continue };
            let Some(title) = first_request(crate::agent_transcript::parse_gemini(&lines.join("\n"))) else { continue };
            out.push(PastSession {
                provider: "gemini".into(),
                id: id.to_string(),
                title,
                cwd: folder.to_path_buf(),
                updated_at: modified(&path),
                account: "system".into(),
                account_label: "System login".into(),
                files: vec![path.clone()],
                size: None,
            });
        }
    }
}

// ── OpenCode and MiMo ────────────────────────────────────────────────────────

/// Both keep every conversation in one SQLite database with a `session`
/// table; a sub-agent's conversation has a parent and isn't listed.
fn opencode_family_sessions(provider: &str, folder: &Path, out: &mut Vec<PastSession>) {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Nullable, Text};

    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        id: String,
        #[diesel(sql_type = Nullable<Text>)]
        title: Option<String>,
        #[diesel(sql_type = Text)]
        directory: String,
        #[diesel(sql_type = BigInt)]
        updated: i64,
    }
    let (dir, file) = if provider == "opencode" { ("opencode", "opencode.db") } else { ("mimocode", "mimocode.db") };
    let Some(db) = crate::agent_transcript::data_home().map(|data| data.join(dir).join(file)) else { return };
    if !db.is_file() {
        return;
    }
    // Read-only: the agent owns this database and may be writing to it.
    let url = format!("file:{}?mode=ro", crate::agent_transcript::uri_path(&db));
    let Ok(mut connection) = diesel::SqliteConnection::establish(&url) else { return };
    let rows: Vec<Row> = diesel::sql_query(
        "SELECT id, title, directory, time_updated AS updated FROM session \
         WHERE parent_id IS NULL AND time_archived IS NULL ORDER BY time_updated DESC LIMIT 400",
    )
    .load(&mut connection)
    .unwrap_or_default();
    for row in rows {
        if !plain_id(&row.id) || !same_folder(Path::new(&row.directory), folder) {
            continue;
        }
        let title = row.title.as_deref().and_then(title_from).unwrap_or_else(|| "Untitled session".into());
        out.push(PastSession {
            provider: provider.into(),
            id: row.id,
            title,
            cwd: folder.to_path_buf(),
            // Milliseconds in the database.
            updated_at: row.updated / 1000,
            account: "system".into(),
            account_label: "System login".into(),
            files: Vec::new(),
            size: None,
        });
    }
}

// ── Copilot ──────────────────────────────────────────────────────────────────

/// A top-level value of a simple YAML file: `key: value`, or a block
/// (`key: |-` followed by indented lines).
fn yaml_value(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix(key).and_then(|rest| rest.strip_prefix(':')) else { continue };
        let rest = rest.trim();
        if rest.starts_with('|') || rest.starts_with('>') {
            let block: Vec<&str> = lines
                .by_ref()
                .take_while(|line| line.starts_with(' ') || line.is_empty())
                .map(str::trim)
                .collect();
            return Some(block.join(" ").trim().to_string()).filter(|value| !value.is_empty());
        }
        let value = rest.trim_matches('"').trim_matches('\'').trim();
        return Some(value.to_string()).filter(|value| !value.is_empty());
    }
    None
}

/// `session-state/<id>/workspace.yaml` names the folder, and the name or
/// summary Copilot gave it; else the first request from its events.
fn copilot_sessions(folder: &Path, out: &mut Vec<PastSession>) {
    let base = std::env::var_os("COPILOT_HOME").map(PathBuf::from).or_else(|| home_dir().map(|home| home.join(".copilot")));
    let Some(state) = base.map(|base| base.join("session-state")) else { return };
    let Ok(entries) = std::fs::read_dir(&state) else { return };
    for dir in entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_dir()) {
        let Ok(text) = std::fs::read_to_string(dir.join("workspace.yaml")) else { continue };
        let Some(cwd) = yaml_value(&text, "cwd") else { continue };
        if !same_folder(Path::new(&cwd), folder) {
            continue;
        }
        let Some(id) = yaml_value(&text, "id").filter(|id| plain_id(id)) else { continue };
        let events = dir.join("events.jsonl");
        let title = yaml_value(&text, "name")
            .or_else(|| yaml_value(&text, "summary"))
            .and_then(|title| title_from(&title))
            .or_else(|| first_request(crate::agent_transcript::parse_copilot(&head_lines(&events))));
        let Some(title) = title else { continue };
        let updated = [modified(&events), modified(&dir.join("workspace.yaml"))].into_iter().max().unwrap_or(0);
        out.push(PastSession {
            provider: "copilot".into(),
            id,
            title,
            cwd: folder.to_path_buf(),
            updated_at: updated,
            account: "system".into(),
            account_label: "System login".into(),
            files: vec![dir.clone()],
            size: None,
        });
    }
}

// ── Cursor ───────────────────────────────────────────────────────────────────

/// `chats/<folder key>/<id>/meta.json`: title, folder and last change.
fn cursor_sessions(folder: &Path, out: &mut Vec<PastSession>) {
    let Some(chats) = home_dir().map(|home| home.join(".cursor").join("chats")) else { return };
    let Ok(groups) = std::fs::read_dir(&chats) else { return };
    for group in groups.filter_map(Result::ok).map(|entry| entry.path()) {
        let Ok(conversations) = std::fs::read_dir(&group) else { continue };
        for dir in conversations.filter_map(Result::ok).map(|entry| entry.path()) {
            let Ok(text) = std::fs::read_to_string(dir.join("meta.json")) else { continue };
            let Ok(meta) = serde_json::from_str::<Value>(&text) else { continue };
            if meta.get("hasConversation").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let Some(cwd) = meta.get("cwd").and_then(Value::as_str) else { continue };
            if !same_folder(Path::new(cwd), folder) {
                continue;
            }
            let Some(id) = dir.file_name().and_then(|name| name.to_str()).filter(|id| plain_id(id)) else { continue };
            let title = meta.get("title").and_then(Value::as_str).and_then(title_from).unwrap_or_else(|| "Untitled chat".into());
            let updated = meta.get("updatedAtMs").and_then(Value::as_i64).map_or_else(|| modified(&dir), |ms| ms / 1000);
            out.push(PastSession {
                provider: "cursor-agent".into(),
                id: id.to_string(),
                title,
                cwd: folder.to_path_buf(),
                updated_at: updated,
                account: "system".into(),
                account_label: "System login".into(),
                files: std::iter::once(dir.clone()).chain(cursor_transcript(id)).collect(),
                size: None,
            });
        }
    }
}

/// Cursor's transcript folder for chat `id`:
/// `projects/<folder key>/agent-transcripts/<id>`.
fn cursor_transcript(id: &str) -> Option<PathBuf> {
    let projects = home_dir()?.join(".cursor").join("projects");
    std::fs::read_dir(projects)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("agent-transcripts").join(id))
        .find(|path| path.is_dir())
}

// ── Space, and deleting ──────────────────────────────────────────────────────

/// Bytes under `paths` (files, and folders walked), not following links.
fn disk_size(paths: &[PathBuf]) -> u64 {
    fn walk(path: &Path) -> u64 {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
        if meta.is_dir() {
            std::fs::read_dir(path)
                .map(|entries| entries.filter_map(Result::ok).map(|entry| walk(&entry.path())).sum())
                .unwrap_or(0)
        } else {
            meta.len()
        }
    }
    paths.iter().map(|path| walk(path)).sum()
}

/// The database an OpenCode-family agent keeps every conversation in.
fn database(provider: &str) -> Option<PathBuf> {
    let (dir, file) = match provider {
        "opencode" => ("opencode", "opencode.db"),
        "mimo" => ("mimocode", "mimocode.db"),
        _ => return None,
    };
    crate::agent_transcript::data_home().map(|data| data.join(dir).join(file)).filter(|db| db.is_file())
}

fn connect(db: &Path, read_only: bool) -> Result<diesel::SqliteConnection, String> {
    use diesel::{Connection, RunQueryDsl};
    let mode = if read_only { "?mode=ro" } else { "" };
    let mut connection = diesel::SqliteConnection::establish(&format!("file:{}{mode}", crate::agent_transcript::uri_path(db)))
        .map_err(|error| format!("Couldn't open the agent's database: {error}"))?;
    // The agent may be writing at the same moment: wait for it briefly.
    let _ = diesel::sql_query("PRAGMA busy_timeout = 5000").execute(&mut connection);
    Ok(connection)
}

/// Fill in the size of conversations kept in a database. Each takes a
/// moment on a large database, so this runs after the list is shown.
pub fn measure(sessions: &mut [PastSession]) {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Text};

    #[derive(QueryableByName)]
    struct Bytes {
        #[diesel(sql_type = BigInt)]
        bytes: i64,
    }
    let mut connections: std::collections::HashMap<String, Option<diesel::SqliteConnection>> = Default::default();
    for session in sessions.iter_mut().filter(|session| session.size.is_none() && session.files.is_empty()) {
        let connection = connections
            .entry(session.provider.clone())
            .or_insert_with(|| database(&session.provider).and_then(|db| connect(&db, true).ok()));
        let Some(connection) = connection else { continue };
        // Each table's rows for this conversation; a table an older version
        // lacks just counts nothing.
        let mut total = 0u64;
        for (table, column) in [("event", "aggregate_id"), ("part", "session_id"), ("message", "session_id"), ("session_message", "session_id")] {
            let query = format!("SELECT COALESCE(SUM(octet_length(data)), 0) AS bytes FROM {table} WHERE {column} = ?");
            if let Ok(rows) = diesel::sql_query(query).bind::<Text, _>(&session.id).load::<Bytes>(connection) {
                total += rows.first().map_or(0, |row| row.bytes.max(0) as u64);
            }
        }
        session.size = Some(total);
    }
}

/// What deleting a conversation did.
#[derive(Clone, Debug, PartialEq)]
pub struct Deleted {
    /// Bytes given back to the disk now.
    pub freed: u64,
    /// For an agent that keeps one database: space now free inside it, given
    /// back to the disk by [`compact`].
    pub reusable: u64,
}

/// The folders an agent keeps its conversations in; nothing outside them is
/// ever deleted.
fn agent_roots(provider: &str) -> Vec<PathBuf> {
    let home = home_dir();
    match provider {
        "claude" | "codex" => homes(provider).into_iter().map(|(_, _, root)| root).collect(),
        "gemini" => home.map(|h| h.join(".gemini")).into_iter().collect(),
        "copilot" => std::env::var_os("COPILOT_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".copilot"))).into_iter().collect(),
        "cursor-agent" => home.map(|h| h.join(".cursor")).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn inside(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(path) = path.canonicalize() else { return false };
    roots.iter().filter_map(|root| root.canonicalize().ok()).any(|root| path.starts_with(&root) && path != root)
}

/// Delete one conversation for good: its files, or, for an agent that keeps
/// one database, through the agent's own `session delete`. Blocking; call it
/// off the UI thread.
pub fn delete(session: &PastSession) -> Result<Deleted, String> {
    if !plain_id(&session.id) {
        return Err("That conversation's id can't be used.".into());
    }
    if let Some(db) = database(&session.provider) {
        return delete_from_database(session, &db);
    }
    if session.files.is_empty() {
        return Err("Dragent doesn't know where this conversation is kept.".into());
    }
    let roots = agent_roots(&session.provider);
    if let Some(outside) = session.files.iter().find(|path| path.exists() && !inside(path, &roots)) {
        return Err(format!("{} isn't in the agent's own folder, so it was left alone.", outside.display()));
    }
    let freed = disk_size(&session.files);
    for path in session.files.iter().filter(|path| path.exists()) {
        let removed = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
        removed.map_err(|error| format!("Couldn't delete {}: {error}", path.display()))?;
    }
    Ok(Deleted { freed, reusable: 0 })
}

fn delete_from_database(session: &PastSession, db: &Path) -> Result<Deleted, String> {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Text};

    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let free_pages = |connection: &mut diesel::SqliteConnection| -> u64 {
        let pages = diesel::sql_query("SELECT freelist_count AS n FROM pragma_freelist_count()").load::<Count>(connection);
        let size = diesel::sql_query("SELECT page_size AS n FROM pragma_page_size()").load::<Count>(connection);
        match (pages, size) {
            (Ok(pages), Ok(size)) => pages.first().map_or(0, |p| p.n.max(0) as u64) * size.first().map_or(0, |s| s.n.max(0) as u64),
            _ => 0,
        }
    };
    let before = free_pages(&mut connect(db, true)?);
    // The agent's own delete keeps its database the way it expects.
    let mut child = crate::host_tools::command(&session.provider)
        .args(["session", "delete", &session.id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Couldn't run {}: {error}", session.provider))?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(120) {
            let _ = child.kill();
            return Err(format!("{} took too long to delete the conversation.", session.provider));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let mut connection = connect(db, false)?;
    let remaining = diesel::sql_query("SELECT COUNT(*) AS n FROM session WHERE id = ?")
        .bind::<Text, _>(&session.id)
        .load::<Count>(&mut connection)
        .map_err(|error| error.to_string())?
        .first()
        .map_or(0, |count| count.n);
    if remaining > 0 {
        let mut detail = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut detail);
        }
        let detail = detail.lines().filter(|line| !line.contains("AVX") && !line.contains("bun-")).last().unwrap_or("").trim().to_string();
        return Err(if detail.is_empty() {
            format!("{} didn't delete the conversation (exit {status}).", session.provider)
        } else {
            detail
        });
    }
    // Its log entries belong to the deleted conversation alone; the agent's
    // delete may leave them behind, and they are most of the space.
    let _ = diesel::sql_query("DELETE FROM event WHERE aggregate_id = ?").bind::<Text, _>(&session.id).execute(&mut connection);
    let after = free_pages(&mut connection);
    Ok(Deleted { freed: 0, reusable: after.saturating_sub(before) })
}

/// Space inside an agent's database that deleted conversations left free:
/// `(free bytes, database bytes)`.
pub fn reclaimable(provider: &str) -> Option<(u64, u64)> {
    use diesel::prelude::*;
    use diesel::sql_types::BigInt;

    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let db = database(provider)?;
    let mut connection = connect(&db, true).ok()?;
    let pages = diesel::sql_query("SELECT freelist_count AS n FROM pragma_freelist_count()").load::<Count>(&mut connection).ok()?;
    let size = diesel::sql_query("SELECT page_size AS n FROM pragma_page_size()").load::<Count>(&mut connection).ok()?;
    let free = pages.first()?.n.max(0) as u64 * size.first()?.n.max(0) as u64;
    Some((free, std::fs::metadata(&db).map_or(0, |meta| meta.len())))
}

/// The agent is running somewhere on this machine.
fn agent_running(provider: &str) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet).with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let own = std::process::id();
    system.processes().values().any(|process| {
        if process.pid().as_u32() == own {
            return false;
        }
        let named = |name: &std::ffi::OsStr| name.to_str().is_some_and(|name| name == provider || name == format!("{provider}.exe"));
        process.exe().and_then(Path::file_name).is_some_and(named)
            || process.cmd().first().and_then(|arg| Path::new(arg).file_name()).is_some_and(named)
            || process.name() == provider
    })
}

/// Give the space deleted conversations left inside an agent's database back
/// to the disk. Only while the agent isn't running: it rewrites the whole
/// file and needs that much free space while it does. Returns bytes freed.
pub fn compact(provider: &str) -> Result<u64, String> {
    use diesel::RunQueryDsl;
    let db = database(provider).ok_or("There's no database to compact.")?;
    if agent_running(provider) {
        return Err(format!("Close {provider} first: it's running, and its database can only be compacted while it's closed."));
    }
    let size = |db: &Path| {
        let wal = db.with_extension("db-wal");
        std::fs::metadata(db).map_or(0, |meta| meta.len()) + std::fs::metadata(wal).map_or(0, |meta| meta.len())
    };
    let before = size(&db);
    let mut connection = connect(&db, false)?;
    diesel::sql_query("VACUUM").execute(&mut connection).map_err(|error| format!("Compacting stopped: {error}. Nothing was lost."))?;
    let _ = diesel::sql_query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&mut connection);
    drop(connection);
    Ok(before.saturating_sub(size(&db)))
}

/// "2.3 GB", "840 MB", "12 KB".
pub fn bytes_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.0} MB", b / (KB * KB))
    } else {
        format!("{:.0} KB", (b / KB).max(1.0))
    }
}

/// Conversations started in any of `folders`, newest first. Reads files
/// only; call it off the UI thread.
pub fn list(folders: &[PathBuf], limit: usize) -> Vec<PastSession> {
    let mut sessions = Vec::new();
    let mut seen_folders: Vec<&PathBuf> = Vec::new();
    for folder in folders {
        if seen_folders.iter().any(|seen| same_folder(seen, folder)) {
            continue;
        }
        seen_folders.push(folder);
        claude_sessions(folder, &mut sessions);
        codex_sessions(folder, &mut sessions);
        gemini_sessions(folder, &mut sessions);
        opencode_family_sessions("opencode", folder, &mut sessions);
        opencode_family_sessions("mimo", folder, &mut sessions);
        copilot_sessions(folder, &mut sessions);
        cursor_sessions(folder, &mut sessions);
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let mut ids = std::collections::HashSet::new();
    sessions.retain(|session| ids.insert((session.provider.clone(), session.id.clone())));
    sessions.truncate(limit);
    for session in sessions.iter_mut().filter(|session| !session.files.is_empty()) {
        session.size = Some(disk_size(&session.files));
    }
    sessions
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago", "in March".
pub fn age(updated_at: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    age_at(updated_at, now)
}

fn age_at(updated_at: i64, now: i64) -> String {
    let seconds = (now - updated_at).max(0);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match seconds {
        0..60 => "just now".into(),
        60..3_600 => plural(seconds / 60, "minute"),
        3_600..86_400 => plural(seconds / 3_600, "hour"),
        86_400..2_592_000 => plural(seconds / 86_400, "day"),
        _ => plural(seconds / 2_592_000, "month"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_yaml_values_inline_and_in_blocks() {
        let text = "id: 7ded-1\ncwd: /Users/me/dev/app\nname: |-\n  Fix the login\n  redirect\nsummary: |-\nuser_named: false\n";
        assert_eq!(yaml_value(text, "id").as_deref(), Some("7ded-1"));
        assert_eq!(yaml_value(text, "cwd").as_deref(), Some("/Users/me/dev/app"));
        assert_eq!(yaml_value(text, "name").as_deref(), Some("Fix the login redirect"));
        // An empty block is no value, and the next key isn't swallowed.
        assert_eq!(yaml_value(text, "summary"), None);
        assert_eq!(yaml_value(text, "missing"), None);
        assert_eq!(yaml_value("cwd: \"/a b\"\n", "cwd").as_deref(), Some("/a b"));
    }

    #[test]
    fn nothing_outside_the_agents_own_folder_is_deleted() {
        let root = scratch("root");
        let inner = root.join("projects").join("a.jsonl");
        std::fs::create_dir_all(inner.parent().unwrap()).unwrap();
        std::fs::write(&inner, "x").unwrap();
        let outside = scratch("elsewhere").join("b.jsonl");
        std::fs::write(&outside, "y").unwrap();
        let roots = vec![root.clone()];
        assert!(inside(&inner, &roots));
        assert!(!inside(&outside, &roots));
        assert!(!inside(&root, &roots), "never the agent's whole folder");
        assert!(!inside(&root.join("projects/../../x"), &roots));
    }

    #[test]
    fn sizes_add_up_files_and_folders() {
        let dir = scratch("size");
        std::fs::write(dir.join("a"), vec![0u8; 1000]).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("b"), vec![0u8; 24]).unwrap();
        assert_eq!(disk_size(&[dir.clone()]), 1024);
        assert_eq!(disk_size(&[dir.join("a")]), 1000);
        assert_eq!(disk_size(&[dir.join("missing")]), 0);
        assert_eq!(bytes_label(5_671 * 1024 * 1024), "5.5 GB");
        assert_eq!(bytes_label(840 * 1024 * 1024), "840 MB");
        assert_eq!(bytes_label(10), "1 KB");
    }

    #[test]
    fn only_plain_ids_reach_a_shell() {
        for ok in ["ses_e31ad6c6bffezSS5sv7piLrtKq", "6ea29aea-972d-497f-afa0-0dd243db0094"] {
            assert!(plain_id(ok), "{ok}");
        }
        for bad in ["", "a b", "x;rm -rf ~", "$(id)", "../x", &"a".repeat(200)] {
            assert!(!plain_id(bad), "{bad}");
        }
    }

    #[test]
    fn every_agent_resumes_with_its_own_command() {
        let session = |provider: &str| PastSession {
            provider: provider.into(), id: "abc".into(), title: String::new(), cwd: PathBuf::new(),
            updated_at: 0, account: "system".into(), account_label: String::new(), ..Default::default()
        };
        assert_eq!(session("claude").resume_command(), "claude --resume abc");
        assert_eq!(session("codex").resume_command(), "codex resume abc");
        assert_eq!(session("gemini").resume_command(), "gemini --resume abc");
        assert_eq!(session("opencode").resume_command(), "opencode --session abc");
        assert_eq!(session("mimo").resume_command(), "mimo --session abc");
        assert_eq!(session("copilot").resume_command(), "copilot --resume abc");
        assert_eq!(session("cursor-agent").resume_command(), "cursor-agent --resume abc");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-history-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn claude_titles_come_from_the_summary_else_the_first_request() {
        let dir = scratch("claude");
        let plain = dir.join("a.jsonl");
        std::fs::write(
            &plain,
            concat!(
                r#"{"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>x</local-command-caveat>"},"cwd":"/repo"}"#, "\n",
                r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#, "\n",
                r#"{"type":"user","message":{"content":[{"type":"text","text":"Fix the   login\nbug please"}]}}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(claude_entry(&plain), Some(("Fix the login bug please".into(), Some("/repo".into()))));

        let summarised = dir.join("b.jsonl");
        std::fs::write(
            &summarised,
            concat!(
                r#"{"type":"summary","summary":"Login bug fix"}"#, "\n",
                r#"{"type":"user","message":{"content":"Fix it"},"cwd":"/repo"}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(claude_entry(&summarised).unwrap().0, "Login bug fix");

        let empty = dir.join("c.jsonl");
        std::fs::write(&empty, r#"{"type":"user","message":{"content":"<command-name>/model</command-name>"}}"#).unwrap();
        assert_eq!(claude_entry(&empty), None);
    }

    #[test]
    fn a_name_or_claude_codes_title_beats_the_first_request() {
        let dir = scratch("titled");
        let file = dir.join("t.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"type":"user","message":{"content":"This session is being continued from a previous conversation"},"cwd":"/repo"}"#, "\n",
                r#"{"type":"ai-title","aiTitle":"Old title"}"#, "\n",
                r#"{"type":"ai-title","aiTitle":"Fixing the logout bug"}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(claude_entry(&file).unwrap().0, "Fixing the logout bug");
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(r#"{"type":"custom-title","customTitle":"Auth work"}"#);
        text.push('\n');
        std::fs::write(&file, text).unwrap();
        assert_eq!(claude_entry(&file).unwrap().0, "Auth work");
    }

    #[test]
    fn codex_entries_name_their_folder_and_first_request() {
        let dir = scratch("codex");
        let file = dir.join("rollout.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"0199-abc","cwd":"/repo"}}"#, "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#, "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"Add a dark mode toggle"}}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(
            codex_entry(&file),
            Some(("0199-abc".into(), "/repo".into(), "Add a dark mode toggle".into()))
        );
    }

    #[test]
    fn resume_commands_and_long_titles() {
        let session = |provider: &str| PastSession {
            provider: provider.into(),
            id: "abc-1".into(),
            title: String::new(),
            cwd: "/repo".into(),
            updated_at: 0,
            account: "system".into(),
            account_label: "System login".into(),
            ..Default::default()
        };
        assert_eq!(session("claude").resume_command(), "claude --resume abc-1");
        assert_eq!(session("codex").resume_command(), "codex resume abc-1");
        let long = "word ".repeat(60);
        let title = title_from(&long).unwrap();
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), TITLE_CHARS + 1);
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age_at(1_000, 1_030), "just now");
        assert_eq!(age_at(1_000, 1_000 + 60), "1 minute ago");
        assert_eq!(age_at(1_000, 1_000 + 3 * 3_600), "3 hours ago");
        assert_eq!(age_at(1_000, 1_000 + 2 * 86_400), "2 days ago");
    }
}

#[cfg(test)]
mod probe {
    /// `DRAGENT_HISTORY_PROBE="/a:/b" cargo test -p dragent_host probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_this_machine() {
        let folders = std::env::var("DRAGENT_HISTORY_PROBE").unwrap_or_default();
        for folder in folders.split(':').filter(|f| !f.is_empty()) {
            let mut sessions = super::list(&[std::path::PathBuf::from(folder)], 500);
            let started = std::time::Instant::now();
            super::measure(&mut sessions);
            println!("  (measured databases in {:?})", started.elapsed());
            let mut by: std::collections::BTreeMap<String, u64> = Default::default();
            for s in &sessions { *by.entry(s.provider.clone()).or_default() += s.size.unwrap_or(0); }
            for (p, b) in &by { println!("  {p:13} {}", super::bytes_label(*b)); }
            if let Some(big) = sessions.iter().max_by_key(|s| s.size.unwrap_or(0)) { println!("  largest: {} {}", big.provider, super::bytes_label(big.size.unwrap_or(0))); }
            for p in ["opencode", "mimo"] { if let Some((free, total)) = super::reclaimable(p) { println!("  {p} database {} with {} free inside", super::bytes_label(total), super::bytes_label(free)); } }
            let mut counts = std::collections::BTreeMap::new();
            for s in &sessions {
                let entry = counts.entry(s.provider.clone()).or_insert((0, 0, i64::MAX, 0));
                entry.0 += 1;
                if !s.title.trim().is_empty() { entry.1 += 1; }
                entry.2 = entry.2.min(s.updated_at);
                entry.3 = entry.3.max(s.updated_at);
            }
            println!("{folder}");
            for (provider, (n, titled, oldest, newest)) in counts {
                println!("  {provider:13} {n:3} sessions, {titled} titled, newest {} | e.g. `{}`", super::age(newest), sessions.iter().find(|s| s.provider == provider).map(|s| s.resume_command()).unwrap_or_default());
                let _ = oldest;
            }
        }
    }
}

#[cfg(test)]
mod delete_probe {
    /// Lists and deletes a throwaway Claude Code conversation this test
    /// writes into the real `~/.claude/projects`; nothing else is touched.
    /// `cargo test -p dragent_host delete_probe -- --ignored`
    #[test]
    #[ignore]
    fn deletes_a_throwaway_claude_conversation() {
        let folder = std::env::temp_dir().join(format!("dragent-delete-check-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let home = super::homes("claude").into_iter().find(|(account, _, _)| account == "system").unwrap().2;
        let projects = home.join("projects").join(crate::agent_transcript::claude_project_folder(&folder));
        std::fs::create_dir_all(&projects).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let file = projects.join(format!("{id}.jsonl"));
        let line = serde_json::json!({"type": "user", "cwd": folder, "message": {"role": "user", "content": "throwaway delete check"}});
        std::fs::write(&file, format!("{line}\n").repeat(200)).unwrap();
        std::fs::create_dir_all(projects.join(&id).join("subagents")).unwrap();
        std::fs::write(projects.join(&id).join("subagents").join("a.jsonl"), "x".repeat(4096)).unwrap();

        let sessions = super::list(&[folder.clone()], 10);
        let session = sessions.iter().find(|s| s.id == id).expect("listed");
        assert_eq!(session.title, "throwaway delete check");
        assert_eq!(session.files.len(), 2, "the file and its folder");
        let size = session.size.unwrap();
        assert!(size > 4096);
        let deleted = super::delete(session).unwrap();
        assert_eq!(deleted.freed, size);
        assert!(!file.exists() && !projects.join(&id).exists());
        assert!(super::list(&[folder.clone()], 10).iter().all(|s| s.id != id));
        let _ = std::fs::remove_dir(&projects);
        let _ = std::fs::remove_dir(&folder);
    }
}
