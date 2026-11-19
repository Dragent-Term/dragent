//! Terminal search overlays and alt-screen.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    AltPanAxis, AltPanDrag, TerminalView, ALT_PAN_AXIS_THRESHOLD_PX, BLOCK_LIST_BOTTOM_GAP_PX,
    BLOCK_MENU_EST_HEIGHT, CMD_HIST_CHROME_PX, CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y,
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

/// What the cover over a phone-sized terminal says.
pub(crate) struct PhoneCover {
    pub title: String,
    pub detail: &'static str,
    /// The phone's grid, shown as a small chip ("40 × 20").
    pub size: String,
    pub button: &'static str,
}

pub(crate) fn phone_cover_copy(
    control: &crate::realtime::phone_control::Control,
) -> Option<PhoneCover> {
    use crate::realtime::phone_control::Control;
    match control {
        Control::Phone { phone_name, cols, rows } => Some(PhoneCover {
            title: format!("In use on {phone_name}"),
            detail: "This terminal is fitted to your phone's screen. Take it back to use it here at full size. Your phone returns to its list of tabs.",
            size: format!("{cols} \u{00d7} {rows}"),
            button: "Take back control",
        }),
        Control::Held { cols, rows } => Some(PhoneCover {
            title: "Still sized for your phone".to_string(),
            detail: "Your phone left this terminal at its own screen size. Restore it to use it here at full size.",
            size: format!("{cols} \u{00d7} {rows}"),
            button: "Restore full size",
        }),
        Control::Desktop => None,
    }
}

/// The cover over a browser tab a phone is using.
pub(crate) fn browser_cover_copy(phone_name: &str) -> PhoneCover {
    PhoneCover {
        title: format!("In use on {phone_name}"),
        detail: "This browser tab is being used from your phone. Take it back to use it here. Your phone returns to its list of tabs.",
        size: String::new(),
        button: "Take back control",
    }
}

/// The veil and dialog over a phone-sized terminal. The button dispatches
/// `take_back` to the pane it is drawn in (a terminal or a browser).
pub(crate) fn render_phone_cover(
    theme: Theme,
    ui: FamilyId,
    mono: FamilyId,
    cover: PhoneCover,
    hover: warpui_core::elements::MouseStateHandle,
    take_back: impl Fn(&mut warpui::EventContext) + 'static,
) -> Box<dyn Element> {
    use warpui::fonts::{Properties, Weight};
    use warpui_core::text_layout::TextAlignment;

    let tint = |alpha: u8| {
        let mut c = theme.accent;
        c.a = alpha;
        c
    };

    // A round accent badge with the phone in it.
    let badge = Container::new(crate::icons::render_icon(
        crate::icons::AppIcon::Smartphone,
        theme.accent,
        22.0,
    ))
    .with_uniform_padding(13.0)
    .with_background_color(tint(34))
    .with_border(Border::all(1.0).with_border_color(tint(70)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(24.0)))
    .finish();

    let title = Text::new(cover.title, ui, crate::theme::text_size(16.0))
        .with_color(theme.main_text())
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_alignment(TextAlignment::Center)
        .finish();
    let detail = Text::new(cover.detail.to_string(), ui, crate::theme::text_size(13.0))
        .with_color(theme.sub_text())
        .with_alignment(TextAlignment::Center)
        .finish();
    let has_size = !cover.size.is_empty();
    let size_chip = Container::new(
        Text::new(cover.size, mono, crate::theme::text_size(11.0))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_padding_left(9.0)
    .with_padding_right(9.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(11.0)))
    .finish();

    let button_label = cover.button;
    let button = Hoverable::new(hover, move |state| {
        let mut bg = theme.accent;
        if state.is_hovered() {
            bg.r = bg.r.saturating_add(16);
            bg.g = bg.g.saturating_add(16);
            bg.b = bg.b.saturating_add(16);
        }
        Container::new(
            Align::new(
                Text::new(button_label.to_string(), ui, crate::theme::text_size(13.0))
                    .with_color(theme.accent_text())
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish(),
            )
            .finish(),
        )
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(19.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| take_back(event_ctx))
    .finish();

    let dialog = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(badge)
            .with_child(Container::new(title).with_padding_top(16.0).finish())
            .with_child(Container::new(detail).with_padding_top(8.0).finish())
            .with_child(if has_size {
                Container::new(size_chip).with_padding_top(14.0).finish()
            } else {
                warpui::elements::Empty::new().finish()
            })
            .with_child(
                Container::new(ConstrainedBox::new(button).with_width(292.0).with_height(38.0).finish())
                    .with_padding_top(20.0)
                    .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(24.0)
    // Rounded parts inside use radii that fit their size: a pixel radius
    // larger than the shape draws no background at all.
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.border()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(16.0)))
    .finish();
    let dialog = EventHandler::new(ConstrainedBox::new(dialog).with_width(340.0).finish())
        .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
        .finish();

    // No content blur in the UI kit: a heavy veil in the pane's own
    // background stands in, leaving only a faint trace of the terminal.
    let mut veil_color = theme.background;
    veil_color.a = 226;
    let veil = EventHandler::new(
        Container::new(Align::new(dialog).finish())
            .with_background_color(veil_color)
            .finish(),
    )
    .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
    .finish();
    veil
}

impl TerminalView {
    /// The phone's hold on this pane's terminal, if a phone sized it.
    pub(crate) fn phone_control_state(&self) -> Option<crate::realtime::phone_control::Control> {
        let session = self.manager.writer().host_session_id()?;
        crate::realtime::phone_control::get(&session)
    }

    /// Give the terminal back to this pane: the pane's own size is sent so
    /// the full view returns, and the companion host releases the phone,
    /// which leaves the tab. Opening the tab on the phone takes it over again.
    pub(crate) fn take_back_from_phone(&mut self, ctx: &mut ViewContext<Self>) {
        let writer = self.manager.writer().clone();
        if let Some(session) = writer.host_session_id() {
            crate::realtime::phone_control::desktop_take_back(&session);
            writer.reassert_size();
        }
        ctx.notify();
    }

    /// Cover a terminal a phone has sized: the phone-sized screen would draw
    /// squashed in this pane, so it is veiled behind a dialog with one button
    /// that brings it back at full size.
    pub(crate) fn overlay_phone_control(&self, root: Box<dyn Element>) -> Box<dyn Element> {
        let Some(cover) = self.phone_control_state().as_ref().and_then(phone_cover_copy) else {
            return root;
        };
        let hover = self
            .entry_mouse_states
            .borrow_mut()
            .entry("phone_cover_take_back".to_string())
            .or_default()
            .clone();
        let veil = render_phone_cover(
            self.theme,
            crate::fonts::ui_role(self.mono_family),
            self.mono_family,
            cover,
            hover,
            |event_ctx| event_ctx.dispatch_typed_action(TerminalAction::TakeBackFromPhone),
        );
        Box::new(Stack::new().with_child(root).with_child(veil))
    }

    pub(crate) fn render_path_picker_overlay(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let (open, entries, selected) = self.path_picker.read(app, |picker, _| {
            (picker.open, picker.entries().to_vec(), picker.selected)
        });
        if !open || entries.is_empty() {
            return None;
        }
        Some(input_overlays::render_path_picker_overlay(
            &entries,
            selected,
            self.theme,
            self.mono_family,
            self.completion_scroll_state.clone(),
            |index| TerminalAction::ClickPathCompletion { index },
        ))
    }

    pub(crate) fn render_history_search_overlay(
        &self,
        app: &AppContext,
    ) -> Option<Box<dyn Element>> {
        if !self.history_search.open {
            return None;
        }
        let results = self.history_search.results.clone();
        let selected = self.history_search.selected;
        let query = self.history_search.query.clone();
        Some(input_overlays::render_history_search_overlay(
            &results,
            selected,
            &query,
            self.theme,
            self.mono_family,
            |index| TerminalAction::HistorySearchClick { index },
        ))
    }

    pub(crate) fn render_workflow_search_overlay(
        &self,
        app: &AppContext,
    ) -> Option<Box<dyn Element>> {
        if !self.workflow_search.open {
            return None;
        }
        let results = self.workflow_search.results.clone();
        let selected = self.workflow_search.selected;
        let query = self.workflow_search.query.clone();
        Some(input_overlays::render_workflow_search_overlay(
            &results,
            selected,
            &query,
            self.theme,
            self.mono_family,
            self.workflow_scroll_state.clone(),
            |index| TerminalAction::WorkflowSearchClick { index },
        ))
    }

    /// Font size for the alt-screen grid — always the fixed monospace size, on
    /// both a local terminal and a mirror. The mirror renders the host's grid at
    /// the same cell size the desktop uses (no scaling), so text matches; overflow
    /// is left to the TUI's own layout, not a zoom.
    pub(crate) fn alt_screen_font_size(&self) -> f32 {
        let natural = crate::theme::mono_font_size();
        // Shared-session mirror: the emulator holds the HOST's grid (rows×cols),
        // which may be larger than this window (a phone / small browser). A TUI's
        // alt-screen is absolutely-positioned cells — it can't be reflowed, and
        // stretching it distorts the text. So scale the font DOWN so all the
        // host's ROWS fit this window (never upscaling past natural size), which
        // keeps the whole screen's height visible and the cell aspect ratio
        // intact. Width is deliberately NOT part of the fit: shrinking to fit a
        // narrow phone would make text unreadable, so excess width is panned
        // instead (see `render_alt_screen_full`). MIN_READABLE_PX is a hard floor
        // — below it text is illegible, so we stop shrinking and pan both axes.
        if self.is_remote {
            const MIN_READABLE_PX: f32 = 9.0;
            if let Some((rows, _cols)) = self.remote_dims {
                if rows > 0 {
                    let grid_h = rows as f32 * self.cell_metrics.height.max(1.0);
                    let win_h = self.output_viewport_height_px.max(1.0);
                    let fit = (win_h / grid_h).min(1.0);
                    if fit < 1.0 {
                        return (natural * fit).max(MIN_READABLE_PX.min(natural));
                    }
                }
            }
        }
        natural
    }

    /// Mirror only: the host's alt-screen grid width in px at the scale this view
    /// renders it, versus the window width. `Some(grid_px)` when it overflows and
    /// therefore needs horizontal panning.
    pub(crate) fn alt_screen_overflow_width(&self) -> Option<f32> {
        if !self.is_remote {
            return None;
        }
        let (_, cols) = self.remote_dims?;
        if cols == 0 {
            return None;
        }
        // Cell width scales with the font (grid_metrics derives the advance from
        // the render size), so scale the natural advance by the same ratio.
        let scale = self.alt_screen_font_size() / crate::theme::mono_font_size().max(1.0);
        let grid_w = cols as f32 * self.cell_metrics.width.max(1.0) * scale;
        let win_w = self.output_viewport_width_px.max(1.0);
        let overflows = grid_w > win_w + 1.0;
        // Diagnostic: run with DRAGENT_LOG_ALTFIT=1 to see whether the mirror
        // thinks the host's grid overflows its window, and why. Prints the host
        // grid, the scale applied, and the two widths being compared — the exact
        // inputs that decide whether horizontal panning is offered at all.
        static LOG_ALTFIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *LOG_ALTFIT.get_or_init(|| std::env::var("DRAGENT_LOG_ALTFIT").is_ok()) {
            log::info!(
                "[altfit] cols={cols} cell_w={:.2} scale={scale:.3} grid_w={grid_w:.1} \
                 win_w={win_w:.1} overflow={overflows} font={:.1}",
                self.cell_metrics.width,
                self.alt_screen_font_size(),
            );
        }
        overflows.then_some(grid_w)
    }

    pub(crate) fn render_alt_screen_full(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono_family;
        // Shown as a chat: the conversation replaces the terminal picture and
        // scrolls itself, so the mouse never goes to the agent's terminal.
        let chat = self.showing_cli_chat(app);
        let mouse_passthrough = !chat && self.model.read(app, |m, _| m.terminal_mouse_active());
        let rich_input_active = self.rich_input_active;
        let active_ai = self.active_ai_tool(app);

        let (lines, cursor_row, cursor_col, cursor_visible, selection, cols, cwd) =
            self.model.read(app, |m, _| {
                m.with_core(|core| {
                    let emu = core.alt_screen_emulator();
                    (
                        emu.alt_screen_all_lines(),
                        emu.cursor_position().0,
                        emu.cursor_position().1,
                        emu.cursor_visible(),
                        core.alt_ui().selection.clone(),
                        emu.dimensions().1,
                        core.cwd().clone(),
                    )
                })
            });
        let chrome = self.model.read(app, |m, _| {
            m.with_core(|core| sample_ai_chrome(core.alt_screen_emulator(), &theme))
        });
        let stale_colors = self
            .model
            .read(app, |m, _| m.with_core(|core| core.stale_app_colors(&theme)));
        let color = |(r, g, b): (u8, u8, u8)| warpui::color::ColorU::new(r, g, b, 255);
        let display_theme = match stale_colors {
            Some(told) => Theme {
                background: color(told.background),
                foreground: color(told.foreground),
                is_dark: told.is_dark,
                ..theme
            },
            None => Theme {
                background: chrome.panel_bg,
                ..theme
            },
        };
        // Content stays visible while rich input is open — the wrapped CLI's
        // normal output (and, unavoidably, whatever it echoes back for a
        // `/`-proxied keystroke — there is no way to distinguish "the input
        // line" from "everything else" in a raw grid) still needs to be seen.
        // Only the blinking caret itself is suppressed, since rich input owns
        // keyboard focus and a second visible caret reads as two active inputs.
        let cursor_visible = if rich_input_active {
            false
        } else {
            cursor_visible && self.alt_cursor_blink_phase()
        };
        // The pane-filling alt-screen element. Normally it fills the pane and
        // clips — no scroll wrapper, so the mouse wheel reaches the TUI and
        // Claude Code / vim scroll their own content.
        let canvas = build_alt_screen_ui(
            lines,
            cursor_row,
            cursor_col,
            cursor_visible,
            selection,
            display_theme,
            mono,
            self.alt_screen_font_size(),
            cols,
            Some(cwd),
            self.alt_hovered_link.clone(),
        );
        // A mirror whose window is NARROWER than the host's grid pans horizontally
        // instead of shrinking the text into illegibility. `horizontal_no_wheel`
        // pans only by dragging its scrollbar, so the mouse WHEEL still belongs to
        // the TUI — horizontal panning never costs vertical scrolling. The
        // scrollbar is themed (thumb = disabled/main text, track = the panel
        // background) rather than left on a default background.
        let alt_overflow = self.alt_screen_overflow_width();
        let canvas: Box<dyn Element> = match alt_overflow {
            Some(grid_w) => {
                // `with_width` (TIGHT — min AND max), not `with_min_width`:
                // AltScreenElement deliberately reports its PANE allocation
                // rather than its content size, so under an unbounded scrollable
                // constraint it would report the viewport width and there would
                // be nothing to pan. Pinning an exact width makes it lay out —
                // and report — the host's full grid width.
                let grid = ConstrainedBox::new(canvas).with_width(grid_w).finish();
                ClippedScrollable::horizontal_axis_locked(
                    self.remote_alt_h_scroll.clone(),
                    grid,
                    ScrollbarWidth::Auto,
                    display_theme.disabled_text().into(),
                    display_theme.main_text().into(),
                    chrome.panel_bg.into(),
                )
                .finish()
            }
            None => canvas,
        };
        let canvas = if chat { self.render_cli_chat(app) } else { canvas };
        let wrapped = self.wrap_terminal_input(canvas, true, mouse_passthrough);
        let alt_id = format!("terminal_alt_viewport_{}", self.pane_uuid);
        let canvas_container = SavePosition::new(
            Container::new(TerminalSizeElement::new(self.resize_tx.clone(), wrapped).finish())
                .with_background_color(display_theme.background)
                .finish(),
            &alt_id,
        )
        .finish();

        let canvas_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Box::new(Expanded::new(1.0, canvas_container)))
            .finish();

        let mut alt_events = EventHandler::new(canvas_column);
        if alt_overflow.is_some() {
            // The host's grid is wider than this window (a phone / narrow
            // browser). Dragging the CONTENT left/right pans it — the only
            // gesture available on a touch screen, where there is no mouse wheel
            // and no scrollbar thumb to grab. Content follows the finger, so a
            // drag to the right reveals what's off the LEFT edge (scroll start
            // decreases), matching how every touch surface behaves.
            let down_drag = self.alt_pan_drag.clone();
            let move_drag = self.alt_pan_drag.clone();
            let up_drag = self.alt_pan_drag.clone();
            let drag_scroll = self.remote_alt_h_scroll.clone();
            alt_events = alt_events
                .on_left_mouse_down(move |event_ctx, _app, pos| {
                    *down_drag.lock().unwrap() = Some(AltPanDrag {
                        start: pos,
                        last_x: pos.x(),
                        axis: None,
                    });
                    event_ctx.dispatch_typed_action(TerminalAction::FocusInput);
                    DispatchEventResult::StopPropagation
                })
                .on_mouse_dragged(move |event_ctx, _app, pos| {
                    let mut guard = move_drag.lock().unwrap();
                    // No drag state => this gesture didn't start on the grid;
                    // ignore it rather than jumping the view by a bogus delta.
                    let Some(drag) = guard.as_mut() else {
                        return DispatchEventResult::PropagateToParent;
                    };
                    // Commit to an axis once the gesture is unambiguous, then
                    // keep it for the rest of the drag so it can't flip midway.
                    if drag.axis.is_none() {
                        let total = pos - drag.start;
                        let (dx, dy) = (total.x().abs(), total.y().abs());
                        if dx.max(dy) >= ALT_PAN_AXIS_THRESHOLD_PX {
                            drag.axis = Some(if dx > dy {
                                AltPanAxis::Horizontal
                            } else {
                                AltPanAxis::Vertical
                            });
                        }
                    }
                    match drag.axis {
                        // Sideways: pan the grid and keep the event to ourselves.
                        Some(AltPanAxis::Horizontal) => {
                            let dx = pos.x() - drag.last_x;
                            drag.last_x = pos.x();
                            drop(guard);
                            if dx != 0.0 {
                                drag_scroll.scroll_by((-dx).into_pixels());
                                event_ctx.notify();
                            }
                            DispatchEventResult::StopPropagation
                        }
                        // Up/down, or not yet decided: leave it to the TUI — on a
                        // touch screen this is how Claude Code is scrolled.
                        _ => DispatchEventResult::PropagateToParent,
                    }
                })
                .on_left_mouse_up(move |_event_ctx, _app, _pos| {
                    *up_drag.lock().unwrap() = None;
                    DispatchEventResult::PropagateToParent
                });
        } else {
            alt_events = alt_events.on_left_mouse_down(|event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::FocusInput);
                DispatchEventResult::StopPropagation
            });
        }
        let canvas_column = alt_events.finish();

        let content: Box<dyn Element> = if let Some(ai_tool) = active_ai {
            let git_status = self.git_repo_status.read(app, |m, _| m.status.clone());
            let git_cwd = self.git_repo_status.read(app, |m, _| m.cwd.clone());
            // Shown as a chat, the agent gets Dragent's agent input; its own
            // terminal keeps the AI input panel.
            let panel = if chat {
                self.render_chat_composer(app)
            } else {
                ai_toolbar::render_ai_input_panel(
                &self.rich_input_editor,
                &theme,
                chrome,
                mono,
                git_status.as_ref(),
                Some(git_cwd.as_path()),
                rich_input_active,
                self.rich_input_proxy_chars > 0,
                self.voice_ui,
                self.voice_spin,
                self.input_notice.as_deref(),
                None,
                ai_tool,
                self.session_agent_image(theme.is_dark),
                &self.ai_panel_actions_with_view(app),
                None,
            )
            };
            let mut col = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(Box::new(Expanded::new(1.0, canvas_column)));
            // `!`-command output card, pinned above the AI input — the AI tool
            // owns the alt-screen, so this path (not the normal pane_col) is what
            // renders while a tool like Claude Code is active.
            if let Some(side_card) = self.render_side_command_card() {
                col = col.with_child(side_card);
            }
            // `!` scratch-shell history popover, pinned directly above the AI
            // input (this alt-screen path is the only place it renders in AI-tool
            // mode — the global popover doesn't open here).
            if self.cmd_hist_rich {
                if let Some(hist) = self.render_cmd_history_popover() {
                    col = col.with_child(hist);
                }
            }
            col.with_child(panel).finish()
        } else {
            canvas_column
        };

        // Same uniform ~10% tint as the normal pane, covering the alt-screen
        // canvas AND the AI input panel / side cards, so the whole alt screen
        // matches the normal terminal's overlay.
        let content = self.with_terminal_scrim(content);
        self.wrap_outer_key_handler(content, app, active_ai)
    }
}

#[cfg(test)]
mod phone_cover_tests {
    use super::phone_cover_copy;
    use crate::realtime::phone_control::Control;

    #[test]
    fn the_cover_names_the_phone_and_offers_the_way_back() {
        let cover = phone_cover_copy(&Control::Phone {
            phone_name: "Ada's iPhone".into(),
            cols: 40,
            rows: 20,
        })
        .unwrap();
        assert_eq!(cover.title, "In use on Ada's iPhone");
        assert_eq!(cover.size, "40 \u{00d7} 20");
        assert_eq!(cover.button, "Take back control");
        let held = phone_cover_copy(&Control::Held { cols: 40, rows: 20 }).unwrap();
        assert_eq!(held.button, "Restore full size");
        assert!(phone_cover_copy(&Control::Desktop).is_none(), "the desktop's own terminal is never covered");
    }
}
