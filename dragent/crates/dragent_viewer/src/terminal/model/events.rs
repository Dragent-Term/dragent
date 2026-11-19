//! Events emitted by [`TerminalModel`] and consumed by views.

use super::block_context::BlockContext;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum TerminalEvent {
    /// PTY bytes were processed; repaint the output region.
    OutputAppended,
    /// Raw PTY bytes just processed, for a shared session to broadcast to
    /// viewers. `block_id` is the block they were fed to (if any). Only consumed
    /// when this pane is being shared; otherwise the view drops it cheaply.
    OutputChunk {
        block_id: Option<usize>,
        bytes: Vec<u8>,
    },
    /// The block list changed structurally (new block, completion, bootstrap).
    BlocksChanged,
    /// A command block finished execution.
    BlockCompleted { context: Arc<BlockContext> },
    /// Windows ConPTY: a block completed (`precmd` seen) but its output may
    /// still be flushing — the view should schedule a drain-timeout that calls
    /// `finalise_draining_block` as the fallback if no rendered fence arrives.
    BlockDraining,
    /// The terminal entered the alternate screen (DEC 47 / 1049).
    AltEntered,
    /// The terminal exited the alternate screen.
    AltExited,
    /// `cwd` or shell link status changed.
    StatusChanged,
    /// The terminal/application changed its window title. Kept separate from
    /// [`StatusChanged`](Self::StatusChanged) because TUIs may animate titles;
    /// presentation churn must not trigger Git, indexing, or CWD work.
    TitleChanged,
    /// Bytes that must be written back to the PTY (kitty ACK, etc.).
    PtyWrite(Vec<u8>),
    /// The running app copied this text to the clipboard (OSC 52).
    ClipboardWrite(String),
    /// Terminal dimensions changed; PTY may need SIGWINCH.
    SizeChanged,
    /// A host-backed terminal adopted the daemon's screen, size and running
    /// command. Layout must be measured again against the restored state.
    HostStateRestored,
    /// User ran `clear` / Ctrl-L — view should insert a viewport gap and scroll.
    ScreenCleared,
}
