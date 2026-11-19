/// Commands sent from the UI thread to the PTY background session.
#[derive(Debug)]
pub enum PtyCommand {
    Write(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    /// Send SIGINT (Ctrl+C) to the foreground process GROUP — reliably stops
    /// wrappers like `npm run …` whose child test runners a bare 0x03 to the
    /// tty might not reach if the wrapper doesn't forward the signal.
    Interrupt,
    /// Firmly STOP the foreground job and every process in its group: SIGINT
    /// (cooperative) then SIGTERM (for SIGINT-ignoring dev servers like
    /// `yarn dev` → `next dev` → node). The interactive shell is in a different
    /// group, so it — and its cd/env — survives.
    Terminate,
    /// Flow control: stop (`true`) or resume (`false`) reading the PTY's
    /// output. While paused the kernel buffer fills and the program blocks on
    /// its next write instead of output being dropped; input, resize and
    /// close keep working.
    PauseRead(bool),
    Shutdown,
}

/// Events emitted from the PTY background session to the application.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    ProcessExited(i32),
    Error(String),
}
