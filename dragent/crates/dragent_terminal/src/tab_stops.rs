//! Tab-stop tracking for the terminal.
//!
//! The VT standard defaults to a tab stop every 8 columns. Terminals must also
//! support dynamic tab stops via `HTS` (ESC H) and `TBC` (CSI g).

/// Tracks which columns have tab stops set.
///
/// Internally stores a `Vec<bool>` that grows with the terminal width.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TabStops {
    stops: Vec<bool>,
}

impl TabStops {
    /// Create tab stops for `cols` columns with the standard 8-column default.
    pub fn new(cols: usize) -> Self {
        let mut stops = vec![false; cols];
        for i in (0..cols).step_by(8) {
            stops[i] = true;
        }
        Self { stops }
    }

    /// Rebuild for a new terminal width, preserving existing stops where
    /// possible and re-seeding the standard 8-column pattern for newly added
    /// columns.
    pub fn resize(&mut self, new_cols: usize) {
        let old_cols = self.stops.len();
        self.stops.resize(new_cols, false);
        // Seed 8-col stops for any newly added columns.
        for i in old_cols..new_cols {
            if i % 8 == 0 {
                self.stops[i] = true;
            }
        }
    }

    /// Set a tab stop at column `col` (HTS / ESC H).
    pub fn set(&mut self, col: usize) {
        if col < self.stops.len() {
            self.stops[col] = true;
        }
    }

    /// Clear the tab stop at column `col` (TBC 0 / CSI 0 g).
    pub fn clear(&mut self, col: usize) {
        if col < self.stops.len() {
            self.stops[col] = false;
        }
    }

    /// Clear all tab stops (TBC 3 / CSI 3 g).
    pub fn clear_all(&mut self) {
        self.stops.fill(false);
    }

    /// Advance to the next tab stop at or after `col`.
    ///
    /// If no stop exists beyond `col`, returns `cols - 1` (the last column).
    pub fn next_tab_stop(&self, col: usize) -> usize {
        let start = col + 1;
        if start >= self.stops.len() {
            return self.stops.len().saturating_sub(1);
        }
        for i in start..self.stops.len() {
            if self.stops[i] {
                return i;
            }
        }
        self.stops.len() - 1
    }

    /// Retreat to the previous tab stop strictly before `col`.
    ///
    /// Returns `0` if no earlier stop exists.
    pub fn prev_tab_stop(&self, col: usize) -> usize {
        if col == 0 {
            return 0;
        }
        for i in (0..col).rev() {
            if self.stops[i] {
                return i;
            }
        }
        0
    }

    /// Advance `n` tab stops from `col`, returning the new column.
    pub fn advance_n(&self, mut col: usize, n: usize) -> usize {
        for _ in 0..n {
            let next = self.next_tab_stop(col);
            if next == col {
                break;
            }
            col = next;
        }
        col
    }

    /// Retreat `n` tab stops from `col`, returning the new column.
    pub fn retreat_n(&self, mut col: usize, n: usize) -> usize {
        for _ in 0..n {
            let prev = self.prev_tab_stop(col);
            if prev == col {
                break;
            }
            col = prev;
        }
        col
    }

    /// Return the full stops slice (for testing / serialisation).
    pub fn as_slice(&self) -> &[bool] {
        &self.stops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stops_every_8() {
        let ts = TabStops::new(40);
        assert!(ts.stops[0]);
        assert!(ts.stops[8]);
        assert!(ts.stops[16]);
        assert!(ts.stops[24]);
        assert!(ts.stops[32]);
        assert!(!ts.stops[1]);
        assert!(!ts.stops[7]);
    }

    #[test]
    fn next_tab_stop_basic() {
        let ts = TabStops::new(40);
        assert_eq!(ts.next_tab_stop(0), 8);
        assert_eq!(ts.next_tab_stop(7), 8);
        assert_eq!(ts.next_tab_stop(8), 16);
    }

    #[test]
    fn prev_tab_stop_basic() {
        let ts = TabStops::new(40);
        assert_eq!(ts.prev_tab_stop(8), 0);
        assert_eq!(ts.prev_tab_stop(16), 8);
    }

    #[test]
    fn set_and_clear() {
        let mut ts = TabStops::new(20);
        ts.set(5);
        assert_eq!(ts.next_tab_stop(4), 5);
        ts.clear(5);
        assert_eq!(ts.next_tab_stop(4), 8);
    }

    #[test]
    fn clear_all_then_set_manual() {
        let mut ts = TabStops::new(20);
        ts.clear_all();
        assert_eq!(ts.next_tab_stop(0), 19); // no stops → last col
        ts.set(4);
        assert_eq!(ts.next_tab_stop(0), 4);
    }

    #[test]
    fn advance_n() {
        let ts = TabStops::new(80);
        assert_eq!(ts.advance_n(0, 3), 24);
    }
}
