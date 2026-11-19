//! Unix PTY integration (mio + non-blocking dup fd).

pub mod adopt;
mod evented;
pub mod process_tree;

pub use evented::EventedPty;
