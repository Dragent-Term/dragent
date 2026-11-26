use std::path::PathBuf;
use std::sync::Arc;
// `web_time::Instant` is `std::time::Instant` on native, and a wasm-safe
// implementation (performance.now) in the browser — plain `std::time::Instant`
// panics on wasm32.
use web_time::Instant;
use web_time::{SystemTime, UNIX_EPOCH};

use super::block_context::{block_context_from_lines, BlockContext};
use crate::console::escape_parse::ColoredLine;

/// Scrollback cap for a block's emulator, from settings (always bounded;
/// a legacy stored `0` loads as the default).
fn configured_scrollback_lines() -> usize {
    crate::console::editor_settings::EditorSettings::global()
        .terminal
        .clamped_scrollback_lines()
}

/// Lifecycle state of a single command block.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BlockStatus {
    /// Command submitted, shell hook not yet confirmed start.
    #[default]
    Pending,
    /// Shell `preexec` hook fired — command is executing.
    Running,
    /// `precmd` / `command_finished` hook fired — command has exited.
    Completed { exit_code: Option<i32> },
}

impl BlockStatus {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
    pub fn is_completed(self) -> bool {
        matches!(self, Self::Completed { .. })
    }
    pub fn exit_code(self) -> Option<i32> {
        if let Self::Completed { exit_code } = self {
            exit_code
        } else {
            None
        }
    }
}

/// A single command execution context — the granular unit of the block list.
///
/// While `status == Running` the block holds a live terminal emulator whose
/// grid is rendered directly.  On completion the grid is snapshotted into
/// `lines` and the emulator is released to free memory.
pub struct Block {
    pub id: usize,
    pub command: String,
    pub cwd: PathBuf,
    pub status: BlockStatus,
    pub started_at: Option<Instant>,
    pub created_at: Instant,
    /// Stable wall-clock creation time used to order shell blocks with agent
    /// blocks across shared-session reconnects and WASM replay.
    pub created_at_unix_ms: i64,
    pub duration_ms: Option<u64>,
    pub expanded: bool,
    pub bookmarked: bool,

    /// Snapshotted output lines — populated on completion.
    pub lines: Arc<Vec<ColoredLine>>,

    /// Live emulator — `Some` while running, `None` after completion.
    pub emulator: Option<Box<dragent_terminal::Emulator>>,

    /// Line count in the live emulator (O(1) proxy, avoids scanning the grid).
    pub live_line_count: usize,

    /// Immutable completion snapshot; `Some` after `set_completion`, `None` while running or pending.
    pub completion: Option<Arc<BlockContext>>,

    /// A directory listing resolved by the HOST, on a shared-session mirror.
    ///
    /// Lives on the block so EVERY render path sees it. The listing is otherwise
    /// derived independently in three places (the block list, the height
    /// estimator, and the block-output renderer), and a mirror cannot derive it
    /// at all — its filesystem belongs to a different machine, and re-parsing the
    /// `ls` text drops entries. Wiring one call site and missing the others just
    /// moves the bug, so the data belongs here.
    pub dir_entries: Option<Vec<crate::console::dir_listing::DirEntry>>,
    /// The cwd belongs to another machine. Never replace its captured output
    /// with a listing of a local stand-in (or a coincidentally matching path).
    pub remote_filesystem: bool,
}

impl Block {
    /// The directory listing to render for this block, if it is one.
    ///
    /// Prefers entries resolved by the HOST (a mirror cannot resolve them
    /// itself), falling back to deriving them locally. Every render path must go
    /// through here — deriving independently is what made a mirror show 4 of 7
    /// entries, because scraping the `ls` text loses names with spaces and lines
    /// wrapped at the host's width.
    pub fn listing(
        &self,
        theme: &crate::theme::Theme,
    ) -> Option<crate::console::dir_listing::DirListing> {
        if let Some(entries) = self.dir_entries.as_ref() {
            return Some(crate::console::dir_listing::DirListing {
                entries: entries.clone(),
            });
        }
        if self.remote_filesystem {
            return None;
        }
        crate::console::dir_listing::try_parse_listing(&self.command, &self.lines, &self.cwd, theme)
    }

    pub fn new(id: usize, command: String, cwd: PathBuf, rows: usize, cols: usize) -> Self {
        Self::new_with_timestamp(id, command, cwd, rows, cols, 0)
    }

    pub fn new_with_timestamp(
        id: usize,
        command: String,
        cwd: PathBuf,
        rows: usize,
        cols: usize,
        created_at_unix_ms: i64,
    ) -> Self {
        let now_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let created_at_unix_ms = if created_at_unix_ms > 0 {
            created_at_unix_ms
        } else {
            now_unix_ms
        };
        Self {
            id,
            command,
            cwd,
            status: BlockStatus::Pending,
            started_at: None,
            created_at: Instant::now(),
            created_at_unix_ms,
            duration_ms: None,
            expanded: true,
            bookmarked: false,
            lines: Arc::new(Vec::new()),
            emulator: Some(Box::new(dragent_terminal::Emulator::with_scrollback(
                rows,
                cols,
                configured_scrollback_lines(),
            ))),
            live_line_count: 0,
            completion: None,
            dir_entries: None,
            remote_filesystem: false,
        }
    }

    /// Drop output captured before the command ran (a starting shell's own
    /// prompt and integration echo), keeping the block and its command.
    pub fn discard_output(&mut self) {
        if let Some(emulator) = self.emulator.as_mut() {
            let (rows, cols) = emulator.dimensions();
            **emulator = dragent_terminal::Emulator::with_scrollback(
                rows,
                cols,
                configured_scrollback_lines(),
            );
        }
        self.live_line_count = 0;
        self.lines = Arc::new(Vec::new());
    }

    /// Shell `preexec` hook confirmed the command started executing.
    pub fn on_preexec(&mut self, command: Option<String>) {
        if let Some(cmd) = command {
            // Only adopt the shell-reported command when we don't already have one.
            // A block created from an input-bar submit already holds the exact text
            // the user typed (e.g. `ls`); the shell's `preexec` value can be
            // alias-expanded (bash reports `ls -F --color=auto --show-control-chars`),
            // so overwriting would replace the clean command with the expansion.
            if !cmd.is_empty() && self.command.is_empty() {
                self.command = cmd;
            }
        }
        self.started_at = Some(Instant::now());
        self.status = BlockStatus::Running;
    }

    /// Shell `precmd` hook — command has finished.  Snapshot emulator output
    /// into `lines` and release the emulator.
    pub fn on_completed(&mut self, exit_code: Option<i32>, theme: &crate::theme::Theme) {
        self.set_completed_meta(exit_code);
        self.snapshot_and_release(theme);
    }

    /// Record completion metadata (exit code, duration) without snapshotting.
    ///
    /// Windows ConPTY delivers the `precmd` OSC interleaved into the middle of
    /// the command's rendered output (ConPTY passes OSCs through ahead of the
    /// text it is still repainting), so the OSC's stream position is NOT a valid
    /// output boundary there. The session core marks the block completed here —
    /// the input bar returns immediately — but keeps feeding this block's
    /// emulator until a rendered fence shows the output is fully flushed, then
    /// calls [`Self::snapshot_and_release`].
    pub fn set_completed_meta(&mut self, exit_code: Option<i32>) {
        if let Some(duration) = self.started_at.map(|t| t.elapsed().as_millis() as u64) {
            self.duration_ms = Some(duration);
        }
        self.status = BlockStatus::Completed { exit_code };
    }

    /// An AI agent chooses all its own colours, so nothing is inferred from the
    /// words in its output the way it is for a plain command's.
    pub fn uses_exact_colors(&self) -> bool {
        super::input_mode::AiTool::from_command(&self.command).is_some()
    }

    /// Snapshot the live emulator grid into `lines` and free the emulator.
    pub fn snapshot_and_release(&mut self, theme: &crate::theme::Theme) {
        // Snapshot output from the live emulator then free it.
        if let Some(emu) = self.emulator.take() {
            let mut lines = if self.uses_exact_colors() {
                crate::console::escape_parse::snapshot_emulator_lines_exact(&emu, theme)
            } else {
                crate::console::escape_parse::snapshot_emulator_lines(&emu, theme)
            };
            // Unix: line 0 is the PTY echo of the submitted command; drop it
            // since the header already shows it.
            #[cfg(not(windows))]
            if !lines.is_empty() {
                lines.remove(0);
            }
            // Windows: the ConPTY hook clears the console at `preexec`, which
            // erases the echo (so line 0 is real output — keep it) but can
            // leave the output positioned at its pre-clear row after a repaint,
            // with blank rows above. Trim those leading blanks instead.
            #[cfg(windows)]
            {
                let leading_blanks = lines
                    .iter()
                    .take_while(|l| l.plain_text().trim().is_empty())
                    .count();
                lines.drain(0..leading_blanks);
            }
            // Keep a generous tail so long Codex/build responses remain usable,
            // while bounding one runaway command's retained memory.
            let max = super::COMPLETED_BLOCK_MAX_LINES;
            if lines.len() > max {
                lines.drain(0..lines.len() - max);
            }
            self.lines = Arc::new(lines);
        }
        self.live_line_count = 0;
    }

    /// Update `live_line_count` from the current emulator state.
    /// Called after each `append_bytes` while the block is running.
    pub fn sync_live_line_count(&mut self) {
        if let Some(emu) = &self.emulator {
            self.live_line_count = live_primary_line_count(emu);
        }
    }

    /// Resize the live emulator.  No-op if already completed.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        if let Some(emu) = &mut self.emulator {
            emu.resize(rows, cols);
            self.live_line_count = live_primary_line_count(emu);
        }
    }

    pub fn is_running(&self) -> bool {
        self.status.is_running()
    }
    pub fn is_completed(&self) -> bool {
        self.status.is_completed()
    }

    pub fn completion(&self) -> Option<&BlockContext> {
        self.completion.as_deref()
    }

    pub fn set_completion(&mut self, context: BlockContext) {
        self.completion = Some(Arc::new(context));
    }

    /// Recompute `completion` from already-snapshotted `lines` (e.g. after SQLite restore).
    pub fn recompute_completion(&mut self) {
        if !self.status.is_completed() {
            return;
        }
        let ctx = block_context_from_lines(
            self.id,
            self.command.clone(),
            self.cwd.clone(),
            self.status.exit_code(),
            self.duration_ms,
            &self.lines,
        );
        self.completion = Some(Arc::new(ctx));
    }
}

pub fn live_primary_line_count(emu: &dragent_terminal::Emulator) -> usize {
    let grid = emu.grid();
    // A cursor is not an end-of-content marker. Normal-buffer TUIs can paint
    // below it; Codex, for example, places its footer there and grows its `/`
    // command popup downward while moving the composer/cursor upward. Using
    // only cursor_row clipped exactly that lower content and made the popup
    // appear pinned. Include the last visually occupied screen row, while still
    // excluding unused blank rows at the bottom for ordinary shell commands.
    grid.scrollback_len() + (grid.cursor_row + 1).max(grid.screen_content_end())
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Block")
            .field("id", &self.id)
            .field("command", &self.command)
            .field("status", &self.status)
            .field("lines", &self.lines.len())
            .field("live_line_count", &self.live_line_count)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::live_primary_line_count;

    #[test]
    fn remote_listing_never_reads_a_local_directory() {
        let theme = crate::theme::Theme::dark();
        let mut block = super::Block::new(
            0, "ls".into(), std::env::temp_dir(), 24, 80,
        );
        assert!(block.listing(&theme).is_some(), "local directory is readable");
        block.remote_filesystem = true;
        assert!(block.listing(&theme).is_none(), "render the remote output instead");
        // A host-supplied empty listing is authoritative, too.
        block.dir_entries = Some(Vec::new());
        assert!(block.listing(&theme).unwrap().entries.is_empty());
    }

    #[test]
    fn live_primary_line_count_includes_scrollback() {
        let mut emu = dragent_terminal::Emulator::new(3, 20);
        emu.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");

        assert!(emu.grid().scrollback_len() > 0);
        assert!(
            live_primary_line_count(&emu) > 3,
            "running blocks must render scrollback, not only visible rows"
        );
    }

    #[test]
    fn live_primary_line_count_includes_content_below_cursor() {
        let mut emu = dragent_terminal::Emulator::new(8, 30);
        // Model a TUI composer on row 2, a popup below it on row 6, then put
        // the cursor back in the composer as Codex does.
        emu.feed(b"\x1b[2J\x1b[2;1Hcomposer\x1b[6;1H/command\x1b[2;4H");

        assert_eq!(emu.grid().cursor_row, 1);
        assert_eq!(live_primary_line_count(&emu), 6);
    }

    #[test]
    fn block_resize_resyncs_live_line_count() {
        let mut block = super::Block::new(0, "seq".into(), std::path::PathBuf::from("/tmp"), 3, 20);
        let emu = block.emulator.as_mut().expect("running block has emulator");
        emu.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        block.sync_live_line_count();
        assert!(block.live_line_count > 3);

        block.resize(8, 20);
        let emu = block.emulator.as_ref().expect("running block has emulator");
        assert_eq!(block.live_line_count, live_primary_line_count(emu));
    }
}
