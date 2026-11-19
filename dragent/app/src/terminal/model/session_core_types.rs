use crate::theme::Theme;
use dragent_pty::PtyFeedStats;
use dragent_terminal::{
    Emulator, HookEvent, MouseEvent, MouseReportingMode, SemanticMarker, TermEvent, TermMode,
};
use std::path::PathBuf;

use super::alt_screen_state::AltScreenState;
use super::block::Block;
use super::block_context::block_context_from_lines;
use super::block_list::BlockList;
use super::events::TerminalEvent;
use super::input_mode::InputMode;
use super::selection::{selection_text, CellSelection};
use super::size::{SizeInfo, SizeReason, SizeUpdate};
use std::sync::Arc;

pub(crate) fn peak_rss_mb() -> f64 {
    runtime_metrics::process_memory().peak_rss_mb
}

/// Current resident set size of this process, in MB. Unlike [`peak_rss_mb`] this
/// can go DOWN, so it distinguishes a transient allocation spike (peak high, current
/// low again) from sustained growth / a leak (current stays high). Diagnostic only.
pub fn current_rss_mb() -> f64 {
    runtime_metrics::process_memory().current_rss_mb
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellLinkStatus {
    Connecting,
    Running,
    Exited(i32),
    Error,
}

/// Side effects produced while feeding the PTY on a background thread.
#[derive(Debug, Clone)]
pub enum SessionEffect {
    Ui(TerminalEvent),
    PtyWrite(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct ActiveClearGap {
    pub original_height_px: f32,
    pub current_height_px: f32,
    pub before_block_id: Option<usize>,
    pub after_all: bool,
    /// The `clear`/`cls` command's own block — the gap stands in for it
    /// visually, so it must never also render as its own (empty) block entry.
    /// `None` for a gap restored from a persisted session (the persistence
    /// format doesn't carry this yet); those fall back to the old behavior.
    pub trigger_block_id: Option<usize>,
}

/// A restored pane measures itself before it knows what is running in its
/// daemon session, so those sizes must never resize the live process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostResizeGate {
    Live,
    /// No checkpoint yet: sizes apply locally only.
    AwaitingCheckpoint,
    /// Checkpoint adopted the daemon's size; measurements taken before the
    /// view saw the restored state are stale and ignored.
    AwaitingView,
}

/// Output gaps shorter than this keep one run of output going.
pub(crate) const OUTPUT_RUN_GAP: std::time::Duration = std::time::Duration::from_millis(3500);

pub struct TerminalSessionCore {
    pub pending_effects: Vec<SessionEffect>,

    /// Bootstrap / prompt emulator used before the first block and between
    /// lifecycle-bound command blocks.
    pub(crate) session: Emulator,
    pub(crate) alt_ui: AltScreenState,
    pub(crate) blocks: BlockList,
    pub(crate) input_mode: InputMode,
    pub(crate) size: SizeInfo,
    pub(crate) link_status: ShellLinkStatus,
    pub(crate) alt_screen_was_active: bool,
    /// The theme colour queries are answered from, once it has changed since
    /// the output feed started. `None` means the feed's own theme is current.
    pub(crate) reply_theme: Option<Theme>,
    /// The colours the running program was last told. It chose its own colours
    /// to suit these, and may have no way to hear that they changed.
    pub(crate) app_colors: Option<dragent_host::screen_protocol::TerminalColors>,
    pub(crate) cwd: PathBuf,
    pub(crate) shell_title: Option<String>,

    pub(crate) hooks_active: bool,
    pub(crate) semantic_active: bool,
    pub(crate) bootstrapped: bool,
    /// A host shell (by path) that is still starting: the daemon holds input
    /// until its first prompt, which clears this.
    pub(crate) host_shell_starting: Option<String>,
    /// The active block was started by the app's submit and the shell hasn't
    /// reported running it yet. A host checkpoint showing an idle prompt then
    /// predates the command rather than ending it.
    pub(crate) submitted_unseen: bool,
    /// Shared-session MIRROR: do not build blocks from the byte stream.
    ///
    /// A mirror still FEEDS the emulator (the alt-screen renderer reads it), but
    /// its command blocks come from the host's snapshots
    /// ([`apply_remote_block`]). Without this it had two sources for the same
    /// command — its own re-parse AND the host's — and rendered each command
    /// twice, the duplicate overlapping and breaking scrollback.
    pub(crate) suppress_local_blocks: bool,
    /// The machine a remote project's shell runs on. Folders it reports are
    /// machine paths; they are mapped into the machine's local stand-in so
    /// git, Files and tab context keep one path space.
    pub(crate) remote_machine: Option<String>,
    /// When output last arrived, and when the current unbroken run of output
    /// began (gaps shorter than [`OUTPUT_RUN_GAP`]). A terminal agent draws a
    /// spinner the whole time it works, so a long run means it's working.
    pub(crate) output_at: Option<web_time::Instant>,
    pub(crate) output_run_since: Option<web_time::Instant>,
    /// Whether layout sizes may reach a host-backed PTY yet.
    pub(crate) host_resize_gate: HostResizeGate,
    pub(crate) suppress_startup_output: bool,
    pub(crate) active_block_id: Option<usize>,
    /// Set when the user submits `clear`/`cls`; honored on the next screen
    /// clear regardless of block state, so a stale active_block_id (left by an
    /// un-finalized prior command) can't swallow the clear (the "type clear
    /// twice" bug). Consumed by `should_clear_viewport`.
    pub(crate) clear_submitted: bool,
    /// Windows ConPTY: block whose `precmd` arrived but whose output may still
    /// be flushing. It keeps receiving PTY bytes until the rendered fence, the
    /// next `preexec`, or the view's drain timeout finalises it. Always `None`
    /// on Unix, where `precmd`'s stream position is a reliable boundary.
    pub(crate) draining_block_id: Option<usize>,
    /// Exit code reported by the `precmd` that started the drain.
    pub(crate) draining_exit_code: Option<i32>,
    /// Tail bytes of the previous chunk kept while draining, so a fence split
    /// across two PTY reads is still detected.
    pub(crate) fence_scan_tail: Vec<u8>,
    pub(crate) active_clear_gap: Option<ActiveClearGap>,
}
