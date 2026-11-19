//! Shared finite canvas for the Shape and Coordinate workspace presentations.
//!
//! Work layouts retain their information architecture when a window becomes
//! narrow. A two-axis viewport exposes overflow instead of squeezing cards
//! until they overlap or removing useful surfaces.

use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance,
};
use warpui::elements::{
    ClippedScrollStateHandle, ConstrainedBox, Element, NewScrollable, ScrollbarWidth,
};

use crate::theme::Theme;

fn finite_canvas_dimension(viewport: f32, minimum: f32) -> f32 {
    let minimum = if minimum.is_finite() {
        minimum.max(1.0)
    } else {
        1.0
    };
    if viewport.is_finite() {
        viewport.max(minimum).max(1.0)
    } else {
        minimum
    }
}

pub(super) fn render(
    theme: &Theme,
    horizontal: ClippedScrollStateHandle,
    vertical: ClippedScrollStateHandle,
    viewport_width: f32,
    viewport_height: f32,
    minimum_width: f32,
    // Pinned canvas height. None sizes the canvas to its content — required
    // when narrow layouts stack taller than any viewport, otherwise the
    // fixed wide-mode height clips the overflow beyond scroll reach.
    // Callers passing None must keep every descendant content-sized
    // (Min, no Expanded): NewScrollable measures them unbounded and a
    // Max-sized Flex below that panics in debug builds.
    minimum_height: Option<f32>,
    child: Box<dyn Element>,
) -> Box<dyn Element> {
    // Exact finite dimensions are important here. NewScrollable lays clipped
    // children out with unbounded constraints; a Max-sized Flex below that
    // would otherwise receive infinity and panic in debug builds.
    let canvas_width = finite_canvas_dimension(viewport_width, minimum_width);
    let canvas_height =
        minimum_height.map(|minimum| finite_canvas_dimension(viewport_height, minimum));
    let mut canvas = ConstrainedBox::new(child).with_width(canvas_width);
    if let Some(height) = canvas_height {
        canvas = canvas.with_height(height);
    }
    let canvas = canvas.finish();

    NewScrollable::horizontal_and_vertical(
        DualAxisConfig::Clipped {
            horizontal: ClippedAxisConfiguration {
                handle: horizontal,
                max_size: Some(canvas_width),
                stretch_child: false,
            },
            vertical: ClippedAxisConfiguration {
                handle: vertical,
                max_size: canvas_height,
                stretch_child: false,
            },
            child: canvas,
        },
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_horizontal_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
    .with_vertical_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
    .with_scrollbar_padding(2.0, 2.0)
    .with_wheel_locked_to_dominant_axis(true)
    .finish()
}

#[cfg(test)]
mod tests {
    use super::finite_canvas_dimension;

    #[test]
    fn constrained_canvas_preserves_a_finite_minimum_for_narrow_viewports() {
        assert_eq!(finite_canvas_dimension(320.0, 1_142.0), 1_142.0);
        assert_eq!(finite_canvas_dimension(1_600.0, 1_142.0), 1_600.0);
    }

    #[test]
    fn constrained_canvas_never_forwards_unbounded_or_invalid_dimensions() {
        assert_eq!(finite_canvas_dimension(f32::INFINITY, 1_142.0), 1_142.0);
        assert_eq!(finite_canvas_dimension(f32::NEG_INFINITY, 720.0), 720.0);
        assert_eq!(finite_canvas_dimension(f32::NAN, 720.0), 720.0);
        assert_eq!(finite_canvas_dimension(0.0, f32::INFINITY), 1.0);
    }
}
