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
use crate::terminal::model::session_core_cwd::is_default_shell_title;
use crate::terminal::model::session_core_types::ActiveClearGap;
use crate::terminal::model::session_core_types::SessionEffect;
use crate::terminal::model::ShellLinkStatus;
use std::sync::Arc;

impl super::session_core_types::TerminalSessionCore {
    pub fn new(cwd: PathBuf, size: SizeInfo, suppress_startup_output: bool) -> Self {
        let rows = size.rows as usize;
        let cols = size.cols as usize;
        Self {
            pending_effects: Vec::new(),
            session: Emulator::new(rows, cols),
            alt_ui: AltScreenState::default(),
            blocks: BlockList::new(),
            input_mode: InputMode::Idle,
            size,
            link_status: ShellLinkStatus::Connecting,
            alt_screen_was_active: false,
            reply_theme: None,
            app_colors: None,
            cwd,
            shell_title: None,
            hooks_active: false,
            semantic_active: false,
            bootstrapped: false,
            host_shell_starting: None,
            submitted_unseen: false,
            suppress_local_blocks: false,
            remote_machine: None,
            output_at: None,
            output_run_since: None,
            host_resize_gate: super::session_core_types::HostResizeGate::Live,
            suppress_startup_output,
            active_block_id: None,
            clear_submitted: false,
            draining_block_id: None,
            draining_exit_code: None,
            fence_scan_tail: Vec::new(),
            active_clear_gap: None,
        }
    }

    /// Mark this core as a shared-session mirror: blocks arrive from the host
    /// rather than being reconstructed locally. See `suppress_local_blocks`.
    pub fn set_suppress_local_blocks(&mut self, suppress: bool) {
        self.suppress_local_blocks = suppress;
    }

    pub fn set_host_resume_pending(&mut self, pending: bool) {
        use super::session_core_types::HostResizeGate;
        self.host_resize_gate =
            if pending { HostResizeGate::AwaitingCheckpoint } else { HostResizeGate::Live };
    }

    /// The view has seen the restored state; its next measurement is real.
    pub fn acknowledge_host_restore(&mut self) {
        use super::session_core_types::HostResizeGate;
        if self.host_resize_gate == HostResizeGate::AwaitingView {
            self.host_resize_gate = HostResizeGate::Live;
        }
    }

    pub fn blocks(&self) -> &BlockList {
        &self.blocks
    }

    /// The host shell's path while it is still starting.
    pub fn host_shell_starting(&self) -> Option<&str> {
        self.host_shell_starting.as_deref()
    }

    /// Shared-session MIRROR: apply one of the host's block snapshots, which is
    /// the authoritative view.
    ///
    /// A mirror used to rebuild blocks by re-parsing the raw PTY stream, so it
    /// only matched the host while its column count and the shell-hook stream
    /// stayed exactly in step. Any drift wrapped lines at the wrong column or
    /// swallowed a block boundary (output piling into a stale, still-"running"
    /// block), and a terminal cannot re-wrap or re-split what it already parsed.
    /// Taking the host's own snapshot removes that whole failure class — and it
    /// is what lets a late joiner be shown scrollback it never received bytes for.
    ///
    /// Upserts by id: creates the block if new, then overwrites its command, cwd,
    /// output lines, and status from the host. `lines` is `None` for a
    /// start/complete notice that carries no output of its own.
    pub fn apply_remote_block(
        &mut self,
        id: usize,
        command: String,
        cwd: Option<PathBuf>,
        created_at_unix_ms: i64,
        lines: Option<Vec<crate::console::escape_parse::ColoredLine>>,
        dir_entries: Option<Vec<crate::console::dir_listing::DirEntry>>,
        running: bool,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
    ) {
        if self.blocks.get(id).is_none() {
            let size = self.size();
            let cwd_for_new = cwd.clone().unwrap_or_else(|| self.cwd.clone());
            let mut block = Block::new_with_timestamp(
                id,
                command.clone(),
                cwd_for_new,
                size.rows as usize,
                size.cols as usize,
                created_at_unix_ms,
            );
            // The host owns the content; a mirror never runs a PTY, so it must not
            // keep a live emulator that would try to parse into this block.
            block.emulator = None;
            block.remote_filesystem = true;
            self.blocks.begin(block);
        }
        let Some(block) = self.blocks.get_mut(id) else {
            return;
        };
        if !command.is_empty() {
            block.command = command;
        }
        if let Some(cwd) = cwd {
            block.cwd = cwd;
        }
        if let Some(lines) = lines {
            block.lines = std::sync::Arc::new(lines);
            block.emulator = None;
        }
        // Only ever SET this, never clear it. The host re-sends a block's lines
        // repeatedly (while running, then once more on completion) and not every
        // one of those carries the listing — clearing on a delta that omits it
        // would drop a good listing and fall back to scraping, which is what made
        // the entries appear intermittently.
        if dir_entries.is_some() {
            block.dir_entries = dir_entries;
        }
        block.status = if running {
            crate::terminal::model::block::BlockStatus::Running
        } else {
            crate::terminal::model::block::BlockStatus::Completed { exit_code }
        };
        if duration_ms.is_some() {
            block.duration_ms = duration_ms;
        }
    }

    /// Snapshot the block list for broadcasting to shared-session viewers.
    /// Broadcast the retained block snapshot to shared-session viewers. The same
    /// 10,000-row bound is used locally and remotely so both views agree.
    ///
    /// Running blocks snapshot their live emulator; completed blocks reuse their
    /// stored styled lines. `theme` resolves ANSI colors to concrete RGBA.
    /// Blocks whose ids are in `skip` (already finalized to viewers) are omitted
    /// so a per-chunk snapshot doesn't re-clone the full history each time.
    pub fn block_share_snapshot(
        &self,
        theme: &crate::theme::Theme,
        skip: &std::collections::HashSet<usize>,
    ) -> Vec<super::block_share::HostBlockSnapshot> {
        use super::block::BlockStatus;
        self.blocks
            .blocks()
            .iter()
            .filter(|block| !skip.contains(&block.id))
            .map(|block| {
                let running = block.status.is_running();
                let mut lines = match &block.emulator {
                    Some(emu) if running && block.uses_exact_colors() => {
                        crate::console::escape_parse::snapshot_emulator_lines_exact(emu, theme)
                    }
                    Some(emu) if running => {
                        crate::console::escape_parse::snapshot_emulator_lines(emu, theme)
                    }
                    _ => (*block.lines).clone(),
                };
                let max = super::COMPLETED_BLOCK_MAX_LINES;
                if lines.len() > max {
                    lines.drain(0..lines.len() - max);
                }
                let (exit_code, failed) = match block.status {
                    BlockStatus::Completed { exit_code } => (
                        exit_code,
                        block.completion().map(|c| c.failed).unwrap_or_else(|| {
                            super::block_context::failed_from_exit_code(exit_code)
                        }),
                    ),
                    _ => (None, false),
                };
                // Resolve a directory listing HERE, on the host, where the real
                // filesystem is. A mirror cannot: its filesystem belongs to a
                // different machine (or does not exist, on the web), and
                // re-parsing the `ls` text drops entries whose names contain
                // spaces or that wrapped at the host's width.
                let dir_entries = if block.remote_filesystem {
                    block.dir_entries.clone()
                } else {
                    crate::console::dir_listing::try_parse_listing(
                        &block.command, &lines, &block.cwd, theme,
                    ).map(|l| l.entries)
                };
                super::block_share::HostBlockSnapshot {
                    id: block.id,
                    created_at_unix_ms: block.created_at_unix_ms,
                    command: block.command.clone(),
                    cwd: block.cwd.to_string_lossy().into_owned(),
                    dir_entries,
                    running,
                    exit_code,
                    duration_ms: block.duration_ms,
                    failed,
                    lines,
                }
            })
            .collect()
    }

    pub fn toggle_block_expansion(&mut self, block_id: usize) {
        self.blocks.toggle_block_expansion(block_id);
    }

    pub fn is_command_running(&self) -> bool {
        // Without shell integration there is no `precmd` to signal a command has
        // finished, so we can never know when one ends. Reporting "running" in
        // that case would leave the optimistic block created on submit running
        // forever, locking the UI and removing the input bar. So treat shells
        // without integration (sh, cmd, or when hooks fail to load) like a plain
        // terminal: never "running", input always available.
        if !self.integration_active() {
            return false;
        }
        self.active_block_id
            .and_then(|id| self.blocks.get(id))
            .is_some_and(|block| !block.is_completed())
    }

    /// The command of the currently-running block, if any — set by the shell's
    /// `preexec` hook regardless of how the command was started (typed locally,
    /// or forwarded from a shared-session controller). Lets the view track the
    /// foreground command even when it didn't submit it itself.
    pub fn running_command(&self) -> Option<String> {
        if !self.integration_active() {
            return None;
        }
        self.active_block_id
            .and_then(|id| self.blocks.get(id))
            .filter(|block| !block.is_completed())
            .map(|block| block.command.clone())
            .filter(|cmd| !cmd.is_empty())
    }

    pub fn integration_active(&self) -> bool {
        self.hooks_active || self.semantic_active
    }

    pub fn emulator(&self) -> &Emulator {
        &self.session
    }

    pub fn active_emulator(&self) -> &Emulator {
        if let Some(id) = self.active_block_id {
            if let Some(block) = self.blocks.get(id) {
                if let Some(emu) = &block.emulator {
                    return emu.as_ref();
                }
            }
        }
        &self.session
    }

    pub fn alt_screen_active(&self) -> bool {
        if self.session.alt_screen_active() {
            return true;
        }
        if let Some(id) = self.active_block_id {
            if let Some(block) = self.blocks.get(id) {
                if let Some(emu) = &block.emulator {
                    return emu.alt_screen_active();
                }
            }
        }
        false
    }

    pub fn alt_screen_emulator(&self) -> &Emulator {
        if self.session.alt_screen_active() {
            return &self.session;
        }
        if let Some(id) = self.active_block_id {
            if let Some(block) = self.blocks.get(id) {
                if let Some(emu) = &block.emulator {
                    if emu.alt_screen_active() {
                        return emu.as_ref();
                    }
                }
            }
        }
        &self.session
    }

    pub fn alt_ui(&self) -> &AltScreenState {
        &self.alt_ui
    }

    /// Snapshot the alt-screen grid (styled rows + cursor) for shared-session
    /// viewers. Resolves ANSI colors against `theme` so the viewer paints exact
    /// cells. Returns the visible screen only (alt buffers have no scrollback).
    pub fn alt_screen_snapshot(
        &self,
        theme: &crate::theme::Theme,
    ) -> super::block_share::AltScreenSnapshot {
        let emu = self.alt_screen_emulator();
        let (cursor_row, cursor_col) = emu.cursor_position();
        let grid = emu.grid();
        let lines = grid
            .screen_styled_lines()
            .iter()
            .map(|l| crate::console::escape_parse::styled_to_colored_exact(l, theme))
            .collect();
        super::block_share::AltScreenSnapshot {
            rows: grid.rows,
            cols: grid.cols,
            cursor_row,
            cursor_col,
            cursor_visible: emu.cursor_visible(),
            lines,
        }
    }

    pub fn size(&self) -> SizeInfo {
        self.size
    }

    pub fn set_restored_shell_title(&mut self, title: String) {
        // Filter here too: sessions persisted before the filter existed may
        // carry a junk default title (e.g. "Administrator: …\powershell.exe").
        if !is_default_shell_title(&title) {
            self.shell_title = Some(title);
        }
    }

    pub fn restore_blocks(&mut self, blocks: Vec<crate::terminal::model::block::Block>) {
        self.blocks.restore(blocks);
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
    }

    pub fn restore_clear_gap(
        &mut self,
        height_px: f32,
        before_block_id: Option<usize>,
        after_all: bool,
    ) {
        if height_px > 0.0 {
            self.active_clear_gap = Some(ActiveClearGap {
                original_height_px: height_px,
                current_height_px: height_px,
                before_block_id,
                after_all,
                trigger_block_id: None,
            });
        }
    }

    pub fn active_clear_gap(&self) -> Option<&ActiveClearGap> {
        self.active_clear_gap.as_ref()
    }

    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    pub fn shell_title(&self) -> Option<&str> {
        self.shell_title.as_deref()
    }

    pub fn set_link_status(&mut self, status: ShellLinkStatus) {
        if self.link_status != status {
            self.link_status = status;
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
        }
    }
}
