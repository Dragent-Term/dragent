//! Plain-text and layout helpers for the input editor.
//!
//! Responsible for: character/line math, wrapping math, indent guides,
//! line-highlight elements. Pure functions and tiny Elements; no editor state.
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

use dragent_document::{
    capabilities_for_text, DiffLineKind, DocumentCapabilities, DocumentDiff, GitDiffDisplayRow,
};
use dragent_editor::{
    Breakpoint, BreakpointRegistry, DiagnosticSeverity, FoldIndex, FoldParser, FoldRegion,
    FoldState, Language, LineChangeKind, ResolvedDiagnostic,
};

use super::LargeSourceLineIndex;

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

/// Terminal and auxiliary input editors stop growing after this height and
/// become internally scrollable. Source editors supply their own explicit
/// `MultiLine` height and are therefore unaffected.

pub(crate) const INPUT_MAX_HEIGHT: f32 = 336.0;
/// Extra visual rows kept above and below the multiline viewport. This prevents
/// fast trackpad scrolling from exposing an unpainted edge while still keeping
/// the per-frame element count independent of document length.
pub(crate) const MULTILINE_OVERSCAN_ROWS: usize = 8;
/// Right padding inside the horizontal-scroll content area for search/rename inputs.
/// Must match the right-margin used in scroll_horizontal_cursor_into_view so that
/// after_layout's max-scroll clamp never invalidates the cursor position computed in render().
pub(crate) const PLAIN_CANVAS_RIGHT_PADDING: f32 = 8.0;

/// Tab stop width from settings (Editor → Tab size), falling back to the
/// historic 4-space default. Read live (not cached) so changes apply without
/// reopening the editor.
pub(crate) fn source_tab_size() -> usize {
    crate::console::editor_settings::EditorSettings::global()
        .editor
        .clamped_tab_size()
}

/// What the Tab key inserts: spaces (Tab size wide) or a tab character.
pub(crate) fn tab_insert_text() -> String {
    let prefs = crate::console::editor_settings::EditorSettings::global();
    if prefs.editor.insert_spaces {
        " ".repeat(prefs.editor.clamped_tab_size())
    } else {
        "\t".to_string()
    }
}
/// Infer the indent unit a document already uses, so Tab/Enter follow the
/// file's convention instead of the global setting (a Go file keeps real
/// tabs, a 2-space JS file keeps 2 spaces). Tab-vs-space is decided by the
/// majority of indented lines; the space width by voting on the positive
/// steps between consecutive indented lines. `None` when nothing is indented.
pub(crate) fn detect_indent_unit(text: &str) -> Option<String> {
    let mut tab_lines = 0usize;
    let mut space_lines = 0usize;
    let mut votes = [0usize; 9];
    let mut previous = 0usize;
    for line in text.lines().take(2000) {
        let trimmed = line.trim_start_matches([' ', '\t']);
        // Blank lines and block-comment continuations (` * foo`) say nothing
        // about the indent unit.
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        let lead = &line[..line.len() - trimmed.len()];
        if lead.starts_with('\t') {
            tab_lines += 1;
            continue;
        }
        let width = lead.len();
        if width > 0 {
            space_lines += 1;
        }
        if width > previous {
            let step = width - previous;
            if step < votes.len() {
                votes[step] += 1;
            }
        }
        previous = width;
    }
    if tab_lines == 0 && space_lines == 0 {
        return None;
    }
    if tab_lines > space_lines {
        return Some("\t".to_string());
    }
    let width = [2usize, 4, 8, 3]
        .into_iter()
        .max_by_key(|w| (votes[*w], std::cmp::Reverse(*w)))
        .filter(|w| votes[*w] > 0)
        .unwrap_or_else(source_tab_size);
    Some(" ".repeat(width))
}

/// Default source buffer text uses the app's 14px base size with a
/// comfortable 1.618 line-height multiplier.
pub(crate) const SOURCE_EDITOR_FONT_SIZE: f32 = 14.0;
pub(crate) const SOURCE_EDITOR_LINE_HEIGHT_RATIO: f32 = 1.618;
pub(crate) const SOURCE_EDITOR_RIGHT_PADDING: f32 = 24.0;
/// Keep pathological generated/minified rows from monopolising the UI thread.
/// The complete source remains in the buffer; this limit applies only to the
/// text handed to WarpUI's glyph shaper for one visible row. Other editors apply
/// the same 1,024-byte per-line layout ceiling in their editor renderers.
pub(crate) const LARGE_SOURCE_MAX_SHAPED_LINE_BYTES: usize = 1_024;
/// Scrollable breathing room after the final source line. This lives inside
/// the document extent (not around the editor), so reaching EOF keeps the
/// caret clear of the horizontal scrollbar without creating an external gap
/// above the Ports bar.
pub(crate) const SOURCE_EDITOR_BOTTOM_PADDING: f32 = 100.0;
/// When editing the final visible rows, keep this much space between the caret
/// line and the bottom of the source viewport. The remaining scroll-beyond-EOF
/// padding is still available through manual scrolling.
pub(crate) const SOURCE_EDITOR_CURSOR_BOTTOM_MARGIN: f32 = 50.0;
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::editor_render::{
    self, EDITOR_BREAKPOINT_ZONE_WIDTH, EDITOR_CONTENT_PADDING_LEFT, EDITOR_FOLD_ZONE_WIDTH,
};

pub(crate) fn multiline_render_range(
    total_rows: usize,
    scroll_top: f32,
    viewport_height: f32,
    line_height: f32,
) -> std::ops::Range<usize> {
    editor_render::viewport_row_range(
        total_rows,
        scroll_top,
        viewport_height,
        line_height,
        MULTILINE_OVERSCAN_ROWS,
    )
}

/// Byte offset of a character index. O(index) scan without allocating —
/// used by the in-place String edit paths so a keystroke never rebuilds the
/// whole buffer as a `Vec<char>`.
pub(crate) fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

pub(crate) fn physical_line_for_char(text: &str, char_index: usize) -> usize {
    text.chars()
        .take(char_index)
        .filter(|character| *character == '\n')
        .count()
}

pub(crate) fn lexical_line_start_states_after_edit(
    language: Language,
    source: &str,
    previous: Option<&[dragent_editor::HighlightState]>,
    dirty_line: usize,
    force_full: bool,
) -> Vec<dragent_editor::HighlightState> {
    let physical_line_count = source.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let Some(previous) = previous.filter(|states| {
        !force_full && states.len() == physical_line_count && dirty_line < states.len()
    }) else {
        return dragent_editor::LexicalHighlighter::line_start_states(language, source);
    };

    let mut states = previous.to_vec();
    let mut state = states[dirty_line];
    for (line_index, line) in source.split('\n').enumerate().skip(dirty_line) {
        states[line_index] = state;
        let old_next = states.get(line_index + 1).copied();
        let _ = dragent_editor::LexicalHighlighter::highlight_line_with_state(
            language, &line, &mut state,
        );
        // Once the outgoing state rejoins the old snapshot, all unchanged
        // following lines necessarily retain their cached start state.
        if old_next == Some(state) {
            break;
        }
    }
    states
}

pub(crate) fn source_content_x(
    pointer_x: f32,
    viewport_x: f32,
    gutter_width: f32,
    horizontal_scroll: f32,
) -> f32 {
    (pointer_x - viewport_x - gutter_width - EDITOR_CONTENT_PADDING_LEFT + horizontal_scroll)
        .max(0.0)
}

pub(crate) fn bounded_large_source_line(line: &str) -> &str {
    if line.len() <= LARGE_SOURCE_MAX_SHAPED_LINE_BYTES {
        return line;
    }
    let mut end = LARGE_SOURCE_MAX_SHAPED_LINE_BYTES;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

pub(crate) fn large_source_layout_char_extent(index: &LargeSourceLineIndex) -> usize {
    index
        .longest_line_chars
        .min(LARGE_SOURCE_MAX_SHAPED_LINE_BYTES)
}

pub(crate) fn identifier_range_in_line(
    line: &str,
    line_char_start: usize,
    char_index: usize,
) -> Option<std::ops::Range<usize>> {
    let chars = line.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return None;
    }
    let mut local = char_index.saturating_sub(line_char_start).min(chars.len());
    if local == chars.len()
        || !chars
            .get(local)
            .is_some_and(|ch| ch.is_alphanumeric() || *ch == '_')
    {
        local = local.checked_sub(1)?;
    }
    if !chars[local].is_alphanumeric() && chars[local] != '_' {
        return None;
    }
    let mut start = local;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    let mut end = local + 1;
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }
    Some((line_char_start + start)..(line_char_start + end))
}

/// Validate that the characters between two quote positions form a path-like
/// import specifier (no spaces, and carrying a separator or relative marker)
/// and return the specifier text.
pub(crate) fn string_link_content(chars: &[char], open: usize, close: usize) -> Option<String> {
    if close <= open + 1 {
        return None;
    }
    let content: String = chars[open + 1..close].iter().collect();
    // Import specifiers never contain spaces; path-like specifiers carry a
    // separator or a relative/alias marker. Plain words ("react") are left to
    // the LSP identifier flow.
    if content.contains(' ') || content.is_empty() {
        return None;
    }
    if !(content.contains('/') || content.starts_with('.')) {
        return None;
    }
    Some(content)
}

/// Find the (open, close) quote positions of the path-like string literal
/// containing `local` on one line. Returns `None` when the cursor is not
/// inside a linkable specifier.
pub(crate) fn string_link_span_in_line(chars: &[char], local: usize) -> Option<(usize, usize)> {
    let is_quote = |offset: usize| {
        chars
            .get(offset)
            .is_some_and(|ch| matches!(ch, '"' | '\'' | '`'))
    };
    let quote_positions: Vec<usize> = (0..chars.len())
        .filter(|&offset| is_quote(offset))
        .collect();

    let (open, close) = if is_quote(local) {
        // The cursor sits on a quote character: it either closes the
        // preceding literal or opens the next one — accept whichever pair
        // yields a path-like specifier.
        let as_close = quote_positions
            .iter()
            .rev()
            .find(|&&position| position < local)
            .map(|&open| (open, local));
        let as_open = quote_positions
            .iter()
            .find(|&&position| position > local)
            .map(|&close| (local, close));
        match (as_close, as_open) {
            (Some(pair), _) if string_link_content(chars, pair.0, pair.1).is_some() => pair,
            (_, Some(pair)) if string_link_content(chars, pair.0, pair.1).is_some() => pair,
            (Some(pair), _) => pair,
            (None, Some(pair)) => pair,
            (None, None) => return None,
        }
    } else {
        // Quotes alternate open/close; the cursor is inside a literal only
        // when an odd number of quotes precede it.
        let quotes_before = quote_positions
            .iter()
            .filter(|&&position| position < local)
            .count();
        if quotes_before % 2 == 0 {
            return None;
        }
        let open = *quote_positions
            .iter()
            .rev()
            .find(|&&position| position < local)?;
        let close = *quote_positions.iter().find(|&&position| position > local)?;
        (open, close)
    };

    string_link_content(chars, open, close)?;
    Some((open, close))
}

pub(crate) fn visual_row_is_visible(
    row: usize,
    scroll_top: f32,
    viewport_height: f32,
    line_height: f32,
) -> bool {
    if viewport_height <= 0.0 || line_height <= 0.0 {
        return false;
    }
    let row_top = row as f32 * line_height;
    let row_bottom = row_top + line_height;
    row_bottom > scroll_top && row_top < scroll_top + viewport_height
}

/// Preserve the current line's indentation and apply the small, predictable
/// language rules that belong on the typing path. Deeper syntax-aware reindent
/// remains the language service's job; this makes Enter feel immediate even
/// before that service has responded.
pub(crate) fn source_newline_indent(chars: &[char], cursor: usize, language: Language) -> String {
    source_newline_indent_with(chars, cursor, language, &tab_insert_text())
}

/// Pure variant with an explicit tab string (hermetic tests).
pub(crate) fn source_newline_indent_with(
    chars: &[char],
    cursor: usize,
    language: Language,
    tab: &str,
) -> String {
    let cursor = cursor.min(chars.len());
    let start = line_start(chars, cursor);
    let prefix: String = chars[start..cursor].iter().collect();
    let base: String = prefix.chars().take_while(|ch| ch.is_whitespace()).collect();
    let trimmed = prefix.trim_end();

    let opens_block = match language {
        Language::Python => trimmed.ends_with(':'),
        Language::Markdown => false,
        Language::PlainText => false,
        _ => trimmed.ends_with('{') || trimmed.ends_with('[') || trimmed.ends_with('('),
    };

    if opens_block {
        format!("{base}{tab}")
    } else {
        base
    }
}

pub(crate) fn source_line_number(chars: &[char], cursor: usize) -> usize {
    chars[..cursor.min(chars.len())]
        .iter()
        .filter(|ch| **ch == '\n')
        .count()
        + 1
}

#[derive(Clone, Copy)]
pub(crate) struct IndentGuidePaint {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) height: f32,
    pub(crate) color: ColorU,
}

pub(crate) fn pixel_snap_rect(bounds: RectF, scale_factor: f32) -> RectF {
    let scale_factor = scale_factor.max(1.0);
    let snap = |value: f32| (value * scale_factor).round() / scale_factor;
    let min_x = snap(bounds.min_x());
    let min_y = snap(bounds.min_y());
    let max_x = snap(bounds.max_x()).max(min_x + 1.0 / scale_factor);
    let max_y = snap(bounds.max_y()).max(min_y + 1.0 / scale_factor);
    RectF::from_points(vec2f(min_x, min_y), vec2f(max_x, max_y))
}

/// A single non-interactive paint pass for every visible indentation span.
/// Keeping these guides out of the row layout prevents flex rounding from
/// introducing seams or changing the text grid.
pub(crate) struct IndentGuideLayer {
    pub(crate) paints: Vec<IndentGuidePaint>,
    pub(crate) size: Vector2F,
    pub(crate) origin: Option<Point>,
}

/// Paints the current source row in viewport coordinates while remaining in
/// the row's background paint order. `NewScrollable` supplies a document-space
/// origin that already includes `-scroll_left`; adding the live scroll offset
/// here pins the rectangle to the source viewport on every paint, including
/// scroll-only frames that do not rebuild the view tree.
pub(crate) struct ActiveSourceLineHighlight {
    pub(crate) horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) width_hint: f32,
    pub(crate) line_height: f32,
    pub(crate) color: ColorU,
    pub(crate) size: Option<Vector2F>,
    pub(crate) origin: Option<Point>,
}

pub(crate) fn active_line_viewport_rect(
    scrolled_row_origin: Vector2F,
    horizontal_scroll: f32,
    candidate_width: f32,
    line_height: f32,
) -> RectF {
    RectF::new(
        vec2f(
            scrolled_row_origin.x() + horizontal_scroll.max(0.0),
            scrolled_row_origin.y(),
        ),
        vec2f(candidate_width.max(1.0), line_height),
    )
}

impl Element for ActiveSourceLineHighlight {
    fn layout(
        &mut self,
        _constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        let size = vec2f(self.width_hint.max(1.0), self.line_height);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _ctx: &mut AfterLayoutContext, _app: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _app: &AppContext) {
        let candidate = active_line_viewport_rect(
            origin,
            self.horizontal_scroll.scroll_start().as_f32(),
            // The active NewScrollable clip reduces this window-wide candidate
            // to the exact visible source area, including live panel resizes.
            ctx.window_size.x(),
            self.line_height,
        );
        let candidate_origin = Point::from_vec2f(candidate.origin(), ctx.scene.z_index());
        self.origin = Some(candidate_origin);
        let Some(visible_bounds) = ctx.scene.visible_rect(candidate_origin, candidate.size())
        else {
            return;
        };
        ctx.scene
            .draw_rect_without_hit_recording(visible_bounds)
            .with_background(self.color);
    }

    fn dispatch_event(
        &mut self,
        _event: &DispatchedEvent,
        _ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        false
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

impl Element for IndentGuideLayer {
    fn layout(
        &mut self,
        _constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        _app: &AppContext,
    ) -> Vector2F {
        self.size
    }

    fn after_layout(&mut self, _ctx: &mut AfterLayoutContext, _app: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _app: &AppContext) {
        let origin_point = Point::from_vec2f(origin, ctx.scene.z_index());
        self.origin = Some(origin_point);

        // `IndentGuideLayer` paints raw scene rectangles instead of regular
        // child elements. Unlike text, raw rectangles are not automatically
        // cropped by their own layout bounds. Intersect them with the active
        // scene clip (the NewScrollable viewport) before drawing, otherwise a
        // horizontally or vertically scrolled guide can paint over editor
        // chrome and neighbouring panes.
        let Some(visible_bounds) = ctx
            .scene
            .visible_rect(origin_point, vec2f(f32::MAX / 4.0, f32::MAX / 4.0))
        else {
            return;
        };
        for guide in &self.paints {
            let guide_bounds = pixel_snap_rect(
                RectF::new(origin + vec2f(guide.x, guide.y), vec2f(1.0, guide.height)),
                ctx.scene.scale_factor(),
            );
            let Some(guide_bounds) = guide_bounds.intersection(visible_bounds) else {
                continue;
            };
            ctx.scene
                .draw_rect_without_hit_recording(guide_bounds)
                .with_background(guide.color);
        }
    }

    fn dispatch_event(
        &mut self,
        _event: &DispatchedEvent,
        _ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        false
    }

    fn size(&self) -> Option<Vector2F> {
        Some(self.size)
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

pub(crate) fn wrapped_row_absolute_index(
    absolute_row_start: usize,
    layout_row_start: usize,
    layout_index: usize,
) -> usize {
    absolute_row_start + layout_index.saturating_sub(layout_row_start)
}
