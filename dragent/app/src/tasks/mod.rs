//! The organisation's task list on the Dragent server.
//!
//! The Tasks page reads [`store::TasksStore`]; the command line and the MCP
//! tools go through [`client`]. Everything here speaks the shapes in
//! `dragent_tasks` exactly as `API.md` describes them.

pub mod cache;
pub mod client;
pub mod mcp;
pub(crate) mod project_hints;
pub mod store;

/// Whether this process runs inside an agent: changes it makes are marked
/// `by_agent`, so a task's history shows who really made them.
pub(crate) fn by_agent() -> bool {
    crate::cli::agent_environment()
}
