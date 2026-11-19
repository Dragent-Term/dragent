//! App-level preferences (agent backend, autonomy, indexing, privacy toggles, MCP).

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    #[default]
    Mock,
    Cloud,
    Ollama,
}

/// Reasoning depth for an agent query. Defined in the shared viewer crate
/// alongside ChatMode so the input-bar actions can name it.
pub use dragent_viewer::agent::effort::AgentEffort;

impl AgentBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mock => "Mock (local dev)",
            Self::Cloud => "Cloud",
            Self::Ollama => "Ollama (local)",
        }
    }
}

pub use dragent_host::mcp_config::{McpServerEntry, McpTransport};

// ── Agent autonomy ────────────────────────────────────────────────────────────
// Single definitions in `dragent_agent::autonomy`, shared by the engine core,
// the Host, and the UI.
pub use dragent_agent::autonomy::{AgentAutonomySettings, ApprovalMode, ContextScope};

// ── Codebase index ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingsBackend {
    /// On-device ONNX embedder — code never leaves the machine. The only
    /// implemented backend today; `Cloud` is reserved for a future server path.
    #[default]
    Local,
    Cloud,
}

impl EmbeddingsBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local (on-device)",
            Self::Cloud => "Cloud",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexSettings {
    /// Master switch for codebase indexing.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Re-index files automatically as they change (file watcher). When off,
    /// the index is built once per repo and only refreshed by Re-index.
    #[serde(default = "default_true")]
    pub auto_index: bool,
    #[serde(default)]
    pub embeddings: EmbeddingsBackend,
    /// Skip files excluded by .gitignore when walking the repo.
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
}

impl Default for IndexSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_index: true,
            embeddings: EmbeddingsBackend::Local,
            respect_gitignore: true,
        }
    }
}

// ── Provider catalog removed — model list is now sourced from GET /api/v1/account ──────────

// ── Provider keys ─────────────────────────────────────────────────────────────

/// Stores one credential (API key or base URL) per provider.
/// Provider IDs match the slugs returned by GET /api/v1/account:
///   anthropic, openai, google (Gemini), xai (Grok), groq, cerebras,
///   openrouter, meta (Muse Spark via the Meta Model API).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderKeys {
    #[serde(default)]
    pub anthropic: Option<String>,
    #[serde(default)]
    pub openai: Option<String>,
    #[serde(default)]
    pub google: Option<String>,
    #[serde(default)]
    pub xai: Option<String>,
    #[serde(default)]
    pub groq: Option<String>,
    #[serde(default)]
    pub cerebras: Option<String>,
    #[serde(default)]
    pub openrouter: Option<String>,
    #[serde(default)]
    pub meta: Option<String>,
    #[serde(default)]
    pub deepseek: Option<String>,
    #[serde(default)]
    pub qwen: Option<String>,
    #[serde(default)]
    pub zhipu: Option<String>,
    #[serde(default)]
    pub moonshot: Option<String>,
    #[serde(default)]
    pub minimax: Option<String>,
    #[serde(default)]
    pub mistral: Option<String>,
    #[serde(default)]
    pub together: Option<String>,
    #[serde(default)]
    pub deepinfra: Option<String>,
    #[serde(default)]
    pub perplexity: Option<String>,
    #[serde(default)]
    pub venice: Option<String>,
    #[serde(default)]
    pub nvidia: Option<String>,
    #[serde(default)]
    pub fireworks: Option<String>,
    #[serde(default)]
    pub cohere: Option<String>,
    #[serde(default)]
    pub ollama_base_url: Option<String>,
    #[serde(default)]
    pub lmstudio_base_url: Option<String>,
}

impl ProviderKeys {
    /// Returns the credential for the given provider ID.
    pub fn get(&self, provider_id: &str) -> Option<&str> {
        match provider_id {
            "anthropic" => self.anthropic.as_deref().filter(|k| !k.is_empty()),
            "openai" => self.openai.as_deref().filter(|k| !k.is_empty()),
            "google" => self.google.as_deref().filter(|k| !k.is_empty()),
            "xai" => self.xai.as_deref().filter(|k| !k.is_empty()),
            "groq" => self.groq.as_deref().filter(|k| !k.is_empty()),
            "cerebras" => self.cerebras.as_deref().filter(|k| !k.is_empty()),
            "openrouter" => self.openrouter.as_deref().filter(|k| !k.is_empty()),
            "meta" => self.meta.as_deref().filter(|k| !k.is_empty()),
            "deepseek" => self.deepseek.as_deref().filter(|k| !k.is_empty()),
            "qwen" => self.qwen.as_deref().filter(|k| !k.is_empty()),
            "zhipu" => self.zhipu.as_deref().filter(|k| !k.is_empty()),
            "moonshot" => self.moonshot.as_deref().filter(|k| !k.is_empty()),
            "minimax" => self.minimax.as_deref().filter(|k| !k.is_empty()),
            "mistral" => self.mistral.as_deref().filter(|k| !k.is_empty()),
            "together" => self.together.as_deref().filter(|k| !k.is_empty()),
            "deepinfra" => self.deepinfra.as_deref().filter(|k| !k.is_empty()),
            "perplexity" => self.perplexity.as_deref().filter(|k| !k.is_empty()),
            "venice" => self.venice.as_deref().filter(|k| !k.is_empty()),
            "nvidia" => self.nvidia.as_deref().filter(|k| !k.is_empty()),
            "fireworks" => self.fireworks.as_deref().filter(|k| !k.is_empty()),
            "cohere" => self.cohere.as_deref().filter(|k| !k.is_empty()),
            "ollama" => self.ollama_base_url.as_deref().filter(|k| !k.is_empty()),
            "lmstudio" => self.lmstudio_base_url.as_deref().filter(|k| !k.is_empty()),
            _ => None,
        }
    }

    pub fn set(&mut self, provider_id: &str, value: String) {
        // Clipboard pastes frequently include a trailing newline or spaces.
        // Persist credentials in canonical form so the selector can unlock and
        // the provider receives the exact key, not an invalid whitespace-suffixed
        // value that silently triggers platform fallback.
        let value = value.trim().to_string();
        match provider_id {
            "anthropic" => self.anthropic = Some(value),
            "openai" => self.openai = Some(value),
            "google" => self.google = Some(value),
            "xai" => self.xai = Some(value),
            "groq" => self.groq = Some(value),
            "cerebras" => self.cerebras = Some(value),
            "openrouter" => self.openrouter = Some(value),
            "meta" => self.meta = Some(value),
            "deepseek" => self.deepseek = Some(value),
            "qwen" => self.qwen = Some(value),
            "zhipu" => self.zhipu = Some(value),
            "moonshot" => self.moonshot = Some(value),
            "minimax" => self.minimax = Some(value),
            "mistral" => self.mistral = Some(value),
            "together" => self.together = Some(value),
            "deepinfra" => self.deepinfra = Some(value),
            "perplexity" => self.perplexity = Some(value),
            "venice" => self.venice = Some(value),
            "nvidia" => self.nvidia = Some(value),
            "fireworks" => self.fireworks = Some(value),
            "cohere" => self.cohere = Some(value),
            "ollama" => self.ollama_base_url = Some(value),
            "lmstudio" => self.lmstudio_base_url = Some(value),
            _ => {}
        }
    }

    pub fn clear(&mut self, provider_id: &str) {
        self.set(provider_id, String::new());
    }
}

// ── Custom models ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomModel {
    pub name: String,
    pub base_url: String,
    pub model_id: String,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Wire protocol spoken by the endpoint. Defaults to chat completions
    /// so existing records keep working; `responses` targets GPT-5.6-class
    /// and other Responses-API endpoints, `anthropic_messages` targets
    /// Anthropic-compatible proxies.
    #[serde(default)]
    pub endpoint_style: dragent_agent::direct::CustomEndpointStyle,
}

// ── AppSettings ───────────────────────────────────────────────────────────────

/// The only model available when the account has no premium-capable funds.
pub const FREE_TIER_MODEL_ID: &str = "gpt-5.6-luna";
pub const AUTO_MODEL_ID: &str = "auto";

/// A favorite model belongs to its CLI agent; identical IDs can mean different things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelFavorite {
    pub agent: String,
    pub model: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub agent_backend: AgentBackend,
    /// Primary model used for agent queries. Auto delegates model selection to
    /// the server-side default configuration; explicit model IDs remain visible
    /// and selectable separately.
    #[serde(default = "default_model")]
    pub selected_model: String,
    /// Reasoning depth sent with each agent query. The server drops it for
    /// models that reject the parameter, so this is safe to leave set while
    /// switching models.
    #[serde(default)]
    pub selected_effort: AgentEffort,
    /// Per-provider credentials (API keys or base URLs), stored locally and sent per-request.
    #[serde(default)]
    pub provider_keys: ProviderKeys,
    /// Custom OpenAI-compatible model endpoints added by the user.
    #[serde(default)]
    pub custom_models: Vec<CustomModel>,
    #[serde(default)]
    pub agent_model_favorites: Vec<AgentModelFavorite>,
    #[serde(default)]
    pub telemetry_enabled: bool,
    #[serde(default)]
    pub crash_reporting_enabled: bool,
    #[serde(default)]
    pub cloud_conversation_storage_enabled: bool,
    /// When the user's own API key is missing or failing for the selected
    /// model's provider, may requests fall back to Dragent's platform keys?
    /// Off = requests fail instead (policy-safe: nothing ever runs on our
    /// keys once the user opted out).
    #[serde(default = "default_true")]
    pub allow_dragent_fallback: bool,
    /// Agent autonomy & self-healing behavior (Settings → Agent → General).
    #[serde(default)]
    pub agent: AgentAutonomySettings,
    /// Codebase indexing behavior (Settings → Code and Index).
    #[serde(default)]
    pub index: IndexSettings,
    /// Stable per-installation identifier for remote/shared sessions. Generated
    /// once on first use and persisted. Read via [`AppSettings::device_id`].
    #[serde(default)]
    pub device_id: Option<String>,
    /// Suppress the "Log out?" confirmation (user checked "Don't ask again").
    #[serde(default)]
    pub skip_logout_confirm: bool,
    /// Desktop notifications and their sound (Settings → Notifications).
    #[serde(default)]
    pub notifications: crate::notifications::NotificationSettings,
    /// Keyboard shortcuts: overrides, terminal policy, Ctrl+Tab order.
    #[serde(default)]
    pub shortcuts: super::shortcut_settings::ShortcutSettings,
    /// Developer permissions: keep-awake behaviour.
    #[serde(default)]
    pub dev_permissions: super::shortcut_settings::DevPermissionsSettings,
    /// Computer use: master switch, confirmation, allowlist.
    #[serde(default)]
    pub computer_use: super::shortcut_settings::ComputerUseSettings,
    /// Voice input (Settings → Voice).
    #[serde(default)]
    pub voice: super::voice_settings::VoiceSettings,
    /// Git defaults (Settings → Version Control).
    #[serde(default)]
    pub git: super::git_settings::GitSettings,
    /// Repo defaults template; per-project overrides live in
    /// `RepoSettingsStore` keyed by project root.
    #[serde(default)]
    pub repo: super::git_settings::RepoSettings,
    /// General / workspace / confirmations / setup-guide (Settings → General).
    #[serde(default)]
    pub general: super::git_settings::GeneralSettings,
    /// MCP servers — not stored in app_settings.json; loaded from ~/.dragent/mcp.json.
    #[serde(skip)]
    pub mcp_servers: Vec<McpServerEntry>,
    /// Generic plugin system overlay: master switch, per-plugin
    /// disables, first-run consents, dev manifest paths.
    #[serde(default)]
    pub plugin: super::plugin_settings::PluginSettings,
    /// Session sharing gate.
    #[serde(default)]
    pub sharing: super::plugin_settings::SharingSettings,
    /// Tasks defaults (view preset, source, visible providers,
    /// default repos / Linear teams).
    #[serde(default)]
    pub tasks: super::task_settings::TaskSettings,
    /// SSH host settings (default host, per-host overrides,
    /// local runtime / WSL, workspace-routing toggles).
    #[serde(default)]
    pub hosts: super::task_settings::HostSettings,
}

fn default_model() -> String {
    AUTO_MODEL_ID.to_string()
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            agent_backend: AgentBackend::Mock,
            selected_model: default_model(),
            selected_effort: AgentEffort::default(),
            provider_keys: ProviderKeys::default(),
            custom_models: Vec::new(),
            agent_model_favorites: Vec::new(),
            telemetry_enabled: false,
            crash_reporting_enabled: false,
            cloud_conversation_storage_enabled: false,
            allow_dragent_fallback: true,
            agent: AgentAutonomySettings::default(),
            index: IndexSettings::default(),
            device_id: None,
            skip_logout_confirm: false,
            notifications: Default::default(),
            shortcuts: Default::default(),
            dev_permissions: Default::default(),
            computer_use: Default::default(),
            voice: Default::default(),
            git: Default::default(),
            repo: Default::default(),
            general: Default::default(),
            mcp_servers: Vec::new(),
            plugin: Default::default(),
            sharing: Default::default(),
            tasks: Default::default(),
            hosts: Default::default(),
        }
    }
}

impl AppSettings {
    // ── Provider routing helpers ──────────────────────────────────────────────

    /// Returns the provider ID for a model ID based on naming conventions.
    pub fn provider_id_for_model(model_id: &str) -> Option<String> {
        Self::provider_id_for_model_static(model_id)
    }

    /// Static version — does not require a `self` reference.
    pub fn provider_id_for_model_static(model_id: &str) -> Option<String> {
        // Explicit routing prefixes first
        if model_id.starts_with("groq/") {
            return Some("groq".into());
        }
        if model_id.starts_with("cerebras/") {
            return Some("cerebras".into());
        }
        if model_id.starts_with("openrouter/") {
            return Some("openrouter".into());
        }
        if model_id.starts_with("ollama/") {
            return Some("ollama".into());
        }
        if model_id.starts_with("lmstudio/") {
            return Some("lmstudio".into());
        }
        if model_id.starts_with("google/") {
            return Some("google".into());
        }
        if model_id.starts_with("meta/") {
            return Some("meta".into());
        }
        if model_id.starts_with("deepseek/") {
            return Some("deepseek".into());
        }
        if model_id.starts_with("qwen/") || model_id.starts_with("alibaba/") {
            return Some("qwen".into());
        }
        if model_id.starts_with("glm/") || model_id.starts_with("zhipu/") {
            return Some("zhipu".into());
        }
        if model_id.starts_with("kimi/") || model_id.starts_with("moonshot/") {
            return Some("moonshot".into());
        }
        if model_id.starts_with("minimax/") {
            return Some("minimax".into());
        }
        if model_id.starts_with("mistral/") {
            return Some("mistral".into());
        }
        if model_id.starts_with("together/") {
            return Some("together".into());
        }
        if model_id.starts_with("deepinfra/") {
            return Some("deepinfra".into());
        }
        if model_id.starts_with("perplexity/") {
            return Some("perplexity".into());
        }
        if model_id.starts_with("venice/") {
            return Some("venice".into());
        }
        if model_id.starts_with("nvidia/") {
            return Some("nvidia".into());
        }
        if model_id.starts_with("fireworks/") {
            return Some("fireworks".into());
        }
        if model_id.starts_with("cohere/") {
            return Some("cohere".into());
        }
        // Muse Spark family only (Meta Model API). The retired Llama API
        // preview is deliberately unsupported: bare llama names belong to
        // third-party hosts (groq/…, ollama/…) or nowhere, never to Meta.
        {
            let lower = model_id.trim().to_ascii_lowercase();
            if lower.starts_with("muse-spark") {
                return Some("meta".into());
            }
        }
        // Convention-based detection
        if model_id.starts_with("claude-") {
            return Some("anthropic".into());
        }
        if model_id.starts_with("gpt-")
            || model_id.starts_with("o1-")
            || model_id.starts_with("o3-")
            || model_id.starts_with("o4-")
        {
            return Some("openai".into());
        }
        if model_id == "o3" || model_id == "o4-mini" {
            return Some("openai".into());
        }
        if model_id.starts_with("gemini-") {
            return Some("google".into());
        }
        if model_id.starts_with("grok-") {
            return Some("xai".into());
        }
        // First-party Chinese-model families by naming convention. Runs after
        // the explicit prefixes above, so "groq/…" / "cerebras/…" hosted
        // variants keep routing to their host (notably bare "qwen*" still
        // resolves provider "qwen" here, but without a user key the turn
        // stays on the cloud path, where the server serves it via Cerebras
        // exactly as before — no behavior change for keyless users).
        // Deliberately narrow for DeepSeek: bare "deepseek-r1*" names belong
        // to local Ollama catalogs; only the two official API ids route here.
        if model_id == "deepseek-chat" || model_id == "deepseek-reasoner" {
            return Some("deepseek".into());
        }
        {
            let lower = model_id.trim().to_ascii_lowercase();
            if lower.starts_with("qwen") {
                return Some("qwen".into());
            }
            if lower.starts_with("glm") {
                return Some("zhipu".into());
            }
            if lower.starts_with("kimi-") || lower.starts_with("moonshot") {
                return Some("moonshot".into());
            }
            if lower.starts_with("minimax") {
                return Some("minimax".into());
            }
            // Bare first-party ids with unambiguous vendor namespaces.
            // Hosted-namespace models (together/meta-llama/…,
            // deepinfra/…, venice/…, nvidia/…, fireworks/…) stay
            // prefix-only: their bare forms collide across hosts.
            if lower.starts_with("mistral-") || lower.starts_with("codestral-") {
                return Some("mistral".into());
            }
            if lower.starts_with("sonar-") || lower == "sonar" {
                return Some("perplexity".into());
            }
            if lower.starts_with("command-") || lower.starts_with("c4ai-") {
                return Some("cohere".into());
            }
        }
        None
    }

    /// Returns the user API key for the provider that owns `model_id`, if any.
    pub fn user_api_key_for_model(&self, model_id: &str) -> Option<&str> {
        let provider_id = Self::provider_id_for_model_static(model_id)?;
        self.provider_keys.get(&provider_id)
    }

    pub fn has_any_user_credential(&self) -> bool {
        [
            "anthropic",
            "openai",
            "google",
            "xai",
            "groq",
            "cerebras",
            "openrouter",
            "meta",
            "deepseek",
            "qwen",
            "zhipu",
            "moonshot",
            "minimax",
            "mistral",
            "together",
            "deepinfra",
            "perplexity",
            "venice",
            "nvidia",
            "fireworks",
            "cohere",
            "ollama",
            "lmstudio",
        ]
        .iter()
        .any(|provider| self.provider_keys.get(provider).is_some())
            || self
                .custom_models
                .iter()
                .any(|model| !model.base_url.trim().is_empty())
    }

    pub fn has_user_credential_for_model(&self, model_id: &str) -> bool {
        self.custom_models
            .iter()
            .any(|model| model.model_id == model_id && !model.base_url.trim().is_empty())
            || self.user_api_key_for_model(model_id).is_some()
            || self.custom_base_url_for_model(model_id).is_some()
    }

    /// Returns the custom base URL for Ollama/LM Studio/custom models.
    pub fn custom_base_url_for_model(&self, model_id: &str) -> Option<&str> {
        // Check custom models list first
        if let Some(cm) = self.custom_models.iter().find(|m| m.model_id == model_id) {
            return Some(&cm.base_url);
        }
        // Ollama/LM Studio base URLs stored in provider_keys
        if model_id.starts_with("ollama/") {
            return self.provider_keys.get("ollama");
        }
        if model_id.starts_with("lmstudio/") {
            return self.provider_keys.get("lmstudio");
        }
        None
    }

    /// Returns true if `model_id` is in the user's `custom_models` list.
    pub fn is_custom_model(&self, model_id: &str) -> bool {
        self.custom_models.iter().any(|m| m.model_id == model_id)
    }

    // ── Key routing ───────────────────────────────────────────────────────────

    /// Headers for a model request that goes to Dragent's server: only
    /// whether Dragent's own keys may serve it (`X-Fallback-Allowed`, the
    /// user's fallback toggle).
    ///
    /// The user's own keys and endpoints never go to the server: a model
    /// they have a key for is called straight from the app (see
    /// `agent::pipeline::step_direct`), so whatever reaches the server runs
    /// on Dragent's keys or not at all.
    pub fn request_key_headers(
        &self,
        _model_name: &str,
    ) -> Result<Vec<(&'static str, String)>, String> {
        let mut headers: Vec<(&'static str, String)> = Vec::new();
        headers.push((
            "X-Fallback-Allowed",
            if self.allow_dragent_fallback {
                "true"
            } else {
                "false"
            }
            .to_string(),
        ));

        Ok(headers)
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    pub fn config_path() -> PathBuf {
        dragent_paths::home().join("app_settings.json")
    }

    /// Stable per-installation id, generated once and persisted. Identifies this
    /// desktop to the backend when registering a machine / opening a shared
    /// session. Cached for the process after the first call.
    pub fn device_id() -> String {
        static DEVICE_ID: OnceLock<String> = OnceLock::new();
        DEVICE_ID
            .get_or_init(|| {
                if let Some(id) = Self::global().device_id.clone() {
                    return id;
                }
                // Load fresh from disk before writing so we don't clobber a
                // concurrent settings edit, and keep an id that appeared in the
                // meantime.
                let mut fresh = Self::load();
                match fresh.device_id.clone() {
                    Some(id) => id,
                    None => {
                        let id = uuid::Uuid::new_v4().to_string();
                        fresh.device_id = Some(id.clone());
                        fresh.save();
                        id
                    }
                }
            })
            .clone()
    }

    /// Whether the "Log out?" confirmation should be skipped.
    pub fn skip_logout_confirm() -> bool {
        Self::global().skip_logout_confirm
    }

    /// Persist the user's "Don't ask again" choice for the logout confirmation.
    pub fn set_skip_logout_confirm(value: bool) {
        let mut fresh = Self::load();
        fresh.skip_logout_confirm = value;
        fresh.save();
    }

    /// MCP servers configured before plugin consents existed were added by
    /// the user, so they count as approved and keep working. `raw` is the
    /// settings file as read: once it holds a `plugin` section, consents are
    /// whatever was saved. A plugin's servers (`from_plugins`) were added by
    /// an install, not the person: they approve those themselves.
    fn approve_servers_from_before_consents(
        &mut self,
        raw: &str,
        servers: &[McpServerEntry],
        from_plugins: &dyn Fn() -> Vec<String>,
    ) {
        let saved_before_consents = serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .is_none_or(|value| value.get("plugin").is_none());
        if !saved_before_consents {
            return;
        }
        let from_plugins = from_plugins();
        for server in servers {
            if from_plugins.contains(&server.name) {
                continue;
            }
            if !self.plugin.has_consent(&server.name) {
                self.plugin
                    .set_consent(&server.name, super::plugin_settings::PLUGIN_CONSENT_VERSION);
            }
        }
    }

    pub fn load() -> Self {
        #[cfg(not(target_family = "wasm"))]
        fn plugin_server_entries() -> Vec<String> {
            dragent_plugins::Record::load(&crate::cli::plugins::store())
                .map(|record| record.server_entries())
                .unwrap_or_default()
        }
        #[cfg(target_family = "wasm")]
        fn plugin_server_entries() -> Vec<String> {
            Vec::new()
        }
        let path = Self::config_path();
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let mut settings: Self = serde_json::from_str(&raw).unwrap_or_default();

        // One-time migration: if ~/.dragent/mcp.json doesn't exist yet, copy any
        // servers that were saved in the old app_settings.json `mcp_servers` array.
        // Because the field is now #[serde(skip)], we extract it from the raw JSON.
        if !super::mcp_config::global_mcp_path().exists() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Ok(old_servers) = serde_json::from_value::<Vec<McpServerEntry>>(
                    value
                        .get("mcp_servers")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                ) {
                    super::mcp_config::migrate_from_app_settings(&old_servers);
                }
            }
        }

        // M2: provider keys live in the OS keychain (native). File values
        // migrate once (file wins, pushed to keychain best-effort), then
        // every slot resolves keychain-first; failures degrade to session
        // memory, never to loss or crash. In-memory maps stay complete so
        // all readers resolve unchanged. Wasm skips this entirely (no
        // keychain backend; the file keeps working as before).
        #[cfg(not(target_family = "wasm"))]
        {
            for slot in super::credentials::KEY_SLOTS {
                let file_value = settings.provider_keys.get(slot).map(str::to_string);
                match super::credentials::resolve_provider_slot(file_value.as_deref(), slot) {
                    Some(resolved) => settings.provider_keys.set(slot, resolved),
                    None => settings.provider_keys.clear(slot),
                }
            }
            for custom in &mut settings.custom_models {
                let file_value = custom.api_key.clone();
                custom.api_key = super::credentials::resolve_custom_key(
                    file_value.as_deref(),
                    &custom.model_id.clone(),
                );
            }
        }

        settings.mcp_servers = super::mcp_config::load_entries();
        let servers = settings.mcp_servers.clone();
        settings.approve_servers_from_before_consents(&raw, &servers, &plugin_server_entries);
        // ~/.dragent/keybindings.json merges over stored
        // shortcuts (hand-editable mirror of the in-settings rebinding).
        settings.shortcuts.load_mirror_merge();
        settings
    }

    /// Clone with all credential material removed. Native builds persist
    /// through this scrubber so provider keys and custom-model secrets can
    /// never reach disk (M2: the OS keychain owns them); the live `self`
    /// keeps the full in-memory maps for every reader. Wasm has no keychain
    /// backend and keeps today's file behavior (callers use this only on
    /// native — see `save`).
    #[cfg(not(target_family = "wasm"))]
    fn without_credentials(&self) -> Self {
        let mut scrubbed = self.clone();
        scrubbed.provider_keys = ProviderKeys::default();
        for custom in &mut scrubbed.custom_models {
            custom.api_key = None;
        }
        scrubbed
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        #[cfg(not(target_family = "wasm"))]
        let json = serde_json::to_string_pretty(&self.without_credentials());
        #[cfg(target_family = "wasm")]
        let json = serde_json::to_string_pretty(self);
        if let Ok(json) = json {
            let _ = fs::write(path, json);
        }
        // Keep the hand-editable keybindings mirror in sync (best-effort).
        self.shortcuts.save_mirror();
        Self::refresh_global(self);
    }

    // ── Global snapshot ───────────────────────────────────────────────────────
    //
    // Runtime consumers outside the settings UI (agent controller, self-heal
    // trigger, context builder, session spawn) read settings through this
    // cached snapshot instead of hitting disk on every decision. `save()`
    // refreshes it, so a change in the settings pane is visible everywhere
    // on the next read.

    fn global_cell() -> &'static RwLock<Arc<AppSettings>> {
        static GLOBAL: OnceLock<RwLock<Arc<AppSettings>>> = OnceLock::new();
        GLOBAL.get_or_init(|| RwLock::new(Arc::new(AppSettings::load())))
    }

    /// Current settings snapshot (cheap: one RwLock read + Arc clone).
    pub fn global() -> Arc<AppSettings> {
        Self::global_cell()
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    /// Re-read the snapshot from disk and the keychain cache. Background
    /// keychain reads call this when they land, since a snapshot taken on
    /// the UI thread before then shows those keys as absent.
    pub(crate) fn reload_global() {
        Self::refresh_global(&Self::load());
    }

    fn refresh_global(settings: &AppSettings) {
        let snapshot = Arc::new(settings.clone());
        match Self::global_cell().write() {
            Ok(mut g) => *g = snapshot,
            Err(mut p) => **p.get_mut() = snapshot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentAutonomySettings, AgentBackend, AppSettings, CustomModel, McpServerEntry,
        ProviderKeys, AUTO_MODEL_ID,
    };

    #[test]
    fn legacy_agent_settings_keep_agentic_workspace_disabled() {
        let settings: AgentAutonomySettings = serde_json::from_str("{}").expect("settings");
        assert!(!settings.agentic_workspace_enabled);
    }

    #[test]
    fn app_settings_default_backend_is_mock() {
        let settings = AppSettings::default();
        assert_eq!(settings.agent_backend, AgentBackend::Mock);
        assert!(!settings.telemetry_enabled);
    }

    #[test]
    fn app_settings_round_trip_json() {
        let mut keys = ProviderKeys::default();
        keys.set("openai", "sk-test".to_string());
        let settings = AppSettings {
            agent_backend: AgentBackend::Cloud,
            selected_model: "gpt-4o".to_string(),
            selected_effort: super::AgentEffort::default(),
            provider_keys: keys,
            custom_models: Vec::new(),
            agent_model_favorites: Vec::new(),
            telemetry_enabled: true,
            crash_reporting_enabled: false,
            cloud_conversation_storage_enabled: true,
            allow_dragent_fallback: true,
            agent: super::AgentAutonomySettings {
                max_agent_steps: 50,
                proactive_error_scanning: true,
                ..Default::default()
            }
            .with_approval_mode(super::ApprovalMode::Auto),
            index: super::IndexSettings {
                respect_gitignore: false,
                ..Default::default()
            },
            device_id: None,
            skip_logout_confirm: false,
            notifications: Default::default(),
            shortcuts: Default::default(),
            dev_permissions: Default::default(),
            computer_use: Default::default(),
            voice: Default::default(),
            git: Default::default(),
            repo: Default::default(),
            general: Default::default(),
            // mcp_servers is #[serde(skip)] — not persisted in app_settings.json;
            // stored separately in ~/.dragent/mcp.json.
            mcp_servers: vec![],
            plugin: Default::default(),
            sharing: Default::default(),
            tasks: Default::default(),
            hosts: Default::default(),
        };
        let json = serde_json::to_string(&settings).unwrap();
        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_backend, AgentBackend::Cloud);
        assert!(restored.telemetry_enabled);
        // mcp_servers is not in the JSON; it's loaded from ~/.dragent/mcp.json at runtime.
        assert_eq!(restored.mcp_servers.len(), 0);
        assert_eq!(restored.provider_keys.get("openai"), Some("sk-test"));
        assert_eq!(restored.agent.approval_mode(), super::ApprovalMode::Auto);
        assert_eq!(restored.agent.max_agent_steps, 50);
        assert!(restored.agent.proactive_error_scanning);
        assert!(restored.agent.auto_fix_on_failure);
        assert!(!restored.index.respect_gitignore);
        assert!(restored.index.enabled);
        // Verify mcp_servers is absent from the serialized JSON.
        assert!(!json.contains("mcp_servers"));
    }

    #[test]
    fn autonomy_and_index_defaults_from_legacy_json() {
        // A pre-existing app_settings.json without the new sections must load
        // with safe defaults (Ask mode, indexing on, gitignore respected).
        let restored: AppSettings =
            serde_json::from_str(r#"{"agent_backend":"cloud","selected_model":"gpt-4o"}"#).unwrap();
        assert_eq!(restored.agent.approval_mode(), super::ApprovalMode::Ask);
        assert!(restored.agent.auto_run_safe_commands);
        assert_eq!(restored.agent.max_agent_steps, 200);
        assert!(!restored.agent.proactive_error_scanning);
        assert!(restored.index.enabled);
        assert!(restored.index.auto_index);
        assert_eq!(restored.index.embeddings, super::EmbeddingsBackend::Local);
    }

    #[test]
    fn plugin_sharing_and_voice_stay_on_from_legacy_json() {
        // Settings saved before these sections existed must keep MCP servers,
        // session sharing and dictation working as they did.
        let restored: AppSettings =
            serde_json::from_str(r#"{"agent_backend":"cloud","selected_model":"gpt-4o"}"#).unwrap();
        assert!(restored.plugin.system_enabled);
        assert!(restored.plugin.disabled_plugins.is_empty());
        assert!(restored.plugin.is_plugin_enabled("github"));
        assert!(restored.sharing.artifact_sharing_enabled);
        assert!(restored.voice.enabled);
    }

    #[test]
    fn servers_from_before_consents_are_approved() {
        let entry = |name: &str| McpServerEntry {
            name: name.into(),
            command: "npx".into(),
            transport: Default::default(),
            args: Vec::new(),
            env: Default::default(),
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: Vec::new(),
        };
        let servers = [entry("Expo"), entry("Github")];
        let none = || Vec::new();
        let mut legacy: AppSettings = serde_json::from_str("{}").unwrap();
        legacy.approve_servers_from_before_consents("{}", &servers, &none);
        assert!(legacy.plugin.has_consent("Expo") && legacy.plugin.has_consent("Github"));
        // Once the plugin section is saved, a new server still needs approval.
        let mut current = AppSettings::default();
        current.approve_servers_from_before_consents(r#"{"plugin":{}}"#, &servers, &none);
        assert!(!current.plugin.has_consent("Expo"));
        // A plugin installed it, not the person: never approved for them.
        let mut legacy: AppSettings = serde_json::from_str("{}").unwrap();
        legacy.approve_servers_from_before_consents("{}", &servers, &|| vec!["Github".to_string()]);
        assert!(legacy.plugin.has_consent("Expo") && !legacy.plugin.has_consent("Github"));
    }

    #[test]
    fn plugin_and_sharing_round_trip() {
        let mut settings = AppSettings::default();
        settings.plugin.system_enabled = true;
        settings.plugin.set_plugin_enabled("github", false);
        settings
            .plugin
            .set_consent("linear", crate::settings::plugin_settings::PLUGIN_CONSENT_VERSION);
        settings.plugin.add_dev_path("/tmp/dev-plugins");
        settings.sharing.artifact_sharing_enabled = true;
        let json = serde_json::to_string(&settings).unwrap();
        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(restored.plugin.system_enabled);
        assert!(!restored.plugin.is_plugin_enabled("github"));
        assert!(restored.plugin.is_plugin_enabled("linear"));
        assert!(restored.plugin.has_consent("linear"));
        assert_eq!(restored.plugin.dev_paths, vec!["/tmp/dev-plugins"]);
        assert!(restored.sharing.artifact_sharing_enabled);
    }

    #[test]
    fn effort_defaults_to_provider_default_and_survives_legacy_json() {
        // A settings file written before the effort control existed must load
        // as Default — i.e. the field is omitted on the wire and behaviour is
        // byte-for-byte what it was before this feature.
        let restored: AppSettings =
            serde_json::from_str(r#"{"agent_backend":"cloud","selected_model":"gpt-4o"}"#).unwrap();
        assert_eq!(restored.selected_effort, super::AgentEffort::Default);
        assert_eq!(restored.selected_effort.wire_value(), None);
    }

    #[test]
    fn effort_round_trips_through_json_as_snake_case() {
        let mut s = AppSettings::default();
        s.selected_effort = super::AgentEffort::High;
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""selected_effort":"high""#), "got: {json}");

        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.selected_effort, super::AgentEffort::High);
        // The wire value must match the server's providers.Effort vocabulary.
        assert_eq!(restored.selected_effort.wire_value(), Some("high"));
    }

    #[test]
    fn every_effort_level_has_a_distinct_wire_value() {
        let wire: Vec<_> = super::AgentEffort::ALL
            .iter()
            .filter_map(|e| e.wire_value())
            .collect();
        assert_eq!(wire, vec!["low", "medium", "high"]);
    }

    #[test]
    fn the_users_own_keys_never_go_to_the_server() {
        let mut s = AppSettings::default();
        s.provider_keys.set("anthropic", "sk-ant-user".to_string());
        s.selected_model = "claude-sonnet-4-6".to_string();
        s.custom_models.push(CustomModel {
            name: "Corp".into(),
            model_id: "corp-model".into(),
            base_url: "https://llm.corp.example/v1".into(),
            api_key: Some("corp-key".into()),
            endpoint_style: Default::default(),
        });
        for model in ["claude-sonnet-4-6", "corp-model", "gpt-5.2"] {
            let h = s.request_key_headers(model).unwrap();
            assert_eq!(
                h.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
                ["X-Fallback-Allowed"],
                "{model}"
            );
            assert!(!h
                .iter()
                .any(|(_, v)| v.contains("sk-ant-user") || v.contains("corp")));
        }

        // The toggle still says whether Dragent's keys may serve it.
        let h = s.request_key_headers("gpt-5.2").unwrap();
        assert_eq!(h[0].1, "true");
        s.allow_dragent_fallback = false;
        let h = s.request_key_headers("gpt-5.2").unwrap();
        assert_eq!(h[0].1, "false");
    }

    #[test]
    fn auto_uses_platform_billing_even_with_a_provider_key() {
        let mut s = AppSettings::default();
        s.provider_keys.set("openai", "sk-openai-user".to_string());

        for model in ["", AUTO_MODEL_ID] {
            let h = s.request_key_headers(model).unwrap();
            assert!(
                !h.iter().any(|(k, _)| *k == "X-User-Api-Key"),
                "Auto must use the platform-funded path so usage is metered"
            );
        }
    }

    #[test]
    fn max_steps_reads_legacy_max_agent_turns_key() {
        let restored: AppSettings =
            serde_json::from_str(r#"{"agent":{"max_agent_turns":25}}"#).unwrap();
        assert_eq!(restored.agent.max_agent_steps, 25);
    }

    #[test]
    fn mcp_server_entry_display_command() {
        let entry = McpServerEntry {
            name: "test".into(),
            command: "npx".into(),
            transport: super::McpTransport::Stdio,
            args: vec!["-y".into(), "some-pkg".into()],
            env: Default::default(),
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: Vec::new(),
        };
        assert_eq!(entry.display_command(), "npx -y some-pkg");
    }

    #[test]
    fn provider_id_for_model_static() {
        assert_eq!(
            AppSettings::provider_id_for_model_static("claude-sonnet-4-6"),
            Some("anthropic".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("gpt-4o"),
            Some("openai".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("o3"),
            Some("openai".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("gemini-2.5-pro"),
            Some("google".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("groq/llama-3.3-70b-versatile"),
            Some("groq".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("cerebras/llama3.1-70b"),
            Some("cerebras".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("openrouter/anthropic/claude-sonnet-4-5"),
            Some("openrouter".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("ollama/llama3.2"),
            Some("ollama".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("lmstudio/local-model"),
            Some("lmstudio".into())
        );
        // Llama names are never Meta's (retired preview): bare llama is
        // unclaimed here, explicit host prefixes still route to their host.
        assert_eq!(
            AppSettings::provider_id_for_model_static("Llama-4-Maverick"),
            None
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("llama-3.3-70b-versatile"),
            None
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("groq/llama-3.3-70b-versatile"),
            Some("groq".into())
        );
        // First-party Chinese-model families.
        assert_eq!(
            AppSettings::provider_id_for_model_static("deepseek-chat"),
            Some("deepseek".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("deepseek-reasoner"),
            Some("deepseek".into())
        );
        // Bare local-style names stay unresolved (Ollama territory).
        assert_eq!(
            AppSettings::provider_id_for_model_static("deepseek-r1:8b"),
            None
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("qwen-max"),
            Some("qwen".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("glm-4-plus"),
            Some("zhipu".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("kimi-k2"),
            Some("moonshot".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("MiniMax-Text-01"),
            Some("minimax".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("moonshot/kimi-k2"),
            Some("moonshot".into())
        );
        // Hosted-model providers: prefixes route, bare vendor namespaces
        // stay prefix-only.
        assert_eq!(
            AppSettings::provider_id_for_model_static("mistral/mistral-large-latest"),
            Some("mistral".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("mistral-large-latest"),
            Some("mistral".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("codestral-latest"),
            Some("mistral".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("sonar-pro"),
            Some("perplexity".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("command-a"),
            Some("cohere".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("together/deepseek-ai/DeepSeek-V3"),
            Some("together".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("deepinfra/meta-llama/Llama-4-Maverick"),
            Some("deepinfra".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("venice/llama-3.3-70b"),
            Some("venice".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("meta/muse-spark-1.3"),
            Some("meta".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("muse-spark-1.3"),
            Some("meta".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("Muse-Spark-1.1"),
            Some("meta".into())
        );
        // Llama names are NOT Meta's: bare llama belongs to third-party
        // hosts or nothing.
        assert_eq!(
            AppSettings::provider_id_for_model_static("Llama-4-Maverick"),
            None
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("llama-3.3-70b-versatile"),
            None
        );
        // Explicit routing prefixes still win.
        assert_eq!(
            AppSettings::provider_id_for_model_static("groq/llama-3.3-70b-versatile"),
            Some("groq".into())
        );
        assert_eq!(
            AppSettings::provider_id_for_model_static("openrouter/muse-spark-1.3"),
            Some("openrouter".into())
        );
    }

    #[test]
    #[cfg(not(target_family = "wasm"))]
    fn save_shape_carries_no_credential_material() {
        // The persisted shape must never contain key material on native:
        // keychain owns durability, the file is only settings.
        let mut s = AppSettings::default();
        s.provider_keys.set("anthropic", "sk-secret".into());
        s.custom_models.push(CustomModel {
            name: "c".into(),
            base_url: "https://x/v1".into(),
            model_id: "m".into(),
            api_key: Some("custom-secret".into()),
            endpoint_style: Default::default(),
        });
        let scrubbed = s.without_credentials();
        assert_eq!(scrubbed.provider_keys.get("anthropic"), None);
        assert!(scrubbed.custom_models[0].api_key.is_none());
        // Live maps stay intact for every reader.
        assert_eq!(s.provider_keys.get("anthropic"), Some("sk-secret"));
        let json = serde_json::to_string(&scrubbed).unwrap();
        assert!(!json.contains("sk-secret"), "leak: {json}");
        assert!(!json.contains("custom-secret"), "leak: {json}");
    }

    #[test]
    fn provider_keys_get_set_clear() {
        let mut keys = ProviderKeys::default();
        assert_eq!(keys.get("anthropic"), None);
        keys.set("anthropic", "sk-ant-abc".to_string());
        assert_eq!(keys.get("anthropic"), Some("sk-ant-abc"));
        keys.clear("anthropic");
        assert_eq!(keys.get("anthropic"), None);
    }

    #[test]
    fn provider_keys_meta_slot_round_trips() {
        let mut keys = ProviderKeys::default();
        assert_eq!(keys.get("meta"), None);
        keys.set("meta", "llama-key".to_string());
        assert_eq!(keys.get("meta"), Some("llama-key"));
        keys.clear("meta");
        assert_eq!(keys.get("meta"), None);
    }

    #[test]
    fn provider_keys_chinese_slots_round_trip() {
        let mut keys = ProviderKeys::default();
        for (slot, key) in [
            ("deepseek", "sk-ds"),
            ("qwen", "sk-qwen"),
            ("zhipu", "sk-zhipu"),
            ("moonshot", "sk-ms"),
            ("minimax", "sk-mm"),
            ("mistral", "sk-mi"),
            ("together", "sk-to"),
            ("deepinfra", "sk-di"),
            ("perplexity", "sk-px"),
            ("venice", "sk-ve"),
            ("nvidia", "sk-nv"),
            ("fireworks", "sk-fw"),
            ("cohere", "sk-co"),
        ] {
            assert_eq!(keys.get(slot), None);
            keys.set(slot, key.to_string());
            assert_eq!(keys.get(slot), Some(key));
            keys.clear(slot);
            assert_eq!(keys.get(slot), None);
        }
        assert!(keys.get("unknown-slot").is_none());
    }

    #[test]
    fn custom_model_endpoint_style_defaults_to_chat() {
        // Records written before the style field existed must keep working.
        let legacy: CustomModel =
            serde_json::from_str(r#"{"name":"m","base_url":"https://x/v1","model_id":"m1"}"#)
                .expect("legacy record");
        assert_eq!(
            legacy.endpoint_style,
            dragent_agent::direct::CustomEndpointStyle::ChatCompletions
        );
        let modern: CustomModel = serde_json::from_str(
            r#"{"name":"m","base_url":"https://x/v1","model_id":"m1","endpoint_style":"responses"}"#,
        )
        .expect("modern record");
        assert_eq!(
            modern.endpoint_style,
            dragent_agent::direct::CustomEndpointStyle::Responses
        );
    }
}
