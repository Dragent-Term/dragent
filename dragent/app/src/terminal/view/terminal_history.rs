//! Terminal history and workflow search.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT, CMD_HIST_CHROME_PX,
    CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y, SCROLL_BOTTOM_EPSILON_PX,
};
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
use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};
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

impl TerminalView {
    pub(crate) fn open_history_search(&mut self, ctx: &mut ViewContext<Self>) {
        let buffer = self
            .command_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        self.history_search.open = true;
        self.history_search.saved_buffer = buffer;
        self.history_search.query.clear();
        self.history_search.selected = 0;
        self.history_search.refresh(&self.command_history);
        self.close_workflow_search(ctx, false);
        ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text("", ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn close_history_search(
        &mut self,
        ctx: &mut ViewContext<Self>,
        restore_buffer: bool,
    ) {
        if !self.history_search.open {
            return;
        }
        let saved = self.history_search.saved_buffer.clone();
        self.history_search.open = false;
        self.history_search.query.clear();
        self.history_search.results.clear();
        self.history_search.selected = 0;
        if restore_buffer {
            ctx.update_view(&self.command_editor, |editor, ectx| {
                editor.set_text(&saved, ectx);
            });
            self.refresh_command_ghost(&saved, ctx);
        }
        ctx.notify();
    }

    // ── Warp-style command-history popover ────────────────────────────────────

    /// Rows the current popover height can show (used to window the list).
    pub(crate) fn cmd_hist_visible_count(&self) -> usize {
        // How many rows fit INSIDE the band, measured — not guessed. The old
        // estimate (72px chrome, 28px rows) was under both, so it asked for one
        // row too many: the rows then pushed the footer past the band's fixed
        // height and it landed on top of the input bar below.
        //
        //   chrome ≈ grip 20 (8+4+8) + header 24 (2+~14+7+1 border)
        //          + footer 29 (7+~14+7+1 border) + list padding 8 (4+4) ≈ 81
        //   row    ≈ text ~18 (12.5px mono) + padding 12 (6+6) ≈ 30
        //
        // Rounded UP (84 / 30) so rounding error costs a row rather than asking
        // for more rows than the user's chosen size implies. The band itself is
        // content-sized, so this only sets HOW MANY rows to show — it can no
        // longer overflow, and it must not depend on the laid-out pane height
        // (that fed layout back into itself and made the band flicker).
        let h = self.cmd_hist_height_px;
        (((h - CMD_HIST_CHROME_PX) / CMD_HIST_ROW_PX).floor() as i64).clamp(3, 20) as usize
    }

    /// Open the history popover: fetch recent commands (persisted; falling back
    /// to this session's in-memory history), remember the typed buffer for Esc,
    /// filter by any typed prefix, select the newest, and preview it.
    pub(crate) fn open_cmd_history(&mut self, ctx: &mut ViewContext<Self>) {
        let buffer = self
            .command_editor
            .read(ctx, |e, _| e.get_text().to_string());
        let prefix = buffer.trim().to_string();

        // Screen-clearing commands are noise in a history list — drop them.
        let is_noise = |c: &str| matches!(c.trim(), "clear" | "cls");

        let mut items: Vec<(String, i64)> = fetch_recent_commands()
            .into_iter()
            .filter(|(c, _)| !is_noise(c))
            .collect();
        if items.is_empty() {
            // No persisted rows yet — show this session's commands as "just now".
            let now = chrono::Utc::now().timestamp_millis();
            let mut seen = std::collections::HashSet::new();
            for cmd in self.command_history.iter().rev() {
                if !is_noise(cmd) && seen.insert(cmd.clone()) {
                    items.push((cmd.clone(), now));
                }
            }
        }
        if !prefix.is_empty() {
            let filtered: Vec<_> = items
                .iter()
                .filter(|(c, _)| c.starts_with(&prefix))
                .cloned()
                .collect();
            if !filtered.is_empty() {
                items = filtered;
            }
        }
        if items.is_empty() {
            return; // nothing to browse
        }

        // History entries are previously executed shell commands. Keep global
        // autodetection enabled, but temporarily route this input as Shell so
        // previewing a command cannot be reclassified as an agent request.
        self.cmd_hist_previous_input_config = Some(self.input_config);
        self.input_config = InputConfig {
            input_type: DetectedInputType::Shell,
            is_locked: true,
        };
        self.sync_input_placeholder(ctx);
        self.cmd_hist_saved = buffer;
        self.cmd_hist_items = items;
        self.cmd_hist_selected = 0; // newest
        self.cmd_hist_rich = false;
        self.cmd_hist_open = true;
        self.preview_cmd_history(ctx);
        ctx.notify();
    }

    /// Write the selected command into the input as a live preview. Targets the
    /// rich AI input (as `!cmd`) or the main command editor per `cmd_hist_rich`.
    pub(crate) fn preview_cmd_history(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some((cmd, _)) = self.cmd_hist_items.get(self.cmd_hist_selected) {
            let cmd = cmd.clone();
            if self.cmd_hist_rich {
                let text = format!("!{cmd}");
                ctx.update_view(&self.rich_input_editor, |e, ectx| {
                    e.set_text(&text, ectx);
                    e.set_ghost_suffix(None, ectx);
                });
            } else {
                ctx.update_view(&self.command_editor, |e, ectx| {
                    e.set_text(&cmd, ectx);
                    e.set_ghost_suffix(None, ectx);
                });
            }
        }
    }

    /// Move the selection: `older` (↑) toward the top of the list, else (↓)
    /// toward the newest. Items are newest-first, so older = higher index.
    /// Pressing ↓ at the newest command steps "below" it — back to an empty
    /// prompt — which closes the popover (matching Warp).
    pub(crate) fn cmd_history_navigate(&mut self, older: bool, ctx: &mut ViewContext<Self>) {
        if self.cmd_hist_items.is_empty() {
            self.close_cmd_history(true, ctx);
            return;
        }
        if older {
            let last = self.cmd_hist_items.len() - 1;
            self.cmd_hist_selected = (self.cmd_hist_selected + 1).min(last);
            self.preview_cmd_history(ctx);
        } else if self.cmd_hist_selected == 0 {
            // Already at the newest → step back to the "empty" prompt and close.
            // For the AI `!` popover that empty prompt is a bare `!`.
            if self.cmd_hist_rich {
                ctx.update_view(&self.rich_input_editor, |e, ectx| {
                    e.set_text("!", ectx);
                    e.set_ghost_suffix(None, ectx);
                });
            } else {
                ctx.update_view(&self.command_editor, |e, ectx| {
                    e.set_text("", ectx);
                    e.set_ghost_suffix(None, ectx);
                });
            }
            self.cmd_hist_open = false;
            self.cmd_hist_items.clear();
            self.cmd_hist_saved.clear();
            if !self.cmd_hist_rich {
                self.restore_history_input_config(ctx);
            }
        } else {
            self.cmd_hist_selected -= 1;
            self.preview_cmd_history(ctx);
        }
        ctx.notify();
    }

    /// Close the popover. `restore` puts back the buffer the user was typing
    /// (Esc); otherwise the previewed command stays (Enter/click, about to run).
    pub(crate) fn close_cmd_history(&mut self, restore: bool, ctx: &mut ViewContext<Self>) {
        if !self.cmd_hist_open {
            return;
        }
        self.cmd_hist_open = false;
        if restore {
            let saved = self.cmd_hist_saved.clone();
            if self.cmd_hist_rich {
                ctx.update_view(&self.rich_input_editor, |e, ectx| {
                    e.set_text(&saved, ectx);
                    e.set_ghost_suffix(None, ectx);
                });
            } else {
                ctx.update_view(&self.command_editor, |e, ectx| {
                    e.set_text(&saved, ectx);
                    e.set_ghost_suffix(None, ectx);
                });
            }
        }
        if restore {
            self.restore_history_input_config(ctx);
        }
        self.cmd_hist_items.clear();
        self.cmd_hist_saved.clear();
        ctx.notify();
    }

    pub(crate) fn restore_history_input_config(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(previous) = self.cmd_hist_previous_input_config.take() {
            self.input_config = previous;
            self.sync_input_placeholder(ctx);
        }
    }

    /// Open the scratch-shell (`!`) history popover from the AI input. Sources
    /// this session's `!` commands (newest first). Returns false when there's
    /// nothing to browse, so the caller can fall through to normal ↑ behaviour.
    pub(crate) fn open_side_cmd_history(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if self.side_cmd_history.is_empty() {
            return false;
        }
        let buffer = self
            .rich_input_editor
            .read(ctx, |e, _| e.get_text().to_string());
        // Everything after the leading `!`, used as a prefix filter.
        let prefix = buffer
            .trim_start()
            .strip_prefix('!')
            .unwrap_or("")
            .trim()
            .to_string();
        let is_noise = |c: &str| matches!(c.trim(), "clear" | "cls");

        let now = chrono::Utc::now().timestamp_millis();
        let mut seen = std::collections::HashSet::new();
        let mut items: Vec<(String, i64)> = Vec::new();
        for cmd in self.side_cmd_history.iter().rev() {
            if is_noise(cmd) || !seen.insert(cmd.clone()) {
                continue;
            }
            if !prefix.is_empty() && !cmd.starts_with(&prefix) {
                continue;
            }
            items.push((cmd.clone(), now));
        }
        if items.is_empty() {
            return false;
        }

        self.cmd_hist_saved = buffer;
        self.cmd_hist_items = items;
        self.cmd_hist_selected = 0; // newest
        self.cmd_hist_rich = true;
        self.cmd_hist_open = true;
        self.preview_cmd_history(ctx);
        ctx.notify();
        true
    }

    /// Warp-style command-history popover: a resizable list of recent commands
    /// with relative timestamps, pinned above the input. Oldest at top, newest
    /// at bottom; the selected row (previewed in the input) is highlighted.
    pub(crate) fn render_cmd_history_popover(&self) -> Option<Box<dyn Element>> {
        if !self.cmd_hist_open || self.cmd_hist_items.is_empty() {
            return None;
        }
        let theme = self.theme;
        let mono = self.mono_family;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let n = self.cmd_hist_items.len();
        let v = self.cmd_hist_visible_count();
        // Window around the selection; selected rides the top edge when browsing
        // into older commands (items are newest-first, so older = higher index).
        let start = self.cmd_hist_selected.saturating_sub(v.saturating_sub(1));
        let end = (start + v).min(n);

        // ── Drag grip (a grab-bar at the top; drag it to resize the popover) ──
        // A cursor-only Hoverable shows the ⇕ resize cursor on hover (the
        // affordance the sidebar has). It has no click/down handler, so it
        // passes the mouse-down straight through to the DragResizeElement that
        // wraps the whole band — which is what actually begins the resize.
        let grip_state = self
            .entry_mouse_states
            .borrow_mut()
            .entry("cmd-hist-grip".to_string())
            .or_default()
            .clone();
        let grip = Hoverable::new(grip_state, move |_st| {
            Container::new(
                Align::new(
                    Container::new(
                        ConstrainedBox::new(warpui::elements::Empty::new().finish())
                            .with_width(36.0)
                            .with_height(4.0)
                            .finish(),
                    )
                    .with_background_color(theme.outline())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                    .finish(),
                )
                .finish(),
            )
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
        })
        .with_cursor(Cursor::ResizeUpDown)
        .finish();

        // ── Header: HISTORY label ─────────────────────────────────────────────
        let header = Container::new(
            Text::new("HISTORY".to_string(), mono, crate::theme::text_size(12.5))
                .with_color(theme.sub_text())
                .with_style(warpui::fonts::Properties {
                    weight: warpui::fonts::Weight::Bold,
                    ..Default::default()
                })
                .finish(),
        )
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(2.0)
        .with_padding_bottom(7.0)
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish();

        // ── Rows: oldest (top) → newest (bottom) ──────────────────────────────
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);
        for idx in (start..end).rev() {
            let (cmd, ts) = &self.cmd_hist_items[idx];
            let selected = idx == self.cmd_hist_selected;
            let secs = ((now_ms - *ts).max(0) / 1000) as u64;
            let ago = history_relative_time(secs);
            let (fg, icon_fg) = if selected {
                (theme.main_text(), theme.accent)
            } else {
                (theme.sub_text(), theme.disabled_text())
            };

            let row_inner = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(crate::icons::render_icon(
                    crate::icons::AppIcon::Terminal,
                    icon_fg,
                    crate::icons::ICON_SIZE_SM,
                ))
                .with_child(Box::new(Expanded::new(
                    1.0,
                    // Single-line + ellipsis: long commands otherwise soft-wrap
                    // (Text::new defaults to wrapping) and the taller rows overlap
                    // each other and the input bar below.
                    Text::new(cmd.clone(), mono, crate::theme::text_size(12.5))
                        .with_color(fg)
                        .with_style(warpui::fonts::Properties {
                            weight: warpui::fonts::Weight::Bold,
                            ..Default::default()
                        })
                        .soft_wrap(false)
                        .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                        .finish(),
                )))
                .with_child(
                    Text::new(ago, mono, crate::theme::text_size(11.0))
                        .with_color(theme.sub_text())
                        .with_style(warpui::fonts::Properties {
                            weight: warpui::fonts::Weight::Bold,
                            ..Default::default()
                        })
                        .finish(),
                )
                .finish();
            let mut row = Container::new(row_inner)
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0);
            if selected {
                row = row
                    .with_background_color(theme.surface_3())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)));
            }
            let row = EventHandler::new(
                Container::new(row.finish())
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::CmdHistoryClick { index: idx });
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }
        let list = Container::new(list.finish())
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish();

        // ── Footer hint ───────────────────────────────────────────────────────
        let footer_pill = |label: &str| -> Box<dyn Element> {
            Container::new(
                Text::new(label.to_string(), mono, crate::theme::text_size(10.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .with_background_color(theme.surface_3())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        };
        let footer = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(footer_pill("\u{2191}"))
                .with_child(footer_pill("\u{2193}"))
                .with_child(
                    Text::new(
                        "to navigate".to_string(),
                        mono,
                        crate::theme::text_size(10.5),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_width(8.0)
                        .finish(),
                )
                .with_child(footer_pill("esc"))
                .with_child(
                    Text::new(
                        "to dismiss".to_string(),
                        mono,
                        crate::theme::text_size(10.5),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .finish(),
        )
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish();

        // CONTENT-DRIVEN height — the same shape as the agent activity strip that
        // already sits correctly above the input (`render_agent_activity_strip`):
        // `MainAxisSize::Min`, no forced height, sized by its children.
        //
        // It used to be a fixed-height `ConstrainedBox` whose row count was
        // PREDICTED from that pixel height. Any error in the prediction (the row
        // and chrome estimates were both low) meant the rows didn't fit, so the
        // footer spilled out of the band and landed on the input bar. Sizing to
        // content makes that impossible — the band is exactly as tall as the rows
        // it draws — and removes the height→rows→height feedback that made the
        // layout flicker. `cmd_hist_height_px` now only expresses the user's
        // desired size (how many rows to show), never a hard box.
        let inner = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(grip)
            .with_child(header)
            .with_child(list)
            .with_child(footer)
            .finish();

        // Full-width band flush to both edges — no rounding, no side margins.
        // Top + bottom hairlines separate it from the output above and the input
        // below.
        // Opaque fill in the pane's own colour — same hue as the input bar, so
        // the two read as one stack, but never see-through: the band floats over
        // the scroll area and a translucent fill lets output slide visibly
        // beneath the history rows.
        //
        // NOTE: this deliberately does NOT paint the theme's background image.
        // Doing so needs an Image in a Stack behind the rows, and the band is
        // content-sized inside a `MainAxisSize::Min` column — so the Stack hands
        // the Image an INFINITE height constraint, `cover()` resolves to an
        // invalid rect, and the renderer panics (`!rect.origin().y().is_infinite()`).
        // Showing the image here would mean giving the band a bounded height
        // again, which is exactly what made the footer spill onto the input.
        let bordered = Container::new(inner)
            .with_border(
                Border::new(1.0)
                    .with_sides(true, false, true, false)
                    .with_border_color(theme.outline()),
            )
            .finish();
        // Tint over the band's own image copy, matching `workspace_fill`: the
        // workspace paints opaque base → image → a translucent `theme.background`
        // fill, and ALL pane content sits on top of that fill. The band repaints
        // the image itself, so without this it skipped the fill and read far more
        // vivid than the surface around it. Same colour, so the two match; on a
        // solid theme it is opaque and simply is the pane colour.
        let tinted = Container::new(bordered)
            .with_background_color(theme.background)
            .finish();
        // Under a background-image theme the band shows the REAL backdrop rather
        // than a flat fill: an opaque base (so scrolling output beneath is fully
        // hidden) + the image at the theme's designed opacity + the rows on top.
        //
        // The image MUST be given an explicit height. It is `cover()`, so under a
        // loose constraint it expands to the constraint's max — previously that
        // max was infinite and the renderer aborted on an invalid rect; here it
        // would simply balloon the band. The height is derived from the SAME row
        // count that built the rows, so the two cannot disagree. It stays an
        // estimate (padding is fixed while text scales with the font setting), so
        // a few px of mismatch just reveals the opaque base behind it.
        let band: Box<dyn Element> = match crate::theme::Theme::bg_image_for(theme.id) {
            Some(bg) => {
                let rows_shown = end.saturating_sub(start) as f32;
                let band_h = CMD_HIST_CHROME_PX + rows_shown * CMD_HIST_ROW_PX;
                // `Original`: `BySize` resizes the multi-MB source on the UI
                // thread the first time, which stalls the app.
                // CLIPPED: `cover()` scales the image to COVER its box, so it is
                // deliberately larger than that box on one axis. Unclipped it
                // painted the overflow outside the band — spilling over the sides
                // and above the top, and growing further as the band grew.
                // Clipping bounds it to exactly the band's rectangle.
                let image = warpui_core::elements::Clipped::new(
                    ConstrainedBox::new(
                        warpui::elements::Image::new(
                            bg.asset_source(),
                            warpui::elements::CacheOption::Original,
                        )
                        .cover()
                        .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                        .finish(),
                    )
                    .with_height(band_h)
                    .finish(),
                )
                .finish();
                Container::new(Box::new(Stack::new().with_child(image).with_child(tinted)))
                    .with_background_color(theme.solid_background())
                    .finish()
            }
            None => Container::new(tinted)
                .with_background_color(theme.solid_background())
                .finish(),
        };
        // DragResizeElement wraps the whole band: pressing anywhere the child
        // rows didn't claim begins a resize, and it captures the drag/up via raw
        // events so motion is smooth and position-independent (drift off the grip
        // is fine). It resizes in deltas — dragging UP grows the popover.
        Some(Box::new(warpui_core::elements::DragResizeElement::new(
            self.cmd_hist_drag.clone(),
            band,
            |dy, event_ctx, _app| {
                event_ctx.dispatch_typed_action(TerminalAction::CmdHistResizeBy { dy });
            },
            None,
        )))
    }

    pub(crate) fn accept_history_search(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.history_search.open {
            return;
        }
        let command = self
            .history_search
            .selected_command()
            .map(str::to_string)
            .or_else(|| {
                if self.history_search.query.is_empty() {
                    None
                } else {
                    Some(self.history_search.query.clone())
                }
            });
        self.history_search.open = false;
        self.history_search.query.clear();
        self.history_search.results.clear();
        self.history_search.selected = 0;
        self.history_search.saved_buffer.clear();

        if let Some(command) = command {
            ctx.update_view(&self.command_editor, |editor, ectx| {
                editor.set_text(&command, ectx);
                editor.set_ghost_suffix(None, ectx);
            });
            self.command_history_idx = None;
        } else {
            self.close_history_search(ctx, true);
            return;
        }
        ctx.notify();
    }

    pub(crate) fn history_search_append(&mut self, ch: &str, ctx: &mut ViewContext<Self>) {
        if !self.history_search.open {
            return;
        }
        self.history_search.query.push_str(ch);
        self.history_search.selected = 0;
        self.history_search.refresh(&self.command_history);
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&self.history_search.query, ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn history_search_backspace(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.history_search.open {
            return;
        }
        if self.history_search.query.is_empty() {
            self.close_history_search(ctx, true);
            return;
        }
        self.history_search.query.pop();
        self.history_search.selected = 0;
        self.history_search.refresh(&self.command_history);
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&self.history_search.query, ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn open_workflow_search(&mut self, ctx: &mut ViewContext<Self>) {
        let buffer = self
            .command_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        self.workflow_search.open = true;
        self.workflow_search.saved_buffer = buffer;
        self.workflow_search.query.clear();
        self.workflow_search.selected = 0;
        self.workflow_search.refresh(&self.workflow_store);
        self.close_history_search(ctx, false);
        ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text("", ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        self.focus_command_editor(ctx);
        ctx.notify();
    }

    pub(crate) fn close_workflow_search(
        &mut self,
        ctx: &mut ViewContext<Self>,
        restore_buffer: bool,
    ) {
        if !self.workflow_search.open {
            return;
        }
        let saved = self.workflow_search.saved_buffer.clone();
        self.workflow_search.open = false;
        self.workflow_search.query.clear();
        self.workflow_search.results.clear();
        self.workflow_search.selected = 0;
        if restore_buffer {
            ctx.update_view(&self.command_editor, |editor, ectx| {
                editor.set_text(&saved, ectx);
                editor.set_ghost_suffix(None, ectx);
            });
        }
        ctx.notify();
    }

    pub(crate) fn accept_workflow_search(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.workflow_search.open {
            return;
        }
        let Some(workflow) = self.workflow_search.selected_workflow().cloned() else {
            self.close_workflow_search(ctx, true);
            return;
        };
        self.workflow_search.open = false;
        self.workflow_search.query.clear();
        self.workflow_search.results.clear();
        self.workflow_search.selected = 0;
        self.workflow_search.saved_buffer.clear();
        self.start_workflow_session(workflow, ctx);
    }

    pub(crate) fn start_workflow_session(
        &mut self,
        workflow: Workflow,
        ctx: &mut ViewContext<Self>,
    ) {
        let session = WorkflowSession::new(workflow);
        let text = session.rendered.clone();
        self.workflow_session = Some(session);
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&text, ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        self.sync_workflow_highlight(ctx);
        self.refresh_input_intel(&text, text.chars().count(), ctx);
        ctx.notify();
    }

    pub(crate) fn sync_workflow_highlight(&mut self, ctx: &mut ViewContext<Self>) {
        let highlight = self
            .workflow_session
            .as_ref()
            .and_then(|s| s.active_range());
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_workflow_arg_highlight(highlight, ectx);
        });
    }

    pub(crate) fn cycle_workflow_arg(&mut self, previous: bool, ctx: &mut ViewContext<Self>) {
        let Some(session) = self.workflow_session.as_mut() else {
            return;
        };
        if previous {
            session.cycle_prev();
        } else {
            session.cycle_next();
        }
        if let Some(range) = session.active_range() {
            let start_char = session.rendered[..range.start].chars().count();
            ctx.update_view(&self.command_editor, |editor, ectx| {
                editor.set_cursor_char_index(start_char, ectx);
            });
        }
        self.sync_workflow_highlight(ctx);
        ctx.notify();
    }

    pub(crate) fn toggle_vim_mode(&mut self, ctx: &mut ViewContext<Self>) {
        let enabled = self.editor_settings.toggle_vim_mode();
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_vim_enabled(enabled, ectx);
        });
        ctx.notify();
    }

    /// Apply persisted editor settings (e.g. after changing them in Settings).
    pub fn sync_editor_settings(&mut self, ctx: &mut ViewContext<Self>) {
        let settings = EditorSettings::load();
        let vim_enabled = settings.vim_mode_enabled;
        let vim_changed = self.editor_settings.vim_mode_enabled != vim_enabled;
        // The code font role already draws the new family and weight; the
        // grid still has to re-measure its cells.
        let typography_changed = self.editor_settings.terminal.font_family
            != settings.terminal.font_family
            || self.editor_settings.editor.font_weight != settings.editor.font_weight
            || self.editor_settings.terminal.font_size != settings.terminal.font_size
            || self.editor_settings.terminal.line_height_pct != settings.terminal.line_height_pct;
        self.editor_settings = settings;
        if vim_changed {
            ctx.update_view(&self.command_editor, |editor, ectx| {
                editor.set_vim_enabled(vim_enabled, ectx);
            });
        }
        if typography_changed {
            // Publish new metrics for every render path, recompute cell
            // geometry, and force the next layout pass to re-derive
            // cols/rows and resize the PTY.
            crate::theme::set_terminal_typography(
                self.editor_settings.terminal.clamped_font_size(),
                self.editor_settings.terminal.line_height_ratio(),
            );
            self.cell_metrics = compute_cell_metrics(ctx, self.mono_family);
            log::info!(
                "[settings] typography applied — size {} ratio {} → cell {:.3}x{:.3}",
                self.editor_settings.terminal.clamped_font_size(),
                self.editor_settings.terminal.line_height_ratio(),
                self.cell_metrics.width,
                self.cell_metrics.height,
            );
            self.last_dims = None;
            self.rebuild_height_index_preserving_scroll(ctx);

            // Content-plane editors follow the typography too: command input,
            // agent prompt input, and the feedback field.
            let family = self.mono_family;
            let size = crate::theme::scaled_mono(crate::theme::INPUT_FONT_SIZE);
            for editor in [
                &self.command_editor,
                &self.rich_input_editor,
                &self.feedback_editor,
                &self.question_editor,
            ] {
                ctx.update_view(editor, |e, ectx| {
                    e.set_font(family, size, ectx);
                    e.set_menu_font(family, ectx);
                });
            }
        }
        ctx.notify();
    }

    pub(crate) fn workflow_search_append(&mut self, ch: &str, ctx: &mut ViewContext<Self>) {
        if !self.workflow_search.open {
            return;
        }
        self.workflow_search.query.push_str(ch);
        self.workflow_search.selected = 0;
        self.workflow_search.refresh(&self.workflow_store);
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&self.workflow_search.query, ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn workflow_search_backspace(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.workflow_search.open {
            return;
        }
        if self.workflow_search.query.is_empty() {
            self.close_workflow_search(ctx, true);
            return;
        }
        self.workflow_search.query.pop();
        self.workflow_search.selected = 0;
        self.workflow_search.refresh(&self.workflow_store);
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&self.workflow_search.query, ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn refresh_command_ghost(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        let completion_open = self.path_picker.read(ctx, |m, _| m.open);
        // A prompt the user chose to send to the agent is prose: only slash
        // commands complete there, never shell commands or file names.
        let agent_prose = self.input_config.is_locked
            && self.input_config.input_type == DetectedInputType::Agent
            && !text.trim_start().starts_with('/');
        let ghost = if completion_open
            || agent_prose
            || self.history_search.open
            || self.workflow_search.open
        {
            None
        } else {
            let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
            self.suggest_engine.on_cwd_changed(&cwd);
            self.suggest_engine
                .suggest(text, &self.command_history, &cwd)
        };
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_ghost_suffix(ghost, ectx);
        });
    }

    pub(crate) fn accept_command_ghost(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.accept_ghost_suffix(ectx);
        });
    }

    pub(crate) fn write_command(&self, command: &str, mode: TermMode) {
        self.write_command_after(None, command, mode);
    }

    /// [`write_command`](Self::write_command), with a shell statement `setup`
    /// on the same line before it (the pop-up terminal's system `BROWSER`).
    /// It goes in front of the tracked command, so an agent launched this way
    /// is still recognised.
    pub(crate) fn write_command_after(&self, setup: Option<&str>, command: &str, mode: TermMode) {
        // Built-in agents are known to the launch helper; a custom agent pane
        // declares its recovery descriptor first so its conversation is tracked.
        #[cfg(not(target_family = "wasm"))]
        if self
            .agent_id
            .as_deref()
            .is_some_and(crate::workspace::is_custom_id)
        {
            crate::workspace::stage_custom_recovery(
                self.pane_uuid,
                command,
                &crate::setup::UiSettings::load(),
            );
        }
        // Each machine owns the launch monitor and conversation records.
        #[cfg(not(target_family = "wasm"))]
        let tracked_command = if self.session_core.lock().remote_machine.is_some() {
            dragent_host::agent_sessions::wrap_remote_command(
                self.pane_uuid,
                self.launch_shell.as_deref().unwrap_or("sh"),
                command,
            )
        } else {
            dragent_host::agent_sessions::wrap_command(
                self.pane_uuid,
                self.launch_shell.as_deref().unwrap_or("sh"),
                command,
            )
        };
        #[cfg(not(target_family = "wasm"))]
        let command = tracked_command.as_str();
        let with_setup;
        let command = match setup {
            Some(setup) => {
                with_setup = format!("{setup}{command}");
                with_setup.as_str()
            }
            None => command,
        };
        let submit = SessionPlatform::detect().command_submit_bytes();
        // Assemble the full submission into one buffer (single PTY write).
        let mut buf: Vec<u8> = Vec::with_capacity(command.len() + 16);
        // Windows ConPTY: conhost can swallow the first input byte when a write
        // races the shell's console-mode transitions (e.g. readline starting up
        // in a fresh tab) — the classic "ls arrives as s". Prefix a sacrificial
        // Ctrl-U (kill-line in readline/zle/PSReadLine; a no-op on an empty
        // prompt) so a swallowed byte is the Ctrl-U, never the command's first
        // character. It also clears any stray characters already sitting in the
        // shell's line editor before our command lands.
        #[cfg(windows)]
        buf.push(0x15);
        // Deliver the WHOLE command as a single BRACKETED PASTE (ESC[200~…ESC[201~)
        // when the shell has bracketed-paste mode on — which interactive shells do
        // at a prompt. Embedded newlines then arrive as DATA (one command to
        // parse) instead of line-by-line submits that split a multi-line command.
        // `encode_paste` falls back to raw (newlines normalized) when the shell has
        // bracketed paste off (e.g. a full-screen app owns the screen), preserving
        // the old behavior there.
        buf.extend_from_slice(&crate::terminal::input::paste::encode_paste(command, mode));
        buf.extend_from_slice(submit); // trailing CR executes the pasted command
        self.manager.writer().write(&buf);
    }

    /// Write a shared controller's raw input straight to this pane's local PTY.
    ///
    /// Ctrl+C needs both the terminal byte and a direct SIGINT to the foreground
    /// process group. The byte alone works for ordinary cooked-mode commands,
    /// but raw-mode programs disable the terminal's `ISIG` handling and would
    /// otherwise ignore a remote controller's interrupt.
    pub fn write_pty_bytes(&self, bytes: &[u8]) {
        self.manager.writer().write(bytes);
        if bytes.contains(&0x03) && !self.manager.writer().is_remote() {
            self.manager.writer().interrupt();
        }
    }

    /// Host: a controlling viewer asked the shared PTY to be `rows`×`cols`. Resize
    /// the real PTY (SIGWINCH) so a TUI re-renders to fill the viewer's window,
    /// and remember it so the host's own layout stops overriding the size.
    pub fn apply_remote_resize(&mut self, rows: u16, cols: u16, ctx: &mut ViewContext<Self>) {
        if self.is_remote || rows == 0 || cols == 0 {
            return;
        }
        if self.remote_requested_dims == Some((rows, cols)) {
            return;
        }
        self.remote_requested_dims = Some((rows, cols));
        let cw = self.cell_metrics.width.max(1.0);
        let ch = self.cell_metrics.height.max(1.0);
        let size = SizeInfo {
            rows,
            cols,
            cell_width: cw,
            cell_height: ch,
            viewport_width_px: cols as f32 * cw,
            viewport_height_px: rows as f32 * ch,
        };
        let update = SizeUpdate::new(size, SizeReason::AltScreenTransition);
        let writer = self.manager.writer().clone();
        self.model.update(ctx, |m, mctx| {
            m.apply_size_update(update, |r, c| writer.resize(r, c), mctx);
        });
        self.last_dims = Some((rows, cols));
        ctx.notify();
    }

    /// Host: sharing/control ended — resume sizing the PTY from the host's own
    /// window on the next layout pass.
    pub fn clear_remote_resize(&mut self, ctx: &mut ViewContext<Self>) {
        if self.remote_requested_dims.take().is_some() {
            self.last_dims = None; // force the next layout to re-apply window size
            ctx.notify();
        }
    }

    /// The active emulator's terminal mode (for bracketed-paste detection).
    pub(crate) fn active_term_mode(&self, ctx: &ViewContext<Self>) -> TermMode {
        self.model.read(ctx, |m, _| {
            m.with_core(|core| core.active_emulator().mode())
        })
    }

    pub(crate) fn apply_recalled_command(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(text, ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        self.refresh_input_intel(text, text.chars().count(), ctx);
        ctx.notify();
    }

    /// Recall the previous/next command from history into the AI rich input
    /// (Shift+Up/Down), bash-style: first ↑ = most recent, ↑ steps older (wraps
    /// at the oldest), ↓ steps newer and clears to empty past the newest. Never
    /// opens the history popover; independent of the command editor's recall.
    /// Shift+↑/↓ in the AI input recalls the `!cmd` MINI-SHELL history
    /// (`side_cmd_history`) — NOT the main terminal history — so the user cycles
    /// through the scratch-shell commands they ran from this panel.
    pub(crate) fn recall_rich_history(&mut self, previous: bool, ctx: &mut ViewContext<Self>) {
        if self.side_cmd_history.is_empty() {
            return;
        }
        let len = self.side_cmd_history.len();
        let new_idx = match (self.rich_input_history_idx, previous) {
            (None, true) => Some(len - 1),  // first ↑ → most recent
            (Some(0), true) => Some(0),     // already at oldest
            (Some(i), true) => Some(i - 1), // older
            (Some(i), false) if i + 1 < len => Some(i + 1), // newer
            (Some(_), false) => None,       // past newest → blank
            (None, false) => return,        // ↓ with nothing recalled
        };
        self.rich_input_history_idx = new_idx;
        // Recalled mini-shell commands keep their `!` prefix so re-submitting
        // runs them in the scratch shell again (side_cmd_history stores the bare
        // command without the prefix).
        let text = new_idx
            .map(|i| format!("!{}", self.side_cmd_history[i]))
            .unwrap_or_default();
        ctx.update_view(&self.rich_input_editor, |editor, ectx| {
            editor.set_text(&text, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn recall_command_history(&mut self, previous: bool, ctx: &mut ViewContext<Self>) {
        if self.history_search.open {
            if previous {
                self.handle_action(&TerminalAction::HistorySearchUp, ctx);
            } else {
                self.handle_action(&TerminalAction::HistorySearchDown, ctx);
            }
            return;
        }
        let completion_open = self.path_picker.read(ctx, |m, _| m.open);
        if completion_open {
            if previous {
                ctx.update_model(&self.path_picker, |m, model_ctx| {
                    m.select_prev(model_ctx);
                });
            } else {
                ctx.update_model(&self.path_picker, |m, model_ctx| {
                    m.select_next(model_ctx);
                });
            }
            ctx.notify();
            return;
        }
        // ── Command-history popover (Warp-style) ──────────────────────────────
        // ↑ opens the resizable recent-commands list; once open, ↑/↓ browse it.
        if self.cmd_hist_open {
            self.cmd_history_navigate(previous, ctx);
            return;
        }
        if previous {
            self.open_cmd_history(ctx);
            return;
        }
        if self.command_history.is_empty() {
            return;
        }

        let buffer = self
            .command_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let trimmed = buffer.trim();

        // Empty line = full-history mode; reset prefix state.
        if trimmed.is_empty() {
            self.history_recall_prefix = None;
            self.command_history_idx = None;
            if !previous {
                return;
            }
        } else if self.history_recall_prefix.is_some() {
            // Prefix-filtered mode (user typed e.g. "git" then started ↑).
            let prefix = self.history_recall_prefix.clone().unwrap_or_default();
            if let Some((idx, text)) = prefixed_history::step_prefixed_history(
                &self.command_history,
                &prefix,
                self.command_history_idx,
                previous,
            ) {
                self.command_history_idx = Some(idx);
                self.apply_recalled_command(&text, ctx);
                return;
            }
            if !previous {
                self.command_history_idx = None;
                self.apply_recalled_command(&prefix, ctx);
                return;
            }
            // No older prefix matches.
            return;
        } else if self.command_history_idx.is_none() && previous {
            // First ↑ from a typed buffer — try prefix filter before full history.
            if let Some((idx, text)) =
                prefixed_history::step_prefixed_history(&self.command_history, trimmed, None, true)
            {
                self.history_recall_prefix = Some(buffer.clone());
                self.command_history_idx = Some(idx);
                self.apply_recalled_command(&text, ctx);
                return;
            }
            // No prefix matches — fall through to full history.
        }

        let next = if previous {
            match self.command_history_idx {
                None => self.command_history.len() - 1,
                Some(idx) => idx.saturating_sub(1),
            }
        } else {
            match self.command_history_idx {
                Some(idx) if idx + 1 < self.command_history.len() => idx + 1,
                Some(_) => {
                    self.command_history_idx = None;
                    self.history_recall_prefix = None;
                    ctx.update_view(&self.command_editor, |editor, ectx| {
                        editor.set_text("", ectx);
                        editor.set_ghost_suffix(None, ectx);
                    });
                    ctx.notify();
                    return;
                }
                None => return,
            }
        };
        self.command_history_idx = Some(next);
        let text = self.command_history[next].clone();
        self.apply_recalled_command(&text, ctx);
    }

    pub(crate) fn shell_prompt_lines_for_input<C: ReadModel>(
        &self,
        ctx: &C,
    ) -> Vec<crate::console::escape_parse::ColoredLine> {
        self.model.read(ctx, |m, _| {
            if m.is_command_running() || m.with_core(|c| c.integration_active()) {
                return Vec::new();
            }
            m.with_core(|c| {
                let emu = c.emulator();
                let (cursor_row, _) = emu.cursor_position();
                let lines = emu.grid().screen_styled_lines();
                if lines.is_empty() {
                    return Vec::new();
                }
                let end = cursor_row.saturating_add(1).min(lines.len());
                let start = end.saturating_sub(1);
                lines[start..end]
                    .iter()
                    .map(|line| styled_to_colored(line, &self.theme))
                    .collect()
            })
        })
    }
}
