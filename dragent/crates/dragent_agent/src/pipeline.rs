//! Pipeline event types, stream errors, and workspace tool disposition.
//!
//! Pure serde/plain-data contracts between the provider pipeline, the engine
//! core, and the Host. Single definitions — the app re-exports these paths.

use serde::Deserialize;

/// SSE event kinds emitted by the server's agent pipeline.
/// These must match `server/internal/agent/pipeline/events.go`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    #[serde(rename = "agent.thinking")]
    Thinking,
    /// Early notification that the LLM has started a tool call (id + name known, args still streaming).
    /// Display-only — do NOT execute. Full args arrive with ToolCall.
    #[serde(rename = "agent.tool_call_started")]
    ToolCallStarted,
    /// Server asking the client to execute a tool locally and POST back the result.
    #[serde(rename = "agent.tool_call")]
    ToolCall,
    /// Server confirming a tool call is complete (sent after it receives the result).
    #[serde(rename = "agent.tool_end")]
    ToolEnd,
    #[serde(rename = "agent.text_delta")]
    TextDelta,
    #[serde(rename = "agent.done")]
    Done,
    /// A single stateless provider step finished; the desktop owns the next
    /// tool/result/model transition and keeps that state locally.
    #[serde(rename = "agent.step_done")]
    StepDone,
    #[serde(rename = "agent.error")]
    Error,
}

/// Parsed server-sent event from the pipeline stream.
#[derive(Clone, Debug, Deserialize)]
pub struct PipelineEvent {
    pub kind: AgentEventKind,

    // agent.thinking / agent.text_delta
    pub text: Option<String>,

    // agent.tool_call / agent.tool_end
    pub tool_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_icon: Option<String>,
    pub args_summary: Option<String>,

    // agent.tool_call — full args JSON for local execution
    pub tool_args: Option<String>,
    /// If true the client must show an approval UI before executing.
    pub requires_approval: Option<bool>,
    /// If true the SERVER executes this tool itself (web_search / fetch_url via
    /// the backend's provider-native web tooling). Display-only for the client:
    /// render the tool card, do NOT execute or post a result — ToolEnd follows.
    pub server_executed: Option<bool>,

    // agent.tool_end
    pub result_summary: Option<String>,
    pub duration_ms: Option<i64>,
    /// Full output for a server-native web tool in desktop-owned mode. It is
    /// delivered over the live response and is not persisted by the server.
    pub tool_output: Option<String>,
    pub tool_is_error: Option<bool>,

    // agent.step_done
    pub assistant_content: Option<String>,
    pub compacted_messages: Option<Vec<serde_json::Value>>,

    // agent.done
    pub total_input_tokens: Option<i64>,
    pub total_cached_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub rounds: Option<i32>,
    /// Current context size: the prompt tokens (input + cached) of the LAST
    /// round, from the provider's usage — the authoritative "context used" for
    /// the context-window meter. NOT cumulative, unlike the totals above.
    pub context_tokens: Option<i64>,

    // agent.error
    pub error: Option<String>,
}

/// Response from POST /api/v1/agent/tasks
#[derive(Clone, Debug, Deserialize)]
pub struct CreateTaskResponse {
    pub task_id: String,
    pub status: String,
}

/// Stream failure modes for the provider pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamError {
    /// Dragent's own session is gone: the person must sign in again.
    Auth(String),
    /// A model provider refused the person's own API key (or there is none).
    /// Never a Dragent sign-in problem: it must not sign anyone out.
    ProviderKey(String),
    /// HTTP 402 — user has no credits left.
    InsufficientCredits(String),
    /// HTTP 429 — rolling token quota exceeded; inner string includes wait duration.
    RateLimited(String),
    Network(String),
    /// The stream went silent — no events AND no server keepalives within the
    /// idle window. Distinct from `Network`: this is "the backend stopped
    /// talking to us", not a transport-level failure. A healthy server pings
    /// every 20s, so silence means the connection or the backend is dead.
    Timeout(String),
    Parse(String),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(s) => write!(f, "sign in required: {s}"),
            Self::ProviderKey(s) => write!(f, "provider key: {s}"),
            Self::InsufficientCredits(s) => write!(f, "{s}"),
            Self::RateLimited(s) => write!(f, "{s}"),
            Self::Network(s) => write!(f, "network error: {s}"),
            Self::Timeout(s) => write!(f, "timed out: {s}"),
            Self::Parse(s) => write!(f, "parse error: {s}"),
        }
    }
}

/// How a tool call from a workspace-bound turn should be handled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceToolDisposition {
    Execute,
    /// The provider-side operation already completed. The desktop records its
    /// event but must not execute it a second time.
    ObserveServerResult,
    NeedsConsent {
        reason: String,
    },
    Denied {
        reason: String,
    },
}

/// Recovery-route failure modes for resuming parked runs.
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeTaskError {
    /// Backend predates the structured recovery route; caller may use its
    /// local journal compatibility path.
    Unavailable,
    /// Backend found the checkpoint but cannot prove replay is side-effect safe.
    Rejected(String),
    Stream(StreamError),
}

/// A tool the embedding host executes (Principal inspection/control tools,
/// worker→Principal messaging): advertised to the provider like an MCP tool,
/// intercepted by the controller as `HostToolCall`, answered by the host.
#[derive(Clone, Debug)]
pub struct HostToolDef {
    pub name: String,
    pub description: String,
    /// JSON schema for the arguments object.
    pub parameters: serde_json::Value,
    /// False for pure inspection (groups into the "Explored" activity card).
    pub mutating: bool,
}
