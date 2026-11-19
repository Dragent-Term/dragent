pub mod graphql;
pub mod mock;
pub mod session;
pub use dragent_agent::api_types as types;

use std::sync::Arc;

use crate::agent::project_context::AnalyzeBlockRequest;
pub use session::{AgentSessionContext, BlockContextSnapshot};
pub use types::{
    AgentAction, AgentActionKind, AgentQueryRequest, AgentQueryResponse, BlockAnalysis,
};

// ── Trait + error — single definitions in `dragent_agent::api` shared by
// the engine core, the Host, and the UI.
pub use dragent_agent::api::{AgentApi, AgentError, ENV_AGENT_API};

// ── Factory ───────────────────────────────────────────────────────────────────

const BACKEND_MOCK: &str = "mock";
const BACKEND_GRAPHQL: &str = "graphql";

/// Returns the configured API implementation.
///
/// Reads [`ENV_AGENT_API`] (default `graphql`).
/// unknown values log a warning and fall back to graphql.
pub fn default_agent_api() -> Arc<dyn AgentApi> {
    match std::env::var(ENV_AGENT_API)
        .unwrap_or_else(|_| BACKEND_GRAPHQL.to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        BACKEND_GRAPHQL | "" => Arc::new(graphql::GraphqlAgentApi::new()),
        BACKEND_MOCK => Arc::new(mock::MockAgentApi::new()),
        other => {
            log::warn!(
                "{ENV_AGENT_API}={other:?} is unknown; using graphql backend (supported: {BACKEND_GRAPHQL}, {BACKEND_MOCK})"
            );
            Arc::new(graphql::GraphqlAgentApi::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_request() -> AgentQueryRequest {
        AgentQueryRequest {
            query: "hello".to_string(),
            session: AgentSessionContext {
                cwd: std::path::PathBuf::from("/tmp"),
                recent_blocks: vec![],
                is_autodetected: true,
                project: None,
                timestamp: chrono::Utc::now(),
                os: "macos".to_string(),
                shell: "zsh".to_string(),
                codebases: Vec::new(),
                attached_blocks: Vec::new(),
                skills: Vec::new(),
            },
        }
    }

    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn default_api_is_usable() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::set(ENV_AGENT_API, BACKEND_GRAPHQL);
        assert!(matches!(
            default_agent_api().submit_query(hello_request()),
            Err(AgentError::NotApplicable)
        ));
    }

    #[test]
    fn env_mock_selects_mock_backend() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::set(ENV_AGENT_API, BACKEND_MOCK);
        assert!(default_agent_api().submit_query(hello_request()).is_ok());
    }

    /// Restores the prior env value on drop so tests do not leak state.
    struct EnvGuard {
        key: &'static str,
        prior: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prior = std::env::var(key).ok();
            // SAFETY: tests run single-threaded; guard restores on drop.
            unsafe { std::env::set_var(key, value) };
            Self { key, prior }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prior {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
