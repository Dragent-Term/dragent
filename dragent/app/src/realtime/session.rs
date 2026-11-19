//! Shared-session state machine (client side).
//!
//! [`SessionCore`] is a **pure** state machine: it consumes inbound protocol
//! messages (JSON text from the realtime transport) and produces (a) outbound
//! frames to send and (b) UI-facing [`SessionEvent`]s — with no sockets, threads,
//! or warpui in the way, so it is fully unit-testable. The thin wrapper that
//! drives it with a [`super::RealtimeHandle`] and delivers events to the owning
//! Entity is layered on top.
//!
//! One core instance represents this client's participation in one session,
//! either as the **host** (owns the PTY, broadcasts output, approves commands)
//! or as a **viewer/controller** (receives output, may request/run commands).

use base64::Engine as _;
use uuid::Uuid;

use dragent_session_protocol as proto;
use proto::Message;

/// UI-facing snapshot of the session.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    /// Handshake in flight (waiting for the server to confirm).
    Connecting,
    /// Hosting a live session.
    HostLive {
        session: proto::SessionId,
        /// Own-device pairing code, if the server issued one.
        pairing_code: Option<String>,
        participants: Vec<proto::Participant>,
    },
    /// Joined a session as a viewer/controller.
    ViewerLive {
        session: proto::SessionId,
        self_role: proto::Role,
        participants: Vec<proto::Participant>,
    },
    /// The session is over.
    Ended { reason: String },
}

/// Something the UI should react to.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The realtime socket connected (`true`) or dropped (`false`, reconnecting).
    Connectivity {
        online: bool,
    },
    /// [`SessionCore::state`] changed; re-read it.
    StateChanged,
    /// The participant list changed.
    ParticipantsChanged,
    /// Viewer: an ordered terminal change to apply to the local mirror.
    Delta(proto::TerminalDelta),
    /// Host: a controller wants to run a command; approve via
    /// [`SessionCore::resolve_command`].
    CommandApprovalRequested {
        request_id: Uuid,
        from: proto::ParticipantId,
        command: String,
    },
    /// Host: a command from the host's OWN account (e.g. the user's other
    /// device) — pre-approved by the server, run it immediately with no prompt.
    RunCommand {
        request_id: Uuid,
        command: String,
    },
    /// Host: run a controller-submitted prompt through the shared pane's local
    /// agent controller.
    RunAgentQuery {
        request_id: Uuid,
        from: proto::ParticipantId,
        query: String,
    },
    /// Host: stop the shared pane's current agent turn.
    StopAgent,
    /// Host: raw input bytes from a controller — write them straight to the
    /// shared pane's PTY (covers typed keys and alt-screen keystrokes).
    RunInput {
        bytes: Vec<u8>,
    },
    /// Host: a controller asked to resize the shared PTY to `rows`×`cols` so a
    /// TUI re-renders to fill the viewer's window.
    ResizeRequested {
        rows: u16,
        cols: u16,
    },
    /// Host: a controller's live input/scroll surface to reflect on the host's
    /// own command editor / AI-tool input / scroll position.
    ControlReceived {
        from: proto::ParticipantId,
        surface: proto::ControlSurface,
    },
    /// Host: any participant moved their pointer in the shared window.
    PointerMoved {
        from: proto::ParticipantId,
        position: proto::PointerPosition,
    },
    /// Host: a view-only participant asked to be upgraded to Edit; prompt the
    /// host to approve (grant control) or decline.
    AccessRequested {
        participant: proto::ParticipantId,
        display_name: String,
    },
    /// Viewer: this participant's Edit request was declined by the host.
    AccessDenied,
    /// Host: a forward was created and can be shown in the share UI.
    ForwardCreated {
        forward_id: String,
        port: u16,
        public_url: String,
    },
    /// Host: satisfy this request by dialing the approved loopback port and
    /// replying with [`SessionCore::forward_response`].
    ForwardRequest {
        forward_id: String,
        port: u16,
        request_id: String,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Host-side lifecycle/data for a raw browser-forward stream. It carries
    /// ordinary HTTP, streamed responses, and WebSocket upgrades alike.
    ForwardWebSocketOpen {
        port: u16,
        method: String,
        path: String,
        request_id: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    ForwardWebSocketMessage {
        request_id: String,
        message_type: String,
        data: Vec<u8>,
    },
    ForwardWebSocketClose {
        request_id: String,
    },
    /// The session ended.
    Ended(String),
    /// A non-fatal problem (decode failure, illegal action).
    Error(String),
}

/// The result of feeding the core an input: frames to send + events to emit.
#[derive(Debug, Default, PartialEq)]
pub struct Effects {
    /// JSON frames to hand to the transport, in order.
    pub outbound: Vec<String>,
    /// Events to deliver to the UI, in order.
    pub events: Vec<SessionEvent>,
}

impl Effects {
    fn send<M: Message>(&mut self, msg: &M) {
        match msg.to_json() {
            Ok(json) => self.outbound.push(json),
            Err(e) => self
                .events
                .push(SessionEvent::Error(format!("encode: {e}"))),
        }
    }
    fn emit(&mut self, event: SessionEvent) {
        self.events.push(event);
    }
}

/// Role-specific identity kept across reconnects.
enum Side {
    Host {
        tab_title: String,
        session: Option<proto::SessionId>,
        reconnect_token: Option<proto::ReconnectToken>,
    },
    Viewer {
        session: proto::SessionId,
        secret: proto::SessionSecret,
        display_name: String,
        self_role: proto::Role,
        /// Set from the server's `Joined`: this viewer authenticated as the host's
        /// own account, so it may drive the host's terminal size.
        is_host_account: bool,
    },
}

/// The client's participation in one shared session.
pub struct SessionCore {
    device_id: String,
    side: Side,
    state: SessionState,
}

impl SessionCore {
    /// Start hosting. Call [`on_connected`](Self::on_connected) once the socket
    /// is up to send the handshake.
    pub fn new_host(device_id: impl Into<String>, tab_title: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            side: Side::Host {
                tab_title: tab_title.into(),
                session: None,
                reconnect_token: None,
            },
            state: SessionState::Connecting,
        }
    }

    /// Join an existing session as a viewer.
    pub fn new_viewer(
        device_id: impl Into<String>,
        session: proto::SessionId,
        secret: proto::SessionSecret,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            side: Side::Viewer {
                session,
                secret,
                display_name: display_name.into(),
                self_role: proto::Role::Viewer,
                is_host_account: false,
            },
            state: SessionState::Connecting,
        }
    }

    /// Viewer: true when it authenticated as the host's own account (may drive the
    /// host's terminal size). Always false for a host or before `Joined`.
    pub fn viewer_is_host_account(&self) -> bool {
        matches!(
            &self.side,
            Side::Viewer {
                is_host_account: true,
                ..
            }
        )
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn is_host(&self) -> bool {
        matches!(self.side, Side::Host { .. })
    }

    /// The socket (re)connected: (re)send the initial handshake. On the first
    /// connect a host sends `Initialize`; after it has a session id, a reconnect
    /// resumes with `Reconnect`. A viewer always (re)sends `Join`.
    pub fn on_connected(&mut self) -> Effects {
        let mut fx = Effects::default();
        match &self.side {
            Side::Host {
                tab_title,
                session,
                reconnect_token,
            } => match (session, reconnect_token) {
                (Some(session), Some(token)) => fx.send(&proto::HostToServer::Reconnect {
                    session: *session,
                    token: token.clone(),
                }),
                _ => fx.send(&proto::HostToServer::Initialize {
                    protocol_version: proto::PROTOCOL_VERSION,
                    device_id: self.device_id.clone(),
                    tab_title: tab_title.clone(),
                }),
            },
            Side::Viewer {
                session,
                secret,
                display_name,
                ..
            } => fx.send(&proto::ViewerToServer::Join {
                protocol_version: proto::PROTOCOL_VERSION,
                session: *session,
                secret: secret.clone(),
                device_id: self.device_id.clone(),
                display_name: display_name.clone(),
            }),
        }
        fx
    }

    /// Feed one inbound JSON frame from the transport.
    pub fn on_message(&mut self, json: &str) -> Effects {
        let mut fx = Effects::default();
        match &mut self.side {
            Side::Host {
                session,
                reconnect_token,
                ..
            } => match proto::ServerToHost::from_json(json) {
                Ok(proto::ServerToHost::Initialized {
                    session: sid,
                    pairing_code,
                    reconnect_token: token,
                }) => {
                    *session = Some(sid);
                    *reconnect_token = Some(token);
                    self.state = SessionState::HostLive {
                        session: sid,
                        pairing_code,
                        participants: Vec::new(),
                    };
                    fx.emit(SessionEvent::StateChanged);
                }
                Ok(proto::ServerToHost::Reconnected) => fx.emit(SessionEvent::StateChanged),
                Ok(proto::ServerToHost::ParticipantsUpdated { participants }) => {
                    if let SessionState::HostLive {
                        participants: p, ..
                    } = &mut self.state
                    {
                        *p = participants;
                    }
                    fx.emit(SessionEvent::ParticipantsChanged);
                }
                Ok(proto::ServerToHost::CommandRequested {
                    request_id,
                    from,
                    command,
                    auto_approved,
                }) => {
                    if auto_approved {
                        fx.emit(SessionEvent::RunCommand {
                            request_id,
                            command,
                        });
                    } else {
                        fx.emit(SessionEvent::CommandApprovalRequested {
                            request_id,
                            from,
                            command,
                        });
                    }
                }
                Ok(proto::ServerToHost::AgentQueryRequested {
                    request_id,
                    from,
                    query,
                }) => fx.emit(SessionEvent::RunAgentQuery {
                    request_id,
                    from,
                    query,
                }),
                Ok(proto::ServerToHost::AgentStopRequested { .. }) => {
                    fx.emit(SessionEvent::StopAgent)
                }
                Ok(proto::ServerToHost::InputReceived { data_base64, .. }) => {
                    match base64::engine::general_purpose::STANDARD.decode(&data_base64) {
                        Ok(bytes) => fx.emit(SessionEvent::RunInput { bytes }),
                        Err(e) => fx.emit(SessionEvent::Error(format!("input decode: {e}"))),
                    }
                }
                Ok(proto::ServerToHost::ResizeRequested { rows, cols, .. }) => {
                    fx.emit(SessionEvent::ResizeRequested { rows, cols });
                }
                Ok(proto::ServerToHost::ControlReceived { from, surface }) => {
                    fx.emit(SessionEvent::ControlReceived { from, surface });
                }
                Ok(proto::ServerToHost::PointerMoved { from, position }) => {
                    fx.emit(SessionEvent::PointerMoved { from, position });
                }
                Ok(proto::ServerToHost::AccessRequested {
                    participant,
                    display_name,
                }) => fx.emit(SessionEvent::AccessRequested {
                    participant,
                    display_name,
                }),
                Ok(proto::ServerToHost::ForwardCreated {
                    forward_id,
                    port,
                    public_url,
                }) => fx.emit(SessionEvent::ForwardCreated {
                    forward_id,
                    port,
                    public_url,
                }),
                Ok(proto::ServerToHost::ForwardRequest {
                    forward_id,
                    port,
                    request_id,
                    method,
                    path,
                    headers,
                    body_base64,
                }) => match base64::engine::general_purpose::STANDARD.decode(body_base64) {
                    Ok(body) => fx.emit(SessionEvent::ForwardRequest {
                        forward_id,
                        port,
                        request_id,
                        method,
                        path,
                        headers,
                        body,
                    }),
                    Err(e) => fx.emit(SessionEvent::Error(format!("forward body decode: {e}"))),
                },
                Ok(proto::ServerToHost::ForwardWebSocketOpen {
                    port,
                    method,
                    path,
                    request_id,
                    headers,
                    body_base64,
                    ..
                }) => match base64::engine::general_purpose::STANDARD.decode(body_base64) {
                    Ok(body) => fx.emit(SessionEvent::ForwardWebSocketOpen {
                        port,
                        method,
                        path,
                        request_id,
                        headers,
                        body,
                    }),
                    Err(e) => fx.emit(SessionEvent::Error(format!(
                        "forward stream body decode: {e}"
                    ))),
                },
                Ok(proto::ServerToHost::ForwardWebSocketMessage {
                    request_id,
                    message_type,
                    data_base64,
                }) => match base64::engine::general_purpose::STANDARD.decode(data_base64) {
                    Ok(data) => fx.emit(SessionEvent::ForwardWebSocketMessage {
                        request_id,
                        message_type,
                        data,
                    }),
                    Err(e) => fx.emit(SessionEvent::Error(format!(
                        "forward websocket decode: {e}"
                    ))),
                },
                Ok(proto::ServerToHost::ForwardWebSocketClose { request_id }) => {
                    fx.emit(SessionEvent::ForwardWebSocketClose { request_id })
                }
                Ok(proto::ServerToHost::ForwardClosed { .. }) => {}
                Ok(proto::ServerToHost::ForwardError { reason }) => {
                    fx.emit(SessionEvent::Error(format!("port forward: {reason}")))
                }
                Ok(proto::ServerToHost::Terminated { reason }) => {
                    self.state = SessionState::Ended {
                        reason: reason.clone(),
                    };
                    fx.emit(SessionEvent::Ended(reason));
                }
                Ok(proto::ServerToHost::Pong) => {}
                Err(e) => fx.emit(SessionEvent::Error(format!("decode: {e}"))),
            },
            Side::Viewer {
                self_role,
                session,
                is_host_account,
                ..
            } => match proto::ServerToViewer::from_json(json) {
                Ok(proto::ServerToViewer::Joined {
                    role,
                    is_host_account: host_acct,
                    ..
                }) => {
                    *self_role = role;
                    *is_host_account = host_acct;
                    self.state = SessionState::ViewerLive {
                        session: *session,
                        self_role: role,
                        participants: Vec::new(),
                    };
                    fx.emit(SessionEvent::StateChanged);
                }
                Ok(proto::ServerToViewer::Delta { delta }) => fx.emit(SessionEvent::Delta(delta)),
                Ok(proto::ServerToViewer::ParticipantsUpdated { participants }) => {
                    if let SessionState::ViewerLive {
                        participants: p, ..
                    } = &mut self.state
                    {
                        *p = participants;
                    }
                    fx.emit(SessionEvent::ParticipantsChanged);
                }
                Ok(proto::ServerToViewer::RoleChanged { role }) => {
                    *self_role = role;
                    if let SessionState::ViewerLive { self_role: r, .. } = &mut self.state {
                        *r = role;
                    }
                    fx.emit(SessionEvent::StateChanged);
                }
                Ok(proto::ServerToViewer::AccessDenied) => fx.emit(SessionEvent::AccessDenied),
                Ok(proto::ServerToViewer::ForwardResponse { .. }) => {}
                Ok(proto::ServerToViewer::ForwardError { reason, .. }) => {
                    fx.emit(SessionEvent::Error(format!("port forward: {reason}")))
                }
                Ok(proto::ServerToViewer::Removed { reason })
                | Ok(proto::ServerToViewer::Ended { reason }) => {
                    self.state = SessionState::Ended {
                        reason: reason.clone(),
                    };
                    fx.emit(SessionEvent::Ended(reason));
                }
                Ok(proto::ServerToViewer::Pong) => {}
                Err(e) => fx.emit(SessionEvent::Error(format!("decode: {e}"))),
            },
        }
        fx
    }

    // ── Host actions ─────────────────────────────────────────────────────────

    /// Host: broadcast an ordered terminal change to participants.
    pub fn broadcast(&self, delta: proto::TerminalDelta) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::Delta { delta });
        }
        fx
    }

    /// Host: approve or deny a pending remote-command request.
    pub fn resolve_command(&self, request_id: Uuid, approved: bool) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::ResolveCommand {
                request_id,
                approved,
            });
        }
        fx
    }

    /// Host: change a participant's access (control vs view-only).
    pub fn set_participant_role(
        &self,
        participant: proto::ParticipantId,
        access: proto::AccessLevel,
    ) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::SetParticipantRole {
                participant,
                access,
            });
        }
        fx
    }

    /// Host: decline a participant's pending Edit request (they stay view-only,
    /// but are told their request was declined).
    pub fn deny_access_request(&self, participant: proto::ParticipantId) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::DenyAccessRequest { participant });
        }
        fx
    }

    /// Host: remove a participant from the session entirely (revoke access).
    pub fn remove_participant(&self, participant: proto::ParticipantId) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::RemoveParticipant { participant });
        }
        fx
    }

    /// Host: request that the gateway expose a loopback HTTP port.
    pub fn create_forward(&self, port: u16, label: impl Into<String>) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::ForwardCreate {
                port,
                label: label.into(),
                tunnel_version: Some(1),
            });
        }
        fx
    }

    /// Host: stop exposing a forward.
    pub fn close_forward(&self, forward_id: impl Into<String>) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::ForwardClose {
                forward_id: forward_id.into(),
            });
        }
        fx
    }

    /// Host: send a loopback HTTP response back to the requesting viewer.
    pub fn forward_response(
        &self,
        request_id: impl Into<String>,
        status: u16,
        headers: Vec<(String, String)>,
        body: &[u8],
    ) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::ForwardResponse {
                request_id: request_id.into(),
                status,
                headers,
                body_base64: base64::engine::general_purpose::STANDARD.encode(body),
            });
        }
        fx
    }

    // ── Viewer actions ───────────────────────────────────────────────────────

    /// Viewer: ask the host to grant control.
    pub fn request_control(&self) -> Effects {
        let mut fx = Effects::default();
        if let Side::Viewer { .. } = self.side {
            fx.send(&proto::ViewerToServer::RequestControl);
        }
        fx
    }

    /// Viewer (controller): request to run a command on the host. Refused
    /// locally unless this participant currently holds control.
    pub fn execute_command(&self, command: impl Into<String>) -> Effects {
        let mut fx = Effects::default();
        match &self.side {
            Side::Viewer { self_role, .. } if *self_role == proto::Role::Controller => {
                fx.send(&proto::ViewerToServer::ExecuteCommand {
                    command: command.into(),
                });
            }
            Side::Viewer { .. } => {
                fx.emit(SessionEvent::Error("view-only: control not granted".into()));
            }
            Side::Host { .. } => {}
        }
        fx
    }

    /// Viewer (controller): ask the host desktop to run an agent prompt. The
    /// browser never contacts the provider or executes local tools itself.
    pub fn submit_agent_query(&self, request_id: Uuid, query: impl Into<String>) -> Effects {
        let mut fx = Effects::default();
        match &self.side {
            Side::Viewer { self_role, .. } if *self_role == proto::Role::Controller => {
                fx.send(&proto::ViewerToServer::AgentQuery {
                    request_id,
                    query: query.into(),
                });
            }
            Side::Viewer { .. } => {
                fx.emit(SessionEvent::Error("view-only: control not granted".into()));
            }
            Side::Host { .. } => {}
        }
        fx
    }

    pub fn stop_agent(&self) -> Effects {
        let mut fx = Effects::default();
        match &self.side {
            Side::Viewer { self_role, .. } if *self_role == proto::Role::Controller => {
                fx.send(&proto::ViewerToServer::AgentStop);
            }
            Side::Viewer { .. } => {
                fx.emit(SessionEvent::Error("view-only: control not granted".into()));
            }
            Side::Host { .. } => {}
        }
        fx
    }

    /// Viewer (controller): forward raw input bytes to the host's PTY. Refused
    /// locally unless this participant currently holds control. Covers typed
    /// commands AND alt-screen keystrokes (Claude Code, vim, htop, …).
    pub fn send_input(&self, bytes: &[u8]) -> Effects {
        let mut fx = Effects::default();
        match &self.side {
            Side::Viewer { self_role, .. } if *self_role == proto::Role::Controller => {
                fx.send(&proto::ViewerToServer::InputBytes {
                    data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                });
            }
            Side::Viewer { .. } => {
                fx.emit(SessionEvent::Error("view-only: control not granted".into()));
            }
            Side::Host { .. } => {}
        }
        fx
    }

    /// Viewer (controller): ask the host to resize its shared PTY to this
    /// viewer's terminal grid, so a full-screen TUI re-renders to fill the
    /// viewer's window. Only sent while this participant holds control.
    pub fn send_resize(&self, rows: u16, cols: u16) -> Effects {
        let mut fx = Effects::default();
        if let Side::Viewer { self_role, .. } = &self.side {
            if *self_role == proto::Role::Controller {
                fx.send(&proto::ViewerToServer::Resize { rows, cols });
            }
        }
        fx
    }

    /// Viewer (controller): push the live input/scroll surface up to the host so
    /// the host desktop reflects what this participant is typing / where it has
    /// scrolled. Only sent while this participant holds control; a view-only
    /// participant silently sends nothing (it can't drive the host).
    pub fn send_control(&self, surface: proto::ControlSurface) -> Effects {
        let mut fx = Effects::default();
        if let Side::Viewer { self_role, .. } = &self.side {
            if *self_role == proto::Role::Controller {
                fx.send(&proto::ViewerToServer::Control { surface });
            }
        }
        fx
    }

    /// Viewer/controller: publish pointer presence. Unlike input/control, this
    /// is intentionally allowed for view-only participants.
    pub fn send_pointer(&self, position: proto::PointerPosition) -> Effects {
        let mut fx = Effects::default();
        if matches!(self.side, Side::Viewer { .. }) {
            fx.send(&proto::ViewerToServer::Pointer { position });
        }
        fx
    }

    /// Viewer: request one HTTP resource through a host-created forward.
    pub fn request_forward(
        &self,
        forward_id: impl Into<String>,
        request_id: impl Into<String>,
        method: impl Into<String>,
        path: impl Into<String>,
        headers: Vec<(String, String)>,
        body: &[u8],
    ) -> Effects {
        let mut fx = Effects::default();
        if !self.is_host() {
            fx.send(&proto::ViewerToServer::ForwardRequest {
                forward_id: forward_id.into(),
                request_id: request_id.into(),
                method: method.into(),
                path: path.into(),
                headers,
                body_base64: base64::engine::general_purpose::STANDARD.encode(body),
            });
        }
        fx
    }

    /// End the session locally. A host also tells the server to terminate it,
    /// forwarding `reason` (when set) so viewers see it as the end message (e.g.
    /// "Session ended due to inactivity").
    pub fn end(&mut self, reason: Option<String>) -> Effects {
        let mut fx = Effects::default();
        if self.is_host() {
            fx.send(&proto::HostToServer::EndSession {
                reason: reason.clone(),
            });
        }
        let local_reason = reason.unwrap_or_else(|| "ended".to_string());
        self.state = SessionState::Ended {
            reason: local_reason.clone(),
        };
        fx.emit(SessionEvent::Ended(local_reason));
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_host(json: &str) -> proto::HostToServer {
        proto::HostToServer::from_json(json).expect("valid HostToServer")
    }
    fn decode_viewer(json: &str) -> proto::ViewerToServer {
        proto::ViewerToServer::from_json(json).expect("valid ViewerToServer")
    }

    #[test]
    fn host_handshake_then_initialized() {
        let mut core = SessionCore::new_host("dev-1", "zsh — ~/dev");
        let fx = core.on_connected();
        assert!(matches!(
            decode_host(&fx.outbound[0]),
            proto::HostToServer::Initialize { .. }
        ));

        let sid = proto::SessionId::new();
        let server = proto::ServerToHost::Initialized {
            session: sid,
            pairing_code: Some("ABCD-1234".into()),
            reconnect_token: proto::ReconnectToken::new("rt"),
        }
        .to_json()
        .unwrap();
        let fx = core.on_message(&server);
        assert_eq!(fx.events, vec![SessionEvent::StateChanged]);
        match core.state() {
            SessionState::HostLive {
                session,
                pairing_code,
                ..
            } => {
                assert_eq!(*session, sid);
                assert_eq!(pairing_code.as_deref(), Some("ABCD-1234"));
            }
            other => panic!("expected HostLive, got {other:?}"),
        }
    }

    #[test]
    fn host_reconnect_uses_reconnect_after_session_known() {
        let mut core = SessionCore::new_host("dev-1", "t");
        core.on_connected();
        let sid = proto::SessionId::new();
        core.on_message(
            &proto::ServerToHost::Initialized {
                session: sid,
                pairing_code: None,
                reconnect_token: proto::ReconnectToken::new("rt"),
            }
            .to_json()
            .unwrap(),
        );
        // Socket dropped and came back: resume, don't open a new session.
        let fx = core.on_connected();
        assert!(matches!(
            decode_host(&fx.outbound[0]),
            proto::HostToServer::Reconnect { .. }
        ));
    }

    #[test]
    fn host_command_request_and_approval() {
        let mut core = SessionCore::new_host("dev-1", "t");
        let req = Uuid::new_v4();
        let pid = proto::ParticipantId::new();
        let fx = core.on_message(
            &proto::ServerToHost::CommandRequested {
                request_id: req,
                from: pid,
                command: "ls -la".into(),
                auto_approved: false,
            }
            .to_json()
            .unwrap(),
        );
        assert_eq!(
            fx.events,
            vec![SessionEvent::CommandApprovalRequested {
                request_id: req,
                from: pid,
                command: "ls -la".into(),
            }]
        );

        let fx = core.resolve_command(req, true);
        match decode_host(&fx.outbound[0]) {
            proto::HostToServer::ResolveCommand {
                request_id,
                approved,
            } => {
                assert_eq!(request_id, req);
                assert!(approved);
            }
            other => panic!("expected ResolveCommand, got {other:?}"),
        }
    }

    #[test]
    fn host_auto_approved_command_runs_without_prompt() {
        let mut core = SessionCore::new_host("dev-1", "t");
        let req = Uuid::new_v4();
        let fx = core.on_message(
            &proto::ServerToHost::CommandRequested {
                request_id: req,
                from: proto::ParticipantId::new(),
                command: "git status".into(),
                auto_approved: true,
            }
            .to_json()
            .unwrap(),
        );
        assert_eq!(
            fx.events,
            vec![SessionEvent::RunCommand {
                request_id: req,
                command: "git status".into(),
            }]
        );
    }

    #[test]
    fn host_broadcasts_delta() {
        let core = SessionCore::new_host("dev-1", "t");
        let fx = core.broadcast(proto::TerminalDelta::OutputChunk {
            block_id: "b1".into(),
            seq: 3,
            data_base64: "aGk=".into(),
        });
        assert!(matches!(
            decode_host(&fx.outbound[0]),
            proto::HostToServer::Delta { .. }
        ));
    }

    #[test]
    fn viewer_join_and_receive_delta() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-2", sid, proto::SessionSecret::new("s"), "Phone");
        let fx = core.on_connected();
        assert!(matches!(
            decode_viewer(&fx.outbound[0]),
            proto::ViewerToServer::Join { .. }
        ));

        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );
        assert!(matches!(core.state(), SessionState::ViewerLive { .. }));

        let fx = core.on_message(
            &proto::ServerToViewer::Delta {
                delta: proto::TerminalDelta::BlockStarted {
                    block_id: "b1".into(),
                    command: "npm test".into(),
                    cwd: None,
                    ts_ms: 0,
                },
            }
            .to_json()
            .unwrap(),
        );
        assert!(matches!(fx.events[0], SessionEvent::Delta(_)));
    }

    #[test]
    fn viewer_control_gate() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-2", sid, proto::SessionSecret::new("s"), "Phone");
        // View-only: command is refused locally.
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );
        let fx = core.execute_command("rm -rf /");
        assert!(fx.outbound.is_empty());
        assert!(matches!(fx.events[0], SessionEvent::Error(_)));

        // Granted control: command goes out.
        core.on_message(
            &proto::ServerToViewer::RoleChanged {
                role: proto::Role::Controller,
            }
            .to_json()
            .unwrap(),
        );
        let fx = core.execute_command("git status");
        assert!(matches!(
            decode_viewer(&fx.outbound[0]),
            proto::ViewerToServer::ExecuteCommand { .. }
        ));
    }

    #[test]
    fn viewer_agent_query_and_stop_require_control() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-agent", sid, proto::SessionSecret::new("s"), "Web");
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );

        let request_id = Uuid::new_v4();
        let denied = core.submit_agent_query(request_id, "inspect this");
        assert!(denied.outbound.is_empty());
        assert!(matches!(denied.events[0], SessionEvent::Error(_)));

        core.on_message(
            &proto::ServerToViewer::RoleChanged {
                role: proto::Role::Controller,
            }
            .to_json()
            .unwrap(),
        );
        let query = core.submit_agent_query(request_id, "inspect this");
        assert_eq!(
            decode_viewer(&query.outbound[0]),
            proto::ViewerToServer::AgentQuery {
                request_id,
                query: "inspect this".into(),
            }
        );
        let stop = core.stop_agent();
        assert_eq!(
            decode_viewer(&stop.outbound[0]),
            proto::ViewerToServer::AgentStop
        );
    }

    #[test]
    fn host_receives_agent_query_and_stop() {
        let mut core = SessionCore::new_host("dev-host", "Shared tab");
        let request_id = Uuid::new_v4();
        let from = proto::ParticipantId::new();

        let query = core.on_message(
            &proto::ServerToHost::AgentQueryRequested {
                request_id,
                from,
                query: "run the focused test".into(),
            }
            .to_json()
            .unwrap(),
        );
        assert_eq!(
            query.events,
            vec![SessionEvent::RunAgentQuery {
                request_id,
                from,
                query: "run the focused test".into(),
            }]
        );

        let stop = core.on_message(
            &proto::ServerToHost::AgentStopRequested { from }
                .to_json()
                .unwrap(),
        );
        assert_eq!(stop.events, vec![SessionEvent::StopAgent]);
    }

    #[test]
    fn viewer_host_account_flag_from_joined() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-x", sid, proto::SessionSecret::new("s"), "Self");
        assert!(!core.viewer_is_host_account());
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Controller,
                is_host_account: true,
            }
            .to_json()
            .unwrap(),
        );
        assert!(core.viewer_is_host_account());
    }

    #[test]
    fn viewer_input_gate_and_forward() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-2", sid, proto::SessionSecret::new("s"), "Phone");
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );
        // View-only: raw input is refused locally (nothing hits the wire).
        let fx = core.send_input(&[0x1b, b'[', b'A']);
        assert!(fx.outbound.is_empty());
        assert!(matches!(fx.events[0], SessionEvent::Error(_)));

        // Granted control: input is base64-framed and sent.
        core.on_message(
            &proto::ServerToViewer::RoleChanged {
                role: proto::Role::Controller,
            }
            .to_json()
            .unwrap(),
        );
        let fx = core.send_input(&[0x03]); // Ctrl-C
        match decode_viewer(&fx.outbound[0]) {
            proto::ViewerToServer::InputBytes { data_base64 } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&data_base64)
                    .unwrap();
                assert_eq!(bytes, vec![0x03]);
            }
            other => panic!("expected InputBytes, got {other:?}"),
        }
    }

    #[test]
    fn viewer_control_surface_gate_and_forward() {
        let sid = proto::SessionId::new();
        let mut core = SessionCore::new_viewer("dev-3", sid, proto::SessionSecret::new("s"), "Web");
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );
        let surface = proto::ControlSurface {
            command_input: Some("ls".into()),
            command_cursor: Some(2),
            ..Default::default()
        };
        // View-only: the control surface never reaches the wire (can't drive host).
        let fx = core.send_control(surface.clone());
        assert!(fx.outbound.is_empty());

        // Granted control: the surface is forwarded to the host.
        core.on_message(
            &proto::ServerToViewer::RoleChanged {
                role: proto::Role::Controller,
            }
            .to_json()
            .unwrap(),
        );
        let fx = core.send_control(surface);
        match decode_viewer(&fx.outbound[0]) {
            proto::ViewerToServer::Control { surface } => {
                assert_eq!(surface.command_input.as_deref(), Some("ls"));
            }
            other => panic!("expected Control, got {other:?}"),
        }
    }

    #[test]
    fn view_only_participant_can_publish_pointer_presence() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-pointer", sid, proto::SessionSecret::new("s"), "Web");
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );

        let position = proto::PointerPosition { x: 0.25, y: 0.75 };
        let fx = core.send_pointer(position);
        match decode_viewer(&fx.outbound[0]) {
            proto::ViewerToServer::Pointer { position: sent } => assert_eq!(sent, position),
            other => panic!("expected Pointer, got {other:?}"),
        }
    }

    #[test]
    fn host_control_received_surfaces_to_ui() {
        let mut core = SessionCore::new_host("dev-4", "zsh");
        let from = proto::ParticipantId::new();
        let fx = core.on_message(
            &proto::ServerToHost::ControlReceived {
                from,
                surface: proto::ControlSurface {
                    scroll: Some(proto::ScrollUpdate {
                        fraction: Some(0.25),
                    }),
                    ..Default::default()
                },
            }
            .to_json()
            .unwrap(),
        );
        assert!(matches!(
            fx.events.as_slice(),
            [SessionEvent::ControlReceived {
                from: actual_from,
                surface
            }] if *actual_from == from
                && surface.scroll.as_ref().and_then(|s| s.fraction) == Some(0.25)
        ));
    }

    #[test]
    fn host_access_request_surfaces() {
        let mut core = SessionCore::new_host("dev-1", "t");
        let pid = proto::ParticipantId::new();
        let fx = core.on_message(
            &proto::ServerToHost::AccessRequested {
                participant: pid,
                display_name: "Sam".into(),
            }
            .to_json()
            .unwrap(),
        );
        assert_eq!(
            fx.events,
            vec![SessionEvent::AccessRequested {
                participant: pid,
                display_name: "Sam".into(),
            }]
        );
    }

    #[test]
    fn host_deny_access_request_sends_frame() {
        let mut core = SessionCore::new_host("dev-1", "t");
        core.on_connected();
        let pid = proto::ParticipantId::new();
        let fx = core.deny_access_request(pid);
        match decode_host(&fx.outbound[0]) {
            proto::HostToServer::DenyAccessRequest { participant } => assert_eq!(participant, pid),
            other => panic!("expected DenyAccessRequest, got {other:?}"),
        }
    }

    #[test]
    fn viewer_access_denied_surfaces() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-2", sid, proto::SessionSecret::new("s"), "Phone");
        core.on_message(
            &proto::ServerToViewer::Joined {
                participant: proto::ParticipantId::new(),
                role: proto::Role::Viewer,
                is_host_account: false,
            }
            .to_json()
            .unwrap(),
        );
        let fx = core.on_message(&proto::ServerToViewer::AccessDenied.to_json().unwrap());
        assert_eq!(fx.events, vec![SessionEvent::AccessDenied]);
    }

    #[test]
    fn host_remove_participant_sends_frame() {
        let mut core = SessionCore::new_host("dev-1", "t");
        core.on_connected();
        let pid = proto::ParticipantId::new();
        let fx = core.remove_participant(pid);
        match decode_host(&fx.outbound[0]) {
            proto::HostToServer::RemoveParticipant { participant } => assert_eq!(participant, pid),
            other => panic!("expected RemoveParticipant, got {other:?}"),
        }
    }

    #[test]
    fn host_input_received_decodes_to_run_input() {
        let mut core = SessionCore::new_host("dev-1", "t");
        let data = base64::engine::general_purpose::STANDARD.encode([0x1b, b'[', b'B']);
        let fx = core.on_message(
            &proto::ServerToHost::InputReceived {
                from: proto::ParticipantId::new(),
                data_base64: data,
            }
            .to_json()
            .unwrap(),
        );
        assert_eq!(
            fx.events,
            vec![SessionEvent::RunInput {
                bytes: vec![0x1b, b'[', b'B'],
            }]
        );
    }

    #[test]
    fn viewer_session_ended() {
        let sid = proto::SessionId::new();
        let mut core =
            SessionCore::new_viewer("dev-2", sid, proto::SessionSecret::new("s"), "Phone");
        let fx = core.on_message(
            &proto::ServerToViewer::Ended {
                reason: "host left".into(),
            }
            .to_json()
            .unwrap(),
        );
        assert_eq!(fx.events, vec![SessionEvent::Ended("host left".into())]);
        assert!(matches!(core.state(), SessionState::Ended { .. }));
    }
}
