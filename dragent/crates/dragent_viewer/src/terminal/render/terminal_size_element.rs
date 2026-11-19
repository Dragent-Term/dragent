//! Layout-driven terminal resize.
//!
//! Transparent wrapper that, on every `after_layout`, reads the pane's measured
//! pixel size and pushes it down a channel. The view converts pixels → rows/cols
//! → SIGWINCH.
//!
//! Why this and not polling `element_position_by_id` on a timer:
//! - It fires on EVERY layout, including a bare window resize that produces no
//!   model event — the previous polling path never resized the PTY on window
//!   drag because nothing told it to re-measure.
//! - It reports the real laid-out size, so there is no guesswork and no lag.

use std::any::Any;

use async_channel::Sender;
use warpui::elements::Point;
use warpui::event::DispatchedEvent;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

/// Wraps terminal content and emits the available **pane** size after layout.
pub struct TerminalSizeElement {
    child: Box<dyn Element>,
    resize_tx: Sender<Vector2F>,
    /// Pane size captured from the incoming layout constraint's `max`.
    pane_size: Option<Vector2F>,
}

impl TerminalSizeElement {
    pub fn new(resize_tx: Sender<Vector2F>, child: Box<dyn Element>) -> Self {
        Self {
            child,
            resize_tx,
            pane_size: None,
        }
    }
}

impl Element for TerminalSizeElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // Rows/cols must derive from the pane size, not sticky content width.
        // Fixed-width cell grids keep intrinsic width when the window shrinks and
        // overflow/clip instead of narrowing — so `child.size()` is sticky on
        // shrink. The constraint's `max` is the area allocated to us and shrinks
        // correctly in both directions.
        let max = constraint.max;
        self.pane_size =
            (max.x().is_finite() && max.y().is_finite() && max.x() >= 1.0 && max.y() >= 1.0)
                .then_some(max);

        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);

        // Prefer the pane constraint; fall back to the laid-out size only when
        // the constraint was unbounded (shouldn't happen for a pane, but keeps
        // us safe in odd parents).
        let pane = self.pane_size.or_else(|| self.child.size());
        if let Some(size) = pane {
            // Unbounded, non-blocking. The consumer dedups identical sizes, so
            // sending every layout is cheap. A closed channel (view torn down)
            // is fine to ignore.
            let _ = self.resize_tx.try_send(size);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        self.child.bounds()
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
        self.child.dispatch_event(event, ctx, app)
    }
}
