//! IME (Input Method Editor) pre-edit buffer.
//!
//! Handles text composition for CJK and other multi-keystroke input methods.
//! Platform-agnostic: the host feeds `ImeEvent`s produced by the OS input
//! layer; this module tracks composition state and exposes structured data
//! back to both the host UI and the terminal emulator.
//!
//! # IME support comparison
//!
//! | Feature                          | Reference       | dragent_terminal         |
//! |----------------------------------|------------|--------------------------|
//! | Pre-edit text                    | ✅ basic   | ✅ with cursor range     |
//! | Candidate list                   | ❌         | ✅ `Vec<ImeCandidate>`   |
//! | Selected candidate tracking      | ❌         | ✅ per-candidate flag    |
//! | Candidate window position        | ❌         | ✅ `CandidatePosition`   |
//! | Pre-edit visual cells            | Partial    | ✅ underlined cells      |
//! | Anchor tracking (row/col)        | ❌         | ✅ `anchor_row/col`      |
//! | Multi-stage composition          | ❌         | ✅ `ImePhase`            |
//! | Discard-on-escape                | ❌         | ✅ `ImeEvent::Cancel`    |

use crate::cell::{Cell, Style};
use crate::flags::Flags;

// ─────────────────────────────────────────────────────────────────────────────
// Events the host → emulator
// ─────────────────────────────────────────────────────────────────────────────

/// Events produced by the platform input layer and consumed by `ImeState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeEvent {
    /// Composition began or updated.
    ///
    /// `text` is the current pre-edit string; `cursor` is a byte range
    /// within `text` indicating where the IM cursor sits.
    Compose {
        text: String,
        /// Byte range within `text` for the active cursor segment.
        cursor: std::ops::Range<usize>,
    },
    /// The composition was committed — `text` should be forwarded to the PTY.
    Commit(String),
    /// The composition was cancelled — discard all pre-edit state.
    Cancel,
    /// The candidate list was updated.
    Candidates(Vec<ImeCandidate>),
}

// ─────────────────────────────────────────────────────────────────────────────
// Candidate list
// ─────────────────────────────────────────────────────────────────────────────

/// One entry in an IME candidate list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImeCandidate {
    /// The candidate string.
    pub text: String,
    /// Optional annotation / reading (e.g., furigana, pinyin).
    pub annotation: Option<String>,
    /// Whether this is the currently highlighted candidate.
    pub selected: bool,
}

/// Preferred position for the candidate window relative to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CandidatePosition {
    /// Show below the cursor (default, most common).
    #[default]
    Below,
    /// Show above the cursor (when there's more room above).
    Above,
    /// Show to the right of the cursor.
    Right,
}

// ─────────────────────────────────────────────────────────────────────────────
// Composition phase
// ─────────────────────────────────────────────────────────────────────────────

/// Tracks which stage of composition is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImePhase {
    /// No composition in progress.
    #[default]
    Idle,
    /// Composing — pre-edit text exists but is not yet committed.
    Composing,
    /// Candidate selection — user is picking from the candidate list.
    SelectingCandidate,
}

// ─────────────────────────────────────────────────────────────────────────────
// ImeState
// ─────────────────────────────────────────────────────────────────────────────

/// Complete IME composition state.
///
/// The emulator holds one `ImeState` instance.  The host reads it to render
/// the pre-edit overlay and the candidate window.
#[derive(Debug, Default)]
pub struct ImeState {
    /// Current pre-edit text (empty when idle).
    pub pre_edit: String,
    /// Byte range within `pre_edit` of the IM cursor position.
    pub cursor_range: std::ops::Range<usize>,
    /// Candidate list (empty when idle or when the IM provides none).
    pub candidates: Vec<ImeCandidate>,
    /// Where to show the candidate window.
    pub candidate_position: CandidatePosition,
    /// Terminal row at which composition is anchored.
    pub anchor_row: usize,
    /// Terminal column at which composition is anchored.
    pub anchor_col: usize,
    /// Current composition phase.
    pub phase: ImePhase,
}

impl ImeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a composition is currently in progress.
    pub fn is_active(&self) -> bool {
        self.phase != ImePhase::Idle
    }

    /// Apply an `ImeEvent` to this state.
    ///
    /// Returns `Some(committed_text)` when a commit happened, `None` otherwise.
    pub fn handle(&mut self, event: ImeEvent) -> Option<String> {
        match event {
            ImeEvent::Compose { text, cursor } => {
                self.pre_edit = text;
                self.cursor_range = cursor;
                if self.phase == ImePhase::Idle {
                    self.phase = ImePhase::Composing;
                }
                None
            }
            ImeEvent::Candidates(list) => {
                self.candidates = list;
                if self.phase == ImePhase::Composing && !self.candidates.is_empty() {
                    self.phase = ImePhase::SelectingCandidate;
                }
                None
            }
            ImeEvent::Commit(text) => {
                self.reset();
                Some(text)
            }
            ImeEvent::Cancel => {
                self.reset();
                None
            }
        }
    }

    /// Begin a new composition at the given terminal position.
    pub fn begin_at(&mut self, row: usize, col: usize) {
        self.reset();
        self.anchor_row = row;
        self.anchor_col = col;
        self.phase = ImePhase::Composing;
    }

    /// Reset all composition state.
    pub fn reset(&mut self) {
        self.pre_edit.clear();
        self.cursor_range = 0..0;
        self.candidates.clear();
        self.phase = ImePhase::Idle;
    }

    /// The currently selected candidate text, if any.
    pub fn selected_candidate(&self) -> Option<&str> {
        self.candidates
            .iter()
            .find(|c| c.selected)
            .map(|c| c.text.as_str())
    }

    /// Build a row of visual cells representing the pre-edit text.
    ///
    /// The pre-edit is shown with an `UNDERLINE` attribute.  The portion
    /// within `cursor_range` is additionally shown with `INVERSE` so the
    /// host can visually distinguish the IM cursor.  Cells are returned
    /// starting from `anchor_col`.
    pub fn pre_edit_cells(&self, base_style: Style, max_cols: usize) -> Vec<Cell> {
        if self.pre_edit.is_empty() {
            return Vec::new();
        }

        let mut cells = Vec::new();
        let cursor_start = self.cursor_range.start;
        let cursor_end = self.cursor_range.end;

        for (byte_pos, ch) in self.pre_edit.char_indices() {
            if cells.len() + self.anchor_col >= max_cols {
                break;
            }

            let mut style = base_style;
            // All pre-edit text is underlined.
            style.flags.insert(Flags::UNDERLINE);
            // Characters inside the IM cursor range are inverted.
            let char_end = byte_pos + ch.len_utf8();
            if byte_pos >= cursor_start && char_end <= cursor_end {
                style.flags.insert(Flags::INVERSE);
            }

            cells.push(Cell::from_char_style(ch, style));
        }

        cells
    }

    /// Number of terminal columns the pre-edit text would occupy.
    pub fn pre_edit_width(&self) -> usize {
        use unicode_width::UnicodeWidthChar;
        self.pre_edit
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(1))
            .sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Style;

    #[test]
    fn idle_initially() {
        let s = ImeState::new();
        assert!(!s.is_active());
        assert_eq!(s.phase, ImePhase::Idle);
    }

    #[test]
    fn compose_activates() {
        let mut s = ImeState::new();
        s.handle(ImeEvent::Compose {
            text: "にほん".into(),
            cursor: 0..9,
        });
        assert!(s.is_active());
        assert_eq!(s.phase, ImePhase::Composing);
        assert_eq!(s.pre_edit, "にほん");
    }

    #[test]
    fn commit_returns_text_and_resets() {
        let mut s = ImeState::new();
        s.handle(ImeEvent::Compose {
            text: "日".into(),
            cursor: 0..3,
        });
        let committed = s.handle(ImeEvent::Commit("日本語".into()));
        assert_eq!(committed, Some("日本語".into()));
        assert!(!s.is_active());
        assert!(s.pre_edit.is_empty());
    }

    #[test]
    fn cancel_resets() {
        let mut s = ImeState::new();
        s.handle(ImeEvent::Compose {
            text: "abc".into(),
            cursor: 0..3,
        });
        s.handle(ImeEvent::Cancel);
        assert!(!s.is_active());
    }

    #[test]
    fn candidates_phase_transition() {
        let mut s = ImeState::new();
        s.handle(ImeEvent::Compose {
            text: "n".into(),
            cursor: 0..1,
        });
        s.handle(ImeEvent::Candidates(vec![
            ImeCandidate {
                text: "の".into(),
                annotation: None,
                selected: true,
            },
            ImeCandidate {
                text: "に".into(),
                annotation: None,
                selected: false,
            },
        ]));
        assert_eq!(s.phase, ImePhase::SelectingCandidate);
        assert_eq!(s.selected_candidate(), Some("の"));
    }

    #[test]
    fn pre_edit_cells_underlined() {
        let mut s = ImeState::new();
        s.handle(ImeEvent::Compose {
            text: "ab".into(),
            cursor: 0..1,
        });
        let cells = s.pre_edit_cells(Style::default(), 80);
        assert_eq!(cells.len(), 2);
        // All cells have UNDERLINE.
        for c in &cells {
            assert!(c.flags.contains(Flags::UNDERLINE));
        }
        // First char (in cursor range) has INVERSE too.
        assert!(cells[0].flags.contains(Flags::INVERSE));
        // Second char (outside cursor range) does not.
        assert!(!cells[1].flags.contains(Flags::INVERSE));
    }

    #[test]
    fn pre_edit_width_cjk() {
        let mut s = ImeState::new();
        s.handle(ImeEvent::Compose {
            text: "日本".into(),
            cursor: 0..6,
        });
        // Each CJK char is 2 columns wide.
        assert_eq!(s.pre_edit_width(), 4);
    }

    #[test]
    fn begin_at_sets_anchor() {
        let mut s = ImeState::new();
        s.begin_at(3, 12);
        assert_eq!(s.anchor_row, 3);
        assert_eq!(s.anchor_col, 12);
        assert_eq!(s.phase, ImePhase::Composing);
    }
}
