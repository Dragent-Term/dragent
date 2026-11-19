//! PTY session lifecycle and background I/O.

pub mod pty_writer;
pub mod terminal_manager;
pub(crate) mod transient_host;

pub use terminal_manager::TerminalManager;
