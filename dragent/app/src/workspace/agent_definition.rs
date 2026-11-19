//! Agent definitions: the single source of truth for agent identity.
//!
//! An [`AgentDefinition`] describes *what an agent is and how it can be
//! launched, detected, and installed*. An agent *session* (one running
//! terminal process, identified by `agent_id` on the pane) references a
//! definition but owns independent runtime state — many sessions can share
//! one definition.
//!
//! Definitions resolve from two origins without the UI caring which:
//! - Built-ins: derived from the existing `PromptAgent` catalog
//!   (label, CLI, bundled image, install script). Nothing is duplicated;
//!   this module only re-presents that data under a stable string id.
//! - Customs: user-provided [`CustomAgentDefinition`] records stored in
//!   `UiSettings.custom_agents`. They flow through the same rail, install,
//!   launch, tab, and input-editor paths as built-ins.
//!
//! Stable ids: built-ins use `PromptAgent::command_key()` (`"claude"`,
//! `"opencode"`, …, `"dragent"`); customs use `"custom:<slug>"`.
//!
//! Session recovery (resuming an agent's own conversation after its process
//! is lost, e.g. a machine reboot) is part of the definition: built-ins take
//! their descriptor from `dragent_host::agent_recovery::builtin`, customs
//! declare one in `session_recovery`. `None` means the agent cannot be
//! resumed exactly, so recovery offers only a new session.

use super::action::PromptAgent;
use crate::setup::UiSettings;
pub use dragent_host::agent_recovery::SessionRecovery;

/// User-provided agent, stored in `UiSettings.custom_agents`. The launch
/// shape reuses [`crate::setup::AgentCommandConfig`] so customs behave like
/// built-ins wherever a command is needed.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CustomAgentDefinition {
    /// URL-safe slug, unique across customs. Referenced as `custom:<slug>`.
    pub id: String,
    /// Display name shown in the rail, tabs, and installer.
    pub name: String,
    /// Artwork: an absolute filesystem path (recommended) or `~`-prefixed
    /// path. Bundled `agent-images/…` paths cannot be added by users, so
    /// customs always resolve through the local filesystem.
    #[serde(default)]
    pub image: Option<String>,
    /// Binary used for installation detection. Defaults to the first token
    /// of `launch.command` when omitted.
    #[serde(default)]
    pub cli: Option<String>,
    /// Shell command that installs the agent. Shown in the shared installer
    /// modal exactly like a built-in install script.
    #[serde(default)]
    pub install_command: Option<String>,
    /// Bare-open and prompted launch configuration (same semantics as the
    /// per-agent settings overrides for built-ins).
    #[serde(default)]
    pub launch: crate::setup::settings::AgentCommandConfig,
    /// How this agent's conversations are identified and resumed. Omitted:
    /// the agent is never resumed automatically.
    #[serde(default)]
    pub session_recovery: Option<SessionRecovery>,
}

/// Resolved artwork for an agent chip/row/tab.
#[derive(Clone, Debug)]
pub enum AgentImage {
    /// Compile-time bundled asset (`agent-images/…`, `icons/…`).
    Bundled(&'static str),
    /// User-supplied file, verified to exist at resolve time.
    File(String),
}

/// The unified view every UI surface resolves: rail, workspace-tab chips,
/// agent input editor, installer, launcher, and session restore.
#[derive(Clone, Debug)]
pub struct AgentDefinition {
    /// Stable id: built-ins use the command key, customs `custom:<slug>`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Artwork, if any resolved for the current theme.
    pub image: Option<AgentImage>,
    /// CLI binary used for installation detection, if any.
    pub cli: Option<String>,
    /// Installer script for the shared install modal, if published.
    pub install_command: Option<String>,
    /// The built-in behind this definition, if any. `None` for customs.
    pub builtin: Option<PromptAgent>,
    /// Exact conversation resume, if the agent supports it.
    pub session_recovery: Option<SessionRecovery>,
}

impl AgentDefinition {
    /// A built-in derived from the existing catalog — no duplicated data.
    pub fn builtin(agent: PromptAgent, is_dark: bool) -> Self {
        Self {
            id: agent.command_key().to_string(),
            name: agent.label().to_string(),
            image: agent.image_path(is_dark).map(AgentImage::Bundled),
            cli: agent.cli().map(str::to_string),
            install_command: agent.install_command().map(str::to_string),
            builtin: Some(agent),
            session_recovery: agent
                .cli()
                .and_then(dragent_host::agent_recovery::builtin),
        }
    }

    /// A user-provided agent. Image paths resolve through the filesystem;
    /// missing files fall back to the generic glyph (never a broken image).
    pub fn custom(custom: &CustomAgentDefinition) -> Self {
        Self {
            id: custom_id(&custom.id),
            name: custom.name.clone(),
            image: custom
                .image
                .as_deref()
                .map(shellexpand_home)
                .filter(|path| std::path::Path::new(&path).is_file())
                .map(AgentImage::File),
            cli: custom
                .cli
                .clone()
                .or_else(|| first_token(&custom.launch.command)),
            install_command: custom.install_command.clone(),
            builtin: None,
            session_recovery: custom.session_recovery.clone(),
        }
    }

    pub fn is_custom(&self) -> bool {
        self.builtin.is_none()
    }

    pub fn supports_resume(&self) -> bool {
        self.session_recovery.is_some()
    }
}

/// Before launching `command` in `pane`, declare a custom agent's recovery
/// descriptor to the launch helper (built-ins are known to it already).
#[cfg(not(target_family = "wasm"))]
pub fn stage_custom_recovery(pane: uuid::Uuid, command: &str, settings: &UiSettings) {
    let Some(executable) = first_token(command.trim_start().trim_start_matches("& ")) else {
        return;
    };
    let name = dragent_host::agent_recovery::executable_name(executable.trim_matches(['\'', '"']));
    if dragent_host::agent_recovery::builtin(name).is_some() {
        return;
    }
    let Some(custom) = settings.custom_agents.iter().find(|custom| {
        custom
            .cli
            .clone()
            .or_else(|| first_token(&custom.launch.command))
            .is_some_and(|cli| dragent_host::agent_recovery::executable_name(&cli) == name)
    }) else {
        return;
    };
    let Some(recovery) = custom.session_recovery.as_ref() else {
        return;
    };
    if let Err(error) = dragent_host::agent_sessions::stage_custom_agent(
        pane,
        &custom_id(&custom.id),
        name,
        recovery,
    ) {
        log::warn!("custom agent recovery not staged pane={pane}: {error}");
    }
}

/// Stable id for a custom slug.
pub fn custom_id(slug: &str) -> String {
    format!("custom:{slug}")
}

/// True for `custom:<slug>` ids (user-provided agents).
pub fn is_custom_id(id: &str) -> bool {
    id.starts_with("custom:")
}

/// All definitions: every built-in plus every custom in `customs`. Pure —
/// safe to call from render paths.
pub fn all_definitions(
    customs: &[CustomAgentDefinition],
    is_dark: bool,
) -> Vec<AgentDefinition> {
    let mut defs: Vec<AgentDefinition> = PromptAgent::ALL
        .into_iter()
        .map(|agent| AgentDefinition::builtin(agent, is_dark))
        .collect();
    defs.extend(customs.iter().map(AgentDefinition::custom));
    defs
}

/// Resolve by stable id (`"claude"`, `"dragent"`, `"custom:<slug>"`).
pub fn definition_by_id(
    id: &str,
    customs: &[CustomAgentDefinition],
    is_dark: bool,
) -> Option<AgentDefinition> {
    if is_custom_id(id) {
        return customs
            .iter()
            .find(|custom| custom_id(&custom.id) == id)
            .map(AgentDefinition::custom);
    }
    // Legacy alias: the MiMo entry briefly shipped with the wrong binary
    // name as its stable id before the registry was corrected to the real
    // `mimo` executable. Persisted panes may still carry it.
    if id == "mimocode" {
        return Some(AgentDefinition::builtin(PromptAgent::MiMoCode, is_dark));
    }
    PromptAgent::ALL
        .into_iter()
        .find(|agent| agent.command_key() == id)
        .map(|agent| AgentDefinition::builtin(agent, is_dark))
}

/// Resolve by CLI binary (`"claude"`, `"opencode"`, …). Used to identify
/// sessions from their running/last command without stored `agent_id`.
pub fn definition_by_cli(
    cli: &str,
    customs: &[CustomAgentDefinition],
    is_dark: bool,
) -> Option<AgentDefinition> {
    if let Some(agent) = PromptAgent::ALL
        .into_iter()
        .find(|agent| agent.cli() == Some(cli))
    {
        return Some(AgentDefinition::builtin(agent, is_dark));
    }
    customs
        .iter()
        .find(|custom| custom.cli.as_deref() == Some(cli))
        .map(AgentDefinition::custom)
}

/// First whitespace-separated token of a shell command, if any.
fn first_token(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .next()
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn shellexpand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Builtin artwork by stable id, for session-bound UI (input editor) that
/// cannot reach settings. Customs resolve through cached definitions at
/// the workspace layer instead.
pub fn builtin_image_for_id(id: &str, is_dark: bool) -> Option<AgentImage> {
    PromptAgent::ALL
        .into_iter()
        .find(|agent| agent.command_key() == id)
        .and_then(|agent| agent.image_path(is_dark))
        .map(AgentImage::Bundled)
}

/// Builtin artwork by CLI binary, for sessions identified from their
/// running command rather than stored identity.
pub fn builtin_image_for_cli(cli: &str, is_dark: bool) -> Option<AgentImage> {
    PromptAgent::ALL
        .into_iter()
        .find(|agent| agent.cli() == Some(cli))
        .and_then(|agent| agent.image_path(is_dark))
        .map(AgentImage::Bundled)
}

/// Installation detection for customs. A custom counts as installed when its
/// launch configuration validates and its binary (explicit `cli` or first
/// launch-command token) resolves on PATH — or on config alone when it
/// declares no binary (aliases/functions, same rule as customized built-ins).
pub fn detect_custom_agents(settings: &UiSettings) -> std::collections::HashSet<String> {
    use super::action_projects::{agent_search_dirs, prompt_agent_command_in};
    let mut installed = std::collections::HashSet::new();
    // One shared search set for the whole pass (single login-shell probe).
    let dirs = agent_search_dirs();
    for custom in &settings.custom_agents {
        if custom.id.trim().is_empty() || custom.name.trim().is_empty() {
            continue;
        }
        if custom.launch.validate().is_err() {
            continue;
        }
        let binary = custom
            .cli
            .clone()
            .or_else(|| first_token(&custom.launch.command));
        match binary {
            Some(binary) => {
                if prompt_agent_command_in(&binary, &dirs).is_some() {
                    installed.insert(custom_id(&custom.id));
                }
            }
            None => {
                installed.insert(custom_id(&custom.id));
            }
        }
    }
    installed
}

#[cfg(test)]
mod tests {
    /// The input panel (rich input, voice, attachments) follows the running
    /// command. Every agent the launcher can start must be recognized by it,
    /// or that agent's pane shows a plain terminal input.
    #[test]
    fn every_launchable_agent_is_recognized_by_the_input_panel() {
        for agent in super::PromptAgent::ALL {
            let Some(cli) = agent.cli() else { continue };
            assert!(
                crate::console::ai_toolbar::AiTool::from_command(cli).is_some(),
                "{} (`{cli}`) would show no AI input panel: add it to AiTool",
                agent.label()
            );
        }
    }

    use super::*;

    #[test]
    fn every_builtin_declares_whether_it_resumes() {
        let resumable: Vec<_> = PromptAgent::ALL
            .into_iter()
            .filter(|agent| AgentDefinition::builtin(*agent, false).supports_resume())
            .map(|agent| agent.command_key())
            .collect();
        let mut expected = vec![
            "claude",
            "codex",
            "gemini",
            "opencode",
            "copilot",
            "codebuddy",
            "cursor-agent",
            "droid",
            "kimi",
            "mimo",
            "muse",
            "qwen",
        ];
        // Droid and Muse report through user-level hooks, which are Unix-only.
        if !cfg!(unix) {
            expected.retain(|agent| !matches!(*agent, "droid" | "muse"));
        }
        assert_eq!(resumable, expected);
    }

    #[test]
    fn custom_agents_declare_recovery_in_settings() {
        let custom: CustomAgentDefinition = serde_json::from_str(
            r#"{"id":"acme","name":"Acme","launch":{"command":"acme","prompt_command":"acme"},
               "session_recovery":{"strategy":{"kind":"preassigned","args":["--session-id","{id}"]},
                                   "resume":["--resume","{id}"],"id_format":"uuid"}}"#,
        )
        .unwrap();
        assert!(AgentDefinition::custom(&custom).supports_resume());
        let legacy: CustomAgentDefinition =
            serde_json::from_str(r#"{"id":"old","name":"Old"}"#).unwrap();
        assert!(!AgentDefinition::custom(&legacy).supports_resume());
    }

    #[test]
    fn legacy_mimocode_id_still_resolves() {
        // The MiMo entry briefly shipped with the wrong binary name as its
        // stable id; persisted panes may still carry it.
        let definition = definition_by_id("mimocode", &[], false).expect("alias resolves");
        assert_eq!(definition.builtin, Some(PromptAgent::MiMoCode));
        assert_eq!(definition.cli.as_deref(), Some("mimo"));
        let current = definition_by_id("mimo", &[], false).expect("current id resolves");
        assert_eq!(current.builtin, Some(PromptAgent::MiMoCode));
    }
}
