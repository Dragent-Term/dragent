//! Terminal rendering: shared grid paint, alt-screen, and block list.
//!
//! The native-free shell/command-block renderer lives in the shared
//! `dragent_viewer` crate (re-exported below so `crate::terminal::render::…`
//! paths keep resolving). The agent-card render files stay here — they depend on
//! the app's agent block types.

pub mod agent_block_approval;
pub mod agent_block_chrome;
pub mod agent_block_element;
pub mod agent_block_message;
pub mod agent_block_steps;
pub mod agent_presentation;
pub mod block_card_element;
pub mod block_list_element;
pub mod block_list_geom;
pub mod block_list_header;
pub mod block_list_paint;
pub mod block_list_scroll;
pub mod block_list_select;
pub mod self_heal_element;

// Shell-render files moved to dragent_viewer.
pub use dragent_viewer::terminal::render::{
    alt_screen_element, block_grid_painter, block_output, color_sampler, dir_listing_paint,
    glyph_cache, grid_metrics, grid_renderer, markdown, mermaid_render, scrim_overlay,
    term_colors, terminal_size_element,
};
// …and their re-exported items.
pub use dragent_viewer::terminal::render::{
    build_alt_screen_ui, char_index_at_terminal_column, line_terminal_column_count,
    sample_ai_chrome, terminal_char_width, terminal_column_at_char_index, terminal_column_center_x,
    x_to_terminal_column, AiChromeColors, CellGlyphCache, CellMetrics, DiagramCanvasState,
    TerminalSizeElement,
};
// The render context is generic over the in-block editor type; the app always
// uses its `InputEditor`, so alias it so existing `BlockRenderContext<'a>` paths
// stay unchanged. (The wasm viewer uses the default `NoEditor`.)
pub type BlockRenderContext<'a> =
    dragent_viewer::terminal::render::BlockRenderContext<'a, crate::workspace::InputEditor>;

pub use crate::console::dir_listing::dir_grid_content_width;
pub use agent_block_element::build_agent_block_element;
pub use block_card_element::{build_sticky_block_header, StickyBlockHeader, BLOCK_HEADER_HEIGHT};
pub use block_list_element::{
    build_block_options_menu, build_bootstrap_grid_ui, build_link_options_menu,
    build_visible_block_list_column, extract_prompt_tail, BlockListScrollAdapter,
    LivePromptSection, BLOCK_LIST_BOTTOM_POSITION_ID, BLOCK_MENU_WIDTH,
};

pub mod agent_transcript;
