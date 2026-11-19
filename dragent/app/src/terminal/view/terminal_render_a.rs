//! Terminal block-list rendering.
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
    pub(crate) fn dir_listing_viewport_width(&self, app: &AppContext) -> f32 {
        let window_width = app
            .window_bounds(&self.window_id)
            .map(|bounds| bounds.width())
            .filter(|w| *w > 1.0);
        match (window_width, self.output_viewport_width_px > 1.0) {
            (Some(w), true) => w.min(self.output_viewport_width_px),
            (Some(w), false) => w,
            (None, true) => self.output_viewport_width_px,
            (None, false) => 760.0,
        }
    }

    pub(crate) fn block_render_context(&self, app: &AppContext) -> BlockRenderContext<'_> {
        // A ChildView can only have one layout owner. When its block header is
        // sticky, the filter bar is rendered in that overlay, so do not also
        // mount the same editor in the scrolling copy of the header.
        let filter_is_sticky = self
            .sticky_header
            .as_ref()
            .zip(self.block_filter.as_ref())
            .is_some_and(|(header, (block_id, _))| header.id == *block_id);
        BlockRenderContext {
            entry_hint: self.entry_hint.as_ref(),
            link_hint: self.link_hint.as_ref(),
            hovered_link: self.hovered_link.as_ref(),
            dir_hover: self.dir_hover,
            mouse_states: &self.entry_mouse_states,
            shimmer_states: &self.shimmer_states,
            diagram_states: &self.diagram_states,
            content_width_px: dir_grid_content_width(self.dir_listing_viewport_width(app)),
            theme: self.theme,
            mono: self.mono_family,
            dir_sort: self.dir_sort,
            dir_view: self.dir_view,
            feedback_editor: Some(&self.feedback_editor),
            question_editor: Some(&self.question_editor),
            block_filter_editor: if filter_is_sticky {
                None
            } else {
                self.block_filter
                    .as_ref()
                    .map(|_| &self.block_filter_editor)
            },
            block_filter_match_index: self.block_filter_match_index,
            copied_key: self.copy_confirmation.key(),
        }
    }

    pub(crate) fn clear_block_selection(&self) {
        self.block_selectable_handle.clear();
        *self.block_selected_text.write() = None;
    }

    pub(crate) fn clear_block_hints(&mut self, ctx: &mut ViewContext<Self>) {
        self.entry_hint = None;
        self.link_hint = None;
        ctx.notify();
    }

    pub(crate) fn open_dir_entry(
        &mut self,
        block_id: usize,
        name: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let cwd = self.block_cwd(block_id, ctx);
        let path = resolve_entry_path(&cwd, name);
        if path.is_dir() {
            // Opening a directory has no in-app editor destination — reveal it
            // in the OS file browser, same as before.
            ctx.open_file_path(&path);
        } else {
            ctx.dispatch_typed_action_deferred(
                crate::workspace::WorkspaceAction::OpenTerminalPath {
                    path,
                    line: None,
                    column: None,
                },
            );
        }
        self.entry_hint = None;
        self.link_hint = None;
        ctx.notify();
    }

    /// Resolve a block's captured cwd, falling back to the terminal's current
    /// cwd for synthetic block ids (bootstrap output, the live prompt) or a
    /// block that has already scrolled out of the model's history.
    pub(crate) fn block_cwd(&self, block_id: usize, ctx: &ViewContext<Self>) -> PathBuf {
        self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .find(|b| b.id == block_id)
                    .map(|b| b.cwd.clone())
                    .unwrap_or_else(|| core.cwd().clone())
            })
        })
    }

    /// Handle a click on a terminal-detected file path (as opposed to a
    /// `http(s)://` URL, which `TerminalAction::OpenLink`'s handler already
    /// routes straight to `ctx.open_url`). `raw` is the matched text,
    /// including any `:line:col` suffix.
    pub(crate) fn open_terminal_path_link(
        &mut self,
        block_id: usize,
        raw: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        use crate::terminal::render::block_output::{resolve_terminal_path, split_path_suffix};

        let (raw_path, line, column) = split_path_suffix(raw);
        if raw_path.is_empty() {
            return;
        }
        let cwd = self.block_cwd(block_id, ctx);
        let path = resolve_terminal_path(raw_path, &cwd);
        if path.is_dir() {
            // No editor destination for a folder — open it in the file browser.
            ctx.open_file_path(&path);
            return;
        }
        ctx.dispatch_typed_action_deferred(crate::workspace::WorkspaceAction::OpenTerminalPath {
            path,
            line,
            column,
        });
    }

    /// Build the visible-only block column for the Phase 1.5 `BlockListScrollAdapter` path.
    /// Returns `(child, use_block_list)` where child is a Flex of ONLY visible entries.
    pub(crate) fn build_block_list_child(
        &self,
        visible_entries: &[crate::terminal::viewport::VisibleEntry],
        first_entry_offset_px: f32,
        app: &AppContext,
    ) -> (Box<dyn Element>, bool) {
        let ctx = self.block_render_context(app);
        self.model.read(app, |m, _| {
            m.with_core(|core| {
                let clear_gap_trigger_block_id = core.active_clear_gap().and_then(|g| g.trigger_block_id);
                let blocks = core.blocks().blocks();
                let bootstrap_lines = &core.blocks().bootstrap_lines;
                let show_prompt = !core.is_command_running();
                let integration_active = core.integration_active();
                let emu = core.emulator();
                let (cursor_row, cursor_col) = emu.cursor_position();
                let cursor_visible = emu.cursor_visible();

                // Agent-only shared sessions can have no shell integration,
                // terminal blocks, or bootstrap output at all. The old gate
                // received agent snapshots but selected the raw grid renderer,
                // making every prompt/response invisible in a clean web mirror.
                let has_agent_blocks = self
                    .agent_controller
                    .read(app, |controller, _| !controller.agent_blocks().is_empty());
                let use_block_list = integration_active
                    || !blocks.is_empty()
                    || !bootstrap_lines.is_empty()
                    || has_agent_blocks;

                if use_block_list {
                    let live_prompt = if show_prompt && !integration_active {
                        Some(extract_prompt_tail(emu.grid().screen_styled_lines(), cursor_row, cursor_col, cursor_visible))
                    } else {
                        None
                    };
                    let (agent_blocks, live_checkpoint_ids) = self.agent_controller.read(app, |c, _| {
                        let agent_blocks = c.agent_blocks().to_vec();
                        let live_checkpoint_ids: std::collections::HashSet<usize> = agent_blocks
                            .iter()
                            .filter_map(|b| {
                                matches!(
                                    b.kind,
                                    crate::agent::controller::block::AgentBlockKind::UserQuery { .. }
                                )
                                .then(|| b.id)
                            })
                            .filter(|id| c.has_live_checkpoint(*id))
                            .collect();
                        (agent_blocks, live_checkpoint_ids)
                    });
                    let mut heal_states = self.self_heal_store.read(app, |store, _| {
                        store.all_states().clone()
                    });
                    // Auto-fix-on-failure gate: proactive scans stay quiet
                    // when it is off — see BlockHealState::projected_for_display.
                    let auto_fix = crate::settings::AppSettings::global().agent.auto_fix_on_failure;
                    for (bid, state) in heal_states.iter_mut() {
                        *state = state.projected_for_display(
                            auto_fix,
                            self.auto_analyzed_blocks.contains(bid),
                            self.revealed_fix_blocks.contains(bid),
                        );
                    }
                    let child = build_visible_block_list_column(
                        visible_entries,
                        blocks,
                        &agent_blocks,
                        bootstrap_lines,
                        live_prompt,
                        &ctx,
                        self.block_height_index.clone(),
                        app.font_cache(),
                        self.output_viewport_height_px,
                        first_entry_offset_px,
                        self.measured_block_heights.clone(),
                        self.measured_agent_block_heights.clone(),
                        self.heights_tx.clone(),
                        self.glyph_cache.clone(),
                        self.block_text_cache.clone(),
                        &heal_states,
                        self.block_filter.as_ref().map(|(id, f)| (*id, f)),
                        clear_gap_trigger_block_id,
                        &live_checkpoint_ids,
                    );
                    (child, true)
                } else {
                    let grid = emu.grid();
                    let total_lines = grid.scrollback_len() + grid.rows;
                    let start = total_lines.saturating_sub(1000);
                    let prompt_lines = grid.styled_lines_in(start, total_lines);
                    let abs_cursor_row = grid.scrollback_len() + cursor_row;
                    let adjusted_cursor_row = abs_cursor_row.saturating_sub(start);
                    let child = build_bootstrap_grid_ui(
                        prompt_lines, adjusted_cursor_row, cursor_col,
                        cursor_visible && self.alt_cursor_blink_phase(),
                        self.theme, self.mono_family,
                    );
                    (child, false)
                }
            })
        })
    }

    /// Cover the whole terminal pane in one faint, uniform tint overlay (~10%).
    ///
    /// Under a background-image theme the terminal surfaces are transparent so the
    /// image shows through; this lays a single tint of the theme background over
    /// the ENTIRE pane — output area, input bar, and (via the same call on the
    /// alt-screen path) the alt-screen canvas and AI input panel — so the whole
    /// screen reads as one cohesive surface over the image. It's painted once, on
    /// top, and is pointer-transparent (`ScrimOverlay` records no hit region), so
    /// the terminal stays fully interactive. No-op on solid themes, where the
    /// opaque surfaces already give a uniform look and a tint would only dim them.
    pub(crate) fn with_terminal_scrim(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        if self.theme.background.a == 255 {
            return content;
        }
        let c = self.theme.background;
        let scrim = warpui::color::ColorU::new(c.r, c.g, c.b, 26); // ~10% opacity
        crate::terminal::render::scrim_overlay::ScrimOverlay::new(content, scrim).finish()
    }

    pub(crate) fn wrap_terminal_input(
        &self,
        content: Box<dyn Element>,
        alt_active: bool,
        mouse_passthrough: bool,
    ) -> Box<dyn Element> {
        // Can this canvas take mouse at all? Alt-screen apps and mouse-reporting
        // apps do; a plain scrollback pane does not (it uses the block list).
        let mouse_capable = alt_active || mouse_passthrough;
        if !mouse_capable {
            return Container::new(content)
                .with_background_color(self.theme.pane_surface())
                .finish();
        }

        // The alt-screen canvas stays fully interactive even while the rich
        // input is open: the user still needs to SELECT text, click overlay
        // controls (e.g. Jump to bottom), and forward mouse events to the
        // wrapped CLI. The one thing we suppress while composing is stealing
        // keyboard focus — `FocusInput` moves focus to the command input and
        // would defocus the rich editor — so canvas clicks keep the rich input
        // focused (see the FocusInput guard in on_left_mouse_down below).
        let rich_active = self.rich_input_active;

        let mut handler = EventHandler::new(
            Container::new(content)
                .with_background_color(self.theme.pane_surface())
                .finish(),
        )
        .with_always_handle()
        // Scroll wheel forwards to the wrapped CLI regardless of the rich
        // input — this is the scroll-while-composing fix.
        .on_scroll_wheel(
            move |event_ctx, _app, position, delta, precise, modifiers| {
                if delta.y().abs() > f32::EPSILON {
                    event_ctx.dispatch_typed_action(TerminalAction::TerminalWheel {
                        position: *position,
                        delta_y: delta.y(),
                        precise,
                        modifiers: *modifiers,
                    });
                }
                DispatchEventResult::StopPropagation
            },
        )
        .on_modifier_state_changed(|event_ctx, _app, code, state| {
            let pressed = matches!(state, KeyState::Pressed);
            match code {
                KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                    event_ctx.dispatch_typed_action(TerminalAction::ShiftHeld(pressed));
                }
                KeyCode::ControlLeft | KeyCode::ControlRight => {
                    event_ctx.dispatch_typed_action(TerminalAction::CtrlHeld(pressed));
                }
                KeyCode::AltLeft | KeyCode::AltRight => {
                    event_ctx.dispatch_typed_action(TerminalAction::AltHeld(pressed));
                }
                KeyCode::SuperLeft | KeyCode::SuperRight => {
                    event_ctx.dispatch_typed_action(TerminalAction::CmdHeld(pressed));
                }
                _ => {}
            }
            DispatchEventResult::PropagateToParent
        });

        {
            handler = handler
                .on_left_mouse_down(move |event_ctx, _app, position| {
                    // While composing in the rich input, don't steal keyboard
                    // focus — selection/click still work, but focus stays put.
                    if !rich_active {
                        event_ctx.dispatch_typed_action(TerminalAction::FocusInput);
                    }
                    event_ctx.dispatch_typed_action(TerminalAction::TerminalMouse {
                        position,
                        button: MouseButton::Left,
                        kind: MouseEventKind::Press,
                        modifiers: ModifiersState::default(),
                    });
                    DispatchEventResult::StopPropagation
                })
                .on_left_mouse_up(move |event_ctx, _app, position| {
                    event_ctx.dispatch_typed_action(TerminalAction::TerminalMouse {
                        position,
                        button: MouseButton::Left,
                        kind: MouseEventKind::Release,
                        modifiers: ModifiersState::default(),
                    });
                    DispatchEventResult::StopPropagation
                })
                .on_right_mouse_down(move |event_ctx, _app, position| {
                    event_ctx.dispatch_typed_action(TerminalAction::TerminalMouse {
                        position,
                        button: MouseButton::Right,
                        kind: MouseEventKind::Press,
                        modifiers: ModifiersState::default(),
                    });
                    DispatchEventResult::StopPropagation
                })
                .on_middle_mouse_down(move |event_ctx, _app, _position| {
                    event_ctx.dispatch_typed_action(TerminalAction::Paste);
                    DispatchEventResult::StopPropagation
                })
                .on_mouse_dragged(move |event_ctx, _app, position| {
                    // on_mouse_dragged fires only for LeftMouseDragged events,
                    // so the held button is always Left. Passing None would
                    // cause ButtonMotion mode to drop these motion events.
                    event_ctx.dispatch_typed_action(TerminalAction::TerminalMouse {
                        position,
                        button: MouseButton::Left,
                        kind: MouseEventKind::Motion,
                        modifiers: ModifiersState::default(),
                    });
                    DispatchEventResult::StopPropagation
                });
        }
        handler.finish()
    }

    /// The agent-mode selector: a compact `● Agent ▾` trigger that
    /// opens a dropdown (above the input) listing Agent / Plan / Debug /
    /// Multitask / Ask. Shell is a separate toggle and never appears here.
    /// One row inside a chip popover: a full-width clickable label.
    pub(crate) fn popover_row(
        &self,
        app: &AppContext,
        key: &str,
        label: String,
        icon: Option<crate::icons::AppIcon>,
        color: warpui::color::ColorU,
        theme: &Theme,
        mono: FamilyId,
        action: TerminalAction,
    ) -> Box<dyn Element> {
        let ms = self.block_render_context(app).mouse_state(key);
        let theme = *theme;
        EventHandler::new(
            Hoverable::new(ms, move |state| {
                let mut inner = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0);
                if let Some(icon) = icon {
                    inner = inner.with_child(crate::icons::render_icon(icon, color, ICON_SIZE_SM));
                }
                inner = inner.with_child(Box::new(Expanded::new(
                    1.0,
                    Text::new(label.clone(), mono, crate::theme::text_size(11.5))
                        .with_color(color)
                        .finish(),
                )));
                let mut row = Container::new(inner.finish())
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
                if state.is_hovered() {
                    row = row.with_background_color(theme.surface_2());
                }
                row.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
        .on_left_mouse_down(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    /// Wrap a popover so clicking anywhere outside closes it. Without
    /// `prevent_interaction_with_other_elements` the dismissing click also
    /// lands on the chip underneath and immediately reopens the popover.
    pub(crate) fn dismissible_popover(
        popover: Box<dyn Element>,
        close: TerminalAction,
    ) -> Box<dyn Element> {
        Box::new(
            Dismiss::new(popover)
                .on_dismiss(move |event_ctx, _app| {
                    event_ctx.dispatch_typed_action(close.clone());
                })
                .prevent_interaction_with_other_elements(),
        )
    }

    /// Standard popover shell: padded card, anchored above its chip.
    pub(crate) fn popover_shell(
        body: Box<dyn Element>,
        width: f32,
        theme: &Theme,
    ) -> Box<dyn Element> {
        ConstrainedBox::new(
            Container::new(body)
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish(),
        )
        .with_width(width)
        .finish()
    }

    /// Anchor a popover above a chip (chips sit near the window bottom, so
    /// these open upward — the child's bottom-left meets the chip's top-left).
    pub(crate) fn anchor_above(
        trigger: Box<dyn Element>,
        popover: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Stack::new()
            .with_child(trigger)
            .with_positioned_overlay_child(
                popover,
                OffsetPositioning::offset_from_parent(
                    Vector2F::new(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                ),
            )
            .finish()
    }
}
