/// Cross-layer agent event bus.
/// `QueryCompleted` is emitted by `AgentController` after each response.
/// `BlockAnalyzed` is reserved for Layer 5 self-heal UI.
pub use dragent_agent::runtime::AgentEvent;
