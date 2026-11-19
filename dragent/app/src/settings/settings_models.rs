use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::settings::AccountStore;
use crate::settings::CustomModel;
use crate::settings::McpTransport;
use crate::settings::SettingsEvent;
use warpui::ReadModel;
use warpui::SingletonEntity;
use warpui::UpdateModel;
use warpui::UpdateView;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_models(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::SelectModel(model_id) => {
                let account = AccountStore::handle(ctx);
                let restricted_to_free = ctx.read_model(&account, |store, _| {
                    store
                        .data
                        .as_ref()
                        .map(|data| {
                            data.usage.model_locked_to_free
                                || data.user.tier.eq_ignore_ascii_case("free")
                        })
                        .unwrap_or(false)
                });
                // A configured user credential is an explicit entitlement to
                // use the provider's supported models, even on the free tier.
                // Keep this in the action handler as well as the renderer;
                // otherwise the picker appears unlocked but every choice is
                // immediately rewritten to Auto.
                let locked_to_free =
                    restricted_to_free && !self.app_settings.has_any_user_credential();
                self.app_settings.selected_model = if locked_to_free {
                    crate::settings::app_settings::AUTO_MODEL_ID.to_string()
                } else {
                    model_id.clone()
                };
                self.primary_picker_open = false;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectEffort(effort) => {
                self.app_settings.selected_effort = *effort;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::TogglePrimaryPicker => {
                self.primary_picker_open = !self.primary_picker_open;
                ctx.notify();
            }
            SettingsAction::UpdateProviderKey(key, value) => {
                if key.starts_with("modal_") {
                    // Custom model modal fields.
                    if let Some(ref mut draft) = self.custom_model_modal {
                        match key.as_str() {
                            "modal_name" => draft.name = value.clone(),
                            "modal_base_url" => draft.base_url = value.clone(),
                            "modal_model_id" => draft.model_id = value.clone(),
                            "modal_api_key" => draft.api_key = value.clone(),
                            _ => {}
                        }
                    }
                } else if key.starts_with("mcp_") && key != "mcp_search_query" {
                    // MCP add/edit form fields — update the draft mirror values.
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
                } else if key.starts_with("org_") {
                    match key.as_str() {
                        "org_invite_email" => self.org.invite_email = value.clone(),
                        "org_team_name" => self.org.team_name = value.clone(),
                        "org_new_name" => self.org.new_org_name = value.clone(),
                        _ => {}
                    }
                    ctx.notify();
                } else if key.starts_with("skill_") && key != "skill_search_query" {
                    ctx.notify();
                } else if matches!(key.as_str(), "agent_command" | "agent_base_command" | "agent_prompt_suffix") {
                    if let Some(config) = self.agent_configs.get_mut(self.command_agent.command_key()) {
                        match key.as_str() {
                            "agent_command" => config.prompt_command = value.clone(),
                            "agent_base_command" => config.command = value.clone(),
                            _ => config.prompt_suffix = value.clone(),
                        }
                    }
                    self.agent_command_status = Some("Unsaved changes".into());
                    ctx.notify();
                } else if key == "search_query" {
                    self.search_query = value.clone();
                    ctx.notify();
                } else if key == "skill_search_query" {
                    self.skill_search_query = value.clone();
                    self.open_picker = None;
                    ctx.notify();
                } else if key == "mcp_search_query" {
                    self.mcp_search_query = value.clone();
                    ctx.notify();
                } else if key == "shortcut_search_query" {
                    self.shortcut_search_query = value.clone();
                    ctx.notify();
                } else if key == "shortcut_binding_draft" {
                    self.shortcut_binding_draft = value.clone();
                    ctx.notify();
                } else if key == "computer_app_draft" {
                    self.computer_app_draft = value.clone();
                    ctx.notify();
                } else if key == "agent_menu_search" {
                    self.agent_menu_search = value.clone();
                    ctx.notify();
                } else if key == "theme_search_query" {
                    self.theme_search_query = value.clone();
                    ctx.notify();
                } else if key == "index_search_query" {
                    self.index_search_query = value.clone();
                    self.index_list_page = 0; // new filter → first page
                    ctx.notify();
                } else if key == "command_agent_search" {
                    self.command_agent_search = value.clone();
                    ctx.notify();
                } else {
                    // Provider key field — in-memory map now, keychain
                    // write (M2) queued off the main thread, auto-saved
                    // immediately (save excludes secrets from disk on native).
                    let outcome = super::credentials::save_provider_key(
                        &mut self.app_settings.provider_keys,
                        key,
                        value.clone(),
                    );
                    self.watch_keychain_write(super::credentials::account_for_slot(key), outcome, ctx);
                    self.app_settings.save();
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            SettingsAction::ClearProviderKey(provider_id) => {
                let outcome = super::credentials::delete_provider_key(
                    &mut self.app_settings.provider_keys,
                    provider_id,
                );
                self.watch_keychain_write(
                    super::credentials::account_for_slot(provider_id),
                    outcome,
                    ctx,
                );
                self.app_settings.save();
                if let Some(editor) = self.provider_key_editors.get(provider_id) {
                    ctx.update_view(editor, |e, ctx| e.set_text("", ctx));
                }
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::AddCustomModel => {
                self.open_custom_model_modal(None, ctx);
            }
            SettingsAction::EditCustomModel(index) => {
                self.open_custom_model_modal(Some(*index), ctx);
            }
            SettingsAction::CloseCustomModelModal => {
                self.custom_model_modal = None;
            }
            SettingsAction::ConfirmCustomModal => {
                if let Some(draft) = self.custom_model_modal.take() {
                    if draft.name.trim().is_empty()
                        || draft.base_url.trim().is_empty()
                        || draft.model_id.trim().is_empty()
                    {
                        // Keep the form open so a partial add/edit is not lost.
                        self.custom_model_modal = Some(draft);
                    } else {
                        let name = draft.name.trim().to_string();
                        let base_url = draft.base_url.trim().to_string();
                        let model_id = draft.model_id.trim().to_string();
                        let api_key = draft.api_key.trim().to_string();
                        // M2: custom secrets live in the keychain (native),
                        // written off the main thread; the record holds the
                        // key in memory only (save scrubs it from disk).
                        let (stored_key, outcome) =
                            super::credentials::save_custom_key(&model_id, &api_key);
                        self.watch_keychain_write(
                            super::credentials::account_for_custom(&model_id),
                            outcome,
                            ctx,
                        );
                        let model = CustomModel {
                            name,
                            base_url,
                            model_id,
                            api_key: stored_key,
                            endpoint_style: draft.endpoint_style,
                        };
                        match draft.editing_index {
                            Some(index) if index < self.app_settings.custom_models.len() => {
                                self.app_settings.custom_models[index] = model;
                            }
                            _ => self.app_settings.custom_models.push(model),
                        }
                        self.app_settings.save();
                        ctx.emit(SettingsEvent::AppSettingsChanged);
                    }
                }
            }
            SettingsAction::RemoveCustomModel(index) => {
                if *index < self.app_settings.custom_models.len() {
                    self.app_settings.custom_models.remove(*index);
                    self.app_settings.save();
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            SettingsAction::ToggleCreditUsage => {
                // Server-owned: it decides whether a request is billed, so the
                // client must not keep its own copy. Flip through the API and
                // let the refetch supply the new value.
                use warpui::SingletonEntity;
                let account = crate::settings::AccountStore::handle(ctx);
                let current = ctx.read_model(&account, |s, _| {
                    s.data
                        .as_ref()
                        .map(|d| d.usage.allow_credit_usage)
                        .unwrap_or(false)
                });
                ctx.update_model(&account, |_s, mctx| {
                    crate::settings::account_store::set_allow_credit_usage(!current, mctx);
                });
            }
            SettingsAction::ToggleDragentFallback => {
                self.app_settings.allow_dragent_fallback =
                    !self.app_settings.allow_dragent_fallback;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            _ => {}
        }
    }
}

impl SettingsView {
    /// Keys read from the Keychain in the background have landed: fill the
    /// ones this view still holds as absent (and their empty fields). Keys
    /// typed or cleared here meanwhile are newer and stay as they are.
    pub(crate) fn adopt_landed_keys(&mut self, ctx: &mut ViewContext<Self>) {
        let fresh = crate::settings::AppSettings::load();
        let mut changed = false;
        for slot in super::credentials::KEY_SLOTS {
            if self.app_settings.provider_keys.get(slot).is_some() {
                continue;
            }
            let Some(key) = fresh.provider_keys.get(slot).map(str::to_string) else {
                continue;
            };
            self.app_settings.provider_keys.set(slot, key.clone());
            if let Some(editor) = self.provider_key_editors.get(*slot) {
                if editor.as_ref(ctx).get_text().is_empty() {
                    ctx.update_view(editor, |e, ctx| e.set_text(&key, ctx));
                }
            }
            changed = true;
        }
        for model in &mut self.app_settings.custom_models {
            if model.api_key.is_some() {
                continue;
            }
            if let Some(landed) = fresh
                .custom_models
                .iter()
                .find(|m| m.model_id == model.model_id)
                .and_then(|m| m.api_key.clone())
            {
                model.api_key = Some(landed);
                changed = true;
            }
        }
        if changed {
            ctx.notify();
        }
    }

    /// Show what a queued keychain write did: a failure stays on the row for
    /// `account` until a later write for it succeeds. Writes overtaken by a
    /// newer one never answer, so a stale outcome can't land last.
    pub(crate) fn watch_keychain_write(
        &mut self,
        account: String,
        outcome: Option<async_channel::Receiver<super::credentials::KeychainOutcome>>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(outcome) = outcome else {
            return;
        };
        ctx.spawn(async move { outcome.recv().await.ok() }, move |view, outcome, ctx| {
            let changed = match outcome {
                None => false,
                Some(Ok(())) => view.keychain_errors.remove(&account).is_some(),
                Some(Err(message)) => {
                    view.keychain_errors.insert(account, message);
                    true
                }
            };
            if changed {
                ctx.notify();
            }
        });
    }
}
