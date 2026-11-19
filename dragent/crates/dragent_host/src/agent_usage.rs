//! How much of each agent account's plan limits is used, and when they reset.
//!
//! Nothing here signs in or calls a provider: each agent already reports its
//! limits locally, and Dragent only reads what it reports.
//!
//! - Claude Code hands its status-line command a JSON document whose
//!   `rate_limits` holds the 5-hour, 7-day and spend windows (subscribers
//!   only, after the first reply of a session). Sessions Dragent starts get a
//!   status-line command pointing at this helper, which saves those windows
//!   for the session's account and then runs the person's own status line,
//!   if they configured one, so it keeps showing exactly as before.
//! - Codex writes `rate_limits` into the session files under its home's
//!   `sessions` folder; the newest one is read.
//!
//! Readings are kept per account under Dragent's home and are never
//! uploaded.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The helper mode Claude Code runs as its status-line command.
pub const STATUS_LINE_ARG: &str = "--dragent-claude-status-line";

/// The person's own status line gets this long before it is left out.
const OWN_STATUS_LINE_TIMEOUT: Duration = Duration::from_secs(4);
/// Status-line input larger than this is not Claude Code's.
const MAX_INPUT: u64 = 4 * 1024 * 1024;

/// One limit window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// "5-hour", "Weekly", "Monthly spend"…
    pub label: String,
    /// 0–100.
    pub used_percent: f64,
    /// Unix seconds when the window starts over, when known.
    pub resets_at: Option<i64>,
}

/// One account's latest reading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// "claude" or "codex".
    pub provider: String,
    /// "system", or the account profile's id.
    pub account: String,
    pub windows: Vec<Window>,
    /// The plan the provider reported, when it did ("plus", "pro"…).
    #[serde(default)]
    pub plan: Option<String>,
    /// Unix seconds of the reading.
    pub updated_at: i64,
}

impl Usage {
    /// The most used window that has not reset yet.
    pub fn highest(&self, now: i64) -> Option<&Window> {
        self.windows
            .iter()
            .filter(|window| window.resets_at.is_none_or(|at| at > now))
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    }
}

/// The account key a session's binding is recorded under.
pub fn account_key(binding: Option<&crate::agent_accounts::Binding>) -> String {
    use crate::agent_accounts::Selection;
    match binding.map(|binding| &binding.selection) {
        Some(Selection::Profile(id)) => id.to_string(),
        _ => "system".into(),
    }
}

fn store() -> PathBuf {
    dragent_paths::home().join("agent-usage")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn safe_key(key: &str) -> String {
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}

fn save(usage: &Usage) -> Result<(), String> {
    let folder = store();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("{}-{}.json", usage.provider, safe_key(&usage.account)));
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(usage).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn saved(provider: &str, account: &str) -> Option<Usage> {
    let path = store().join(format!("{provider}-{}.json", safe_key(account)));
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

// ── Claude Code ──────────────────────────────────────────────────────────────

/// The windows in Claude Code's status-line input, or `None` when it
/// reported none (API-key sign-ins, or before the session's first reply).
pub fn claude_windows(input: &Value) -> Option<Vec<Window>> {
    let limits = input.get("rate_limits")?.as_object()?;
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "5-hour"), ("seven_day", "Weekly"), ("spend_limit", "Spend")] {
        let Some(window) = limits.get(key) else { continue };
        let Some(used) = window.get("used_percentage").and_then(Value::as_f64) else { continue };
        let label = match (key, window.get("period").and_then(Value::as_str)) {
            ("spend_limit", Some(period)) => format!("{} spend", capitalised(period)),
            _ => label.to_string(),
        };
        windows.push(Window {
            label,
            used_percent: used.clamp(0.0, 100.0),
            resets_at: window.get("resets_at").and_then(Value::as_i64),
        });
    }
    (!windows.is_empty()).then_some(windows)
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The status line the person configured for Claude Code, most specific
/// settings first, leaving out Dragent's own.
fn own_status_line(input: &Value) -> Option<String> {
    let project = input
        .pointer("/workspace/project_dir")
        .or_else(|| input.pointer("/workspace/current_dir"))
        .or_else(|| input.get("cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let user = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")));
    let mut files = Vec::new();
    if let Some(project) = &project {
        files.push(project.join(".claude/settings.local.json"));
        files.push(project.join(".claude/settings.json"));
    }
    if let Some(user) = user {
        files.push(user.join("settings.json"));
    }
    files.iter().find_map(|file| {
        let settings: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
        let line = settings.get("statusLine")?;
        if line.get("type").and_then(Value::as_str) != Some("command") {
            return None;
        }
        let command = line.get("command")?.as_str()?.trim();
        (!command.is_empty() && !command.contains(STATUS_LINE_ARG)).then(|| command.to_string())
    })
}

/// Run the person's status line with Claude Code's input; its output, or
/// nothing if it fails or takes too long.
fn run_own_status_line(command: &str, input: &[u8]) -> Vec<u8> {
    #[cfg(windows)]
    let mut child = Command::new("cmd");
    #[cfg(windows)]
    child.args(["/C", command]);
    #[cfg(not(windows))]
    let mut child = Command::new("sh");
    #[cfg(not(windows))]
    child.args(["-c", command]);
    let Ok(mut child) = child
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < OWN_STATUS_LINE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut output);
    }
    output
}

/// The status-line helper: save the account's limits, then show the
/// person's own status line. Never fails: Claude Code shows what it prints.
pub fn run_status_line(account: &str) -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    let Ok(document) = serde_json::from_slice::<Value>(&input) else {
        return 0;
    };
    if let Some(windows) = claude_windows(&document) {
        let _ = save(&Usage {
            provider: "claude".into(),
            account: account.to_string(),
            windows,
            plan: None,
            updated_at: now(),
        });
    }
    if let Some(command) = own_status_line(&document) {
        let output = run_own_status_line(&command, &input);
        let _ = std::io::stdout().write_all(&output);
    }
    0
}

/// The `--settings` value that makes a Claude Code session report to the
/// helper at `helper` (a shell command line, already quoted).
pub fn claude_settings(helper: &str) -> String {
    serde_json::json!({
        "statusLine": { "type": "command", "command": helper, "padding": 0 }
    })
    .to_string()
}

// ── Codex ────────────────────────────────────────────────────────────────────

/// The windows in one Codex `rate_limits` object.
pub fn codex_windows(limits: &Value) -> Vec<Window> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(*key)?;
            let used = window.get("used_percent").and_then(Value::as_f64)?;
            let minutes = window.get("window_minutes").and_then(Value::as_i64);
            Some(Window {
                label: minutes.map_or_else(|| "Limit".to_string(), window_label),
                used_percent: used.clamp(0.0, 100.0),
                resets_at: window.get("resets_at").and_then(Value::as_i64),
            })
        })
        .collect()
}

/// "5-hour", "Weekly", "Monthly", or "N-day"/"N-hour" for other lengths.
fn window_label(minutes: i64) -> String {
    match minutes {
        300 => "5-hour".into(),
        1440 => "Daily".into(),
        10080 => "Weekly".into(),
        43200 | 44640 => "Monthly".into(),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-minute"),
    }
}

/// The `rate_limits` object anywhere in one session-file line.
fn find_limits(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(map) => map
            .get("rate_limits")
            .filter(|limits| limits.is_object())
            .or_else(|| map.values().find_map(find_limits)),
        Value::Array(items) => items.iter().find_map(find_limits),
        _ => None,
    }
}

/// The newest session files under a Codex home, newest first.
fn codex_session_files(home: &Path, limit: usize) -> Vec<PathBuf> {
    // sessions/YYYY/MM/DD/*.jsonl: walk the dated folders newest first.
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
                let mut found: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&day)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                    .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
                    .collect();
                found.sort_by(|a, b| b.0.cmp(&a.0));
                files.extend(found.into_iter().map(|(_, path)| path));
                if files.len() >= limit {
                    files.truncate(limit);
                    return files;
                }
            }
        }
    }
    files
}

/// The latest limits Codex recorded under `home`.
pub fn codex_usage(home: &Path, account: &str) -> Option<Usage> {
    for file in codex_session_files(home, 5) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let latest = text.lines().rev().filter(|line| line.contains("\"rate_limits\"")).find_map(|line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let limits = find_limits(&value)?.clone();
            let windows = codex_windows(&limits);
            (!windows.is_empty()).then_some((limits, windows))
        });
        if let Some((limits, windows)) = latest {
            let updated_at = file
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or_else(now, |elapsed| elapsed.as_secs() as i64);
            return Some(Usage {
                provider: "codex".into(),
                account: account.to_string(),
                windows,
                plan: limits.get("plan_type").and_then(Value::as_str).map(str::to_string),
                updated_at,
            });
        }
    }
    None
}

/// Where Codex keeps an account's data: the profile's own folder, or the
/// system login's home.
pub(crate) fn codex_home(account: &str) -> Option<PathBuf> {
    if account == "system" {
        std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
    } else {
        Some(crate::agent_accounts::root().join(account))
    }
}

/// One account the usage view lists.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountUsage {
    pub provider: String,
    /// "system" or a profile id.
    pub account: String,
    /// "System login", or the profile's label.
    pub label: String,
    /// New sessions use this account by default.
    pub default: bool,
    pub usage: Option<Usage>,
}

/// Every Claude Code and Codex account on this machine with its latest
/// reading. Reads files only; call it off the UI thread.
pub fn accounts() -> Vec<AccountUsage> {
    use crate::agent_accounts::{Registry, Selection};
    let registry = Registry::load().unwrap_or_default();
    let mut rows = Vec::new();
    for provider in ["claude", "codex"] {
        let default = registry.defaults.get(provider).cloned().unwrap_or_default();
        let mut accounts = vec![("system".to_string(), "System login".to_string())];
        accounts.extend(
            registry
                .profiles
                .iter()
                .filter(|profile| profile.provider == provider)
                .map(|profile| (profile.id.to_string(), profile.label.clone())),
        );
        for (account, label) in accounts {
            let is_default = match &default {
                Selection::Profile(id) => id.to_string() == account,
                Selection::System | Selection::Inherit => account == "system",
            };
            let usage = match provider {
                "codex" => codex_home(&account).and_then(|home| codex_usage(&home, &account)),
                _ => saved(provider, &account),
            };
            rows.push(AccountUsage {
                provider: provider.into(),
                account,
                label,
                default: is_default,
                usage,
            });
        }
    }
    rows
}

/// Make `account` ("system" or a profile id) the one new `provider`
/// sessions use on this machine.
pub fn set_default(provider: &str, account: &str) -> Result<(), String> {
    use crate::agent_accounts::{Registry, Selection};
    let selection = if account == "system" {
        Selection::System
    } else {
        Selection::Profile(account.parse().map_err(|_| "Unknown account".to_string())?)
    };
    Registry::update(|registry| registry.set_default(provider, None, selection))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-usage-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_claude_code_status_line_limits() {
        let input = serde_json::json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600 },
                "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600 },
                "spend_limit": { "used_percentage": 62.8, "resets_at": 1740787200, "period": "monthly" }
            }
        });
        let windows = claude_windows(&input).unwrap();
        let labels: Vec<_> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5-hour", "Weekly", "Monthly spend"]);
        assert_eq!(windows[1].used_percent, 41.2);
        assert_eq!(windows[0].resets_at, Some(1738425600));
        // API-key sign-ins report no limits; nor does a session before its first reply.
        assert_eq!(claude_windows(&serde_json::json!({"model": {}})), None);
        assert_eq!(claude_windows(&serde_json::json!({"rate_limits": {}})), None);
    }

    #[test]
    fn reads_the_newest_codex_limits() {
        let home = scratch("codex");
        let day = home.join("sessions/2026/11/15");
        std::fs::create_dir_all(&day).unwrap();
        let older = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":10.0,"window_minutes":300,"resets_at":100},"secondary":null,"plan_type":"plus"}}}"#;
        let newer = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300,"resets_at":200},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":900},"plan_type":"plus"}}}"#;
        std::fs::write(day.join("rollout-a.jsonl"), format!("{{\"type\":\"session_meta\"}}\n{older}\n{newer}\n")).unwrap();
        let usage = codex_usage(&home, "system").unwrap();
        assert_eq!(usage.plan.as_deref(), Some("plus"));
        assert_eq!(
            usage.windows,
            [
                Window { label: "5-hour".into(), used_percent: 12.5, resets_at: Some(200) },
                Window { label: "Weekly".into(), used_percent: 40.0, resets_at: Some(900) },
            ]
        );
        assert_eq!(usage.highest(0).unwrap().label, "Weekly");
        // A window past its reset no longer counts.
        assert_eq!(usage.highest(1000), None);
    }

    #[test]
    fn window_lengths_read_as_people_say_them() {
        assert_eq!(window_label(300), "5-hour");
        assert_eq!(window_label(10080), "Weekly");
        assert_eq!(window_label(43200), "Monthly");
        assert_eq!(window_label(2880), "2-day");
    }

    #[test]
    fn the_status_line_setting_points_at_the_helper() {
        let settings: Value = serde_json::from_str(&claude_settings("'/bin/dragent' --x")).unwrap();
        assert_eq!(settings["statusLine"]["type"], "command");
        assert_eq!(settings["statusLine"]["command"], "'/bin/dragent' --x");
    }

    #[cfg(unix)]
    #[test]
    fn the_persons_own_status_line_still_shows() {
        let output = run_own_status_line("read input; echo \"mine:$input\"", b"{}\n");
        assert_eq!(String::from_utf8(output).unwrap(), "mine:{}\n");
        // One that hangs is left out rather than freezing Claude Code's footer.
        let started = std::time::Instant::now();
        assert!(run_own_status_line("sleep 30", b"").is_empty());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn dragent_never_runs_itself_as_the_persons_status_line() {
        let project = scratch("claude-project");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            project.join(".claude/settings.json"),
            format!(r#"{{"statusLine":{{"type":"command","command":"x {STATUS_LINE_ARG} system"}}}}"#),
        )
        .unwrap();
        std::fs::write(
            project.join(".claude/settings.local.json"),
            r#"{"statusLine":{"type":"command","command":"my-line"}}"#,
        )
        .unwrap();
        let input = serde_json::json!({ "workspace": { "project_dir": project } });
        assert_eq!(own_status_line(&input).as_deref(), Some("my-line"));
        std::fs::remove_file(project.join(".claude/settings.local.json")).unwrap();
        // Only Dragent's own entry in the project: fall through, never loop.
        assert_ne!(own_status_line(&input).as_deref().map(|c| c.contains(STATUS_LINE_ARG)), Some(true));
    }
}
