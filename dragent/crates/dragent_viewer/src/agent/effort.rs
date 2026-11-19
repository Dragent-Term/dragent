//! Reasoning effort for an agent turn — the Low / Medium / High selector that
//! sits in the input bar next to the mode and model pickers.
//!
//! Effort controls how much the model reasons before answering. Providers spell
//! it differently on the wire (Anthropic nests it under `output_config.effort`,
//! OpenAI-compatible APIs take a top-level `reasoning_effort`), so the client
//! only ever sends the level name and lets the server map it.
//!
//! `Default` means "send nothing and take the provider's own default", so a
//! user who never touches the control keeps today's behaviour exactly.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentEffort {
    #[default]
    Default,
    Low,
    Medium,
    High,
}

impl AgentEffort {
    /// The value sent to the server. `None` = omit the field entirely.
    pub fn wire_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Auto",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// Hint shown under each option in the dropdown.
    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Default => "Let the model decide",
            Self::Low => "Fastest and cheapest — short, scoped tasks",
            Self::Medium => "Balanced depth and cost",
            Self::High => "Deepest reasoning — slower, more tokens",
        }
    }

    pub const ALL: [Self; 4] = [Self::Default, Self::Low, Self::Medium, Self::High];

    /// The picker rows to render for `model_id`, in display order.
    ///
    /// Effort-capable models get four rows (the bare model plus one per level);
    /// everything else gets a single `Default` row. `Default` leads because it
    /// means "whatever the provider defaults to", which is not a fixed level —
    /// Anthropic and Grok 4.5 default to high, Gemini flash-lite to minimal —
    /// so slotting it in among the levels would misstate it.
    pub fn rows_for(model_id: &str) -> &'static [Self] {
        if Self::model_supports(model_id) {
            &Self::ALL
        } else {
            &[Self::Default]
        }
    }

    /// Whether `model_id` accepts a reasoning-effort level. Drives whether the
    /// picker is shown at all.
    ///
    /// KEEP IN SYNC with `ModelSupportsEffort` in
    /// `server/internal/agent/providers/effort.go`. The server is the real
    /// authority — it drops the field for unsupported models, so a stale client
    /// can never fail a request — but the client can't ask: there is no
    /// `/api/models` fetch and the model list is hardcoded. This mirror decides
    /// visibility only.
    ///
    /// Exclusions that look like oversights but are deliberate:
    ///   - `claude-haiku-4-5` / `claude-sonnet-4-5` reject the parameter.
    ///   - `gpt-5.6-*` rejects any level but "none" once function tools are
    ///     present, and the agent pipeline always sends tools.
    ///   - `gemini-2.5-*` predates `thinkingLevel` (legacy `thinkingBudget`
    ///     only); sending both is a 400.
    ///   - plain `grok-4` errors on `reasoning_effort`, unlike 4.3 / 4.5.
    pub fn model_supports(model_id: &str) -> bool {
        let m = model_id.trim().to_ascii_lowercase();

        // Anthropic — output_config.effort. Opus 4.5+ and Sonnet 4.6+.
        // Haiku 4.5 and Sonnet 4.5 reject the parameter.
        if m.starts_with("claude-opus-")
            || m.starts_with("claude-fable-")
            || m.starts_with("claude-mythos-")
            || m.starts_with("claude-sonnet-4-6")
            || m.starts_with("claude-sonnet-5")
        {
            return true;
        }
        // OpenAI — reasoning_effort. gpt-5.6 is pinned to "none" whenever
        // function tools are present, which is always here.
        if m.starts_with("gpt-5.6-") {
            return false;
        }
        if m.starts_with("gpt-5.") {
            return true;
        }
        // Google — generationConfig.thinkingConfig.thinkingLevel. Gemini 3.x
        // only; 2.5 has just the legacy thinkingBudget.
        if m.starts_with("gemini-3") {
            return true;
        }
        // xAI — reasoning_effort. Explicit allowlist: plain grok-4 ERRORS on
        // the parameter, unlike 4.3 / 4.5.
        if m.starts_with("grok-4.3") || m.starts_with("grok-4.5") {
            return true;
        }
        // Meta Muse Spark — reasoning effort (Responses `reasoning.effort`,
        // Chat `reasoning_effort`). Reasoning family: levels apply.
        m.starts_with("muse-spark")
    }
}

#[cfg(test)]
mod tests {
    use super::AgentEffort as E;

    #[test]
    fn capability_mirrors_the_server_table() {
        // Anthropic
        assert!(E::model_supports("claude-opus-4-8"));
        assert!(E::model_supports("claude-opus-4-7"));
        // Opus 4.5 takes low/medium/high — it only lacks xhigh/max.
        assert!(E::model_supports("claude-opus-4-5"));
        assert!(E::model_supports("claude-sonnet-4-6"));
        assert!(E::model_supports("claude-fable-5"));
        assert!(!E::model_supports("claude-haiku-4-5"));
        assert!(!E::model_supports("claude-sonnet-4-5"));

        // OpenAI
        assert!(E::model_supports("gpt-5.5"));
        assert!(E::model_supports("gpt-5.4-mini"));
        assert!(!E::model_supports("gpt-5.6-luna"));
        assert!(!E::model_supports("gpt-5.6-terra"));
        assert!(!E::model_supports("gpt-5.6-sol"));

        // Google — 3.x yes, 2.5 no.
        assert!(E::model_supports("gemini-3.1-pro"));
        assert!(E::model_supports("gemini-3.5-flash"));
        assert!(!E::model_supports("gemini-2.5-flash"));

        // xAI — 4.3/4.5 yes; plain grok-4 errors on the parameter.
        assert!(E::model_supports("grok-4.3"));
        assert!(E::model_supports("grok-4.5"));
        assert!(!E::model_supports("grok-4"));
        assert!(!E::model_supports("grok-build-0.1"));

        // Meta Muse Spark — effort-capable reasoning family.
        assert!(E::model_supports("muse-spark-1.3"));
        assert!(E::model_supports("muse-spark-1.1"));
        assert_eq!(E::rows_for("muse-spark-1.3").len(), 4);

        // No mapping.
        assert!(!E::model_supports("llama-4-scout-17b-16e-instruct"));
        assert!(!E::model_supports(""));

        // Matches the server's trim + lowercase normalisation.
        assert!(E::model_supports("  Claude-Opus-4-8 "));
    }

    #[test]
    fn rows_expand_only_for_capable_models() {
        // Capable → the bare model plus one row per level, Default first.
        assert_eq!(
            E::rows_for("claude-opus-4-8"),
            &[E::Default, E::Low, E::Medium, E::High]
        );
        assert_eq!(E::rows_for("gemini-3.1-pro").len(), 4);
        assert_eq!(E::rows_for("grok-4.3").len(), 4);

        // Incapable → exactly one row, and it must be the bare model so the
        // picker never offers a level the request would reject.
        assert_eq!(E::rows_for("claude-haiku-4-5"), &[E::Default]);
        assert_eq!(E::rows_for("gpt-5.6-luna"), &[E::Default]);
        assert_eq!(E::rows_for("gemini-2.5-flash"), &[E::Default]);
        assert_eq!(E::rows_for("grok-4"), &[E::Default]);
    }

    #[test]
    fn wire_values_match_the_server_vocabulary() {
        let wire: Vec<_> = E::ALL.iter().filter_map(|e| e.wire_value()).collect();
        assert_eq!(wire, vec!["low", "medium", "high"]);
        assert_eq!(E::Default.wire_value(), None, "Auto must omit the field");
    }
}
