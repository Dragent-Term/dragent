//! A window that draws chosen screens with sample data and saves each as a
//! PNG, so UI work can be looked at without running Dragent itself: no
//! sign-in, no keychain, no workspaces, nothing written but the images.
//!
//! `cargo run -p dragent --example ui_preview -- <folder>`

mod demo;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use warpui::elements::{
    Align, ClippedScrollStateHandle, ConstrainedBox, Container, Element, ParentElement, Stack,
};
use warpui::fonts::{Cache as FontCache, FamilyId};
use warpui::geometry::vector::vec2f;
use warpui::platform::{CapturedFrame, WindowBounds};
use warpui::{
    AppContext, Entity, EntityId, ReadView, SingletonEntity, TypedActionView, UpdateView, View, ViewContext,
    ViewHandle, WindowId,
};
use warpui_core::elements::MouseStateHandle;

use crate::integrations::{Provider, Source, Status};
use crate::settings::integrations::IntegrationsState;
use crate::settings::SettingsSection;
use crate::settings::pages::integrations::{dialog, page, Model};
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputMode, LinkKind, PromptAgent, PromptLink, PromptMenu};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scene {
    /// The navbar's notification badge and popover.
    NotificationBell { all_read: bool },
    WorkspaceActivity { dark: bool },
    CoordinationSidebar { dark: bool },
    /// The chat's effort slider at several levels.
    EffortSlider { dark: bool },
    /// The current sidebar, for the marketing site's Codex clip.
    MarketingSidebar,
    HoverCard { dark: bool },
    /// The header usage popover on one agent's tab (0 Claude, 1 Codex, 2 OpenCode).
    UsagePopover(usize),
    AgentTranscript { narrow: bool },
    PullRequests(usize),
    CommandTerminal { remote: bool },
    /// A terminal pane a phone has sized, under its cover.
    PhoneCover { dark: bool, held: bool },
    ReviewSetup,
    ComputerAsk { dark: bool },
    OpenReviewWorkspace,
    Page { dark: bool },
    Dialog(Provider),
    AddMenu,
    Results,
    NewTabMenu { dark: bool },
    Teams { dark: bool },
    /// The Tasks page with sample tasks.
    Tasks { dark: bool, board: bool, tracker: bool },
    TeamsActivity,
    /// The coordination's Board; `adding` opens the new-task form.
    TeamsBoard { dark: bool, adding: bool },
    /// The Board while the lead writes the plan (no tasks yet).
    TeamsPlanning,
    TeamsMessage,
    TeamsScrolled,
    TeamsNarrow,
    TeamsLeadStopped,
    TeamsNoPlan,
    TeamsFinished,
    TeamsDelete,
    TeamsStoppedEarly,
    TeamsEmpty,
    TeamMenu,
    LeadModelMenu,
    TaskModelMenu,
    Demo(demo::DemoScene),
    CoordinationComposer { narrow: bool, solo: bool },
    GitSetup { gitlab: bool, connected: bool },
}

const SCENES: &[(&str, Scene)] = &[
    ("workspace_activity_dark", Scene::WorkspaceActivity { dark: true }),
    ("workspace_activity_light", Scene::WorkspaceActivity { dark: false }),
    ("marketing_sidebar", Scene::MarketingSidebar),
    ("effort_slider_dark", Scene::EffortSlider { dark: true }),
    ("effort_slider_light", Scene::EffortSlider { dark: false }),
    ("coordination_sidebar_dark", Scene::CoordinationSidebar { dark: true }),
    ("coordination_sidebar_light", Scene::CoordinationSidebar { dark: false }),
    ("usage_popover_claude", Scene::UsagePopover(0)),
    ("usage_popover_codex", Scene::UsagePopover(1)),
    ("usage_popover_opencode", Scene::UsagePopover(2)),
    ("hover_card_dark", Scene::HoverCard { dark: true }),
    ("hover_card_light", Scene::HoverCard { dark: false }),
    ("pr_overview",Scene::PullRequests(0)),
    ("pr_files",Scene::PullRequests(1)),
    ("pr_checks",Scene::PullRequests(2)),
    ("pr_activity",Scene::PullRequests(3)),
    ("pr_create",Scene::PullRequests(4)),
    ("pr_light",Scene::PullRequests(5)),
    ("pr_empty",Scene::PullRequests(6)),
    ("pr_narrow",Scene::PullRequests(7)),
    ("pr_merge_confirm",Scene::PullRequests(8)),
    ("pr_split",Scene::PullRequests(9)),
    ("pr_draft_review",Scene::PullRequests(10)),
    ("pr_admin_merge",Scene::PullRequests(11)),
    ("pr_narrow_files",Scene::PullRequests(12)),
    ("pr_ready",Scene::PullRequests(13)),
    ("gitlab_overview",Scene::PullRequests(14)),
    ("gitlab_files",Scene::PullRequests(15)),
    ("gitlab_create",Scene::PullRequests(16)),
    ("gitlab_checks",Scene::PullRequests(17)),
    ("review_connections",Scene::PullRequests(18)),
    ("reviews_loading",Scene::PullRequests(19)),
    ("reviews_connection_error",Scene::PullRequests(20)),
    ("reviews_background_refresh",Scene::PullRequests(21)),
    ("setup_github",Scene::GitSetup{gitlab:false,connected:false}),
    ("setup_gitlab",Scene::GitSetup{gitlab:true,connected:false}),
    ("setup_gitlab_connected",Scene::GitSetup{gitlab:true,connected:true}),
    ("review_overview_arriving",Scene::PullRequests(25)),
    ("review_files_arriving",Scene::PullRequests(26)),
    ("review_discussion_arriving",Scene::PullRequests(27)),
    ("review_project_picker",Scene::PullRequests(28)),
    ("review_state_picker",Scene::PullRequests(29)),
    ("review_filter_picker",Scene::PullRequests(30)),
    ("review_branch_picker",Scene::PullRequests(31)),
    ("review_merge_picker",Scene::PullRequests(32)),
    ("review_narrow_picker",Scene::PullRequests(33)),
    ("review_merged_activity",Scene::PullRequests(34)),
    ("review_merged_line_comment",Scene::PullRequests(35)),
    ("pr_create_choose_source",Scene::PullRequests(36)),
    ("review_source_picker",Scene::PullRequests(37)),
    ("review_template_picker",Scene::PullRequests(38)),
    ("pr_create_preview",Scene::PullRequests(39)),
    ("pr_check_log",Scene::PullRequests(40)),
    ("review_login_terminal",Scene::CommandTerminal{remote:false}),
    ("review_ssh_login_terminal",Scene::CommandTerminal{remote:true}),
    ("terminal_phone_cover_dark", Scene::PhoneCover { dark: true, held: false }),
    ("terminal_phone_cover_light", Scene::PhoneCover { dark: false, held: false }),
    ("terminal_phone_cover_held", Scene::PhoneCover { dark: true, held: true }),
    ("review_agent_setup",Scene::ReviewSetup),
    ("review_open_workspace",Scene::OpenReviewWorkspace),
    ("integrations_dark", Scene::Page { dark: true }),
    ("integrations_light", Scene::Page { dark: false }),
    ("dialog_jira_connect", Scene::Dialog(Provider::Jira)),
    ("dialog_linear_connected", Scene::Dialog(Provider::Linear)),
    ("dialog_github_missing", Scene::Dialog(Provider::Github)),
    ("dialog_sentry_team", Scene::Dialog(Provider::Sentry)),
    ("dialog_notion_share", Scene::Dialog(Provider::Notion)),
    ("prompt_add_menu", Scene::AddMenu),
    ("prompt_linear_results", Scene::Results),
    ("header_new_tab_dark", Scene::NewTabMenu { dark: true }),
    ("header_new_tab_light", Scene::NewTabMenu { dark: false }),
    ("tasks_list_dark", Scene::Tasks { dark: true, board: false, tracker: false }),
    ("tasks_board_dark", Scene::Tasks { dark: true, board: true, tracker: false }),
    ("tasks_list_light", Scene::Tasks { dark: false, board: false, tracker: false }),
    ("tasks_tracker_dark", Scene::Tasks { dark: true, board: false, tracker: true }),
    ("notification_bell", Scene::NotificationBell { all_read: false }),
    ("notification_bell_empty", Scene::NotificationBell { all_read: true }),
    ("teams_dark", Scene::Teams { dark: true }),
    ("teams_light", Scene::Teams { dark: false }),
    ("teams_activity", Scene::TeamsActivity),
    ("teams_board_dark", Scene::TeamsBoard { dark: true, adding: false }),
    ("teams_board_light", Scene::TeamsBoard { dark: false, adding: false }),
    ("teams_board_adding", Scene::TeamsBoard { dark: true, adding: true }),
    ("teams_planning", Scene::TeamsPlanning),
    ("teams_message_multiline", Scene::TeamsMessage),
    ("teams_plan_scrolled", Scene::TeamsScrolled),
    ("teams_narrow", Scene::TeamsNarrow),
    ("teams_lead_stopped", Scene::TeamsLeadStopped),
    ("teams_no_plan", Scene::TeamsNoPlan),
    ("teams_finished", Scene::TeamsFinished),
    ("teams_delete", Scene::TeamsDelete),
    ("teams_stopped_early", Scene::TeamsStoppedEarly),
    ("teams_empty", Scene::TeamsEmpty),
    ("prompt_team_menu", Scene::TeamMenu),
    ("coordination_lead_model", Scene::LeadModelMenu),
    ("coordination_task_model", Scene::TaskModelMenu),
    ("coordination_composer_wide", Scene::CoordinationComposer { narrow: false, solo: false }),
    ("coordination_composer_narrow", Scene::CoordinationComposer { narrow: true, solo: false }),
    ("solo_composer_model", Scene::CoordinationComposer { narrow: false, solo: true }),
    ("demo_overview", Scene::Demo(demo::DemoScene::Terminal)),
    ("demo_project_solo", Scene::Demo(demo::DemoScene::Solo)),
    ("demo_project_creating", Scene::Demo(demo::DemoScene::SoloBusy)),
    ("demo_project_coordinate", Scene::Demo(demo::DemoScene::Coordinate)),
    ("demo_ssh_machines", Scene::Demo(demo::DemoScene::Machines)),
    ("demo_ssh_system", Scene::Demo(demo::DemoScene::MachineSystem)),
    ("demo_ssh_agents", Scene::Demo(demo::DemoScene::MachineAgents)),
    ("demo_editor", Scene::Demo(demo::DemoScene::Editor)),
    ("editor_find_counter_312", Scene::Demo(demo::DemoScene::EditorFind { matches: 312 })),
    ("editor_find_counter_12345", Scene::Demo(demo::DemoScene::EditorFind { matches: 12345 })),
    ("demo_diff", Scene::Demo(demo::DemoScene::Diff)),
    ("demo_pr_review", Scene::Demo(demo::DemoScene::Review)),
    ("demo_sidebar_search", Scene::Demo(demo::DemoScene::SidebarSearch)),
    ("demo_sidebar_git_filter", Scene::Demo(demo::DemoScene::SidebarGitFilter)),
    ("demo_automations", Scene::Demo(demo::DemoScene::Automations)),
    ("demo_accounts", Scene::Demo(demo::DemoScene::Accounts)),
    ("demo_models_settings", Scene::Demo(demo::DemoScene::Models)),
    ("demo_mcp", Scene::Demo(demo::DemoScene::Mcp)),
    ("demo_skills", Scene::Demo(demo::DemoScene::Skills)),
    ("demo_notifications", Scene::Demo(demo::DemoScene::Notifications)),
    ("demo_appearance", Scene::Demo(demo::DemoScene::Appearance)),
    ("demo_privacy", Scene::Demo(demo::DemoScene::Privacy)),
    ("agent_transcript", Scene::AgentTranscript { narrow: false }),
    ("agent_transcript_narrow", Scene::AgentTranscript { narrow: true }),
    ("settings_account", settings(SettingsSection::Account)),
    ("settings_general", settings(SettingsSection::General)),
    ("settings_appearance", settings(SettingsSection::Appearance)),
    ("settings_terminal_input", settings(SettingsSection::TerminalInput)),
    ("settings_version_control", settings(SettingsSection::VersionControl)),
    ("settings_version_control_custom", settings_after(SettingsSection::VersionControl, "custom_prefix")),
    ("settings_keyboard", settings(SettingsSection::KeyboardShortcuts)),
    ("settings_keyboard_rebind", settings_after(SettingsSection::KeyboardShortcuts, "rebind")),
    ("settings_agent_general", settings(SettingsSection::AgentGeneral)),
    ("settings_agent_general_agents", settings_after(SettingsSection::AgentGeneral, "behavior_agents")),
    ("settings_agent_general_picker", settings_after(SettingsSection::AgentGeneral, "behavior_picker")),
    ("settings_agent_models", settings(SettingsSection::AgentModels)),
    ("settings_agent_mcp", settings(SettingsSection::AgentMcp)),
    ("settings_agent_mcp_form", settings_after(SettingsSection::AgentMcp, "mcp_form")),
    ("settings_agent_skills", settings(SettingsSection::AgentSkills)),
    ("settings_agent_skills_search", settings_after(SettingsSection::AgentSkills, "skills_search")),
    ("settings_agent_skills_menu", settings_after(SettingsSection::AgentSkills, "skills_menu")),
    ("settings_agent_skills_all", settings_after(SettingsSection::AgentSkills, "skills_all")),
    ("settings_agent_skills_details", settings_after(SettingsSection::AgentSkills, "skills_details")),
    ("settings_agent_skills_empty", settings_after(SettingsSection::AgentSkills, "skills_empty")),
    ("settings_agent_skills_create", settings_after(SettingsSection::AgentSkills, "skills_create")),
    ("settings_agent_skills_create_error", settings_after(SettingsSection::AgentSkills, "skills_create_error")),
    ("settings_agent_commands", settings(SettingsSection::AgentCommands)),
    ("settings_agent_accounts", settings(SettingsSection::AgentAccounts)),
    ("settings_usage", settings(SettingsSection::Usage)),
    ("settings_editor", settings(SettingsSection::Editor)),
    ("settings_code_index", settings(SettingsSection::CodeIndex)),
    ("settings_teams", settings(SettingsSection::Teams)),
    ("settings_projects", settings(SettingsSection::Projects)),
    ("settings_members", settings(SettingsSection::Members)),
    ("settings_billing", settings(SettingsSection::Billing)),
    ("settings_integrations", settings_after(SettingsSection::Integrations, "integrations_services")),
    ("settings_integrations_tasks", settings_after(SettingsSection::Integrations, "integrations_tasks")),
    ("settings_integrations_agents", settings_after(SettingsSection::Integrations, "integrations_agents")),
    ("settings_privacy", settings(SettingsSection::Privacy)),
    ("settings_notifications", settings(SettingsSection::Notifications)),
    ("settings_voice", settings(SettingsSection::Voice)),
    (
        "settings_voice_engine_picker",
        settings_after(SettingsSection::Voice, "voice_engine"),
    ),
    (
        "settings_voice_language_picker",
        settings_after(SettingsSection::Voice, "voice_lang"),
    ),
    (
        "settings_voice_legacy_model",
        settings_after(SettingsSection::Voice, "voice_legacy"),
    ),
    (
        "settings_voice_system",
        settings_after(SettingsSection::Voice, "voice_system"),
    ),
    (
        "settings_developer_permissions",
        settings(SettingsSection::DeveloperPermissions),
    ),
    ("computer_ask_dark", Scene::ComputerAsk { dark: true }),
    ("computer_ask_light", Scene::ComputerAsk { dark: false }),
    (
        "settings_computer_use",
        settings(SettingsSection::ComputerUse),
    ),
    (
        "settings_computer_reset",
        settings_after(SettingsSection::ComputerUse, "computer_reset"),
    ),
    (
        "settings_remote_access",
        settings(SettingsSection::RemoteAccess),
    ),
    ("settings_machines", settings(SettingsSection::Machines)),
    ("settings_mobile_signed_out", settings(SettingsSection::Mobile)),
    ("settings_mobile_idle", settings_after(SettingsSection::Mobile, "idle")),
    ("settings_mobile_live", settings_after(SettingsSection::Mobile, "live")),
    ("settings_mobile_live_light", settings_after(SettingsSection::Mobile, "live_light")),
    ("settings_mobile_enlarged", settings_after(SettingsSection::Mobile, "enlarged")),
    ("settings_mobile_expired", settings_after(SettingsSection::Mobile, "expired")),
    ("settings_mobile_paired", settings_after(SettingsSection::Mobile, "paired")),
    ("settings_mobile_revoke", settings_after(SettingsSection::Mobile, "revoke")),
    ("settings_mobile_narrow", settings_after(SettingsSection::Mobile, "narrow")),
    ("settings_mobile_reset", settings_after(SettingsSection::Mobile, "reset")),
    ("settings_mobile_no_store", settings_after(SettingsSection::Mobile, "no_store")),
    ("settings_mobile_records_reset", settings_after(SettingsSection::Mobile, "records_reset")),
    ("settings_about", settings(SettingsSection::About)),

];

const fn settings(section: SettingsSection) -> Scene {
    Scene::Demo(demo::DemoScene::Settings(section))
}

const fn settings_after(section: SettingsSection, state: &'static str) -> Scene {
    Scene::Demo(demo::DemoScene::SettingsAfter(section, state))
}

/// `DRAGENT_PREVIEW_ONLY=settings_,demo_ssh` limits a run to the scenes whose
/// names start with one of the comma-separated prefixes.
fn wanted(scene: usize) -> bool {
    std::env::var("DRAGENT_PREVIEW_ONLY").map_or(true, |prefixes| {
        prefixes.split(',').any(|prefix| SCENES[scene].0.starts_with(prefix.trim()))
    })
}

fn next_wanted(after: usize) -> Option<usize> {
    (after + 1..SCENES.len()).find(|&scene| wanted(scene))
}

/// Writes one captured frame as an image (the example brings the encoder,
/// so the app itself doesn't depend on one).
pub type Save = fn(CapturedFrame, &std::path::Path) -> Result<(), String>;

pub fn run(out: PathBuf, save: Save) {
    let builder = warpui::platform::AppBuilder::new(
        warpui::platform::AppCallbacks::default(),
        Box::new(dragent_assets::DragentAssets),
        None,
    );
    let _ = builder.run(move |ctx| {
        ctx.add_singleton_model(crate::settings::AccountStore::new);
        demo::register(ctx);
        let options = warpui::AddWindowOptions {
            window_bounds: WindowBounds::ExactSize(vec2f(if std::env::var_os("DRAGENT_DEMO_CAPTURE").is_some() { 1600.0 } else { 1180.0 }, 900.0)),
            window_style: warpui::platform::WindowStyle::NotStealFocus,
            ..Default::default()
        };
        #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
        let (window_id, _) =
            ctx.add_window(options, move |view_ctx| Preview::new(view_ctx, out, save));
        #[cfg(target_os = "macos")]
        ctx.windows()
            .show_window_and_focus_app_without_ordering_front(window_id);
    });
}

struct Preview {
    window_id: WindowId,
    view_id: EntityId,
    save: Save,
    out: PathBuf,
    scene: usize,
    recorded_frames: usize,
    recorded_input_lines: usize,
    recorded_keys: std::collections::VecDeque<warpui_core::keymap::Keystroke>,
    recording_started: std::time::Instant,
    ui: FamilyId,
    code: FamilyId,
    integrations: IntegrationsState,
    hovers: RefCell<HashMap<String, MouseStateHandle>>,
    scroll: ClippedScrollStateHandle,
    search: ViewHandle<InputEditor>,
    teams: ViewHandle<crate::teams::TeamsView>,
    tasks: ViewHandle<crate::tasks_page::TasksView>,
    demo: demo::DemoViews,
    prs: ViewHandle<crate::pull_requests::PullRequestsView>,
    review_editor: ViewHandle<InputEditor>,
    review_setup: Option<crate::pull_requests::ReviewSetup>,
}

fn editor(
    ctx: &mut ViewContext<Preview>,
    ui: FamilyId,
    theme: Theme,
    placeholder: &'static str,
) -> ViewHandle<InputEditor> {
    ctx.add_typed_action_view(move |inner| {
        InputEditor::new(
            placeholder,
            ui,
            crate::theme::text_size(12.5),
            theme.main_text(),
            theme.disabled_text(),
            theme.accent,
            theme,
            InputMode::Plain {
                key: format!("preview_{placeholder}"),
            },
            inner.focus_handle(),
        )
    })
}

impl Preview {
    fn new(ctx: &mut ViewContext<Self>, out: PathBuf, save: Save) -> Self {
        let ui = FontCache::handle(ctx).update(ctx, |cache, _| crate::fonts::ui_family(cache));
        let theme = Theme::dark();
        let mut integrations = IntegrationsState::new(ctx, ui, theme);
        integrations.statuses = sample_statuses();
        integrations.server = true;
        integrations.admin = true;
        let search = editor(ctx, ui, theme, "Search");
        let mono = FontCache::handle(ctx).update(ctx, |cache, _| crate::fonts::code_family(cache));
        let teams =
            ctx.add_typed_action_view(|inner| crate::teams::TeamsView::new(inner, ui, mono, theme));
        ctx.update_view(&teams, |page, page_ctx| {
            sample_teams(page);
            page_ctx.notify();
        });
        let tasks = ctx.add_typed_action_view(|inner| crate::tasks_page::TasksView::new(inner, ui, mono, theme));
        ctx.update_view(&tasks, |page, page_ctx| {
            crate::tasks_page::preview::seed(page, crate::tasks_page::Layout::Table, false);
            page_ctx.notify();
        });
        let demo = demo::DemoViews::new(ctx, ui, mono, theme);
        let prs=ctx.add_typed_action_view(|c|crate::pull_requests::PullRequestsView::new(c,ui,mono,theme));
        ctx.subscribe_to_view(&prs,|preview,_,event,c| {
            if let crate::pull_requests::Event::ReviewSetup(review)=event {
                assert!(review.head.len()>=40);
                if !review.launch_agent {
                    assert!(review.instructions.is_empty());
                    println!("Open workspace handoff is deferred and starts no agent");
                    return;
                }
                assert!(review.instructions.contains(&review.url));
                assert!(review.instructions.contains(&review.head));
                assert!(review.instructions.contains("Present findings"));
                preview.review_setup=Some(review.clone());
                c.update_view(&preview.review_editor,|e,c| {
                    e.set_text(&review.instructions,c);e.schedule_width_reflow(650.0,c);
                });
                println!("Review setup handoff contains pinned identity and prefilled instructions (no fetch)");
            }
        });
        ctx.update_view(&prs,|p,c|{crate::pull_requests::preview::seed(p,0,c);if std::env::var_os("DRAGENT_PREVIEW_PR_ONLY").is_some(){crate::pull_requests::preview::smoke(p,c);}});
        let review_editor=ctx.add_typed_action_view(|c| InputEditor::new("Review instructions…",mono,crate::theme::text_size(13.0),
            theme.main_text(),theme.disabled_text(),theme.accent,theme,InputMode::MultiLine{key:"preview_review".into(),max_height:220},c.focus_handle()));
        ctx.update_view(&review_editor,|e,c|e.set_text("Review pull request #128: Add secure team invitations\nhttps://github.com/acme/app/pull/128\n\nCompare feat/team-invites against main at the pinned commit. Inspect the changed files, checks, and unresolved threads.\n\nRun relevant tests and report actionable findings with file and line references. Present findings first. Do not edit tracked files, commit, push, merge, or publish comments unless the user asks.",c));
        ctx.update_view(&review_editor,|e,c|e.schedule_width_reflow(650.0,c));
        let _ = std::fs::create_dir_all(&out);
        let initial_scene=if std::env::var_os("DRAGENT_PREVIEW_AGENT_ONLY").is_some() { SCENES.iter().position(|(name,_)|*name=="agent_transcript").unwrap() } else if std::env::var_os("DRAGENT_PREVIEW_TEAMS_ONLY").is_some() {
            ctx.update_view(&teams,|page,c|page.set_theme(Theme::default(),c));
            SCENES.iter().position(|(name,_)|*name=="teams_dark").unwrap()
        } else {
            (0..SCENES.len()).find(|&scene| wanted(scene)).expect("DRAGENT_PREVIEW_ONLY matches no scene")
        };
        let initial_scene = std::env::var("DRAGENT_PREVIEW_INTERACTIVE")
            .ok()
            .map(|name| SCENES.iter().position(|(scene, _)| *scene == name)
                .unwrap_or_else(|| panic!("Unknown interactive preview scene: {name}")))
            .unwrap_or(initial_scene);
        let mut preview = Self {
            window_id: ctx.window_id(),
            view_id: ctx.view_id(),
            save,
            out,
            scene: initial_scene,
            recorded_frames: 0,
            recorded_input_lines: 0,
            recorded_keys: std::collections::VecDeque::new(),
            recording_started: std::time::Instant::now(),
            ui,
            code: mono,
            integrations,
            hovers: RefCell::default(),
            scroll: ClippedScrollStateHandle::default(),
            search,
            teams,
            tasks,
            demo,
            prs,
            review_editor,
            review_setup: None,
        };
        if std::env::var_os("DRAGENT_PREVIEW_INTERACTIVE").is_some() {
            // Hold a sample-data window for recording actual native interactions.
            // No account, provider, SSH connection, or scheduler is started.
            match SCENES[initial_scene].1 {
                Scene::Demo(scene) => preview.demo.prepare(scene, ctx),
                Scene::PullRequests(scene) => ctx.update_view(&preview.prs, |page, c| {
                    crate::pull_requests::preview::seed(page, scene, c);
                }),
                Scene::Teams { dark } => ctx.update_view(&preview.teams, |page, c| {
                    page.set_theme(if dark { Theme::default() } else { Theme::light() }, c);
                }),
                _ => {}
            }
            if std::env::var_os("DRAGENT_PREVIEW_RECORD").is_some() {
                preview.schedule_recording(ctx);
            }
        } else {
            if let Scene::Demo(scene) = SCENES[initial_scene].1 {
                preview.demo.prepare(scene, ctx);
            }
            preview.schedule_capture(ctx);
        }
        preview
    }

    // Capture the live window framebuffer while native controls are operated.
    // Unlike a desktop recording this cannot include other apps or notifications.
    fn schedule_recording(&self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            async { warpui::r#async::Timer::after(std::time::Duration::from_millis(67)).await },
            |view, _, ctx| view.record_frame(ctx),
        );
    }

    fn record_frame(&mut self, ctx: &mut ViewContext<Self>) {
        let elapsed = self.recording_started.elapsed().as_millis();
        let seconds = std::env::var("DRAGENT_PREVIEW_RECORD_SECONDS")
            .ok().and_then(|s| s.parse::<u128>().ok()).unwrap_or(90);
        if elapsed >= seconds * 1000 {
            std::process::exit(0);
        }
        // Optional local keyboard replay for repeatable native UI recordings.
        // Each key passes through the production editor action handler; this does
        // not set a rendered frame or simulate agent/backend results.
        if let Ok(input) = std::fs::read_to_string(self.out.join("input.jsonl")) {
            let lines: Vec<_> = input.lines().collect();
            for line in lines.iter().skip(self.recorded_input_lines) {
                if let Ok(command) = serde_json::from_str::<serde_json::Value>(line) {
                    if command["select_all"].as_bool() == Some(true) {
                        self.recorded_keys.push_back(warpui_core::keymap::Keystroke::parse("cmd-a").unwrap());
                    }
                    if let Some(text) = command["text"].as_str() {
                        for ch in text.chars() {
                            self.recorded_keys.push_back(warpui_core::keymap::Keystroke {
                                key: if ch == ' ' { "space".into() } else { ch.to_string() },
                                ..Default::default()
                            });
                        }
                    }
                }
            }
            self.recorded_input_lines = lines.len();
        }
        if let Some(key) = self.recorded_keys.pop_front() {
            let editor = match SCENES[self.scene].1 {
                Scene::PullRequests(_) => Some(ctx.read_view(&self.prs, |view, _| view.composer.clone())),
                Scene::Teams { .. } => Some(ctx.read_view(&self.teams, |view, _| view.message_editor.clone())),
                Scene::Demo(_) => Some(self.demo.recording_prompt()),
                _ => None,
            };
            if let Some(editor) = editor {
                ctx.update_view(&editor, |editor, c| {
                    c.focus_self();
                    editor.handle_action(&crate::workspace::InputEditorAction::KeyDown(key), c);
                });
            }
        }
        let path = self.out.join(format!("frame-{:06}-{elapsed:08}.png", self.recorded_frames));
        self.recorded_frames += 1;
        let save = self.save;
        if let Some(window) = ctx.windows().platform_window(self.window_id) {
            window.as_ctx().request_frame_capture(Box::new(move |frame| {
                if let Err(error) = save(frame, &path) {
                    eprintln!("Recording frame failed: {error}");
                }
            }));
        }
        ctx.notify();
        self.schedule_recording(ctx);
    }

    fn schedule_capture(&self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            async { warpui::r#async::Timer::after(std::time::Duration::from_millis(900)).await },
            |view, _, ctx| view.capture(ctx),
        );
    }

    fn capture(&mut self, ctx: &mut ViewContext<Self>) {
        let (name, _) = SCENES[self.scene];
        let path = self.out.join(format!("{name}.png"));
        let last = (std::env::var_os("DRAGENT_PREVIEW_AGENT_ONLY").is_some() && matches!(SCENES[self.scene].1, Scene::AgentTranscript { narrow: true })) || next_wanted(self.scene).is_none() || (std::env::var_os("DRAGENT_PREVIEW_PR_ONLY").is_some() && SCENES[self.scene].0=="review_open_workspace")
            || (std::env::var_os("DRAGENT_PREVIEW_TEAMS_ONLY").is_some() && matches!(SCENES[self.scene].1,Scene::TeamsEmpty));
        let save = self.save;
        if let Some(window) = ctx.windows().platform_window(self.window_id) {
            window
                .as_ctx()
                .request_frame_capture(Box::new(move |frame| {
                    match save(frame, &path) {
                        Ok(()) => println!("saved {}", path.display()),
                        Err(error) => eprintln!("couldn't save {}: {error}", path.display()),
                    }
                    if last {
                        std::process::exit(0);
                    }
                }));
        }
        if !last {
            ctx.spawn(
                async {
                    warpui::r#async::Timer::after(std::time::Duration::from_millis(500)).await
                },
                |view, _, ctx| {
                    view.scene = next_wanted(view.scene).unwrap_or(view.scene + 1);
                    view.integrations.open = match SCENES[view.scene].1 {
                        Scene::Dialog(provider) => Some(provider),
                        _ => None,
                    };
                    let scene = SCENES[view.scene].1;
                    if let Scene::PullRequests(scene)=scene { let prs=view.prs.clone();ctx.update_view(&prs,|p,c|crate::pull_requests::preview::seed(p,scene,c)); }
                    if let Scene::Demo(scene) = scene { view.demo.prepare(scene, ctx); }
                    if let Scene::Tasks { dark, board, tracker } = scene {
                        let tasks = view.tasks.clone();
                        ctx.update_view(&tasks, |page, page_ctx| {
                            page.set_theme(if dark { Theme::default() } else { Theme::light() }, page_ctx);
                            let layout = if board { crate::tasks_page::Layout::Board } else { crate::tasks_page::Layout::Table };
                            crate::tasks_page::preview::seed(page, layout, tracker);
                            page_ctx.notify();
                        });
                    }
                    let teams = view.teams.clone();
                    ctx.update_view(&teams, |page, page_ctx| {
                        match scene {
                            Scene::Teams { dark } => {
                                page.set_theme(
                                    if dark { Theme::default() } else { Theme::light() },
                                    page_ctx,
                                );
                                sample_teams(page);
                            }
                            Scene::TaskModelMenu => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                page.model_task = Some("t2".into());
                                page.model_tab = crate::teams::models::ModelTab::Agent(PromptAgent::Claude);
                                page.model_query = "opus".into();
                                let editor = page.model_editor.clone();
                                page_ctx.update_view(&editor, |editor, ctx| editor.set_text("opus", ctx));
                            }
                            Scene::TeamsMessage | Scene::TeamsScrolled => {
                                page.set_theme(Theme::default(),page_ctx);
                                sample_teams(page);
                                page.message="Please verify the login flow.\n\nInclude expired tokens and explain any failing tests before continuing.".into();
                                let text=page.message.clone();
                                page_ctx.update_view(&page.message_editor,|editor,c| {editor.set_text(&text,c);editor.schedule_width_reflow(238.0,c);});
                                if matches!(scene,Scene::TeamsScrolled) {
                                    use warpui_core::units::IntoPixels;
                                    if let Some(team)=page.team.as_mut() {
                                        let template=team.tasks[0].clone();
                                        for n in 0..24 {let mut task=template.clone();task.id=format!("extra-{n}");task.title=format!("Verify additional coordination scenario {}",n+1);team.tasks.push(task);}
                                    }
                                    page.detail_scroll.scroll_to(750.0.into_pixels());
                                }
                            }
                            Scene::TeamsActivity => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                page.tab = crate::teams::DetailTab::Activity;
                            }
                            Scene::TeamsBoard { dark, adding } => {
                                page.set_theme(if dark { Theme::default() } else { Theme::light() }, page_ctx);
                                sample_teams(page);
                                page.tab = crate::teams::DetailTab::Board;
                                page.adding_task = adding;
                            }
                            Scene::TeamsPlanning => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                page.tab = crate::teams::DetailTab::Board;
                                if let Some(team) = page.team.as_mut() {
                                    team.status = dragent_host::team::TeamStatus::Planning;
                                    team.tasks.clear();
                                }
                            }
                            Scene::TeamsNarrow => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                page.width.set(1000.0);
                                page_ctx.update_view(&page.message_editor,|editor,c|editor.schedule_width_reflow(618.0,c));
                            }
                            Scene::TeamsLeadStopped => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                page.lead = crate::teams::LeadState::default();
                            }
                            Scene::TeamsNoPlan => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                page.lead = crate::teams::LeadState::default();
                                if let Some(team) = page.team.as_mut() {
                                    team.status = dragent_host::team::TeamStatus::Planning;
                                    team.title = "iwant to build a greenfield app, using the latest sdk, building similar features".into();
                                    team.tasks.clear();
                                    team.messages.clear();
                                }
                                page.workers.clear();
                            }
                            Scene::TeamsStoppedEarly => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                sample_stopped_early(page);
                                page.tab = crate::teams::DetailTab::Overview;
                            }
                            Scene::TeamsFinished => {
                                page.set_theme(Theme::default(), page_ctx);
                                sample_teams(page);
                                sample_finished(page);
                                page.expanded.insert("t1".to_string());
                            }
                            Scene::TeamsDelete => {
                                page.set_theme(Theme::dark(), page_ctx);
                                sample_teams(page);
                                sample_finished(page);
                                page.confirm_delete = Some(crate::teams::DeletePlan {
                                    remove: vec![
                                        ("t2".into(), "/work/app-auth-api".into()),
                                        ("t3".into(), "/work/app-auth-ui".into()),
                                        ("t4".into(), "/work/app-auth-tests".into()),
                                    ],
                                    dirty: vec!["t3".into()],
                                    workers: vec!["w1".into(), "w2".into(), "w3".into()],
                                });
                            }
                            Scene::TeamsEmpty => {
                                page.set_theme(Theme::dark(), page_ctx);
                                page.teams.clear();
                                page.team = None;
                                page.loading = false;
                            }
                            _ => {}
                        }
                        page_ctx.notify();
                    });
                    ctx.notify();
                    view.schedule_capture(ctx);
                },
            );
        }
    }

    fn hover(&self, key: &str) -> MouseStateHandle {
        self.hovers
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }
}

fn sample_statuses() -> HashMap<Provider, Status> {
    let mut statuses: HashMap<Provider, Status> = Provider::ALL
        .into_iter()
        .map(|provider| {
            let status = match provider.unavailable() {
                Some(reason) => Status::Unavailable(reason.to_string()),
                None => Status::NotConnected,
            };
            (provider, status)
        })
        .collect();
    statuses.insert(
        Provider::Linear,
        Status::Connected {
            account: Some("Ada Lovelace (ada@acme.dev)".into()),
            source: Source::Account,
        },
    );
    statuses.insert(
        Provider::Sentry,
        Status::Connected {
            account: Some("Acme".into()),
            source: Source::Team {
                by: "Grace Hopper".into(),
                manageable: true,
            },
        },
    );
    statuses.insert(
        Provider::Github,
        Status::Unavailable(
            "The GitHub CLI isn't signed in. Run gh auth login in a terminal.".into(),
        ),
    );
    statuses
}

/// A team at work: one task done and approved, one working (at an
/// approval prompt), one blocked on a question, one waiting; and a
/// question for the person.
fn sample_teams(page: &mut crate::teams::TeamsView) {
    use crate::teams::WorkerState;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let min = 60_000_i64;
    let task = |id: &str, title: &str, status: &str, agent: &str, model: Option<&str>| {
        serde_json::json!({
            "id": id, "title": title, "prompt": "", "kind": "work", "status": status,
            "agent": agent, "model": model, "created_at_ms": now, "updated_at_ms": now,
        })
    };
    let mut tasks = vec![
        task("t1", "Invite API", "done", "codex", Some("gpt-5-codex")),
        task("t2", "Invite page", "running", "claude", Some("sonnet")),
        task("t3", "Review the API", "done", "claude", Some("opus")),
        task("t4", "Expiry emails", "blocked", "codex", None),
        task("t5", "Integrate", "pending", "claude", None),
        task(
            "t6",
            "QA the whole feature",
            "pending",
            "claude",
            Some("opus"),
        ),
    ];
    tasks[0]["result"] = serde_json::json!({"summary": "POST /invites sends a signed link that expires; 14 tests pass."});
    tasks[0]["branch"] = "invite-api-t1-1b4f".into();
    tasks[0]["attempts"] = 2.into();
    tasks[1]["depends_on"] = serde_json::json!(["t1"]);
    tasks[1]["branch"] = "invite-page-t2-1b4f".into();
    tasks[1]["worker"] =
        serde_json::json!({"pane": "00000000-0000-0000-0000-000000000002", "agent": "claude"});
    tasks[2]["kind"] = "review".into();
    tasks[2]["review_of"] = "t1".into();
    tasks[2]["depends_on"] = serde_json::json!(["t1"]);
    tasks[2]["result"] = serde_json::json!({"summary": "Email validation and token hashing are right; tests pass.", "verdict": "approve"});
    tasks[3]["reason"] =
        "Which mail provider should the expiry emails go through: Resend or SES?".into();
    tasks[3]["worker"] =
        serde_json::json!({"pane": "00000000-0000-0000-0000-000000000004", "agent": "codex"});
    tasks[4]["depends_on"] = serde_json::json!(["t2", "t3", "t4"]);
    tasks[5]["kind"] = "review".into();
    tasks[5]["qa"] = true.into();
    tasks[5]["review_of"] = "t5".into();
    tasks[5]["depends_on"] = serde_json::json!(["t5"]);
    let message = |id: u64,
                   from: serde_json::Value,
                   to: serde_json::Value,
                   kind: &str,
                   body: &str,
                   ago: i64| {
        serde_json::json!({"id": id, "from": from, "to": to, "kind": kind, "body": body, "at_ms": now - ago * min, "read": true})
    };
    let event = |revision: u64, ago: i64, change: serde_json::Value| serde_json::json!({"revision": revision, "at_ms": now - ago * min, "change": change});
    let team = serde_json::json!({
        "id": "team-1b4f4a69",
        "title": "Let teammates invite people by email",
        "project_root": "/Users/ada/code/acme-app",
        "status": "running",
        "lead": {"pane": "00000000-0000-0000-0000-000000000001", "agent": "claude"},
        "roster": [{"agent": "claude", "models": []}, {"agent": "codex", "models": []}],
        "cost": "balanced",
        "max_parallel_per_agent": 3,
        "tasks": tasks,
        "messages": [
            message(1, serde_json::json!({"task": "t4"}), "lead".into(), "question", "Which mail provider should the expiry emails go through: Resend or SES?", 25),
            message(2, "lead".into(), "user".into(), "needs_user", "The expiry emails need a sender. Should they go through Resend (already used for receipts) or SES?", 12),
            message(3, "dragent".into(), "lead".into(), "finding", "The worker on t2 (Invite page) is waiting for an approval in its terminal.", 4),
        ],
        "events": [
            event(1, 118, serde_json::json!({"team": {"status": "planning"}})),
            event(2, 115, serde_json::json!({"task_added": {"task": "t1"}})),
            event(3, 115, serde_json::json!({"task_added": {"task": "t2"}})),
            event(4, 114, serde_json::json!({"task_added": {"task": "t3"}})),
            event(5, 114, serde_json::json!({"task_added": {"task": "t4"}})),
            event(6, 113, serde_json::json!({"task_added": {"task": "t5"}})),
            event(6, 113, serde_json::json!({"task_added": {"task": "t6"}})),
            event(7, 104, serde_json::json!({"task": {"task": "t1", "status": "running"}})),
            event(8, 101, serde_json::json!({"task": {"task": "t4", "status": "running"}})),
            event(9, 62, serde_json::json!({"task": {"task": "t1", "status": "done"}})),
            event(10, 58, serde_json::json!({"task": {"task": "t2", "status": "running"}})),
            event(11, 25, serde_json::json!({"message": {"id": 1, "to": "lead"}})),
            event(12, 24, serde_json::json!({"task": {"task": "t4", "status": "blocked"}})),
            event(13, 19, serde_json::json!({"task": {"task": "t3", "status": "done"}})),
            event(14, 12, serde_json::json!({"message": {"id": 2, "to": "user"}})),
            event(15, 4, serde_json::json!({"message": {"id": 3, "to": "lead"}})),
        ],
        "created_at_ms": now - 118 * min,
        "updated_at_ms": now - 4 * min,
    });
    let team: dragent_host::team::Team = serde_json::from_value(team).expect("a sample team");
    let summary = |id: &str,
                   title: &str,
                   status: &str,
                   tasks: usize,
                   done: usize,
                   running: usize,
                   blocked: usize,
                   needs: usize| {
        serde_json::from_value::<dragent_host::team::TeamSummary>(serde_json::json!({
            "id": id, "title": title, "project_root": "/Users/ada/code/acme-app", "status": status,
            "tasks": tasks, "done": done, "running": running, "blocked": blocked, "needs_user": needs,
            "updated_at_ms": now - (tasks as i64 * 37 % 300) * min,
        }))
        .expect("a sample summary")
    };
    page.teams = vec![
        summary(
            "team-1b4f4a69",
            "Let teammates invite people by email",
            "running",
            6,
            2,
            1,
            1,
            1,
        ),
        summary(
            "team-77828b44",
            "Fix the flaky upload tests",
            "done",
            3,
            3,
            0,
            0,
            0,
        ),
        summary(
            "team-9c2e0d11",
            "Dark mode for the settings pages",
            "planning",
            0,
            0,
            0,
            0,
            0,
        ),
    ];
    page.selected = Some(team.id.clone());
    page.team = Some(team);
    page.workers = [
        ("t2".to_string(), WorkerState::NeedsApproval),
        ("t4".to_string(), WorkerState::Idle),
    ]
    .into();
    page.loading = false;
    page.error = None;
    page.lead = crate::teams::LeadState {
        state: Some(WorkerState::Idle),
        cwd: None,
        asleep: None,
    };
    page.cleanup = None;
    page.confirm_cleanup = false;
    page.tab = crate::teams::DetailTab::Overview;
    page.width.set(1180.0);
}

/// The sample team, finished, about to be cleaned up.
/// A run from before plain summaries: dense reports, no outcome from its lead,
/// and work it never got to.
fn sample_stopped_early(page: &mut crate::teams::TeamsView) {
    use dragent_host::team::{TaskResult, TaskStatus, TeamStatus};
    let dense = [
        "Scaffolded invite service on branch invite-api-t1-1b4f (commit d089dff). Fresh package.json (zod ~3.25.1, resend 4.7.1, drizzle 0.44.2); dropped nodemailer, bull. Router layout (index + accept), token contract at src/invites/types.ts. Legacy src files removed (recoverable from master). Did not touch app/accept.tsx (later tasks).",
        "Committed a4c51f0: token signer + pure expiry reducer + standalone mailer, all five ordered states, unified limits, percentage progress, reset handling, adapter contract. Only five owned files changed. 30 tests pass and strict owned-file typecheck passes using isolated /tmp tooling.",
        "t3: added app/accept.tsx (signed link check + expired state wired to invite engine, 325px layout preserved, Alert+router.back on success), src/components/InviteCard.tsx. Committed a3a514a. Isolated strict tsc clean on owned files, engine tests 30/30 pass.",
    ];
    if let Some(team) = page.team.as_mut() {
        team.status = TeamStatus::Stopped;
        team.messages.clear();
        for (index, task) in team.tasks.iter_mut().enumerate() {
            task.reason = None;
            if let Some(report) = dense.get(index) {
                task.status = TaskStatus::Done;
                task.result = Some(TaskResult {
                    summary: report.to_string(),
                    files: vec!["src/invites/types.ts".into(), "app/accept.tsx".into()],
                    checks: vec!["npm test: 30 passed".into()],
                    ..Default::default()
                });
            } else {
                task.status = TaskStatus::Cancelled;
                task.result = None;
            }
        }
    }
    page.teams[0].status = dragent_host::team::TeamStatus::Stopped;
    page.replies.lead = Some(
        "The invite flow works end to end: people get a signed link by email and can accept it. \
         Expiry emails and the final QA pass didn't run before this was stopped.\n\n\
         Start with the accept screen if you pick this up again."
            .to_string(),
    );
}

fn sample_finished(page: &mut crate::teams::TeamsView) {
    use dragent_host::team::{TaskResult, TaskStatus, TeamStatus};
    if let Some(team) = page.team.as_mut() {
        team.status = TeamStatus::Done;
        team.outcome = Some(
            "Teammates can now invite people by email. Each invite is a signed link that expires \
             after a week, and the new page shows who is still pending. Everything is uncommitted \
             in your checkout, ready for you to try and commit."
                .to_string(),
        );
        for task in &mut team.tasks {
            task.status = TaskStatus::Done;
            task.reason = None;
            if task.qa {
                task.result = Some(TaskResult {
                    summary: "Invited, accepted and expired invites as a user; edge cases and all 212 tests pass.".into(),
                    verdict: Some(dragent_host::team::Verdict::Approve),
                    ..Default::default()
                });
            } else if task.result.is_none() {
                task.result = Some(TaskResult {
                    summary: format!("{} works, and its checks pass.", task.title),
                    details: Some("Uses the shared mailer; no new dependencies.".to_string()),
                    files: vec!["src/invites/page.tsx".into(), "src/invites/api.ts".into()],
                    checks: vec!["npm test: 212 passed".into()],
                    ..Default::default()
                });
            }
        }
        team.messages.clear();
    }
    page.teams[0].status = dragent_host::team::TeamStatus::Done;
    page.teams[0].done = 6;
    page.teams[0].running = 0;
    page.teams[0].blocked = 0;
    page.teams[0].needs_user = 0;
    page.workers.clear();
    let worktree = |name: &str| {
        std::path::PathBuf::from(format!("/Users/ada/.dragent/worktrees/acme-app/{name}"))
    };
    page.cleanup = Some(crate::teams::Cleanup {
        remove: vec![
            ("t1".into(), worktree("invite-api-t1-1b4f")),
            ("t2".into(), worktree("invite-page-t2-1b4f")),
        ],
        dirty: vec![("t4".into(), worktree("expiry-emails-t4-1b4f"))],
        keep: vec![("t5".into(), worktree("integrate-t5-1b4f"))],
    });
    page.confirm_cleanup = true;
}

fn sample_links() -> Vec<PromptLink> {
    let link = |key: &str, title: &str, detail: &str| PromptLink {
        kind: LinkKind::Item(Provider::Linear),
        identifier: key.into(),
        title: title.into(),
        url: format!("https://linear.app/acme/issue/{key}"),
        body: String::new(),
        number: None,
        head_branch: None,
        branch: None,
        detail: Some(detail.into()),
        reference: None,
    };
    vec![
        link(
            "ENG-128",
            "Retry failed uploads with backoff",
            "Engineering · In Progress",
        ),
        link(
            "ENG-131",
            "Checkout page shows the wrong currency for EU customers",
            "Engineering · Todo",
        ),
        link(
            "DES-14",
            "New empty state for the inbox",
            "Design · Backlog",
        ),
        link(
            "ENG-97",
            "Flaky test: sync resumes after reconnect",
            "Engineering · Todo",
        ),
    ]
}

impl Entity for Preview {
    type Event = ();
}

impl View for Preview {
    fn ui_name() -> &'static str {
        "UiPreview"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let scene = SCENES[self.scene].1;
        let theme = match scene {
            Scene::ComputerAsk { dark: false }
            | Scene::Page { dark: false }
            | Scene::NewTabMenu { dark: false }
            | Scene::PhoneCover { dark: false, .. }
            | Scene::Teams { dark: false }
            | Scene::Demo(demo::DemoScene::SettingsAfter(SettingsSection::Mobile, "live_light")) => {
                Theme::light()
            }
            _ => Theme::dark(),
        };
        let hover = |key: &str| self.hover(key);
        let model = Model {
            ui: self.ui,
            theme: &theme,
            view_id: self.view_id,
            state: &self.integrations,
            hover: &hover,
            columns: crate::settings::pages::integrations::grid_columns(900.0),
        };
        let content: Box<dyn Element> = match scene {
            Scene::WorkspaceActivity { dark } => {
                let theme = if dark { Theme::dark() } else { Theme::light() };
                crate::workspace::workspace_activity::preview(self.ui, &theme)
            }
            Scene::CoordinationSidebar { dark } => {
                let theme = if dark { Theme::dark() } else { Theme::light() };
                crate::workspace::panel_coordination_preview(self.ui, &theme, self.view_id)
            }
            Scene::MarketingSidebar => crate::workspace::panel_coordination::marketing_sidebar(
                self.ui,
                // The clip was recorded in Onyx.
                &Theme::all_infos().into_iter().find(|info| info.id == "onyx").map(|info| info.theme).unwrap_or_else(Theme::dark),
                self.view_id,
                &self.search,
                (252.0, 900.0),
            ),
            Scene::EffortSlider { dark } => {
                let theme = if dark { Theme::dark() } else { Theme::light() };
                crate::terminal::view::effort_slider_preview(self.ui, &theme)
            }
            Scene::UsagePopover(tab) => {
                use crate::workspace::PromptAgent;
                let agent = [PromptAgent::Claude, PromptAgent::Codex, PromptAgent::OpenCode][tab.min(2)];
                crate::workspace::usage_popover_preview(self.ui, &Theme::from_id("github_dark"), self.view_id, agent)
            }
            Scene::HoverCard { dark } => {
                let theme = if dark { Theme::dark() } else { Theme::light() };
                crate::workspace::hover_card_preview(self.ui, &theme, self.view_id)
            }
            Scene::AgentTranscript { narrow } => self.agent_transcript_preview(narrow),
            Scene::ComputerAsk { .. } => crate::workspace::render_computer_preview(
                &theme,
                self.ui,
                self.view_id,
                &hover,
                &self.scroll,
            ),
            Scene::GitSetup { gitlab, connected } => {
                use crate::setup::{
                    checklist_slide::{render_checklist, ChecklistViewState},
                    git_provider::GitProvider,
                    onboarding_flow::GitCliState,
                };
                let status = if connected {
                    GitCliState::Connected
                } else {
                    GitCliState::ReadyToConnect
                };
                render_checklist(
                    &theme,
                    self.ui,
                    self.view_id,
                    ChecklistViewState {
                        account_connected: true,
                        browser_auth_active: false,
                        auth_error: None,
                        sign_in_code: None,
                        provider: if gitlab {
                            GitProvider::Gitlab
                        } else {
                            GitProvider::Github
                        },
                        git: &status,
                    },
                    |key| self.hover(key),
                    self.hover("back"),
                    self.hover("finish"),
                )
            }
            Scene::Page { .. } | Scene::Dialog(_) => {
                let page = Container::new(
                    ConstrainedBox::new(page(&model, || warpui::elements::Empty::new().finish()))
                        .with_max_width(900.0)
                        .finish(),
                )
                .with_padding_left(48.0)
                .with_padding_right(48.0)
                .with_padding_top(36.0)
                .finish();
                match dialog(&model) {
                    Some(dialog) => Stack::new().with_child(page).with_child(dialog).finish(),
                    None => page,
                }
            }
            Scene::Teams { .. }
            | Scene::TeamsActivity
            | Scene::TeamsBoard { .. }
            | Scene::TeamsPlanning
            | Scene::TeamsMessage
            | Scene::TeamsScrolled
            | Scene::TeamsNarrow
            | Scene::TeamsLeadStopped
            | Scene::TeamsNoPlan
            | Scene::TeamsFinished | Scene::TeamsDelete | Scene::TeamsStoppedEarly
            | Scene::TeamsEmpty
            | Scene::TaskModelMenu => {
                let page=warpui::elements::ChildView::new(&self.teams).finish();
                if matches!(SCENES[self.scene].1,Scene::TeamsNarrow) {ConstrainedBox::new(page).with_width(1000.0).finish()}else{page}
            },
            Scene::Tasks { .. } => warpui::elements::ChildView::new(&self.tasks).finish(),
            Scene::CoordinationComposer { narrow, solo } => {
                let composer = crate::workspace::prompt_box(
                    &theme, self.ui, self.view_id, &self.search, PromptAgent::Dragent,
                    (!solo, &[PromptAgent::Claude, PromptAgent::Codex, PromptAgent::Gemini, PromptAgent::OpenCode, PromptAgent::Copilot]),
                    Some("Sonnet 4.6"), false, narrow,
                    crate::voice::VoiceUiState::Idle, 0, &self.hovers,
                );
                Container::new(Align::new(ConstrainedBox::new(composer)
                    .with_width(if narrow { 360.0 } else { 680.0 }).finish()).top_left().finish())
                    .with_padding_left(40.0).with_padding_top(60.0).finish()
            }
            Scene::ReviewSetup | Scene::OpenReviewWorkspace => {
                let review=crate::pull_requests::ReviewSetup{
                    target:crate::pull_requests::data::Target{root:"/demo/acme".into(),label:"Acme".into()},number:128,
                    head:"a1b2c3d4e5f6789012345678901234567890abcd".into(),source:"feat/team-invites".into(),base:"main".into(),
                    title:"Add secure team invitations".into(),url:"https://github.com/acme/app/pull/128".into(),gitlab:false,instructions:String::new(),launch_agent:true,
                };
                let mut review=self.review_setup.as_ref().unwrap_or(&review).clone();
                review.launch_agent=matches!(SCENES[self.scene].1,Scene::ReviewSetup);
                let modal=crate::workspace::render_review_setup(self.view_id,&theme,self.ui,&self.hovers,&review,&self.review_editor,
                    PromptAgent::Claude,"Sonnet 4.6",false,None);
                let mut stack=Stack::new().with_child(warpui::elements::ChildView::new(&self.prs).finish());
                stack.add_overlay_child(Container::new(Align::new(modal).finish()).with_background_color(warpui::color::ColorU::new(0,0,0,150)).finish());
                stack.finish()
            },
            Scene::PhoneCover { held, .. } => {
                use crate::realtime::phone_control::Control;
                let control = if held {
                    Control::Held { cols: 46, rows: 28 }
                } else {
                    Control::Phone { phone_name: "Ada's iPhone".into(), cols: 46, rows: 28 }
                };
                let lines = "~/dev/shop  main\n$ npm run dev\n\n> shop@0.1.0 dev\n> next dev\n\n  ▲ Next.js 15.1\n  - Local:   http://localhost:3000\n\n ✓ Ready in 1.4s\n ○ Compiling / ...\n ✓ Compiled / in 812ms\n GET / 200 in 941ms\n GET /favicon.ico 200 in 12ms";
                let terminal = Container::new(Align::new(
                    warpui::elements::Text::new(lines.to_string(), self.ui, crate::theme::text_size(13.0))
                        .with_color(theme.main_text())
                        .finish(),
                ).top_left().finish())
                .with_uniform_padding(18.0)
                .with_background_color(theme.background)
                .finish();
                let cover = crate::terminal::view::terminal_overlays::phone_cover_copy(&control).expect("covered");
                let veil = crate::terminal::view::terminal_overlays::render_phone_cover(theme, self.ui, self.ui, cover, self.hover("phone_cover"), |_| {});
                Box::new(Stack::new().with_child(terminal).with_child(veil)) as Box<dyn Element>
            }
            Scene::CommandTerminal { remote } => {
                let samples = if remote {
                    "$ glab auth login\n\n? What GitLab instance do you want to log into? GitLab.com\n? How would you like to log in? Device\n\nOpen the displayed URL in your browser to continue.\n\nSample terminal output · no command is executed in this preview"
                } else {
                    "$ gh auth login --hostname github.com --git-protocol https --web\n\n! Copy your one-time code: ABCD-EFGH\nPress Enter to open github.com in your browser…\n\nSample terminal output · no command is executed in this preview"
                };
                let content=Container::new(warpui::elements::Text::new(samples.to_string(),self.ui,crate::theme::text_size(12.5)).with_color(theme.main_text()).finish()).with_uniform_padding(16.0).finish();
                let modal=crate::workspace::render_embedded_terminal_modal(
                    self.view_id,&theme,self.ui,&self.hovers,
                    if remote { "Sign in to GitLab" } else { "Sign in to GitHub" },"",
                    if remote { "Running on SSH machine production. Answer prompts here, then close this window to return to the page." } else { "Running on this computer. Answer prompts here, then close this window to return to the page." },
                    Some(content),false,None,false,false,
                );
                let mut stack=Stack::new().with_child(warpui::elements::ChildView::new(&self.prs).finish());
                stack.add_overlay_child(Container::new(Align::new(modal).finish()).with_background_color(warpui::color::ColorU::new(0,0,0,150)).finish());
                stack.finish()
            },
            Scene::PullRequests(scene) => {
                let child=warpui::elements::ChildView::new(&self.prs).finish();
                if scene==7 || scene==12 || scene==33 {ConstrainedBox::new(child).with_width(if scene==12 || scene==33{760.0}else{1000.0}).finish()}else{child}
            },
            Scene::Demo(scene) => self.demo.render(scene, self, app),
            Scene::LeadModelMenu => {
                let choices = vec![("sonnet".into(), "Sonnet".into()), ("opus".into(), "Opus".into())];
                Container::new(Align::new(crate::teams::models::picker(
                    &theme, self.ui, self.view_id, &self.hovers, &self.scroll, &self.search,
                    "", PromptAgent::Claude, Some("sonnet"), &choices,
                    |model| crate::workspace::WorkspaceAction::SetPromptLeadModel { model },
                    crate::workspace::WorkspaceAction::ClosePromptMenus,
                    crate::workspace::WorkspaceAction::ClearPromptAgentSearch,
                    Some(crate::teams::models::prompt_panel(PromptAgent::Claude,
                        &[PromptAgent::Claude, PromptAgent::Codex, PromptAgent::Gemini, PromptAgent::OpenCode].into(),
                        app, crate::teams::models::ModelTab::Agent(PromptAgent::Claude))),
                )).top_left().finish()).with_padding_left(60.0).with_padding_top(60.0).finish()
            }
            Scene::TeamMenu => {
                let installed: HashSet<PromptAgent> = [
                    PromptAgent::Claude,
                    PromptAgent::Codex,
                    PromptAgent::Gemini,
                    PromptAgent::OpenCode,
                    PromptAgent::Copilot,
                ]
                .into();
                let menu_for = |menu: PromptMenu| {
                    crate::workspace::render_prompt_menu(
                        self.view_id,
                        &theme,
                        self.ui,
                        &self.hovers,
                        &self.scroll,
                        menu,
                        PromptAgent::Claude,
                        (
                            true,
                            &[PromptAgent::Codex],
                            dragent_host::team::Cost::Balanced,
                        ),
                        &[],
                        None,
                        None,
                        &installed,
                        false,
                        "This Mac",
                        false,
                        &[],
                        &[],
                        None,
                        None,
                        &[],
                        None,
                        &self.search,
                        "",
                        &self.search,
                        "",
                        &self.search,
                        &[],
                        &[],
                        false,
                        None,
                        false,
                        &[],
                    )
                };
                // The agent menu beside it: the two should look alike.
                let menu = warpui::elements::Flex::row()
                    .with_spacing(48.0)
                    .with_child(menu_for(PromptMenu::Agent))
                    .with_child(menu_for(PromptMenu::Team))
                    .finish();
                Container::new(Align::new(menu).top_left().finish())
                    .with_padding_left(60.0)
                    .with_padding_top(60.0)
                    .finish()
            }
            Scene::NotificationBell { all_read } => {
                use warpui::elements::{CrossAxisAlignment, Flex};
                let notice = |id: &str, name: &str, detail: &str| crate::workspace::AutomationNotice {
                    run_id: id.into(),
                    automation_id: id.into(),
                    name: name.into(),
                    detail: detail.into(),
                };
                let notices = if all_read {
                    Vec::new()
                } else {
                    vec![
                        notice("a", "Morning dependency check", "Finished · 5m"),
                        notice("b", "Nightly test run", "Failed · 2h"),
                        notice("c", "Triage new issues", "Waiting for you · 3h"),
                    ]
                };
                let bell = crate::workspace::render_attention_bell(
                    self.ui,
                    &theme,
                    self.view_id,
                    self.hovers.borrow_mut().entry("preview_bell".to_string()).or_default().clone(),
                    0,
                    notices.len(),
                    false,
                );
                let (menu, _, _) = crate::workspace::attention_menu_overlay(
                    &[],
                    &notices,
                    None,
                    &self.review_editor,
                    "",
                    &theme,
                    self.ui,
                    self.view_id,
                );
                Container::new(
                    Align::new(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::End)
                            .with_spacing(6.0)
                            .with_child(bell)
                            .with_child(menu)
                            .finish(),
                    )
                    .top_left()
                    .finish(),
                )
                .with_padding_left(60.0)
                .with_padding_top(40.0)
                .finish()
            }
            Scene::NewTabMenu { .. } => {
                let menu = crate::workspace::render_header_new_tab_menu(
                    self.view_id,
                    &theme,
                    self.ui,
                    &self.hovers,
                    false,
                );
                Container::new(Align::new(menu).top_left().finish())
                    .with_padding_left(60.0)
                    .with_padding_top(60.0)
                    .finish()
            }
            Scene::AddMenu | Scene::Results => {
                let results = if scene == Scene::Results {
                    sample_links()
                } else {
                    Vec::new()
                };
                let linked = if scene == Scene::Results {
                    vec![results[0].clone()]
                } else {
                    Vec::new()
                };
                let menu = crate::workspace::render_prompt_menu(
                    self.view_id,
                    &theme,
                    self.ui,
                    &self.hovers,
                    &self.scroll,
                    if scene == Scene::Results {
                        PromptMenu::Links(LinkKind::Item(Provider::Linear))
                    } else {
                        PromptMenu::Add
                    },
                    PromptAgent::Claude,
                    (false, &[], dragent_host::team::Cost::Balanced),
                    &[],
                    None,
                    None,
                    &HashSet::new(),
                    false,
                    "This Mac",
                    false,
                    &[],
                    &[],
                    None,
                    None,
                    &[],
                    None,
                    &self.search,
                    "",
                    &self.search,
                    "",
                    &self.search,
                    &linked,
                    &results,
                    false,
                    None,
                    false,
                    &[
                        LinkKind::Item(Provider::Github),
                        LinkKind::Item(Provider::Linear),
                        LinkKind::Item(Provider::Sentry),
                        LinkKind::Item(Provider::Slack),
                    ],
                );
                Container::new(Align::new(menu).top_left().finish())
                    .with_padding_left(60.0)
                    .with_padding_top(60.0)
                    .finish()
            }
        };
        Container::new(content)
            .with_background_color(theme.background)
            .finish()
    }
}

impl TypedActionView for Preview {
    type Action = crate::settings::SettingsAction;

    fn handle_action(&mut self, _: &Self::Action, _: &mut ViewContext<Self>) {}
}

impl Preview {
    fn agent_transcript_preview(&self, narrow: bool) -> Box<dyn Element> {
        use crate::agent::controller::block::{AgentBlock, AgentBlockKind as Kind, ToolStepStatus};
        use warpui::elements::{Flex, CrossAxisAlignment, ClippedScrollable, ScrollbarWidth};
        let theme = Theme::default();
        let make = |id, kind| AgentBlock::new(id, uuid::Uuid::nil(), kind);
        let tool = |id, name: &str, args: &str, output: &str, expanded| make(id, Kind::ToolStep {
            tool_id: format!("preview-{id}"), name: name.into(), icon: String::new(),
            args_summary: args.into(), status: ToolStepStatus::Done,
            result_summary: Some(output.into()), duration_ms: Some(340), expanded, diff: None,
        });
        let blocks = vec![
            make(1, Kind::UserQuery { text: "Fix the checkout redirect and verify the payment flow.".into(), author: None, attached_blocks: Vec::new(), attachments: Vec::new(), checkpoint: None }),
            make(2, Kind::TurnGroup { expanded: true, started_at_ms: 0, duration_ms: Some(12400), status_note: None, children: vec![
                make(3, Kind::AgentMessage { text: "I found the redirect happens before the payment result is saved. I’ll check the handler, then run the focused tests.".into(), is_streaming: false }),
                tool(4, "Read File", "src/checkout/payment.ts", "Read 148 lines", false),
                tool(13, "Search Files", "pattern=redirectAfterPayment", "src/checkout/payment.ts:42: redirectAfterPayment(order)", false),
                tool(5, "Run Command", "pnpm test checkout --runInBand", "PASS checkout/payment.test.ts\n  ✓ persists the payment before redirecting\n  ✓ keeps declined payments on the checkout page\nwarning: src/legacy/cart.ts uses a deprecated API\n\nTests: 2 passed, 0 failed, 2 total\nTime: 1.24 s", true),
                make(6, Kind::AgentMessage { text: "The payment is now saved before navigation. Both checkout tests pass.\n\n**Changed:** `src/checkout/payment.ts`\n\nThe failed-payment path still keeps the user on checkout.\n\n- `src/checkout/redirect.ts:12-30`: the old redirect helper is now unused.".into(), is_streaming: false }),
            ]}),
            make(8, Kind::UserQuery { text: "hello".into(), author: None, attached_blocks: Vec::new(), attachments: Vec::new(), checkpoint: None }),
            make(9, Kind::TurnGroup { expanded: true, started_at_ms: 0, duration_ms: Some(6000), status_note: None, children: vec![
                make(10, Kind::AgentMessage { text: "Hello! How can I help?".into(), is_streaming: false }),
            ]}),
            make(11, Kind::Empty),
            make(12, Kind::UserQuery { text: "run the review again".into(), author: None, attached_blocks: Vec::new(), attachments: Vec::new(), checkpoint: None }),
            make(7, Kind::ApprovalRequest { action_id: uuid::Uuid::nil(), command: "pnpm install".into(), reason: "Install the updated dependencies before running the full suite. This runs in the background.".into(), status: crate::agent::controller::block::ApprovalStatus::Pending }),
        ];
        let shimmers = RefCell::new(HashMap::new());
        let diagrams = RefCell::new(HashMap::new());
        let context = crate::terminal::render::BlockRenderContext {
            entry_hint: None, link_hint: None, hovered_link: None, dir_hover: Default::default(),
            mouse_states: &self.hovers, shimmer_states: &shimmers, diagram_states: &diagrams,
            content_width_px: if narrow { 430.0 } else { 850.0 }, theme, mono: self.code,
            dir_sort: Default::default(), dir_view: Default::default(),
            feedback_editor: None, question_editor: None, block_filter_editor: None, block_filter_match_index: 0, copied_key: None,
        };
        // Laid out as `render_agent_transcript` lays out the real conversation.
        use crate::terminal::render::agent_transcript::{renders_nothing, MESSAGE_GAP};
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(MESSAGE_GAP);
        for block in blocks.iter().filter(|block| !renders_nothing(block)) { column = column.with_child(crate::terminal::render::build_agent_block_element(block, &context, theme, self.code, &HashSet::new())); }
        let edge = crate::theme::TERMINAL_PADDING_LEFT;
        let body = Container::new(column.finish()).with_padding_left(edge).with_padding_right(edge).with_padding_top(16.0).with_padding_bottom(24.0).with_background_color(theme.background).finish();
        let body = ConstrainedBox::new(body).with_width(if narrow { 490.0 } else { 950.0 }).finish();
        ClippedScrollable::vertical(self.scroll.clone(), Align::new(body).top_center().finish(), ScrollbarWidth::Auto, theme.sub_text().into(), theme.main_text().into(), theme.background.into()).finish()
    }
}
