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
use crate::realtime::manager::ActiveSession;
use crate::realtime::manager::PortForwardInfo;
use crate::realtime::manager::SessionManagerEvent;
use crate::realtime::manager::ThrottleTick;
use crate::realtime::manager::IDLE_CHECK;
use crate::realtime::manager::IDLE_GRACE;
use crate::realtime::manager::IDLE_WARN_AFTER;
use crate::realtime::manager::INACTIVITY_END_REASON;
use crate::settings::AppSettings;
use crate::terminal::model::block_share::{wire_lines, HostBlockSnapshot};

/// Host-only cursor tracking which blocks have been announced (`BlockStarted`)
/// and finalized (`BlockCompleted`) to viewers, so repeated snapshots only emit
/// the deltas that changed.
impl super::manager::SessionManager {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let device_id = AppSettings::device_id();
        #[cfg(not(target_family = "wasm"))]
        let forward_tunnel = crate::realtime::forward_tunnel::start(device_id.clone());
        let load_device = device_id.clone();
        ctx.spawn(
            async move { crate::realtime::rest::list_port_forwards(&load_device) },
            |this, result, mctx| {
                if let Ok(forwards) = result {
                    this.forwards = forwards
                        .into_iter()
                        .map(|f| PortForwardInfo {
                            id: f.id,
                            port: f.port,
                            url: f.public_url,
                        })
                        .collect();
                    mctx.emit(SessionManagerEvent::Changed);
                }
            },
        );
        Self {
            session: None,
            forwards: Vec::new(),
            forward_error: None,
            forward_busy: false,
            #[cfg(not(target_family = "wasm"))]
            _forward_tunnel: Some(forward_tunnel),
            idle_epoch: 0,
            share_busy: None,
            viewer_theme_id: None,
        }
    }

    /// Viewer-side: the host's current theme id (from `SurfaceState`), if known.
    pub fn viewer_theme_id(&self) -> Option<String> {
        self.viewer_theme_id.clone()
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    pub fn is_host(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.controller.is_host())
    }

    pub fn is_online(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.online)
    }

    pub fn pairing_code(&self) -> Option<&str> {
        self.session
            .as_ref()
            .and_then(|s| s.pairing_code.as_deref())
    }

    pub fn participants(&self) -> &[Participant] {
        self.session
            .as_ref()
            .map(|s| s.participants.as_slice())
            .unwrap_or(&[])
    }

    pub fn share_link(&self) -> Option<&str> {
        self.session.as_ref().and_then(|s| s.share_link.as_deref())
    }

    pub fn port_forwards(&self) -> &[PortForwardInfo] {
        &self.forwards
    }

    pub fn forward_error(&self) -> Option<&str> {
        self.forward_error.as_deref()
    }

    pub fn forward_busy(&self) -> bool {
        self.forward_busy
    }

    pub fn set_forwarding_authenticated(
        &mut self,
        ctx: &mut ModelContext<Self>,
        authenticated: bool,
    ) {
        #[cfg(not(target_family = "wasm"))]
        if authenticated {
            if self._forward_tunnel.is_none() {
                self._forward_tunnel = Some(crate::realtime::forward_tunnel::start(
                    AppSettings::device_id(),
                ));
            }
        } else if let Some(tunnel) = self._forward_tunnel.take() {
            tunnel.stop();
        }
        if authenticated {
            self.refresh_port_forwards(ctx);
        } else {
            self.forwards.clear();
            self.forward_error = None;
            self.forward_busy = false;
            ctx.emit(SessionManagerEvent::Changed);
        }
    }

    pub fn refresh_port_forwards(&mut self, ctx: &mut ModelContext<Self>) {
        let device_id = AppSettings::device_id();
        ctx.spawn(
            async move { crate::realtime::rest::list_port_forwards(&device_id) },
            |this, result, mctx| {
                if let Ok(forwards) = result {
                    this.forwards = forwards
                        .into_iter()
                        .map(|f| PortForwardInfo {
                            id: f.id,
                            port: f.port,
                            url: f.public_url,
                        })
                        .collect();
                    mctx.emit(SessionManagerEvent::Changed);
                }
            },
        );
    }

    /// What "Anyone with the link" currently grants (None = link access off).
    pub fn link_access(&self) -> Option<AccessLevel> {
        self.session.as_ref().and_then(|s| s.link_access)
    }

    /// The role the email-invite field will grant (true = Edit).
    pub fn invite_control(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.invite_control)
    }

    /// Human label for when this session began.
    pub fn started_label(&self) -> &str {
        self.session
            .as_ref()
            .map(|s| s.started_label.as_str())
            .unwrap_or("")
    }

    /// The most recent connection error, if the session isn't online.
    pub fn last_error(&self) -> Option<&str> {
        self.session.as_ref().and_then(|s| s.last_error.as_deref())
    }

    pub fn session_id(&self) -> Option<String> {
        self.session.as_ref().and_then(|s| match &s.state {
            SessionState::HostLive { session, .. } | SessionState::ViewerLive { session, .. } => {
                Some(session.to_string())
            }
            _ => None,
        })
    }

    /// True when this workspace has joined a session as a viewer (not hosting).
    pub fn is_viewer(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| !s.controller.is_host())
    }

    /// Viewer: this participant's current role (View vs Controller/Edit). `None`
    /// when not a live viewer.
    pub fn viewer_role(&self) -> Option<dragent_session_protocol::Role> {
        self.session.as_ref().and_then(|s| match &s.state {
            SessionState::ViewerLive { self_role, .. } => Some(*self_role),
            _ => None,
        })
    }

    /// Viewer: whether this participant currently holds Edit (control).
    /// Host: the share action currently waiting on the network, if any.
    pub fn share_busy(&self) -> Option<crate::workspace::share_sheet::ShareOp> {
        self.share_busy
    }

    pub fn viewer_can_edit(&self) -> bool {
        self.viewer_role() == Some(dragent_session_protocol::Role::Controller)
    }

    /// Viewer: authenticated as the host's OWN account (e.g. the user's second
    /// device) — such a controller may drive the host's terminal size. A teammate
    /// (different account) sees the host's grid letterboxed instead.
    pub fn viewer_is_host_account(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.controller.viewer_is_host_account())
    }

    /// Start hosting a terminal tab. Its output should be forwarded via
    /// [`broadcast_delta`](Self::broadcast_delta) by the host output tap.
    pub fn start_host(
        &mut self,
        ctx: &mut ModelContext<Self>,
        pane: Uuid,
        tab_title: impl Into<String>,
    ) {
        self.stop_internal();
        log::info!(
            "[share] starting host session — opening gateway {}",
            crate::realtime::gateway_url()
        );
        let (tx, rx) = async_channel::unbounded::<RealtimeEvent>();
        let controller = SessionController::start_host(AppSettings::device_id(), tab_title, tx);
        self.subscribe(ctx, rx);
        let started = chrono::Local::now().format("%-I:%M%P on %m/%d").to_string();
        let mut session = ActiveSession::host(controller, started);
        session.shared_pane = Some(pane);
        self.session = Some(session);
        // Start the idle watchdog: the session auto-stops after IDLE_TIMEOUT with
        // no activity (covers "nobody ever joins" and "joined but went idle").
        self.start_idle_watchdog(ctx);
    }

    /// Host: share from a CLEAN screen — treat every block that already exists as
    /// already delivered, so the replay skips it and participants only ever see
    /// output produced from now on.
    ///
    /// The alternative (the default) replays the terminal's scrollback, which is
    /// the whole reason blocks are broadcast as snapshots: a viewer receives no
    /// bytes for anything that happened before it connected, so history can only
    /// reach it as the host's own rendering.
    pub fn suppress_scrollback(&mut self, block_ids: Vec<usize>) {
        if let Some(s) = self.session.as_mut() {
            for id in block_ids {
                s.share_cursor.started.insert(id);
                s.share_cursor.completed.insert(id);
            }
        }
    }

    /// Mark the hosted session as having just seen activity (terminal output,
    /// controller input, or a participant change), resetting the idle countdown.
    pub(crate) fn note_activity(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.last_activity = Instant::now();
        }
    }

    /// Begin the self-rescheduling idle watchdog for a hosted session. Bumps the
    /// generation so any older watchdog chain stops.
    fn start_idle_watchdog(&mut self, ctx: &mut ModelContext<Self>) {
        if !self.is_host() {
            return;
        }
        self.note_activity();
        self.idle_epoch += 1;
        let epoch = self.idle_epoch;
        ctx.spawn(Timer::after(IDLE_CHECK), move |this, _, mctx| {
            this.idle_watchdog(epoch, mctx);
        });
    }

    /// Periodic idle check driving the two-phase inactivity flow:
    ///   • idle ≥ `IDLE_WARN_AFTER` (10m) → show the idle-warning modal (once),
    ///   • idle ≥ `IDLE_WARN_AFTER + IDLE_GRACE` (15m) with no response → auto-end
    ///     "due to inactivity",
    ///   • activity resumed below the warning threshold → dismiss the warning.
    /// Reschedules itself; a generation mismatch (session restarted/stopped)
    /// silently ends the chain.
    fn idle_watchdog(&mut self, epoch: u64, ctx: &mut ModelContext<Self>) {
        let (elapsed, warned) = match self.session.as_ref() {
            Some(s) if self.idle_epoch == epoch && s.controller.is_host() => {
                (s.last_activity.elapsed(), s.idle_warned)
            }
            _ => return, // superseded, or no longer hosting — stop the chain
        };

        if elapsed >= IDLE_WARN_AFTER + IDLE_GRACE {
            log::info!(
                "[share] auto-ending shared session — no activity for {}s (no response to warning)",
                elapsed.as_secs()
            );
            self.end_with_reason(Some(INACTIVITY_END_REASON.to_string()), ctx);
            return;
        }

        if elapsed >= IDLE_WARN_AFTER && !warned {
            // Cross the warning threshold: prompt the host once.
            if let Some(s) = self.session.as_mut() {
                s.idle_warned = true;
            }
            ctx.emit(SessionManagerEvent::IdleWarning);
        } else if elapsed < IDLE_WARN_AFTER && warned {
            // Activity resumed on its own before the grace ran out — dismiss.
            if let Some(s) = self.session.as_mut() {
                s.idle_warned = false;
            }
            ctx.emit(SessionManagerEvent::IdleWarningCleared);
        }

        ctx.spawn(Timer::after(IDLE_CHECK), move |this, _, mctx| {
            this.idle_watchdog(epoch, mctx);
        });
    }

    /// A join link was opened (deep link). Emits [`SessionManagerEvent::JoinRequested`]
    /// so the workspace can open a mirror tab and connect — routing a URL callback
    /// (no view context) to the workspace through this singleton.
    pub fn request_join(&mut self, ctx: &mut ModelContext<Self>, session: String, secret: String) {
        ctx.emit(SessionManagerEvent::JoinRequested { session, secret });
    }

    /// The terminal pane whose output this host session broadcasts (if hosting).
    pub fn shared_pane(&self) -> Option<Uuid> {
        self.session.as_ref().and_then(|s| s.shared_pane)
    }

    /// Host: block ids already finalized (`BlockCompleted` sent) to viewers.
    /// Callers pass this to `block_share_snapshot` to skip re-cloning finished
    /// blocks on every output chunk.
    pub fn shared_finalized_blocks(&self) -> std::collections::HashSet<usize> {
        self.session
            .as_ref()
            .map(|s| s.share_cursor.completed.clone())
            .unwrap_or_default()
    }

    /// Host: broadcast a chunk of the shared pane's raw terminal output to
    /// viewers. Called from the terminal view's output-tap for the shared pane.
    ///
    /// A single PTY read can be large (a failed `yarn dev`, a build log, a `cat`
    /// of a big file). base64-encoded into ONE frame, a big burst overran the
    /// gateway's WebSocket read limit, which dropped the host's connection and
    /// tore the session down for viewers. Split the raw bytes into bounded pieces
    /// so no single output frame can ever exceed the limit — the viewer feeds
    /// them into its emulator sequentially, so chunk boundaries don't matter.
    pub fn broadcast_output(&mut self, block_id: Option<usize>, bytes: &[u8]) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        s.last_activity = Instant::now(); // terminal output = activity
        let block = block_id.map(|b| b.to_string()).unwrap_or_default();
        // 256 KiB raw → ~342 KiB base64, comfortably under the frame valve and the
        // gateway's 8 MiB read limit even with envelope overhead.
        for piece in bytes.chunks(256 * 1024) {
            s.output_seq += 1;
            s.controller.broadcast_delta(TerminalDelta::OutputChunk {
                block_id: block.clone(),
                seq: s.output_seq,
                data_base64: base64::engine::general_purpose::STANDARD.encode(piece),
            });
        }
    }

    /// Host: throttle decision for a live block snapshot broadcast. See
    /// [`ThrottleTick`]. Lifecycle broadcasts (start/complete/resync) bypass this.
    pub fn block_throttle_tick(&mut self, interval: Duration) -> ThrottleTick {
        self.session
            .as_mut()
            .map(|s| s.block_throttle.tick(interval))
            .unwrap_or(ThrottleTick::Skip)
    }

    /// Host: a trailing block broadcast timer fired — reset the throttle window.
    pub fn block_throttle_fire(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.block_throttle.fire();
        }
    }

    /// Host: throttle decision for an agent-block broadcast during streaming.
    pub fn agent_throttle_tick(&mut self, interval: Duration) -> ThrottleTick {
        self.session
            .as_mut()
            .map(|s| s.agent_throttle.tick(interval))
            .unwrap_or(ThrottleTick::Skip)
    }

    /// Host: a trailing agent broadcast timer fired — reset the throttle window.
    pub fn agent_throttle_fire(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.agent_throttle.fire();
        }
    }

    /// Host: broadcast the shared pane's block structure to viewers. Given the
    /// current block-list snapshot, emit `BlockStarted` for newly seen blocks,
    /// `BlockLines` for blocks whose styled output may have changed (running
    /// blocks, plus the final snapshot at completion), and `BlockCompleted` once
    /// each block finishes. Idempotent: unchanged completed blocks emit nothing.
    pub fn broadcast_blocks(&mut self, snapshot: Vec<HostBlockSnapshot>) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let mut deltas: Vec<TerminalDelta> = Vec::new();
        for block in &snapshot {
            let block_id = block.id.to_string();
            if s.share_cursor.started.insert(block.id) {
                deltas.push(TerminalDelta::BlockStarted {
                    block_id: block_id.clone(),
                    command: block.command.clone(),
                    cwd: Some(block.cwd.clone()),
                    ts_ms: block.created_at_unix_ms,
                });
            }
            // Re-send styled lines while running (output grows) and once more on
            // the tick the block completes. Skip blocks already finalized.
            let already_final = s.share_cursor.completed.contains(&block.id);
            if block.running || !already_final {
                deltas.push(TerminalDelta::BlockLines {
                    block_id: block_id.clone(),
                    base_line: 0,
                    lines: wire_lines(&block.lines),
                    // The host already resolved any directory listing from its own
                    // filesystem; send it so the viewer renders the same entries
                    // instead of scraping them back out of the text.
                    dir_entries: block
                        .dir_entries
                        .as_deref()
                        .map(crate::terminal::model::block_share::wire_dir_entries),
                });
            }
            if !block.running && s.share_cursor.completed.insert(block.id) {
                deltas.push(TerminalDelta::BlockCompleted {
                    block_id,
                    exit_code: block.exit_code,
                    duration_ms: block.duration_ms,
                    failed: block.failed,
                });
            }
        }
        if deltas.is_empty() {
            return;
        }
        s.last_activity = Instant::now();
        for delta in deltas {
            s.controller.broadcast_delta(delta);
        }
    }

    /// Host: broadcast a styled alt-screen grid snapshot to viewers (vim, htop,
    /// Claude Code, …). Full snapshot each call — the grid is bounded to
    /// rows×cols, so this stays cheap even when a full-screen program repaints.
    pub fn broadcast_alt_screen(
        &mut self,
        snapshot: crate::terminal::model::block_share::AltScreenSnapshot,
    ) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        s.last_activity = Instant::now();
        s.controller.broadcast_delta(TerminalDelta::AltScreen {
            rows: snapshot.rows,
            cols: snapshot.cols,
            cursor_row: snapshot.cursor_row,
            cursor_col: snapshot.cursor_col,
            cursor_visible: snapshot.cursor_visible,
            lines: wire_lines(&snapshot.lines),
        });
    }

    /// Host: broadcast the full agent-block list (AI conversation) to viewers.
    /// Sent as a complete snapshot — agent turns are small, so the viewer's
    /// reducer just replaces its list.
    pub fn broadcast_agent_blocks(
        &mut self,
        blocks: Vec<dragent_session_protocol::WireAgentBlock>,
    ) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        s.last_activity = Instant::now();
        s.controller
            .broadcast_delta(TerminalDelta::AgentBlocks { blocks });
    }
}
