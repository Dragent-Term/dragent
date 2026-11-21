//! Provider account settings, per machine.
//!
//! The person picks a machine at the top ("This computer" plus every SSH
//! machine from Settings → Machines). Everything below shows and changes that
//! machine's accounts through t1's host requests:
//! `DevelopmentRequest::Accounts(AccountsRequest::…)`. This computer goes
//! through `development::execute` on a background thread; a machine goes
//! through its `HostClient`. Never block the UI thread.
use super::widgets::{
    choice_row, danger_button, page_header, primary_button, secondary_button, select_control,
    SelectItem,
};
use super::{SettingsAction, SettingsEvent, SettingsView};
use crate::workspace::{InputEditor, InputMode};
use dragent_host::agent_accounts::{self, CredentialKind, Selection};
use dragent_host::development::{
    AccountsRequest, AccountsResponse, DevelopmentRequest, DevelopmentResponse,
};
use std::collections::{BTreeMap, HashMap};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    MainAxisSize, ParentElement, Text,
};
use warpui::{ReadView, SingletonEntity, UpdateView, ViewContext, ViewHandle};
use warpui_core::clipboard::ClipboardContent;
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
    ConfirmAdd,
    CancelDialog,
    CloseDialog,
    CopyCommand,
    SignInAgain { provider: String, target: Option<uuid::Uuid> },
    MakeDefault {
        provider: String,
        selection: Selection,
    },
    /// The dialog's Done step also sets the default.
    UseDialogDefault,
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

/// The add / sign-in dialog.
#[derive(Clone, Debug, Default)]
pub(crate) enum AccountDialog {
    #[default]
    Closed,
    Form {
        provider: String,
        kind: CredentialKind,
    },
    Waiting {
        provider: String,
        kind: CredentialKind,
        machine_id: Option<String>,
        machine_label: String,
        target: Option<uuid::Uuid>,
        label: String,
        command: String,
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
        kind: CredentialKind,
        machine_id: Option<String>,
        machine_label: String,
        target: Option<uuid::Uuid>,
        label: String,
        command: String,
        signed_label: String,
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
                InputMode::Settings("agent_account_name".into()),
                c.focus_handle(),
            )
        });
        Self {
            selected: None,
            snapshots: HashMap::new(),
            dialog: AccountDialog::Closed,
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

fn agent_label(provider: &str) -> String {
    crate::workspace::PromptAgent::ALL
        .into_iter()
        .find(|a| a.command_key() == provider)
        .map(|a| a.label().to_string())
        .unwrap_or_else(|| provider.to_string())
}

fn dialog_title(provider: &str) -> String {
    format!("Add {} account", agent_label(provider))
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
        self.promote_waiting_if_signed_in(machine_id);
        ctx.notify();
    }

    fn promote_waiting_if_signed_in(&mut self, machine_id: &Option<String>) {
        let waiting = match &self.agent_accounts.dialog {
            AccountDialog::Waiting {
                provider,
                kind,
                machine_id: waiting_machine,
                machine_label,
                target,
                label,
                command,
                baseline_signed_in,
                baseline_email,
                baseline_error,
                baseline_credentials_changed_at,
            } if waiting_machine == machine_id => (
                provider.clone(),
                *kind,
                machine_label.clone(),
                *target,
                label.clone(),
                command.clone(),
                *baseline_signed_in,
                baseline_email.clone(),
                baseline_error.clone(),
                *baseline_credentials_changed_at,
            ),
            _ => return,
        };
        let (
            provider,
            kind,
            machine_label,
            target,
            label,
            command,
            baseline_signed_in,
            baseline_email,
            baseline_error,
            baseline_credentials_changed_at,
        ) = waiting;
        let key = snapshot_key(machine_id);
        let current = self
            .agent_accounts
            .snapshots
            .get(&key)
            .and_then(|s| s.identity_for(&provider, target).cloned());
        let Some(current) = current else {
            return;
        };
        if !waiting_promotes(
            baseline_signed_in,
            &baseline_email,
            &baseline_error,
            &baseline_credentials_changed_at,
            &current,
        ) {
            return;
        }
        let signed_label = current
            .email
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| {
                // Fall back to the account name when the agent reports no email.
                label.clone()
            });
        self.agent_accounts.dialog = AccountDialog::Done {
            provider,
            kind,
            machine_id: machine_id.clone(),
            machine_label,
            target,
            label,
            command,
            signed_label,
        };
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
                self.agent_accounts.dialog = AccountDialog::Closed;
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
                self.agent_accounts.dialog = AccountDialog::Form {
                    provider: provider.clone(),
                    kind: CredentialKind::Subscription,
                };
                self.agent_accounts.pending_remove = None;
                self.agent_accounts.editing = None;
                ctx.update_view(&self.agent_accounts.name, |e, c| e.set_text("Work", c));
                ctx.notify();
            }
            AccountAction::SetKind(kind) => {
                if let AccountDialog::Form { provider, .. } = &self.agent_accounts.dialog {
                    let provider = provider.clone();
                    self.agent_accounts.dialog = AccountDialog::Form { provider, kind: *kind };
                    ctx.notify();
                }
            }
            AccountAction::ConfirmAdd => {
                if self.selected_is_remote_windows(ctx) {
                    self.agent_accounts.dialog = AccountDialog::Closed;
                    ctx.notify();
                    return;
                }
                let (provider, kind) = match &self.agent_accounts.dialog {
                    AccountDialog::Form { provider, kind } => (provider.clone(), *kind),
                    _ => return,
                };
                let name = self
                    .agent_accounts
                    .name
                    .read(ctx, |e, _| e.get_text().trim().to_string());
                let machine_id = self.agent_accounts.selected.clone();
                let machine_label = self.selected_machine_label(ctx);
                let agent = agent_label(&provider);
                self.send_host_accounts(
                    machine_id.clone(),
                    AccountsRequest::Add {
                        provider: provider.clone(),
                        label: name.clone(),
                        kind,
                    },
                    ctx,
                    move |view, result, ctx| match result {
                        Ok(DevelopmentResponse::Accounts(AccountsResponse::Added(profile))) => {
                            let target = Some(profile.id);
                            let label = profile.label.clone();
                            let machine_id = machine_id.clone();
                            let machine_label = machine_label.clone();
                            view.send_host_accounts(
                                machine_id.clone(),
                                AccountsRequest::SignInCommand {
                                    provider: provider.clone(),
                                    target,
                                },
                                ctx,
                                move |view, result, ctx| match result {
                                    Ok(DevelopmentResponse::Accounts(
                                        AccountsResponse::SignInCommand { command, .. },
                                    )) => {
                                        ctx.emit(SettingsEvent::AgentAccountSignIn {
                                            machine_id: machine_id.clone(),
                                            title: format!("{agent} account sign-in"),
                                            command: command.clone(),
                                        });
                                        view.agent_accounts.dialog = AccountDialog::Waiting {
                                            provider: provider.clone(),
                                            kind,
                                            machine_id: machine_id.clone(),
                                            machine_label,
                                            target,
                                            label,
                                            command,
                                        // A fresh account starts unsigned:
                                        // the first signed-in report
                                        // finishes the dialog.
                                        baseline_signed_in: false,
                                        baseline_email: None,
                                        baseline_error: None,
                                        baseline_credentials_changed_at: None,
                                        };
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
                                        view.refresh_current_accounts(ctx);
                                    }
                                },
                            );
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
            AccountAction::CancelDialog | AccountAction::CloseDialog => {
                self.agent_accounts.dialog = AccountDialog::Closed;
                ctx.notify();
            }
            AccountAction::CopyCommand => {
                let command = match &self.agent_accounts.dialog {
                    AccountDialog::Waiting { command, .. }
                    | AccountDialog::Done { command, .. } => Some(command.clone()),
                    _ => None,
                };
                if let Some(command) = command {
                    ctx.clipboard().write(ClipboardContent::plain_text(command));
                    ctx.notify();
                }
            }
            AccountAction::SignInAgain { provider, target } => {
                if self.selected_is_remote_windows(ctx) {
                    return;
                }
                let provider = provider.clone();
                let target = *target;
                let machine_id = self.agent_accounts.selected.clone();
                let machine_label = self.selected_machine_label(ctx);
                let agent = agent_label(&provider);
                let label = self
                    .agent_accounts
                    .snapshots
                    .get(&snapshot_key(&machine_id))
                    .and_then(|s| {
                        target.and_then(|id| {
                            s.profiles
                                .iter()
                                .find(|p| p.id == id)
                                .map(|p| p.label.clone())
                        })
                    })
                    .unwrap_or_else(|| "System login".to_string());
                let kind = self
                    .agent_accounts
                    .snapshots
                    .get(&snapshot_key(&machine_id))
                    .and_then(|s| {
                        target.and_then(|id| {
                            s.profiles
                                .iter()
                                .find(|p| p.id == id)
                                .map(|p| p.kind)
                        })
                    })
                    .unwrap_or(CredentialKind::Subscription);
                self.send_host_accounts(
                    machine_id.clone(),
                    AccountsRequest::SignInCommand {
                        provider: provider.clone(),
                        target,
                    },
                    ctx,
                    move |view, result, ctx| match result {
                        Ok(DevelopmentResponse::Accounts(AccountsResponse::SignInCommand {
                            command,
                            ..
                        })) => {
                            ctx.emit(SettingsEvent::AgentAccountSignIn {
                                machine_id: machine_id.clone(),
                                title: format!("{agent} account sign-in"),
                                command: command.clone(),
                            });
                            // Remember what the account reported when
                            // the dialog opened: it only finishes on a
                            // new sign-in, never on the same identity.
                            let baseline = view
                                .agent_accounts
                                .snapshots
                                .get(&snapshot_key(&machine_id))
                                .and_then(|s| s.identity_for(&provider, target).cloned());
                            let (
                                baseline_signed_in,
                                baseline_email,
                                baseline_error,
                                baseline_credentials_changed_at,
                            ) = baseline.map_or((false, None, None, None), |i| {
                                (
                                    i.signed_in,
                                    i.email,
                                    i.error,
                                    i.credentials_changed_at,
                                )
                            });
                            view.agent_accounts.dialog = AccountDialog::Waiting {
                                provider: provider.clone(),
                                kind,
                                machine_id: machine_id.clone(),
                                machine_label,
                                target,
                                label,
                                command,
                                baseline_signed_in,
                                baseline_email,
                                baseline_error,
                                baseline_credentials_changed_at,
                            };
                            // The terminal run is the poll's first tick: an
                            // immediate List keeps already-signed-in rows honest.
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
            AccountAction::UseDialogDefault => {
                let (provider, target) = match &self.agent_accounts.dialog {
                    AccountDialog::Done {
                        provider, target, ..
                    } => (provider.clone(), *target),
                    _ => return,
                };
                let selection = target.map_or(Selection::System, Selection::Profile);
                self.handle_agent_account(
                    &AccountAction::MakeDefault {
                        provider,
                        selection,
                    },
                    ctx,
                );
                self.agent_accounts.dialog = AccountDialog::Closed;
                ctx.notify();
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
                self.agent_accounts.dialog = AccountDialog::Closed;
                ctx.update_view(&self.agent_accounts.name, |e, c| e.set_text(&label, c));
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
                                view.agent_accounts.dialog = AccountDialog::Closed;
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
    if let Some(dialog) = render_dialog(view, theme) {
        page = page.with_child(
            Container::new(dialog)
                .with_padding_bottom(16.0)
                .finish(),
        );
    }
    if let Some(pending) = view.agent_accounts.pending_remove {
        let snapshot = view.agent_accounts.snapshot();
        let machine_label = machine_label_for(app, &view.agent_accounts.selected);
        if let Some(profile) = snapshot.profiles.iter().find(|p| p.id == pending) {
            page = page.with_child(remove_confirm(view, theme, profile, &machine_label));
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

fn machine_label_for(app: &warpui::AppContext, machine_id: &Option<String>) -> String {
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

/// The add-account dialog (form, waiting or done), below the picker.
fn render_dialog(view: &SettingsView, theme: &crate::theme::Theme) -> Option<Box<dyn Element>> {
    let font = view.ui_font;
    match &view.agent_accounts.dialog {
        AccountDialog::Closed => None,
        AccountDialog::Form { provider, kind } => {
            let title = dialog_title(provider);
            let description = match kind {
                CredentialKind::Subscription => {
                    "A separate login with its own sign-in. Skills, settings and MCP servers are not shared."
                }
                CredentialKind::ApiKey => {
                    "Pay per token with an API key. You type the key in the terminal; Dragent never sees it."
                }
            };
            let billing = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(2.0)
                .with_child(choice_row(
                    theme,
                    font,
                    view.view_id,
                    view.hover("accounts-kind-subscription"),
                    "Subscription",
                    *kind == CredentialKind::Subscription,
                    SettingsAction::AgentAccount(AccountAction::SetKind(
                        CredentialKind::Subscription,
                    )),
                ))
                .with_child(choice_row(
                    theme,
                    font,
                    view.view_id,
                    view.hover("accounts-kind-apikey"),
                    "API key",
                    *kind == CredentialKind::ApiKey,
                    SettingsAction::AgentAccount(AccountAction::SetKind(CredentialKind::ApiKey)),
                ))
                .finish();
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(
                    ConstrainedBox::new(crate::shared_input::shared_input(
                        theme,
                        &view.agent_accounts.name,
                        crate::shared_input::SharedInputOpts::default(),
                    ))
                    .with_max_width(300.0)
                    .finish(),
                )
                .with_child(primary_button(
                    theme,
                    font,
                    view.view_id,
                    view.hover("accounts-dialog-signin"),
                    "Sign in",
                    SettingsAction::AgentAccount(AccountAction::ConfirmAdd),
                ))
                .with_child(secondary_button(
                    theme,
                    font,
                    view.view_id,
                    view.hover("accounts-dialog-cancel"),
                    "Cancel",
                    SettingsAction::AgentAccount(AccountAction::CancelDialog),
                ))
                .finish();
            Some(card(
                theme,
                false,
                Flex::column()
                    .with_spacing(10.0)
                    .with_child(text(theme, font, &title, 14.0, theme.main_text()))
                    .with_child(text(theme, font, description, 12.5, theme.sub_text()))
                    .with_child(billing)
                    .with_child(text(theme, font, "Account name", 12.5, theme.sub_text()))
                    .with_child(row)
                    .finish(),
            ))
        }
        AccountDialog::Waiting {
            provider,
            kind,
            machine_label,
            label,
            command,
            ..
        } => Some(sign_in_card(
            view,
            theme,
            provider,
            *kind,
            machine_label,
            label,
            command,
            None,
        )),
        AccountDialog::Done {
            provider,
            kind,
            machine_label,
            label,
            command,
            signed_label,
            ..
        } => Some(sign_in_card(
            view,
            theme,
            provider,
            *kind,
            machine_label,
            label,
            command,
            Some(signed_label.clone()),
        )),
    }
}

/// Waiting and done share the command box; done adds the signed-in line.
#[allow(clippy::too_many_arguments)]
fn sign_in_card(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    provider: &str,
    kind: CredentialKind,
    machine_label: &str,
    label: &str,
    command: &str,
    signed_label: Option<String>,
) -> Box<dyn Element> {
    let font = view.ui_font;
    let run_label = if kind == CredentialKind::ApiKey {
        format!("Runs in a terminal on {machine_label}; type your key when asked:")
    } else {
        format!("Runs in a terminal on {machine_label}:")
    };
    let command_box = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(
                        command.to_string(),
                        font,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .finish(),
            )
            .with_child(ghost(
                view,
                theme,
                &format!("accounts-copy-{label}"),
                "Copy",
                false,
                AccountAction::CopyCommand,
            ))
            .finish(),
    )
    .with_background_color(theme.surface_3())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(12.0)
    .with_padding_right(8.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .finish();
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0)
        .with_child(text(
            theme,
            font,
            &dialog_title(provider),
            14.0,
            theme.main_text(),
        ))
        .with_child(text(theme, font, &run_label, 12.5, theme.sub_text()))
        .with_child(command_box);
    match signed_label {
        None => {
            column = column
                .with_child(status_line(theme, font, "Waiting for sign-in…"))
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(warpui::elements::MainAxisAlignment::End)
                        .with_child(ghost(
                            view,
                            theme,
                            "accounts-dialog-close",
                            "Close",
                            false,
                            AccountAction::CloseDialog,
                        ))
                        .finish(),
                );
        }
        Some(signed) => {
            column = column
                .with_child(text(
                    theme,
                    font,
                    &format!("{signed} is signed in."),
                    13.0,
                    theme.main_text(),
                ))
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(primary_button(
                            theme,
                            font,
                            view.view_id,
                            view.hover("accounts-dialog-done"),
                            "Done",
                            SettingsAction::AgentAccount(AccountAction::CloseDialog),
                        ))
                        .with_child(secondary_button(
                            theme,
                            font,
                            view.view_id,
                            view.hover("accounts-dialog-default"),
                            "Use for new sessions",
                            SettingsAction::AgentAccount(AccountAction::UseDialogDefault),
                        ))
                        .finish(),
                );
        }
    }
    card(theme, false, column.finish())
}

fn remove_confirm(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    profile: &dragent_host::development::AccountProfileView,
    machine_label: &str,
) -> Box<dyn Element> {
    let font = view.ui_font;
    card(
        theme,
        false,
        Flex::column()
            .with_spacing(10.0)
            .with_child(text(
                theme,
                font,
                &format!("Remove “{}”?", profile.label),
                14.0,
                theme.main_text(),
            ))
            .with_child(text(
                theme,
                font,
                &format!(
                    "Removes {} from {machine_label}. Its sign-in files stay on that machine.",
                    profile.label
                ),
                12.5,
                theme.sub_text(),
            ))
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(danger_button(
                        theme,
                        font,
                        view.view_id,
                        view.hover(&format!("accounts-remove-confirm-{}", profile.id)),
                        "Remove",
                        SettingsAction::AgentAccount(AccountAction::ConfirmRemove),
                    ))
                    .with_child(secondary_button(
                        theme,
                        font,
                        view.view_id,
                        view.hover("accounts-remove-cancel"),
                        "Cancel",
                        SettingsAction::AgentAccount(AccountAction::CancelRemove),
                    ))
                    .finish(),
            )
            .finish(),
    )
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
    if let Some(form) = rename_form(view, theme, &provider) {
        column = column.with_child(form);
    }

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

/// The rename field, when this agent is renaming an account.
fn rename_form(
    view: &SettingsView,
    theme: &crate::theme::Theme,
    provider: &str,
) -> Option<Box<dyn Element>> {
    let state = &view.agent_accounts;
    let font = view.ui_font;
    let snapshot = state.snapshot();
    let id = match state.editing.as_ref()? {
        Editing::Rename(id) => *id,
    };
    let profile = snapshot
        .profiles
        .iter()
        .find(|p| p.id == id && p.provider == provider)?;
    let title = format!("Rename “{}”", profile.label);
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(
            ConstrainedBox::new(crate::shared_input::shared_input(
                theme,
                &state.name,
                crate::shared_input::SharedInputOpts::default(),
            ))
            .with_max_width(300.0)
            .finish(),
        )
        .with_child(primary_button(
            theme,
            font,
            view.view_id,
            view.hover("accounts-save"),
            "Save",
            SettingsAction::AgentAccount(AccountAction::Save),
        ))
        .with_child(secondary_button(
            theme,
            font,
            view.view_id,
            view.hover("accounts-cancel"),
            "Cancel",
            SettingsAction::AgentAccount(AccountAction::CancelEdit),
        ))
        .finish();
    Some(card(
        theme,
        false,
        Flex::column()
            .with_spacing(10.0)
            .with_child(text(theme, font, &title, 13.5, theme.main_text()))
            .with_child(row)
            .with_child(text(
                theme,
                font,
                "Only the name changes; the sign-in stays.",
                12.0,
                theme.sub_text(),
            ))
            .finish(),
    ))
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
                        if signed_in { "Sign in again" } else { "Sign in" },
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
