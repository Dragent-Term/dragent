//! A pane-covering, pointer-transparent tint overlay.
//!
//! `ScrimOverlay` wraps an arbitrary child element and paints a single
//! translucent rectangle over the child's ENTIRE painted area — after the child
//! itself is painted, so it sits visually on top. The rect is drawn via
//! [`draw_rect_without_hit_recording`], so it records NO hit region: every mouse
//! event still reaches the child underneath (the terminal output, input bar,
//! alt-screen canvas, links, selection, buttons — all stay fully interactive).
//!
//! It exists so the whole terminal screen can carry one uniform tint layer that
//! is guaranteed to cover every pixel of the pane, independent of how many nested
//! containers the content below is built from — the fix for uneven, per-container
//! tint stacking. Layout/size/events all delegate to the child unchanged, so the
//! overlay is invisible to layout and hit-testing.

use std::any::Any;

use warpui::color::ColorU;
use warpui::elements::Point;
use warpui::event::DispatchedEvent;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

pub struct ScrimOverlay {
    child: Box<dyn Element>,
    color: ColorU,
    /// The child's laid-out size, captured in `layout()` for the paint rect.
    size: Option<Vector2F>,
}

impl ScrimOverlay {
    pub fn new(child: Box<dyn Element>, color: ColorU) -> Self {
        Self {
            child,
            color,
            size: None,
        }
    }

    pub fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }
}

impl Element for ScrimOverlay {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
        // Overlay the tint on top of everything the child painted. No hit
        // recording, so the child stays fully interactive underneath.
        if let Some(size) = self.size {
            if size.x() >= 1.0 && size.y() >= 1.0 {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(origin, size))
                    .with_background(self.color);
            }
        }
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
