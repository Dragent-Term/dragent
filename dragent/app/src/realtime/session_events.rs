//! `SessionManager` — the app-level singleton coordinating shared sessions.
//!
//! It owns the active session's [`SessionController`], bridges the realtime
//! transport into it (forwarding the transport's `async_channel` into
//! [`SessionController::on_event`] via `spawn_stream_local`), keeps a UI-facing
//! snapshot (state, participants, pairing code, connectivity), mirrors a
//! viewer's incoming output into a headless emulator, and emits
//! [`SessionManagerEvent`]s that views (the Share sheet, the mirror tab, the
//! terminal) subscribe to.
//!
//! The UI surfaces that drive and observe this (Share sheet, mirror tab, host
//! output tap) are wired in follow-up steps; this file is the backbone they hang
//! off. `dead_code` is allowed until those land.
#![allow(dead_code)]

use base64::Engine as _;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;
use web_time::Instant;

use dragent_session_protocol::Message;
use uuid::Uuid;

use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

use dragent_session_protocol::{
    AccessLevel, Participant, ParticipantId, TerminalDelta, TerminalSurfaceState,
};

use super::controller::SessionController;
use super::session::{SessionEvent, SessionState};
use super::RealtimeEvent;
use crate::realtime::manager::ForwardWebSocketCommand;
use crate::realtime::manager::HostShareCursor;
use crate::realtime::manager::PortForwardInfo;
use crate::realtime::manager::SessionManagerEvent;
use crate::settings::AppSettings;
use crate::terminal::model::block_share::{wire_lines, HostBlockSnapshot};

/// Host-only cursor tracking which blocks have been announced (`BlockStarted`)
/// and finalized (`BlockCompleted`) to viewers, so repeated snapshots only emit
/// the deltas that changed.
impl super::manager::SessionManager {
    pub(crate) fn subscribe(
        &mut self,
        ctx: &mut ModelContext<Self>,
        rx: async_channel::Receiver<RealtimeEvent>,
    ) {
        ctx.spawn_stream_local(
            rx,
            |this, ev: RealtimeEvent, mctx| this.on_realtime_event(ev, mctx),
            |_, _| {},
        );
    }

    fn on_realtime_event(&mut self, ev: RealtimeEvent, ctx: &mut ModelContext<Self>) {
        let ui_events = match self.session.as_mut() {
            Some(s) => s.controller.on_event(ev),
            None => return,
        };
        for e in ui_events {
            self.handle_session_event(e, ctx);
        }
    }

    fn handle_session_event(&mut self, e: SessionEvent, ctx: &mut ModelContext<Self>) {
        match e {
            SessionEvent::Connectivity { online } => {
                if let Some(s) = self.session.as_mut() {
                    s.online = online;
                    if online {
                        s.last_error = None; // a successful connect clears the error
                    }
                }
                ctx.emit(SessionManagerEvent::Changed);
            }
            SessionEvent::StateChanged | SessionEvent::ParticipantsChanged => {
                let before = self
                    .session
                    .as_ref()
                    .map(|s| s.participants.len())
                    .unwrap_or(0);
                self.sync_snapshot();
                let after = self
                    .session
                    .as_ref()
                    .map(|s| s.participants.len())
                    .unwrap_or(0);
                if let Some(id) = self.session_id() {
                    if self.is_host() {
                        log::info!("[share] host session live — session id {id}");
                    } else {
                        log::info!("[share] joined session {id}");
                    }
                }
                // A new participant joined: replay the session so far to them.
                // Reset the share cursor so every existing block re-broadcasts,
                // then ask the shared pane to push its full state. A viewer
                // authenticated as the host's own account is deliberately
                // omitted from the visible participant list, so that join can
                // leave the count unchanged; equal-count updates are therefore
                // also treated as a possible join. A leave (after < before)
                // cannot need a replay.
                if self.is_host() && after >= before {
                    if let Some(s) = self.session.as_mut() {
                        s.share_cursor = HostShareCursor::default();
                    }
                    ctx.emit(SessionManagerEvent::ResyncViewers);
                }
                // A participant coming or going counts as activity.
                self.note_activity();
                ctx.emit(SessionManagerEvent::Changed);
            }
            SessionEvent::Delta(delta) => {
                // Hand raw output to the mirror view (it renders identically to
                // the host). BlockStarted/Completed are implicit in the stream.
                if let TerminalDelta::OutputChunk { data_base64, .. } = &delta {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_base64)
                    {
                        ctx.emit(SessionManagerEvent::MirrorOutput { bytes });
                    }
                }
                // Block structure is the host's OWN rendering — forward it so the
                // mirror draws exactly what the host drew, rather than its own
                // re-parse of the stream (which drifts on any width or shell-hook
                // desync, and cannot be un-wrapped once wrong).
                if matches!(
                    &delta,
                    TerminalDelta::BlockStarted { .. }
                        | TerminalDelta::BlockLines { .. }
                        | TerminalDelta::BlockCompleted { .. }
                ) {
                    ctx.emit(SessionManagerEvent::MirrorBlock {
                        delta: delta.clone(),
                    });
                }
                if let TerminalDelta::AgentBlocks { blocks } = &delta {
                    ctx.emit(SessionManagerEvent::MirrorAgentBlocks {
                        blocks: blocks.clone(),
                    });
                }
                // Track the host's theme so the viewer's surface matches it.
                // `Changed` prompts `ViewerRoot` to re-read `viewer_theme_id()`
                // and re-theme the mirror.
                if let TerminalDelta::SurfaceState { state } = &delta {
                    if state.theme_id != self.viewer_theme_id {
                        self.viewer_theme_id = state.theme_id.clone();
                        ctx.emit(SessionManagerEvent::Changed);
                    }
                    // Push the host's input-layer state to the mirror so its
                    // AI-tool input panel matches (visible on the viewer too).
                    ctx.emit(SessionManagerEvent::MirrorSurface {
                        state: state.clone(),
                    });
                }
            }
            SessionEvent::CommandApprovalRequested {
                request_id,
                from,
                command,
            } => ctx.emit(SessionManagerEvent::ApprovalRequested {
                request_id,
                from,
                command,
            }),
            SessionEvent::RunCommand {
                request_id,
                command,
            } => ctx.emit(SessionManagerEvent::RunCommand {
                request_id,
                command,
            }),
            SessionEvent::RunAgentQuery {
                request_id,
                from,
                query,
            } => {
                self.note_activity();
                ctx.emit(SessionManagerEvent::RunAgentQuery {
                    request_id,
                    from,
                    query,
                })
            }
            SessionEvent::StopAgent => {
                self.note_activity();
                ctx.emit(SessionManagerEvent::StopAgent)
            }
            SessionEvent::RunInput { bytes } => {
                self.note_activity(); // controller input = activity
                ctx.emit(SessionManagerEvent::RunInput { bytes })
            }
            SessionEvent::ResizeRequested { rows, cols } => {
                self.note_activity();
                ctx.emit(SessionManagerEvent::RunResize { rows, cols })
            }
            SessionEvent::ControlReceived { from, surface } => {
                self.note_activity(); // controller input/scroll = activity
                ctx.emit(SessionManagerEvent::ApplyControlSurface { from, surface })
            }
            SessionEvent::PointerMoved { from, position } => {
                self.note_activity();
                ctx.emit(SessionManagerEvent::RemotePointerMoved { from, position })
            }
            SessionEvent::AccessRequested {
                participant,
                display_name,
            } => ctx.emit(SessionManagerEvent::AccessRequested {
                participant,
                display_name,
            }),
            SessionEvent::AccessDenied => ctx.emit(SessionManagerEvent::AccessDenied),
            SessionEvent::ForwardCreated {
                forward_id,
                port,
                public_url,
            } => {
                // Port forwards carry their own capability token. They remain
                // available when terminal tab sharing is disabled and must not
                // inherit a session-share credential.
                let display_url = public_url;
                if let Some(existing) = self
                    .forwards
                    .iter_mut()
                    .find(|f| f.id == forward_id || f.port == port)
                {
                    existing.id = forward_id.clone();
                    existing.port = port;
                    existing.url = display_url.clone();
                } else {
                    self.forwards.push(PortForwardInfo {
                        id: forward_id.clone(),
                        port,
                        url: display_url.clone(),
                    });
                }
                ctx.emit(SessionManagerEvent::ForwardCreated {
                    forward_id,
                    public_url: display_url,
                });
                ctx.emit(SessionManagerEvent::Changed);
            }
            SessionEvent::ForwardRequest {
                forward_id: _,
                port,
                request_id,
                method,
                path,
                headers,
                body,
            } => {
                // The host-side connector is intentionally loopback-only. A
                // request for an unknown forward is rejected by the gateway;
                // this event carries the approved request from that gateway.
                let result =
                    crate::realtime::forward::request(port, &method, &path, &headers, &body);
                if let Some(s) = self.session.as_ref() {
                    match result {
                        Ok(response) => s.controller.forward_response(
                            request_id,
                            response.status,
                            response.headers,
                            &response.body,
                        ),
                        Err(reason) => s.controller.forward_response(
                            request_id,
                            502,
                            vec![("content-type".into(), "text/plain".into())],
                            reason.as_bytes(),
                        ),
                    }
                }
            }
            SessionEvent::ForwardWebSocketOpen {
                port,
                method,
                path,
                request_id,
                headers,
                body,
            } =>
            {
                #[cfg(not(target_family = "wasm"))]
                if let Some(s) = self.session.as_mut() {
                    let (tx, rx) = mpsc::channel();
                    s.forward_websockets.insert(request_id.clone(), tx);
                    let handle = s.controller.transport_handle();
                    std::thread::spawn(move || {
                        match crate::realtime::forward::stream(
                            port, &method, &path, &headers, &body,
                        ) {
                            Ok(mut socket) => {
                                handle.send(
                                    dragent_session_protocol::HostToServer::ForwardWebSocketOpened { request_id: request_id.clone() }
                                        .to_json().unwrap_or_default(),
                                );
                                loop {
                                    let mut stream_failed = false;
                                    while let Ok(command) = rx.try_recv() {
                                        match command {
                                            ForwardWebSocketCommand::Message {
                                                message_type: _,
                                                data,
                                            } => {
                                                if crate::realtime::forward::stream_write(
                                                    &mut socket,
                                                    &data,
                                                )
                                                .is_err()
                                                {
                                                    stream_failed = true;
                                                    break;
                                                }
                                            }
                                            ForwardWebSocketCommand::Close => return,
                                        }
                                    }
                                    if stream_failed {
                                        break;
                                    }
                                    match crate::realtime::forward::stream_read(&mut socket) {
                                        Ok(Some(data)) => handle.send(
                                            dragent_session_protocol::HostToServer::ForwardWebSocketMessage {
                                                request_id: request_id.clone(), message_type: "data".into(),
                                                data_base64: base64::engine::general_purpose::STANDARD.encode(data),
                                            }.to_json().unwrap_or_default(),
                                        ),
                                        Ok(None) => continue,
                                        Err(_) => break,
                                    }
                                }
                                handle.send(
                                    dragent_session_protocol::HostToServer::ForwardWebSocketClosed { request_id }
                                        .to_json().unwrap_or_default(),
                                );
                            }
                            Err(_) => handle.send(
                                dragent_session_protocol::HostToServer::ForwardWebSocketClosed {
                                    request_id,
                                }
                                .to_json()
                                .unwrap_or_default(),
                            ),
                        }
                    });
                }
            }
            SessionEvent::ForwardWebSocketMessage {
                request_id,
                message_type,
                data,
            } => {
                if let Some(s) = self.session.as_ref() {
                    if let Some(channel) = s.forward_websockets.get(&request_id) {
                        let _ =
                            channel.send(ForwardWebSocketCommand::Message { message_type, data });
                    }
                }
            }
            SessionEvent::ForwardWebSocketClose { request_id } => {
                if let Some(s) = self.session.as_mut() {
                    if let Some(channel) = s.forward_websockets.remove(&request_id) {
                        let _ = channel.send(ForwardWebSocketCommand::Close);
                    }
                }
            }
            SessionEvent::Ended(reason) => {
                log::warn!("[share] session ended by server: {reason}");
                self.session = None;
                ctx.emit(SessionManagerEvent::Ended(reason));
            }
            SessionEvent::Error(err) => {
                log::warn!("[share] transport error: {err}");
                if let Some(s) = self.session.as_mut() {
                    s.online = false;
                    s.last_error = Some(err.clone());
                }
                ctx.emit(SessionManagerEvent::Error(err));
                ctx.emit(SessionManagerEvent::Changed);
            }
        }
    }

    fn sync_snapshot(&mut self) {
        if let Some(s) = self.session.as_mut() {
            let state = s.controller.state().clone();
            match &state {
                SessionState::HostLive {
                    participants,
                    pairing_code,
                    ..
                } => {
                    s.participants = participants.clone();
                    s.pairing_code = pairing_code.clone();
                }
                SessionState::ViewerLive { participants, .. } => {
                    s.participants = participants.clone();
                }
                _ => {}
            }
            s.state = state;
        }
    }
}
