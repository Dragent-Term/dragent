//! Scratch-shell PTY for terminal side commands.
//!
//! SideShell lifecycle and output helpers. Moved verbatim; same behavior.

use crate::agent::input_classifier::{
    heuristic::HeuristicInputClassifier, ClassifyContext, InputClassifier,
};
use crate::agent::{DetectedInputType, InputConfig, TabSessionKind};
#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use crate::console::ai_toolbar::{self, AiPanelActions, AiTool};
use crate::console::block_filter::{block_output_lines, count_block_matches};
use crate::console::command_intel;
use crate::console::dir_listing::{resolve_entry_path, DirSort, DirView};
use crate::console::editor_settings::EditorSettings;
use crate::console::escape_parse::styled_to_colored;
use crate::console::fs_suggest::should_suggest_paths;
use crate::console::history_search::HistorySearchState;
use crate::console::input_bar;
use crate::console::input_editing;
use crate::console::input_overlays;
use crate::console::path_picker::PathPicker;
use crate::console::prefixed_history;
use crate::console::shell_aliases::ShellAliases;
use crate::console::suggest_engine::SuggestEngine;
use crate::console::workflow_search::WorkflowSearchState;
use crate::console::workflows::{Workflow, WorkflowSession, WorkflowStore};
use crate::icons::ICON_SIZE_SM;
use crate::process_ext::NoWindow;
use crate::setup::UiSettings;
use crate::terminal::input::{
    encode_keystroke, encode_paste, map_pointer_to_cell, modifiers_from_ui, mouse_event,
    scroll_arrow_bytes, should_encode_wheel_as_mouse,
    should_forward_key_to_pty_while_ai_panel_open, should_passthrough_mouse, wheel_button,
    RichInputKeyContext,
};
use crate::terminal::manager::TerminalManager;
use crate::terminal::model::block_height_tree::BlockHeightIndex;
use crate::terminal::model::{
    ShellLinkStatus, SizeInfo, SizeReason, SizeUpdate, TerminalEvent, TerminalModel,
};
use crate::terminal::render::{
    build_alt_screen_ui, build_block_options_menu, build_bootstrap_grid_ui,
    build_sticky_block_header, build_visible_block_list_column, dir_grid_content_width,
    extract_prompt_tail, sample_ai_chrome, BlockListScrollAdapter, BlockRenderContext,
    CellGlyphCache, CellMetrics, DiagramCanvasState, StickyBlockHeader, TerminalSizeElement,
    BLOCK_HEADER_HEIGHT, BLOCK_LIST_BOTTOM_POSITION_ID,
};
use crate::terminal::viewport::{
    is_block_list_virtualization_enabled, ScrollMode, ScrollPosition, ViewportState,
};
use crate::terminal::TerminalEventListener;
use crate::theme::{Theme, INPUT_FONT_SIZE, TERMINAL_PADDING_LEFT};
use crate::workspace::persistence::PersistedBlock;
use crate::workspace::{InputEditor, InputEditorEvent, InputMode, DEFAULT_SESSION_TITLE};
use async_channel::Sender;
use dragent_pty::{resolve_shell, PtyEvent, SessionPlatform};
use dragent_resource_policy::{ActivityLevel, MemoryPressure, ResourceBudget};
use dragent_shell_hooks::ShellKind;
use dragent_terminal::{MouseButton, MouseEventKind, TermMode};
use parking_lot::{FairMutex, RwLock};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, Dismiss, DispatchEventResult, Element, EventHandler, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, SelectableArea,
    SelectionHandle, SelectionUpdateArgs, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::Vector2F;
use warpui::keymap::Keystroke;
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::{IntoPixels, Pixels};
use warpui::{
    r#async::Timer, AppContext, Entity, EntityId, FocusContext, GetSingletonModelHandle,
    ModelHandle, ReadModel, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{LiveElement, ScrollTarget, ScrollToPositionMode};
use warpui_core::event::{KeyState, ModifiersState};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::scene::Border;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::WindowId;
use web_time::Instant;
/// Scratch-shell PTY geometry. The emulator lays commands like `ls` out for this
/// width; the card renders the resulting grid in monospace so columns line up.
pub(crate) const SIDE_SHELL_ROWS: usize = 24;
pub(crate) const SIDE_SHELL_COLS: usize = 100;
/// Scrollback cap (lines) so a chatty session can't grow without bound.
pub(crate) const SIDE_SHELL_SCROLLBACK: usize = 5000;

/// A persistent scratch shell. Like every terminal it runs in the host's PTY
/// daemon (so it is covered by the host's supervision, flow control and
/// upgrade handoff), not inside the app. Dropping it closes the session.
pub(crate) struct SideShell {
    pub(crate) handle: SideShellIo,
    /// A full VT emulator fed the raw output stream. Read back as plain text
    /// for the card. Locked by both the stream thread (feed) and the UI
    /// thread (render).
    pub(crate) emulator: std::sync::Arc<std::sync::Mutex<dragent_terminal::Emulator>>,
    /// Bumped on every output chunk — drives the "active" indicator.
    pub(crate) activity: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

enum SideInput {
    Bytes(Vec<u8>),
    Close,
}

/// Input side of the scratch shell. Every write goes through one ordered
/// queue to the host, so bytes sent before the host session exists are
/// delivered, in order, once it does.
pub(crate) struct SideShellIo {
    tx: std::sync::mpsc::Sender<SideInput>,
}

impl SideShellIo {
    pub(crate) fn write(&self, bytes: &[u8]) -> Result<(), ()> {
        self.tx
            .send(SideInput::Bytes(bytes.to_vec()))
            .map_err(|_| ())
    }

    /// Stop the foreground job. The Ctrl+C byte the caller already wrote does
    /// it on every platform: the PTY line discipline turns it into SIGINT for
    /// the foreground group (Unix), ConPTY into CTRL_C_EVENT (Windows).
    pub(crate) fn terminate(&self) -> Result<(), ()> {
        Ok(())
    }

    pub(crate) fn shutdown(&self) -> Result<(), ()> {
        self.tx.send(SideInput::Close).map_err(|_| ())
    }

    /// Host sessions don't report the foreground job yet, so callers use
    /// recent output as the "running" signal (as they already do on
    /// Windows, which has no process groups).
    pub(crate) fn foreground_running(&self) -> bool {
        false
    }
}

impl Drop for SideShell {
    fn drop(&mut self) {
        let _ = self.handle.shutdown();
    }
}

/// Render the scratch shell's terminal grid (scrollback + visible screen) into
/// colored segments per line — so ANSI colors (git's red/green, ls, etc.) show,
/// like a real terminal. Right-trims each row and drops the blank tail.
pub(crate) fn side_shell_colored_lines(
    emu: &std::sync::Mutex<dragent_terminal::Emulator>,
    theme: &Theme,
    max_lines: usize,
) -> Vec<Vec<(String, warpui::color::ColorU)>> {
    let guard = match emu.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    // `grid()` is always the primary (non-alt) screen, so a command that briefly
    // enters the alt screen (e.g. a pager) doesn't wipe the transcript view.
    let grid = guard.grid();
    let total = grid.total_lines();
    let start = total.saturating_sub(max_lines);
    let default_fg = theme.main_text();
    let mut lines: Vec<Vec<(String, warpui::color::ColorU)>> = Vec::new();
    for i in start..total {
        let Some(row) = grid.line(i) else { continue };
        let colored = crate::console::escape_parse::styled_to_colored(&row.styled_line(), theme);
        let mut segs: Vec<(String, warpui::color::ColorU)> = colored
            .spans
            .iter()
            .map(|s| (s.text.clone(), s.color.unwrap_or(default_fg)))
            .collect();
        // Right-trim trailing padding (blank cells the fixed-width grid emits).
        while segs
            .last()
            .map(|(t, _)| t.trim().is_empty())
            .unwrap_or(false)
        {
            segs.pop();
        }
        if let Some(last) = segs.last_mut() {
            last.0 = last.0.trim_end().to_string();
        }
        lines.push(segs);
    }
    while lines
        .last()
        .map(|l| l.iter().all(|(t, _)| t.is_empty()))
        .unwrap_or(false)
    {
        lines.pop();
    }
    lines
}

/// Start the persistent scratch shell as a host session in `cwd`. Returns
/// immediately; the session is created in the background and anything
/// written meanwhile is queued. The emulator shows why if the host can't be
/// reached.
pub(crate) fn spawn_side_shell(cwd: &std::path::Path) -> Option<SideShell> {
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    // Unit tests never start or touch a live host (the test binary would be
    // spawned as one).
    if cfg!(test) {
        return None;
    }
    let emulator = Arc::new(Mutex::new(dragent_terminal::Emulator::with_scrollback(
        SIDE_SHELL_ROWS,
        SIDE_SHELL_COLS,
        SIDE_SHELL_SCROLLBACK,
    )));
    let activity = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (tx, rx) = std::sync::mpsc::channel::<SideInput>();
    let root = cwd.to_string_lossy().into_owned();
    let feed_emu = emulator.clone();
    let resume_emu = emulator.clone();
    let feed_activity = activity.clone();
    std::thread::spawn(move || {
        let feed =
            move |bytes: &[u8], operations: Option<&[dragent_terminal::operations::Operation]>| {
                if let Ok(mut e) = feed_emu.lock() {
                    if let Some(operations) = operations {
                        // The daemon already parsed this frame. Applying decoded
                        // operations keeps this auxiliary terminal off the UI
                        // parser path and prevents duplicate device replies.
                        e.apply_operations(operations);
                    } else {
                        e.feed(bytes);
                    }
                }
                feed_activity.fetch_add(1, Ordering::Relaxed);
            };
        // A root home registers the user's directory with the host (no
        // checkout, never deleted); the shell runs right there.
        let setup = (|| -> Result<_, String> {
            let client = dragent_host::HostClient::ensure_host()?;
            let workspace_id = client.create_local_workspace(&root)?;
            let home = client.ensure_home(&workspace_id, &root)?;
            let session = client.create_terminal(
                &workspace_id,
                &home.id,
                None,
                Some(&root),
                SIDE_SHELL_COLS as u16,
                SIDE_SHELL_ROWS as u16,
            )?;
            Ok((client, workspace_id, home.id, session.id))
        })();
        let (client, workspace_id, home_id, session_id) = match setup {
            Ok(parts) => parts,
            Err(error) => {
                feed(
                    format!("\r\n[scratch shell unavailable: {error}]\r\n").as_bytes(),
                    None,
                );
                return;
            }
        };

        // Output: stream from the host, reconnecting from the last offset.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (client, workspace_id, session_id, stop) = (
                client.clone(),
                workspace_id.clone(),
                session_id.clone(),
                stop.clone(),
            );
            let mut feed = feed;
            std::thread::spawn(move || {
                let mut offset = 0u64;
                let mut attempt = 0u32;
                loop {
                    let mut exited = false;
                    let result = client.stream_terminal_operations(
                        &workspace_id,
                        &session_id,
                        offset,
                        |chunk| {
                            if stop.load(Ordering::Relaxed) {
                                return false;
                            }
                            attempt = 0;
                            if let Some(resume) = &chunk.resume {
                                let Ok(mut emulator) = resume_emu.lock() else { return false; };
                                if emulator.restore_display_checkpoint(&resume.display).is_err() { return false; }
                            }
                            offset = offset.max(chunk.base + chunk.data.len() as u64);
                            if !chunk.data.is_empty() {
                                let Some(operations) = chunk.operations.as_deref() else {
                                    return false;
                                };
                                feed(&chunk.data, Some(operations));
                            }
                            exited = chunk.exit_code.is_some();
                            true
                        },
                    );
                    if exited || stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if result.is_err() && client.terminal(&workspace_id, &session_id).is_err() {
                        return; // the session is gone for good
                    }
                    std::thread::sleep(Duration::from_millis(
                        (250u64 << attempt.min(4)).min(4_000),
                    ));
                    attempt = attempt.saturating_add(1);
                }
            });
        }

        // Input, in order, until the view drops the shell.
        while let Ok(SideInput::Bytes(bytes)) = rx.recv() {
            let _ = client.send_terminal_input(&workspace_id, &session_id, &bytes);
        }
        stop.store(true, Ordering::Relaxed);
        let _ = client.close_terminal(&workspace_id, &session_id);
        let _ = client.teardown_worktree(&workspace_id, &home_id);
    });

    Some(SideShell {
        handle: SideShellIo { tx },
        emulator,
        activity,
    })
}

use super::terminal_view::TerminalView;

impl TerminalView {
    /// Record a scratch-shell command in this session's history (newest last),
    /// de-duplicating an immediate repeat so ↑ steps through distinct commands.
    pub(crate) fn record_side_command(&mut self, command: &String) {
        if command.is_empty() {
            return;
        }
        if self.side_cmd_history.last() == Some(command) {
            return;
        }
        self.side_cmd_history.push(command.clone());
    }

    /// Handle a `!`-prefixed submission from the AI input. An empty command
    /// (`!` alone) just reopens the scratch-shell card; `!clear` wipes the card's
    /// output (the shell — and its cd/env — lives on). A real command is written
    /// straight to the PTY (the shell echoes and runs it, streaming output),
    /// so state like `cd`/exports persists across `!` commands.
    pub(crate) fn send_side_command(&mut self, command: String, ctx: &mut ViewContext<Self>) {
        if command.is_empty() {
            if self.side_shell.is_some() {
                self.side_card_open = true;
                self.arm_side_poll(ctx);
                ctx.notify();
            }
            return;
        }
        // `!clear` / `!cls` clears the transcript without disturbing the shell —
        // reset the emulator grid in place (the feed callback shares the Arc, so
        // it keeps writing into the fresh emulator).
        if matches!(command.to_ascii_lowercase().as_str(), "clear" | "cls") {
            if let Some(sh) = self.side_shell.as_ref() {
                if let Ok(mut e) = sh.emulator.lock() {
                    *e = dragent_terminal::Emulator::with_scrollback(
                        SIDE_SHELL_ROWS,
                        SIDE_SHELL_COLS,
                        SIDE_SHELL_SCROLLBACK,
                    );
                }
                // Nudge the shell to repaint a fresh prompt into the empty grid.
                let _ = sh.handle.write(b"\r");
            }
            self.record_side_command(&"clear".to_string());
            self.side_card_open = true;
            self.arm_side_poll(ctx);
            ctx.notify();
            return;
        }
        if self.side_shell.is_none() {
            let cwd = self.cwd(ctx);
            self.side_shell = spawn_side_shell(&cwd);
            if self.side_shell.is_none() {
                self.show_error_notice(("Terminal operation failed".to_string(), "Couldn't start the scratch shell.".to_string()), ctx);
                ctx.notify();
                return;
            }
        }
        if let Some(sh) = self.side_shell.as_ref() {
            // Same bracketed-paste delivery as the main input bar: send the whole
            // (possibly multi-line) command as one paste so the scratch shell
            // parses it as a single command, then a CR to run it. `encode_paste`
            // wraps in ESC[200~…ESC[201~ when the shell has bracketed paste on
            // (interactive shells do), else sends raw with newlines normalized.
            let mode = sh
                .emulator
                .lock()
                .map(|e| e.mode())
                .unwrap_or_else(|_| TermMode::empty());
            let mut buf = crate::terminal::input::paste::encode_paste(&command, mode);
            // Carriage return is Enter on a PTY line discipline (ConPTY requires
            // it; Unix accepts it). `\n` alone doesn't submit on Windows.
            buf.push(b'\r');
            let _ = sh.handle.write(&buf);
        }
        self.record_side_command(&command);
        // A new command jumps back to the tail so its output is visible even if
        // the user had scrolled up into history, and marks the shell active so the
        // Stop button shows immediately (before the first output arrives).
        self.side_follow_tail = true;
        self.side_activity_at = web_time::Instant::now();
        self.side_card_open = true;
        self.arm_side_poll(ctx);
        ctx.notify();
    }

    /// Stop whatever the scratch shell is running (the Stop button). Two paths,
    /// so it works on every platform and reaches child processes:
    ///   • Write the 0x03 (Ctrl+C) byte to the PTY. On Windows this is the ONLY
    ///     interrupt that works — ConPTY turns it into a CTRL_C_EVENT delivered
    ///     to every process attached to the pseudoconsole (the job + its
    ///     children). On Unix cooked mode the kernel's ISIG turns it into SIGINT.
    ///   • `terminate()` (Unix): SIGINT then SIGTERM to the foreground process
    ///     GROUP, so `yarn dev` → `next`/node children all die even when ISIG is
    ///     off (raw-mode TUIs). No-op on Windows (the byte above already did it).
    /// The shell itself is in a different group / not the foreground job, so
    /// cd/env persist. Harmless at an idle prompt.
    pub(crate) fn cancel_side_command(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(sh) = self.side_shell.as_ref() {
            let _ = sh.handle.write(&[0x03]);
            let _ = sh.handle.terminate();
        }
        ctx.notify();
    }

    /// Start (once) the repaint loop that surfaces streamed PTY output while the
    /// card is open. Self-cancels when the card closes.
    pub(crate) fn arm_side_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.workspace_active || self.side_poll_armed {
            return;
        }
        self.side_poll_armed = true;
        self.poll_side_shell(ctx);
    }

    pub(crate) fn poll_side_shell(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.workspace_active || !self.side_card_open || self.side_shell.is_none() {
            self.side_poll_armed = false;
            return;
        }
        // Note when output last streamed — drives the "process running" state that
        // shows/hides the Stop button.
        if let Some(sh) = self.side_shell.as_ref() {
            let a = sh.activity.load(std::sync::atomic::Ordering::Relaxed);
            if a != self.side_activity_seen {
                self.side_activity_seen = a;
                self.side_activity_at = web_time::Instant::now();
            }
        }
        // Follow the newest output unless the user scrolled up. scroll_to clamps
        // to the real bottom after layout, so a huge value pins it to the tail.
        if self.side_follow_tail {
            self.side_scroll.scroll_to(Pixels::new(f32::MAX));
        }
        ctx.notify();
        ctx.spawn(Timer::after(Duration::from_millis(120)), |view, _, mctx| {
            view.poll_side_shell(mctx);
        });
    }

    /// `×` on the scratch-shell card: hide it but KEEP the shell running and its
    /// output intact, so the next `!` reopens it where it left off.
    pub(crate) fn dismiss_side_command(&mut self, ctx: &mut ViewContext<Self>) {
        self.side_card_open = false;
        ctx.notify();
    }

    /// `⟲` on the card: shut the scratch shell down and start a fresh one (clears
    /// its state and output) — the escape hatch for a wedged shell.
    pub(crate) fn reset_side_shell(&mut self, ctx: &mut ViewContext<Self>) {
        self.side_shell = None; // Drop shuts the PTY down.
        let cwd = self.cwd(ctx);
        self.side_shell = spawn_side_shell(&cwd);
        self.arm_side_poll(ctx);
        ctx.notify();
    }
}
