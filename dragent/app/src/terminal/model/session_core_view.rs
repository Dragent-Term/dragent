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
use crate::terminal::model::session_core_types::ActiveClearGap;
use crate::terminal::model::session_core_types::SessionEffect;
use std::sync::Arc;

impl super::session_core_types::TerminalSessionCore {
    /// Viewport clear: preserve command history, blank live surface, and insert clear gap.
    pub fn apply_clear_viewport(&mut self, viewport_height_px: f32) {
        self.active_block_id = None;
        self.submitted_unseen = false;
        self.clear_submitted = false;

        let blocks = self.blocks.blocks();
        let (before_block_id, trigger_block_id) = if blocks.is_empty() {
            (None, None)
        } else {
            let last_idx = blocks.len() - 1;
            if Self::is_clear_command(&blocks[last_idx].command) {
                // The `clear`/`cls` command itself is the active block — the gap
                // stands in for it, so it must be excluded from rendering as its
                // own (empty) block entry, not just have the gap placed near it.
                let before = if blocks.len() >= 2 {
                    Some(blocks[blocks.len() - 2].id)
                } else {
                    None
                };
                (before, Some(blocks[last_idx].id))
            } else {
                (Some(blocks[last_idx].id), None)
            }
        };
        let after_all = false;

        let gap_height = viewport_height_px.max(self.size.cell_height);
        self.active_clear_gap = Some(ActiveClearGap {
            original_height_px: gap_height,
            current_height_px: gap_height,
            before_block_id,
            after_all,
            trigger_block_id,
        });

        self.clear_session_surface_preserving_bootstrap();
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
    }

    pub fn shrink_active_clear_gap(
        &mut self,
        block_heights: &std::collections::HashMap<usize, f32>,
        agent_block_heights: &std::collections::HashMap<usize, f32>,
        shell_blocks_created: &std::collections::HashMap<usize, i64>,
        agent_blocks_created: &std::collections::HashMap<usize, i64>,
        live_prompt_height: f32,
    ) {
        let Some(ref mut gap) = self.active_clear_gap else {
            return;
        };

        let mut height_added = 0.0;
        let mut has_blocks_after = false;

        // A clear command's own creation order is the boundary. Using only the
        // previous shell block makes agent blocks created between that command
        // and the previous shell command look like post-clear content, so they
        // remain below the gap until a second clear.
        let anchor_time = gap
            .trigger_block_id
            .or(gap.before_block_id)
            .and_then(|anchor_id| shell_blocks_created.get(&anchor_id).copied());

        for (&block_id, &h) in block_heights {
            let is_after = match anchor_time {
                Some(t) => {
                    if let Some(&created) = shell_blocks_created.get(&block_id) {
                        created > t
                    } else {
                        block_id > gap.before_block_id.unwrap()
                    }
                }
                None => true,
            };
            if is_after {
                height_added += h;
                has_blocks_after = true;
            }
        }

        for (&agent_id, &h) in agent_block_heights {
            let is_after = match anchor_time {
                Some(t) => {
                    if let Some(&created) = agent_blocks_created.get(&agent_id) {
                        created > t
                    } else {
                        true
                    }
                }
                None => true,
            };
            if is_after {
                height_added += h;
                has_blocks_after = true;
            }
        }

        if has_blocks_after {
            height_added += 24.0;
        }
        height_added += live_prompt_height;

        let new_height = (gap.original_height_px - height_added).clamp(0.0, gap.original_height_px);
        if (gap.current_height_px - new_height).abs() > 0.01 {
            gap.current_height_px = new_height;
            if gap.current_height_px <= 0.5 {
                self.active_clear_gap = None;
                self.pending_effects
                    .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
            }
        }
    }

    pub fn is_clear_command(command: &str) -> bool {
        if ["&&", "||", "|", ";"]
            .iter()
            .any(|sep| command.contains(sep))
        {
            return false;
        }
        let parts: Vec<&str> = command.split_whitespace().collect();
        matches!(parts.first().copied(), Some("clear") | Some("cls"))
    }

    pub(crate) fn should_clear_viewport(&mut self) -> bool {
        if self.session.alt_screen_active() {
            return false;
        }
        // A clear the user explicitly submitted always takes effect, whatever
        // the block bookkeeping looks like.
        if self.clear_submitted {
            self.clear_submitted = false;
            return true;
        }
        match self.active_block_id {
            None => true,
            Some(active_id) => self
                .blocks
                .get(active_id)
                .is_some_and(|block| Self::is_clear_command(&block.command)),
        }
    }

    pub(crate) fn refresh_bootstrap_lines(&mut self, theme: &Theme) {
        self.blocks.bootstrap_lines =
            crate::console::escape_parse::snapshot_emulator_lines(&self.session, theme);
    }

    pub(crate) fn sync_alt_screen_edges(&mut self) {
        let session_alt = self.session.alt_screen_active();
        let block_alt = self
            .active_block_id
            .and_then(|id| self.blocks.get(id))
            .and_then(|b| b.emulator.as_ref())
            .is_some_and(|e| e.alt_screen_active());
        let active = session_alt || block_alt;

        if active && !self.alt_screen_was_active {
            self.alt_screen_was_active = true;
            self.input_mode = self.input_mode.clone().on_alt_screen_entered();
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::AltEntered));
        } else if !active && self.alt_screen_was_active {
            self.alt_screen_was_active = false;
            self.alt_ui.clear_selection();
            self.input_mode = self.input_mode.clone().on_alt_screen_exited();
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::AltExited));
        }
    }

    /// Apply a measured viewport size and optionally resize the emulator grid.
    pub fn apply_size_update(&mut self, update: SizeUpdate, send_sigwinch: impl FnOnce(u16, u16)) {
        use super::session_core_types::HostResizeGate;
        if self.host_resize_gate == HostResizeGate::AwaitingView {
            return;
        }
        // An alt-screen transition must push through even when pixel size is
        // unchanged, so the TUI is told to redraw at the (possibly identical)
        // geometry on enter/exit. Every other reason is a no-op if nothing moved.
        let dims_unchanged =
            update.info.rows == self.size.rows && update.info.cols == self.size.cols;
        if dims_unchanged && update.reason != SizeReason::AltScreenTransition {
            // Keep cached pixel metrics current for hit-testing without
            // re-resizing the grid or signalling the shell.
            self.size = update.info;
            return;
        }
        let started = std::time::Instant::now();
        self.size = update.info;
        self.session
            .resize(update.info.rows as usize, update.info.cols as usize);

        if let Some(id) = self.active_block_id {
            if let Some(block) = self.blocks.get_mut(id) {
                block.resize(update.info.rows as usize, update.info.cols as usize);
            }
        }

        // Interactive normal-buffer TUIs (notably Codex) derive the height of
        // their composer and completion popups from the PTY dimensions. The
        // emulator and the child process must therefore resize together; only
        // resizing our local grid leaves the process drawing bottom rows that
        // are clipped behind Dragent's pinned AI input panel.
        if self.host_resize_gate == HostResizeGate::Live {
            send_sigwinch(update.info.rows, update.info.cols);
        }
        crate::terminal::performance::RESIZE_APPLY.record(started.elapsed());
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::SizeChanged));
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::OutputAppended));
    }

    pub fn app_cursor_keys(&self) -> bool {
        self.active_emulator().mode().contains(TermMode::APP_CURSOR)
    }

    // Once shell hooks report completion, stale TUI pointer modes must not
    // turn a scroll gesture into text for the shell to execute.
    fn accepts_terminal_pointer_input(&self) -> bool {
        !self.integration_active() || self.is_command_running()
    }

    pub fn terminal_mouse_active(&self) -> bool {
        self.accepts_terminal_pointer_input()
            && (self.alt_screen_active() || self.mouse_reporting_mode() != MouseReportingMode::None)
    }

    pub fn mouse_reporting_mode(&self) -> MouseReportingMode {
        if !self.accepts_terminal_pointer_input() {
            return MouseReportingMode::None;
        }
        self.active_emulator().mouse_state().reporting
    }

    pub fn focus_tracking_enabled(&self) -> bool {
        self.accepts_terminal_pointer_input() && self.active_emulator().mouse_state().focus_tracking
    }

    /// Whether the running app asked to hear about dark/light changes. The
    /// shell gate stops a crashed app's request sending reports to the prompt.
    pub fn color_scheme_updates_enabled(&self) -> bool {
        self.accepts_terminal_pointer_input()
            && self
                .active_emulator()
                .mode()
                .contains(TermMode::COLOR_SCHEME_UPDATES)
    }

    /// The theme colour queries are answered from. The output feed keeps the
    /// theme its tab opened with, and an app asks again after a theme change.
    pub fn set_reply_theme(&mut self, theme: Theme) {
        self.reply_theme = Some(theme);
    }

    /// The colours to draw the running program with instead of `live`, when it
    /// was told the opposite scheme and hasn't asked since. Some programs only
    /// ask at startup and cannot follow a theme change: drawing their default
    /// text in the new scheme, over the backgrounds they chose for the old one,
    /// makes it unreadable. The pane follows `live` once they ask again.
    pub fn stale_app_colors(
        &self,
        live: &Theme,
    ) -> Option<dragent_host::screen_protocol::TerminalColors> {
        self.app_colors.filter(|told| told.is_dark != live.is_dark)
    }

    /// The theme just flipped from `previous`. An agent already running may have
    /// asked for its colours before this app started watching, in a session or
    /// even a daemon from earlier, so nothing recorded what it was told. It chose
    /// its colours under `previous`: the theme it has been showing.
    pub fn note_scheme_flipped_from(&mut self, previous: &Theme) {
        let agent = self
            .running_command()
            .as_deref()
            .and_then(super::input_mode::AiTool::from_command)
            .is_some();
        if agent && self.app_colors.is_none() {
            let rgb = |c: warpui::color::ColorU| (c.r, c.g, c.b);
            self.app_colors = Some(dragent_host::screen_protocol::TerminalColors {
                foreground: rgb(previous.foreground),
                background: rgb(previous.background),
                is_dark: previous.is_dark,
            });
        }
    }

    /// Whether the running program can only pick up the `live` theme by being
    /// started again: it was told the other scheme and never asked to hear about
    /// changes. One that did ask follows on its own.
    pub fn needs_restart_for_theme(&self, live: &Theme) -> bool {
        self.stale_app_colors(live).is_some() && !self.color_scheme_updates_enabled()
    }

    pub fn encode_terminal_mouse(&self, event: MouseEvent) -> Option<Vec<u8>> {
        if !self.accepts_terminal_pointer_input() {
            return None;
        }
        self.active_emulator().mouse_state().encode(event)
    }

    pub fn start_selection(&mut self, row: usize, col: usize) {
        self.alt_ui.selection = Some(CellSelection::new(row, col));
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::OutputAppended));
    }

    pub fn clear_alt_selection(&mut self) {
        if self.alt_ui.selection.take().is_some() {
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::OutputAppended));
        }
    }

    pub fn extend_selection(&mut self, row: usize, col: usize) {
        if let Some(sel) = self.alt_ui.selection.as_mut() {
            sel.set_cursor(row, col);
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::OutputAppended));
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.alt_ui.selection.as_ref()?;
        let lines = self.alt_screen_emulator().alt_screen_all_lines();
        let text = selection_text(&lines, sel);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn focus_in_bytes(&self) -> Option<Vec<u8>> {
        if self.focus_tracking_enabled() {
            Some(b"\x1b[I".to_vec())
        } else {
            None
        }
    }

    pub fn focus_out_bytes(&self) -> Option<Vec<u8>> {
        if self.focus_tracking_enabled() {
            Some(b"\x1b[O".to_vec())
        } else {
            None
        }
    }

    pub fn take_pending_effects(&mut self) -> Vec<SessionEffect> {
        std::mem::take(&mut self.pending_effects)
    }
}
