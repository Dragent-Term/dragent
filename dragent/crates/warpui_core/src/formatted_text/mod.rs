//! Formatted text types and markdown parsing for `FormattedTextElement`.
//!
//! MIT-licensed original implementation for `warpui_core`.

mod parser;
mod types;
pub mod weight;

pub use parser::{
    parse_html, parse_image_prefix, parse_image_run_line, parse_inline_markdown, parse_markdown,
    parse_markdown_with_gfm_tables,
};
pub use types::*;
