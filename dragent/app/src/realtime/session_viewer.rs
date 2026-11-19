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
use crate::realtime::manager::access_str;
use crate::realtime::manager::access_str_for;
use crate::realtime::manager::ActiveSession;
use crate::realtime::manager::PortForwardInfo;
use crate::realtime::manager::SessionManagerEvent;
use crate::settings::AppSettings;
use crate::terminal::model::block_share::{wire_lines, HostBlockSnapshot};

/// Host-only cursor tracking which blocks have been announced (`BlockStarted`)
/// and finalized (`BlockCompleted`) to viewers, so repeated snapshots only emit
/// the deltas that changed.
impl super::manager::SessionManager {
    /// Join a session as a viewer, with credentials from a claimed pairing code
    /// or a resolved share link.
    pub fn start_viewer(
        &mut self,
        ctx: &mut ModelContext<Self>,
        session_id: &str,
        secret: &str,
        display_name: impl Into<String>,
    ) -> Result<(), uuid::Error> {
        self.stop_internal();
        let (tx, rx) = async_channel::unbounded::<RealtimeEvent>();
        let controller = SessionController::start_viewer(
            AppSettings::device_id(),
            session_id,
            secret,
            display_name,
            tx,
        )?;
        self.subscribe(ctx, rx);
        self.session = Some(ActiveSession::viewer(controller));
        Ok(())
    }

    pub fn end(&mut self, ctx: &mut ModelContext<Self>) {
        self.end_with_reason(None, ctx);
    }

    /// End the session, optionally forwarding a reason to viewers (shown as their
    /// end message). Used by the inactivity auto-end.
    pub fn end_with_reason(&mut self, reason: Option<String>, ctx: &mut ModelContext<Self>) {
        if let Some(s) = self.session.as_mut() {
            s.controller.end(reason.clone());
        }
        self.session = None;
        ctx.emit(SessionManagerEvent::Ended(
            reason.unwrap_or_else(|| "ended".into()),
        ));
    }

    /// Host: dismiss the idle warning and reset the inactivity countdown — the
    /// user chose to keep the session alive.
    pub fn keep_session_alive(&mut self, ctx: &mut ModelContext<Self>) {
        self.note_activity();
        if let Some(s) = self.session.as_mut() {
            if s.idle_warned {
                s.idle_warned = false;
                ctx.emit(SessionManagerEvent::IdleWarningCleared);
            }
        }
    }

    pub(crate) fn stop_internal(&mut self) {
        if let Some(s) = &self.session {
            s.controller.stop();
        }
        self.session = None;
    }

    pub fn approve_command(&self, request_id: Uuid, approved: bool) {
        if let Some(s) = &self.session {
            s.controller.resolve_command(request_id, approved);
        }
    }

    pub fn broadcast_delta(&self, delta: TerminalDelta) {
        if let Some(s) = &self.session {
            s.controller.broadcast_delta(delta);
        }
    }

    /// Host: publish the non-PTY presentation state used by the terminal
    /// surface. Viewers render this in WASM; the WebSocket remains transport
    /// only and does not interpret the fields.
    pub fn broadcast_surface_state(&self, state: TerminalSurfaceState) {
        self.broadcast_delta(TerminalDelta::SurfaceState { state });
    }

    /// Host: expose a local service with its own forward capability. The
    /// desktop only dials loopback when a public connection arrives.
    pub fn create_port_forward(
        &mut self,
        ctx: &mut ModelContext<Self>,
        port: u16,
        label: impl Into<String>,
    ) {
        if self.forward_busy {
            return;
        }
        self.forward_busy = true;
        self.forward_error = None;
        ctx.emit(SessionManagerEvent::Changed);
        let device_id = AppSettings::device_id();
        let label = label.into();
        ctx.spawn(
            async move { crate::realtime::rest::create_port_forward(&device_id, port, &label) },
            move |this, result, mctx| {
                this.forward_busy = false;
                match result {
                    Ok(forward) => {
                        this.forward_error = None;
                        this.forwards.retain(|f| f.port != forward.port);
                        this.forwards.push(PortForwardInfo {
                            id: forward.id.clone(),
                            port: forward.port,
                            url: forward.public_url.clone(),
                        });
                        mctx.emit(SessionManagerEvent::ForwardCreated {
                            forward_id: forward.id,
                            public_url: forward.public_url,
                        });
                        mctx.emit(SessionManagerEvent::ForwardOpFinished {
                            notice: format!("Port {} forwarded", forward.port),
                            error: None,
                        });
                    }
                    Err(error) => {
                        let message = format!("could not forward port {port}: {error:?}");
                        this.forward_error = Some(message.clone());
                        mctx.emit(SessionManagerEvent::ForwardOpFinished {
                            notice: "Couldn't forward port".into(),
                            error: Some(message),
                        });
                    }
                }
                mctx.emit(SessionManagerEvent::Changed);
            },
        );
    }

    pub fn close_port_forward(
        &mut self,
        ctx: &mut ModelContext<Self>,
        forward_id: impl Into<String>,
    ) {
        let forward_id = forward_id.into();
        let device_id = AppSettings::device_id();
        ctx.spawn(
            async move {
                crate::realtime::rest::close_port_forward(&device_id, &forward_id)?;
                Ok::<_, crate::api::ApiError>(forward_id)
            },
            |this, result, mctx| {
                match result {
                    Ok(forward_id) => {
                        this.forwards.retain(|f| f.id != forward_id);
                        this.forward_error = None;
                        mctx.emit(SessionManagerEvent::ForwardOpFinished {
                            notice: "Port forward stopped".into(),
                            error: None,
                        });
                    }
                    Err(error) => {
                        let message = format!("could not stop port forward: {error:?}");
                        this.forward_error = Some(message.clone());
                        mctx.emit(SessionManagerEvent::ForwardOpFinished {
                            notice: "Couldn't stop port forward".into(),
                            error: Some(message),
                        });
                    }
                }
                mctx.emit(SessionManagerEvent::Changed);
            },
        );
    }

    pub fn set_participant_role(&self, participant: ParticipantId, access: AccessLevel) {
        if let Some(s) = &self.session {
            s.controller.set_participant_role(participant, access);
        }
    }

    /// Host: remove a participant from the session (revoke access) by id string.
    pub fn remove_participant(&self, participant_id: &str) {
        if let (Some(s), Ok(uuid)) = (&self.session, participant_id.parse::<Uuid>()) {
            s.controller.remove_participant(ParticipantId(uuid));
        }
    }

    /// Host: resolve a pending Edit request. Approving grants the participant
    /// Control; declining notifies the requester they were declined (they stay
    /// view-only).
    pub fn resolve_access_request(&self, participant_id: &str, approve: bool) {
        let Ok(uuid) = participant_id.parse::<Uuid>() else {
            return;
        };
        if approve {
            self.set_participant_role(ParticipantId(uuid), AccessLevel::Control);
        } else if let Some(s) = &self.session {
            s.controller.deny_access_request(ParticipantId(uuid));
        }
    }

    /// Host: change what "Anyone with the link" grants. `None` revokes the link;
    /// `Some(access)` (re)issues one at that access level. Runs the blocking REST
    /// calls on a background thread.
    pub fn set_link_access(&mut self, ctx: &mut ModelContext<Self>, access: Option<AccessLevel>) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        // Revoke the previous link, if any (best effort — a stale link expires).
        if let Some(tok) = self.session.as_mut().and_then(|s| s.link_token.take()) {
            ctx.spawn(
                async move {
                    let _ = crate::realtime::rest::revoke_share_link(&tok);
                },
                |_this, _out: (), _mctx| {},
            );
        }
        let Some(access) = access else {
            if let Some(s) = self.session.as_mut() {
                s.share_link = None;
                s.link_access = None;
            }
            ctx.emit(SessionManagerEvent::Changed);
            return;
        };
        let access_str = access_str(access);
        // Mark the control busy so the sheet can show a spinner immediately —
        // creating a link is a round trip, and without this the button looked
        // like it had done nothing.
        self.share_busy = Some(crate::workspace::share_sheet::ShareOp::LinkAccess);
        ctx.emit(SessionManagerEvent::Changed);
        ctx.spawn(
            async move { crate::realtime::rest::create_share_link(&session_id, access_str) },
            move |this, result, mctx| {
                this.share_busy = None;
                match result {
                    Ok(link) => {
                        if let Some(s) = this.session.as_mut() {
                            s.share_link = Some(link.join_url);
                            s.link_token = Some(link.token);
                            s.link_access = Some(access);
                        }
                        mctx.emit(SessionManagerEvent::ShareOpFinished {
                            op: crate::workspace::share_sheet::ShareOp::LinkAccess,
                            error: None,
                        });
                        mctx.emit(SessionManagerEvent::Changed);
                    }
                    Err(e) => {
                        let msg = format!("could not update link access: {e:?}");
                        mctx.emit(SessionManagerEvent::ShareOpFinished {
                            op: crate::workspace::share_sheet::ShareOp::LinkAccess,
                            error: Some(msg.clone()),
                        });
                        mctx.emit(SessionManagerEvent::Error(msg));
                    }
                }
            },
        );
    }

    /// Host: set the role the email-invite field will grant.
    pub fn set_invite_role(&mut self, ctx: &mut ModelContext<Self>, control: bool) {
        if let Some(s) = self.session.as_mut() {
            s.invite_control = control;
        }
        ctx.emit(SessionManagerEvent::Changed);
    }

    /// Host: email an invite for the given address at the current invite role.
    pub fn invite_email(&mut self, ctx: &mut ModelContext<Self>, email: String) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let access_str = access_str_for(self.invite_control());
        self.share_busy = Some(crate::workspace::share_sheet::ShareOp::Invite);
        ctx.emit(SessionManagerEvent::Changed);
        ctx.spawn(
            async move { crate::realtime::rest::invite_by_email(&session_id, &email, access_str) },
            |this, result, mctx| {
                this.share_busy = None;
                match result {
                    Ok(_) => {
                        mctx.emit(SessionManagerEvent::ShareOpFinished {
                            op: crate::workspace::share_sheet::ShareOp::Invite,
                            error: None,
                        });
                        mctx.emit(SessionManagerEvent::Changed);
                    }
                    Err(e) => {
                        let msg = format!("could not send invite: {e:?}");
                        mctx.emit(SessionManagerEvent::ShareOpFinished {
                            op: crate::workspace::share_sheet::ShareOp::Invite,
                            error: Some(msg.clone()),
                        });
                        mctx.emit(SessionManagerEvent::Error(msg));
                    }
                }
            },
        );
    }

    pub fn request_control(&self) {
        if let Some(s) = &self.session {
            s.controller.request_control();
        }
    }

    pub fn execute_command(&self, command: impl Into<String>) {
        if let Some(s) = &self.session {
            let _ = s.controller.execute_command(command);
        }
    }

    pub fn submit_agent_query(&self, query: impl Into<String>) {
        if let Some(s) = &self.session {
            s.controller.submit_agent_query(Uuid::new_v4(), query);
        }
    }

    pub fn stop_agent(&self) {
        if let Some(s) = &self.session {
            s.controller.stop_agent();
        }
    }

    /// Viewer (controller): forward raw input bytes (typed keys + alt-screen
    /// keystrokes) to the host. Silently ignored without control or a session.
    pub fn send_input(&self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            let _ = s.controller.send_input(bytes);
        }
    }

    /// Viewer (controller): ask the host to resize its shared PTY to this
    /// viewer's grid. Silently ignored without control or a session.
    pub fn send_resize(&self, rows: u16, cols: u16) {
        if let Some(s) = &self.session {
            let _ = s.controller.send_resize(rows, cols);
        }
    }

    /// Viewer (controller): push this participant's live input/scroll surface up
    /// to the host so the host desktop reflects it. Role-gated in the core (a
    /// view-only participant sends nothing).
    pub fn send_control(&self, surface: dragent_session_protocol::ControlSurface) {
        if let Some(s) = &self.session {
            s.controller.send_control(surface);
        }
    }

    /// Viewer/controller: publish pointer presence independently of Edit.
    pub fn send_pointer(&self, position: dragent_session_protocol::PointerPosition) {
        if let Some(s) = &self.session {
            s.controller.send_pointer(position);
        }
    }
}
