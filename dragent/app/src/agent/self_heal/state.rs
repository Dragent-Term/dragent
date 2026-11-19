use crate::agent::api::types::BlockAnalysis;

#[derive(Clone, Debug)]
pub enum BlockHealState {
    Idle,
    /// Single-shot analysis in progress.
    Analyzing,
    /// Agentic loop is running; `round` = number of tool-calling turns so far.
    AgentRunning {
        round: u8,
    },
    Ready(BlockAnalysis),
    /// Presentation-only: a background (proactive) analysis found a fix but
    /// "Auto-fix on failure" is off, so the full card is held behind a compact
    /// "Fix available" chip until the user reveals it. Produced by the view
    /// when projecting states for render — the store never stores it.
    ReadyCollapsed(BlockAnalysis),
    Error {
        message: String,
    },
    Dismissed,
}

impl Default for BlockHealState {
    fn default() -> Self {
        Self::Idle
    }
}

impl BlockHealState {
    /// How this state should *display* under the "Auto-fix on failure"
    /// setting. Manual analyses (user clicked Analyze) always show in full.
    /// With auto-fix OFF, a proactively-triggered analysis stays quiet:
    /// in-flight and failed states hide entirely, and a found fix collapses
    /// to the "Fix available" chip until the user reveals it.
    pub fn projected_for_display(
        &self,
        auto_fix_enabled: bool,
        was_auto_analyzed: bool,
        user_revealed: bool,
    ) -> BlockHealState {
        if auto_fix_enabled || !was_auto_analyzed || user_revealed {
            return self.clone();
        }
        match self {
            Self::Ready(analysis) => Self::ReadyCollapsed(analysis.clone()),
            Self::Analyzing | Self::AgentRunning { .. } | Self::Error { .. } => Self::Idle,
            other => other.clone(),
        }
    }

    /// Short tag for logs.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Analyzing => "Analyzing",
            Self::AgentRunning { .. } => "AgentRunning",
            Self::Ready(_) => "Ready",
            Self::ReadyCollapsed(_) => "ReadyCollapsed",
            Self::Error { .. } => "Error",
            Self::Dismissed => "Dismissed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis() -> BlockAnalysis {
        BlockAnalysis {
            explanation: "dir missing".to_string(),
            suggested_command: Some("mkdir x".to_string()),
            confidence: 0.9,
            rotated_refresh_token: None,
            rotated_access_token: None,
            rotated_access_token_expires_in: None,
        }
    }

    #[test]
    fn auto_fix_on_shows_everything_unchanged() {
        for st in [
            BlockHealState::Analyzing,
            BlockHealState::Ready(analysis()),
            BlockHealState::Error {
                message: "x".into(),
            },
        ] {
            assert_eq!(st.projected_for_display(true, true, false).tag(), st.tag());
        }
    }

    #[test]
    fn manual_analysis_never_gated() {
        // was_auto_analyzed = false → full display even with auto-fix off.
        let st = BlockHealState::Ready(analysis());
        assert_eq!(st.projected_for_display(false, false, false).tag(), "Ready");
        assert_eq!(
            BlockHealState::Analyzing
                .projected_for_display(false, false, false)
                .tag(),
            "Analyzing"
        );
    }

    #[test]
    fn auto_fix_off_collapses_proactive_results() {
        assert_eq!(
            BlockHealState::Ready(analysis())
                .projected_for_display(false, true, false)
                .tag(),
            "ReadyCollapsed"
        );
        assert_eq!(
            BlockHealState::Analyzing
                .projected_for_display(false, true, false)
                .tag(),
            "Idle"
        );
        assert_eq!(
            BlockHealState::AgentRunning { round: 2 }
                .projected_for_display(false, true, false)
                .tag(),
            "Idle"
        );
        assert_eq!(
            BlockHealState::Error {
                message: "x".into()
            }
            .projected_for_display(false, true, false)
            .tag(),
            "Idle"
        );
        // Dismissed stays dismissed.
        assert_eq!(
            BlockHealState::Dismissed
                .projected_for_display(false, true, false)
                .tag(),
            "Dismissed"
        );
    }

    #[test]
    fn reveal_restores_full_card() {
        assert_eq!(
            BlockHealState::Ready(analysis())
                .projected_for_display(false, true, true)
                .tag(),
            "Ready"
        );
    }
}
