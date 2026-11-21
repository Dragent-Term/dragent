//! Runtime seam between the agent engine and its owner.
//!
//! The engine core emits [`CoreEvent`]s into an [`EventSink`] instead of
//! WarpUI `ctx.emit`; pumps are poll-once functions the owner reschedules;
//! configuration arrives through [`AgentHostConfig`] instead of globals.
//! The app implements these traits against WarpUI + its registries today;
//! the Host will implement them against threads and IPC in later phases.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Cross-layer bus event (query completed, block analyzed, tool executed).
/// Single definition — the app re-exports this path.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A full query response arrived from the backend.
    QueryCompleted {
        response: super::api_types::AgentQueryResponse,
    },
    /// On-demand block analysis completed.
    BlockAnalyzed {
        block_id: usize,
        analysis: super::api_types::BlockAnalysis,
    },
    /// A tool was executed. Carries the raw result for further processing.
    ToolExecuted {
        result: super::files::ToolResultKind,
    },
}

/// Framework-free agent event. Mirrors the UI-side event enum 1:1 (including
/// payloads) so the adapter translation is total and mechanical.
#[derive(Debug, Clone)]
pub enum CoreEvent {
    BlocksChanged,
    ApprovalResolved {
        action_id: uuid::Uuid,
        approved: bool,
    },
    Error {
        message: String,
    },
    SessionExpired,
    BudgetUpdated {
        turns: u32,
        input_tokens: u64,
        output_tokens: u64,
    },
    Agent(AgentEvent),
    TurnCompleted {
        conversation_id: uuid::Uuid,
        assistant_text: String,
        model_name: String,
        tool_calls: Vec<super::types::TurnToolCall>,
    },
    ConsentRequested {
        action_id: uuid::Uuid,
        call_id: String,
        tool_name: String,
        args_json: String,
        reason: String,
    },
    QuestionOpened {
        action_id: uuid::Uuid,
        call_id: String,
        summary: String,
    },
    QuestionAnswered {
        action_id: uuid::Uuid,
    },
    HostToolCall {
        task_id: String,
        call_id: String,
        name: String,
        args_json: String,
    },
    RunCommandInTerminal {
        task_id: String,
        call_id: String,
        command: String,
        args_json: String,
        allow_terminal_cwd_change: bool,
    },
}

/// Destination for core events. WarpUI adapter pushes into `ctx.emit`;
/// the Host will push into its event bus / IPC stream.
pub trait EventSink: Send {
    fn push(&mut self, event: CoreEvent);
}

/// Blanket sink collecting events for tests and headless drivers.
#[derive(Debug, Default)]
pub struct VecSink {
    pub events: Vec<CoreEvent>,
}

impl EventSink for VecSink {
    fn push(&mut self, event: CoreEvent) {
        self.events.push(event);
    }
}

/// Spawns background work. Implemented with `std::thread` on native.
pub trait TaskSpawner: Send + Sync {
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>);
}

/// Fire-and-forget background spawn (mirrors the app's `bg::detach`:
/// real thread on native, no-op on wasm where threads don't exist).
pub fn spawn_background(task: Box<dyn FnOnce() + Send + 'static>) {
    #[cfg(not(target_family = "wasm"))]
    {
        std::thread::spawn(task);
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = task;
    }
}

/// Inline spawner for tests and single-threaded hosts.
pub struct InlineSpawner;

impl TaskSpawner for InlineSpawner {
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>) {
        task();
    }
}

/// Executes agent tools against the real environment. The app implements
/// this with its executor registry; the Host will implement it against its
/// own tool runtime. Mirrors the registry's `run_*` surface.
pub trait ToolExecutor: Send + Sync {
    fn run_read_files(&self, paths: &[std::path::PathBuf]) -> super::files::ReadFilesResult;
    fn run_grep(&self, pattern: &str, path: Option<&std::path::Path>) -> super::files::GrepResult;
    fn run_write_file(
        &self,
        path: &std::path::Path,
        content: &str,
    ) -> super::files::WriteFileOutcome;
    fn run_edit_file(
        &self,
        path: &std::path::Path,
        old_str: &str,
        new_str: &str,
    ) -> super::files::EditFileOutcome;
    fn run_list_directory(
        &self,
        path: &std::path::Path,
        recursive: bool,
    ) -> super::files::ListDirectoryResult;
    fn run_search_codebase(
        &self,
        query: &str,
        repo_root: &std::path::Path,
    ) -> super::files::SearchCodebaseResult;
}

/// Local tool-host environment: pane-scoped shell state, completion
/// registry, and one-shot read grants. Implemented app-side by the
/// local-tools module; owned by the Host in later phases.
pub trait ToolHost: ToolExecutor {
    fn set_project_cwd(&self, cwd: &Path, pane_id: uuid::Uuid);
    fn clear_shell_state(&self, pane_id: uuid::Uuid);
    fn kill_command(&self, task_id: &str) -> bool;
    fn read_paths_for_tool(&self, name: &str, args_json: &str, pane_id: uuid::Uuid)
        -> Vec<PathBuf>;
    fn external_read_paths_for_tool(
        &self,
        name: &str,
        args_json: &str,
        pane_id: uuid::Uuid,
    ) -> Vec<PathBuf>;
    /// Files a tool would change outside the project: these ask in every
    /// approval mode, and the executor refuses them without this call's grant.
    fn external_write_paths_for_tool(
        &self,
        _name: &str,
        _args_json: &str,
        _pane_id: uuid::Uuid,
    ) -> Vec<PathBuf> {
        Vec::new()
    }
    fn grant_external_read_once(&self, call_id: &str, roots: Vec<PathBuf>);
    fn execute_with_call_id(
        &self,
        call_id: &str,
        name: &str,
        args_json: &str,
        pane_id: uuid::Uuid,
    ) -> (String, bool);
    fn register_completion_sender(
        &self,
        pane_id: uuid::Uuid,
        tx: std::sync::mpsc::SyncSender<super::background::BackgroundCompletion>,
    );
    fn rebind_completion_sender(&self, old_pane_id: uuid::Uuid, new_pane_id: uuid::Uuid);
}

/// Durable JSONL transcript store factory. Implemented app-side by the
/// session store; the Host will implement it against its persistence
/// authority.
pub trait TranscriptStore: Send + Sync {
    fn load_last(
        &self,
        cwd: &Path,
        pane_uuid: uuid::Uuid,
    ) -> Option<super::session_store::LoadedSession>;
    fn open_new(&self, cwd: &Path, pane_uuid: uuid::Uuid) -> Box<dyn TranscriptSession>;
    fn open_existing(&self, path: PathBuf, session_id: uuid::Uuid) -> Box<dyn TranscriptSession>;
}

/// One open transcript file. Implemented app-side by the session store.
pub trait TranscriptSession: Send {
    fn append(&self, entry: &super::session_store::TranscriptEntry);
}

/// Launches provider-stream turns and posts tool results back. Implemented
/// app-side by the pipeline functions; the Host will implement it against
/// its provider runtime.
pub trait PipelineLauncher: Send + Sync {
    /// Spawn the provider stream for one turn. The implementation owns the
    /// background thread and delivers `PipelineEvent`s into `tx`.
    fn launch_stream(&self, request: StreamLaunchRequest, tx: StreamSender);
    fn post_tool_result(
        &self,
        task_id: &str,
        call_id: &str,
        output: &str,
        is_error: bool,
    ) -> Result<(), super::pipeline::StreamError>;
    fn cancel_task(&self, task_id: &str) -> bool;
    fn register_host_tools(&self, run_id: &str, defs: Vec<super::pipeline::HostToolDef>);
    fn launch_recovered_run(
        &self,
        run_id: &str,
        tx: StreamSender,
    ) -> Result<(), super::pipeline::ResumeTaskError>;
    fn launch_recovered_tool(
        &self,
        task_id: &str,
        call_id: &str,
        output: &str,
        is_error: bool,
        tx: StreamSender,
    ) -> Result<(), super::pipeline::ResumeTaskError>;
}

/// Channel delivering pipeline stream items into the engine pump.
pub type StreamSender =
    std::sync::mpsc::Sender<Result<super::pipeline::PipelineEvent, super::pipeline::StreamError>>;

/// Everything the core needs to launch one provider-stream turn. Prepared by
/// the core (history, prompts, budgets); executed by the owner.
#[derive(Debug, Clone)]
pub struct StreamLaunchRequest {
    pub model_name: String,
    pub system: String,
    pub query: String,
    pub history: Vec<super::types::SessionMessage>,
    pub mode: String,
    pub allowed_tools: Option<Vec<String>>,
    pub run_id: Option<String>,
    /// Structured server resume, when recovering (`(task_id, context)`).
    pub resume: Option<(String, String)>,
}

/// Background agent hooks (memory extraction, session consolidation,
/// scoped verification). Implemented app-side; no-op by default so hosts
/// without these services still run.
pub trait AgentHooks: Send + Sync {
    fn looks_like_directive(&self, _msg: &str) -> bool {
        false
    }
    fn try_extract_and_save(
        &self,
        _model_name: &str,
        _history: &[super::types::SessionMessage],
        _cwd: &Path,
    ) {
    }
    fn on_session_start(&self, _model_name: &str, _cwd: &Path) {}
    fn is_scoped_check(&self, _pane_id: uuid::Uuid, _args_json: &str) -> bool {
        false
    }
}

/// No-op hooks for tests and minimal hosts.
pub struct NoHooks;

impl AgentHooks for NoHooks {}

/// Host-injected configuration replacing global reads (`AppSettings`,
/// `PermissionStore::load`, env) inside engine code.
#[derive(Debug, Clone)]
pub struct AgentHostConfig {
    /// Provider/model routing preferences (opaque to the core).
    pub model_route: String,
    /// Whether agent autonomy shortcuts are enabled.
    pub auto_run_safe_commands: bool,
    /// The user's approval mode, set by the host.
    pub approval_mode: super::autonomy::ApprovalMode,
    /// Project root the engine may operate in.
    pub project_root: std::path::PathBuf,
    /// Extra environment for spawned processes.
    pub env: HashMap<String, String>,
}

impl Default for AgentHostConfig {
    fn default() -> Self {
        Self {
            model_route: "auto".to_string(),
            auto_run_safe_commands: false,
            approval_mode: super::autonomy::ApprovalMode::default(),
            project_root: std::path::PathBuf::from("."),
            env: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_sink_collects_core_events_headless() {
        let mut sink = VecSink::default();
        sink.push(CoreEvent::BlocksChanged);
        sink.push(CoreEvent::ApprovalResolved {
            action_id: uuid::Uuid::nil(),
            approved: true,
        });
        assert_eq!(sink.events.len(), 2);
        assert!(matches!(sink.events[0], CoreEvent::BlocksChanged));
    }

    #[test]
    fn host_config_defaults_are_safe() {
        let config = AgentHostConfig::default();
        assert_eq!(config.model_route, "auto");
        assert!(!config.auto_run_safe_commands);
        assert_eq!(config.approval_mode, crate::autonomy::ApprovalMode::Ask);
    }
}
