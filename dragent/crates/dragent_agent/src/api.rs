//! Agent backend API surface shared by the engine core and the Host.
//!
//! The `AgentApi` trait is what the core programs against; the GraphQL and
//! mock implementations stay app-side. Request/response/error types live in
//! [`super::api_types`].

use super::api_types::{
    AgentAction, AgentQueryRequest, AgentQueryResponse, AgentStepRequest, AgentStepResponse,
    BlockAnalysis,
};
use super::project::AnalyzeBlockRequest;
use super::session::AgentSessionContext;

/// Supported values for `DRAGENT_AGENT_API` (read app-side by the factory).
pub const ENV_AGENT_API: &str = "DRAGENT_AGENT_API";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// The operation is not applicable (e.g. `analyze_block` on a success block).
    NotApplicable,
    /// Caller-supplied data is invalid.
    InvalidRequest(String),
    /// Backend unreachable or refused connection.
    BackendUnavailable(String),
    /// Too many requests.
    RateLimited,
    /// Unexpected internal error.
    Internal(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotApplicable => write!(f, "not applicable"),
            Self::InvalidRequest(s) => write!(f, "invalid request: {s}"),
            Self::BackendUnavailable(s) => write!(f, "backend unavailable: {s}"),
            Self::RateLimited => write!(f, "rate limited"),
            Self::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}

pub trait AgentApi: Send + Sync {
    fn submit_query(&self, request: AgentQueryRequest) -> Result<AgentQueryResponse, AgentError>;
    fn analyze_block(&self, request: &AnalyzeBlockRequest) -> Result<BlockAnalysis, AgentError>;
    fn continue_conversation(
        &self,
        conversation_id: uuid::Uuid,
        tool_results: Vec<AgentAction>,
        session: &AgentSessionContext,
    ) -> Result<AgentQueryResponse, AgentError>;
    /// Run one turn of the agentic tool-calling loop.
    /// Returns tool_calls for the client to execute (then call again), or a final content string.
    fn agent_step(&self, request: AgentStepRequest) -> Result<AgentStepResponse, AgentError>;
}

/// Returns the configured API implementation kind. App-side factory reads
/// this (default `graphql`); unknown values fall back with a warning.
pub fn api_kind_from_env() -> String {
    std::env::var(ENV_AGENT_API).unwrap_or_else(|_| "graphql".to_string())
}
