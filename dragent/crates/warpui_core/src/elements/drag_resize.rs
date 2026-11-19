use std::mem;
use std::sync::{Arc, Mutex};

use pathfinder_geometry::vector::Vector2F;

use super::{Point, ZIndex};
use crate::event::DispatchedEvent;
use crate::platform::Cursor;
use crate::{
    AfterLayoutContext, AppContext, Element, Event, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

/// Shared handle for drag-to-resize state, following the same `Arc<Mutex<_>>`
/// pattern as `ResizableStateHandle`. The view creates the handle once at
/// construction and passes it into `DragResizeElement` each render.
pub type DragResizeHandle = Arc<Mutex<DragResizeState>>;

pub fn drag_resize_handle() -> DragResizeHandle {
    Arc::new(Mutex::new(DragResizeState::default()))
}

/// Drag axis: which pointer coordinate drives the resize deltas.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum DragResizeAxis {
    /// Vertical drags (existing behavior): deltas from Y coordinates.
    #[default]
    Vertical,
    /// Horizontal drags: deltas from X coordinates.
    Horizontal,
}

/// Tracks whether a drag-to-resize operation is in progress.
#[derive(Default)]
pub struct DragResizeState {
    is_dragging: bool,
    last_x: f32,
    last_y: f32,
}

impl DragResizeState {
    fn begin(&mut self, x: f32, y: f32) {
        self.is_dragging = true;
        self.last_x = x;
        self.last_y = y;
    }

    fn end(&mut self) {
        self.is_dragging = false;
    }

    fn is_dragging(&self) -> bool {
        self.is_dragging
    }

    /// Compute the delta since the last event along `axis` and update state.
    fn consume_delta(&mut self, x: f32, y: f32, axis: DragResizeAxis) -> f32 {
        let delta = match axis {
            DragResizeAxis::Vertical => y - self.last_y,
            DragResizeAxis::Horizontal => x - self.last_x,
        };
        self.last_x = x;
        self.last_y = y;
        delta
    }
}

/// Callback invoked during a resize drag. Receives the vertical delta (pixels).
type ResizeUpdateFn = Box<dyn Fn(f32, &mut EventContext, &AppContext)>;

/// Callback invoked when a resize drag finishes.
pub type ResizeEndFn = Box<dyn Fn(&mut EventContext, &AppContext)>;

/// An element that enables drag-to-resize on its entire surface area.
///
/// The element dispatches events to its child first. If the child does not
/// handle a `LeftMouseDown`, the element begins a resize operation. Subsequent
/// `LeftMouseDragged` / `LeftMouseUp` events are captured via `raw_event()` so
/// they work regardless of cursor position (same pattern used by `Resizable`).
pub struct DragResizeElement {
    child: Box<dyn Element>,
    handle: DragResizeHandle,
    axis: DragResizeAxis,
    on_resize_update: ResizeUpdateFn,
    on_resize_end: Option<ResizeEndFn>,
    origin: Option<Point>,
    child_max_z_index: Option<ZIndex>,
    is_hovering: bool,
}

impl DragResizeElement {
    pub fn new(
        handle: DragResizeHandle,
        child: Box<dyn Element>,
        on_resize_update: impl Fn(f32, &mut EventContext, &AppContext) + 'static,
        on_resize_end: Option<ResizeEndFn>,
    ) -> Self {
        Self::with_axis(
            handle,
            child,
            DragResizeAxis::Vertical,
            on_resize_update,
            on_resize_end,
        )
    }

    /// Same as [`DragResizeElement::new`], but tracking the given axis.
    /// Horizontal dividers (left/right splits) pass [`DragResizeAxis::Horizontal`].
    pub fn with_axis(
        handle: DragResizeHandle,
        child: Box<dyn Element>,
        axis: DragResizeAxis,
        on_resize_update: impl Fn(f32, &mut EventContext, &AppContext) + 'static,
        on_resize_end: Option<ResizeEndFn>,
    ) -> Self {
        Self {
            child,
            handle,
            axis,
            on_resize_update: Box::new(on_resize_update),
            on_resize_end,
            origin: None,
            child_max_z_index: None,
            is_hovering: false,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, DragResizeState> {
        // This is the same (slightly scary) pattern as `Resizable::state()`.
        // Poisoning should only occur after a prior panic (already in a bad state).
        self.handle.lock().expect("DragResizeState lock poisoned")
    }

    fn cursor(&self) -> Cursor {
        match self.axis {
            DragResizeAxis::Vertical => Cursor::ResizeUpDown,
            DragResizeAxis::Horizontal => Cursor::ResizeLeftRight,
        }
    }
}

impl Element for DragResizeElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // Always let the child see the event first.
        let child_handled = self.child.dispatch_event(event, ctx, app);

        // Use raw_event() for drag/up so they are position-independent.
        match event.raw_event() {
            Event::LeftMouseDown { position, .. } => {
                if child_handled {
                    return true;
                }
                // Check if the click is within our bounds.
                if let (Some(origin), Some(size)) = (self.origin, self.size()) {
                    if let Some(rect) = ctx.visible_rect(origin, size) {
                        if rect.contains_point(*position) {
                            self.state().begin(position.x(), position.y());
                            return true;
                        }
                    }
                }
                false
            }
            Event::LeftMouseDragged { position, .. } => {
                if self.state().is_dragging() {
                    if let Some(z_index) = self.child_max_z_index {
                        ctx.set_cursor(self.cursor(), z_index);
                    }
                    let axis = self.axis;
                    let delta = self.state().consume_delta(position.x(), position.y(), axis);
                    (self.on_resize_update)(delta, ctx, app);
                    return true;
                }
                child_handled
            }
            Event::LeftMouseUp { .. } => {
                if self.state().is_dragging() {
                    self.state().end();
                    if let Some(on_end) = &self.on_resize_end {
                        (on_end)(ctx, app);
                    }
                    ctx.reset_cursor();
                    return true;
                }
                child_handled
            }
            Event::MouseMoved { position, .. } => {
                if self.state().is_dragging() {
                    if let Some(z_index) = self.child_max_z_index {
                        ctx.set_cursor(self.cursor(), z_index);
                    }
                    return true;
                }

                let is_hovering = if let (Some(origin), Some(size)) = (self.origin, self.size()) {
                    ctx.visible_rect(origin, size)
                        .is_some_and(|rect| rect.contains_point(*position))
                } else {
                    false
                };
                let was_hovering = mem::replace(&mut self.is_hovering, is_hovering);

                if is_hovering && !was_hovering {
                    if let Some(z_index) = self.child_max_z_index {
                        ctx.set_cursor(self.cursor(), z_index);
                    }
                } else if !is_hovering && was_hovering {
                    ctx.reset_cursor();
                }

                child_handled
            }
            _ => child_handled,
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }
}
