//! Versioned wire contract for synchronized agentic-workspace state.
//!
//! This protocol is distinct from the shared-terminal protocol in the parent
//! module. Workspace commands express user intent and are idempotent by command
//! id. Workspace events are immutable facts ordered within one work item.
//!
//! Hub: submodule declarations + re-exports. Command, event, error and test
//! content lives in the `workspace_*` submodules.

pub use super::workspace_commands::*;
pub use super::workspace_error::*;
pub use super::workspace_events::*;

/// First independently-versioned agentic-workspace wire contract.
pub const WORKSPACE_PROTOCOL_VERSION: u32 = 1;
