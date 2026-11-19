use std::sync::{Arc, Mutex};

use dragent_pty::PtySessionHandle;

/// Sink for a remote (shared-session mirror) writer — receives the bytes that
/// would go to a local PTY and forwards them to the host's PTY over the session.
pub type RemoteInputSink = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Binding slot for a host-owned PTY session. The tab is created before the
/// host RPCs finish, so early input/resizes buffer here and flush in order
/// once [`HostPtyBinding::bind`] lands. All host I/O runs on ONE background
/// worker per binding, so bytes can never reorder across threads.
pub struct HostPtyBinding {
    tx: std::sync::mpsc::Sender<HostPtyOp>,
    latest_resize: Arc<Mutex<Option<(u16, u16)>>>,
    last_input: Mutex<web_time::Instant>,
    /// The host session this pane is bound to, once bound.
    session: Arc<Mutex<Option<String>>>,
    /// The size this pane last asked for, sent again when the desktop takes
    /// the terminal back from a phone.
    last_size: Mutex<Option<(u16, u16)>>,
}

/// Whether a phone sized this session, so the desktop pane must not send its
/// own size or typing (see `realtime::phone_control`).
fn phone_sized(session: &str) -> bool {
    crate::realtime::phone_control::phone_sized(session)
}

/// Cap for pre-bind buffered input (bootstrap + early typing). Past the cap
/// new bytes are dropped with a warning — the session will still bind.
const MAX_PENDING_OUT: usize = 1 << 20;

enum HostPtyOp {
    SetClient(dragent_host::HostClient),
    Bind {
        workspace_id: String,
        session_id: String,
    },
    Write(Vec<u8>, std::time::Instant),
    WriteConfirmed(Vec<u8>, std::sync::mpsc::Sender<Result<(), String>>),
    Resize,
    Colors(dragent_host::screen_protocol::TerminalColors),
    Unbind,
    AwaitBind,
    Detach,
}

impl HostPtyBinding {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<HostPtyOp>();
        let latest_resize = Arc::new(Mutex::new(None));
        let worker_resize = latest_resize.clone();
        let session = Arc::new(Mutex::new(None));
        let worker_session = session.clone();
        let binding = Self {
            tx,
            latest_resize,
            last_input: Mutex::new(web_time::Instant::now()),
            session,
            last_size: Mutex::new(None),
        };
        std::thread::Builder::new()
            .name("dragent-host-pty".to_string())
            .spawn(move || Self::worker(rx, worker_resize, worker_session))
            .expect("host PTY worker thread");
        binding
    }

    pub fn set_client(&self, client: dragent_host::HostClient) { self.op(HostPtyOp::SetClient(client)); }

    /// Queue bootstrap bytes ahead of everything (shell-hook source line).
    pub fn preload(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self
                .tx
                .send(HostPtyOp::Write(bytes.to_vec(), std::time::Instant::now()));
        }
    }

    /// Attach the live host session; flushes buffered input + resize in order.
    pub fn bind(&self, workspace_id: &str, session_id: &str) {
        let _ = self.tx.send(HostPtyOp::Bind {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
        });
    }

    /// Stop the worker. The host session itself is untouched — explicit tab
    /// close owns `close_terminal`; app quit must leave sessions running.
    pub fn detach(&self) {
        self.op(HostPtyOp::Detach);
    }

    /// Forget an unavailable session but keep the worker for a later bind.
    /// Input and resizes are dropped until then, so nothing typed into a
    /// dead pane can reach the session that replaces it.
    pub fn unbind(&self) {
        self.latest_resize.lock().unwrap().take();
        self.op(HostPtyOp::Unbind);
    }

    /// A replacement session is being created for this pane after an `unbind`:
    /// from here on, input is held for it again instead of being dropped.
    pub fn await_bind(&self) {
        self.op(HostPtyOp::AwaitBind);
    }

    fn op(&self, op: HostPtyOp) {
        let _ = self.tx.send(op);
    }

    /// The colours this terminal answers queries with. The latest are kept and
    /// sent again on every bind, since a new session starts without them.
    fn set_colors(&self, colors: dragent_host::screen_protocol::TerminalColors) {
        self.op(HostPtyOp::Colors(colors));
    }

    fn resize(&self, cols: u16, rows: u16) {
        log::debug!("[resize-debug] host resize requested cols={cols} rows={rows} bt={}", std::backtrace::Backtrace::force_capture().to_string().lines().filter(|l| l.contains("dragent::")).take(8).collect::<Vec<_>>().join(" | "));
        *self.last_size.lock().unwrap() = Some((cols, rows));
        let mut pending = self.latest_resize.lock().unwrap();
        // One queued wakeup carries the latest dimensions, including while
        // the worker is waiting for an earlier host RPC.
        if pending.replace((cols, rows)).is_none() {
            let _ = self.tx.send(HostPtyOp::Resize);
        }
    }

    fn worker(
        rx: std::sync::mpsc::Receiver<HostPtyOp>,
        latest_resize: Arc<Mutex<Option<(u16, u16)>>>,
        bound_session: Arc<Mutex<Option<String>>>,
    ) {
        let mut binding: Option<(String, String)> = None;
        // False only between `unbind` and the next `bind`.
        let mut accepting = true;
        let mut pending_out: Vec<u8> = Vec::new();
        let mut pending_resize: Option<(u16, u16)> = None;
        let mut client: Option<dragent_host::HostClient> = None;
        let mut authority: Option<dragent_host::HostClient> = None;
        // A client is dropped after a failed write; the next one is the host
        // as it is now, since a local host may have restarted under a new
        // endpoint the bound client doesn't know.
        let ensure_client = |authority: &Option<dragent_host::HostClient>, client: &mut Option<dragent_host::HostClient>| -> bool {
            if client.is_none() {
                *client = match authority {
                    Some(bound) => bound.reconnected().ok(),
                    None => dragent_host::HostClient::ensure_host().ok(),
                };
            }
            client.is_some()
        };
        // The host no longer has the bound terminal (its PTY service lost it).
        // Writing on would fail the same way for every key and focus report,
        // and reconnect each time; until the tab binds again, input is dropped.
        let mut terminal_gone = false;
        let mut colors: Option<dragent_host::screen_protocol::TerminalColors> = None;
        let mut colors_unsent = false;
        let send_colors = |authority: &Option<dragent_host::HostClient>,
                           client: &mut Option<dragent_host::HostClient>,
                           binding: &Option<(String, String)>,
                           colors: &Option<dragent_host::screen_protocol::TerminalColors>,
                           unsent: &mut bool| {
            if !*unsent {
                return;
            }
            let (Some((workspace_id, session_id)), Some(colors)) = (binding, colors) else {
                return;
            };
            if !ensure_client(authority, client) {
                return;
            }
            let result = client.as_ref().expect("ensured above").set_terminal_colors(
                workspace_id,
                session_id,
                *colors,
            );
            match result {
                Ok(()) => *unsent = false,
                Err(error) => {
                    log::warn!("[host-pty] colours failed: {error}");
                    *client = None;
                }
            }
        };
        let mut deferred_op = None;
        loop {
            let retry_pending = binding.is_some()
                && (!pending_out.is_empty() || pending_resize.is_some() || colors_unsent);
            let op = if deferred_op.is_some() {
                deferred_op.take()
            } else if retry_pending {
                match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(op) => Some(op),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(op) => Some(op),
                    Err(_) => break,
                }
            };
            if matches!(op, Some(HostPtyOp::Detach)) {
                break;
            }
            if matches!(op, Some(HostPtyOp::Resize)) {
                if let Some(size) = latest_resize.lock().unwrap().take() {
                    pending_resize = Some(size);
                }
            }
            if !pending_out.is_empty() {
                if let Some((workspace_id, session_id)) = &binding {
                    if ensure_client(&authority, &mut client) {
                        let result = client.as_ref().expect("ensured above").send_terminal_input(
                            workspace_id,
                            session_id,
                            &pending_out,
                        );
                        match result {
                            Ok(()) => pending_out.clear(),
                            Err(error) => {
                                log::warn!("[host-pty] pending flush failed: {error}");
                                client = None;
                            }
                        }
                    }
                }
            }
            if pending_out.is_empty() {
                // A phone-sized terminal keeps the phone's size: the pane's
                // own size waits here until the desktop takes it back.
                if let (Some((workspace_id, session_id)), Some((cols, rows))) =
                    (&binding, pending_resize)
                {
                    if phone_sized(session_id) {
                    } else if ensure_client(&authority, &mut client) {
                        if client
                            .as_ref()
                            .expect("ensured above")
                            .resize_terminal(workspace_id, session_id, cols, rows)
                            .is_ok()
                        {
                            pending_resize = None;
                        } else {
                            client = None;
                        }
                    }
                }
            }
            send_colors(&authority, &mut client, &binding, &colors, &mut colors_unsent);
            let Some(op) = op else { continue };
            match op {
                HostPtyOp::AwaitBind => accepting = true,
                HostPtyOp::Colors(next) => {
                    colors = Some(next);
                    colors_unsent = true;
                    send_colors(&authority, &mut client, &binding, &colors, &mut colors_unsent);
                }
                HostPtyOp::SetClient(next) => { authority = Some(next.clone()); client = Some(next); }
                HostPtyOp::Bind {
                    workspace_id,
                    session_id,
                } => {
                    accepting = true;
                    binding = Some((workspace_id.clone(), session_id.clone()));
                    *bound_session.lock().unwrap() = Some(session_id.clone());
                    terminal_gone = false;
                    colors_unsent = true;
                    send_colors(&authority, &mut client, &binding, &colors, &mut colors_unsent);
                    if !pending_out.is_empty() && ensure_client(&authority, &mut client) {
                        let result = client.as_ref().expect("ensured above").send_terminal_input(
                            &workspace_id,
                            &session_id,
                            &pending_out,
                        );
                        if let Err(e) = result {
                            log::warn!("[host-pty] bind flush failed: {e}");
                            client = None;
                        } else {
                            pending_out.clear();
                        }
                    }
                    if let Some((cols, rows)) = pending_resize.take() {
                        if phone_sized(&session_id) {
                            pending_resize = Some((cols, rows));
                        } else if ensure_client(&authority, &mut client) {
                            if client
                                .as_ref()
                                .expect("ensured above")
                                .resize_terminal(&workspace_id, &session_id, cols, rows)
                                .is_err()
                            {
                                client = None;
                                pending_resize = Some((cols, rows));
                            }
                        } else {
                            pending_resize = Some((cols, rows));
                        }
                    }
                }
                HostPtyOp::WriteConfirmed(bytes, reply) => {
                    let result = if !accepting || !pending_out.is_empty() {
                        Err("Agent terminal is not ready. Try again after it reconnects.".into())
                    } else if let Some((workspace_id, session_id)) = &binding {
                        if ensure_client(&authority, &mut client) {
                            client.as_ref().unwrap().send_terminal_input(workspace_id, session_id, &bytes)
                        } else { Err("Couldn't connect to the terminal host".into()) }
                    } else { Err("Agent terminal is not connected".into()) };
                    let _ = reply.send(result);
                }
                HostPtyOp::Write(_, _) if !accepting || terminal_gone => {}
                HostPtyOp::Write(mut bytes, queued_at) => {
                    crate::terminal::performance::HOST_INPUT_QUEUE.record(queued_at.elapsed());
                    // Drain only already-queued, adjacent input. A scroll burst
                    // needs one host round trip, not one per mouse report. Never
                    // cross a bind/resize/detach boundary or wait to fill a batch.
                    while bytes.len() < 64 * 1024 {
                        match rx.try_recv() {
                            Ok(HostPtyOp::Write(next, _))
                                if bytes.len() + next.len() <= 64 * 1024 =>
                            {
                                bytes.extend_from_slice(&next);
                            }
                            Ok(next) => {
                                deferred_op = Some(next);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    if bytes.is_empty() {
                        continue;
                    }
                    if !pending_out.is_empty() {
                        if pending_out.len() + bytes.len() <= MAX_PENDING_OUT {
                            pending_out.extend_from_slice(&bytes);
                        } else {
                            log::warn!("[host-pty] dropping pending input (over cap)");
                        }
                        continue;
                    }
                    match &binding {
                        Some((workspace_id, session_id)) => {
                            if ensure_client(&authority, &mut client) {
                                let started = std::time::Instant::now();
                                let result = client
                                    .as_ref()
                                    .expect("ensured above")
                                    .send_terminal_input(workspace_id, session_id, &bytes);
                                crate::terminal::performance::HOST_INPUT_RPC
                                    .record(started.elapsed());
                                if let Err(e) = result {
                                    if terminal_is_gone(&e) {
                                        // The connection is fine: keep it.
                                        log::warn!(
                                            "[host-pty] {e}: dropping input until the tab reconnects"
                                        );
                                        terminal_gone = true;
                                    } else {
                                        log::warn!("[host-pty] write failed: {e}");
                                        client = None;
                                    }
                                }
                            } else if bytes.len() <= MAX_PENDING_OUT {
                                pending_out.extend_from_slice(&bytes);
                            }
                        }
                        None => {
                            if pending_out.len() + bytes.len() <= MAX_PENDING_OUT {
                                pending_out.extend_from_slice(&bytes);
                            } else {
                                log::warn!("[host-pty] dropping pre-bind input (over cap)");
                            }
                        }
                    }
                }
                HostPtyOp::Resize => {}
                HostPtyOp::Unbind => {
                    binding = None;
                    bound_session.lock().unwrap().take();
                    accepting = false;
                    pending_out.clear();
                    pending_resize = None;
                }
                HostPtyOp::Detach => break,
            }
        }
    }
}

/// The host answered that the terminal doesn't exist (any more), as opposed
/// to a connection or transport failure worth retrying.
fn terminal_is_gone(error: &str) -> bool {
    error.contains("unknown terminal")
}

#[cfg(test)]
mod gone_tests {
    #[test]
    fn only_a_missing_terminal_stops_the_writes() {
        assert!(super::terminal_is_gone("unknown terminal term-cb5fb203"));
        assert!(!super::terminal_is_gone("connection refused"));
        assert!(!super::terminal_is_gone("Terminal host disconnected"));
    }
}

impl Default for HostPtyBinding {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialized write path for a terminal session. Normally this is the local
/// interactive PTY; for a shared-session *mirror* (remote mode) it forwards
/// every write — typed commands AND raw alt-screen keystrokes — to the host
/// instead of a local shell, so control works transparently through the same
/// call sites that drive a local terminal.
#[derive(Clone)]
pub struct PtyWriter {
    inner: WriterInner,
}

#[derive(Clone)]
enum WriterInner {
    Local(PtySessionHandle),
    Remote(RemoteInputSink),
    Host(Arc<HostPtyBinding>),
}

impl PtyWriter {
    pub fn new(handle: PtySessionHandle) -> Self {
        Self {
            inner: WriterInner::Local(handle),
        }
    }

    /// A writer whose input is forwarded to the host of a shared session.
    pub fn remote(sink: RemoteInputSink) -> Self {
        Self {
            inner: WriterInner::Remote(sink),
        }
    }

    /// A writer for a host-OWNED tab session: input/resize route to the
    /// daemon's PTY through the binding slot (buffered pre-bind, ordered).
    pub fn host(binding: Arc<HostPtyBinding>) -> Self {
        Self {
            inner: WriterInner::Host(binding),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.inner, WriterInner::Remote(_) | WriterInner::Host(_))
    }

    pub fn is_host(&self) -> bool {
        matches!(self.inner, WriterInner::Host(_))
    }

    /// Whether a process group other than the interactive shell currently owns
    /// this local terminal. This is the source of truth when UI shell-integration
    /// state is unavailable or briefly stale.
    pub fn foreground_running(&self) -> bool {
        match &self.inner {
            WriterInner::Local(handle) => handle.foreground_running(),
            WriterInner::Remote(_) | WriterInner::Host(_) => false,
        }
    }

    /// Called off the UI thread; host writes acknowledge delivery to the PTY.
    pub(crate) fn write_confirmed(&self, bytes: &[u8]) -> Result<(), String> {
        match &self.inner {
            WriterInner::Local(handle) => handle.write(bytes).map_err(|error| error.to_string()),
            WriterInner::Remote(_) => Err("Comment delivery requires a terminal on the workspace's machine".into()),
            WriterInner::Host(binding) => {
                let (tx, rx) = std::sync::mpsc::channel();
                binding.tx.send(HostPtyOp::WriteConfirmed(bytes.to_vec(), tx)).map_err(|_| "Terminal host disconnected".to_string())?;
                rx.recv().map_err(|_| "Terminal host did not acknowledge the comment".to_string())?
            }
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        match &self.inner {
            WriterInner::Local(handle) => {
                if let Err(e) = handle.write(bytes) {
                    log::error!("PTY write failed: {e}");
                }
            }
            WriterInner::Remote(sink) => sink(bytes),
            WriterInner::Host(binding) => {
                // Covered on the desktop while a phone has it: nothing typed
                // blind here may reach the terminal.
                if binding.session.lock().unwrap().as_deref().is_some_and(phone_sized) {
                    return;
                }
                *binding.last_input.lock().unwrap() = web_time::Instant::now();
                binding.op(HostPtyOp::Write(bytes.to_vec(), std::time::Instant::now()));
            }
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        match &self.inner {
            WriterInner::Local(handle) => {
                if let Err(e) = handle.resize(rows, cols) {
                    log::error!("PTY resize failed: {e}");
                }
            }
            // Remote: the host owns the PTY size; sizing is negotiated separately.
            WriterInner::Remote(_) => {}
            WriterInner::Host(binding) => {
                binding.resize(cols, rows);
            }
        }
    }

    /// The host session a daemon-owned terminal is bound to, once bound.
    pub fn host_session_id(&self) -> Option<String> {
        match &self.inner {
            WriterInner::Host(binding) => binding.session.lock().unwrap().clone(),
            WriterInner::Local(_) | WriterInner::Remote(_) => None,
        }
    }

    /// Send this pane's size to a daemon-owned terminal again (the desktop
    /// took it back from a phone that had resized it).
    pub fn reassert_size(&self) {
        if let WriterInner::Host(binding) = &self.inner {
            let size = *binding.last_size.lock().unwrap();
            if let Some((cols, rows)) = size {
                binding.resize(cols, rows);
            }
        }
    }

    /// How long since anything was sent to a daemon-owned terminal, or `None`
    /// for a terminal that doesn't track it.
    pub fn input_quiet_for(&self) -> Option<std::time::Duration> {
        match &self.inner {
            WriterInner::Host(binding) => Some(binding.last_input.lock().unwrap().elapsed()),
            WriterInner::Local(_) | WriterInner::Remote(_) => None,
        }
    }

    /// Tell a daemon-owned terminal its colours, which it then answers queries
    /// with. Returns false for a terminal that answers from this process.
    pub fn set_terminal_colors(
        &self,
        colors: dragent_host::screen_protocol::TerminalColors,
    ) -> bool {
        match &self.inner {
            WriterInner::Host(binding) => {
                binding.set_colors(colors);
                true
            }
            WriterInner::Local(_) | WriterInner::Remote(_) => false,
        }
    }

    /// Send SIGINT (Ctrl+C) to a local foreground process group.
    ///
    /// A shared-session mirror sends the 0x03 byte through [`Self::write`]. The
    /// host recognizes that byte and performs this local signal itself, avoiding
    /// a second remote input frame and preserving the host's process-group
    /// semantics. Host-owned tabs behave identically (same daemon semantics).
    pub fn interrupt(&self) {
        match &self.inner {
            WriterInner::Local(handle) => {
                let _ = handle.interrupt();
            }
            WriterInner::Remote(_) => {}
            // Callers already wrote Ctrl-C. A second byte can exit a CLI agent.
            WriterInner::Host(_) => {},
        }
    }

    pub fn shutdown(&self) {
        match &self.inner {
            WriterInner::Local(handle) => {
                let _ = handle.shutdown();
            }
            // Mirror detach: the host run keeps going headless.
            WriterInner::Remote(_) => {}
            // Host-owned tab teardown: detach only. The session itself is
            // closed explicitly on tab close; app quit must leave it running.
            WriterInner::Host(binding) => binding.detach(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::PtyWriter;

    #[test]
    fn remote_interrupt_does_not_emit_a_duplicate_ctrl_c_byte() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink_received = received.clone();
        let writer = PtyWriter::remote(Arc::new(move |bytes| {
            sink_received.lock().unwrap().push(bytes.to_vec());
        }));

        writer.write(&[0x03]);
        writer.interrupt();

        assert_eq!(*received.lock().unwrap(), vec![vec![0x03]]);
        assert!(!writer.foreground_running());
    }
}
