//! Agent autonomy settings: approval mode, context scope, and safety knobs.
//!
//! Pure serde model shared by the engine core, the Host, and the UI.
//! Single definition — the app re-exports these paths.
//!
//! Settings → Agent → General: default/disabled TUI agents, the agent's
//! approval mode, workspace trust, idle-tab confirmation, and Codex launch
//! permissions. Every addition is `#[serde(default)]`-backed so
//! pre-existing settings files deserialize unchanged.

use serde::{Deserialize, Serialize};

/// How much Dragent's agent may do before asking you (Settings → Agent →
/// General). Agents launched in terminals follow their own launch flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// File changes, shell commands (except read-only ones while "Auto-run
    /// safe commands" is on), pull requests and posts to connected services
    /// ask first. The default.
    #[default]
    Ask,
    /// File edits run; shell commands, moving or deleting files, pull
    /// requests and posts ask first.
    AcceptEdits,
    /// Nothing asks, except reading files outside the project.
    Auto,
}

impl ApprovalMode {
    pub const ALL: [Self; 3] = [Self::Ask, Self::AcceptEdits, Self::Auto];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::AcceptEdits => "Accept edits",
            Self::Auto => "Don't ask",
        }
    }

    /// What the mode lets through without asking, in one sentence.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Ask => "File changes, commands, pull requests and posts ask first.",
            Self::AcceptEdits => {
                "File edits run; commands, moving or deleting files, pull requests and posts ask first."
            }
            Self::Auto => "Nothing asks, except reading files outside the project.",
        }
    }
}

/// How the approval mode is stored: as the two settings earlier builds
/// had, in values every build reads. A value an older Dragent couldn't
/// parse would make it fall back to defaults for all of its settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StoredApproval {
    Manual,
    #[default]
    Suggest,
    Auto,
}

/// The second stored half of the approval mode; Accept edits lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StoredPermission {
    Ask,
    AcceptEdits,
    Auto,
    #[default]
    FullAccess,
}

/// Which files the automatic context builder may draw from when pre-fetching
/// semantically relevant code for a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextScope {
    /// Only fragments under the pane's current working directory.
    CurrentDir,
    /// The whole indexed workspace (repo root). Default.
    #[default]
    Workspace,
    /// Only files tracked by git.
    GitTracked,
}

impl ContextScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentDir => "Current directory",
            Self::Workspace => "Workspace",
            Self::GitTracked => "Git-tracked only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAutonomySettings {
    /// Exposes the end-to-end local Agentic Workspace while it remains
    /// experimental. Existing settings files deserialize to disabled.
    #[serde(default)]
    pub agentic_workspace_enabled: bool,
    /// Stored form of [`approval_mode`](Self::approval_mode); change it with
    /// [`set_approval_mode`](Self::set_approval_mode).
    #[serde(default, rename = "approval_mode")]
    pub stored_approval: StoredApproval,
    /// Let the agent run read-only shell commands (ls, cat, git status …)
    /// without asking, even when approval would otherwise be required.
    #[serde(default = "default_true")]
    pub auto_run_safe_commands: bool,
    /// Upper limit on tool calls within a single agent task (one user query's
    /// streaming loop). Reads legacy configs that stored `max_agent_turns`.
    #[serde(default = "default_max_steps", alias = "max_agent_turns")]
    pub max_agent_steps: u32,
    /// Analyze failed commands in the background and surface a fix without
    /// being asked. Off by default — each scan is an LLM call.
    #[serde(default)]
    pub proactive_error_scanning: bool,
    /// When an analysis produces a fix, show the fix card immediately.
    #[serde(default = "default_true")]
    pub auto_fix_on_failure: bool,
    #[serde(default)]
    pub context_scope: ContextScope,

    // ── Agents settings ──────────────────────────────────────
    /// Command key of the default TUI agent (`None` = Dragent's chooser
    /// decides, currently Claude-first). Keys match
    /// `PromptAgent::command_key()` (`"claude"`, `"codex"`, …).
    #[serde(default)]
    pub default_agent: Option<String>,
    /// Command keys hidden from agent pickers and launch paths.
    #[serde(default)]
    pub disabled_agents: Vec<String>,
    /// Second half of the stored approval mode.
    #[serde(default, rename = "permission_mode")]
    pub stored_permission: StoredPermission,
    /// When false, untrusted-workspace prompts are skipped: folders open as
    /// if trusted.
    #[serde(default = "default_true")]
    pub workspace_trust_enabled: bool,
    /// Ask before closing a tab with nothing running. Off by default:
    /// closing an idle pane loses nothing (running processes use the General
    /// setting). Stored under a new name so the old default-on value saved
    /// by earlier builds (`confirm_close_pinned_tab`) no longer applies.
    #[serde(default, rename = "confirm_close_idle")]
    pub confirm_close_idle_tab: bool,
    /// When true, Codex launches keep their isolated-server sandbox bypass
    /// flags; when false those flags are stripped so Codex runs sandboxed.
    #[serde(default = "default_true")]
    pub terminal_server_isolation: bool,
    /// Minutes an agent tab must sit idle, finished and off screen before
    /// its agent process is stopped to free memory (it resumes on the same
    /// conversation when the tab is opened). `0` never puts agents to sleep.
    #[serde(default = "default_idle_sleep_minutes")]
    pub idle_sleep_minutes: u32,
}

/// The choices offered for [`AgentAutonomySettings::idle_sleep_minutes`].
pub const IDLE_SLEEP_CHOICES: [u32; 4] = [15, 30, 60, 0];

fn default_idle_sleep_minutes() -> u32 {
    15
}

fn default_true() -> bool {
    true
}

fn default_max_steps() -> u32 {
    200
}

impl Default for AgentAutonomySettings {
    fn default() -> Self {
        Self {
            agentic_workspace_enabled: false,
            stored_approval: StoredApproval::Suggest,
            auto_run_safe_commands: true,
            max_agent_steps: default_max_steps(),
            proactive_error_scanning: false,
            auto_fix_on_failure: true,
            context_scope: ContextScope::Workspace,
            default_agent: None,
            disabled_agents: Vec::new(),
            stored_permission: StoredPermission::FullAccess,
            workspace_trust_enabled: true,
            confirm_close_idle_tab: false,
            terminal_server_isolation: true,
            idle_sleep_minutes: default_idle_sleep_minutes(),
        }
    }
}

impl AgentAutonomySettings {
    /// How much Dragent's agent may do before asking. Settings saved while
    /// there were two controls read as the stricter of them, except that an
    /// explicit "Accept edits" now does what it says.
    pub fn approval_mode(&self) -> ApprovalMode {
        match (self.stored_approval, self.stored_permission) {
            (StoredApproval::Manual, _) | (_, StoredPermission::Ask) => ApprovalMode::Ask,
            (_, StoredPermission::AcceptEdits) => ApprovalMode::AcceptEdits,
            (StoredApproval::Auto, _) => ApprovalMode::Auto,
            (StoredApproval::Suggest, _) => ApprovalMode::Ask,
        }
    }

    /// Store `mode` so earlier builds read it as the same or stricter: they
    /// see Accept edits as their asking default.
    pub fn set_approval_mode(&mut self, mode: ApprovalMode) {
        (self.stored_approval, self.stored_permission) = match mode {
            ApprovalMode::Ask => (StoredApproval::Suggest, StoredPermission::FullAccess),
            ApprovalMode::AcceptEdits => (StoredApproval::Suggest, StoredPermission::AcceptEdits),
            ApprovalMode::Auto => (StoredApproval::Auto, StoredPermission::FullAccess),
        };
    }

    /// [`set_approval_mode`](Self::set_approval_mode), for building settings.
    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.set_approval_mode(mode);
        self
    }

    /// Whether `agent_key` (a `PromptAgent::command_key()`) is disabled.
    pub fn is_agent_disabled(&self, agent_key: &str) -> bool {
        self.disabled_agents.iter().any(|key| key == agent_key)
    }

    /// Filter a list of command keys down to the enabled ones, preserving order.
    pub fn enabled_agent_keys(&self, keys: impl IntoIterator<Item = String>) -> Vec<String> {
        keys.into_iter()
            .filter(|key| !self.is_agent_disabled(key))
            .collect()
    }

    /// Resolve the configured default agent against a candidate list of
    /// command keys: the configured key when present and not disabled, else
    /// the first enabled candidate, else `None`.
    pub fn resolve_default_agent(&self, candidates: &[String]) -> Option<String> {
        if let Some(configured) = self.default_agent.as_deref() {
            if !configured.trim().is_empty()
                && !self.is_agent_disabled(configured)
                && candidates.iter().any(|key| key == configured)
            {
                return Some(configured.to_string());
            }
        }
        candidates
            .iter()
            .find(|key| !self.is_agent_disabled(key))
            .cloned()
    }

    /// Toggle one agent key in [`disabled_agents`](Self::disabled_agents).
    /// Returns true when the agent is now disabled.
    pub fn toggle_disabled_agent(&mut self, agent_key: &str) -> bool {
        if let Some(index) = self
            .disabled_agents
            .iter()
            .position(|key| key == agent_key)
        {
            self.disabled_agents.remove(index);
            false
        } else {
            self.disabled_agents.push(agent_key.to_string());
            true
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_ignore_retired_controls() {
        let settings: AgentAutonomySettings = serde_json::from_value(serde_json::json!({
            "confirm_close_pinned_tab": true,
            "status_hooks_enabled": false,
            "state_rules_path": "unused.md",
            "state_rules_live_updates": false,
            "tab_auto_generate_title": false,
            "workspace_trust_enabled": false,
            "terminal_server_isolation": false
        })).unwrap();
        // The old idle-close value (default on) no longer applies: idle
        // panes close without asking unless the new setting is turned on.
        assert!(!settings.confirm_close_idle_tab);
        assert!(!settings.workspace_trust_enabled);
        assert!(!settings.terminal_server_isolation);
        let saved = serde_json::to_value(&settings).unwrap();
        assert_eq!(saved["confirm_close_idle"], false);
        assert!(saved.get("confirm_close_pinned_tab").is_none());
        assert!(saved.get("confirm_close_idle_tab").is_none());
        assert!(saved.get("state_rules_path").is_none());
        let restored: AgentAutonomySettings = serde_json::from_value(saved).unwrap();
        assert_eq!(restored, settings);
    }

    #[test]
    fn legacy_empty_settings_keep_defaults() {
        let settings: AgentAutonomySettings = serde_json::from_str("{}").expect("settings");
        assert_eq!(settings.approval_mode(), ApprovalMode::Ask);
        assert_eq!(settings.default_agent, None);
        assert!(settings.disabled_agents.is_empty());
        assert!(settings.workspace_trust_enabled);
        assert!(!settings.confirm_close_idle_tab);
        assert!(settings.terminal_server_isolation);
    }

    #[test]
    fn settings_from_the_two_old_controls_read_as_one_mode() {
        let read = |approval: &str, permission: &str| {
            serde_json::from_str::<AgentAutonomySettings>(&format!(
                r#"{{"approval_mode":"{approval}","permission_mode":"{permission}"}}"#
            ))
            .expect("settings")
            .approval_mode()
        };
        // The stricter of the two wins…
        assert_eq!(read("manual", "full_access"), ApprovalMode::Ask);
        assert_eq!(read("auto", "ask"), ApprovalMode::Ask);
        assert_eq!(read("suggest", "full_access"), ApprovalMode::Ask);
        assert_eq!(read("suggest", "auto"), ApprovalMode::Ask);
        assert_eq!(read("auto", "full_access"), ApprovalMode::Auto);
        assert_eq!(read("auto", "auto"), ApprovalMode::Auto);
        // …and a chosen "Accept edits" means it.
        assert_eq!(read("suggest", "accept_edits"), ApprovalMode::AcceptEdits);
        assert_eq!(read("auto", "accept_edits"), ApprovalMode::AcceptEdits);
        assert_eq!(read("manual", "accept_edits"), ApprovalMode::Ask);
    }

    #[test]
    fn modes_are_stored_in_values_earlier_builds_read() {
        for mode in ApprovalMode::ALL {
            let settings = AgentAutonomySettings::default().with_approval_mode(mode);
            let json = serde_json::to_value(&settings).expect("json");
            let approval = json["approval_mode"].as_str().expect("approval_mode");
            let permission = json["permission_mode"].as_str().expect("permission_mode");
            assert!(["manual", "suggest", "auto"].contains(&approval), "{approval}");
            assert!(
                ["ask", "accept_edits", "auto", "full_access"].contains(&permission),
                "{permission}"
            );
            let restored: AgentAutonomySettings = serde_json::from_value(json).expect("restore");
            assert_eq!(restored.approval_mode(), mode);
        }
        // An earlier build sees Don't ask as Auto and the others as its
        // asking default, never as more permissive.
        let auto = AgentAutonomySettings::default().with_approval_mode(ApprovalMode::Auto);
        assert_eq!(auto.stored_approval, StoredApproval::Auto);
        let edits = AgentAutonomySettings::default().with_approval_mode(ApprovalMode::AcceptEdits);
        assert_eq!(edits.stored_approval, StoredApproval::Suggest);
    }

    #[test]
    fn disabled_and_default_agent_resolution() {
        let mut settings = AgentAutonomySettings::default();
        let candidates = ["claude", "codex", "gemini"]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        // No configured default: first enabled candidate.
        assert_eq!(
            settings.resolve_default_agent(&candidates).as_deref(),
            Some("claude")
        );
        settings.default_agent = Some("codex".to_string());
        assert_eq!(
            settings.resolve_default_agent(&candidates).as_deref(),
            Some("codex")
        );
        // A disabled configured default falls back to the first enabled.
        assert!(settings.toggle_disabled_agent("codex"));
        assert!(settings.is_agent_disabled("codex"));
        assert_eq!(
            settings.resolve_default_agent(&candidates).as_deref(),
            Some("claude")
        );
        // Unknown configured keys fall back too.
        settings.default_agent = Some("nope".to_string());
        assert_eq!(
            settings.resolve_default_agent(&candidates).as_deref(),
            Some("claude")
        );
        assert_eq!(
            settings.enabled_agent_keys(candidates),
            vec!["claude".to_string(), "gemini".to_string()]
        );
        assert!(!settings.toggle_disabled_agent("codex"));
        assert!(!settings.is_agent_disabled("codex"));
    }

    #[test]
    fn permission_and_trust_flags_round_trip() {
        let mut settings = AgentAutonomySettings::default();
        settings.set_approval_mode(ApprovalMode::AcceptEdits);
        settings.disabled_agents.push("droid".to_string());
        settings.default_agent = Some("claude".to_string());
        settings.workspace_trust_enabled = false;
        settings.confirm_close_idle_tab = false;
        settings.terminal_server_isolation = false;
        let json = serde_json::to_string(&settings).expect("json");
        let restored: AgentAutonomySettings = serde_json::from_str(&json).expect("restore");
        assert_eq!(restored, settings);
    }

}
