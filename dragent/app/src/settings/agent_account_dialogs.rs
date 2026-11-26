//! The Agent Accounts page's pop-ups: add an account (or replace its key),
//! signed in, rename and remove. The subscription sign-in between them is the
//! workspace's pop-up terminal, in the same place on screen.
use super::agent_accounts::{
    agent_label, machine_label_for, AccountAction, AccountDialog, AccountForm, Editing, FormStep,
};
use super::api_key_check::key_issuer;
use super::{SettingsAction, SettingsView};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Size, Tone, Variant};
use crate::shared_input::{shared_input, SharedInputOpts};
use crate::theme::Theme;
use crate::workspace::InputEditor;
use dragent_host::agent_accounts::CredentialKind;
use warpui::elements::{
    Container, CrossAxisAlignment, DispatchEventResult, Element, EventHandler, Expanded, Flex,
    ParentElement,
};
use warpui::platform::Cursor;
use warpui::ViewHandle;
use warpui_core::elements::Hoverable;
use warpui_core::scene::{Border, CornerRadius, Radius};

const WIDTH: f32 = 520.0;

fn act(action: AccountAction) -> SettingsAction {
    SettingsAction::AgentAccount(action)
}

impl SettingsView {
    /// The pop-up over the Agent Accounts page, if one is open.
    pub(super) fn render_account_dialog(
        &self,
        theme: &Theme,
        app: &warpui::AppContext,
    ) -> Option<Box<dyn Element>> {
        let state = &self.agent_accounts;
        match &state.dialog {
            AccountDialog::Form(form) => return Some(self.account_form(theme, form)),
            AccountDialog::Done {
                provider,
                label,
                note,
                make_default,
                ..
            } => {
                return Some(self.account_done(theme, provider, label, note.as_deref(), *make_default))
            }
            // The pop-up terminal shows this step.
            AccountDialog::Waiting { .. } | AccountDialog::Closed => {}
        }
        let snapshot = state.snapshot();
        if let Some(Editing::Rename(id)) = &state.editing {
            let profile = snapshot.profiles.iter().find(|p| p.id == *id)?;
            return Some(self.account_rename(theme, &profile.provider, &profile.label));
        }
        let id = state.pending_remove?;
        let profile = snapshot.profiles.iter().find(|p| p.id == id)?;
        let machine = match &state.selected {
            None => "this computer".to_string(),
            selected => machine_label_for(app, selected),
        };
        Some(self.account_remove(theme, &profile.provider, &profile.label, &machine))
    }

    fn account_form(&self, theme: &Theme, form: &AccountForm) -> Box<dyn Element> {
        let ui = self.ui_font;
        let agent = agent_label(&form.provider);
        let busy = form.step != FormStep::Editing;
        let issuer = key_issuer(&form.provider);
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(18.0);
        if form.target.is_none() {
            let plans = if form.provider == "codex" {
                "Your ChatGPT plan"
            } else {
                "Your Pro or Max plan"
            };
            body = body
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_spacing(10.0)
                        .with_child(self.kind_card(theme, form, CredentialKind::Subscription, AppIcon::User, "Subscription", plans))
                        .with_child(self.kind_card(theme, form, CredentialKind::ApiKey, AppIcon::Key, "API key", "Pay for what you use"))
                        .finish(),
                )
                .with_child(mui::field(
                    ui,
                    theme,
                    "Name",
                    self.account_input(theme, &self.agent_accounts.name, None),
                    None,
                ));
        }
        if form.kind == CredentialKind::ApiKey {
            let show = mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("accounts-key-show"),
                None,
                if form.show_key { "Hide" } else { "Show" },
                Variant::Ghost,
                Size::Xs,
                true,
                act(AccountAction::ToggleShowKey),
            );
            let place = if form.machine_id.is_some() {
                format!("on {}", form.machine_label)
            } else {
                "on this computer".to_string()
            };
            body = body.with_child(mui::field(
                ui,
                theme,
                "API key",
                self.account_input(theme, &self.agent_accounts.key, Some(show)),
                Some(&format!(
                    "Checked with {issuer}, then kept only {place}. It never goes to Dragent's servers."
                )),
            ));
        } else {
            body = body.with_child(mui::text(
                ui,
                "Next, you sign in with your browser. Skills, settings and MCP servers stay separate from your other accounts.",
                12.0,
                theme.disabled_text(),
            ));
        }
        if let Some(error) = &form.error {
            body = body.with_child(mui::notice(
                theme,
                ui,
                Tone::Error,
                if form.offer_unchecked { "Key not checked" } else { "Not saved yet" },
                Some(error.clone()),
                None,
            ));
        }
        let primary = match (form.step, form.kind, &form.target) {
            (FormStep::Checking, ..) => "Checking key…",
            (FormStep::Saving, CredentialKind::ApiKey, _) => "Saving…",
            (FormStep::Saving, ..) => "Opening sign-in…",
            (_, CredentialKind::Subscription, _) => "Continue",
            (_, _, Some(_)) => "Save key",
            (_, _, None) => "Add account",
        };
        let unchecked = form.offer_unchecked.then(|| {
            mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("accounts-form-unchecked"),
                None,
                "Add without checking",
                Variant::Ghost,
                Size::Md,
                !busy,
                act(AccountAction::AddUnchecked),
            )
        });
        let footer = mui::dialog_footer(
            unchecked,
            vec![
                self.account_button(theme, "accounts-form-cancel", "Cancel", Variant::Secondary, true, AccountAction::CloseDialog),
                self.account_button(theme, "accounts-form-submit", primary, Variant::Primary, !busy, AccountAction::ConfirmAdd),
            ],
        );
        let (title, subtitle) = match &form.target {
            Some((_, label)) => (
                format!("Replace the key for {label}"),
                agent.clone(),
            ),
            None => (
                format!("Add {agent} account"),
                format!("On {}", if form.machine_id.is_some() { form.machine_label.as_str() } else { "this computer" }),
            ),
        };
        self.account_dialog(
            theme,
            &form.provider,
            &title,
            Some(&subtitle),
            self.account_keys(body.finish(), AccountAction::ConfirmAdd, AccountAction::CloseDialog),
            footer,
            AccountAction::CloseDialog,
        )
    }

    /// One way to pay, as a card; the chosen one has an accent outline.
    fn kind_card(
        &self,
        theme: &Theme,
        form: &AccountForm,
        kind: CredentialKind,
        icon: AppIcon,
        title: &'static str,
        subtitle: &'static str,
    ) -> Box<dyn Element> {
        let selected = form.kind == kind;
        let ui = self.ui_font;
        let id = self.view_id;
        let theme = *theme;
        let card = Hoverable::new(self.hover(&format!("accounts-kind-{}", kind.as_str())), move |hover| {
            let color = if selected { theme.accent } else { theme.sub_text() };
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(11.0)
                .with_child(render_icon(icon, color, 17.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_spacing(3.0)
                            .with_child(mui::medium(ui, title, 13.0, theme.main_text()))
                            .with_child(mui::text(ui, subtitle, 11.5, theme.sub_text()))
                            .finish(),
                    )
                    .finish(),
                )
                .finish();
            Container::new(row)
                .with_background_color(if selected {
                    mui::alpha(theme.accent, 22)
                } else if hover.is_hovered() {
                    theme.surface_2()
                } else {
                    theme.surface_1()
                })
                .with_border(Border::all(if selected { 1.5 } else { 1.0 }).with_border_color(if selected {
                    theme.accent
                } else {
                    theme.outline()
                }))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CARD)))
                .with_padding_left(13.0)
                .with_padding_right(13.0)
                .with_padding_top(12.0)
                .with_padding_bottom(12.0)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, act(AccountAction::SetKind(kind))))
        .finish();
        Expanded::new(1.0, card).finish()
    }

    fn account_done(
        &self,
        theme: &Theme,
        provider: &str,
        label: &str,
        note: Option<&str>,
        make_default: Option<bool>,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(18.0)
            .with_child(mui::notice(
                theme,
                ui,
                Tone::Success,
                &format!("{label} is set up"),
                note.map(str::to_string),
                None,
            ));
        if let Some(on) = make_default {
            body = body.with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(12.0)
                    .with_child(
                        Expanded::new(
                            1.0,
                            mui::text(ui, "Use for new sessions", 13.0, theme.main_text()),
                        )
                        .finish(),
                    )
                    .with_child(mui::switch(
                        theme,
                        self.view_id,
                        self.hover("accounts-done-default"),
                        on,
                        act(AccountAction::ToggleDialogDefault),
                    ))
                    .finish(),
            );
        }
        let footer = mui::dialog_footer(
            None,
            vec![self.account_button(theme, "accounts-done", "Done", Variant::Primary, true, AccountAction::FinishDialog)],
        );
        self.account_dialog(
            theme,
            provider,
            &format!("{} account", agent_label(provider)),
            None,
            body.finish(),
            footer,
            AccountAction::CloseDialog,
        )
    }

    fn account_rename(&self, theme: &Theme, provider: &str, label: &str) -> Box<dyn Element> {
        let body = mui::field(
            self.ui_font,
            theme,
            "Name",
            self.account_input(theme, &self.agent_accounts.name, None),
            Some("Only the name changes; the sign-in stays."),
        );
        let footer = mui::dialog_footer(
            None,
            vec![
                self.account_button(theme, "accounts-rename-cancel", "Cancel", Variant::Secondary, true, AccountAction::CancelEdit),
                self.account_button(theme, "accounts-rename-save", "Save", Variant::Primary, true, AccountAction::Save),
            ],
        );
        self.account_dialog(
            theme,
            provider,
            &format!("Rename {label}"),
            None,
            self.account_keys(body, AccountAction::Save, AccountAction::CancelEdit),
            footer,
            AccountAction::CancelEdit,
        )
    }

    fn account_remove(&self, theme: &Theme, provider: &str, label: &str, machine: &str) -> Box<dyn Element> {
        let body = mui::text(
            self.ui_font,
            format!("Removes {label} from {machine}. Its sign-in files stay on that machine."),
            13.0,
            theme.sub_text(),
        );
        let footer = mui::dialog_footer(
            None,
            vec![
                self.account_button(theme, "accounts-remove-cancel", "Cancel", Variant::Secondary, true, AccountAction::CancelRemove),
                self.account_button(theme, "accounts-remove-confirm", "Remove", Variant::Destructive, true, AccountAction::ConfirmRemove),
            ],
        );
        self.account_dialog(
            theme,
            provider,
            &format!("Remove {label}?"),
            None,
            body,
            footer,
            AccountAction::CancelRemove,
        )
    }

    /// The shared frame. Only its own controls close it: a click beside it
    /// must not throw away what was typed.
    #[allow(clippy::too_many_arguments)]
    fn account_dialog(
        &self,
        theme: &Theme,
        provider: &str,
        title: &str,
        subtitle: Option<&str>,
        body: Box<dyn Element>,
        footer: Box<dyn Element>,
        close: AccountAction,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let mut labels = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(3.0)
            .with_child(mui::semibold(ui, title, 16.0, theme.main_text()));
        if let Some(subtitle) = subtitle {
            labels = labels.with_child(mui::text(ui, subtitle, 12.5, theme.sub_text()));
        }
        let mut header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(13.0);
        if let Some(agent) = crate::workspace::PromptAgent::ALL
            .into_iter()
            .find(|agent| agent.command_key() == provider)
        {
            header = header.with_child(crate::workspace::agent_image(agent, theme, 28.0, 1.0));
        }
        let header = header
            .with_child(Expanded::new(1.0, labels.finish()).finish())
            .with_child(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("accounts-dialog-x"),
                Some(AppIcon::XClose),
                "",
                Variant::Ghost,
                Size::Xs,
                true,
                act(close),
            ))
            .finish();
        mui::dialog::<SettingsAction>(
            theme,
            ui,
            self.view_id,
            self.hover("accounts-dialog-unused"),
            &self.agent_accounts.scroll,
            header,
            body,
            Some(footer),
            WIDTH,
            None,
        )
    }

    fn account_button(
        &self,
        theme: &Theme,
        key: &str,
        label: &str,
        variant: Variant,
        enabled: bool,
        action: AccountAction,
    ) -> Box<dyn Element> {
        mui::button(
            theme,
            self.ui_font,
            self.view_id,
            self.hover(key),
            None,
            label,
            variant,
            Size::Md,
            enabled,
            act(action),
        )
    }

    fn account_input(
        &self,
        theme: &Theme,
        editor: &ViewHandle<InputEditor>,
        trailing: Option<Box<dyn Element>>,
    ) -> Box<dyn Element> {
        shared_input(
            theme,
            editor,
            SharedInputOpts {
                height: Some(mui::CONTROL_HEIGHT),
                radius: mui::RADIUS_CONTROL,
                pad_left: 11.0,
                pad_right: if trailing.is_some() { 4.0 } else { 11.0 },
                pad_top: 0.0,
                pad_bottom: 0.0,
                trailing,
                ..Default::default()
            },
        )
    }

    /// Enter and Escape in a pop-up's fields.
    fn account_keys(&self, body: Box<dyn Element>, enter: AccountAction, escape: AccountAction) -> Box<dyn Element> {
        let id = self.view_id;
        EventHandler::new(body)
            .on_keydown(move |ctx, _, keystroke| match keystroke.key.as_str() {
                "enter" => {
                    ctx.dispatch_typed_action_for(id, act(enter.clone()));
                    DispatchEventResult::StopPropagation
                }
                "escape" => {
                    ctx.dispatch_typed_action_for(id, act(escape.clone()));
                    DispatchEventResult::StopPropagation
                }
                _ => DispatchEventResult::PropagateToParent,
            })
            .finish()
    }
}
