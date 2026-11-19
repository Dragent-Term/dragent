//! `dragent-plugin.json`: what a plugin is and what it brings.
//!
//! ```json
//! {
//!   "schema": 1,
//!   "id": "acme-tickets",
//!   "name": "Acme Tickets",
//!   "version": "1.2.0",
//!   "description": "Search and update Acme tickets.",
//!   "publisher": "Acme",
//!   "skills": ["skills/acme-tickets"],
//!   "servers": {
//!     "tickets": {
//!       "transport": "stdio",
//!       "command": "node",
//!       "args": ["${PLUGIN_DIR}/server/index.js"],
//!       "env": { "ACME_TOKEN": { "secret": true, "description": "An Acme API token" } }
//!     }
//!   },
//!   "requires": { "commands": ["node"] }
//! }
//! ```
//!
//! Every path is relative to the package and stays inside it. A stdio
//! server runs a program on PATH or one in the package (`./bin/server`);
//! `${PLUGIN_DIR}` in its arguments is the installed package's folder. A
//! package that ships one build per system names each in `platforms`
//! (`macos`, `windows`, `linux`); a server with no program for this system
//! isn't added here. An
//! http server is an `https://` address (or `http://localhost`), with no
//! `env`: it signs in, or takes a token, in Settings → MCP.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE: &str = "dragent-plugin.json";
/// The largest manifest read.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub publisher: Option<String>,
    /// Package folders holding a `SKILL.md` each.
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub servers: BTreeMap<String, ServerSpec>,
    #[serde(default)]
    pub requires: Requires,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    /// Programs that must be on PATH for the servers to start.
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Http,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    pub transport: Transport,
    /// stdio: a program on PATH, or `./path` inside the package.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// stdio: the program (and arguments) on one system, in place of
    /// `command`, for a package that ships a build per system.
    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformCommand>,
    /// http: the server's address.
    #[serde(default)]
    pub url: Option<String>,
    /// Settings the server reads from its environment; secrets are filled
    /// in by the person, never shipped in the package.
    #[serde(default)]
    pub env: BTreeMap<String, EnvSpec>,
}

/// The systems a server's program can be named for.
pub const SYSTEMS: [&str; 3] = ["macos", "windows", "linux"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformCommand {
    pub command: String,
    /// Its own arguments; the server's `args` when absent.
    #[serde(default)]
    pub args: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSpec {
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub description: String,
    /// A non-secret default value.
    #[serde(default)]
    pub default: Option<String>,
}

fn word(text: &str, max: usize) -> bool {
    let bytes = text.as_bytes();
    !text.is_empty()
        && text.len() <= max
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && !text.ends_with('-')
}

/// A path inside the package: relative, no `..`, no drive or root.
pub fn inside(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

impl Manifest {
    /// Read and check the manifest of the package at `dir`.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join(MANIFEST_FILE);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| format!("{} has no {MANIFEST_FILE}: it isn't a Dragent plugin.", dir.display()))?;
        if !meta.is_file() {
            return Err(format!("{MANIFEST_FILE} must be a plain file."));
        }
        if meta.len() > MAX_MANIFEST_BYTES {
            return Err(format!("{MANIFEST_FILE} is larger than 64 KB."));
        }
        let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|error| format!("{MANIFEST_FILE} isn't valid: {error}"))?;
        manifest.check()?;
        Ok(manifest)
    }

    /// Everything that must hold before anything is installed.
    pub fn check(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("schema {} isn't one this Dragent reads (it reads 1).", self.schema));
        }
        if !word(&self.id, 40) {
            return Err(format!(
                "id “{}” must be 1–40 lowercase letters, digits and hyphens, starting with a letter.",
                self.id
            ));
        }
        if self.id == "dragent" || self.id.starts_with("dragent-") {
            return Err("ids starting with “dragent” are Dragent's own.".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 80 {
            return Err("name must be 1–80 characters.".into());
        }
        let version_ok = !self.version.is_empty()
            && self.version.len() <= 40
            && self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
        if !version_ok {
            return Err(format!("version “{}” must be like 1.2.0.", self.version));
        }
        if self.skills.is_empty() && self.servers.is_empty() {
            return Err("a plugin brings at least one skill or server.".into());
        }
        for skill in &self.skills {
            if !inside(skill) {
                return Err(format!("skill path “{skill}” must stay inside the package."));
            }
            let name = Path::new(skill)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !word(name, 64) {
                return Err(format!(
                    "skill folder “{name}” must be lowercase letters, digits and hyphens (it becomes the skill's name)."
                ));
            }
        }
        for name in &self.requires.commands {
            if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains(' ') {
                return Err(format!("required command “{name}” must be a program name, like node."));
            }
        }
        for (name, server) in &self.servers {
            if !word(name, 30) {
                return Err(format!("server name “{name}” must be lowercase letters, digits and hyphens."));
            }
            server.check(name)?;
        }
        Ok(())
    }
}

impl ServerSpec {
    fn check(&self, name: &str) -> Result<(), String> {
        match self.transport {
            Transport::Stdio => {
                if self.command.is_none() && self.platforms.is_empty() {
                    return Err(format!("server “{name}” needs a command."));
                }
                if self.url.is_some() {
                    return Err(format!("server “{name}” is stdio: it takes a command, not a url."));
                }
                for system in self.platforms.keys() {
                    if !SYSTEMS.contains(&system.as_str()) {
                        return Err(format!(
                            "server “{name}” names a program for “{system}”: platforms are macos, windows and linux."
                        ));
                    }
                }
                for command in self.programs() {
                    if command.trim().is_empty() {
                        return Err(format!("server “{name}” needs a command."));
                    }
                    if let Some(local) = command.strip_prefix("./") {
                        if !inside(local) {
                            return Err(format!("server “{name}”'s command must stay inside the package."));
                        }
                    } else if command.contains('/') || command.contains('\\') || command.contains(' ') {
                        return Err(format!(
                            "server “{name}”'s command is a program name on PATH or ./path in the package, with arguments in args."
                        ));
                    }
                }
            }
            Transport::Http => {
                let url = self
                    .url
                    .as_deref()
                    .ok_or_else(|| format!("server “{name}” needs a url."))?;
                // The host itself, not a name that starts the same way
                // (`localhost.example.com`).
                let local = ["http://localhost", "http://127.0.0.1"].iter().any(|prefix| {
                    url.strip_prefix(prefix)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
                });
                if !(url.starts_with("https://") || local) {
                    return Err(format!("server “{name}”'s url must be https:// (or http://localhost)."));
                }
                if self.command.is_some() || !self.args.is_empty() || !self.platforms.is_empty() {
                    return Err(format!("server “{name}” is http: it takes a url, not a command."));
                }
                if !self.env.is_empty() {
                    return Err(format!(
                        "server “{name}” is http: it has no env; it signs in, or takes a token, in Settings → MCP."
                    ));
                }
            }
        }
        for (key, spec) in &self.env {
            let ok = !key.is_empty()
                && key.as_bytes()[0].is_ascii_uppercase()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
            if !ok {
                return Err(format!("setting “{key}” must be an UPPER_CASE name."));
            }
            if spec.secret && spec.default.is_some() {
                return Err(format!("secret “{key}” can't ship a value: the person fills it in."));
            }
        }
        Ok(())
    }

    /// Every program it names: its `command` and each system's.
    pub fn programs(&self) -> Vec<&str> {
        self.command
            .as_deref()
            .into_iter()
            .chain(self.platforms.values().map(|platform| platform.command.as_str()))
            .collect()
    }

    /// The stdio program and arguments on `system` (as
    /// `std::env::consts::OS` names it); `None` when it names none there.
    pub fn program_on(&self, system: &str) -> Option<(&str, &[String])> {
        match self.platforms.get(system) {
            Some(platform) => Some((
                platform.command.as_str(),
                platform.args.as_deref().unwrap_or(&self.args),
            )),
            None => self.command.as_deref().map(|command| (command, self.args.as_slice())),
        }
    }

    /// The systems it runs on.
    pub fn systems(&self) -> Vec<&'static str> {
        SYSTEMS
            .into_iter()
            .filter(|system| self.transport == Transport::Http || self.program_on(system).is_some())
            .collect()
    }

    /// The command line as it will run on `system`, for the person to read.
    pub fn display(&self, plugin_dir: &Path, system: &str) -> String {
        match self.transport {
            Transport::Http => self.url.clone().unwrap_or_default(),
            Transport::Stdio => match self.program_on(system) {
                Some((command, args)) => {
                    let mut parts = vec![resolve_command(command, plugin_dir)];
                    parts.extend(args.iter().map(|arg| substitute(arg, plugin_dir)));
                    parts.join(" ")
                }
                None => format!("(nothing on {system})"),
            },
        }
    }
}

/// `./bin/server` becomes a path in the installed package; a program name
/// stays as it is (found on PATH when the server starts).
pub fn resolve_command(command: &str, plugin_dir: &Path) -> String {
    match command.strip_prefix("./") {
        Some(local) => plugin_dir.join(local).display().to_string(),
        None => command.to_string(),
    }
}

/// `${PLUGIN_DIR}` in an argument is the installed package's folder.
pub fn substitute(arg: &str, plugin_dir: &Path) -> String {
    arg.replace("${PLUGIN_DIR}", &plugin_dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(value: serde_json::Value) -> Result<Manifest, String> {
        let manifest: Manifest = serde_json::from_value(value).map_err(|e| e.to_string())?;
        manifest.check().map(|()| manifest)
    }

    fn base() -> serde_json::Value {
        json!({
            "schema": 1, "id": "acme-tickets", "name": "Acme Tickets", "version": "1.2.0",
            "skills": ["skills/acme-tickets"],
            "servers": {"tickets": {"transport": "stdio", "command": "node", "args": ["${PLUGIN_DIR}/server.js"],
                "env": {"ACME_TOKEN": {"secret": true}}}}
        })
    }

    #[test]
    fn a_good_manifest_passes_and_shows_what_will_run() {
        let manifest = manifest(base()).unwrap();
        assert_eq!(
            manifest.servers["tickets"].display(Path::new("/p/acme"), "macos"),
            "node /p/acme/server.js"
        );
        assert_eq!(resolve_command("./bin/run", Path::new("/p/acme")), "/p/acme/bin/run");
    }

    #[test]
    fn manifests_that_reach_outside_or_ship_secrets_are_refused() {
        let cases: Vec<(&str, serde_json::Value)> = vec![
            ("/id", json!("Acme")),
            ("/id", json!("dragent-x")),
            ("/schema", json!(2)),
            ("/version", json!("1.0 beta")),
            ("/skills", json!(["../outside"])),
            ("/skills", json!(["/etc/skills"])),
            ("/skills", json!(["skills/Bad_Name"])),
            ("/servers/tickets/command", json!("/usr/bin/node")),
            ("/servers/tickets/command", json!("./../escape")),
            ("/servers/tickets/command", json!("node server.js")),
            ("/servers/tickets/env", json!({"ACME_TOKEN": {"secret": true, "default": "abc"}})),
            ("/servers/tickets/env", json!({"lower": {}})),
        ];
        for (pointer, value) in cases {
            let mut bad = base();
            *bad.pointer_mut(pointer).unwrap() = value.clone();
            assert!(manifest(bad).is_err(), "{pointer} = {value}");
        }
        let mut http = base();
        http["servers"] = json!({"api": {"transport": "http", "url": "http://example.com/mcp"}});
        assert!(manifest(http.clone()).is_err(), "plain http to another host");
        http["servers"]["api"]["url"] = json!("http://localhost.example.com/mcp");
        assert!(manifest(http.clone()).is_err(), "a host that only starts like localhost");
        http["servers"]["api"]["url"] = json!("http://localhost:8080/mcp");
        assert!(manifest(http.clone()).is_ok());
        http["servers"]["api"]["env"] = json!({"API_TOKEN": {"secret": true}});
        assert!(manifest(http.clone()).is_err(), "an http server's env would go nowhere");
        http["servers"]["api"].as_object_mut().unwrap().remove("env");
        http["servers"]["api"]["url"] = json!("https://example.com/mcp");
        assert!(manifest(http).is_ok());
        // A build per system: only the three systems, each inside the
        // package or on PATH like any command.
        let mut per_system = base();
        per_system["servers"]["tickets"]["platforms"] =
            json!({"windows": {"command": "./bin/server.exe"}, "linux": {"command": "server", "args": ["--serve"]}});
        assert!(manifest(per_system.clone()).is_ok());
        for platforms in [
            json!({"freebsd": {"command": "server"}}),
            json!({"windows": {"command": "./../server.exe"}}),
            json!({"windows": {"command": "C:\\tools\\server.exe"}}),
            json!({"windows": {"program": "server"}}),
        ] {
            let mut bad = base();
            bad["servers"]["tickets"]["platforms"] = platforms.clone();
            assert!(manifest(bad).is_err(), "{platforms}");
        }
        let mut http = base();
        http["servers"] = json!({"api": {"transport": "http", "url": "https://example.com/mcp", "platforms": {"linux": {"command": "x"}}}});
        assert!(manifest(http).is_err(), "an http server runs nothing");
        let mut unknown = base();
        unknown["run_on_install"] = json!("curl … | sh");
        assert!(manifest(unknown).is_err(), "unknown fields are refused, not ignored");
    }
}
