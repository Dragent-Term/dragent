//! `.dragent/workspace.json` (version 1) plus the user-local override
//! `.dragent/workspace.local.json`, merged key by key over the shared file.
//!
//! The schema is deliberately small: steps (`run` or `shell`), per-step
//! `cwd`/`platforms`/`timeout_secs`/`env`, the `init`/`launch`/`cleanup`
//! sections, `env_required` (names only) and `copy` (explicit files only).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Config file inside the repository, relative to its root.
pub const CONFIG_REL: &str = ".dragent/workspace.json";
/// User-local override, same schema, merged over the shared file.
pub const LOCAL_REL: &str = ".dragent/workspace.local.json";
/// The only version this build reads.
pub const VERSION: u32 = 1;
/// When a step omits `timeout_secs`.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// When a ready check omits `timeout_secs`.
pub const DEFAULT_READY_TIMEOUT_SECS: u64 = 30;
/// How much process output travels with a failure.
pub const OUTPUT_TAIL_CHARS: usize = 4000;

/// One validation problem, with the JSON path of the offending value
/// (e.g. `init[0].run`, `launch.dev.steps[1].shell`, `copy[0].to`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub path: String,
    pub message: String,
}

impl ConfigError {
    fn at(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ConfigError {}

/// An executable step or an explicit shell string.
///
/// `{"run": ["npm", "ci"]}` runs the executable directly (no shell).
/// `{"shell": "npm ci && npm run build"}` runs through `sh -c` on
/// macOS/Linux and PowerShell (`-NoProfile -Command`) on Windows.
/// Exactly one of the two must be present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Repository-relative directory the step runs in. Must stay inside
    /// the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `macos`, `linux`, `windows`. Omitted means all platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Literal non-secret values. Secrets stay out of the file: list
    /// their names in `env_required` and keep values in the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
}

/// What a launch profile waits for after starting its steps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyCheck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// A named launch profile: steps whose last process keeps running,
/// with an optional readiness probe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchProfile {
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<ReadyCheck>,
}

/// One explicit file to bring from the project's main checkout into a
/// new worktree, or to create from a committed example when missing.
///
/// `{"from": ".env", "to": ".env"}` copies an ignored local file;
/// `{"from_example": ".env.example", "to": ".env"}` creates it from the
/// example. `if_missing` defaults to true and never overwrites: an
/// existing destination is always preserved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_example: Option<String>,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_missing: Option<bool>,
}

/// The whole repository configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub version: u32,
    #[serde(default)]
    pub init: Vec<Step>,
    #[serde(default)]
    pub launch: BTreeMap<String, LaunchProfile>,
    #[serde(default)]
    pub cleanup: Vec<Step>,
    #[serde(default)]
    pub env_required: Vec<String>,
    #[serde(default)]
    pub copy: Vec<CopyEntry>,
}

impl WorkspaceConfig {
    /// Validate field values, naming each problem's JSON path.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != VERSION {
            return Err(ConfigError::at(
                "version",
                format!("unsupported version {}, expected {}", self.version, VERSION),
            ));
        }
        for (index, step) in self.init.iter().enumerate() {
            step.validate(&format!("init[{index}]"))?;
        }
        let mut profiles: Vec<&String> = self.launch.keys().collect();
        profiles.sort();
        for name in profiles {
            let profile = &self.launch[name];
            validate_profile_name(name)?;
            if profile.steps.is_empty() {
                return Err(ConfigError::at(
                    format!("launch.{name}.steps"),
                    "a launch profile needs at least one step",
                ));
            }
            for (index, step) in profile.steps.iter().enumerate() {
                step.validate(&format!("launch.{name}.steps[{index}]"))?;
            }
            if let Some(ready) = &profile.ready {
                ready.validate(&format!("launch.{name}.ready"))?;
            }
        }
        for (index, step) in self.cleanup.iter().enumerate() {
            step.validate(&format!("cleanup[{index}]"))?;
        }
        for (index, name) in self.env_required.iter().enumerate() {
            if name.trim().is_empty() || name.contains('=') || name.contains(char::is_whitespace) {
                return Err(ConfigError::at(
                    format!("env_required[{index}]"),
                    "must be a variable NAME, e.g. DATABASE_URL (never a value)",
                ));
            }
        }
        for (index, entry) in self.copy.iter().enumerate() {
            entry.validate(&format!("copy[{index}]"))?;
        }
        Ok(())
    }

    /// Steps of `init` that run on this platform.
    pub fn init_plan(&self) -> Vec<(usize, &Step)> {
        self.init
            .iter()
            .enumerate()
            .filter(|(_, step)| step.applies_to_current())
            .collect()
    }
}

fn validate_profile_name(name: &str) -> Result<(), ConfigError> {
    if name.trim().is_empty() {
        return Err(ConfigError::at("launch", "profile names must not be empty"));
    }
    if name.contains('/') || name.contains('\\') || name.contains('.') {
        return Err(ConfigError::at(
            format!("launch.{name}"),
            "profile names must not contain /, \\ or .",
        ));
    }
    Ok(())
}

impl Step {
    pub fn validate(&self, path: &str) -> Result<(), ConfigError> {
        match (&self.run, &self.shell) {
            (Some(run), None) => {
                if run.is_empty() || run.iter().any(|arg| arg.is_empty()) {
                    return Err(ConfigError::at(
                        format!("{path}.run"),
                        "run needs a program and its arguments, e.g. [\"npm\", \"ci\"]",
                    ));
                }
            }
            (None, Some(shell)) => {
                if shell.trim().is_empty() {
                    return Err(ConfigError::at(
                        format!("{path}.shell"),
                        "shell must not be empty",
                    ));
                }
            }
            (Some(_), Some(_)) => {
                return Err(ConfigError::at(path, "use run or shell, not both"));
            }
            (None, None) => {
                return Err(ConfigError::at(path, "a step needs run or shell"));
            }
        }
        if let Some(cwd) = &self.cwd {
            crate::paths::check_relative_inside(cwd)
                .map_err(|message| ConfigError::at(format!("{path}.cwd"), message))?;
        }
        if let Some(platforms) = &self.platforms {
            if platforms.is_empty() {
                return Err(ConfigError::at(
                    format!("{path}.platforms"),
                    "platforms must not be empty (omit it for all platforms)",
                ));
            }
            for platform in platforms {
                if !matches!(platform.as_str(), "macos" | "linux" | "windows") {
                    return Err(ConfigError::at(
                        format!("{path}.platforms"),
                        format!("unknown platform \"{platform}\" (macos, linux, windows)"),
                    ));
                }
            }
        }
        if let Some(timeout) = self.timeout_secs {
            if timeout == 0 || timeout > 86_400 {
                return Err(ConfigError::at(
                    format!("{path}.timeout_secs"),
                    "timeout_secs must be 1 to 86400",
                ));
            }
        }
        if let Some(env) = &self.env {
            for key in env.keys() {
                if key.trim().is_empty() || key.contains('=') || key.contains(char::is_whitespace) {
                    return Err(ConfigError::at(
                        format!("{path}.env"),
                        format!("bad variable name \"{key}\""),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Whether this step runs on the current platform.
    pub fn applies_to_current(&self) -> bool {
        match &self.platforms {
            None => true,
            Some(platforms) => platforms.iter().any(|p| p == crate::platform::current()),
        }
    }

    pub fn timeout_secs_or_default(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }

    /// Human-readable summary for plans and records.
    pub fn describe(&self) -> String {
        if let Some(run) = &self.run {
            return shell_join(run);
        }
        self.shell.clone().unwrap_or_default()
    }
}

impl ReadyCheck {
    fn validate(&self, path: &str) -> Result<(), ConfigError> {
        match (&self.port, &self.url) {
            (Some(_), None) | (None, Some(_)) => {}
            (Some(_), Some(_)) => {
                return Err(ConfigError::at(path, "use port or url, not both"));
            }
            (None, None) => {
                return Err(ConfigError::at(path, "ready needs port or url"));
            }
        }
        if let Some(port) = self.port {
            if port == 0 {
                return Err(ConfigError::at(
                    format!("{path}.port"),
                    "port must be 1 to 65535",
                ));
            }
        }
        if let Some(url) = &self.url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(ConfigError::at(
                    format!("{path}.url"),
                    "url must start with http:// or https://",
                ));
            }
        }
        if let Some(timeout) = self.timeout_secs {
            if timeout == 0 || timeout > 3600 {
                return Err(ConfigError::at(
                    format!("{path}.timeout_secs"),
                    "timeout_secs must be 1 to 3600",
                ));
            }
        }
        Ok(())
    }

    pub fn timeout_secs_or_default(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_READY_TIMEOUT_SECS)
    }
}

impl CopyEntry {
    fn validate(&self, path: &str) -> Result<(), ConfigError> {
        match (&self.from, &self.from_example) {
            (Some(_), None) | (None, Some(_)) => {}
            (Some(_), Some(_)) => {
                return Err(ConfigError::at(path, "use from or from_example, not both"));
            }
            (None, None) => {
                return Err(ConfigError::at(
                    path,
                    "copy needs from (an ignored file) or from_example (a committed example)",
                ));
            }
        }
        if let Some(from) = &self.from {
            crate::paths::check_relative_inside(from)
                .map_err(|message| ConfigError::at(format!("{path}.from"), message))?;
        }
        if let Some(example) = &self.from_example {
            crate::paths::check_relative_inside(example)
                .map_err(|message| ConfigError::at(format!("{path}.from_example"), message))?;
        }
        crate::paths::check_relative_inside(&self.to)
            .map_err(|message| ConfigError::at(format!("{path}.to"), message))?;
        Ok(())
    }

    pub fn if_missing_or_default(&self) -> bool {
        self.if_missing.unwrap_or(true)
    }
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains(' ') || arg.contains('"') || arg.contains('\'') {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse and validate one config file's bytes.
pub fn parse_bytes(bytes: &[u8]) -> Result<WorkspaceConfig, ConfigError> {
    let config: WorkspaceConfig = serde_json::from_slice(bytes)
        .map_err(|error| ConfigError::at("(root)", format!("invalid JSON: {error}")))?;
    config.validate()?;
    Ok(config)
}

/// Read the shared config plus the local override (when present) and merge
/// them key by key: scalar sections and maps replace per key, step lists
/// replace wholesale when the override sets them.
pub fn load_merged(worktree: &Path) -> Result<Option<WorkspaceConfig>, ConfigError> {
    let shared_path = worktree.join(CONFIG_REL);
    let shared_bytes = match std::fs::read(&shared_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ConfigError::at(
                CONFIG_REL,
                format!("could not read: {error}"),
            ))
        }
    };
    let local_path = worktree.join(LOCAL_REL);
    let local_bytes = match std::fs::read(&local_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ConfigError::at(
                LOCAL_REL,
                format!("could not read: {error}"),
            ))
        }
    };
    match (shared_bytes, local_bytes) {
        (None, None) => Ok(None),
        (shared, local) => {
            let mut merged =
                shared
                    .as_deref()
                    .map(parse_bytes)
                    .transpose()?
                    .unwrap_or(WorkspaceConfig {
                        version: VERSION,
                        init: Vec::new(),
                        launch: BTreeMap::new(),
                        cleanup: Vec::new(),
                        env_required: Vec::new(),
                        copy: Vec::new(),
                    });
            if let Some(local) = local.as_deref() {
                let overlay: WorkspaceConfig = parse_bytes(local).map_err(|error| ConfigError {
                    path: format!("{LOCAL_REL}:{}", error.path),
                    message: error.message,
                })?;
                merged.merge_overlay(overlay);
                merged.validate()?;
            }
            Ok(Some(merged))
        }
    }
}

impl WorkspaceConfig {
    /// Merge the local override over the shared config, key by key.
    pub fn merge_overlay(&mut self, overlay: WorkspaceConfig) {
        // `version` always matches after validation; keep the shared one.
        if !overlay.init.is_empty() {
            self.init = overlay.init;
        }
        for (name, profile) in overlay.launch {
            self.launch.insert(name, profile);
        }
        if !overlay.cleanup.is_empty() {
            self.cleanup = overlay.cleanup;
        }
        if !overlay.env_required.is_empty() {
            self.env_required = overlay.env_required;
        }
        if !overlay.copy.is_empty() {
            self.copy = overlay.copy;
        }
    }
}

/// Names of required variables that are missing or empty.
pub fn missing_env(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| std::env::var_os(name.as_str()).is_none_or(|value| value.is_empty()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_run(program: &str) -> Step {
        Step {
            run: Some(vec![program.to_string()]),
            shell: None,
            cwd: None,
            platforms: None,
            timeout_secs: None,
            env: None,
        }
    }

    #[test]
    fn schema_errors_name_the_field() {
        let bad = serde_json::json!({
            "version": 1,
            "init": [{"run": ["echo"], "cwd": "../escape"}],
        });
        let error = parse_bytes(serde_json::to_vec(&bad).unwrap().as_slice()).unwrap_err();
        assert_eq!(error.path, "init[0].cwd");

        let both = serde_json::json!({
            "version": 1,
            "init": [{"run": ["echo"], "shell": "echo"}],
        });
        let error = parse_bytes(serde_json::to_vec(&both).unwrap().as_slice()).unwrap_err();
        assert_eq!(error.path, "init[0]");

        let ready = serde_json::json!({
            "version": 1,
            "launch": {"dev": {"steps": [{"run": ["npm", "start"]}], "ready": {}}},
        });
        let error = parse_bytes(serde_json::to_vec(&ready).unwrap().as_slice()).unwrap_err();
        assert_eq!(error.path, "launch.dev.ready");

        let copy = serde_json::json!({
            "version": 1,
            "copy": [{"from": "/abs", "to": ".env"}],
        });
        let error = parse_bytes(serde_json::to_vec(&copy).unwrap().as_slice()).unwrap_err();
        assert_eq!(error.path, "copy[0].from");

        let version = serde_json::json!({"version": 2});
        let error = parse_bytes(serde_json::to_vec(&version).unwrap().as_slice()).unwrap_err();
        assert_eq!(error.path, "version");
    }

    #[test]
    fn platforms_reject_unknown_values() {
        let mut step = step_run("echo");
        step.platforms = Some(vec!["plan9".into()]);
        assert_eq!(
            step.validate("init[0]").unwrap_err().path,
            "init[0].platforms"
        );
    }

    #[test]
    fn overlay_replaces_lists_and_merges_profiles() {
        let mut base = WorkspaceConfig {
            version: VERSION,
            init: vec![step_run("a")],
            launch: BTreeMap::from([(
                "dev".to_string(),
                LaunchProfile {
                    steps: vec![step_run("old")],
                    ready: None,
                },
            )]),
            cleanup: Vec::new(),
            env_required: vec!["A".into()],
            copy: Vec::new(),
        };
        let overlay = WorkspaceConfig {
            version: VERSION,
            init: vec![step_run("b")],
            launch: BTreeMap::from([(
                "web".to_string(),
                LaunchProfile {
                    steps: vec![step_run("new")],
                    ready: None,
                },
            )]),
            cleanup: Vec::new(),
            env_required: Vec::new(),
            copy: Vec::new(),
        };
        base.merge_overlay(overlay);
        assert_eq!(base.init.len(), 1);
        assert_eq!(base.init[0].describe(), "b");
        assert!(base.launch.contains_key("dev"));
        assert!(base.launch.contains_key("web"));
        // Empty overlay lists leave the base alone.
        assert_eq!(base.env_required, vec!["A".to_string()]);
    }

    #[test]
    fn missing_env_lists_names_only() {
        std::env::remove_var("DRAGENT_WS_TEST_MISSING_XYZ");
        let missing = missing_env(&["DRAGENT_WS_TEST_MISSING_XYZ".into(), "PATH".into()]);
        assert_eq!(missing, vec!["DRAGENT_WS_TEST_MISSING_XYZ".to_string()]);
    }
}
