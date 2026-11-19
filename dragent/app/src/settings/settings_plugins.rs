//! Plugins + sharing action handlers.
//!
//! The MCP servers in `~/.dragent/mcp.json` are the plugin inventory: each
//! server name is a plugin id, so per-plugin toggles take effect by filtering
//! / blocking at the `agent::mcp` layer and reconnecting the affected server.

use super::action::SettingsAction;
use super::settings_view::SettingsView;
use super::SettingsEvent;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_plugin_actions(
        &mut self,
        action: &SettingsAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            SettingsAction::TogglePluginSystem => {
                self.app_settings.plugin.system_enabled =
                    !self.app_settings.plugin.system_enabled;
                let on = self.app_settings.plugin.system_enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                // Turning the system off drops every live plugin connection
                // immediately; turning it on reconnects the enabled servers.
                if on {
                    for server in self.app_settings.mcp_servers.clone() {
                        if server.enabled
                            && self.app_settings.plugin.is_plugin_enabled(&server.name)
                        {
                            let tx = self.mcp_reconnect_tx.clone();
                            let name_for_cb = server.name.clone();
                            crate::agent::mcp::reinit_server(
                                &server.name,
                                Some(Box::new(move || {
                                    let _ = tx.send_blocking(name_for_cb);
                                })),
                            );
                        }
                    }
                    self.status_message =
                        Some("Plugin system enabled — reconnecting servers…".into());
                } else if let Ok(mut guard) = crate::agent::mcp::MCP_MANAGER.write() {
                    if let Some(mgr) = guard.as_mut() {
                        for server in &self.app_settings.mcp_servers {
                            mgr.remove_server(&server.name);
                        }
                    }
                    self.status_message =
                        Some("Plugin system disabled — all plugin tools off.".into());
                }
            }
            SettingsAction::TogglePluginEnabled(id) => {
                // Flip list membership: when the system is on this toggles the
                // effective state; when off it pre-arms the state for next
                // enable (is_plugin_enabled is false for all while off).
                let target_on = if self.app_settings.plugin.system_enabled {
                    !self.app_settings.plugin.is_plugin_enabled(id)
                } else {
                    self.app_settings
                        .plugin
                        .disabled_plugins
                        .iter()
                        .any(|d| d == id)
                };
                self.app_settings.plugin.set_plugin_enabled(id, target_on);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                if target_on {
                    let tx = self.mcp_reconnect_tx.clone();
                    let name_for_cb = id.clone();
                    crate::agent::mcp::reinit_server(
                        id,
                        Some(Box::new(move || {
                            let _ = tx.send_blocking(name_for_cb);
                        })),
                    );
                    self.status_message = Some(format!("Plugin '{id}' enabled."));
                } else if let Ok(mut guard) = crate::agent::mcp::MCP_MANAGER.write() {
                    if let Some(mgr) = guard.as_mut() {
                        mgr.remove_server(id);
                    }
                    self.status_message = Some(format!("Plugin '{id}' disabled."));
                }
            }
            SettingsAction::SetPluginConsent { plugin, version } => {
                self.app_settings.plugin.set_consent(plugin, version);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                self.status_message = Some(format!("Approved '{plugin}' ({version})."));
            }
            SettingsAction::ToggleArtifactSharing => {
                self.app_settings.sharing.artifact_sharing_enabled =
                    !self.app_settings.sharing.artifact_sharing_enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            _ => {}
        }
    }
}
