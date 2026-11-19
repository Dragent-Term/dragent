//! Production terminal stack — see `docs/architecture/overview.md`.

#[cfg(any(test, feature = "performance-harness"))]
pub mod dev_window;
pub mod event_listener;
pub mod input;
pub mod manager;
pub mod model;
pub(crate) mod output_activity;
pub(crate) mod performance;
pub mod render;
pub mod view;
pub mod viewport;

pub use event_listener::TerminalEventListener;
pub use model::{TerminalEvent, TerminalModel};
pub use view::terminal_view::TerminalView;
