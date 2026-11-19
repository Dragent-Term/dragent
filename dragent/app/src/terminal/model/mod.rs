// The native-free block model lives in the shared `dragent_viewer` crate; these
// modules are re-exported so the app's existing `crate::terminal::model::…` paths
// keep resolving. The files below stay in the app for now — they have edges to
// app-only/native code (block_height_tree → dir-listing render; terminal_model →
// project_sidebar; terminal_session_core → dragent_pty; block_share → protocol).
pub use dragent_viewer::terminal::model::{
    alt_screen_state, block, block_context, block_list, events, input_mode, selection, size,
    BLOCK_HEADER_HEIGHT_PX, COMPLETED_BLOCK_MAX_LINES, LIVE_BLOCK_MAX_RENDER_LINES,
};

pub mod block_height_tree;
pub mod block_share;
mod session_core_blocks;
mod session_core_cwd;
mod session_core_pty;
mod session_core_types;
mod session_core_view;
pub mod terminal_model;
pub mod terminal_session_core;

pub use block_context::{
    block_context_from_lines, is_signal_exit, BlockContext, BLOCK_CONTEXT_MAX_OUTPUT_BYTES,
};
pub use block_height_tree::{BlockHeightIndex, BlockListEntry, BlockListSummary};
pub use events::TerminalEvent;
pub use size::{SizeInfo, SizeReason, SizeUpdate};
pub use terminal_model::TerminalModel;
pub use terminal_session_core::{ShellLinkStatus, TerminalSessionCore};
