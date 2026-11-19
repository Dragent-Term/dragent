//! Wire message types for Dragent's remote / shared terminal sessions.
//!
//! A shared session has one **host** (the desktop that owns the real PTY and
//! broadcasts output) and any number of **participants** (viewers, or controllers
//! that may request to run commands). Messages flow host↔server and viewer↔server
//! over a WebSocket; the server relays between them. Everything here is
//! serialized to/from JSON.
//!
//! ## Design note (clean-room)
//! These types are Dragent's own. No code is copied from Warp's AGPL
//! `session-sharing-protocol` crate — that crate is intentionally not a
//! dependency. Only the general host/viewer + ordered-terminal-event *shape*
//! informed this schema.

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod workspace;
mod workspace_commands;
mod workspace_error;
mod workspace_events;
#[cfg(test)]
mod workspace_tests;

/// Protocol version. Bump on any incompatible wire change; the server and client
/// exchange it during connect so mismatches fail fast.
pub const PROTOCOL_VERSION: u32 = 2;

// ── Identifiers ──────────────────────────────────────────────────────────────

/// A shared session's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A participant's id within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParticipantId(pub Uuid);

impl ParticipantId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ParticipantId {
    fn default() -> Self {
        Self::new()
    }
}

/// A capability token that grants access to a session. Anyone holding the secret
/// can join, so its `Debug`/`Display` are **masked** to keep it out of logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSecret(String);

impl SessionSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// The underlying token. Use only where the raw value is genuinely needed
    /// (e.g. building a join URL); never log the result.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionSecret(***)")
    }
}

impl fmt::Display for SessionSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A token that lets a host reattach to its session after a dropped connection
/// without re-pairing. Masked like [`SessionSecret`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectToken(String);

impl ReconnectToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ReconnectToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReconnectToken(***)")
    }
}

// ── Roles & participants ─────────────────────────────────────────────────────

/// A participant's role in the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The desktop that owns the PTY.
    Host,
    /// May request to run commands (subject to host approval / rate limits).
    Controller,
    /// Read-only: sees output, cannot run anything.
    Viewer,
}

/// The access a participant is granted. `ViewOnly` is control disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    Control,
    ViewOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub id: ParticipantId,
    pub display_name: String,
    /// Profile photo URL when the participant has an account; empty means
    /// clients should fall back to an initials avatar.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
    pub role: Role,
    #[serde(default)]
    pub present: bool,
}

/// Serializable presentation state for the shared terminal surface.
///
/// PTY bytes are not enough to reconstruct Dragent's UI: the input editor,
/// ghost completion, AI-tool editor, and transient tool chrome are all state
/// owned by `TerminalView`. Hosts publish this alongside output so a WASM
/// mirror can render the same surface without reimplementing host heuristics.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalSurfaceState {
    #[serde(default)]
    pub command_input: String,
    #[serde(default)]
    pub command_ghost: Option<String>,
    #[serde(default)]
    pub command_cursor: usize,
    #[serde(default)]
    pub ai_tool: Option<String>,
    #[serde(default)]
    pub ai_input: String,
    #[serde(default)]
    pub ai_ghost: Option<String>,
    #[serde(default)]
    pub ai_cursor: usize,
    #[serde(default)]
    pub ai_status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub activity: Option<String>,
    #[serde(default)]
    pub side_command: Option<String>,
    #[serde(default)]
    pub command_running: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    /// The host's active theme id (e.g. "dark", "midnight-photo"). Viewers apply
    /// `Theme::from_id` so the terminal surface — background, colours, and
    /// **bundled image backgrounds** — matches the host live. Bundled theme
    /// assets are embedded in the viewer, so only the id needs to travel.
    #[serde(default)]
    pub theme_id: Option<String>,
    /// The host terminal's grid dimensions. A viewer's mirror re-parses the raw
    /// PTY byte stream, which is laid out for *these* rows×cols (a TUI positions
    /// text with absolute cursor moves for one specific grid) — so the mirror
    /// must size its emulator to match, or the same renderer draws a different,
    /// misaligned grid. `None` from an older host leaves the mirror self-sizing.
    #[serde(default)]
    pub rows: Option<u16>,
    #[serde(default)]
    pub cols: Option<u16>,
    /// The host's block-list scroll position as a fraction of its scrollable
    /// range: `0.0` = scrolled to the top, `1.0` = at the bottom. `None` means
    /// the host is following the bottom (auto-scroll) — the viewer should pin to
    /// the bottom and keep following new output. A fraction (rather than a pixel
    /// offset) is height-independent, so a viewer whose window / font differ from
    /// the host still lands at the same relative place. Viewers apply this only
    /// when they are NOT the one driving the scroll.
    #[serde(default)]
    pub scroll_fraction: Option<f32>,
}

// ── Ordered terminal events (host → viewers) ─────────────────────────────────

/// An 8-bit RGBA color already resolved against the host's theme, so viewers
/// paint the exact pixels the desktop shows without re-resolving ANSI palettes.
pub type WireColor = [u8; 4];

/// SGR attribute bits carried alongside a span (bold / italic / underline …).
/// Matches the desktop emulator's `Flags::SGR_MASK` bit layout so a viewer can
/// interpret them identically.
pub type WireFlags = u16;

/// One styled run of text within a terminal line. Mirrors the desktop
/// `TextSpan { text, color, bg }`, with colors resolved to RGBA.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireSpan {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<WireColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<WireColor>,
    #[serde(default, skip_serializing_if = "is_zero_flags")]
    pub flags: WireFlags,
}

fn is_zero_flags(flags: &WireFlags) -> bool {
    *flags == 0
}

/// One entry of a directory listing, resolved by the HOST.
///
/// A viewer cannot derive this itself: the desktop builds a listing from its own
/// filesystem (the reliable source), and a mirror is a different machine — its
/// filesystem is either absent (web) or, worse, someone else's. Falling back to
/// scraping the `ls` text loses entries, because that output is a width-formatted
/// grid and names may contain spaces. So the host sends what it resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDirEntry {
    pub name: String,
    /// `directory` | `executable` | `file` | `symlink`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Colour the host resolved for this entry, RGBA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<WireColor>,
}

/// One rendered output line — the wire form of the desktop `ColoredLine`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireLine {
    pub spans: Vec<WireSpan>,
}

// ── Agent blocks (host → viewers) ────────────────────────────────────────────

/// One line of a rendered diff (mirrors the desktop `DiffLine`). `kind` is one of
/// `context` / `add` / `remove`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDiffLine {
    pub kind: String,
    pub text: String,
}

/// A per-file change made by an edit tool (mirrors the desktop `FileDiff`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<WireDiffLine>,
    #[serde(default)]
    pub truncated: bool,
}

/// All file changes captured for one tool call.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDiff {
    pub files: Vec<WireFileDiff>,
}

/// A read-only projection of one agent block for viewers. The desktop
/// `AgentBlockKind` is far richer (interactive approval/question/plan state); a
/// viewer only *displays* the turn, so this carries just the presentation fields.
/// Status enums are flattened to lowercase strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireAgentAuthor {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireAgentKind {
    UserQuery {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<WireAgentAuthor>,
    },
    AgentMessage {
        text: String,
        #[serde(default)]
        streaming: bool,
    },
    SystemNote {
        text: String,
    },
    Thinking {
        content: String,
        #[serde(default)]
        complete: bool,
    },
    ToolStep {
        name: String,
        icon: String,
        args: String,
        /// `running` / `done` / `error`.
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<WireDiff>,
    },
    ActionResult {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        output_preview: String,
    },
    Approval {
        command: String,
        reason: String,
        /// `pending` / `approved` / `approved_always` / `rejected`.
        status: String,
    },
    Question {
        header: String,
        question: String,
        options: Vec<String>,
        /// `pending` / `answered` / `dismissed`.
        status: String,
    },
    PlanReview {
        content: String,
        /// `pending` / `approved` / `refining`.
        status: String,
    },
    Queued {
        text: String,
    },
    Loading {
        message: String,
    },
    Error {
        message: String,
    },
    TurnGroup {
        children: Vec<WireAgentBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_note: Option<String>,
        #[serde(default)]
        expanded: bool,
    },
    ActivityGroup {
        blocks: Vec<WireAgentBlock>,
        #[serde(default)]
        expanded: bool,
    },
    Empty,
}

/// One agent block on the wire: a stable id plus its display projection.
/// `ts_ms` is the block's wall-clock creation time (unix ms), used by viewers to
/// interleave agent blocks with shell command blocks in one timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireAgentBlock {
    pub id: usize,
    #[serde(default)]
    pub ts_ms: i64,
    #[serde(flatten)]
    pub kind: WireAgentKind,
}

/// One ordered change to the shared terminal. Block lifecycle events bracket the
/// styled output of each command so viewers reconstruct the desktop's block-card
/// UI (not a flat byte grid). `OutputChunk` is reserved for full-screen
/// alt-screen mirroring (vim, htop, …) where the block model does not apply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalDelta {
    /// Replaces the non-PTY presentation state of the shared terminal.
    SurfaceState { state: TerminalSurfaceState },
    /// A new command block started (a fresh card appears, marked running).
    /// `ts_ms` is the wall-clock time (unix ms) the host announced the block, so
    /// viewers can interleave it with agent blocks in one timeline.
    BlockStarted {
        block_id: String,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        ts_ms: i64,
    },
    /// The current styled output of a block. `base_line` is the index of the
    /// first line in `lines`; a viewer replaces its rows `[base_line, base_line +
    /// lines.len())` with these, growing the block as needed. Sent incrementally
    /// while running and once more with the final snapshot on completion.
    BlockLines {
        block_id: String,
        base_line: usize,
        lines: Vec<WireLine>,
        /// Present when this block's output IS a directory listing, resolved by
        /// the host. Viewers render these instead of re-parsing `lines`, which
        /// cannot be done reliably (see [`WireDirEntry`]). `#[serde(default)]`
        /// so an older host that omits it falls back to the text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir_entries: Option<Vec<WireDirEntry>>,
    },
    /// A command block finished.
    BlockCompleted {
        block_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Whether the host classified this exit as a genuine failure (mirrors
        /// `BlockContext::failed`, which ignores signal + informational exits).
        #[serde(default)]
        failed: bool,
    },
    /// Raw PTY output (base64-encoded by the sender). This is the byte-stream
    /// representation a mirror that re-runs the VT parser consumes; a viewer that
    /// renders styled snapshots (block list + [`AltScreen`](Self::AltScreen))
    /// ignores it.
    OutputChunk {
        block_id: String,
        seq: u64,
        data_base64: String,
    },
    /// A full styled snapshot of the alt-screen grid (vim, htop, Claude Code, …).
    /// Sent while an alt-screen program owns the terminal; the host resolves ANSI
    /// colors against its theme so viewers paint the exact cells. `lines` has one
    /// entry per visible row; `cursor_*` places the block/insert cursor.
    AltScreen {
        rows: usize,
        cols: usize,
        cursor_row: usize,
        cursor_col: usize,
        cursor_visible: bool,
        lines: Vec<WireLine>,
    },
    /// The alt-screen program exited; viewers return to the block-list view.
    AltScreenEnded,
    /// The full ordered list of agent blocks (the AI conversation turns). Sent as
    /// a complete snapshot whenever the agent block list changes — agent turns are
    /// small and this keeps the viewer's reducer trivial.
    AgentBlocks { blocks: Vec<WireAgentBlock> },
}

// ── Host ↔ server ────────────────────────────────────────────────────────────

/// Messages the host sends to the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToServer {
    /// Open a new shared session for a terminal tab.
    Initialize {
        protocol_version: u32,
        device_id: String,
        tab_title: String,
    },
    /// Reattach to an existing session after a dropped connection.
    Reconnect {
        session: SessionId,
        token: ReconnectToken,
    },
    /// Broadcast an ordered terminal change to participants.
    Delta {
        delta: TerminalDelta,
    },
    /// Change a participant's access.
    SetParticipantRole {
        participant: ParticipantId,
        access: AccessLevel,
    },
    /// Decline a participant's pending request for Edit (control). Approval is
    /// expressed by [`SetParticipantRole`]; this notifies the requester their ask
    /// was declined (they stay view-only).
    DenyAccessRequest {
        participant: ParticipantId,
    },
    /// Remove a participant from the session entirely (revoke access). They are
    /// disconnected with a [`ServerToViewer::Removed`].
    RemoveParticipant {
        participant: ParticipantId,
    },
    /// Approve or deny a pending remote-command request.
    ResolveCommand {
        request_id: Uuid,
        approved: bool,
    },
    /// End the session for everyone. `reason` (when set) is shown to viewers as
    /// the end message — e.g. "Session ended due to inactivity". Absent ends with
    /// the default host-left message.
    EndSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Ask the gateway to expose a loopback HTTP service from this desktop.
    /// The desktop never accepts inbound connections; it only dials the
    /// approved loopback port when the gateway sends a request.
    ForwardCreate {
        port: u16,
        label: String,
        /// New clients carry forwarding traffic on the dedicated binary tunnel.
        /// Absent on older clients, which the gateway may serve through the
        /// legacy session-socket compatibility path during rollout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tunnel_version: Option<u8>,
    },
    /// Stop exposing a previously-created forward.
    ForwardClose {
        forward_id: String,
    },
    /// Response bytes for one forwarded HTTP request. Bodies are base64 because
    /// the session control socket is text-only.
    ForwardResponse {
        request_id: String,
        status: u16,
        headers: Vec<(String, String)>,
        body_base64: String,
    },
    /// Acknowledge that the desktop opened a loopback byte stream for a browser
    /// forward. The stream transports every HTTP connection, not only WebSocket
    /// upgrades, so response bodies can remain streaming and bidirectional.
    ForwardWebSocketOpened {
        request_id: String,
    },
    /// Raw bytes for the forwarded connection. `message_type` is retained for
    /// wire compatibility and is always `data` for new clients.
    ForwardWebSocketMessage {
        request_id: String,
        message_type: String,
        data_base64: String,
    },
    ForwardWebSocketClosed {
        request_id: String,
    },
    Ping,
}

/// Messages the server sends to the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToHost {
    /// Session created. `pairing_code` (own-device) may be shown as text/QR.
    Initialized {
        session: SessionId,
        pairing_code: Option<String>,
        reconnect_token: ReconnectToken,
    },
    Reconnected,
    ParticipantsUpdated {
        participants: Vec<Participant>,
    },
    /// A controller wants to run `command`. If `auto_approved` is true the
    /// request came from the host's OWN account (e.g. the user's other device),
    /// so the host runs it immediately without prompting; otherwise the host must
    /// approve via `ResolveCommand`.
    CommandRequested {
        request_id: Uuid,
        from: ParticipantId,
        command: String,
        #[serde(default)]
        auto_approved: bool,
    },
    /// A controller submitted an agent prompt. The host runs it through its
    /// existing AgentController, so local tools, permissions, and billing stay
    /// owned by the host environment.
    AgentQueryRequested {
        request_id: Uuid,
        from: ParticipantId,
        query: String,
    },
    /// A controller asked to stop the currently-running agent turn.
    AgentStopRequested {
        from: ParticipantId,
    },
    /// Raw input bytes from a controller to write to the host's PTY — covers
    /// typed commands AND alt-screen keystrokes (driving Claude Code, vim, …).
    /// Forwarded only for participants that hold Control (Edit).
    InputReceived {
        from: ParticipantId,
        data_base64: String,
    },
    /// A controller asked the host to resize its shared PTY to the given grid, so
    /// a TUI re-renders to fill the viewer's window. Forwarded only for Edit.
    ResizeRequested {
        from: ParticipantId,
        rows: u16,
        cols: u16,
    },
    /// A controller's live input/scroll surface to apply to the host's own
    /// editors + scroll (the mirror of `SurfaceState`). Forwarded only for Edit.
    ControlReceived {
        from: ParticipantId,
        surface: ControlSurface,
    },
    /// A participant moved their pointer in the shared window. This presence
    /// signal is available to viewers and controllers alike; it grants no edit
    /// capability.
    PointerMoved {
        from: ParticipantId,
        position: PointerPosition,
    },
    /// A view-only participant asked to be upgraded to Edit (control). The host
    /// approves/declines via [`HostToServer::SetParticipantRole`].
    AccessRequested {
        participant: ParticipantId,
        display_name: String,
    },
    /// The gateway accepted a forward request and assigned its public id.
    ForwardCreated {
        forward_id: String,
        port: u16,
        public_url: String,
    },
    /// A viewer request that the host should satisfy by dialing 127.0.0.1.
    ForwardRequest {
        forward_id: String,
        port: u16,
        request_id: String,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body_base64: String,
    },
    /// Open a raw byte stream to the approved loopback port. The initial HTTP
    /// request is supplied separately; every byte after that is relayed as-is.
    ForwardWebSocketOpen {
        forward_id: String,
        port: u16,
        request_id: String,
        #[serde(default = "default_forward_stream_method")]
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        #[serde(default)]
        body_base64: String,
    },
    /// Raw bytes for the forwarded connection; no WebSocket frames are parsed.
    ForwardWebSocketMessage {
        request_id: String,
        message_type: String,
        data_base64: String,
    },
    ForwardWebSocketClose {
        request_id: String,
    },
    ForwardClosed {
        forward_id: String,
    },
    ForwardError {
        reason: String,
    },
    Terminated {
        reason: String,
    },
    Pong,
}

fn default_forward_stream_method() -> String {
    "GET".to_string()
}

/// A scroll update carried in a [`ControlSurface`]. Its PRESENCE (the surface's
/// `scroll` field is `Some`) means "the controller changed the scroll"; the inner
/// `fraction` is `None` when the controller is following the bottom (auto-scroll),
/// or `Some(0.0..=1.0)` for an absolute position. This two-level option lets a
/// message that only carries a text edit omit scroll entirely.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScrollUpdate {
    #[serde(default)]
    pub fraction: Option<f32>,
}

/// A participant pointer position, normalized to the shared window. Normalized
/// coordinates keep the cursor aligned when the browser viewer and host desktop
/// have different window sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PointerPosition {
    pub x: f32,
    pub y: f32,
}

/// The live input/scroll surface a CONTROLLER (Edit) pushes UP to the host, so
/// the host desktop reflects what the web participant is doing in lock-step —
/// the mirror image of [`TerminalSurfaceState`] (host → viewers). Every field is
/// optional so a single keystroke or scroll only carries what changed: the
/// active editor's text+cursor, and/or a scroll update. The host applies present
/// fields to its own command editor / AI-tool editor / scroll position.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_cursor: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_cursor: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll: Option<ScrollUpdate>,
}

// ── Viewer ↔ server ──────────────────────────────────────────────────────────

/// Messages a viewer/controller sends to the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewerToServer {
    /// Join a session with a per-link capability secret. The server maps the
    /// secret to a role (Edit vs View); an Edit secret additionally requires the
    /// connection to be authenticated.
    Join {
        protocol_version: u32,
        session: SessionId,
        secret: SessionSecret,
        device_id: String,
        display_name: String,
    },
    /// Ask the host to be upgraded to a controller.
    RequestControl,
    /// Request to run a command (only honored if granted control).
    ExecuteCommand {
        command: String,
    },
    /// Submit an agent prompt to the host desktop. `request_id` makes retries
    /// after reconnect idempotent so one click cannot create two billed tasks.
    AgentQuery {
        request_id: Uuid,
        query: String,
    },
    /// Stop the host's current agent turn without terminating detached servers.
    AgentStop,
    /// Raw input bytes to write to the host's PTY (typed keys, alt-screen
    /// keystrokes). Only forwarded if the sender holds Control (Edit).
    InputBytes {
        data_base64: String,
    },
    /// Ask the host to resize its shared PTY to the viewer's terminal dimensions,
    /// so a full-screen TUI re-renders to fill the viewer's window exactly. Only
    /// honored for a controller (Edit); the host's own terminal follows suit.
    Resize {
        rows: u16,
        cols: u16,
    },
    /// A controller's live input/scroll surface — what it's typing in the command
    /// line / AI-tool input and where it has scrolled — so the host desktop
    /// reflects it live. Only honored for a controller (Edit).
    Control {
        surface: ControlSurface,
    },
    /// Publish this participant's pointer without requesting Edit access.
    Pointer {
        position: PointerPosition,
    },
    /// Request a forwarded HTTP resource through a host-created forward.
    ForwardRequest {
        forward_id: String,
        request_id: String,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body_base64: String,
    },
    Ping,
}

/// Messages the server sends to a viewer/controller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToViewer {
    Joined {
        participant: ParticipantId,
        role: Role,
        /// True when this viewer authenticated as the HOST's own account (e.g. the
        /// user opened their own session from a second device). Such a controller
        /// may drive the host's terminal SIZE (the host resizes to match); a
        /// different user (a teammate) may not — they see the host's grid
        /// letterboxed to their window instead. `#[serde(default)]` so older
        /// servers that omit it read as `false` (safe: host-authoritative).
        #[serde(default)]
        is_host_account: bool,
    },
    Delta {
        delta: TerminalDelta,
    },
    ParticipantsUpdated {
        participants: Vec<Participant>,
    },
    RoleChanged {
        role: Role,
    },
    /// This viewer's request for Edit was declined by the host (stays view-only).
    AccessDenied,
    Removed {
        reason: String,
    },
    ForwardResponse {
        request_id: String,
        status: u16,
        headers: Vec<(String, String)>,
        body_base64: String,
    },
    ForwardError {
        request_id: String,
        reason: String,
    },
    Ended {
        reason: String,
    },
    Pong,
}

// ── Codec ────────────────────────────────────────────────────────────────────

/// JSON encode/decode helpers, implemented for every message type.
pub trait Message: Serialize + DeserializeOwned + Sized {
    fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
    fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

impl<T: Serialize + DeserializeOwned> Message for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Message + PartialEq + std::fmt::Debug>(value: T) {
        let json = value.to_json().expect("encode");
        let back = T::from_json(&json).expect("decode");
        assert_eq!(value, back, "round-trip mismatch via {json}");
    }

    #[test]
    fn host_messages_round_trip() {
        round_trip(HostToServer::Initialize {
            protocol_version: PROTOCOL_VERSION,
            device_id: "dev-123".into(),
            tab_title: "zsh — ~/dev".into(),
        });
        round_trip(HostToServer::Delta {
            delta: TerminalDelta::OutputChunk {
                block_id: "b1".into(),
                seq: 7,
                data_base64: "aGVsbG8=".into(),
            },
        });
        round_trip(HostToServer::ResolveCommand {
            request_id: Uuid::new_v4(),
            approved: true,
        });
        round_trip(HostToServer::DenyAccessRequest {
            participant: ParticipantId::new(),
        });
        round_trip(HostToServer::RemoveParticipant {
            participant: ParticipantId::new(),
        });
        round_trip(HostToServer::EndSession { reason: None });
        round_trip(HostToServer::EndSession {
            reason: Some("Session ended due to inactivity".into()),
        });
        round_trip(HostToServer::Ping);
    }

    #[test]
    fn server_and_viewer_messages_round_trip() {
        let pid = ParticipantId::new();
        round_trip(ServerToHost::Initialized {
            session: SessionId::new(),
            pairing_code: Some("ABCD-1234".into()),
            reconnect_token: ReconnectToken::new("tok"),
        });
        round_trip(ServerToHost::CommandRequested {
            request_id: Uuid::new_v4(),
            from: pid,
            command: "ls -la".into(),
            auto_approved: false,
        });
        let agent_request = Uuid::new_v4();
        round_trip(ServerToHost::AgentQueryRequested {
            request_id: agent_request,
            from: pid,
            query: "fix the failing test".into(),
        });
        round_trip(ServerToHost::AgentStopRequested { from: pid });
        round_trip(ViewerToServer::Join {
            protocol_version: PROTOCOL_VERSION,
            session: SessionId::new(),
            secret: SessionSecret::new("s3cr3t"),
            device_id: "dev-9".into(),
            display_name: "Phone".into(),
        });
        round_trip(ViewerToServer::ExecuteCommand {
            command: "git status".into(),
        });
        round_trip(ViewerToServer::AgentQuery {
            request_id: agent_request,
            query: "fix the failing test".into(),
        });
        round_trip(ViewerToServer::AgentStop);
        round_trip(ViewerToServer::InputBytes {
            data_base64: "GWhpCg==".into(),
        });
        round_trip(ServerToHost::InputReceived {
            from: pid,
            data_base64: "GWhpCg==".into(),
        });
        round_trip(ViewerToServer::Control {
            surface: ControlSurface {
                command_input: Some("git ".into()),
                command_cursor: Some(4),
                scroll: Some(ScrollUpdate {
                    fraction: Some(0.5),
                }),
                ..Default::default()
            },
        });
        round_trip(ViewerToServer::Pointer {
            position: PointerPosition { x: 0.25, y: 0.75 },
        });
        round_trip(ServerToHost::ControlReceived {
            from: pid,
            surface: ControlSurface {
                ai_input: Some("summarize".into()),
                ai_cursor: Some(9),
                scroll: Some(ScrollUpdate { fraction: None }),
                ..Default::default()
            },
        });
        round_trip(ServerToHost::PointerMoved {
            from: pid,
            position: PointerPosition { x: 0.25, y: 0.75 },
        });
        round_trip(ServerToHost::AccessRequested {
            participant: pid,
            display_name: "Sam".into(),
        });
        round_trip(ServerToViewer::Joined {
            participant: pid,
            role: Role::Viewer,
            is_host_account: false,
        });
        round_trip(ServerToViewer::Delta {
            delta: TerminalDelta::BlockCompleted {
                block_id: "b2".into(),
                exit_code: Some(0),
                duration_ms: Some(1234),
                failed: false,
            },
        });
        round_trip(ServerToViewer::Delta {
            delta: TerminalDelta::BlockStarted {
                block_id: "b3".into(),
                command: "ls -la".into(),
                cwd: Some("/home/sam/dev".into()),
                ts_ms: 1_700_000_000_000,
            },
        });
        round_trip(ServerToViewer::Delta {
            delta: TerminalDelta::BlockLines {
                block_id: "b3".into(),
                base_line: 0,
                lines: vec![WireLine {
                    spans: vec![
                        WireSpan {
                            text: "drwxr-xr-x".into(),
                            fg: Some([80, 200, 120, 255]),
                            bg: None,
                            flags: 1,
                        },
                        WireSpan {
                            text: "  src/".into(),
                            ..Default::default()
                        },
                    ],
                }],
                dir_entries: Some(vec![WireDirEntry {
                    name: "src".into(),
                    kind: "directory".into(),
                    size: None,
                    color: Some([80, 200, 120, 255]),
                }]),
            },
        });
        round_trip(ServerToViewer::RoleChanged {
            role: Role::Controller,
        });
        round_trip(ServerToViewer::AccessDenied);
        round_trip(ServerToViewer::Delta {
            delta: TerminalDelta::AltScreen {
                rows: 40,
                cols: 120,
                cursor_row: 3,
                cursor_col: 7,
                cursor_visible: true,
                lines: vec![WireLine {
                    spans: vec![WireSpan {
                        text: "-- INSERT --".into(),
                        fg: Some([80, 200, 120, 255]),
                        bg: None,
                        flags: 0,
                    }],
                }],
            },
        });
        round_trip(ServerToViewer::Delta {
            delta: TerminalDelta::AltScreenEnded,
        });
        round_trip(ServerToViewer::Delta {
            delta: TerminalDelta::AgentBlocks {
                blocks: vec![
                    WireAgentBlock {
                        id: 1,
                        ts_ms: 1_700_000_000_100,
                        kind: WireAgentKind::UserQuery {
                            text: "refactor the parser".into(),
                            author: Some(WireAgentAuthor {
                                display_name: "Sam".into(),
                                avatar_url: "https://example.test/sam.png".into(),
                            }),
                        },
                    },
                    WireAgentBlock {
                        id: 2,
                        ts_ms: 1_700_000_000_200,
                        kind: WireAgentKind::TurnGroup {
                            children: vec![WireAgentBlock {
                                id: 3,
                                ts_ms: 1_700_000_000_300,
                                kind: WireAgentKind::ToolStep {
                                    name: "Edit".into(),
                                    icon: "📄".into(),
                                    args: "src/parser.rs".into(),
                                    status: "done".into(),
                                    result: Some("updated".into()),
                                    duration_ms: Some(210),
                                    diff: Some(WireDiff {
                                        files: vec![WireFileDiff {
                                            path: "src/parser.rs".into(),
                                            added: 3,
                                            removed: 1,
                                            lines: vec![WireDiffLine {
                                                kind: "add".into(),
                                                text: "let x = 1;".into(),
                                            }],
                                            truncated: false,
                                        }],
                                    }),
                                },
                            }],
                            duration_ms: Some(1500),
                            status_note: None,
                            expanded: true,
                        },
                    },
                ],
            },
        });
    }

    #[test]
    fn output_chunk_preserves_ordering_and_shape() {
        // The wire form is self-describing and carries the sequence number.
        let msg = HostToServer::Delta {
            delta: TerminalDelta::OutputChunk {
                block_id: "b1".into(),
                seq: 42,
                data_base64: "eA==".into(),
            },
        };
        let json = msg.to_json().unwrap();
        assert!(json.contains("\"type\":\"delta\""));
        assert!(json.contains("\"kind\":\"output_chunk\""));
        assert!(json.contains("\"seq\":42"));
    }

    #[test]
    fn secret_and_reconnect_token_are_masked_but_serialize_fully() {
        let secret = SessionSecret::new("super-secret-value");
        // Masked in logs...
        assert_eq!(format!("{secret:?}"), "SessionSecret(***)");
        assert_eq!(format!("{secret}"), "***");
        // ...but the real value is on the wire and recoverable.
        let json = serde_json::to_string(&secret).unwrap();
        assert!(json.contains("super-secret-value"));
        let back: SessionSecret = serde_json::from_str(&json).unwrap();
        assert_eq!(back.reveal(), "super-secret-value");

        let rt = ReconnectToken::new("rtok");
        assert_eq!(format!("{rt:?}"), "ReconnectToken(***)");
    }

    #[test]
    fn roles_and_access_levels_use_snake_case() {
        assert_eq!(serde_json::to_string(&Role::Host).unwrap(), "\"host\"");
        assert_eq!(
            serde_json::to_string(&AccessLevel::ViewOnly).unwrap(),
            "\"view_only\""
        );
    }
}
