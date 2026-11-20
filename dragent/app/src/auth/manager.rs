//! Auth orchestration — browser sign-in, callback handling, session restore.

use std::time::Duration;
use web_time::Instant;

use log::warn;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::browser::browser_sign_in_url;
use super::callback::AuthCallback;
use super::completion::{parse_auth_completion, AuthCompletion};
use super::poll::{try_poll_pending_session, PollResult};
use super::session_store::{clear_refresh_token, load_refresh_token, save_refresh_token};
use super::types::{AuthManagerEvent, AuthStatus, AuthUser};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Fire the silent restore shortly after startup so the UI has time to render first.
const RESTORE_DELAY: Duration = Duration::from_millis(200);
/// Return a cached access token only if it has at least this long left before expiry.
const ACCESS_TOKEN_MIN_REMAINING: Duration = Duration::from_secs(30);
/// Re-validate this long before the access token lapses, so a session that died
/// server-side is discovered even while the app sits idle (see `schedule_revalidation`).
const REVALIDATE_LEAD: Duration = Duration::from_secs(60);
/// Never re-validate more often than this, whatever the server reports.
const REVALIDATE_MIN_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionSource {
    /// Deep link, polling, or other automatic delivery — CSRF state must match.
    Automatic,
    /// User explicitly pasted from the clipboard — accept token even if state expired.
    ManualPaste,
}

/// Why a silent restore is running. Both refresh the token through the same
/// single-flight path; they differ only in what the UI is told afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreKind {
    /// First restore of this session (startup, or just after sign-in).
    Initial,
    /// Scheduled re-validation of a session that is already live.
    Revalidate,
}

pub struct AuthManager {
    status: AuthStatus,
    pending_state: Option<String>,
    refresh_token: Option<String>,
    /// Cached short-lived access token and its expiry instant.
    access_token: Option<String>,
    access_token_expires_at: Option<Instant>,
    /// Bumped on every sign-in and sign-out. A scheduled re-validation carries
    /// the epoch it was armed under and does nothing if that no longer matches,
    /// so timers from a previous session can't drive the current one (or stack
    /// up into several parallel refresh chains across sign-in/out cycles).
    session_epoch: u64,
}

impl Entity for AuthManager {
    type Event = AuthManagerEvent;
}

impl SingletonEntity for AuthManager {}

impl AuthManager {
    /// Credential-free model for isolated UI captures; never restores a real session.
    pub(crate) fn preview(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            status: AuthStatus::LoggedOut,
            pending_state: None,
            refresh_token: None,
            access_token: None,
            access_token_expires_at: None,
            session_epoch: 0,
        }
    }

    /// Flip an isolated UI capture between its signed-out and signed-in pages.
    pub(crate) fn preview_set_signed_in(&mut self, signed_in: bool) {
        self.status = if signed_in {
            AuthStatus::LoggedIn(AuthUser {
                user_id: None,
                first_name: None,
                last_name: None,
                avatar_url: None,
            })
        } else {
            AuthStatus::LoggedOut
        };
    }

    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // The API client wipes credentials and shouts here the instant it learns
        // the session is dead — from ANY thread (agent, account fetch, voice).
        // This is what turns "expired token" into "signed out" immediately,
        // instead of leaving a logged-in UI on top of a session that is gone.
        ctx.spawn_stream_local(
            crate::api::session_expired_rx(),
            |this, _: (), mctx| this.handle_session_expired(mctx),
            |_, _| {},
        );

        #[cfg(windows)]
        if let Some(rx) = super::get_deep_link_receiver() {
            ctx.spawn_stream_local(
                rx,
                // `this` is THIS AuthManager, already checked out of AppContext's
                // model table for the duration of this callback. `handle_open_url`
                // re-fetches the AuthManager handle and calls `ctx.update_model` on
                // it — i.e. tries to check the same model out a SECOND time while
                // it's already checked out. `update_model` finds it missing and
                // panics with "Circular model update", which unwinds the main
                // thread and kills the whole app. That's why the already-running
                // instance a deep link gets forwarded to used to vanish instead of
                // just coming to the front. Call `handle_completion` straight on
                // `this` instead of round-tripping through `handle_open_url`.
                |this, msg, mctx| {
                    if msg == "NEWWINDOW" {
                        // Plain relaunch while already running (taskbar/Start
                        // Menu/desktop icon, no URL) — open another window
                        // instead of just bringing the existing one forward.
                        // `add_window` touches its own window/view state, not
                        // `self.models`, so — unlike `handle_open_url` above —
                        // there's no reentrant-update risk calling it directly
                        // from inside this already-checked-out AuthManager update.
                        #[cfg(not(target_family = "wasm"))]
                        crate::desktop::open_new_window(mctx);
                        let _ = mctx.windows().activate_app();
                    } else if msg.starts_with("dragent://") {
                        if crate::agent::mcp::is_mcp_oauth_callback(&msg) {
                            crate::agent::mcp::handle_mcp_oauth_callback(&msg);
                        } else {
                            match parse_auth_completion(&msg) {
                                Ok(completion) => this.handle_completion(
                                    completion,
                                    CompletionSource::Automatic,
                                    mctx,
                                ),
                                Err(err) => warn!("invalid auth URL: {err:#}"),
                            }
                        }
                        let _ = mctx.windows().activate_app();
                    }
                },
                |_, _| {},
            );
        }

        if let Some(token) = load_refresh_token() {
            // Schedule background validation; status is Restoring until the exchange succeeds.
            ctx.spawn(Timer::after(RESTORE_DELAY), |this, _, mctx| {
                this.run_silent_restore(RestoreKind::Initial, mctx)
            });
            Self {
                status: AuthStatus::Restoring,
                pending_state: None,
                refresh_token: Some(token),
                access_token: None,
                access_token_expires_at: None,
                session_epoch: 0,
            }
        } else {
            Self {
                status: AuthStatus::LoggedOut,
                pending_state: None,
                refresh_token: None,
                access_token: None,
                access_token_expires_at: None,
                session_epoch: 0,
            }
        }
    }

    /// The session died on its own (refresh token expired/revoked). Drop every
    /// credential and put the UI in the signed-out state at once. Idempotent —
    /// several code paths can discover the same expiry.
    fn handle_session_expired(&mut self, ctx: &mut ModelContext<Self>) {
        if matches!(self.status, AuthStatus::LoggedOut) && self.refresh_token.is_none() {
            return; // already signed out — nothing to do
        }
        warn!("[auth] session expired — signing out");
        self.reset_session_state();
        // The keychain is normally already cleared by whoever detected the
        // expiry; do it again in case we got here another way. Harmless.
        if let Err(err) = clear_refresh_token() {
            warn!("failed to clear keychain session: {err:#}");
        }
        crate::api::API_CLIENT.clear();
        self.status = AuthStatus::LoggedOut;
        ctx.emit(AuthManagerEvent::SessionExpired);
        ctx.notify();
    }

    /// Forget the current session's tokens and invalidate its scheduled timers.
    fn reset_session_state(&mut self) {
        self.pending_state = None;
        self.refresh_token = None;
        self.access_token = None;
        self.access_token_expires_at = None;
        self.session_epoch = self.session_epoch.wrapping_add(1);
    }

    /// Re-validate the session shortly before the cached access token lapses.
    ///
    /// Without this, a session that the server killed while the app sat idle is
    /// only discovered at the user's next action — they keep seeing a signed-in
    /// account that no longer exists. The refresh this performs is one the app
    /// would have to do at its next request anyway, so it costs nothing extra;
    /// if it fails, `poll_restore_result` signs the user out on the spot.
    fn schedule_revalidation(&mut self, expires_in_secs: u64, ctx: &mut ModelContext<Self>) {
        let delay = Duration::from_secs(expires_in_secs)
            .saturating_sub(REVALIDATE_LEAD)
            .max(REVALIDATE_MIN_INTERVAL);
        let epoch = self.session_epoch;
        ctx.spawn(Timer::after(delay), move |this, _, mctx| {
            if this.session_epoch != epoch || this.refresh_token.is_none() {
                return; // signed out, or a newer session owns the schedule now
            }
            this.run_silent_restore(RestoreKind::Revalidate, mctx);
        });
    }

    /// Returns the cached access token if it still has sufficient lifetime remaining.
    pub fn cached_access_token(&self) -> Option<&str> {
        let expires_at = self.access_token_expires_at?;
        if Instant::now() + ACCESS_TOKEN_MIN_REMAINING < expires_at {
            self.access_token.as_deref()
        } else {
            None
        }
    }

    /// Cache a newly-received access token with its server-reported lifetime.
    pub fn cache_access_token(&mut self, token: String, expires_in_secs: u64) {
        let margin = expires_in_secs.saturating_sub(ACCESS_TOKEN_MIN_REMAINING.as_secs());
        self.access_token = Some(token);
        self.access_token_expires_at = Some(Instant::now() + Duration::from_secs(margin));
    }

    pub fn status(&self) -> &AuthStatus {
        &self.status
    }

    pub fn is_logged_in(&self) -> bool {
        matches!(self.status, AuthStatus::LoggedIn(_) | AuthStatus::Restoring)
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// Update the in-memory refresh token and persist it to the OS keychain.
    /// Called after token rotation so the next Fix button press uses the current token.
    pub fn update_refresh_token(&mut self, new_token: String) {
        if let Err(err) = save_refresh_token(&new_token) {
            warn!("failed to persist rotated refresh token: {err:#}");
        }
        self.refresh_token = Some(new_token);
    }

    /// Start browser sign-in. Opens in the browser; completion can arrive via deep link,
    /// background polling, or manual paste — redirect is optional.
    pub fn begin_browser_sign_in(&mut self, ctx: &mut ModelContext<Self>) -> String {
        let state = uuid::Uuid::new_v4().to_string();
        self.pending_state = Some(state.clone());
        self.status = AuthStatus::SigningIn;
        self.schedule_poll(ctx);
        browser_sign_in_url(&state)
    }

    pub fn cancel_browser_sign_in(&mut self) {
        if matches!(self.status, AuthStatus::SigningIn) {
            self.pending_state = None;
            self.status = if self.refresh_token.is_some() {
                AuthStatus::LoggedIn(AuthUser {
                    user_id: self.user_id_from_status(),
                    first_name: None,
                    last_name: None,
                    avatar_url: None,
                })
            } else {
                AuthStatus::LoggedOut
            };
        }
    }

    pub fn handle_auth_callback(&mut self, callback: AuthCallback, ctx: &mut ModelContext<Self>) {
        self.handle_completion(
            AuthCompletion::Callback(callback),
            CompletionSource::Automatic,
            ctx,
        );
    }

    fn handle_completion(
        &mut self,
        completion: AuthCompletion,
        source: CompletionSource,
        ctx: &mut ModelContext<Self>,
    ) {
        match completion {
            AuthCompletion::Callback(callback) => {
                if source == CompletionSource::ManualPaste {
                    self.pending_state = None;
                    self.finish_sign_in(callback.refresh_token, callback.user_id, ctx);
                    return;
                }

                let Some(expected_state) = self.pending_state.take() else {
                    warn!("auth callback received with no pending browser sign-in");
                    self.emit_failure("No sign-in in progress".to_string(), ctx);
                    return;
                };

                if callback.state != expected_state {
                    warn!("auth callback CSRF state mismatch");
                    self.pending_state = None;
                    self.emit_failure("Sign-in link expired. Try again.".to_string(), ctx);
                    self.status = AuthStatus::LoggedOut;
                    return;
                }

                self.finish_sign_in(callback.refresh_token, callback.user_id, ctx);
            }
            AuthCompletion::RefreshTokenOnly(token) => {
                if source == CompletionSource::Automatic
                    && !matches!(self.status, AuthStatus::SigningIn)
                {
                    self.emit_failure(
                        "Start sign-in before pasting a refresh token.".to_string(),
                        ctx,
                    );
                    return;
                }
                self.pending_state = None;
                self.finish_sign_in(token, None, ctx);
            }
        }
    }

    /// Kick off a silent session restore on a background OS thread so the model
    /// thread is never blocked. Polls the result channel every 20 ms via a timer.
    ///
    /// SINGLE-FLIGHT: the restore goes through `API_CLIENT.restore_session()`,
    /// the one and only token-exchange path (guarded by its exchange lock).
    /// The server rotates refresh tokens and revokes the whole family on
    /// reuse — a second exchanger here once raced the client and destroyed
    /// the session on every app launch. Never exchange directly.
    fn run_silent_restore(&mut self, kind: RestoreKind, ctx: &mut ModelContext<Self>) {
        if self.refresh_token.is_none() {
            return;
        }

        let (tx, rx) =
            std::sync::mpsc::channel::<Result<crate::api::RestoredSession, crate::api::ApiError>>();
        crate::bg::detach(move || {
            let _ = tx.send(crate::api::API_CLIENT.restore_session());
        });

        ctx.spawn(
            Timer::after(Duration::from_millis(20)),
            move |this, _, mctx| {
                this.poll_restore_result(rx, kind, mctx);
            },
        );
    }

    fn poll_restore_result(
        &mut self,
        rx: std::sync::mpsc::Receiver<Result<crate::api::RestoredSession, crate::api::ApiError>>,
        kind: RestoreKind,
        ctx: &mut ModelContext<Self>,
    ) {
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.spawn(
                    Timer::after(Duration::from_millis(20)),
                    move |this, _, mctx| {
                        this.poll_restore_result(rx, kind, mctx);
                    },
                );
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                warn!("[auth] silent restore thread disconnected unexpectedly");
                self.assume_offline_session(ctx);
            }
            Ok(Ok(session)) => {
                // The client already persisted the rotated refresh token and
                // cached the access token — mirror its state, don't re-derive.
                self.refresh_token = crate::api::API_CLIENT.current_refresh_token();
                self.cache_access_token(session.access_token, session.expires_in_secs);
                self.status = AuthStatus::LoggedIn(AuthUser {
                    user_id: session.user.user_id,
                    first_name: session.user.first_name,
                    last_name: session.user.last_name,
                    avatar_url: session.user.avatar_url,
                });
                // Keep the session validated for as long as the app is open.
                self.schedule_revalidation(session.expires_in_secs, ctx);
                if kind == RestoreKind::Initial {
                    ctx.emit(AuthManagerEvent::SessionRestored);
                }
                ctx.notify();
            }
            // The token is gone or the server refused it: this session is over.
            // Signing the user out here is the whole point — the old code showed
            // a logged-in UI backed by a dead session until the next restart.
            Ok(Err(crate::api::ApiError::NotAuthenticated)) => {
                self.handle_session_expired(ctx);
            }
            // Anything else (offline, 5xx, malformed body) is transient: the
            // token may well be fine. Don't punish the user for a flaky network
            // — keep the session and let the next call settle it.
            Ok(Err(err)) => {
                warn!("[auth] silent session restore failed (transient): {err}");
                self.assume_offline_session(ctx);
            }
        }
    }

    /// Restore couldn't reach the server. Keep the stored session and stay
    /// signed in — the identity fills in on the next successful exchange.
    fn assume_offline_session(&mut self, ctx: &mut ModelContext<Self>) {
        self.status = AuthStatus::LoggedIn(AuthUser {
            user_id: None,
            first_name: None,
            last_name: None,
            avatar_url: None,
        });
        // Try again on the normal cadence rather than never.
        self.schedule_revalidation(REVALIDATE_MIN_INTERVAL.as_secs(), ctx);
        ctx.notify();
    }

    /// The code to match in the browser while a sign-in is waiting.
    pub fn pending_sign_in_code(&self) -> Option<String> {
        if !matches!(self.status, AuthStatus::SigningIn) {
            return None;
        }
        self.pending_state.as_deref().map(super::code::sign_in_code)
    }

    pub fn log_out(&mut self, ctx: &mut ModelContext<Self>) {
        // End the session on the server too, so a copied token stops working.
        // In the background, and best effort: signing out here never waits
        // on the network.
        let access = self.access_token.clone();
        let refresh = self.refresh_token.clone();
        if access.is_some() || refresh.is_some() {
            std::thread::spawn(move || {
                let access = match access {
                    Some(token) => Some(token),
                    None => refresh
                        .and_then(|r| super::token_exchange::exchange(&r).ok())
                        .map(|e| e.access_token),
                };
                if let Some(token) = access {
                    if let Err(err) = super::token_exchange::sign_out_on_server(&token) {
                        warn!("server sign-out failed: {err}");
                    }
                }
            });
        }
        if let Err(err) = clear_refresh_token() {
            warn!("failed to clear keychain session: {err:#}");
        }
        crate::api::API_CLIENT.clear();
        self.reset_session_state();
        self.status = AuthStatus::LoggedOut;
        ctx.emit(AuthManagerEvent::LoggedOut);
        ctx.notify();
    }

    fn finish_sign_in(
        &mut self,
        refresh_token: String,
        user_id: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        if let Err(err) = save_refresh_token(&refresh_token) {
            warn!("failed to persist refresh token: {err:#}");
            self.emit_failure("Could not save session securely.".to_string(), ctx);
            self.status = AuthStatus::LoggedOut;
            return;
        }

        // A new session starts here — retire any timers armed by the previous one.
        self.session_epoch = self.session_epoch.wrapping_add(1);
        self.pending_state = None;
        self.refresh_token = Some(refresh_token.clone());
        // Notify ApiClient of the new refresh token so it can exchange on next server call
        crate::api::API_CLIENT.set_refresh_token(refresh_token.clone());
        // Silently exchange right away so we get user_id + a cached access token
        // without waiting for the first Fix press.
        ctx.spawn(Timer::after(Duration::ZERO), |this, _, mctx| {
            this.run_silent_restore(RestoreKind::Initial, mctx)
        });
        self.status = AuthStatus::LoggedIn(AuthUser {
            user_id,
            first_name: None,
            last_name: None,
            avatar_url: None,
        });
        ctx.emit(AuthManagerEvent::AuthComplete);
        ctx.notify();
    }

    fn schedule_poll(&mut self, ctx: &mut ModelContext<Self>) {
        ctx.spawn(Timer::after(POLL_INTERVAL), |auth, _, ctx| {
            if !matches!(auth.status, AuthStatus::SigningIn) {
                return;
            }
            let Some(state) = auth.pending_state.clone() else {
                return;
            };

            // The HTTP poll BLOCKS for up to its 8s timeout — running it
            // here (timer callbacks are on the UI thread) froze the whole
            // app on every poll whenever the auth endpoint was slow. Do
            // the request on a worker thread; the UI only checks a channel.
            let (tx, rx) = std::sync::mpsc::channel();
            crate::bg::detach(move || {
                let _ = tx.send(try_poll_pending_session(&state));
            });
            auth.await_poll_result(rx, ctx);
        });
    }

    /// Non-blocking wait for the background poll's result: re-arm a short UI
    /// timer and `try_recv` until the worker thread reports back.
    fn await_poll_result(
        &mut self,
        rx: std::sync::mpsc::Receiver<PollResult>,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.spawn(
            Timer::after(Duration::from_millis(100)),
            move |auth, _, ctx| {
                if !matches!(auth.status, AuthStatus::SigningIn) {
                    return;
                }
                match rx.try_recv() {
                    Ok(PollResult::Ready(callback)) => auth.handle_auth_callback(callback, ctx),
                    Ok(PollResult::Pending) => auth.schedule_poll(ctx),
                    Err(std::sync::mpsc::TryRecvError::Empty) => auth.await_poll_result(rx, ctx),
                    // Worker died (should not happen) — resume the normal cadence.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => auth.schedule_poll(ctx),
                }
            },
        );
    }

    fn emit_failure(&self, message: String, ctx: &mut ModelContext<Self>) {
        ctx.emit(AuthManagerEvent::AuthFailed { message });
        ctx.notify();
    }

    fn user_id_from_status(&self) -> Option<String> {
        match &self.status {
            AuthStatus::LoggedIn(user) => user.user_id.clone(),
            _ => None,
        }
    }
}

/// Handle an incoming deep link from the OS (optional path — polling/paste also work).
pub fn handle_open_url(ctx: &mut warpui::AppContext, raw_url: &str) {
    use warpui::UpdateModel;

    let completion = match parse_auth_completion(raw_url) {
        Ok(completion) => completion,
        Err(err) => {
            warn!("invalid auth URL: {err:#}");
            return;
        }
    };

    let handle = AuthManager::handle(ctx);
    ctx.update_model(&handle, |auth, ctx| {
        auth.handle_completion(completion, CompletionSource::Automatic, ctx)
    });
}

/// Complete sign-in from clipboard paste (callback URL or raw refresh token).
pub fn handle_pasted_auth(ctx: &mut warpui::AppContext, raw: &str) {
    use warpui::UpdateModel;

    let completion = match parse_auth_completion(raw) {
        Ok(completion) => completion,
        Err(err) => {
            let handle = AuthManager::handle(ctx);
            ctx.update_model(&handle, |auth, ctx| {
                auth.emit_failure(format!("Could not read sign-in data: {err}"), ctx);
            });
            return;
        }
    };

    let handle = AuthManager::handle(ctx);
    ctx.update_model(&handle, |auth, ctx| {
        auth.handle_completion(completion, CompletionSource::ManualPaste, ctx)
    });
}
