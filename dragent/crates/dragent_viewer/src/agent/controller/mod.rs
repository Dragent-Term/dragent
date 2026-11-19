//! The agent-role type, lifted out of the app's (native) `agent::controller`
//! module so the shared, native-free render/chat-mode layer can reference it.
//! The app re-exports `AgentRole` from here, so its existing
//! `crate::agent::controller::AgentRole` paths keep resolving.

pub use dragent_agent::controller::AgentRole;
