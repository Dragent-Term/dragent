//! Native-free render + model layer shared by the desktop app (`dragent`) and the
//! WASM web viewer (`dragent_web_viewer`), so both draw the identical terminal UI
//! from one source of truth — no drift, no reimplementation.
//!
//! Everything here must compile to `wasm32-unknown-unknown`: no PTY, MCP, ONNX,
//! process spawning, or platform-native crates. Modules are lifted from the app
//! crate leaf-first; the app re-exports them (`pub use dragent_viewer::…`) so its
//! existing `crate::…` paths keep resolving.

pub mod agent;
pub mod console;
pub mod icons;
pub mod terminal;
pub mod theme;
