//! Background tool completion records shared by the engine core and the Host.
//! Delivery registries stay owner-side; the record itself is plain data.

/// Completion of a fire-and-forget background command execution.
#[derive(Debug, Clone)]
pub struct BackgroundCompletion {
    pub task_id: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
}
