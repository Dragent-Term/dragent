//! Typed IPC protocol between the Dragent UI and the persistent Dragent Host.
//!
//! Transport-independent domain types. The transport uses length-prefixed
//! frames over loopback TCP (one connection per unary RPC; one long-lived
//! connection per event subscription). All IDs are logical — never PIDs, FDs,
//! or paths.
//!
//! A frame's payload is a message: `[u32 BE json_len][JSON][binary tail]`.
//! Byte fields (terminal output/input, git stdout, history tails) are not
//! written into the JSON — they ride raw in the tail and the JSON holds a
//! small `{"tail":[offset,len]}` reference ([`tail_bytes`]). High-volume
//! terminal traffic is therefore not inflated by JSON number arrays.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Bump on ANY `Request`/`Response` change. Ping carries the envelope
/// version, so a stale daemon fails ping and `ensure_host` replaces it
/// instead of serving RPCs it cannot deserialize (which surfaces as
/// silent downgrades app-side). v2: `EnsureHome`/`HomeEnsured`, `HomeKind`.
/// v4: messages carry byte fields in a binary tail (see module docs).
/// v5: `SubscribeTerminal` output streams.
/// v6: Unix domain socket transport (`ServiceEndpoint::socket`).
/// v7: `RunGit` stdin/env.
/// v8: `AdoptCheckout`, `HomeKind::AgentCheckout`.
/// v9: `CreateTerminal` forwards the selected shell to the PTY daemon.
/// v10: daemon-owned terminal screen projections.
/// v11: decoded VT operation streams.
/// v12: daemon-owned shell integration and readiness before command execution.
/// v13: device/cursor queries answered by the daemon, never by parsed clients.
/// v16: remote filesystem operations and integrated remote terminal startup.
/// Older remote runtimes must not be reused: they reject Fs requests and can
/// launch shells without the lifecycle hooks required for interactive input.
/// v17: headless coordination commands and remote agent session snapshots.
pub const PROTOCOL_VERSION: u32 = 17;
/// Maximum encoded control/event frame. Frames are length-prefixed so JSON
/// payloads cannot be split or confused with keepalive bytes.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
/// Frozen, version-independent, token-gated request to restart only the host.
pub const HOST_UPDATE_MAGIC: &[u8] = b"DRAGENT-HOST-UPDATE-V1\n";
/// CLI flag that turns the current binary into the host service process.
pub const SERVICE_ARG: &str = "--dragent-host-service";
/// Env override for the endpoint file location (integration tests).
pub const ENDPOINT_FILE_ENV: &str = "DRAGENT_HOST_ENDPOINT_FILE";
pub const ENDPOINT_FILENAME: &str = "host-v1.json";

pub fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn write_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid frame length",
        ));
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

// ── Messages: JSON header + binary tail ──

std::thread_local! {
    /// Tail being collected while a message is encoded on this thread.
    static TAIL_OUT: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    /// Tail of the message being decoded on this thread.
    static TAIL_IN: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// Serde adapter for byte fields: inside [`encode_message`] the bytes go to
/// the binary tail and only their location is serialized; anywhere else
/// (plain `serde_json`, persisted files) they serialize as an ordinary array.
/// Decoding accepts both forms.
pub mod tail_bytes {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct TailRef {
        tail: (u64, u64),
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Tail(TailRef),
        Inline(Vec<u8>),
    }

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let located = super::TAIL_OUT.with(|cell| {
            cell.borrow_mut().as_mut().map(|tail| {
                let offset = tail.len() as u64;
                tail.extend_from_slice(bytes);
                (offset, bytes.len() as u64)
            })
        });
        match located {
            Some(tail) => TailRef { tail }.serialize(serializer),
            None => bytes.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        match Wire::deserialize(deserializer)? {
            Wire::Inline(bytes) => Ok(bytes),
            Wire::Tail(TailRef {
                tail: (offset, len),
            }) => super::TAIL_IN.with(|cell| {
                let tail = cell.borrow();
                let tail = tail
                    .as_ref()
                    .ok_or_else(|| D::Error::custom("tail reference outside a message"))?;
                let start = usize::try_from(offset).map_err(D::Error::custom)?;
                let end = start
                    .checked_add(usize::try_from(len).map_err(D::Error::custom)?)
                    .filter(|end| *end <= tail.len())
                    .ok_or_else(|| D::Error::custom("tail reference out of range"))?;
                Ok(tail[start..end].to_vec())
            }),
        }
    }
}

/// Encode a message payload: `[u32 BE json_len][JSON][tail]`.
pub fn encode_message<T: Serialize>(message: &T) -> io::Result<Vec<u8>> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TAIL_OUT.with(|cell| cell.borrow_mut().take());
        }
    }
    TAIL_OUT.with(|cell| *cell.borrow_mut() = Some(Vec::new()));
    let _reset = Reset;
    let json = serde_json::to_vec(message).map_err(io::Error::other)?;
    let tail = TAIL_OUT
        .with(|cell| cell.borrow_mut().take())
        .unwrap_or_default();
    let json_len = u32::try_from(json.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "message too large"))?;
    let mut out = Vec::with_capacity(4 + json.len() + tail.len());
    out.extend_from_slice(&json_len.to_be_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&tail);
    Ok(out)
}

/// Decode a payload produced by [`encode_message`].
pub fn decode_message<T: serde::de::DeserializeOwned>(payload: &[u8]) -> io::Result<T> {
    let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    let header: [u8; 4] = payload
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or_else(|| invalid("message shorter than its header"))?;
    let json_len = u32::from_be_bytes(header) as usize;
    let json_end = 4usize
        .checked_add(json_len)
        .filter(|end| *end <= payload.len())
        .ok_or_else(|| invalid("json length exceeds message"))?;
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TAIL_IN.with(|cell| cell.borrow_mut().take());
        }
    }
    TAIL_IN.with(|cell| *cell.borrow_mut() = Some(payload[json_end..].to_vec()));
    let _reset = Reset;
    serde_json::from_slice(&payload[4..json_end]).map_err(io::Error::other)
}

/// Encode `message` and write it as one frame.
pub fn write_message<T: Serialize>(writer: &mut impl Write, message: &T) -> io::Result<()> {
    write_frame(writer, &encode_message(message)?)
}

/// Read one frame and decode its message.
pub fn read_message<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> io::Result<T> {
    decode_message(&read_frame(reader)?)
}

/// Constant-time token comparison (no early exit on first mismatch,
/// length folded in). Timing side-channels against loopback tokens are
/// a narrow threat, but equality is the auth boundary — cheap to harden.
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).unwrap_or(&0) ^ b.get(i).unwrap_or(&0);
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn terminal_shell_setting_round_trips_and_legacy_requests_still_parse() {
        let legacy = r#"{"CreateTerminal":{"workspace_id":"workspace","worktree_id":"home","command":null,"cwd":null,"cols":80,"rows":24}}"#;
        let mut request: Request = serde_json::from_str(legacy).unwrap();
        if let Request::CreateTerminal { shell, .. } = &mut request {
            assert!(shell.is_none());
            *shell = Some("/bin/zsh".into());
        } else {
            panic!("wrong request");
        }
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(
            matches!(serde_json::from_str::<Request>(&encoded).unwrap(), Request::CreateTerminal { shell: Some(shell), .. } if shell == "/bin/zsh")
        );
    }

    #[test]
    fn token_compare_is_constant_time_correct() {
        assert!(tokens_equal("", ""));
        assert!(tokens_equal("abc123", "abc123"));
        assert!(!tokens_equal("abc123", "abc124"));
        assert!(!tokens_equal("abc123", "abc1234"));
        assert!(!tokens_equal("abc1234", "abc123"));
        assert!(!tokens_equal("", "x"));
        assert!(!tokens_equal("aaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaab"));
    }

    #[test]
    fn length_prefixed_frames_round_trip_and_reject_invalid_lengths() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, br#"{"ok":true}"#).unwrap();
        assert_eq!(
            read_frame(&mut Cursor::new(bytes)).unwrap(),
            br#"{"ok":true}"#
        );

        let mut invalid = 0u32.to_be_bytes().to_vec();
        invalid.extend_from_slice(b"ignored");
        assert!(read_frame(&mut Cursor::new(invalid)).is_err());
    }

    #[test]
    fn byte_fields_ride_in_the_tail_not_the_json() {
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let message = Response::TerminalData {
            data: data.clone(),
            base: 7,
            total: 4103,
            exit_code: None,
        };
        let encoded = encode_message(&message).unwrap();
        let json_len = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
        // The JSON stays small; the raw bytes sit after it, unencoded.
        assert!(json_len < 200, "json was {json_len} bytes");
        assert_eq!(&encoded[4 + json_len..], &data[..]);
        match decode_message::<Response>(&encoded).unwrap() {
            Response::TerminalData {
                data: back,
                base,
                total,
                ..
            } => {
                assert_eq!((back, base, total), (data, 7, 4103));
            }
            other => panic!("unexpected {other:?}"),
        }
        // Frame round trip over a stream.
        let mut wire = Vec::new();
        write_message(&mut wire, &message).unwrap();
        let back: Response = read_message(&mut Cursor::new(wire)).unwrap();
        assert!(matches!(back, Response::TerminalData { .. }));
    }

    #[test]
    fn byte_fields_outside_a_message_stay_plain_arrays() {
        let message = Response::TerminalData {
            data: vec![1, 2, 3],
            base: 0,
            total: 3,
            exit_code: None,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("[1,2,3]"), "{json}");
        let back: Response = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Response::TerminalData { data, .. } if data == vec![1, 2, 3]));
    }

    #[test]
    fn out_of_range_tail_reference_is_rejected() {
        let json = br#"{"TerminalData":{"data":{"tail":[0,99]},"base":0,"total":0}}"#;
        let mut payload = (json.len() as u32).to_be_bytes().to_vec();
        payload.extend_from_slice(json);
        payload.extend_from_slice(b"short");
        assert!(decode_message::<Response>(&payload).is_err());
    }
}

/// Where a second process finds a running host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub version: u32,
    #[serde(default)]
    pub build_id: String,
    /// Loopback TCP port (Windows, or when no socket could be created).
    pub port: u16,
    /// Unix domain socket path (macOS/Linux); preferred when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    pub token: String,
    pub pid: u32,
    pub host_id: String,
    pub started_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub host_id: String,
    pub version: u32,
    pub pid: u32,
    pub started_at_unix_ms: u64,
}

/// Shaped-but-minimal runtime snapshot. Counts stay zero until later phases
/// move runtimes behind the host boundary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeState {
    pub workspaces: u32,
    pub agents: u32,
    pub terminals: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Development(crate::development::DevelopmentRequest),
    /// Teams of agents: plan, tasks, mailbox (`team.rs`).
    Team(crate::team::TeamRequest),
    Coordination { tool: String, arguments: serde_json::Value, caller: serde_json::Value },
    Ping,
    GetHostInfo,
    GetRuntimeState,
    RunGit {
        root: String,
        args: Vec<String>,
        /// Piped to git's stdin (e.g. a patch for `git apply`); empty = none.
        #[serde(default, with = "tail_bytes")]
        stdin: Vec<u8>,
        /// Extra environment for this git process.
        #[serde(default)]
        env: Vec<(String, String)>,
        /// Variables removed from git's environment.
        #[serde(default)]
        env_remove: Vec<String>,
    },
    /// Open a subscription stream: the server keeps this connection open and
    /// writes one [`EventEnvelope`] JSON line per event until disconnect.
    Subscribe,
    Heartbeat,
    Shutdown,
    // ── Phase 2: canonical workspace state ──
    ListWorkspaces,
    CreateWorkspace {
        root: String,
    },
    CreateLocalWorkspace {
        root: String,
    },
    GetWorkspaceState {
        workspace_id: String,
    },
    AcceptWorkspaceCommand {
        workspace_id: String,
        command: dragent_session_protocol::workspace::WorkspaceCommandEnvelope,
    },
    // ── Phase 3: persistence authority (workspace domain) ──
    SaveWorkspaceProject {
        project: dragent_workspace_model::Project,
    },
    SaveWorkspaceWorkItem {
        work_item: dragent_workspace_model::WorkItem,
    },
    SaveWorkspaceEvent {
        event: dragent_workspace_model::WorkspaceEvent,
    },
    LoadWorkspaceProject {
        project_id: String,
    },
    LoadRecentWorkspaceEvents {
        limit: usize,
    },
    GetSchedulerStatus {
        workspace_id: String,
    },
    // ── Phase 6.1: worktree-backed workspaces + teardown saga ──
    CreateWorktree {
        workspace_id: String,
        flow: String,
        repo_root: String,
        branch: String,
    },
    /// Register an existing directory as a session home (no disk changes).
    EnsureHome {
        workspace_id: String,
        root: String,
    },
    /// Put an existing linked worktree (an agent checkout) under the host's
    /// ledger so reconcile and teardown cover it. Idempotent per path.
    AdoptCheckout {
        workspace_id: String,
        repo_root: String,
        path: String,
        flow: String,
    },
    ListWorktrees {
        workspace_id: String,
    },
    GetWorktree {
        workspace_id: String,
        worktree_id: String,
    },
    TeardownWorktree {
        workspace_id: String,
        worktree_id: String,
    },
    CreateStandaloneSession {
        workspace_id: String,
        label: String,
    },
    TeardownStandaloneSession {
        workspace_id: String,
        session_id: String,
    },
    AttachSession {
        workspace_id: String,
        worktree_id: String,
        session_id: String,
    },
    DetachSession {
        workspace_id: String,
        worktree_id: String,
        session_id: String,
    },
    AllocatePort {
        workspace_id: String,
        worktree_id: String,
    },
    ReleasePort {
        workspace_id: String,
        worktree_id: String,
        port: u16,
    },
    // ── Phase 6.2: terminal sessions owned per home ──
    CreateTerminal {
        workspace_id: String,
        worktree_id: String,
        command: Option<String>,
        #[serde(default)]
        shell: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    ListTerminals {
        workspace_id: String,
    },
    GetTerminal {
        workspace_id: String,
        session_id: String,
    },
    SendTerminalInput {
        workspace_id: String,
        session_id: String,
        #[serde(with = "tail_bytes")]
        data: Vec<u8>,
    },
    ReadTerminal {
        workspace_id: String,
        session_id: String,
        max_lines: Option<usize>,
    },
    ReadTerminalScreen {
        workspace_id: String,
        session_id: String,
    },
    ReadTerminalSince {
        workspace_id: String,
        session_id: String,
        offset: u64,
        max_bytes: usize,
    },
    /// Long-lived: stream the session's output from `offset` (replay-ring
    /// catch-up, then live) as `TerminalData` responses on this connection.
    /// The last one carries `exit_code`; a closed session ends with an error.
    /// Empty `TerminalData` frames are keepalives.
    SubscribeTerminal {
        workspace_id: String,
        session_id: String,
        offset: u64,
    },
    /// Parsed output; reconnect offsets must be operation-frame boundaries.
    SubscribeTerminalOperations {
        workspace_id: String,
        session_id: String,
        offset: u64,
    },
    ResizeTerminal {
        workspace_id: String,
        session_id: String,
        cols: u16,
        rows: u16,
    },
    SetTerminalColors {
        workspace_id: String,
        session_id: String,
        colors: crate::screen_protocol::TerminalColors,
    },
    CloseTerminal {
        workspace_id: String,
        session_id: String,
    },
    // ── Phase 6.3: agent runs in terminal sessions ──
    ListAgentConfigs,
    AddAgentConfig {
        label: String,
        command: String,
        args: Vec<String>,
        prompt_transport: PromptTransport,
        prompt_args: Vec<String>,
        env: std::collections::BTreeMap<String, String>,
    },
    RunAgent {
        workspace_id: String,
        worktree_id: String,
        agent: String,
        prompt: String,
        dispatch: AgentDispatch,
    },
    /// An agent's launch monitor asks whether the agent may start.
    AskToStart(StartAsk),
    /// The person wants a waiting agent (its pane or terminal) started now,
    /// whatever memory says. Also valid before its agent has asked.
    StartAnyway {
        id: String,
    },
    /// A waiting start was called off.
    LeaveStartLine {
        id: String,
    },
    /// Which agents wait or are paused, and what each one holds. Read-only.
    AgentResources,
    /// Let a paused agent (its pane or terminal) run again.
    ResumeAgent {
        id: String,
    },
    GetAgentRun {
        run_id: String,
    },
    ListAgentRuns {
        workspace_id: String,
    },
    CheckAgentReadiness {
        agent: String,
    },
    // ── Project file watching (git-porcelain polling) ──
    WatchWorkspace {
        workspace_id: String,
    },
    UnwatchWorkspace {
        workspace_id: String,
    },
    ListWatches,
    // ── Coordination runtime: launch proposals (pure, nothing executes) ──
    ProposeAgentRuns {
        workspace_id: String,
    },
    // ── UI restore protocol: re-attach by id, never by path ──
    GetRestorePlan {
        workspace_id: String,
        known_session_ids: Vec<String>,
        max_tail_lines: Option<usize>,
    },
    // ── PTY split: supervised daemon owns the TerminalRegistry ──
    RestartPtyDaemon,
    GetPtyStatus,
    // ── Phase 1 cold restore: per-session disk history ──
    ReadSessionHistory {
        session_id: String,
        max_bytes: usize,
    },
    ListSessionHistory,
    PurgeHomeHistory {
        home_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Development(crate::development::DevelopmentResponse),
    Team(crate::team::TeamResponse),
    Coordination(serde_json::Value),
    Pong,
    HostInfo(HostInfo),
    RuntimeState(RuntimeState),
    GitOutput {
        status: i32,
        #[serde(with = "tail_bytes")]
        stdout: Vec<u8>,
        #[serde(with = "tail_bytes")]
        stderr: Vec<u8>,
    },
    Ack,
    WorkspaceList {
        workspace_ids: Vec<String>,
    },
    WorkspaceCreated {
        workspace_id: String,
    },
    WorkspaceStateSnapshot(WorkspaceSnapshot),
    WorkspaceEventsAccepted {
        count: usize,
    },
    WorkspaceProjectLoaded {
        project: Option<dragent_workspace_model::Project>,
    },
    WorkspaceEventList {
        events: Vec<dragent_workspace_model::WorkspaceEvent>,
    },
    SchedulerStatusReport {
        status: Option<SchedulerStatus>,
    },
    // ── Phase 6.1: worktree-backed workspaces + teardown saga ──
    WorktreeCreated {
        home: WorktreeHome,
    },
    HomeEnsured {
        home: WorktreeHome,
    },
    WorktreeList {
        homes: Vec<WorktreeHome>,
    },
    WorktreeSnapshot(WorktreeHome),
    WorktreeTornDown {
        report: TeardownReport,
    },
    StandaloneSessionCreated {
        home: WorktreeHome,
    },
    StandaloneSessionTornDown {
        worktree_id: String,
    },
    PortAllocated {
        port: u16,
    },
    // ── Phase 6.2: terminal sessions owned per home ──
    TerminalOpened {
        session: TerminalSession,
    },
    TerminalList {
        sessions: Vec<TerminalSession>,
    },
    TerminalSnapshot(TerminalSession),
    TerminalScreen(super::screen_protocol::TerminalScreen),
    TerminalOutput {
        text: String,
        alive: bool,
        exit_code: Option<i32>,
    },
    TerminalOperations {
        frame: super::screen_protocol::OperationFrame,
    },
    TerminalData {
        #[serde(with = "tail_bytes")]
        data: Vec<u8>,
        base: u64,
        total: u64,
        /// Set once the shell has exited: the client can show the exit and
        /// stop polling instead of mistaking the end for a lost session.
        #[serde(default)]
        exit_code: Option<i32>,
    },
    // ── Phase 6.3: agent runs in terminal sessions ──
    AgentConfigList {
        configs: Vec<AgentConfig>,
    },
    AgentConfigAdded {
        config: AgentConfig,
    },
    AgentRunStarted {
        run: AgentRun,
    },
    AgentRunSnapshot {
        run: AgentRun,
    },
    AgentRunList {
        runs: Vec<AgentRun>,
    },
    StartAnswer(StartAnswer),
    AgentResources(AgentResources),
    AgentReadinessReport {
        readiness: AgentReadiness,
    },
    AgentRouteList {
        proposals: Vec<super::router::RouteProposal>,
    },
    WatchList {
        workspace_ids: Vec<String>,
    },
    RestorePlanSnapshot(RestorePlan),
    // ── PTY split: supervised daemon health ──
    PtyStatusReport {
        daemon_pid: Option<u32>,
        daemon_id: Option<String>,
        owned: bool,
        circuit_open: bool,
        crash_attempts: u32,
    },
    // ── Phase 1 cold restore: per-session disk history ──
    SessionHistory {
        record: HistoryRecord,
        #[serde(with = "tail_bytes")]
        tail: Vec<u8>,
    },
    SessionHistoryList {
        records: Vec<HistoryRecord>,
    },
}

/// Point-in-time projection of one host-managed workspace. Logical IDs plus
/// durable domain records only — never handles, paths-as-authority, or PIDs.
/// Snapshot summary for the scheduler-status RPC (counts only — full
/// projections stay in-process until later phases need them over IPC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerStatus {
    pub workspace_id: String,
    pub has_plan: bool,
    pub ready_tasks: usize,
    pub launchable_tasks: usize,
    pub work_complete: bool,
}

impl SchedulerStatus {
    pub fn from_projection(
        workspace_id: &str,
        projection: &dragent_workspace_model::SchedulerProjection,
    ) -> Self {
        Self {
            workspace_id: workspace_id.to_string(),
            has_plan: true,
            ready_tasks: projection.ready_task_ids.len(),
            launchable_tasks: projection.launchable_task_ids.len(),
            work_complete: projection.work_complete,
        }
    }

    pub fn no_plan(workspace_id: &str) -> Self {
        Self {
            workspace_id: workspace_id.to_string(),
            has_plan: false,
            ready_tasks: 0,
            launchable_tasks: 0,
            work_complete: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub workspace_id: String,
    pub project: dragent_workspace_model::Project,
    pub active_work: Option<dragent_workspace_model::WorkItem>,
    pub event_count: usize,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    HostReady {
        host_id: String,
    },
    HeartbeatAck {
        client_id: String,
    },
    ShutdownNotice,
    WorkspaceCreated {
        workspace_id: String,
    },
    WorkspaceUpdated {
        workspace_id: String,
    },
    SchedulerEvaluated {
        workspace_id: String,
        ready_tasks: usize,
        launchable_tasks: usize,
        work_complete: bool,
    },
    // ── Phase 6.1: worktree-backed workspaces + teardown saga ──
    WorktreeCreated {
        workspace_id: String,
        worktree_id: String,
    },
    WorktreeTornDown {
        workspace_id: String,
        worktree_id: String,
    },
    WorkspaceFilesChanged {
        workspace_id: String,
        paths: Vec<PathBuf>,
    },
    // ── Phase 6.2: terminal sessions owned per home ──
    TerminalOpened {
        workspace_id: String,
        session_id: String,
    },
    TerminalClosed {
        workspace_id: String,
        session_id: String,
    },
    // ── Phase 6.3: agent runs in terminal sessions ──
    AgentRunStarted {
        workspace_id: String,
        worktree_id: String,
        run_id: String,
    },
    AgentRunEnded {
        workspace_id: String,
        worktree_id: String,
        run_id: String,
    },
    /// A team's plan, tasks or messages changed.
    TeamChanged {
        team_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub version: u32,
    pub token: String,
    pub client_id: String,
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub version: u32,
    pub result: Result<Response, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event: Event,
}

// ── Phase 6.1: worktree-backed workspaces + teardown saga ──

/// One host-managed checkout (git worktree) or standalone session directory.
/// Standalone sessions are ledger homes with `repo: None`; session
/// attachments are logical IDs listed in `sessions` (move semantics on
/// attach). Only `Live` homes are visible over IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeHome {
    pub id: String,
    pub workspace_id: String,
    pub flow: String,
    pub path: String,
    pub repo: Option<GitBinding>,
    pub ports: Vec<u16>,
    pub sessions: Vec<String>,
    pub state: WorktreeState,
    pub created_at_unix_ms: u64,
    /// Ownership class. Missing on rows written before kinds existed, which
    /// were all managed checkouts or marker-guarded scratch dirs — both
    /// behave exactly like [`HomeKind::Worktree`], hence the default.
    #[serde(default)]
    pub kind: HomeKind,
}

/// What the host may do with a home's directory. `Root` homes are
/// pre-existing user directories registered read-only: reconcile treats a
/// present path as healthy (no marker, no git-registration check) and
/// teardown never touches the filesystem — it only disposes sessions and
/// drops the registration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum HomeKind {
    /// Checkout created by `git worktree add` under the managed root.
    #[default]
    Worktree,
    /// Scratch directory created by the host under the managed root.
    Standalone,
    /// Pre-existing user directory (`ensure_root_home`). Never deleted.
    Root,
    /// A linked worktree created by the app for an agent run and handed to
    /// the host (`adopt_checkout`). No marker is written into it (it would
    /// show up in the agent's diff); ownership is proven by git listing it
    /// as a linked worktree of its repository.
    AgentCheckout,
}

/// Git provenance for a worktree-backed home. The branch is deliberately
/// kept on teardown (never deleted) — only the registration and the
/// checkout directory are removed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitBinding {
    pub repo_root: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorktreeState {
    Live,
    Tombstoned { reason: String },
}

/// Ordered teardown outcome. `branch_kept` is `Some` for worktree-backed
/// homes (the branch survives by design) and `None` for standalone
/// sessions. Non-fatal cleanup failures land in `warnings`, never in `Err`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeardownReport {
    pub worktree_id: String,
    pub sessions_detached: usize,
    pub ports_released: usize,
    pub branch_kept: Option<String>,
    pub warnings: Vec<String>,
}

// ── Phase 6.2: terminal sessions owned per home ──

/// One host-owned interactive PTY session. The `id` is attached to the
/// owning [`WorktreeHome`] via its `sessions` list (move semantics on
/// attach); teardown closes attached terminals through the same hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalSession {
    pub id: String,
    pub workspace_id: String,
    pub worktree_id: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub state: TerminalState,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerminalState {
    Running,
    Exited { code: i32 },
}

// ── Phase 6.3: agent runs in terminal sessions ──
//
// Agent runs execute in terminal sessions owned per worktree home: the run
// id doubles as the session id string attached to the home (move semantics
// via the worktree manager). No terminal internals are referenced here —
// only id strings — so this slice stays daemon-agnostic while terminals.rs
// lands in parallel.

/// How the assembled prompt reaches the agent process.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptTransport {
    /// Prompt appended as a final quoted argv element.
    #[default]
    Argv,
    /// Prompt delivered on stdin via a quoted heredoc.
    Stdin,
}

/// One launchable agent definition. `preset_id` is `Some` for seeded catalog
/// entries (`preset-<slug>` ids); user configs get `agent-<uuid>` ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    pub id: String,
    pub preset_id: Option<String>,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub prompt_transport: PromptTransport,
    pub prompt_args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub order: u32,
}

/// Point-in-time projection of one agent run. `id` is the session id string
/// attached to the owning home; `output` is the captured screen tail.
/// `memory_bytes` is what the agent and everything it started holds, when
/// that has been measured.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRun {
    pub id: String,
    pub workspace_id: String,
    pub worktree_id: String,
    pub agent: String,
    pub label: String,
    pub state: AgentRunState,
    pub started_at: u64,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Coordination linkage echoed from dispatch (see AgentDispatch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentRunState {
    /// Its terminal is open and the agent starts in it when memory allows.
    Waiting,
    Running,
    /// Stopped for holding too much memory, until the person resumes it.
    Paused,
    Exited { code: i32 },
}

/// A start asking whether it may go.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartAsk {
    /// The asking agent's pane, or its terminal where it has no pane.
    pub id: String,
    #[serde(default)]
    pub terminal_id: String,
    /// What the person sees ("claude").
    pub label: String,
    /// When it first asked (Unix milliseconds): its place in the line.
    pub since_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StartAnswer {
    Go,
    Wait {
        position: usize,
        /// Why; `None` when only its turn hasn't come yet.
        reason: Option<WaitReason>,
    },
}

/// Why new agents wait instead of starting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaitReason {
    /// The machine is critically low on memory.
    MemoryCritical,
    /// The cap the person set is reached.
    AtCap { live: usize, max: usize },
}

/// A start that waits, as shown to the person.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitingStart {
    pub id: String,
    pub terminal_id: String,
    pub label: String,
    pub since_ms: u64,
    /// Its place in the line, from 1.
    pub position: usize,
}

/// One agent and what it holds, with everything it started.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMemoryUse {
    pub pane: String,
    pub terminal_id: String,
    pub label: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentResources {
    pub waiting: Vec<WaitingStart>,
    pub paused: Vec<AgentMemoryUse>,
    pub running: Vec<AgentMemoryUse>,
}

/// Per-launch overrides merged over the resolved config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentOverrides {
    pub model: Option<String>,
    pub command: Option<String>,
    pub extra_args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
}

/// Per-launch inputs: overrides, context file reads, staged attachments.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentDispatch {
    pub overrides: AgentOverrides,
    pub context_from: Vec<String>,
    pub context_chars: usize,
    pub attachments: Vec<String>,
    /// Optional coordination linkage: the agent instance and execution
    /// lease this run fulfills. The lease watchdog enforces expiry
    /// (absent ids mean an unmanaged run — never touched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
}

/// Honest install probe for one agent reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReadiness {
    pub agent: String,
    pub label: String,
    pub installed: bool,
    pub detail: String,
}

/// Disk history tombstone for one dead PTY session. Written at spawn,
/// stamped on exit; survives daemon crashes for cold restore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryRecord {
    pub session_id: String,
    pub workspace_id: String,
    pub home_id: String,
    pub cols: u16,
    pub rows: u16,
    pub cwd: String,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

/// Everything the UI needs to re-attach after a restart, keyed by
/// logical ids only. `homes` are live homes; `sessions` carry a bounded
/// tail for instant repaint; `gone_session_ids` are known ids the host
/// no longer holds (UI falls back to SQLite history for those panes).
/// `history` carries workspace-filtered disk tombstones for cold restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePlan {
    pub workspace_id: String,
    pub homes: Vec<WorktreeHome>,
    pub sessions: Vec<RestoreSession>,
    pub gone_session_ids: Vec<String>,
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
}

/// One live session snapshot plus its replay tail for re-attach.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreSession {
    pub session: TerminalSession,
    pub tail: String,
    pub alive: bool,
    pub exit_code: Option<i32>,
}

// ── PTY split: host-supervised daemon owns the TerminalRegistry ──
//
// The host process no longer spawns shells itself. It supervises a
// `dragent-host --dragent-pty-service` child (one per org dir) that owns the
// registry; host terminal RPCs forward as daemon RPCs (JSON lines over
// loopback TCP, one connection per unary RPC — the host/discovery framing).

/// CLI flag that turns the current binary into the PTY daemon child process.
pub const PTY_SERVICE_ARG: &str = "--dragent-pty-service";
/// Manifest file (next to the host endpoint) describing the live daemon.
pub const PTY_MANIFEST_FILENAME: &str = "pty-daemon.json";
/// Env override carrying the daemon token into the child process.
pub const PTY_TOKEN_ENV: &str = "DRAGENT_PTY_TOKEN";
/// Daemon IPC version spoken by this build.
pub const DAEMON_PROTOCOL_VERSION: u32 = 10;
/// Daemon IPC versions this build accepts.
pub const DAEMON_SUPPORTED_VERSIONS: &[u32] = &[10];
/// Base of the stable-port block for PTY daemons (one port per org dir).
pub const PTY_PORT_BASE: u16 = 47800;
/// Width of the stable-port block.
const PTY_PORT_SPAN: u64 = 1024;

/// Stable loopback port for one org dir: FNV-1a over the org string mapped
/// into the PTY block. Lets a new host re-adopt a surviving daemon without
/// any rendezvous beyond the manifest file.
pub fn pty_stable_port(org: &str) -> u16 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in org.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    PTY_PORT_BASE + (hash % PTY_PORT_SPAN) as u16
}

/// Where a host finds (or adopts) its PTY daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyDaemonManifest {
    pub version: u32,
    #[serde(default)]
    pub build_id: String,
    pub pid: u32,
    pub port: u16,
    /// Unix domain socket path (macOS/Linux); preferred when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    pub token: String,
    pub daemon_id: String,
    pub started_at_unix_ms: u64,
}

/// An explicit rejection before execution; safe for the host to retry once.
pub const PTY_UPDATE_RETRY: &str = "terminal daemon handoff completed; retry request";

/// Version-independent upgrade request to a PTY daemon, sent as a raw frame
/// (not a message) so that a daemon of ANY later build can still be asked to
/// hand its sessions to a new binary: `MAGIC <token> "\n" <successor exe>`.
/// The reply is a raw frame `OK`, `EMPTY`, or `ERR <reason>`. Frozen: never change.
pub const PTY_UPGRADE_MAGIC: &[u8] = b"DRAGENT-PTY-UPGRADE-V1\n";

/// One live session as a daemon hands it to its successor during an
/// upgrade. This record crosses build versions, so it only ever gains
/// fields (with serde defaults) — never renames or removes them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffSession {
    pub id: String,
    pub workspace_id: String,
    pub worktree_id: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub created_at_unix_ms: u64,
    /// The shell still running on the PTY.
    pub shell_pid: u32,
    /// PTY master fd number, inherited by the successor at the same number.
    pub master_fd: i32,
    /// Absolute offset of `ring[0]` in the session's output stream.
    pub ring_start: u64,
    /// The replay ring's contents.
    #[serde(with = "tail_bytes")]
    pub ring: Vec<u8>,
    #[serde(default)]
    pub operations: Option<super::screen_protocol::OperationCheckpoint>,
}

/// Everything a successor daemon adopts. Written with [`encode_message`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandoffSnapshot {
    #[serde(default)]
    pub sessions: Vec<HandoffSession>,
}

/// Unary RPCs the host sends the PTY daemon. Session ids are daemon-scoped;
/// the host keeps workspace gating by checking the returned snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonRequest {
    Hello {
        version: u32,
    },
    CreateTerminal {
        home: WorktreeHome,
        workspace_id: String,
        command: Option<String>,
        #[serde(default)]
        shell: Option<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    GetTerminal {
        session_id: String,
    },
    ListTerminals {
        workspace_id: String,
    },
    ListSessionsForHome {
        home_id: String,
    },
    SendTerminalInput {
        session_id: String,
        #[serde(with = "tail_bytes")]
        data: Vec<u8>,
    },
    ReadTerminal {
        session_id: String,
        max_lines: Option<usize>,
    },
    ReadTerminalScreen {
        session_id: String,
    },
    ReadTerminalSince {
        session_id: String,
        offset: u64,
        max_bytes: usize,
    },
    /// Long-lived output stream (see `Request::SubscribeTerminal`).
    SubscribeTerminal {
        session_id: String,
        offset: u64,
    },
    /// Parsed output; reconnect offsets must be operation-frame boundaries.
    SubscribeTerminalOperations {
        session_id: String,
        offset: u64,
    },
    ResizeTerminal {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    SetTerminalColors {
        session_id: String,
        colors: crate::screen_protocol::TerminalColors,
    },
    CloseTerminal {
        session_id: String,
    },
    ReadSessionHistory {
        session_id: String,
        max_bytes: usize,
    },
    ListSessionHistory,
    PurgeHomeHistory {
        home_id: String,
    },
    Handoff,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    HelloAck {
        version: u32,
        daemon_id: String,
        started_at: u64,
    },
    TerminalOpened {
        session: TerminalSession,
    },
    TerminalList {
        sessions: Vec<TerminalSession>,
    },
    SessionIds {
        ids: Vec<String>,
    },
    TerminalSnapshot(TerminalSession),
    TerminalScreen(super::screen_protocol::TerminalScreen),
    TerminalOutput {
        session_id: String,
        text: String,
        alive: bool,
        exit_code: Option<i32>,
    },
    TerminalOperations {
        frame: super::screen_protocol::OperationFrame,
    },
    TerminalData {
        session_id: String,
        base_offset: u64,
        #[serde(with = "tail_bytes")]
        data: Vec<u8>,
        total_bytes: u64,
        #[serde(default)]
        exit_code: Option<i32>,
    },
    SessionHistory {
        record: HistoryRecord,
        #[serde(with = "tail_bytes")]
        tail: Vec<u8>,
    },
    SessionHistoryList {
        records: Vec<HistoryRecord>,
    },
    Ack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRequestEnvelope {
    pub version: u32,
    pub token: String,
    pub request: DaemonRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonResponseEnvelope {
    pub version: u32,
    pub result: Result<DaemonResponse, String>,
}
