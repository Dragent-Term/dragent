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
use crate::terminal::model::session_core_cwd::cwd_changed;
use crate::terminal::model::session_core_cwd::is_default_shell_title;
use crate::terminal::model::session_core_cwd::normalize_shell_cwd;
use crate::terminal::model::session_core_cwd::resolve_cd_target;
use crate::terminal::model::session_core_types::current_rss_mb;
use crate::terminal::model::session_core_types::peak_rss_mb;
use crate::terminal::model::session_core_types::SessionEffect;
use std::sync::Arc;

/// The daemon answered this query when it parsed the frame, in order with the
/// app's other queries. Replay must never answer it again.
fn answered_by_daemon(event: &TermEvent) -> bool {
    matches!(
        event,
        TermEvent::Reply(_) | TermEvent::ColorQuery { .. } | TermEvent::ColorSchemeQuery
    )
}

impl super::session_core_types::TerminalSessionCore {
    /// A folder the shell reported, in this Mac's path space.
    fn local_cwd(&self, reported: PathBuf) -> PathBuf {
        match &self.remote_machine {
            Some(machine) => crate::remote_fs::alias_remote_path(machine, &reported),
            None => reported,
        }
    }

    pub(crate) fn restore_host_resume(
        &mut self,
        resume: &dragent_host::screen_protocol::TerminalResume,
        theme: &Theme,
    ) -> Result<(), String> {
        let emulator = Self::decode_host_resume(resume, self.size.rows, self.size.cols)?;
        self.apply_host_resume(resume, emulator, theme)
    }

    /// Decode a resume's display checkpoint. Needs no session state, so
    /// callers run it without holding the session lock.
    pub(crate) fn decode_host_resume(
        resume: &dragent_host::screen_protocol::TerminalResume,
        rows: u16,
        cols: u16,
    ) -> Result<Emulator, String> {
        let mut emulator = Emulator::with_scrollback(
            rows as usize,
            cols as usize,
            crate::console::editor_settings::EditorSettings::global()
                .terminal
                .clamped_scrollback_lines(),
        );
        emulator.restore_display_checkpoint(&resume.display)?;
        Ok(emulator)
    }

    /// Adopt a resume whose checkpoint [`Self::decode_host_resume`] decoded.
    pub(crate) fn apply_host_resume(
        &mut self,
        resume: &dragent_host::screen_protocol::TerminalResume,
        emulator: Emulator,
        theme: &Theme,
    ) -> Result<(), String> {
        // An older, unintegrated shell cannot distinguish an idle prompt from
        // a running interactive program. Treating its missing command as idle
        // would complete the block and route keystrokes into our input editor.
        // A shell that is still starting has integration installed; its first
        // prompt is on the way, so it is not mistaken for an unintegrated one.
        if !resume.shell_ready && resume.command.is_none() && resume.starting.is_none() {
            return Err("Shell integration unavailable. Update the machine runtime and open a new terminal.".into());
        }
        // Decoded first (see `decode_host_resume`): a failed checkpoint
        // never reaches here, so it cannot alter routing/state.
        // The daemon's grid is the PTY's real size. Adopting it means the next
        // layout resizes the process only if the pane truly differs from it.
        let (rows, cols) = emulator.dimensions();
        self.size.rows = rows as u16;
        self.size.cols = cols as u16;
        self.host_resize_gate = super::session_core_types::HostResizeGate::AwaitingView;
        self.suppress_startup_output = false;
        self.bootstrapped = true;
        if !resume.cwd.is_empty() { self.cwd = self.local_cwd(PathBuf::from(&resume.cwd)); }
        if let Some(command) = &resume.command {
            self.submitted_unseen = false;
            let command = dragent_terminal::shell_integration::agent_display_command(command);
            self.on_preexec(command.to_owned());
            self.input_mode = InputMode::from_submitted_command(command);
            if let Some(block) = self.active_block_id.and_then(|id| self.blocks.get_mut(id)) {
                block.emulator = Some(Box::new(emulator));
                block.sync_live_line_count();
            }
            // The shell's previous display cannot retain an obsolete alt flag.
            self.session = Emulator::with_scrollback(
                self.size.rows as usize,
                self.size.cols as usize,
                crate::console::editor_settings::EditorSettings::global()
                    .terminal
                    .clamped_scrollback_lines(),
            );
        } else {
            // A command the app just submitted that the shell hasn't run yet
            // is still queued behind this checkpoint, so its block stays open
            // for its own hooks. Any other running block ended unseen.
            if self.active_block_id.is_some() && !self.submitted_unseen {
                self.on_precmd(None, None, theme);
            }
            self.session = emulator;
            if self.active_block_id.is_none() {
                self.input_mode = InputMode::Idle;
            }
        }
        self.hooks_active =
            resume.shell_ready || resume.command.is_some() || resume.starting.is_some();
        self.host_shell_starting = resume.starting.clone();
        self.app_colors = resume.answered_colors;
        self.sync_alt_screen_edges();
        self.alt_ui.bump_paint();
        self.pending_effects.push(SessionEffect::Ui(TerminalEvent::StatusChanged));
        self.pending_effects.push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
        self.pending_effects.push(SessionEffect::Ui(TerminalEvent::OutputAppended));
        self.pending_effects.push(SessionEffect::Ui(TerminalEvent::HostStateRestored));
        Ok(())
    }

    /// Process PTY output bytes. Emits events; does not touch the view layer.
    pub fn feed_pty_bytes(&mut self, bytes: &[u8], theme: &Theme) -> PtyFeedStats {
        self.feed_output(bytes, None, theme)
    }

    /// Host output is already parsed. Keep the display interpreter and effects
    /// local during migration, without passing source bytes through a parser.
    pub fn feed_operations(
        &mut self,
        bytes: &[u8],
        operations: &[dragent_terminal::operations::Operation],
        theme: &Theme,
    ) -> PtyFeedStats {
        self.feed_output(bytes, Some(operations), theme)
    }

    fn feed_output(
        &mut self,
        bytes: &[u8],
        operations: Option<&[dragent_terminal::operations::Operation]>,
        theme: &Theme,
    ) -> PtyFeedStats {
        if bytes.is_empty() {
            return PtyFeedStats { bytes_processed: 0 };
        }
        let now = web_time::Instant::now();
        if self
            .output_at
            .is_none_or(|at| now.duration_since(at) > super::session_core_types::OUTPUT_RUN_GAP)
        {
            self.output_run_since = Some(now);
        }
        self.output_at = Some(now);

        let log_pty = std::env::var_os("DRAGENT_LOG_PTY").is_some();
        let t0 = if log_pty {
            Some(web_time::Instant::now())
        } else {
            None
        };

        let fed_block_id = self.active_block_id;
        if let Some(operations) = operations {
            self.apply_and_process(operations, theme);
        } else {
            self.feed_and_process(bytes, theme);
        }

        // ── Feed diagnostics ───────────────────────────────────────────────────
        // Runs on the PTY reader thread, UNDER the shared session lock, so the
        // measured duration IS the lock-hold time per feed. Periodic totals show
        // whether the drain is CPU-bound (high avg feed µs / low MB/s) and whether
        // memory is growing (blocks / total buffered lines climbing → OOM source).
        if let Some(t0) = t0 {
            use std::cell::RefCell;
            use std::time::Duration;
            use web_time::Instant;
            thread_local! {
                static PTY_STATS: RefCell<(u64, u64, Duration, Option<Instant>)> =
                    const { RefCell::new((0, 0, Duration::ZERO, None)) };
            }
            let dur = t0.elapsed();
            let blocks = self.blocks.blocks().len();
            let buffered_lines: usize = self.blocks.blocks().iter().map(|b| b.lines.len()).sum();
            PTY_STATS.with(|s| {
                let mut s = s.borrow_mut();
                s.0 += 1;
                s.1 += bytes.len() as u64;
                s.2 += dur;
                let now = Instant::now();
                let last = *s.3.get_or_insert(now);
                let elapsed = now.duration_since(last);
                if elapsed >= Duration::from_millis(500) {
                    let secs = elapsed.as_secs_f64().max(1e-6);
                    log::info!(
                        "[pty] {:.0} feeds/s, {:.2} MB/s, avg feed {:.0}µs | blocks={} buffered_lines={} rss={:.0}/{:.0}MB (cur/peak)",
                        s.0 as f64 / secs,
                        s.1 as f64 / 1e6 / secs,
                        s.2.as_micros() as f64 / s.0.max(1) as f64,
                        blocks,
                        buffered_lines,
                        current_rss_mb(),
                        peak_rss_mb(),
                    );
                    *s = (0, 0, Duration::ZERO, Some(now));
                }
            });
        }

        // Track alt-screen edges even while startup output is suppressed.
        //
        // Suppression hides pre-join noise, but it must not hide a MODE CHANGE.
        // A mirror sets it at construction and only clears it on the first
        // `precmd` — so a viewer that joins at a prompt (having missed that
        // hook) still had it set when the host started a full-screen program,
        // returned early here, and never learned the alt-screen had opened. The
        // effect was stark and looked unrelated: no AI-tool panel for Claude
        // Code, and a blank screen if the alt path was rendered anyway. It only
        // worked when a VIEWER started the program, because that round-trip
        // produced a `precmd` first and cleared suppression.
        let alt_before = self.alt_screen_active();
        self.sync_alt_screen_edges();

        if self.suppress_startup_output {
            // A mode change still has to reach the screen: suppression skips the
            // repaint events below, so entering or leaving the alt-screen while
            // suppressed would be recorded but never drawn.
            if self.alt_screen_active() != alt_before {
                self.alt_ui.bump_paint();
                self.pending_effects
                    .push(SessionEffect::Ui(TerminalEvent::OutputAppended));
            }
            return PtyFeedStats {
                bytes_processed: bytes.len(),
            };
        }

        if fed_block_id.is_some() {}

        self.alt_ui.bump_paint();
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::OutputAppended));
        // Ride the same effect path so a shared session can mirror this pane's
        // output. The view broadcasts it only when this pane is being shared.
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::OutputChunk {
                block_id: fed_block_id,
                bytes: bytes.to_vec(),
            }));
        return PtyFeedStats {
            bytes_processed: bytes.len(),
        };
    }

    fn apply_and_process(
        &mut self,
        operations: &[dragent_terminal::operations::Operation],
        theme: &Theme,
    ) {
        // ConPTY's rendered fence belongs to the prompt. Split at the decoded
        // CSI so it works even when the source sequence crossed PTY reads.
        #[cfg(windows)]
        if self.draining_block_id.is_some() || self.active_block_id.is_some() {
            let mut rest = operations;
            while let Some(index) = rest.iter().position(|op| matches!(op,
                dragent_terminal::operations::Operation::Csi { params, private: None, action: b'm', .. }
                if params.windows(5).any(|p| p == [48, 2, 1, 2, 3]))) {
                let events = self.apply_active_operations(&rest[..index], theme);
                self.process_events(events, theme);
                self.finalise_draining_block(theme);
                let events = self.apply_active_operations(&rest[index..index + 1], theme);
                self.process_events(events, theme);
                rest = &rest[index + 1..];
            }
            let events = self.apply_active_operations(rest, theme);
            self.process_events(events, theme);
            return;
        }
        // Shell hooks change which emulator owns subsequent output. Apply
        // each hook before the following operation, including on cold replay.
        let mut start = 0;
        for (index, operation) in operations.iter().enumerate() {
            if matches!(operation, dragent_terminal::operations::Operation::Osc(payload)
                if payload.starts_with(b"7979;") || payload.starts_with(b"133;")) {
                let events = self.apply_active_operations(&operations[start..=index], theme);
                self.process_events(events, theme);
                start = index + 1;
            }
        }
        let events = self.apply_active_operations(&operations[start..], theme);
        self.process_events(events, theme);
    }

    /// Remember what the running program was told when it asked for colours.
    fn note_colors_answered(&mut self, events: &[TermEvent], theme: &Theme) {
        if events.iter().any(|e| matches!(e, TermEvent::ColorQuery { .. })) {
            let current = self.reply_theme.as_ref().unwrap_or(theme);
            let rgb = |c: warpui::color::ColorU| (c.r, c.g, c.b);
            self.app_colors = Some(dragent_host::screen_protocol::TerminalColors {
                foreground: rgb(current.foreground),
                background: rgb(current.background),
                is_dark: current.is_dark,
            });
        }
    }

    fn apply_active_operations(
        &mut self,
        operations: &[dragent_terminal::operations::Operation],
        theme: &Theme,
    ) -> Vec<TermEvent> {
        if operations.is_empty() {
            return Vec::new();
        }
        if let Some(id) = self.active_block_id.or(self.draining_block_id) {
            if let Some(block) = self.blocks.get_mut(id) {
                if let Some(emu) = block.emulator.as_mut() {
                    emu.apply_operations(operations);
                    let events: Vec<TermEvent> = emu.take_events();
                    block.sync_live_line_count();
                    self.note_colors_answered(&events, theme);
                    return events.into_iter()
                        .filter(|event| !answered_by_daemon(event)).collect();
                }
            }
        }
        self.session.apply_operations(operations);
        let events: Vec<TermEvent> = self.session.take_events();
        self.note_colors_answered(&events, theme);
        events.into_iter()
            .filter(|event| !answered_by_daemon(event)).collect()
    }

    /// Feed PTY bytes to whichever emulator owns them and process the events.
    ///
    /// While a block is active or draining (Windows), the chunk is scanned for
    /// the rendered fence so the block can be split and finalised at the exact
    /// point ConPTY finished repainting its output. The scan must also run for
    /// an ACTIVE block because a fast command's `precmd` and fence arrive in
    /// the same chunk — waiting for the drain state to exist would feed the
    /// prompt bytes into the block first.
    fn feed_and_process(&mut self, bytes: &[u8], theme: &Theme) {
        #[cfg(windows)]
        if self.draining_block_id.is_some() || self.active_block_id.is_some() {
            self.feed_split_at_fence(bytes, theme);
            return;
        }
        let events = self.feed_active_emulator(bytes);
        self.process_events(events, theme);
    }

    /// Feed a chunk while a block is capturing, splitting at the rendered fence.
    ///
    /// The fence is the SGR that paints the sentinel prompt cell our shell
    /// hooks emit right after the `precmd` OSC (see `HookEvent::Fence`; bg RGB
    /// 1,2,3). Unlike OSC/DECSET passthrough — which ConPTY forwards ahead of
    /// the text it is still repainting — cell content and its attributes are
    /// re-emitted strictly in order, so every byte before the sentinel is
    /// fully-flushed command output and everything from it on belongs to the
    /// prompt. The scan matches the parameter substring rather than a full
    /// sequence because ConPTY may merge SGR parameters when re-emitting.
    ///
    /// Splitting the bytes (rather than only reacting to the emulator's Fence
    /// event) is what keeps the shell prompt OUT of the block's grid — the
    /// event alone fires at the right time but only after the whole chunk,
    /// prompt included, has already been fed. A stale sentinel from a repaint
    /// of an older prompt makes the split a harmless no-op: `finalise` does
    /// nothing unless a block is draining, and both halves route to the same
    /// still-active block.
    #[cfg(windows)]
    fn feed_split_at_fence(&mut self, bytes: &[u8], theme: &Theme) {
        const FENCE: &[u8] = b"48;2;1;2;3";
        let mut window = std::mem::take(&mut self.fence_scan_tail);
        let carry = window.len();
        window.extend_from_slice(bytes);

        if let Some(pos) = window.windows(FENCE.len()).position(|w| w == FENCE) {
            // Split THIS chunk at the fence START (the sentinel cell belongs to
            // the prompt side). Bytes from the previous chunk (the carry) were
            // already fed to the capturing block last time; backtrack past the
            // ESC[ introducer so the partial SGR doesn't leak into the block.
            let sgr_start = window[..pos]
                .iter()
                .rposition(|&b| b == 0x1b)
                .unwrap_or(pos);
            let split = sgr_start.saturating_sub(carry).min(bytes.len());
            let (head, rest) = bytes.split_at(split);
            log::debug!(
                "[drain] fence found: chunk={}B head={}B rest={}B",
                bytes.len(),
                head.len(),
                rest.len()
            );
            let events = self.feed_active_emulator(head);
            self.process_events(events, theme);
            self.finalise_draining_block(theme);
            if !rest.is_empty() {
                let events = self.feed_active_emulator(rest);
                self.process_events(events, theme);
            }
        } else {
            let events = self.feed_active_emulator(bytes);
            self.process_events(events, theme);
            // Keep enough tail to detect a fence split across chunks, including
            // a preceding ESC[ introducer and parameters.
            let keep = window.len().min(FENCE.len() + 15);
            self.fence_scan_tail = window[window.len() - keep..].to_vec();
        }
    }

    fn feed_active_emulator(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        if bytes.is_empty() {
            return Vec::new();
        }
        // A draining block (completed, output still flushing) keeps receiving
        // bytes until it is finalised; a newly-started block takes priority.
        if let Some(id) = self.active_block_id.or(self.draining_block_id) {
            if let Some(block) = self.blocks.get_mut(id) {
                if let Some(emu) = block.emulator.as_mut() {
                    emu.feed(bytes);
                    let events = emu.take_events();
                    block.sync_live_line_count();
                    return events;
                }
            }
        }

        self.session.feed(bytes);
        self.session.take_events()
    }

    fn clear_session_surface(&mut self) {
        self.session.feed(b"\x1b[2J\x1b[H");
        let _ = self.session.take_events();
        self.blocks.bootstrap_lines.clear();
        self.blocks.bootstrap_height_px = 0.0;
    }

    pub(crate) fn clear_session_surface_preserving_bootstrap(&mut self) {
        self.session.feed(b"\x1b[2J\x1b[H");
        let _ = self.session.take_events();
    }

    fn process_events(&mut self, events: Vec<TermEvent>, theme: &Theme) {
        for event in events {
            match event {
                TermEvent::Hook(HookEvent::Preexec { command }) => {
                    self.submitted_unseen = false;
                    // An adopted older daemon may still emit the private
                    // launch monitor as the preexec command.
                    self.on_preexec(dragent_terminal::shell_integration::agent_display_command(&command).to_string());
                }
                TermEvent::Hook(HookEvent::Precmd { exit_code, cwd }) => {
                    let cwd_path = if cwd.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(&cwd))
                    };
                    self.on_precmd(Some(exit_code), cwd_path, theme);
                }
                TermEvent::Hook(HookEvent::Clear) => {
                    if self.should_clear_viewport() {
                        self.pending_effects
                            .push(SessionEffect::Ui(TerminalEvent::ScreenCleared));
                    }
                }
                TermEvent::Hook(HookEvent::Fence) => {
                    // The sentinel prompt cell rendered — every byte of the
                    // completed command's output has flushed through ConPTY.
                    // Event order is parse order, so this also works when the
                    // fence arrives in the same chunk as precmd (the raw
                    // byte-scanner below can't see that case). Repaint-driven
                    // repeats find nothing draining and no-op.
                    #[cfg(windows)]
                    {
                        log::debug!("[drain] fence cell rendered");
                        self.finalise_draining_block(theme);
                    }
                }
                TermEvent::SemanticPrompt(marker) => {
                    self.semantic_active = true;
                    self.handle_semantic(marker, theme);
                }
                TermEvent::TitleChanged(title) => {
                    let t = title.trim().to_string();
                    if !t.is_empty()
                        && !is_default_shell_title(&t)
                        && self.shell_title.as_deref() != Some(t.as_str())
                    {
                        self.shell_title = Some(t);
                        self.pending_effects
                            .push(SessionEffect::Ui(TerminalEvent::TitleChanged));
                    }
                }
                TermEvent::DcsReceived(response) => {
                    self.pending_effects.push(SessionEffect::PtyWrite(response));
                }
                TermEvent::Reply(response) => {
                    self.pending_effects.push(SessionEffect::PtyWrite(response));
                }
                TermEvent::ColorQuery { index } => {
                    self.note_colors_answered(&[TermEvent::ColorQuery { index }], theme);
                    let current = self.reply_theme.as_ref().unwrap_or(theme);
                    let c = if index == 10 {
                        current.foreground
                    } else {
                        current.background
                    };
                    self.pending_effects.push(SessionEffect::PtyWrite(
                        dragent_terminal::color_query_report(index, (c.r, c.g, c.b)),
                    ));
                }
                TermEvent::ColorSchemeQuery => {
                    let is_dark = self.reply_theme.as_ref().unwrap_or(theme).is_dark;
                    self.pending_effects.push(SessionEffect::PtyWrite(
                        dragent_terminal::color_scheme_report(is_dark),
                    ));
                }
                TermEvent::ClipboardWrite(text) => {
                    self.pending_effects
                        .push(SessionEffect::Ui(TerminalEvent::ClipboardWrite(text)));
                }
                TermEvent::ScreenCleared => {
                    if self.should_clear_viewport() {
                        self.pending_effects
                            .push(SessionEffect::Ui(TerminalEvent::ScreenCleared));
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn on_preexec(&mut self, command: String) {
        self.app_colors = None;
        if std::env::var_os("DRAGENT_LOG_PTY").is_some() {
            log::info!(
                "[mem] on_preexec cmd={:?} blocks={} rss={:.0}MB",
                command.chars().take(40).collect::<String>(),
                self.blocks.blocks().len(),
                current_rss_mb(),
            );
        }
        self.hooks_active = true;
        if let Some(new_cwd) = resolve_cd_target(&self.cwd, &command).map(|cwd| self.local_cwd(cwd)) {
            if cwd_changed(&self.cwd, &new_cwd) {
                self.cwd = new_cwd;
                self.pending_effects
                    .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
            }
        }
        // A mirror's blocks are the host's; creating one here would duplicate it.
        if self.suppress_local_blocks {
            return;
        }
        if let Some(id) = self.active_block_id {
            if let Some(block) = self.blocks.get_mut(id) {
                block.on_preexec(Some(command));
            }
        } else {
            // No block yet means the command was typed straight at the shell
            // prompt rather than through the app's input bar, so this hook
            // report is the only text we have. Strip known shell-alias
            // expansions (e.g. Git Bash's `ls` -> `ls -F --color=auto
            // --show-control-chars`) to show what was actually typed.
            let command = crate::console::dir_listing::strip_ls_alias_flags(&command).into_owned();
            let id = self.blocks.next_id();
            let mut block = Block::new(
                id,
                command.clone(),
                self.cwd.clone(),
                self.size.rows as usize,
                self.size.cols as usize,
            );
            block.remote_filesystem = self.remote_machine.is_some();
            block.on_preexec(Some(command.clone()));
            self.blocks.begin(block);
            self.active_block_id = Some(id);
            self.input_mode = InputMode::from_submitted_command(&command);
        }
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
    }

    /// Start a command block when the user submits from the input bar (before shell hooks fire).
    /// The running program's session was closed on purpose and a new one will
    /// replace it. Without this, its block stays open and the next command is
    /// written using the old program's terminal modes, such as bracketed paste.
    /// Nothing failed or finished, so this neither marks the pane failed nor
    /// reports a completed command.
    pub fn end_running_command(&mut self, theme: &Theme) {
        if self.active_block_id.is_some() {
            self.on_precmd(Some(0), None, theme);
            self.finalise_draining_block(theme);
        }
        self.pending_effects.retain(|effect| {
            !matches!(effect, SessionEffect::Ui(TerminalEvent::BlockCompleted { .. }))
        });
        self.sync_alt_screen_edges();
    }

    pub fn begin_command(&mut self, command: String) {
        if Self::is_clear_command(&command) {
            self.clear_submitted = true;
        }
        if self.active_block_id.is_none() {
            self.on_preexec(command);
            self.submitted_unseen = self.active_block_id.is_some();
        }
    }

    pub(crate) fn on_precmd(
        &mut self,
        exit_code: Option<i32>,
        cwd: Option<PathBuf>,
        theme: &Theme,
    ) {
        self.app_colors = None;
        self.hooks_active = true;
        // The starting shell's first prompt. A command the app submitted
        // meanwhile is still queued in the daemon behind it, not finished.
        let startup_prompt = self.host_shell_starting.take().is_some();
        if startup_prompt {
            // Everything so far was the shell starting (its prompt, the
            // integration echo), not output of the queued command.
            if self.submitted_unseen {
                if let Some(block) = self.active_block_id.and_then(|id| self.blocks.get_mut(id)) {
                    block.discard_output();
                }
            }
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
        }
        self.session.reset_pointer_modes();
        if !self.bootstrapped {
            self.bootstrapped = true;
            if self.suppress_startup_output {
                self.suppress_startup_output = false;
                self.clear_session_surface();
            } else {
                self.refresh_bootstrap_lines(theme);
            }
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
        }
        if let Some(new_cwd) = cwd.map(normalize_shell_cwd).map(|cwd| self.local_cwd(cwd)) {
            if cwd_changed(&self.cwd, &new_cwd) {
                self.cwd = new_cwd;
                self.pending_effects
                    .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
            }
        }
        if self.active_block_id.is_some() && !(startup_prompt && self.submitted_unseen) {
            // Windows ConPTY interleaves this OSC into the middle of the
            // command's rendered output, so its stream position cannot be the
            // output boundary — defer the snapshot until the rendered fence.
            // Unix PTYs stream in order; finalise at the hook as usual.
            #[cfg(windows)]
            self.begin_drain(exit_code, theme);
            #[cfg(not(windows))]
            self.finalise_active_block(exit_code, theme);
        }
    }

    /// Windows ConPTY: mark the active block completed but defer its snapshot.
    ///
    /// ConPTY passes the `precmd` OSC through ahead of output it is still
    /// repainting, so finalising here would truncate the block (e.g. `ls`
    /// capturing only its first entry). Instead: record the exit code, return
    /// the input bar immediately, and keep feeding this block's emulator until
    /// the rendered `ESC[?2004h` fence, the next `preexec`, or the view's
    /// drain timeout — whichever comes first.
    #[cfg(windows)]
    fn begin_drain(&mut self, exit_code: Option<i32>, theme: &Theme) {
        // A previous block still draining (timeout not yet fired) must close
        // before the next one starts draining.
        self.finalise_draining_block(theme);
        self.submitted_unseen = false;

        let Some(id) = self.active_block_id.take() else {
            return;
        };
        if let Some(block) = self.blocks.get_mut(id) {
            block.set_completed_meta(exit_code);
        }
        self.draining_block_id = Some(id);
        self.draining_exit_code = exit_code;
        self.fence_scan_tail.clear();
        self.input_mode = self.input_mode.clone().on_block_completed();
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlockDraining));
    }

    /// Snapshot and close a block whose completion was deferred by the drain
    /// path. No-op when nothing is draining, so the fence, the drain timeout,
    /// and the next `precmd` can all call it without double-finalising.
    pub fn finalise_draining_block(&mut self, theme: &Theme) {
        let Some(id) = self.draining_block_id.take() else {
            return;
        };
        log::debug!("[drain] finalising block {id}");
        let exit_code = self.draining_exit_code.take();
        self.fence_scan_tail.clear();

        let context = if let Some(block) = self.blocks.get_mut(id) {
            block.snapshot_and_release(theme);
            let ctx = block_context_from_lines(
                block.id,
                block.command.clone(),
                block.cwd.clone(),
                exit_code,
                block.duration_ms,
                &block.lines,
            );
            block.set_completion(ctx.clone());
            ctx
        } else {
            return;
        };

        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlockCompleted {
                context: Arc::new(context),
            }));
    }

    fn handle_semantic(&mut self, marker: SemanticMarker, theme: &Theme) {
        match marker {
            SemanticMarker::CommandStart => {
                self.submitted_unseen = false;
                let mark = self.session.cursor_abs_line();
                if self.active_block_id.is_some() {
                    self.apply_inferred_command(mark);
                } else {
                    self.start_block_for_semantic_command(mark);
                }
                self.pending_effects
                    .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
            }
            SemanticMarker::CommandEnd { exit_code } => {
                self.on_semantic_command_end(exit_code, theme);
            }
            SemanticMarker::PromptStart | SemanticMarker::PromptEnd => {
                if !self.bootstrapped {
                    self.refresh_bootstrap_lines(theme);
                }
            }
            SemanticMarker::Property { key, value } => {
                if key == "cwd" && !value.is_empty() {
                    let new_cwd = self.local_cwd(PathBuf::from(&value));
                    if cwd_changed(&self.cwd, &new_cwd) {
                        self.cwd = new_cwd;
                        self.pending_effects
                            .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
                    }
                }
            }
        }
    }

    fn start_block_for_semantic_command(&mut self, mark: usize) {
        let command = self.infer_command_text(mark);
        let id = self.blocks.next_id();
        let block = Block::new(
            id,
            command.clone(),
            self.cwd.clone(),
            self.size.rows as usize,
            self.size.cols as usize,
        );
        let mut block = block;
        block.remote_filesystem = self.remote_machine.is_some();
        block.on_preexec(if command.is_empty() {
            None
        } else {
            Some(command.clone())
        });
        self.blocks.begin(block);
        self.active_block_id = Some(id);
        if !command.is_empty() {
            self.input_mode = InputMode::from_submitted_command(&command);
        }
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
    }

    /// Read the command line from the bootstrap emulator — the line immediately
    /// above the OSC 133 output mark.
    fn infer_command_text(&self, output_mark: usize) -> String {
        if output_mark == 0 {
            return String::new();
        }
        self.session
            .styled_lines_in(output_mark.saturating_sub(1), output_mark)
            .last()
            .map(|line| line.plain_text().trim().to_string())
            .unwrap_or_default()
    }

    fn apply_inferred_command(&mut self, output_mark: usize) {
        let inferred = self.infer_command_text(output_mark);
        if inferred.is_empty() {
            return;
        }
        if let Some(id) = self.active_block_id {
            if let Some(block) = self.blocks.get_mut(id) {
                if block.command.is_empty() {
                    block.command = inferred;
                }
            }
        }
    }

    fn on_semantic_command_end(&mut self, exit_code: Option<i32>, theme: &Theme) {
        self.semantic_active = true;
        self.session.reset_pointer_modes();
        if !self.bootstrapped {
            self.bootstrapped = true;
            self.refresh_bootstrap_lines(theme);
            self.pending_effects
                .push(SessionEffect::Ui(TerminalEvent::StatusChanged));
        }
        if self.active_block_id.is_some() {
            self.finalise_active_block(exit_code, theme);
        }
    }

    fn finalise_active_block(&mut self, exit_code: Option<i32>, theme: &Theme) {
        self.submitted_unseen = false;
        let Some(id) = self.active_block_id.take() else {
            return;
        };

        let log_mem = std::env::var_os("DRAGENT_LOG_PTY").is_some();
        let rss_before = if log_mem { current_rss_mb() } else { 0.0 };
        let context = if let Some(block) = self.blocks.get_mut(id) {
            block.on_completed(exit_code, theme);
            if log_mem {
                log::info!(
                    "[mem] block {} completed: snapshot {} lines, rss {:.0}->{:.0}MB",
                    block.id,
                    block.lines.len(),
                    rss_before,
                    current_rss_mb(),
                );
            }
            let ctx = block_context_from_lines(
                block.id,
                block.command.clone(),
                block.cwd.clone(),
                exit_code,
                block.duration_ms,
                &block.lines,
            );
            block.set_completion(ctx.clone());
            ctx
        } else {
            return;
        };

        if std::env::var_os("DRAGENT_LOG_BLOCK_CONTEXT").is_some() {
            log::debug!(
                "block completed id={} cmd={:?} exit={:?} failed={} out_bytes={} truncated={}",
                context.block_id,
                context.command,
                context.exit_code,
                context.failed,
                context.output_text.len(),
                context.output_truncated,
            );
        }

        self.input_mode = self.input_mode.clone().on_block_completed();
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlocksChanged));
        self.pending_effects
            .push(SessionEffect::Ui(TerminalEvent::BlockCompleted {
                context: Arc::new(context),
            }));
    }
}
