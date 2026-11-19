/// Concrete terminal dimensions computed from the pane's measured viewport.
///
/// `SizeInfo` is the single struct that flows from the layout event (window
/// resize, panel open/close, font change) down to every component that cares
/// about terminal size.  Nothing derives dimensions independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeInfo {
    /// Visible columns (PTY width).
    pub cols: u16,
    /// Visible rows (PTY height).
    pub rows: u16,
    /// Monospace cell width in logical pixels.
    pub cell_width: f32,
    /// Monospace cell height in logical pixels (font size × leading).
    pub cell_height: f32,
    /// Usable viewport width in pixels (excludes scrollbar).
    pub viewport_width_px: f32,
    /// Usable viewport height in pixels (excludes input bar).
    pub viewport_height_px: f32,
}

/// Minimum grid dimensions, matching common terminal floors.
pub const MIN_COLS: u16 = 2;
pub const MIN_ROWS: u16 = 1;

impl SizeInfo {
    /// Build `SizeInfo` from a measured pane size and real cell geometry.
    ///
    /// This is the single source of truth for the pixels → rows/cols mapping.
    /// `cell_width` / `cell_height` must come from the actual render font (the
    /// grid renderer's `CellMetrics`), and the padding must match what the grid
    /// renderer paints, so the PTY
    /// grid and the on-screen grid stay in lockstep.
    ///
    /// Cols are derived from the pane width so they stay stable as content changes.
    /// Rows use the actual output viewport height (which excludes the input bar),
    /// so the shell knows the real number of visible rows for completion menus and
    /// line-editing display. In alt-screen mode the viewport height equals the full
    /// pane height (no input bar), so alt-screen TUIs are unaffected.
    pub fn from_pane(
        pane_width_px: f32,
        pane_height_px: f32,
        cell_width: f32,
        cell_height: f32,
        padding_x: f32,
        padding_y: f32,
    ) -> Self {
        let cell_width = cell_width.max(1.0);
        let cell_height = cell_height.max(1.0);

        let usable_w = (pane_width_px - 2.0 * padding_x).max(0.0);
        let usable_h = (pane_height_px - 2.0 * padding_y).max(0.0);

        let cols = ((usable_w / cell_width).floor() as i64).max(MIN_COLS as i64) as u16;
        let rows = ((usable_h / cell_height).floor() as i64).max(MIN_ROWS as i64) as u16;

        Self {
            cols,
            rows,
            cell_width,
            cell_height,
            viewport_width_px: pane_width_px,
            viewport_height_px: pane_height_px,
        }
    }
}

impl Default for SizeInfo {
    fn default() -> Self {
        // Provisional geometry before the first layout measurement arrives;
        // immediately corrected by the first `after_layout` size update.
        Self::from_pane(760.0, 480.0, 7.8, 15.6, 16.0, 6.0)
    }
}

/// Reason a size update was triggered.
///
/// Lets `TerminalSession::apply_size_update` distinguish ordinary geometry
/// changes from alt-screen transitions, which must redraw even when the pixel
/// size did not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeReason {
    /// The window was resized by the user.
    WindowResize,
    /// Alt-screen entered/exited — must resize immediately.
    AltScreenTransition,
}

/// A resize event flowing from the layout layer to `TerminalSession`.
///
/// Created by `PaneView` in response to measured viewport bounds changes.
/// `TerminalSession::apply_size_update` applies it atomically: updates the
/// emulator grid dimensions, conditionally sends SIGWINCH, and emits
/// `SessionEvent::SizeChanged`.
#[derive(Clone, Copy, Debug)]
pub struct SizeUpdate {
    pub info: SizeInfo,
    pub reason: SizeReason,
}

impl SizeUpdate {
    pub fn new(info: SizeInfo, reason: SizeReason) -> Self {
        Self { info, reason }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cols_from_pane_match_expected_formula() {
        // cols = floor((w - 2*pad_x) / cell_w); rows = floor((h - 2*pad_y) / cell_h)
        // (800 - 32) / 8 = 96 ; (480 - 12) / 16 = 29.25 -> 29
        let s = SizeInfo::from_pane(800.0, 480.0, 8.0, 16.0, 16.0, 6.0);
        assert_eq!(s.cols, 96);
        assert_eq!(s.rows, 29);
    }

    #[test]
    fn floors_partial_cells() {
        // 100px wide, 10px cell, no padding -> exactly 10 cols, no rounding up.
        let s = SizeInfo::from_pane(105.0, 60.0, 10.0, 10.0, 0.0, 0.0);
        assert_eq!(s.cols, 10);
        assert_eq!(s.rows, 6);
    }

    #[test]
    fn clamps_to_minimums_on_tiny_pane() {
        let s = SizeInfo::from_pane(1.0, 1.0, 8.0, 16.0, 16.0, 6.0);
        assert_eq!(s.cols, MIN_COLS);
        assert_eq!(s.rows, MIN_ROWS);
    }

    #[test]
    fn guards_against_zero_cell_size() {
        // Must not divide by zero; cell sizes are floored to 1px.
        let s = SizeInfo::from_pane(80.0, 40.0, 0.0, 0.0, 0.0, 0.0);
        assert!(s.cols >= MIN_COLS);
        assert!(s.rows >= MIN_ROWS);
    }
}
