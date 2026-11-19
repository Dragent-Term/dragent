//! WarpUI presentation for the UI-independent `dragent_editor` domain.

use dragent_document::DiffLineKind;
use dragent_editor::{
    BreakpointState, DiagnosticSeverity, Language, LexicalHighlighter, LineChangeKind,
};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Point, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::vec2f;
use warpui::geometry::vector::Vector2F;
use warpui::{AfterLayoutContext, AppContext, LayoutContext, PaintContext, SizeConstraint};
use warpui_core::event::DispatchedEvent;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::{ClipBounds, EventContext};

use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::editor_palette::EditorPalette;

/// Fixed editor chrome: one unified gutter row per line.
///
/// ```text
/// Gutter Row
/// ├── Breakpoint Slot (change rail + breakpoint dot)
/// ├── Diagnostic Lane (error/warning dot)
/// ├── Line Number Slot (right-aligned number)
/// └── Debug Slot (fold chevron / current-line indicator)
/// ```
///
/// Every line renders all four lanes inside a single `Flex::row` container so
/// the row reads as one control area (`• 123 >`) instead of unrelated pieces.
/// Hit-testing in `handle_gutter_click` uses the same zone widths, so clicks
/// keep routing to breakpoint vs. fold lanes exactly as before.
///
/// The number slot — and therefore the whole gutter — grows with the final
/// line number: files up to 999 lines keep the base width, and larger files
/// widen by one digit at a time so e.g. line 1100 is never clipped.
pub const EDITOR_BREAKPOINT_ZONE_WIDTH: f32 = 10.0;
/// Dedicated folding / debug interaction lane. Fits the 12px chevron with a
/// pixel of breathing room on each side — no wider, so it can never cover a
/// line number's final digit.
pub const EDITOR_FOLD_ZONE_WIDTH: f32 = 14.0;
/// Fixed lane for the diagnostic dot. It stays reserved even without a
/// diagnostic so numbers never shift while scrolling through diagnostics.
pub const EDITOR_DIAGNOSTIC_LANE_WIDTH: f32 = 6.0;
/// Digits the number slot always fits: small files keep the stable base width
/// instead of reflowing the whole editor on every added digit.
pub const EDITOR_GUTTER_BASE_DIGITS: usize = 3;
/// Conservative monospace digit advance. Deliberately above the `0.62`
/// estimate used for horizontal content measurement: under-estimating here
/// clips the final digit under the fold lane (seen with 5-digit lines), so
/// this keeps ~10% margin instead.
const GUTTER_DIGIT_WIDTH_FACTOR: f32 = 0.66;
const GUTTER_NUMBER_RIGHT_PAD: f32 = 4.0;

/// Number of digit columns reserved for `max_line` (at least the base three).
pub fn gutter_digits_for_max_line(max_line: usize) -> usize {
    max_line
        .max(1)
        .to_string()
        .len()
        .max(EDITOR_GUTTER_BASE_DIGITS)
}

/// Width of the right-aligned line-number slot for a file ending at
/// `max_line`, at the given editor font size.
pub fn gutter_number_width(max_line: usize, font_size: f32) -> f32 {
    let number_font_size = (font_size - 1.0).max(10.0);
    gutter_digits_for_max_line(max_line) as f32 * number_font_size * GUTTER_DIGIT_WIDTH_FACTOR
        + GUTTER_NUMBER_RIGHT_PAD
}

/// Total gutter width for a file ending at `max_line`. Callers must use this
/// (not a constant) for painting, hit-testing and content offsets so the
/// three never disagree.
pub fn gutter_width_for_lines(max_line: usize, font_size: f32) -> f32 {
    EDITOR_BREAKPOINT_ZONE_WIDTH
        + EDITOR_DIAGNOSTIC_LANE_WIDTH
        + gutter_number_width(max_line, font_size)
        + EDITOR_FOLD_ZONE_WIDTH
}
/// Fixed breathing room between editor chrome and the scrollable content
/// viewport. It cannot be consumed by horizontal scrolling.
pub const EDITOR_CONTENT_PADDING_LEFT: f32 = 8.0;
const SOURCE_GUTTER_MARKER_HEIGHT: f32 = 20.0;

/// Return the stable row window needed for a clipped viewport. Callers keep
/// the full content extent with spacers and materialize only this range. This
/// is shared by the source editor and aggregate review/search surfaces so
/// scrolling cost depends on viewport size rather than document size.
pub fn viewport_row_range(
    total_rows: usize,
    scroll_top: f32,
    viewport_height: f32,
    row_height: f32,
    overscan_rows: usize,
) -> std::ops::Range<usize> {
    if total_rows == 0 || row_height <= 0.0 {
        return 0..0;
    }
    let visible_rows = (viewport_height.max(row_height) / row_height).ceil() as usize + 1;
    let max_first_visible = total_rows.saturating_sub(visible_rows);
    let first_visible =
        ((scroll_top.max(0.0) / row_height).floor() as usize).min(max_first_visible);
    first_visible.saturating_sub(overscan_rows)
        ..(first_visible + visible_rows + overscan_rows).min(total_rows)
}

/// Paint a diagnostic wave as one element. This avoids allocating and laying
/// out hundreds of tiny containers for long diagnostic ranges while scrolling.
pub fn diagnostic_squiggle(width: f32, color: ColorU) -> Box<dyn Element> {
    DiagnosticSquiggle {
        size: vec2f(width.max(4.0), 3.0),
        color,
        origin: None,
    }
    .finish()
}

struct DiagnosticSquiggle {
    size: Vector2F,
    color: ColorU,
    origin: Option<Point>,
}

impl Element for DiagnosticSquiggle {
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
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let mut x = 0.0;
        let mut raised = true;
        while x < self.size.x() {
            let segment_width = (self.size.x() - x).min(2.0);
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    origin + vec2f(x, if raised { 0.0 } else { 2.0 }),
                    vec2f(segment_width, 1.0),
                ))
                .with_background(self.color);
            raised = !raised;
            x += 2.0;
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

/// Paint a virtualised gutter slice at a vertical offset inside a fixed,
/// clipped viewport. Unlike a second scrollable, this element never owns or
/// mutates scroll state, so folding cannot make two layouts fight over the
/// document's vertical anchor.
pub fn fixed_gutter_viewport(
    child: Box<dyn Element>,
    offset_y: f32,
    width: f32,
    height: f32,
) -> Box<dyn Element> {
    FixedGutterViewport {
        child,
        offset_y,
        viewport_size: vec2f(width, height),
        origin: None,
    }
    .finish()
}

struct FixedGutterViewport {
    child: Box<dyn Element>,
    offset_y: f32,
    viewport_size: Vector2F,
    origin: Option<Point>,
}

impl Element for FixedGutterViewport {
    fn layout(
        &mut self,
        _constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(
            SizeConstraint::new(
                vec2f(self.viewport_size.x(), 0.0),
                vec2f(self.viewport_size.x(), f32::INFINITY),
            ),
            ctx,
            app,
        );
        self.viewport_size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let origin_point = Point::from_vec2f(origin, ctx.scene.z_index());
        self.origin = Some(origin_point);

        // Respect the clip already established by the document/editor pane.
        // Starting a fresh `BoundedBy(viewport)` layer directly replaced the
        // parent clip instead of intersecting it, so a gutter measured before
        // the dock opened could paint line numbers through the split and over
        // the terminal. `visible_rect` intersects with the active layer, exactly
        // like WarpUI's `Clipped` element does.
        if let Some(bounds) = ctx.scene.visible_rect(origin_point, self.viewport_size) {
            ctx.scene.start_layer(ClipBounds::BoundedBy(bounds));
            self.child
                .paint(origin + vec2f(0.0, self.offset_y), ctx, app);
            ctx.scene.stop_layer();
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        Some(self.viewport_size)
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Longest source line a read-only diff row lays out. Shaping cost grows
/// much faster than line length: a highlighted 458,345-character minified
/// line measured 28 s to lay out, 2,000 characters about 2 ms. Rows show a
/// "more characters" marker past this; the editor keeps the full line.
pub const DIFF_ROW_MAX_CHARS: usize = 2_000;

/// `line` cut to at most `max_chars` characters (on a char boundary) and the
/// number of characters left out.
pub fn display_prefix(line: &str, max_chars: usize) -> (&str, usize) {
    match line.char_indices().nth(max_chars) {
        Some((cut, _)) => (&line[..cut], line[cut..].chars().count()),
        None => (line, 0),
    }
}

pub fn highlighted_source(
    line: &str,
    language: Language,
    theme: &Theme,
    font: FamilyId,
    size: f32,
    weight: Weight,
    show_whitespace: bool,
) -> Text {
    highlighted_source_with_state(
        line,
        language,
        &mut dragent_editor::HighlightState::default(),
        theme,
        font,
        size,
        weight,
        show_whitespace,
    )
}

/// Render spaces as visible dots. Applied per already-sliced highlight
/// piece (never to the lexer's input), so byte ranges stay valid and the
/// 1:1 character mapping keeps caret/selection math exact. Tabs are left
/// alone: their display width varies, so no single glyph maps 1:1.
pub fn show_whitespace_dots(text: &str) -> String {
    text.replace(' ', "\u{b7}")
}

pub fn highlighted_source_with_state(
    line: &str,
    language: Language,
    state: &mut dragent_editor::HighlightState,
    theme: &Theme,
    font: FamilyId,
    size: f32,
    weight: Weight,
    show_whitespace: bool,
) -> Text {
    let spans = LexicalHighlighter::highlight_line_with_state(language, line, state);
    highlighted_source_from_spans(line, &spans, theme, font, size, weight, show_whitespace)
}

/// Build a source line's text from precomputed spans (byte ranges within
/// `line`, sorted, non-overlapping) — the tree-sitter path and the lexical
/// path share this so both render identically apart from their spans.
pub fn highlighted_source_from_spans(
    line: &str,
    spans: &[dragent_editor::HighlightSpan],
    theme: &Theme,
    font: FamilyId,
    size: f32,
    weight: Weight,
    show_whitespace: bool,
) -> Text {
    let palette = EditorPalette::from_theme(theme);
    let mut text = Text::new("", font, size);
    let properties = Properties::default().weight(weight);
    let shown = |piece: &str| -> String {
        if show_whitespace {
            show_whitespace_dots(piece)
        } else {
            piece.to_string()
        }
    };
    let mut cursor = 0;
    for span in spans {
        if span.range.start < cursor || span.range.end > line.len() {
            continue;
        }
        if span.range.start > cursor {
            text.add_text_with_highlights(
                &shown(&line[cursor..span.range.start]),
                palette.plain(),
                properties,
            );
        }
        text.add_text_with_highlights(
            &shown(&line[span.range.clone()]),
            palette.token(span.kind),
            properties,
        );
        cursor = span.range.end;
    }
    if cursor < line.len() {
        text.add_text_with_highlights(&shown(&line[cursor..]), palette.plain(), properties);
    }
    text
}

pub fn gutter(
    show_numbers: bool,
    line: usize,
    max_line: usize,
    continuation: bool,
    line_change: Option<LineChangeKind>,
    diagnostic: Option<DiagnosticSeverity>,
    breakpoint: Option<BreakpointState>,
    foldable: bool,
    collapsed: bool,
    fold_gutter_hovered: bool,
    active_line: bool,
    theme: &Theme,
    font: FamilyId,
    size: f32,
) -> Box<dyn Element> {
    let change_marker = match line_change {
        Some(LineChangeKind::Added) => Some((theme.ui_green(), 4.0, SOURCE_GUTTER_MARKER_HEIGHT)),
        Some(LineChangeKind::Modified) => Some((theme.accent, 4.0, SOURCE_GUTTER_MARKER_HEIGHT)),
        Some(LineChangeKind::Deleted) => Some((theme.ui_red(), 7.0, 3.0)),
        None => None,
    };
    let change_marker = if let Some((color, width, height)) = change_marker {
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(width)
                .with_height(height)
                .finish(),
        )
        .with_background_color(color)
        .finish()
    } else {
        ConstrainedBox::new(Empty::new().finish())
            .with_width(4.0)
            .with_height(SOURCE_GUTTER_MARKER_HEIGHT)
            .finish()
    };
    let breakpoint_marker: Box<dyn Element> = if let Some(state) = breakpoint {
        let color = match state {
            BreakpointState::Enabled => theme.ui_red(),
            BreakpointState::Disabled => with_alpha(theme.ui_red(), 0.35),
            BreakpointState::Unverified => with_alpha(theme.ui_red(), 0.65),
        };
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(9.0)
                .with_height(9.0)
                .finish(),
        )
        .with_background_color(color)
        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)))
        .finish()
    } else {
        ConstrainedBox::new(Empty::new().finish())
            .with_width(9.0)
            .with_height(9.0)
            .finish()
    };
    let diagnostic_dot: Box<dyn Element> = if let Some(severity) = diagnostic {
        let color = match severity {
            DiagnosticSeverity::Error => theme.ui_red(),
            DiagnosticSeverity::Warning => theme.ui_yellow(),
            DiagnosticSeverity::Information => theme.accent,
            DiagnosticSeverity::Hint => theme.sub_text(),
        };
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(4.0)
                .with_height(4.0)
                .finish(),
        )
        .with_background_color(color)
        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)))
        .finish()
    } else {
        Empty::new().finish()
    };
    // Slot 1 — breakpoint: diff rail and breakpoint dot share one fixed glyph
    // lane. Keeping the lane independent from the line-number width prevents
    // the Git rail from shifting when line numbers grow in digit count.
    let breakpoint_slot = ConstrainedBox::new(
        Stack::new()
            .with_child(Align::new(change_marker).left().finish())
            .with_child(Align::new(breakpoint_marker).finish())
            .finish(),
    )
    .with_width(EDITOR_BREAKPOINT_ZONE_WIDTH)
    .with_height(SOURCE_GUTTER_MARKER_HEIGHT)
    .finish();
    let label = if continuation {
        String::new()
    } else {
        line.to_string()
    };
    // Slot 2 — diagnostic lane: a fixed 6px lane keeps the dot signal without
    // shifting numbers as diagnostics appear and disappear while scrolling.
    let diagnostic_lane = ConstrainedBox::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(diagnostic_dot)
            .finish(),
    )
    .with_width(EDITOR_DIAGNOSTIC_LANE_WIDTH)
    .with_height(SOURCE_GUTTER_MARKER_HEIGHT)
    .finish();
    // Slot 3 — line number: plain right-aligned text in a slot sized from the
    // file's final line number, so "1100" gets a wider lane instead of
    // clipping. Deliberately NOT a Stack overlay: overlaying the dot on top
    // of the text hid the numbers entirely. Also deliberately NOT
    // height-pinned: WarpUI drops text whose line height exceeds its max
    // height, and the app-wide typography ratio can make 14px text taller
    // than 20px — that pin is what made numbers vanish. The row height is
    // fixed by the caller; Center alignment keeps the number vertically
    // centered. Explicit ratio 1.0 + no soft-wrap keeps every number on one
    // line that always fits its digit-sized slot.
    let number_slot: Box<dyn Element> = if !show_numbers {
        ConstrainedBox::new(Empty::new().finish())
            .with_width(0.0)
            .finish()
    } else {
        let number_width = gutter_number_width(max_line, size);
        ConstrainedBox::new(
            Align::new(
                Container::new(
                    Text::new(label, font, (size - 1.0).max(10.0))
                        .with_color(if active_line {
                            theme.sub_text()
                        } else {
                            theme.disabled_text()
                        })
                        .with_line_height_ratio(1.0)
                        .soft_wrap(false)
                        .finish(),
                )
                .with_padding_right(GUTTER_NUMBER_RIGHT_PAD)
                .finish(),
            )
            .right()
            .finish(),
        )
        .with_width(number_width)
        .finish()
    };
    // Slot 4 — debug: fold chevron / current-line indicator. Expanded controls
    // reveal on gutter hover; collapsed controls remain visible so the hidden
    // range is always discoverable and reversible.
    let fold_icon = if !continuation && foldable && (fold_gutter_hovered || collapsed) {
        render_icon(
            if collapsed {
                AppIcon::ChevronRight
            } else {
                AppIcon::ChevronDown
            },
            theme.sub_text(),
            12.0,
        )
    } else {
        Empty::new().finish()
    };
    let debug_slot = ConstrainedBox::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(fold_icon)
            .finish(),
    )
    .with_width(EDITOR_FOLD_ZONE_WIDTH)
    .with_height(SOURCE_GUTTER_MARKER_HEIGHT)
    .finish();
    // One unified gutter row per line: breakpoint | diagnostic | number |
    // debug. The outer width matches `gutter_width_for_lines` exactly so the
    // painted row, the fixed viewport and hit-testing never disagree.
    let gutter_width = gutter_width_for_lines(max_line, size);
    ConstrainedBox::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(0.0)
            .with_child(breakpoint_slot)
            .with_child(diagnostic_lane)
            .with_child(number_slot)
            .with_child(debug_slot)
            .finish(),
    )
    .with_width(gutter_width)
    .finish()
}

/// Two-coordinate gutter used by an inline Git diff projection.
///
/// Unlike the normal editor gutter, this keeps the base and current line
/// numbers visible together. The background is painted inside the fixed
/// gutter as well as the source row so additions and deletions read as one
/// continuous semantic strip rather than a detached marker.
pub fn git_diff_gutter(
    old_line: Option<usize>,
    new_line: Option<usize>,
    kind: DiffLineKind,
    theme: &Theme,
    font: FamilyId,
    size: f32,
) -> Box<dyn Element> {
    let (marker, marker_color, rail_color, background) = match kind {
        DiffLineKind::Added => (
            "+",
            theme.ui_green(),
            theme.ui_green(),
            with_alpha(theme.ui_green(), 0.095),
        ),
        DiffLineKind::Removed => (
            "−",
            theme.ui_red(),
            theme.ui_red(),
            with_alpha(theme.ui_red(), 0.095),
        ),
        DiffLineKind::Context => (
            "",
            theme.disabled_text(),
            ColorU::transparent_black(),
            theme.background,
        ),
    };
    let number = |line: Option<usize>| {
        Text::new(
            line.map(|line| line.to_string()).unwrap_or_default(),
            font,
            (size - 1.5).max(10.0),
        )
        .with_color(theme.disabled_text())
        .finish()
    };
    let number_slot = |line| {
        ConstrainedBox::new(
            Align::new(
                Container::new(number(line))
                    .with_padding_right(3.0)
                    .finish(),
            )
            .right()
            .finish(),
        )
        .with_width(31.0)
        .finish()
    };
    let rail = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(3.0)
            .with_height(SOURCE_GUTTER_MARKER_HEIGHT)
            .finish(),
    )
    .with_background_color(rail_color)
    .finish();
    let marker = ConstrainedBox::new(
        Align::new(
            Text::new(marker.to_string(), font, (size - 1.0).max(10.0))
                .with_color(marker_color)
                .finish(),
        )
        .finish(),
    )
    .with_width(20.0)
    .finish();

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(0.0)
            .with_child(rail)
            .with_child(number_slot(old_line))
            .with_child(number_slot(new_line))
            .with_child(marker)
            .finish(),
    )
    .with_background_color(background)
    .finish()
}

fn with_alpha(color: warpui::color::ColorU, alpha: f32) -> warpui::color::ColorU {
    warpui::color::ColorU::new(
        color.r,
        color.g,
        color.b,
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    )
}
