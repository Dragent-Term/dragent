//! Tokens each coding agent used on this computer, by day, agent, model and
//! project, with what the same work would cost at API list prices.
//!
//! Every figure comes from the agents' own records on disk; nothing is sent
//! anywhere. Sources:
//! - Claude Code: each reply's `usage` in its conversation files.
//! - Codex: the running `total_token_usage` in its session files (the
//!   difference between consecutive readings is one turn's use).
//! - OpenCode and MiMo Code: each assistant message's `tokens` (and the
//!   `cost` they record, when they bill per token).
//! - GitHub Copilot CLI: its `assistant_usage_events` table.
//! - Gemini CLI and Qwen Code: replies carrying `tokens` in their chat files.
//!
//! Reading every conversation is slow the first time, so each file's daily
//! totals are cached under Dragent's home and only changed files are read
//! again.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// History kept and offered: 90 days.
pub const MAX_DAYS: i64 = 90;
/// Bump when what a cached file row means changes.
const CACHE_VERSION: u32 = 1;

// ── Rows ─────────────────────────────────────────────────────────────────────

/// Totals for one (agent, model, day, project).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub agent: String,
    pub model: String,
    /// Local calendar day, as days since 0001-01-01.
    pub day: i32,
    #[serde(default)]
    pub project: Option<String>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    #[serde(default)]
    pub cache_write_1h: u64,
    /// Replies counted.
    pub replies: u64,
    /// What the agent itself recorded spending, in dollars (per-token plans).
    #[serde(default)]
    pub recorded_cost: f64,
}

impl Row {
    fn key(&self) -> (String, String, i32, Option<String>) {
        (self.agent.clone(), self.model.clone(), self.day, self.project.clone())
    }
    fn add(&mut self, other: &Row) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.replies += other.replies;
        self.recorded_cost += other.recorded_cost;
    }
}

/// Merge rows with the same key.
fn merged(rows: impl IntoIterator<Item = Row>) -> Vec<Row> {
    let mut by_key: BTreeMap<(String, String, i32, Option<String>), Row> = BTreeMap::new();
    for row in rows {
        by_key
            .entry(row.key())
            .and_modify(|existing| existing.add(&row))
            .or_insert(row);
    }
    by_key.into_values().collect()
}

fn local_day(unix_seconds: i64) -> Option<i32> {
    use chrono::{Datelike, TimeZone};
    chrono::Local
        .timestamp_opt(unix_seconds, 0)
        .single()
        .map(|time| time.date_naive().num_days_from_ce())
}

/// Today, as days since 0001-01-01 in local time.
pub fn today() -> i32 {
    local_day(now()).unwrap_or_default()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Unix seconds from an RFC 3339 timestamp.
fn parse_time(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp())
}

fn as_u64(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

// ── Prices ───────────────────────────────────────────────────────────────────

/// Dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

/// Anthropic's published API list prices for Claude models. Cache writes
/// are 1.25× input (5-minute) or 2× input (1-hour). Models not listed, and
/// other providers' models, get no estimate rather than a guess.
pub fn price(model: &str) -> Option<Price> {
    let model = model.to_ascii_lowercase().replace('.', "-");
    let has = |name: &str| model.contains(name);
    let price = |input: f64, output: f64, cache_read: f64| Some(Price { input, output, cache_read });
    if has("fable-5-2") || has("mythos-5-2") {
        price(8.0, 40.0, 0.20)
    } else if has("fable-5-1") || has("mythos-5-1") {
        price(10.0, 50.0, 0.25)
    } else if has("fable-5") || has("mythos-5") {
        price(10.0, 50.0, 1.00)
    } else if has("opus-5-5") {
        price(4.0, 20.0, 0.20)
    } else if has("opus-5") || has("opus-4-8") || has("opus-4-7") || has("opus-4-6") {
        price(5.0, 25.0, 0.50)
    } else if has("sonnet-5-5") {
        price(2.0, 10.0, 0.10)
    } else if has("sonnet-5") {
        price(2.0, 10.0, 0.20)
    } else if has("sonnet-4-6") {
        price(3.0, 15.0, 0.30)
    } else if has("haiku-5-5") {
        price(0.10, 0.50, 0.01)
    } else if has("haiku-4-5") {
        price(1.0, 5.0, 0.10)
    } else {
        None
    }
}

/// What one row would cost at API list prices: `None` when the model has no
/// known price and the agent recorded none.
pub fn estimated_cost(row: &Row) -> Option<f64> {
    if row.recorded_cost > 0.0 {
        return Some(row.recorded_cost);
    }
    let price = price(&row.model)?;
    let million = 1_000_000.0;
    Some(
        (row.input as f64 * price.input
            + row.output as f64 * price.output
            + row.cache_read as f64 * price.cache_read
            + row.cache_write_5m as f64 * price.input * 1.25
            + row.cache_write_1h as f64 * price.input * 2.0)
            / million,
    )
}

/// What caching saved: cache reads billed as reads instead of full input.
pub fn cache_savings(row: &Row) -> Option<f64> {
    let price = price(&row.model)?;
    Some(row.cache_read as f64 * (price.input - price.cache_read) / 1_000_000.0)
}

// ── File cache ───────────────────────────────────────────────────────────────

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: HashMap<String, CachedFile>,
}

#[derive(Serialize, Deserialize)]
struct CachedFile {
    size: u64,
    modified: i64,
    rows: Vec<Row>,
}

fn cache_path() -> PathBuf {
    dragent_paths::home().join("usage").join("token-cache.json")
}

fn load_cache() -> Cache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|cache| cache.version == CACHE_VERSION)
        .unwrap_or_default()
}

fn save_cache(cache: &Cache) {
    let path = cache_path();
    let Some(folder) = path.parent() else { return };
    if std::fs::create_dir_all(folder).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(cache) else { return };
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

fn stamp(path: &Path) -> Option<(u64, i64)> {
    let meta = path.metadata().ok()?;
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((meta.len(), modified))
}

/// Rows for one file: from the cache when it is unchanged, else read.
fn file_rows(cache: &mut Cache, seen: &mut Vec<String>, path: &Path, read: impl FnOnce(&Path) -> Vec<Row>) -> Vec<Row> {
    let Some((size, modified)) = stamp(path) else { return Vec::new() };
    let key = path.to_string_lossy().into_owned();
    seen.push(key.clone());
    if let Some(cached) = cache.files.get(&key) {
        if cached.size == size && cached.modified == modified {
            return cached.rows.clone();
        }
    }
    let rows = merged(read(path));
    cache.files.insert(key, CachedFile { size, modified, rows: rows.clone() });
    rows
}

/// `.jsonl` files under `root` changed within the history window.
fn recent_files(root: &Path, extensions: &[&str], since: i64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()).is_some_and(|ext| extensions.contains(&ext))
                && stamp(&path).is_some_and(|(_, modified)| modified >= since)
            {
                files.push(path);
            }
        }
    }
    files
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    std::fs::File::open(path)
        .into_iter()
        .flat_map(|file| BufReader::new(file).lines().map_while(Result::ok))
}

// ── Claude Code ──────────────────────────────────────────────────────────────

fn claude_homes() -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
        .into_iter()
        .collect();
    homes.extend(account_profiles("claude"));
    homes
}

fn account_profiles(provider: &str) -> Vec<PathBuf> {
    crate::agent_accounts::Registry::load()
        .unwrap_or_default()
        .profiles
        .iter()
        .filter(|profile| profile.provider == provider)
        .map(|profile| crate::agent_accounts::root().join(profile.id.to_string()))
        .collect()
}

/// Claude Code repeats a reply's `usage` on every content block it writes
/// for it: each reply (message id) is counted once.
fn read_claude(path: &Path) -> Vec<Row> {
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for line in lines(path) {
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(message) = value.get("message") else { continue };
        let model = message.get("model").and_then(Value::as_str).unwrap_or_default();
        if model.is_empty() || model.starts_with('<') {
            continue;
        }
        let id = message.get("id").and_then(Value::as_str).unwrap_or_default();
        if !id.is_empty() && !seen.insert(id.to_string()) {
            continue;
        }
        let Some(usage) = message.get("usage") else { continue };
        let Some(day) = value.get("timestamp").and_then(Value::as_str).and_then(parse_time).and_then(local_day) else {
            continue;
        };
        let written = as_u64(usage.get("cache_creation_input_tokens"));
        let hour = as_u64(usage.pointer("/cache_creation/ephemeral_1h_input_tokens"));
        rows.push(Row {
            agent: "claude".into(),
            model: model.to_string(),
            day,
            project: value.get("cwd").and_then(Value::as_str).map(str::to_string),
            input: as_u64(usage.get("input_tokens")),
            output: as_u64(usage.get("output_tokens")),
            cache_read: as_u64(usage.get("cache_read_input_tokens")),
            cache_write_5m: written.saturating_sub(hour),
            cache_write_1h: hour,
            replies: 1,
            recorded_cost: 0.0,
        });
    }
    rows
}

// ── Codex ────────────────────────────────────────────────────────────────────

fn codex_homes() -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .into_iter()
        .collect();
    homes.extend(account_profiles("codex"));
    homes
}

/// One turn's use is the growth of the session's running total. Codex
/// counts cached input inside `input_tokens`; it is split out here.
fn read_codex(path: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    let (mut model, mut project) = (String::from("unknown"), None);
    let mut previous: Option<(u64, u64, u64, u64)> = None;
    for line in lines(path) {
        let interesting = line.contains("\"token_count\"") || line.contains("\"turn_context\"") || line.contains("\"session_meta\"");
        if !interesting {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(payload) = value.get("payload") else { continue };
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                project = payload.get("cwd").and_then(Value::as_str).map(str::to_string);
            }
            Some("turn_context") => {
                if let Some(name) = payload.get("model").and_then(Value::as_str) {
                    model = name.to_string();
                }
            }
            _ if payload.get("type").and_then(Value::as_str) == Some("token_count") => {
                let Some(total) = payload.pointer("/info/total_token_usage") else { continue };
                let reading = (
                    as_u64(total.get("input_tokens")),
                    as_u64(total.get("cached_input_tokens")),
                    as_u64(total.get("output_tokens")),
                    as_u64(total.get("reasoning_output_tokens")),
                );
                let before = previous.unwrap_or((0, 0, 0, 0));
                previous = Some(reading);
                // A repeated reading (no new turn) or a reset adds nothing.
                if reading.0 < before.0 || reading == before {
                    continue;
                }
                let Some(day) = value.get("timestamp").and_then(Value::as_str).and_then(parse_time).and_then(local_day) else {
                    continue;
                };
                let input = reading.0 - before.0;
                let cached = reading.1.saturating_sub(before.1).min(input);
                rows.push(Row {
                    agent: "codex".into(),
                    model: model.clone(),
                    day,
                    project: project.clone(),
                    input: input - cached,
                    output: reading.2.saturating_sub(before.2),
                    cache_read: cached,
                    replies: 1,
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    rows
}

// ── Gemini CLI / Qwen Code ───────────────────────────────────────────────────

/// Replies carrying a `tokens` object, wherever the chat file nests them.
fn read_gemini_family(agent: &'static str) -> impl Fn(&Path) -> Vec<Row> {
    move |path: &Path| {
        let mut rows = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let text = std::fs::read_to_string(path).unwrap_or_default();
        // Whole-file JSON, or one JSON value per line.
        let values: Vec<Value> = match serde_json::from_str::<Value>(&text) {
            Ok(value) => vec![value],
            Err(_) => text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect(),
        };
        fn walk(value: &Value, found: &mut Vec<Value>) {
            match value {
                Value::Object(map) => {
                    if map.get("tokens").is_some_and(Value::is_object) {
                        found.push(value.clone());
                    }
                    map.values().for_each(|child| walk(child, found));
                }
                Value::Array(items) => items.iter().for_each(|child| walk(child, found)),
                _ => {}
            }
        }
        let mut replies = Vec::new();
        values.iter().for_each(|value| walk(value, &mut replies));
        for reply in replies {
            let id = reply.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            if !id.is_empty() && !seen.insert(id) {
                continue;
            }
            let tokens = &reply["tokens"];
            let Some(day) = reply.get("timestamp").and_then(Value::as_str).and_then(parse_time).and_then(local_day) else {
                continue;
            };
            let cached = as_u64(tokens.get("cached"));
            rows.push(Row {
                agent: agent.into(),
                model: reply.get("model").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                day,
                project: None,
                input: as_u64(tokens.get("input")).saturating_sub(cached),
                output: as_u64(tokens.get("output")) + as_u64(tokens.get("thoughts")),
                cache_read: cached,
                replies: 1,
                ..Default::default()
            });
        }
        rows
    }
}

// ── Databases (OpenCode, MiMo Code, Copilot) ─────────────────────────────────

fn open_read_only(path: &Path) -> Option<diesel::SqliteConnection> {
    use diesel::Connection;
    if !path.is_file() {
        return None;
    }
    // In place and read-only: these databases can be many gigabytes and the
    // agent may be writing to them.
    let location = path.to_string_lossy().replace('?', "%3f").replace('#', "%23");
    diesel::SqliteConnection::establish(&format!("file:{location}?mode=ro")).ok()
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")))
}

fn read_opencode_family(agent: &str, database: &Path, since: i64) -> Vec<Row> {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Text};
    #[derive(QueryableByName)]
    struct Message {
        #[diesel(sql_type = BigInt)]
        time_created: i64,
        #[diesel(sql_type = Text)]
        data: String,
    }
    let Some(mut connection) = open_read_only(database) else { return Vec::new() };
    let messages: Vec<Message> = diesel::sql_query(
        "SELECT time_created, data FROM message WHERE time_created >= ? AND data LIKE '%\"tokens\"%'",
    )
    .bind::<BigInt, _>(since * 1000)
    .load(&mut connection)
    .unwrap_or_default();
    let mut rows = Vec::new();
    for message in messages {
        let Ok(data) = serde_json::from_str::<Value>(&message.data) else { continue };
        if data.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let tokens = &data["tokens"];
        let Some(day) = local_day(message.time_created / 1000) else { continue };
        rows.push(Row {
            agent: agent.into(),
            model: data.get("modelID").and_then(Value::as_str).unwrap_or("unknown").to_string(),
            day,
            project: data.pointer("/path/root").or_else(|| data.pointer("/path/cwd")).and_then(Value::as_str).map(str::to_string),
            input: as_u64(tokens.get("input")),
            output: as_u64(tokens.get("output")) + as_u64(tokens.get("reasoning")),
            cache_read: as_u64(tokens.pointer("/cache/read")),
            cache_write_5m: as_u64(tokens.pointer("/cache/write")),
            replies: 1,
            recorded_cost: data.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
            ..Default::default()
        });
    }
    rows
}

fn read_copilot(database: &Path, since: i64) -> Vec<Row> {
    use diesel::prelude::*;
    use diesel::sql_types::{BigInt, Nullable, Text};
    #[derive(QueryableByName)]
    struct Event {
        #[diesel(sql_type = Text)]
        model: String,
        #[diesel(sql_type = Nullable<BigInt>)]
        input_tokens: Option<i64>,
        #[diesel(sql_type = Nullable<BigInt>)]
        output_tokens: Option<i64>,
        #[diesel(sql_type = Nullable<BigInt>)]
        cache_read_tokens: Option<i64>,
        #[diesel(sql_type = Nullable<BigInt>)]
        cache_write_tokens: Option<i64>,
        #[diesel(sql_type = Nullable<BigInt>)]
        reasoning_tokens: Option<i64>,
        #[diesel(sql_type = Nullable<Text>)]
        created_at: Option<String>,
    }
    let Some(mut connection) = open_read_only(database) else { return Vec::new() };
    let events: Vec<Event> = diesel::sql_query(
        "SELECT model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, created_at \
         FROM assistant_usage_events",
    )
    .load(&mut connection)
    .unwrap_or_default();
    let positive = |value: Option<i64>| value.unwrap_or(0).max(0) as u64;
    events
        .into_iter()
        .filter_map(|event| {
            let at = event.created_at.as_deref().and_then(|text| {
                parse_time(text).or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok().map(|t| t.and_utc().timestamp())
                })
            })?;
            if at < since {
                return None;
            }
            Some(Row {
                agent: "copilot".into(),
                model: event.model,
                day: local_day(at)?,
                project: None,
                input: positive(event.input_tokens),
                output: positive(event.output_tokens) + positive(event.reasoning_tokens),
                cache_read: positive(event.cache_read_tokens),
                cache_write_5m: positive(event.cache_write_tokens),
                replies: 1,
                ..Default::default()
            })
        })
        .collect()
}

// ── Collecting ───────────────────────────────────────────────────────────────

/// Every agent's rows for the last [`MAX_DAYS`] days. Slow the first time
/// (it reads every recent conversation), quick after that. Blocking: call it
/// off the UI thread.
pub fn collect() -> Vec<Row> {
    let since = now() - MAX_DAYS * 86_400;
    let mut cache = load_cache();
    let mut seen = Vec::new();
    let mut rows = Vec::new();
    for home in claude_homes() {
        for file in recent_files(&home.join("projects"), &["jsonl"], since) {
            rows.extend(file_rows(&mut cache, &mut seen, &file, read_claude));
        }
    }
    for home in codex_homes() {
        for file in recent_files(&home.join("sessions"), &["jsonl"], since) {
            rows.extend(file_rows(&mut cache, &mut seen, &file, read_codex));
        }
    }
    for (agent, folder) in [("gemini", ".gemini/tmp"), ("qwen", ".qwen/tmp")] {
        let Some(root) = dirs::home_dir().map(|home| home.join(folder)) else { continue };
        let read = read_gemini_family(agent);
        for file in recent_files(&root, &["json", "jsonl"], since) {
            if file.components().any(|part| part.as_os_str() == "chats") {
                rows.extend(file_rows(&mut cache, &mut seen, &file, &read));
            }
        }
    }
    if let Some(data) = data_home() {
        rows.extend(read_opencode_family("opencode", &data.join("opencode/opencode.db"), since));
        rows.extend(read_opencode_family("mimo", &data.join("mimocode/mimocode.db"), since));
    }
    let copilot = std::env::var_os("COPILOT_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".copilot")));
    if let Some(copilot) = copilot {
        rows.extend(read_copilot(&copilot.join("session-store.db"), since));
    }
    // Forget files that are gone or fell out of the window.
    cache.files.retain(|key, _| seen.contains(key));
    cache.version = CACHE_VERSION;
    save_cache(&cache);
    let oldest = today() - MAX_DAYS as i32;
    merged(rows.into_iter().filter(|row| row.day > oldest))
}

// ── Summaries ────────────────────────────────────────────────────────────────

/// Totals of any group of rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub replies: u64,
    /// Estimated API cost of the priced part.
    pub cost: f64,
    /// Tokens whose model has no known price (left out of `cost`).
    pub unpriced_tokens: u64,
    pub cache_savings: f64,
}

impl Totals {
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
    fn add(&mut self, row: &Row) {
        self.input += row.input;
        self.output += row.output;
        self.cache_read += row.cache_read;
        self.cache_write += row.cache_write_5m + row.cache_write_1h;
        self.replies += row.replies;
        match estimated_cost(row) {
            Some(cost) => self.cost += cost,
            None => {
                self.unpriced_tokens += row.input + row.output + row.cache_read + row.cache_write_5m + row.cache_write_1h
            }
        }
        self.cache_savings += cache_savings(row).unwrap_or(0.0);
    }
}

/// Everything the usage page shows for one range of days.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// First day of the range (inclusive) and today.
    pub from: i32,
    pub to: i32,
    pub total: Totals,
    /// Per day in the range (oldest first), per agent.
    pub days: Vec<(i32, BTreeMap<String, Totals>)>,
    /// Largest first.
    pub agents: Vec<(String, Totals)>,
    pub models: Vec<(String, String, Totals)>,
    pub projects: Vec<(String, Totals)>,
}

/// Sum `rows` over the last `days` days (1 = today).
pub fn summarise(rows: &[Row], days: i32) -> Summary {
    let to = today();
    let from = to - days.max(1) + 1;
    let mut total = Totals::default();
    let mut per_day: BTreeMap<i32, BTreeMap<String, Totals>> = (from..=to).map(|day| (day, BTreeMap::new())).collect();
    let mut agents: HashMap<String, Totals> = HashMap::new();
    let mut models: HashMap<(String, String), Totals> = HashMap::new();
    let mut projects: HashMap<String, Totals> = HashMap::new();
    for row in rows.iter().filter(|row| row.day >= from && row.day <= to) {
        total.add(row);
        per_day.entry(row.day).or_default().entry(row.agent.clone()).or_default().add(row);
        agents.entry(row.agent.clone()).or_default().add(row);
        models.entry((row.agent.clone(), row.model.clone())).or_default().add(row);
        if let Some(project) = &row.project {
            projects.entry(project.clone()).or_default().add(row);
        }
    }
    let by_tokens = |a: &Totals, b: &Totals| b.tokens().cmp(&a.tokens());
    let mut agents: Vec<_> = agents.into_iter().collect();
    agents.sort_by(|a, b| by_tokens(&a.1, &b.1));
    let mut models: Vec<_> = models.into_iter().map(|((agent, model), totals)| (agent, model, totals)).collect();
    models.sort_by(|a, b| by_tokens(&a.2, &b.2));
    let mut projects: Vec<_> = projects.into_iter().collect();
    projects.sort_by(|a, b| by_tokens(&a.1, &b.1));
    Summary { from, to, total, days: per_day.into_iter().collect(), agents, models, projects }
}

/// "1.2M", "34.5K", "812".
pub fn short_count(count: u64) -> String {
    match count {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}K", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// "$12.34", "$0.42", "<$0.01".
pub fn dollars(amount: f64) -> String {
    if amount > 0.0 && amount < 0.01 {
        "<$0.01".into()
    } else if amount >= 1000.0 {
        format!("${:.0}", amount)
    } else {
        format!("${amount:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-tokens-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn claude_replies_count_once_with_cache_writes_split_by_duration() {
        let file = scratch("claude").join("a.jsonl");
        let reply = r#"{"type":"assistant","timestamp":"2026-11-15T10:00:00Z","cwd":"/repo","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":200,"cache_read_input_tokens":5000,"cache_creation_input_tokens":300,"cache_creation":{"ephemeral_1h_input_tokens":100,"ephemeral_5m_input_tokens":200}}}}"#;
        let synthetic = r#"{"type":"assistant","timestamp":"2026-11-15T10:00:00Z","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        std::fs::write(&file, format!("{reply}\n{reply}\n{synthetic}\n")).unwrap();
        let rows = merged(read_claude(&file));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!((row.input, row.output, row.cache_read), (10, 200, 5000));
        assert_eq!((row.cache_write_5m, row.cache_write_1h, row.replies), (200, 100, 1));
        assert_eq!(row.project.as_deref(), Some("/repo"));
        // $4 in, $20 out, $0.20 cache read, writes 1.25× / 2× input.
        let expected = (10.0 * 4.0 + 200.0 * 20.0 + 5000.0 * 0.20 + 200.0 * 5.0 + 100.0 * 8.0) / 1e6;
        assert!((estimated_cost(row).unwrap() - expected).abs() < 1e-12);
    }

    #[test]
    fn codex_turns_are_the_growth_of_the_running_total() {
        let file = scratch("codex").join("rollout.jsonl");
        let line = |input: u64, cached: u64, output: u64| {
            format!(r#"{{"timestamp":"2026-11-15T10:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":0}}}}}}}}"#)
        };
        let text = [
            r#"{"type":"session_meta","payload":{"cwd":"/repo"}}"#.to_string(),
            r#"{"type":"turn_context","payload":{"model":"gpt-x"}}"#.to_string(),
            line(1000, 200, 50),
            line(1000, 200, 50),
            line(3000, 1200, 80),
        ]
        .join("\n");
        std::fs::write(&file, text).unwrap();
        let rows = merged(read_codex(&file));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        // 1000 then +2000 input, of which 200 then +1000 cached: 800 + 1000 uncached.
        assert_eq!((row.input, row.cache_read, row.output, row.replies), (1800, 1200, 80, 2));
        assert_eq!(row.model, "gpt-x");
        assert_eq!(estimated_cost(row), None, "no guessed price for other providers' models");
    }

    #[test]
    fn gemini_replies_are_found_wherever_they_are_nested() {
        let file = scratch("gemini").join("chats").join("session.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"messages":[{"id":"1","type":"user"},{"id":"2","type":"gemini","model":"gemini-x","timestamp":"2026-11-15T10:00:00Z","tokens":{"input":900,"output":40,"cached":100,"thoughts":10}}]}"#,
        )
        .unwrap();
        let rows = read_gemini_family("gemini")(&file);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].input, rows[0].output, rows[0].cache_read), (800, 50, 100));
    }

    #[test]
    fn summaries_group_by_day_agent_model_and_project() {
        let today = today();
        let row = |agent: &str, model: &str, day: i32, output: u64| Row {
            agent: agent.into(),
            model: model.into(),
            day,
            project: Some("/repo".into()),
            output,
            replies: 1,
            ..Default::default()
        };
        let rows = vec![
            row("claude", "claude-sonnet-5-5", today, 1_000_000),
            row("codex", "gpt-x", today - 1, 500),
            row("claude", "claude-sonnet-5-5", today - 30, 7),
        ];
        let week = summarise(&rows, 7);
        assert_eq!(week.days.len(), 7);
        assert_eq!(week.total.output, 1_000_500, "the 30-day-old row is outside the week");
        assert_eq!(week.agents[0].0, "claude");
        assert!((week.total.cost - 10.0).abs() < 1e-9, "1M Sonnet 5.5 output tokens = $10");
        assert_eq!(week.total.unpriced_tokens, 500);
        assert_eq!(week.projects[0].0, "/repo");
        assert_eq!(summarise(&rows, 1).total.output, 1_000_000);
    }

    #[test]
    fn prices_follow_anthropics_published_rates() {
        assert_eq!(price("claude-opus-5-5"), Some(Price { input: 4.0, output: 20.0, cache_read: 0.20 }));
        assert_eq!(price("claude-opus-5-5[1m]").unwrap().input, 4.0);
        assert_eq!(price("claude-sonnet-4.6").unwrap().output, 15.0, "Copilot writes versions with dots");
        assert_eq!(price("claude-fable-5-2").unwrap().cache_read, 0.20);
        assert_eq!(price("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(price("gpt-6-luna"), None);
        assert_eq!(short_count(1_234_567), "1.2M");
        assert_eq!(dollars(0.004), "<$0.01");
    }
}
