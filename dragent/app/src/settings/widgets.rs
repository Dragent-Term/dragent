//! Shared settings row widgets — Dragent settings design system.
//! Hub: layout constants + re-exports of the widget submodules.

pub const NAV_WIDTH: f32 = 226.0;
/// Collapsed icon-rail width while the theme chooser is open.
pub const NAV_RAIL_WIDTH: f32 = 56.0;
pub const CONTENT_MAX_WIDTH: f32 = 1040.0;
pub const CONTENT_PADDING: f32 = 40.0;
pub const PAGE_TITLE_SIZE: f32 = 20.0;
pub const PAGE_CAPTION_SIZE: f32 = 13.0;
pub const CATEGORY_LABEL_SIZE: f32 = 10.5;
pub const NAV_ITEM_RADIUS: f32 = 8.0;
pub const NAV_ITEM_H_MARGIN: f32 = 10.0;
pub const SHORTCUT_ROW_HEIGHT: f32 = 44.0;
pub const SHORTCUT_ROW_PADDING_X: f32 = 20.0;
pub const SHORTCUT_ROW_PADDING_Y: f32 = 10.0;

pub(crate) use super::widgets_controls::*;
pub(crate) use super::widgets_mcp::*;
pub(crate) use super::widgets_nav::*;
pub(crate) use super::widgets_panels::*;
pub(crate) use super::widgets_rows::*;
pub(crate) use super::widgets_shortcuts::*;

pub(crate) fn with_alpha(color: warpui::color::ColorU, alpha: f32) -> warpui::color::ColorU {
    warpui::color::ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}
