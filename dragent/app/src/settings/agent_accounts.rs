//! Provider account settings, distinct from Dragent organisation sign-in.
use super::{SettingsAction, SettingsEvent, SettingsView};
use dragent_host::agent_accounts::{self, Registry, Selection};
use warpui::{ReadView, UpdateView, ViewContext, ViewHandle};
use crate::workspace::{InputEditor, InputMode};
use warpui::elements::{ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, Hoverable, MainAxisSize, ParentElement, Text};
use warpui_core::scene::{Border, CornerRadius, Radius};
use super::widgets::{page_header, primary_button, secondary_button};

#[derive(Clone, Debug)]
pub enum AccountAction {
    Select(String), Refresh, Add, Rename(uuid::Uuid), Remove(uuid::Uuid),
    Login(uuid::Uuid), Status(uuid::Uuid), Default { project: bool, selection: Selection },
    /// Open the name field to add an account for this agent.
    StartAdd(String),
    /// Open the name field to rename this account.
    StartRename(uuid::Uuid),
    /// Add or rename with what's in the name field.
    Save,
    CancelEdit,
    /// Make this account the agent's default.
    MakeDefault { provider: String, selection: Selection },
    /// Check the agent's own (system) login.
    CheckSystem(String),
}

/// The name field's job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Editing {
    Add(String),
    Rename(uuid::Uuid),
}

pub(crate) struct AccountsState {
    pub provider: String,
    pub registry: Registry,
    pub name: ViewHandle<InputEditor>,
    pub status: std::collections::HashMap<uuid::Uuid, String>,
    /// Each agent's own login: "Signed in", "Sign-in required", "Checking…".
    pub system: std::collections::HashMap<String, String>,
    /// Who each account is signed in as, keyed by a profile id or
    /// "system:<provider>".
    pub who: std::collections::HashMap<String, agent_accounts::Identity>,
    /// The latest limits reading of every account.
    pub usage: Vec<dragent_host::agent_usage::AccountUsage>,
    pub editing: Option<Editing>,
}

fn system_key(provider: &str) -> String {
    format!("system:{provider}")
}
impl AccountsState {
    pub fn new(ctx: &mut ViewContext<SettingsView>, font: warpui::fonts::FamilyId, theme: crate::theme::Theme) -> Self {
        let name = ctx.add_typed_action_view(|c| InputEditor::new("Account name, e.g. Work or Personal",font,crate::theme::scaled(13.0),theme.main_text(),theme.disabled_text(),theme.accent,theme,InputMode::Settings("agent_account_name".into()),c.focus_handle()));
        Self { provider: "claude".into(), registry: Registry::default(), name, status: Default::default(), system: Default::default(), who: Default::default(), usage: Vec::new(), editing: None }
    }
}
impl SettingsView {
    pub(crate) fn handle_agent_account(&mut self, action: &AccountAction, ctx: &mut ViewContext<Self>) {
        let provider = self.agent_accounts.provider.clone();
        let result = match action {
            AccountAction::Select(provider) => { self.agent_accounts.provider = provider.clone(); self.open_picker = None; self.refresh_account_statuses(ctx); Ok(()) }
            AccountAction::Refresh => { self.refresh_account_statuses(ctx); Ok(()) }
            AccountAction::Add => {
                let name = self.agent_accounts.name.read(ctx, |e,_| e.get_text().to_string());
                Registry::update(|r| r.add(&provider, &name)).map(|_| ())
            }
            AccountAction::Rename(id) => {
                let name = self.agent_accounts.name.read(ctx, |e,_| e.get_text().to_string());
                Registry::update(|r| r.rename(*id, &name))
            }
            AccountAction::Remove(id) => Registry::update(|r| r.remove(*id)),
            AccountAction::Default { project, selection } => Registry::update(|r| r.set_default(&provider, project.then_some(self.project_root.as_path()), selection.clone())),
            AccountAction::Login(id) => {
                let id = *id;
                // Immediate loading state — the native sign-in runs in a
                // terminal tab; the poller below refreshes the row when it lands.
                self.agent_accounts.status.insert(id, "Signing in…".into());
                ctx.emit(SettingsEvent::AgentAccountLogin(id));
                self.schedule_login_poll(id, 0, ctx);
                Ok(())
            }
            AccountAction::Status(id) => { self.check_account_status(*id, ctx); Ok(()) }
            AccountAction::StartAdd(provider) => {
                self.agent_accounts.provider = provider.clone();
                self.agent_accounts.editing = Some(Editing::Add(provider.clone()));
                ctx.update_view(&self.agent_accounts.name, |e, c| e.set_text("", c));
                Ok(())
            }
            AccountAction::StartRename(id) => {
                let label = self.agent_accounts.registry.profiles.iter().find(|p| p.id == *id).map(|p| p.label.clone()).unwrap_or_default();
                self.agent_accounts.editing = Some(Editing::Rename(*id));
                ctx.update_view(&self.agent_accounts.name, |e, c| e.set_text(&label, c));
                Ok(())
            }
            AccountAction::CancelEdit => { self.agent_accounts.editing = None; Ok(()) }
            AccountAction::Save => {
                let name = self.agent_accounts.name.read(ctx, |e,_| e.get_text().trim().to_string());
                let result = match self.agent_accounts.editing.clone() {
                    Some(Editing::Add(provider)) => Registry::update(|r| r.add(&provider, &name)).map(|_| ()),
                    Some(Editing::Rename(id)) => Registry::update(|r| r.rename(id, &name)),
                    None => Ok(()),
                };
                if result.is_ok() { self.agent_accounts.editing = None; }
                result
            }
            AccountAction::MakeDefault { provider, selection } => {
                let result = Registry::update(|r| r.set_default(provider, None, selection.clone()));
                self.load_account_usage(ctx);
                result
            }
            AccountAction::CheckSystem(provider) => { self.probe_system(provider.clone(), ctx); Ok(()) }
        };
        if let Err(error) = result { ctx.emit(SettingsEvent::AgentAccountError(error)); }
        match Registry::load() { Ok(registry) => self.agent_accounts.registry = registry, Err(error) => ctx.emit(SettingsEvent::AgentAccountError(error)) }
        ctx.notify();
    }

    /// Single status probe for manual Check — surfaces errors.
    fn check_account_status(&mut self, id: uuid::Uuid, ctx: &mut ViewContext<Self>) {
        self.probe_account_status(id, true, ctx);
    }

    /// Check an agent's own login and who it's signed in as.
    fn probe_system(&mut self, provider: String, ctx: &mut ViewContext<Self>) {
        self.agent_accounts.system.insert(provider.clone(), "Checking…".into());
        let key = provider.clone();
        ctx.spawn(async move {
            let binding = agent_accounts::Binding::system(&provider);
            agent_accounts::identity(&binding, std::path::Path::new(&provider))
        }, move |view, result: Result<agent_accounts::Identity, String>, ctx| {
            match result {
                Ok(who) => {
                    view.agent_accounts.system.insert(key.clone(), if who.signed_in { "Signed in" } else { "Sign-in required" }.into());
                    view.agent_accounts.who.insert(system_key(&key), who);
                }
                Err(_) => { view.agent_accounts.system.insert(key, "Not checked".into()); }
            }
            ctx.notify();
        });
    }

    /// Read every account's latest limits (files only).
    fn load_account_usage(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(async move { dragent_host::agent_usage::accounts() }, |view, usage, ctx| {
            view.agent_accounts.usage = usage;
            ctx.notify();
        });
    }

    /// Probe every profile of the selected provider (section open, agent
    /// switch). Silent on failure — rows fall back to “Not checked” instead
    /// of error modals. Skips rows mid-sign-in so the login poller keeps
    /// ownership of them.
    fn refresh_account_statuses(&mut self, ctx: &mut ViewContext<Self>) {
        if let Ok(registry) = Registry::load() {
            self.agent_accounts.registry = registry;
        }
        let providers: Vec<String> = account_agents().iter().map(|a| a.command_key().to_string()).collect();
        for provider in &providers {
            self.probe_system(provider.clone(), ctx);
        }
        self.load_account_usage(ctx);
        let ids: Vec<uuid::Uuid> = self.agent_accounts.registry.profiles.iter()
            .filter(|p| providers.contains(&p.provider))
            .map(|p| p.id)
            .collect();
        for id in ids {
            if self.agent_accounts.status.get(&id).is_some_and(|s| s == "Signing in…") {
                continue;
            }
            self.probe_account_status(id, false, ctx);
        }
    }

    /// Single probe. Loud (manual Check) surfaces errors; quiet (automatic
    /// refresh) falls back to “Not checked”.
    fn probe_account_status(&mut self, id: uuid::Uuid, loud: bool, ctx: &mut ViewContext<Self>) {
        self.agent_accounts.status.insert(id, "Checking…".into());
        ctx.spawn(async move {
            let registry = Registry::load()?;
            let profile = registry.profiles.iter().find(|p| p.id == id).ok_or("Account no longer exists")?;
            let binding = registry.resolve(&profile.provider, None, &Selection::Profile(id))?;
            agent_accounts::identity(&binding, std::path::Path::new(&profile.provider)).map(|who| (id, who))
        }, move |view, result: Result<(uuid::Uuid, agent_accounts::Identity), String>, ctx| {
            match result {
                Ok((id, who)) => {
                    let status = if who.signed_in { "Signed in" } else { "Sign-in required" };
                    view.agent_accounts.status.insert(id, status.into());
                    view.agent_accounts.who.insert(id.to_string(), who);
                }
                Err(error) => {
                    view.agent_accounts.status.remove(&id);
                    if loud { ctx.emit(SettingsEvent::AgentAccountError(error)); }
                }
            }
            ctx.notify();
        });
    }

    /// Re-check a signing-in account with backoff until it lands or we give
    /// up. Only continues while the row still says “Signing in…” so a manual
    /// Check/Remove always wins and nothing polls forever. Background misses
    /// stay silent — the row just falls back to “Not checked”.
    fn schedule_login_poll(&mut self, id: uuid::Uuid, attempt: u8, ctx: &mut ViewContext<Self>) {
        const DELAYS_SECS: [u64; 4] = [20, 30, 60, 120];
        let Some(wait) = DELAYS_SECS.get(attempt as usize) else {
            if self.agent_accounts.status.get(&id).is_some_and(|s| s == "Signing in…") {
                self.agent_accounts.status.insert(id, "Not checked".into());
                ctx.notify();
            }
            return;
        };
        let wait = *wait;
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(wait)),
            move |view, _, ctx| {
                if !view.agent_accounts.status.get(&id).is_some_and(|s| s == "Signing in…") {
                    return;
                }
                view.poll_login_attempt(id, attempt, ctx);
            },
        );
    }

    fn poll_login_attempt(&mut self, id: uuid::Uuid, attempt: u8, ctx: &mut ViewContext<Self>) {
        ctx.spawn(async move {
            let registry = Registry::load()?;
            let profile = registry.profiles.iter().find(|p| p.id == id).ok_or("Account no longer exists")?;
            let binding = registry.resolve(&profile.provider, None, &Selection::Profile(id))?;
            agent_accounts::status(&binding, std::path::Path::new(&profile.provider)).map(str::to_owned).map(|status| (id, status))
        }, move |view, result: Result<(uuid::Uuid, String), String>, ctx| {
            match result {
                Ok((id, status)) if status == "Signed in" => { view.agent_accounts.status.insert(id, status); ctx.notify(); }
                Ok((id, _)) => view.schedule_login_poll(id, attempt + 1, ctx),
                Err(_) => view.schedule_login_poll(id, attempt + 1, ctx),
            }
        });
    }
}
/// The installed agents whose logins Dragent can keep apart, as the agents
/// they are.
fn account_agents() -> Vec<crate::workspace::PromptAgent> {
    #[cfg(not(target_family = "wasm"))]
    let installed = crate::workspace::installed_agents();
    #[cfg(target_family = "wasm")]
    let installed: Vec<crate::workspace::PromptAgent> = Vec::new();
    installed
        .into_iter()
        .filter(|agent| agent_accounts::capability(agent.command_key()).isolated)
        .collect()
}

/// The agent menu under the Agent chip: the prompt screen's menu, listing
/// the installed agents with separate logins.
pub fn render_agent_menu(view: &SettingsView, theme: &crate::theme::Theme) -> Box<dyn Element> {
    let id = view.view_id;
    let agents: std::collections::HashSet<_> = account_agents().into_iter().collect();
    let selected = crate::workspace::PromptAgent::ALL
        .into_iter()
        .find(|agent| agent.command_key() == view.agent_accounts.provider);
    let menu = crate::workspace::render_agent_menu(crate::workspace::AgentMenuProps {
        theme,
        ui: view.ui_font,
        states: &view.hover_states,
        scroll: &view.agent_menu_scroll,
        search_editor: &view.agent_menu_search_editor,
        search: &view.agent_menu_search,
        selected,
        installed: &agents,
        checking: false,
        installed_only: true,
        with_dragent: false,
        pick: std::rc::Rc::new(move |ectx, agent, _| {
            ectx.dispatch_typed_action_for(
                id,
                SettingsAction::AgentAccount(AccountAction::Select(agent.command_key().to_string())),
            );
        }),
        clear_search: std::rc::Rc::new(move |ectx| {
            ectx.dispatch_typed_action_for(id, SettingsAction::ClearAgentMenuSearch);
        }),
    });
    warpui_core::elements::Dismiss::new(menu)
        .on_dismiss(move |ectx, _| {
            ectx.dispatch_typed_action_for(id, SettingsAction::TogglePicker("accounts_agent".to_string()))
        })
        .prevent_interaction_with_other_elements()
        .finish()
}

pub fn render(view: &SettingsView, theme: &crate::theme::Theme) -> Box<dyn Element> {
    let font = view.ui_font;
    let mut page = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(page_header(
            theme,
            font,
            "Agent Accounts",
            "Keep Work and Personal logins apart, see each account's limits, and choose which one new sessions use.",
        ));
    let supported = account_agents();
    if supported.is_empty() {
        return page
            .with_child(card(
                theme,
                false,
                Flex::column()
                    .with_spacing(6.0)
                    .with_child(text(theme, font, "No agent with separate logins is installed", 14.0, theme.main_text()))
                    .with_child(text(
                        theme,
                        font,
                        "Claude Code and Codex can keep Work and Personal logins apart. Install one from the new-workspace screen's agent menu.",
                        12.5,
                        theme.sub_text(),
                    ))
                    .finish(),
            ))
            .finish();
    }
    for agent in supported {
        page = page.with_child(agent_section(view, theme, agent));
    }
    page.with_child(
        Container::new(text(
            theme,
            font,
            "New sessions use the default account. You can pick another one when you start a session.",
            12.0,
            theme.sub_text(),
        ))
        .with_padding_top(4.0)
        .finish(),
    )
    .finish()
}

/// One agent: its name with "Add account", then a card per account.
fn agent_section(view: &SettingsView, theme: &crate::theme::Theme, agent: crate::workspace::PromptAgent) -> Box<dyn Element> {
    let font = view.ui_font;
    let state = &view.agent_accounts;
    let provider = agent.command_key().to_string();
    let id = view.view_id;
    let add = {
        let provider = provider.clone();
        let theme = *theme;
        Hoverable::new(view.hover(&format!("accounts-add-{provider}")), move |hover| {
            let color = if hover.is_hovered() { theme.main_text() } else { theme.sub_text() };
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(crate::icons::render_icon(crate::icons::AppIcon::Plus, color, 13.0))
                .with_child(Text::new("Add account".to_string(), font, crate::theme::text_size(12.5)).with_color(color).finish())
                .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, SettingsAction::AgentAccount(AccountAction::StartAdd(provider.clone())))
        })
        .finish()
    };
    let header = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(crate::workspace::agent_image(agent, theme, 18.0, 1.0))
        .with_child(text(theme, font, agent.label(), 15.0, theme.main_text()))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(add)
        .finish();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(header);
    if let Some(form) = name_form(view, theme, &provider) {
        column = column.with_child(form);
    }

    let default = state.registry.defaults.get(&provider).cloned().unwrap_or(Selection::System);
    let mut cards: Vec<Box<dyn Element>> = vec![account_card(view, theme, &provider, None, matches!(default, Selection::System | Selection::Inherit))];
    for profile in state.registry.profiles.iter().filter(|p| p.provider == provider) {
        cards.push(account_card(view, theme, &provider, Some(profile), default == Selection::Profile(profile.id)));
    }
    // Two cards a row.
    let mut cards = cards.into_iter().peekable();
    while let Some(first) = cards.next() {
        let second = cards.next().unwrap_or_else(|| Empty::new().finish());
        column = column.with_child(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0)
                .with_child(Expanded::new(1.0, first).finish())
                .with_child(Expanded::new(1.0, second).finish())
                .finish(),
        );
    }
    Container::new(column.finish()).with_padding_bottom(28.0).finish()
}

/// The name field, when this agent is adding or renaming an account.
fn name_form(view: &SettingsView, theme: &crate::theme::Theme, provider: &str) -> Option<Box<dyn Element>> {
    let state = &view.agent_accounts;
    let font = view.ui_font;
    let title = match state.editing.as_ref()? {
        Editing::Add(p) if p == provider => "New account".to_string(),
        Editing::Rename(id) => {
            let profile = state.registry.profiles.iter().find(|p| p.id == *id && p.provider == provider)?;
            format!("Rename “{}”", profile.label)
        }
        _ => return None,
    };
    let saving = if matches!(state.editing, Some(Editing::Add(_))) { "Add account" } else { "Save" };
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(
            ConstrainedBox::new(crate::shared_input::shared_input(theme, &state.name, crate::shared_input::SharedInputOpts::default()))
                .with_max_width(300.0)
                .finish(),
        )
        .with_child(primary_button(theme, font, view.view_id, view.hover("accounts-save"), saving, SettingsAction::AgentAccount(AccountAction::Save)))
        .with_child(secondary_button(theme, font, view.view_id, view.hover("accounts-cancel"), "Cancel", SettingsAction::AgentAccount(AccountAction::CancelEdit)))
        .finish();
    let hint = if matches!(state.editing, Some(Editing::Add(_))) {
        "A name like Work or Personal. Then sign in on its card: the agent's own sign-in opens in a tab."
    } else {
        "Only the name changes; the sign-in stays."
    };
    Some(card(
        theme,
        false,
        Flex::column()
            .with_spacing(10.0)
            .with_child(text(theme, font, &title, 13.5, theme.main_text()))
            .with_child(row)
            .with_child(text(theme, font, hint, 12.0, theme.sub_text()))
            .finish(),
    ))
}

/// One account: who it is, its limits, and what can be done with it.
/// `profile` None is the agent's own (system) login.
fn account_card(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    provider: &str,
    profile: Option<&dragent_host::agent_accounts::Profile>,
    is_default: bool,
) -> Box<dyn Element> {
    let state = &view.agent_accounts;
    let font = view.ui_font;
    let (key, label, status) = match profile {
        Some(p) => (
            p.id.to_string(),
            p.label.clone(),
            state.status.get(&p.id).cloned().unwrap_or_else(|| "Not checked".into()),
        ),
        None => (
            system_key(provider),
            "System login".to_string(),
            state.system.get(provider).cloned().unwrap_or_else(|| "Not checked".into()),
        ),
    };
    let who = state.who.get(&key);
    let usage_key = profile.map_or_else(|| "system".to_string(), |p| p.id.to_string());
    let usage = state.usage.iter().find(|u| u.provider == provider && u.account == usage_key).and_then(|u| u.usage.as_ref());
    let plan = who.and_then(|w| w.plan.clone()).or_else(|| usage.and_then(|u| u.plan.clone()));
    let signed_in = status == "Signed in";
    let busy = status == "Checking…" || status == "Signing in…";

    // Top line: default mark, name, plan, status.
    let mark = {
        let selection = profile.map_or(Selection::System, |p| Selection::Profile(p.id));
        let provider = provider.to_string();
        let id = view.view_id;
        let theme = *theme;
        Hoverable::new(view.hover(&format!("accounts-mark-{key}")), move |hover| {
            if is_default {
                crate::icons::render_icon(crate::icons::AppIcon::CircleCheck, theme.accent, 16.0)
            } else {
                // An empty ring: "make this the default".
                let ring = if hover.is_hovered() { theme.sub_text() } else { theme.disabled_text() };
                ConstrainedBox::new(
                    Container::new(Empty::new().finish())
                        .with_border(Border::all(1.5).with_border_color(ring))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                        .finish(),
                )
                .with_width(14.0)
                .with_height(14.0)
                .finish()
            }
        })
        .on_click(move |ectx, _, _| {
            if !is_default {
                ectx.dispatch_typed_action_for(
                    id,
                    SettingsAction::AgentAccount(AccountAction::MakeDefault { provider: provider.clone(), selection: selection.clone() }),
                );
            }
        })
        .finish()
    };
    let mut top = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(mark)
        .with_child(text(theme, font, &label, 14.0, theme.main_text()));
    if let Some(plan) = plan {
        top = top.with_child(tag(theme, font, &plan.to_uppercase()));
    }
    top = top
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(status_line(theme, font, &status));

    // Second line: who it's signed in as, and where its login lives.
    let place = match profile {
        Some(_) => "Its own profile",
        None if provider == "codex" => "~/.codex",
        None => "~/.claude",
    };
    let identity = match who.and_then(|w| w.email.clone()) {
        Some(email) => format!("{email} · {place}"),
        None if signed_in => place.to_string(),
        None => format!("Not signed in · {place}"),
    };

    let mut body = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(top.finish())
        .with_child(Container::new(text(theme, font, &identity, 12.5, theme.sub_text())).with_padding_left(24.0).finish());

    // Limits.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default();
    let windows: Vec<_> = usage.map(|u| u.windows.iter().filter(|w| w.resets_at.is_none_or(|at| at > now)).collect()).unwrap_or_default();
    if windows.is_empty() {
        body = body.with_child(
            Container::new(text(theme, font, "Limits show here after its first session.", 12.0, theme.disabled_text()))
                .with_padding_left(24.0)
                .finish(),
        );
    } else {
        let mut limits = Flex::column().with_spacing(8.0);
        for window in windows {
            limits = limits.with_child(limit_row(theme, font, &window.label, window.used_percent, window.resets_at.map(|at| at - now)));
        }
        body = body.with_child(Container::new(limits.finish()).with_padding_left(24.0).with_padding_top(2.0).finish());
    }

    // Footer: default, then actions.
    let mut footer = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0);
    footer = footer.with_child(if is_default {
        text(theme, font, "Default for new sessions", 12.0, theme.accent)
    } else {
        ghost(
            view,
            theme,
            &format!("accounts-default-{key}"),
            "Make default",
            false,
            AccountAction::MakeDefault {
                provider: provider.to_string(),
                selection: profile.map_or(Selection::System, |p| Selection::Profile(p.id)),
            },
        )
    });
    footer = footer.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if !busy {
        match profile {
            Some(p) => {
                if !signed_in {
                    footer = footer.with_child(ghost(view, theme, &format!("accounts-signin-{key}"), "Sign in", true, AccountAction::Login(p.id)));
                }
                footer = footer
                    .with_child(ghost(view, theme, &format!("accounts-check-{key}"), "Check", false, AccountAction::Status(p.id)))
                    .with_child(ghost(view, theme, &format!("accounts-rename-{key}"), "Rename", false, AccountAction::StartRename(p.id)))
                    .with_child(ghost(view, theme, &format!("accounts-remove-{key}"), "Remove", false, AccountAction::Remove(p.id)));
            }
            None => {
                footer = footer.with_child(ghost(view, theme, &format!("accounts-check-{key}"), "Check", false, AccountAction::CheckSystem(provider.to_string())));
            }
        }
    }
    body = body
        .with_child(
            ConstrainedBox::new(Container::new(Empty::new().finish()).with_background_color(theme.outline()).finish())
                .with_height(1.0)
                .finish(),
        )
        .with_child(footer.finish());

    card(theme, is_default, body.finish())
}

/// A rounded panel; the default account's has an accent outline.
fn card(theme: &crate::theme::Theme, highlighted: bool, child: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(child)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(if highlighted { 1.5 } else { 1.0 }).with_border_color(if highlighted { theme.accent } else { theme.outline() }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(12.0)
        .finish()
}

fn text(_theme: &crate::theme::Theme, font: warpui::fonts::FamilyId, value: &str, size: f32, color: warpui::color::ColorU) -> Box<dyn Element> {
    Text::new(value.to_string(), font, crate::theme::text_size(size)).with_color(color).finish()
}

/// A small uppercase plan tag ("MAX", "PLUS").
fn tag(theme: &crate::theme::Theme, font: warpui::fonts::FamilyId, value: &str) -> Box<dyn Element> {
    Container::new(text(theme, font, value, 10.5, theme.sub_text()))
        .with_background_color(theme.surface_3())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .finish()
}

/// A coloured dot and the account's state.
fn status_line(theme: &crate::theme::Theme, font: warpui::fonts::FamilyId, status: &str) -> Box<dyn Element> {
    let color = match status {
        "Signed in" => theme.ui_green(),
        "Checking…" | "Signing in…" | "Not checked" => theme.disabled_text(),
        _ => theme.ui_yellow(),
    };
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(color)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.5)))
                    .finish(),
            )
            .with_width(7.0)
            .with_height(7.0)
            .finish(),
        )
        .with_child(text(theme, font, status, 12.0, theme.sub_text()))
        .finish()
}

/// One limit: its name, a bar, the share used and when it starts over.
fn limit_row(theme: &crate::theme::Theme, font: warpui::fonts::FamilyId, label: &str, used: f64, resets_in: Option<i64>) -> Box<dyn Element> {
    let ratio = (used / 100.0).clamp(0.0, 1.0) as f32;
    let color = if ratio >= 0.9 {
        theme.ui_red()
    } else if ratio >= 0.7 {
        theme.ui_yellow()
    } else {
        theme.main_text()
    };
    let bar = ConstrainedBox::new(
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(
                    Expanded::new(
                        ratio.max(0.015),
                        Container::new(Empty::new().finish())
                            .with_background_color(color)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(Expanded::new((1.0 - ratio).max(0.0001), Empty::new().finish()).finish())
                .finish(),
        )
        .with_background_color(theme.surface_3())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
        .finish(),
    )
    .with_height(4.0)
    .finish();
    let fixed = |child: Box<dyn Element>, width: f32| ConstrainedBox::new(child).with_width(width).finish();
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(fixed(text(theme, font, label, 12.0, theme.sub_text()), 84.0))
        .with_child(Expanded::new(1.0, bar).finish())
        .with_child(fixed(text(theme, font, &format!("{}%", used.round() as i64), 12.0, theme.main_text()), 40.0))
        .with_child(fixed(
            text(theme, font, &resets_in.map(|s| format!("resets in {}", duration_label(s))).unwrap_or_default(), 12.0, theme.disabled_text()),
            110.0,
        ))
        .finish()
}

/// "49m", "4h 29m", "5d 21h".
fn duration_label(seconds: i64) -> String {
    let minutes = (seconds.max(0) + 59) / 60;
    let (days, hours, mins) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

/// A borderless text button; `strong` uses the accent colour.
fn ghost(view: &SettingsView, theme: &crate::theme::Theme, key: &str, label: &str, strong: bool, action: AccountAction) -> Box<dyn Element> {
    let font = view.ui_font;
    let id = view.view_id;
    let label = label.to_string();
    let theme = *theme;
    Hoverable::new(view.hover(key), move |hover| {
        let color = if strong {
            theme.accent
        } else if hover.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let mut chip = Container::new(Text::new(label.clone(), font, crate::theme::text_size(12.5)).with_color(color).finish())
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)));
        if hover.is_hovered() {
            chip = chip.with_background_color(theme.surface_3());
        }
        chip.finish()
    })
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, SettingsAction::AgentAccount(action.clone())))
    .finish()
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations_read_naturally() {
        assert_eq!(super::duration_label(49 * 60), "49m");
        assert_eq!(super::duration_label(4 * 3600 + 29 * 60), "4h 29m");
        assert_eq!(super::duration_label(5 * 86400 + 21 * 3600), "5d 21h");
        assert_eq!(super::duration_label(-5), "0m");
    }
}
