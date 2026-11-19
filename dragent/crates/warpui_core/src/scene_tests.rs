use pathfinder_geometry::vector::vec2f;

use super::*;
use crate::rendering;

#[test]
fn test_hit_rect_recording() {
    let mut scene = Scene::new(1., rendering::Config::default());
    assert_eq!(ZIndex::new(0), scene.z_index());

    scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(100., 100.)));
    assert_eq!(ZIndex::new(0), scene.z_index());
    assert!(!scene.is_covered(Point::new(10., 10., ZIndex::new(0))));

    scene.start_layer(ClipBounds::ActiveLayer);
    scene.draw_rect_with_hit_recording(RectF::new(vec2f(50., 50.), vec2f(100., 100.)));
    assert_eq!(ZIndex::new(1), scene.z_index());
    assert!(!scene.is_covered(Point::new(10., 10., ZIndex::new(0))));
    assert!(scene.is_covered(Point::new(60., 60., ZIndex::new(0))));

    scene.start_layer(ClipBounds::ActiveLayer);
    scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(100., 100.)));
    assert_eq!(ZIndex::new(2), scene.z_index());
    assert!(scene.is_covered(Point::new(10., 10., ZIndex::new(0))));
    assert!(scene.is_covered(Point::new(60., 60., ZIndex::new(1))));
}

#[test]
fn test_nested_clip_bounds_with_intersection() {
    let mut scene = Scene::new(1., rendering::Config::default());

    let bounds1 = RectF::new(Vector2F::zero(), Vector2F::new(10., 10.));
    scene.start_layer(ClipBounds::BoundedBy(bounds1));

    let bounds2 = RectF::new(Vector2F::new(5., 5.), Vector2F::new(10., 10.));
    scene.start_layer(ClipBounds::BoundedByActiveLayerAnd(bounds2));

    assert_eq!(
        scene.active_layer().clip_bounds,
        Some(RectF::new(Vector2F::new(5., 5.), Vector2F::new(5., 5.)))
    );
}

#[test]
fn test_nested_clip_bounds_with_no_intersection() {
    let mut scene = Scene::new(1., rendering::Config::default());

    let bounds1 = RectF::new(Vector2F::zero(), Vector2F::new(10., 10.));
    scene.start_layer(ClipBounds::BoundedBy(bounds1));

    let bounds2 = RectF::new(Vector2F::new(100., 100.), Vector2F::new(10., 10.));
    scene.start_layer(ClipBounds::BoundedByActiveLayerAnd(bounds2));

    // We should have explicit bounds for this layer.  (None represents a lack
    // of clipping, not clipping they layer down to nothingness.)
    assert!(scene.active_layer().clip_bounds.is_some());
    // The clip bounds should have an area of zero.
    assert!(scene.active_layer().clip_bounds.unwrap().is_empty());
}

#[test]
fn test_click_through_layer_does_not_cover_lower_layers() {
    let mut scene = Scene::new(1., rendering::Config::default());

    scene.start_layer(ClipBounds::ActiveLayer);
    scene.set_active_layer_click_through();
    scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(100., 100.)));

    assert!(!scene.is_covered(Point::new(10., 10., ZIndex::new(0))));
}

fn surface_scene() -> (Scene, NativeSurface) {
    let mut scene = Scene::new(1., rendering::Config::default());
    scene.start_layer(ClipBounds::ActiveLayer);
    scene.add_native_surface(
        7,
        vec2f(100., 100.),
        vec2f(400., 300.),
        Rc::new(|_: NativePlacement| {}),
    );
    scene.stop_layer();
    let surface = scene.native_surfaces()[0].clone();
    (scene, surface)
}

#[test]
fn native_surface_is_placed_when_nothing_covers_it() {
    let (mut scene, surface) = surface_scene();
    // Content painted beside the surface, or below it in an earlier layer,
    // leaves it visible.
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(800., 600.)))
        .with_background(ColorU::new(10, 10, 10, 255));
    scene.start_layer(ClipBounds::ActiveLayer);
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(520., 100.), vec2f(100., 100.)))
        .with_background(ColorU::new(10, 10, 10, 255));
    scene.stop_layer();
    assert_eq!(
        scene.native_surface_placement(&surface),
        NativePlacement::Shown(RectF::new(vec2f(100., 100.), vec2f(400., 300.)))
    );
}

#[test]
fn opaque_menu_above_a_native_surface_hides_it() {
    let (mut scene, surface) = surface_scene();
    scene.start_overlay_layer(ClipBounds::None);
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(80., 80.), vec2f(200., 160.)))
        .with_background(ColorU::new(30, 30, 30, 255));
    scene.stop_layer();
    assert_eq!(
        scene.native_surface_placement(&surface),
        NativePlacement::Covered(RectF::new(vec2f(100., 100.), vec2f(400., 300.)))
    );
}

#[test]
fn invisible_translucent_and_hairline_content_keeps_a_native_surface() {
    let (mut scene, surface) = surface_scene();
    scene.start_overlay_layer(ClipBounds::None);
    // A click-outside catcher, a dimming veil, a focus border, and a divider
    // handle straddling the edge.
    scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(800., 600.)));
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(800., 600.)))
        .with_background(ColorU::new(0, 0, 0, 60));
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(100., 100.), vec2f(400., 300.)))
        .with_border(Border::all(1.).with_border_color(ColorU::new(255, 255, 255, 255)));
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(96., 100.), vec2f(8., 300.)))
        .with_background(ColorU::new(80, 80, 80, 255));
    scene.stop_layer();
    assert!(matches!(scene.native_surface_placement(&surface), NativePlacement::Shown(_)));
}

#[test]
fn clipped_away_content_does_not_hide_a_native_surface() {
    let (mut scene, surface) = surface_scene();
    scene.start_layer(ClipBounds::BoundedBy(RectF::new(
        vec2f(0., 0.),
        vec2f(90., 600.),
    )));
    scene
        .draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(800., 600.)))
        .with_background(ColorU::new(30, 30, 30, 255));
    scene.stop_layer();
    assert!(matches!(scene.native_surface_placement(&surface), NativePlacement::Shown(_)));
}

#[test]
fn native_surface_is_clipped_to_its_layer() {
    let mut scene = Scene::new(1., rendering::Config::default());
    scene.start_layer(ClipBounds::BoundedBy(RectF::new(
        vec2f(0., 0.),
        vec2f(300., 250.),
    )));
    scene.add_native_surface(1, vec2f(100., 100.), vec2f(400., 300.), Rc::new(|_| {}));
    scene.stop_layer();
    let surface = scene.native_surfaces()[0].clone();
    assert_eq!(
        scene.native_surface_placement(&surface),
        NativePlacement::Shown(RectF::new(vec2f(100., 100.), vec2f(200., 150.)))
    );

    let mut scene = Scene::new(1., rendering::Config::default());
    scene.start_layer(ClipBounds::BoundedBy(RectF::new(
        vec2f(600., 0.),
        vec2f(100., 100.),
    )));
    scene.add_native_surface(1, vec2f(100., 100.), vec2f(400., 300.), Rc::new(|_| {}));
    scene.stop_layer();
    let surface = scene.native_surfaces()[0].clone();
    assert_eq!(scene.native_surface_placement(&surface), NativePlacement::Hidden);
}
