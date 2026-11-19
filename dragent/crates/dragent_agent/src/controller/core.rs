//! Agent engine core: the `AgentController` state machine without UI.
//!
//! Same struct the app runs today, with two seams cut for host ownership:
//! configuration arrives through [`ControllerDeps`] (trait objects the app
//! implements against WarpUI + its registries today, the Host against
//! threads + IPC later) instead of concrete app services and globals, and
//! events fan out through [`EventSink`][crate::runtime::EventSink] instead
//! of a WarpUI context. Pumps are poll-once functions the owner
//! reschedules; nothing here spawns timers or touches the framework.
//!
//! Method implementations live in the sibling modules (`blocks`, …),
//! moved verbatim apart from the seam rewrites.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use uuid::Uuid;

use super::AgentRole;
use crate::api::AgentApi;
use crate::attachments::MessageAttachment;
use crate::block::{AgentAuthor, AgentBlock};
use crate::checkpoint_store::CheckpointStore;
use crate::permission_store::PermissionStore;
use crate::pipeline::{HostToolDef, PipelineEvent, StreamError};
use crate::runtime::{
    AgentHooks, AgentHostConfig, PipelineLauncher, ToolHost, TranscriptSession, TranscriptStore,
};
use crate::session::AgentSessionContext;
use crate::state::ConversationState;
use crate::types::{
    DisplayTextFilter, PendingToolCall, QueuedMessage, ResponseStyle, SessionMessage, ToolPolicy,
    TurnToolCall,
};
use crate::{background::BackgroundCompletion, runtime::CoreEvent};

/// Everything the engine borrows from its owner. The app builds this from
/// its registries; the Host will build it from its runtime. No globals.
pub struct ControllerDeps {
    pub api: Arc<dyn AgentApi>,
    pub tools: Arc<dyn ToolHost>,
    pub transcripts: Arc<dyn TranscriptStore>,
    pub pipeline: Arc<dyn PipelineLauncher>,
    pub hooks: Arc<dyn AgentHooks>,
    pub config: AgentHostConfig,
}

/// Staged compacted history from the previous turn: `Some((compacted,
/// base_len))` where `base_len` is the history length the snapshot was
/// taken from. Consumed at the next turn start and merged with entries
/// appended since — never applied blindly, so in-turn appends are
/// preserved and shrinks discard the stale slot.
pub(crate) type CompactedHistorySlot = Arc<Mutex<Option<(Vec<SessionMessage>, usize)>>>;

/// A consent request waiting behind the currently visible approval card.
pub(crate) struct QueuedApproval {
    pub action_id: Uuid,
    pub command: String,
    pub reason: String,
    pub conversation_id: Uuid,
}

/// UI-independent agent controller. Field-for-field the app's controller
/// with app-concrete types replaced by moved shared types + injected deps.
pub struct AgentController {
    pub(crate) deps: ControllerDeps,
    pub(crate) blocks: Vec<AgentBlock>,
    pub(crate) next_block_id: usize,
    pub(crate) state: ConversationState,
    pub(crate) active_conversation_id: Option<Uuid>,
    /// Working directory this controller was opened for (used for session store).
    pub(crate) cwd: PathBuf,
    /// `action_id` of the approved RunCommand awaiting shell completion.
    pub(crate) approved_action_id: Option<Uuid>,
    /// Command text of the approved RunCommand (for ActionResult block).
    pub(crate) approved_command: Option<String>,
    /// Current tool-use round within the active turn (reset on new query).
    pub(crate) tool_round: u8,
    /// Store the active session context for the duration of a multi-round query.
    pub(crate) active_session: Option<AgentSessionContext>,
    /// Active streaming task ID (for permission resolution).
    pub(crate) streaming_task_id: Option<String>,
    /// Accumulated streaming text (for the final AgentMessage block).
    pub(crate) streaming_text: String,
    /// When true the next TextDelta starts a fresh AgentMessage block instead of
    /// appending to streaming_text. Set when a ToolCall is dispatched so that the
    /// LLM's follow-up text after the tool round appears as a new block rather than
    /// being prepended with all of the text from the previous round.
    pub(crate) reset_streaming_on_next_text: bool,
    /// Block ID of the current ThinkingStep block (updated in-place as deltas arrive).
    pub(crate) thinking_block_id: Option<usize>,
    /// Block ID of the transient Loading placeholder shown right after query submission.
    pub(crate) loading_block_id: Option<usize>,
    /// Destructive tool calls waiting for user approval before local execution.
    /// Keyed by the locally-generated action_id shown in the ApprovalRequest block.
    pub(crate) pending_tool_calls: HashMap<Uuid, PendingToolCall>,
    pub(crate) queued_approvals: VecDeque<QueuedApproval>,
    /// User prompts submitted while a turn was already running. They wait here
    /// (shown as "Queued" pills) and start FIFO as each turn finishes, instead
    /// of wiping the conversation to run immediately.
    pub(crate) message_queue: VecDeque<QueuedMessage>,
    /// Receives completion events from background tasks started with wait_ms_before_async.
    /// The watcher pump drains this and starts a new turn automatically.
    pub(crate) bg_completion_rx: Option<std::sync::mpsc::Receiver<BackgroundCompletion>>,
    /// Finished-background-task summaries that arrived while a turn was still
    /// running (so they couldn't resume the agent immediately). The watcher
    /// drains these into a system continuation the moment the agent goes Idle —
    /// without this, a completion (including a KILLED command's) that lands
    /// during the "I'm waiting for X" narration window is lost and the agent
    /// stays stuck "waiting" for a task that already ended.
    pub(crate) deferred_bg_summaries: Vec<String>,
    /// Plans waiting for user to click Implement or Refine.
    /// Keyed by plan_id (locally generated). Stores the raw plan text.
    pub(crate) pending_plans: HashMap<Uuid, String>,
    /// Conversation history accumulated across queries in this session.
    /// Sent with each new task so the LLM has full context of what was discussed.
    pub(crate) session_history: Vec<SessionMessage>,
    /// Saved from the last start_streaming_query call so approve_plan can
    /// reuse the same model and system prompt without requiring terminal_view
    /// to pass them again.
    pub(crate) last_model_name: String,
    /// Shared with the background thread; the thread writes the built system
    /// prompt here before calling create_task. approve_plan reads it after the
    /// agent has already responded (so it is always populated by then).
    pub(crate) last_system_arc: Arc<std::sync::Mutex<String>>,
    /// JSONL transcript for this session (None until the first query).
    /// Open sessions only — the store factory lives in deps.
    pub(crate) session_store: Option<Box<dyn TranscriptSession>>,
    /// Write-time content snapshots, keyed by user-message
    /// block id. Restoring a message rewinds exactly the files it changed.
    pub(crate) checkpoints: CheckpointStore,
    /// Block id of the UserQuery for the turn in progress. Set ONLY by a real
    /// user prompt (not system continuations), so an auto-resumed turn's
    /// writes attach to the originating user message's checkpoint.
    pub(crate) current_query_block_id: Option<usize>,
    /// Persisted tool approval preferences (always-allow / always-deny).
    pub(crate) permission_store: PermissionStore,
    /// Cumulative token usage for this session.
    pub(crate) session_input_tokens: u64,
    pub(crate) session_cached_input_tokens: u64,
    pub(crate) session_output_tokens: u64,
    /// Current context size (input + cached prompt tokens of the last turn),
    /// from the provider's usage. Snapshot, not cumulative — drives the
    /// context-window meter.
    pub(crate) current_context_tokens: u64,
    /// Number of user turns in this session.
    pub(crate) turn_count: u32,
    /// User-selected response verbosity style.
    pub response_style: ResponseStyle,
    /// When true, write/execute tool calls are auto-rejected until the user
    /// approves the plan. Set only for non-trivial queries that produce a plan;
    /// cleared on plan approval and NOT reset between turns.
    pub(crate) gate_tools_until_plan: bool,
    /// Whether the current chat mode gates write/exec tools behind a plan (Plan
    /// mode). Applied to the next user turn by `start_streaming_query`.
    pub(crate) mode_gate: bool,
    /// Ask mode: hard-refuse any mutating tool call, regardless of what the
    /// model attempts. Belt over the prompt-level "read-only" instruction.
    pub(crate) read_only_mode: bool,
    /// Command-intent requests always require a visible approval, even when
    /// the user's normal autonomy mode or an always-allow rule would auto-run
    /// the generated shell command.
    pub(crate) force_command_approval: bool,
    /// Whether the user's own prompt explicitly allowed changing the visible
    /// terminal's working directory.
    pub(crate) allow_terminal_cwd_change: bool,
    /// The running turn was started by an automation: nobody is there to
    /// approve a tool call or answer a question, so neither waits.
    pub(crate) unattended: bool,
    /// The next submitted query starts an unattended turn.
    pub(crate) next_query_unattended: bool,
    /// The unattended turn may comment, change statuses and create items in
    /// connected services (its automation allows it); otherwise it only reads.
    pub(crate) unattended_service_writes: bool,
    /// What the next submitted query's automation allows there.
    pub(crate) next_query_service_writes: bool,
    /// Role injected into the system prompt to configure Coordinator/Worker behavior.
    pub role: AgentRole,
    /// Number of tool calls executed in this session since last extraction check.
    pub(crate) tool_call_count: u32,
    /// SQLite writer channel for agent block persistence.
    /// Set by the owner after construction via `set_persistence`.
    pub(crate) persistence_sender:
        Option<std::sync::mpsc::SyncSender<dragent_persistence::ModelEvent>>,
    /// Pane UUID used as the key for agent block rows in SQLite.
    pub(crate) pane_uuid: Uuid,
    /// Evidence gate: how many times we've auto-nudged this user turn to verify
    /// unverified edits. Capped so a failing check can re-nudge once without looping.
    pub(crate) evidence_nudge_count: u8,
    /// Set by the Done handler (which lacks pumping context) when the evidence
    /// gate should fire; consumed by `poll_stream`, which has the context.
    pub(crate) pending_evidence_nudge: Option<String>,
    /// Incremented whenever a running stream is force-cancelled so that orphaned
    /// `poll_stream` pumps silently drop events from the old stream.
    pub(crate) stream_generation: u64,
    /// Shared with in-flight tool threads. Set on cancel_stream so the outer
    /// timeout wrapper posts a cancellation result immediately instead of waiting.
    pub(crate) tool_cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Pipeline call IDs already accepted by this controller. A provider retry
    /// must not execute a local write or command twice.
    pub(crate) active_tool_calls: HashSet<String>,
    pub(crate) completed_tool_calls: HashSet<String>,
    /// Relative paths of files written/edited by the agent in this session.
    /// Populated when write tools execute. Used to drive the git diff bar UI.
    pub agent_modified_files: Vec<String>,
    /// Image attachments for the query currently being submitted; drained into
    /// the user SessionMessage in start_streaming_query_impl.
    pub(crate) pending_query_attachments: Vec<MessageAttachment>,
    /// Identity attached to the next real user-query block.
    pub(crate) pending_query_author: Option<AgentAuthor>,
    /// Durable execution checkpoint selected by an error card's Continue action.
    /// Consumed by `start_streaming_query` so the continuation is system-owned
    /// and cannot masquerade as a new user-authored "continue" message.
    pub(crate) pending_failed_resume: Option<String>,
    /// Source task for structured server recovery. The journal checkpoint is
    /// retained alongside it as a rollout fallback for older deployments.
    pub(crate) pending_server_resume_task_id: Option<String>,
    /// Embedded-host scope (Work Item Principal, …): when set, the provider only
    /// sees these tools (`pipeline::start_scoped_task_stream`) instead of the
    /// terminal's full local + MCP inventory. `None` = unrestricted terminal.
    pub(crate) allowed_tools: Option<Vec<String>>,
    /// Embedded-host findings budget: executed research tools per turn beyond
    /// which further calls get a "synthesize now" tool error instead of
    /// running. `None` = unlimited (terminal default).
    pub(crate) tool_call_budget: Option<u32>,
    /// Executed-tool count for the CURRENT turn (budget accounting only).
    pub(crate) turn_executed_tool_calls: u32,
    /// Every tool call the model issued this turn, with its result once known.
    /// Handed to hosts in `TurnCompleted` so post-turn validation (observed
    /// files, findings digest) can happen without a second stream loop.
    pub(crate) turn_tool_calls: Vec<TurnToolCall>,
    /// Optional presentation filter for the assistant's streamed text: the
    /// block shows `f(raw, is_streaming)` while `streaming_text` /
    /// `session_history` keep the raw model output. Lets a structured-output
    /// host (JSON with an `assistant_message` field) render only the prose.
    pub(crate) display_text_filter: Option<DisplayTextFilter>,
    /// Events produced inside `apply_stream_event` (which has no sink) and
    /// flushed by `poll_stream` right after — today only `TurnCompleted`.
    pub(crate) deferred_events: Vec<CoreEvent>,
    /// Text shown on the next user block when the wire query differs from
    /// what the user typed (an embedded host wraps the message in its own
    /// prompt scaffold). Consumed by the next user-origin turn.
    pub(crate) pending_query_display: Option<String>,
    /// Contract-governed hosts (a Work Contract's worker/reviewer): decides
    /// per tool call whether it runs, needs a live decision, or is denied —
    /// on top of the terminal's own capability checks, replacing the user's
    /// autonomy setting as the source of approval decisions.
    pub(crate) tool_policy: Option<ToolPolicy>,
    /// Stable provider run identity for hosts whose attempt already owns one
    /// (billing idempotency + server checkpoint recovery). Only used with
    /// `allowed_tools`; `None` = a fresh local run id per turn.
    pub(crate) run_id_override: Option<String>,
    /// Tools the embedding host executes (Principal inspection/control,
    /// worker→Principal messaging). Advertised to the provider with the
    /// scoped stream; a call is surfaced as `HostToolCall` instead of being
    /// executed locally.
    pub(crate) host_tools: Vec<HostToolDef>,
    /// Set while a restart-recovered local run is parked on approval cards
    /// and no stream exists yet: the run is not registered in the pipeline's
    /// live table, so approvals commit their result into the run's durable
    /// checkpoint (`start_recovered_local_tool_stream`) and the model resumes
    /// from there. Cleared on the first stream event.
    pub(crate) recovery_tx: Option<mpsc::Sender<Result<PipelineEvent, StreamError>>>,
    /// Staged compacted history from the previous turn:
    /// `Some((compacted, base_len))` where `base_len` is the history length
    /// the snapshot was taken from. Consumed at the next turn start and
    /// merged with entries appended since — never applied blindly, so
    /// in-turn appends are preserved and shrinks discard the stale slot.
    pub(crate) compacted_history_slot: CompactedHistorySlot,
}

impl AgentController {
    pub fn new(deps: ControllerDeps) -> Self {
        let cwd = deps.config.project_root.clone();
        Self {
            cwd,
            deps,
            blocks: Vec::new(),
            next_block_id: 0,
            state: ConversationState::Idle,
            active_conversation_id: None,
            approved_action_id: None,
            approved_command: None,
            tool_round: 0,
            active_session: None,
            streaming_task_id: None,
            streaming_text: String::new(),
            reset_streaming_on_next_text: false,
            thinking_block_id: None,
            loading_block_id: None,
            pending_tool_calls: HashMap::new(),
            queued_approvals: VecDeque::new(),
            message_queue: VecDeque::new(),
            bg_completion_rx: None,
            deferred_bg_summaries: Vec::new(),
            pending_plans: HashMap::new(),
            session_history: Vec::new(),
            last_model_name: String::new(),
            last_system_arc: Arc::new(Mutex::new(String::new())),
            session_store: None,
            checkpoints: CheckpointStore::default(),
            current_query_block_id: None,
            permission_store: PermissionStore::default(),
            session_input_tokens: 0,
            session_cached_input_tokens: 0,
            session_output_tokens: 0,
            current_context_tokens: 0,
            turn_count: 0,
            response_style: ResponseStyle::Default,
            gate_tools_until_plan: false,
            mode_gate: false,
            read_only_mode: false,
            force_command_approval: false,
            allow_terminal_cwd_change: false,
            unattended: false,
            next_query_unattended: false,
            unattended_service_writes: false,
            next_query_service_writes: false,
            role: AgentRole::default(),
            tool_call_count: 0,
            persistence_sender: None,
            pane_uuid: Uuid::new_v4(),
            evidence_nudge_count: 0,
            pending_evidence_nudge: None,
            stream_generation: 0,
            tool_cancelled: Arc::new(AtomicBool::new(false)),
            active_tool_calls: HashSet::new(),
            completed_tool_calls: HashSet::new(),
            agent_modified_files: Vec::new(),
            pending_query_attachments: Vec::new(),
            pending_query_author: None,
            pending_failed_resume: None,
            pending_server_resume_task_id: None,
            allowed_tools: None,
            tool_call_budget: None,
            turn_executed_tool_calls: 0,
            turn_tool_calls: Vec::new(),
            display_text_filter: None,
            deferred_events: Vec::new(),
            pending_query_display: None,
            tool_policy: None,
            run_id_override: None,
            host_tools: Vec::new(),
            recovery_tx: None,
            compacted_history_slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Wire in the persistence writer and pane UUID. Rebinds checkpoints to
    /// the active conversation and re-keys the background-completion sender
    /// from the construction-time UUID (see `session.rs`).
    pub fn set_persistence(
        &mut self,
        pane_uuid: Uuid,
        sender: std::sync::mpsc::SyncSender<dragent_persistence::ModelEvent>,
    ) {
        let old_uuid = self.pane_uuid;
        self.pane_uuid = pane_uuid;
        self.persistence_sender = Some(sender);
        self.bind_checkpoints_for_active_conversation();
        // The completion sender was registered under old_uuid at construction time.
        // Update its key so completions reach the right channel.
        self.deps
            .tools
            .rebind_completion_sender(old_uuid, pane_uuid);
    }

    // ── Pump contract ─────────────────────────────────────────────────────
    // The owner (app `poll_stream`, later the Host) drives the engine through
    // these instead of touching fields: per-event deferred-event cadence,
    // stream-orphan checks, and continuation boot must read the same state
    // the engine mutates. Each is a trivial accessor so the pump stays a
    // faithful move of the old inline code.

    /// Events batched by sink-free handlers (`apply_stream_event`, …),
    /// drained by the owner right after each event — the old
    /// `for event in take(&mut self.deferred_events)` sites.
    pub fn drain_deferred(&mut self) -> Vec<CoreEvent> {
        std::mem::take(&mut self.deferred_events)
    }

    /// Current stream generation for orphan-stream checks.
    pub fn stream_generation(&self) -> u64 {
        self.stream_generation
    }

    /// In-flight task id, for error-card attribution.
    pub fn streaming_task_id(&self) -> Option<String> {
        self.streaming_task_id.clone()
    }

    /// A recovered run is live again: results post normally now.
    pub fn pane_uuid(&self) -> Uuid { self.pane_uuid }

    pub fn clear_recovery_tx(&mut self) {
        self.recovery_tx = None;
    }

    /// Evidence-gate note stashed by the Done handler; consumed by the pump,
    /// which owns the context the continuation needs.
    pub fn take_pending_evidence_nudge(&mut self) -> Option<String> {
        self.pending_evidence_nudge.take()
    }

    /// Latest built system prompt, shared with the background thread.
    pub fn last_system_snapshot(&self) -> String {
        self.last_system_arc
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Shared system-prompt cell, for continuations that must read the
    /// prompt the background thread built (not a snapshot taken now).
    pub fn last_system_arc(&self) -> Arc<std::sync::Mutex<String>> {
        Arc::clone(&self.last_system_arc)
    }

    /// Owner-injected approval preferences (the app loads them from disk;
    /// the engine default is empty so tests stay hermetic).
    pub fn set_permission_store(&mut self, store: PermissionStore) {
        self.permission_store = store;
    }

    /// Create the background-completion channel and register its sender under
    /// this controller's pane id, mirroring construction-time wiring in the
    /// app. Called once by the owner right after `new`.
    pub fn init_bg_completions(&mut self) {
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        self.deps
            .tools
            .register_completion_sender(self.pane_uuid, tx);
        self.bg_completion_rx = Some(rx);
    }
}

// ── Test stubs ──────────────────────────────────────────────────────────────
// Minimal owner implementations so engine unit tests run with zero app or
// host services.

#[cfg(test)]
pub mod test_stubs {
    use super::*;
    use crate::api_types::{AgentQueryResponse, BlockAnalysis};
    use crate::files::{
        EditFileOutcome, GrepResult, ListDirectoryResult, ReadFilesResult, SearchCodebaseResult,
        WriteFileOutcome,
    };

    pub struct StubApi;

    impl AgentApi for StubApi {
        fn submit_query(
            &self,
            request: crate::api_types::AgentQueryRequest,
        ) -> Result<AgentQueryResponse, crate::api::AgentError> {
            // Scripted by query text so mock-backend flows stay testable:
            // "deploy" parks an approval, anything else answers plainly.
            if request.query.contains("deploy") {
                Ok(AgentQueryResponse {
                    conversation_id: Uuid::new_v4(),
                    actions: vec![crate::api_types::AgentAction {
                        id: Uuid::new_v4(),
                        kind: crate::api_types::AgentActionKind::RunCommand {
                            command: "cargo build".to_string(),
                            reason: "verify".to_string(),
                        },
                        requires_approval: true,
                    }],
                })
            } else {
                Ok(AgentQueryResponse {
                    conversation_id: Uuid::new_v4(),
                    actions: vec![crate::api_types::AgentAction::message("mock reply")],
                })
            }
        }

        fn analyze_block(
            &self,
            _request: &crate::project::AnalyzeBlockRequest,
        ) -> Result<BlockAnalysis, crate::api::AgentError> {
            unimplemented!("stub")
        }

        fn continue_conversation(
            &self,
            _conversation_id: Uuid,
            _tool_results: Vec<crate::api_types::AgentAction>,
            _session: &AgentSessionContext,
        ) -> Result<AgentQueryResponse, crate::api::AgentError> {
            unimplemented!("stub")
        }

        fn agent_step(
            &self,
            _request: crate::api_types::AgentStepRequest,
        ) -> Result<crate::api_types::AgentStepResponse, crate::api::AgentError> {
            unimplemented!("stub")
        }
    }

    pub struct StubTools;

    impl crate::runtime::ToolExecutor for StubTools {
        fn run_read_files(&self, _paths: &[PathBuf]) -> ReadFilesResult {
            unimplemented!("stub")
        }
        fn run_grep(&self, _pattern: &str, _path: Option<&std::path::Path>) -> GrepResult {
            unimplemented!("stub")
        }
        fn run_write_file(&self, _path: &std::path::Path, _content: &str) -> WriteFileOutcome {
            unimplemented!("stub")
        }
        fn run_edit_file(
            &self,
            _path: &std::path::Path,
            _old_str: &str,
            _new_str: &str,
        ) -> EditFileOutcome {
            unimplemented!("stub")
        }
        fn run_list_directory(
            &self,
            _path: &std::path::Path,
            _recursive: bool,
        ) -> ListDirectoryResult {
            unimplemented!("stub")
        }
        fn run_search_codebase(
            &self,
            _query: &str,
            _repo_root: &std::path::Path,
        ) -> SearchCodebaseResult {
            unimplemented!("stub")
        }
    }

    impl crate::runtime::ToolHost for StubTools {
        fn set_project_cwd(&self, _cwd: &std::path::Path, _pane_id: Uuid) {}
        fn clear_shell_state(&self, _pane_id: Uuid) {}
        fn kill_command(&self, _task_id: &str) -> bool {
            false
        }
        fn read_paths_for_tool(
            &self,
            _name: &str,
            _args_json: &str,
            _pane_id: Uuid,
        ) -> Vec<PathBuf> {
            Vec::new()
        }
        fn external_read_paths_for_tool(
            &self,
            _name: &str,
            _args_json: &str,
            _pane_id: Uuid,
        ) -> Vec<PathBuf> {
            Vec::new()
        }
        fn grant_external_read_once(&self, _call_id: &str, _roots: Vec<PathBuf>) {}
        fn execute_with_call_id(
            &self,
            _call_id: &str,
            _name: &str,
            _args_json: &str,
            _pane_id: Uuid,
        ) -> (String, bool) {
            (String::new(), false)
        }
        fn register_completion_sender(
            &self,
            _pane_id: Uuid,
            _tx: std::sync::mpsc::SyncSender<BackgroundCompletion>,
        ) {
        }
        fn rebind_completion_sender(&self, _old_pane_id: Uuid, _new_pane_id: Uuid) {}
    }

    pub struct StubTranscripts;

    impl TranscriptStore for StubTranscripts {
        fn load_last(
            &self,
            _cwd: &std::path::Path,
            _pane_uuid: Uuid,
        ) -> Option<crate::session_store::LoadedSession> {
            None
        }
        fn open_new(&self, _cwd: &std::path::Path, _pane_uuid: Uuid) -> Box<dyn TranscriptSession> {
            Box::new(StubSession)
        }
        fn open_existing(&self, _path: PathBuf, _session_id: Uuid) -> Box<dyn TranscriptSession> {
            Box::new(StubSession)
        }
    }

    pub struct StubSession;

    impl TranscriptSession for StubSession {
        fn append(&self, _entry: &crate::session_store::TranscriptEntry) {}
    }

    pub struct StubPipeline;

    impl crate::runtime::PipelineLauncher for StubPipeline {
        fn launch_stream(
            &self,
            _request: crate::runtime::StreamLaunchRequest,
            _tx: crate::runtime::StreamSender,
        ) {
        }
        fn post_tool_result(
            &self,
            _task_id: &str,
            _call_id: &str,
            _output: &str,
            _is_error: bool,
        ) -> Result<(), crate::pipeline::StreamError> {
            Ok(())
        }
        fn cancel_task(&self, _task_id: &str) -> bool {
            false
        }
        fn register_host_tools(&self, _run_id: &str, _defs: Vec<crate::pipeline::HostToolDef>) {}
        fn launch_recovered_run(
            &self,
            _run_id: &str,
            _tx: crate::runtime::StreamSender,
        ) -> Result<(), crate::pipeline::ResumeTaskError> {
            Ok(())
        }
        fn launch_recovered_tool(
            &self,
            _task_id: &str,
            _call_id: &str,
            _output: &str,
            _is_error: bool,
            _tx: crate::runtime::StreamSender,
        ) -> Result<(), crate::pipeline::ResumeTaskError> {
            Ok(())
        }
    }

    pub fn test_deps() -> ControllerDeps {
        ControllerDeps {
            api: Arc::new(StubApi),
            tools: Arc::new(StubTools),
            transcripts: Arc::new(StubTranscripts),
            pipeline: Arc::new(StubPipeline),
            hooks: Arc::new(crate::runtime::NoHooks),
            config: AgentHostConfig::default(),
        }
    }
}
