//! Shared visual grammar for native IDE surfaces.
//!
//! These values are intentionally semantic rather than a bag of pixel
//! constants. Editor tabs, search bands, diagnostics, Git review, tests, and
//! future language-service surfaces should compose from this vocabulary so the
//! workspace feels designed as one system.

use warpui::color::ColorU;
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, Element, Fill, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Stack,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui_core::ui_components::tool_tip::Tooltip;

use crate::theme::{Theme, PANEL_HEADER_HEIGHT};

pub struct IdeMetrics;

impl IdeMetrics {
    pub const TAB_WIDTH: f32 = 205.0;
    pub const TAB_ICON: f32 = 16.0;
    pub const TAB_TRAILING: f32 = 20.0;
    pub const TAB_GAP: f32 = 7.0;
    pub const TAB_PAD_LEFT: f32 = 11.0;
    pub const TAB_PAD_RIGHT: f32 = 6.0;
    // Tab content fills the shared bar exactly: rail 2 + pad 5.5 +
    // trailing slot 20 + pad 5.5 = 33, the full content box inside
    // `nav_header_bar`, so the tab's bottom edge sits directly on the
    // divider with no gap. Exact fit is safe here: the row height is
    // pinned by the fixed 20px trailing slot (12px text can never push
    // past it), so font metrics cannot overflow.
    pub const TAB_PAD_Y: f32 = 5.5;
    pub const TAB_ACTIVE_RAIL: f32 = 2.0;

    pub const TOOLBAR_HEIGHT: f32 = 36.0;
    pub const CONTROL_SIZE: f32 = 28.0;
    pub const CONTROL_GAP: f32 = 4.0;
    pub const PANEL_GUTTER: f32 = 12.0;
    pub const READING_GUTTER: f32 = 24.0;
}

/// Shared navbar geometry. Every header in a split row — terminal pane
/// header, editor tab bar, breadcrumb bar, sidebar tabs, Changes/History —
/// wraps its content row in this, so all bottom dividers sit on one line:
///
/// ```text
/// +-------------+----------------+-------------+
/// ```
///
/// Rules (terminal header is the reference):
/// - `min_height` is PANEL_HEADER_HEIGHT (34) for every bar, except the
///   file tab bar, which passes 0.0: it shrink-fits its cards
///   (`navbar height = file card height`), so no empty space can sit
///   between the cards and the divider.
/// - zero vertical padding, zero margin: inner content centers in the
///   leftover space instead of pushing its bar taller.
/// - one bottom border in `outline` at the same position in every bar.
/// - minimum, never fixed, height: if content ever overflows, the bar grows
///   instead of clipping labels. Content below is sized to fit, so growth
///   never happens in practice.
///
/// Different content, same header geometry: callers pass any content row
/// (tabs, breadcrumb, tab buttons) plus their horizontal padding. Nothing
/// inside may add vertical padding that exceeds the shared height.
pub fn nav_header_bar(
    content: Box<dyn Element>,
    theme: &Theme,
    padding_left: f32,
    padding_right: f32,
    min_height: f32,
) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(content)
            .with_background_color(theme.panel_background())
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .with_padding_left(padding_left)
            .with_padding_right(padding_right)
            .finish(),
    )
    .with_min_height(min_height)
    .finish()
}

pub fn tab_background(theme: &Theme, active: bool, hovered: bool) -> ColorU {
    if active {
        // Active work belongs visually to the canvas, not to the tab rail.
        theme.pane_surface()
    } else if hovered {
        theme.surface_2()
    } else {
        theme.panel_background()
    }
}

pub fn tab_active_rail(theme: &Theme, active: bool) -> ColorU {
    if active {
        theme.accent
    } else {
        ColorU::new(0, 0, 0, 0)
    }
}

pub fn tab_divider(theme: &Theme) -> ColorU {
    theme.outline()
}

/// Attach the standard IDE hover help above a compact toolbar/status control.
/// Keeping this here prevents every editor surface from inventing its own
/// tooltip spacing, type scale, and contrast.
pub(crate) fn tooltip_above(
    child: Box<dyn Element>,
    hovered: bool,
    label: impl Into<String>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    if !hovered {
        return child;
    }
    let styles = UiComponentStyles {
        font_family_id: Some(font),
        font_size: Some(crate::theme::text_size(10.5)),
        font_color: Some(theme.main_text()),
        background: Some(Fill::Solid(theme.surface_2())),
        border_color: Some(Fill::Solid(theme.outline())),
        border_width: Some(1.0),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(5.0))),
        padding: Some(Coords {
            top: 4.0,
            bottom: 4.0,
            left: 7.0,
            right: 7.0,
        }),
        ..Default::default()
    };
    let tooltip = Tooltip::new(label.into(), styles).build().finish();
    Stack::new()
        .with_child(child)
        .with_positioned_overlay_child(
            tooltip,
            OffsetPositioning::offset_from_parent(
                vec2f(0.0, -6.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::TopMiddle,
                ChildAnchor::BottomMiddle,
            ),
        )
        .finish()
}

/// Editor toolbars sit at the top of their surface, so their hover help opens
/// below the control instead of being clipped against the window chrome.
pub(crate) fn tooltip_below(
    child: Box<dyn Element>,
    hovered: bool,
    label: impl Into<String>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    if !hovered {
        return child;
    }
    let styles = UiComponentStyles {
        font_family_id: Some(font),
        font_size: Some(crate::theme::text_size(10.5)),
        font_color: Some(theme.main_text()),
        background: Some(Fill::Solid(theme.surface_2())),
        border_color: Some(Fill::Solid(theme.outline())),
        border_width: Some(1.0),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(5.0))),
        padding: Some(Coords {
            top: 4.0,
            bottom: 4.0,
            left: 7.0,
            right: 7.0,
        }),
        ..Default::default()
    };
    let tooltip = Tooltip::new(label.into(), styles).build().finish();
    Stack::new()
        .with_child(child)
        .with_positioned_overlay_child(
            tooltip,
            OffsetPositioning::offset_from_parent(
                vec2f(0.0, 6.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::BottomMiddle,
                ChildAnchor::TopMiddle,
            ),
        )
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_tab_uses_accent_as_its_focus_rail() {
        let theme = Theme::dark();
        assert_eq!(tab_active_rail(&theme, true), theme.accent);
        assert_eq!(tab_active_rail(&theme, false), ColorU::new(0, 0, 0, 0));
        assert_eq!(tab_divider(&theme), theme.outline());
    }
}
