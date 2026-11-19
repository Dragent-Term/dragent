//! Paint-only activity for the workspace's Git icon tile.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::OnceLock;
use std::time::Duration;

use warpui::color::ColorU;
use warpui::elements::{Align, Element, Point, SizeConstraint};
use warpui::geometry::{
    rect::RectF,
    vector::{vec2f, Vector2F},
};
use warpui::{AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext};
use warpui_core::event::DispatchedEvent;
use warpui_core::scene::{Border, CornerRadius, DropShadow, Radius};
use web_time::Instant;

use super::worktree_status::WorktreeAgentStatus;
use crate::icons::{render_icon, AppIcon};

/// About 14% under the original 22: the sidebar's icons sit smaller.
const SIDE: f32 = 19.0;
const RADIUS: f32 = 5.0;
const BORDER_WIDTH: f32 = 1.3;
/// The glyph inside a tile.
pub(crate) const TILE_ICON: f32 = 10.0;
/// A tile's side, for rows that draw a plain tile of the same size.
pub(crate) const TILE_SIDE: f32 = SIDE;
const LAP_SECONDS: f32 = 2.4;
const TRAIL_FRACTION: f32 = 0.28;
const TRAIL_SAMPLES: usize = 44;
/// The status mark pinned to the tile's bottom-right corner, as on the phone.
const MARK_SIDE: f32 = 10.5;
const MARK_DOT: f32 = 6.0;
const SPINNER_RADIUS: f32 = 2.9;
const SPINNER_STROKE: f32 = 1.7;
const SPINNER_SAMPLES: usize = 22;
/// The spinner's gap, as a share of its circle.
const SPINNER_GAP: f32 = 0.25;
const SPINNER_SECONDS: f32 = 0.9;

pub(crate) fn workspace_icon_tile(
    icon: AppIcon,
    tint: ColorU,
    background: ColorU,
    status: &WorktreeAgentStatus,
    accent: ColorU,
    theme: &crate::theme::Theme,
) -> Box<dyn Element> {
    let working = matches!(status, WorktreeAgentStatus::Working { .. });
    let mut tile = ActivityTile::new(
        render_icon(icon, if working { accent } else { tint }, TILE_ICON),
        background,
        accent,
        working,
    );
    tile.mark = status.mark_color(theme).map(|color| Mark {
        color,
        spinning: working,
        ring: theme.solid_background(),
        outline: theme.outline(),
    });
    tile.finish()
}

/// An agent's logo on the workspace tile's backing, with the same corner
/// mark (a spinner while it works, its status dot otherwise) but no lap
/// around the border: that stays the workspace's.
pub(crate) fn agent_badge_tile(
    glyph: Box<dyn Element>,
    background: ColorU,
    status: &WorktreeAgentStatus,
    theme: &crate::theme::Theme,
) -> Box<dyn Element> {
    let working = matches!(status, WorktreeAgentStatus::Working { .. });
    let mut tile = ActivityTile::new(glyph, background, theme.accent, false);
    tile.mark = status.mark_color(theme).map(|color| Mark {
        color,
        spinning: working,
        ring: theme.solid_background(),
        outline: theme.outline(),
    });
    tile.finish()
}

/// What the corner mark shows: a spinner while working, else a dot.
#[derive(Clone, Copy)]
struct Mark {
    color: ColorU,
    spinning: bool,
    /// Its backing, the sidebar's colour, cut out of the tile's corner.
    ring: ColorU,
    outline: ColorU,
}

struct ActivityTile {
    icon: Box<dyn Element>,
    background: ColorU,
    accent: ColorU,
    working: bool,
    mark: Option<Mark>,
    /// Fixed time is used by the preview and scene tests.
    elapsed: Option<Duration>,
    origin: Option<Point>,
}

impl ActivityTile {
    fn new(icon: Box<dyn Element>, background: ColorU, accent: ColorU, working: bool) -> Self {
        Self {
            icon: Align::new(icon).finish(),
            background,
            accent,
            working,
            mark: None,
            elapsed: None,
            origin: None,
        }
    }

    fn elapsed(&self) -> Duration {
        static START: OnceLock<Instant> = OnceLock::new();
        self.elapsed
            .unwrap_or_else(|| START.get_or_init(Instant::now).elapsed())
    }

    /// The corner mark: a backing disc, then a spinner or a dot.
    fn paint_mark(&self, origin: Vector2F, ctx: &mut PaintContext) {
        let Some(mark) = self.mark else {
            return;
        };
        let corner = origin + vec2f(SIDE - MARK_SIDE + 3.0, SIDE - MARK_SIDE + 3.0);
        let center = corner + vec2f(MARK_SIDE / 2.0, MARK_SIDE / 2.0);
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(corner, vec2f(MARK_SIDE, MARK_SIDE)))
            .with_background(mark.ring)
            .with_border(Border::all(1.0).with_border_color(mark.outline))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MARK_SIDE / 2.0)));
        if mark.spinning {
            // A ring with a gap, turning: round samples along the arc.
            let turn = (self.elapsed().as_secs_f32() / SPINNER_SECONDS).rem_euclid(1.0);
            for index in 0..SPINNER_SAMPLES {
                let along = index as f32 / (SPINNER_SAMPLES - 1) as f32 * (1.0 - SPINNER_GAP);
                let angle = (turn + along) * TAU - FRAC_PI_2;
                let point = center + vec2f(angle.cos(), angle.sin()) * SPINNER_RADIUS;
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        point - vec2f(SPINNER_STROKE / 2.0, SPINNER_STROKE / 2.0),
                        vec2f(SPINNER_STROKE, SPINNER_STROKE),
                    ))
                    .with_background(mark.color)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(SPINNER_STROKE / 2.0)));
            }
        } else {
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    center - vec2f(MARK_DOT / 2.0, MARK_DOT / 2.0),
                    vec2f(MARK_DOT, MARK_DOT),
                ))
                .with_background(mark.color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MARK_DOT / 2.0)));
        }
    }

    fn frame(&self) -> Option<(f32, f32)> {
        if !self.working {
            return None;
        }
        // A shared monotonic clock survives hover/status-driven tree rebuilds,
        // so they cannot restart a lap partway around the tile.
        let elapsed = self.elapsed();
        let phase = (elapsed.as_secs_f32() / LAP_SECONDS).rem_euclid(1.0);
        let glow = 0.5 - 0.5 * (phase * TAU).cos();
        Some((phase, glow))
    }
}

/// Constant-speed clockwise travel along the rounded square's centerline.
/// Each side consists of its straight edge followed by a quarter-circle.
fn perimeter_point(phase: f32) -> Vector2F {
    let inset = BORDER_WIDTH / 2.0 + 0.2;
    let side = SIDE - 2.0 * inset;
    let radius = RADIUS - inset;
    let straight = side - 2.0 * radius;
    let sector = straight + FRAC_PI_2 * radius;
    let distance = phase.rem_euclid(1.0) * sector * 4.0;
    let quadrant = (distance / sector).floor() as usize;
    let local = distance % sector;
    let (x, y) = if local <= straight {
        (radius + local, 0.0)
    } else {
        let angle = (local - straight) / radius;
        (
            side - radius + radius * angle.sin(),
            radius - radius * angle.cos(),
        )
    };
    let point = match quadrant % 4 {
        0 => vec2f(x, y),
        1 => vec2f(side - y, x),
        2 => vec2f(side - x, side - y),
        _ => vec2f(y, side - x),
    };
    point + vec2f(inset, inset)
}

impl Element for ActivityTile {
    fn layout(&mut self, _: SizeConstraint, ctx: &mut LayoutContext, app: &AppContext) -> Vector2F {
        let size = vec2f(SIDE, SIDE);
        self.icon.layout(SizeConstraint::new(size, size), ctx, app);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.icon.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let frame = self.frame();
        let color = |alpha| ColorU::new(self.accent.r, self.accent.g, self.accent.b, alpha);
        let tile = ctx
            .scene
            .draw_rect_with_hit_recording(RectF::new(origin, vec2f(SIDE, SIDE)))
            .with_background(self.background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS)));
        if let Some((_, glow)) = frame {
            tile.with_border(Border::all(0.7).with_border_color(color(38)));
            // A soft halo behind the glyph makes its light breathe gently.
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    origin + vec2f(6.0, 5.5),
                    vec2f(7.0, 8.0),
                ))
                .with_background(color((10.0 + 18.0 * glow) as u8))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_drop_shadow(DropShadow {
                    color: color((38.0 + 48.0 * glow) as u8),
                    offset: vec2f(0.0, 0.0),
                    blur_radius: 4.0,
                    spread_radius: 1.0,
                });
        }
        self.icon.paint(origin, ctx, app);
        if let Some((phase, _)) = frame {
            // Closely spaced round samples produce a continuous thin stroke
            // around corners, with a fading tail and a bright leading tip.
            for index in 0..TRAIL_SAMPLES {
                let t = index as f32 / (TRAIL_SAMPLES - 1) as f32;
                let center = origin + perimeter_point(phase - TRAIL_FRACTION * (1.0 - t));
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        center - vec2f(BORDER_WIDTH / 2.0, BORDER_WIDTH / 2.0),
                        vec2f(BORDER_WIDTH, BORDER_WIDTH),
                    ))
                    .with_background(color((255.0 * t.powf(1.6)) as u8))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BORDER_WIDTH / 2.0)));
            }
        }
        self.paint_mark(origin, ctx);
        let spinning = self.mark.is_some_and(|mark| mark.spinning);
        if (frame.is_some() || spinning) && self.elapsed.is_none() {
            // 20 fps: each tick repaints the whole window for as long as work
            // is running (see the shimmer's rate for why this isn't 30).
            ctx.repaint_after(Duration::from_millis(50));
        }
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
        Some(vec2f(SIDE, SIDE))
    }
    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Production tiles at successive points in one lap, for visual QA.
pub(crate) fn preview(
    font: warpui::fonts::FamilyId,
    theme: &crate::theme::Theme,
) -> Box<dyn Element> {
    use warpui::elements::{
        ConstrainedBox, Container, CrossAxisAlignment, Expanded, Flex, ParentElement, Text,
    };
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(24.0);
    for (label, elapsed, status) in [
        (
            "Working · start",
            0.0,
            WorktreeAgentStatus::Working {
                label: String::new(),
            },
        ),
        (
            "Working · quarter lap",
            0.6,
            WorktreeAgentStatus::Working {
                label: String::new(),
            },
        ),
        (
            "Working · half lap",
            1.2,
            WorktreeAgentStatus::Working {
                label: String::new(),
            },
        ),
        (
            "Working · three-quarter lap",
            1.8,
            WorktreeAgentStatus::Working {
                label: String::new(),
            },
        ),
        (
            "Finished · ready for review",
            0.0,
            WorktreeAgentStatus::Review,
        ),
        ("Needs input", 0.0, WorktreeAgentStatus::Permission),
        ("Failed", 0.0, WorktreeAgentStatus::Failed),
        ("Idle", 0.0, WorktreeAgentStatus::Idle),
    ] {
        let working = matches!(status, WorktreeAgentStatus::Working { .. });
        let mut tile = ActivityTile::new(
            render_icon(
                AppIcon::GitBranch,
                if working {
                    theme.accent
                } else {
                    theme.main_text()
                },
                TILE_ICON,
            ),
            theme.surface_2(),
            theme.accent,
            working,
        );
        tile.mark = status.mark_color(theme).map(|color| Mark {
            color,
            spinning: working,
            ring: theme.solid_background(),
            outline: theme.outline(),
        });
        tile.elapsed = Some(Duration::from_secs_f32(elapsed));
        rows = rows.with_child(
            ConstrainedBox::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(14.0)
                    .with_child(tile.finish())
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(label.to_string(), font, 13.0)
                                .with_color(theme.main_text())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .with_width(320.0)
            .finish(),
        );
    }
    Container::new(rows.finish())
        .with_padding_top(36.0)
        .with_padding_bottom(36.0)
        .with_padding_left(36.0)
        .with_padding_right(36.0)
        .with_background_color(theme.sidebar_background())
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use warpui::platform::WindowStyle;
    use warpui::{App, Entity, Presenter, UpdateView, View, ViewContext, WindowInvalidation};

    #[test]
    fn highlight_completes_a_closed_clockwise_lap_without_corner_jumps() {
        let first = perimeter_point(0.0);
        assert!((first - perimeter_point(1.0)).length() < 0.001);
        assert!((first - perimeter_point(-1.0)).length() < 0.001);
        let quarter = perimeter_point(0.25);
        let half = perimeter_point(0.5);
        let three_quarters = perimeter_point(0.75);
        assert!(quarter.x() > first.x() && quarter.y() > first.y());
        assert!(half.x() < quarter.x() && half.y() > quarter.y());
        assert!(three_quarters.x() < half.x() && three_quarters.y() < half.y());
        let mut previous = first;
        for step in 1..=500 {
            let point = perimeter_point(step as f32 / 500.0);
            assert!(point.x() >= 0.0 && point.x() <= SIDE);
            assert!(point.y() >= 0.0 && point.y() <= SIDE);
            assert!((point - previous).length() < 0.2);
            previous = point;
        }
    }

    #[test]
    fn a_full_lap_repeats_the_motion_and_glow_smoothly() {
        use warpui::elements::Empty;
        let mut tile = ActivityTile::new(
            Empty::new().finish(),
            ColorU::black(),
            ColorU::white(),
            true,
        );
        tile.elapsed = Some(Duration::ZERO);
        let start = tile.frame().unwrap();
        tile.elapsed = Some(Duration::from_secs_f32(LAP_SECONDS / 2.0));
        let middle = tile.frame().unwrap();
        assert!(middle.1 > start.1 + 0.9);
        tile.elapsed = Some(Duration::from_secs_f32(LAP_SECONDS));
        let end = tile.frame().unwrap();
        assert!((start.0 - end.0).abs() < 0.001);
        assert!((start.1 - end.1).abs() < 0.001);
        tile.working = false;
        assert!(tile.frame().is_none());
    }

    struct Harness {
        status: WorktreeAgentStatus,
    }

    impl Entity for Harness {
        type Event = ();
    }

    impl warpui::TypedActionView for Harness {
        type Action = ();
    }

    impl View for Harness {
        fn ui_name() -> &'static str {
            "WorkspaceActivityHarness"
        }
        fn render(&self, _: &AppContext) -> Box<dyn Element> {
            workspace_icon_tile(
                AppIcon::GitBranch,
                ColorU::white(),
                ColorU::black(),
                &self.status,
                ColorU::new(0, 200, 230, 255),
                &crate::theme::Theme::default(),
            )
        }
    }

    #[test]
    fn native_tile_removes_the_animated_border_and_glow_when_work_stops() {
        App::test(dragent_assets::DragentAssets, |mut app| async move {
            let (window, harness) = app.add_window(WindowStyle::NotStealFocus, |_| Harness {
                status: WorktreeAgentStatus::Idle,
            });
            let mut presenter = Presenter::new(window);
            for (status, animated) in [
                (
                    WorktreeAgentStatus::Working {
                        label: "Thinking".into(),
                    },
                    true,
                ),
                (WorktreeAgentStatus::Permission, false),
                (
                    WorktreeAgentStatus::Working {
                        label: "Editing".into(),
                    },
                    true,
                ),
                (WorktreeAgentStatus::Review, false),
                (WorktreeAgentStatus::Failed, false),
                (WorktreeAgentStatus::Idle, false),
            ] {
                let shown = status.clone();
                harness.update(&mut app, |view, ctx: &mut ViewContext<Harness>| {
                    view.status = shown;
                    ctx.notify();
                });
                app.update(|ctx| {
                    presenter.invalidate(
                        WindowInvalidation {
                            updated: HashSet::from([harness.id()]),
                            ..Default::default()
                        },
                        ctx,
                    );
                    let scene = presenter.build_scene(vec2f(100.0, 100.0), 1.0, None, ctx);
                    let rects: Vec<_> = scene.layers().flat_map(|layer| &layer.rects).collect();
                    assert_eq!(
                        rects.iter().any(|rect| rect.drop_shadow.is_some()),
                        animated
                    );
                    // The tile, then (while working) its halo and moving
                    // border; then the corner mark: a disc holding a
                    // spinner while working, a dot otherwise; none when idle.
                    let idle = matches!(status, WorktreeAgentStatus::Idle);
                    let mark = if idle {
                        0
                    } else if animated {
                        1 + SPINNER_SAMPLES
                    } else {
                        2
                    };
                    let tile = if animated { TRAIL_SAMPLES + 2 } else { 1 };
                    assert_eq!(rects.len(), tile + mark, "{status:?}");
                    assert_eq!(rects[0].bounds.size(), vec2f(SIDE, SIDE));
                });
            }
        });
    }
}

/// A light that laps the edge of a rounded box while something starts: the
/// same trail the workspace tile's lap draws, around any size of box. The
/// child is drawn and receives events as usual.
pub(crate) struct EdgeLap {
    child: Box<dyn Element>,
    color: ColorU,
    radius: f32,
    origin: Option<Point>,
    size: Option<Vector2F>,
}

impl EdgeLap {
    pub(crate) fn new(child: Box<dyn Element>, color: ColorU, radius: f32) -> Self {
        Self { child, color, radius, origin: None, size: None }
    }
}

/// A point at `t` (0..1) along a rounded rectangle's edge, clockwise from
/// the top-left corner's end, at constant speed.
fn rounded_rect_point(size: Vector2F, radius: f32, t: f32) -> Vector2F {
    let (w, h) = (size.x(), size.y());
    let r = radius.min(w / 2.0).min(h / 2.0);
    let straight_x = (w - 2.0 * r).max(0.0);
    let straight_y = (h - 2.0 * r).max(0.0);
    let arc = FRAC_PI_2 * r;
    let total = 2.0 * (straight_x + straight_y) + 4.0 * arc;
    let mut d = t.rem_euclid(1.0) * total;
    // Top edge, top-right corner, right edge, bottom-right, bottom, bottom-left, left, top-left.
    let corner = |cx: f32, cy: f32, start: f32, along: f32| {
        let angle = start + along / r.max(0.001);
        vec2f(cx + r * angle.cos(), cy + r * angle.sin())
    };
    if d < straight_x { return vec2f(r + d, 0.0); }
    d -= straight_x;
    if d < arc { return corner(w - r, r, -FRAC_PI_2, d); }
    d -= arc;
    if d < straight_y { return vec2f(w, r + d); }
    d -= straight_y;
    if d < arc { return corner(w - r, h - r, 0.0, d); }
    d -= arc;
    if d < straight_x { return vec2f(w - r - d, h); }
    d -= straight_x;
    if d < arc { return corner(r, h - r, FRAC_PI_2, d); }
    d -= arc;
    if d < straight_y { return vec2f(0.0, h - r - d); }
    d -= straight_y;
    corner(r, r, std::f32::consts::PI, d.min(arc))
}

impl Element for EdgeLap {
    fn layout(&mut self, constraint: SizeConstraint, ctx: &mut LayoutContext, app: &AppContext) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
        let Some(size) = self.size else { return };
        static START: OnceLock<Instant> = OnceLock::new();
        // About three seconds a lap, whatever the box's size.
        let head = (START.get_or_init(Instant::now).elapsed().as_secs_f32() / 3.0).rem_euclid(1.0);
        const TAIL: f32 = 0.22;
        const STROKE: f32 = 1.8;
        // Close enough together to read as one line on any size of box.
        let perimeter = 2.0 * (size.x() + size.y());
        let samples = ((perimeter * TAIL / 0.9) as usize).clamp(60, 600);
        for index in 0..samples {
            let t = index as f32 / (samples - 1) as f32;
            let at = origin + rounded_rect_point(size, self.radius, head - TAIL * (1.0 - t));
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(at - vec2f(STROKE / 2.0, STROKE / 2.0), vec2f(STROKE, STROKE)))
                .with_background(ColorU::new(self.color.r, self.color.g, self.color.b, (255.0 * t.powf(1.6)) as u8))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(STROKE / 2.0)));
        }
        ctx.repaint_after(Duration::from_millis(33));
    }

    fn dispatch_event(&mut self, event: &DispatchedEvent, ctx: &mut EventContext, app: &AppContext) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// A soft ring that comes and goes around an agent's face, one face after
/// another (`index` of `count`), while a team gathers.
pub(crate) struct FacePulse {
    child: Box<dyn Element>,
    index: usize,
    count: usize,
    color: ColorU,
    origin: Option<Point>,
    size: Option<Vector2F>,
}

impl FacePulse {
    pub(crate) fn new(child: Box<dyn Element>, index: usize, count: usize, color: ColorU) -> Self {
        Self { child, index, count: count.max(1), color, origin: None, size: None }
    }
}

impl Element for FacePulse {
    fn layout(&mut self, constraint: SizeConstraint, ctx: &mut LayoutContext, app: &AppContext) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
        let Some(size) = self.size else { return };
        static START: OnceLock<Instant> = OnceLock::new();
        // Each face has its turn in a 0.7 s slot; the ring rises and fades in it.
        let slot = 0.7;
        let cycle = slot * self.count as f32;
        let now = START.get_or_init(Instant::now).elapsed().as_secs_f32().rem_euclid(cycle);
        let local = ((now - slot * self.index as f32) / slot).clamp(0.0, 1.0);
        let glow = (local * std::f32::consts::PI).sin();
        if glow > 0.01 {
            let ring = 1.5;
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(origin - vec2f(ring, ring), size + vec2f(2.0 * ring, 2.0 * ring)))
                .with_border(Border::all(ring).with_border_color(ColorU::new(self.color.r, self.color.g, self.color.b, (230.0 * glow) as u8)))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size.x() / 2.0 + ring)));
        }
        ctx.repaint_after(Duration::from_millis(33));
    }

    fn dispatch_event(&mut self, event: &DispatchedEvent, ctx: &mut EventContext, app: &AppContext) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
