//! Dragent's clients for driving a coding agent as a chat.
//!
//! A chat needs the same things from every agent: the models it offers (and a
//! way to switch them), its reply as it is written, each tool call with its
//! status, when a turn ends and why, permission questions to answer, and a
//! real cancel. Three ways of getting them, behind one [`ChatDriver`]:
//!
//! - the Agent Client Protocol ([`Agent`]), which OpenCode, Gemini, Qwen,
//!   Cursor and Copilot speak in their ACP mode;
//! - Codex's own app-server protocol ([`Codex`]);
//! - Claude Code's streaming JSON mode ([`Claude`]).
//!
//! All three report in the same [`Event`] / [`Update`] vocabulary, so the chat
//! on top doesn't know which one it talks to. Requests block their caller
//! until the agent answers: call them off the UI thread.

mod claude;
mod codex;
mod jsonrpc;
mod options;
mod rpc;
mod update;

pub use claude::Claude;
pub use codex::Codex;
pub use options::{Choice, Picker, SessionOptions, SetVia};
pub use rpc::{Agent, Event, PermissionOption, PermissionRequest, RpcError};
pub use update::{PlanEntry, ToolCall, ToolContent, ToolPatch, Update};

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    Cancelled,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Other(String),
}

impl StopReason {
    pub fn parse(text: &str) -> Self {
        match text {
            "end_turn" => Self::EndTurn,
            "cancelled" => Self::Cancelled,
            "max_tokens" => Self::MaxTokens,
            "max_turn_requests" => Self::MaxTurnRequests,
            "refusal" => Self::Refusal,
            other => Self::Other(other.to_string()),
        }
    }
}

/// A started session: its id and what can be chosen in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub options: SessionOptions,
}

/// One agent driven as a chat.
pub trait ChatDriver: Send + Sync {
    /// The agent's own messages. Taken once, before [`ChatDriver::open`].
    fn take_events(&self) -> Option<std::sync::mpsc::Receiver<Event>>;
    /// Start talking: continue conversation `resume` when the agent can,
    /// otherwise start a new one in `cwd`.
    fn open(&self, cwd: &std::path::Path, resume: Option<&str>) -> Result<Session, RpcError>;
    /// Whether opening an earlier conversation replays it as events. When
    /// not, the chat shows its history from the agent's transcript.
    fn replays_history(&self) -> bool;
    /// Send a message and wait for the turn to end.
    fn prompt(&self, session_id: &str, text: &str) -> Result<StopReason, RpcError>;
    /// Stop the running turn (without waiting).
    fn cancel(&self, session_id: &str) -> Result<(), RpcError>;
    /// Choose `value` in the picker set `via`; the new options when they changed.
    fn set_option(&self, session_id: &str, via: &SetVia, value: &str) -> Result<Option<SessionOptions>, RpcError>;
    /// Answer a permission request with one of its options (`None`: the turn
    /// is being stopped).
    fn answer_permission(&self, request: &serde_json::Value, option_id: Option<&str>) -> Result<(), RpcError>;
    fn shutdown(&self);
    /// What the agent last wrote to its error output (why it stopped).
    fn error_output(&self) -> Option<String>;
}

/// How Dragent talks to an agent in a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverKind {
    Acp,
    Codex,
    Claude,
}

/// The driver for `provider`, the program it runs and that program's
/// arguments for the chat (Claude's depend on the conversation, see
/// [`Claude::spawn`]).
pub fn driver_for(provider: &str) -> Option<(DriverKind, &'static str, &'static [&'static str])> {
    Some(match provider {
        "claude" => (DriverKind::Claude, "claude", &[]),
        "codex" => (DriverKind::Codex, "codex", &["app-server"]),
        "opencode" => (DriverKind::Acp, "opencode", &["acp"]),
        "gemini" => (DriverKind::Acp, "gemini", &["--acp"]),
        "qwen" => (DriverKind::Acp, "qwen", &["--acp"]),
        "cursor-agent" => (DriverKind::Acp, "cursor-agent", &["acp"]),
        "copilot" => (DriverKind::Acp, "copilot", &["--acp"]),
        _ => return None,
    })
}

/// The command that starts `provider` in its ACP mode, for the agents that
/// speak it themselves.
pub fn launch(provider: &str) -> Option<(&'static str, &'static [&'static str])> {
    driver_for(provider).filter(|(kind, _, _)| *kind == DriverKind::Acp).map(|(_, program, args)| (program, args))
}

/// Start the right driver for `provider`: `program` is the resolved binary,
/// `args` go before the driver's own, `env` is added to the process's.
pub fn start(
    provider: &str,
    program: &std::path::Path,
    args: &[String],
    cwd: &std::path::Path,
    env: &[(String, String)],
) -> std::io::Result<std::sync::Arc<dyn ChatDriver>> {
    let Some((kind, _, own)) = driver_for(provider) else {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, format!("{provider} can't be shown as a chat")));
    };
    let mut all: Vec<String> = args.to_vec();
    all.extend(own.iter().map(|a| a.to_string()));
    Ok(match kind {
        DriverKind::Acp => std::sync::Arc::new(Agent::spawn(program, &all, cwd, env)?),
        DriverKind::Codex => std::sync::Arc::new(Codex::spawn(program, &all, cwd, env)?),
        DriverKind::Claude => std::sync::Arc::new(Claude::spawn(program, &all, env)),
    })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_drivers;
