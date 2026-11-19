use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::process::Command;

use warpui::elements::{Container, Element, ParentElement};
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, SingletonEntity, TypedActionView,
    UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::MouseStateHandle;
use warpui_core::fonts::Cache as FontCache;

use crate::auth::{
    extract_auth_paste, handle_open_url, handle_pasted_auth, AuthManager, AuthManagerEvent,
};
use crate::process_ext::NoWindow;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::Theme;

use super::checklist_slide::{render_checklist, ChecklistViewState};
use super::git_provider::GitProvider;
use super::github_auth_view::render_git_auth_view;
use super::intro_slide::render_intro_slide;
use super::settings::UiSettings;

/// Welcome + a single "Get set up" checklist.
pub const SLIDE_COUNT: usize = 2;
/// Required connections: Dragent account and the user's chosen Git provider.
pub const TASK_COUNT: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCliState {
    Checking,
    Missing,
    Installing,
    ReadyToConnect,
    AwaitingAuthentication,
    Connected,
    Error(String),
}

#[derive(Clone, Debug)]
pub enum OnboardingAction {
    Next,
    Back,
    StartBrowserAuth,
    CancelBrowserAuth,
    CopySignInLink,
    PasteAuthCallback,
    SelectGitProvider(GitProvider),
    CheckGitCli,
    InstallGitCli,
    ConnectGitCli,
    CancelGitAuth,
    RetryGitAuth,
    CopyGitCode,
    Finish,
}

pub enum OnboardingEvent {
    Completed { theme_id: String },
}

pub struct OnboardingFlow {
    slide: usize,
    theme_id: String,
    ui_font: FamilyId,
    view_id: EntityId,
    hover_states: HashMap<String, MouseStateHandle>,
    browser_auth_active: bool,
    sign_in_url: Option<String>,
    auth_error: Option<String>,
    git_provider: GitProvider,
    git_cli_state: GitCliState,
    git_auth_terminal: Option<ViewHandle<TerminalView>>,
    git_auth_host: Option<crate::terminal::manager::transient_host::TransientHostTerminal>,
    git_auth_generation: u64,
    git_auth_output: String,
    git_auth_code: Option<String>,
    git_auth_message: Option<String>,
    account_connected: bool,
}

impl OnboardingFlow {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let settings = UiSettings::load();

        let font_cache = ctx.get_singleton_model_handle::<FontCache>();
        let ui_font = ctx.update_model(&font_cache, |cache, _| crate::fonts::ui_family(cache));

        let view_id = ctx.view_id();

        let mut hover_states: HashMap<String, MouseStateHandle> = HashMap::new();
        for key in [
            "next",
            "back",
            "finish",
            "signin",
            "github_action",
            "github_cancel",
            "github_retry",
            "github_copy_code",
            "github_verify",
            "provider_github",
            "provider_gitlab",
            "copy_link",
            "paste_callback",
            "cancel_browser",
        ] {
            hover_states.insert(key.to_string(), MouseStateHandle::default());
        }

        let auth = AuthManager::handle(ctx);
        let account_connected = ctx.update_model(&auth, |auth, _| auth.is_logged_in());
        ctx.subscribe_to_model(&auth, |flow, _, event, ctx| {
            match event {
                AuthManagerEvent::AuthComplete => {
                    flow.browser_auth_active = false;
                    flow.auth_error = None;
                    flow.account_connected = true;
                    ctx.notify();
                }
                AuthManagerEvent::AuthFailed { message } => {
                    flow.auth_error = Some(message.clone());
                    ctx.notify();
                }
                AuthManagerEvent::LoggedOut => {
                    flow.account_connected = false;
                    ctx.notify();
                }
                AuthManagerEvent::SessionRestored => {
                    flow.account_connected = true;
                    ctx.notify();
                }
                // Session died mid-onboarding: surface it on the setup screen
                // so the user knows why they're being asked to sign in again.
                AuthManagerEvent::SessionExpired => {
                    flow.browser_auth_active = false;
                    flow.account_connected = false;
                    flow.auth_error =
                        Some("Your session expired. Please sign in again.".to_string());
                    ctx.notify();
                }
            }
        });

        Self {
            slide: 0,
            theme_id: settings.theme_id,
            ui_font,
            view_id,
            hover_states,
            browser_auth_active: false,
            sign_in_url: None,
            auth_error: None,
            git_provider: if settings.gitlab_cli_setup_completed
                && !settings.github_cli_setup_completed
            {
                GitProvider::Gitlab
            } else {
                GitProvider::Github
            },
            git_cli_state: GitCliState::Checking,
            git_auth_terminal: None,
            git_auth_host: None,
            git_auth_generation: 0,
            git_auth_output: String::new(),
            git_auth_code: None,
            git_auth_message: None,
            account_connected,
        }
    }

    fn hover(&self, key: &str) -> MouseStateHandle {
        self.hover_states.get(key).cloned().unwrap_or_default()
    }

    fn setup_complete(&self) -> bool {
        self.account_connected && matches!(self.git_cli_state, GitCliState::Connected)
    }

    fn complete(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.setup_complete() {
            return;
        }
        let mut settings = UiSettings::load();
        settings.onboarding_completed = true;
        match self.git_provider {
            GitProvider::Github => settings.github_cli_setup_completed = true,
            GitProvider::Gitlab => settings.gitlab_cli_setup_completed = true,
        }
        settings.theme_id = self.theme_id.clone();
        settings.save();
        ctx.emit(OnboardingEvent::Completed {
            theme_id: self.theme_id.clone(),
        });
    }

    fn current_theme(&self) -> Theme {
        Theme::default()
    }

    fn check_git_cli(&mut self, ctx: &mut ViewContext<Self>) {
        self.git_cli_state = GitCliState::Checking;
        let generation = self.git_auth_generation;
        let provider = self.git_provider;
        ctx.spawn(
            background(move || detect_git_cli(provider)),
            move |flow, result, ctx| {
                if flow.git_auth_generation != generation || flow.git_provider != provider {
                    return;
                }
                let result = result.unwrap_or_else(GitCliState::Error);
                if flow.git_auth_terminal.is_some() {
                    if matches!(result, GitCliState::Connected) {
                        flow.close_git_auth(ctx);
                    } else {
                        flow.git_auth_message = Some(format!(
                            "{} is not connected yet. Finish the prompts below or restart sign-in.",
                            provider.label()
                        ));
                    }
                }
                flow.git_cli_state = result;
                ctx.notify();
            },
        );
    }

    fn install_git_cli(&mut self, ctx: &mut ViewContext<Self>) {
        self.git_cli_state = GitCliState::Installing;
        let provider = self.git_provider;
        let generation = self.git_auth_generation;
        ctx.spawn(
            background(move || {
                install_git_cli(provider)?;
                Ok::<_, String>(detect_git_cli(provider))
            }),
            move |flow, result, ctx| {
                if flow.git_auth_generation != generation || flow.git_provider != provider {
                    return;
                }
                flow.git_cli_state = result.and_then(|r| r).unwrap_or_else(GitCliState::Error);
                ctx.notify();
            },
        );
    }

    fn connect_git_cli(&mut self, ctx: &mut ViewContext<Self>) {
        self.close_git_auth(ctx);
        self.git_auth_output.clear();
        self.git_auth_code = None;
        self.git_auth_message = None;
        self.git_cli_state = GitCliState::AwaitingAuthentication;
        let provider = self.git_provider;
        let generation = self.git_auth_generation;
        let cwd = dirs::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(std::env::temp_dir);
        let theme = Theme::default();
        let slot = std::sync::Arc::new(crate::terminal::manager::pty_writer::HostPtyBinding::new());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shell = dragent_pty::resolve_shell(
            crate::console::editor_settings::EditorSettings::global()
                .terminal
                .default_shell
                .clone()
                .or_else(|| std::env::var("SHELL").ok()),
        );
        let terminal = ctx.add_typed_action_view(|terminal_ctx| {
            TerminalView::new_host(cwd.clone(), slot.clone(), terminal_ctx)
        });
        let feed = terminal.read(ctx, |view, _| view.host_output_sink(cancel.clone()));
        let (host, events) = crate::terminal::manager::transient_host::TransientHostTerminal::start(
            cwd, shell, slot, cancel, feed,
        );
        self.git_auth_host = Some(host);
        let event_terminal = terminal.clone();
        ctx.spawn_stream_local(
            events,
            move |flow, event, ctx| {
                if flow.git_auth_generation != generation {
                    return;
                }
                use crate::terminal::manager::transient_host::TransientHostEvent;
                ctx.update_view(&event_terminal, |view, tctx| match event {
                    TransientHostEvent::Ready => view.set_host_notice(None, tctx),
                    TransientHostEvent::Error(error) => view.set_host_notice(Some(error), tctx),
                    TransientHostEvent::Exited(code) => view.mark_remote_exited(code, tctx),
                });
            },
            |_, _| {},
        );
        let model = terminal.read(ctx, |view, _| view.terminal_model());
        ctx.subscribe_to_model(&model, move |flow, _, event, ctx| {
            if flow.git_auth_generation != generation || flow.git_auth_terminal.is_none() {
                return;
            }
            match event {
                TerminalEvent::OutputChunk { bytes, .. } => {
                    if provider != GitProvider::Github {
                        return;
                    }
                    if flow.git_auth_code.is_some() {
                        return;
                    }
                    flow.git_auth_output
                        .push_str(&String::from_utf8_lossy(bytes));
                    if flow.git_auth_output.len() > 4096 {
                        let keep_from = flow.git_auth_output.len() - 4096;
                        let keep_from = flow.git_auth_output.ceil_char_boundary(keep_from);
                        flow.git_auth_output.drain(..keep_from);
                    }
                    if let Some(code) = find_github_device_code(&flow.git_auth_output) {
                        ctx.clipboard()
                            .write(ClipboardContent::plain_text(code.clone()));
                        flow.git_auth_code = Some(code);
                        if let Some(terminal) = &flow.git_auth_terminal {
                            ctx.update_view(terminal, |view, _| view.write_pty_bytes(b"\r"));
                        }
                        ctx.notify();
                    }
                }
                TerminalEvent::BlockCompleted { context }
                    if context
                        .command
                        .starts_with(&format!("{} auth login", provider.cli())) =>
                {
                    flow.check_git_cli(ctx);
                }
                _ => {}
            }
        });
        self.git_auth_terminal = Some(terminal.clone());
        ctx.update_view(&terminal, |view, terminal_ctx| {
            view.set_theme(theme, terminal_ctx);
            view.submit_shell_command(provider.login_command(), terminal_ctx);
        });
        ctx.notify();
    }

    fn close_git_auth(&mut self, ctx: &mut ViewContext<Self>) {
        self.git_auth_generation = self.git_auth_generation.wrapping_add(1);
        self.git_auth_host.take();
        if let Some(terminal) = self.git_auth_terminal.take() {
            ctx.update_view(&terminal, |view, _| view.shutdown());
        }
        self.git_auth_output.clear();
        self.git_auth_code = None;
        self.git_auth_message = None;
    }
}

async fn background<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = futures::channel::oneshot::channel();
    crate::bg::detach(move || {
        let _ = tx.send(job());
    });
    rx.await
        .map_err(|_| "Repository setup worker stopped".to_string())
}

fn detect_git_cli(provider: GitProvider) -> GitCliState {
    let mut command = dragent_host::host_tools::command(provider.cli());
    let installed = command.arg("--version").no_window().output();
    if !matches!(installed, Ok(output) if output.status.success()) {
        return GitCliState::Missing;
    }

    let mut command = dragent_host::host_tools::command(provider.cli());
    if let Some(home) = dirs::home_dir() {
        command.current_dir(home);
    }
    match command.args(provider.status_args()).no_window().output() {
        Ok(output) if output.status.success() => GitCliState::Connected,
        Ok(_) => GitCliState::ReadyToConnect,
        Err(error) => {
            GitCliState::Error(format!("Could not check {} CLI: {error}", provider.label()))
        }
    }
}

fn install_git_cli(provider: GitProvider) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = dragent_host::host_tools::command("brew");
        command.args(["install", provider.cli()]);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("winget");
        command.args([
            "install",
            "--id",
            provider.winget_package(),
            "--exact",
            "--accept-source-agreements",
            "--accept-package-agreements",
        ]);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    return Err(format!(
        "Install {} CLI from {}, then choose Retry.",
        provider.label(),
        provider.install_url()
    ));

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let output = command.no_window().output().map_err(|error| {
            format!(
                "Could not start the {} CLI installer: {error}",
                provider.label()
            )
        })?;
        if output.status.success() {
            Ok(())
        } else {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if detail.is_empty() {
                format!(
                    "{} CLI installation exited with {}",
                    provider.label(),
                    output.status
                )
            } else {
                detail
            })
        }
    }
}

fn find_github_device_code(output: &str) -> Option<String> {
    let mut plain = String::with_capacity(output.len());
    let mut chars = output.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.next() == Some('[') {
            for control in chars.by_ref() {
                if ('@'..='~').contains(&control) {
                    break;
                }
            }
        } else if ch != '\r' {
            plain.push(ch);
        }
    }
    let start = plain.rfind("one-time code:")? + "one-time code:".len();
    let code = plain[start..].trim_start().get(..9)?;
    let bytes = code.as_bytes();
    (bytes.len() == 9
        && bytes[4] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, byte)| i == 4 || byte.is_ascii_uppercase() || byte.is_ascii_digit()))
    .then(|| code.to_string())
}

impl Entity for OnboardingFlow {
    type Event = OnboardingEvent;
}

impl View for OnboardingFlow {
    fn ui_name() -> &'static str {
        "OnboardingFlow"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        let theme = self.current_theme();
        let flow_id = self.view_id;
        let ui_font = self.ui_font;

        let content: Box<dyn Element> = match self.slide {
            0 => render_intro_slide(&theme, ui_font, flow_id, self.hover("next")),
            _ if self.git_auth_terminal.is_some() => render_git_auth_view(
                &theme,
                ui_font,
                flow_id,
                self.git_provider,
                self.git_auth_terminal.as_ref().unwrap(),
                self.git_auth_code.as_deref(),
                self.git_auth_message.as_deref(),
                self.hover("github_cancel"),
                self.hover("github_retry"),
                self.hover("github_copy_code"),
                self.hover("github_verify"),
            ),
            _ => render_checklist(
                &theme,
                ui_font,
                flow_id,
                ChecklistViewState {
                    account_connected: self.account_connected,
                    browser_auth_active: self.browser_auth_active,
                    auth_error: self.auth_error.as_deref(),
                    provider: self.git_provider,
                    git: &self.git_cli_state,
                },
                |key| self.hover(key),
                self.hover("back"),
                self.hover("finish"),
            ),
        };

        Container::new(content)
            .with_background_color(theme.solid_bg())
            .finish()
    }
}

impl TypedActionView for OnboardingFlow {
    type Action = OnboardingAction;

    fn handle_action(&mut self, action: &OnboardingAction, ctx: &mut ViewContext<Self>) {
        match action {
            OnboardingAction::Next => {
                if self.slide + 1 < SLIDE_COUNT {
                    self.slide += 1;
                    self.check_git_cli(ctx);
                }
            }
            OnboardingAction::Back => {
                if self.git_auth_terminal.is_some() {
                    self.close_git_auth(ctx);
                    self.check_git_cli(ctx);
                } else if self.browser_auth_active {
                    self.browser_auth_active = false;
                    let auth = AuthManager::handle(ctx);
                    ctx.update_model(&auth, |auth, _| auth.cancel_browser_sign_in());
                } else if self.slide > 0 {
                    self.slide -= 1;
                }
            }
            OnboardingAction::StartBrowserAuth => {
                let auth = AuthManager::handle(ctx);
                let url =
                    ctx.update_model(&auth, |auth, auth_ctx| auth.begin_browser_sign_in(auth_ctx));
                ctx.open_url(&url);
                self.sign_in_url = Some(url);
                self.browser_auth_active = true;
                self.auth_error = None;
            }
            OnboardingAction::CancelBrowserAuth => {
                let auth = AuthManager::handle(ctx);
                ctx.update_model(&auth, |auth, _| auth.cancel_browser_sign_in());
                self.browser_auth_active = false;
                self.auth_error = None;
            }
            OnboardingAction::CopySignInLink => {
                if let Some(url) = &self.sign_in_url {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(url.clone()));
                }
            }
            OnboardingAction::PasteAuthCallback => {
                let clip = ctx.clipboard().read();
                match extract_auth_paste(&clip) {
                    Some(pasted) => {
                        self.auth_error = None;
                        handle_pasted_auth(ctx, &pasted);
                    }
                    None => {
                        self.auth_error = Some(
                            "Clipboard is empty or does not contain a sign-in link or token. \
                             Copy the callback URL from your browser, then try again."
                                .to_string(),
                        );
                    }
                }
            }
            OnboardingAction::CheckGitCli => self.check_git_cli(ctx),
            OnboardingAction::SelectGitProvider(provider) => {
                if self.git_provider != *provider {
                    self.close_git_auth(ctx);
                    self.git_provider = *provider;
                    self.check_git_cli(ctx);
                }
            }
            OnboardingAction::InstallGitCli => self.install_git_cli(ctx),
            OnboardingAction::ConnectGitCli => self.connect_git_cli(ctx),
            OnboardingAction::CancelGitAuth => {
                self.close_git_auth(ctx);
                self.check_git_cli(ctx);
            }
            OnboardingAction::RetryGitAuth => self.connect_git_cli(ctx),
            OnboardingAction::CopyGitCode => {
                if let Some(code) = &self.git_auth_code {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(code.clone()));
                }
            }
            // Account sign-in and the selected provider's authentication are required.
            OnboardingAction::Finish => self.complete(ctx),
        }
        ctx.notify();
    }
}
