//! UI-side state for the alternate screen (selection, dirty tracking).

use super::selection::CellSelection;

/// Per-alt-screen UI metadata. The VT grid lives in the session [`Emulator`](dragent_terminal::Emulator).
#[derive(Clone, Debug, Default)]
pub struct AltScreenState {
    /// Monotonic paint generation — bumped when model signals alt output changed.
    pub paint_generation: u64,
    /// Cell-range selection overlay (Shift+drag or Select mouse mode).
    pub selection: Option<CellSelection>,
}

impl AltScreenState {
    pub fn bump_paint(&mut self) {
        self.paint_generation = self.paint_generation.wrapping_add(1);
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }
}
