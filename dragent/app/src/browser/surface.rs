//! The element that reserves a browser pane's area in the scene. The native
//! guest is positioned from the presenter's placement after each frame. When
//! a menu or dialog is drawn above the pane, the page is captured and the
//! capture is drawn in its place, so the menu shows over the page instead of
//! over a blank pane.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_channel::Sender;
use warpui::color::ColorU;
use warpui::elements::{Element, Point};
use warpui::event::DispatchedEvent;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::image_cache::StaticImage;
use warpui::scene::{CornerRadius, NativePlacement, NativeSurfacePlacer};
use warpui::SizeConstraint;
use warpui::{AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext};
use web_time::Instant;

use super::native::Engine;

static NEXT_SURFACE_ID: AtomicUsize = AtomicUsize::new(1);

/// The page as captured while something covers it.
#[derive(Default)]
pub(super) enum Cover {
    #[default]
    None,
    /// Capturing; the live page stays up until the capture is drawn.
    Capturing,
    Ready(Arc<StaticImage>),
    /// No capture: the pane shows its background while covered.
    Unavailable,
}

pub(super) struct SurfaceState {
    id: usize,
    pub engine: Option<Engine>,
    /// Latest bounds in window points; `None` while not on screen.
    pub placement: Option<RectF>,
    /// On screen, under a menu or dialog.
    pub covered: bool,
    /// A phone is driving this page: while something covers the pane the
    /// page is parked out of sight instead of hidden, so it keeps running.
    pub park: bool,
    /// The size the parked page is laid out at (a phone's mobile view).
    pub park_size: Option<(f32, f32)>,
    pub cover: Cover,
    /// When the pane was last on screen, for discarding idle engines.
    pub last_visible: Instant,
    /// The user typed into the page: never release its engine while hidden.
    pub keep_alive: bool,
    pub wake: Sender<()>,
}

impl SurfaceState {
    pub fn new(wake: Sender<()>) -> Self {
        Self {
            id: NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed),
            engine: None,
            placement: None,
            covered: false,
            park: false,
            park_size: None,
            cover: Cover::None,
            last_visible: Instant::now(),
            keep_alive: false,
            wake,
        }
    }

    /// Whether the live page should be on screen now.
    pub fn shows_engine(&self) -> bool {
        self.placement.is_some() && (!self.covered || matches!(self.cover, Cover::Capturing))
    }

    /// Re-apply visibility after the cover changed.
    pub fn sync_engine(&mut self) {
        let (placement, show) = (self.placement, self.shows_engine());
        Self::apply(&mut self.engine, placement, show, self.park, self.park_size);
    }

    fn apply(
        engine: &mut Option<Engine>,
        bounds: Option<RectF>,
        show: bool,
        park: bool,
        park_size: Option<(f32, f32)>,
    ) {
        let Some(engine) = engine else { return };
        match bounds {
            // On screen but covered, with a phone using it: keep it alive.
            Some(bounds) if !show && park => engine.park(bounds, park_size),
            _ => engine.place(bounds, show),
        }
    }

    fn placer(state: &Rc<RefCell<Self>>) -> NativeSurfacePlacer {
        let weak = Rc::downgrade(state);
        Rc::new(move |placement: NativePlacement| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let Ok(mut surface) = state.try_borrow_mut() else {
                return;
            };
            surface.place(placement, &weak);
        })
    }

    fn place(&mut self, placement: NativePlacement, weak: &Weak<RefCell<Self>>) {
        let bounds = placement.bounds();
        let covered = matches!(placement, NativePlacement::Covered(_));
        let was_visible = self.placement.is_some();
        let was_covered = self.covered;
        self.placement = bounds;
        self.covered = covered;
        if bounds.is_some() {
            self.last_visible = Instant::now();
        }
        let mut wake = was_visible != bounds.is_some() || (bounds.is_some() && self.engine.is_none());
        if !covered {
            self.cover = Cover::None;
        } else if !was_covered {
            let capture = self
                .engine
                .as_ref()
                .filter(|engine| engine.is_visible() && engine.can_capture());
            self.cover = match capture {
                Some(engine) => {
                    let weak = weak.clone();
                    // The live page stays up until the next frame draws the
                    // capture in its place, so the pane never flashes empty.
                    engine.capture(Box::new(move |image| {
                        let Some(state) = weak.upgrade() else {
                            return;
                        };
                        let Ok(mut surface) = state.try_borrow_mut() else {
                            return;
                        };
                        // A capture slower than the wait still replaces the
                        // empty pane while the menu is open.
                        if matches!(surface.cover, Cover::Capturing | Cover::Unavailable) {
                            surface.cover = match image {
                                Some(image) => Cover::Ready(Arc::new(image)),
                                None => Cover::Unavailable,
                            };
                            let _ = surface.wake.try_send(());
                        }
                    }));
                    Cover::Capturing
                }
                None => Cover::Unavailable,
            };
            wake = true;
        }
        let show = self.shows_engine();
        Self::apply(&mut self.engine, bounds, show, self.park, self.park_size);
        if wake {
            let _ = self.wake.try_send(());
        }
    }
}

pub(super) struct BrowserSurface {
    state: Rc<RefCell<SurfaceState>>,
    background: ColorU,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl BrowserSurface {
    pub fn new(state: Rc<RefCell<SurfaceState>>, background: ColorU) -> Self {
        Self {
            state,
            background,
            size: None,
            origin: None,
        }
    }
}

impl Element for BrowserSurface {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let pick = |max: f32, min: f32| if max.is_finite() { max } else { min };
        let size = Vector2F::new(
            pick(constraint.max.x(), constraint.min.x()),
            pick(constraint.max.y(), constraint.min.y()),
        );
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.size.unwrap_or_default();
        let area = RectF::new(origin, size);
        let (id, capture) = {
            let state = self.state.borrow();
            let capture = match (&state.cover, state.covered) {
                (Cover::Ready(image), true) => Some(image.clone()),
                _ => None,
            };
            (state.id, capture)
        };
        // Shown while the page starts, and wherever the guest is hidden.
        ctx.scene
            .draw_rect_without_hit_recording(area)
            .with_background(self.background);
        if let Some(image) = capture {
            ctx.scene.draw_image(area, image, 1.0, CornerRadius::default());
        }
        ctx.scene
            .add_native_surface(id, origin, size, SurfaceState::placer(&self.state));
    }

    fn dispatch_event(
        &mut self,
        _: &DispatchedEvent,
        _: &mut EventContext,
        _: &AppContext,
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
