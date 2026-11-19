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
use crate::settings::AppSettings;
use crate::terminal::model::block_share::{wire_lines, HostBlockSnapshot};

/// Host-only cursor tracking which blocks have been announced (`BlockStarted`)
/// and finalized (`BlockCompleted`) to viewers, so repeated snapshots only emit
/// the deltas that changed.
#[derive(Default)]

pub(crate) struct HostShareCursor {
    pub(crate) started: std::collections::HashSet<usize>,
    pub(crate) completed: std::collections::HashSet<usize>,
}

/// What a throttled live broadcaster should do on this call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThrottleTick {
    /// Enough time has elapsed — broadcast now (leading edge).
    Now,
    /// Within the throttle window and nothing is scheduled — the caller should
    /// arm a trailing broadcast after this delay so the final state isn't lost.
    Arm(Duration),
    /// Within the window and a trailing broadcast is already armed — do nothing.
    Skip,
}

/// Leading + trailing edge throttle. `tick` returns `Now` at most once per
/// `interval`; when it returns `Arm` the caller schedules a trailing broadcast
/// and later calls `fire`, guaranteeing the last update in a burst is delivered.
pub(crate) struct ThrottleState {
    last: Instant,
    armed: bool,
}

impl Default for ThrottleState {
    fn default() -> Self {
        // Start "stale" so the first broadcast of any burst (incl. sharing that
        // starts while output/an agent turn is already in flight) fires
        // immediately on the leading edge rather than waiting out a window.
        Self {
            last: Instant::now()
                .checked_sub(Duration::from_secs(3600))
                .unwrap_or_else(Instant::now),
            armed: false,
        }
    }
}

impl ThrottleState {
    pub(crate) fn tick(&mut self, interval: Duration) -> ThrottleTick {
        let elapsed = self.last.elapsed();
        if elapsed >= interval {
            self.last = Instant::now();
            ThrottleTick::Now
        } else if !self.armed {
            self.armed = true;
            ThrottleTick::Arm(interval - elapsed)
        } else {
            ThrottleTick::Skip
        }
    }

    /// The trailing timer fired: clear the armed flag and reset the window.
    pub(crate) fn fire(&mut self) {
        self.armed = false;
        self.last = Instant::now();
    }
}

/// Map an access level to the wire string the REST endpoints expect.
pub(crate) fn access_str(a: AccessLevel) -> &'static str {
    match a {
        AccessLevel::Control => crate::realtime::rest::ACCESS_EDIT,
        _ => crate::realtime::rest::ACCESS_VIEW,
    }
}

pub(crate) fn access_str_for(control: bool) -> &'static str {
    if control {
        crate::realtime::rest::ACCESS_EDIT
    } else {
        crate::realtime::rest::ACCESS_VIEW
    }
}

/// After this long with NO ACTIVITY — no terminal output, no controller input,
/// no participant change — a hosted session prompts the user (via an idle-warning
/// modal) to keep it alive or end it. A session nobody is using shouldn't hold a
/// gateway connection open indefinitely.
pub(crate) const IDLE_WARN_AFTER: Duration = Duration::from_secs(600); // 10 minutes
/// Grace period after the idle warning with still no activity (and no response)
/// before the session auto-ends "due to inactivity".
pub(crate) const IDLE_GRACE: Duration = Duration::from_secs(300); // 5 minutes
/// How often the idle watchdog wakes to check the elapsed-since-activity time.
pub(crate) const IDLE_CHECK: Duration = Duration::from_secs(30);
/// End reason shown to viewers when the session auto-ends from inactivity.
pub(crate) const INACTIVITY_END_REASON: &str = "Session ended due to inactivity";

/// Events emitted to interested views.
#[derive(Debug, Clone)]
pub enum SessionManagerEvent {
    /// The session state, participants, connectivity, or mirror content changed.
    Changed,
    /// Host: run this command locally now — it came from the host's own account
    /// and was auto-approved by the server (no prompt).
    RunCommand { request_id: Uuid, command: String },
    /// Host: execute a controller-submitted agent prompt on the shared pane.
    RunAgentQuery {
        request_id: Uuid,
        from: ParticipantId,
        query: String,
    },
    /// Host: stop the shared pane's active agent turn.
    StopAgent,
    /// Host: a remote controller needs approval to run a command.
    ApprovalRequested {
        request_id: Uuid,
        from: ParticipantId,
        command: String,
    },
    /// Host: raw input bytes from a controller — write straight to the shared
    /// pane's PTY (typed keys + alt-screen keystrokes: Claude Code, vim, …).
    RunInput { bytes: Vec<u8> },
    /// Host: a controller asked to resize the shared PTY to `rows`×`cols`, so a
    /// TUI re-renders to fill the viewer's window. The shared pane applies it.
    RunResize { rows: u16, cols: u16 },
    /// Host: a controller's live input/scroll surface — the shared pane reflects
    /// it on the host's own command editor / AI-tool input / scroll position.
    ApplyControlSurface {
        from: dragent_session_protocol::ParticipantId,
        surface: dragent_session_protocol::ControlSurface,
    },
    /// Host: update a participant's collaborative pointer.
    RemotePointerMoved {
        from: dragent_session_protocol::ParticipantId,
        position: dragent_session_protocol::PointerPosition,
    },
    /// Host: a loopback port has been made available to viewers.
    ForwardCreated {
        forward_id: String,
        public_url: String,
    },
    ForwardOpFinished {
        notice: String,
        error: Option<String>,
    },
    /// Host: a view-only participant asked for Edit — prompt to approve/decline.
    AccessRequested {
        participant: ParticipantId,
        display_name: String,
    },
    /// Viewer: this participant's Edit request was declined by the host.
    AccessDenied,
    /// Viewer: a chunk of the host's terminal output to feed into the mirror
    /// view (raw PTY bytes — the mirror `TerminalView` renders them identically).
    MirrorOutput { bytes: Vec<u8> },
    /// Viewer: one of the host's authoritative BLOCK snapshots (start / styled
    /// lines / completion). The mirror renders these instead of re-parsing the
    /// raw byte stream, so its view cannot drift from the host's.
    MirrorBlock { delta: TerminalDelta },
    /// Viewer: authoritative agent conversation snapshot from the host.
    MirrorAgentBlocks {
        blocks: Vec<dragent_session_protocol::WireAgentBlock>,
    },
    /// Viewer: the host's input-layer surface state (which AI-tool panel is up,
    /// its text, running state) — applied to the mirror so its input bar matches
    /// the host's. PTY bytes alone can't reconstruct this.
    MirrorSurface { state: TerminalSurfaceState },
    /// Host: a share action that hit the network finished. `error` is `None` on
    /// success. The share sheet uses this to drop its spinner and show a timed
    /// confirmation, so every networked control acknowledges itself.
    ShareOpFinished {
        op: crate::workspace::share_sheet::ShareOp,
        error: Option<String>,
    },
    /// A join link was opened — the workspace should open a mirror tab + connect.
    JoinRequested { session: String, secret: String },
    /// Host: a new participant joined — the shared pane should re-broadcast its
    /// full current state (blocks, agent blocks, surface) so the late joiner
    /// sees the session so far, not a blank screen.
    ResyncViewers,
    /// Host: the session has been idle for [`IDLE_WARN_AFTER`] — prompt the user
    /// to keep it or end it. Auto-ends after [`IDLE_GRACE`] more with no response.
    IdleWarning,
    /// Host: the idle warning no longer applies (activity resumed, or the user
    /// chose to keep the session) — dismiss the prompt.
    IdleWarningCleared,
    /// The session ended.
    Ended(String),
    /// A non-fatal error to surface.
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForwardInfo {
    pub id: String,
    pub port: u16,
    pub url: String,
}

pub(crate) struct ActiveSession {
    pub(crate) controller: SessionController,
    pub(crate) online: bool,
    pub(crate) state: SessionState,
    pub(crate) participants: Vec<Participant>,
    pub(crate) pairing_code: Option<String>,
    /// Host-only: the current shareable link URL (present when link access is on).
    pub(crate) share_link: Option<String>,
    /// Host-only: the token backing `share_link`, kept so it can be revoked.
    pub(crate) link_token: Option<String>,
    /// Host-only: what "Anyone with the link" grants (None = link access off).
    pub(crate) link_access: Option<AccessLevel>,
    pub(crate) forward_websockets:
        std::collections::HashMap<String, Sender<ForwardWebSocketCommand>>,
    /// Host-only: the role the email-invite field will grant (true = Edit).
    pub(crate) invite_control: bool,
    /// Host-only: human label for when the session began (e.g. "9:07pm on 07/23").
    pub(crate) started_label: String,
    /// The most recent connection/transport error, cleared once online.
    pub(crate) last_error: Option<String>,
    /// Host-only: the terminal pane whose output is broadcast to viewers.
    pub(crate) shared_pane: Option<Uuid>,
    /// Host-only: monotonically increasing sequence for output chunks (ordering).
    pub(crate) output_seq: u64,
    /// Host-only: tracks which blocks have been announced/finalized to viewers so
    /// [`broadcast_blocks`](SessionManager::broadcast_blocks) only sends what changed.
    pub(crate) share_cursor: HostShareCursor,
    /// Host-only: throttle for the per-output-chunk running-block snapshot (which
    /// clones the block's whole scrollback). Lifecycle events bypass this.
    pub(crate) block_throttle: ThrottleState,
    /// Host-only: throttle for agent-block broadcasts during streaming (the whole
    /// agent block list is re-serialized on every token otherwise).
    pub(crate) agent_throttle: ThrottleState,
    /// Host-only: the last time this session saw ANY activity (terminal output,
    /// controller input, or a participant change). Drives the idle auto-stop.
    pub(crate) last_activity: Instant,
    /// Host-only: whether the idle-warning modal is currently showing (edge state
    /// so the watchdog only emits the warning/clear events on transitions).
    pub(crate) idle_warned: bool,
}

pub(crate) enum ForwardWebSocketCommand {
    Message { message_type: String, data: Vec<u8> },
    Close,
}

pub struct SessionManager {
    pub(crate) session: Option<ActiveSession>,
    pub(crate) forwards: Vec<PortForwardInfo>,
    pub(crate) forward_error: Option<String>,
    pub(crate) forward_busy: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) _forward_tunnel: Option<crate::realtime::forward_tunnel::ForwardTunnelHandle>,
    /// Monotonic generation for the idle watchdog. Bumped when a watchdog is
    /// (re)started so any older watchdog chain becomes a no-op — survives session
    /// restarts (never resets), so a stale watchdog can't attach to a new session.
    pub(crate) idle_epoch: u64,
    /// Host-side: a share action currently waiting on the network, so the sheet
    /// can show which control is busy.
    pub(crate) share_busy: Option<crate::workspace::share_sheet::ShareOp>,
    /// Viewer-side: the host's theme id, from the latest `SurfaceState` delta.
    /// The web viewer applies `Theme::from_id` so its surface matches the host.
    pub(crate) viewer_theme_id: Option<String>,
}

impl Entity for SessionManager {
    type Event = SessionManagerEvent;
}
impl SingletonEntity for SessionManager {}

impl ActiveSession {
    pub(crate) fn host(controller: SessionController, started_label: String) -> Self {
        Self {
            controller,
            online: false,
            state: SessionState::Connecting,
            participants: Vec::new(),
            pairing_code: None,
            share_link: None,
            link_token: None,
            link_access: None,
            forward_websockets: std::collections::HashMap::new(),
            invite_control: false,
            started_label,
            last_error: None,
            shared_pane: None,
            output_seq: 0,
            share_cursor: HostShareCursor::default(),
            block_throttle: ThrottleState::default(),
            agent_throttle: ThrottleState::default(),
            last_activity: Instant::now(),
            idle_warned: false,
        }
    }

    pub(crate) fn viewer(controller: SessionController) -> Self {
        Self {
            controller,
            online: false,
            state: SessionState::Connecting,
            participants: Vec::new(),
            pairing_code: None,
            share_link: None,
            link_token: None,
            link_access: None,
            forward_websockets: std::collections::HashMap::new(),
            invite_control: false,
            started_label: String::new(),
            last_error: None,
            shared_pane: None,
            output_seq: 0,
            share_cursor: HostShareCursor::default(),
            block_throttle: ThrottleState::default(),
            agent_throttle: ThrottleState::default(),
            last_activity: Instant::now(),
            idle_warned: false,
        }
    }
}
pub(crate) use super::session_events::*;
pub(crate) use super::session_host::*;
pub(crate) use super::session_viewer::*;
