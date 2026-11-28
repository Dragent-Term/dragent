//! Settings → Integrations: connect the services work comes from. The
//! connections themselves live in `crate::integrations`; this is the page's
//! state and what its buttons do (the page draws it, see
//! `pages::integrations`).

use std::collections::HashMap;

use warpui::elements::ClippedScrollStateHandle;
use warpui::fonts::FamilyId;
use warpui::{ReadView, UpdateView, View, ViewContext, ViewHandle};

use crate::integrations::{Credentials, Provider, Status};
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputMode};

use super::settings_view::SettingsView;

#[derive(Clone, Debug, PartialEq)]
pub enum IntegrationAction {
    /// Ask every provider again how it's connected.
    Refresh,
    /// Show a provider's dialog (its connection, or a form to connect).
    Open(Provider),
    Close,
    /// Check and save what the dialog's form holds, or sign in through the
    /// browser.
    Connect(Provider),
    Disconnect(Provider),
    OpenHelp(Provider),
    /// Whether the connection being made is for the whole team.
    SetShared(bool),
    Tab(IntegrationsTab),
    /// Show what Dragent would write into an agent's own settings so it
    /// gets Dragent's tools (Cursor, Gemini, Qwen), waiting for a yes.
    AllowAgentTools(crate::agent::mcp_agent_sync::SyncAgent),
    /// Take Dragent's entry back out of that agent's settings.
    RemoveAgentTools(crate::agent::mcp_agent_sync::SyncAgent),
}

/// The page's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IntegrationsTab {
    /// What's connected, and everything that can be.
    #[default]
    Services,
    /// How linking work to a new workspace starts out.
    Tasks,
    /// Which coding agents get the connected services.
    Agents,
}

pub struct IntegrationsState {
    /// The latest answer per provider; missing while it's being asked.
    pub statuses: HashMap<Provider, Status>,
    /// The provider whose dialog is open.
    pub open: Option<Provider>,
    /// The provider being checked with its service right now.
    pub busy: Option<Provider>,
    pub error: Option<(Provider, String)>,
    /// Dragent's server keeps connections, so they follow the account and
    /// can be shared.
    pub server: bool,
    /// The user may share connections with the team.
    pub admin: bool,
    /// The open dialog's connection is for the whole team.
    pub shared: bool,
    pub fields: HashMap<(Provider, &'static str), ViewHandle<InputEditor>>,
    pub scroll: ClippedScrollStateHandle,
    pub tab: IntegrationsTab,
    /// The coding agents found on this computer at the last refresh.
    pub agents_present: Vec<crate::agent::mcp_agent_sync::SyncAgent>,
    /// Agents the person allowed Dragent's entry in their own settings.
    pub tools_allowed: std::collections::BTreeSet<crate::agent::mcp_agent_sync::SyncAgent>,
    /// An agent whose settings are being changed now.
    pub tools_busy: Option<crate::agent::mcp_agent_sync::SyncAgent>,
    /// What went wrong changing an agent's settings.
    pub tools_error: Option<String>,
    generation: u64,
}

impl IntegrationsState {
    pub fn new<V: View>(ctx: &mut ViewContext<V>, ui: FamilyId, theme: Theme) -> Self {
        let mut fields = HashMap::new();
        for provider in Provider::ALL {
            for field in provider.fields() {
                let key = format!("integration_{}_{}", provider.key(), field.id);
                let secret = field.secret;
                let placeholder = field.placeholder;
                let editor = ctx.add_typed_action_view(move |inner| {
                    let mut editor = InputEditor::new(
                        placeholder,
                        ui,
                        crate::theme::text_size(13.0),
                        theme.main_text(),
                        theme.disabled_text(),
                        theme.accent,
                        theme,
                        InputMode::Plain { key },
                        inner.focus_handle(),
                    );
                    if secret {
                        editor.set_secure(true, inner);
                    }
                    editor
                });
                fields.insert((provider, field.id), editor);
            }
        }
        Self {
            statuses: HashMap::new(),
            open: None,
            busy: None,
            error: None,
            server: false,
            admin: false,
            shared: false,
            fields,
            scroll: ClippedScrollStateHandle::default(),
            tab: IntegrationsTab::default(),
            agents_present: Vec::new(),
            tools_allowed: Default::default(),
            tools_busy: None,
            tools_error: None,
            generation: 0,
        }
    }
}

impl SettingsView {
    /// Ask every provider how it's connected (off the main thread: the
    /// server is asked for the account's and the team's connections,
    /// GitHub runs the GitHub CLI, the others read the keychain).
    pub(crate) fn refresh_integrations(&mut self, ctx: &mut ViewContext<Self>) {
        self.integrations.generation = self.integrations.generation.wrapping_add(1);
        let generation = self.integrations.generation;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            if let Err(error) = crate::integrations::remote::refresh() {
                log::info!("[integrations] the server's connections: {error}");
            }
            let remote = crate::integrations::remote::current();
            let statuses: HashMap<Provider, Status> = Provider::ALL
                .into_iter()
                .map(|provider| (provider, crate::integrations::status(provider)))
                .collect();
            let home = dirs::home_dir().unwrap_or_default();
            let agents: Vec<_> = crate::agent::mcp_agent_sync::SyncAgent::ALL
                .into_iter()
                .filter(|agent| agent.is_present(&home))
                .collect();
            let allowed = crate::integrations::agents::allowed();
            let _ = tx.send((statuses, remote.available, remote.admin, agents, allowed));
        });
        ctx.spawn(rx, move |view, answer, ctx| {
            if view.integrations.generation != generation {
                return;
            }
            if let Ok((statuses, server, admin, agents, allowed)) = answer {
                view.integrations.tools_allowed = allowed;
                view.integrations.statuses = statuses;
                view.integrations.server = server;
                view.integrations.admin = admin;
                view.integrations.agents_present = agents;
            }
            ctx.notify();
        });
    }

    fn clear_integration_fields(&mut self, provider: Provider, ctx: &mut ViewContext<Self>) {
        for field in provider.fields() {
            if let Some(editor) = self.integrations.fields.get(&(provider, field.id)) {
                ctx.update_view(editor, |editor, editor_ctx| editor.set_text("", editor_ctx));
            }
        }
    }

    pub(crate) fn handle_integration_action(
        &mut self,
        action: &IntegrationAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            IntegrationAction::Refresh => {
                self.integrations.statuses.clear();
                self.refresh_integrations(ctx);
            }
            IntegrationAction::Open(provider) => {
                self.integrations.open = Some(*provider);
                self.integrations.error = None;
                self.integrations.shared = false;
                self.integrations.scroll = ClippedScrollStateHandle::default();
                self.clear_integration_fields(*provider, ctx);
                let connected = matches!(
                    self.integrations.statuses.get(provider),
                    Some(Status::Connected { .. })
                );
                if let Some(editor) = provider
                    .fields()
                    .first()
                    .filter(|_| !connected)
                    .and_then(|field| self.integrations.fields.get(&(*provider, field.id)))
                {
                    ctx.focus(editor);
                }
            }
            IntegrationAction::Close => {
                if let Some(provider) = self.integrations.open.take() {
                    self.clear_integration_fields(provider, ctx);
                }
                self.integrations.error = None;
            }
            IntegrationAction::Connect(provider) => {
                if self.integrations.busy.is_some() {
                    return;
                }
                let provider = *provider;
                let credentials: Credentials = provider
                    .fields()
                    .iter()
                    .filter_map(|field| {
                        let editor = self.integrations.fields.get(&(provider, field.id))?;
                        let text = editor.read(ctx, |editor, _| editor.get_text().to_string());
                        Some((field.id.to_string(), text))
                    })
                    .collect();
                self.integrations.busy = Some(provider);
                self.integrations.error = None;
                let shared = self.integrations.shared;
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let _ = tx.send(crate::integrations::connect(provider, credentials, shared));
                });
                ctx.spawn(rx, move |view, result, ctx| {
                    view.integrations.busy = None;
                    match result {
                        Ok(Ok(connection)) => {
                            view.integrations.statuses.insert(
                                provider,
                                Status::Connected {
                                    account: connection.account,
                                    source: connection.source,
                                },
                            );
                            // Keys never linger in the form.
                            view.clear_integration_fields(provider, ctx);
                            // Other agents' tools appear with the first one.
                            crate::integrations::agents::sync_in_background();
                        }
                        Ok(Err(error)) => view.integrations.error = Some((provider, error)),
                        Err(_) => {
                            view.integrations.error =
                                Some((provider, "The check didn't finish.".into()))
                        }
                    }
                    ctx.notify();
                });
            }
            IntegrationAction::Disconnect(provider) => {
                if self.integrations.busy.is_some() {
                    return;
                }
                let provider = *provider;
                self.integrations.busy = Some(provider);
                self.integrations.error = None;
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let result = crate::integrations::disconnect(provider);
                    // Another connection may take its place (the team's).
                    let _ = tx.send(result.map(|()| crate::integrations::status(provider)));
                });
                ctx.spawn(rx, move |view, result, ctx| {
                    view.integrations.busy = None;
                    match result {
                        Ok(Ok(status)) => {
                            view.integrations.statuses.insert(provider, status);
                            crate::integrations::agents::sync_in_background();
                        }
                        Ok(Err(error)) => view.integrations.error = Some((provider, error)),
                        Err(_) => {
                            view.integrations.error =
                                Some((provider, "Disconnecting didn't finish.".into()))
                        }
                    }
                    ctx.notify();
                });
            }
            IntegrationAction::OpenHelp(provider) => ctx.dispatch_typed_action_deferred(crate::workspace::WorkspaceAction::OpenExternalUrl { url: provider.help().1.to_string() }),
            IntegrationAction::SetShared(shared) => {
                self.integrations.shared = *shared && self.integrations.admin;
            }
            IntegrationAction::Tab(tab) => self.integrations.tab = *tab,
            IntegrationAction::AllowAgentTools(agent) => {
                // Writes into another agent's settings: show exactly what first.
                let home = dirs::home_dir().unwrap_or_default();
                self.integrations.tools_error = None;
                self.pending_agent_change = Some(super::settings_view::AgentChange::AllowTools {
                    agent: *agent,
                    file: agent.settings_file(&home),
                });
            }
            IntegrationAction::RemoveAgentTools(agent) => self.change_agent_tools(*agent, false, ctx),
        }
        ctx.notify();
    }

    /// Allow or withdraw Dragent's entry in `agent`'s own settings, off the
    /// main thread, then show where it stands.
    pub(crate) fn change_agent_tools(
        &mut self,
        agent: crate::agent::mcp_agent_sync::SyncAgent,
        on: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.integrations.tools_busy.is_some() {
            return;
        }
        self.integrations.tools_busy = Some(agent);
        self.integrations.tools_error = None;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let result = crate::integrations::agents::set_allowed(agent, on);
            let _ = tx.send((result, crate::integrations::agents::allowed()));
        });
        ctx.spawn(rx, move |view, answer, ctx| {
            view.integrations.tools_busy = None;
            match answer {
                Ok((result, allowed)) => {
                    view.integrations.tools_allowed = allowed;
                    view.integrations.tools_error = result.err();
                }
                Err(_) => view.integrations.tools_error = Some("The change didn't finish.".into()),
            }
            ctx.notify();
        });
    }
}
