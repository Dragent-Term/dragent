use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::theme::{Theme, DEFAULT_THEME_ID};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AgentCommandConfig {
    pub command: String,
    pub prompt_command: String,
    #[serde(default)]
    pub prompt_suffix: String,
    #[serde(default)]
    pub prompt_stdin: bool,
}

impl Default for AgentCommandConfig {
    /// Empty commands never validate: an unset custom launch stays
    /// inert instead of launching an empty shell command.
    fn default() -> Self {
        Self {
            command: String::new(),
            prompt_command: String::new(),
            prompt_suffix: String::new(),
            prompt_stdin: false,
        }
    }
}

impl AgentCommandConfig {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        super::agent_command::validate(&self.command, &self.prompt_command, &self.prompt_suffix)
    }

    /// The shell line that starts this agent on `prompt`. A shell on this
    /// machine (`local`) reads a long or multi-line prompt from a file, so
    /// its terminal echoes a short command rather than the whole prompt.
    pub(crate) fn build_prompt_command(
        &self,
        prompt: &str,
        shell: &str,
        local: bool,
    ) -> Result<String, &'static str> {
        use super::agent_command::PromptSource;
        self.validate()?;
        let staged = (local && (prompt.contains(['\n', '\r']) || prompt.chars().count() > 160))
            .then(|| super::agent_command::stage_prompt(prompt))
            .and_then(|staged| {
                staged
                    .map_err(|error| log::warn!("[agent-launch] prompt file: {error}"))
                    .ok()
            });
        super::agent_command::build_prompt_command(
            &self.command,
            &self.prompt_command,
            &self.prompt_suffix,
            self.prompt_stdin,
            match &staged {
                Some(path) => PromptSource::File(path),
                None => PromptSource::Inline(prompt),
            },
            shell,
            &uuid::Uuid::new_v4().simple().to_string(),
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UiSettings {
    #[serde(default)]
    pub diff_comment_terminal: Option<String>,
    #[serde(default)]
    pub diff_comment_agent: Option<String>,
    #[serde(default)]
    pub diff_comment_new_tab: bool,
    /// Where a diff comment starts a new agent session in this workspace:
    /// "split" or "tab". Falls back to `diff_comment_new_tab` when unset.
    #[serde(default)]
    pub diff_comment_placement: Option<String>,
    pub onboarding_completed: bool,
    #[serde(default)]
    pub github_cli_setup_completed: bool,
    #[serde(default)]
    pub gitlab_cli_setup_completed: bool,
    pub theme_id: String,
    /// Follow the OS light/dark mode (Warp "Sync with OS" pattern).
    #[serde(default)]
    pub sync_os: bool,
    /// Theme applied when the OS is in light mode while syncing.
    #[serde(default = "default_sync_light_id")]
    pub sync_light_id: String,
    /// Theme applied when the OS is in dark mode while syncing.
    #[serde(default = "default_sync_dark_id")]
    pub sync_dark_id: String,
    /// User-defined launch commands keyed by PromptAgent::command_key().
    #[serde(default)]
    pub agent_commands: HashMap<String, String>,
    #[serde(default)]
    pub agent_command_configs: HashMap<String, AgentCommandConfig>,
    /// User-provided agents. They resolve through the same agent registry
    /// as built-ins (rail, tabs, installer, launcher, input editor).
    #[serde(default)]
    pub custom_agents: Vec<crate::workspace::CustomAgentDefinition>,
}

fn default_sync_light_id() -> String {
    "gruvbox_light".to_string()
}

fn default_sync_dark_id() -> String {
    DEFAULT_THEME_ID.to_string()
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            diff_comment_terminal: None,
            diff_comment_agent: None,
            diff_comment_new_tab: false,
            diff_comment_placement: None,
            onboarding_completed: false,
            github_cli_setup_completed: false,
            gitlab_cli_setup_completed: false,
            theme_id: DEFAULT_THEME_ID.to_string(),
            sync_os: false,
            sync_light_id: default_sync_light_id(),
            sync_dark_id: default_sync_dark_id(),
            agent_commands: HashMap::new(),
            agent_command_configs: HashMap::new(),
            custom_agents: Vec::new(),
        }
    }
}

impl UiSettings {
    pub fn theme(&self) -> Theme {
        Theme::from_id(&self.theme_id)
    }

    /// Theme id dictated by the OS mode when syncing, `None` when sync is off.
    pub fn os_matched_id(&self, os_dark: bool) -> Option<String> {
        if !self.sync_os {
            return None;
        }
        Some(if os_dark {
            self.sync_dark_id.clone()
        } else {
            self.sync_light_id.clone()
        })
    }

    pub fn load() -> Self {
        let mut settings: UiSettings = settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if force_show_onboarding() {
            settings.onboarding_completed = false;
        }

        settings
    }

    pub fn save(&self) {
        let _ = self.save_checked();
    }

    pub fn save_checked(&self) -> std::io::Result<()> {
        let path = settings_path()
            .ok_or_else(|| std::io::Error::other("Settings directory is unavailable"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

fn settings_path() -> Option<PathBuf> {
    Some(dragent_paths::home().join("ui_settings.json"))
}

/// Set `DRAGENT_SHOW_ONBOARDING=1` to show the wizard even when onboarding was completed.
fn force_show_onboarding() -> bool {
    std::env::var_os("DRAGENT_SHOW_ONBOARDING").is_some_and(|v| {
        v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
    })
}

/// Deletes persisted UI settings so the next launch shows onboarding again.
pub fn reset_for_onboarding() -> bool {
    if let Some(path) = settings_path() {
        if path.exists() {
            return std::fs::remove_file(path).is_ok();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_onboarding_not_completed() {
        let s = UiSettings::default();
        assert!(!s.onboarding_completed);
        assert_eq!(s.theme_id, DEFAULT_THEME_ID);
    }

    #[test]
    fn round_trip_serialization() {
        let s = UiSettings {
            onboarding_completed: true,
            github_cli_setup_completed: true,
            theme_id: "light".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: UiSettings = serde_json::from_str(&json).unwrap();
        assert!(back.onboarding_completed);
        assert!(back.github_cli_setup_completed);
        assert_eq!(back.theme_id, "light");
    }

    #[test]
    fn saved_agent_fields_drive_prompt_launches_after_reload() {
        let config = AgentCommandConfig {
            command: "capture --plain".into(),
            prompt_command: "capture --prompt".into(),
            prompt_suffix: "--suffix".into(),
            prompt_stdin: true,
        };
        let mut settings = UiSettings::default();
        settings
            .agent_command_configs
            .insert("claude".into(), config.clone());
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: UiSettings = serde_json::from_str(&json).unwrap();
        let saved = &loaded.agent_command_configs["claude"];
        assert_eq!(saved, &config);
        let launch = saved.build_prompt_command("hello", "/bin/zsh", false).unwrap();
        assert!(launch.starts_with("capture --prompt --suffix <<'"));
        assert!(launch.contains("\nhello\n"));
        let mut argument = saved.clone();
        argument.prompt_stdin = false;
        argument.prompt_command.clear();
        assert_eq!(
            argument.build_prompt_command("hello", "/bin/zsh", false).unwrap(),
            "capture --plain 'hello' --suffix"
        );
    }
}
