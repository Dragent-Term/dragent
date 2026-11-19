//! Agent tool display helpers.
//!
//! Single definition lives in `dragent_agent::controller::tools`; re-exported
//! here so existing `crate::agent::controller::tools::*` paths keep resolving.

pub use dragent_agent::controller::tools::{
    humanize_tool_action, is_checkpoint_write_tool, is_read_only_tool_op, strip_mcp_prefix,
    tool_activity_label, tool_display_name,
};
