use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::FairMutex;
use warpui::{Entity, ModelContext};

use super::block::Block;
use super::events::TerminalEvent;
use super::size::{SizeInfo, SizeUpdate};
use super::terminal_session_core::{SessionEffect, ShellLinkStatus, TerminalSessionCore};

/// Session model for one interactive terminal pane (warpui entity wrapper).
pub struct TerminalModel {
    core: Arc<FairMutex<TerminalSessionCore>>,
}

impl Entity for TerminalModel {
    type Event = TerminalEvent;
}

impl TerminalModel {
    pub fn from_core(core: Arc<FairMutex<TerminalSessionCore>>) -> Self {
        Self { core }
    }

    pub fn drain_pending_effects(&mut self, ctx: &mut ModelContext<Self>) {
        let effects = self.core.lock().take_pending_effects();
        self.apply_effects(effects, ctx);
    }

    fn apply_effects(&mut self, effects: Vec<SessionEffect>, ctx: &mut ModelContext<Self>) {
        // Coalesce all OutputAppended into one event per drain cycle so the view
        // callback fires once regardless of how many PTY chunks were processed.
        let mut saw_output = false;
        for effect in effects {
            match effect {
                SessionEffect::Ui(TerminalEvent::OutputAppended) => saw_output = true,
                SessionEffect::Ui(event) => ctx.emit(event),
                SessionEffect::PtyWrite(bytes) => ctx.emit(TerminalEvent::PtyWrite(bytes)),
            }
        }
        if saw_output {
            ctx.emit(TerminalEvent::OutputAppended);
        }
    }

    fn drain_core_effects(&mut self, ctx: &mut ModelContext<Self>) {
        let effects = self.core.lock().take_pending_effects();
        self.apply_effects(effects, ctx);
    }

    pub fn toggle_block_expansion(&mut self, block_id: usize) {
        self.core.lock().toggle_block_expansion(block_id);
    }

    pub fn is_command_running(&self) -> bool {
        self.core.lock().is_command_running()
    }

    pub fn alt_screen_active(&self) -> bool {
        self.core.lock().alt_screen_active()
    }

    pub fn size(&self) -> SizeInfo {
        self.core.lock().size()
    }

    pub fn set_restored_shell_title(&mut self, title: String) {
        self.core.lock().set_restored_shell_title(title);
    }

    pub fn restore_blocks(&mut self, blocks: Vec<Block>, ctx: &mut ModelContext<Self>) {
        self.core.lock().restore_blocks(blocks);
        self.drain_core_effects(ctx);
    }

    pub fn restore_clear_gap(
        &mut self,
        height_px: f32,
        before_block_id: Option<usize>,
        after_all: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.core
            .lock()
            .restore_clear_gap(height_px, before_block_id, after_all);
        self.drain_core_effects(ctx);
    }

    pub fn begin_command(&mut self, command: String, ctx: &mut ModelContext<Self>) {
        self.core.lock().begin_command(command);
        self.drain_core_effects(ctx);
    }

    /// Drain-timeout fallback (Windows ConPTY): snapshot a completed block whose
    /// output capture was deferred past `precmd`. No-op if the rendered fence
    /// already finalised it.
    pub fn finalise_draining_block(
        &mut self,
        theme: &crate::theme::Theme,
        ctx: &mut ModelContext<Self>,
    ) {
        self.core.lock().finalise_draining_block(theme);
        self.drain_core_effects(ctx);
    }

    pub fn cwd(&self) -> PathBuf {
        self.core.lock().cwd().clone()
    }

    /// Snapshot the block list for shared-session broadcast (see
    /// [`block_share`](super::block_share)).
    pub fn block_share_snapshot(
        &self,
        theme: &crate::theme::Theme,
        skip: &std::collections::HashSet<usize>,
    ) -> Vec<super::block_share::HostBlockSnapshot> {
        self.core.lock().block_share_snapshot(theme, skip)
    }

    /// Styled alt-screen grid snapshot for shared-session viewers.
    pub fn alt_screen_snapshot(
        &self,
        theme: &crate::theme::Theme,
    ) -> super::block_share::AltScreenSnapshot {
        self.core.lock().alt_screen_snapshot(theme)
    }

    pub fn shell_title(&self) -> Option<String> {
        self.core.lock().shell_title().map(str::to_owned)
    }

    pub fn set_link_status(&mut self, status: ShellLinkStatus, ctx: &mut ModelContext<Self>) {
        self.core.lock().set_link_status(status);
        self.drain_core_effects(ctx);
    }

    pub fn apply_clear_viewport(&mut self, viewport_height_px: f32, ctx: &mut ModelContext<Self>) {
        self.core.lock().apply_clear_viewport(viewport_height_px);
        self.drain_core_effects(ctx);
    }

    /// Reduce viewport clear gap height as post-clear blocks grow.
    pub fn shrink_active_clear_gap(
        &mut self,
        block_heights: &std::collections::HashMap<usize, f32>,
        agent_block_heights: &std::collections::HashMap<usize, f32>,
        shell_blocks_created: &std::collections::HashMap<usize, i64>,
        agent_blocks_created: &std::collections::HashMap<usize, i64>,
        live_prompt_height: f32,
        ctx: &mut ModelContext<Self>,
    ) {
        self.core.lock().shrink_active_clear_gap(
            block_heights,
            agent_block_heights,
            shell_blocks_created,
            agent_blocks_created,
            live_prompt_height,
        );
        self.drain_core_effects(ctx);
    }

    pub fn end_running_command(
        &mut self,
        theme: &crate::theme::Theme,
        ctx: &mut ModelContext<Self>,
    ) {
        self.core.lock().end_running_command(theme);
        self.drain_core_effects(ctx);
    }

    pub fn apply_size_update(
        &mut self,
        update: SizeUpdate,
        send_sigwinch: impl FnOnce(u16, u16),
        ctx: &mut ModelContext<Self>,
    ) {
        self.core.lock().apply_size_update(update, send_sigwinch);
        self.drain_core_effects(ctx);
    }

    pub fn app_cursor_keys(&self) -> bool {
        self.core.lock().app_cursor_keys()
    }

    pub fn terminal_mouse_active(&self) -> bool {
        self.core.lock().terminal_mouse_active()
    }

    pub fn mouse_reporting_mode(&self) -> dragent_terminal::MouseReportingMode {
        self.core.lock().mouse_reporting_mode()
    }

    pub fn encode_terminal_mouse(&self, event: dragent_terminal::MouseEvent) -> Option<Vec<u8>> {
        self.core.lock().encode_terminal_mouse(event)
    }

    pub fn start_selection(&mut self, row: usize, col: usize, ctx: &mut ModelContext<Self>) {
        self.core.lock().start_selection(row, col);
        self.drain_core_effects(ctx);
    }

    pub fn extend_selection(&mut self, row: usize, col: usize, ctx: &mut ModelContext<Self>) {
        self.core.lock().extend_selection(row, col);
        self.drain_core_effects(ctx);
    }

    pub fn clear_alt_selection(&mut self, ctx: &mut ModelContext<Self>) {
        self.core.lock().clear_alt_selection();
        self.drain_core_effects(ctx);
    }

    pub fn selected_text(&self) -> Option<String> {
        self.core.lock().selected_text()
    }

    pub fn focus_in_bytes(&self) -> Option<Vec<u8>> {
        self.core.lock().focus_in_bytes()
    }

    pub fn focus_out_bytes(&self) -> Option<Vec<u8>> {
        self.core.lock().focus_out_bytes()
    }

    /// Read-only access for render paths that need the live session core.
    pub fn acknowledge_host_restore(&self) {
        self.core.lock().acknowledge_host_restore();
    }

    pub fn host_shell_starting(&self) -> Option<String> {
        self.core.lock().host_shell_starting().map(str::to_owned)
    }

    pub fn with_core<R>(&self, f: impl FnOnce(&TerminalSessionCore) -> R) -> R {
        f(&self.core.lock())
    }

    /// Feed raw program output, as the PTY would (tests only).
    #[cfg(test)]
    pub(crate) fn feed_for_test(&self, bytes: &[u8]) {
        self.core
            .lock()
            .feed_pty_bytes(bytes, &crate::theme::Theme::default());
    }

    /// Shared-session MIRROR: apply one of the host's authoritative block
    /// snapshots. See [`TerminalSessionCore::apply_remote_block`].
    #[allow(clippy::too_many_arguments)]
    pub fn apply_remote_block(
        &mut self,
        id: usize,
        command: String,
        cwd: Option<std::path::PathBuf>,
        created_at_unix_ms: i64,
        lines: Option<Vec<crate::console::escape_parse::ColoredLine>>,
        dir_entries: Option<Vec<crate::console::dir_listing::DirEntry>>,
        running: bool,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.core.lock().apply_remote_block(
            id,
            command,
            cwd,
            created_at_unix_ms,
            lines,
            dir_entries,
            running,
            exit_code,
            duration_ms,
        );
        self.drain_core_effects(ctx);
    }

    /// Mirror: whether the host's block with this id is currently running, so a
    /// lines-only delta can preserve the status it already has.
    pub fn remote_block_running(&self, id: usize) -> bool {
        self.core
            .lock()
            .blocks()
            .get(id)
            .map(|b| b.status.is_running())
            .unwrap_or(true)
    }
}

/// Return the cached `BlockContext` for a completed block, if available.
pub fn block_context(
    model: &TerminalModel,
    block_id: usize,
) -> Option<std::sync::Arc<super::block_context::BlockContext>> {
    let core = model.core.lock();
    core.blocks().get(block_id)?.completion.clone()
}

/// Snapshot completed block output for sidebar harvesting.
pub fn completed_block_snapshot(
    model: &TerminalModel,
    block_id: usize,
) -> Option<crate::workspace::project_sidebar::BlockSnapshot> {
    use crate::workspace::project_sidebar::BlockSnapshot;

    let core = model.core.lock();
    let block = core.blocks().get(block_id)?;
    if !block.status.is_completed() {
        return None;
    }
    let tail_lines = block
        .lines
        .iter()
        .rev()
        .take(40)
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>()
        })
        .filter(|l| !l.trim().is_empty())
        .collect();
    Some(BlockSnapshot { tail_lines })
}
