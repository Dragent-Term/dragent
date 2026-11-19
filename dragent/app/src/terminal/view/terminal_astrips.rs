//! Terminal agent and self-heal strips.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    BranchDiffOp, TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT,
    CMD_HIST_CHROME_PX, CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y,
    SCROLL_BOTTOM_EPSILON_PX,
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
    /// Live agent-command strip above the input: one row per running command
    /// (foreground or backgrounded) with spinner, elapsed time, newest output
    /// line, and a Cancel button. Nothing renders when nothing is running —
    /// background machinery that is behaving stays out of the way, but a
    /// command the agent is waiting on is never invisible.
    pub(crate) fn render_agent_activity_strip(&self) -> Option<Box<dyn Element>> {
        let runs = crate::agent::local_tools::running_agent_commands_for(self.pane_uuid);
        if runs.is_empty() {
            return None;
        }
        let theme = self.theme;
        let mono = self.mono_family;
        // Circle frames, not braille: braille is absent from Fira Code, which is
        // the only font the web build embeds — the spinner was tofu there.
        const FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];

        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(2.0);

        for run in runs.iter().take(4) {
            let frame = FRAMES[(run.elapsed_secs as usize) % FRAMES.len()];
            let cmd = Self::command_display_name(&run.command);
            let mins = run.elapsed_secs / 60;
            let secs = run.elapsed_secs % 60;
            let elapsed = if mins > 0 {
                format!("{mins}m {secs:02}s")
            } else {
                format!("{secs}s")
            };
            let tag = if run.backgrounded {
                " · background"
            } else {
                ""
            };

            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(
                    Text::new(
                        format!("{frame} agent running `{cmd}` — {elapsed}{tag}"),
                        mono,
                        crate::theme::text_size(11.5),
                    )
                    .with_color(theme.accent)
                    .finish(),
                );
            // No output/last-line: it carries file paths and arbitrary text
            // that distort the strip (long paths once wrapped into a 1-char
            // column). The strip only signals THAT a command is running.
            // Spacer pins the Cancel button to the far right edge.
            row = row.with_child(Box::new(warpui::elements::Expanded::new(
                1.0,
                warpui::elements::Empty::new().finish(),
            )));
            let cancel_id = run.id.clone();
            let cancel_state = self
                .entry_mouse_states
                .borrow_mut()
                .entry(format!("strip-cancel-{}", run.id))
                .or_default()
                .clone();
            let cancel_btn: Box<dyn Element> = Box::new(
                warpui_core::elements::Hoverable::new(cancel_state, move |state| {
                    let (color, border) = if state.is_hovered() {
                        (theme.ui_red(), theme.ui_red())
                    } else {
                        (theme.sub_text(), theme.outline())
                    };
                    {
                        let icon = Container::new(
                            warpui::elements::Align::new(crate::icons::render_icon(
                                crate::icons::AppIcon::XClose,
                                color,
                                10.0,
                            ))
                            .finish(),
                        )
                        .with_border(warpui_core::scene::Border::all(1.0).with_border_color(border))
                        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                            warpui_core::scene::Radius::Pixels(5.0),
                        ))
                        .finish();
                        warpui::elements::ConstrainedBox::new(icon)
                            .with_width(18.0)
                            .with_height(18.0)
                            .finish()
                    }
                })
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::CancelAgentCommand {
                        task_id: cancel_id.clone(),
                    });
                }),
            );
            row = row.with_child(cancel_btn);
            col = col.with_child(row.finish());
        }
        if runs.len() > 4 {
            col = col.with_child(
                Text::new(
                    format!("… and {} more running", runs.len() - 4),
                    mono,
                    crate::theme::text_size(11.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            );
        }
        let body = col.finish();
        Some(
            Container::new(body)
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_background_color(theme.surface_1())
                .with_border(
                    warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                )
                .finish(),
        )
    }

    /// Terminal-style card for the persistent `!` scratch shell, pinned above the
    /// AI input. Streams live PTY output (the shell echoes each command and its
    /// output, exactly like a real terminal), with a Stop button that sends
    /// Ctrl+C to the foreground process, a ⟲ reset, and a × that only HIDES it
    /// (the shell and its output live on). Rendered only while `side_card_open`.
    pub(crate) fn render_side_command_card(&self) -> Option<Box<dyn Element>> {
        use std::sync::atomic::Ordering;
        if !self.side_card_open {
            return None;
        }
        let sh = self.side_shell.as_ref()?;
        let theme = self.theme;
        let mono = self.mono_family;

        // ── Header: title  ·  live dot  ·  Stop  ·  ⟲  ·  × ───────────────────
        let title = Text::new("Shell".to_string(), mono, crate::theme::text_size(12.0))
            .with_color(theme.main_text())
            .finish();

        // The PTY streams as things happen, so a persistent "live" dot is enough
        // — the reappearing prompt in the body already signals idle vs. busy.
        let _ = sh.activity.load(Ordering::Relaxed);
        let live_dot = Container::new(
            warpui::elements::ConstrainedBox::new(warpui::elements::Empty::new().finish())
                .with_width(6.0)
                .with_height(6.0)
                .finish(),
        )
        .with_background_color(warpui::color::ColorU::new(
            theme.accent.r,
            theme.accent.g,
            theme.accent.b,
            200,
        ))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .finish();

        // Small icon button used for ⟲ (reset) and × (close).
        let icon_btn = |this: &Self,
                        key: &str,
                        icon: crate::icons::AppIcon,
                        action: TerminalAction|
         -> Box<dyn Element> {
            let state = this
                .entry_mouse_states
                .borrow_mut()
                .entry(key.to_string())
                .or_default()
                .clone();
            Box::new(
                warpui_core::elements::Hoverable::new(state, move |st| {
                    let (color, border) = if st.is_hovered() {
                        (theme.main_text(), theme.main_text())
                    } else {
                        (theme.sub_text(), theme.outline())
                    };
                    let inner = Container::new(
                        warpui::elements::Align::new(crate::icons::render_icon(icon, color, 10.0))
                            .finish(),
                    )
                    .with_border(Border::all(1.0).with_border_color(border))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .finish();
                    warpui::elements::ConstrainedBox::new(inner)
                        .with_width(18.0)
                        .with_height(18.0)
                        .finish()
                })
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(action.clone());
                }),
            )
        };

        // A job is "running" when a foreground process (not the idle shell) owns
        // the terminal — accurate even for a SILENT process, so Stop no longer
        // flickers off during output gaps. On Windows (no process groups)
        // foreground_running() is always false, so fall back to recent output
        // activity there; the activity check also bridges the brief moment
        // between submitting a command and the child taking the foreground.
        let running = sh.handle.foreground_running()
            || self.side_activity_at.elapsed() < std::time::Duration::from_millis(1500);

        // Stop = Ctrl+C to the foreground process group. Text button so it reads
        // clearly (there's no stop glyph in the icon set). Built only while a
        // process is running.
        let stop_btn: Option<Box<dyn Element>> = if running {
            let stop_state = self
                .entry_mouse_states
                .borrow_mut()
                .entry("side-shell-stop".to_string())
                .or_default()
                .clone();
            Some(Box::new(
                warpui_core::elements::Hoverable::new(stop_state, move |st| {
                    let color = if st.is_hovered() {
                        theme.ui_red()
                    } else {
                        theme.sub_text()
                    };
                    let border = if st.is_hovered() {
                        theme.ui_red()
                    } else {
                        theme.outline()
                    };
                    Container::new(
                        Text::new("Stop".to_string(), mono, crate::theme::text_size(10.5))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .with_padding_top(1.0)
                    .with_padding_bottom(1.0)
                    .with_border(Border::all(1.0).with_border_color(border))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .finish()
                })
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::CancelSideCommand);
                }),
            ))
        } else {
            None
        };
        let reset_btn = icon_btn(
            self,
            "side-shell-reset",
            crate::icons::AppIcon::Undo2,
            TerminalAction::ResetSideShell,
        );
        let close_btn = icon_btn(
            self,
            "side-shell-close",
            crate::icons::AppIcon::XClose,
            TerminalAction::CloseSideCommand,
        );

        let mut header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(title)
            .with_child(live_dot)
            .with_child(Box::new(Expanded::new(
                1.0,
                warpui::elements::Empty::new().finish(),
            )));
        // Jump-to-bottom appears only while the user has scrolled up (follow off).
        if !self.side_follow_tail {
            header = header.with_child(icon_btn(
                self,
                "side-shell-jump",
                crate::icons::AppIcon::ArrowDown,
                TerminalAction::SideShellFollowTail { follow: true },
            ));
        }
        if let Some(stop_btn) = stop_btn {
            header = header.with_child(stop_btn);
        }
        let header = header.with_child(reset_btn).with_child(close_btn).finish();

        // ── Drag grip (resize the card's height) ──────────────────────────────
        let grip_state = self
            .entry_mouse_states
            .borrow_mut()
            .entry("side-shell-grip".to_string())
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
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish()
        })
        .with_cursor(Cursor::ResizeUpDown)
        .finish();

        // ── Body: full scrollback with ANSI COLORS, selectable, scrollable ──────
        let pane_view_id = self.pane_view_id;
        // Cap the displayed window: the whole thing is laid out every ~120ms
        // repaint, so rendering all 5000 scrollback lines as colored spans would
        // jank. 1000 lines is generous scrollback for a scratch shell.
        let colored = side_shell_colored_lines(&sh.emulator, &theme, 1000);
        let font_size = crate::theme::scaled_mono(12.0);
        let default_fg = theme.main_text();
        let text_el = if colored.is_empty() {
            Text::new(
                "Scratch shell — cd and env persist across `!` commands.".to_string(),
                mono,
                font_size,
            )
            .with_color(theme.sub_text())
            .with_selectable(true)
            .finish()
        } else {
            // One selectable Text with a colored span per emulator segment, so
            // git red/green, ls colors, etc. render like a real terminal.
            let mut text = Text::new(String::new(), mono, font_size);
            for (idx, line) in colored.iter().enumerate() {
                if idx > 0 {
                    text.add_text_with_highlights(
                        "\n".to_string(),
                        default_fg,
                        warpui_core::fonts::Properties::default(),
                    );
                }
                for (seg, color) in line {
                    text.add_text_with_highlights(
                        seg.clone(),
                        *color,
                        warpui_core::fonts::Properties::default(),
                    );
                }
            }
            text.with_selectable(true).finish()
        };
        let text_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(Box::new(Expanded::new(1.0, text_el)))
            .finish();
        // Drag over the text to select → auto-copy (matches the terminal).
        let selectable = SelectableArea::new(
            self.side_selectable.clone(),
            move |args: SelectionUpdateArgs, event_ctx, _app| {
                if let Some(text) = args.selection {
                    event_ctx
                        .dispatch_typed_action_for(pane_view_id, TerminalAction::CopyText { text });
                }
            },
            text_row,
        )
        .finish();
        let scrollable = ClippedScrollable::vertical(
            self.side_scroll.clone(),
            selectable,
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.main_text().into(),
            theme.surface_1().into(),
        )
        .finish();
        // Fixed (drag-resizable) height so the content scrolls within it.
        let scroll_box = ConstrainedBox::new(scrollable)
            .with_height(self.side_card_height_px)
            .finish();
        // A manual scroll-UP pauses tail-following (so streaming output doesn't
        // yank the view back down while the user reads history). The scrollable
        // itself still scrolls — this only records intent.
        let scroll_box = EventHandler::new(scroll_box)
            .with_always_handle()
            .on_scroll_wheel(|event_ctx, _app, _pos, delta, _precise, _mods| {
                if delta.y() > 0.0 {
                    event_ctx.dispatch_typed_action(TerminalAction::SideShellFollowTail {
                        follow: false,
                    });
                }
                DispatchEventResult::PropagateToParent
            })
            .finish();

        let col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(4.0)
            .with_child(grip)
            .with_child(header)
            .with_child(scroll_box)
            .finish();

        let card = Container::new(col)
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(2.0)
            .with_padding_bottom(8.0)
            .with_background_color(theme.surface_1())
            .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
            .finish();

        // DragResizeElement resizes the card height from the grip/header (the
        // selectable body claims its own drags, so a text selection there won't
        // start a resize). Dragging up grows it.
        Some(Box::new(warpui_core::elements::DragResizeElement::new(
            self.side_card_drag.clone(),
            card,
            |dy, event_ctx, _app| {
                event_ctx.dispatch_typed_action(TerminalAction::SideCardResizeBy { dy });
            },
            None,
        )))
    }

    /// Self-heal status strip above the input. While a failed command is being
    /// analyzed or auto-fixed, its status stays PINNED here — glued directly to
    /// the input like the agent activity strip — instead of scrolling away in
    /// the block stream. The operation is still running, so it must stay in
    /// view even as new commands push its origin block up and out of sight.
    /// One row per actively-working block. The inline analyzing/fixing cards in
    /// the block stream are suppressed while a block shows here (see
    /// block_list_element.rs) so it isn't displayed twice.
    pub(crate) fn render_self_heal_strip(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        use crate::agent::self_heal::BlockHealState;

        let states = self
            .self_heal_store
            .read(app, |store, _| store.all_states().clone());
        let mut active: Vec<(usize, String, bool)> = states
            .iter()
            .filter_map(|(id, st)| match st {
                BlockHealState::Analyzing => {
                    Some((*id, "analyzing failed command".to_string(), false))
                }
                BlockHealState::AgentRunning { round } => Some((
                    *id,
                    format!(
                        "fixing — step {} (diagnosing, editing, verifying)",
                        round + 1
                    ),
                    true,
                )),
                _ => None,
            })
            .collect();
        if active.is_empty() {
            return None;
        }
        active.sort_by_key(|(id, _, _)| *id);

        let theme = self.theme;
        let mono = self.mono_family;

        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(2.0);

        for (block_id, phase, is_fixing) in active.iter().take(4) {
            let accent = if *is_fixing {
                theme.accent
            } else {
                theme.ui_red()
            };
            let dot = Container::new(
                warpui::elements::ConstrainedBox::new(warpui::elements::Empty::new().finish())
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
            )
            .with_background_color(warpui::color::ColorU::new(
                accent.r, accent.g, accent.b, 160,
            ))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(3.0),
            ))
            .finish();

            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(dot)
                .with_child(
                    Text::new(
                        format!("Self-heal · {phase} #{block_id}…"),
                        mono,
                        crate::theme::text_size(11.5),
                    )
                    .with_color(accent)
                    .finish(),
                );
            // Spacer pins the Dismiss button to the far right edge.
            row = row.with_child(Box::new(warpui::elements::Expanded::new(
                1.0,
                warpui::elements::Empty::new().finish(),
            )));

            let bid = *block_id;
            let dismiss_state = self
                .entry_mouse_states
                .borrow_mut()
                .entry(format!("heal-strip-dismiss-{block_id}"))
                .or_default()
                .clone();
            let dismiss_btn: Box<dyn Element> = Box::new(
                warpui_core::elements::Hoverable::new(dismiss_state, move |state| {
                    let (color, border) = if state.is_hovered() {
                        (theme.ui_red(), theme.ui_red())
                    } else {
                        (theme.sub_text(), theme.outline())
                    };
                    let icon = Container::new(
                        warpui::elements::Align::new(crate::icons::render_icon(
                            crate::icons::AppIcon::XClose,
                            color,
                            10.0,
                        ))
                        .finish(),
                    )
                    .with_border(warpui_core::scene::Border::all(1.0).with_border_color(border))
                    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                        warpui_core::scene::Radius::Pixels(5.0),
                    ))
                    .finish();
                    warpui::elements::ConstrainedBox::new(icon)
                        .with_width(18.0)
                        .with_height(18.0)
                        .finish()
                })
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx
                        .dispatch_typed_action(TerminalAction::DismissBlockFix { block_id: bid });
                }),
            );
            row = row.with_child(dismiss_btn);
            col = col.with_child(row.finish());
        }
        if active.len() > 4 {
            col = col.with_child(
                Text::new(
                    format!("… and {} more", active.len() - 4),
                    mono,
                    crate::theme::text_size(11.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            );
        }

        Some(
            Container::new(col.finish())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_background_color(theme.surface_1())
                .with_border(
                    warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                )
                .finish(),
        )
    }
}
