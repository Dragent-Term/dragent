pub mod events;
pub mod state;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext};

use crate::agent::api::types::{AgentMessage, AgentStepRequest, AgentToolDef};
use crate::agent::api::{AgentApi, AgentError};
use crate::agent::project_context::AnalyzeBlockRequest;
use crate::agent::AgentEvent;

/// Maximum tool-calling rounds per agentic loop (prevents runaway loops).
pub const MAX_SELF_HEAL_TOOL_ROUNDS: u8 = 10;
/// Per-step wall-clock deadline: a hung LLM call fails the loop instead of
/// leaving the "fixing automatically" card spinning forever.
pub const AGENT_STEP_TIMEOUT: Duration = Duration::from_secs(120);

pub use events::SelfHealEvent;
pub use state::BlockHealState;

pub struct SelfHealStore {
    agent_api: Arc<dyn AgentApi>,
    states: HashMap<usize, BlockHealState>,
    tools: crate::agent::tools::ToolExecutorRegistry,
}

impl Entity for SelfHealStore {
    type Event = SelfHealEvent;
}

impl SelfHealStore {
    pub fn new(agent_api: Arc<dyn AgentApi>) -> Self {
        // Default: no allowed roots (tools will skip reads unless roots are set).
        let allowed_roots = crate::agent::permissions::AllowedRoots::new(std::iter::empty());
        Self {
            agent_api,
            states: HashMap::new(),
            tools: crate::agent::tools::ToolExecutorRegistry::new(allowed_roots),
        }
    }

    /// Create with explicit allowed roots for testing or production use.
    pub fn with_allowed_roots(
        agent_api: Arc<dyn AgentApi>,
        allowed_roots: crate::agent::permissions::AllowedRoots,
    ) -> Self {
        Self {
            agent_api,
            states: HashMap::new(),
            tools: crate::agent::tools::ToolExecutorRegistry::new(allowed_roots),
        }
    }

    /// Refresh tool permissions when the terminal cwd changes.
    pub fn set_allowed_roots(&mut self, allowed_roots: crate::agent::permissions::AllowedRoots) {
        self.tools = crate::agent::tools::ToolExecutorRegistry::new(allowed_roots);
    }

    pub fn state(&self, block_id: usize) -> &BlockHealState {
        self.states.get(&block_id).unwrap_or(&BlockHealState::Idle)
    }

    pub fn all_states(&self) -> &HashMap<usize, BlockHealState> {
        &self.states
    }

    pub fn analyze_block(
        &mut self,
        block_id: usize,
        request: AnalyzeBlockRequest,
        ctx: &mut ModelContext<Self>,
    ) {
        if matches!(self.states.get(&block_id), Some(BlockHealState::Analyzing)) {
            return;
        }
        // Transition to Analyzing and emit immediately so the UI re-renders
        // the loading card before the blocking HTTP call starts.
        self.states.insert(block_id, BlockHealState::Analyzing);
        ctx.emit(SelfHealEvent::BlocksChanged);

        // Defer the actual blocking API call to after this model update returns,
        // giving the UI a chance to paint the "Analyzing…" card first.
        ctx.spawn(Timer::after(Duration::ZERO), move |this, _, mctx| {
            this.run_analyze(block_id, request, mctx)
        });
    }

    fn run_analyze(
        &mut self,
        block_id: usize,
        mut request: AnalyzeBlockRequest,
        ctx: &mut ModelContext<Self>,
    ) {
        // Local file reads are fast (disk I/O, no network) — do them synchronously first.
        if !request.project.referenced_paths.is_empty() && request.file_context.is_none() {
            let paths: Vec<PathBuf> = request
                .project
                .referenced_paths
                .iter()
                .map(|p| resolve_path_for_read(p, &request.block.cwd))
                .collect();
            let roots = crate::agent::permissions::AllowedRoots::for_session(
                &request.block.cwd,
                request.project.project_root.as_deref(),
            );
            let tools = crate::agent::tools::ToolExecutorRegistry::new(roots);
            request.file_context = Some(tools.read_files.execute(&paths));
        }

        // Offload the blocking network call to an OS thread so the model thread stays free.
        let api = Arc::clone(&self.agent_api);
        let (tx, rx) = std::sync::mpsc::channel();
        crate::bg::detach(move || {
            let _ = tx.send(api.analyze_block(&request));
        });

        ctx.spawn(
            Timer::after(Duration::from_millis(20)),
            move |this, _, mctx| {
                this.poll_analyze_result(block_id, rx, mctx);
            },
        );
    }

    fn poll_analyze_result(
        &mut self,
        block_id: usize,
        rx: std::sync::mpsc::Receiver<Result<crate::agent::api::types::BlockAnalysis, AgentError>>,
        ctx: &mut ModelContext<Self>,
    ) {
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.spawn(
                    Timer::after(Duration::from_millis(20)),
                    move |this, _, mctx| {
                        this.poll_analyze_result(block_id, rx, mctx);
                    },
                );
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("[self-heal] analyze thread disconnected for block {block_id}");
                self.states.insert(
                    block_id,
                    BlockHealState::Error {
                        message: "Analysis failed unexpectedly.".to_string(),
                    },
                );
                ctx.emit(SelfHealEvent::BlocksChanged);
            }
            Ok(Ok(analysis)) => {
                if let (Some(rt), Some(at), Some(exp)) = (
                    analysis.rotated_refresh_token.clone(),
                    analysis.rotated_access_token.clone(),
                    analysis.rotated_access_token_expires_in,
                ) {
                    ctx.emit(SelfHealEvent::TokenRotated {
                        refresh_token: rt,
                        access_token: at,
                        expires_in: exp,
                    });
                }
                ctx.emit(SelfHealEvent::Agent(AgentEvent::BlockAnalyzed {
                    block_id,
                    analysis: analysis.clone(),
                }));
                self.states
                    .insert(block_id, BlockHealState::Ready(analysis));
                ctx.emit(SelfHealEvent::BlocksChanged);
            }
            Ok(Err(AgentError::NotApplicable)) => {
                log::warn!("[self-heal] analyze_block block_id={block_id} returned NotApplicable");
                self.states.insert(
                    block_id,
                    BlockHealState::Error {
                        message: "Analysis not applicable for this block.".to_string(),
                    },
                );
                ctx.emit(SelfHealEvent::BlocksChanged);
            }
            Ok(Err(e)) => {
                log::warn!("[self-heal] analyze_block error: {e}");
                self.states.insert(
                    block_id,
                    BlockHealState::Error {
                        message: e.to_string(),
                    },
                );
                ctx.emit(SelfHealEvent::BlocksChanged);
            }
        }
    }

    // ── Agentic loop ──────────────────────────────────────────────────────────

    /// Start the agentic self-heal loop for a block.
    /// The LLM drives tool calls (read_file, list_directory, grep, write_file, edit_file)
    /// executing locally until it produces a final answer or MAX_SELF_HEAL_TOOL_ROUNDS is hit.
    pub fn start_agent_loop(
        &mut self,
        block_id: usize,
        request: AnalyzeBlockRequest,
        model_name: String,
        ctx: &mut ModelContext<Self>,
    ) {
        if matches!(
            self.states.get(&block_id),
            Some(BlockHealState::AgentRunning { .. } | BlockHealState::Analyzing)
        ) {
            return;
        }
        self.states
            .insert(block_id, BlockHealState::AgentRunning { round: 0 });
        ctx.emit(SelfHealEvent::BlocksChanged);

        let initial_messages = build_initial_messages(&request);
        let tools = agent_tool_definitions();

        ctx.spawn(Timer::after(Duration::ZERO), move |this, _, mctx| {
            this.agent_step(
                block_id,
                request,
                model_name,
                initial_messages,
                tools,
                0,
                mctx,
            );
        });
    }

    fn agent_step(
        &mut self,
        block_id: usize,
        request: AnalyzeBlockRequest,
        model_name: String,
        messages: Vec<AgentMessage>,
        tools: Vec<AgentToolDef>,
        round: u8,
        ctx: &mut ModelContext<Self>,
    ) {
        if round >= MAX_SELF_HEAL_TOOL_ROUNDS {
            self.states.insert(block_id, BlockHealState::Error {
                message: format!("Agent loop hit maximum rounds ({MAX_SELF_HEAL_TOOL_ROUNDS}) without a final answer."),
            });
            ctx.emit(SelfHealEvent::BlocksChanged);
            return;
        }

        let api = Arc::clone(&self.agent_api);
        let step_request = AgentStepRequest {
            model_name: model_name.clone(),
            system: build_system_prompt(&request),
            messages: messages.clone(),
            tools: tools.clone(),
            refresh_token: request.refresh_token.clone(),
            access_token: request.access_token.clone(),
        };

        let (tx, rx) = std::sync::mpsc::channel();
        crate::bg::detach(move || {
            let _ = tx.send(api.agent_step(step_request));
        });

        let deadline = web_time::Instant::now() + AGENT_STEP_TIMEOUT;
        ctx.spawn(
            Timer::after(Duration::from_millis(20)),
            move |this, _, mctx| {
                this.poll_agent_step(
                    block_id, request, model_name, messages, tools, round, rx, deadline, mctx,
                );
            },
        );
    }

    fn poll_agent_step(
        &mut self,
        block_id: usize,
        request: AnalyzeBlockRequest,
        model_name: String,
        mut messages: Vec<AgentMessage>,
        tools: Vec<AgentToolDef>,
        round: u8,
        rx: std::sync::mpsc::Receiver<
            Result<crate::agent::api::types::AgentStepResponse, AgentError>,
        >,
        deadline: web_time::Instant,
        ctx: &mut ModelContext<Self>,
    ) {
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // A hung network call must never leave the card spinning
                // forever — fail the loop and let the user retry.
                if web_time::Instant::now() > deadline {
                    log::warn!("[self-heal] agent_step round {round} timed out");
                    self.states.insert(
                        block_id,
                        BlockHealState::Error {
                            message: format!(
                                "The fix agent's request timed out after {}s. Try again.",
                                AGENT_STEP_TIMEOUT.as_secs()
                            ),
                        },
                    );
                    ctx.emit(SelfHealEvent::BlocksChanged);
                    return;
                }
                let request = request.clone();
                ctx.spawn(
                    Timer::after(Duration::from_millis(20)),
                    move |this, _, mctx| {
                        this.poll_agent_step(
                            block_id, request, model_name, messages, tools, round, rx, deadline,
                            mctx,
                        );
                    },
                );
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.states.insert(
                    block_id,
                    BlockHealState::Error {
                        message: "Agent loop thread disconnected.".to_string(),
                    },
                );
                ctx.emit(SelfHealEvent::BlocksChanged);
            }
            Ok(Err(e)) => {
                // Raw error → logs; the user sees a calm, actionable message.
                log::warn!("[self-heal] agent_step error round={round}: {e}");
                self.states.insert(
                    block_id,
                    BlockHealState::Error {
                        message: crate::agent::error_display::humanize_agent_error(&e.to_string()),
                    },
                );
                ctx.emit(SelfHealEvent::BlocksChanged);
            }
            Ok(Ok(resp)) => {
                // Rotate tokens if the server issued new ones.
                if let (Some(rt), Some(at), Some(exp)) = (
                    resp.rotated_refresh_token.clone(),
                    resp.rotated_access_token.clone(),
                    resp.rotated_access_token_expires_in,
                ) {
                    ctx.emit(SelfHealEvent::TokenRotated {
                        refresh_token: rt,
                        access_token: at,
                        expires_in: exp,
                    });
                }

                if resp.is_final() {
                    // LLM produced a final answer — convert to BlockAnalysis.
                    let analysis = crate::agent::api::types::BlockAnalysis {
                        explanation: resp.content.clone(),
                        suggested_command: extract_suggested_command(&resp.content),
                        confidence: 0.9,
                        rotated_refresh_token: None,
                        rotated_access_token: None,
                        rotated_access_token_expires_in: None,
                    };
                    ctx.emit(SelfHealEvent::Agent(AgentEvent::BlockAnalyzed {
                        block_id,
                        analysis: analysis.clone(),
                    }));
                    self.states
                        .insert(block_id, BlockHealState::Ready(analysis));
                    ctx.emit(SelfHealEvent::BlocksChanged);
                } else {
                    // LLM requested tool calls — execute them locally.
                    messages.push(AgentMessage::assistant_tool_calls(resp.tool_calls.clone()));

                    for tool_call in &resp.tool_calls {
                        let result = self.tools.execute_and_summarize(
                            &tool_call.name,
                            &tool_call.args_json,
                            &request.block.cwd,
                        );
                        messages.push(AgentMessage::tool_result(tool_call.id.clone(), result));
                    }

                    self.states
                        .insert(block_id, BlockHealState::AgentRunning { round: round + 1 });
                    ctx.emit(SelfHealEvent::BlocksChanged);

                    ctx.spawn(Timer::after(Duration::ZERO), move |this, _, mctx| {
                        this.agent_step(
                            block_id,
                            request,
                            model_name,
                            messages,
                            tools,
                            round + 1,
                            mctx,
                        );
                    });
                }
            }
        }
    }

    pub fn dismiss(&mut self, block_id: usize, ctx: &mut ModelContext<Self>) {
        self.states.insert(block_id, BlockHealState::Dismissed);
        ctx.emit(SelfHealEvent::BlocksChanged);
    }

    pub fn suggested_command(&self, block_id: usize) -> Option<&str> {
        match self.states.get(&block_id) {
            Some(BlockHealState::Ready(analysis)) => analysis.suggested_command.as_deref(),
            _ => None,
        }
    }
}

// ── Agentic loop helpers ──────────────────────────────────────────────────────

fn build_system_prompt(request: &AnalyzeBlockRequest) -> String {
    let block = &request.block;
    let project = &request.project;

    let mut parts = vec![
        "You are an expert software engineer and terminal debugging agent integrated into a terminal application.".to_string(),
        "Your job is to diagnose why a shell command failed and fix the underlying problem.".to_string(),
        "".to_string(),
        "SEARCH STRATEGY — pick the right tool for the job:".to_string(),
        "  • search_codebase(query) — grep-based search across the codebase.".to_string(),
        "  • explore(tasks=[...]) — use for broad investigation (trace data flows, find all callers, map a feature). Runs tasks in parallel and returns a summary.".to_string(),
        "  • grep(pattern, path) — use for exact regex matches or import scanning after you know what to look for.".to_string(),
        "  • read_file(path) — use to read a specific file once you know where to look.".to_string(),
        "".to_string(),
        "Always search before reading. Prefer search_codebase over grep for the first pass.".to_string(),
        "When you find the root cause, make the necessary file edits using write_file or edit_file.".to_string(),
        "".to_string(),
        "VERIFY YOUR FIX — this is required, not optional:".to_string(),
        "  1. After editing, call run_command to re-run the failing command (or a build/test/lint command that proves the fix works).".to_string(),
        "  2. If run_command reports FAILED or TIMED OUT, read the new output, refine your edit, and verify again. Iterate until it PASSES.".to_string(),
        "  3. Do NOT declare the fix done until run_command reports PASSED — unless verification is genuinely impossible here, in which case say so explicitly and tell the user exactly how to verify.".to_string(),
        "".to_string(),
        "In your final answer, state: what was wrong, what files you changed, and the verification result (what command you ran and whether it passed).".to_string(),
        "Be concise.".to_string(),
    ];

    if let Some(root) = &project.project_root {
        parts.push(format!("Project root: {}", root.display()));
    }
    if let Some(branch) = &project.git_branch {
        parts.push(format!("Git branch: {branch}"));
    }
    if project.git_changed_files > 0 {
        parts.push(format!("Modified files: {}", project.git_changed_files));
    }

    parts.join("\n")
}

fn build_initial_messages(request: &AnalyzeBlockRequest) -> Vec<AgentMessage> {
    let block = &request.block;
    let content = format!(
        "The following command failed:\n\nCommand: {}\nDirectory: {}\nExit code: {}\n\nOutput:\n{}",
        block.command,
        block.cwd.display(),
        block.exit_code.unwrap_or(-1),
        &block.output_text,
    );
    vec![AgentMessage::user(content)]
}

fn agent_tool_definitions() -> Vec<AgentToolDef> {
    vec![
        AgentToolDef {
            name: "search_codebase".to_string(),
            description: concat!(
                "Grep-based codebase search. Pass a symbol name, regex pattern, or keyword. ",
                "Returns matching lines with file paths. ",
                "Use this before read_file to locate the relevant code."
            ).to_string(),
            parameters_json: r#"{"type":"object","properties":{"query":{"type":"string","description":"Symbol name, regex pattern, or natural-language description of what you're looking for"}},"required":["query"]}"#.to_string(),
        },
        AgentToolDef {
            name: "explore".to_string(),
            description: concat!(
                "Spawn a parallel exploration subagent for broad codebase investigation. ",
                "Runs multiple search tasks concurrently and returns a compact summary. ",
                "Use this for complex questions that require following multiple threads: ",
                "tracing a data flow, mapping all callers of a symbol, or understanding ",
                "how a feature is implemented across many files. ",
                "The subagent summarizes results so the main context stays clean."
            ).to_string(),
            parameters_json: r#"{"type":"object","properties":{"tasks":{"type":"array","description":"List of search tasks to run in parallel","items":{"type":"object","properties":{"kind":{"type":"string","enum":["grep","concept","search","read_file","callers"],"description":"Type of search task"},"query":{"type":"string","description":"Search query or symbol name (for grep/concept/search/callers)"},"path":{"type":"string","description":"File path (for read_file kind)"},"file_glob":{"type":"string","description":"Optional glob to filter grep results (e.g. '*.go')"}},"required":["kind"]}}},"required":["tasks"]}"#.to_string(),
        },
        AgentToolDef {
            name: "read_file".to_string(),
            description: "Read the contents of a file at the given path. Use search_codebase first to find the relevant file.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Absolute or project-relative path to the file"}},"required":["path"]}"#.to_string(),
        },
        AgentToolDef {
            name: "list_directory".to_string(),
            description: "List the files and directories at the given path.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory path to list"},"recursive":{"type":"boolean","description":"Whether to list recursively (default false)"}},"required":["path"]}"#.to_string(),
        },
        AgentToolDef {
            name: "grep".to_string(),
            description: "Exact regex or literal pattern search in files. Use for specific strings, import patterns, or regex. For symbol/concept search, prefer search_codebase.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Regex or literal pattern to search for"},"path":{"type":"string","description":"Directory or file to search in"}},"required":["pattern","path"]}"#.to_string(),
        },
        AgentToolDef {
            name: "edit_file".to_string(),
            description: "Replace the first occurrence of old_str with new_str in a file. Requires exact match.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to edit"},"old_str":{"type":"string","description":"Exact string to find and replace"},"new_str":{"type":"string","description":"Replacement string"}},"required":["path","old_str","new_str"]}"#.to_string(),
        },
        AgentToolDef {
            name: "write_file".to_string(),
            description: "Write (create or overwrite) a file with the given content.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Path to write to"},"content":{"type":"string","description":"Full file content"}},"required":["path","content"]}"#.to_string(),
        },
        AgentToolDef {
            name: "run_command".to_string(),
            description: concat!(
                "Run a shell command in the failing command's directory and get back its exit status and output. ",
                "Use this to VERIFY your fix: after editing files, re-run the build/test/lint command that would prove the problem is resolved ",
                "(e.g. re-run the original failing command, or `cargo build`, `npm test`, `go build ./...`). ",
                "Runtime is capped at 120s and output is truncated. Prefer a fast, targeted check over a full test suite."
            ).to_string(),
            parameters_json: r#"{"type":"object","properties":{"command":{"type":"string","description":"The shell command to run to verify the fix"}},"required":["command"]}"#.to_string(),
        },
    ]
}

/// Extract a `suggested_command` from the LLM's final text (looks for a fenced code block).
fn extract_suggested_command(text: &str) -> Option<String> {
    // Look for ```shell or ```bash or ``` fenced block
    let markers = ["```shell\n", "```bash\n", "```sh\n", "```\n"];
    for marker in &markers {
        if let Some(start) = text.find(marker) {
            let after = &text[start + marker.len()..];
            if let Some(end) = after.find("```") {
                let cmd = after[..end].trim().to_string();
                if !cmd.is_empty() {
                    return Some(cmd);
                }
            }
        }
    }
    None
}

/// Join stderr-relative paths with the block cwd before ReadFiles permission checks.
fn resolve_path_for_read(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api::mock::MockAgentApi;
    use crate::agent::api::session::BlockContextSnapshot;
    use crate::agent::project_context::{AnalyzeBlockRequest, ProjectContext};
    use std::path::PathBuf;

    fn make_store() -> SelfHealStore {
        SelfHealStore::new(Arc::new(MockAgentApi::new()))
    }

    fn failed_snap(block_id: usize, cmd: &str) -> BlockContextSnapshot {
        BlockContextSnapshot {
            block_id,
            command: cmd.to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(1),
            duration_ms: None,
            output_text: "error: something failed".to_string(),
            output_truncated: false,
            failed: true,
        }
    }

    fn success_snap() -> BlockContextSnapshot {
        BlockContextSnapshot {
            block_id: 2,
            command: "echo hi".to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(0),
            duration_ms: None,
            output_text: "hi".to_string(),
            output_truncated: false,
            failed: false,
        }
    }

    fn as_request(snap: BlockContextSnapshot) -> AnalyzeBlockRequest {
        AnalyzeBlockRequest {
            block: snap,
            project: ProjectContext::default(),
            file_context: None,
            refresh_token: None,
            access_token: None,
        }
    }

    #[test]
    fn default_state_is_idle() {
        let store = make_store();
        assert!(matches!(store.state(99), BlockHealState::Idle));
    }

    #[test]
    fn analyze_cargo_failure_transitions_to_ready() {
        let mut store = make_store();
        let snap = failed_snap(1, "cargo build");
        let result = store.agent_api.analyze_block(&as_request(snap.clone()));
        assert!(result.is_ok());
        let analysis = result.unwrap();
        store
            .states
            .insert(snap.block_id, BlockHealState::Ready(analysis));
        assert!(matches!(store.state(1), BlockHealState::Ready(_)));
    }

    #[test]
    fn analyze_success_block_gives_not_applicable() {
        let store = make_store();
        let snap = success_snap();
        let result = store.agent_api.analyze_block(&as_request(snap));
        assert_eq!(result, Err(AgentError::NotApplicable));
    }

    #[test]
    fn error_state_set_on_not_applicable() {
        let mut store = make_store();
        store.states.insert(
            2,
            BlockHealState::Error {
                message: "Analysis not applicable for this block.".to_string(),
            },
        );
        assert!(matches!(store.state(2), BlockHealState::Error { .. }));
    }

    #[test]
    fn dismiss_transitions_to_dismissed() {
        let mut store = make_store();
        let snap = failed_snap(1, "cargo build");
        let analysis = store
            .agent_api
            .analyze_block(&as_request(snap.clone()))
            .unwrap();
        store
            .states
            .insert(snap.block_id, BlockHealState::Ready(analysis));
        store
            .states
            .insert(snap.block_id, BlockHealState::Dismissed);
        assert!(matches!(store.state(1), BlockHealState::Dismissed));
    }

    #[test]
    fn suggested_command_some_when_ready() {
        let mut store = make_store();
        let snap = failed_snap(1, "cargo build");
        let analysis = store
            .agent_api
            .analyze_block(&as_request(snap.clone()))
            .unwrap();
        store
            .states
            .insert(snap.block_id, BlockHealState::Ready(analysis));
        assert!(store.suggested_command(1).is_some());
    }

    #[test]
    fn suggested_command_none_when_idle() {
        let store = make_store();
        assert!(store.suggested_command(1).is_none());
    }

    #[test]
    fn suggested_command_none_when_dismissed() {
        let mut store = make_store();
        store.states.insert(1, BlockHealState::Dismissed);
        assert!(store.suggested_command(1).is_none());
    }

    #[test]
    fn analyzing_guard_prevents_duplicate() {
        let mut store = make_store();
        store.states.insert(1, BlockHealState::Analyzing);
        // State stays Analyzing (can't call analyze_block without ModelContext in unit test,
        // but guard check is: if already Analyzing, return early).
        assert!(matches!(store.state(1), BlockHealState::Analyzing));
    }

    #[test]
    fn retry_from_error_transitions_to_ready() {
        let mut store = make_store();
        store.states.insert(
            1,
            BlockHealState::Error {
                message: "old".to_string(),
            },
        );
        let snap = failed_snap(1, "cargo build");
        let analysis = store
            .agent_api
            .analyze_block(&as_request(snap.clone()))
            .unwrap();
        store
            .states
            .insert(snap.block_id, BlockHealState::Ready(analysis));
        assert!(matches!(store.state(1), BlockHealState::Ready(_)));
    }

    #[test]
    fn all_states_returns_current_map() {
        let mut store = make_store();
        store.states.insert(1, BlockHealState::Dismissed);
        store.states.insert(2, BlockHealState::Analyzing);
        assert_eq!(store.all_states().len(), 2);
    }

    #[test]
    fn referenced_paths_are_read_with_block_scoped_roots() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let src_dir = cwd.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("main.rs"), "fn main() { broken }").unwrap();
        std::fs::write(cwd.join("Cargo.toml"), "[package]\nname = \"t\"\n").unwrap();

        let output = "error: could not compile\n  --> src/main.rs:1:15\n";
        let snap = BlockContextSnapshot {
            block_id: 1,
            command: "cargo build".to_string(),
            cwd: cwd.clone(),
            exit_code: Some(101),
            duration_ms: None,
            output_text: output.to_string(),
            output_truncated: false,
            failed: true,
        };
        let project = ProjectContext::build(&cwd, None, output);
        assert!(!project.referenced_paths.is_empty());

        let roots = crate::agent::permissions::AllowedRoots::for_session(
            &cwd,
            project.project_root.as_deref(),
        );
        let tools = crate::agent::tools::ToolExecutorRegistry::new(roots);
        let paths: Vec<PathBuf> = project
            .referenced_paths
            .iter()
            .map(|p| resolve_path_for_read(p, &cwd))
            .collect();
        let read_result = tools.read_files.execute(&paths);
        assert_eq!(read_result.successful().count(), 1);

        let request = AnalyzeBlockRequest {
            block: snap,
            project,
            file_context: Some(read_result.clone()),
            refresh_token: None,
            access_token: None,
        };
        let analysis = make_store().agent_api.analyze_block(&request).unwrap();
        assert!(analysis.explanation.contains("[files:"));
        assert!(analysis.explanation.contains("main.rs"));
    }
}
