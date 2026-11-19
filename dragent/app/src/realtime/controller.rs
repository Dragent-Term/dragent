//! Session controller — the client's shared-session brain.
//!
//! Ties the realtime [transport](super) to the pure [`SessionCore`] state
//! machine: it feeds inbound transport events into the core, sends the core's
//! outbound frames over the [`RealtimeHandle`], and hands the UI a stream of
//! [`SessionEvent`]s. A thin warpui Entity drives this by forwarding the
//! transport's `async_channel` into [`on_event`](SessionController::on_event)
//! via `spawn_stream_local`; the controller itself stays free of warpui so it
//! is unit-testable.

use base64::Engine as _;
use uuid::Uuid;

use dragent_session_protocol::{
    AccessLevel, Message, ParticipantId, SessionId, SessionSecret, TerminalDelta,
};

use super::session::{Effects, SessionCore, SessionEvent, SessionState};
use super::{RealtimeConfig, RealtimeEvent, RealtimeHandle};

use dragent_session_protocol as proto;

/// Largest frame we will hand to the transport. The gateway sets an 8 MiB
/// WebSocket read limit; exceeding it drops the host connection and ends the
/// session for viewers. Frame producers already bound their output (raw output
/// is chunked; styled snapshots are line-capped), so this is a final safety
/// valve: a pathologically wide snapshot is dropped with a warning rather than
/// killing the whole session — the next periodic snapshot recovers it.
const MAX_FRAME_BYTES: usize = 6 * 1024 * 1024;

pub struct SessionController {
    core: SessionCore,
    handle: RealtimeHandle,
}

impl SessionController {
    pub fn new(core: SessionCore, handle: RealtimeHandle) -> Self {
        Self { core, handle }
    }

    /// Start hosting: open the transport and prepare a host session. Inbound
    /// transport events arrive on `events_tx` and must be fed to [`on_event`].
    pub fn start_host(
        device_id: impl Into<String>,
        tab_title: impl Into<String>,
        events_tx: async_channel::Sender<RealtimeEvent>,
    ) -> Self {
        let handle = super::start(RealtimeConfig::gateway(), events_tx);
        Self::new(SessionCore::new_host(device_id, tab_title), handle)
    }

    /// Start viewing with credentials from a claimed pairing code or resolved
    /// share link.
    pub fn start_viewer(
        device_id: impl Into<String>,
        session_id: &str,
        secret: &str,
        display_name: impl Into<String>,
        events_tx: async_channel::Sender<RealtimeEvent>,
    ) -> Result<Self, uuid::Error> {
        let session = SessionId(Uuid::parse_str(session_id)?);
        let core =
            SessionCore::new_viewer(device_id, session, SessionSecret::new(secret), display_name);
        let handle = super::start(RealtimeConfig::gateway(), events_tx);
        Ok(Self::new(core, handle))
    }

    pub fn state(&self) -> &SessionState {
        self.core.state()
    }

    pub fn is_host(&self) -> bool {
        self.core.is_host()
    }

    pub fn transport_handle(&self) -> RealtimeHandle {
        self.handle.clone()
    }

    /// Viewer: authenticated as the host's own account (may drive host size).
    pub fn viewer_is_host_account(&self) -> bool {
        self.core.viewer_is_host_account()
    }

    /// Drive the controller from one realtime transport event, returning the UI
    /// events to surface.
    pub fn on_event(&mut self, ev: RealtimeEvent) -> Vec<SessionEvent> {
        match ev {
            RealtimeEvent::Connected => {
                // (Re)send the handshake, and tell the UI we're online.
                let fx = self.core.on_connected();
                let mut out = vec![SessionEvent::Connectivity { online: true }];
                out.extend(self.drain(fx));
                out
            }
            RealtimeEvent::Message(json) => {
                let fx = self.core.on_message(&json);
                self.drain(fx)
            }
            RealtimeEvent::Disconnected { .. } => {
                vec![SessionEvent::Connectivity { online: false }]
            }
            RealtimeEvent::Error(e) => vec![SessionEvent::Error(e)],
        }
    }

    /// Send the core's outbound frames and return its UI events.
    fn drain(&self, fx: Effects) -> Vec<SessionEvent> {
        for frame in fx.outbound {
            if frame.len() > MAX_FRAME_BYTES {
                log::warn!(
                    "[share] dropping {}-byte frame over the {}-byte cap to keep the session alive",
                    frame.len(),
                    MAX_FRAME_BYTES
                );
                continue;
            }
            self.handle.send(frame);
        }
        fx.events
    }

    // ── Host actions ─────────────────────────────────────────────────────────

    pub fn broadcast_delta(&self, delta: TerminalDelta) {
        let fx = self.core.broadcast(delta);
        self.drain(fx);
    }

    pub fn resolve_command(&self, request_id: Uuid, approved: bool) {
        let fx = self.core.resolve_command(request_id, approved);
        self.drain(fx);
    }

    pub fn set_participant_role(&self, participant: ParticipantId, access: AccessLevel) {
        let fx = self.core.set_participant_role(participant, access);
        self.drain(fx);
    }

    pub fn deny_access_request(&self, participant: ParticipantId) {
        let fx = self.core.deny_access_request(participant);
        self.drain(fx);
    }

    pub fn remove_participant(&self, participant: ParticipantId) {
        let fx = self.core.remove_participant(participant);
        self.drain(fx);
    }

    pub fn create_forward(&self, port: u16, label: impl Into<String>) {
        let fx = self.core.create_forward(port, label);
        self.drain(fx);
    }

    pub fn close_forward(&self, forward_id: impl Into<String>) {
        let fx = self.core.close_forward(forward_id);
        self.drain(fx);
    }

    pub fn forward_response(
        &self,
        request_id: impl Into<String>,
        status: u16,
        headers: Vec<(String, String)>,
        body: &[u8],
    ) {
        let fx = self
            .core
            .forward_response(request_id, status, headers, body);
        self.drain(fx);
    }

    pub fn forward_websocket_opened(&self, request_id: impl Into<String>) {
        self.handle.send(
            proto::HostToServer::ForwardWebSocketOpened {
                request_id: request_id.into(),
            }
            .to_json()
            .unwrap_or_default(),
        );
    }

    pub fn forward_websocket_message(
        &self,
        request_id: impl Into<String>,
        message_type: impl Into<String>,
        data: &[u8],
    ) {
        self.handle.send(
            proto::HostToServer::ForwardWebSocketMessage {
                request_id: request_id.into(),
                message_type: message_type.into(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(data),
            }
            .to_json()
            .unwrap_or_default(),
        );
    }

    pub fn forward_websocket_closed(&self, request_id: impl Into<String>) {
        self.handle.send(
            proto::HostToServer::ForwardWebSocketClosed {
                request_id: request_id.into(),
            }
            .to_json()
            .unwrap_or_default(),
        );
    }

    // ── Viewer actions ───────────────────────────────────────────────────────

    pub fn request_control(&self) {
        let fx = self.core.request_control();
        self.drain(fx);
    }

    pub fn execute_command(&self, command: impl Into<String>) -> Vec<SessionEvent> {
        let fx = self.core.execute_command(command);
        self.drain(fx)
    }

    /// Forward raw input bytes to the host's PTY (typed keys + alt-screen).
    pub fn send_input(&self, bytes: &[u8]) -> Vec<SessionEvent> {
        let fx = self.core.send_input(bytes);
        self.drain(fx)
    }

    /// Ask the host to resize its shared PTY to this viewer's grid.
    pub fn send_resize(&self, rows: u16, cols: u16) -> Vec<SessionEvent> {
        let fx = self.core.send_resize(rows, cols);
        self.drain(fx)
    }

    /// Push this controller's live input/scroll surface up to the host.
    pub fn send_control(&self, surface: dragent_session_protocol::ControlSurface) {
        let fx = self.core.send_control(surface);
        self.drain(fx);
    }

    pub fn send_pointer(&self, position: dragent_session_protocol::PointerPosition) {
        let fx = self.core.send_pointer(position);
        self.drain(fx);
    }

    pub fn submit_agent_query(&self, request_id: Uuid, query: impl Into<String>) {
        let fx = self.core.submit_agent_query(request_id, query);
        self.drain(fx);
    }

    pub fn stop_agent(&self) {
        let fx = self.core.stop_agent();
        self.drain(fx);
    }

    pub fn request_forward(
        &self,
        forward_id: impl Into<String>,
        request_id: impl Into<String>,
        method: impl Into<String>,
        path: impl Into<String>,
        headers: Vec<(String, String)>,
        body: &[u8],
    ) {
        let fx = self
            .core
            .request_forward(forward_id, request_id, method, path, headers, body);
        self.drain(fx);
    }

    // ── Lifecycle ────────────────────────────────────────────────────────────

    /// End the session and stop the transport. `reason`, when set, is forwarded
    /// to viewers as the end message (e.g. inactivity auto-end).
    pub fn end(&mut self, reason: Option<String>) -> Vec<SessionEvent> {
        let fx = self.core.end(reason);
        let events = self.drain(fx);
        self.handle.stop();
        events
    }

    /// Stop the transport without sending an explicit end (e.g. on sign-out).
    pub fn stop(&self) {
        self.handle.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_session_protocol::{self as proto, Message, ReconnectToken, Role};

    #[test]
    fn host_connect_sends_initialize_then_goes_live() {
        let (handle, outbound) = RealtimeHandle::for_test();
        let mut ctrl = SessionController::new(SessionCore::new_host("dev-1", "zsh"), handle);

        let events = ctrl.on_event(RealtimeEvent::Connected);
        assert!(events.contains(&SessionEvent::Connectivity { online: true }));
        let sent = outbound.try_recv().expect("initialize queued");
        assert!(matches!(
            proto::HostToServer::from_json(&sent).unwrap(),
            proto::HostToServer::Initialize { .. }
        ));

        let init = proto::ServerToHost::Initialized {
            session: SessionId::new(),
            pairing_code: Some("ABCD-2K7Q".into()),
            reconnect_token: ReconnectToken::new("rt"),
        }
        .to_json()
        .unwrap();
        let events = ctrl.on_event(RealtimeEvent::Message(init));
        assert!(events.contains(&SessionEvent::StateChanged));
        assert!(matches!(ctrl.state(), SessionState::HostLive { .. }));
    }

    #[test]
    fn connectivity_and_error_events() {
        let (handle, _out) = RealtimeHandle::for_test();
        let mut ctrl = SessionController::new(SessionCore::new_host("d", "t"), handle);
        assert_eq!(
            ctrl.on_event(RealtimeEvent::Disconnected { will_retry: true }),
            vec![SessionEvent::Connectivity { online: false }]
        );
        assert_eq!(
            ctrl.on_event(RealtimeEvent::Error("boom".into())),
            vec![SessionEvent::Error("boom".into())]
        );
    }

    #[test]
    fn host_broadcast_sends_delta_frame() {
        let (handle, outbound) = RealtimeHandle::for_test();
        let ctrl = SessionController::new(SessionCore::new_host("d", "t"), handle);
        ctrl.broadcast_delta(TerminalDelta::OutputChunk {
            block_id: "b1".into(),
            seq: 1,
            data_base64: "aGk=".into(),
        });
        let sent = outbound.try_recv().expect("delta queued");
        assert!(matches!(
            proto::HostToServer::from_json(&sent).unwrap(),
            proto::HostToServer::Delta { .. }
        ));
    }

    #[test]
    fn viewer_execute_without_control_errs_and_sends_nothing() {
        let (handle, outbound) = RealtimeHandle::for_test();
        let mut ctrl = SessionController::new(
            SessionCore::new_viewer("d", SessionId::new(), SessionSecret::new("s"), "V"),
            handle,
        );
        let joined = proto::ServerToViewer::Joined {
            participant: ParticipantId::new(),
            role: Role::Viewer,
            is_host_account: false,
        }
        .to_json()
        .unwrap();
        ctrl.on_event(RealtimeEvent::Message(joined));

        let events = ctrl.execute_command("rm -rf /");
        assert!(matches!(events.as_slice(), [SessionEvent::Error(_)]));
        assert!(outbound.try_recv().is_err(), "nothing should be sent");
    }
}
