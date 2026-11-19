use crate::agent::detected_input_type::DetectedInputType;

/// Session signals passed into the input classifier on each keystroke.
#[derive(Clone, Debug, Default)]
pub struct ClassifyContext {
    /// The pane's current detected type before this classification pass.
    pub current_type: DetectedInputType,
    /// Recent shell commands (newest last), used for prefix/history matching.
    pub recent_commands: Vec<String>,
    /// Whether the most recently completed block exited with failure.
    pub last_block_failed: bool,
    /// True when the user's last submit went to the agent stub (not PTY).
    pub after_agent_submit: bool,
}

impl ClassifyContext {
    pub fn shell_default() -> Self {
        Self::default()
    }
}
