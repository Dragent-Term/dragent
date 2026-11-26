//! Auth orchestration — browser sign-in, callback handling, session restore.

use std::time::Duration;
use web_time::Instant;

use log::warn;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::browser::browser_sign_in_url;
use super::callback::AuthCallback;
use super::completion::{parse_auth_completion, AuthCompletion};
use super::poll::{redeem_handoff, try_poll_pending_session, PollResult};
use super::session_store::{clear_refresh_token, load_refresh_token, save_refresh_token};
use super::types::{AuthManagerEvent, AuthStatus, AuthUser};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// The server forgets a started sign-in after 10 minutes; a little longer, so
/// an approval at the last moment is still collected.
const SIGN_IN_LIMIT: Duration = Duration::from_secs(11 * 60);
const SIGN_IN_EXPIRED: &str = "This sign-in expired before it was approved.";
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

/// What a launch's silent session restore means for the signed-in state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreOutcome {
    /// The server confirmed the session.
    SignedIn,
    /// The server refused the token: the session is over.
    SignOut,
    /// No usable answer (offline, server error, bad body): keep the cached
    /// session; the next exchange settles it.
    KeepSession,
}

fn restore_outcome<T>(result: &Result<T, crate::api::ApiError>) -> RestoreOutcome {
    match result {
        Ok(_) => RestoreOutcome::SignedIn,
        Err(crate::api::ApiError::NotAuthenticated) => RestoreOutcome::SignOut,
        Err(_) => RestoreOutcome::KeepSession,
    }
}

pub struct AuthManager {
    status: AuthStatus,
    pending_state: Option<String>,
    /// PKCE verifier of the pending browser sign-in; never leaves this app
    /// except to the server, to collect that sign-in.
    pending_verifier: Option<String>,
    /// When the pending browser sign-in started.
    pending_since: Option<Instant>,
    /// Why the last browser sign-in didn't finish, until the person tries
    /// again or cancels.
    sign_in_problem: Option<String>,
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
            pending_verifier: None,
            pending_since: None,
            sign_in_problem: None,
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
                    } else if msg.starts_with(&format!("{}://", dragent_paths::url_scheme())) {
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

        // Local mode: signed in as the demo identity from the first frame.
        // No Keychain read, no restore, no token anywhere.
        if crate::api::is_local_mode() {
            crate::startup_timing::skip(crate::startup_timing::Phase::Auth);
            return Self {
                status: AuthStatus::LoggedIn(AuthUser::local_demo()),
                pending_state: None,
                pending_verifier: None,
                pending_since: None,
                sign_in_problem: None,
                refresh_token: None,
                access_token: None,
                access_token_expires_at: None,
                session_epoch: 0,
            };
        }

        if let Some(token) = load_refresh_token() {
            // Schedule background validation; status is Restoring until the exchange succeeds.
            ctx.spawn(Timer::after(RESTORE_DELAY), |this, _, mctx| {
                this.run_silent_restore(RestoreKind::Initial, mctx)
            });
            Self {
                status: AuthStatus::Restoring,
                pending_state: None,
                pending_verifier: None,
                pending_since: None,
                sign_in_problem: None,
                refresh_token: Some(token),
                access_token: None,
                access_token_expires_at: None,
                session_epoch: 0,
            }
        } else {
            crate::startup_timing::skip(crate::startup_timing::Phase::Auth);
            Self {
                status: AuthStatus::LoggedOut,
                pending_state: None,
                pending_verifier: None,
                pending_since: None,
                sign_in_problem: None,
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
        crate::api::API_CLIENT.clear();
        // The keychain is normally already cleared by whoever detected the
        // expiry; do it again in case we got here another way. Harmless.
        if let Err(err) = clear_refresh_token() {
            warn!("failed to clear keychain session: {err:#}");
        }
        self.status = AuthStatus::LoggedOut;
        ctx.emit(AuthManagerEvent::SessionExpired);
        ctx.notify();
    }

    /// Forget the current session's tokens and invalidate its scheduled timers.
    fn reset_session_state(&mut self) {
        self.pending_state = None;
        self.pending_verifier = None;
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

    /// Signed in as the local demo identity (local mode): there is no
    /// account to sign in to or out of.
    pub fn is_local(&self) -> bool {
        crate::api::is_local_mode()
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
    ///
    /// Local mode has no sign-in: returns an empty URL (open nothing) and
    /// reports why.
    pub fn begin_browser_sign_in(&mut self, ctx: &mut ModelContext<Self>) -> String {
        if self.is_local() {
            self.emit_failure(crate::api::NEEDS_ACCOUNT.to_string(), ctx);
            return String::new();
        }
        let state = uuid::Uuid::new_v4().to_string();
        let verifier = super::pkce::new_verifier();
        let challenge = super::pkce::challenge(&verifier);
        self.pending_state = Some(state.clone());
        self.pending_verifier = Some(verifier);
        self.pending_since = Some(Instant::now());
        self.sign_in_problem = None;
        self.status = AuthStatus::SigningIn;
        self.schedule_poll(ctx);
        browser_sign_in_url(&state, &challenge)
    }

    pub fn cancel_browser_sign_in(&mut self) {
        self.sign_in_problem = None;
        self.stop_browser_sign_in();
    }

    pub fn clear_sign_in_problem(&mut self) {
        self.sign_in_problem = None;
    }

    /// Why the last browser sign-in didn't finish.
    pub fn sign_in_problem(&self) -> Option<&str> {
        self.sign_in_problem.as_deref()
    }

    fn fail_sign_in(&mut self, message: String, ctx: &mut ModelContext<Self>) {
        self.sign_in_problem = Some(message.clone());
        self.emit_failure(message, ctx);
    }

    /// Give up on a sign-in the server has forgotten; whether it did.
    fn expire_sign_in(&mut self, now: Instant) -> bool {
        let expired = matches!(self.status, AuthStatus::SigningIn)
            && self
                .pending_since
                .is_some_and(|since| now.duration_since(since) >= SIGN_IN_LIMIT);
        if expired {
            self.stop_browser_sign_in();
        }
        expired
    }

    fn stop_browser_sign_in(&mut self) {
        self.pending_since = None;
        if matches!(self.status, AuthStatus::SigningIn) {
            self.pending_state = None;
            self.pending_verifier = None;
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
        // A pasted or linked sign-in can't apply to a build with no server;
        // never store its token.
        if self.is_local() {
            self.emit_failure(crate::api::NEEDS_ACCOUNT.to_string(), ctx);
            return;
        }
        match completion {
            AuthCompletion::Callback(callback) => {
                // A pasted link with a one-time code is redeemed like a
                // deep link (it needs this sign-in's verifier).
                if source == CompletionSource::ManualPaste && callback.code.is_none() {
                    self.pending_state = None;
                    self.pending_verifier = None;
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
                    self.fail_sign_in("Sign-in link expired. Try again.".to_string(), ctx);
                    self.status = AuthStatus::LoggedOut;
                    return;
                }

                // A one-time code: redeem it with the verifier first, then
                // finish with the refresh token it yields.
                if let Some(code) = callback.code.clone() {
                    let Some(verifier) = self.pending_verifier.clone() else {
                        self.emit_failure("Sign-in link expired. Try again.".to_string(), ctx);
                        return;
                    };
                    // Keep the sign-in pending until the redemption answers.
                    self.pending_state = Some(expected_state.clone());
                    let (tx, rx) = std::sync::mpsc::channel();
                    crate::bg::detach(move || {
                        let _ = tx.send(redeem_handoff(&expected_state, &code, &verifier));
                    });
                    self.await_redemption(rx, ctx);
                    return;
                }

                self.pending_verifier = None;
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

        let epoch = self.session_epoch;
        ctx.spawn(
            Timer::after(Duration::from_millis(20)),
            move |this, _, mctx| {
                this.poll_restore_result(rx, kind, epoch, mctx);
            },
        );
    }

    /// The answer to a restore started in `epoch` still concerns this
    /// session. The app is usable while a slow network answers: by then the
    /// person may have signed out, or in as someone else.
    fn restore_is_current(&self, epoch: u64) -> bool {
        self.session_epoch == epoch && self.refresh_token.is_some()
    }

    fn poll_restore_result(
        &mut self,
        rx: std::sync::mpsc::Receiver<Result<crate::api::RestoredSession, crate::api::ApiError>>,
        kind: RestoreKind,
        epoch: u64,
        ctx: &mut ModelContext<Self>,
    ) {
        if !self.restore_is_current(epoch) {
            log::info!("[auth] dropping the restore of a session that has ended");
            if kind == RestoreKind::Initial {
                crate::startup_timing::mark(crate::startup_timing::Phase::Auth);
            }
            return;
        }
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.spawn(
                    Timer::after(Duration::from_millis(20)),
                    move |this, _, mctx| {
                        this.poll_restore_result(rx, kind, epoch, mctx);
                    },
                );
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                warn!("[auth] silent restore thread disconnected unexpectedly");
                if kind == RestoreKind::Initial {
                    crate::startup_timing::mark(crate::startup_timing::Phase::Auth);
                }
                self.assume_offline_session(ctx);
            }
            Ok(result) => {
                if kind == RestoreKind::Initial {
                    crate::startup_timing::mark(crate::startup_timing::Phase::Auth);
                }
                self.apply_restore_result(result, kind, ctx);
            }
        }
    }

    /// Apply what a silent restore answered. The app never waited for it:
    /// the window and the cached session were up from the first frame.
    fn apply_restore_result(
        &mut self,
        result: Result<crate::api::RestoredSession, crate::api::ApiError>,
        kind: RestoreKind,
        ctx: &mut ModelContext<Self>,
    ) {
        match (restore_outcome(&result), result) {
            (RestoreOutcome::SignedIn, Ok(session)) => {
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
            (RestoreOutcome::SignOut, _) => {
                self.handle_session_expired(ctx);
            }
            // Anything else (offline, 5xx, malformed body) is transient: the
            // token may well be fine. Don't punish the user for a flaky network
            // — keep the session and let the next call settle it.
            (_, result) => {
                if let Err(err) = result {
                    warn!("[auth] silent session restore failed (transient): {err}");
                }
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
        // The local demo identity has nothing to sign out of.
        if self.is_local() {
            return;
        }
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
        // The client first: a token exchange still running checks it before
        // and after storing what it got.
        crate::api::API_CLIENT.clear();
        if let Err(err) = clear_refresh_token() {
            warn!("failed to clear keychain session: {err:#}");
        }
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
        // Notify ApiClient of the new refresh token so it can exchange on next
        // server call; before storing it, so that an exchange still running
        // for the previous session can't store its own over it.
        crate::api::API_CLIENT.set_refresh_token(refresh_token.clone());
        if let Err(err) = save_refresh_token(&refresh_token) {
            warn!("failed to persist refresh token: {err:#}");
            crate::api::API_CLIENT.clear();
            self.fail_sign_in("Could not save session securely.".to_string(), ctx);
            self.status = AuthStatus::LoggedOut;
            return;
        }

        // A new session starts here — retire any timers armed by the previous one.
        self.session_epoch = self.session_epoch.wrapping_add(1);
        self.pending_state = None;
        self.pending_since = None;
        self.sign_in_problem = None;
        self.refresh_token = Some(refresh_token);
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

    /// Whether `attempt` is the sign-in still being waited for.
    fn waiting_for(&self, attempt: &str) -> bool {
        matches!(self.status, AuthStatus::SigningIn)
            && self.pending_state.as_deref() == Some(attempt)
    }

    fn schedule_poll(&mut self, ctx: &mut ModelContext<Self>) {
        // Each attempt polls for itself: one replaced by "Try again" stops.
        let Some(attempt) = self.pending_state.clone() else {
            return;
        };
        ctx.spawn(Timer::after(POLL_INTERVAL), move |auth, _, ctx| {
            if !auth.waiting_for(&attempt) {
                return;
            }
            if auth.expire_sign_in(Instant::now()) {
                auth.fail_sign_in(SIGN_IN_EXPIRED.to_string(), ctx);
                return;
            }
            let (Some(state), Some(verifier)) =
                (auth.pending_state.clone(), auth.pending_verifier.clone())
            else {
                return;
            };

            // The HTTP poll BLOCKS for up to its 8s timeout — running it
            // here (timer callbacks are on the UI thread) froze the whole
            // app on every poll whenever the auth endpoint was slow. Do
            // the request on a worker thread; the UI only checks a channel.
            let (tx, rx) = std::sync::mpsc::channel();
            crate::bg::detach(move || {
                let _ = tx.send(try_poll_pending_session(&state, &verifier));
            });
            auth.await_poll_result(attempt, rx, ctx);
        });
    }

    /// Non-blocking wait for the background poll's result: re-arm a short UI
    /// timer and `try_recv` until the worker thread reports back.
    fn await_poll_result(
        &mut self,
        attempt: String,
        rx: std::sync::mpsc::Receiver<PollResult>,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.spawn(
            Timer::after(Duration::from_millis(100)),
            move |auth, _, ctx| {
                if !auth.waiting_for(&attempt) {
                    return;
                }
                match rx.try_recv() {
                    Ok(PollResult::Ready(callback)) => auth.handle_auth_callback(callback, ctx),
                    Ok(PollResult::Pending) => auth.schedule_poll(ctx),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        auth.await_poll_result(attempt, rx, ctx)
                    }
                    // Worker died (should not happen) — resume the normal cadence.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => auth.schedule_poll(ctx),
                }
            },
        );
    }

    /// Non-blocking wait for a one-time code's redemption.
    fn await_redemption(
        &mut self,
        rx: std::sync::mpsc::Receiver<Result<AuthCallback, String>>,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.spawn(
            Timer::after(Duration::from_millis(100)),
            move |auth, _, ctx| match rx.try_recv() {
                Ok(Ok(callback)) => auth.handle_auth_callback(callback, ctx),
                Ok(Err(message)) => {
                    // Polling may still deliver this sign-in once approved.
                    // Cancelled or already signed in: nothing to say.
                    if matches!(auth.status, AuthStatus::SigningIn) {
                        auth.fail_sign_in(message, ctx);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => auth.await_redemption(rx, ctx),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
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

#[cfg(test)]
mod restore_order_tests {
    use super::*;
    use crate::api::ApiError;

    fn manager(status: AuthStatus) -> AuthManager {
        AuthManager {
            status,
            pending_state: None,
            pending_verifier: None,
            pending_since: None,
            sign_in_problem: None,
            refresh_token: Some("stored".into()),
            access_token: None,
            access_token_expires_at: None,
            session_epoch: 0,
        }
    }

    #[test]
    fn a_stored_session_counts_as_signed_in_before_the_refresh_answers() {
        // Launch never waits for the network: until the refresh answers, the
        // cached session is the signed-in state the window shows.
        assert!(manager(AuthStatus::Restoring).is_logged_in());
    }

    #[test]
    fn an_answer_for_an_ended_session_is_dropped() {
        let mut auth = manager(AuthStatus::Restoring);
        let epoch = auth.session_epoch;
        assert!(auth.restore_is_current(epoch));
        // Signed out while the network was slow…
        auth.reset_session_state();
        assert!(!auth.restore_is_current(epoch));
        // …and in again as someone else.
        auth.refresh_token = Some("another".into());
        assert!(!auth.restore_is_current(epoch));
        assert!(auth.restore_is_current(auth.session_epoch));
    }

    #[test]
    fn a_network_failure_keeps_the_session() {
        let offline: Result<(), ApiError> = Err(ApiError::Network("connect timed out".into()));
        assert_eq!(restore_outcome(&offline), RestoreOutcome::KeepSession);
        let server: Result<(), ApiError> = Err(ApiError::Server {
            status: 503,
            body: String::new(),
        });
        assert_eq!(restore_outcome(&server), RestoreOutcome::KeepSession);
        let parse: Result<(), ApiError> = Err(ApiError::Parse("eof".into()));
        assert_eq!(restore_outcome(&parse), RestoreOutcome::KeepSession);
    }

    #[test]
    fn a_refused_token_signs_out_when_the_refresh_finally_answers() {
        let refused: Result<(), ApiError> = Err(ApiError::NotAuthenticated);
        assert_eq!(restore_outcome(&refused), RestoreOutcome::SignOut);
        assert_eq!(
            restore_outcome(&Ok::<(), ApiError>(())),
            RestoreOutcome::SignedIn
        );
    }
}

#[cfg(test)]
mod sign_in_tests {
    use super::*;
    use warpui::App;

    fn waiting(app: &mut App) -> warpui::ModelHandle<AuthManager> {
        let auth = app.add_model(AuthManager::preview);
        auth.update(app, |auth, _| {
            auth.status = AuthStatus::SigningIn;
            auth.pending_state = Some("first".into());
            auth.pending_verifier = Some("verifier".into());
            auth.pending_since = Some(Instant::now());
        });
        auth
    }

    #[test]
    fn a_sign_in_waits_for_the_limit_then_expires() {
        App::test((), |mut app| async move {
            let auth = waiting(&mut app);
            auth.update(&mut app, |auth, _| {
                let start = auth.pending_since.unwrap();
                assert!(!auth.expire_sign_in(start + SIGN_IN_LIMIT - Duration::from_secs(1)));
                assert!(auth.pending_sign_in_code().is_some());
                assert!(auth.expire_sign_in(start + SIGN_IN_LIMIT));
                assert!(auth.pending_sign_in_code().is_none());
                assert!(matches!(auth.status, AuthStatus::LoggedOut));
            });
        });
    }

    #[test]
    fn a_replaced_attempt_is_no_longer_waited_for() {
        App::test((), |mut app| async move {
            let auth = waiting(&mut app);
            auth.update(&mut app, |auth, _| {
                assert!(auth.waiting_for("first"));
                auth.pending_state = Some("second".into());
                assert!(!auth.waiting_for("first"));
                assert!(auth.waiting_for("second"));
            });
        });
    }

    #[test]
    fn cancelling_clears_the_problem() {
        App::test((), |mut app| async move {
            let auth = waiting(&mut app);
            auth.update(&mut app, |auth, ctx| {
                auth.fail_sign_in(SIGN_IN_EXPIRED.to_string(), ctx);
                assert_eq!(auth.sign_in_problem(), Some(SIGN_IN_EXPIRED));
                auth.cancel_browser_sign_in();
                assert_eq!(auth.sign_in_problem(), None);
                assert!(matches!(auth.status, AuthStatus::LoggedOut));
            });
        });
    }
}
