use super::action::SettingsAction;
use super::settings_view::clear_mcp_token_file;
use super::settings_view::SettingsView;
use crate::settings::McpTransport;
use crate::settings::SettingsEvent;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_mcp_actions(
        &mut self,
        action: &SettingsAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            SettingsAction::RemoveMcpServer(index) => {
                if *index < self.app_settings.mcp_servers.len() {
                    let name = self.app_settings.mcp_servers[*index].name.clone();
                    self.app_settings.mcp_servers.remove(*index);
                    self.persist_mcp_servers(ctx);
                    self.mcp_status_message = Some(format!("Removed '{}'.", name));
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            SettingsAction::ShowMcpForm(editing_index) => {
                self.open_mcp_form(*editing_index, ctx);
            }
            SettingsAction::HideMcpForm => {
                self.mcp_form = None;
            }
            SettingsAction::UpdateMcpDraftField(key, value) => {
                if let Some(ref mut draft) = self.mcp_form {
                    match key.as_str() {
                        "mcp_name" => draft.name = value.clone(),
                        "mcp_command" => draft.command = value.clone(),
                        "mcp_args" => draft.args_raw = value.clone(),
                        "mcp_env" => draft.env_raw = value.clone(),
                        "mcp_bearer_token" => draft.bearer_token = value.clone(),
                        "mcp_transport_stdio" => draft.transport = McpTransport::Stdio,
                        "mcp_transport_sse" => draft.transport = McpTransport::Sse,
                        _ => {}
                    }
                }
            }
            SettingsAction::SaveMcpServer => {
                self.save_mcp_draft(ctx);
            }
            SettingsAction::ToggleMcpDraftAgent(key) => {
                if let Some(draft) = &mut self.mcp_form {
                    if let Some(at) = draft.sync_agents.iter().position(|k| k == key) {
                        draft.sync_agents.remove(at);
                    } else {
                        draft.sync_agents.push(key.clone());
                    }
                }
                ctx.notify();
            }
            SettingsAction::SelectAllMcpDraftAgents => {
                if let Some(draft) = &mut self.mcp_form {
                    let home = dirs::home_dir().unwrap_or_default();
                    draft.sync_agents = crate::agent::mcp_agent_sync::SyncAgent::ALL
                        .into_iter()
                        .filter(|agent| agent.is_present(&home))
                        .map(|agent| agent.key().to_string())
                        .collect();
                }
                ctx.notify();
            }
            SettingsAction::ToggleMcpCapabilities(name) => {
                if !self.mcp_expanded.remove(name) {
                    self.mcp_expanded.insert(name.clone());
                }
                ctx.notify();
            }
            SettingsAction::ToggleMcpEnabled(index) => {
                if let Some(server) = self.app_settings.mcp_servers.get_mut(*index) {
                    server.enabled = !server.enabled;
                    let enabled = server.enabled;
                    let name = server.name.clone();
                    self.persist_mcp_servers(ctx);
                    ctx.emit(SettingsEvent::AppSettingsChanged);

                    if enabled {
                        // Reconnect and re-discover tools from the server.
                        let tx = self.mcp_reconnect_tx.clone();
                        let name_for_cb = name.clone();
                        self.mcp_status_message = Some(format!("Connecting '{name}'…"));
                        crate::agent::mcp::reinit_server(
                            &name,
                            Some(Box::new(move || {
                                let _ = tx.send_blocking(name_for_cb);
                            })),
                        );
                    } else {
                        // Drop the live connection immediately.
                        if let Ok(mut guard) = crate::agent::mcp::MCP_MANAGER.write() {
                            if let Some(mgr) = guard.as_mut() {
                                mgr.remove_server(&name);
                            }
                        }
                        self.mcp_status_message = Some(format!("'{name}' disconnected."));
                    }
                }
            }
            SettingsAction::TriggerMcpOAuth(index) => {
                if let Some(server) = self.app_settings.mcp_servers.get(*index) {
                    if server.transport != crate::settings::McpTransport::Sse {
                        return;
                    }
                    let name = server.name.clone();
                    let tx = self.mcp_reconnect_tx.clone();
                    self.mcp_status_message = Some(format!(
                        "Connecting '{name}' — a browser window may open for authentication…"
                    ));
                    let name_for_cb = name.clone();
                    crate::agent::mcp::reinit_server(
                        &name,
                        Some(Box::new(move || {
                            let _ = tx.send_blocking(name_for_cb);
                        })),
                    );
                }
            }
            SettingsAction::DisconnectMcpOAuth(index) => {
                if let Some(server) = self.app_settings.mcp_servers.get(*index) {
                    let name = server.name.clone();
                    clear_mcp_token_file(&name);
                    self.mcp_status_message = Some(format!("Disconnected '{}'.", name));
                }
            }
            SettingsAction::ImportClaudeDesktop => {
                self.import_claude_desktop(ctx);
            }
            SettingsAction::ApplyPluginServers {
                upsert,
                remove,
                revoke,
            } => {
                let drop_live = |name: &str| {
                    if let Ok(mut guard) = crate::agent::mcp::MCP_MANAGER.write() {
                        if let Some(mgr) = guard.as_mut() {
                            mgr.remove_server(name);
                        }
                    }
                };
                for name in remove {
                    self.app_settings.mcp_servers.retain(|server| &server.name != name);
                    self.app_settings.plugin.consents.remove(name);
                    clear_mcp_token_file(name);
                    drop_live(name);
                }
                for name in revoke {
                    self.app_settings.plugin.consents.remove(name);
                    drop_live(name);
                }
                for entry in upsert {
                    match self
                        .app_settings
                        .mcp_servers
                        .iter_mut()
                        .find(|server| server.name == entry.name)
                    {
                        Some(server) => *server = entry.clone(),
                        None => self.app_settings.mcp_servers.push(entry.clone()),
                    }
                }
                self.app_settings.save();
                self.persist_mcp_servers(ctx);
                ctx.emit(SettingsEvent::AppSettingsChanged);
                ctx.notify();
            }
            _ => {}
        }
    }
}

impl SettingsView {
    /// Save the server list, then bring the other agents' configs in line
    /// off the main thread. What the sync did is added to the status line.
    pub(crate) fn persist_mcp_servers(&mut self, ctx: &mut ViewContext<Self>) {
        super::mcp_config::save_entries(&self.app_settings.mcp_servers);
        #[cfg(not(target_family = "wasm"))]
        {
            let entries = self.app_settings.mcp_servers.clone();
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(crate::agent::mcp_agent_sync::sync_agents(&entries));
            });
            ctx.spawn(async move { rx.recv().await }, |view, report, ctx| {
                let Some(summary) = report.ok().and_then(|report| report.summary()) else {
                    return;
                };
                view.mcp_status_message = Some(match view.mcp_status_message.take() {
                    Some(message) => format!("{message} {summary}"),
                    None => summary,
                });
                ctx.notify();
            });
        }
    }
}
