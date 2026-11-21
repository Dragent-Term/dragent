//! Sample-only capture fixtures. These views never sign in, dial SSH, run a
//! scheduler, or execute an agent. Feature pages use the production renderers.
use super::*;
use crate::settings::{SettingsSection, SettingsView};
use crate::workspace::project_sidebar::{
    GitPanelState, ProjectSidebarModel, ProjectSidebarView, ReviewCheck, ReviewCheckStatus,
    ReviewComment, ReviewPrState, ReviewPullRequest, SidebarTab,
};
use warpui::elements::{ChildView, CrossAxisAlignment, Expanded, Flex, MainAxisSize, Text};
use warpui::{UpdateModel, ViewContext};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DemoScene {
    Terminal,
    Solo,
    SoloBusy,
    Coordinate,
    Machines,
    MachineSystem,
    MachineAgents,
    Editor,
    EditorFind {
        matches: usize,
    },
    Diff,
    Review,
    /// The sidebar's Search tab with every field filled in.
    SidebarSearch,
    /// Source Control with its file filter filled in.
    SidebarGitFilter,
    Automations,
    Accounts,
    Models,
    Mcp,
    Skills,
    Notifications,
    Appearance,
    Privacy,
    /// A settings section with the editors it creates when opened. Its other
    /// on-open work (refreshes, account fetches) is skipped: some of it starts
    /// background services by relaunching the current executable, which here
    /// is the preview itself.
    Settings(SettingsSection),
    /// A settings section in a named state (see [`settings_state`]), e.g. with
    /// a picker or an editor open.
    SettingsAfter(SettingsSection, &'static str),
}

fn sample_projects() -> Vec<crate::settings::ProjectSettingsEntry> {
    let project = |name: &str, path: &str, machine: &str, workspaces, hidden| {
        crate::settings::ProjectSettingsEntry {
            root: PathBuf::from(path),
            name: name.into(),
            path: path.into(),
            machine: machine.into(),
            remote: machine != "This computer",
            workspaces,
            groups: 0,
            hidden,
            in_sidebar: !hidden,
        }
    };
    vec![
        project("acme-app", "/demo/code/acme-app", "This computer", 3, false),
        project(
            "design-system",
            "/demo/code/design-system",
            "This computer",
            1,
            false,
        ),
        project(
            "marketing-site",
            "/demo/code/marketing-site",
            "This computer",
            0,
            true,
        ),
        project(
            "payments-api",
            "/srv/payments-api",
            "Development server",
            2,
            false,
        ),
    ]
}

/// The clicks that put a settings page into a named preview state.
fn settings_state(state: &str) -> Vec<crate::settings::SettingsAction> {
    use crate::settings::agent_accounts::AccountAction;
    use crate::settings::integrations::{IntegrationAction, IntegrationsTab};
    use crate::settings::SettingsAction as A;
    match state {
        // The Add account dialog, as the person sees it: subscription…
        "accounts_add" => vec![A::AgentAccount(AccountAction::StartAdd("claude".into()))],
        // …and with API-key billing picked.
        "accounts_add_apikey" => vec![
            A::AgentAccount(AccountAction::StartAdd("claude".into())),
            A::AgentAccount(AccountAction::SetKind(
                dragent_host::agent_accounts::CredentialKind::ApiKey,
            )),
        ],
        "rebind" => vec![A::StartRebind("new-tab".into())],
        "custom_prefix" => vec![A::SelectBranchPrefix(crate::settings::BranchPrefix::Custom)],
        "integrations_services" => vec![A::Integration(IntegrationAction::Tab(
            IntegrationsTab::Services,
        ))],
        "integrations_tasks" => vec![A::Integration(IntegrationAction::Tab(
            IntegrationsTab::Tasks,
        ))],
        "integrations_agents" => vec![A::Integration(IntegrationAction::Tab(
            IntegrationsTab::Agents,
        ))],
        "voice_legacy" => Vec::new(),
        "voice_system" => vec![A::SelectVoiceBackend(
            crate::settings::voice_settings::VoiceBackend::System,
        )],
        "mcp_form" => vec![A::ShowMcpForm(None)],
        "behavior_agents" => Vec::new(),
        "behavior_picker" => vec![A::TogglePicker("default_agent".into())],
        "computer_reset" => vec![A::RequestComputerPermissionReset],
        "skills_menu" => crate::agent::skill_discovery::skills_for_scope(None)
            .first()
            .map(|skill| {
                vec![
                    A::UpdateProviderKey("skill_search_query".into(), skill.name.clone()),
                    A::TogglePicker(format!("skill-menu:{}", skill.source_path.display())),
                ]
            })
            .unwrap_or_default(),
        "skills_all" => vec![A::ToggleSkillGroup("shared".into())],
        "skills_search" => vec![A::UpdateProviderKey(
            "skill_search_query".into(),
            "computer".into(),
        )],
        "skills_details" => vec![
            A::UpdateProviderKey("skill_search_query".into(), "dragent-computer".into()),
            A::ToggleSkillDetails(
                crate::agent::skill_discovery::built_in_skills()
                    .into_iter()
                    .find(|skill| skill.name == "dragent-computer")
                    .map(|skill| skill.source_path)
                    .unwrap_or_default(),
            ),
        ],
        "skills_empty" => vec![A::UpdateProviderKey(
            "skill_search_query".into(),
            "no-results-for-this-query".into(),
        )],
        "skills_create" => vec![A::ShowSkillCreator],
        "skills_create_error" => vec![A::ShowSkillCreator, A::SaveSkill],
        // Anything else names a picker to open.
        picker => vec![A::TogglePicker(picker.to_string())],
    }
}

pub(super) fn register(ctx: &mut AppContext) {
    // Sample agents found installed, for pages that list installed agents.
    crate::workspace::remember_installed_agents(
        &[
            PromptAgent::Claude,
            PromptAgent::Codex,
            PromptAgent::Gemini,
            PromptAgent::OpenCode,
        ]
        .into_iter()
        .collect(),
    );
    ctx.add_singleton_model(crate::auth::AuthManager::preview);
    ctx.add_singleton_model(crate::settings::OrgStore::new);
    ctx.add_singleton_model(crate::organisation_context::OrganisationContext::preview);
    ctx.add_singleton_model(crate::settings::machines_store::MachinesStore::preview);
    ctx.add_singleton_model(crate::automations::AutomationRuntime::preview);
    let store = crate::settings::machines_store::MachinesStore::handle(ctx);
    ctx.update_model(&store, |store, _| {
        use crate::settings::machines_store::*;
        use dragent_host::{
            machine_registry::Machine,
            remote::{Authentication, SshHost},
        };
        store.registry.machines = vec![Machine {
            id: "demo-server".into(),
            connection: SshHost {
                host: "192.0.2.42".into(),
                name: "Development server".into(),
                proxy_jump: None,
                user: Some("developer".into()),
                port: Some(22),
                identity_file: Some("~/.ssh/id_ed25519".into()),
                config_file: None,
                forward_agent: false,
                authentication: Authentication::Key,
            },
        }];
        let mut conn = MachineConnection::default();
        conn.ssh = SshState::Connected;
        conn.runtime = RuntimeState::Running;
        conn.version = Some(dragent_host::protocol::PROTOCOL_VERSION);
        conn.environment = Some(Environment {
            home: "/home/developer".into(),
            shell: "/bin/bash".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            tools: vec![
                ("git".into(), true),
                ("node".into(), true),
                ("claude".into(), true),
                ("codex".into(), true),
            ],
        });
        conn.metrics = Some(Metrics {
            cpu_percent: 12.0,
            memory_used: 3_000_000_000,
            memory_total: 16_000_000_000,
            disk_used: 24_000_000_000,
            disk_total: 128_000_000_000,
            uptime_seconds: 172_800,
        });
        store.set_preview_connection("demo-server".into(), conn);
    });
}

pub(super) struct DemoViews {
    settings: ViewHandle<SettingsView>,
    automations: ViewHandle<crate::automations::view::AutomationsView>,
    editor: ViewHandle<InputEditor>,
    prompt: ViewHandle<InputEditor>,
    review: ViewHandle<ProjectSidebarView>,
    mono: FamilyId,
}

const CODE: &str = "import { randomBytes, createHash } from 'node:crypto';\nimport { db } from './database';\nimport { sendEmail } from './mail';\n\n/** Invite a teammate without storing a reusable plaintext token. */\nexport async function inviteTeammate(email: string, organizationId: string) {\n  const normalizedEmail = email.trim().toLowerCase();\n  const token = randomBytes(32).toString('hex');\n  const tokenHash = createHash('sha256').update(token).digest('hex');\n\n  const invitation = await db.invitation.create({\n    data: {\n      email: normalizedEmail,\n      organizationId,\n      tokenHash,\n      expiresAt: new Date(Date.now() + 24 * 60 * 60 * 1000),\n    },\n  });\n\n  await sendEmail({\n    to: normalizedEmail,\n    subject: 'You are invited to Acme',\n    template: 'team-invitation',\n    variables: { invitationUrl: `/accept-invite?token=${token}` },\n  });\n\n  return { id: invitation.id, email: normalizedEmail };\n}\n";

impl DemoViews {
    pub(super) fn recording_prompt(&self) -> ViewHandle<InputEditor> {
        self.prompt.clone()
    }
    pub fn new(ctx: &mut ViewContext<Preview>, ui: FamilyId, mono: FamilyId, theme: Theme) -> Self {
        let id = ctx.view_id();
        let settings =
            ctx.add_typed_action_view(|c| SettingsView::new(c, id, ui, "/demo/acme-app".into()));
        ctx.update_view(&settings, |view, c| {
            view.theme_id = "dark".into();
            view.app_settings = crate::settings::AppSettings::default();
            view.projects = sample_projects();
            view.projects_org = dragent_paths::organisation_id();
            view.app_settings.git.open_in_apps = vec!["Cursor".into(), "VS Code".into()];
            view.integrations.statuses = sample_statuses();
            view.integrations.server = true;
            view.integrations.admin = true;
            {
                use crate::agent::mcp_agent_sync::SyncAgent;
                view.integrations.agents_present =
                    vec![SyncAgent::ClaudeCode, SyncAgent::Codex, SyncAgent::Gemini];
            }
            // One local machine with a subscription + an API-key account, and
            // one remote machine with its own account.
            let work = uuid::Uuid::from_u128(2);
            let keys = uuid::Uuid::from_u128(4);
            view.agent_accounts.selected = None;
            view.agent_accounts.snapshots.insert(
                String::new(),
                crate::settings::agent_accounts::MachineSnapshot {
                    profiles: vec![
                        dragent_host::development::AccountProfileView {
                            id: work,
                            provider: "claude".into(),
                            label: "Work".into(),
                            kind: dragent_host::agent_accounts::CredentialKind::Subscription,
                        },
                        dragent_host::development::AccountProfileView {
                            id: keys,
                            provider: "claude".into(),
                            label: "Keys".into(),
                            kind: dragent_host::agent_accounts::CredentialKind::ApiKey,
                        },
                    ],
                    defaults: [(
                        "claude".to_string(),
                        dragent_host::agent_accounts::Selection::Profile(work),
                    )]
                    .into_iter()
                    .collect(),
                    identities: vec![
                        dragent_host::development::AccountIdentityView {
                            target: Some(work),
                            provider: "claude".into(),
                            signed_in: true,
                            email: Some("me@example.com".into()),
                            plan: Some("max".into()),
                            kind: Some(dragent_host::agent_accounts::CredentialKind::Subscription),
                            error: None,
                            credentials_changed_at: Some(1_700_000_000),
                        },
                        dragent_host::development::AccountIdentityView {
                            target: Some(keys),
                            provider: "claude".into(),
                            signed_in: true,
                            email: None,
                            plan: None,
                            kind: Some(dragent_host::agent_accounts::CredentialKind::ApiKey),
                            error: None,
                            credentials_changed_at: Some(1_700_000_000),
                        },
                        dragent_host::development::AccountIdentityView {
                            target: None,
                            provider: "claude".into(),
                            signed_in: false,
                            email: None,
                            plan: None,
                            kind: None,
                            error: None,
                            credentials_changed_at: None,
                        },
                        dragent_host::development::AccountIdentityView {
                            target: None,
                            provider: "codex".into(),
                            signed_in: true,
                            email: Some("coder@example.com".into()),
                            plan: Some("plus".into()),
                            kind: None,
                            error: None,
                            credentials_changed_at: None,
                        },
                    ],
                    loading: false,
                    error: None,
                    not_connected: false,
                },
            );
            let remote_work = uuid::Uuid::from_u128(5);
            view.agent_accounts.snapshots.insert(
                "demo-server".to_string(),
                crate::settings::agent_accounts::MachineSnapshot {
                    profiles: vec![dragent_host::development::AccountProfileView {
                        id: remote_work,
                        provider: "claude".into(),
                        label: "Server".into(),
                        kind: dragent_host::agent_accounts::CredentialKind::Subscription,
                    }],
                    defaults: [(
                        "claude".to_string(),
                        dragent_host::agent_accounts::Selection::System,
                    )]
                    .into_iter()
                    .collect(),
                    identities: vec![dragent_host::development::AccountIdentityView {
                        target: Some(remote_work),
                        provider: "claude".into(),
                        signed_in: false,
                        email: None,
                        plan: None,
                        kind: Some(dragent_host::agent_accounts::CredentialKind::Subscription),
                        error: None,
                        credentials_changed_at: None,
                    }],
                    loading: false,
                    error: None,
                    not_connected: false,
                },
            );
            c.notify();
        });
        let automations = ctx.add_typed_action_view(|c| {
            crate::automations::view::AutomationsView::new(c, ui, mono, theme)
        });
        ctx.update_view(&automations, |view, c| {
            view.projects = vec![("/demo/acme-app".into(), "Acme app".into())];
            c.notify();
        });
        let editor = ctx.add_typed_action_view(|c| {
            InputEditor::new(
                "",
                mono,
                crate::theme::text_size(14.0),
                theme.main_text(),
                theme.sub_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "demo_source".into(),
                    max_height: 1000,
                },
                c.focus_handle(),
            )
        });
        ctx.update_view(&editor, |view, c| {
            view.set_text(CODE, c);
            view.set_document_path(Some(std::path::Path::new("src/invitations.ts")), c);
        });
        let prompt = super::editor(ctx, ui, theme, "Describe what you want to build…");
        let model = ctx.add_model(|_| ProjectSidebarModel::new());
        let review = ctx.add_typed_action_view(|c| {
            ProjectSidebarView::new(model, id, theme, ui, mono, Default::default(), c)
        });
        ctx.update_view(&review, |view,c| {
            view.sidebar_tab = SidebarTab::Review;
            view.review_pr = ReviewPrState { pr: Some(ReviewPullRequest { number: 128, title: "Add secure team invitations".into(), url:"https://github.com/example/acme/pull/128".into(), state:"open".into(), review_decision:"approved".into(), base_branch:"main".into(), head_branch:"feat/team-invites".into(), checks: vec![
                ReviewCheck {name:"Tests".into(),status:ReviewCheckStatus::Success,url:None,duration_text:Some("42s".into())},
                ReviewCheck {name:"Build".into(),status:ReviewCheckStatus::Success,url:None,duration_text:Some("18s".into())},
                ReviewCheck {name:"Lint".into(),status:ReviewCheckStatus::Success,url:None,duration_text:Some("8s".into())},
            ] }), comments:vec![ReviewComment { id:"demo-thread".into(),author:"Reviewer".into(),body:"Please cover expired invitations and make the acceptance action idempotent.".into(),path:Some("src/invitations.ts".into()),line:Some(16),kind:"review".into(),..Default::default()}], error:None,non_git_folder:false,loading_pr:false,loading_comments:false };
            c.notify();
        });
        Self {
            settings,
            automations,
            editor,
            prompt,
            review,
            mono,
        }
    }
    pub fn prepare(&mut self, scene: DemoScene, ctx: &mut ViewContext<Preview>) {
        if let DemoScene::Settings(section) | DemoScene::SettingsAfter(section, _) = scene {
            use warpui::TypedActionView as _;
            let actions = match scene {
                // Mobile states are sample data, not clicks: its actions call the server.
                DemoScene::SettingsAfter(SettingsSection::Mobile, _) => Vec::new(),
                DemoScene::SettingsAfter(_, state) => settings_state(state),
                _ => Vec::new(),
            };
            let auth = crate::auth::AuthManager::handle(ctx);
            ctx.update_model(&auth, |auth, _| {
                auth.preview_set_signed_in(matches!(
                    scene,
                    DemoScene::SettingsAfter(SettingsSection::Mobile, _)
                ));
            });
            ctx.update_view(&self.settings, |view, c| {
                view.section = section;
                if section == SettingsSection::Usage {
                    view.usage = crate::settings::usage::preview_state();
                }
                if let DemoScene::SettingsAfter(SettingsSection::Mobile, state) = scene {
                    view.preview_mobile_state(state, c);
                }
                view.open_picker = None;
                // A dialog from an earlier scene would cover this one.
                view.mcp_form = None;
                view.skill_draft = None;
                view.expanded_skill = None;
                view.skill_search_query.clear();
                view.skill_groups_expanded.clear();
                view.integrations.open = None;
                view.machines.page = crate::settings::machines::MachinePage::List;
                match section {
                    SettingsSection::General | SettingsSection::VersionControl => {
                        view.ensure_git_editors(c)
                    }
                    SettingsSection::ComputerUse => view.refresh_computer_permissions(c),
                    SettingsSection::CodeIndex => view.ensure_ignore_editor(c),
                    SettingsSection::Projects => view.keep_project_selected(c),
                    _ => {}
                }
                if matches!(scene, DemoScene::SettingsAfter(SettingsSection::Voice, "voice_legacy")) {
                    view.app_settings.voice.stt_model = "whisper-1".into();
                    view.app_settings.voice.user_models = vec!["custom-whisper-small".into()];
                }
                for action in &actions {
                    view.handle_action(action, c);
                }
                match scene {
                    DemoScene::SettingsAfter(SettingsSection::AgentSkills, "skills_search") => assert_eq!(view.skill_search_query, "computer", "Search action must reach the skill inventory"),
                    DemoScene::SettingsAfter(SettingsSection::AgentSkills, "skills_empty") => assert_eq!(view.skill_search_query, "no-results-for-this-query"),
                    DemoScene::SettingsAfter(SettingsSection::AgentSkills, "skills_all") => assert!(view.skill_groups_expanded.contains("shared")),
                    DemoScene::SettingsAfter(SettingsSection::AgentSkills, "skills_details") => assert!(view.expanded_skill.is_some()),
                    DemoScene::SettingsAfter(SettingsSection::AgentSkills, "skills_create_error") => assert!(view.skill_draft.is_some() && view.status_message.is_some(), "Invalid input must keep the form open and explain why it cannot be saved"),
                    _ => {}
                }
                if view.section == SettingsSection::AgentSkills {
                    let query = view.skill_search_query.clone();
                    c.update_view(&view.skill_search_editor, |editor, ec| editor.set_text(&query, ec));
                }
                // Behavior's agent sections sit below the fold.
                use warpui_core::units::IntoPixels as _;
                let behavior = matches!(scene, DemoScene::SettingsAfter(_, state) if state.starts_with("behavior"));
                view.content_scroll.scroll_to(if behavior { 880.0 } else { 0.0 }.into_pixels());
                c.notify();
            });
            return;
        }
        let section = match scene {
            DemoScene::Machines | DemoScene::MachineSystem | DemoScene::MachineAgents => {
                Some(SettingsSection::Machines)
            }
            DemoScene::Accounts => Some(SettingsSection::AgentAccounts),
            DemoScene::Models => Some(SettingsSection::AgentModels),
            DemoScene::Mcp => Some(SettingsSection::AgentMcp),
            DemoScene::Skills => Some(SettingsSection::AgentSkills),
            DemoScene::Notifications => Some(SettingsSection::Notifications),
            DemoScene::Appearance => Some(SettingsSection::Appearance),
            DemoScene::Privacy => Some(SettingsSection::Privacy),
            _ => None,
        };
        if let Some(section) = section {
            ctx.update_view(&self.settings, |view, c| {
                view.section = section;
                view.machines.page = if scene == DemoScene::Machines {
                    crate::settings::machines::MachinePage::List
                } else {
                    crate::settings::machines::MachinePage::Detail("demo-server".into())
                };
                view.machines.tab = if scene == DemoScene::MachineAgents {
                    crate::settings::machines::MachineTab::Agents
                } else {
                    crate::settings::machines::MachineTab::System
                };
                c.notify();
            });
        }
        if matches!(
            scene,
            DemoScene::Solo | DemoScene::SoloBusy | DemoScene::Coordinate
        ) {
            ctx.update_view(&self.prompt,|e,c| e.set_text("Add secure team invitations. Cover expiry, retries, and duplicate requests. Review the change and verify it end to end.",c));
        }
        if matches!(
            scene,
            DemoScene::SidebarSearch | DemoScene::SidebarGitFilter
        ) {
            ctx.update_view(&self.review, |view, c| {
                if scene == DemoScene::SidebarSearch {
                    view.sidebar_tab = SidebarTab::Search;
                    view.global_search_replace_open = true;
                    view.global_search_details_open = true;
                    for (editor, text) in [
                        (view.global_search_query_editor.clone(), "inviteTeammate"),
                        (view.global_search_replace_editor.clone(), "inviteMember"),
                        (view.global_search_include_editor.clone(), "src/**/*.ts"),
                        (view.global_search_exclude_editor.clone(), "node_modules"),
                    ] {
                        c.update_view(&editor, |e, ec| e.set_text(text, ec));
                    }
                } else {
                    view.sidebar_tab = SidebarTab::SourceControl;
                    let editor = view.git_filter_editor.clone();
                    c.update_view(&editor, |e, ec| e.set_text("invitations", ec));
                }
                c.notify();
            });
        }
        if let DemoScene::EditorFind { matches } = scene {
            ctx.update_view(&self.review, |view, c| {
                c.update_view(&view.buffer_search_editor, |editor, ec| {
                    editor.set_text("h", ec)
                });
                view.buffer_search_matches = (0..matches).map(|n| n..n + 1).collect();
                view.buffer_search_active = Some(if matches == 312 { 287 } else { matches - 1 });
                view.buffer_search_error = None;
                view.buffer_replace_open = true;
                c.notify();
            });
        }
        if matches!(
            scene,
            DemoScene::Editor | DemoScene::Diff | DemoScene::Review
        ) {
            ctx.update_view(&self.editor, |view, c| {
                view.set_text(CODE, c);
                if scene == DemoScene::Diff {
                    let before = CODE
                        .replace(
                            "  const tokenHash = createHash('sha256').update(token).digest('hex');",
                            "  const tokenHash = token;",
                        )
                        .replace("24 * 60 * 60 * 1000", "7 * 24 * 60 * 60 * 1000");
                    view.set_git_diff_projection(
                        Some((
                            dragent_document::DocumentDiff::between(&before, CODE),
                            before.lines().count(),
                            CODE.lines().count(),
                            true,
                        )),
                        c,
                    );
                } else {
                    view.set_git_diff_projection(None, c);
                }
            });
        }
    }
    pub fn render(
        &self,
        scene: DemoScene,
        preview: &Preview,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        if matches!(
            scene,
            DemoScene::Machines
                | DemoScene::MachineSystem
                | DemoScene::MachineAgents
                | DemoScene::Accounts
                | DemoScene::Models
                | DemoScene::Mcp
                | DemoScene::Skills
                | DemoScene::Notifications
                | DemoScene::Appearance
                | DemoScene::Privacy
                | DemoScene::Settings(_)
                | DemoScene::SettingsAfter(..)
        ) {
            return ChildView::new(&self.settings).finish();
        }
        if let DemoScene::EditorFind { .. } = scene {
            let theme = Theme::dark();
            let bar = _app.read_view(&self.review, |view, _| {
                crate::workspace::project_sidebar::render_buffer_search_bar(
                    view, &theme, preview.ui, self.mono,
                )
            });
            return Align::new(ConstrainedBox::new(bar).with_width(565.0).finish()).finish();
        }
        if scene == DemoScene::Automations {
            return ChildView::new(&self.automations).finish();
        }
        let theme = Theme::dark();
        let ui = preview.ui;
        let content = if matches!(
            scene,
            DemoScene::Solo | DemoScene::SoloBusy | DemoScene::Coordinate
        ) {
            crate::workspace::render_demo_project_prompt(crate::workspace::ProjectPromptProps {
                workspace_view_id: preview.view_id,
                theme: &theme,
                ui_font: ui,
                mono_font: self.mono,
                editor: &self.prompt,
                project_root: &PathBuf::from("/demo/acme-app"),
                project_name: "Acme app".into(),
                agent: PromptAgent::Claude,
                team_on: scene == DemoScene::Coordinate,
                team_agents: vec![PromptAgent::Codex, PromptAgent::OpenCode],
                lead_model: Some("Sonnet".into()),
                prompt_empty: false,
                busy: scene == DemoScene::SoloBusy,
                attachment_names: Vec::new(),
                attachment_images: Vec::new(),
                linked_items: Vec::new(),
                task: None,
                attachment_scroll: &preview.scroll,
                voice_ui: crate::voice::VoiceUiState::Idle,
                voice_spin: 0,
                notice: None,
                current_branch: Some("main".into()),
                worktree_name: "New worktree".into(),
                device_label: "This Mac".into(),
                machine: None,
                error: None,
                available_width: 900.0,
                mouse_states: &preview.hovers,
            })
        } else if matches!(
            scene,
            DemoScene::Editor
                | DemoScene::Diff
                | DemoScene::Review
                | DemoScene::SidebarSearch
                | DemoScene::SidebarGitFilter
        ) {
            let mut main = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(
                    ConstrainedBox::new(file_tree(ui, &theme))
                        .with_width(205.0)
                        .finish(),
                )
                .with_child(Expanded::new(1.0, ChildView::new(&self.editor).finish()).finish());
            if matches!(
                scene,
                DemoScene::Review | DemoScene::SidebarSearch | DemoScene::SidebarGitFilter
            ) {
                main = main.with_child(
                    ConstrainedBox::new(ChildView::new(&self.review).finish())
                        .with_width(365.0)
                        .finish(),
                );
            }
            main.finish()
        } else {
            terminal(ui, self.mono, &theme)
        };
        shell(ui, &theme, scene, preview.view_id, content)
    }
}
fn text(ui: FamilyId, s: &str, size: f32, color: warpui::color::ColorU) -> Box<dyn Element> {
    Text::new(s.to_owned(), ui, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}
fn file_tree(ui: FamilyId, t: &Theme) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0)
        .with_child(text(ui, "FILES     +  ↻", 12.0, t.sub_text()));
    for s in [
        "⌄  src",
        "    api",
        "    invitations.ts",
        "    accept-invite.ts",
        "    mail.ts",
        "⌄  tests",
        "    invitations.test.ts",
        "    acceptance.test.ts",
        "   package.json",
        "   README.md",
    ] {
        col = col.with_child(text(
            ui,
            s,
            13.0,
            if s.contains("invitations.ts") {
                t.accent
            } else {
                t.sub_text()
            },
        ));
    }
    Container::new(col.finish())
        .with_padding_left(18.0)
        .with_padding_top(18.0)
        .with_padding_right(10.0)
        .with_background_color(t.sidebar_background())
        .finish()
}
fn terminal(ui: FamilyId, mono: FamilyId, t: &Theme) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(18.0);
    for (cmd,out) in [
        ("git status --short"," M src/invitations.ts\n M tests/invitations.test.ts\n"),
        ("npm test -- invitations"," PASS  tests/invitations.test.ts\n  ✓ hashes invitation tokens\n  ✓ rejects expired invitations\n  ✓ accepts repeated requests safely\n\n Tests: 14 passed · 0 failed"),
        ("dragent agents running","Claude Code    lead     Waiting for you\nCodex          worker   Implementation finished\nOpenCode       review   Review approved"),
    ] {col=col.with_child(Container::new(Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(14.0)
        .with_child(text(mono,&format!("›  {cmd}                                                   ok"),13.0,t.accent))
        .with_child(text(mono,out,13.0,t.main_text())).finish()).with_background_color(t.surface_1()).with_padding_left(20.0).with_padding_right(20.0).with_padding_top(18.0).with_padding_bottom(18.0).finish());}
    col = col
        .with_child(Expanded::new(1.0, Flex::column().finish()).finish())
        .with_child(text(
            ui,
            "⌁  ~/projects/acme-app   ·   feat/team-invites",
            12.0,
            t.sub_text(),
        ))
        .with_child(text(
            mono,
            "›  Ask AI or run a command",
            15.0,
            t.disabled_text(),
        ));
    Container::new(col.finish())
        .with_padding_left(24.0)
        .with_padding_right(24.0)
        .with_padding_top(20.0)
        .with_padding_bottom(24.0)
        .finish()
}
fn shell(
    ui: FamilyId,
    t: &Theme,
    scene: DemoScene,
    view_id: EntityId,
    body: Box<dyn Element>,
) -> Box<dyn Element> {
    let title = match scene {
        DemoScene::Solo | DemoScene::SoloBusy | DemoScene::Coordinate => "New workspace",
        DemoScene::Editor => "invitations.ts",
        DemoScene::Diff => "Changes · invitations.ts",
        DemoScene::Review => "Review · secure team invitations",
        DemoScene::SidebarSearch => "Search",
        DemoScene::SidebarGitFilter => "Source control",
        _ => "Terminal",
    };
    let mut nav = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(20.0);
    for s in [
        "＋  New Workspace",
        "◷  Automations",
        "▱  Agent Coordination",
        "PROJECTS",
        "⌄  Acme app",
        "    feat/team-invites",
        "    fix/retry-errors",
        "⌄  Remote service",
        "    Development server",
    ] {
        nav = nav.with_child(text(ui, s, 13.0, t.sub_text()));
    }
    // The real footer (Star on GitHub, organisation, help, sign out).
    let states = std::cell::RefCell::new(std::collections::HashMap::new());
    let nav = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Expanded::new(
                    1.0,
                    Container::new(nav.finish())
                        .with_padding_left(20.0)
                        .with_padding_right(18.0)
                        .with_padding_top(22.0)
                        .finish(),
                )
                .finish(),
            )
            .with_child(crate::workspace::render_demo_org_footer(
                t, ui, view_id, &states,
            ))
            .finish(),
    )
    .with_background_color(t.sidebar_background())
    .finish();
    let center=Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_main_axis_size(MainAxisSize::Max)
        .with_child(Container::new(text(ui,&format!("{title}                                               Acme app    ·    Demo data"),13.0,t.sub_text())).with_padding_left(20.0).with_padding_top(13.0).with_padding_bottom(13.0).with_background_color(t.surface_1()).finish())
        .with_child(Expanded::new(1.0,body).finish()).finish();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(ConstrainedBox::new(nav).with_width(225.0).finish())
            .with_child(Expanded::new(1.0, center).finish())
            .finish(),
    )
    .with_background_color(t.background)
    .finish()
}
