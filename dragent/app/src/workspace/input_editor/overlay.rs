//! Inline overlays: mentions, carets, highlights, diagnostics.
//!
//! Blocks here are impl InputEditor: same type, same behavior.

use super::model::*;
use super::text::*;
use crate::console::command_intel::{diagnostic_cols_on_line, diagnostics_for};
use crate::console::input_bar::{
    build_highlighted_command, is_likely_command, style_runs_for_command,
};
use crate::console::input_editing::{
    line_end, line_start, matching_close_bracket, word_end, word_start,
};
use crate::console::input_layout::{
    caret_x_for_char_index, caret_x_in_layout, caret_x_plain, char_col_at_x,
    char_col_at_x_in_layout, char_index_at_click_plain, line_height_with_spacing,
    selection_cols_on_line, unwrapped_line_layout, visual_subline_layouts,
};
use crate::console::mention_popup::MentionEntry;
use crate::console::vim_mode::{VimContext, VimDispatch, VimEffect, VimMode, VimState};
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::editor_render::{
    self, EDITOR_BREAKPOINT_ZONE_WIDTH, EDITOR_CONTENT_PADDING_LEFT, EDITOR_FOLD_ZONE_WIDTH,
};
use dragent_document::{
    capabilities_for_text, DiffLineKind, DocumentCapabilities, DocumentDiff, GitDiffDisplayRow,
};
use dragent_editor::{
    Breakpoint, BreakpointRegistry, DiagnosticSeverity, FoldIndex, FoldParser, FoldRegion,
    FoldState, Language, LineChangeKind, ResolvedDiagnostic,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance, SingleAxisConfig,
};
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded,
    Fill, Flex, MainAxisAlignment, MainAxisSize, NewScrollable, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Point, SavePosition, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::{Cache as FontCache, FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::keymap::Keystroke;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{
    AfterLayoutContext, AppContext, Entity, EventContext, FocusHandle, LayoutContext, PaintContext,
    SizeConstraint, TypedActionView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Hoverable, MouseStateHandle, SELECTED_HIGHLIGHT_COLOR};
use warpui_core::event::DispatchedEvent;
use warpui_core::event::KeyState;
use warpui_core::event::ModifiersState;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::units::IntoPixels;

use super::InputEditor;

impl InputEditor {
    pub(crate) fn line_with_overlays(
        &self,
        text_el: Box<dyn Element>,
        line_text: &str,
        line_char_offset: usize,
        line_cursor_col: Option<usize>,
        source_line: Option<usize>,
        is_active_source_line: bool,
        app: &AppContext,
        syntax_highlighted: bool,
        draw_ghost_arrow: bool,
    ) -> Box<dyn Element> {
        let mut stack = Stack::new();

        if is_active_source_line {
            stack = stack.with_positioned_child(
                self.active_source_line_element(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, 0.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::MiddleLeft,
                    ChildAnchor::MiddleLeft,
                ),
            );
        }

        // Search results are sorted by buffer position. Large documents can
        // have tens of thousands of matches; only inspect ranges that can
        // intersect this visible row instead of replaying the whole search
        // result set for every rendered row.
        let line_end = line_char_offset.saturating_add(line_text.chars().count());
        let search_start = if self.document_capabilities.large_file {
            self.search_matches
                .partition_point(|range| range.end <= line_char_offset)
        } else {
            0
        };
        let search_end = if self.document_capabilities.large_file {
            self.search_matches
                .partition_point(|range| range.start < line_end)
        } else {
            self.search_matches.len()
        };
        for index in search_start..search_end {
            let range = &self.search_matches[index];
            if let Some((col_start, col_end)) = selection_cols_on_line(
                line_char_offset,
                line_text.chars().count(),
                range.start,
                range.end,
            ) {
                let x_start = self.caret_x_on_line(line_text, col_start, app, syntax_highlighted);
                let crosses_line = range.end > line_char_offset + line_text.chars().count();
                let x_end = if crosses_line {
                    self.continued_range_end_x(line_text, app, syntax_highlighted)
                } else {
                    self.caret_x_on_line(line_text, col_end, app, syntax_highlighted)
                };
                let Some((x_start, x_end)) = self.visible_highlight_span(x_start, x_end) else {
                    continue;
                };
                let offset = OffsetPositioning::offset_from_parent(
                    vec2f(x_start, 0.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::MiddleLeft,
                    ChildAnchor::MiddleLeft,
                );
                stack = stack.with_positioned_child(
                    self.search_highlight_element(
                        (x_end - x_start).max(1.0),
                        self.active_search_match == Some(index),
                    ),
                    offset,
                );
            }
        }

        if let Some((sel_start, sel_end)) = self.active_selection_range() {
            if let Some((col_start, col_end)) = selection_cols_on_line(
                line_char_offset,
                line_text.chars().count(),
                sel_start,
                sel_end,
            ) {
                let x_start = self.caret_x_on_line(line_text, col_start, app, syntax_highlighted);
                let is_wrapped_selection = sel_end > line_char_offset + line_text.chars().count();
                let x_end = if is_wrapped_selection {
                    self.continued_range_end_x(line_text, app, syntax_highlighted)
                } else {
                    self.caret_x_on_line(line_text, col_end, app, syntax_highlighted)
                };
                if let Some((x_start, x_end)) = self.visible_highlight_span(x_start, x_end) {
                    let width = (x_end - x_start).max(1.0);
                    let highlight_offset = OffsetPositioning::offset_from_parent(
                        vec2f(x_start, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::MiddleLeft,
                        ChildAnchor::MiddleLeft,
                    );
                    stack = stack.with_positioned_child(
                        self.selection_highlight_element(width),
                        highlight_offset,
                    );
                }
            }
        }

        for chip in &self.chip_spans {
            if let Some((col_start, col_end)) = selection_cols_on_line(
                line_char_offset,
                line_text.chars().count(),
                chip.range.start,
                chip.range.end,
            ) {
                let x_start = self.caret_x_on_line(line_text, col_start, app, syntax_highlighted);
                let x_end = self.caret_x_on_line(line_text, col_end, app, syntax_highlighted);
                let width = (x_end - x_start).max(1.0);
                let chip_offset = OffsetPositioning::offset_from_parent(
                    vec2f(x_start, 0.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::MiddleLeft,
                    ChildAnchor::MiddleLeft,
                );
                stack = stack.with_positioned_child(self.chip_pill_element(width), chip_offset);
            }
        }

        stack = stack.with_child(text_el);

        if self.is_source_editor() {
            let diagnostic_indices = source_line
                .and_then(|line| self.source_diagnostics_by_line.get(&line))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let diagnostic_layout = (!diagnostic_indices.is_empty()).then(|| {
                unwrapped_line_layout(
                    line_text,
                    self.ui_font,
                    self.font_size(),
                    Properties::default().weight(self.font_weight),
                    app.font_cache(),
                )
            });
            for &index in diagnostic_indices {
                let Some(resolved) = self.source_diagnostics.get(index) else {
                    continue;
                };
                if let Some((col_start, col_end)) = selection_cols_on_line(
                    line_char_offset,
                    line_text.chars().count(),
                    resolved.range.start,
                    resolved.range.end,
                ) {
                    let layout = diagnostic_layout
                        .as_ref()
                        .expect("layout exists for diagnostics");
                    let x_start = caret_x_in_layout(line_text, layout, col_start);
                    let x_end = caret_x_in_layout(line_text, layout, col_end);
                    let offset = OffsetPositioning::offset_from_parent(
                        vec2f(x_start, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::BottomLeft,
                        ChildAnchor::BottomLeft,
                    );
                    stack = stack.with_positioned_child(
                        self.diagnostic_interaction_element(
                            (x_end - x_start).max(4.0),
                            index,
                            resolved,
                        ),
                        offset,
                    );
                }
            }

            if let Some((index, resolved)) = self.active_diagnostic.and_then(|index| {
                self.source_diagnostics
                    .get(index)
                    .map(|value| (index, value))
            }) {
                if source_line == Some(resolved.diagnostic.range.start.line as usize + 1) {
                    stack = stack.with_positioned_overlay_child(
                        self.diagnostic_problem_panel(index, resolved),
                        OffsetPositioning::offset_from_parent(
                            vec2f(0.0, 5.0),
                            ParentOffsetBounds::WindowByPosition,
                            ParentAnchor::BottomLeft,
                            ChildAnchor::TopLeft,
                        ),
                    );
                }
            }
        }

        if self.mode == InputMode::TerminalCommand && syntax_highlighted {
            let line_byte_start = self
                .buffer
                .char_indices()
                .nth(line_char_offset)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let diags = diagnostics_for(&self.buffer);
            for (start_col, end_col) in diagnostic_cols_on_line(line_byte_start, line_text, &diags)
            {
                let x_start = self.caret_x_on_line(line_text, start_col, app, true);
                let x_end = self.caret_x_on_line(line_text, end_col, app, true);
                let width = (x_end - x_start).max(1.0);
                let err_offset = OffsetPositioning::offset_from_parent(
                    vec2f(x_start, 0.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::BottomLeft,
                );
                stack =
                    stack.with_positioned_child(self.error_underline_element(width), err_offset);
            }
        }

        if let Some(hover) = self
            .definition_hover
            .as_ref()
            .filter(|hover| hover.resolved)
        {
            if let Some((col_start, col_end)) = selection_cols_on_line(
                line_char_offset,
                line_text.chars().count(),
                hover.symbol_range.start,
                hover.symbol_range.end,
            ) {
                let x_start = self.caret_x_on_line(line_text, col_start, app, syntax_highlighted);
                let x_end = self.caret_x_on_line(line_text, col_end, app, syntax_highlighted);
                stack = stack.with_positioned_child(
                    self.definition_hover_element((x_end - x_start).max(1.0)),
                    OffsetPositioning::offset_from_parent(
                        vec2f(x_start, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::MiddleLeft,
                        ChildAnchor::MiddleLeft,
                    ),
                );
            }
        }

        if self.remote_caret.is_some() || (self.focus_handle.is_focused(app) && self.cursor_visible)
        {
            if let Some(cursor_col) = line_cursor_col {
                let cursor_x = self.caret_x_on_line(line_text, cursor_col, app, syntax_highlighted);
                let centered_caret = self.remote_caret.is_none()
                    && (self.is_source_editor()
                        || !matches!(self.mode, InputMode::TerminalCommand));
                let cursor_y = if self.remote_caret.is_some() || centered_caret {
                    0.0
                } else {
                    -2.0
                };
                let cursor_offset = OffsetPositioning::offset_from_parent(
                    vec2f(cursor_x, cursor_y),
                    ParentOffsetBounds::Unbounded,
                    if centered_caret {
                        ParentAnchor::MiddleLeft
                    } else {
                        ParentAnchor::BottomLeft
                    },
                    if self.remote_caret.is_some() {
                        ChildAnchor::BottomMiddle
                    } else if centered_caret {
                        ChildAnchor::MiddleLeft
                    } else {
                        ChildAnchor::BottomLeft
                    },
                );
                let cursor = self
                    .remote_caret_element()
                    .unwrap_or_else(|| self.cursor_element());
                stack = stack.with_positioned_child(cursor, cursor_offset);
            }
        }

        if draw_ghost_arrow {
            let arrow_icon = render_icon(AppIcon::ArrowRight, self.theme.disabled_text(), 13.0);
            let arrow_btn = EventHandler::new(
                Container::new(arrow_icon)
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(2.0)
                    .with_padding_bottom(2.0)
                    .with_border(Border::all(1.0).with_border_color(self.theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish(),
            )
            .on_left_mouse_down(|event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(InputEditorAction::AcceptGhost);
                DispatchEventResult::StopPropagation
            })
            .finish();

            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(stack.finish())
                .with_child(Container::new(arrow_btn).with_margin_left(25.0).finish())
                .finish()
        } else {
            stack.finish()
        }
    }

    /// Pixel offset (relative to this editor's own bounds) of the open
    /// mention popup's anchor — the top of the visual line containing the
    /// triggering `@`. The popup itself is bottom-anchored to this point
    /// (see `render`) so it opens upward, since the AI input box normally
    /// sits near the bottom of the pane.
    pub(crate) fn mention_anchor_offset(&self, app: &AppContext) -> Option<Vector2F> {
        let at = self.active_mention?;
        let vls = self.all_visual_lines(self.layout_width.get(), app.font_cache());
        if vls.is_empty() {
            return None;
        }
        let row = Self::cursor_visual_row(at, &vls);
        let (line_text, abs_offset) = &vls[row];
        let col = at.saturating_sub(*abs_offset);
        let x = self.caret_x_on_line(line_text, col, app, false);
        let lh = self.line_height();
        let scroll_y = self.scroll_state.scroll_start().as_f32();
        let y = row as f32 * lh - scroll_y;
        Some(vec2f(x, y))
    }

    /// The `@`-mention candidate list, positioned by the caller via
    /// `mention_anchor_offset`. `None` while no popup is open.
    pub(crate) fn render_mention_popup(&self) -> Option<Box<dyn Element>> {
        if self.active_mention.is_none() || self.mention_candidates.is_empty() {
            return None;
        }
        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (idx, entry) in self.mention_candidates.iter().enumerate() {
            let is_selected = idx == self.mention_selected;
            let icon = if entry.is_dir {
                AppIcon::Folder
            } else {
                AppIcon::Terminal
            };
            let label = entry.label.clone();
            let mut row = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(render_icon(icon, self.theme.sub_text(), 13.0))
                    .with_child(Box::new(
                        Text::new(label, self.ui_font, crate::theme::text_size(12.0))
                            .with_color(self.theme.main_text()),
                    ))
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0);
            if is_selected {
                row = row.with_background_color(self.theme.accent_overlay());
            }
            let row_el = EventHandler::new(row.finish())
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(InputEditorAction::ClickMentionEntry(idx));
                    DispatchEventResult::StopPropagation
                })
                .finish();
            list = list.with_child(row_el);
        }
        let modal = Container::new(
            ConstrainedBox::new(list.finish())
                .with_max_height(200.0)
                .finish(),
        )
        .with_background_color(self.theme.surface_2())
        .with_border(Border::all(1.5).with_border_color(self.theme.accent))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish();
        Some(ConstrainedBox::new(modal).with_width(240.0).finish())
    }

    pub(crate) fn caret_x_on_line(
        &self,
        line_text: &str,
        col: usize,
        app: &AppContext,
        syntax_highlighted: bool,
    ) -> f32 {
        if self.is_source_editor() {
            let layout = unwrapped_line_layout(
                line_text,
                self.ui_font,
                self.font_size(),
                Properties::default().weight(self.font_weight),
                app.font_cache(),
            );
            caret_x_in_layout(line_text, &layout, col)
        } else if syntax_highlighted {
            caret_x_for_char_index(
                line_text,
                col,
                self.ui_font,
                self.font_size(),
                &self.theme,
                app,
            )
        } else {
            caret_x_plain(line_text, col, self.ui_font, self.font_size(), app)
        }
    }

    /// Horizontal range of row-content x that is currently on screen, for
    /// source rows (unwrapped, scrolled horizontally). `None` for wrapped
    /// editors, whose rows never scroll sideways.
    fn visible_source_x_range(&self) -> Option<(f32, f32)> {
        if !self.is_source_editor() {
            return None;
        }
        let viewport = self.source_horizontal_viewport_width(
            self.available_width.get() + self.source_gutter_width() + EDITOR_CONTENT_PADDING_LEFT,
        );
        let scroll_x = self
            .effective_horizontal_scroll_state()
            .scroll_start()
            .as_f32()
            .max(0.0);
        Some((scroll_x, scroll_x + viewport))
    }

    /// Right edge, in row-content coordinates, of a highlight on a row that a
    /// selection/search range continues past. Wrapped editors fill to the
    /// viewport width. Source rows slide left as the editor scrolls
    /// horizontally, so a viewport-width bar ended partway across the screen
    /// once scrolled; instead reach the visible right edge or past the end of
    /// the line's own text (plus one newline cell), whichever is further.
    fn continued_range_end_x(
        &self,
        line_text: &str,
        app: &AppContext,
        syntax_highlighted: bool,
    ) -> f32 {
        let Some((_, visible_end)) = self.visible_source_x_range() else {
            return self.available_width.get();
        };
        let text_end = self.caret_x_on_line(
            line_text,
            line_text.chars().count(),
            app,
            syntax_highlighted,
        );
        visible_end.max(text_end + self.font_size() * 0.6)
    }

    /// Clip a highlight span to what's on screen. Positioned children are
    /// laid out with the *window* width as their maximum, so an unclipped bar
    /// for a long line scrolled far right would be silently truncated and
    /// break again; a visible-only span is never wider than the viewport.
    /// `None` when the span is entirely off screen.
    fn visible_highlight_span(&self, x_start: f32, x_end: f32) -> Option<(f32, f32)> {
        match self.visible_source_x_range() {
            Some((visible_start, visible_end)) => {
                clip_highlight_span(x_start, x_end, visible_start, visible_end)
            }
            None => Some((x_start, x_end)),
        }
    }

    pub(crate) fn selection_highlight_element(&self, width: f32) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(width)
                .with_height(self.line_height())
                .finish(),
        )
        .with_background_color(*SELECTED_HIGHLIGHT_COLOR)
        .finish()
    }

    pub(crate) fn active_source_line_element(&self) -> Box<dyn Element> {
        let accent = self.theme.accent;
        ActiveSourceLineHighlight {
            horizontal_scroll: self.rendered_horizontal_scroll_state(),
            width_hint: self.available_width.get(),
            line_height: self.line_height(),
            color: ColorU::new(accent.r, accent.g, accent.b, 22),
            size: None,
            origin: None,
        }
        .finish()
    }

    pub(crate) fn indent_guide_color(&self, active_line: bool) -> ColorU {
        if active_line {
            // Guides are editor structure, not an application call-to-action.
            // Keep the active guide neutral and visibly stronger than its peers.
            let text = self.theme.main_text();
            ColorU::new(text.r, text.g, text.b, 112)
        } else {
            let text = self.theme.main_text();
            ColorU::new(text.r, text.g, text.b, 68)
        }
    }

    pub(crate) fn search_highlight_element(&self, width: f32, active: bool) -> Box<dyn Element> {
        let color = if active {
            self.theme.accent_overlay()
        } else {
            let yellow = self.theme.ui_yellow();
            ColorU::new(yellow.r, yellow.g, yellow.b, 72)
        };
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(width)
                .with_height(self.line_height())
                .finish(),
        )
        .with_background_color(color)
        .finish()
    }

    /// Pill background painted behind an `@`-mention chip's label text.
    pub(crate) fn chip_pill_element(&self, width: f32) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(width)
                .with_height(self.line_height())
                .finish(),
        )
        .with_background_color(self.theme.accent_overlay())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    }

    pub(crate) fn error_underline_element(&self, width: f32) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(width)
                .with_height(2.0)
                .finish(),
        )
        .with_background_color(self.theme.ansi_red())
        .finish()
    }

    pub(crate) fn definition_hover_element(&self, width: f32) -> Box<dyn Element> {
        let height = self.line_height();
        let underline = ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(self.theme.main_text())
                .finish(),
        )
        .with_width(width)
        .with_height(1.0)
        .finish();
        let content = Stack::new()
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(width)
                    .with_height(height)
                    .finish(),
            )
            .with_positioned_child(
                underline,
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, -2.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::BottomLeft,
                ),
            )
            .finish();
        Hoverable::new(self.definition_hover_mouse_state.clone(), move |_state| {
            content
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    pub(crate) fn diagnostic_color(&self, severity: DiagnosticSeverity) -> ColorU {
        let color = match severity {
            DiagnosticSeverity::Error => self.theme.ui_red(),
            DiagnosticSeverity::Warning => self.theme.ui_yellow(),
            DiagnosticSeverity::Information => self.theme.accent,
            DiagnosticSeverity::Hint => self.theme.sub_text(),
        };
        color
    }

    pub(crate) fn diagnostic_label(resolved: &ResolvedDiagnostic) -> String {
        match (
            resolved.diagnostic.source.as_deref(),
            resolved.diagnostic.code.as_deref(),
        ) {
            (Some(source), Some(code)) => format!("{source}({code})"),
            (Some(source), None) => source.to_string(),
            (None, Some(code)) => code.to_string(),
            (None, None) => "language server".to_string(),
        }
    }

    pub(crate) fn diagnostic_text_button(
        &self,
        label: impl Into<String>,
        action: InputEditorAction,
    ) -> Box<dyn Element> {
        EventHandler::new(
            Container::new(
                Text::new(label.into(), self.ui_font, crate::theme::text_size(11.5))
                    .with_color(self.theme.accent)
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish(),
        )
        .on_left_mouse_down(move |event_ctx, _app, _position| {
            event_ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    pub(crate) fn diagnostic_icon_button(
        &self,
        icon: AppIcon,
        action: InputEditorAction,
    ) -> Box<dyn Element> {
        EventHandler::new(
            ConstrainedBox::new(
                Container::new(render_icon(icon, self.theme.main_text(), 13.0))
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .with_padding_top(5.0)
                    .with_padding_bottom(5.0)
                    .finish(),
            )
            .with_width(27.0)
            .finish(),
        )
        .on_left_mouse_down(move |event_ctx, _app, _position| {
            event_ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    pub(crate) fn diagnostic_hover_card(
        &self,
        index: usize,
        resolved: &ResolvedDiagnostic,
    ) -> Box<dyn Element> {
        let label = Self::diagnostic_label(resolved);
        let content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Text::new(
                    resolved.diagnostic.message.clone(),
                    self.ui_font,
                    crate::theme::text_size(12.5),
                )
                .with_color(self.theme.main_text())
                .soft_wrap(true)
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(label, self.ui_font, crate::theme::text_size(11.0))
                        .with_color(self.theme.disabled_text())
                        .finish(),
                )
                .with_padding_top(5.0)
                .finish(),
            )
            .with_child(
                Container::new(self.diagnostic_text_button(
                    "View Problem",
                    InputEditorAction::OpenDiagnostic(index),
                ))
                .with_margin_top(7.0)
                .finish(),
            )
            .finish();
        let width = (self.available_width.get() - 28.0).clamp(300.0, 720.0);
        ConstrainedBox::new(
            Container::new(content)
                .with_padding_left(14.0)
                .with_padding_right(14.0)
                .with_padding_top(12.0)
                .with_padding_bottom(8.0)
                .with_background_color(self.theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(self.theme.accent))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish(),
        )
        .with_width(width)
        .finish()
    }

    pub(crate) fn diagnostic_interaction_element(
        &self,
        width: f32,
        index: usize,
        resolved: &ResolvedDiagnostic,
    ) -> Box<dyn Element> {
        let state = self
            .diagnostic_mouse_states
            .get(index)
            .cloned()
            .unwrap_or_default();
        Hoverable::new(state, |mouse| {
            let target = Stack::new()
                .with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(width)
                        .with_height(self.line_height())
                        .finish(),
                )
                .with_positioned_child(
                    editor_render::diagnostic_squiggle(
                        width,
                        self.diagnostic_color(resolved.diagnostic.severity),
                    ),
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::BottomLeft,
                        ChildAnchor::BottomLeft,
                    ),
                )
                .finish();
            if mouse.is_hovered() {
                Stack::new()
                    .with_child(target)
                    .with_positioned_overlay_child(
                        self.diagnostic_hover_card(index, resolved),
                        OffsetPositioning::offset_from_parent(
                            // Keep the interactive hover card flush with the
                            // underline. A visual gap causes the originating
                            // hover target to disappear before the pointer can
                            // enter the card.
                            vec2f(0.0, 0.0),
                            ParentOffsetBounds::WindowByPosition,
                            ParentAnchor::TopLeft,
                            ChildAnchor::BottomLeft,
                        ),
                    )
                    .finish()
            } else {
                target
            }
        })
        .with_hover_in_delay(Duration::from_millis(180))
        // The card is a child overlay. Keep it mounted long enough for the
        // pointer to leave the small underline and enter its buttons; child
        // handlers receive the click first through defer_events_to_children.
        .with_hover_out_delay(Duration::from_millis(650))
        .with_defer_events_to_children()
        .finish()
    }

    pub(crate) fn diagnostic_problem_panel(
        &self,
        index: usize,
        resolved: &ResolvedDiagnostic,
    ) -> Box<dyn Element> {
        let file = self
            .source_path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Problem");
        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new("×", self.ui_font, crate::theme::text_size(17.0))
                    .with_color(self.diagnostic_color(resolved.diagnostic.severity))
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(
                        format!(
                            "{file}  {} of {} problems",
                            index + 1,
                            self.source_diagnostics.len()
                        ),
                        self.ui_font,
                        crate::theme::text_size(11.5),
                    )
                    .with_color(self.theme.main_text())
                    .finish(),
                )
                .with_padding_left(8.0)
                .finish(),
            )
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(
                self.diagnostic_icon_button(
                    AppIcon::ArrowUp,
                    InputEditorAction::PreviousDiagnostic,
                ),
            )
            .with_child(
                self.diagnostic_icon_button(AppIcon::ArrowDown, InputEditorAction::NextDiagnostic),
            )
            .with_child(
                self.diagnostic_icon_button(AppIcon::XClose, InputEditorAction::CloseDiagnostic),
            )
            .finish();
        let message = format!(
            "{}  {}",
            resolved.diagnostic.message,
            Self::diagnostic_label(resolved)
        );
        let panel = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(header)
                    .with_padding_left(12.0)
                    .with_padding_right(6.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .with_border(Border::bottom(1.0).with_border_color(self.theme.accent))
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(message, self.ui_font, crate::theme::text_size(12.0))
                        .with_color(self.theme.main_text())
                        .soft_wrap(true)
                        .finish(),
                )
                .with_padding_left(16.0)
                .with_padding_right(16.0)
                .with_padding_top(10.0)
                .with_padding_bottom(11.0)
                .finish(),
            )
            .finish();
        ConstrainedBox::new(
            Container::new(panel)
                .with_background_color(self.theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(self.theme.accent))
                .finish(),
        )
        .with_width((self.available_width.get() - 12.0).clamp(320.0, 1000.0))
        .finish()
    }

    pub(crate) fn insert_char(&mut self, c: char, ctx: &mut ViewContext<Self>) {
        let edit_started = std::time::Instant::now();
        if let Some((start, end)) = self.active_selection_range() {
            // Typing a bracket/quote opener over a non-empty selection wraps
            // it (`foo` selected, type `(` -> `(foo)`, selection preserved
            // around `foo`) instead of deleting the selection and inserting
            // the opener alone — the same "surround selection" VS Code
            // both do. A closer (`)`, not in `matching_close_bracket`'s
            // domain) still just replaces the selection, like plain typing.
            if start != end {
                if let Some(close) = matching_close_bracket(c) {
                    self.surround_selection(start, end, c, close, ctx);
                    return;
                }
            }
            self.delete_range(start, end, ctx);
        } else {
            self.snapshot_undo();
        }
        // Typing while the cursor sits inside a chip's interior resumes right
        // after the chip instead of splitting it.
        self.cursor = self.cursor.min(self.buffer.chars().count());
        self.cursor = self.clamp_off_chip(self.cursor, true);
        let edit_at = self.cursor;

        // Smart dedent: a closing delimiter typed on a blank line aligns with
        // the indentation of the line containing its matching opener (VS
        // Code behavior) — same transaction as the character insert.
        // Only this rare closer/JSX path builds a char vec; ordinary typing
        // edits the String in place so a keystroke never rebuilds the buffer.
        if self.is_source_editor()
            && !self.document_capabilities.large_file
            && (matches!(c, '}' | ')' | ']')
                || (c == '>'
                    && matches!(
                        self.source_language,
                        Language::TypeScript | Language::JavaScript | Language::Html
                    )))
        {
            let chars = self.char_vec();
            let line_start = line_start(&chars, self.cursor);
            // Brace closers only dedent on a blank line; a JSX closing tag
            // dedents once `</` has been typed (the helper validates the
            // pattern itself).
            let eligible = if c == '>' {
                true
            } else {
                chars[line_start..self.cursor]
                    .iter()
                    .all(|character| character.is_whitespace())
            };
            if eligible {
                let indent = if c == '>' {
                    crate::workspace::smart_indent::jsx_closing_tag_indent(&chars, self.cursor)
                } else {
                    crate::workspace::smart_indent::closer_line_indent(
                        &chars,
                        self.cursor,
                        c,
                        self.source_language,
                    )
                };
                if let Some(new_indent) = indent {
                    let new_indent_len = new_indent.chars().count();
                    let prefix_end = line_start
                        + chars[line_start..self.cursor]
                            .iter()
                            .take_while(|character| character.is_whitespace())
                            .count();
                    if prefix_end - line_start != new_indent_len {
                        let byte_start = byte_index_for_char(&self.buffer, line_start);
                        let byte_end = byte_index_for_char(&self.buffer, prefix_end);
                        self.buffer.replace_range(byte_start..byte_end, &new_indent);
                        self.cursor = line_start + new_indent_len;
                    }
                }
            }
        }

        let byte_at = byte_index_for_char(&self.buffer, self.cursor);
        self.buffer.insert(byte_at, c);
        self.cursor += 1;
        let mut inserted_len = 1isize;
        if let Some(close) = matching_close_bracket(c) {
            let after = byte_at + c.len_utf8();
            if !self.buffer[after..].starts_with(close) {
                self.buffer.insert(after, close);
                inserted_len += 1;
            }
        }
        self.invalidate_buffer_content();
        self.rebase_chips(edit_at, inserted_len);
        if self.is_agent_mode && c == '@' {
            let chars_now = self.char_vec();
            let is_boundary = edit_at == 0
                || chars_now
                    .get(edit_at - 1)
                    .map(|pc| pc.is_whitespace())
                    .unwrap_or(true);
            if is_boundary {
                self.active_mention = Some(edit_at);
            }
        }
        self.apply_edit(ctx);
        let elapsed = edit_started.elapsed();
        if elapsed > std::time::Duration::from_millis(50) {
            log::warn!(
                "[editor-perf] slow character insert: {:?} buffer_bytes={} cursor={}",
                elapsed,
                self.buffer.len(),
                self.cursor
            );
        }
    }

    /// Wrap `[start, end)` in `open`/`close` (Cmd+/Ctrl+ nothing — this is
    /// what typing a bracket/quote opener over a selection does instead of
    /// replacing it). The selection is preserved around the original text,
    /// now sitting one character further in — matching VS Code exactly.
    fn surround_selection(
        &mut self,
        start: usize,
        end: usize,
        open: char,
        close: char,
        ctx: &mut ViewContext<Self>,
    ) {
        self.snapshot_undo();
        let open_byte = byte_index_for_char(&self.buffer, start);
        self.buffer.insert(open_byte, open);
        self.rebase_chips(start, 1);
        let close_byte = byte_index_for_char(&self.buffer, end + 1);
        self.buffer.insert(close_byte, close);
        self.rebase_chips(end + 1, 1);
        self.invalidate_buffer_content();
        self.selection_anchor = Some(start + 1);
        self.cursor = end + 1;
        self.apply_edit(ctx);
    }

    pub(crate) fn cursor_element(&self) -> Box<dyn Element> {
        use crate::console::editor_settings::{CursorStyle, EditorSettings};
        let style = self
            .cursor_style_override
            .unwrap_or_else(|| EditorSettings::global().terminal.cursor_style);
        let h = self.font_size() + 2.0;
        match style {
            CursorStyle::Bar => Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(2.0)
                    .with_height(h)
                    .finish(),
            )
            .with_background_color(self.cursor_color)
            .finish(),
            CursorStyle::Block => {
                // Semi-transparent block the width of one cell.
                let mut c = self.cursor_color;
                c.a = 170;
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width((self.font_size() * 0.6).max(4.0))
                        .with_height(h)
                        .finish(),
                )
                .with_background_color(c)
                .finish()
            }
            CursorStyle::Underline => Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width((self.font_size() * 0.6).max(4.0))
                    .with_height(h)
                    .finish(),
            )
            .with_border(Border::bottom(2.0).with_border_color(self.cursor_color))
            .finish(),
        }
    }

    /// Avatar-headed collaborator caret: a compact circular head with a thin
    /// stem dropping to the participant's insertion point.
    pub(crate) fn remote_caret_element(&self) -> Option<Box<dyn Element>> {
        let identity = self.remote_caret.as_ref()?;
        let avatar = crate::workspace::share_sheet::avatar_sized(
            &identity.display_name,
            "",
            &identity.avatar_url,
            &self.theme,
            self.ui_font,
            16.0,
        );
        let stem = Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(2.0)
                .with_height(self.font_size() + 3.0)
                .finish(),
        )
        .with_background_color(self.theme.accent)
        .finish();
        Some(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(1.0)
                .with_child(avatar)
                .with_child(stem)
                .finish(),
        )
    }
}
