use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub enum ConversationState {
    #[default]
    Idle,
    ProcessingQuery,
    Streaming,
    ExecutingTool,
    AwaitingApproval {
        action_id: Uuid,
    },
    /// The agent called `ask_user`; the stream is parked until the user answers
    /// every question in the referenced `UserQuestion` block.
    AwaitingUserChoice {
        action_id: Uuid,
    },
}
