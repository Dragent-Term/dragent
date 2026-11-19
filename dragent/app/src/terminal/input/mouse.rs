//! Map UI pointer events to terminal grid cells using measured [`SizeInfo`].

use dragent_terminal::{MouseButton, MouseEvent, MouseEventKind, MouseModifiers};
use warpui::geometry::vector::Vector2F;
use warpui_core::event::ModifiersState;

use crate::terminal::model::SizeInfo;
use crate::terminal::render::grid_renderer::ALT_GRID_PADDING_TOP;
use crate::theme::TERMINAL_PADDING_LEFT;

pub fn modifiers_from_ui(mods: ModifiersState) -> MouseModifiers {
    MouseModifiers {
        shift: mods.shift,
        alt: mods.alt,
        ctrl: mods.ctrl,
    }
}

/// Map a pointer position inside the alt-screen viewport to a zero-based grid cell.
pub fn map_pointer_to_cell(
    position: Vector2F,
    viewport_origin: Vector2F,
    size: SizeInfo,
    scroll_top_px: f32,
) -> Option<(usize, usize)> {
    let x = position.x() - viewport_origin.x() - TERMINAL_PADDING_LEFT;
    let y = position.y() - viewport_origin.y() - ALT_GRID_PADDING_TOP + scroll_top_px;

    let col_idx = (x / size.cell_width).floor() as i32;
    let row_idx = (y / size.cell_height).floor() as i32;

    let col = col_idx.clamp(0, size.cols as i32 - 1) as usize;
    let row = row_idx.clamp(0, size.rows as i32 - 1) as usize;

    Some((row, col))
}

/// Like [`map_pointer_to_cell`] but returns `None` when the pointer is OUTSIDE
/// the grid instead of clamping to the nearest edge cell.
///
/// Used for drag-EXTEND: clamping there means overshooting the viewport (e.g.
/// dragging down into the AI input panel below a shortened alt-screen while the
/// rich input is open) snaps the selection to the last row/column and appears to
/// "highlight the whole page". Ignoring out-of-bounds motion keeps the selection
/// where the pointer actually is.
pub fn map_pointer_to_cell_in_bounds(
    position: Vector2F,
    viewport_origin: Vector2F,
    size: SizeInfo,
    scroll_top_px: f32,
) -> Option<(usize, usize)> {
    let x = position.x() - viewport_origin.x() - TERMINAL_PADDING_LEFT;
    let y = position.y() - viewport_origin.y() - ALT_GRID_PADDING_TOP + scroll_top_px;
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let col_idx = (x / size.cell_width).floor() as i32;
    let row_idx = (y / size.cell_height).floor() as i32;
    if col_idx < 0 || col_idx >= size.cols as i32 || row_idx < 0 || row_idx >= size.rows as i32 {
        return None;
    }
    Some((row_idx as usize, col_idx as usize))
}

pub fn mouse_event(
    button: MouseButton,
    kind: MouseEventKind,
    row: usize,
    col: usize,
    modifiers: MouseModifiers,
) -> MouseEvent {
    MouseEvent {
        button,
        col,
        row,
        modifiers,
        kind,
    }
}

pub fn wheel_button(delta_y: f32) -> MouseButton {
    if delta_y > 0.0 {
        MouseButton::WheelUp
    } else {
        MouseButton::WheelDown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_click_inside_viewport() {
        let size = SizeInfo::from_pane(800.0, 480.0, 8.0, 16.0, 16.0, 6.0);
        let origin = Vector2F::zero();
        let pos = Vector2F::new(
            TERMINAL_PADDING_LEFT + size.cell_width * 2.5,
            ALT_GRID_PADDING_TOP + size.cell_height * 3.5,
        );
        let (row, col) = map_pointer_to_cell(pos, origin, size, 0.0).unwrap();
        assert_eq!(row, 3);
        assert_eq!(col, 2);
    }

    #[test]
    fn maps_click_outside_viewport_clamping() {
        let size = SizeInfo::from_pane(800.0, 480.0, 8.0, 16.0, 16.0, 6.0);
        let origin = Vector2F::zero();

        // Negative coords (padding left/top)
        let pos_neg = Vector2F::new(-10.0, -10.0);
        let (row, col) = map_pointer_to_cell(pos_neg, origin, size, 0.0).unwrap();
        assert_eq!(row, 0);
        assert_eq!(col, 0);

        // Coordinates exceeding size
        let pos_large = Vector2F::new(2000.0, 2000.0);
        let (row, col) = map_pointer_to_cell(pos_large, origin, size, 0.0).unwrap();
        assert_eq!(row, size.rows as usize - 1);
        assert_eq!(col, size.cols as usize - 1);
    }
}
