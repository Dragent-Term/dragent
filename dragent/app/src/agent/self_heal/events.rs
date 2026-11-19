use crate::agent::AgentEvent;

#[derive(Clone, Debug)]
pub enum SelfHealEvent {
    BlocksChanged,
    Agent(AgentEvent),
    /// Server rotated tokens during analysis — caller must persist refresh token and cache access token.
    TokenRotated {
        refresh_token: String,
        access_token: String,
        expires_in: u64,
    },
}
