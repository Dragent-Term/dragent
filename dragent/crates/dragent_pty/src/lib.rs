//! Cross-platform pseudo-terminal management for Dragent.
//!
//! PTY I/O runs on a single background event loop per session:
//! - Unix: mio + non-blocking PTY fd + SIGCHLD
//! - Windows: polling + unblocked ConPTY pipes (Alacritty Apache pattern)

// Pure modules (all targets): plain data types + string helpers, no native deps.
mod ansi;
mod messages;
mod platform;
mod traits;

// Native modules: the real PTY implementation (mio / portable-pty / libc /
// tokio). Gated out on wasm, where `wasm_stub` supplies their public API.
#[cfg(not(target_family = "wasm"))]
mod event_loop;
#[cfg(unix)]
mod mio_channel;
#[cfg(not(target_family = "wasm"))]
mod session;
#[cfg(not(target_family = "wasm"))]
mod shell_env;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

// Wasm stubs for the native modules' public API (unreachable no-ops).
#[cfg(target_family = "wasm")]
mod wasm_stub;

pub use ansi::strip_for_display;

/// Wait (up to `timeout`) for close-time kill escalations still running, so a
/// process that exits right after closing its shells doesn't abandon them
/// mid-chain. Returns whether they all finished. No-op off Unix.
pub fn wait_for_pending_kills(timeout: std::time::Duration) -> bool {
    #[cfg(unix)]
    {
        unix::process_tree::wait_for_pending_kills(timeout)
    }
    #[cfg(not(unix))]
    {
        let _ = timeout;
        true
    }
}
pub use async_channel::Receiver;
pub use messages::{PtyCommand, PtyEvent};
pub use platform::{LineEnding, SessionPlatform};

#[cfg(not(target_family = "wasm"))]
pub use event_loop::{
    FeedCallback, PtyFeedStats, SyncBufferLenCallback, WakeupCallback, READ_BUFFER_SIZE,
};
#[cfg(unix)]
pub use session::{adopt_interactive, adopt_interactive_paused};
#[cfg(not(target_family = "wasm"))]
pub use session::{
    execute_command, spawn_interactive, spawn_interactive_with_args, CommandResult, PtyConfig,
    PtySessionHandle,
};
#[cfg(not(target_family = "wasm"))]
pub use shell_env::{available_shells, configure_std_shell_command, resolve_shell, ShellChoice};
#[cfg(windows)]
pub use shell_env::fresh_windows_path;

#[cfg(target_family = "wasm")]
pub use wasm_stub::{
    available_shells, configure_std_shell_command, execute_command, resolve_shell,
    spawn_interactive, spawn_interactive_with_args, CommandResult, FeedCallback, PtyConfig,
    PtyFeedStats, PtySessionHandle, ShellChoice, SyncBufferLenCallback, WakeupCallback,
    READ_BUFFER_SIZE,
};

#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;

/// Runs a single shell command in a fresh PTY and returns captured output.
#[cfg(not(target_family = "wasm"))]
pub async fn run_command(command: String, cwd: Option<PathBuf>) -> CommandResult {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = execute_command(&command, cwd);
        let _ = tx.send(result);
    });
    rx.await.unwrap_or_else(|e| CommandResult {
        output: vec![format!("internal error: {e}")],
        exit_code: 1,
    })
}
