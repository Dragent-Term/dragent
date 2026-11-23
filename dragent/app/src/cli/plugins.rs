//! `dragent plugins`: inspect, install, update, switch off and remove plugins
//! (packages of skills and MCP servers; see `dragent_plugins`).
//!
//! What runs is the person's call: a plugin's servers are added switched
//! off, an agent never switches one on, and an update that changes what a
//! server runs switches it off again and withdraws its approval. Server
//! entries change through the open app (which owns the list), or in its
//! files while it's closed.

use std::path::{Path, PathBuf};

use dragent_host::mcp_config::{McpServerEntry, McpTransport};
use dragent_host::outcome::{Failure, Status};
use dragent_plugins::package::{self, Inspection};
use dragent_plugins::servers::{self, Planned};
use dragent_plugins::skills::{self, Owned};
use dragent_plugins::{InstalledPlugin, Manifest, Record, Source, Transport};
use serde_json::{json, Value};

use super::{ask_app, ask_app_within, instance, pretty, requests, Flags};

const HELP: &str = "\
Plugins: a package of skills (instructions for agents) and MCP servers (tools).

Usage: dragent plugins <command>

  inspect SOURCE [--ref REF]   What a package brings, read without running anything:
                               its skills, the exact programs its servers run, the
                               settings and secrets they need
  install SOURCE [--ref REF]   Install it: skills go to ~/.agents/skills; servers are
                               added switched off (the person turns them on in
                               Settings → MCP and approves their tools there)
  list                         Installed plugins
  status [ID]                  Each step: package, skills, and per account each server:
                               configured, on, approved, connected, tools
  check ID --tool NAME [--args JSON] [--server NAME] [--account NAME]
                               Call one tool to prove it works (needs it on and approved)
  update ID [--from SOURCE] [--ref REF]
                               A new version; a server whose program changed is switched
                               off and its approval withdrawn
  disable ID | enable ID       Take its skills out (or back) and switch its servers off;
                               credentials stay
  accounts add ID NAME         Another account: its own copy of every server
  accounts remove ID NAME
  remove ID                    Remove it, its servers' entries, tokens and approvals
  sync                         Put back what's missing (server entries, skills)

SOURCE is a folder holding dragent-plugin.json, or a git repository address.
Files you changed in a plugin's skills are never overwritten or removed.";

pub fn run(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let json_out = flags.json || super::presentation::wants_json(args);
    let value = match flags.subcommand() {
        "help" => return Ok(HELP.into()),
        "inspect" => inspect(&flags)?,
        "install" | "add" => install(&flags)?,
        "list" | "ls" => list()?,
        "status" => status(flags.argument(0))?,
        "check" => check(&flags)?,
        "update" | "upgrade" => update(&flags)?,
        "disable" => switch(&flags, false)?,
        "enable" => switch(&flags, true)?,
        "accounts" | "account" => accounts(&flags)?,
        "remove" | "uninstall" => remove(&flags)?,
        "sync" => sync()?,
        other => {
            return Err(invalid(
                "unknown_command",
                format!("unknown plugins command “{other}”. Run `dragent plugins help`."),
            ))
        }
    };
    Ok(if json_out { pretty(value) } else { text(&value) })
}

fn invalid(code: &str, message: impl Into<String>) -> String {
    Failure::new(Status::InvalidInput, code, message)
        .next("`dragent plugins help` shows the commands.")
        .encode()
}

pub(crate) fn store() -> PathBuf {
    dragent_paths::home().join("plugins")
}

/// Where agents (Dragent's and others) find shared skills.
fn skills_root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".agents").join("skills")
}

/// Place a plugin's skills where agents find them. A Dev build never writes
/// into the shared skills folders other apps read: the plugin is still
/// recorded, but its skills stay out.
fn provision_shared_skills(
    root: &Path,
    package: &Path,
    manifest: &Manifest,
    previous: &Owned,
    reserved: &dyn Fn(&str) -> bool,
) -> (Owned, dragent_plugins::skills::Report) {
    if dragent_paths::is_dev() {
        log::info!("[plugins] Dev build: leaving the shared skills folders alone");
        return (previous.clone(), Default::default());
    }
    skills::provision(root, package, manifest, previous, reserved)
}

fn load() -> Result<Record, String> {
    Record::load(&store())
}

fn installed<'a>(record: &'a Record, id: &str) -> Result<&'a InstalledPlugin, String> {
    record.get(id).ok_or_else(|| {
        Failure::new(Status::InvalidTarget, "not_installed", format!("No plugin “{id}” is installed."))
            .next("`dragent plugins list` shows what's installed.")
            .encode()
    })
}

fn id_argument(flags: &Flags) -> Result<String, String> {
    flags
        .argument(0)
        .map(str::to_string)
        .ok_or_else(|| invalid("missing_argument", "name the plugin by its id"))
}

/// Names a plugin may not place skills under: Dragent's own, and those
/// another plugin placed.
fn reserved<'a>(record: &'a Record, id: &'a str) -> impl Fn(&str) -> bool + 'a {
    move |name: &str| {
        name == "dragent"
            || name.starts_with("dragent-")
            || dragent_host::agent_skills::BUNDLED_SKILLS
                .iter()
                .any(|skill| skill.name == name)
            || record.skill_owner(name).is_some_and(|owner| owner != id)
    }
}

// ── Sources ──────────────────────────────────────────────────────────────────

/// A package ready to read: a folder, or a repository cloned into a scratch
/// folder (removed when dropped). Cloning runs nothing from the repository.
struct Fetched {
    dir: PathBuf,
    source: Source,
    scratch: Option<PathBuf>,
}

impl Drop for Fetched {
    fn drop(&mut self) {
        if let Some(scratch) = &self.scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

fn is_repository(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("git@") || source.starts_with("ssh://")
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("couldn't run git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn fetch(source: &str, reference: Option<&str>) -> Result<Fetched, String> {
    if is_repository(source) {
        let scratch = store().join(format!(".fetch-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
        let mut args = vec!["clone", "--quiet", "--depth", "1"];
        if let Some(reference) = reference {
            args.extend(["--branch", reference]);
        }
        args.extend(["--", source, "package"]);
        // Owns the scratch folder from here: dropping it cleans up.
        let mut fetched = Fetched {
            dir: scratch.join("package"),
            source: Source {
                kind: "git".into(),
                location: source.into(),
                commit: None,
            },
            scratch: Some(scratch.clone()),
        };
        git(&scratch, &args).map_err(|error| {
            Failure::new(Status::Unavailable, "fetch_failed", format!("Couldn't get {source}: {error}"))
                .next("Check the address (and --ref), and that git can reach it.")
                .encode()
        })?;
        fetched.source.commit = git(&fetched.dir, &["rev-parse", "HEAD"]).ok();
        return Ok(fetched);
    }
    let dir = super::absolute(source);
    if !dir.is_dir() {
        return Err(invalid(
            "no_such_source",
            format!("{} isn't a folder or a git repository address.", dir.display()),
        ));
    }
    let dir = dir.canonicalize().unwrap_or(dir);
    Ok(Fetched {
        source: Source {
            kind: "folder".into(),
            location: dir.display().to_string(),
            commit: None,
        },
        dir,
        scratch: None,
    })
}

fn read_package(fetched: &Fetched) -> Result<(Manifest, Inspection), String> {
    let refuse = |error: String| {
        Failure::new(Status::InvalidInput, "invalid_package", error)
            .next("Fix the package's dragent-plugin.json (or ask its author); nothing was installed.")
            .encode()
    };
    let manifest = Manifest::read(&fetched.dir).map_err(refuse)?;
    let inspection = package::inspect(&fetched.dir).map_err(refuse)?;
    Ok((manifest, inspection))
}

// ── Server entries ───────────────────────────────────────────────────────────

/// Entries for `planned`, keeping what the person set on existing ones
/// (credentials, on/off) unless what a server runs changed: then it's off
/// and its approval goes (the second list).
fn entries_for(planned: &[Planned], existing: &[McpServerEntry]) -> (Vec<McpServerEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut revoke = Vec::new();
    for plan in planned {
        let old = existing.iter().find(|entry| entry.name == plan.entry);
        let transport = match plan.transport {
            Transport::Stdio => McpTransport::Stdio,
            Transport::Http => McpTransport::Sse,
        };
        let changed = old.is_some_and(|old| {
            old.command != plan.command || old.args != plan.args || old.transport != transport
        });
        if changed {
            revoke.push(plan.entry.clone());
        }
        let env = plan
            .env
            .iter()
            .map(|(key, default, _)| {
                let kept = old
                    .and_then(|old| old.env.get(key))
                    .filter(|value| !value.is_empty())
                    .cloned();
                (key.clone(), kept.unwrap_or_else(|| default.clone().unwrap_or_default()))
            })
            .collect();
        entries.push(McpServerEntry {
            name: plan.entry.clone(),
            command: plan.command.clone(),
            transport,
            args: plan.args.clone(),
            env,
            enabled: old.is_some_and(|old| old.enabled && !changed),
            bearer_token: old.and_then(|old| old.bearer_token.clone()),
            auth_url: old.and_then(|old| old.auth_url.clone()),
            token_url: old.and_then(|old| old.token_url.clone()),
            oauth_scope: old.and_then(|old| old.oauth_scope.clone()),
            sync_agents: old.map(|old| old.sync_agents.clone()).unwrap_or_default(),
        });
    }
    (entries, revoke)
}

fn current_entries() -> Vec<McpServerEntry> {
    crate::settings::mcp_config::load_entries()
}

/// Withdraw approvals in the settings file while the app is closed,
/// touching nothing else in it.
fn revoke_offline(names: &[String]) -> Result<(), String> {
    if names.is_empty() {
        return Ok(());
    }
    let path = crate::settings::AppSettings::config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|_| format!("{} isn't readable, so approvals weren't withdrawn", path.display()))?;
    if let Some(consents) = value
        .get_mut("plugin")
        .and_then(|plugin| plugin.get_mut("consents"))
        .and_then(Value::as_object_mut)
    {
        for name in names {
            consents.remove(name);
        }
    }
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| error.to_string())
}

/// Apply server entry changes: through the open app, or in its files.
fn apply(upsert: Vec<McpServerEntry>, remove: Vec<String>, revoke: Vec<String>) -> Result<(), String> {
    if upsert.is_empty() && remove.is_empty() && revoke.is_empty() {
        return Ok(());
    }
    if instance::app_running() {
        ask_app(requests::Request::PluginServers { upsert, remove, revoke }, false)?;
        return Ok(());
    }
    let mut entries = current_entries();
    entries.retain(|entry| !remove.contains(&entry.name));
    for entry in upsert {
        match entries.iter_mut().find(|old| old.name == entry.name) {
            Some(old) => *old = entry,
            None => entries.push(entry),
        }
    }
    crate::settings::mcp_config::save_entries(&entries);
    for name in &remove {
        crate::agent::tools_mcp_auth::clear_mcp_token(name);
    }
    let mut withdrawn = remove.clone();
    withdrawn.extend(revoke);
    revoke_offline(&withdrawn)
}

/// Entries in Settings → MCP already using a name `planned` needs: adding
/// the plugin's would take over a server the person set up.
fn names_taken(planned: &[Planned]) -> Vec<String> {
    let existing = current_entries();
    planned
        .iter()
        .filter(|plan| existing.iter().any(|entry| entry.name == plan.entry))
        .map(|plan| plan.entry.clone())
        .collect()
}

fn refuse_taken(taken: &[String]) -> Result<(), String> {
    if taken.is_empty() {
        return Ok(());
    }
    Err(Failure::new(
        Status::InvalidInput,
        "server_name_taken",
        format!(
            "Settings → MCP already has a server named {}: adding this would take it over.",
            taken.join(", ")
        ),
    )
    .next("Ask the person whether to rename or remove that server first; nothing was installed.")
    .encode())
}

// ── Commands ─────────────────────────────────────────────────────────────────

fn inspect(flags: &Flags) -> Result<Value, String> {
    let source = flags
        .argument(0)
        .ok_or_else(|| invalid("missing_argument", "name a folder or repository to inspect"))?;
    let fetched = fetch(source, flags.value("ref"))?;
    let (manifest, inspection) = read_package(&fetched)?;
    let record = load()?;
    let servers_taken = if record.get(&manifest.id).is_none() {
        names_taken(&servers::plan(&manifest, &fetched.dir, &["default".to_string()]))
    } else {
        Vec::new()
    };
    let taken = reserved(&record, &manifest.id);
    let collisions: Vec<String> = inspection
        .skills
        .iter()
        .filter(|skill| {
            taken(&skill.name)
                || (record.get(&manifest.id).is_none() && skills_root().join(&skill.name).exists())
        })
        .map(|skill| skill.name.clone())
        .collect();
    Ok(json!({
        "package": inspection,
        "source": fetched.source,
        "installed_version": record.get(&manifest.id).map(|plugin| plugin.version.clone()),
        "skill_names_taken": collisions,
        "server_names_taken": servers_taken,
        "note": "Nothing from the package ran. Installing adds its servers switched off: the person decides whether they run.",
    }))
}

fn install(flags: &Flags) -> Result<Value, String> {
    let source = flags
        .argument(0)
        .ok_or_else(|| invalid("missing_argument", "name a folder or repository to install"))?;
    let fetched = fetch(source, flags.value("ref"))?;
    let (manifest, inspection) = read_package(&fetched)?;
    let mut record = load()?;
    if record.get(&manifest.id).is_some() {
        return Err(Failure::new(
            Status::InvalidInput,
            "already_installed",
            format!("{} is already installed.", manifest.id),
        )
        .next(format!("`dragent plugins update {}` brings a new version.", manifest.id))
        .encode());
    }
    let accounts = vec!["default".to_string()];
    refuse_taken(&names_taken(&servers::plan(&manifest, &fetched.dir, &accounts)))?;
    let path = package::copy_into(&store(), &fetched.dir, &manifest)?;
    let (owned, skill_report) = provision_shared_skills(
        &skills_root(),
        &path,
        &manifest,
        &Owned::new(),
        &reserved(&record, &manifest.id),
    );
    let planned = servers::plan(&manifest, &path, &accounts);
    let (entries, _) = entries_for(&planned, &current_entries());
    let mut plugin = InstalledPlugin {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        source: fetched.source.clone(),
        path: path.clone(),
        installed_at_ms: now_ms(),
        enabled: true,
        skills: owned,
        accounts,
        unfinished: Vec::new(),
    };
    let applied = apply(entries, Vec::new(), Vec::new());
    if let Err(error) = &applied {
        plugin.unfinished.push(format!("servers not added: {error}"));
    }
    record.put(plugin);
    record.save(&store())?;
    if let Err(error) = applied {
        return Err(Failure::new(
            Status::Partial,
            "servers_not_added",
            format!("{} is installed and its skills are in place, but its servers weren't added: {error}", manifest.id),
        )
        .next(format!("`dragent plugins sync` tries again; `dragent plugins status {}`.", manifest.id))
        .encode());
    }
    Ok(json!({
        "installed": manifest.id,
        "version": manifest.version,
        "path": path,
        "skills": skill_report,
        "servers_not_for_this_system": inspection
            .servers
            .iter()
            .filter(|server| !server.runs_here)
            .map(|server| json!({ "name": server.name, "systems": server.systems }))
            .collect::<Vec<_>>(),
        "servers": inspection.servers.iter().filter(|server| server.runs_here).map(|server| json!({
            "entry": servers::entry_name(&manifest.id, &server.name, "default"),
            "runs": server.runs.replace("<plugin folder>", &path.display().to_string()),
            "settings": server.settings,
            "secrets": server.secrets,
            "on": false,
        })).collect::<Vec<_>>(),
        "next": "The person fills in its secrets, turns its servers on in Settings → MCP and approves their tools there; then `dragent plugins status` shows each step.",
    }))
}

fn list() -> Result<Value, String> {
    let record = load()?;
    Ok(json!(record
        .plugins
        .iter()
        .map(|plugin| json!({
            "id": plugin.id,
            "name": plugin.name,
            "version": plugin.version,
            "enabled": plugin.enabled,
            "source": plugin.source,
            "accounts": plugin.accounts,
            "unfinished": plugin.unfinished,
        }))
        .collect::<Vec<_>>()))
}

/// The state of each server entry: through the open app, or from its files
/// (where connection and tools aren't known).
fn server_states(names: &[String]) -> Vec<Value> {
    if instance::app_running() {
        if let Some(rows) = ask_app_within(
            requests::Request::PluginServerStatus { names: names.to_vec() },
            Some(std::time::Duration::from_secs(10)),
        )
        .ok()
        .flatten()
        .and_then(|response| response.data)
        .and_then(|data| data["servers"].as_array().cloned())
        {
            return rows;
        }
    }
    let entries = current_entries();
    let consents: Value = std::fs::read_to_string(crate::settings::AppSettings::config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|settings| settings["plugin"]["consents"].clone())
        .unwrap_or(Value::Null);
    names
        .iter()
        .map(|name| {
            let entry = entries.iter().find(|entry| &entry.name == name);
            json!({
                "name": name,
                "configured": entry.is_some(),
                "on": entry.is_some_and(|entry| entry.enabled),
                "approved": !consents[name].is_null(),
                "connected": Value::Null,
                "tools": Value::Null,
                "missing_settings": entry
                    .map(|entry| entry.env.iter().filter(|(_, value)| value.is_empty()).map(|(key, _)| key.clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
                "note": "Dragent isn't open: whether it connects isn't known.",
            })
        })
        .collect()
}

/// The first step a server still needs.
fn next_step(state: &Value) -> &'static str {
    if state["configured"] != true {
        "not_configured"
    } else if state["missing_settings"].as_array().is_some_and(|missing| !missing.is_empty()) {
        "needs_settings"
    } else if state["on"] != true {
        "off"
    } else if state["approved"] != true {
        "not_approved"
    } else if state["connected"] == false {
        "not_connected"
    } else if state["connected"] == true {
        "ready"
    } else {
        "unknown"
    }
}

fn plugin_status(plugin: &InstalledPlugin) -> Value {
    let root = skills_root();
    let skills: Vec<Value> = plugin
        .skills
        .iter()
        .map(|(name, files)| {
            let mut edited = Vec::new();
            let mut missing = Vec::new();
            for (file, recorded) in files {
                match std::fs::read(root.join(name).join(file)) {
                    Ok(bytes) if &skills::hash(&bytes) == recorded => {}
                    Ok(_) => edited.push(file.clone()),
                    Err(_) => missing.push(file.clone()),
                }
            }
            json!({ "name": name, "files": files.len(), "edited": edited, "missing": missing })
        })
        .collect();
    let manifest = Manifest::read(&plugin.path).ok();
    let planned = manifest
        .as_ref()
        .map(|manifest| servers::plan(manifest, &plugin.path, &plugin.accounts))
        .unwrap_or_default();
    let names: Vec<String> = planned.iter().map(|plan| plan.entry.clone()).collect();
    let states = server_states(&names);
    let accounts: Vec<Value> = plugin
        .accounts
        .iter()
        .map(|account| {
            let servers: Vec<Value> = planned
                .iter()
                .zip(&states)
                .filter(|(plan, _)| &plan.account == account)
                .map(|(plan, state)| {
                    let mut state = state.clone();
                    state["server"] = json!(plan.server);
                    state["next_step"] = json!(next_step(&state));
                    state
                })
                .collect();
            json!({ "account": account, "servers": servers })
        })
        .collect();
    json!({
        "id": plugin.id,
        "name": plugin.name,
        "version": plugin.version,
        "enabled": plugin.enabled,
        "package": { "path": plugin.path, "present": manifest.is_some(), "source": plugin.source },
        "skills": skills,
        "accounts": accounts,
        "unfinished": plugin.unfinished,
        "call_checked": "Run `dragent plugins check ID --tool NAME` with a read-only tool to prove a call works.",
    })
}

fn status(id: Option<&str>) -> Result<Value, String> {
    let record = load()?;
    match id {
        Some(id) => Ok(plugin_status(installed(&record, id)?)),
        None => Ok(json!(record.plugins.iter().map(plugin_status).collect::<Vec<_>>())),
    }
}

fn check(flags: &Flags) -> Result<Value, String> {
    let id = id_argument(flags)?;
    let record = load()?;
    let plugin = installed(&record, &id)?;
    let manifest = Manifest::read(&plugin.path)?;
    let tool = flags
        .value("tool")
        .ok_or_else(|| invalid("missing_argument", "name the tool to call with --tool (pick one that only reads)"))?;
    let server = match flags.value("server") {
        Some(server) => server.to_string(),
        None if manifest.servers.len() == 1 => manifest.servers.keys().next().cloned().unwrap_or_default(),
        None => {
            return Err(invalid(
                "missing_argument",
                format!(
                    "{id} has several servers: name one with --server ({})",
                    manifest.servers.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            ))
        }
    };
    let account = flags.value("account").unwrap_or("default");
    if !plugin.accounts.iter().any(|known| known == account) {
        return Err(invalid("unknown_account", format!("{id} has no account “{account}”.")));
    }
    let arguments: Value = match flags.value("args") {
        Some(text) => serde_json::from_str(text).map_err(|_| invalid("invalid_args", "--args takes a JSON object"))?,
        None => json!({}),
    };
    if !instance::app_running() {
        return Err(Failure::new(Status::Unavailable, "app_not_open", "Calling a tool needs Dragent open: it runs the servers.")
            .next("Ask the person to open Dragent.")
            .encode());
    }
    let entry = servers::entry_name(&id, &server, account);
    let response = ask_app_within(
        requests::Request::PluginCheck {
            server: entry.clone(),
            tool: tool.to_string(),
            arguments,
        },
        Some(std::time::Duration::from_secs(60)),
    )?
    .ok_or_else(|| Failure::new(Status::Timeout, "no_answer", "The tool didn't answer within a minute.").encode())?;
    let data = response.data.unwrap_or_default();
    let failed = data["failed"].as_bool().unwrap_or(true);
    Ok(json!({
        "plugin": id,
        "server": entry,
        "account": account,
        "tool": tool,
        "works": !failed,
        "output": data["output"],
    }))
}

fn update(flags: &Flags) -> Result<Value, String> {
    let id = id_argument(flags)?;
    let mut record = load()?;
    let before = installed(&record, &id)?.clone();
    let source = flags
        .value("from")
        .map(str::to_string)
        .unwrap_or_else(|| before.source.location.clone());
    let fetched = fetch(&source, flags.value("ref"))?;
    let (manifest, _) = read_package(&fetched)?;
    if manifest.id != id {
        return Err(invalid(
            "wrong_package",
            format!("That package is “{}”, not {id}.", manifest.id),
        ));
    }
    let path = package::copy_into(&store(), &fetched.dir, &manifest)?;
    let (owned, skill_report) = if before.enabled {
        provision_shared_skills(
            &skills_root(),
            &path,
            &manifest,
            &before.skills,
            &reserved(&record, &id),
        )
    } else {
        (before.skills.clone(), Default::default())
    };
    let old_entries = before.server_entries();
    let planned = servers::plan(&manifest, &path, &before.accounts);
    let (mut entries, revoke) = entries_for(&planned, &current_entries());
    if !before.enabled {
        for entry in &mut entries {
            entry.enabled = false;
        }
    }
    let gone: Vec<String> = old_entries
        .into_iter()
        .filter(|name| !planned.iter().any(|plan| &plan.entry == name))
        .collect();
    let applied = apply(entries, gone.clone(), revoke.clone());
    if before.path != path {
        let _ = std::fs::remove_dir_all(&before.path);
    }
    let mut plugin = before.clone();
    plugin.version = manifest.version.clone();
    plugin.name = manifest.name.clone();
    plugin.source = fetched.source.clone();
    plugin.path = path;
    plugin.skills = owned;
    plugin.unfinished.clear();
    if let Err(error) = &applied {
        plugin.unfinished.push(format!("servers not updated: {error}"));
    }
    record.put(plugin);
    record.save(&store())?;
    applied.map_err(|error| {
        Failure::new(Status::Partial, "servers_not_updated", format!("{id} is updated but its servers weren't: {error}"))
            .next("`dragent plugins sync` tries again.")
            .encode()
    })?;
    Ok(json!({
        "updated": id,
        "from": before.version,
        "to": manifest.version,
        "skills": skill_report,
        "switched_off_and_unapproved": revoke,
        "removed_servers": gone,
        "note": if revoke.is_empty() { "No server's program changed." } else { "These servers now run something different: the person turns them on and approves them again." },
    }))
}

fn switch(flags: &Flags, on: bool) -> Result<Value, String> {
    let id = id_argument(flags)?;
    let mut record = load()?;
    let mut plugin = installed(&record, &id)?.clone();
    let manifest = Manifest::read(&plugin.path)?;
    let report = if on {
        let (owned, report) = provision_shared_skills(
            &skills_root(),
            &plugin.path,
            &manifest,
            &plugin.skills,
            &reserved(&record, &id),
        );
        plugin.skills = owned;
        report
    } else {
        let (left, report) = skills::unprovision(&skills_root(), &plugin.skills);
        plugin.skills = left;
        report
    };
    // Switching on never starts a server: that's the person's step.
    let mut entries = Vec::new();
    if !on {
        let planned = servers::plan(&manifest, &plugin.path, &plugin.accounts);
        let (mut planned_entries, _) = entries_for(&planned, &current_entries());
        for entry in &mut planned_entries {
            entry.enabled = false;
        }
        entries = planned_entries;
    }
    apply(entries, Vec::new(), Vec::new())?;
    plugin.enabled = on;
    record.put(plugin);
    record.save(&store())?;
    Ok(json!({
        "plugin": id,
        "enabled": on,
        "skills": report,
        "note": if on {
            "Its skills are back. Its servers stay as they are: the person turns them on in Settings → MCP."
        } else {
            "Its skills are out and its servers off; its credentials and settings stay."
        },
    }))
}

fn accounts(flags: &Flags) -> Result<Value, String> {
    let action = flags.argument(0).unwrap_or_default().to_string();
    let id = flags
        .argument(1)
        .ok_or_else(|| invalid("missing_argument", "usage: dragent plugins accounts add|remove ID NAME"))?
        .to_string();
    let account = flags
        .argument(2)
        .ok_or_else(|| invalid("missing_argument", "usage: dragent plugins accounts add|remove ID NAME"))?
        .to_string();
    if !servers::valid_account(&account) {
        return Err(invalid("invalid_account", "an account name is 1–20 lowercase letters, digits and hyphens"));
    }
    let mut record = load()?;
    let mut plugin = installed(&record, &id)?.clone();
    let manifest = Manifest::read(&plugin.path)?;
    let planned = servers::plan(&manifest, &plugin.path, std::slice::from_ref(&account));
    match action.as_str() {
        "add" => {
            if plugin.accounts.contains(&account) {
                return Err(invalid("account_exists", format!("{id} already has the account {account}")));
            }
            refuse_taken(&names_taken(&planned))?;
            let (entries, _) = entries_for(&planned, &current_entries());
            apply(entries, Vec::new(), Vec::new())?;
            plugin.accounts.push(account.clone());
        }
        "remove" => {
            if account == "default" {
                return Err(invalid("default_account", "the default account goes with the plugin (`dragent plugins remove`)"));
            }
            if !plugin.accounts.contains(&account) {
                return Err(invalid("unknown_account", format!("{id} has no account {account}")));
            }
            apply(Vec::new(), planned.iter().map(|plan| plan.entry.clone()).collect(), Vec::new())?;
            plugin.accounts.retain(|known| known != &account);
        }
        _ => return Err(invalid("unknown_command", "usage: dragent plugins accounts add|remove ID NAME")),
    }
    record.put(plugin);
    record.save(&store())?;
    Ok(json!({
        "plugin": id,
        "account": account,
        "action": action,
        "server_entries": planned.iter().map(|plan| plan.entry.clone()).collect::<Vec<_>>(),
        "note": if action == "add" {
            "Its servers are added switched off: the person fills in this account's secrets, turns them on and approves them."
        } else {
            "Its servers' entries, tokens and approvals are gone."
        },
    }))
}

fn remove(flags: &Flags) -> Result<Value, String> {
    let id = id_argument(flags)?;
    let mut record = load()?;
    let plugin = installed(&record, &id)?.clone();
    let (_, report) = skills::unprovision(&skills_root(), &plugin.skills);
    let entries = plugin.server_entries();
    let mut unfinished = Vec::new();
    if let Err(error) = apply(Vec::new(), entries.clone(), Vec::new()) {
        unfinished.push(format!("server entries not removed ({}): {error}", entries.join(", ")));
    }
    if let Err(error) = std::fs::remove_dir_all(store().join("packages").join(&id)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            unfinished.push(format!("package folder not removed: {error}"));
        }
    }
    if unfinished.is_empty() {
        record.remove(&id);
    } else {
        let mut kept = plugin.clone();
        kept.skills = Owned::new();
        kept.enabled = false;
        kept.unfinished = unfinished.clone();
        record.put(kept);
    }
    record.save(&store())?;
    if !unfinished.is_empty() {
        return Err(Failure::new(
            Status::Partial,
            "remove_incomplete",
            format!("{id} is partly removed: {}", unfinished.join("; ")),
        )
        .next(format!("`dragent plugins remove {id}` again once that's fixed; `dragent plugins status {id}` shows what's left."))
        .encode());
    }
    Ok(json!({
        "removed": id,
        "skills": report,
        "server_entries_removed": entries,
        "note": "Files you changed in its skills were kept (skills.kept).",
    }))
}

fn sync() -> Result<Value, String> {
    let mut record = load()?;
    let existing = current_entries();
    let mut done = Vec::new();
    for mut plugin in record.plugins.clone() {
        let Ok(manifest) = Manifest::read(&plugin.path) else {
            done.push(json!({ "plugin": plugin.id, "problem": "its package is missing: install it again" }));
            continue;
        };
        let report = if plugin.enabled {
            // Asked to put back what's missing: files gone from the skills
            // folder come back (edited ones stay as they are).
            let previous = skills::forget_missing(&skills_root(), &plugin.skills);
            let (owned, report) = provision_shared_skills(
                &skills_root(),
                &plugin.path,
                &manifest,
                &previous,
                &reserved(&record, &plugin.id),
            );
            plugin.skills = owned;
            Some(report)
        } else {
            None
        };
        // Only missing entries come back (switched off); existing ones are
        // the person's to change.
        let planned: Vec<Planned> = servers::plan(&manifest, &plugin.path, &plugin.accounts)
            .into_iter()
            .filter(|plan| !existing.iter().any(|entry| entry.name == plan.entry))
            .collect();
        let (entries, _) = entries_for(&planned, &existing);
        let restored: Vec<String> = entries.iter().map(|entry| entry.name.clone()).collect();
        match apply(entries, Vec::new(), Vec::new()) {
            Ok(()) => plugin.unfinished.clear(),
            Err(error) => plugin.unfinished = vec![format!("servers not added: {error}")],
        }
        done.push(json!({ "plugin": plugin.id, "skills": report, "servers_restored": restored, "unfinished": plugin.unfinished }));
        record.put(plugin);
    }
    record.save(&store())?;
    Ok(json!(done))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// Readable text for people; agents get the JSON.
fn text(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, command: &str, enabled: bool, token: &str) -> McpServerEntry {
        McpServerEntry {
            name: name.into(),
            command: command.into(),
            transport: McpTransport::Stdio,
            args: vec!["--stdio".into()],
            env: [("ACME_TOKEN".to_string(), token.to_string())].into_iter().collect(),
            enabled,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: Vec::new(),
        }
    }

    fn planned(command: &str) -> Planned {
        Planned {
            entry: "acme-tickets-tickets".into(),
            server: "tickets".into(),
            account: "default".into(),
            transport: Transport::Stdio,
            command: command.into(),
            args: vec!["--stdio".into()],
            env: vec![("ACME_TOKEN".into(), None, true), ("ACME_SITE".into(), Some("acme.example".into()), false)],
        }
    }

    #[test]
    fn new_servers_start_off_and_a_changed_program_loses_its_approval() {
        let (entries, revoke) = entries_for(&[planned("/p/1/bin/server")], &[]);
        assert!(!entries[0].enabled, "never started without the person");
        assert!(revoke.is_empty());
        assert_eq!(entries[0].env["ACME_SITE"], "acme.example");
        assert_eq!(entries[0].env["ACME_TOKEN"], "");

        // The person filled the secret and turned it on; same program: kept.
        let existing = [entry("acme-tickets-tickets", "/p/1/bin/server", true, "s3cret")];
        let (entries, revoke) = entries_for(&[planned("/p/1/bin/server")], &existing);
        assert!(entries[0].enabled && revoke.is_empty());
        assert_eq!(entries[0].env["ACME_TOKEN"], "s3cret");

        // A new version runs something else: off, approval withdrawn,
        // credentials kept for when the person turns it back on.
        let (entries, revoke) = entries_for(&[planned("/p/2/bin/server")], &existing);
        assert!(!entries[0].enabled);
        assert_eq!(revoke, vec!["acme-tickets-tickets"]);
        assert_eq!(entries[0].env["ACME_TOKEN"], "s3cret");
    }

    #[test]
    fn a_server_says_its_next_step() {
        let state = |configured: bool, missing: &[&str], on: bool, approved: bool, connected: Value| {
            json!({"configured": configured, "missing_settings": missing, "on": on, "approved": approved, "connected": connected})
        };
        assert_eq!(next_step(&state(false, &[], false, false, Value::Null)), "not_configured");
        assert_eq!(next_step(&state(true, &["ACME_TOKEN"], false, false, Value::Null)), "needs_settings");
        assert_eq!(next_step(&state(true, &[], false, false, Value::Null)), "off");
        assert_eq!(next_step(&state(true, &[], true, false, Value::Null)), "not_approved");
        assert_eq!(next_step(&state(true, &[], true, true, json!(false))), "not_connected");
        assert_eq!(next_step(&state(true, &[], true, true, json!(true))), "ready");
        assert_eq!(next_step(&state(true, &[], true, true, Value::Null)), "unknown");
    }

    #[test]
    fn repository_addresses_are_told_from_folders() {
        assert!(is_repository("https://github.com/acme/tickets-plugin"));
        assert!(is_repository("git@github.com:acme/tickets-plugin.git"));
        assert!(!is_repository("./plugins/acme"));
        assert!(!is_repository("/Users/me/acme"));
    }

    #[test]
    fn dev_builds_keep_plugin_skills_out_of_shared_folders() {
        // Test builds are the Dev edition: provisioning records nothing and
        // writes nothing into the shared root.
        if !dragent_paths::is_dev() {
            return;
        } // only meaningful in the Dev edition
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        std::fs::create_dir_all(package.join("demo")).unwrap();
        std::fs::write(package.join("demo/SKILL.md"), "demo").unwrap();
        let manifest = Manifest {
            schema: 1,
            id: "demo".into(),
            name: "Demo".into(),
            version: "1.0.0".into(),
            description: String::new(),
            publisher: None,
            skills: vec!["demo".into()],
            servers: Default::default(),
            requires: Default::default(),
        };
        let shared = root.path().join("shared");
        let (owned, report) =
            provision_shared_skills(&shared, &package, &manifest, &Owned::new(), &|_: &str| {
                false
            });
        assert!(owned.is_empty());
        assert!(report.written.is_empty());
        assert!(!shared.exists());
    }
}
