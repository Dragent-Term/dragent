//! Settings pane inside a workspace tab — sidebar nav + page content.

#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, Element, Flex, MainAxisAlignment, MainAxisSize, ParentElement, Point,
    ScrollStateHandle, ScrollbarWidth, Shrinkable, Stack, Text, UniformListState,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::FamilyId;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Entity, EntityId, EventContext, GetSingletonModelHandle,
    LayoutContext, PaintContext, ReadModel, SingletonEntity, SizeConstraint, TypedActionView,
    UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::account_store::{AccountStore, AccountStoreEvent};
use crate::auth::{AuthManager, AuthManagerEvent};
use crate::console::editor_settings::EditorSettings;
use crate::setup::{reset_for_onboarding, UiSettings};
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputMode};

use super::action::{SettingsAction, SettingsSection};
use super::app_settings::{AppSettings, CustomModel, McpServerEntry, McpTransport};
use super::nav;
use super::pages;
use super::widgets::{primary_button, secondary_button, CONTENT_MAX_WIDTH, CONTENT_PADDING};

pub enum SettingsEvent {
    AgentAccountLogin(uuid::Uuid),
    AgentAccountSignIn {
        machine_id: Option<String>,
        title: String,
        command: String,
    },
    AgentAccountError(String),
    Project(super::ProjectSettingsAction),
    /// A machine was added, edited, renamed, removed or (dis)connected.
    MachinesChanged,
    /// Open the Add Project dialog with this machine as the location.
    AddRemoteProject(String),
    OpenRemoteProject(String),
    RemoveRemoteProject(String),
    AttachRemoteSession {
        machine_id: String,
        session: dragent_host::TerminalSession,
    },
    /// Install, update or remove tools in the install sheet, in a terminal
    /// on the machine.
    RunToolJob {
        machine_id: String,
        job: crate::settings::tool_catalog::ToolJob,
    },
    ThemeChanged(String),
    EditorSettingsChanged,
    AppSettingsChanged,
    Close,
    OpenSkill(std::path::PathBuf),
}

/// Live draft state while the MCP add/edit form is open.
pub(crate) struct McpServerDraft {
    pub name_editor: ViewHandle<InputEditor>,
    pub command_editor: ViewHandle<InputEditor>,
    pub args_editor: ViewHandle<InputEditor>,
    pub env_editor: ViewHandle<InputEditor>,
    pub bearer_token_editor: ViewHandle<InputEditor>,
    // Mirror values kept in sync by UpdateMcpDraftField actions.
    pub name: String,
    pub command: String,
    pub args_raw: String, // one arg per line
    pub env_raw: String,  // KEY=VALUE lines
    pub bearer_token: String,
    pub transport: McpTransport,
    /// None = creating a new server; Some(i) = editing server at index i.
    pub editing_index: Option<usize>,
    /// Other agents to also write this server into (agent keys).
    pub sync_agents: Vec<String>,
}

/// Live draft state while the custom model add/edit modal is open.
pub(crate) struct CustomModelDraft {
    pub name_editor: ViewHandle<InputEditor>,
    pub base_url_editor: ViewHandle<InputEditor>,
    pub model_id_editor: ViewHandle<InputEditor>,
    pub api_key_editor: ViewHandle<InputEditor>,
    // Mirror values kept in sync by UpdateProviderKey actions from the editors.
    pub name: String,
    pub base_url: String,
    pub model_id: String,
    pub api_key: String,
    /// Wire protocol for the endpoint. No picker in the modal yet (UI
    /// follow-up) — preserved across edits, settable via config, defaulting
    /// to chat completions for new entries.
    pub endpoint_style: dragent_agent::direct::CustomEndpointStyle,
    /// None = creating a new model; Some(i) = editing model at index i.
    pub editing_index: Option<usize>,
}

/// Text fields on the Organisation pages, created on first visit.
pub(crate) struct OrgEditors {
    pub invite_email: ViewHandle<InputEditor>,
    pub team_name: ViewHandle<InputEditor>,
    pub new_org_name: ViewHandle<InputEditor>,
}

/// View-local state of the Organisation pages (server data lives in `OrgStore`).
pub(crate) struct OrgPageState {
    pub editors: Option<OrgEditors>,
    pub switcher_open: bool,
    pub create_open: bool,
    /// Plan list shows annual prices (the dashboard's default) or monthly.
    pub show_annual: bool,
    pub invite_role: String,
    pub invite_email: String,
    pub team_name: String,
    pub new_org_name: String,
    /// Destructive action awaiting its second click (e.g. "remove:<user>").
    pub confirm: Option<String>,
    /// Remaining 5 s refreshes after a checkout opened in the browser.
    pub purchase_watch: u32,
    /// Credit balance and plan when that checkout opened.
    pub purchase_baseline: Option<(i64, Option<String>)>,
}

impl Default for OrgPageState {
    fn default() -> Self {
        Self {
            editors: None,
            switcher_open: false,
            create_open: false,
            show_annual: true,
            invite_role: "member".to_string(),
            invite_email: String::new(),
            team_name: String::new(),
            new_org_name: String::new(),
            confirm: None,
            purchase_watch: 0,
            purchase_baseline: None,
        }
    }
}

pub(crate) struct SkillDraft {
    pub name_editor: ViewHandle<InputEditor>,
    pub description_editor: ViewHandle<InputEditor>,
    pub body_editor: ViewHandle<InputEditor>,
    pub project_scope: bool,
    pub disable_model_invocation: bool,
}

pub struct SettingsView {
    pub(crate) agent_accounts: super::agent_accounts::AccountsState,
    pub(crate) usage: super::usage::UsageState,
    pub(crate) projects: Vec<super::ProjectSettingsEntry>,
    pub(crate) projects_org: String,
    pub(crate) projects_search: String,
    pub(crate) projects_search_editor: ViewHandle<InputEditor>,
    pub(crate) machines: super::machines::MachinesState,
    pub(crate) mobile: super::mobile::MobileState,
    pub(crate) integrations: super::integrations::IntegrationsState,
    pub(crate) task_hosts: super::settings_tasks_hosts::TaskHostState,
    pub(crate) view_id: EntityId,
    pub(crate) project_root: std::path::PathBuf,
    pub(crate) ui_font: FamilyId,
    pub(crate) section: SettingsSection,
    /// Agent selected in Settings → Agent → Commands.
    pub(crate) command_agent: crate::workspace::PromptAgent,
    pub(crate) agent_command_editor: ViewHandle<InputEditor>,
    pub(crate) agent_command_value: String,
    pub(crate) agent_base_editor: ViewHandle<InputEditor>,
    pub(crate) agent_suffix_editor: ViewHandle<InputEditor>,
    pub(crate) agent_configs: HashMap<String, crate::setup::settings::AgentCommandConfig>,
    pub(crate) agent_command_status: Option<String>,
    pub(crate) agent_nav_open: bool,
    /// Umbrella names currently collapsed in the settings nav tree.
    pub(crate) collapsed_umbrellas: Vec<&'static str>,
    pub(crate) theme_id: String,
    pub(crate) editor_settings: EditorSettings,
    pub(crate) app_settings: AppSettings,
    pub(crate) hover_states: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) auth_status_message: Option<String>,
    pub(crate) status_message: Option<String>,
    pub(crate) theme_status_message: Option<String>,
    pub(crate) theme_chooser_open: bool,
    pub(crate) theme_sync_os: bool,
    pub(crate) mcp_status_message: Option<String>,
    /// Servers whose tools, prompts and resources are expanded.
    pub(crate) mcp_expanded: std::collections::HashSet<String>,
    /// Actual pixel width of the content pane (window width minus sidebar).
    /// Captured via ViewportWidthCapture each layout pass and used to make
    /// cards responsive — content is capped at CONTENT_MAX_WIDTH but shrinks
    /// to fit when the pane is narrower.
    pub(crate) content_pane_width: f32,
    content_width_tx: async_channel::Sender<f32>,
    /// Viewport height captured each layout pass (see `ViewportHeightCapture`).
    /// The settings row lives under an unbounded-height ancestor, so internal
    /// scrollables cannot rely on parent constraints — they cap against this.
    /// 0.0 until the first layout pass (callers fall back to 700).
    content_pane_height: f32,
    content_height_tx: async_channel::Sender<f32>,
    /// Provider IDs for which the server has its own key configured.
    /// Hardcoded until GET /api/models is wired in.
    pub(crate) server_providers: Vec<String>,
    /// When Some, the "Add custom model" modal is open.
    pub(crate) custom_model_modal: Option<CustomModelDraft>,
    pub(crate) skill_draft: Option<SkillDraft>,
    pub(crate) skill_groups_expanded: std::collections::HashSet<String>,
    pub(crate) expanded_skill: Option<std::path::PathBuf>,
    pub(crate) skill_detail_scroll: ClippedScrollStateHandle,
    pub(crate) pending_skill_delete: Option<std::path::PathBuf>,
    /// When Some, the MCP add/edit form is open.
    pub(crate) mcp_form: Option<McpServerDraft>,
    /// Whether the "Log out?" confirmation modal is open.
    pub(crate) logout_confirm_open: bool,
    /// The modal's "Don't ask again" checkbox state.
    pub(crate) logout_dont_ask: bool,
    pub(crate) content_scroll: ClippedScrollStateHandle,
    /// Independent scroll state for the settings navigation tree.
    pub(crate) nav_scroll: ClippedScrollStateHandle,
    /// Independent scroll state for the Agent Commands sidebar.
    pub(crate) command_agents_scroll: ClippedScrollStateHandle,
    pub(crate) command_agent_search: String,
    pub(crate) command_agent_search_editor: ViewHandle<InputEditor>,
    pub(crate) search_query: String,
    pub(crate) search_editor: ViewHandle<InputEditor>,
    pub(crate) mcp_search_query: String,
    pub(crate) mcp_search_editor: ViewHandle<InputEditor>,
    /// Filter for the skill inventory.
    pub(crate) skill_search_query: String,
    pub(crate) skill_search_editor: ViewHandle<InputEditor>,
    pub(crate) shortcut_search_query: String,
    pub(crate) shortcut_search_editor: ViewHandle<InputEditor>,
    /// Theme picker search (filters the curated theme catalog).
    pub(crate) theme_search_query: String,
    pub(crate) theme_search_editor: ViewHandle<InputEditor>,
    /// Scroll state for the theme LIST alone. The list scrolls inside its own
    /// bounded box so the search field above it stays put instead of scrolling
    /// away with the page — you need the field visible while browsing results.
    pub(crate) theme_list_scroll: ScrollStateHandle,
    pub(crate) font_picker_scroll: ClippedScrollStateHandle,
    pub(crate) ui_font_picker_scroll: ClippedScrollStateHandle,
    /// The Appearance catalog is intentionally viewported. A full theme card
    /// contains a terminal mockup and may decode an image, so constructing all
    /// cards whenever Settings repaints causes a visible pause.
    pub(crate) theme_list_state: UniformListState,
    /// Indexed-folders accordion: collapsed to a count by default; expands to a
    /// searchable, paginated card list.
    pub(crate) index_list_expanded: bool,
    pub(crate) index_search_query: String,
    pub(crate) index_search_editor: ViewHandle<InputEditor>,
    pub(crate) index_list_page: usize,
    pub(crate) primary_picker_open: bool,
    /// One masked InputEditor per provider ID, keyed by provider ID string.
    pub(crate) provider_key_editors: HashMap<String, ViewHandle<InputEditor>>,
    /// Keychain writes that failed, by keychain account, shown on the
    /// matching provider or custom-model row until a later write succeeds.
    pub(crate) keychain_errors: HashMap<String, String>,
    /// Background MCP reconnect threads post the server name here when done.
    /// The subscription handler calls ctx.notify() so the UI refreshes.
    pub(crate) mcp_reconnect_tx: async_channel::Sender<String>,
    /// Which inline dropdown (select widget) is currently open, by key.
    pub(crate) open_picker: Option<String>,
    /// Why adding the `dragent` command to the shell profile failed.
    pub(crate) cli_setup_error: Option<String>,
    /// What the last change to the agent skill setting did.
    pub(crate) agent_skill_note: Option<String>,
    /// Persistent drag state per slider, keyed like hover states.
    pub(crate) slider_states:
        RefCell<HashMap<String, warpui_core::ui_components::slider::SliderStateHandle>>,
    /// Multiline editor for the global index ignore patterns (Code and Index
    /// page). Created on first visit, pre-filled from index.ignore.
    pub(crate) ignore_patterns_editor: Option<ViewHandle<InputEditor>>,
    /// Status line shown on the Code and Index page after re-index / save.
    pub(crate) index_status_message: Option<String>,
    /// True while the 2 s repaint timer for the live index status card is armed.
    index_poll_armed: bool,
    /// Last index status fetched for the Code indexing page (`None` until
    /// the first answer arrives).
    pub(crate) index_statuses: Option<Vec<dragent_index::RepoIndexStatus>>,
    /// True while the 1 s repaint timer for the live session countdown is armed.
    account_poll_armed: bool,
    os_poll_armed: bool,
    /// Organisation pages: switcher, forms, and purchase follow-up.
    pub(crate) org: OrgPageState,
    /// Mono font families installed on this machine. They are discovered once,
    /// lazily, when the Typography controls are opened.
    pub(crate) available_mono_fonts: Vec<String>,
    /// Every installed family, for the interface font.
    pub(crate) available_ui_fonts: Vec<String>,
    font_discovery_started: bool,
    // ── Git / General / Repo ────────────────────────────────
    pub(crate) git_prefix_custom_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) workspace_dir_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) commit_ai_agent_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) pr_bot_draft: String,
    pub(crate) pr_bot_draft_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) open_in_draft: String,
    pub(crate) open_in_draft_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) selected_repo_project: Option<std::path::PathBuf>,
    pub(crate) repo_editors: std::collections::HashMap<String, warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) repo_hook_draft: String,
    pub(crate) repo_hook_draft_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    pub(crate) repo_symlink_draft: String,
    pub(crate) repo_symlink_draft_editor: Option<warpui::ViewHandle<crate::workspace::InputEditor>>,
    // ── Shortcuts / dev-permissions / computer-use ──────────
    /// Action id currently being rebound on the Keyboard page (`None` = idle).
    pub(crate) shortcut_rebind_target: Option<String>,
    /// Live text of the chord editor for the row being rebound.
    pub(crate) shortcut_binding_draft: String,
    pub(crate) shortcut_binding_editor: ViewHandle<InputEditor>,
    /// Last rebinding outcome (error stays visible so the row stays editable).
    pub(crate) shortcut_status_message: Option<String>,
    /// Draft app name typed into the Computer-use allowlist adder.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_permissions: Option<dragent_computer::Permissions>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_permission_note: Option<String>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_permissions_loading: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_permission_generation: u64,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_permission_polling: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_permission_visible: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) computer_reset_confirm: bool,
    pub(crate) computer_app_draft: String,
    pub(crate) computer_app_editor: ViewHandle<InputEditor>,
    /// Last developer-permissions note (e.g. recheck result / wake failure).
    pub(crate) dev_perm_note: Option<String>,
    /// Search in the settings agent menus (Behavior's default agent, Agent
    /// Accounts' agent); one is open at a time.
    pub(crate) agent_menu_search: String,
    pub(crate) agent_menu_search_editor: ViewHandle<InputEditor>,
    pub(crate) agent_menu_scroll: ClippedScrollStateHandle,
}

/// Where Agent → Behavior's default-agent menu opens from.
pub(crate) const DEFAULT_AGENT_CHIP_TAG: &str = "settings_default_agent_chip";
/// Where Agent Accounts' agent menu opens from.
pub(crate) const ACCOUNTS_AGENT_CHIP_TAG: &str = "settings_accounts_agent_chip";

impl SettingsView {
    pub fn new(
        ctx: &mut ViewContext<Self>,
        _workspace_view_id: EntityId,
        ui_font: FamilyId,
        project_root: std::path::PathBuf,
    ) -> Self {
        let ui = UiSettings::load();
        let view_id = ctx.view_id();

        let auth = AuthManager::handle(ctx);
        ctx.subscribe_to_model(&auth, |view, _, event, ctx| {
            match event {
                AuthManagerEvent::AuthComplete => {
                    view.auth_status_message = Some("Signed in".to_string());
                    let account = AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.fetch(mctx));
                    ctx.notify();
                }
                AuthManagerEvent::AuthFailed { message } => {
                    view.auth_status_message = Some(message.clone());
                    ctx.notify();
                }
                AuthManagerEvent::LoggedOut | AuthManagerEvent::SessionExpired => {
                    view.auth_status_message =
                        Some(if matches!(event, AuthManagerEvent::SessionExpired) {
                            // The user didn't ask to be signed out — say why.
                            "Your session expired. Please sign in again.".to_string()
                        } else {
                            "Signed out".to_string()
                        });
                    // Drop the cached account data immediately — the page must
                    // not keep showing the signed-out user's name and usage.
                    let account = AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.clear(mctx));
                    let org_store = crate::settings::OrgStore::handle(ctx);
                    ctx.update_model(&org_store, |store, mctx| store.clear(mctx));
                    view.org = OrgPageState::default();
                    ctx.notify();
                }
                AuthManagerEvent::SessionRestored => {
                    let account = AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.fetch(mctx));
                    ctx.notify();
                }
            }
        });

        // Organisation pages: re-render on data, refetch account usage when the
        // active organisation changes (credits resolve against it), and open
        // checkout/portal links in the browser.
        let org_store = crate::settings::OrgStore::handle(ctx);
        ctx.subscribe_to_model(&org_store, |view, _, event, ctx| {
            match event {
                crate::settings::OrgStoreEvent::Updated => {}
                crate::settings::OrgStoreEvent::ActiveChanged => {
                    let account = AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.fetch(mctx));
                }
                crate::settings::OrgStoreEvent::OpenUrl(url) => {
                    ctx.open_url(url);
                    view.arm_purchase_watch(ctx);
                }
            }
            ctx.notify();
        });

        // Machines: connection and runtime state changes repaint the page.
        let machines = <super::machines_store::MachinesStore as warpui::SingletonEntity>::handle(ctx);
        ctx.subscribe_to_model(&machines, |view, _, _, ctx| {
            if view.section == SettingsSection::Machines {
                ctx.notify();
            }
        });

        // Re-render the account page when account data arrives.
        let account = AccountStore::handle(ctx);
        ctx.subscribe_to_model(&account, |_, _, event, ctx| match event {
            AccountStoreEvent::DataUpdated | AccountStoreEvent::FetchFailed(_) => {
                ctx.notify();
            }
        });

        // If the session was already restored before this view was opened (the
        // common case — the session restores at startup, settings open later),
        // kick off an immediate account fetch so the account page shows real data.
        let already_logged_in = ctx.read_model(&auth, |a, _| a.is_logged_in());
        if already_logged_in {
            let account = AccountStore::handle(ctx);
            ctx.update_model(&account, |store, mctx| store.fetch(mctx));
        }

        // Keys still being read from the Keychain load as absent here; take
        // them up when they land. Subscribed before the load so none is missed.
        ctx.spawn_stream_local(
            super::credentials::subscribe_keychain_reads(),
            |view, (), ctx| view.adopt_landed_keys(ctx),
            |_, _| {},
        );
        let app_settings = AppSettings::load();
        let theme = Theme::from_id(&ui.theme_id);
        let initial_config = crate::workspace::agent_command_config(
            crate::workspace::PromptAgent::Claude, &ui,
        );
        let initial_agent_command = initial_config.prompt_command.clone();
        let agent_configs = crate::workspace::PromptAgent::ALL.into_iter()
            .map(|agent| (agent.command_key().to_string(), crate::workspace::agent_command_config(agent, &ui)))
            .collect();

        // Optional system fonts are discovered lazily when Terminal & Input is
        // opened. Workspace construction must not enumerate or load fonts.
        let available_mono_fonts = Vec::new();

        // Channel used by background MCP reconnect threads to notify the UI.
        let (mcp_reconnect_tx, mcp_reconnect_rx) = async_channel::unbounded::<String>();
        // Register this sender globally so deep-link OAuth callbacks can also trigger
        // a settings refresh (no settings tab needs to be the initiator).
        crate::agent::mcp::register_reconnect_notifier(mcp_reconnect_tx.clone());
        // Default section is Account — start the live session-countdown ticker.
        Self::spawn_account_poll(ctx);
        ctx.spawn_stream_local(
            mcp_reconnect_rx,
            |view, name, ctx| {
                let count = crate::agent::mcp::tool_count_for(&name);
                view.mcp_status_message = Some(if count > 0 {
                    format!("'{}' connected — {} tool(s) available.", name, count)
                } else {
                    format!("'{}' reconnected (0 tools — check credentials).", name)
                });
                // Navigate to the MCP page so the user sees the result immediately.
                view.section = crate::settings::action::SettingsSection::AgentMcp;
                ctx.notify();
            },
            |_, _| {},
        );

        let (content_width_tx, content_width_rx) = async_channel::unbounded::<f32>();
        let (content_height_tx, content_height_rx) = async_channel::unbounded::<f32>();
        ctx.spawn_stream_local(
            content_width_rx,
            |view, width: f32, ctx| {
                if (view.content_pane_width - width).abs() > 1.0 {
                    view.content_pane_width = width;
                    ctx.notify();
                }
            },
            |_, _| {},
        );
        ctx.spawn_stream_local(
            content_height_rx,
            |view, height: f32, ctx| {
                if (view.content_pane_height - height).abs() > 1.0 {
                    view.content_pane_height = height;
                    ctx.notify();
                }
            },
            |_, _| {},
        );

        let search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search settings…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("search_query".to_string()),
                inner.focus_handle(),
            )
        });

        let mcp_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search MCP Servers…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("mcp_search_query".to_string()),
                inner.focus_handle(),
            )
        });

        let skill_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search skills by name or description…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("skill_search_query".to_string()),
                inner.focus_handle(),
            )
        });

        let shortcut_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search shortcuts…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("shortcut_search_query".to_string()),
                inner.focus_handle(),
            )
        });

        let shortcut_binding_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "cmdorctrl-t…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("shortcut_binding_draft".to_string()),
                inner.focus_handle(),
            )
        });

        let computer_app_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "App name, e.g. Terminal…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("computer_app_draft".to_string()),
                inner.focus_handle(),
            )
        });

        let agent_menu_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search agents",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("agent_menu_search".to_string()),
                inner.focus_handle(),
            )
        });

        let index_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search indexed folders…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("index_search_query".to_string()),
                inner.focus_handle(),
            )
        });

        let theme_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search themes…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("theme_search_query".to_string()),
                inner.focus_handle(),
            )
        });

        let command_agent_search_editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new(
                "Search agents…",
                ui_font,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Settings("command_agent_search".to_string()),
                inner.focus_handle(),
            )
        });

        let agent_command_editor = {
            let initial = initial_agent_command.clone();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "claude --dangerously-skip-permissions",
                    ui_font,
                    crate::theme::text_size(12.5),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings("agent_command".to_string()),
                    inner.focus_handle(),
                    false,
                    &initial,
                )
            })
        };

        let mut command_input = |key: &str, initial: &str| {
            let key = key.to_string();
            let initial = initial.to_string();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "", ui_font, crate::theme::text_size(12.5), theme.main_text(),
                    theme.disabled_text(), theme.accent, theme,
                    InputMode::Settings(key), inner.focus_handle(), false, &initial,
                )
            })
        };
        let agent_base_editor = command_input("agent_base_command", &initial_config.command);
        let agent_suffix_editor = command_input("agent_prompt_suffix", &initial_config.prompt_suffix);

        // Pre-create one masked editor per supported provider.
        let provider_entries: &[(&str, &str)] = &[
            ("anthropic", "sk-ant-…"),
            ("openai", "sk-…"),
            ("google", "AIza…"),
            ("xai", "xai-…"),
            ("groq", "gsk_…"),
            ("cerebras", "csk-…"),
            ("openrouter", "sk-or-…"),
        ];
        let mut provider_key_editors: HashMap<String, ViewHandle<InputEditor>> = HashMap::new();
        for &(pid, ph) in provider_entries {
            let mode_key = pid.to_string();
            let init = app_settings
                .provider_keys
                .get(pid)
                .unwrap_or("")
                .to_string();
            let editor = ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    ph,
                    ui_font,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings(mode_key),
                    inner.focus_handle(),
                    true,
                    &init,
                )
            });
            provider_key_editors.insert(pid.to_string(), editor);
        }

        let projects_search_editor = ctx.add_typed_action_view(|editor_ctx| InputEditor::new(
            "Search projects", ui_font, crate::theme::scaled(13.0),
            theme.main_text(), theme.disabled_text(), theme.accent, theme,
            InputMode::Settings("projects_search".into()), editor_ctx.focus_handle(),
        ));
        let agent_accounts = super::agent_accounts::AccountsState::new(ctx, ui_font, theme);
        let machines = super::machines::MachinesState::new(ctx, ui_font, theme);
        let integrations = super::integrations::IntegrationsState::new(ctx, ui_font, theme);
        let task_hosts = super::settings_tasks_hosts::TaskHostState::new(ctx, ui_font, theme);
        Self {
            agent_accounts,
            usage: Default::default(),
            projects: Vec::new(),
            projects_org: dragent_paths::organisation_id(),
            projects_search: String::new(),
            projects_search_editor,
            machines,
            mobile: super::mobile::MobileState::default(),
            integrations,
            task_hosts,
            view_id,
            project_root,
            ui_font,
            section: SettingsSection::Account,
            command_agent: crate::workspace::PromptAgent::Claude,
            agent_command_editor,
            agent_command_value: initial_agent_command,
            agent_base_editor,
            agent_suffix_editor,
            agent_configs,
            agent_command_status: None,
            agent_nav_open: true,
            collapsed_umbrellas: Vec::new(),
            theme_id: ui.theme_id,
            editor_settings: EditorSettings::load(),
            app_settings,
            hover_states: RefCell::new(HashMap::new()),
            auth_status_message: None,
            status_message: None,
            theme_status_message: None,
            theme_chooser_open: false,
            theme_sync_os: crate::setup::UiSettings::load().sync_os,
            mcp_status_message: None,
            mcp_expanded: std::collections::HashSet::new(),
            server_providers: vec!["anthropic".into(), "openai".into()],
            custom_model_modal: None,
            skill_draft: None,
            expanded_skill: None,
            skill_groups_expanded: Default::default(),
            skill_detail_scroll: ClippedScrollStateHandle::default(),
            pending_skill_delete: None,
            logout_confirm_open: false,
            logout_dont_ask: false,
            mcp_form: None,
            content_scroll: ClippedScrollStateHandle::new(),
            nav_scroll: ClippedScrollStateHandle::new(),
            command_agents_scroll: ClippedScrollStateHandle::new(),
            command_agent_search: String::new(),
            command_agent_search_editor,
            search_query: String::new(),
            search_editor,
            mcp_search_query: String::new(),
            mcp_search_editor,
            skill_search_query: String::new(),
            skill_search_editor,
            shortcut_search_query: String::new(),
            shortcut_search_editor,
            shortcut_rebind_target: None,
            shortcut_binding_draft: String::new(),
            shortcut_binding_editor,
            shortcut_status_message: None,
            #[cfg(not(target_family = "wasm"))]
            computer_permissions: None,
            #[cfg(not(target_family = "wasm"))]
            computer_permission_note: None,
            #[cfg(not(target_family = "wasm"))]
            computer_permissions_loading: false,
            #[cfg(not(target_family = "wasm"))]
            computer_permission_generation: 0,
            #[cfg(not(target_family = "wasm"))]
            computer_permission_polling: false,
            #[cfg(not(target_family = "wasm"))]
            computer_permission_visible: false,
            #[cfg(not(target_family = "wasm"))]
            computer_reset_confirm: false,
            computer_app_draft: String::new(),
            computer_app_editor,
            dev_perm_note: None,
            agent_menu_search: String::new(),
            agent_menu_search_editor,
            agent_menu_scroll: ClippedScrollStateHandle::new(),
            theme_search_query: String::new(),
            theme_search_editor,
            theme_list_scroll: ScrollStateHandle::default(),
            font_picker_scroll: ClippedScrollStateHandle::new(),
            ui_font_picker_scroll: ClippedScrollStateHandle::new(),
            theme_list_state: UniformListState::default(),
            index_list_expanded: false,
            index_search_query: String::new(),
            index_search_editor,
            index_list_page: 0,
            content_pane_width: 0.0,
            content_pane_height: 0.0,
            content_width_tx,
            content_height_tx,
            primary_picker_open: false,
            provider_key_editors,
            keychain_errors: HashMap::new(),
            mcp_reconnect_tx,
            open_picker: None,
            cli_setup_error: None,
            agent_skill_note: None,
            slider_states: RefCell::new(HashMap::new()),
            ignore_patterns_editor: None,
            index_status_message: None,
            index_poll_armed: false,
            index_statuses: None,
            account_poll_armed: true,
            os_poll_armed: false,
            available_mono_fonts,
            available_ui_fonts: Vec::new(),
            font_discovery_started: false,
            org: OrgPageState::default(),
            git_prefix_custom_editor: None,
            workspace_dir_editor: None,
            commit_ai_agent_editor: None,
            pr_bot_draft: String::new(),
            pr_bot_draft_editor: None,
            open_in_draft: String::new(),
            open_in_draft_editor: None,
            selected_repo_project: None,
            repo_editors: Default::default(),
            repo_hook_draft: String::new(),
            repo_hook_draft_editor: None,
            repo_symlink_draft: String::new(),
            repo_symlink_draft_editor: None,
        }
    }

    pub(crate) fn hover(&self, key: &str) -> MouseStateHandle {
        self.hover_states
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Live-apply an app-wide typography change: new UI family for this
    /// view's chrome and re-font its input editors so the settings page
    /// itself follows the Typography controls.
    pub fn apply_typography(&mut self, ui_font: FamilyId, ctx: &mut ViewContext<Self>) {
        self.ui_font = ui_font;
        self.sync_machine_inputs(ctx);
        let theme = self.theme();
        self.task_hosts.sync_inputs(theme, ui_font, ctx);
        let scaled = crate::theme::scaled(12.5);
        for editor in [
            &self.search_editor,
            &self.mcp_search_editor,
            &self.skill_search_editor,
            &self.shortcut_search_editor,
            &self.shortcut_binding_editor,
            &self.computer_app_editor,
            &self.index_search_editor,
            &self.theme_search_editor,
            &self.command_agent_search_editor,
            &self.projects_search_editor,
        ]
        .into_iter()
        .chain(self.ignore_patterns_editor.iter())
        .chain(self.git_prefix_custom_editor.iter())
        .chain(self.workspace_dir_editor.iter())
        .chain(self.commit_ai_agent_editor.iter())
        .chain(self.pr_bot_draft_editor.iter())
        .chain(self.open_in_draft_editor.iter())
        .chain(self.repo_hook_draft_editor.iter())
        .chain(self.repo_symlink_draft_editor.iter())
        .chain(self.repo_editors.values())
        {
            ctx.update_view(editor, |e, ectx| {
                e.set_font(ui_font, scaled, ectx);
                e.set_menu_font(ui_font, ectx);
            });
        }
        let key_size = crate::theme::scaled(13.0);
        for editor in self.provider_key_editors.values() {
            ctx.update_view(editor, |e, ectx| {
                e.set_font(ui_font, key_size, ectx);
                e.set_menu_font(ui_font, ectx);
            });
        }
        let mut modal_editors = Vec::new();
        if let Some(draft) = &self.mcp_form {
            modal_editors.extend([
                &draft.name_editor,
                &draft.command_editor,
                &draft.args_editor,
                &draft.env_editor,
                &draft.bearer_token_editor,
            ]);
        }
        if let Some(draft) = &self.custom_model_modal {
            modal_editors.extend([
                &draft.name_editor,
                &draft.base_url_editor,
                &draft.model_id_editor,
                &draft.api_key_editor,
            ]);
        }
        if let Some(draft) = &self.skill_draft {
            modal_editors.extend([
                &draft.name_editor,
                &draft.description_editor,
                &draft.body_editor,
            ]);
        }
        for editor in modal_editors {
            ctx.update_view(editor, |e, ectx| {
                let size = e.font_size();
                e.set_font(ui_font, size, ectx);
                e.set_menu_font(ui_font, ectx);
            });
        }
        ctx.notify();
    }

    pub(crate) fn picker_open(&self, key: &str) -> bool {
        self.open_picker.as_deref() == Some(key)
    }

    pub(crate) fn slider(
        &self,
        key: &str,
    ) -> warpui_core::ui_components::slider::SliderStateHandle {
        self.slider_states
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Lazily create the multiline ignore-patterns editor, pre-filled from the
    /// global index.ignore file, when the Code and Index page is first opened.
    pub(crate) fn ensure_ignore_editor(&mut self, ctx: &mut ViewContext<Self>) {
        if self.ignore_patterns_editor.is_some() {
            return;
        }
        let ui = self.ui_font;
        let theme = self.theme();
        let initial = dragent_index::walk::global_ignore_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let editor = ctx.add_typed_action_view(move |inner| {
            InputEditor::new_secure(
                "node_modules/\ndist/\n*.log",
                ui,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "index_ignore_patterns".to_string(),
                    max_height: 160,
                },
                inner.focus_handle(),
                false,
                &initial,
            )
        });
        self.ignore_patterns_editor = Some(editor);
    }

    pub(crate) fn ensure_font_discovery(&mut self, ctx: &mut ViewContext<Self>) {
        if self.font_discovery_started {
            return;
        }
        self.font_discovery_started = true;

        #[cfg(not(target_family = "wasm"))]
        {
            let font_cache = ctx.get_singleton_model_handle::<warpui_core::fonts::Cache>();
            // Show Menlo immediately on macOS. Full enumeration can take
            // longer, and some CoreText TTC faces lack a monospace flag.
            #[cfg(target_os = "macos")]
            if ctx.update_model(&font_cache, |cache, _| {
                cache.load_system_font("Menlo").is_ok()
            }) {
                self.available_mono_fonts.push("Menlo".to_string());
                ctx.notify();
            }
            let discovery = ctx.update_model(&font_cache, |cache, model_ctx| {
                cache.all_system_fonts(model_ctx)
            });
            ctx.spawn(discovery, |view, fonts, ctx| {
                let mut all_families = fonts
                    .iter()
                    .map(|(_, info)| info.family_name.clone())
                    .collect::<Vec<_>>();
                all_families.sort_by_key(|family| family.to_lowercase());
                all_families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
                view.available_ui_fonts = all_families;
                let mut families = fonts
                    .into_iter()
                    .filter_map(|(_, info)| info.is_monospace.then_some(info.family_name))
                    .filter(|family| {
                        !family.eq_ignore_ascii_case("Fira Code")
                            && !family.eq_ignore_ascii_case("JetBrains Mono")
                    })
                    .collect::<Vec<_>>();
                families.extend(view.available_mono_fonts.iter().cloned());
                families.sort_by_key(|family| family.to_lowercase());
                families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
                view.available_mono_fonts = families;
                log::info!(
                    "[fonts] installed monospace families: {:?}",
                    view.available_mono_fonts
                );
                ctx.notify();
            });
        }
    }

    /// Keep the Code indexing page's status current while it is visible
    /// ("Indexing… / synced Ns ago"): fetch now, then every 2 s.
    pub(crate) fn arm_index_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if self.index_poll_armed {
            return;
        }
        self.index_poll_armed = true;
        Self::spawn_index_poll(ctx);
    }

    /// Fetch the index status, then again every 2 s while the page shows.
    /// The page draws whatever arrived last; asking the index service while
    /// drawing froze the app whenever the service was slow or down.
    fn spawn_index_poll(ctx: &mut ViewContext<SettingsView>) {
        ctx.spawn(
            Self::fetch_index_statuses(),
            |view: &mut SettingsView, statuses, ctx| {
                if let Some(statuses) = statuses {
                    view.index_statuses = Some(statuses);
                }
                ctx.notify();
                if view.section != SettingsSection::CodeIndex {
                    view.index_poll_armed = false;
                    return;
                }
                ctx.spawn(
                    warpui::r#async::Timer::after(std::time::Duration::from_secs(2)),
                    |_: &mut SettingsView, _, ctx| Self::spawn_index_poll(ctx),
                );
            },
        );
    }

    /// The service is asked on its own thread: futures spawned here are
    /// polled on the main thread.
    fn fetch_index_statuses(
    ) -> impl std::future::Future<Output = Option<Vec<dragent_index::RepoIndexStatus>>> {
        #[cfg(not(target_family = "wasm"))]
        {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(dragent_index::get_manager().index_status());
            });
            async move { rx.recv().await.ok() }
        }
        #[cfg(target_family = "wasm")]
        {
            let statuses = dragent_index::get_manager().index_status();
            async move { Some(statuses) }
        }
    }

    /// Called when the settings panel is (re)opened. If the Account page will
    /// be shown, fetch fresh usage immediately so the user never sees stale
    /// numbers on open — no manual Refresh needed.
    pub fn on_opened(&mut self, ctx: &mut ViewContext<Self>) {
        if self.section == SettingsSection::Projects {
            ctx.emit(SettingsEvent::Project(super::ProjectSettingsAction::Refresh));
        }
        if self.section == SettingsSection::Account {
            let account = AccountStore::handle(ctx);
            ctx.update_model(&account, |store, mctx| store.fetch(mctx));
            self.arm_account_poll(ctx);
            ctx.notify();
        }
        if self.section.is_org_page() {
            self.enter_org_page(ctx);
            ctx.notify();
        }
        // Resume OS-theme following if the user left "Sync with OS" on:
        // apply immediately in case the OS changed while away, then poll.
        if crate::setup::UiSettings::load().sync_os {
            Self::apply_os_theme(self, ctx);
            self.arm_os_theme_poll(ctx);
        }
    }

    pub(crate) fn arm_account_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if self.account_poll_armed {
            return;
        }
        self.account_poll_armed = true;
        Self::spawn_account_poll(ctx);
    }

    fn spawn_account_poll(ctx: &mut ViewContext<SettingsView>) {
        // Usage numbers update reactively via the AccountStore subscription the
        // moment a task charges (no polling). This timer only does a cheap
        // LOCAL repaint every 30 s — NO network/DB call — so that if the user
        // is idle on this page when the 5-hour window's clock time passes, the
        // session label flips to "no active session" without a manual refresh.
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(30)),
            |view: &mut SettingsView, _, ctx| {
                if view.section == SettingsSection::Account {
                    ctx.notify();
                    Self::spawn_account_poll(ctx);
                } else {
                    view.account_poll_armed = false;
                }
            },
        );
    }

    pub(crate) fn arm_os_theme_poll(&mut self, ctx: &mut ViewContext<SettingsView>) {
        if self.os_poll_armed {
            return;
        }
        self.os_poll_armed = true;
        Self::spawn_os_theme_poll(ctx);
    }

    pub(crate) fn spawn_os_theme_poll(ctx: &mut ViewContext<SettingsView>) {
        // Follows the OS light/dark mode while "Sync with OS" is on (Warp
        // pattern). Cheap local check every 5 s; applies the matching theme
        // only on drift, then re-arms. Manual theme picks turn sync off, so
        // the poll never fights the user.
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(5)),
            |view: &mut SettingsView, _, ctx| {
                if crate::setup::UiSettings::load().sync_os {
                    Self::spawn_os_theme_poll(ctx);
                } else {
                    view.os_poll_armed = false;
                    return;
                }
                Self::apply_os_theme(view, ctx);
            },
        );
    }

    /// Apply the OS-matched theme now if it drifted. Returns true when a
    /// theme change was emitted. Shared by the poll timer and settings-open.
    pub(crate) fn apply_os_theme(
        view: &mut SettingsView,
        ctx: &mut ViewContext<SettingsView>,
    ) -> bool {
        let ui = crate::setup::UiSettings::load();
        let Some(expected) = ui.os_matched_id(matches!(
            ctx.system_theme(),
            warpui_core::platform::SystemTheme::Dark
        )) else {
            return false;
        };
        if view.theme_id != expected {
            view.theme_id = expected.clone();
            view.sync_machine_inputs(ctx);
            let mut ui2 = crate::setup::UiSettings::load();
            ui2.theme_id = expected.clone();
            ui2.save();
            ctx.emit(crate::settings::SettingsEvent::ThemeChanged(expected));
            return true;
        }
        false
    }

    pub(crate) fn theme(&self) -> Theme {
        Theme::from_id(&self.theme_id)
    }

    /// Captured settings-row height for capping internal scrollables (nav
    /// tree, icon rail, theme gallery, content pane). Falls back to 700
    /// before the first layout pass reports back.
    pub(crate) fn pane_height(&self) -> f32 {
        if self.content_pane_height > 0.0 {
            self.content_pane_height
        } else {
            700.0
        }
    }

    fn render_content(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme();
        match self.section {
            SettingsSection::Account => pages::render_account(self, &theme, app),
            SettingsSection::General => pages::render_general(self, &theme),
            SettingsSection::Appearance => pages::render_appearance(self, &theme),
            SettingsSection::TerminalInput => pages::render_terminal_input(self, &theme),
            SettingsSection::VersionControl => pages::render_version_control(self, &theme),
            SettingsSection::KeyboardShortcuts => pages::render_keyboard_shortcuts(self, &theme),
            SettingsSection::AgentGeneral => pages::render_agent_general(self, &theme, app),
            SettingsSection::AgentModels => pages::render_agent_models(self, &theme, app),
            SettingsSection::AgentMcp => pages::render_agent_mcp(self, &theme),
            SettingsSection::AgentSkills => pages::render_skills(self, &theme),
            SettingsSection::AgentAccounts => super::agent_accounts::render(self, &theme, app),
            SettingsSection::Usage => super::usage::render(self, &theme),
            SettingsSection::AgentCommands => pages::render_agent_commands(self, &theme),
            SettingsSection::Editor => pages::render_editor(self, &theme),
            SettingsSection::CodeIndex => pages::render_code_index(self, &theme),
            SettingsSection::Teams => pages::render_teams(self, &theme, app),
            SettingsSection::Projects => pages::render_projects(self, &theme),
            SettingsSection::Members => pages::render_members(self, &theme, app),
            SettingsSection::Billing => pages::render_billing(self, &theme, app),
            SettingsSection::Integrations => pages::render_integrations(self, &theme),
            SettingsSection::Privacy => pages::render_privacy(self, &theme, app),
            SettingsSection::Notifications => pages::render_notifications(self, &theme),
            SettingsSection::Voice => pages::render_voice(self, &theme),
            SettingsSection::DeveloperPermissions => {
                pages::render_developer_permissions(self, &theme)
            }
            SettingsSection::ComputerUse => pages::render_computer_use(self, &theme),
            SettingsSection::RemoteAccess => pages::render_remote_access(self, &theme),
            SettingsSection::Mobile => pages::render_mobile(self, &theme, app),
            SettingsSection::Machines => self.render_machines(&theme, app),
            SettingsSection::About => pages::render_about(self, &theme),
        }
    }
}

impl Entity for SettingsView {
    type Event = SettingsEvent;
}

impl View for SettingsView {
    fn ui_name() -> &'static str {
        "SettingsView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme();

        // Theme chooser open → collapse the sidebar to its icon rail;
        // the chooser docks on the right.
        let sidebar = if self.theme_chooser_open {
            ConstrainedBox::new(
                Container::new(nav::render_nav_rail(self, &theme))
                    .with_background_color(theme.sidebar_background())
                    .with_border(Border::right(1.0).with_border_color(theme.border()))
                    .with_padding_top(12.0)
                    .finish(),
            )
            .with_width(super::widgets::NAV_RAIL_WIDTH)
            .finish()
        } else {
            ConstrainedBox::new(
                Container::new(nav::render_nav(self, &theme))
                    .with_background_color(theme.sidebar_background())
                    .with_border(Border::right(1.0).with_border_color(theme.border()))
                    .finish(),
            )
            .with_width(super::widgets::NAV_WIDTH)
            .finish()
        };

        let content = if self.section == SettingsSection::AgentCommands {
            // Commands owns a sidebar scroll surface; the form never scrolls.
            ConstrainedBox::new(pages::render_agent_commands(self, &theme))
                .with_height(self.pane_height())
                .finish()
        } else {
            let scrollable = ClippedScrollable::vertical(
                self.content_scroll.clone(),
                ResponsivePaddingContainer::new(
                    self.render_content(app), ColorU::new(0, 0, 0, 0), self.content_pane_width,
                ).finish(),
                ScrollbarWidth::Auto, theme.disabled_text().into(),
                theme.sub_text().into(), theme.background.into(),
            ).finish();
            ConstrainedBox::new(scrollable).with_max_height(self.pane_height()).finish()
        };

        // ViewportWidthCapture sits outside the scroll — it sees the real bounded
        // pane constraint from the workspace layout and reports it back to the view
        // so ResponsivePaddingContainer can compute correct centering/responsive padding.
        let captured_content = Box::new(ViewportWidthCapture {
            child: content,
            tx: self.content_width_tx.clone(),
        });

        let mut base = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(sidebar)
            .with_child(Shrinkable::new(1.0, captured_content).finish());
        if self.theme_chooser_open {
            base = base.with_child(pages::render_theme_chooser(self, &theme));
        }
        let base = Box::new(ViewportHeightCapture {
            child: base.finish(),
            tx: self.content_height_tx.clone(),
        });

        let agent_menu = if self.section == SettingsSection::AgentGeneral && self.picker_open("default_agent") {
            Some((pages::render_default_agent_menu(self, &theme), DEFAULT_AGENT_CHIP_TAG))
        } else if self.section == SettingsSection::AgentAccounts && self.picker_open("accounts_agent") {
            Some((super::agent_accounts::render_agent_menu(self, &theme), ACCOUNTS_AGENT_CHIP_TAG))
        } else {
            None
        };
        let base: Box<dyn Element> = if let Some((menu, tag)) = agent_menu {
            Stack::new()
                .with_child(base)
                .with_positioned_overlay_child(
                    menu,
                    warpui::elements::OffsetPositioning::offset_from_save_position_element(
                        tag,
                        warpui::geometry::vector::vec2f(0.0, 6.0),
                        warpui::elements::PositionedElementOffsetBounds::WindowByPosition,
                        warpui::elements::PositionedElementAnchor::BottomLeft,
                        warpui::elements::ChildAnchor::TopLeft,
                    ),
                )
                .finish()
        } else {
            base
        };
        let base = if self.section == SettingsSection::AgentSkills {
            if let Some(menu) = pages::render_skill_menu(self, &theme) {
                Stack::new().with_child(base).with_positioned_overlay_child(menu,
                    warpui::elements::OffsetPositioning::offset_from_save_position_element(
                        pages::SKILL_MENU_TAG, warpui::geometry::vector::vec2f(0.0,6.0),
                        warpui::elements::PositionedElementOffsetBounds::WindowByPosition,
                        warpui::elements::PositionedElementAnchor::BottomRight,
                        warpui::elements::ChildAnchor::TopRight)).finish()
            } else { base }
        } else { base };
        if let Some(modal) = (self.section == SettingsSection::Machines)
            .then(|| self.render_machine_modal(&theme, app))
            .flatten()
        {
            Stack::new().with_child(base).with_child(modal).finish()
        } else if let Some(modal) = (self.section == SettingsSection::Integrations)
            .then(|| pages::render_integration_dialog(self, &theme))
            .flatten()
        {
            Stack::new().with_child(base).with_child(modal).finish()
        } else if let Some(modal) = (self.section == SettingsSection::Mobile)
            .then(|| pages::render_mobile_overlay(self, &theme))
            .flatten()
        {
            Stack::new().with_child(base).with_child(modal).finish()
        } else if self.pending_skill_delete.is_some() {
            let modal = self.render_skill_delete_confirm(&theme);
            Stack::new().with_child(base).with_child(modal).finish()
        } else if self.logout_confirm_open {
            let modal = self.render_logout_confirm_modal(&theme, app);
            Stack::new().with_child(base).with_child(modal).finish()
        } else if let Some(draft) = &self.skill_draft {
            let modal = self.render_skill_modal(&theme, draft);
            Stack::new().with_child(base).with_child(modal).finish()
        } else if let Some(draft) = &self.custom_model_modal {
            let modal = self.render_custom_model_modal(&theme, draft);
            Stack::new().with_child(base).with_child(modal).finish()
        } else if let Some(draft) = &self.mcp_form {
            let modal = self.render_mcp_form_modal(&theme, draft);
            Stack::new().with_child(base).with_child(modal).finish()
        } else {
            base
        }
    }
}

impl TypedActionView for SettingsView {
    type Action = SettingsAction;

    fn handle_action(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        if let SettingsAction::AgentAccount(action) = action { self.handle_agent_account(action, ctx); return; }
        if let SettingsAction::Usage(action) = action { self.handle_usage(action, ctx); return; }
        if let SettingsAction::Project(action) = action {
            ctx.emit(SettingsEvent::Project(action.clone()));
            return;
        }
        if let SettingsAction::UpdateProviderKey(key, value) = action {
            if key == "projects_search" {
                self.projects_search = value.clone();
                ctx.notify();
                return;
            }
            if self.handle_git_text(action, ctx) {
                return;
            }
        }
        if let SettingsAction::Machine(action) = action { self.handle_machine_action(action, ctx); return; }
        if let SettingsAction::Mobile(action) = action { self.handle_mobile_action(action, ctx); return; }
        if let SettingsAction::Integration(action) = action { self.handle_integration_action(action, ctx); return; }
        if let SettingsAction::Notification(action) = action { self.handle_notification_action(action, ctx); return; }
        if let SettingsAction::TaskHost(action) = action { self.handle_task_host_action(action, ctx); return; }
        self.handle_nav(action, ctx);
        self.handle_theme(action, ctx);
        self.handle_voice(action, ctx);
        self.handle_models(action, ctx);
        self.handle_skills_actions(action, ctx);
        self.handle_plugin_actions(action, ctx);
        self.handle_mcp_actions(action, ctx);
        self.handle_account(action, ctx);
        self.handle_org(action, ctx);
        self.handle_agent(action, ctx);
        self.handle_index(action, ctx);
        self.handle_git(action, ctx);
        self.handle_shortcuts(action, ctx);
        self.handle_misc(action, ctx);
        ctx.notify();
    }
}

impl SettingsView {
    pub(crate) fn import_claude_desktop(&mut self, ctx: &mut ViewContext<Self>) {
        let config_path =
            dirs::config_dir().map(|d| d.join("claude").join("claude_desktop_config.json"));
        let Some(path) = config_path else {
            self.mcp_status_message = Some("Could not locate config directory.".to_string());
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            self.mcp_status_message =
                Some("No Claude Desktop config found. Install Claude Desktop first.".to_string());
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            self.mcp_status_message = Some("Failed to parse Claude Desktop config.".to_string());
            return;
        };

        let mut added = 0usize;
        let mut skipped = 0usize;
        if let Some(servers) = json.get("mcpServers").and_then(|v| v.as_object()) {
            for (name, cfg) in servers {
                if self
                    .app_settings
                    .mcp_servers
                    .iter()
                    .any(|s| &s.name == name)
                {
                    skipped += 1;
                    continue;
                }
                let command = cfg
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args: Vec<String> = cfg
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let env: std::collections::HashMap<String, String> = cfg
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();

                if command.is_empty() {
                    skipped += 1;
                    continue;
                }

                self.app_settings.mcp_servers.push(McpServerEntry {
                    name: name.clone(),
                    command,
                    transport: McpTransport::Stdio,
                    args,
                    env,
                    enabled: true,
                    bearer_token: None,
                    auth_url: None,
                    token_url: None,
                    oauth_scope: None,
                    sync_agents: Vec::new(),
                });
                added += 1;
            }
        }

        if added > 0 {
            self.persist_mcp_servers(ctx);
            ctx.emit(SettingsEvent::AppSettingsChanged);
        }
        self.mcp_status_message = Some(match (added, skipped) {
            (0, 0) => "No MCP servers found in Claude Desktop config.".to_string(),
            (a, 0) => format!("Imported {a} server(s) from Claude Desktop."),
            (0, s) => format!("All {s} Claude Desktop server(s) already configured."),
            (a, s) => format!("Imported {a} server(s); skipped {s} already present."),
        });
    }
}

/// Remove the stored OAuth token file for an MCP server.
pub(crate) fn clear_mcp_token_file(server_name: &str) {
    let safe: String = server_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dragent_paths::platform_support()
        .join("mcp-tokens")
        .join(format!("{safe}.json"));
    let _ = std::fs::remove_file(path);
}

/// Returns true if a stored OAuth token exists for this MCP server name.
pub fn mcp_token_exists(server_name: &str) -> bool {
    let safe: String = server_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dragent_paths::platform_support()
        .join("mcp-tokens")
        .join(format!("{safe}.json"))
        .exists()
}

/// The width pages get inside a settings pane `pane_width` wide (0 = not
/// measured yet, guessed wide like the first frame's layout).
pub(crate) fn content_width(pane_width: f32) -> f32 {
    ResponsivePaddingContainer::content_width(pane_width)
}

struct ResponsivePaddingContainer {
    child: Box<dyn Element>,
    theme_bg: ColorU,
    /// Actual content-pane pixel width captured by ViewportWidthCapture.
    /// 0 = not yet known (first frame); falls back to constraint.max.x() in that case.
    pane_width: f32,
    /// Padding computed in layout() and reused in paint()/size() for consistency.
    padding: Vector2F,
    /// Final size computed by layout(), including the viewport minimum height.
    /// Keeping this separately is important because the child scrollable may be
    /// shorter than the visible pane while the themed surface must still fill it.
    laid_out_size: Option<Vector2F>,
}

impl ResponsivePaddingContainer {
    pub fn new(child: Box<dyn Element>, theme_bg: ColorU, pane_width: f32) -> Self {
        Self {
            child,
            theme_bg,
            pane_width,
            padding: Vector2F::zero(),
            laid_out_size: None,
        }
    }

    pub fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }

    fn content_width(pane_width: f32) -> f32 {
        let pane = if pane_width > 0.0 {
            pane_width
        } else {
            CONTENT_MAX_WIDTH + 80.0
        };
        (pane - 2.0 * Self::compute_padding(pane).x()).clamp(0.0, CONTENT_MAX_WIDTH)
    }

    fn compute_padding(effective_w: f32) -> Vector2F {
        // When wide enough, center the content by using symmetric padding.
        // The centering point is CONTENT_MAX_WIDTH (680) + 2 × min_padding (40 each side).
        const CONTENT_MAX: f32 = CONTENT_MAX_WIDTH;
        const MIN_PAD: f32 = 40.0;

        let padding_x = if effective_w > CONTENT_MAX + 2.0 * MIN_PAD {
            // Center: add equal side margins so the content area = CONTENT_MAX
            (effective_w - CONTENT_MAX) / 2.0
        } else if effective_w < 500.0 {
            12.0
        } else if effective_w < 700.0 {
            24.0
        } else {
            MIN_PAD
        };

        Vector2F::new(padding_x, 40.0)
    }
}

impl Element for ResponsivePaddingContainer {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // Use the captured pane width when available (finite & non-zero).
        // Fall back to the constraint's max if pane_width is not yet known.
        let effective_w = if self.pane_width > 0.0 {
            self.pane_width
        } else if constraint.max.x().is_finite() {
            constraint.max.x()
        } else {
            CONTENT_MAX_WIDTH + 80.0 // safe first-frame guess: assume wide
        };

        let padding = Self::compute_padding(effective_w);
        self.padding = padding;

        // Content width = pane width minus both side paddings, clamped ≥ 0.
        let content_max_w = (effective_w - padding.x() * 2.0).max(0.0);

        let child_constraint = SizeConstraint::new(
            Vector2F::zero(),
            Vector2F::new(
                content_max_w,
                // Vertical max: pass through (scrollable gives ∞ in scroll direction).
                if constraint.max.y().is_finite() {
                    (constraint.max.y() - padding.y() * 2.0).max(0.0)
                } else {
                    f32::INFINITY
                },
            ),
        );

        let child_size = self.child.layout(child_constraint, ctx, app);
        let content_size = child_size + padding * 2.0;

        // Keep the themed surface at least as tall as the visible settings
        // viewport. Short pages otherwise shrink to their content height and
        // expose the workspace background below them, which makes the image
        // look darker/lighter in bands on tabs with little content.
        let min_viewport_height = if constraint.max.y().is_finite() {
            constraint.max.y()
        } else {
            0.0
        };
        let laid_out_size =
            Vector2F::new(content_size.x(), content_size.y().max(min_viewport_height));
        self.laid_out_size = Some(laid_out_size);
        laid_out_size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self.size().unwrap_or(Vector2F::zero());
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(origin, size))
            .with_background(self.theme_bg);
        self.child.paint(origin + self.padding, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.laid_out_size
            .or_else(|| self.child.size().map(|s| s + self.padding * 2.0))
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        if let (Some(origin), Some(size)) = (self.child.origin(), self.size()) {
            let org = Vector2F::new(origin.x(), origin.y());
            Some(RectF::new(org, size))
        } else {
            self.child.bounds()
        }
    }

    fn parent_data(&self) -> Option<&dyn Any> {
        self.child.parent_data()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}

/// Transparent wrapper that captures the incoming layout constraint height
/// and sends it to the settings view. Internal scrollables (nav tree, icon
/// rail, theme gallery, content pane) cap against it — see `pane_height`.
struct ViewportHeightCapture {
    child: Box<dyn Element>,
    tx: async_channel::Sender<f32>,
}

impl Element for ViewportHeightCapture {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let h = constraint.max.y();
        if h.is_finite() && h > 0.0 {
            let _ = self.tx.try_send(h);
        }
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        self.child.bounds()
    }

    fn parent_data(&self) -> Option<&dyn Any> {
        self.child.parent_data()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}

/// Transparent wrapper that captures the incoming layout constraint width (the real
/// pane width from the workspace layout) and sends it to the settings view so that
/// `ResponsivePaddingContainer` can compute responsive padding correctly even when
/// `ClippedScrollable` passes an unbounded horizontal constraint to its content.
struct ViewportWidthCapture {
    child: Box<dyn Element>,
    tx: async_channel::Sender<f32>,
}

impl Element for ViewportWidthCapture {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let w = constraint.max.x();
        if w.is_finite() && w > 0.0 {
            let _ = self.tx.try_send(w);
        }
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        self.child.bounds()
    }

    fn parent_data(&self) -> Option<&dyn Any> {
        self.child.parent_data()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}
