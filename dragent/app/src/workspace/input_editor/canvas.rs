//! Editor canvas renderers (empty/plain/command/large).
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
    pub(crate) fn render_empty_state(&self, cx: &AppContext) -> Box<dyn Element> {
        let placeholder_el = Box::new(
            Text::new(self.placeholder.clone(), self.ui_font, self.font_size())
                .with_style(Properties::default().weight(self.font_weight))
                .with_line_height_ratio(self.line_height_ratio())
                .with_color(self.placeholder_color),
        );
        let multiline = matches!(self.mode, InputMode::MultiLine { .. });
        let canvas = if (self.remote_caret.is_some() || self.focus_handle.is_focused(cx))
            && multiline
        {
            // A multi-line box is often taller than its text. The caret sits
            // on the first line, where typing starts, not the box's middle
            // (or between the lines of a wrapped placeholder).
            let placeholder_el = Container::new(placeholder_el)
                .with_padding_left(4.0)
                .finish();
            let first_line = ConstrainedBox::new(Empty::new().finish())
                .with_height(self.font_size() * self.line_height_ratio())
                .finish();
            let caret =
                self.line_with_overlays(first_line, "", 0, Some(0), None, false, cx, false, false);
            Stack::new()
                .with_child(placeholder_el)
                .with_positioned_child(
                    caret,
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::TopLeft,
                        ChildAnchor::TopLeft,
                    ),
                )
                .finish()
        } else if self.remote_caret.is_some() || self.focus_handle.is_focused(cx) {
            // Empty inputs used a separate stack that put the caret on top of
            // the placeholder and at a different vertical anchor from typed
            // text. Share the normal line overlay so both states line up.
            let placeholder_el = Container::new(placeholder_el)
                .with_padding_left(4.0)
                .finish();
            self.line_with_overlays(
                placeholder_el,
                "",
                0,
                Some(0),
                None,
                false,
                cx,
                false,
                false,
            )
        } else {
            placeholder_el
        };
        if multiline {
            // Keep the text at its own height at the top of a taller box;
            // stretched, the line's overlays would centre in the box.
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(canvas)
                .finish()
        } else if self.mode == InputMode::TerminalCommand {
            canvas
        } else {
            self.single_line_canvas(canvas)
        }
    }

    /// A one-line input is intrinsically one text line tall. Inheriting a
    /// taller minimum from its parent makes the scroll viewport grow while
    /// the glyphs stay at its top, separating them from the centered caret.
    fn single_line_canvas(&self, canvas: Box<dyn Element>) -> Box<dyn Element> {
        ConstrainedBox::new(canvas)
            .with_height(self.font_size() * self.line_height_ratio())
            .finish()
    }

    pub(crate) fn render_plain_canvas(&self, app: &AppContext) -> Box<dyn Element> {
        let display = self.display_text();
        let text_el = Box::new(
            Text::new(display.clone(), self.ui_font, self.font_size())
                .with_style(Properties::default().weight(self.font_weight))
                .with_line_height_ratio(self.line_height_ratio())
                .with_color(self.text_color),
        );
        // Render selection highlight inside the scrollable, but omit the cursor (None).
        // The cursor is drawn outside the ClippedScrollable below so it isn't
        // clipped by the text line's tight vertical clip region.
        let content =
            self.line_with_overlays(text_el, &display, 0, None, None, false, app, false, false);
        let content_padded = Container::new(content)
            .with_padding_right(PLAIN_CANVAS_RIGHT_PADDING)
            .finish();
        let scrollable = ClippedScrollable::horizontal(
            self.scroll_state.clone(),
            content_padded,
            ScrollbarWidth::None,
            ColorU::transparent_black().into(),
            ColorU::transparent_black().into(),
            ColorU::transparent_black().into(),
        )
        .with_overlayed_scrollbar()
        .finish();

        // Place the cursor on a Stack layer outside the ClippedScrollable.
        // cursor_x is the absolute pixel offset within the full (unscrolled) content.
        // We subtract the CLAMPED scroll (matching what ClippedScrollable::after_layout
        // will enforce in paint()) to get the correct viewport-relative x. Using the raw
        // scroll_start() would lag by up to one frame whenever after_layout reduces the
        // scroll (e.g. after a delete shrinks the content, or after scroll overshoots
        // the content width while typing).
        let canvas = if self.remote_caret.is_some()
            || (self.focus_handle.is_focused(app) && self.cursor_visible)
        {
            let raw_scroll = self.scroll_state.scroll_start().as_f32();
            let viewport_w = self.scroll_state.viewport_width().max(1.0);
            let text_w = caret_x_plain(
                &display,
                display.chars().count(),
                self.ui_font,
                self.font_size(),
                app,
            );
            let max_scroll = (text_w + PLAIN_CANVAS_RIGHT_PADDING - viewport_w).max(0.0);
            let scroll_x = raw_scroll.min(max_scroll);
            let cursor_x = self.caret_x_on_line(&display, self.cursor, app, false) - scroll_x;
            // Anchor the local caret to the actual laid-out line midpoint.
            // This stays correct for any font size or line-height ratio.
            let cursor_offset = OffsetPositioning::offset_from_parent(
                vec2f(cursor_x, 0.0),
                ParentOffsetBounds::Unbounded,
                if self.remote_caret.is_some() {
                    ParentAnchor::BottomLeft
                } else {
                    ParentAnchor::MiddleLeft
                },
                if self.remote_caret.is_some() {
                    ChildAnchor::BottomMiddle
                } else {
                    ChildAnchor::MiddleLeft
                },
            );
            let cursor = self
                .remote_caret_element()
                .unwrap_or_else(|| self.cursor_element());
            Stack::new()
                .with_child(scrollable)
                .with_positioned_child(cursor, cursor_offset)
                .finish()
        } else {
            scrollable
        };
        self.single_line_canvas(canvas)
    }

    ///  editor canvas: syntax-highlighted lines, ghost suffix, click-to-place caret.
    /// A diff line's gutter with the comment +. It shows while the pointer
    /// is anywhere on the line (`line` is `(number, deleted)`), and the
    /// gutter's own hover state is kept per line so it survives redraws.
    fn render_diff_comment_gutter(&self, line: (usize, bool), gutter: Box<dyn Element>) -> Box<dyn Element> {
        let theme = self.theme;
        let height = self.line_height();
        let line_hovered = self.diff_comment_hover_line == Some(line);
        let hover = {
            let mut handles = self.diff_comment_hover.borrow_mut();
            if handles.len() > 4096 {
                handles.clear();
            }
            handles
                .entry(format!("{}:{}", line.0, line.1))
                .or_default()
                .clone()
        };
        Hoverable::new(hover, move |state| {
            if !state.is_hovered() && !line_hovered {
                return gutter;
            }
            let badge = Container::new(render_icon(
                AppIcon::Plus,
                theme.background,
                crate::theme::scaled(10.0),
            ))
            .with_uniform_padding(2.0)
            .with_background_color(theme.accent)
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(4.0),
            ))
            .finish();
            Stack::new()
                .with_child(gutter)
                .with_positioned_child(
                    ConstrainedBox::new(Align::new(badge).finish())
                        .with_width(20.0)
                        .with_height(height)
                        .finish(),
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::ParentByPosition,
                        ParentAnchor::MiddleRight,
                        ChildAnchor::MiddleRight,
                    ),
                )
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    pub(crate) fn render_command_canvas(&self, app: &AppContext) -> Box<dyn Element> {
        if self.is_source_editor() && self.document_capabilities.large_file {
            return self.render_large_source_canvas(app);
        }
        // Use the width cached by the most recent event handler (which has ViewContext).
        // Falls back to 10000.0 (no wrap) until the first interaction.
        let available_width = self.layout_width.get();
        // MultiLine mode never syntax-highlights; TerminalCommand follows agent/cmd heuristic.
        // Exception: in the AI-tool input (highlighting normally suppressed → white
        // text), a `!`-prefixed line IS a scratch-shell command, so restore the
        // command colors for it.
        let bang_override =
            self.suppress_command_highlight && self.buffer.trim_start().starts_with('!');
        let is_cmd = matches!(self.mode, InputMode::TerminalCommand)
            && !self.is_agent_mode
            && (bang_override
                || (!self.suppress_command_highlight && is_likely_command(&self.buffer)));
        let is_source_editor =
            self.source_path.is_some() && matches!(self.mode, InputMode::MultiLine { .. });
        let max_height = match &self.mode {
            InputMode::MultiLine { max_height, .. } => *max_height as f32,
            _ => INPUT_MAX_HEIGHT,
        };

        let visual_lines = self.visual_line_map(available_width, app.font_cache());
        let git_row_width = visual_lines
            .iter()
            .map(|line| line.layout.width + line.layout.trailing_whitespace_width)
            .fold(0.0_f32, f32::max)
            .max(
                (available_width - self.source_gutter_width() - EDITOR_CONTENT_PADDING_LEFT)
                    .max(1.0),
            )
            + SOURCE_EDITOR_RIGHT_PADDING;
        let git_diff_rows = self
            .git_diff_projection
            .as_ref()
            .map(|projection| projection.rows.as_slice());
        let fold_index =
            (is_source_editor && git_diff_rows.is_none() && self.document_capabilities.folding)
                .then(|| self.fold_index());
        let visible_visual_indices = fold_index
            .as_ref()
            .map(|index| self.visible_visual_indices(&visual_lines, index, available_width));
        let total = git_diff_rows.map_or_else(
            || {
                visible_visual_indices
                    .as_ref()
                    .map_or(visual_lines.len(), |indices| indices.len())
            },
            |rows| rows.len(),
        );
        let line_height = self.line_height();
        // One gutter width for the whole editor, derived from the final line
        // number. Paint, viewport, hit-testing and content offsets all share
        // these so a wide lane (e.g. line 1100) never disagrees anywhere.
        let gutter_max_line = self.source_max_line();
        let gutter_width = if crate::console::editor_settings::EditorSettings::global()
            .editor
            .line_numbers
        {
            editor_render::gutter_width_for_lines(gutter_max_line, self.font_size())
        } else {
            editor_render::gutter_width_for_lines(gutter_max_line, self.font_size())
                - editor_render::gutter_number_width(gutter_max_line, self.font_size())
        };
        let fold_gutter_hovered = self
            .fold_gutter_hover
            .lock()
            .map(|state| state.is_hovered())
            .unwrap_or(false);
        let render_range = if matches!(self.mode, InputMode::MultiLine { .. }) {
            let measured_height = self.available_height.get();
            let viewport_height = if measured_height.is_finite() && measured_height > 0.0 {
                measured_height.min(max_height)
            } else {
                max_height
            };
            multiline_render_range(
                total,
                self.scroll_state.scroll_start().as_f32(),
                viewport_height,
                line_height,
            )
        } else {
            0..total
        };
        let render_start = render_range.start;
        let render_end = render_range.end;
        let source_cursor_line =
            is_source_editor.then(|| source_line_number(&self.char_vec(), self.cursor));
        // Folding and guides project the same parsed ranges. Indentation only
        // chooses a range's horizontal column; it never invents a guide.
        // Guides render only when enabled in settings (Editor → Indent guides).
        let source_indent_guides = (is_source_editor
            && git_diff_rows.is_none()
            && self.document_capabilities.indentation_guides
            && crate::console::editor_settings::EditorSettings::global()
                .editor
                .indent_guides)
            .then(|| {
                fold_index
                    .as_ref()
                    .map(|index| {
                        index
                            .regions()
                            .filter_map(|region| {
                                region.guide_line_range().map(|range| {
                                    let start = *range.start();
                                    let end = *range.end();
                                    (region.clone(), start, end)
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let active_indent_region: Option<FoldRegion> = source_cursor_line.and_then(|cursor_line| {
            fold_index
                .as_ref()
                .and_then(|index| index.innermost_region_at(cursor_line).cloned())
        });
        let source_display_grid = is_source_editor.then(|| {
            if source_indent_guides.is_empty() {
                SourceDisplayGrid {
                    source_rows: Vec::new(),
                    total_rows: total,
                }
            } else {
                SourceDisplayGrid::build(
                    &visual_lines,
                    visible_visual_indices
                        .as_ref()
                        .map(|indices| indices.as_slice()),
                )
            }
        });
        let mut lines_col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        let mut fixed_gutter_col = is_source_editor.then(Flex::column);
        let comment_states = (is_source_editor && self.document_capabilities.syntax_highlighting)
            .then(|| self.lexical_line_start_states());

        if render_start > 0 {
            let spacer_height = render_start as f32 * line_height;
            lines_col = lines_col.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(spacer_height)
                    .finish(),
            );
        }

        for display_index in render_start..render_end {
            let projected_row = git_diff_rows.and_then(|rows| rows.get(display_index));
            if let Some(GitDiffDisplayRow::Collapsed(region)) = projected_row {
                let id = region.id;
                let label = format!("{} hidden lines", region.line_count);
                // Diff controls are part of the source editor, so follow its
                // configured font size instead of using a separately scaled
                // hard-coded UI size.
                let diff_control_size = self.font_size();
                let collapsed = Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Container::new(Empty::new().finish())
                                .with_background_color(self.theme.outline())
                                .finish(),
                        )
                        .with_child(
                            Text::new(label, self.menu_font, diff_control_size)
                                .with_color(self.theme.sub_text())
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(crate::theme::scaled(8.0))
                .with_background_color(self.theme.background)
                .with_border(Border::top(1.0).with_border_color(self.theme.outline()))
                .finish();
                lines_col = lines_col.with_child(
                    ConstrainedBox::new(collapsed)
                        .with_width(git_row_width)
                        .with_height(line_height)
                        .finish(),
                );
                if let Some(gutter_col) = fixed_gutter_col.take() {
                    let reveal_button = |icon, direction| {
                        EventHandler::new(
                            Container::new(render_icon(
                                icon,
                                self.theme.sub_text(),
                                diff_control_size,
                            ))
                            .with_padding_left(crate::theme::scaled(4.0))
                            .with_padding_right(crate::theme::scaled(4.0))
                            .with_padding_top(crate::theme::scaled(3.0))
                            .with_padding_bottom(crate::theme::scaled(3.0))
                            .finish(),
                        )
                        .on_left_mouse_down(move |event_ctx, _app, _position| {
                            event_ctx.dispatch_typed_action(
                                InputEditorAction::ExpandGitDiffRegion { id, direction },
                            );
                            DispatchEventResult::StopPropagation
                        })
                        .finish()
                    };
                    fixed_gutter_col = Some(
                        gutter_col.with_child(
                            ConstrainedBox::new(
                                Flex::row()
                                    .with_main_axis_size(MainAxisSize::Max)
                                    .with_main_axis_alignment(MainAxisAlignment::Center)
                                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                    .with_child(reveal_button(AppIcon::ArrowUp, -1))
                                    .with_child(reveal_button(AppIcon::ArrowDown, 1))
                                    .finish(),
                            )
                            .with_height(line_height)
                            .finish(),
                        ),
                    );
                }
                continue;
            }
            if let Some(GitDiffDisplayRow::Deleted { old_line, text }) = projected_row {
                let code = editor_render::highlighted_source(
                    text,
                    self.source_language,
                    &self.theme,
                    self.ui_font,
                    self.font_size(),
                    self.font_weight,
                    crate::console::editor_settings::EditorSettings::global()
                        .editor
                        .show_whitespace,
                )
                .soft_wrap(false)
                .with_line_height_ratio(self.line_height_ratio())
                .finish();
                let mut tint = self.theme.ui_red();
                tint.a = 24;
                lines_col = lines_col.with_child(
                    ConstrainedBox::new(Container::new(code).with_background_color(tint).finish())
                        .with_width(git_row_width)
                        .with_height(line_height)
                        .finish(),
                );
                if let Some(gutter_col) = fixed_gutter_col.take() {
                    let gutter = self.render_diff_comment_gutter((*old_line, true), editor_render::git_diff_gutter(
                        Some(*old_line),
                        None,
                        DiffLineKind::Removed,
                        &self.theme,
                        self.ui_font,
                        self.font_size(),
                    ));
                    fixed_gutter_col = Some(
                        gutter_col.with_child(
                            ConstrainedBox::new(gutter)
                                .with_height(line_height)
                                .finish(),
                        ),
                    );
                }
                continue;
            }
            let projected_line = projected_row.and_then(|row| match row {
                GitDiffDisplayRow::Current {
                    old_line,
                    line,
                    kind,
                } => Some((*old_line, *line, *kind)),
                _ => None,
            });
            let vl_idx = if let Some((_, line, _)) = projected_line {
                let direct = line.saturating_sub(1);
                if visual_lines
                    .get(direct)
                    .is_some_and(|visual| visual.source_line == line && !visual.continuation)
                {
                    direct
                } else {
                    visual_lines
                        .iter()
                        .position(|visual| visual.source_line == line && !visual.continuation)
                        .unwrap_or(direct.min(visual_lines.len().saturating_sub(1)))
                }
            } else {
                visible_visual_indices
                    .as_ref()
                    .map_or(display_index, |indices| indices[display_index])
            };
            let visual_line = &visual_lines[vl_idx];
            let mut folded_text = None;
            if is_source_editor
                && !visual_line.continuation
                && fold_index.as_ref().is_some_and(|index| {
                    self.fold_state.is_collapsed(visual_line.source_line, index)
                })
            {
                if let Some(region) = fold_index
                    .as_ref()
                    .and_then(|index| index.region_starting_at(visual_line.source_line))
                {
                    folded_text = region.folded_line(&self.buffer);
                }
            }
            let visual_text = folded_text.as_deref().unwrap_or(&visual_line.text);
            let abs_offset = visual_line.abs_char_offset;
            let is_last = vl_idx + 1 == visual_lines.len();
            let mut highlight_state = comment_states
                .as_ref()
                .and_then(|states| states.get(visual_line.source_line.saturating_sub(1)))
                .copied()
                .unwrap_or_default();
            let syntax_spans = (is_source_editor && self.document_capabilities.syntax_highlighting)
                .then(|| {
                    self.syntax_spans_for_row(visual_line.source_line, abs_offset, visual_text)
                })
                .flatten();
            let mut text_el = if let Some(spans) = syntax_spans {
                editor_render::highlighted_source_from_spans(
                    visual_text,
                    &spans,
                    &self.theme,
                    self.ui_font,
                    self.font_size(),
                    self.font_weight,
                    crate::console::editor_settings::EditorSettings::global()
                        .editor
                        .show_whitespace,
                )
            } else if is_source_editor && self.document_capabilities.syntax_highlighting {
                editor_render::highlighted_source_with_state(
                    visual_text,
                    self.source_language,
                    &mut highlight_state,
                    &self.theme,
                    self.ui_font,
                    self.font_size(),
                    self.font_weight,
                    crate::console::editor_settings::EditorSettings::global()
                        .editor
                        .show_whitespace,
                )
            } else if is_source_editor {
                Text::new(
                    if crate::console::editor_settings::EditorSettings::global()
                        .editor
                        .show_whitespace
                    {
                        crate::workspace::editor_render::show_whitespace_dots(visual_text)
                    } else {
                        visual_text.to_string()
                    },
                    self.ui_font,
                    self.font_size(),
                )
                .with_color(self.theme.main_text())
            } else {
                build_highlighted_command(
                    visual_text,
                    &self.theme,
                    self.ui_font,
                    self.font_size(),
                    is_cmd,
                )
                .with_style(Properties::default().weight(self.font_weight))
            }
            .soft_wrap(false)
            .with_line_height_ratio(self.line_height_ratio());

            // A fold placeholder is presentation-only. The source buffer and
            // all character offsets continue to refer to the untouched text.
            let mut draw_ghost_arrow = false;
            if is_last {
                if let Some(ref ghost) = self.ghost_suffix {
                    let len = self.buffer.chars().count();
                    if self.cursor.min(len) == len {
                        text_el.add_text_with_highlights(
                            ghost.clone(),
                            self.placeholder_color,
                            warpui_core::fonts::Properties::default(),
                        );
                        draw_ghost_arrow = true;
                    }
                }
            }

            let line_cursor = {
                let lo = abs_offset;
                let is_on_line = if projected_line.is_some() {
                    self.cursor >= lo
                        && self.cursor <= lo.saturating_add(visual_line.text.chars().count())
                } else if vl_idx + 1 < visual_lines.len() {
                    let next_lo = visual_lines[vl_idx + 1].abs_char_offset;
                    self.cursor >= lo && self.cursor < next_lo
                } else {
                    self.cursor >= lo
                };
                if is_on_line {
                    Some(self.cursor - lo)
                } else {
                    None
                }
            };

            let code = self.line_with_overlays(
                Box::new(text_el),
                // Overlay coordinates remain source coordinates. Folded text
                // is a display transform and must never redefine selection,
                // diagnostic, search or cursor offsets.
                &visual_line.text,
                abs_offset,
                line_cursor,
                is_source_editor.then_some(visual_line.source_line),
                source_cursor_line == Some(visual_line.source_line)
                    && self.focus_handle.is_focused(app),
                app,
                is_cmd,
                draw_ghost_arrow,
            );
            let row = if is_source_editor {
                let foldable = !visual_line.continuation
                    && fold_index.as_ref().is_some_and(|index| {
                        index.region_starting_at(visual_line.source_line).is_some()
                    });
                let breakpoint = self
                    .source_path
                    .as_deref()
                    .and_then(|path| self.breakpoints.state(path, visual_line.source_line));
                if let Some(gutter_col) = fixed_gutter_col.take() {
                    let projected_change = projected_line.and_then(|(_, _, kind)| match kind {
                        DiffLineKind::Added => Some(LineChangeKind::Added),
                        DiffLineKind::Context => None,
                        DiffLineKind::Removed => Some(LineChangeKind::Deleted),
                    });
                    let fixed_gutter = if let Some((old_line, new_line, kind)) = projected_line {
                        self.render_diff_comment_gutter((new_line, matches!(kind, DiffLineKind::Removed)), editor_render::git_diff_gutter(
                            old_line,
                            Some(new_line),
                            kind,
                            &self.theme,
                            self.ui_font,
                            self.font_size(),
                        ))
                    } else {
                        editor_render::gutter(
                            crate::console::editor_settings::EditorSettings::global()
                                .editor
                                .line_numbers,
                            visual_line.source_line,
                            gutter_max_line,
                            visual_line.continuation,
                            projected_change.or_else(|| {
                                self.source_line_changes
                                    .get(&visual_line.source_line)
                                    .copied()
                            }),
                            self.source_diagnostic_lines
                                .get(&visual_line.source_line)
                                .copied(),
                            breakpoint,
                            foldable,
                            fold_index.as_ref().is_some_and(|index| {
                                self.fold_state.is_collapsed(visual_line.source_line, index)
                            }),
                            fold_gutter_hovered,
                            source_cursor_line == Some(visual_line.source_line)
                                && self.focus_handle.is_focused(app),
                            &self.theme,
                            self.ui_font,
                            self.font_size(),
                        )
                    };
                    fixed_gutter_col = Some(
                        gutter_col.with_child(
                            ConstrainedBox::new(fixed_gutter)
                                .with_height(line_height)
                                .finish(),
                        ),
                    );
                }
                if projected_line.is_some_and(|(_, _, kind)| kind == DiffLineKind::Added) {
                    let mut tint = self.theme.ui_green();
                    tint.a = 24;
                    ConstrainedBox::new(Container::new(code).with_background_color(tint).finish())
                        .with_width(git_row_width)
                        .finish()
                } else {
                    code
                }
            } else {
                code
            };
            lines_col =
                lines_col.with_child(ConstrainedBox::new(row).with_height(line_height).finish());
        }

        if render_end < total {
            let spacer_height = (total - render_end) as f32 * line_height;
            lines_col = lines_col.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(spacer_height)
                    .finish(),
            );
        }

        let lines = lines_col.finish();
        let lines: Box<dyn Element> = if is_source_editor {
            let display_grid = source_display_grid
                .as_ref()
                .expect("source editor always has a display grid");

            let mut paints = Vec::new();
            // A guide sits exactly where its opening line's code starts, so
            // measure that line's leading whitespace with the same text layout
            // that draws it. Multiplying a column count by the space width
            // assumed a tab is 4 columns, but a tab renders at the font's own
            // advance — so tab-indented code (Go) drifted a column right per
            // nesting level while space-indented code looked fine.
            let source_lines: Vec<&str> = if source_indent_guides.is_empty() {
                Vec::new()
            } else {
                self.buffer.split('\n').collect()
            };
            let space_width = self.caret_x_on_line(" ", 1, app, true);
            let mut indent_x_by_prefix: HashMap<&str, f32> = HashMap::new();
            let row_tab_width = super::text::source_tab_size();
            for (guide, guide_start, guide_end) in &source_indent_guides {
                let opener = source_lines.get(guide.start_line.saturating_sub(1));
                // The guide's column must be measured exactly like each row's
                // first-content column (same function, same tab width), or a
                // tab-indented file hides guides whenever the Tab size setting
                // differs from the fold index's 4-column default.
                let guide_column = opener
                    .and_then(|line| first_source_content_column(line, row_tab_width))
                    .unwrap_or(guide.indent_column);
                let x = match opener {
                    Some(line) => {
                        let text_start = line.len() - line.trim_start_matches([' ', '\t']).len();
                        let prefix = &line[..text_start];
                        *indent_x_by_prefix.entry(prefix).or_insert_with(|| {
                            self.caret_x_on_line(prefix, prefix.chars().count(), app, true)
                        })
                    }
                    None => space_width * guide.indent_column as f32,
                };
                let is_active = active_indent_region.as_ref() == Some(guide);
                let color = self.indent_guide_color(is_active);
                for (start_row, end_row) in
                    display_grid.visible_guide_runs(*guide_start, *guide_end, guide_column)
                {
                    paints.push(IndentGuidePaint {
                        x,
                        y: start_row as f32 * line_height,
                        // Every visible segment owns the complete row box, so
                        // adjacent unobstructed rows meet without seams even
                        // with a comfortable source line height.
                        height: (end_row.saturating_sub(start_row) + 1) as f32 * line_height,
                        color,
                    });
                }
            }
            let guide_layer = IndentGuideLayer {
                paints,
                size: vec2f(1.0, display_grid.total_rows as f32 * line_height),
                origin: None,
            }
            .finish();

            // Paint guides before rows and glyphs. Guides are treated as part of
            // the editor background layer; painting them as an overlay made the
            // lines visually sit on top of code and escape normal stack clips.
            Stack::new()
                .with_positioned_child(
                    guide_layer,
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::TopLeft,
                        ChildAnchor::TopLeft,
                    ),
                )
                .with_child(lines)
                .finish()
        } else {
            lines
        };

        let nonactive_thumb = Fill::Gradient {
            start: vec2f(0.0, 0.0),
            end: vec2f(0.0, 1.0),
            start_color: self.theme.accent_overlay(),
            end_color: self.theme.accent,
        };
        let active_thumb = Fill::Gradient {
            start: vec2f(0.0, 0.0),
            end: vec2f(0.0, 1.0),
            start_color: self.theme.accent,
            end_color: self.theme.accent_overlay(),
        };
        let scrollbar_track: Fill = warpui::color::ColorU::new(255, 255, 255, 18).into();

        let scrollable: Box<dyn Element> = if is_source_editor {
            // Keep the content extent stable while vertically virtualising.
            // Otherwise a long line entering/leaving overscan would resize the
            // horizontal scrollbar and make the viewport jump.
            let content_width = visual_lines
                .iter()
                .map(|line| line.layout.width + line.layout.trailing_whitespace_width)
                .fold(0.0_f32, f32::max)
                + SOURCE_EDITOR_RIGHT_PADDING;
            let content = ConstrainedBox::new(
                Container::new(lines)
                    .with_padding_bottom(self.source_editor_bottom_padding())
                    .finish(),
            )
            .with_min_width(content_width)
            .finish();
            if self.external_horizontal_scroll_state.is_some() {
                NewScrollable::vertical(
                    SingleAxisConfig::Clipped {
                        handle: self.scroll_state.clone(),
                        child: content,
                    },
                    nonactive_thumb,
                    active_thumb,
                    scrollbar_track,
                )
                .with_vertical_scrollbar(ScrollableAppearance::new(
                    ScrollbarWidth::Custom(6.0),
                    true,
                ))
                .with_scrollbar_padding(0.0, 0.0)
                .finish()
            } else {
                NewScrollable::horizontal_and_vertical(
                    DualAxisConfig::Clipped {
                        horizontal: ClippedAxisConfiguration {
                            handle: self.effective_horizontal_scroll_state().clone(),
                            max_size: None,
                            stretch_child: false,
                        },
                        vertical: ClippedAxisConfiguration {
                            handle: self.scroll_state.clone(),
                            max_size: None,
                            stretch_child: false,
                        },
                        child: content,
                    },
                    nonactive_thumb,
                    active_thumb,
                    scrollbar_track,
                )
                .with_horizontal_scrollbar(ScrollableAppearance::new(
                    ScrollbarWidth::Custom(6.0),
                    true,
                ))
                .with_vertical_scrollbar(ScrollableAppearance::new(
                    ScrollbarWidth::Custom(6.0),
                    true,
                ))
                .with_scrollbar_padding(0.0, 0.0)
                .with_wheel_locked_to_dominant_axis(true)
                .finish()
            }
        } else {
            ClippedScrollable::vertical(
                self.scroll_state.clone(),
                lines,
                ScrollbarWidth::Custom(6.0),
                nonactive_thumb,
                active_thumb,
                scrollbar_track,
            )
            .with_overlayed_scrollbar()
            .finish()
        };

        let scrollable = EventHandler::new(scrollable)
            .with_always_handle()
            .on_scroll_wheel(|event_ctx, _app, _position, delta, _precise, _modifiers| {
                event_ctx.dispatch_typed_action(InputEditorAction::ScrollWheel(delta.y()));
                DispatchEventResult::PropagateToParent
            })
            .finish();

        // One element owns the folding-column hover state. Reusing the same
        // MouseStateHandle across every row lets each non-hovered row clear the
        // state, which made the chevrons disappear. The fixed overlay also lets
        // hovering an ordinary row reveal every fold point in the viewport.
        let scrollable = if is_source_editor {
            let lane_height = self.available_height.get().min(max_height).max(line_height);
            let gutter_offset_y = render_start as f32 * line_height
                - self.scroll_state.scroll_start().as_f32().max(0.0);
            // The gutter is painted as a fixed overlay so it stays visible
            // during horizontal scrolling. It owns hover, clicks and wheel in
            // ONE layer: Hoverable paints the unified gutter row
            // (breakpoint | number | debug) and the outer EventHandler routes
            // clicks into the existing gutter action path. Wheel deltas drive
            // `scroll_state` directly — dispatching `ScrollWheel` can never
            // scroll a source editor (the action early-returns), which is why
            // the gutter used to be a scroll dead-zone.
            let gutter_max_scroll =
                (total as f32 * line_height + self.source_editor_bottom_padding() - lane_height)
                    .max(0.0);
            let gutter_wheel_scroll = self.scroll_state.clone();
            let scroll_gutter_by_wheel =
                move |event_ctx: &mut EventContext,
                      _app: &AppContext,
                      _position: &Vector2F,
                      delta: &Vector2F,
                      precise: bool,
                      _modifiers: &ModifiersState| {
                    let mut pixels = delta.y();
                    if !precise {
                        pixels *= 40.0;
                    }
                    let current = gutter_wheel_scroll.scroll_start().as_f32().max(0.0);
                    let next = (current - pixels).clamp(0.0, gutter_max_scroll);
                    if (next - current).abs() > f32::EPSILON {
                        gutter_wheel_scroll.scroll_to(next.into_pixels());
                        event_ctx.notify();
                        DispatchEventResult::StopPropagation
                    } else {
                        DispatchEventResult::PropagateToParent
                    }
                };
            let fixed_gutter_inner: Box<dyn Element> =
                Container::new(editor_render::fixed_gutter_viewport(
                    fixed_gutter_col
                        .expect("source editor always builds a fixed gutter")
                        .finish(),
                    gutter_offset_y,
                    gutter_width,
                    lane_height,
                ))
                .with_background_color(self.theme.background)
                .finish();
            // A Git projection has interactive reveal controls in the
            // painted gutter itself. The folding hover overlay would sit
            // above them and consume every press, making the arrows
            // appear inert — so only the plain gutter gets the hover layer.
            // Wheel still scrolls in both cases via the outer handler.
            let gutter_painted: Box<dyn Element> = if git_diff_rows.is_some() {
                fixed_gutter_inner
            } else {
                Hoverable::new(self.fold_gutter_hover.clone(), move |_| fixed_gutter_inner).finish()
            };
            let gutter_element = EventHandler::new(gutter_painted)
                .with_always_handle()
                .on_left_mouse_down(|event_ctx, _app, position| {
                    event_ctx.dispatch_typed_action(InputEditorAction::ClickAt(position));
                    DispatchEventResult::StopPropagation
                })
                .on_scroll_wheel(scroll_gutter_by_wheel)
                .finish();
            let gutter = ConstrainedBox::new(gutter_element)
                .with_width(gutter_width)
                .with_height(lane_height)
                .finish();

            // Fixed editor chrome and scrollable source content are siblings,
            // matching VS Code's `contentLeft` layout. Horizontal scrolling can
            // no longer consume a duplicate gutter or paint text underneath it.
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(
                    ConstrainedBox::new(gutter)
                        .with_width(gutter_width)
                        .with_height(lane_height)
                        .finish(),
                )
                .with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(EDITOR_CONTENT_PADDING_LEFT)
                        .with_height(lane_height)
                        .finish(),
                )
                .with_child(Expanded::new(1.0, scrollable).finish())
                .finish()
        } else {
            scrollable
        };

        SavePosition::new(
            ConstrainedBox::new(scrollable)
                .with_max_height(max_height)
                .finish(),
            &self.viewport_position_id(),
        )
        .finish()
    }

    /// Render a large source buffer without constructing a `VisualLine` and a
    /// shaped text layout for every physical line. The line index is compact,
    /// while this method materializes only the viewport plus overscan rows.
    pub(crate) fn render_large_source_canvas(&self, app: &AppContext) -> Box<dyn Element> {
        let available_width = self.layout_width.get();
        let max_height = match &self.mode {
            InputMode::MultiLine { max_height, .. } => *max_height as f32,
            _ => INPUT_MAX_HEIGHT,
        };
        let line_height = self.line_height();
        let index = self.large_source_index();
        let total = index.line_count();
        let viewport_height = {
            let measured = self.available_height.get();
            if measured.is_finite() && measured > 0.0 {
                measured.min(max_height)
            } else {
                max_height
            }
        };
        let range = multiline_render_range(
            total,
            self.scroll_state.scroll_start().as_f32(),
            viewport_height,
            line_height,
        );
        let render_start = range.start;
        let render_end = range.end;
        let source_cursor_line = index.line_for_char(self.cursor) + 1;
        let mut lines_col = Flex::column();
        let mut fixed_gutter_col = Flex::column();

        if render_start > 0 {
            lines_col = lines_col.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(render_start as f32 * line_height)
                    .finish(),
            );
        }

        for line_number in render_start..render_end {
            let Some(line) =
                self.large_source_visual_line(&index, line_number, app.font_cache(), false)
            else {
                continue;
            };
            // Keep the normal source-editor palette in large-file mode. The
            // renderer is already viewport based, so highlighting these few
            // visible rows does not require the whole document to be parsed
            // or laid out at once.
            let syntax_highlighted = true;
            let mut highlight_state = index
                .comment_states
                .get(line_number)
                .copied()
                .unwrap_or_default();
            let text_el = editor_render::highlighted_source_with_state(
                &line.text,
                self.source_language,
                &mut highlight_state,
                &self.theme,
                self.ui_font,
                self.font_size(),
                self.font_weight,
                crate::console::editor_settings::EditorSettings::global()
                    .editor
                    .show_whitespace,
            )
            .soft_wrap(false)
            .with_line_height_ratio(self.line_height_ratio());
            let line_end = index
                .line_range(&self.buffer, line_number)
                .map(|(_, _, length)| line.abs_char_offset + length)
                .unwrap_or(line.abs_char_offset);
            let line_cursor = (self.cursor >= line.abs_char_offset && self.cursor <= line_end)
                .then_some(self.cursor.saturating_sub(line.abs_char_offset));
            let code = self.line_with_overlays(
                Box::new(text_el),
                &line.text,
                line.abs_char_offset,
                line_cursor,
                Some(line.source_line),
                source_cursor_line == line.source_line && self.focus_handle.is_focused(app),
                app,
                syntax_highlighted,
                false,
            );
            lines_col =
                lines_col.with_child(ConstrainedBox::new(code).with_height(line_height).finish());

            let breakpoint = self
                .source_path
                .as_deref()
                .and_then(|path| self.breakpoints.state(path, line.source_line));
            fixed_gutter_col = fixed_gutter_col.with_child(
                ConstrainedBox::new(editor_render::gutter(
                    crate::console::editor_settings::EditorSettings::global()
                        .editor
                        .line_numbers,
                    line.source_line,
                    total,
                    false,
                    self.source_line_changes.get(&line.source_line).copied(),
                    self.source_diagnostic_lines.get(&line.source_line).copied(),
                    breakpoint,
                    false,
                    false,
                    false,
                    source_cursor_line == line.source_line && self.focus_handle.is_focused(app),
                    &self.theme,
                    self.ui_font,
                    self.font_size(),
                ))
                .with_height(line_height)
                .finish(),
            );
        }

        if render_end < total {
            let height = (total - render_end) as f32 * line_height;
            lines_col = lines_col.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(height)
                    .finish(),
            );
            fixed_gutter_col = fixed_gutter_col.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(height)
                    .finish(),
            );
        }

        let lines = lines_col.finish();
        let content_width = large_source_layout_char_extent(&index) as f32
            * (self.font_size() * 0.62)
            + SOURCE_EDITOR_RIGHT_PADDING;
        let content = ConstrainedBox::new(
            Container::new(lines)
                .with_padding_bottom(self.source_editor_bottom_padding())
                .finish(),
        )
        .with_min_width(content_width.max(available_width));
        let nonactive_thumb = Fill::Gradient {
            start: vec2f(0.0, 0.0),
            end: vec2f(0.0, 1.0),
            start_color: self.theme.accent_overlay(),
            end_color: self.theme.accent,
        };
        let active_thumb = Fill::Gradient {
            start: vec2f(0.0, 0.0),
            end: vec2f(0.0, 1.0),
            start_color: self.theme.accent,
            end_color: self.theme.accent_overlay(),
        };
        let scrollbar_track: Fill = warpui::color::ColorU::new(255, 255, 255, 18).into();
        let content = content.finish();
        let scrollable = if self.external_horizontal_scroll_state.is_some() {
            NewScrollable::vertical(
                SingleAxisConfig::Clipped {
                    handle: self.scroll_state.clone(),
                    child: content,
                },
                nonactive_thumb,
                active_thumb,
                scrollbar_track,
            )
            .with_vertical_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
            .with_scrollbar_padding(0.0, 0.0)
            .finish()
        } else {
            NewScrollable::horizontal_and_vertical(
                DualAxisConfig::Clipped {
                    horizontal: ClippedAxisConfiguration {
                        handle: self.effective_horizontal_scroll_state().clone(),
                        max_size: None,
                        stretch_child: false,
                    },
                    vertical: ClippedAxisConfiguration {
                        handle: self.scroll_state.clone(),
                        max_size: None,
                        stretch_child: false,
                    },
                    child: content,
                },
                nonactive_thumb,
                active_thumb,
                scrollbar_track,
            )
            .with_horizontal_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
            .with_vertical_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
            .with_scrollbar_padding(0.0, 0.0)
            .with_wheel_locked_to_dominant_axis(true)
            .finish()
        };
        let scrollable = EventHandler::new(scrollable)
            .with_always_handle()
            .on_scroll_wheel(|event_ctx, _app, _position, delta, _precise, _modifiers| {
                event_ctx.dispatch_typed_action(InputEditorAction::ScrollWheel(delta.y()));
                DispatchEventResult::PropagateToParent
            })
            .finish();

        let lane_height = self.available_height.get().min(max_height).max(line_height);
        let gutter_offset_y =
            render_start as f32 * line_height - self.scroll_state.scroll_start().as_f32().max(0.0);
        // Large-file width comes from the full line index so rows never
        // disagree with the normal path's buffer-based width.
        let large_gutter_width = if crate::console::editor_settings::EditorSettings::global()
            .editor
            .line_numbers
        {
            editor_render::gutter_width_for_lines(total.max(1), self.font_size())
        } else {
            editor_render::gutter_width_for_lines(total.max(1), self.font_size())
                - editor_render::gutter_number_width(total.max(1), self.font_size())
        };
        let fixed_gutter = Container::new(editor_render::fixed_gutter_viewport(
            fixed_gutter_col.finish(),
            gutter_offset_y,
            large_gutter_width,
            lane_height,
        ))
        .with_background_color(self.theme.background)
        .finish();
        // Large-file gutter: same direct-scroll contract as the normal path.
        // `ScrollWheel` actions never move a source editor, so the wheel
        // handler drives `scroll_state` itself instead of dispatching.
        let large_gutter_max_scroll =
            (total as f32 * line_height + self.source_editor_bottom_padding() - lane_height)
                .max(0.0);
        let large_gutter_wheel = self.scroll_state.clone();
        let fixed_gutter = EventHandler::new(fixed_gutter)
            .with_always_handle()
            .on_left_mouse_down(|event_ctx, _app, position| {
                event_ctx.dispatch_typed_action(InputEditorAction::ClickAt(position));
                DispatchEventResult::StopPropagation
            })
            .on_scroll_wheel(
                move |event_ctx, _app, _position, delta, precise, _modifiers| {
                    let mut pixels = delta.y();
                    if !precise {
                        pixels *= 40.0;
                    }
                    let current = large_gutter_wheel.scroll_start().as_f32().max(0.0);
                    let next = (current - pixels).clamp(0.0, large_gutter_max_scroll);
                    if (next - current).abs() > f32::EPSILON {
                        large_gutter_wheel.scroll_to(next.into_pixels());
                        event_ctx.notify();
                        DispatchEventResult::StopPropagation
                    } else {
                        DispatchEventResult::PropagateToParent
                    }
                },
            )
            .finish();
        let gutter = ConstrainedBox::new(fixed_gutter)
            .with_width(large_gutter_width)
            .with_height(lane_height)
            .finish();
        let scrollable = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(gutter)
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(EDITOR_CONTENT_PADDING_LEFT)
                    .with_height(lane_height)
                    .finish(),
            )
            .with_child(Expanded::new(1.0, scrollable).finish())
            .finish();
        SavePosition::new(
            ConstrainedBox::new(scrollable)
                .with_max_height(max_height)
                .finish(),
            &self.viewport_position_id(),
        )
        .finish()
    }
}
