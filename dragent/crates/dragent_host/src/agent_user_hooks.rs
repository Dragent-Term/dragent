//! Conversation reporting for agents that read hooks only from their
//! user-level config (Droid, Muse).
//!
//! Dragent merges one entry into that config. The entry runs a shim in the
//! Dragent home, which runs the launch helper in ancestry mode: the hook's
//! nearest ancestor that is a registered Dragent launch monitor identifies the
//! pane. Agents started outside Dragent have no such ancestor, so the entry is
//! inert for them, and nothing depends on the hook's environment (Muse clears
//! it). A registration is valid only while its process start time matches, so
//! a recycled pid never inherits it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const ANCESTRY_HOOK_ARG: &str = "--dragent-agent-session-hook-ancestry";
const SHIM: &str = "agent-session-hook";
const REGISTRY: &str = "agent-monitors";
/// Events whose payload names the conversation (and marks it used).
const EVENTS: [&str; 2] = ["SessionStart", "UserPromptSubmit"];

/// A launch monitor, registered while its agent runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    pub organisation_id: String,
    pub pane: Uuid,
    pub generation: Uuid,
    /// `ps` start time, guarding against pid reuse.
    pub started: String,
}

/// One row of the process table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub started: String,
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// The agent's config file: `$HOME/<file>`, or `<XDG config dir>/<file>`.
pub fn config_path(file: &str, xdg: bool) -> Option<PathBuf> {
    let base = if xdg {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| user_home().map(|home| home.join(".config")))?
    } else {
        user_home()?
    };
    Some(base.join(file))
}

/// Add `command` to each reporting event of a hook config, keeping every
/// existing entry. `Ok(None)` when it is already present. A file that is not
/// a JSON object is an error: it is never replaced.
pub fn merge_hook(
    existing: Option<&str>,
    section: Option<&str>,
    seed: Option<&str>,
    command: &str,
) -> Result<Option<String>, String> {
    let mut root: serde_json::Value = match existing {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str(text).map_err(|e| format!("unreadable hook config: {e}"))?
        }
        _ => match seed {
            Some(seed) => serde_json::from_str(seed).map_err(|e| e.to_string())?,
            None => serde_json::json!({}),
        },
    };
    let object = root.as_object_mut().ok_or("hook config is not an object")?;
    let events = match section {
        Some(section) => object
            .entry(section)
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or("hook section is not an object")?,
        None => object,
    };
    let mut changed = false;
    for event in EVENTS {
        let groups = events
            .entry(event)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or("hook event is not a list")?;
        let present = groups.iter().any(|group| {
            group["hooks"]
                .as_array()
                .is_some_and(|hooks| hooks.iter().any(|hook| hook["command"] == command))
        });
        if !present {
            groups.push(serde_json::json!({
                "hooks": [{"type": "command", "command": command, "timeout": 10}]
            }));
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    serde_json::to_string_pretty(&root)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Make sure the agent's user config reports to Dragent. Idempotent; the
/// file is rewritten only when the entry is missing.
pub fn ensure_installed(
    file: &str,
    xdg: bool,
    section: Option<&str>,
    seed: Option<&str>,
    helper: &Path,
) -> Result<(), String> {
    let shim = write_shim(helper)?;
    let command = shim_command(&shim);
    let path = config_path(file, xdg).ok_or("no home directory")?;
    let existing = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let Some(merged) = merge_hook(existing.as_deref(), section, seed, &command)? else {
        return Ok(());
    };
    let dir = path.parent().ok_or("hook config has no directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{}.dragent-{}", SHIM, Uuid::new_v4()));
    fs::write(&tmp, merged).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    if let Ok(meta) = fs::metadata(&path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Hook commands are run by a shell; quote only when the path needs it.
fn shim_command(shim: &Path) -> String {
    let path = shim.to_string_lossy();
    if path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        path.into_owned()
    } else {
        super::agent_sessions::quote(&path, "sh")
    }
}

/// The stable command the agent config names. It follows the current helper
/// executable and Dragent home, which may move between app versions.
fn write_shim(helper: &Path) -> Result<PathBuf, String> {
    let home = dragent_paths::home();
    let quote = |value: &str| super::agent_sessions::quote(value, "sh");
    let helper = quote(&helper.to_string_lossy());
    let script = format!(
        "#!/bin/sh\n\
         # Dragent: report this agent's conversation to the Dragent pane that\n\
         # launched it. Does nothing for agents started outside Dragent.\n\
         [ -x {helper} ] || exit 0\n\
         DRAGENT_HOME={} exec {helper} {ANCESTRY_HOOK_ARG}\n",
        quote(&home.to_string_lossy()),
    );
    let dir = home.join("bin");
    let shim = dir.join(SHIM);
    if fs::read_to_string(&shim).is_ok_and(|current| current == script) {
        return Ok(shim);
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{SHIM}.{}", Uuid::new_v4()));
    let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(script.as_bytes())
        .map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, &shim).map_err(|e| e.to_string())?;
    Ok(shim)
}

fn registry() -> PathBuf {
    dragent_paths::home().join(REGISTRY)
}

/// Register the calling monitor for the lifetime of its agent. Entries of
/// monitors that died without unregistering are pruned here.
pub fn register(monitor: &Monitor) -> Result<(), String> {
    let dir = registry();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let pid = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|pid| pid.parse::<u32>().ok());
            if pid.is_some_and(|pid| !alive(pid)) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let pid = std::process::id();
    let tmp = dir.join(format!(".{pid}.{}", Uuid::new_v4()));
    fs::write(
        &tmp,
        serde_json::to_vec(monitor).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(&tmp, dir.join(format!("{pid}.json"))).map_err(|e| e.to_string())
}

pub fn unregister() {
    let _ = fs::remove_file(registry().join(format!("{}.json", std::process::id())));
}

fn registered(proc: &Proc) -> Option<Monitor> {
    let bytes = fs::read(registry().join(format!("{}.json", proc.pid))).ok()?;
    let monitor: Monitor = serde_json::from_slice(&bytes).ok()?;
    (normalize(&monitor.started) == normalize(&proc.started)).then_some(monitor)
}

fn normalize(time: &str) -> String {
    time.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// `ps` formats start times per locale and time zone, and hooks may run with
/// a cleared environment: both sides of the comparison must use the same.
fn ps() -> Command {
    let mut command = Command::new("ps");
    command.env("LC_ALL", "C").env("TZ", "UTC");
    command
}

/// Start time of one process, as `ps` reports it.
pub fn process_started(pid: u32) -> Option<String> {
    let out = ps()
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let started = normalize(&String::from_utf8_lossy(&out.stdout));
    (!started.is_empty()).then_some(started)
}

/// Every process with its parent and start time.
pub fn process_table() -> Option<Vec<Proc>> {
    let out = ps()
        .args(["-A", "-o", "pid=,ppid=,lstart="])
        .output()
        .ok()?;
    Some(parse_process_table(&String::from_utf8_lossy(&out.stdout)))
}

/// `pid ppid <weekday> <month> <day> <time> <year>` rows.
fn parse_process_table(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            let started = fields.collect::<Vec<_>>().join(" ");
            (!started.is_empty()).then_some(Proc { pid, ppid, started })
        })
        .collect()
}

/// The registered monitor nearest above `pid`, if any.
pub fn owning_monitor(table: &[Proc], pid: u32) -> Option<Monitor> {
    nearest_ancestor(table, pid, registered)
}

fn nearest_ancestor<T>(table: &[Proc], pid: u32, lookup: impl Fn(&Proc) -> Option<T>) -> Option<T> {
    let parent_of = |pid: u32| table.iter().find(|p| p.pid == pid).map(|p| p.ppid);
    let mut current = parent_of(pid)?;
    // Bounded: a malformed table must not loop.
    for _ in 0..64 {
        if current <= 1 {
            return None;
        }
        let proc = table.iter().find(|p| p.pid == current)?;
        if let Some(found) = lookup(proc) {
            return Some(found);
        }
        current = proc.ppid;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: &str = "/home/u/.dragent/bin/agent-session-hook";

    #[test]
    fn merging_keeps_other_hooks_and_is_idempotent() {
        let existing = r#"{"schema_version":1,"managed_hooks_path":"/other/hooks.json",
            "hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"other"}]}]}}"#;
        let merged = merge_hook(Some(existing), Some("hooks"), None, COMMAND)
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(value["managed_hooks_path"], "/other/hooks.json");
        let start = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start[0]["hooks"][0]["command"], "other");
        assert_eq!(start[1]["hooks"][0]["command"], COMMAND);
        assert_eq!(
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            COMMAND
        );
        assert!(
            merge_hook(Some(&merged), Some("hooks"), None, COMMAND)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn new_files_are_seeded_and_events_may_be_top_level() {
        let seeded = merge_hook(
            None,
            Some("hooks"),
            Some(r#"{"schema_version":1}"#),
            COMMAND,
        )
        .unwrap()
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&seeded).unwrap();
        assert_eq!(value["schema_version"], 1);
        let top = merge_hook(Some("{}"), None, None, COMMAND)
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&top).unwrap();
        assert_eq!(value["SessionStart"][0]["hooks"][0]["command"], COMMAND);
    }

    #[test]
    fn unreadable_configs_are_never_replaced() {
        assert!(merge_hook(Some("{not json"), None, None, COMMAND).is_err());
        assert!(merge_hook(Some("[]"), None, None, COMMAND).is_err());
        assert!(merge_hook(Some(r#"{"hooks":[]}"#), Some("hooks"), None, COMMAND).is_err());
    }

    #[test]
    fn nearest_registered_ancestor_owns_the_hook() {
        let table = parse_process_table(
            "  1     0 Wed Sep 30 09:00:00 2026\n\
             100     1 Wed Sep 30 10:00:00 2026\n\
             200   100 Wed Sep 30 10:00:01 2026\n\
             300   200 Wed Sep 30 10:00:02 2026\n\
             400   300 Wed Sep 30 10:00:03 2026\n\
             500   400 Wed Sep 30 10:00:04 2026\n",
        );
        assert_eq!(table.len(), 6);
        assert_eq!(table[1].started, "Wed Sep 30 10:00:00 2026");
        // 500 = hook, 400 = agent, 300 = monitor, 200 = shell, 100 = outer monitor.
        let registered = [100u32, 300];
        let owner = |p: &Proc| registered.contains(&p.pid).then_some(p.pid);
        assert_eq!(nearest_ancestor(&table, 500, owner), Some(300));
        assert_eq!(nearest_ancestor(&table, 200, owner), Some(100));
        assert_eq!(nearest_ancestor(&table, 100, owner), None);
        assert_eq!(nearest_ancestor(&table, 999, owner), None);
    }
}
