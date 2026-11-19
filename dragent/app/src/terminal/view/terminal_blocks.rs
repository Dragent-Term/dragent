//! Terminal block list: output, menus, overlays, layout.
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
    build_link_options_menu, build_sticky_block_header, build_visible_block_list_column,
    dir_grid_content_width, extract_prompt_tail, sample_ai_chrome, BlockListScrollAdapter,
    BlockRenderContext, CellGlyphCache, CellMetrics, DiagramCanvasState, StickyBlockHeader,
    TerminalSizeElement, BLOCK_HEADER_HEIGHT, BLOCK_LIST_BOTTOM_POSITION_ID,
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
    /// Full plain-text output of a block (used by the ⋯ menu "Copy output").
    pub(crate) fn block_output_text(
        &self,
        block_id: usize,
        ctx: &ViewContext<Self>,
    ) -> Option<String> {
        use crate::terminal::model::block_context::colored_lines_to_plain_text;
        let theme = self.theme;
        self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .find(|b| b.id == block_id)
                    .map(|b| colored_lines_to_plain_text(&block_output_lines(b, &theme)))
            })
        })
    }

    /// Command line plus full output of a block ("Copy command & output").
    pub(crate) fn block_command_and_output_text(
        &self,
        block_id: usize,
        ctx: &ViewContext<Self>,
    ) -> Option<String> {
        use crate::terminal::model::block_context::colored_lines_to_plain_text;
        let theme = self.theme;
        self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .find(|b| b.id == block_id)
                    .map(|b| {
                        let out = colored_lines_to_plain_text(&block_output_lines(b, &theme));
                        if out.is_empty() {
                            b.command.clone()
                        } else {
                            format!("{}\n{}", b.command, out)
                        }
                    })
            })
        })
    }

    /// Scroll so the block's header aligns with the viewport top.
    pub(crate) fn scroll_block_to_top(&mut self, block_id: usize) {
        if let Some(top) = self.block_height_index.block_top_px(block_id) {
            *self.block_scroll_pos.lock().unwrap() = ScrollPosition::fixed(top);
            self.auto_scroll = false;
        }
    }

    /// Scroll so the block's bottom aligns with the viewport bottom.
    pub(crate) fn scroll_block_to_bottom(&mut self, block_id: usize) {
        let idx = &self.block_height_index;
        if let (Some(top), Some(h)) = (idx.block_top_px(block_id), idx.block_height_px(block_id)) {
            let vp = self.output_viewport_height_px.max(1.0);
            *self.block_scroll_pos.lock().unwrap() = ScrollPosition::fixed((top + h - vp).max(0.0));
            self.auto_scroll = false;
        }
    }

    /// Select the next or previous matching output line and bring it into view.
    /// Filtered output is laid out as one line per match, so the selected index
    /// can be mapped to a stable block-local scroll position without changing
    /// the terminal's normal scroll behavior.
    pub(crate) fn move_block_filter_match(
        &mut self,
        block_id: usize,
        delta: i32,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some((active_id, filter)) = self.block_filter.as_ref() else {
            return;
        };
        if *active_id != block_id || filter.is_empty() || filter.regex_error {
            return;
        }
        let theme = self.theme;
        let match_count = self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .find(|block| block.id == block_id)
                    .map(|block| count_block_matches(block, filter, &theme))
                    .unwrap_or(0)
            })
        });
        if match_count == 0 {
            return;
        }
        self.block_filter_match_index = if delta < 0 {
            if self.block_filter_match_index == 0 {
                match_count - 1
            } else {
                self.block_filter_match_index - 1
            }
        } else {
            (self.block_filter_match_index + 1) % match_count
        };
        if let Some(top) = self.block_height_index.block_top_px(block_id) {
            // The filter toolbar is part of the measured block header, and text
            // begins below an additional body inset. The old calculation omitted
            // both and then aligned the nominal row with the viewport top, where
            // the sticky command header covered it. Keep the selected result near
            // the viewport centre instead, with enough chrome allowance to make
            // the real painted row visible at narrow terminal heights as well.
            const FILTER_TOOLBAR_HEIGHT: f32 = 40.0;
            const FILTER_BODY_TOP_PADDING: f32 = 20.0;
            let line_height = self.cell_metrics.height.max(1.0);
            let target_center = top
                + BLOCK_HEADER_HEIGHT
                + FILTER_TOOLBAR_HEIGHT
                + FILTER_BODY_TOP_PADDING
                + (self.block_filter_match_index as f32 + 0.5) * line_height;
            let viewport = self.output_viewport_height(ctx).max(line_height);
            let total = self.block_height_index.total_height_px();
            let max_scroll = (total - viewport).max(0.0);
            let target_scroll = (target_center - viewport * 0.5).clamp(0.0, max_scroll);
            *self.block_scroll_pos.lock().unwrap() = ScrollPosition::fixed(target_scroll);
            self.auto_scroll = false;
        }
        self.schedule_sync_sticky_header(ctx);
        ctx.notify();
    }

    /// Closes the block options dropdown, if any, and clears its click-anchor
    /// so a later "⋯"-button open doesn't inherit a stale right-click position.
    pub(crate) fn close_block_menu(&mut self) {
        self.open_block_menu = None;
        self.block_menu_anchor = None;
    }

    /// If a block's options dropdown is open, overlay it on `base`, anchored just
    /// below the block header at the right edge. Clicking outside dismisses it.
    pub(crate) fn wrap_block_menu_overlay(
        &self,
        base: Box<dyn Element>,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let Some(block_id) = self.open_block_menu else {
            return base;
        };
        let Some(block_top) = self.block_height_index.block_top_px(block_id) else {
            return base;
        };

        let total = self.block_height_index.total_height_px();
        let vp = self.output_viewport_height_px.max(1.0);
        let scroll_top = self
            .block_scroll_pos
            .lock()
            .unwrap()
            .scroll_top_px(total, vp);
        // Anchor just under the header; clamp so an off-top block pins near the top.
        let y = (block_top - scroll_top).clamp(0.0, vp) + BLOCK_HEADER_HEIGHT;
        // Keep the whole menu inside the viewport: if it would overflow the bottom,
        // shift it up so its bottom edge sits at the viewport bottom.
        let menu_h = BLOCK_MENU_EST_HEIGHT;
        let y = if y + menu_h > vp {
            (vp - menu_h).max(0.0)
        } else {
            y
        };

        let (command, running) = self
            .model
            .read(app, |m, _| {
                m.with_core(|core| {
                    core.blocks()
                        .blocks()
                        .iter()
                        .find(|b| b.id == block_id)
                        .map(|b| (b.command.clone(), b.is_running()))
                })
            })
            .unwrap_or_default();
        let filter_active = matches!(self.block_filter, Some((id, _)) if id == block_id);

        let render_ctx = self.block_render_context(app);
        let menu = build_block_options_menu(block_id, command, filter_active, running, &render_ctx);

        let dismissible: Box<dyn Element> = Box::new(
            Dismiss::new(menu)
                .on_dismiss(|event_ctx, _app| {
                    event_ctx.dispatch_typed_action(TerminalAction::CloseBlockMenu);
                })
                // Consume the dismissing click so it does not also re-trigger the
                // ⋯ button underneath (which would immediately reopen the menu).
                .prevent_interaction_with_other_elements(),
        );

        let offset = if let Some(anchor) = self.block_menu_anchor {
            // Opened via right-click — anchor exactly at the click point (small
            // nudge off the cursor), clamped to stay fully inside the viewport.
            OffsetPositioning::offset_from_parent(
                anchor + warpui::geometry::vector::vec2f(4.0, 4.0),
                ParentOffsetBounds::ParentByPosition,
                ParentAnchor::TopLeft,
                ChildAnchor::TopLeft,
            )
        } else {
            // Opened via the "⋯" button — header-anchored positioning.
            OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(-10.0, y),
                ParentOffsetBounds::ParentBySize,
                ParentAnchor::TopRight,
                ChildAnchor::TopRight,
            )
        };
        Box::new(
            Stack::new()
                .with_child(base)
                .with_positioned_child(dismissible, offset),
        )
    }

    /// If a link's action menu is open (a plain click on a detected URL or
    /// file path — see `TerminalAction::SelectLink`), overlay it on `base`,
    /// anchored at the click point. Clicking outside dismisses it, same as
    /// `wrap_block_menu_overlay`.
    pub(crate) fn wrap_link_menu_overlay(
        &self,
        base: Box<dyn Element>,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let Some(hint) = self.open_link_menu.as_ref() else {
            return base;
        };
        let Some(anchor) = self.link_menu_anchor else {
            return base;
        };

        let render_ctx = self.block_render_context(app);
        let menu = build_link_options_menu(hint, &render_ctx);

        let dismissible: Box<dyn Element> = Box::new(
            Dismiss::new(menu)
                .on_dismiss(|event_ctx, _app| {
                    event_ctx.dispatch_typed_action(TerminalAction::CloseLinkMenu);
                })
                .prevent_interaction_with_other_elements(),
        );

        let offset = OffsetPositioning::offset_from_parent(
            anchor + warpui::geometry::vector::vec2f(4.0, 4.0),
            ParentOffsetBounds::ParentByPosition,
            ParentAnchor::TopLeft,
            ChildAnchor::TopLeft,
        );
        Box::new(
            Stack::new()
                .with_child(base)
                .with_positioned_child(dismissible, offset),
        )
    }

    /// If a checkpoint restore preview popover is open, overlay it on `base`,
    /// anchored to that message's "⟲ Restore checkpoint" button. Clicking
    /// outside dismisses it, same as `wrap_block_menu_overlay`.
    pub(crate) fn wrap_checkpoint_preview_overlay(
        &self,
        base: Box<dyn Element>,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let Some(block_id) = self.open_checkpoint_preview else {
            return base;
        };
        let Some((stats, removed_user_turns)) = self
            .agent_controller
            .read(app, |c, _| c.checkpoint_restore_preview(block_id))
        else {
            // Evicted from the store between opening and rendering — nothing
            // accurate to show, so just don't overlay anything.
            return base;
        };

        let theme = self.theme;
        let mono = self.mono_family;
        let summary_lines = checkpoint_summary_lines(&stats, removed_user_turns);

        // Per-file rows, styled like the branch-diff panel's file list: a
        // colored extension icon, filename, shortened directory, then
        // +added/-deleted counts.
        let mut file_col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (path, added, deleted) in &stats {
            let fname = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let dir = shorten_dir(
                path.parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default()
                    .as_str(),
            );
            let ext = std::path::Path::new(&fname)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let icon_color = match ext {
                "rs" => warpui_core::color::ColorU {
                    r: 222,
                    g: 110,
                    b: 38,
                    a: 255,
                },
                "ts" | "tsx" => warpui_core::color::ColorU {
                    r: 49,
                    g: 120,
                    b: 198,
                    a: 255,
                },
                "js" | "jsx" => warpui_core::color::ColorU {
                    r: 247,
                    g: 223,
                    b: 30,
                    a: 255,
                },
                "py" => warpui_core::color::ColorU {
                    r: 55,
                    g: 118,
                    b: 171,
                    a: 255,
                },
                _ => theme.sub_text(),
            };
            let icon_label = match ext {
                "rs" => "R",
                "ts" | "tsx" => "T",
                "js" | "jsx" => "J",
                "py" => "P",
                "go" => "G",
                _ => "·",
            };
            let icon_el = Container::new(
                Text::new(icon_label.to_string(), mono, crate::theme::text_size(9.0))
                    .with_color(warpui_core::color::ColorU {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    })
                    .finish(),
            )
            .with_padding_left(3.0)
            .with_padding_right(3.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .with_background_color(icon_color)
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(3.0),
            ))
            .finish();

            let file_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(icon_el)
                .with_child(
                    Text::new(fname, mono, crate::theme::text_size(12.0))
                        .with_color(theme.main_text())
                        .with_style(warpui::fonts::Properties {
                            weight: warpui::fonts::Weight::Medium,
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_child(
                    Text::new(dir, mono, crate::theme::text_size(11.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(Box::new(Expanded::new(
                    1.0,
                    warpui::elements::Empty::new().finish(),
                )))
                .with_child(
                    Text::new(format!("+{added}"), mono, crate::theme::text_size(11.5))
                        .with_color(theme.ansi_green())
                        .finish(),
                )
                .with_child(
                    Text::new(format!("-{deleted}"), mono, crate::theme::text_size(11.5))
                        .with_color(theme.ui_red())
                        .finish(),
                )
                .finish();

            file_col = file_col.with_child(
                Container::new(file_row)
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(8.0)
                    .with_background_color(theme.surface_1())
                    .with_border(
                        warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                    )
                    .finish(),
            );
        }
        let file_scrollable = ClippedScrollable::vertical(
            self.checkpoint_preview_scroll.clone(),
            file_col.finish(),
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish();
        let file_list = ConstrainedBox::new(file_scrollable)
            .with_max_height(160.0)
            .finish();

        let button = |label: String,
                      color: warpui::color::ColorU,
                      key: &str,
                      action: TerminalAction|
         -> Box<dyn Element> {
            let state = self
                .entry_mouse_states
                .borrow_mut()
                .entry(format!("checkpoint-preview-{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(state, move |st| {
                    let bg = if st.is_hovered() {
                        warpui::color::ColorU {
                            r: color.r,
                            g: color.g,
                            b: color.b,
                            a: 28,
                        }
                    } else {
                        warpui::color::ColorU::transparent_black()
                    };
                    Container::new(
                        Text::new(label.clone(), mono, crate::theme::text_size(12.5))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .with_background_color(bg)
                    .with_border(warpui_core::scene::Border::all(1.0).with_border_color(color))
                    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                        warpui_core::scene::Radius::Pixels(7.0),
                    ))
                    .finish()
                })
                .with_cursor(warpui::platform::Cursor::PointingHand)
                .on_click(move |ectx, _, _| ectx.dispatch_typed_action(action.clone())),
            )
        };

        let buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::End)
            .with_spacing(8.0)
            .with_child(button(
                "Cancel".to_string(),
                theme.sub_text(),
                "cancel",
                TerminalAction::CloseCheckpointPreview,
            ))
            .with_child(button(
                "Restore".to_string(),
                theme.ui_red(),
                "restore",
                TerminalAction::RestoreToMessageConfirmed { block_id },
            ))
            .finish();

        let header = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(6.0)
                .with_child(
                    Text::new(
                        "Restore this checkpoint?".to_string(),
                        mono,
                        crate::theme::text_size(13.5),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_spacing(2.0)
                        .with_children(summary_lines.iter().map(|l| {
                            Text::new(format!("• {l}"), mono, crate::theme::text_size(12.0))
                                .with_color(theme.sub_text())
                                .finish()
                        }))
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .finish();

        let footer = Container::new(buttons)
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
            .finish();

        let popover_content = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(header)
                .with_child(file_list)
                .with_child(footer)
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(8.0),
        ))
        .finish();
        let popover = ConstrainedBox::new(popover_content)
            .with_width(320.0)
            .finish();

        let dismissible: Box<dyn Element> = Box::new(
            Dismiss::new(popover)
                .on_dismiss(|event_ctx, _app| {
                    event_ctx.dispatch_typed_action(TerminalAction::CloseCheckpointPreview);
                })
                .prevent_interaction_with_other_elements(),
        );

        // ParentByPosition (not WindowByPosition) clamps the popover to this
        // pane's own bounds — this pane is one of possibly several side by
        // side (plus the right sidebar), and WindowByPosition let the popover
        // extend past this pane's own clip region into a neighboring one,
        // where it painted with a see-through background.
        let offset = OffsetPositioning::offset_from_save_position_element(
            &format!("checkpoint_btn_{block_id}"),
            warpui::geometry::vector::vec2f(0.0, 4.0),
            PositionedElementOffsetBounds::ParentByPosition,
            PositionedElementAnchor::BottomLeft,
            ChildAnchor::TopLeft,
        );
        Box::new(
            Stack::new()
                .with_child(base)
                .with_positioned_child(dismissible, offset),
        )
    }

    pub(crate) fn apply_clear_viewport(&mut self, ctx: &mut ViewContext<Self>) {
        let viewport_height = self.output_viewport_height(ctx);
        if viewport_height <= 1.0 {
            ctx.spawn(
                Timer::after(Duration::from_millis(0)),
                move |view, _, mut ctx| {
                    view.apply_clear_viewport(&mut ctx);
                    view.rebuild_height_index(&mut ctx);
                    view.pin_to_bottom(&mut ctx);
                    ctx.notify();
                },
            );
            return;
        }
        self.output_viewport_height_px = viewport_height;
        self.model.update(ctx, |m, mctx| {
            m.apply_clear_viewport(viewport_height, mctx);
        });
    }

    pub(crate) fn after_terminal_view_layout(
        &mut self,
        pane_px: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        let (w, h) = (pane_px.x(), pane_px.y());
        if w < 1.0 || h < 1.0 {
            return;
        }

        self.sync_output_viewport_width(w, ctx);

        // Shared-session mirror that does NOT drive the host size — a read-only
        // viewer OR a teammate controller (different account). It adopts the
        // host's exact grid (its emulator = the host's rows×cols) and letterboxes
        // it to this window (see `alt_screen_font_size`), so the host stays
        // authoritative and content is never distorted. Only a controller that is
        // the host's OWN account self-sizes (below) and drives the host to match.
        if self.is_remote && !self.remote_drives_host_size() {
            if let Some((rows, cols)) = self.remote_dims {
                // Record this window's real height BEFORE returning: the normal
                // update paths below (and `schedule_viewport_geometry_sync`) are
                // never reached on this branch, so without this the field keeps
                // its construction-time default and the alt-screen fit-scale
                // (`alt_screen_font_size`) would size against a phantom height.
                if (h - self.output_viewport_height_px).abs() > 1.0 {
                    self.output_viewport_height_px = h;
                    if self.auto_scroll {
                        self.scroll_to_bottom();
                    }
                }
                let cur = self
                    .model
                    .read(ctx, |m, _| m.with_core(|c| (c.size().rows, c.size().cols)));
                if cur != (rows, cols) {
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
                    self.model.update(ctx, |m, mctx| {
                        m.apply_size_update(update, |_, _| {}, mctx);
                    });
                }
                return;
            }
            // No host dims yet — fall through to self-size until they arrive.
        }

        // Host with a controlling viewer driving the shared PTY: the viewer's
        // window sets the size, so don't override it from the host's own window.
        if !self.is_remote && self.remote_requested_dims.is_some() {
            return;
        }

        let alt_active = self.model.read(ctx, |m, _| m.alt_screen_active());
        let id = if alt_active {
            format!("terminal_alt_viewport_{}", self.pane_uuid)
        } else {
            format!("terminal_output_viewport_{}", self.pane_uuid)
        };
        let rich_input_active = self.rich_input_active;
        let viewport_h = if alt_active && !rich_input_active {
            // When the AI input panel is collapsed in alt-screen mode, we report the
            // full height `h` to the PTY. This allows rich TUIs like Claude Code to
            // detect sufficient terminal height and render their full welcome banner
            // and graphics layout (such as the orange space invader box) rather than
            // falling back to a condensed text layout.
            h
        } else if let Some(rect) = ctx.element_position_by_id(&id) {
            rect.height()
        } else {
            let input_box_h = if self.model.read(ctx, |m, _| m.is_command_running()) {
                0.0
            } else {
                75.0
            };
            (h - input_box_h).max(1.0)
        };

        let viewport_resized = (viewport_h - self.output_viewport_height_px).abs() > 1.0;
        if viewport_resized {
            self.output_viewport_height_px = viewport_h;
            if self.auto_scroll {
                self.scroll_to_bottom();
            }
        }

        let new_size = SizeInfo::from_pane(
            w,
            viewport_h,
            self.cell_metrics.width,
            self.cell_metrics.height,
            RESIZE_PADDING_X,
            RESIZE_PADDING_Y,
        );

        let alt_active = self.model.read(ctx, |m, _| m.alt_screen_active());
        let dims = (new_size.rows, new_size.cols);
        let rich_input_active = self.rich_input_active;

        if self.last_dims == Some(dims)
            && self.last_alt_active == alt_active
            && self.last_rich_input_active == rich_input_active
        {
            return;
        }

        // Windows ConPTY repaints the ENTIRE viewport on any resize (cursor-home +
        // full redraw of every visible row), re-emitting the prior screen — old
        // prompts and scrollback — into whichever block emulator is currently
        // capturing. That both pollutes the running command's block with unrelated
        // history and can interleave the shell-integration OSC hooks into the middle
        // of its output. dragent resizes the PTY on layout shifts (the input/running
        // bar changing height mid-command changes the row count), so this fires
        // constantly. Defer the resize while a non-alt command is running: leave
        // `last_dims` unchanged so the next layout pass after the command completes
        // applies the pending size. Cols are unaffected by the bar height, so output
        // width stays correct meanwhile. Alt-screen apps (TUIs) must resize live and
        // are excluded. No-op on Unix, whose PTYs stream cleanly and never repaint.
        #[cfg(windows)]
        if !alt_active && self.model.read(ctx, |m, _| m.is_command_running()) {
            return;
        }

        let reason = if self.last_alt_active != alt_active {
            SizeReason::AltScreenTransition
        } else {
            SizeReason::WindowResize
        };

        self.last_dims = Some(dims);
        self.last_alt_active = alt_active;
        self.last_rich_input_active = rich_input_active;

        let update = SizeUpdate::new(new_size, reason);
        log::debug!("[resize-debug] view dims={:?} alt={} rich={} notice={:?} running={} h={} viewport_h={} reason={:?}", dims, alt_active, rich_input_active, self.input_notice, self.model.read(ctx, |m, _| m.is_command_running()), h, viewport_h, reason);
        let writer = self.manager.writer().clone();
        self.model.update(ctx, |m, mctx| {
            m.apply_size_update(update, |rows, cols| writer.resize(rows, cols), mctx);
        });
        self.schedule_viewport_geometry_sync(ctx);

        // A host-account controller just self-sized — tell the host to resize its
        // shared PTY to match, so the TUI re-renders to fill this viewer's window
        // (the writer above is a remote sink and doesn't reach the host's PTY size;
        // this does). Read-only viewers AND teammate controllers returned earlier
        // (they adopt the host's grid), so only a host-account controller is here.
        if self.remote_drives_host_size() {
            use warpui::SingletonEntity;
            let sm = crate::realtime::manager::SessionManager::handle(ctx);
            let (rows, cols) = dims;
            ctx.read_model(&sm, |m, _| m.send_resize(rows, cols));
        }
    }
}
