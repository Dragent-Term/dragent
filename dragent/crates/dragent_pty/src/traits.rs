//! Cross-platform PTY I/O traits (EventedPty pattern).

use std::io;

/// Child process exited (exit code when available).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExit {
    Exited(i32),
}

/// Token ids shared by the PTY event loops.
#[cfg(unix)]
pub const CHANNEL_TOKEN: usize = 0;
pub const PTY_IO_TOKEN: usize = 1;
pub const CHILD_TOKEN: usize = 2;

/// Pseudoterminal that integrates with the platform event loop.
pub trait EventedPty: Send {
    fn resize(&self, rows: u16, cols: u16) -> io::Result<()>;
    /// SIGINT the child's process group (Ctrl+C). Default no-op for platforms
    /// that don't support it.
    fn interrupt_child(&self) {}
    /// Firmly terminate the foreground job's process group (SIGINT + SIGTERM)
    /// without killing the shell. Defaults to [`interrupt_child`] on platforms
    /// that can't signal a group.
    ///
    /// [`interrupt_child`]: Self::interrupt_child
    fn terminate_child(&self) {
        self.interrupt_child();
    }
    fn hangup_child(&self) {
        self.kill_child();
    }
    fn kill_child(&self);
    #[allow(dead_code)]
    fn try_child_exit(&mut self) -> Option<ChildExit>;
}
