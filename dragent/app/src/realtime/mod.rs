//! Realtime WebSocket transport for remote / shared sessions.
//!
//! One persistent WebSocket to the backend session gateway, used by both the
//! **host** (broadcasts terminal output, receives command requests) and a
//! **native participant** (receives output, sends commands). The transport here
//! is deliberately message-agnostic: it moves JSON **text frames** in and out.
//! The session layer above encodes/decodes them with [`dragent_session_protocol`].
//!
//! ## Design
//! Mirrors the voice module (`crate::voice`): a dedicated OS thread runs a
//! synchronous [`tungstenite`] loop with a short read timeout so one loop
//! services sends, receives, and the stop flag. Inbound frames and lifecycle
//! changes are delivered to the owning UI Entity through an `async_channel`
//! (consume with `ctx.spawn_stream_local`). We do **not** use
//! `graphql-ws-client` — that speaks the GraphQL-subscription subprotocol, which
//! is wrong for our custom session frames.
//!
//! Keepalive is handled at the protocol layer (the session sends
//! `Ping`/receives `Pong` as normal messages); the transport only auto-responds
//! to WebSocket control pings (tungstenite does this) and detects a dead socket
//! via send/read errors, then reconnects with capped backoff. A `stop` flag
//! (set on sign-out via `crate::api::session_expired_rx`, or when the user ends
//! the session) tears the loop down.

pub(crate) mod companion_accounts;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod companion_coordination;
pub(crate) mod companion_history;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod companion_host;
pub(crate) mod companion_starts;
pub(crate) mod companion_tasks;
pub(crate) mod companion_trackers;
pub(crate) mod companion_uploads;
pub(crate) mod companion_usage;
pub mod controller;
pub(crate) mod desktop_catalog;
pub(crate) mod keep_responsive;
// Phone relay end-to-end encryption (docs/design/e2ee.md). Behind the `e2ee`
// feature until the P2 audit gate (§3.3); native only (§1.3: keys live only in
// the signed desktop app).
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
pub mod e2e_channel;
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
pub mod e2e_keys;
// E9a seals pushes here and sends them with
// `companion_host::send_host_notify(phone_id, blob)`; with the feature the
// live host socket (`companion_host::e2ee_live`) carries them as
// `host-notify` frames. Settings → Mobile reaches the live host through
// `companion_host::e2ee_live::{begin_pairing, cancel_pairing, revoke_phone,
// reset_keys, status}`.
#[cfg(not(target_family = "wasm"))]
pub(crate) mod desktop_publisher;
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
pub mod e2e_notify;
#[cfg(all(feature = "e2ee", not(target_family = "wasm")))]
pub mod e2e_store;
pub mod forward;
pub mod forward_tunnel;
pub mod manager;
pub mod phone_agent;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod phone_browser;
pub mod phone_control;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod phone_git;
pub mod qr;
pub mod rest;
pub mod session;
pub(crate) mod session_events;
pub(crate) mod session_host;
pub(crate) mod session_viewer;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

/// Path appended to the backend origin for the session gateway.
const GATEWAY_PATH: &str = "/realtime";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
/// Short so the loop stays responsive to sends and the stop flag.
const READ_TIMEOUT: Duration = Duration::from_millis(40);
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[cfg(not(target_family = "wasm"))]
pub(crate) type Socket =
    tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// Lifecycle + inbound messages delivered to the owning Entity.
#[derive(Debug, Clone)]
pub enum RealtimeEvent {
    /// The socket is open and authenticated; safe to send.
    Connected,
    /// An inbound JSON text frame (decode with the session protocol).
    Message(String),
    /// The socket dropped. `will_retry` = the client is reconnecting with
    /// backoff; `false` means it has stopped for good.
    Disconnected { will_retry: bool },
    /// A non-fatal error worth surfacing (connect failure, auth error).
    Error(String),
}

/// Handle to a running realtime connection. Dropping does **not** stop it —
/// call [`RealtimeHandle::stop`] (matches the voice module's contract).
#[derive(Clone)]
pub struct RealtimeHandle {
    outbound: async_channel::Sender<String>,
    stop: Arc<AtomicBool>,
}

impl RealtimeHandle {
    /// Queue a JSON text frame to send. Non-blocking and safe from the UI thread.
    pub fn send(&self, json: impl Into<String>) {
        let _ = self.outbound.try_send(json.into());
    }

    /// Stop the connection and its reconnect loop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// A detached handle with no transport thread, for tests: the returned
    /// receiver observes everything [`send`](Self::send) queues.
    #[cfg(test)]
    pub fn for_test() -> (RealtimeHandle, async_channel::Receiver<String>) {
        let (tx, rx) = async_channel::unbounded::<String>();
        (
            RealtimeHandle {
                outbound: tx,
                stop: Arc::new(AtomicBool::new(false)),
            },
            rx,
        )
    }
}

/// Connection configuration.
pub struct RealtimeConfig {
    /// Full `ws(s)://…` URL of the gateway.
    pub url: String,
}

impl RealtimeConfig {
    /// Default config pointing at the backend's session gateway.
    pub fn gateway() -> Self {
        Self { url: gateway_url() }
    }
}

/// Open a realtime connection. Inbound events are streamed into `tx`; the
/// returned handle sends outbound frames and stops the connection.
///
/// Native: drives a background `tungstenite` thread. The browser transport
/// (web-sys `WebSocket`) is wired separately — until then the wasm variant
/// returns an inert handle so the session layer type-checks and renders.
#[cfg(not(target_family = "wasm"))]
pub fn start(config: RealtimeConfig, tx: async_channel::Sender<RealtimeEvent>) -> RealtimeHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (outbound_tx, outbound_rx) = async_channel::unbounded::<String>();
    let stop_thread = stop.clone();
    let _ = std::thread::Builder::new()
        .name("realtime-ws".into())
        .spawn(move || run(config, tx, outbound_rx, stop_thread));
    RealtimeHandle {
        outbound: outbound_tx,
        stop,
    }
}

/// Wasm: the browser `WebSocket` transport. Mirrors the native loop at the
/// message level — `RealtimeEvent`s out, queued text frames in — so the session
/// layer is transport-agnostic. Browsers can't set request headers, so the
/// signed-in viewer's short-lived ticket (when present) rides as `?ticket=`; the secret-based
/// viewer handshake itself is done by the session layer over frames.
#[cfg(target_family = "wasm")]
pub fn start(config: RealtimeConfig, tx: async_channel::Sender<RealtimeEvent>) -> RealtimeHandle {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let stop = Arc::new(AtomicBool::new(false));
    let (outbound_tx, outbound_rx) = async_channel::unbounded::<String>();

    // The signed-in viewer joins with a short-lived ticket, never the access
    // token: the host page asks its own server for one (the session lives in
    // an httpOnly cookie there) and keeps `window.__DRAGENT_TICKET__` fresh,
    // so each (re)connect reads a current one. A ticket works only on
    // /realtime and only for a minute. An anonymous link viewer connects
    // without one, authorised by the session secret.
    let mut url = config.url.clone();
    let ticket = browser_global("__DRAGENT_TICKET__").filter(|t| !t.is_empty());
    if let Some(ticket) = ticket {
        let sep = if url.contains('?') { '&' } else { '?' };
        let ticket: String = url::form_urlencoded::byte_serialize(ticket.as_bytes()).collect();
        url = format!("{url}{sep}ticket={ticket}");
    }

    let ws = match web_sys::WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(e) => {
            let _ = tx.try_send(RealtimeEvent::Error(format!(
                "websocket open failed: {e:?}"
            )));
            return RealtimeHandle {
                outbound: outbound_tx,
                stop,
            };
        }
    };
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

    // onopen → Connected.
    {
        let tx = tx.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let _ = tx.try_send(RealtimeEvent::Connected);
        });
        ws.set_onopen(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
    // onmessage → Message (text frames; the session protocol is JSON text).
    {
        let tx = tx.clone();
        let cb =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                if let Some(text) = e.data().as_string() {
                    let _ = tx.try_send(RealtimeEvent::Message(text));
                }
            });
        ws.set_onmessage(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
    // onerror → Error.
    {
        let tx = tx.clone();
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            let _ = tx.try_send(RealtimeEvent::Error("websocket error".to_string()));
        });
        ws.set_onerror(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
    // onclose → Disconnected.
    {
        let tx = tx.clone();
        let cb = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |_e: web_sys::CloseEvent| {
            let _ = tx.try_send(RealtimeEvent::Disconnected { will_retry: false });
        });
        ws.set_onclose(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }

    // Pump queued outbound frames → the socket. Stops (and closes) on `stop`.
    {
        let ws = ws.clone();
        let stop = stop.clone();
        wasm_bindgen_futures::spawn_local(async move {
            while let Ok(msg) = outbound_rx.recv().await {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if ws.ready_state() == web_sys::WebSocket::OPEN {
                    let _ = ws.send_with_str(&msg);
                }
            }
            let _ = ws.close();
        });
    }

    RealtimeHandle {
        outbound: outbound_tx,
        stop,
    }
}

/// Reconnect loop: connect, serve until the socket drops, back off, repeat —
/// until the stop flag is set.
#[cfg(not(target_family = "wasm"))]
fn run(
    config: RealtimeConfig,
    tx: async_channel::Sender<RealtimeEvent>,
    outbound_rx: async_channel::Receiver<String>,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_MIN;
    while !stop.load(Ordering::Relaxed) {
        let connected = match connect_and_serve(&config, &tx, &outbound_rx, &stop) {
            Ok(connected) => connected,
            Err(err) => {
                log::warn!("[realtime] connection to {} failed: {err}", config.url);
                let _ = tx.send_blocking(RealtimeEvent::Error(err));
                false
            }
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let _ = tx.send_blocking(RealtimeEvent::Disconnected { will_retry: true });
        // A connection that actually came up resets the backoff so a single
        // blip doesn't inflate the delay.
        if connected {
            backoff = BACKOFF_MIN;
        }
        sleep_interruptible(backoff, &stop);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    let _ = tx.send_blocking(RealtimeEvent::Disconnected { will_retry: false });
}

/// One connection attempt + serve loop. Returns `Ok(true)` if the socket opened
/// (so the caller resets backoff), `Ok(false)` if it never connected, `Err` on a
/// connect/auth failure worth surfacing.
#[cfg(not(target_family = "wasm"))]
fn connect_and_serve(
    config: &RealtimeConfig,
    tx: &async_channel::Sender<RealtimeEvent>,
    outbound_rx: &async_channel::Receiver<String>,
    stop: &AtomicBool,
) -> Result<bool, String> {
    // Bearer token — blocking, but we're on a background thread. Auto-refreshes
    // and rotates on 401 inside the API client.
    let token = crate::api::API_CLIENT
        .get_access_token()
        .map_err(|e| format!("auth failed: {e:?}"))?;
    log::info!(
        "[realtime] connecting to {} (bearer {} chars)",
        config.url,
        token.len()
    );

    let mut socket = dial(&config.url, &token)?;
    if stop.load(Ordering::Relaxed) {
        let _ = socket.close(None);
        return Ok(true);
    }
    set_socket_read_timeout(&mut socket, READ_TIMEOUT);
    log::info!("[realtime] connected to {}", config.url);
    let _ = tx.send_blocking(RealtimeEvent::Connected);

    loop {
        // Flush queued outbound frames FIRST — a frame queued right before
        // `stop()` (e.g. the host's EndSession) must still reach the server, so
        // it deletes the session. Flushing before the stop check guarantees that.
        while let Ok(msg) = outbound_rx.try_recv() {
            log::debug!(
                "[realtime] -> {}",
                crate::agent::tools_background::utf8_prefix(&msg, 200)
            );
            if socket.send(tungstenite::Message::Text(msg.into())).is_err() {
                return Ok(true); // socket died mid-send → reconnect
            }
        }

        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            return Ok(true);
        }

        // Read one frame (blocks up to READ_TIMEOUT).
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                log::debug!(
                    "[realtime] <- {}",
                    crate::agent::tools_background::utf8_prefix(&text, 200)
                );
                let _ = tx.send_blocking(RealtimeEvent::Message(text.to_string()));
            }
            // Binary/ping/pong: tungstenite auto-replies to pings; we ignore the
            // rest at the transport layer (protocol keepalive rides Text frames).
            Ok(tungstenite::Message::Binary(_))
            | Ok(tungstenite::Message::Ping(_))
            | Ok(tungstenite::Message::Pong(_))
            | Ok(tungstenite::Message::Frame(_)) => {}
            Ok(tungstenite::Message::Close(_)) => return Ok(true),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Idle read timeout — normal; loop to service sends/stop.
            }
            Err(_) => return Ok(true), // any other error → treat as disconnect
        }
    }
}

/// Dial the gateway with a bounded connect timeout (tungstenite's own
/// `connect` has none and can hang forever), supporting both `wss` and `ws`.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn dial(url: &str, token: &str) -> Result<Socket, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("bad gateway url: {e}"))?;
    let tls = match parsed.scheme() {
        "wss" => true,
        "ws" => false,
        other => return Err(format!("unsupported gateway scheme: {other}")),
    };
    let host = parsed
        .host_str()
        .ok_or_else(|| "gateway url has no host".to_string())?
        .to_string();
    let port = parsed.port().unwrap_or(if tls { 443 } else { 80 });

    use std::net::ToSocketAddrs;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("dns failed: {e}"))?
        .next()
        .ok_or_else(|| "dns returned no address".to_string())?;
    let tcp = std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("tcp connect failed: {e}"))?;
    tcp.set_nodelay(true)
        .map_err(|e| format!("tcp nodelay failed: {e}"))?;
    let _ = tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(HANDSHAKE_TIMEOUT));

    let request = build_request(url, token)?;
    let (socket, _response) = if tls {
        tungstenite::client_tls(request, tcp).map_err(|e| format!("tls ws connect failed: {e}"))?
    } else {
        tungstenite::client(request, tungstenite::stream::MaybeTlsStream::Plain(tcp))
            .map_err(|e| format!("ws connect failed: {e}"))?
    };
    Ok(socket)
}

#[cfg(not(target_family = "wasm"))]
fn build_request(
    url: &str,
    token: &str,
) -> Result<tungstenite::handshake::client::Request, String> {
    use tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("bad request: {e}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .map_err(|_| "invalid auth header".to_string())?,
    );
    Ok(request)
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn set_socket_read_timeout(socket: &mut Socket, dur: Duration) {
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Rustls(s) => {
            let _ = s.get_mut().set_read_timeout(Some(dur));
        }
        tungstenite::stream::MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(dur));
        }
        _ => {}
    }
}

/// Sleep for `dur`, waking early if `stop` is set.
#[cfg(not(target_family = "wasm"))]
fn sleep_interruptible(dur: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

/// The gateway WebSocket URL.
///
/// Native derives it from the configured backend origin. The **web viewer** is
/// served from a different origin than the backend (e.g. page `beta.dragent.dev`,
/// backend `beta.api.dragent.dev`), and the backend origin is an *env* value the
/// browser can't read — so the host page injects the fully-resolved gateway
/// (`window.__DRAGENT_GATEWAY__`, from its `/api/session/config`). A `?gateway=`
/// query param overrides it for local testing.
pub fn gateway_url() -> String {
    #[cfg(target_family = "wasm")]
    if let Some(g) = browser_gateway_override() {
        return g;
    }
    ws_url_from_http(&crate::api::backend_url(), GATEWAY_PATH)
}

/// Read a non-empty string global off `window` (injected by the host page).
#[cfg(target_family = "wasm")]
pub(crate) fn browser_global(name: &str) -> Option<String> {
    let win = web_sys::window()?;
    let val = js_sys::Reflect::get(&win, &wasm_bindgen::JsValue::from_str(name)).ok()?;
    val.as_string().filter(|s| !s.is_empty())
}

#[cfg(target_family = "wasm")]
fn browser_gateway_override() -> Option<String> {
    let win = web_sys::window()?;
    // 1. `?gateway=wss://…` — explicit override (local/dev testing).
    if let Ok(search) = win.location().search() {
        if !search.is_empty() {
            if let Ok(u) = url::Url::parse(&format!("http://x/{search}")) {
                if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "gateway") {
                    let v = v.into_owned();
                    if !v.is_empty() {
                        return Some(v);
                    }
                }
            }
        }
    }
    // 2. `window.__DRAGENT_GATEWAY__` — injected by the host page from its env.
    browser_global("__DRAGENT_GATEWAY__")
}

/// Convert an `http(s)://` origin into a `ws(s)://…{path}` URL.
fn ws_url_from_http(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let swapped = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        base.to_string()
    } else {
        format!("wss://{base}")
    };
    format!("{swapped}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_maps_scheme_and_appends_path() {
        assert_eq!(
            ws_url_from_http("https://abc.awsapprunner.com", "/realtime"),
            "wss://abc.awsapprunner.com/realtime"
        );
        assert_eq!(
            ws_url_from_http("http://localhost:8080/", "/realtime"),
            "ws://localhost:8080/realtime"
        );
        // Already a ws url — scheme preserved.
        assert_eq!(
            ws_url_from_http("wss://host", "/realtime"),
            "wss://host/realtime"
        );
        // Bare host defaults to wss.
        assert_eq!(ws_url_from_http("host:9000", "/x"), "wss://host:9000/x");
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let mut b = BACKOFF_MIN;
        let mut seen = Vec::new();
        for _ in 0..12 {
            seen.push(b);
            b = (b * 2).min(BACKOFF_MAX);
        }
        assert_eq!(seen[0], BACKOFF_MIN);
        assert!(seen.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*seen.last().unwrap(), BACKOFF_MAX);
    }

    #[test]
    fn dial_rejects_non_ws_scheme() {
        let err = dial("http://localhost/realtime", "tok").unwrap_err();
        assert!(err.contains("unsupported gateway scheme"), "got: {err}");
    }
}
