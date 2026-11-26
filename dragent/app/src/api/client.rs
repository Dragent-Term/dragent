//! Central HTTP client — all server calls and token management live here.
//!
//! Every outbound request (GraphQL, embedding proxy) goes through `API_CLIENT`.
//! On a 401 the client exchanges the refresh token silently and retries once,
//! so callers never have to handle token rotation themselves.
//!
//! When a refresh token turns out to be permanently dead (expired, revoked, or
//! its family revoked by the server's reuse detection), the client wipes every
//! credential and broadcasts on [`session_expired_rx`]. `AuthManager` listens
//! and signs the user out at once — an expired session must never leave a
//! logged-in-looking UI behind.

use std::sync::atomic::{AtomicBool, Ordering};

use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};

pub static API_CLIENT: Lazy<ApiClient> = Lazy::new(|| ApiClient::new(super::backend_url()));

/// Broadcast for "this session is dead". Unbounded, but at most one message is
/// ever sent per session (see `ApiClient::expired_signalled`), so it cannot
/// grow even when nothing is listening (tests, headless runs).
static SESSION_EXPIRED: Lazy<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
    Lazy::new(async_channel::unbounded);

/// Subscribe to session-expiry notifications. Every message means: the stored
/// credentials are gone and the user must sign in again.
pub fn session_expired_rx() -> async_channel::Receiver<()> {
    SESSION_EXPIRED.1.clone()
}

/// One part of a `multipart/form-data` request.
#[derive(Clone, Debug)]
pub enum MultipartPart {
    Field {
        name: String,
        value: String,
    },
    File {
        name: String,
        filename: String,
        mime: String,
        data: Vec<u8>,
    },
}

/// Encode `parts` as a multipart body delimited by `boundary`.
pub fn multipart_body(boundary: &str, parts: &[MultipartPart]) -> Vec<u8> {
    // Header values are quoted; strip characters that would end the quote
    // or the header line.
    let clean = |s: &str| s.replace(['"', '\r', '\n'], "_");
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match part {
            MultipartPart::Field { name, value } => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                        clean(name)
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(value.as_bytes());
            }
            MultipartPart::File {
                name,
                filename,
                mime,
                data,
            } => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                        clean(name),
                        clean(filename),
                        clean(mime)
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(data);
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// A successful response's JSON, or `null` when it has no body: actions such
/// as switching organisation answer `204 No Content`, which is success.
#[cfg(not(target_family = "wasm"))]
fn json_or_null(resp: ureq::Response) -> Result<serde_json::Value, ApiError> {
    let text = resp
        .into_string()
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| ApiError::Parse(e.to_string()))
}

// ── Error type ────────────────────────────────────────────────────────────────

/// A token answer arrived for a session that has ended meanwhile.
const SESSION_CHANGED: &str = "the session changed while signing in";

#[derive(Debug)]
pub enum ApiError {
    /// Local mode (a Dev build with no server): the call was not sent.
    LocalMode,
    NotAuthenticated,
    Network(String),
    Server { status: u16, body: String },
    Parse(String),
}

impl ApiError {
    /// The call wasn't made because this build runs without a server.
    pub fn is_local_mode(&self) -> bool {
        matches!(self, Self::LocalMode)
    }

    /// Text fit to show a user: the server's own `{"error": "..."}` message
    /// when it sent one (e.g. "only the owner can subscribe"), otherwise a
    /// short description instead of a raw status dump.
    pub fn user_message(&self) -> String {
        match self {
            Self::LocalMode => super::NEEDS_ACCOUNT.to_string(),
            Self::NotAuthenticated => "Sign in to continue.".to_string(),
            Self::Network(_) => {
                "Couldn't reach Dragent. Check your connection and try again.".to_string()
            }
            Self::Server { status, body } => serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                .filter(|m| !m.trim().is_empty())
                .map(|m| {
                    let mut chars = m.chars();
                    chars
                        .next()
                        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or(m)
                })
                .unwrap_or_else(|| format!("Request failed ({status}).")),
            Self::Parse(_) => "Received an unexpected response from Dragent.".to_string(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalMode => write!(f, "{}", super::NEEDS_ACCOUNT),
            Self::NotAuthenticated => write!(f, "sign in required"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::Server { status, body } => write!(f, "server error {status}: {body}"),
            Self::Parse(e) => write!(f, "parse error: {e}"),
        }
    }
}

// ── Token state ───────────────────────────────────────────────────────────────

/// User identity captured from the most recent token exchange.
#[derive(Clone, Debug, Default)]
pub struct SessionUser {
    pub user_id: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Everything `AuthManager` needs to mark a session live WITHOUT performing
/// its own token exchange. The server rotates refresh tokens and revokes the
/// whole token family on reuse, so this client must be the ONLY exchanger —
/// a second exchange path once raced this one at startup and destroyed the
/// session family on every launch.
pub struct RestoredSession {
    pub user: SessionUser,
    pub access_token: String,
    pub expires_in_secs: u64,
}

struct TokenState {
    refresh_token: Option<String>,
    access_token: Option<String>,
    access_token_expires_at: Option<web_time::Instant>,
    user: SessionUser,
}

// ── ApiClient ─────────────────────────────────────────────────────────────────

pub struct ApiClient {
    base_url: String,
    token_state: RwLock<TokenState>,
    /// Ensures only one token exchange runs at a time across threads.
    exchange_in_progress: Mutex<()>,
    /// Latched when the session is declared dead, so the expiry is broadcast
    /// exactly once even when several threads discover it at the same moment.
    /// Reset when new credentials arrive.
    expired_signalled: AtomicBool,
    /// Local mode: every request answers [`ApiError::LocalMode`] before any
    /// token lookup or network I/O.
    local_mode: bool,
}

impl ApiClient {
    fn new(base_url: String) -> Self {
        Self::with_mode(base_url, super::is_local_mode())
    }

    fn with_mode(base_url: String, local_mode: bool) -> Self {
        Self {
            base_url,
            token_state: RwLock::new(TokenState {
                refresh_token: None,
                access_token: None,
                access_token_expires_at: None,
                user: SessionUser::default(),
            }),
            exchange_in_progress: Mutex::new(()),
            expired_signalled: AtomicBool::new(false),
            local_mode,
        }
    }

    // ── Auth state setters ────────────────────────────────────────────────────

    /// Call after successful sign-in or session restore.
    pub fn set_tokens(&self, refresh_token: String, access_token: String, expires_in_secs: u64) {
        // Trim 5 minutes off as a safety margin so we never use a token that's
        // about to expire mid-request. With 24h tokens this is negligible.
        let margin = expires_in_secs.saturating_sub(300);
        let mut state = self.token_state.write();
        state.refresh_token = Some(refresh_token);
        state.access_token = Some(access_token);
        state.access_token_expires_at =
            Some(web_time::Instant::now() + std::time::Duration::from_secs(margin));
        drop(state);
        self.expired_signalled.store(false, Ordering::SeqCst);
    }

    /// Call when a new refresh token arrives but the access token hasn't been exchanged yet.
    /// Clears any cached access token so the next call triggers a fresh exchange.
    pub fn set_refresh_token(&self, refresh_token: String) {
        let mut state = self.token_state.write();
        state.refresh_token = Some(refresh_token);
        state.access_token = None;
        state.access_token_expires_at = None;
        // Possibly a different account — identity is re-learned on next exchange.
        state.user = SessionUser::default();
        drop(state);
        self.expired_signalled.store(false, Ordering::SeqCst);
    }

    /// Clear all credentials on logout.
    pub fn clear(&self) {
        let mut state = self.token_state.write();
        state.refresh_token = None;
        state.access_token = None;
        state.access_token_expires_at = None;
        state.user = SessionUser::default();
        drop(state);
        // A deliberate sign-out is not an expiry — arm the latch for next time.
        self.expired_signalled.store(false, Ordering::SeqCst);
    }

    /// Declare the session permanently dead: wipe every credential (memory AND
    /// the saved credentials file) and tell the app to sign out. Called when the server
    /// says the refresh token is revoked/expired, or when it rejects an access
    /// token we minted seconds ago — neither is recoverable by retrying.
    ///
    /// Safe to call from any thread and any number of times; the broadcast
    /// fires once per session.
    fn expire_session(&self, reason: &str) {
        {
            let mut state = self.token_state.write();
            state.refresh_token = None;
            state.access_token = None;
            state.access_token_expires_at = None;
            state.user = SessionUser::default();
        }
        let _ = crate::auth::clear_refresh_token();

        if signal_expiry_once(&self.expired_signalled) {
            log::warn!("[auth] session expired ({reason}) — signing out");
        }
    }

    /// Drop the cached access token so the next request gets a fresh one
    /// (after a 401 on a token that should still have been valid).
    pub fn forget_access_token(&self) {
        let mut state = self.token_state.write();
        state.access_token = None;
        state.access_token_expires_at = None;
    }

    /// The refresh token currently held (post-rotation). `AuthManager` mirrors
    /// this after a restore instead of tracking its own copy.
    pub fn current_refresh_token(&self) -> Option<String> {
        self.token_state.read().refresh_token.clone()
    }

    // ── Session restore (single-flight) ───────────────────────────────────────

    /// Restore the session through the ONE exchange path. Blocks — call from a
    /// background thread. On success the rotated refresh token is already
    /// persisted and the access token cached; the returned struct carries the
    /// user identity so the caller never needs to exchange on its own.
    pub fn restore_session(&self) -> Result<RestoredSession, ApiError> {
        let access_token = self.get_access_token()?;
        let state = self.token_state.read();
        let expires_in_secs = state
            .access_token_expires_at
            .map(|exp| {
                exp.saturating_duration_since(web_time::Instant::now())
                    .as_secs()
            })
            .unwrap_or(0);
        Ok(RestoredSession {
            user: state.user.clone(),
            access_token,
            expires_in_secs,
        })
    }

    // ── Token acquisition ─────────────────────────────────────────────────────

    /// Returns a valid access token, exchanging the refresh token if necessary.
    /// Blocks during the network exchange — must be called off the UI thread.
    pub fn get_access_token(&self) -> Result<String, ApiError> {
        // Every request asks for a token first, so this one check keeps a
        // local-mode build off the network (and out of the Keychain).
        if self.local_mode {
            return Err(ApiError::LocalMode);
        }
        // Fast path: valid cached token
        {
            let state = self.token_state.read();
            if let (Some(ref token), Some(exp)) =
                (&state.access_token, state.access_token_expires_at)
            {
                if exp > web_time::Instant::now() {
                    log::debug!("[auth] access token cache hit");
                    return Ok(token.clone());
                }
            }
        }

        // Slow path: serialize exchanges across threads
        log::info!("[auth] access token expired or missing — acquiring exchange lock");
        let _guard = self.exchange_in_progress.lock();

        // Re-check: another thread may have refreshed while we waited for the lock
        {
            let state = self.token_state.read();
            if let (Some(ref token), Some(exp)) =
                (&state.access_token, state.access_token_expires_at)
            {
                if exp > web_time::Instant::now() {
                    log::info!("[auth] token refreshed by another thread while waiting — reusing");
                    return Ok(token.clone());
                }
            }
        }

        // Get refresh token: in-memory state first, then the saved session
        // (DRAGENT_REFRESH_TOKEN or this edition's credentials file)
        let refresh_token = {
            let state = self.token_state.read();
            state.refresh_token.clone()
        };
        let refresh_token = match refresh_token {
            Some(rt) => rt,
            None => match crate::auth::load_refresh_token() {
                Some(rt) => {
                    log::info!(
                        "[auth] loaded refresh token from {}",
                        crate::auth::refresh_token_source()
                    );
                    self.token_state.write().refresh_token = Some(rt.clone());
                    rt
                }
                None => {
                    log::warn!("[auth] no refresh token found — user not authenticated");
                    return Err(ApiError::NotAuthenticated);
                }
            },
        };

        // Exchange (exchange_in_progress held; token_state NOT held — avoids deadlock)
        log::info!("[auth] exchanging refresh token for new access token");
        let t = web_time::Instant::now();
        let result = crate::auth::exchange_token(&refresh_token).map_err(|e| {
            // A revoked / permanently invalid refresh token can never recover:
            // wipe the credentials and sign the user out right now. A network
            // error, by contrast, is transient — keep the session and retry later.
            if is_token_revoked_error(&e) {
                // Only the session that asked: the person may have signed
                // out, or in as someone else, while the server answered.
                if !self.holds_refresh_token(&refresh_token) {
                    return ApiError::Network(SESSION_CHANGED.into());
                }
                self.expire_session("refresh token revoked or expired");
                return ApiError::NotAuthenticated;
            }
            ApiError::Network(e)
        })?;
        log::info!(
            "[auth] token exchange completed in {}ms",
            t.elapsed().as_millis()
        );

        self.store_exchange(&refresh_token, result, |token| {
            let _ = match token {
                Some(token) => crate::auth::save_refresh_token(token),
                None => crate::auth::clear_refresh_token(),
            };
        })
    }

    /// Whether the session is still the one that held `token`.
    fn holds_refresh_token(&self, token: &str) -> bool {
        self.token_state.read().refresh_token.as_deref() == Some(token)
    }

    /// Adopt the answer to exchanging `used`, unless that session ended while
    /// the server answered: storing its rotated token would sign the person
    /// back in (or over the account they switched to) at the next launch.
    /// `persist` writes the stored token (`None` clears it); a sign-in or
    /// sign-out changes the token here before it changes the stored one, so
    /// re-checking after the write always leaves the two agreeing.
    fn store_exchange(
        &self,
        used: &str,
        result: crate::auth::token_exchange::ExchangeResult,
        persist: impl Fn(Option<&str>),
    ) -> Result<String, ApiError> {
        if !self.holds_refresh_token(used) {
            return Err(ApiError::Network(SESSION_CHANGED.into()));
        }
        persist(Some(&result.refresh_token));

        let expires_in = (result.expires_in as u64).saturating_sub(30);
        let access_token = result.access_token.clone();

        let mut state = self.token_state.write();
        if state.refresh_token.as_deref() != Some(used) {
            let current = state.refresh_token.clone();
            drop(state);
            persist(current.as_deref());
            return Err(ApiError::Network(SESSION_CHANGED.into()));
        }
        state.refresh_token = Some(result.refresh_token);
        state.access_token = Some(result.access_token);
        state.access_token_expires_at =
            Some(web_time::Instant::now() + std::time::Duration::from_secs(expires_in));
        state.user = SessionUser {
            user_id: Some(result.user_id),
            first_name: result.first_name,
            last_name: result.last_name,
            avatar_url: result.avatar_url,
        };

        Ok(access_token)
    }

    // ── HTTP ──────────────────────────────────────────────────────────────────

    /// POST JSON to `path` (relative to base_url).
    /// Injects Bearer auth automatically. On 401, exchanges a fresh token and retries once.
    pub fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, ApiError> {
        self.post_with_headers(path, body, timeout_secs, &[])
    }

    /// PATCH JSON to `path`. Same auth, refresh, and retry behaviour as `post`.
    pub fn patch(
        &self,
        path: &str,
        body: &serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, ApiError> {
        self.json_with_method("PATCH", path, body, timeout_secs, &[])
    }

    /// PUT JSON to `path`. Same auth, refresh, and retry behaviour as `post`.
    pub fn put(
        &self,
        path: &str,
        body: &serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, ApiError> {
        self.json_with_method("PUT", path, body, timeout_secs, &[])
    }

    /// Bind a desktop snapshot to the account that collected it. The relay
    /// rejects this assertion if shared auth changes during token refresh or
    /// a transport retry, so retired data cannot be stored for a new account.
    pub fn put_for_user(
        &self,
        path: &str,
        body: &serde_json::Value,
        expected_user: &str,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, ApiError> {
        if expected_user.trim().is_empty() || expected_user.chars().any(char::is_control) {
            return Err(ApiError::NotAuthenticated);
        }
        self.json_with_method(
            "PUT",
            path,
            body,
            timeout_secs,
            &[("X-Dragent-Publisher-User", expected_user)],
        )
    }

    /// DELETE `path`. Same auth, refresh, and retry behaviour as `post`.
    pub fn delete(&self, path: &str, timeout_secs: u64) -> Result<serde_json::Value, ApiError> {
        self.json_with_method("DELETE", path, &serde_json::Value::Null, timeout_secs, &[])
    }

    // ── Companion phone pairing ─────────────────────────────────────────────
    //
    // The phone never signs in: the signed-in desktop creates a single-use
    // pairing code, shows it as a QR, and the phone redeems it once. These
    // three calls are the desktop side of that contract; all block and must
    // run off the UI thread like every other method here.

    /// POST a new single-use pairing code (TTL 10 min, bound to this user).
    pub fn create_companion_pairing(
        &self,
        timeout_secs: u64,
    ) -> Result<CompanionPairing, ApiError> {
        let json = self.post(
            "v1/companion/pairings",
            &serde_json::json!({}),
            timeout_secs,
        )?;
        CompanionPairing::from_json(&json)
    }

    /// List the phones paired to this account.
    pub fn list_companion_devices(
        &self,
        timeout_secs: u64,
    ) -> Result<Vec<CompanionDevice>, ApiError> {
        let json = self.get("v1/companion/devices", timeout_secs)?;
        CompanionDevice::list_from_json(&json)
    }

    /// Revoke one paired phone (its sockets close, its tokens stop working).
    pub fn revoke_companion_device(
        &self,
        device_id: &str,
        timeout_secs: u64,
    ) -> Result<(), ApiError> {
        let path = format!("v1/companion/devices/{device_id}");
        self.delete(&path, timeout_secs)?;
        Ok(())
    }

    /// POST a `multipart/form-data` body (text fields and files). Same auth
    /// and 401 refresh as `post`, but transport failures are NOT retried: the
    /// request may already have been accepted, and a form submission must
    /// not be delivered twice.
    #[cfg(not(target_family = "wasm"))]
    pub fn post_multipart(
        &self,
        path: &str,
        parts: &[MultipartPart],
        timeout_secs: u64,
    ) -> Result<serde_json::Value, ApiError> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let boundary = format!("dragent-{}", uuid::Uuid::new_v4().simple());
        let body = multipart_body(&boundary, parts);
        let send = |tok: &str| {
            ureq::post(&url)
                .set("Authorization", &format!("Bearer {tok}"))
                .set(
                    "Content-Type",
                    &format!("multipart/form-data; boundary={boundary}"),
                )
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .send_bytes(&body)
        };
        let status_error = |code: u16, resp: ureq::Response| ApiError::Server {
            status: code,
            body: resp.into_string().unwrap_or_default(),
        };
        let token = self.get_access_token()?;
        match send(&token) {
            Ok(resp) => json_or_null(resp),
            Err(ureq::Error::Status(401, _)) => {
                {
                    let mut state = self.token_state.write();
                    state.access_token = None;
                    state.access_token_expires_at = None;
                }
                let new_token = self.get_access_token()?;
                match send(&new_token) {
                    Ok(resp) => json_or_null(resp),
                    Err(ureq::Error::Status(401, _)) => {
                        self.expire_session("server rejected a freshly issued access token");
                        Err(ApiError::NotAuthenticated)
                    }
                    Err(ureq::Error::Status(code, resp)) => Err(status_error(code, resp)),
                    Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
                }
            }
            Err(ureq::Error::Status(code, resp)) => Err(status_error(code, resp)),
            Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
        }
    }

    /// Like `post`, but also attaches the given extra (name, value) headers.
    pub fn post_with_headers(
        &self,
        path: &str,
        body: &serde_json::Value,
        timeout_secs: u64,
        extra: &[(&str, &str)],
    ) -> Result<serde_json::Value, ApiError> {
        self.json_with_method("POST", path, body, timeout_secs, extra)
    }

    /// POST JSON and keep the response body open as a stream. This is the
    /// initial agent-turn transport: task creation and SSE delivery share one
    /// request, while the ordinary GET stream is reserved for reconnects.
    ///
    /// Unlike the JSON POST helper, this deliberately does not retry transport
    /// failures: once a streaming POST reaches the server, replaying it could
    /// create a second agent task. A 401 is safe to retry because authentication
    /// middleware rejects it before the handler can create a task.
    #[cfg(not(target_family = "wasm"))]
    pub fn post_stream_with_headers(
        &self,
        path: &str,
        body: &serde_json::Value,
        extra: &[(&str, &str)],
    ) -> Result<ureq::Response, ApiError> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let send = |token: &str| {
            let agent = ureq::builder()
                .timeout_connect(std::time::Duration::from_secs(10))
                .timeout_read(std::time::Duration::from_secs(90))
                .build();
            let mut req = agent
                .post(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .set("Accept", "text/event-stream")
                .set("Cache-Control", "no-cache");
            for (name, value) in extra {
                req = req.set(name, value);
            }
            req.send_json(body)
        };

        let token = self.get_access_token()?;
        match send(&token) {
            Ok(resp) => Ok(resp),
            Err(ureq::Error::Status(401, _)) => {
                {
                    let mut state = self.token_state.write();
                    state.access_token = None;
                    state.access_token_expires_at = None;
                }
                let new_token = self.get_access_token()?;
                match send(&new_token) {
                    Ok(resp) => Ok(resp),
                    Err(ureq::Error::Status(401, _)) => {
                        self.expire_session("server rejected a freshly issued access token");
                        Err(ApiError::NotAuthenticated)
                    }
                    Err(ureq::Error::Status(code, resp)) => Err(ApiError::Server {
                        status: code,
                        body: resp.into_string().unwrap_or_default(),
                    }),
                    Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
                }
            }
            Err(ureq::Error::Status(code, resp)) => Err(ApiError::Server {
                status: code,
                body: resp.into_string().unwrap_or_default(),
            }),
            Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
        }
    }

    /// Shared body for every JSON-with-payload verb. The 401-refresh and the
    /// transient-retry loop are subtle enough that having one copy matters
    /// more than saving a parameter.
    ///
    /// Native-only: uses the blocking `ureq` HTTP client. The web viewer talks to
    /// the backend over its session WebSocket, not this REST client — on wasm the
    /// method short-circuits to a network error (see the wasm variant below).
    #[cfg(not(target_family = "wasm"))]
    fn json_with_method(
        &self,
        method: &str,
        path: &str,
        body: &serde_json::Value,
        timeout_secs: u64,
        extra: &[(&str, &str)],
    ) -> Result<serde_json::Value, ApiError> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let make_req = |tok: &str| {
            let mut req = ureq::request(method, &url)
                .set("Authorization", &format!("Bearer {tok}"))
                .set("Content-Type", "application/json")
                .timeout(std::time::Duration::from_secs(timeout_secs));
            for (name, value) in extra {
                req = req.set(name, value);
            }
            req
        };

        // Transient transport failures (a dropped connection, TLS reset, or an
        // App Runner cold-start) must NOT surface to the user as "Connection
        // error" on the first blip — retry a couple of times with backoff. Only
        // network/transport errors are retried; HTTP status errors, auth, and
        // parse failures are definitive and returned immediately.
        const MAX_ATTEMPTS: u32 = 3;
        let mut attempt: u32 = 1;
        loop {
            let token = self.get_access_token()?;
            let result = match make_req(&token).send_json(body) {
                Ok(resp) => json_or_null(resp),

                Err(ureq::Error::Status(401, _)) => {
                    {
                        let mut state = self.token_state.write();
                        state.access_token = None;
                        state.access_token_expires_at = None;
                    }
                    let new_token = self.get_access_token()?;
                    match make_req(&new_token).send_json(body) {
                        Ok(resp) => json_or_null(resp),
                        // The server rejected an access token minted moments ago —
                        // the session is dead on its side (user deleted, family
                        // revoked). Retrying can only loop; sign out instead.
                        Err(ureq::Error::Status(401, _)) => {
                            self.expire_session("server rejected a freshly issued access token");
                            Err(ApiError::NotAuthenticated)
                        }
                        Err(ureq::Error::Status(code, resp)) => Err(ApiError::Server {
                            status: code,
                            body: resp.into_string().unwrap_or_default(),
                        }),
                        Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
                    }
                }

                Err(ureq::Error::Status(code, resp)) => Err(ApiError::Server {
                    status: code,
                    body: resp.into_string().unwrap_or_default(),
                }),
                Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
            };

            match result {
                Err(ApiError::Network(ref detail)) if attempt < MAX_ATTEMPTS => {
                    let backoff =
                        std::time::Duration::from_millis(300u64 * (1u64 << (attempt - 1)));
                    log::warn!(
                        "[api] transient network error on {} {} (attempt {}/{}), retrying in {:?}: {}",
                        method, path, attempt, MAX_ATTEMPTS, backoff, detail
                    );
                    std::thread::sleep(backoff);
                    attempt += 1;
                    continue;
                }
                other => return other,
            }
        }
    }

    /// Wasm: the REST client is unavailable in the viewer (no `ureq`).
    #[cfg(target_family = "wasm")]
    fn json_with_method(
        &self,
        _method: &str,
        _path: &str,
        _body: &serde_json::Value,
        _timeout_secs: u64,
        _extra: &[(&str, &str)],
    ) -> Result<serde_json::Value, ApiError> {
        Err(ApiError::Network(
            "REST client unavailable in the web viewer".to_string(),
        ))
    }

    /// GET a JSON resource from `path` (relative to base_url).
    /// Injects Bearer auth automatically. On 401, exchanges a fresh token and retries once.
    #[cfg(not(target_family = "wasm"))]
    pub fn get(&self, path: &str, timeout_secs: u64) -> Result<serde_json::Value, ApiError> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        // GET is idempotent — safe to retry transient transport blips with
        // backoff so a dropped connection / cold-start never shows the user a
        // "Connection error". Status/auth/parse errors are returned immediately.
        const MAX_ATTEMPTS: u32 = 3;
        let mut attempt: u32 = 1;
        loop {
            let token = self.get_access_token()?;
            let result = match ureq::get(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .call()
            {
                Ok(resp) => resp
                    .into_json::<serde_json::Value>()
                    .map_err(|e| ApiError::Parse(e.to_string())),

                Err(ureq::Error::Status(401, _)) => {
                    {
                        let mut state = self.token_state.write();
                        state.access_token = None;
                        state.access_token_expires_at = None;
                    }
                    let new_token = self.get_access_token()?;
                    match ureq::get(&url)
                        .set("Authorization", &format!("Bearer {new_token}"))
                        .timeout(std::time::Duration::from_secs(timeout_secs))
                        .call()
                    {
                        Ok(resp) => resp
                            .into_json::<serde_json::Value>()
                            .map_err(|e| ApiError::Parse(e.to_string())),
                        // Fresh token rejected too — the session is dead server-side.
                        Err(ureq::Error::Status(401, _)) => {
                            self.expire_session("server rejected a freshly issued access token");
                            Err(ApiError::NotAuthenticated)
                        }
                        Err(ureq::Error::Status(code, resp)) => Err(ApiError::Server {
                            status: code,
                            body: resp.into_string().unwrap_or_default(),
                        }),
                        Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
                    }
                }

                Err(ureq::Error::Status(code, resp)) => Err(ApiError::Server {
                    status: code,
                    body: resp.into_string().unwrap_or_default(),
                }),
                Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
            };

            match result {
                Err(ApiError::Network(ref detail)) if attempt < MAX_ATTEMPTS => {
                    let backoff =
                        std::time::Duration::from_millis(300u64 * (1u64 << (attempt - 1)));
                    log::warn!(
                        "[api] transient network error on GET {} (attempt {}/{}), retrying in {:?}: {}",
                        path, attempt, MAX_ATTEMPTS, backoff, detail
                    );
                    std::thread::sleep(backoff);
                    attempt += 1;
                    continue;
                }
                other => return other,
            }
        }
    }

    /// Wasm: no `ureq` GET in the viewer.
    #[cfg(target_family = "wasm")]
    pub fn get(&self, _path: &str, _timeout_secs: u64) -> Result<serde_json::Value, ApiError> {
        Err(ApiError::Network(
            "REST client unavailable in the web viewer".to_string(),
        ))
    }

    /// Send a GraphQL query/mutation. Returns the full JSON body on success.
    pub fn graphql(
        &self,
        query: &str,
        variables: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, ApiError> {
        self.graphql_with_headers(query, variables, timeout_secs, &[])
    }

    /// Like `graphql`, but also attaches extra (name, value) request headers.
    pub fn graphql_with_headers(
        &self,
        query: &str,
        variables: serde_json::Value,
        timeout_secs: u64,
        extra: &[(&str, &str)],
    ) -> Result<serde_json::Value, ApiError> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        self.post_with_headers("query", &body, timeout_secs, extra)
    }
}

// ── Companion pairing types + pure helpers ──────────────────────────────────

/// A single-use phone-pairing code created for the signed-in desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanionPairing {
    pub pairing_id: String,
    pub code: String,
    pub gateway_url: String,
    /// Server `expires_at`, normalised to epoch seconds.
    pub expires_at_epoch: i64,
}

impl CompanionPairing {
    fn from_json(json: &serde_json::Value) -> Result<Self, ApiError> {
        let get_str = |key: &str| {
            json.get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| ApiError::Parse(format!("pairing missing {key:?}")))
        };
        let expires_at_epoch = json
            .get("expires_at")
            .and_then(parse_expires_at_epoch)
            .ok_or_else(|| ApiError::Parse("pairing missing expires_at".to_string()))?;
        Ok(Self {
            pairing_id: get_str("pairing_id")?,
            code: get_str("code")?,
            gateway_url: get_str("gateway_url")?,
            expires_at_epoch,
        })
    }
}

/// A phone paired to this account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanionDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

impl CompanionDevice {
    /// The server answers a top-level list; accept a `{"devices": [...]}` envelope too.
    fn list_from_json(json: &serde_json::Value) -> Result<Vec<Self>, ApiError> {
        let arr = if let Some(arr) = json.as_array() {
            arr
        } else if let Some(arr) = json.get("devices").and_then(|v| v.as_array()) {
            arr
        } else {
            return Err(ApiError::Parse(
                "devices response is not a list".to_string(),
            ));
        };
        arr.iter()
            .map(|item| {
                let get_str = |key: &str| {
                    item.get(key)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| ApiError::Parse(format!("device missing {key:?}")))
                };
                Ok(Self {
                    device_id: get_str("device_id")?,
                    name: get_str("name").unwrap_or_else(|_| "Phone".to_string()),
                    platform: get_str("platform").unwrap_or_default(),
                    created_at: get_str("created_at").unwrap_or_default(),
                    last_seen_at: item
                        .get("last_seen_at")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                })
            })
            .collect()
    }
}

/// `expires_at` arrives either as epoch seconds (number or numeric string)
/// or as an RFC 3339 timestamp — normalise to epoch seconds.
fn parse_expires_at_epoch(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if let Ok(n) = s.parse::<i64>() {
                return Some(n);
            }
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp())
        }
        _ => None,
    }
}

/// Deep link encoded in the pairing QR (the phone scans it or pastes the
/// code shown beside it): `<scheme>://pair?c=<code>&g=<gateway urlencoded>`
/// where `<scheme>` is this edition's `dragent_paths::url_scheme()`.
pub fn companion_pairing_deep_link(code: &str, gateway_url: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(gateway_url.as_bytes()).collect();
    let scheme = dragent_paths::url_scheme();
    format!("{scheme}://pair?c={code}&g={encoded}")
}

/// Seconds left before `expires_at_epoch`, never negative.
pub fn pairing_remaining_secs(expires_at_epoch: i64, now_epoch: i64) -> u64 {
    expires_at_epoch.saturating_sub(now_epoch).max(0) as u64
}

/// `m:ss` countdown label, e.g. `9:59`.
pub fn format_pairing_countdown(remaining_secs: u64) -> String {
    format!("{}:{:02}", remaining_secs / 60, remaining_secs % 60)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Broadcast a session expiry, at most once per `latch`. Returns whether this
/// call was the one that notified — several threads can hit a dead token at the
/// same instant and we must not sign the user out (or log) more than once.
fn signal_expiry_once(latch: &AtomicBool) -> bool {
    if latch.swap(true, Ordering::SeqCst) {
        return false;
    }
    // Send only fails when every receiver is gone — fine, nothing to sign out.
    let _ = SESSION_EXPIRED.0.try_send(());
    true
}

/// Returns true if a token-exchange error message indicates a permanently
/// invalid refresh token (revoked, expired, or deleted server-side).
/// These sessions can never recover — the user must sign in again.
fn is_token_revoked_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("revoked")
        || lower.contains("refresh token")
            && (lower.contains("invalid") || lower.contains("expired"))
        || lower.contains("token not found")
        || lower.contains("token has expired")
        || lower.contains("token is invalid")
        || lower.contains("no such token")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchanged() -> crate::auth::token_exchange::ExchangeResult {
        crate::auth::token_exchange::ExchangeResult {
            access_token: "access".into(),
            refresh_token: "rotated".into(),
            user_id: "u".into(),
            expires_in: 3600,
            first_name: None,
            last_name: None,
            avatar_url: None,
        }
    }

    #[test]
    fn a_late_exchange_never_brings_back_a_session_that_ended() {
        let client = ApiClient::with_mode("http://127.0.0.1:9".into(), false);
        let stored = std::cell::RefCell::new(Some("old".to_string()));
        let persist = |token: Option<&str>| *stored.borrow_mut() = token.map(str::to_owned);

        // Still the same session: the rotated token is adopted.
        client.set_refresh_token("old".into());
        assert!(client.store_exchange("old", exchanged(), persist).is_ok());
        assert_eq!(stored.borrow().as_deref(), Some("rotated"));
        assert!(client.holds_refresh_token("rotated"));

        // Signed out before the answer.
        client.clear();
        persist(None);
        assert!(client.store_exchange("old", exchanged(), persist).is_err());
        assert_eq!(*stored.borrow(), None);
        assert_eq!(client.current_refresh_token(), None);

        // Signed in as someone else while the rotated token was written.
        client.set_refresh_token("old".into());
        let switched = std::cell::Cell::new(false);
        let result = client.store_exchange("old", exchanged(), |token| {
            if !switched.replace(true) {
                client.set_refresh_token("other".into());
                persist(Some("other"));
            }
            persist(token);
        });
        assert!(result.is_err());
        assert_eq!(stored.borrow().as_deref(), Some("other"));
        assert!(client.holds_refresh_token("other"));
    }

    /// Local mode answers every verb without opening a connection: a
    /// listener standing in for the server must never see one.
    #[test]
    fn local_mode_client_never_touches_the_network() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let client = ApiClient::with_mode(base, true);
        let body = serde_json::json!({});
        let results = [
            client.get("v1/me", 1).err(),
            client.post("v1/x", &body, 1).err(),
            client.patch("v1/x", &body, 1).err(),
            client.put("v1/x", &body, 1).err(),
            client.delete("v1/x", 1).err(),
            client.graphql("{ me { id } }", body.clone(), 1).err(),
            client.get_access_token().err(),
            client.create_companion_pairing(1).err(),
            client.post_multipart("v1/upload", &[], 1).err(),
            client.post_stream_with_headers("v1/stream", &body, &[]).err(),
        ];
        for err in results {
            let err = err.expect("local mode must not succeed");
            assert!(err.is_local_mode(), "{err}");
            assert_eq!(err.user_message(), "Needs a Dragent account");
        }
        assert!(
            listener.accept().is_err(),
            "local mode must not connect to the server"
        );
    }

    #[test]
    fn multipart_body_frames_fields_and_files() {
        let body = multipart_body(
            "B",
            &[
                MultipartPart::Field {
                    name: "content".into(),
                    value: "hi \"there\"".into(),
                },
                MultipartPart::File {
                    name: "file0".into(),
                    filename: "a\"b.png".into(),
                    mime: "image/png".into(),
                    data: vec![1, 2],
                },
            ],
        );
        let mut expected = b"--B\r\nContent-Disposition: form-data; name=\"content\"\r\n\r\nhi \"there\"\r\n--B\r\nContent-Disposition: form-data; name=\"file0\"; filename=\"a_b.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
        expected.extend_from_slice(&[1, 2]);
        expected.extend_from_slice(b"\r\n--B--\r\n");
        assert_eq!(body, expected);
    }

    /// The exact strings the server sends back (identity/components/refresh_token.go)
    /// wrapped the way `exchange_token` reports them. If these stop matching, an
    /// expired session silently degrades into an endless retry loop instead of a
    /// sign-out — so pin them.
    #[test]
    fn recognises_server_dead_token_errors() {
        for msg in [
            r#"GraphQL errors: [{"message":"invalid or expired refresh token"}]"#,
            r#"GraphQL errors: [{"message":"refresh token has been revoked previously"}]"#,
            "token not found",
            "The token has expired",
        ] {
            assert!(is_token_revoked_error(msg), "should be fatal: {msg}");
        }
    }

    /// Transient failures must NOT sign the user out — being offline is not
    /// the same as being logged out.
    #[test]
    fn keeps_session_on_transient_errors() {
        for msg in [
            "network error: Dns Failed",
            "HTTP 502",
            "parse error: expected value",
            r#"GraphQL errors: [{"message":"rate limit exceeded"}]"#,
        ] {
            assert!(!is_token_revoked_error(msg), "should be transient: {msg}");
        }
    }

    /// Many threads can discover the same dead token at once; the app must be
    /// told exactly once.
    #[test]
    fn expiry_is_broadcast_once_per_session() {
        let rx = session_expired_rx();
        while rx.try_recv().is_ok() {} // drain anything from other tests

        let latch = AtomicBool::new(false);
        assert!(signal_expiry_once(&latch), "first call notifies");
        assert!(!signal_expiry_once(&latch), "second call is a no-op");
        assert!(!signal_expiry_once(&latch), "and stays a no-op");

        assert!(rx.try_recv().is_ok(), "exactly one notification…");
        assert!(rx.try_recv().is_err(), "…and no more");

        // A new sign-in re-arms the latch.
        latch.store(false, Ordering::SeqCst);
        assert!(signal_expiry_once(&latch), "next session can expire too");
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn pairing_deep_link_encodes_gateway() {
        let link = companion_pairing_deep_link("ABCDEFGH1234567890AB", "https://relay.example.com:443/x");
        let scheme = dragent_paths::url_scheme();
        assert!(
            link.starts_with(&format!("{scheme}://pair?c=ABCDEFGH1234567890AB&g=")),
            "unexpected link: {link}"
        );
        assert_eq!(
            url::Url::parse(&link).expect("deep link must parse").scheme(),
            scheme
        );
        assert!(
            !link.contains("://relay"),
            "gateway must be urlencoded: {link}"
        );
        // Round-trips through a URL parser back to the original gateway.
        let parsed = url::Url::parse(&link).expect("deep link must parse");
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(
            pairs.get("c").map(|s| s.as_ref()),
            Some("ABCDEFGH1234567890AB")
        );
        assert_eq!(
            pairs.get("g").map(|s| s.as_ref()),
            Some("https://relay.example.com:443/x")
        );
    }

    #[test]
    fn pairing_countdown_formats_and_saturates() {
        assert_eq!(pairing_remaining_secs(1_700_000_600, 1_700_000_000), 600);
        assert_eq!(pairing_remaining_secs(1_700_000_000, 1_700_000_600), 0);
        assert_eq!(pairing_remaining_secs(100, 200), 0);
        assert_eq!(format_pairing_countdown(600), "10:00");
        assert_eq!(format_pairing_countdown(599), "9:59");
        assert_eq!(format_pairing_countdown(61), "1:01");
        assert_eq!(format_pairing_countdown(5), "0:05");
        assert_eq!(format_pairing_countdown(0), "0:00");
    }

    #[test]
    fn pairing_parses_expiry_shapes() {
        assert_eq!(
            parse_expires_at_epoch(&serde_json::json!(1_700_000_600)),
            Some(1_700_000_600)
        );
        assert_eq!(
            parse_expires_at_epoch(&serde_json::json!("1700000600")),
            Some(1_700_000_600)
        );
        assert_eq!(
            parse_expires_at_epoch(&serde_json::json!("2030-01-01T00:00:00Z")),
            Some(1_893_456_000)
        );
        assert_eq!(parse_expires_at_epoch(&serde_json::json!(null)), None);
        assert_eq!(parse_expires_at_epoch(&serde_json::json!("soon")), None);
    }

    #[test]
    fn pairing_from_json_pins_server_shape() {
        let json = serde_json::json!({
            "pairing_id": "p_123",
            "code": "ABCDEFGH1234567890AB",
            "expires_at": "2030-01-01T00:00:00Z",
            "gateway_url": "https://relay.example.com",
        });
        let pairing = CompanionPairing::from_json(&json).expect("parses");
        assert_eq!(pairing.pairing_id, "p_123");
        assert_eq!(pairing.code, "ABCDEFGH1234567890AB");
        assert_eq!(pairing.gateway_url, "https://relay.example.com");
        assert_eq!(pairing.expires_at_epoch, 1_893_456_000);

        let bad = serde_json::json!({"pairing_id": "p_123"});
        assert!(CompanionPairing::from_json(&bad).is_err());
    }

    #[test]
    fn devices_list_accepts_array_and_envelope() {
        let entry = serde_json::json!({
            "device_id": "d_1",
            "name": "Pixel 9",
            "platform": "android",
            "created_at": "2026-01-01T00:00:00Z",
            "last_seen_at": "2026-01-02T00:00:00Z",
        });
        let array = serde_json::Value::Array(vec![entry.clone()]);
        let devices = CompanionDevice::list_from_json(&array).expect("array parses");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "d_1");
        assert_eq!(devices[0].name, "Pixel 9");
        assert_eq!(
            devices[0].last_seen_at.as_deref(),
            Some("2026-01-02T00:00:00Z")
        );

        let envelope = serde_json::json!({"devices": [entry]});
        assert_eq!(
            CompanionDevice::list_from_json(&envelope)
                .expect("envelope parses")
                .len(),
            1
        );
        assert!(CompanionDevice::list_from_json(&serde_json::json!({})).is_err());
    }
}
