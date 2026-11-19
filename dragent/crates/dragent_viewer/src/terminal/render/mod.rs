//! Terminal rendering — the native-free shell/command-block renderer shared with
//! the desktop app. The agent-card render files (agent_block_element,
//! block_list_element, block_card_element, self_heal_element, agent_presentation)
//! stay in the app for now; they depend on the app's agent block types.

pub mod alt_screen_element;
pub mod block_grid_painter;
pub mod block_output;
pub mod color_sampler;
pub mod dir_listing_paint;
pub mod glyph_cache;
pub mod grid_metrics;
pub mod grid_renderer;
pub mod markdown;
pub mod markdown_agent;
pub mod markdown_blocks;
pub mod markdown_inline;
pub mod markdown_tables;
pub mod mermaid_render;
pub mod scrim_overlay;
pub mod term_colors;
pub mod terminal_size_element;

pub use alt_screen_element::build_alt_screen_ui;
pub use block_output::{BlockRenderContext, DiagramCanvasState, NoEditor};
pub use color_sampler::{sample_ai_chrome, AiChromeColors};
pub use glyph_cache::CellGlyphCache;
pub use grid_metrics::{
    char_index_at_terminal_column, line_terminal_column_count, terminal_char_width,
    terminal_column_at_char_index, terminal_column_center_x, x_to_terminal_column, CellMetrics,
};
pub use terminal_size_element::TerminalSizeElement;
