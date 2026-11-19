//! App-side engine host implementations (`ControllerDeps`).
//!
//! The engine core (`dragent_agent::controller`) is UI-free: configuration
//! arrives through [`ControllerDeps`][dragent_agent::controller::core::ControllerDeps]
//! and events fan out through an [`EventSink`][dragent_agent::runtime::EventSink].
//! This module implements those seams against the app's registries (tool
//! executors, session store, pipeline launchers, memory hooks) so the thin
//! `AgentController` wrapper in `controller/` stays free of service wiring.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dragent_agent::api::AgentApi;
use dragent_agent::controller::core::{AgentController as EngineController, ControllerDeps};
use dragent_agent::controller::session_helpers::resume_error_to_stream_error;
use dragent_agent::files::{
    EditFileOutcome, GrepResult, ListDirectoryResult, ReadFilesResult, SearchCodebaseResult,
    ToolResultKind, WriteFileOutcome,
};
use dragent_agent::permissions::AllowedRoots;
use dragent_agent::pipeline::{HostToolDef, ResumeTaskError, StreamError};
use dragent_agent::runtime::{
    AgentHooks, AgentHostConfig, CoreEvent, EventSink, PipelineLauncher, StreamLaunchRequest,
    StreamSender, ToolExecutor, ToolHost, TranscriptSession, TranscriptStore, VecSink,
};
use dragent_agent::session_store::{LoadedSession, TranscriptEntry};
use dragent_agent::types::SessionMessage;
use uuid::Uuid;
use warpui::ModelContext;

use crate::agent::controller::events::AgentControllerEvent;
use crate::agent::controller::AgentController;
use crate::agent::tools::ToolExecutorRegistry;

// ── Tool host ─────────────────────────────────────────────────────────────────

/// Engine [`ToolHost`] over the app's [`ToolExecutorRegistry`].
///
/// The registry is behind a [`Mutex`] so `set_allowed_roots` can swap it
/// while in-flight tool threads still hold this same [`Arc`] (the engine
/// core keeps its own clone in `ControllerDeps`).
pub struct AppToolHost {
    registry: Mutex<ToolExecutorRegistry>,
}

impl AppToolHost {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self {
            registry: Mutex::new(ToolExecutorRegistry::new(allowed_roots)),
        }
    }

    pub fn set_allowed_roots(&self, allowed_roots: AllowedRoots) {
        *self.registry.lock().unwrap() = ToolExecutorRegistry::new(allowed_roots);
    }

    fn reg<T>(&self, f: impl FnOnce(&ToolExecutorRegistry) -> T) -> T {
        let guard = self.registry.lock().unwrap();
        f(&guard)
    }
}

impl ToolExecutor for AppToolHost {
    fn run_read_files(&self, paths: &[PathBuf]) -> ReadFilesResult {
        match self.reg(|r| r.run_read_files(paths)) {
            ToolResultKind::ReadFiles(out) => out,
            other => {
                panic!("[agent] registry contract violation: run_read_files returned {other:?}")
            }
        }
    }

    fn run_grep(&self, pattern: &str, path: Option<&Path>) -> GrepResult {
        match self.reg(|r| r.run_grep(pattern, path)) {
            ToolResultKind::Grep(out) => out,
            other => {
                panic!("[agent] registry contract violation: run_grep returned {other:?}")
            }
        }
    }

    fn run_write_file(&self, path: &Path, content: &str) -> WriteFileOutcome {
        match self.reg(|r| r.run_write_file(path, content)) {
            ToolResultKind::WriteFile(out) => out,
            other => {
                panic!("[agent] registry contract violation: run_write_file returned {other:?}")
            }
        }
    }

    fn run_edit_file(&self, path: &Path, old_str: &str, new_str: &str) -> EditFileOutcome {
        match self.reg(|r| r.run_edit_file(path, old_str, new_str)) {
            ToolResultKind::EditFile(out) => out,
            other => {
                panic!("[agent] registry contract violation: run_edit_file returned {other:?}")
            }
        }
    }

    fn run_list_directory(&self, path: &Path, recursive: bool) -> ListDirectoryResult {
        match self.reg(|r| r.run_list_directory(path, recursive)) {
            ToolResultKind::ListDirectory(out) => out,
            other => {
                panic!("[agent] registry contract violation: run_list_directory returned {other:?}")
            }
        }
    }

    fn run_search_codebase(&self, query: &str, repo_root: &Path) -> SearchCodebaseResult {
        match self.reg(|r| r.run_search_codebase(query, repo_root)) {
            ToolResultKind::SearchCodebase(out) => out,
            other => {
                panic!(
                    "[agent] registry contract violation: run_search_codebase returned {other:?}"
                )
            }
        }
    }
}

impl ToolHost for AppToolHost {
    fn set_project_cwd(&self, cwd: &Path, pane_id: Uuid) {
        crate::agent::local_tools::set_project_cwd(cwd, pane_id);
    }

    fn clear_shell_state(&self, pane_id: Uuid) {
        crate::agent::local_tools::clear_shell_state(pane_id);
    }

    fn kill_command(&self, task_id: &str) -> bool {
        crate::agent::local_tools::kill_command(task_id)
    }

    fn read_paths_for_tool(&self, name: &str, args_json: &str, pane_id: Uuid) -> Vec<PathBuf> {
        crate::agent::local_tools::read_paths_for_tool(name, args_json, pane_id)
    }

    fn external_read_paths_for_tool(
        &self,
        name: &str,
        args_json: &str,
        pane_id: Uuid,
    ) -> Vec<PathBuf> {
        crate::agent::local_tools::external_read_paths_for_tool(name, args_json, pane_id)
    }

    fn grant_external_read_once(&self, call_id: &str, roots: Vec<PathBuf>) {
        crate::agent::local_tools::grant_external_read_once(call_id, roots);
    }

    fn execute_with_call_id(
        &self,
        call_id: &str,
        name: &str,
        args_json: &str,
        pane_id: Uuid,
    ) -> (String, bool) {
        crate::agent::local_tools::execute_with_call_id(call_id, name, args_json, pane_id)
    }

    fn register_completion_sender(
        &self,
        pane_id: Uuid,
        tx: std::sync::mpsc::SyncSender<dragent_agent::background::BackgroundCompletion>,
    ) {
        crate::agent::local_tools::register_completion_sender(pane_id, tx);
    }

    fn rebind_completion_sender(&self, old_pane_id: Uuid, new_pane_id: Uuid) {
        crate::agent::local_tools::rebind_completion_sender(old_pane_id, new_pane_id);
    }
}

// ── Transcripts ───────────────────────────────────────────────────────────────

/// Engine [`TranscriptStore`] over the app's JSONL [`SessionStore`][crate::agent::session_store::SessionStore].
pub struct AppTranscripts;

impl TranscriptStore for AppTranscripts {
    fn load_last(&self, cwd: &Path, pane_uuid: Uuid) -> Option<LoadedSession> {
        crate::agent::session_store::SessionStore::load_last(cwd, pane_uuid)
    }

    fn open_new(&self, cwd: &Path, pane_uuid: Uuid) -> Box<dyn TranscriptSession> {
        Box::new(AppTranscriptStore(
            crate::agent::session_store::SessionStore::new(cwd, pane_uuid),
        ))
    }

    fn open_existing(&self, path: PathBuf, session_id: Uuid) -> Box<dyn TranscriptSession> {
        Box::new(AppTranscriptStore(
            crate::agent::session_store::SessionStore::continue_from(path, session_id),
        ))
    }
}

/// One open JSONL transcript. Newtype around the store so the orphan rule
/// lets this module implement the foreign [`TranscriptSession`] trait.
pub struct AppTranscriptStore(pub crate::agent::session_store::SessionStore);

impl TranscriptSession for AppTranscriptStore {
    fn append(&self, entry: &TranscriptEntry) {
        self.0.append(entry);
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

/// Engine [`PipelineLauncher`] over the app's pipeline launch functions.
#[derive(Default)]
pub struct AppPipeline {
    /// The terminal tab the agent answers in, as of its latest turn: a tab
    /// that leads coordinated work gets the lead's tools.
    pane: Mutex<Option<Uuid>>,
}

impl AppPipeline {
    pub fn set_pane(&self, pane: Uuid) {
        if let Ok(mut current) = self.pane.lock() {
            *current = Some(pane);
        }
    }

    /// Tools this turn gets beyond everyone's: the lead's, while its tab
    /// leads coordinated work.
    fn turn_tools(&self) -> Vec<serde_json::Value> {
        #[cfg(not(target_family = "wasm"))]
        {
            let pane = self.pane.lock().ok().and_then(|pane| *pane);
            if pane.is_some_and(|pane| crate::cli::team::led_by(&pane.to_string()).is_some()) {
                return crate::cli::team::lead_tool_defs()
                    .into_iter()
                    .map(|(name, description, schema)| {
                        serde_json::json!({
                            "name": name,
                            "description": description,
                            "parameters": serde_json::from_str::<serde_json::Value>(&schema)
                                .unwrap_or(serde_json::json!({"type": "object", "properties": {}})),
                        })
                    })
                    .collect();
            }
        }
        Vec::new()
    }
}

impl PipelineLauncher for AppPipeline {
    fn launch_stream(&self, request: StreamLaunchRequest, tx: StreamSender) {
        use crate::agent::pipeline as pipe;
        let StreamLaunchRequest {
            model_name,
            system,
            query,
            history,
            mode,
            allowed_tools,
            run_id,
            resume,
        } = request;
        // Structured server resume: the run already owns a provider task, so
        // re-enter it instead of opening a fresh stream.
        if let Some((task_id, context)) = resume {
            match pipe::start_resumed_task_stream(&task_id, &model_name, &context, tx.clone()) {
                Ok(()) => {}
                Err(e) => {
                    let _ = tx.send(Err(resume_error_to_stream_error(e)));
                }
            }
            return;
        }
        // A coordination lead owns its model across turns and app restarts.
        // Provider resumes above retain the model of the existing run.
        #[cfg(not(target_family = "wasm"))]
        let model_name = if allowed_tools.is_none() {
            self.pane.lock().ok().and_then(|pane| *pane)
                .and_then(|pane| crate::cli::team::led_by(&pane.to_string()))
                .and_then(|team| team.lead_model)
                .map(|model| if model == crate::settings::AUTO_MODEL_ID { String::new() } else { model })
                .unwrap_or(model_name)
        } else { model_name };
        let turn_tools = if allowed_tools.is_none() {
            self.turn_tools()
        } else {
            Vec::new()
        };
        // Semantic retry: transient open failures (429/5xx) back off and retry
        // before the error-card path. Mirrors the old inline turn bootstrap.
        let started = pipe::start_stream_with_retry("task-stream", || {
            match (&allowed_tools, &run_id) {
                // Contract-governed attempt: its own stable run identity.
                (Some(tools), Some(run_id)) => pipe::start_scoped_task_stream_with_run_id(
                    run_id,
                    &model_name,
                    &system,
                    &query,
                    &history,
                    &mode,
                    tools,
                    tx.clone(),
                ),
                // Embedded host: the provider only sees the host's allowlist.
                (Some(tools), None) => pipe::start_scoped_task_stream(
                    &model_name,
                    &system,
                    &query,
                    &history,
                    &mode,
                    tools,
                    tx.clone(),
                ),
                (None, _) => pipe::start_task_stream(
                    &model_name,
                    &system,
                    &query,
                    &history,
                    &mode,
                    turn_tools.clone(),
                    tx.clone(),
                ),
            }
        });
        if let Err(e) = started {
            log::warn!("[agent] start_task_stream failed: {e}");
            let _ = tx.send(Err(e));
        }
    }

    fn post_tool_result(
        &self,
        task_id: &str,
        call_id: &str,
        output: &str,
        is_error: bool,
    ) -> Result<(), StreamError> {
        crate::agent::pipeline::post_tool_result(task_id, call_id, output, is_error)
    }

    fn cancel_task(&self, task_id: &str) -> bool {
        crate::agent::pipeline::cancel_local_task(task_id)
    }

    fn register_host_tools(&self, run_id: &str, defs: Vec<HostToolDef>) {
        crate::agent::pipeline::register_host_tools(run_id, defs);
    }

    fn launch_recovered_run(&self, run_id: &str, tx: StreamSender) -> Result<(), ResumeTaskError> {
        crate::agent::pipeline::start_recovered_local_run_stream(run_id, tx)
    }

    fn launch_recovered_tool(
        &self,
        task_id: &str,
        call_id: &str,
        output: &str,
        is_error: bool,
        tx: StreamSender,
    ) -> Result<(), ResumeTaskError> {
        // A recovered run's streaming task id IS its run id
        // (`recover_local_run` sets both), so the tool stream commits into
        // the run's durable checkpoint under that id.
        crate::agent::pipeline::start_recovered_local_tool_stream(
            task_id, call_id, output, is_error, tx,
        )
        .map(|_| ())
    }
}

// ── Hooks ─────────────────────────────────────────────────────────────────────

/// Engine [`AgentHooks`] over the app's memory/verification services.
pub struct AppHooks;

impl AgentHooks for AppHooks {
    fn looks_like_directive(&self, msg: &str) -> bool {
        crate::agent::memory_extractor::looks_like_directive(msg)
    }

    fn try_extract_and_save(&self, model_name: &str, history: &[SessionMessage], cwd: &Path) {
        crate::agent::memory_extractor::try_extract_and_save(model_name, history, cwd);
    }

    fn on_session_start(&self, model_name: &str, cwd: &Path) {
        crate::agent::dream_consolidator::on_session_start(model_name, cwd);
    }

    fn is_scoped_check(&self, pane_id: Uuid, args_json: &str) -> bool {
        crate::agent::host_verification::is_scoped_check(pane_id, args_json)
    }
}

// ── Deps factory ──────────────────────────────────────────────────────────────

/// Build the engine [`ControllerDeps`] for a terminal controller plus the
/// shared [`AppToolHost`] behind them (kept by the wrapper for
/// `set_allowed_roots`).
pub fn controller_deps(
    api: Arc<dyn AgentApi>,
    allowed_roots: AllowedRoots,
    cwd: PathBuf,
) -> (ControllerDeps, Arc<AppToolHost>, Arc<AppPipeline>) {
    let tools = Arc::new(AppToolHost::new(allowed_roots));
    let autonomy = crate::settings::AppSettings::global().agent.clone();
    let config = AgentHostConfig {
        model_route: "auto".to_string(),
        auto_run_safe_commands: autonomy.auto_run_safe_commands,
        approval_mode: autonomy.approval_mode(),
        project_root: cwd,
        env: HashMap::new(),
    };
    let pipeline = Arc::new(AppPipeline::default());
    let deps = ControllerDeps {
        api,
        tools: tools.clone(),
        transcripts: Arc::new(AppTranscripts),
        pipeline: pipeline.clone(),
        hooks: Arc::new(AppHooks),
        config,
    };
    (deps, tools, pipeline)
}

// ── Event translation ─────────────────────────────────────────────────────────

/// Total 1:1 translation of engine events to UI events (payloads included).
pub fn translate_event(event: CoreEvent) -> AgentControllerEvent {
    match event {
        CoreEvent::BlocksChanged => AgentControllerEvent::BlocksChanged,
        CoreEvent::ApprovalResolved {
            action_id,
            approved,
        } => AgentControllerEvent::ApprovalResolved {
            action_id,
            approved,
        },
        CoreEvent::Error { message } => AgentControllerEvent::Error { message },
        CoreEvent::SessionExpired => AgentControllerEvent::SessionExpired,
        CoreEvent::BudgetUpdated {
            turns,
            input_tokens,
            output_tokens,
        } => AgentControllerEvent::BudgetUpdated {
            turns,
            input_tokens,
            output_tokens,
        },
        CoreEvent::Agent(e) => AgentControllerEvent::Agent(e),
        CoreEvent::TurnCompleted {
            conversation_id,
            assistant_text,
            model_name,
            tool_calls,
        } => AgentControllerEvent::TurnCompleted {
            conversation_id,
            assistant_text,
            model_name,
            tool_calls,
        },
        CoreEvent::ConsentRequested {
            action_id,
            call_id,
            tool_name,
            args_json,
            reason,
        } => AgentControllerEvent::ConsentRequested {
            action_id,
            call_id,
            tool_name,
            args_json,
            reason,
        },
        CoreEvent::QuestionOpened {
            action_id,
            call_id,
            summary,
        } => AgentControllerEvent::QuestionOpened {
            action_id,
            call_id,
            summary,
        },
        CoreEvent::QuestionAnswered { action_id } => {
            AgentControllerEvent::QuestionAnswered { action_id }
        }
        CoreEvent::HostToolCall {
            task_id,
            call_id,
            name,
            args_json,
        } => AgentControllerEvent::HostToolCall {
            task_id,
            call_id,
            name,
            args_json,
        },
        CoreEvent::RunCommandInTerminal {
            task_id,
            call_id,
            command,
            args_json,
            allow_terminal_cwd_change,
        } => AgentControllerEvent::RunCommandInTerminal {
            task_id,
            call_id,
            command,
            args_json,
            allow_terminal_cwd_change,
        },
    }
}

// ── Pump ──────────────────────────────────────────────────────────────────────

/// Run one engine call with a fresh sink, then emit the translated events in
/// order. The owner decides notification and (re)pumping from what was
/// emitted; nothing here spawns timers.
pub fn pump(
    ctx: &mut ModelContext<AgentController>,
    engine: &mut EngineController,
    run: impl FnOnce(&mut EngineController, &mut VecSink),
) {
    let mut sink = VecSink::default();
    run(engine, &mut sink);
    for event in sink.events {
        ctx.emit(translate_event(event));
    }
}

/// Emit the engine's deferred events (batched by sink-free handlers such as
/// `apply_stream_event`) in order. Call right after each pumped event, as the
/// old per-event `for event in take(&mut deferred_events)` sites did.
pub fn emit_deferred(ctx: &mut ModelContext<AgentController>, engine: &mut EngineController) {
    for event in engine.drain_deferred() {
        ctx.emit(translate_event(event));
    }
}
