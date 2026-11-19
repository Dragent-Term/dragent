//! Alt-screen text selection in terminal cell coordinates.

use dragent_terminal::StyledLine;

/// Inclusive anchor and cursor cells for a drag selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSelection {
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}

impl CellSelection {
    pub fn new(row: usize, col: usize) -> Self {
        Self {
            anchor: (row, col),
            cursor: (row, col),
        }
    }

    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor = (row, col);
    }

    /// Normalized inclusive bounds `(start_row, start_col)` .. `(end_row, end_col)`.
    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        let (r1, c1) = self.anchor;
        let (r2, c2) = self.cursor;
        if r1 < r2 || (r1 == r2 && c1 <= c2) {
            ((r1, c1), (r2, c2))
        } else {
            ((r2, c2), (r1, c1))
        }
    }

    pub fn contains_cell(&self, row: usize, col: usize) -> bool {
        let ((sr, sc), (er, ec)) = self.normalized();
        if sr == er {
            row == sr && col >= sc && col <= ec
        } else {
            (row > sr && row < er) || (row == sr && col >= sc) || (row == er && col <= ec)
        }
    }
}

/// Extract selected plain text from alt-screen styled lines.
pub fn selection_text(lines: &[StyledLine], selection: &CellSelection) -> String {
    let ((sr, sc), (er, ec)) = selection.normalized();
    let mut out = String::new();
    for (row_idx, line) in lines.iter().enumerate() {
        if row_idx < sr || row_idx > er {
            continue;
        }
        let cells = line_to_cells(line);
        let start = if row_idx == sr { sc } else { 0 };
        let end = if row_idx == er {
            ec.saturating_add(1).min(cells.len())
        } else {
            cells.len()
        };
        if start < end {
            for ch in &cells[start..end] {
                out.push(*ch);
            }
        }
        if row_idx < er {
            out.push('\n');
        }
    }
    out
}

fn line_to_cells(line: &StyledLine) -> Vec<char> {
    let mut cells = Vec::new();
    for span in &line.spans {
        for ch in span.text.chars() {
            let w = char_display_width(ch);
            if w == 0 {
                continue;
            }
            cells.push(ch);
            for _ in 1..w {
                cells.push(' ');
            }
        }
    }
    cells
}

fn char_display_width(c: char) -> usize {
    match c as u32 {
        0x00..=0x1f | 0x7f..=0x9f => 0,
        0x0300..=0x036f | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe00..=0xfe0f => 0,
        0x1100..=0x115f
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x20000..=0x2fffd
        | 0x30000..=0x3fffd => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_terminal::{Style, StyledSpan};

    fn line(text: &str) -> StyledLine {
        StyledLine {
            spans: vec![StyledSpan {
                text: text.to_string(),
                style: Style::default(),
            }],
            wrapped: false,
        }
    }

    #[test]
    fn extracts_single_line_range() {
        let lines = vec![line("hello world")];
        let sel = CellSelection {
            anchor: (0, 1),
            cursor: (0, 4),
        };
        assert_eq!(selection_text(&lines, &sel), "ello");
    }

    #[test]
    fn contains_cell_checks_stream_selection() {
        let sel = CellSelection {
            anchor: (0, 5),
            cursor: (2, 3),
        };
        // Row 0: only >= 5 is selected
        assert!(!sel.contains_cell(0, 4));
        assert!(sel.contains_cell(0, 5));
        assert!(sel.contains_cell(0, 80));

        // Row 1: everything is selected
        assert!(sel.contains_cell(1, 0));
        assert!(sel.contains_cell(1, 80));

        // Row 2: only <= 3 is selected
        assert!(sel.contains_cell(2, 0));
        assert!(sel.contains_cell(2, 3));
        assert!(!sel.contains_cell(2, 4));

        // Row 3: nothing is selected
        assert!(!sel.contains_cell(3, 0));
    }
}
