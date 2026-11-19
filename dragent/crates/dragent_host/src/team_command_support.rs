//! Environment and validation for the headless coordination CLI.
use std::path::PathBuf;
const SWITCHES: [&str; 14] = [
    "json",
    "text",
    "quiet",
    "force",
    "no-enter",
    "local",
    "all",
    "peek",
    "keep-uncommitted",
    "approve",
    "changes",
    "qa",
    "integrated",
    "patch",
];

pub struct Flags {
    pub positional: Vec<String>,
    pub values: Vec<(String, String)>,
    pub switches: Vec<String>,
    pub json: bool,
    pub quiet: bool,
}

impl Flags {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut flags = Flags {
            positional: Vec::new(),
            values: Vec::new(),
            switches: Vec::new(),
            json: false,
            quiet: false,
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let Some(name) = arg.strip_prefix("--") else {
                flags.positional.push(arg.clone());
                continue;
            };
            if let Some((name, value)) = name.split_once('=') {
                flags.values.push((name.to_string(), value.to_string()));
            } else if SWITCHES.contains(&name) {
                flags.switches.push(name.to_string());
            } else if name == "help" {
                flags.positional.insert(0, "help".into());
            } else {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                flags.values.push((name.to_string(), value.clone()));
            }
        }
        // Asked-for output wins over the automatic JSON.
        flags.json =
            flags.on("json") || (!flags.on("text") && !flags.on("quiet") && agent_environment());
        flags.quiet = flags.on("quiet");
        Ok(flags)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn all(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }

    pub fn on(&self, name: &str) -> bool {
        self.switches.iter().any(|switch| switch == name)
    }

    pub fn subcommand(&self) -> &str {
        self.positional
            .first()
            .map(String::as_str)
            .unwrap_or("list")
    }

    /// The `index`th word after the subcommand.
    pub fn argument(&self, index: usize) -> Option<&str> {
        self.positional.get(index + 1).map(String::as_str)
    }

    /// `--prompt` or `--prompt-file`.
    pub fn prompt(&self) -> Result<String, String> {
        match (self.value("prompt"), self.value("prompt-file")) {
            (Some(_), Some(_)) => Err("use --prompt or --prompt-file, not both".into()),
            (Some(text), None) => Ok(text.to_string()),
            (None, Some(file)) => std::fs::read_to_string(absolute(file))
                .map_err(|error| format!("couldn't read {file}: {error}")),
            (None, None) => Ok(String::new()),
        }
    }
}

pub fn absolute(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    path.canonicalize().unwrap_or(path)
}

pub fn pretty(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

pub fn agent_environment() -> bool {
    let set = |name: &str| {
        std::env::var_os(name)
            .is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    };
    [
        "CI",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "GEMINI_CLI",
        "QWEN_CODE",
        "DRAGENT_JSON",
    ]
    .iter()
    .any(|name| set(name))
}

/// The pane of the nearest ancestor that is Dragent's session helper.
#[cfg(unix)]
pub fn helper_pane() -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-ww", "-o", "pid=,ppid=,args="])
        .output()
        .ok()?;
    let table = String::from_utf8_lossy(&output.stdout);
    pane_of_ancestor(&table, std::process::id())
}

#[cfg(not(unix))]
pub fn helper_pane() -> Option<String> {
    None
}

/// Walk up from `pid` through `ps` rows (`pid ppid args…`) to a process
/// started as `… --dragent-agent-run <pane> …`.
fn pane_of_ancestor(table: &str, pid: u32) -> Option<String> {
    let rows: Vec<(u32, u32, &str)> = table
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (ppid, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            Some((pid.parse().ok()?, ppid.parse().ok()?, args))
        })
        .collect();
    let parent_of = |pid: u32| rows.iter().find(|row| row.0 == pid).map(|row| row.1);
    let mut current = parent_of(pid)?;
    // Bounded: a malformed table must not loop.
    for _ in 0..64 {
        if current <= 1 {
            return None;
        }
        let (_, ppid, args) = rows.iter().find(|row| row.0 == current)?;
        let mut words = args.split_whitespace();
        if words.any(|word| word == crate::agent_sessions::RUN_ARG) {
            if let Some(pane) = words
                .next()
                .filter(|pane| uuid::Uuid::parse_str(pane).is_ok())
            {
                return Some(pane.to_string());
            }
        }
        current = *ppid;
    }
    None
}

pub fn agent_key(key: &str) -> Result<String, String> {
    const AGENTS: &[&str] = &[
        "dragent",
        "claude",
        "codex",
        "gemini",
        "opencode",
        "amp",
        "copilot",
        "agy",
        "auggie",
        "autohand",
        "crush",
        "cline",
        "codebuddy",
        "codebuff",
        "command-code",
        "cn",
        "cursor-agent",
        "devin",
        "droid",
        "freebuff",
        "goose",
        "grok",
        "hermes",
        "jules",
        "junie",
        "kilo",
        "kimi",
        "kiro-cli",
        "letta",
        "mimo",
        "vibe",
        "muse",
        "omp",
        "pi",
        "prime-agent",
        "qodercli",
        "qwen",
        "rovodev",
        "zero",
    ];
    if AGENTS.contains(&key) {
        Ok(key.into())
    } else {
        Err(format!("unknown agent “{key}”"))
    }
}

pub fn check_model(key: &str, model: &str) -> Result<(), String> {
    let key = agent_key(key)?;
    if ![
        "claude",
        "codex",
        "gemini",
        "opencode",
        "copilot",
        "cursor-agent",
        "qwen",
    ]
    .contains(&key.as_str())
    {
        return Err(format!("{key} doesn't take a model from the command line"));
    }
    if model.is_empty()
        || !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:/@[]".contains(c))
    {
        return Err(format!("“{model}” isn't a model name"));
    }
    Ok(())
}
pub fn chosen_project_at(project: Option<&str>) -> Result<PathBuf, String> {
    let path = project
        .map(absolute)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    if path.is_dir() {
        Ok(path)
    } else {
        Err(format!("{} isn't a folder", path.display()))
    }
}
