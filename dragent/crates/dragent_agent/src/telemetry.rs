//! Control-plane telemetry types (M4): the technical invariant behind
//! "we see that you work, never what you work on".
//!
//! These types make the privacy claim structural instead of promissory:
//! there is no field capable of carrying prompts, file contents, patches,
//! tool outputs, provider keys, model responses, or terminal output —
//! because no such field exists. Provider identity is a closed enum
//! (unknown slugs map to `Unknown`, never forwarded raw); model identity is
//! deliberately absent v1 (provider + counters suffice for cost/usage
//! analytics; model-level detail lives server-side for managed runs).
//!
//! Anything needing richer diagnostics belongs in the explicit,
//! user-consented debug-bundle path — never smuggled through here.

use serde::{Deserialize, Serialize};

/// Closed provider vocabulary. Unknown slugs map to `Unknown` so an
/// unexpected string can never flow into a telemetry payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSlug {
    Anthropic,
    OpenAI,
    Google,
    Xai,
    Groq,
    Cerebras,
    OpenRouter,
    Meta,
    DeepSeek,
    Qwen,
    Zhipu,
    Moonshot,
    MiniMax,
    Mistral,
    Together,
    DeepInfra,
    Perplexity,
    Venice,
    Nvidia,
    Fireworks,
    Cohere,
    Ollama,
    LmStudio,
    Custom,
    Unknown,
}

impl ProviderSlug {
    /// Map a provider id slug. Anything unrecognized becomes `Unknown` —
    /// telemetry must degrade, never echo.
    pub fn from_slug(slug: &str) -> Self {
        match slug {
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAI,
            "google" => Self::Google,
            "xai" => Self::Xai,
            "groq" => Self::Groq,
            "cerebras" => Self::Cerebras,
            "openrouter" => Self::OpenRouter,
            "meta" => Self::Meta,
            "deepseek" => Self::DeepSeek,
            "qwen" | "alibaba" => Self::Qwen,
            "zhipu" | "glm" => Self::Zhipu,
            "moonshot" | "kimi" => Self::Moonshot,
            "minimax" => Self::MiniMax,
            "mistral" => Self::Mistral,
            "together" => Self::Together,
            "deepinfra" => Self::DeepInfra,
            "perplexity" => Self::Perplexity,
            "venice" => Self::Venice,
            "nvidia" => Self::Nvidia,
            "fireworks" => Self::Fireworks,
            "cohere" => Self::Cohere,
            "ollama" => Self::Ollama,
            "lmstudio" => Self::LmStudio,
            "custom" => Self::Custom,
            _ => Self::Unknown,
        }
    }
}

/// Failure classes only — never messages (provider errors can echo queries).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Auth,
    Credits,
    RateLimited,
    Network,
    Timeout,
    Parse,
    LoopGuard,
    Cancelled,
    ApprovalDenied,
}

/// Which execution path served the turn. Metadata, not content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPath {
    /// Provider-direct from the local engine (BYOK key or local server).
    Direct,
    /// Dragent cloud step path (managed credentials).
    Cloud,
    /// Custom endpoint.
    Custom,
}

/// One completed execution. Constructible ONLY from scalars and closed
/// enums — there is no stringly-typed content anywhere in this struct, so
/// `telemetry.prompt = execution.prompt.clone()` cannot even be written.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionTelemetry {
    pub execution_id: uuid::Uuid,
    pub provider: ProviderSlug,
    pub path: ExecutionPath,
    pub started_at_ms: i64,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<ErrorClass>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_cover_every_direct_route() {
        for slug in [
            "anthropic",
            "openai",
            "google",
            "xai",
            "groq",
            "cerebras",
            "openrouter",
            "meta",
            "deepseek",
            "qwen",
            "alibaba",
            "zhipu",
            "glm",
            "moonshot",
            "kimi",
            "minimax",
            "mistral",
            "together",
            "deepinfra",
            "perplexity",
            "venice",
            "nvidia",
            "fireworks",
            "cohere",
            "ollama",
            "lmstudio",
            "custom",
        ] {
            assert_ne!(
                ProviderSlug::from_slug(slug),
                ProviderSlug::Unknown,
                "{slug}"
            );
        }
        assert_eq!(
            ProviderSlug::from_slug("evil-provider"),
            ProviderSlug::Unknown
        );
        assert_eq!(ProviderSlug::from_slug(""), ProviderSlug::Unknown);
    }

    #[test]
    fn serialized_shape_contains_no_content_capable_fields() {
        let event = ExecutionTelemetry {
            execution_id: uuid::Uuid::nil(),
            provider: ProviderSlug::Anthropic,
            path: ExecutionPath::Direct,
            started_at_ms: 1,
            duration_ms: 2,
            success: true,
            error: None,
            input_tokens: 3,
            output_tokens: 4,
            tool_calls: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        for forbidden in [
            "prompt",
            "content",
            "file",
            "patch",
            "key",
            "response",
            "output_text",
            "terminal",
            "message",
            "query",
        ] {
            assert!(
                !json.to_lowercase().contains(forbidden),
                "telemetry shape leaked {forbidden}: {json}"
            );
        }
        // And the type itself offers no such field to set.
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let allowed = [
            "execution_id",
            "provider",
            "path",
            "started_at_ms",
            "duration_ms",
            "success",
            "error",
            "input_tokens",
            "output_tokens",
            "tool_calls",
        ];
        for key in parsed.as_object().unwrap().keys() {
            assert!(allowed.contains(&key.as_str()), "unexpected field {key}");
        }
    }
}
