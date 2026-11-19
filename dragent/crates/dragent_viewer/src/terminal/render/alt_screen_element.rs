//! Full-screen TUI canvas.
//!
//! `AltScreenElement` is a proper [`Element`] implementation: it claims
//! `constraint.max` as its size in `layout()` so it always fills the pane, and
//! it clips its child content so the rendered grid never visually overflows
//! when the window is narrowed.
//!
//! ## Why the old `build_alt_screen_ui` (Flex column) was broken on shrink
//!
//! The previous implementation returned a `Flex::column` tree whose intrinsic
//! width was the widest text row (80 cols × cell_width). When the pane shrank:
//! - `TerminalSizeElement` correctly told the PTY about the smaller size (channel
//!   message fired), but
//! - the `Flex::column`'s `size()` was still reporting the old 80-col content
//!   width — it had no mechanism to shrink below its natural content size.
//! - `Align::new(canvas).top_left()` positioned but did not clip, so the grid
//!   visually overflowed to the right even after the PTY had been told the
//!   smaller size.
//!
//! ## What this struct does
//!
//! 1. `layout()` — captures `constraint.max` (the pane's allocated area) and
//!    passes the SAME constraint down to the inner element so it lays out
//!    within the pane bounds. Returns `constraint.max` (not content size).
//! 2. `after_layout()` — delegates to child.
//! 3. `paint()` — paints child at origin, then overlays a background rectangle
//!    over any horizontal overflow so visual clipping is clean.
//! 4. `size()` — returns `pane_size` (the constraint, always the pane), never
//!    the inner Flex's intrinsic content size.
//! 5. Saves the element position under `viewport_id` via [`SavePosition`] so
//!    `TerminalView::viewport_origin()` can find it for mouse hit-testing.
//!
//! Keyboard handling lives on [`TerminalView`]'s outer `EventHandler` — do not
//! wrap this element in another key handler or Ctrl+C and other chords never
//! reach the PTY.

use std::any::Any;

use dragent_terminal::{MouseButton, MouseEventKind, StyledLine};
use warpui::elements::Point;
use warpui::elements::{
    Align, Container, Expanded, Flex, MainAxisSize, ParentElement, SavePosition,
};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::FamilyId;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui_core::event::ModifiersState;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

use super::block_output::{grid_link_at, GridLink};
use super::grid_metrics::CellMetrics;
use super::grid_renderer::{build_alt_screen_column, GridLineParams, ALT_GRID_PADDING_TOP};
use crate::terminal::model::selection::CellSelection;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::{terminal_line_height_ratio, Theme, TERMINAL_PADDING_LEFT};

/// A pane-filling wrapper for the alt-screen grid.
///
/// Sizes itself to the pane constraint (not the inner grid's content), then
/// clips any horizontal overflow so the grid is never wider than the pane.
pub struct AltScreenElement {
    /// Inner element tree (the grid / SavePosition wrapping it).
    child: Box<dyn Element>,
    /// The pane size captured from `constraint.max` in `layout()`.
    pane_size: Option<Vector2F>,
    /// Background colour for the overflow-fill strip.
    theme: Theme,
    /// Plain text of each row, for link hit-testing on hover/click. Mirrors
    /// what `build_alt_screen_column` rendered this frame.
    lines: Vec<String>,
    /// The terminal's soft-wrap flag per row, and the grid width — so a link
    /// the app (or the terminal) wrapped onto the next row is found whole.
    wrapped: Vec<bool>,
    cols: usize,
    /// Resolves relative paths when confirming a wrapped path exists.
    cwd: Option<std::path::PathBuf>,
    mono_family: FamilyId,
    font_size: f32,
    /// This element's own screen-space origin, captured each `paint()` so
    /// `dispatch_event` can convert absolute pointer positions to a grid
    /// cell without a `ViewContext` round-trip (this is a raw `Element`, not
    /// a view — same pattern `BlockListElement` uses for its own hover).
    last_origin: Option<Vector2F>,
    /// Lazily measured from the real font the first time we need to hit-test
    /// a pointer position; cheap to keep since `AltScreenElement` persists
    /// across frames like the rest of this element tree.
    cell_metrics: Option<CellMetrics>,
    /// `(row, start_col, end_col)` segments of the link under the pointer,
    /// terminal columns — drawn as an underline in `paint()` and used by
    /// `dispatch_event` to avoid redundant cursor churn.
    hovered_link: Option<Vec<(usize, usize, usize)>>,
}

impl AltScreenElement {
    #[allow(clippy::too_many_arguments)]
    fn new(
        child: Box<dyn Element>,
        theme: Theme,
        lines: Vec<String>,
        wrapped: Vec<bool>,
        cols: usize,
        cwd: Option<std::path::PathBuf>,
        mono_family: FamilyId,
        font_size: f32,
        hovered_link: Option<Vec<(usize, usize, usize)>>,
    ) -> Self {
        Self {
            child,
            pane_size: None,
            theme,
            lines,
            wrapped,
            cols,
            cwd,
            mono_family,
            font_size,
            last_origin: None,
            cell_metrics: None,
            hovered_link,
        }
    }

    /// Consume `self` and return it boxed.
    pub fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }

    /// Hit-test a pointer position (absolute/window coordinates) against the
    /// currently rendered rows. Returns the link with every row segment it
    /// covers, or `None` outside the grid / not over a link.
    fn link_at(&mut self, app: &AppContext, position: Vector2F) -> Option<GridLink> {
        let origin = self.last_origin?;
        let metrics = *self.cell_metrics.get_or_insert_with(|| {
            CellMetrics::from_font(
                app.font_cache(),
                self.mono_family,
                self.font_size,
                terminal_line_height_ratio(),
            )
        });
        let rel_x = position.x() - origin.x() - TERMINAL_PADDING_LEFT;
        let rel_y = position.y() - origin.y() - ALT_GRID_PADDING_TOP;
        if rel_x < 0.0 || rel_y < 0.0 {
            return None;
        }
        let col = (rel_x / metrics.width).floor() as usize;
        let row = (rel_y / metrics.height).floor() as usize;
        grid_link_at(
            &self.lines,
            &self.wrapped,
            self.cols,
            self.cwd.as_deref(),
            row,
            col,
        )
    }
}

impl Element for AltScreenElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // Always report the pane size, not the content size. The inner Flex grid
        // (80 cols × cell_width) that doesn't shrink; reporting that as our
        // size would cause the pane to think its content is wider than the
        // window, preventing correct shrink behaviour.
        let max = constraint.max;
        self.pane_size =
            (max.x().is_finite() && max.y().is_finite() && max.x() >= 1.0 && max.y() >= 1.0)
                .then_some(max);

        // Layout the child with the SAME constraint so rows are bounded to
        // the pane width (CrossAxisAlignment::Stretch on the column forces
        // each row to this width).
        self.child.layout(constraint, ctx, app);

        // Always return the pane allocation — not child.size().
        self.pane_size
            .unwrap_or_else(|| self.child.size().unwrap_or(Vector2F::zero()))
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.last_origin = Some(origin);
        self.child.paint(origin, ctx, app);

        // If the inner grid overflows the pane horizontally (content wider
        // than pane_size.x()), fill the overflow strip with the background
        // colour so stale characters don't bleed past the right edge.
        if let (Some(pane), Some(child_size)) = (self.pane_size, self.child.size()) {
            let overflow_w = child_size.x() - pane.x();
            if overflow_w > 0.5 {
                let strip_origin = Vector2F::new(origin.x() + pane.x(), origin.y());
                let strip_size = Vector2F::new(overflow_w + 1.0, pane.y());
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(strip_origin, strip_size))
                    .with_background(self.theme.pane_surface());
            }
        }

        // Underline the hovered link, same treatment as normal block output —
        // a link becomes visibly active on hover, not only after a click.
        if let Some(segments) = self.hovered_link.clone() {
            let metrics = *self.cell_metrics.get_or_insert_with(|| {
                CellMetrics::from_font(
                    app.font_cache(),
                    self.mono_family,
                    self.font_size,
                    terminal_line_height_ratio(),
                )
            });
            for (row, start_col, end_col) in segments {
                let row_y = origin.y()
                    + ALT_GRID_PADDING_TOP
                    + row as f32 * metrics.height
                    + metrics.height
                    - 2.0;
                let x = origin.x() + TERMINAL_PADDING_LEFT + start_col as f32 * metrics.width;
                let w = end_col.saturating_sub(start_col) as f32 * metrics.width;
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(vec2f(x, row_y), vec2f(w, 1.0)))
                    .with_background(self.theme.link());
            }
        }
    }

    fn size(&self) -> Option<Vector2F> {
        // Report the pane size — never the inner grid's content width.
        // Rows/cols derive from pane size, not content size.
        self.pane_size.or_else(|| self.child.size())
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        // Report bounds based on pane_size so hit-testing is correct.
        if let (Some(origin), Some(pane)) = (self.child.origin(), self.pane_size) {
            let org = Vector2F::new(origin.x(), origin.y());
            Some(RectF::new(org, pane))
        } else {
            self.child.bounds()
        }
    }

    fn parent_data(&self) -> Option<&dyn Any> {
        self.child.parent_data()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // Hover-only: this never consumes the event (a TUI's own mouse-tracking
        // mode, drag-selection, and PTY passthrough all still need to see raw
        // motion), it just updates cursor + underline as a side effect before
        // falling through to the child exactly as before.
        if let Event::MouseMoved { position, .. } = event.raw_event() {
            let new_hover = self.link_at(app, *position).map(|link| link.segments);
            if new_hover != self.hovered_link {
                // Keep this element's copy in step with the view's (see the
                // block-list hover): the copy paints and drives change
                // detection whenever this element survives to the next frame.
                self.hovered_link = new_hover.clone();
                ctx.notify();
                match new_hover.clone() {
                    Some(segments) => {
                        ctx.dispatch_typed_action(TerminalAction::HoverAltLink { segments });
                    }
                    None => ctx.dispatch_typed_action(TerminalAction::ClearHoverAltLink),
                }
            }
            if new_hover.is_some() {
                if let Some(z_index) = self.origin().map(|o| o.z_index()) {
                    ctx.set_cursor(Cursor::PointingHand, z_index);
                }
            }
        }
        // The pane's own handlers only hear the mouse inside the pane, so a drag
        // that leaves it is reported from here. Apps often act on the release:
        // Claude Code copies its selection then, and a drag commonly ends on the
        // input panel below. The view ignores this if no gesture is in progress.
        let outside_pane = match event.raw_event() {
            Event::LeftMouseDragged { position, .. } => Some((*position, MouseEventKind::Motion)),
            Event::LeftMouseUp { position, .. } => Some((*position, MouseEventKind::Release)),
            _ => None,
        }
        .filter(|(position, _)| {
            !self
                .origin()
                .zip(self.pane_size)
                .and_then(|(origin, size)| ctx.visible_rect(origin, size))
                .is_some_and(|rect| rect.contains_point(*position))
        });
        if let Some((position, kind)) = outside_pane {
            ctx.dispatch_typed_action(TerminalAction::TerminalMouse {
                position,
                button: MouseButton::Left,
                kind,
                modifiers: ModifiersState::default(),
            });
        }
        self.child.dispatch_event(event, ctx, app)
    }
}

// ── builders ────────────────────────────────────────────────────────────────

/// Inner layout tree for the alt-screen grid (the Flex column + SavePosition).
///
/// The resulting tree is meant to be wrapped by `AltScreenElement` so its
/// intrinsic content size is never reported outward.
fn build_viewport_grid(
    canvas: Box<dyn Element>,
    viewport_id: &'static str,
    theme: Theme,
) -> Box<dyn Element> {
    let aligned = Align::new(canvas).top_left().finish();
    let fill = Container::new(aligned)
        // Transparent under an image theme so the alt-screen canvas stacks no tint
        // of its own — the single workspace-level tint shows through, matching the
        // normal terminal exactly (opaque background on solid themes).
        .with_background_color(theme.pane_surface())
        .finish();

    SavePosition::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Box::new(Expanded::new(1.0, fill)))
            .finish(),
        viewport_id,
    )
    .finish()
}

/// Build the alt-screen pane-filling element.
///
/// Returns an [`AltScreenElement`] that:
/// - reports `constraint.max` (pane size) as its own size
/// - clips horizontal overflow with a background fill
/// - saves viewport position under `"terminal_alt_viewport"` for mouse hit-testing
pub fn build_alt_screen_ui(
    lines: Vec<StyledLine>,
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    selection: Option<CellSelection>,
    theme: Theme,
    mono_family: FamilyId,
    font_size: f32,
    cols: usize,
    cwd: Option<std::path::PathBuf>,
    hovered_link: Option<Vec<(usize, usize, usize)>>,
) -> Box<dyn Element> {
    let canvas = build_alt_screen_column(
        &lines,
        cursor_row,
        cursor_col,
        cursor_visible,
        selection.as_ref(),
        GridLineParams {
            theme,
            mono_family,
            padding_left: TERMINAL_PADDING_LEFT,
            font_size,
        },
    );

    let plain_lines: Vec<String> = lines.iter().map(StyledLine::plain_text).collect();
    let wrapped: Vec<bool> = lines.iter().map(|line| line.wrapped).collect();
    let inner = build_viewport_grid(canvas, "terminal_alt_viewport", theme);
    AltScreenElement::new(
        inner,
        theme,
        plain_lines,
        wrapped,
        cols,
        cwd,
        mono_family,
        font_size,
        hovered_link,
    )
    .finish()
}
