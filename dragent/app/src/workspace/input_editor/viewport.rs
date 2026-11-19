//! Virtualize source rows against the allocated viewport on the first layout,
//! including layouts caused by window or split resizing without pointer events.
use super::InputEditor;
use warpui::elements::{Element, Empty, Point};
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
    WeakViewHandle,
};
use warpui_core::event::DispatchedEvent;

pub(super) struct SourceEditorViewport {
    editor: WeakViewHandle<InputEditor>,
    child: Box<dyn Element>,
}

impl SourceEditorViewport {
    pub(super) fn new(editor: WeakViewHandle<InputEditor>) -> Self {
        Self {
            editor,
            child: Empty::new().finish(),
        }
    }
}

impl Element for SourceEditorViewport {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        if let Some(editor) = self.editor.upgrade(app) {
            self.child = editor.read(app, |editor, app| {
                let width = viewport_extent(constraint.max.x(), ctx.window_size.x());
                let height = viewport_extent(constraint.max.y(), ctx.window_size.y());
                let text_width = editor.text_layout_width(width);
                editor.available_width.set(text_width);
                editor.layout_width.set(text_width);
                editor.available_height.set(height);
                editor.render_command_canvas(app)
            });
        }
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
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
        self.child.size()
    }
    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }
}

fn viewport_extent(max: f32, window: f32) -> f32 {
    if max.is_finite() {
        max.max(0.0)
    } else {
        window.max(0.0)
    }
}
