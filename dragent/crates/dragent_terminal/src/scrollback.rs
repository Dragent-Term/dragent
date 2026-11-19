//! B+ tree-backed scrollback buffer (backed by zed-sum-tree, Apache-2.0).
//!
//! Uses `SumTree<Row>` as the backing store, giving O(log n) seeks by row
//! index or byte offset instead of the O(n) scans a plain `VecDeque` requires.
//!
//! # Summary metrics
//! Each `Row` contributes to the `RowSummary` aggregate:
//!
//! | Field               | Meaning                                         |
//! |---------------------|-------------------------------------------------|
//! | `row_count`         | 1 per row — enables O(log n) row-index seek     |
//! | `visible_row_count` | 0 for collapsed rows — powers filtered walks    |
//! | `byte_count`        | UTF-8 bytes of char content — O(log n) by bytes |

use zed_sum_tree::{Bias, ContextLessSummary, Dimension, FilterCursor, Item, SumTree};

use crate::cell::StyledLine;
use crate::grid::Row;

// ─────────────────────────────────────────────────────────────────────────────
// RowSummary — aggregate stored at every B+ tree node
// ─────────────────────────────────────────────────────────────────────────────

/// Per-row aggregate accumulated across B+ tree nodes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowSummary {
    pub row_count: usize,
    pub visible_row_count: usize,
    pub byte_count: usize,
}

impl ContextLessSummary for RowSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.row_count += other.row_count;
        self.visible_row_count += other.visible_row_count;
        self.byte_count += other.byte_count;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Item impl for Row
// ─────────────────────────────────────────────────────────────────────────────

impl Item for Row {
    type Summary = RowSummary;

    fn summary(&self, _: ()) -> RowSummary {
        RowSummary {
            row_count: 1,
            visible_row_count: if self.collapsed { 0 } else { 1 },
            byte_count: self.cells.iter().map(|c| c.ch.len_utf8()).sum(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RowCount — dimension for seeking by row index
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct RowCount(pub usize);

impl<'a> Dimension<'a, RowSummary> for RowCount {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, s: &'a RowSummary, _: ()) {
        self.0 += s.row_count;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VisibleRowCount — dimension for seeking by visible row index
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct VisibleRowCount(pub usize);

impl<'a> Dimension<'a, RowSummary> for VisibleRowCount {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, s: &'a RowSummary, _: ()) {
        self.0 += s.visible_row_count;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ByteOffset — dimension for seeking by byte position
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteOffset(pub usize);

impl<'a> Dimension<'a, RowSummary> for ByteOffset {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, s: &'a RowSummary, _: ()) {
        self.0 += s.byte_count;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Scrollback
// ─────────────────────────────────────────────────────────────────────────────

/// A B+ tree-backed terminal scrollback buffer.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Scrollback {
    #[serde(with = "checkpoint_rows")]
    tree: SumTree<Row>,
    version: u64,
    rewrite_version: u64,
    pub max_rows: usize,
    pub evicted: usize,
}

mod checkpoint_rows {
    use super::*;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(
        tree: &SumTree<Row>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        tree.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SumTree<Row>, D::Error> {
        let rows = Vec::<Row>::deserialize(deserializer)?;
        Ok(SumTree::from_iter(rows, ()))
    }
}

impl Scrollback {
    pub fn new(max_rows: usize) -> Self {
        Self {
            tree: SumTree::new(()),
            version: 0,
            rewrite_version: 0,
            max_rows,
            evicted: 0,
        }
    }

    // ── Capacity ─────────────────────────────────────────────────────────────

    pub fn len(&self) -> usize {
        self.tree.summary().row_count
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn visible_len(&self) -> usize {
        self.tree.summary().visible_row_count
    }

    /// Monotonic write counter; incremented on every mutation.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Changes to existing history, excluding append and front eviction.
    pub fn rewrite_version(&self) -> u64 {
        self.rewrite_version
    }

    // ── Writes ───────────────────────────────────────────────────────────────

    pub fn push(&mut self, row: Row) {
        self.tree.push(row, ());
        // A zero capacity is the documented unlimited mode. In that mode the
        // tree is append-only and no historical rows are evicted.
        if self.max_rows == 0 {
            self.version += 1;
            return;
        }
        // Evict in amortized batches. Trimming exactly one row per push (the old
        // behavior) rebuilt the B+ tree spine on *every* line once the buffer was
        // full, so high-throughput output like `yes` (a newline every 2 bytes) was
        // O(rebuild) per line — ~15ms per 8 KiB feed, which pinned the shared
        // session lock and froze the UI. Instead we let the buffer overshoot by
        // `max_rows / 8` and then drop the whole excess in a single rebuild, so a
        // rebuild happens roughly once per `slack` lines rather than every line.
        // `len()` is therefore a soft bound in `[max_rows, max_rows + slack]`;
        // every consumer reads the live count, so the transient overshoot (a few
        // percent extra rows, trivial memory) is harmless. The proportional slack
        // also keeps small buffers exact: `max_rows/8 == 0` there, reproducing the
        // original one-row-per-push eviction.
        let slack = self.max_rows / 8;
        if self.len() > self.max_rows + slack {
            let excess = self.len() - self.max_rows;
            let new_tree = {
                let mut cursor = self.tree.cursor::<RowCount>(());
                cursor.seek(&RowCount(excess), Bias::Right);
                cursor.suffix()
            };
            self.tree = new_tree;
            self.evicted += excess;
        }
        self.version += 1;
    }

    pub fn replace_all(&mut self, rows: impl IntoIterator<Item = Row>) {
        self.rewrite_version += 1;
        let mut all: Vec<Row> = rows.into_iter().collect();
        if self.max_rows > 0 && all.len() > self.max_rows {
            let excess = all.len() - self.max_rows;
            self.evicted += excess;
            self.tree = SumTree::from_iter(all.drain(excess..), ());
        } else {
            self.tree = SumTree::from_iter(all, ());
        }
        self.version += 1;
    }

    pub fn pop_last_n(&mut self, n: usize) -> Vec<Row> {
        let total = self.len();
        let take = n.min(total);
        if take == 0 {
            return Vec::new();
        }
        self.rewrite_version += 1;
        let keep = total - take;
        if keep == 0 {
            let all = self.tree.items(());
            self.tree = SumTree::new(());
            self.version += 1;
            all
        } else {
            let (prefix, tail_items) = {
                let mut cursor = self.tree.cursor::<RowCount>(());
                let prefix = cursor.slice(&RowCount(keep), Bias::Right);
                let tail = cursor.suffix().items(());
                (prefix, tail)
            };
            self.tree = prefix;
            self.version += 1;
            tail_items
        }
    }

    pub fn clear(&mut self) {
        self.rewrite_version += 1;
        self.tree = SumTree::new(());
        self.version += 1;
    }

    // ── In-place mutation ────────────────────────────────────────────────────

    /// Mutate the row at absolute index `idx` in-place via `f`.
    ///
    /// Rebuilds the tree around the changed row. Acceptable for the infrequent
    /// async-highlight and collapse/expand operations where O(n) is fine.
    pub fn update_row<F>(&mut self, idx: usize, f: F) -> bool
    where
        F: FnOnce(&mut Row),
    {
        let total = self.len();
        if idx >= total {
            return false;
        }
        let mut items = self.tree.items(());
        f(&mut items[idx]);
        self.rewrite_version += 1;
        self.tree = SumTree::from_iter(items, ());
        self.version += 1;
        true
    }

    // ── Collapse / expand ────────────────────────────────────────────────────

    pub fn collapse_rows(&mut self, start: usize, end: usize) {
        let total = self.len();
        let end = end.min(total);
        if start >= end {
            return;
        }
        let mut items = self.tree.items(());
        for row in &mut items[start..end] {
            row.collapsed = true;
        }
        self.rewrite_version += 1;
        self.tree = SumTree::from_iter(items, ());
        self.version += 1;
    }

    pub fn expand_rows(&mut self, start: usize, end: usize) {
        let total = self.len();
        let end = end.min(total);
        if start >= end {
            return;
        }
        let mut items = self.tree.items(());
        for row in &mut items[start..end] {
            row.collapsed = false;
        }
        self.rewrite_version += 1;
        self.tree = SumTree::from_iter(items, ());
        self.version += 1;
    }

    // ── Reads ────────────────────────────────────────────────────────────────

    pub fn get_cloned(&self, idx: usize) -> Option<Row> {
        if idx >= self.len() {
            return None;
        }
        let mut cursor = self.tree.cursor::<RowCount>(());
        cursor.seek(&RowCount(idx), Bias::Right);
        cursor.item().cloned()
    }

    pub fn all_rows(&self) -> Vec<Row> {
        self.tree.items(())
    }

    pub fn each_row<F: FnMut(&Row)>(&self, mut f: F) {
        for row in self.tree.iter() {
            f(row);
        }
    }

    /// Iterate only visible (non-collapsed) rows, skipping collapsed subtrees.
    pub fn each_visible_row<F: FnMut(&Row)>(&self, mut f: F) {
        let mut cursor: FilterCursor<_, Row, ()> = self
            .tree
            .filter((), |s: &RowSummary| s.visible_row_count > 0);
        cursor.next();
        while let Some(row) = cursor.item() {
            f(row);
            cursor.next();
        }
    }

    pub fn styled_lines_in(&self, start: usize, end: usize) -> Vec<StyledLine> {
        let end = end.min(self.len());
        (start..end)
            .filter_map(|i| self.get_cloned(i))
            .map(|r| r.styled_line())
            .collect()
    }

    pub fn visible_styled_lines_in(&self, vis_start: usize, vis_end: usize) -> Vec<StyledLine> {
        let mut lines = Vec::new();
        let mut vis_idx = 0usize;
        let mut cursor: FilterCursor<_, Row, ()> = self
            .tree
            .filter((), |s: &RowSummary| s.visible_row_count > 0);
        cursor.next();
        while let Some(row) = cursor.item() {
            if vis_idx >= vis_end {
                break;
            }
            if vis_idx >= vis_start {
                lines.push(row.styled_line());
            }
            vis_idx += 1;
            cursor.next();
        }
        lines
    }

    /// Byte-offset of the start of row `idx`.
    pub fn byte_offset_of_row(&self, idx: usize) -> usize {
        if idx == 0 || self.is_empty() {
            return 0;
        }
        let mut cursor = self.tree.cursor::<RowCount>(());
        cursor
            .summary::<RowCount, RowSummary>(&RowCount(idx), Bias::Right)
            .byte_count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Style};

    fn blank_row(cols: usize) -> Row {
        Row::blank(cols)
    }

    fn row_with_char(ch: char, cols: usize) -> Row {
        let mut r = Row::blank(cols);
        r.cells[0] = Cell::from_char_style(ch, Style::default());
        r
    }

    #[test]
    fn push_and_len() {
        let mut sb = Scrollback::new(100);
        assert_eq!(sb.len(), 0);
        sb.push(blank_row(10));
        sb.push(blank_row(10));
        assert_eq!(sb.len(), 2);
    }

    #[test]
    fn eviction_at_capacity() {
        let mut sb = Scrollback::new(3);
        for i in 0..5usize {
            sb.push(row_with_char(
                char::from_u32('a' as u32 + i as u32).unwrap(),
                10,
            ));
        }
        assert_eq!(sb.len(), 3);
        assert_eq!(sb.evicted, 2);
        let first = sb.get_cloned(0).unwrap();
        assert_eq!(first.cells[0].ch, 'c');
    }

    #[test]
    fn eviction_is_batched_but_bounded() {
        // With a non-zero slack (max_rows/8), the buffer is allowed to overshoot
        // max_rows transiently, then trims the whole excess at once. It must never
        // exceed max_rows + slack, and eviction must still drop the oldest rows.
        let max_rows = 8usize;
        let slack = max_rows / 8; // == 1
        let mut sb = Scrollback::new(max_rows);
        for i in 0..64usize {
            sb.push(row_with_char(
                char::from_u32('0' as u32 + (i % 10) as u32).unwrap(),
                4,
            ));
            assert!(
                sb.len() <= max_rows + slack,
                "len {} exceeded soft bound {}",
                sb.len(),
                max_rows + slack,
            );
        }
        // Oldest surviving row is consistent with the number evicted.
        assert_eq!(sb.len() + sb.evicted, 64);
        let expected_first = char::from_u32('0' as u32 + (sb.evicted % 10) as u32).unwrap();
        assert_eq!(sb.get_cloned(0).unwrap().cells[0].ch, expected_first);
    }

    #[test]
    fn get_cloned_seek() {
        let mut sb = Scrollback::new(100);
        for i in 0..10usize {
            sb.push(row_with_char(
                char::from_u32('0' as u32 + i as u32).unwrap(),
                20,
            ));
        }
        let r5 = sb.get_cloned(5).unwrap();
        assert_eq!(r5.cells[0].ch, '5');
    }

    #[test]
    fn pop_last_n() {
        let mut sb = Scrollback::new(100);
        for i in 0..5usize {
            sb.push(row_with_char(
                char::from_u32('0' as u32 + i as u32).unwrap(),
                10,
            ));
        }
        let popped = sb.pop_last_n(2);
        assert_eq!(sb.len(), 3);
        assert_eq!(popped.len(), 2);
        assert_eq!(popped[0].cells[0].ch, '3');
        assert_eq!(popped[1].cells[0].ch, '4');
    }

    #[test]
    fn replace_all_trims_to_max() {
        let mut sb = Scrollback::new(3);
        let rows: Vec<Row> = (0..7).map(|_| blank_row(10)).collect();
        sb.replace_all(rows);
        assert_eq!(sb.len(), 3);
        assert_eq!(sb.evicted, 4);
    }

    #[test]
    fn version_increments_on_mutation() {
        let mut sb = Scrollback::new(100);
        let v0 = sb.version();
        sb.push(blank_row(10));
        assert!(sb.version() > v0);
    }

    #[test]
    fn byte_offset_monotone() {
        let mut sb = Scrollback::new(100);
        sb.push(row_with_char('a', 10));
        sb.push(row_with_char('b', 10));
        assert_eq!(sb.byte_offset_of_row(0), 0);
        let off1 = sb.byte_offset_of_row(1);
        assert!(off1 > 0, "offset of row 1 should be > 0, got {off1}");
    }

    #[test]
    fn visible_len_counts_only_non_collapsed() {
        let mut sb = Scrollback::new(100);
        for _ in 0..5 {
            sb.push(blank_row(10));
        }
        assert_eq!(sb.visible_len(), 5);
        sb.collapse_rows(1, 3);
        assert_eq!(sb.visible_len(), 3);
        sb.expand_rows(1, 3);
        assert_eq!(sb.visible_len(), 5);
    }

    #[test]
    fn each_visible_row_skips_collapsed() {
        let mut sb = Scrollback::new(100);
        for i in 0..5usize {
            sb.push(row_with_char(
                char::from_u32('a' as u32 + i as u32).unwrap(),
                10,
            ));
        }
        sb.collapse_rows(1, 3);
        let mut visible: Vec<char> = Vec::new();
        sb.each_visible_row(|r| visible.push(r.cells[0].ch));
        assert_eq!(visible, vec!['a', 'd', 'e']);
    }

    #[test]
    fn update_row_live_edit() {
        use crate::flags::Flags;
        let mut sb = Scrollback::new(100);
        sb.push(row_with_char('X', 10));
        let found = sb.update_row(0, |row| {
            row.cells[0].flags.insert(Flags::UNDERLINE);
        });
        assert!(found);
        let r = sb.get_cloned(0).unwrap();
        assert!(r.cells[0].flags.contains(Flags::UNDERLINE));
    }

    #[test]
    fn visible_styled_lines_in_respects_collapse() {
        let mut sb = Scrollback::new(100);
        for i in 0..4usize {
            sb.push(row_with_char(
                char::from_u32('a' as u32 + i as u32).unwrap(),
                10,
            ));
        }
        sb.collapse_rows(1, 2);
        let lines = sb.visible_styled_lines_in(0, 3);
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert_eq!(texts[0], "a");
        assert_eq!(texts[1], "c");
        assert_eq!(texts[2], "d");
    }
}
