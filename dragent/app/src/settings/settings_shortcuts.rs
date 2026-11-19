use super::action::SettingsAction;
use super::settings_view::SettingsView;
use super::shortcut_settings;
use crate::settings::SettingsEvent;
use warpui::{UpdateView, ViewContext};

impl SettingsView {
    pub(crate) fn handle_shortcuts(
        &mut self,
        action: &SettingsAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            SettingsAction::StartRebind(action_id) => {
                self.shortcut_rebind_target = Some(action_id.clone());
                self.shortcut_status_message = None;
                // Prefill with the chord in effect so editing is incremental.
                let current = self
                    .app_settings
                    .shortcuts
                    .effective_chord(action_id)
                    .unwrap_or_default();
                self.shortcut_binding_draft = current.clone();
                ctx.update_view(&self.shortcut_binding_editor, |e, ectx| {
                    e.set_text(&current, ectx);
                });
                ctx.notify();
            }
            SettingsAction::CancelRebind => {
                self.shortcut_rebind_target = None;
                self.shortcut_status_message = None;
                ctx.notify();
            }
            SettingsAction::SetBinding { action, chord } => {
                let chord = if chord.trim().is_empty() {
                    // Empty commit with a live draft = use the editor text.
                    self.shortcut_binding_draft.clone()
                } else {
                    chord.clone()
                };
                match self.app_settings.shortcuts.set_binding(action, &chord) {
                    Ok(()) => {
                        self.app_settings.save();
                        self.shortcut_rebind_target = None;
                        self.shortcut_status_message = Some(format!(
                            "“{action}” is now {chord} (mirrored to keybindings.json)."
                        ));
                        crate::workspace::sync_shortcut_bindings(ctx);
                        ctx.emit(SettingsEvent::AppSettingsChanged);
                    }
                    Err(error) => {
                        // Stay in edit mode so the user can fix the chord.
                        self.shortcut_status_message = Some(error);
                        ctx.notify();
                    }
                }
            }
            SettingsAction::ResetBinding(action_id) => {
                if self.app_settings.shortcuts.reset_binding(action_id) {
                    self.app_settings.save();
                    self.shortcut_status_message =
                        Some(format!("“{action_id}” restored to its default."));
                    crate::workspace::sync_shortcut_bindings(ctx);
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                } else {
                    self.shortcut_status_message = None;
                }
                if self.shortcut_rebind_target.as_deref() == Some(action_id.as_str()) {
                    self.shortcut_rebind_target = None;
                }
                ctx.notify();
            }
            SettingsAction::ResetAllBindings => {
                self.app_settings.shortcuts.reset_all_bindings();
                self.app_settings.save();
                self.shortcut_rebind_target = None;
                self.shortcut_status_message =
                    Some("All shortcuts restored to defaults.".to_string());
                crate::workspace::sync_shortcut_bindings(ctx);
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectShortcutPolicy(policy) => {
                self.app_settings.shortcuts.terminal_shortcut_policy = *policy;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectCtrlTabOrder(order) => {
                self.app_settings.shortcuts.ctrl_tab_order = *order;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleKeepAwake => {
                self.app_settings.dev_permissions.keep_awake_while_agents_run =
                    !self.app_settings.dev_permissions.keep_awake_while_agents_run;
                self.app_settings.save();
                shortcut_settings::sync_wake_lock(&self.app_settings.dev_permissions);
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectAwakeMode(mode) => {
                self.app_settings.dev_permissions.awake_mode = *mode;
                self.app_settings.save();
                shortcut_settings::sync_wake_lock(&self.app_settings.dev_permissions);
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::RecheckDeveloperPermissions => {
                let report = shortcut_settings::query_permission_report();
                self.dev_perm_note = Some(format!(
                    "Rechecked: accessibility {}, full-disk-access {}.",
                    report.accessibility.label(),
                    report.full_disk_access.label()
                ));
                ctx.notify();
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::RefreshComputerPermissions => self.refresh_computer_permissions(ctx),
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::ToggleComputerSkill => {
                let enabled = !crate::cli::agent_skill::computer_enabled();
                let result = crate::cli::agent_skill::set_computer_enabled(enabled);
                self.computer_permission_note = Some(format!(
                    "Desktop skill: {} installed, {} removed, {} user-owned files preserved.",
                    result.written.len(),
                    result.removed.len(),
                    result.kept.len()
                ));
                ctx.notify();
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::RequestComputerPermissionReset => {
                self.computer_reset_confirm = true;
                ctx.notify();
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::CancelComputerPermissionReset => {
                self.computer_reset_confirm = false;
                ctx.notify();
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::ConfirmComputerPermissionReset => {
                if !self.computer_reset_confirm {
                    return;
                }
                self.computer_reset_confirm = false;
                self.computer_permission_generation += 1;
                let generation = self.computer_permission_generation;
                self.computer_permissions_loading = true;
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    let _ = tx.send(dragent_computer::reset_permissions());
                });
                ctx.spawn(async move {rx.await.ok()},move |view,result,ctx| {
                    if view.computer_permission_generation!=generation {return;}
                    view.computer_permissions_loading=false;
                    view.computer_permission_note=Some(match result {Some(Ok(()))=>"Access reset. Reopen Dragent and grant the permissions again when needed.".into(),Some(Err(error))=>error,None=>"Permission reset stopped unexpectedly.".into()});
                    view.refresh_computer_permissions(ctx);
                    ctx.notify();
                });
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::OpenComputerPermission(kind) => {
                let kind = *kind;
                let (tx, rx) = futures::channel::oneshot::channel();
                crate::bg::detach(move || {
                    dragent_computer::request_permission(kind);
                    let _ = tx.send(dragent_computer::open_permission_settings(kind));
                });
                ctx.spawn(async move { rx.await.ok() }, |view, result, ctx| {
                    view.computer_permission_note = result.and_then(Result::err);
                    view.refresh_computer_permissions(ctx);
                    ctx.notify();
                });
            }
            SettingsAction::ToggleComputerUse => {
                self.app_settings.computer_use.enabled = !self.app_settings.computer_use.enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleComputerUseConfirm => {
                self.app_settings.computer_use.require_confirmation =
                    !self.app_settings.computer_use.require_confirmation;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::AllowComputerApp(app) => {
                let app = if app.trim().is_empty() {
                    self.computer_app_draft.clone()
                } else {
                    app.clone()
                };
                if app.trim().is_empty() {
                    return;
                }
                self.app_settings.computer_use.allow_app(&app);
                self.app_settings.save();
                self.computer_app_draft.clear();
                ctx.update_view(&self.computer_app_editor, |e, ectx| e.set_text("", ectx));
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::RemoveComputerApp(app) => {
                self.app_settings.computer_use.remove_app(app);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            _ => {}
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl SettingsView {
    pub(crate) fn refresh_computer_permissions(&mut self, ctx: &mut ViewContext<Self>) {
        self.computer_permission_visible = true;
        if self.computer_permissions_loading {
            return;
        }
        self.computer_permissions_loading = true;
        self.computer_permission_generation += 1;
        let generation = self.computer_permission_generation;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let _ = tx.send(dragent_computer::permissions());
        });
        ctx.spawn(async move { rx.await.ok() }, move |view, report, ctx| {
            if view.computer_permission_generation != generation {
                return;
            }
            view.computer_permissions = report;
            view.computer_permissions_loading = false;
            view.arm_computer_permission_poll(ctx);
            ctx.notify();
        });
        ctx.notify();
    }
}

#[cfg(not(target_family = "wasm"))]
impl SettingsView {
    fn arm_computer_permission_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.computer_permission_visible
            || self.computer_permission_polling
            || self.section != super::SettingsSection::ComputerUse
        {
            return;
        }
        self.computer_permission_polling = true;
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(3)),
            |view, _, ctx| {
                view.computer_permission_polling = false;
                if view.computer_permission_visible
                    && view.section == super::SettingsSection::ComputerUse
                {
                    view.refresh_computer_permissions(ctx);
                    view.arm_computer_permission_poll(ctx);
                }
            },
        );
    }
}
