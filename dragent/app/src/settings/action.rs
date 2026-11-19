use super::app_settings::{AgentBackend, ApprovalMode, ContextScope, EmbeddingsBackend};
use super::git_settings::{BranchPrefix, DiffView, JiraCasing, ScGroupOrder, ScViewMode};
use super::voice_settings::VoiceBackend;
use crate::console::editor_settings::{CursorStyle, MacOptionAsAlt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Account,
    General,
    Appearance,
    TerminalInput,
    VersionControl,
    KeyboardShortcuts,
    AgentGeneral,
    AgentModels,
    AgentMcp,
    AgentSkills,
    AgentCommands,
    AgentAccounts,
    Usage,
    Editor,
    CodeIndex,
    Teams,
    Projects,
    Members,
    Billing,
    Integrations,
    Privacy,
    Notifications,
    Voice,
    DeveloperPermissions,
    ComputerUse,
    RemoteAccess,
    Mobile,
    Machines,
    About,
}

impl SettingsSection {
    pub fn is_agent(self) -> bool {
        matches!(
            self,
            Self::AgentGeneral
                | Self::AgentModels
                | Self::AgentMcp
                | Self::AgentSkills
                | Self::AgentCommands
                | Self::AgentAccounts
        )
    }

    /// Pages backed by `OrgStore` (live organisation data).
    pub fn is_org_page(self) -> bool {
        matches!(self, Self::Members | Self::Teams | Self::Billing)
    }
}

#[derive(Clone, Debug)]
pub enum SettingsAction {
    AgentAccount(super::agent_accounts::AccountAction),
    Usage(super::usage::UsageAction),
    Project(super::ProjectSettingsAction),
    Machine(super::machines::MachineAction),
    Mobile(super::mobile::MobileAction),
    Integration(super::integrations::IntegrationAction),
    TaskHost(super::settings_tasks_hosts::TaskHostAction),
    Notification(super::pages::notifications::NotificationAction),
    Close,
    ClearSettingsSearch,
    ClearCommandAgentSearch,
    /// Empty the search in the settings agent menus.
    ClearAgentMenuSearch,
    SelectSection(SettingsSection),
    SelectCommandAgent(crate::workspace::PromptAgent),
    SaveAgentCommand,
    ResetAgentCommand,
    SetAgentPromptStdin(bool),
    ToggleAgentNav,
    ToggleUmbrella(&'static str),
    SelectTheme(String),
    ToggleThemeSyncOs,
    ImportTheme,
    ThemeFilePicked {
        path: String,
    },
    OpenThemeChooser,
    CloseThemeChooser,
    ToggleVimMode,
    ToggleInputAutodetect,
    SelectModel(String),
    /// Set the reasoning depth sent with each agent query.
    SelectEffort(crate::settings::AgentEffort),
    TogglePrimaryPicker,
    /// Dispatched automatically by Settings-mode InputEditors on every keystroke.
    /// `key` is a modal field key ("modal_name", "modal_base_url", etc.) or "search_query".
    UpdateProviderKey(String, String),
    ClearProviderKey(String),
    AddCustomModel,
    EditCustomModel(usize),
    RemoveCustomModel(usize),
    CloseCustomModelModal,
    ConfirmCustomModal,
    ToggleDragentFallback,
    /// Opt in/out of spending purchased credits once the plan allowance or a
    /// usage window is exhausted. Server-side preference, not a local setting.
    ToggleCreditUsage,
    /// Toggle the indexed-folders accordion open/closed.
    ToggleIndexList,
    /// Jump the indexed-folders list to a page (0-based).
    IndexListPage(usize),
    RemoveMcpServer(usize),
    /// Open the add/edit modal. None = new server, Some(i) = edit existing.
    ShowMcpForm(Option<usize>),
    HideMcpForm,
    /// Fired on every keystroke in an MCP form editor (field_key, value).
    UpdateMcpDraftField(String, String),
    SaveMcpServer,
    ToggleMcpEnabled(usize),
    /// Show or hide a server's tools, prompts and resources (by server name).
    ToggleMcpCapabilities(String),
    /// In the server form: add or drop one agent the server is also written into.
    ToggleMcpDraftAgent(String),
    /// In the server form: write the server into every agent found on this machine.
    SelectAllMcpDraftAgents,
    /// Start the browser-based OAuth flow for the SSE server at index.
    TriggerMcpOAuth(usize),
    /// Disconnect (clear the stored OAuth token) for the server at index.
    DisconnectMcpOAuth(usize),
    ImportClaudeDesktop,
    /// A plugin's server entries changed (`dragent plugins`): add or replace
    /// `upsert`, remove `remove` with their tokens and approvals, withdraw the
    /// approval of `revoke`.
    ApplyPluginServers {
        upsert: Vec<crate::settings::McpServerEntry>,
        remove: Vec<String>,
        revoke: Vec<String>,
    },
    RefreshAccount,
    SignIn,
    SignOut,
    /// Confirm signing out from the "Log out?" modal.
    ConfirmSignOut,
    /// Dismiss the "Log out?" modal without signing out.
    CancelSignOut,
    /// Toggle the modal's "Don't ask again" checkbox.
    ToggleLogoutDontAsk,
    OpenPrivacyPolicy,
    OpenDocumentation,
    ResetOnboarding,
    CheckForUpdates,
    CopyVersion,

    // ── Agent → Skills ───────────────────────────────────────────────────────
    ShowSkillCreator,
    HideSkillCreator,
    SaveSkill,
    ToggleSkillScope,
    ToggleSkillModelInvocation,
    OpenSkill(std::path::PathBuf),
    DeleteSkill(std::path::PathBuf),
    ConfirmDeleteSkill,
    CancelDeleteSkill,
    ToggleSkillDetails(std::path::PathBuf),
    CloseSkillMenu,
    ToggleSkillGroup(String),

    // ── Plugins + sharing ─────────────────────────────────────
    /// Master switch for all non-built-in plugin tools.
    TogglePluginSystem,
    /// Enable/disable one plugin id (MCP server names are plugin ids).
    TogglePluginEnabled(String),
    /// Persist first-run approval of a plugin at a consent version.
    SetPluginConsent { plugin: String, version: String },
    /// Gate for artifact (session) sharing UI + backend.
    ToggleArtifactSharing,
    // ── Generic dropdown state ────────────────────────────────────────────────
    /// Open/close the inline dropdown identified by `key` (one open at a time).
    TogglePicker(String),

    // ── Agent → General (autonomy & self-healing) ─────────────────────────────
    SelectApprovalMode(ApprovalMode),
    /// Minutes before an idle agent is put to sleep; 0 = never.
    SelectIdleSleepMinutes(u32),
    ToggleAgenticWorkspace,
    ToggleAutoRunSafeCommands,
    ToggleProactiveErrorScanning,
    ToggleAutoFixOnFailure,
    SelectContextScope(ContextScope),

    // ── Agent → General (Agents settings) ────────────────────────
    /// `None` = Dragent's chooser decides (Claude-first).
    SelectDefaultAgent(Option<String>),
    /// Add/remove one agent command key from the disabled list.
    ToggleDisabledAgent(String),
    ToggleWorkspaceTrust,
    /// General → Confirmations: ask before closing an idle tab.
    ToggleConfirmCloseIdle,
    ToggleServerIsolation,

    // ── Voice ─────────────────────────────────────────────────
    ToggleVoice,
    SelectVoiceBackend(VoiceBackend),
    SelectLanguage(String),
    ToggleConfirmBeforeInsert,

    // ── Shortcuts ─────────────────────────────────────────────
    /// Begin capturing a new chord for `action_id` (keyboard page row).
    StartRebind(String),
    CancelRebind,
    /// Commit `chord` for `action_id` (validated; invalid stays in edit mode).
    SetBinding { action: String, chord: String },
    ResetBinding(String),
    ResetAllBindings,
    SelectShortcutPolicy(super::shortcut_settings::TerminalShortcutPolicy),
    SelectCtrlTabOrder(super::shortcut_settings::CtrlTabOrder),

    // ── Developer permissions ─────────────────────────────────
    ToggleKeepAwake,
    SelectAwakeMode(super::shortcut_settings::AwakeMode),
    /// Re-query OS permission state (accessibility / full-disk-access).
    RecheckDeveloperPermissions,

    // ── Computer use ──────────────────────────────────────────
    ToggleComputerUse,
    #[cfg(not(target_family = "wasm"))]
    RefreshComputerPermissions,
    #[cfg(not(target_family = "wasm"))]
    OpenComputerPermission(dragent_computer::PermissionKind),
    #[cfg(not(target_family = "wasm"))]
    RequestComputerPermissionReset,
    #[cfg(not(target_family = "wasm"))]
    ConfirmComputerPermissionReset,
    #[cfg(not(target_family = "wasm"))]
    CancelComputerPermissionReset,
    #[cfg(not(target_family = "wasm"))]
    ToggleComputerSkill,
    ToggleComputerUseConfirm,
    /// Add the app name in the allowlist editor.
    AllowComputerApp(String),
    RemoveComputerApp(String),

    // ── Terminal & input ──────────────────────────────────────────────────────
    /// `None` = system default shell.
    SelectDefaultShell(Option<String>),
    /// Add the `dragent` command to the user's shell profile.
    InstallCliInShell,
    /// Copy Dragent's skill into other agents' skill folders, or remove it.
    ToggleAgentSkill,
    /// `None` = platform default (Menlo on macOS, Fira Code elsewhere).
    SelectTerminalFont(Option<String>),
    SelectInterfaceFont(Option<String>),
    SetTerminalFontSize(f32),
    /// Percent of font size (120 = 1.2×).
    SetTerminalLineHeight(u32),
    SelectCursorStyle(CursorStyle),
    ToggleCursorBlink,
    /// Editor caret + terminal grid blink interval in milliseconds.
    SetCursorBlinkRate(u32),
    /// Keybinding mode for the command input (segmented Default / Vim).
    SetVimMode(bool),

    // ── Source editor ─────────────────────────────────────────────────────────
    SelectEditorFontWeight(warpui::fonts::Weight),
    /// Tab stop width in spaces (1–8).
    SetTabSize(u32),
    ToggleInsertSpaces,
    ToggleLineNumbers,
    ToggleShowWhitespace,
    ToggleIndentGuides,
    // ── Terminal input ──────────────────────────────────────
    SelectMacOptionAsAlt(MacOptionAsAlt),
    ToggleJisYen,
    ToggleClipboardOnSelect,
    ToggleCopyTrimsGutter,
    /// `None` = default word boundaries; `Some(chars)` = extra break chars.
    SetWordSeparator(Option<String>),
    /// Toggle middle-click paste (None = platform default).
    ToggleMiddleClickPaste,
    // ── Editor behavior ─────────────────────────────────────
    ToggleAutoSave,
    /// Debounced autosave delay in ms (clamped 100–10000).
    SetAutoSaveDelay(u32),
    ToggleMinimap,
    /// `None` = follow the terminal code font.
    SelectEditorFontFamily(Option<String>),
    ToggleWordWrap,
    ToggleSpellcheck,
    ToggleMarkdownReview,
    TogglePreviewTabs,

    // ── Code and Index ────────────────────────────────────────────────────────
    ToggleIndexingEnabled,
    ToggleAutoIndex,
    ToggleRespectGitignore,
    SelectEmbeddingsBackend(EmbeddingsBackend),
    /// Force a full rebuild of the repo index at the given root.
    ReindexRepo(std::path::PathBuf),
    /// Delete a repo index (removes store + snapshot, stops the watcher).
    DeleteIndex(std::path::PathBuf),
    /// Persist the ignore-patterns editor to the global index.ignore file and
    /// re-index so the new rules apply.
    SaveIgnorePatterns,

    // ── Organisation (members, teams, billing & usage) ─────────────────────────
    OrgRefresh,
    OrgDismissMessage,
    /// Open/close the organisation switcher list.
    OrgToggleSwitcher,
    /// Make the organisation with this id the active one.
    OrgSwitch(String),
    /// Show/hide the "new organisation" field in the switcher.
    OrgToggleCreate,
    OrgCreate,
    OrgLeave,
    /// Role for the next invitation ("member" | "admin").
    OrgSetInviteRole(String),
    OrgInvite,
    OrgCancelInvitation(String),
    OrgSetMemberRole {
        user_id: String,
        role: String,
    },
    OrgRemoveMember(String),
    /// Accept or decline an invitation addressed to the signed-in user.
    OrgRespondInvitation {
        id: String,
        accept: bool,
    },
    TeamCreate,
    TeamDelete(String),
    /// Plans shown: annual (true) or monthly.
    BillingShowAnnual(bool),
    /// Start checkout for a subscription plan (organisation owners).
    BillingSubscribe(String),
    /// Open the organisation's billing portal (invoices, payment method).
    BillingOpenPortal,
    /// Start checkout for a one-time credit pack.
    BuyCredits(String),

    // ── Git / Version Control ─────────────────────────────────
    ToggleAutoRenameBranch,
    SelectBranchPrefix(BranchPrefix),
    SelectJiraCasing(JiraCasing),
    SelectDiffView(DiffView),
    ToggleDiffWordWrap,
    ToggleDiffShowWhitespace,
    ToggleDiffCollapseUnchanged,
    SelectScViewMode(ScViewMode),
    SelectScGroupOrder(ScGroupOrder),
    ToggleScCompareUpstream,
    ToggleFollowSymlinks,
    ToggleShowIgnored,
    ToggleCommitAi,
    SelectCommitAiAgent(Option<String>),
    ClearCommitAiAgent,
    AddPrBotOverride,
    RemovePrBotOverride(usize),
    AddOpenInApp,
    RemoveOpenInApp(usize),
    OpenInApp { app: String, path: String },

    // ── General / workspace / confirmations / setup-guide ─────
    SetWorkspaceDir(Option<String>),
    ClearWorkspaceDir,
    ToggleNestWorkspaces,
    ToggleRefreshBaseRef,
    ToggleConfirmCloseTerminal,
    ToggleConfirmDeleteWorktree,
    ToggleOpenLinksInApp,
    ToggleRestoreSessions,
    CompleteSetupGuide,
    ResetSetupGuide,
    DismissSetupGuideItem(String),
    UndismissSetupGuideItem(String),

    // ── Per-project repo overrides (persisted in RepoSettingsStore) ─────────
    SelectRepoProject(std::path::PathBuf),
    ClearRepoOverride(std::path::PathBuf),
    SetRepoBaseRef { project: std::path::PathBuf, value: Option<String> },
    SetRepoBasePath { project: std::path::PathBuf, value: Option<String> },
    SetRepoHookMode { project: std::path::PathBuf, value: Option<String> },
    AddRepoHookScript { project: std::path::PathBuf },
    RemoveRepoHookScript { project: std::path::PathBuf, index: usize },
    SetRepoIssueSource { project: std::path::PathBuf, value: String },
    SetRepoGhAccount { project: std::path::PathBuf, value: Option<String> },
    SetRepoForkSync { project: std::path::PathBuf, value: Option<String> },
    SetRepoIdentity { project: std::path::PathBuf, value: Option<String> },
    SetRepoVisibility { project: std::path::PathBuf, value: Option<String> },
    AddRepoSymlink { project: std::path::PathBuf },
    RemoveRepoSymlink { project: std::path::PathBuf, index: usize },
    SetRepoGroupId { project: std::path::PathBuf, value: Option<String> },
    SetRepoOrder { project: std::path::PathBuf, value: i32 },
    SetRepoHostSetup { project: std::path::PathBuf, value: Option<String> },
    SetRepoWindowsRuntime { project: std::path::PathBuf, value: Option<String> },
}
