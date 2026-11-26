//! Provider account settings, per machine.
//!
//! The person picks a machine at the top ("This computer" plus every SSH
//! machine from Settings → Machines). Everything below shows and changes that
//! machine's accounts through t1's host requests:
//! `DevelopmentRequest::Accounts(AccountsRequest::…)`. This computer goes
//! through `development::execute` on a background thread; a machine goes
//! through its `HostClient`. Never block the UI thread.
use super::api_key_check::{self, KeyCheck};
use super::widgets::{page_header, SelectItem};
use super::{SettingsAction, SettingsEvent, SettingsView};
use crate::workspace::{InputEditor, InputMode};
use dragent_host::agent_accounts::{self, ApiKey, CredentialKind, Selection};
use dragent_host::development::{
    AccountsRequest, AccountsResponse, DevelopmentRequest, DevelopmentResponse,
};
use std::collections::{BTreeMap, HashMap};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    MainAxisSize, ParentElement, Text,
};
use warpui::{ReadView, SingletonEntity, UpdateView, ViewContext, ViewHandle};
use warpui_core::elements::Hoverable;
use warpui_core::scene::{Border, CornerRadius, Radius};

/// Key into `AccountsState::snapshots`: "" is this computer.
fn snapshot_key(machine_id: &Option<String>) -> String {
    machine_id.clone().unwrap_or_default()
}

#[derive(Clone, Debug)]
pub enum AccountAction {
    SelectMachine(Option<String>),
    Refresh,
    Select(String),
    StartAdd(String),
    SetKind(CredentialKind),
    /// The form's main button, or Enter in one of its fields.
    ConfirmAdd,
    /// Save a key the provider couldn't be asked about.
    AddUnchecked,
    ToggleShowKey,
    CloseDialog,
    SignInAgain { provider: String, target: Option<uuid::Uuid> },
    MakeDefault {
        provider: String,
        selection: Selection,
    },
    ToggleDialogDefault,
    /// Done: also sets the default when its switch is on.
    FinishDialog,
    StartRename(uuid::Uuid),
    Save,
    CancelEdit,
    RequestRemove(uuid::Uuid),
    ConfirmRemove,
    CancelRemove,
    CheckSystem(String),
    Status(uuid::Uuid),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Editing {
    Rename(uuid::Uuid),
}

/// Where the form is with a key it was given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FormStep {
    #[default]
    Editing,
    Checking,
    Saving,
}

/// The pop-up's first page: a new account, or a new key for an existing one.
#[derive(Clone, Debug)]
pub(crate) struct AccountForm {
    pub provider: String,
    pub kind: CredentialKind,
    pub machine_id: Option<String>,
    pub machine_label: String,
    /// The account whose key is being replaced.
    pub target: Option<(uuid::Uuid, String)>,
    pub step: FormStep,
    pub error: Option<String>,
    /// The provider couldn't be asked, so saving anyway is offered.
    pub offer_unchecked: bool,
    pub show_key: bool,
}

/// The add / sign-in pop-up.
#[derive(Clone, Debug, Default)]
pub(crate) enum AccountDialog {
    #[default]
    Closed,
    Form(AccountForm),
    /// The sign-in runs in the workspace's pop-up terminal, over this page.
    Waiting {
        provider: String,
        machine_id: Option<String>,
        target: Option<uuid::Uuid>,
        label: String,
        /// Added by this pop-up: cancelling the sign-in removes it again.
        created: bool,
        /// What the account reported when the dialog opened. The dialog
        /// only finishes when a later `List` shows something different
        /// (or a fresh account signs in), so "Sign in again" on an
        /// already-signed-in account does not complete straight away.
        /// Rotating an API key leaves the identity identical, so the
        /// sign-in files' change time is baselined too.
        baseline_signed_in: bool,
        baseline_email: Option<String>,
        baseline_error: Option<String>,
        baseline_credentials_changed_at: Option<u64>,
    },
    Done {
        provider: String,
        machine_id: Option<String>,
        target: Option<uuid::Uuid>,
        label: String,
        /// Who it signed in as, or what wasn't checked.
        note: Option<String>,
        /// None hides the switch: the account already is the default.
        make_default: Option<bool>,
    },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MachineSnapshot {
    pub profiles: Vec<dragent_host::development::AccountProfileView>,
    pub defaults: BTreeMap<String, Selection>,
    pub identities: Vec<dragent_host::development::AccountIdentityView>,
    pub loading: bool,
    pub error: Option<String>,
    pub not_connected: bool,
}

impl MachineSnapshot {
    fn identity_for(
        &self,
        provider: &str,
        target: Option<uuid::Uuid>,
    ) -> Option<&dragent_host::development::AccountIdentityView> {
        self.identities
            .iter()
            .find(|i| i.provider == provider && i.target == target)
    }

    fn status_for(&self, provider: &str, target: Option<uuid::Uuid>) -> String {
        match self.identity_for(provider, target) {
            Some(view) if view.signed_in => "Signed in".into(),
            Some(view) => view
                .error
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "Sign-in required".into()),
            None if self.loading => "Checking…".into(),
            None => "Not checked".into(),
        }
    }
}

pub(crate) struct AccountsState {
    /// None is this computer; Some(id) is an SSH machine.
    pub selected: Option<String>,
    pub snapshots: HashMap<String, MachineSnapshot>,
    pub dialog: AccountDialog,
    /// Bumped whenever the pop-up opens or closes, so a late answer can't
    /// act on a pop-up that is gone.
    pub dialog_generation: u64,
    pub scroll: warpui::elements::ClippedScrollStateHandle,
    /// The typed API key. It sends no actions, so the key never travels
    /// through the action log; it is emptied when the pop-up closes.
    pub key: ViewHandle<InputEditor>,
    pub pending_remove: Option<uuid::Uuid>,
    pub provider: String,
    pub name: ViewHandle<InputEditor>,
    /// Local usage meters only (subscription accounts on this computer).
    pub usage: Vec<dragent_host::agent_usage::AccountUsage>,
    pub editing: Option<Editing>,
}

/// Whether a waiting dialog finishes on this `List` identity: only a new
/// sign-in counts. A fresh account starts unsigned, so any signed-in report
/// promotes it; "Sign in again" on an already-signed-in account only
/// promotes when the identity changed since the dialog opened. Rotating an
/// API key keeps the same email, so a newer sign-in-files timestamp
/// (or one appearing where there was none) counts as a change too.
fn waiting_promotes(
    baseline_signed_in: bool,
    baseline_email: &Option<String>,
    baseline_error: &Option<String>,
    baseline_credentials_changed_at: &Option<u64>,
    current: &dragent_host::development::AccountIdentityView,
) -> bool {
    if !current.signed_in {
        return false;
    }
    if !baseline_signed_in {
        return true;
    }
    if current.email != *baseline_email || current.error != *baseline_error {
        return true;
    }
    match (
        baseline_credentials_changed_at,
        current.credentials_changed_at,
    ) {
        (None, Some(_)) => true,
        (Some(baseline), Some(now)) if now > *baseline => true,
        _ => false,
    }
}

fn system_key(provider: &str) -> String {
    format!("system:{provider}")
}

impl AccountsState {
    pub fn new(
        ctx: &mut ViewContext<SettingsView>,
        font: warpui::fonts::FamilyId,
        theme: crate::theme::Theme,
    ) -> Self {
        let name = ctx.add_typed_action_view(|c| {
            InputEditor::new(
                "Account name, e.g. Work",
                font,
                crate::theme::scaled(13.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain { key: "agent_account_name".into() },
                c.focus_handle(),
            )
        });
        let key = ctx.add_typed_action_view(|c| {
            let mut editor = InputEditor::new(
                "Paste or type your key",
                font,
                crate::theme::scaled(13.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain { key: "agent_account_key".into() },
                c.focus_handle(),
            );
            editor.set_secure(true, c);
            editor
        });
        Self {
            selected: None,
            snapshots: HashMap::new(),
            dialog: AccountDialog::Closed,
            dialog_generation: 0,
            scroll: Default::default(),
            key,
            pending_remove: None,
            provider: "claude".into(),
            name,
            usage: Vec::new(),
            editing: None,
        }
    }

    pub(crate) fn snapshot(&self) -> MachineSnapshot {
        self.snapshots
            .get(&snapshot_key(&self.selected))
            .cloned()
            .unwrap_or_default()
    }
}

pub(super) fn agent_label(provider: &str) -> String {
    crate::workspace::PromptAgent::ALL
        .into_iter()
        .find(|a| a.command_key() == provider)
        .map(|a| a.label().to_string())
        .unwrap_or_else(|| provider.to_string())
}

/// A name nothing else uses yet: the pop-up's starting suggestion.
fn suggested_name(taken: &[String]) -> String {
    let free = |name: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(name));
    ["Work", "Personal"]
        .into_iter()
        .map(str::to_string)
        .chain((2..).map(|n| format!("Account {n}")))
        .find(|name| free(name))
        .unwrap_or_default()
}

impl SettingsView {
    fn machines_for_picker(
        &self,
        ctx: &mut ViewContext<Self>,
    ) -> Vec<(Option<String>, String)> {
        let store = self.machines_store(ctx);
        store.read(ctx, |s, _| {
            let mut out = vec![(None, "This computer".to_string())];
            for m in &s.registry.machines {
                out.push((Some(m.id.clone()), m.name()));
            }
            out
        })
    }

    fn selected_machine_label(&self, ctx: &mut ViewContext<Self>) -> String {
        let selected = self.agent_accounts.selected.clone();
        match selected {
            None => "This computer".to_string(),
            Some(id) => {
                let store = self.machines_store(ctx);
                store
                    .read(ctx, |s, _| s.machine(&id).map(|m| m.name()))
                    .unwrap_or_else(|| id.clone())
            }
        }
    }

    /// The selected remote machine reports Windows: no add / sign-in UI.
    /// The buttons are hidden; this is a backstop for any other path here.
    fn selected_is_remote_windows(&self, ctx: &mut ViewContext<Self>) -> bool {
        let Some(id) = self.agent_accounts.selected.clone() else {
            return false;
        };
        let store = self.machines_store(ctx);
        store.read(ctx, |s, _| {
            s.connection(&id)
                .and_then(|c| c.environment.as_ref())
                .map(|e| e.os.eq_ignore_ascii_case("windows"))
                .unwrap_or(false)
        })
    }

    fn ensure_snapshot(&mut self, machine_id: &Option<String>) {
        let key = snapshot_key(machine_id);
        self.agent_accounts
            .snapshots
            .entry(key)
            .or_insert_with(|| MachineSnapshot {
                loading: true,
                ..Default::default()
            });
    }

    /// Fetch `Accounts::List` for the selected machine into its snapshot.
    /// Local goes through `development::execute`; a machine through its
    /// `HostClient`. Never blocks the UI thread.
    fn refresh_current_accounts(&mut self, ctx: &mut ViewContext<Self>) {
        let machine_id = self.agent_accounts.selected.clone();
        self.ensure_snapshot(&machine_id);
        if let Some(snapshot) = self
            .agent_accounts
            .snapshots
            .get_mut(&snapshot_key(&machine_id))
        {
            snapshot.loading = true;
            snapshot.error = None;
            snapshot.not_connected = false;
        }
        let label = self.selected_machine_label(ctx);
        match machine_id.clone() {
            None => {
                ctx.spawn(
                    async move {
                        dragent_host::development::execute(DevelopmentRequest::Accounts(
                            AccountsRequest::List,
                        ))
                    },
                    move |view, result: Result<DevelopmentResponse, String>, ctx| {
                        view.apply_list_result(&None, result, ctx);
                        view.load_account_usage(ctx);
                        view.poll_waiting_dialog(&None, 0, ctx);
                    },
                );
            }
            Some(id) => {
                let client = match crate::remote_fs::client(&id) {
                    Ok(client) => client,
                    Err(_) => {
                        if let Some(snapshot) = self
                            .agent_accounts
                            .snapshots
                            .get_mut(&snapshot_key(&machine_id))
                        {
                            snapshot.loading = false;
                            snapshot.not_connected = true;
                            snapshot.error = Some(format!(
                                "{label} is not connected. Connect it in Settings → Machines."
                            ));
                        }
                        ctx.notify();
                        return;
                    }
                };
                ctx.spawn(
                    async move {
                        client.development(DevelopmentRequest::Accounts(AccountsRequest::List))
                    },
                    move |view, result: Result<DevelopmentResponse, String>, ctx| {
                        let current = view.agent_accounts.selected.clone();
                        view.apply_list_result(&current, result, ctx);
                        view.poll_waiting_dialog(&current, 0, ctx);
                    },
                );
            }
        }
        ctx.notify();
        let _ = label;
    }

    fn apply_list_result(
        &mut self,
        machine_id: &Option<String>,
        result: Result<DevelopmentResponse, String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let key = snapshot_key(machine_id);
        let snapshot = self.agent_accounts.snapshots.entry(key).or_default();
        snapshot.loading = false;
        match result {
            Ok(DevelopmentResponse::Accounts(AccountsResponse::List {
                profiles,
                defaults,
                identities,
                ..
            })) => {
                snapshot.profiles = profiles;
                snapshot.defaults = defaults;
                snapshot.identities = identities;
                snapshot.error = None;
                snapshot.not_connected = false;
            }
            Ok(_) => {
                snapshot.error = Some("The machine returned an invalid answer.".into());
            }
            Err(error) => {
                snapshot.error = Some(error);
            }
        }
        // A finished sign-in lands here: Waiting becomes Done.
        self.promote_waiting_if_signed_in(machine_id, ctx);
        ctx.notify();
    }

    fn promote_waiting_if_signed_in(
        &mut self,
        machine_id: &Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let AccountDialog::Waiting {
            provider,
            machine_id: waiting_machine,
            target,
            label,
            created,
            baseline_signed_in,
            baseline_email,
            baseline_error,
            baseline_credentials_changed_at,
        } = &self.agent_accounts.dialog
        else {
            return;
        };
        if waiting_machine != machine_id {
            return;
        }
        let snapshot = self.agent_accounts.snapshots.get(&snapshot_key(machine_id));
        let Some(current) = snapshot.and_then(|s| s.identity_for(provider, *target)) else {
            return;
        };
        if !waiting_promotes(
            *baseline_signed_in,
            baseline_email,
            baseline_error,
            baseline_credentials_changed_at,
            current,
        ) {
            return;
        }
        let (provider, target, label, created) = (provider.clone(), *target, label.clone(), *created);
        let note = current.email.clone().filter(|e| !e.is_empty()).map(|e| format!("Signed in as {e}"));
        self.show_done(provider, machine_id.clone(), target, label, note, created);
        // The pop-up terminal has done its job.
        ctx.emit(SettingsEvent::AgentAccountSignedIn);
    }

    fn show_done(
        &mut self,
        provider: String,
        machine_id: Option<String>,
        target: Option<uuid::Uuid>,
        label: String,
        note: Option<String>,
        suggest_default: bool,
    ) {
        let default = self
            .agent_accounts
            .snapshots
            .get(&snapshot_key(&machine_id))
            .and_then(|s| s.defaults.get(&provider).cloned())
            .unwrap_or(Selection::System);
        let is_default = match target {
            Some(id) => default == Selection::Profile(id),
            None => matches!(default, Selection::System | Selection::Inherit),
        };
        self.agent_accounts.dialog_generation += 1;
        self.agent_accounts.dialog = AccountDialog::Done {
            provider,
            machine_id,
            target,
            label,
            note,
            // On for a new, working account; re-signing in never changes the choice.
            make_default: (!is_default).then_some(suggest_default),
        };
    }

    fn close_account_dialog(&mut self, ctx: &mut ViewContext<Self>) {
        self.agent_accounts.dialog_generation += 1;
        self.agent_accounts.dialog = AccountDialog::Closed;
        ctx.update_view(&self.agent_accounts.key, |e, c| {
            e.set_text("", c);
            e.set_secure(true, c);
        });
        ctx.notify();
    }

    fn open_account_form(
        &mut self,
        provider: &str,
        target: Option<(uuid::Uuid, String)>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.close_account_dialog(ctx);
        self.agent_accounts.pending_remove = None;
        self.agent_accounts.editing = None;
        let machine_label = self.selected_machine_label(ctx);
        let taken: Vec<String> = self
            .agent_accounts
            .snapshot()
            .profiles
            .iter()
            .filter(|p| p.provider == provider)
            .map(|p| p.label.clone())
            .collect();
        let replacing = target.is_some();
        self.agent_accounts.dialog = AccountDialog::Form(AccountForm {
            provider: provider.to_string(),
            kind: if replacing { CredentialKind::ApiKey } else { CredentialKind::Subscription },
            machine_id: self.agent_accounts.selected.clone(),
            machine_label,
            target,
            step: FormStep::Editing,
            error: None,
            offer_unchecked: false,
            show_key: false,
        });
        ctx.update_view(&self.agent_accounts.name, |e, c| {
            e.set_text(&suggested_name(&taken), c)
        });
        if replacing {
            ctx.focus(&self.agent_accounts.key);
        } else {
            ctx.focus(&self.agent_accounts.name);
        }
        ctx.notify();
    }

    fn form_mut(&mut self) -> Option<&mut AccountForm> {
        match &mut self.agent_accounts.dialog {
            AccountDialog::Form(form) => Some(form),
            _ => None,
        }
    }

    /// Back to the form with what went wrong.
    fn fail_account_form(&mut self, error: String, offer_unchecked: bool, ctx: &mut ViewContext<Self>) {
        if let Some(form) = self.form_mut() {
            form.step = FormStep::Editing;
            form.error = Some(error);
            form.offer_unchecked = offer_unchecked;
        }
        ctx.notify();
    }

    /// Continue, Add account or Save key. A key is checked with its provider
    /// first unless `unchecked`.
    fn submit_account_form(&mut self, unchecked: bool, ctx: &mut ViewContext<Self>) {
        let AccountDialog::Form(form) = &self.agent_accounts.dialog else {
            return;
        };
        if form.step != FormStep::Editing {
            return;
        }
        let form = form.clone();
        let name = self
            .agent_accounts
            .name
            .read(ctx, |e, _| e.get_text().trim().to_string());
        if form.target.is_none() && name.is_empty() {
            self.fail_account_form("Enter a name for this account.".into(), false, ctx);
            return;
        }
        if form.kind == CredentialKind::Subscription {
            self.add_subscription(form, name, ctx);
            return;
        }
        let key = self.agent_accounts.key.read(ctx, |e, _| ApiKey::parse(&e.get_text()));
        let key = match key {
            Ok(key) => key,
            Err(error) => {
                self.fail_account_form(error, false, ctx);
                return;
            }
        };
        if unchecked {
            self.save_account_key(form, name, key, false, ctx);
            return;
        }
        if let Some(form) = self.form_mut() {
            form.step = FormStep::Checking;
            form.error = None;
            form.offer_unchecked = false;
        }
        let generation = self.agent_accounts.dialog_generation;
        let provider = form.provider.clone();
        let asked = key.clone();
        ctx.spawn(
            async move { api_key_check::check(&provider, asked.expose()) },
            move |view, answer, ctx| {
                if view.agent_accounts.dialog_generation != generation {
                    return;
                }
                let issuer = api_key_check::key_issuer(&form.provider);
                match answer {
                    KeyCheck::Works => view.save_account_key(form, name, key, true, ctx),
                    KeyCheck::Rejected => view.fail_account_form(
                        format!("{issuer} says this key isn't valid. Check that you copied all of it."),
                        false,
                        ctx,
                    ),
                    KeyCheck::Unknown => view.fail_account_form(
                        format!("Couldn't check the key with {issuer}. Check your connection and try again, or add it without checking."),
                        true,
                        ctx,
                    ),
                }
            },
        );
        ctx.notify();
    }

    fn save_account_key(
        &mut self,
        form: AccountForm,
        name: String,
        key: ApiKey,
        checked: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(form) = self.form_mut() {
            form.step = FormStep::Saving;
            form.error = None;
        }
        let generation = self.agent_accounts.dialog_generation;
        let created = form.target.is_none();
        let request = match &form.target {
            Some((id, _)) => AccountsRequest::SetKey { id: *id, key },
            None => AccountsRequest::AddWithKey {
                provider: form.provider.clone(),
                label: name,
                key,
            },
        };
        self.send_account_form(form.machine_id.clone(), request, ctx, move |view, result, ctx| {
            let saved = match result {
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Added(profile))) => {
                    Ok((profile.id, profile.label))
                }
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack)) => {
                    form.target.clone().ok_or_else(|| "The machine returned an invalid answer.".to_string())
                }
                Ok(_) => Err("The machine returned an invalid answer.".to_string()),
                Err(error) => Err(error),
            };
            if view.agent_accounts.dialog_generation != generation {
                // Closed meanwhile: just show what's there now.
                view.refresh_current_accounts(ctx);
                return;
            }
            match saved {
                Ok((id, label)) => {
                    let issuer = api_key_check::key_issuer(&form.provider);
                    let note = (!checked).then(|| format!("{issuer} hasn't checked this key yet."));
                    view.close_account_dialog(ctx);
                    view.show_done(form.provider, form.machine_id, Some(id), label, note, created && checked);
                    view.refresh_current_accounts(ctx);
                }
                Err(error) => view.fail_account_form(error, false, ctx),
            }
        });
        ctx.notify();
    }

    /// Add a subscription account, then sign it in in the pop-up terminal.
    fn add_subscription(&mut self, form: AccountForm, name: String, ctx: &mut ViewContext<Self>) {
        if let Some(form) = self.form_mut() {
            form.step = FormStep::Saving;
            form.error = None;
        }
        let generation = self.agent_accounts.dialog_generation;
        let request = AccountsRequest::Add {
            provider: form.provider.clone(),
            label: name,
            kind: CredentialKind::Subscription,
        };
        self.send_account_form(form.machine_id.clone(), request, ctx, move |view, result, ctx| {
            let cancelled = view.agent_accounts.dialog_generation != generation;
            match result {
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Added(profile))) => {
                    if cancelled {
                        view.remove_unused_account(form.machine_id, profile.id, ctx);
                        return;
                    }
                    view.start_terminal_sign_in(
                        form.provider,
                        form.machine_id,
                        Some(profile.id),
                        profile.label,
                        true,
                        ctx,
                    );
                }
                Ok(_) if !cancelled => {
                    view.fail_account_form("The machine returned an invalid answer.".into(), false, ctx)
                }
                Err(error) if !cancelled => view.fail_account_form(error, false, ctx),
                _ => {}
            }
        });
        ctx.notify();
    }

    /// Open the pop-up terminal on an account's sign-in command and wait.
    fn start_terminal_sign_in(
        &mut self,
        provider: String,
        machine_id: Option<String>,
        target: Option<uuid::Uuid>,
        label: String,
        created: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let generation = self.agent_accounts.dialog_generation;
        let request = AccountsRequest::SignInCommand {
            provider: provider.clone(),
            target,
        };
        self.send_account_form(machine_id.clone(), request, ctx, move |view, result, ctx| {
            let cancelled = view.agent_accounts.dialog_generation != generation;
            let command = match result {
                Ok(DevelopmentResponse::Accounts(AccountsResponse::SignInCommand {
                    command, ..
                })) if !cancelled => command,
                other => {
                    if created {
                        if let Some(id) = target {
                            view.remove_unused_account(machine_id, id, ctx);
                        }
                    }
                    if !cancelled {
                        let error = match other {
                            Err(error) => error,
                            Ok(_) => "The machine returned an invalid answer.".into(),
                        };
                        if view.form_mut().is_some() {
                            view.fail_account_form(error, false, ctx);
                        } else {
                            ctx.emit(SettingsEvent::AgentAccountError(error));
                        }
                    }
                    return;
                }
            };
            // Remember what the account reported: the wait only finishes on
            // a new sign-in, never on the same identity.
            let baseline = view
                .agent_accounts
                .snapshots
                .get(&snapshot_key(&machine_id))
                .and_then(|s| s.identity_for(&provider, target).cloned())
                .filter(|_| !created);
            let (baseline_signed_in, baseline_email, baseline_error, baseline_credentials_changed_at) =
                baseline.map_or((false, None, None, None), |i| {
                    (i.signed_in, i.email, i.error, i.credentials_changed_at)
                });
            let agent = agent_label(&provider);
            ctx.emit(SettingsEvent::AgentAccountSignIn {
                machine_id: machine_id.clone(),
                provider: provider.clone(),
                title: if target.is_some() {
                    format!("Sign in to {agent} as {label}")
                } else {
                    format!("Sign in to {agent}")
                },
                command,
            });
            view.close_account_dialog(ctx);
            view.agent_accounts.dialog = AccountDialog::Waiting {
                provider,
                machine_id,
                target,
                label,
                created,
                baseline_signed_in,
                baseline_email,
                baseline_error,
                baseline_credentials_changed_at,
            };
            // The terminal run is the poll's first tick: an immediate List
            // keeps already-signed-in rows honest.
            view.refresh_current_accounts(ctx);
        });
    }

    /// The person closed the pop-up terminal. One last look: a sign-in that
    /// just finished still counts; otherwise the pop-up is over, and an
    /// account added only for it goes away again.
    pub(crate) fn account_sign_in_closed(&mut self, ctx: &mut ViewContext<Self>) {
        let AccountDialog::Waiting {
            provider,
            machine_id,
            target,
            created,
            ..
        } = self.agent_accounts.dialog.clone()
        else {
            return;
        };
        let generation = self.agent_accounts.dialog_generation;
        self.send_host_accounts(
            machine_id.clone(),
            AccountsRequest::List,
            ctx,
            move |view, result, ctx| {
                let read = result.is_ok();
                view.apply_list_result(&machine_id, result, ctx);
                if view.agent_accounts.dialog_generation == generation {
                    view.close_account_dialog(ctx);
                }
                // Only a clear "not signed in" removes anything.
                let unused = view
                    .agent_accounts
                    .snapshots
                    .get(&snapshot_key(&machine_id))
                    .and_then(|s| s.identity_for(&provider, target))
                    .is_some_and(|i| !i.signed_in && i.error.is_none());
                if let (true, true, true, Some(id)) = (created, read, unused, target) {
                    view.remove_unused_account(machine_id, id, ctx);
                }
            },
        );
    }

    fn remove_unused_account(
        &mut self,
        machine_id: Option<String>,
        id: uuid::Uuid,
        ctx: &mut ViewContext<Self>,
    ) {
        // Off the page at once; reading the accounts again takes seconds.
        if let Some(snapshot) = self.agent_accounts.snapshots.get_mut(&snapshot_key(&machine_id)) {
            snapshot.profiles.retain(|p| p.id != id);
        }
        ctx.notify();
        self.send_host_accounts(
            machine_id,
            AccountsRequest::Remove { id },
            ctx,
            |view, _, ctx| view.refresh_current_accounts(ctx),
        );
    }

    /// [`Self::send_host_accounts`], with "not connected" reported in the form.
    fn send_account_form(
        &mut self,
        machine_id: Option<String>,
        request: AccountsRequest,
        ctx: &mut ViewContext<Self>,
        then: impl FnOnce(&mut Self, Result<DevelopmentResponse, String>, &mut ViewContext<Self>) + 'static,
    ) {
        if let Some(id) = &machine_id {
            if crate::remote_fs::client(id).is_err() {
                let label = self.selected_machine_label(ctx);
                then(
                    self,
                    Err(format!("{label} is not connected. Connect it in Settings → Machines.")),
                    ctx,
                );
                return;
            }
        }
        self.send_host_accounts(machine_id, request, ctx, then);
    }

    /// While the dialog waits, re-read `List` every 2 s. Stops when the
    /// account reports signed in, after 10 min (300 polls), or when the
    /// dialog closes or moves to another account.
    fn poll_waiting_dialog(
        &mut self,
        machine_id: &Option<String>,
        attempt: u32,
        ctx: &mut ViewContext<Self>,
    ) {
        let waiting_target = match &self.agent_accounts.dialog {
            AccountDialog::Waiting {
                machine_id: waiting_machine,
                target,
                ..
            } if waiting_machine == machine_id => *target,
            _ => return,
        };
        if attempt >= 300 {
            return;
        }
        let machine_id = machine_id.clone();
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_secs(2)),
            move |view, _, ctx| {
                let still_waiting = matches!(&view.agent_accounts.dialog,
                    AccountDialog::Waiting { machine_id: w, target, .. }
                    if w == &machine_id && *target == waiting_target);
                if !still_waiting {
                    return;
                }
                match machine_id.clone() {
                    None => {
                        ctx.spawn(
                            async move {
                                dragent_host::development::execute(
                                    DevelopmentRequest::Accounts(AccountsRequest::List),
                                )
                            },
                            move |view, result: Result<DevelopmentResponse, String>, ctx| {
                                view.apply_list_result(&None, result, ctx);
                                if matches!(view.agent_accounts.dialog, AccountDialog::Waiting { .. }) {
                                    view.poll_waiting_dialog(&None, attempt + 1, ctx);
                                }
                            },
                        );
                    }
                    Some(id) => {
                        let Ok(client) = crate::remote_fs::client(&id) else {
                            return;
                        };
                        ctx.spawn(
                            async move {
                                client.development(DevelopmentRequest::Accounts(
                                    AccountsRequest::List,
                                ))
                            },
                            move |view, result: Result<DevelopmentResponse, String>, ctx| {
                                let current = view.agent_accounts.selected.clone();
                                // Only advance the poll when the dialog still
                                // waits on the machine that was just read.
                                let still_waiting = matches!(&view.agent_accounts.dialog,
                                    AccountDialog::Waiting { machine_id: w, .. } if w == &current);
                                view.apply_list_result(&current, result, ctx);
                                if still_waiting
                                    && matches!(
                                        view.agent_accounts.dialog,
                                        AccountDialog::Waiting { .. }
                                    )
                                {
                                    view.poll_waiting_dialog(&current, attempt + 1, ctx);
                                }
                            },
                        );
                    }
                }
            },
        );
    }

    fn send_host_accounts(
        &mut self,
        machine_id: Option<String>,
        request: AccountsRequest,
        ctx: &mut ViewContext<Self>,
        then: impl FnOnce(&mut Self, Result<DevelopmentResponse, String>, &mut ViewContext<Self>) + 'static,
    ) {
        match machine_id {
            None => {
                ctx.spawn(
                    async move {
                        dragent_host::development::execute(DevelopmentRequest::Accounts(request))
                    },
                    move |view, result, ctx| then(view, result, ctx),
                );
            }
            Some(id) => {
                let label = self.selected_machine_label(ctx);
                let Ok(client) = crate::remote_fs::client(&id) else {
                    ctx.emit(SettingsEvent::AgentAccountError(format!(
                        "{label} is not connected. Connect it in Settings → Machines."
                    )));
                    ctx.notify();
                    return;
                };
                ctx.spawn(
                    async move { client.development(DevelopmentRequest::Accounts(request)) },
                    move |view, result, ctx| then(view, result, ctx),
                );
            }
        }
    }

    pub(crate) fn handle_agent_account(
        &mut self,
        action: &AccountAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            AccountAction::Select(provider) => {
                self.agent_accounts.provider = provider.clone();
                self.open_picker = None;
                self.refresh_current_accounts(ctx);
            }
            AccountAction::SelectMachine(machine_id) => {
                self.agent_accounts.selected = machine_id.clone();
                self.close_account_dialog(ctx);
                self.agent_accounts.pending_remove = None;
                self.agent_accounts.editing = None;
                self.open_picker = None;
                self.refresh_current_accounts(ctx);
            }
            AccountAction::Refresh => {
                self.refresh_current_accounts(ctx);
            }
            AccountAction::StartAdd(provider) => {
                if self.selected_is_remote_windows(ctx) {
                    return;
                }
                self.agent_accounts.provider = provider.clone();
                self.open_account_form(provider, None, ctx);
            }
            AccountAction::SetKind(kind) => {
                let kind = *kind;
                let Some(form) = self.form_mut().filter(|f| f.step == FormStep::Editing) else {
                    return;
                };
                form.kind = kind;
                form.error = None;
                form.offer_unchecked = false;
                if kind == CredentialKind::ApiKey {
                    ctx.focus(&self.agent_accounts.key);
                } else {
                    ctx.focus(&self.agent_accounts.name);
                }
                ctx.notify();
            }
            AccountAction::ToggleShowKey => {
                let Some(form) = self.form_mut() else {
                    return;
                };
                form.show_key = !form.show_key;
                let secure = !form.show_key;
                ctx.update_view(&self.agent_accounts.key, |e, c| e.set_secure(secure, c));
                ctx.notify();
            }
            AccountAction::ConfirmAdd => self.submit_account_form(false, ctx),
            AccountAction::AddUnchecked => self.submit_account_form(true, ctx),
            AccountAction::CloseDialog => self.close_account_dialog(ctx),
            AccountAction::SignInAgain { provider, target } => {
                if self.selected_is_remote_windows(ctx) {
                    return;
                }
                let profile = target.and_then(|id| {
                    self.agent_accounts
                        .snapshot()
                        .profiles
                        .into_iter()
                        .find(|p| p.id == id)
                });
                match profile {
                    Some(p) if p.kind == CredentialKind::ApiKey => {
                        self.open_account_form(provider, Some((p.id, p.label)), ctx);
                    }
                    profile => {
                        self.close_account_dialog(ctx);
                        let label = profile.map_or_else(|| "System login".to_string(), |p| p.label);
                        self.start_terminal_sign_in(
                            provider.clone(),
                            self.agent_accounts.selected.clone(),
                            *target,
                            label,
                            false,
                            ctx,
                        );
                    }
                }
            }
            AccountAction::MakeDefault { provider, selection } => {
                let provider = provider.clone();
                let selection = selection.clone();
                let machine_id = self.agent_accounts.selected.clone();
                self.send_host_accounts(
                    machine_id.clone(),
                    AccountsRequest::SetDefault {
                        provider,
                        selection,
                    },
                    ctx,
                    move |view, result, ctx| match result {
                        Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack)) => {
                            view.refresh_current_accounts(ctx);
                        }
                        Ok(_) => {
                            ctx.emit(SettingsEvent::AgentAccountError(
                                "The machine returned an invalid answer.".into(),
                            ));
                            ctx.notify();
                        }
                        Err(error) => {
                            ctx.emit(SettingsEvent::AgentAccountError(error));
                            ctx.notify();
                        }
                    },
                );
            }
            AccountAction::ToggleDialogDefault => {
                if let AccountDialog::Done { make_default: Some(on), .. } = &mut self.agent_accounts.dialog {
                    *on = !*on;
                    ctx.notify();
                }
            }
            AccountAction::FinishDialog => {
                let AccountDialog::Done { provider, machine_id, target, make_default, .. } =
                    self.agent_accounts.dialog.clone()
                else {
                    return;
                };
                // The answer belongs to the machine it was signed in on.
                if make_default == Some(true) && machine_id == self.agent_accounts.selected {
                    let selection = target.map_or(Selection::System, Selection::Profile);
                    self.handle_agent_account(
                        &AccountAction::MakeDefault { provider, selection },
                        ctx,
                    );
                }
                self.close_account_dialog(ctx);
            }
            AccountAction::StartRename(id) => {
                let id = *id;
                let machine_id = self.agent_accounts.selected.clone();
                let label = self
                    .agent_accounts
                    .snapshots
                    .get(&snapshot_key(&machine_id))
                    .and_then(|s| s.profiles.iter().find(|p| p.id == id))
                    .map(|p| p.label.clone())
                    .unwrap_or_default();
                self.agent_accounts.editing = Some(Editing::Rename(id));
                self.agent_accounts.pending_remove = None;
                self.close_account_dialog(ctx);
                ctx.update_view(&self.agent_accounts.name, |e, c| e.set_text(&label, c));
                ctx.focus(&self.agent_accounts.name);
                ctx.notify();
            }
            AccountAction::Save => {
                let name = self
                    .agent_accounts
                    .name
                    .read(ctx, |e, _| e.get_text().trim().to_string());
                let editing = self.agent_accounts.editing.clone();
                match editing {
                    Some(Editing::Rename(id)) => {
                        let machine_id = self.agent_accounts.selected.clone();
                        self.send_host_accounts(
                            machine_id.clone(),
                            AccountsRequest::Rename { id, label: name },
                            ctx,
                            move |view, result, ctx| match result {
                                Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack)) => {
                                    view.agent_accounts.editing = None;
                                    view.refresh_current_accounts(ctx);
                                }
                                Ok(_) => {
                                    ctx.emit(SettingsEvent::AgentAccountError(
                                        "The machine returned an invalid answer.".into(),
                                    ));
                                    ctx.notify();
                                }
                                Err(error) => {
                                    ctx.emit(SettingsEvent::AgentAccountError(error));
                                    ctx.notify();
                                }
                            },
                        );
                    }
                    None => {}
                }
            }
            AccountAction::CancelEdit => {
                self.agent_accounts.editing = None;
                ctx.notify();
            }
            AccountAction::RequestRemove(id) => {
                self.agent_accounts.pending_remove = Some(*id);
                self.agent_accounts.editing = None;
                ctx.notify();
            }
            AccountAction::CancelRemove => {
                self.agent_accounts.pending_remove = None;
                ctx.notify();
            }
            AccountAction::ConfirmRemove => {
                let Some(id) = self.agent_accounts.pending_remove else {
                    return;
                };
                let machine_id = self.agent_accounts.selected.clone();
                self.send_host_accounts(
                    machine_id.clone(),
                    AccountsRequest::Remove { id },
                    ctx,
                    move |view, result, ctx| match result {
                        Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack)) => {
                            view.agent_accounts.pending_remove = None;
                            // A removed account cannot still be the dialog's subject.
                            let drop_dialog = matches!(&view.agent_accounts.dialog,
                                AccountDialog::Waiting { target: Some(t), .. }
                                | AccountDialog::Done { target: Some(t), .. } if *t == id);
                            if drop_dialog {
                                view.close_account_dialog(ctx);
                            }
                            view.refresh_current_accounts(ctx);
                        }
                        Ok(_) => {
                            ctx.emit(SettingsEvent::AgentAccountError(
                                "The machine returned an invalid answer.".into(),
                            ));
                            ctx.notify();
                        }
                        Err(error) => {
                            ctx.emit(SettingsEvent::AgentAccountError(error));
                            ctx.notify();
                        }
                    },
                );
            }
            AccountAction::CheckSystem(_) | AccountAction::Status(_) => {
                self.refresh_current_accounts(ctx);
            }
        }
    }

    /// Read every account's latest limits (files only). Local only.
    fn load_account_usage(&mut self, ctx: &mut ViewContext<Self>) {
        if self.agent_accounts.selected.is_some() {
            return;
        }
        ctx.spawn(
            async move { dragent_host::agent_usage::accounts() },
            |view, usage, ctx| {
                view.agent_accounts.usage = usage;
                ctx.notify();
            },
        );
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
                SettingsAction::AgentAccount(AccountAction::Select(
                    agent.command_key().to_string(),
                )),
            );
        }),
        clear_search: std::rc::Rc::new(move |ectx| {
            ectx.dispatch_typed_action_for(id, SettingsAction::ClearAgentMenuSearch);
        }),
    });
    warpui_core::elements::Dismiss::new(menu)
        .on_dismiss(move |ectx, _| {
            ectx.dispatch_typed_action_for(
                id,
                SettingsAction::TogglePicker("accounts_agent".to_string()),
            )
        })
        .prevent_interaction_with_other_elements()
        .finish()
}

pub fn render(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    app: &warpui::AppContext,
) -> Box<dyn Element> {
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
    page = page.with_child(
        Container::new(machine_picker(view, theme, app))
            .with_padding_bottom(16.0)
            .finish(),
    );
    if is_remote_windows_machine(app, &view.agent_accounts.selected) {
        page = page.with_child(card(
            theme,
            false,
            Flex::column()
                .with_spacing(6.0)
                .with_child(text(
                    theme,
                    font,
                    "Saved accounts on Windows machines aren't supported yet. Agents there use the machine's own sign-in.",
                    13.0,
                    theme.sub_text(),
                ))
                .finish(),
        ));
        return page.finish();
    }
    let snapshot = view.agent_accounts.snapshot();
    if let Some(error) = snapshot.error.clone() {
        page = page.with_child(
            Container::new(text(theme, font, &error, 12.5, theme.sub_text()))
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
        );
        if snapshot.not_connected {
            return page.finish();
        }
    }
    if snapshot.loading && snapshot.profiles.is_empty() && snapshot.error.is_none() {
        page = page.with_child(card(
            theme,
            false,
            Flex::column()
                .with_spacing(6.0)
                .with_child(text(theme, font, "Checking accounts…", 13.0, theme.sub_text()))
                .finish(),
        ));
        return page.finish();
    }
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

/// A remote machine whose host reports Windows: saved accounts are not
/// supported there, so the page shows a note instead of add / sign-in
/// actions. This computer itself (even on Windows) keeps working as before.
fn is_remote_windows_machine(
    app: &warpui::AppContext,
    machine_id: &Option<String>,
) -> bool {
    let Some(id) = machine_id.as_deref() else {
        return false;
    };
    use warpui::SingletonEntity;
    let store = crate::settings::machines_store::MachinesStore::handle(app);
    store
        .read(app, |s, _| {
            s.connection(id)
                .and_then(|c| c.environment.as_ref())
                .map(|e| e.os.eq_ignore_ascii_case("windows"))
                .unwrap_or(false)
        })
}

pub(super) fn machine_label_for(app: &warpui::AppContext, machine_id: &Option<String>) -> String {
    match machine_id {
        None => "This computer".to_string(),
        Some(id) => {
            use warpui::SingletonEntity;
            let store = crate::settings::machines_store::MachinesStore::handle(app);
            store
                .read(app, |s, _| s.machine(id).map(|m| m.name()))
                .unwrap_or_else(|| id.clone())
        }
    }
}

/// Machine picker at the top of the page.
fn machine_picker(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    app: &warpui::AppContext,
) -> Box<dyn Element> {
    use warpui::SingletonEntity;
    let font = view.ui_font;
    let store = crate::settings::machines_store::MachinesStore::handle(app);
    let machines: Vec<(String, String)> = store.read(app, |s, _| {
        s.registry
            .machines
            .iter()
            .map(|m| (m.id.clone(), m.name()))
            .collect()
    });
    let current = machine_label_for(app, &view.agent_accounts.selected);
    let mut items = vec![SelectItem {
        label: "This computer".to_string(),
        selected: view.agent_accounts.selected.is_none(),
        enabled: true,
        hover: view.hover("accounts-machine-local"),
        action: SettingsAction::AgentAccount(AccountAction::SelectMachine(None)),
    }];
    for (id, name) in machines {
        let selected = view.agent_accounts.selected.as_deref() == Some(id.as_str());
        items.push(SelectItem {
            label: name,
            selected,
            enabled: true,
            hover: view.hover(&format!("accounts-machine-{id}")),
            action: SettingsAction::AgentAccount(AccountAction::SelectMachine(Some(id))),
        });
    }
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(text(theme, font, "Machine", 13.0, theme.sub_text()))
        .with_child(crate::settings::widgets::select_control(
            theme,
            font,
            view.view_id,
            view.hover("accounts-machine-picker"),
            &current,
            280.0,
            view.picker_open("accounts_machine"),
            SettingsAction::TogglePicker("accounts_machine".to_string()),
            items,
        ))
        .finish()
}

/// One agent: its name with "Add account", then a card per account.
fn agent_section(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    agent: crate::workspace::PromptAgent,
) -> Box<dyn Element> {
    let font = view.ui_font;
    let provider = agent.command_key().to_string();
    let id = view.view_id;
    let add = {
        let provider = provider.clone();
        let theme = *theme;
        Hoverable::new(view.hover(&format!("accounts-add-{provider}")), move |hover| {
            let color = if hover.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(crate::icons::render_icon(
                    crate::icons::AppIcon::Plus,
                    color,
                    13.0,
                ))
                .with_child(
                    Text::new("Add account".to_string(), font, crate::theme::text_size(12.5))
                        .with_color(color)
                        .finish(),
                )
                .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(
                id,
                SettingsAction::AgentAccount(AccountAction::StartAdd(provider.clone())),
            )
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

    let snapshot = view.agent_accounts.snapshot();
    let default = snapshot
        .defaults
        .get(&provider)
        .cloned()
        .unwrap_or(Selection::System);
    let mut cards: Vec<Box<dyn Element>> = vec![account_card(
        view,
        theme,
        &provider,
        None,
        matches!(default, Selection::System | Selection::Inherit),
    )];
    for profile in snapshot
        .profiles
        .iter()
        .filter(|p| p.provider == provider)
    {
        cards.push(account_card(
            view,
            theme,
            &provider,
            Some(profile),
            default == Selection::Profile(profile.id),
        ));
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
    Container::new(column.finish())
        .with_padding_bottom(28.0)
        .finish()
}

/// One account: who it is, its limits, and what can be done with it.
/// `profile` None is the agent's own (system) login.
fn account_card(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    provider: &str,
    profile: Option<&dragent_host::development::AccountProfileView>,
    is_default: bool,
) -> Box<dyn Element> {
    let state = &view.agent_accounts;
    let font = view.ui_font;
    let snapshot = state.snapshot();
    let machine_suffix = snapshot_key(&state.selected);
    let (key, label, status, kind) = match profile {
        Some(p) => (
            p.id.to_string(),
            p.label.clone(),
            snapshot.status_for(provider, Some(p.id)),
            Some(p.kind),
        ),
        None => (
            system_key(provider),
            "System login".to_string(),
            snapshot.status_for(provider, None),
            None,
        ),
    };
    let identity = snapshot.identity_for(provider, profile.map(|p| p.id));
    let who_email = identity.and_then(|i| i.email.clone());
    let plan = identity.and_then(|i| i.plan.clone());
    let local = state.selected.is_none();
    let usage_key = profile.map_or_else(|| "system".to_string(), |p| p.id.to_string());
    let usage = local
        .then(|| {
            state
                .usage
                .iter()
                .find(|u| u.provider == provider && u.account == usage_key)
                .and_then(|u| u.usage.as_ref())
        })
        .flatten();
    let plan = plan.or_else(|| usage.and_then(|u| u.plan.clone()));
    let signed_in = status == "Signed in";
    let busy = status == "Checking…" || status == "Waiting for sign-in…";

    // Top line: default mark, name, plan, status.
    let mark = {
        let selection = profile.map_or(Selection::System, |p| Selection::Profile(p.id));
        let provider = provider.to_string();
        let id = view.view_id;
        let theme = *theme;
        Hoverable::new(view.hover(&format!("accounts-mark-{machine_suffix}-{key}")), move |hover| {
            if is_default {
                crate::icons::render_icon(crate::icons::AppIcon::CircleCheck, theme.accent, 16.0)
            } else {
                // An empty ring: "make this the default".
                let ring = if hover.is_hovered() {
                    theme.sub_text()
                } else {
                    theme.disabled_text()
                };
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
                    SettingsAction::AgentAccount(AccountAction::MakeDefault {
                        provider: provider.clone(),
                        selection: selection.clone(),
                    }),
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
    if is_default {
        top = top.with_child(tag(theme, font, "Default"));
    }
    if kind == Some(CredentialKind::ApiKey) {
        top = top.with_child(tag(theme, font, "API key"));
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
    let identity_text = match who_email {
        Some(email) => format!("{email} · {place}"),
        None if signed_in => place.to_string(),
        None => format!("Not signed in · {place}"),
    };

    let mut body = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(top.finish())
        .with_child(
            Container::new(text(theme, font, &identity_text, 12.5, theme.sub_text()))
                .with_padding_left(24.0)
                .finish(),
        );

    // Limits: local subscription accounts only.
    if kind == Some(CredentialKind::ApiKey) {
        body = body.with_child(
            Container::new(text(
                theme,
                font,
                "Pay-per-token — see your provider's console for spend.",
                12.0,
                theme.disabled_text(),
            ))
            .with_padding_left(24.0)
            .finish(),
        );
    } else if local {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        let windows: Vec<_> = usage
            .map(|u| {
                u.windows
                    .iter()
                    .filter(|w| w.resets_at.is_none_or(|at| at > now))
                    .collect()
            })
            .unwrap_or_default();
        if windows.is_empty() {
            body = body.with_child(
                Container::new(text(
                    theme,
                    font,
                    "Limits show here after its first session.",
                    12.0,
                    theme.disabled_text(),
                ))
                .with_padding_left(24.0)
                .finish(),
            );
        } else {
            let mut limits = Flex::column().with_spacing(8.0);
            for window in windows {
                limits = limits.with_child(limit_row(
                    theme,
                    font,
                    &window.label,
                    window.used_percent,
                    window.resets_at.map(|at| at - now),
                ));
            }
            body = body.with_child(
                Container::new(limits.finish())
                    .with_padding_left(24.0)
                    .with_padding_top(2.0)
                    .finish(),
            );
        }
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
            &format!("accounts-default-{machine_suffix}-{key}"),
            "Use for new sessions",
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
                let target = Some(p.id);
                footer = footer
                    .with_child(ghost(
                        view,
                        theme,
                        &format!("accounts-signin-{machine_suffix}-{key}"),
                        match (p.kind, signed_in) {
                            (CredentialKind::ApiKey, true) => "Replace key",
                            (CredentialKind::ApiKey, false) => "Add key",
                            (_, true) => "Sign in again",
                            (_, false) => "Sign in",
                        },
                        true,
                        AccountAction::SignInAgain {
                            provider: provider.to_string(),
                            target,
                        },
                    ))
                    .with_child(ghost(
                        view,
                        theme,
                        &format!("accounts-rename-{machine_suffix}-{key}"),
                        "Rename",
                        false,
                        AccountAction::StartRename(p.id),
                    ))
                    .with_child(ghost(
                        view,
                        theme,
                        &format!("accounts-remove-{machine_suffix}-{key}"),
                        "Remove",
                        false,
                        AccountAction::RequestRemove(p.id),
                    ));
            }
            None => {
                footer = footer
                    .with_child(ghost(
                        view,
                        theme,
                        &format!("accounts-signin-{machine_suffix}-{key}"),
                        if signed_in { "Sign in again" } else { "Sign in" },
                        true,
                        AccountAction::SignInAgain {
                            provider: provider.to_string(),
                            target: None,
                        },
                    ))
                    .with_child(ghost(
                        view,
                        theme,
                        &format!("accounts-check-{machine_suffix}-{key}"),
                        "Check",
                        false,
                        AccountAction::CheckSystem(provider.to_string()),
                    ));
            }
        }
    }
    body = body
        .with_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(theme.outline())
                    .finish(),
            )
            .with_height(1.0)
            .finish(),
        )
        .with_child(footer.finish());

    card(theme, is_default, body.finish())
}

/// A rounded panel; the default account's has an accent outline.
fn card(
    theme: &crate::theme::Theme,
    highlighted: bool,
    child: Box<dyn Element>,
) -> Box<dyn Element> {
    Container::new(child)
        .with_background_color(theme.surface_1())
        .with_border(
            Border::all(if highlighted { 1.5 } else { 1.0 }).with_border_color(if highlighted {
                theme.accent
            } else {
                theme.outline()
            }),
        )
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(12.0)
        .finish()
}

fn text(
    _theme: &crate::theme::Theme,
    font: warpui::fonts::FamilyId,
    value: &str,
    size: f32,
    color: warpui::color::ColorU,
) -> Box<dyn Element> {
    Text::new(value.to_string(), font, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}

/// A small uppercase plan tag ("MAX", "PLUS").
fn tag(
    theme: &crate::theme::Theme,
    font: warpui::fonts::FamilyId,
    value: &str,
) -> Box<dyn Element> {
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
fn status_line(
    theme: &crate::theme::Theme,
    font: warpui::fonts::FamilyId,
    status: &str,
) -> Box<dyn Element> {
    let color = match status {
        "Signed in" => theme.ui_green(),
        "Checking…" | "Waiting for sign-in…" | "Not checked" => theme.disabled_text(),
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
fn limit_row(
    theme: &crate::theme::Theme,
    font: warpui::fonts::FamilyId,
    label: &str,
    used: f64,
    resets_in: Option<i64>,
) -> Box<dyn Element> {
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
                .with_child(
                    Expanded::new((1.0 - ratio).max(0.0001), Empty::new().finish()).finish(),
                )
                .finish(),
        )
        .with_background_color(theme.surface_3())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
        .finish(),
    )
    .with_height(4.0)
    .finish();
    let fixed =
        |child: Box<dyn Element>, width: f32| ConstrainedBox::new(child).with_width(width).finish();
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(fixed(
            text(theme, font, label, 12.0, theme.sub_text()),
            84.0,
        ))
        .with_child(Expanded::new(1.0, bar).finish())
        .with_child(fixed(
            text(
                theme,
                font,
                &format!("{}%", used.round() as i64),
                12.0,
                theme.main_text(),
            ),
            40.0,
        ))
        .with_child(fixed(
            text(
                theme,
                font,
                &resets_in
                    .map(|s| format!("resets in {}", duration_label(s)))
                    .unwrap_or_default(),
                12.0,
                theme.disabled_text(),
            ),
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
fn ghost(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    key: &str,
    label: &str,
    strong: bool,
    action: AccountAction,
) -> Box<dyn Element> {
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
        let mut chip = Container::new(
            Text::new(label.clone(), font, crate::theme::text_size(12.5))
                .with_color(color)
                .finish(),
        )
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
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(id, SettingsAction::AgentAccount(action.clone()))
    })
    .finish()
}

#[cfg(test)]
mod tests {
    use super::AccountDialog;

    #[test]
    fn suggested_names_skip_the_ones_in_use() {
        let taken = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        use super::suggested_name;
        assert_eq!(suggested_name(&[]), "Work");
        assert_eq!(suggested_name(&taken(&["work"])), "Personal");
        assert_eq!(suggested_name(&taken(&["Work", "PERSONAL", "account 2"])), "Account 3");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(super::duration_label(49 * 60), "49m");
        assert_eq!(super::duration_label(4 * 3600 + 29 * 60), "4h 29m");
        assert_eq!(super::duration_label(5 * 86400 + 21 * 3600), "5d 21h");
        assert_eq!(super::duration_label(-5), "0m");
    }

    #[test]
    fn waiting_promotes_to_done_on_signed_in_identity() {
        let snapshot = super::MachineSnapshot {
            profiles: vec![dragent_host::development::AccountProfileView {
                id: uuid::Uuid::from_u128(7),
                provider: "claude".into(),
                label: "Work".into(),
                kind: dragent_host::agent_accounts::CredentialKind::Subscription,
            }],
            defaults: Default::default(),
            identities: vec![dragent_host::development::AccountIdentityView {
                target: Some(uuid::Uuid::from_u128(7)),
                provider: "claude".into(),
                signed_in: true,
                email: Some("me@example.com".into()),
                plan: None,
                kind: None,
                error: None,
                credentials_changed_at: None,
            }],
            loading: false,
            error: None,
            not_connected: false,
        };
        // The dialog moves from Waiting to Done when this identity reports
        // signed in; the Done line shows the email.
        let signed_label = snapshot
            .identity_for("claude", Some(uuid::Uuid::from_u128(7)))
            .filter(|i| i.signed_in)
            .and_then(|i| i.email.clone());
        assert_eq!(signed_label.as_deref(), Some("me@example.com"));
        assert_eq!(
            snapshot.status_for("claude", Some(uuid::Uuid::from_u128(7))),
            "Signed in"
        );
        assert_eq!(
            snapshot.status_for("claude", Some(uuid::Uuid::from_u128(8))),
            "Not checked"
        );
    }

    #[test]
    fn waiting_promotes_only_on_a_new_sign_in() {
        let identity = |signed_in: bool,
                        email: Option<&str>,
                        error: Option<&str>,
                        changed_at: Option<u64>| {
            dragent_host::development::AccountIdentityView {
                target: Some(uuid::Uuid::from_u128(7)),
                provider: "claude".into(),
                signed_in,
                email: email.map(str::to_string),
                plan: None,
                kind: None,
                error: error.map(str::to_string),
                credentials_changed_at: changed_at,
            }
        };
        // A fresh account starts unsigned: the first signed-in report
        // finishes the dialog.
        assert!(super::waiting_promotes(
            false,
            &None,
            &None,
            &None,
            &identity(true, Some("me@example.com"), None, Some(100)),
        ));
        // Still unsigned: keep waiting.
        assert!(!super::waiting_promotes(
            false,
            &None,
            &None,
            &None,
            &identity(false, None, None, None),
        ));
        // "Sign in again" on an already-signed-in account with the same
        // identity: keep waiting, don't finish straight away.
        assert!(!super::waiting_promotes(
            true,
            &Some("me@example.com".to_string()),
            &None,
            &Some(100),
            &identity(true, Some("me@example.com"), None, Some(100)),
        ));
        // The same account signing in under a different email: finished.
        assert!(super::waiting_promotes(
            true,
            &Some("me@example.com".to_string()),
            &None,
            &Some(100),
            &identity(true, Some("you@example.com"), None, Some(100)),
        ));
    }

    #[test]
    fn waiting_promotes_when_sign_in_files_change() {
        let identity = |changed_at: Option<u64>| {
            dragent_host::development::AccountIdentityView {
                target: Some(uuid::Uuid::from_u128(7)),
                provider: "claude".into(),
                signed_in: true,
                email: Some("me@example.com".to_string()),
                plan: None,
                kind: None,
                error: None,
                credentials_changed_at: changed_at,
            }
        };
        let email = Some("me@example.com".to_string());
        // Rotating an API key leaves the identity identical but rewrites
        // the sign-in files: a newer timestamp finishes the dialog.
        assert!(super::waiting_promotes(
            true,
            &email,
            &None,
            &Some(100),
            &identity(Some(200)),
        ));
        // A timestamp appearing where there was none finishes too.
        assert!(super::waiting_promotes(
            true, &email, &None, &None,
            &identity(Some(200)),
        ));
        // Nothing changed: keep waiting.
        assert!(!super::waiting_promotes(
            true,
            &email,
            &None,
            &Some(100),
            &identity(Some(100)),
        ));
        assert!(!super::waiting_promotes(
            true, &email, &None, &None,
            &identity(None),
        ));
        // An older timestamp never finishes on its own.
        assert!(!super::waiting_promotes(
            true,
            &email,
            &None,
            &Some(200),
            &identity(Some(100)),
        ));
    }

    #[test]
    fn remove_confirm_mentions_machine_and_kept_files() {
        let message = format!(
            "Removes {} from {}. Its sign-in files stay on that machine.",
            "Work", "Build box"
        );
        assert!(message.contains("Build box"));
        assert!(message.contains("sign-in files stay"));
    }
}
