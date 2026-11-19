//! Terminal block model (native-free) shared with the desktop app.

pub mod alt_screen_state;
pub mod block;
pub mod block_context;
pub mod block_list;
pub mod events;
pub mod input_mode;
pub mod selection;
pub mod size;

// Block-size constants. Relocated here from `block_height_tree` (which stays in
// the app for now — it has a dir-listing render edge) so the native-free model
// can reference them. The app re-exports them at `crate::terminal::model::…`.
pub const BLOCK_HEADER_HEIGHT_PX: f32 = 70.0;
/// Maximum retained rows per command block. This is intentionally large enough
/// for long agent/build responses while preventing an unbounded process from
/// retaining terminal history forever.
pub const LIVE_BLOCK_MAX_RENDER_LINES: usize = 10_000;
pub const COMPLETED_BLOCK_MAX_LINES: usize = 10_000;

pub use block_context::{
    block_context_from_lines, is_signal_exit, BlockContext, BLOCK_CONTEXT_MAX_OUTPUT_BYTES,
};
pub use events::TerminalEvent;
pub use size::{SizeInfo, SizeReason, SizeUpdate};
