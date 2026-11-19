mod account_store;
mod action;
pub(crate) mod agent_accounts;
pub(crate) mod usage;
mod app_settings;
mod config;
pub mod credentials;
pub mod git_consumers;
mod git_settings;
pub mod integrations;
pub(crate) mod logout_confirm;
pub(crate) mod machines;
pub(crate) mod machines_store;
pub mod mcp_config;
mod mobile;
mod nav;
mod org_store;
pub mod plugin_settings;
mod projects;
pub(crate) mod settings_tasks_hosts;
pub mod task_settings;
pub(crate) mod tool_catalog;
pub use projects::ProjectSettingsAction;
pub(crate) use projects::ProjectSettingsEntry;
pub mod pages;
mod settings_account;
mod settings_agent;
mod settings_git;
mod settings_index;
mod settings_mcp_actions;
mod settings_misc;
mod settings_models;
mod settings_nav;
mod settings_org;
mod settings_plugins;
mod settings_shortcuts;
mod settings_skills;
mod settings_skills_actions;
mod settings_theme;
mod settings_view;
mod settings_voice;
pub mod shortcut_settings;
pub mod telemetry_outbox;
pub mod voice_settings;
mod widgets;
mod widgets_controls;
mod widgets_mcp;
mod widgets_nav;
mod widgets_panels;
mod widgets_rows;
mod widgets_shortcuts;

pub use account_store::{
    AccountData, AccountStore, AccountStoreEvent, ModelInfo as AccountModelInfo,
};
pub use action::{SettingsAction, SettingsSection};
pub use app_settings::{
    AgentAutonomySettings, AgentBackend, AgentEffort, AgentModelFavorite, AppSettings,
    ApprovalMode, ContextScope, CustomModel, EmbeddingsBackend, IndexSettings, McpServerEntry,
    McpTransport, ProviderKeys, AUTO_MODEL_ID, FREE_TIER_MODEL_ID,
};
pub use git_settings::{
    BranchPrefix, DiffView, GeneralSettings, GitSettings, JiraCasing, RepoSettings,
    RepoSettingsStore, ScGroupOrder, ScViewMode,
};
pub use org_store::{OrgStore, OrgStoreEvent};
pub use plugin_settings::{
    DevPluginManifest, PluginSettings, SharingSettings, PLUGIN_CONSENT_VERSION,
};
pub use settings_view::{SettingsEvent, SettingsView};
pub use shortcut_settings::{
    AwakeMode, ComputerUseRequest, ComputerUseSettings, CtrlTabOrder, DevPermissionsSettings,
    PermissionReport, PermissionState, ShortcutSettings, TerminalShortcutPolicy,
};
pub use voice_settings::{DictationMode, VoiceSettings};
