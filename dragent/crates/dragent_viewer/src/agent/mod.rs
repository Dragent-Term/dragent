//! Native-free agent types shared with the app: the agent role and chat mode
//! (and, later, the agent block model the viewer renders). The heavy agent
//! machinery (pipeline, MCP, attachments) stays in the app crate.

pub mod chat_mode;
pub mod controller;
pub use dragent_agent::edit_diff;
pub mod effort;
