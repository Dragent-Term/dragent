//! Height index for the block list — O(log n) seek by pixel offset.
//!
//! Backed by `zed_sum_tree::SumTree`. Each `BlockListEntry` carries its
//! own pixel height; the tree accumulates them into `BlockListSummary`.
//!
//! Cursors are kept internal to index methods. Phase 1 mutations use O(n)
//! full-rebuild for simplicity; O(log n) splice path deferred to Phase 2.

use zed_sum_tree::{Bias, ContextLessSummary, Dimension, Item, SumTree};

use crate::terminal::model::block::{live_primary_line_count, Block};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

// Must stay in sync with `BLOCK_HEADER_HEIGHT` and the header container padding
// in block_card_element.rs / block_list_element.rs (padding 22+22 around the
// ~26px icon buttons + border).
// Block-size constants moved to the shared `dragent_viewer` model layer; kept
// re-exported here so existing `block_height_tree::…` paths still resolve.
pub use crate::terminal::model::{
    BLOCK_HEADER_HEIGHT_PX, COMPLETED_BLOCK_MAX_LINES, LIVE_BLOCK_MAX_RENDER_LINES,
};
/// Visual separation after every command or agent card. This is part of the
/// indexed content, rather than container margin, so virtual scrolling,
/// selection and sticky headers all agree on the document geometry.
pub const BLOCK_VERTICAL_GAP_PX: f32 = 16.0;

const CARD_BODY_PADDING: f32 = 40.0;
const GRID_PADDING: f32 = 12.0;
const LINE_BOTTOM_PAD: f32 = 3.0;
/// Top padding above a live block's body — must match the `20.0` in
/// `BlockListElement::after_layout` / the paint path so the estimate and the
/// measured height agree for the running block.
const LIVE_BODY_TOP_PAD: f32 = 20.0;

// ─────────────────────────────────────────────────────────────────────────────
// Entry kinds
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum BlockListEntry {
    Block {
        block_id: usize,
        height_px: f32,
    },
    /// An agent conversation block (message, approval card, etc.).
    /// Placed after all shell blocks, before LivePrompt.
    AgentBlock {
        agent_block_id: usize,
        height_px: f32,
    },
    Bootstrap {
        height_px: f32,
    },
    ViewportClearGap {
        height_px: f32,
    },
    BlockSpacing {
        height_px: f32,
    },
    LivePrompt {
        height_px: f32,
    },
}

impl BlockListEntry {
    pub fn height_px(&self) -> f32 {
        match self {
            Self::Block { height_px, .. }
            | Self::AgentBlock { height_px, .. }
            | Self::Bootstrap { height_px }
            | Self::ViewportClearGap { height_px }
            | Self::BlockSpacing { height_px }
            | Self::LivePrompt { height_px } => *height_px,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChronologicalEntryData {
    Block {
        block_id: usize,
        height_px: f32,
        order_ms: i64,
    },
    AgentBlock {
        agent_block_id: usize,
        height_px: f32,
        order_ms: i64,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Summary
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockListSummary {
    pub total_height_px: f32,
    pub entry_count: usize,
    pub block_count: usize,
}

impl ContextLessSummary for BlockListSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.total_height_px += other.total_height_px;
        self.entry_count += other.entry_count;
        self.block_count += other.block_count;
    }
}

impl Item for BlockListEntry {
    type Summary = BlockListSummary;

    fn summary(&self, _: ()) -> BlockListSummary {
        BlockListSummary {
            total_height_px: self.height_px(),
            entry_count: 1,
            block_count: if matches!(self, Self::Block { .. }) {
                1
            } else {
                0
            },
        }
    }
    // Note: AgentBlock is intentionally excluded from block_count (not a shell block).
}

// ─────────────────────────────────────────────────────────────────────────────
// Dimension for cursor seeks
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct HeightPx(f32);

impl<'a> Dimension<'a, BlockListSummary> for HeightPx {
    fn zero(_: ()) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, s: &'a BlockListSummary, _: ()) {
        self.0 += s.total_height_px;
    }
}

impl Ord for HeightPx {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or_else(|| self.0.total_cmp(&other.0))
    }
}
impl PartialOrd for HeightPx {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for HeightPx {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for HeightPx {}

// ─────────────────────────────────────────────────────────────────────────────
// BlockHeightIndex
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BlockHeightIndex {
    tree: SumTree<BlockListEntry>,
}

impl BlockHeightIndex {
    pub fn new() -> Self {
        Self {
            tree: SumTree::new(()),
        }
    }

    // ── Structural rebuild ────────────────────────────────────────────────────

    pub fn rebuild_from(
        &mut self,
        blocks: &[Block],
        bootstrap_height: f32,
        clear_gap_height: f32,
        clear_gap_before_block_id: Option<usize>,
        clear_gap_after_all: bool,
        live_prompt_height: f32,
        cell_height: f32,
    ) {
        let mut entries: Vec<BlockListEntry> = Vec::with_capacity(blocks.len() + 4);

        if bootstrap_height > 0.0 {
            entries.push(BlockListEntry::Bootstrap {
                height_px: bootstrap_height,
            });
        }

        let mut gap_inserted = false;

        if clear_gap_height > 0.0 && clear_gap_before_block_id.is_none() && !clear_gap_after_all {
            entries.push(BlockListEntry::ViewportClearGap {
                height_px: clear_gap_height,
            });
            gap_inserted = true;
        }

        for block in blocks {
            let height_px = estimate_block_height(block, cell_height);
            entries.push(BlockListEntry::Block {
                block_id: block.id,
                height_px,
            });
            entries.push(BlockListEntry::BlockSpacing {
                height_px: BLOCK_VERTICAL_GAP_PX,
            });
            if clear_gap_height > 0.0
                && Some(block.id) == clear_gap_before_block_id
                && !clear_gap_after_all
            {
                entries.push(BlockListEntry::ViewportClearGap {
                    height_px: clear_gap_height,
                });
                gap_inserted = true;
            }
        }

        if clear_gap_height > 0.0 && !gap_inserted {
            entries.push(BlockListEntry::ViewportClearGap {
                height_px: clear_gap_height,
            });
        }

        if live_prompt_height > 0.0 {
            entries.push(BlockListEntry::LivePrompt {
                height_px: live_prompt_height,
            });
        }
        self.tree = SumTree::from_iter(entries, ());
    }

    /// Rebuild from pre-computed chronological entries (shell and agent blocks interleaved in creation order).
    pub fn rebuild_from_entries(
        &mut self,
        bootstrap_height: f32,
        clear_gap_height: f32,
        clear_gap_before_block_id: Option<usize>,
        clear_gap_after_all: bool,
        live_prompt_height: f32,
        clear_gap_before_order_ms: Option<i64>,
        chronological_data: &[ChronologicalEntryData],
    ) {
        let capacity = chronological_data.len() + 6;
        let mut entries: Vec<BlockListEntry> = Vec::with_capacity(capacity);

        if bootstrap_height > 0.0 {
            entries.push(BlockListEntry::Bootstrap {
                height_px: bootstrap_height,
            });
        }

        let mut gap_inserted = false;

        if clear_gap_height > 0.0 && clear_gap_before_block_id.is_none() && !clear_gap_after_all {
            entries.push(BlockListEntry::ViewportClearGap {
                height_px: clear_gap_height,
            });
            gap_inserted = true;
        }

        for &item in chronological_data {
            match item {
                ChronologicalEntryData::Block {
                    block_id,
                    height_px,
                    order_ms,
                } => {
                    if clear_gap_height > 0.0
                        && clear_gap_before_order_ms.is_some_and(|anchor| order_ms > anchor)
                        && !gap_inserted
                        && !clear_gap_after_all
                    {
                        entries.push(BlockListEntry::ViewportClearGap {
                            height_px: clear_gap_height,
                        });
                        gap_inserted = true;
                    }
                    entries.push(BlockListEntry::Block {
                        block_id,
                        height_px,
                    });
                    entries.push(BlockListEntry::BlockSpacing {
                        height_px: BLOCK_VERTICAL_GAP_PX,
                    });
                }
                ChronologicalEntryData::AgentBlock {
                    agent_block_id,
                    height_px,
                    order_ms,
                } => {
                    if clear_gap_height > 0.0
                        && clear_gap_before_order_ms.is_some_and(|anchor| order_ms > anchor)
                        && !gap_inserted
                        && !clear_gap_after_all
                    {
                        entries.push(BlockListEntry::ViewportClearGap {
                            height_px: clear_gap_height,
                        });
                        gap_inserted = true;
                    }
                    entries.push(BlockListEntry::AgentBlock {
                        agent_block_id,
                        height_px,
                    });
                    entries.push(BlockListEntry::BlockSpacing {
                        height_px: BLOCK_VERTICAL_GAP_PX,
                    });
                }
            }
        }

        if clear_gap_height > 0.0 && !gap_inserted {
            entries.push(BlockListEntry::ViewportClearGap {
                height_px: clear_gap_height,
            });
        }

        if live_prompt_height > 0.0 {
            entries.push(BlockListEntry::LivePrompt {
                height_px: live_prompt_height,
            });
        }
        self.tree = SumTree::from_iter(entries, ());
    }

    // ── Point updates (O(n) for Phase 1) ─────────────────────────────────────

    pub fn set_block_height(&mut self, block_id: usize, height_px: f32) {
        let items: Vec<BlockListEntry> = self
            .tree
            .items(())
            .into_iter()
            .map(|e| match e {
                BlockListEntry::Block { block_id: id, .. } if id == block_id => {
                    BlockListEntry::Block {
                        block_id: id,
                        height_px,
                    }
                }
                other => other,
            })
            .collect();
        self.tree = SumTree::from_iter(items, ());
    }

    pub fn set_agent_block_height(&mut self, agent_block_id: usize, height_px: f32) {
        let items: Vec<BlockListEntry> = self
            .tree
            .items(())
            .into_iter()
            .map(|e| match e {
                BlockListEntry::AgentBlock {
                    agent_block_id: id, ..
                } if id == agent_block_id => BlockListEntry::AgentBlock {
                    agent_block_id: id,
                    height_px,
                },
                other => other,
            })
            .collect();
        self.tree = SumTree::from_iter(items, ());
    }

    pub fn set_live_prompt_height(&mut self, height_px: f32) {
        let items: Vec<BlockListEntry> = self
            .tree
            .items(())
            .into_iter()
            .map(|e| match e {
                BlockListEntry::LivePrompt { .. } => BlockListEntry::LivePrompt { height_px },
                other => other,
            })
            .collect();
        self.tree = SumTree::from_iter(items, ());
    }

    pub fn set_clear_gap_height(&mut self, height_px: f32) {
        let items: Vec<BlockListEntry> = self
            .tree
            .items(())
            .into_iter()
            .map(|e| match e {
                BlockListEntry::ViewportClearGap { .. } => {
                    BlockListEntry::ViewportClearGap { height_px }
                }
                other => other,
            })
            .collect();
        self.tree = SumTree::from_iter(items, ());
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub fn total_height_px(&self) -> f32 {
        self.tree.summary().total_height_px
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Returns entries that intersect `[scroll_top, scroll_top + viewport_height)`,
    /// each annotated with its content-coordinate top and how many pixels are
    /// clipped above the viewport.
    pub fn visible_entries(&self, scroll_top: f32, viewport_height: f32) -> Vec<IndexedEntry> {
        let scroll_bottom = scroll_top + viewport_height;
        let mut result: Vec<IndexedEntry> = Vec::new();

        // Seek to the first entry whose bottom edge exceeds scroll_top.
        let mut cursor = self.tree.cursor::<HeightPx>(());
        cursor.seek(&HeightPx(scroll_top), Bias::Right);

        let mut accumulated = cursor.start().0;
        let mut tree_index = self.count_entries_before(scroll_top);

        loop {
            let Some(entry) = cursor.item() else { break };
            let entry_top = accumulated;
            let entry_height = entry.height_px();

            if entry_top >= scroll_bottom {
                break;
            }

            let partial = (scroll_top - entry_top).max(0.0);
            result.push(IndexedEntry {
                tree_index,
                entry: entry.clone(),
                content_top_px: entry_top,
                partial_offset_px: partial,
            });

            accumulated += entry_height;
            tree_index += 1;
            cursor.next();
        }

        result
    }

    /// Returns `(block_id, pixels_into_block)` for the first block that
    /// intersects `scroll_top`.
    pub fn first_visible_block_at(&self, scroll_top: f32) -> Option<(usize, f32)> {
        let mut cursor = self.tree.cursor::<HeightPx>(());
        cursor.seek(&HeightPx(scroll_top), Bias::Right);
        let mut accumulated = cursor.start().0;

        loop {
            match cursor.item() {
                Some(BlockListEntry::Block { block_id, .. }) => {
                    let partial = (scroll_top - accumulated).max(0.0);
                    return Some((*block_id, partial));
                }
                Some(entry) => {
                    accumulated += entry.height_px();
                    cursor.next();
                }
                None => return None,
            }
        }
    }

    /// Laid-out pixel height of a single block, if present in the index.
    pub fn block_height_px(&self, target_id: usize) -> Option<f32> {
        self.block_heights_map().get(&target_id).copied()
    }

    /// The shell-block `Block` entry (not an agent block) whose vertical range
    /// in content space contains `content_y`, if any.
    pub fn block_id_at_content_y(&self, content_y: f32) -> Option<usize> {
        self.visible_entries(content_y, 1.0)
            .into_iter()
            .find_map(|e| match e.entry {
                BlockListEntry::Block { block_id, .. } => Some(block_id),
                _ => None,
            })
    }

    pub fn block_top_px(&self, target_id: usize) -> Option<f32> {
        let mut y = 0.0f32;
        for entry in self.tree.items(()) {
            match &entry {
                BlockListEntry::Block { block_id, .. } if *block_id == target_id => {
                    return Some(y);
                }
                BlockListEntry::Bootstrap { .. } if target_id == usize::MAX => {
                    return Some(y);
                }
                BlockListEntry::LivePrompt { .. } if target_id == usize::MAX - 1 => {
                    return Some(y);
                }
                _ => y += entry.height_px(),
            }
        }
        None
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn block_heights_map(&self) -> std::collections::HashMap<usize, f32> {
        self.tree
            .items(())
            .into_iter()
            .filter_map(|e| match e {
                BlockListEntry::Block {
                    block_id,
                    height_px,
                } => Some((block_id, height_px)),
                _ => None,
            })
            .collect()
    }

    /// Count entries whose combined height is < offset_px (O(n) for Phase 1).
    fn count_entries_before(&self, offset_px: f32) -> usize {
        let mut y = 0.0f32;
        let mut count = 0;
        for entry in self.tree.items(()) {
            if y >= offset_px {
                break;
            }
            y += entry.height_px();
            count += 1;
        }
        count
    }
}

impl Default for BlockHeightIndex {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IndexedEntry — returned from visible_entries()
// ─────────────────────────────────────────────────────────────────────────────

pub struct IndexedEntry {
    pub tree_index: usize,
    pub entry: BlockListEntry,
    /// Y offset in content coordinates.
    pub content_top_px: f32,
    /// Pixels of this entry scrolled above the viewport top.
    pub partial_offset_px: f32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Height estimation
// ─────────────────────────────────────────────────────────────────────────────

pub fn estimate_block_height(block: &Block, cell_height: f32) -> f32 {
    if !block.expanded {
        return BLOCK_HEADER_HEIGHT_PX;
    }
    let body_lines = if let Some(emu) = &block.emulator {
        // Must mirror the live-render window in block_list_element: Unix hides
        // echo row 0; on Windows the preexec hook's buffer clear wipes the echo
        // and output starts at row 0, so nothing may be subtracted — otherwise
        // a single-line live redraw (npm/pip progress) measures as empty.
        let echo_rows = if cfg!(windows) { 0 } else { 1 };
        live_primary_line_count(emu).saturating_sub(echo_rows)
    } else {
        block.lines.len().min(COMPLETED_BLOCK_MAX_LINES)
    };
    if body_lines == 0 {
        return BLOCK_HEADER_HEIGHT_PX + CARD_BODY_PADDING;
    }
    let line_height = if block.emulator.is_some() {
        cell_height
    } else {
        cell_height + LINE_BOTTOM_PAD
    };
    // For the live (emulator) block this must match after_layout's measured body:
    // `header + 20px top pad + rows*cell_h`. The old value (52px) over-estimated by
    // ~30px, which showed as a downward snap when a block switched estimate→measured.
    let overhead = if block.emulator.is_some() {
        LIVE_BODY_TOP_PAD
    } else {
        CARD_BODY_PADDING
    };
    BLOCK_HEADER_HEIGHT_PX + overhead + body_lines as f32 * line_height
}

pub fn estimate_dir_listing_height(
    listing: &crate::console::dir_listing::DirListing,
    dir_view: crate::console::dir_listing::DirView,
    content_width_px: f32,
) -> f32 {
    BLOCK_HEADER_HEIGHT_PX
        + crate::terminal::render::dir_listing_paint::dir_listing_body_height(
            &listing.entries,
            dir_view,
            content_width_px,
        )
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::model::block::{Block, BlockStatus};

    fn make_index(heights: &[(usize, f32)]) -> BlockHeightIndex {
        let blocks: Vec<Block> = heights
            .iter()
            .map(|&(id, _)| {
                let mut b = Block::new(id, format!("cmd_{id}"), std::path::PathBuf::new(), 24, 80);
                b.status = BlockStatus::Completed { exit_code: Some(0) };
                b.expanded = true;
                b
            })
            .collect();
        let mut idx = BlockHeightIndex::new();
        idx.rebuild_from(&blocks, 0.0, 0.0, None, false, 0.0, 14.0);
        for &(id, h) in heights {
            idx.set_block_height(id, h);
        }
        idx
    }

    #[test]
    fn height_ordering_is_consistent_for_seeks() {
        let values = [f32::NEG_INFINITY, -1.0, -0.0, 0.0, 1.0, f32::INFINITY, f32::NAN];
        for left in values {
            for right in values {
                let (a, b) = (HeightPx(left), HeightPx(right));
                assert_eq!(a.partial_cmp(&b), Some(a.cmp(&b)));
                assert_eq!(a == b, a.cmp(&b).is_eq());
            }
        }
        assert_eq!(HeightPx(-0.0), HeightPx(0.0));
        assert!(HeightPx(1.0) < HeightPx(2.0));
    }

    #[test]
    fn total_height_sums_entries() {
        // Every block owns a following indexed spacing entry.
        let idx = make_index(&[(1, 100.0), (2, 200.0), (3, 50.0)]);
        assert!((idx.total_height_px() - 398.0).abs() < 0.1);
    }

    #[test]
    fn total_height_includes_bootstrap_and_prompt() {
        let mut idx = BlockHeightIndex::new();
        idx.rebuild_from(&[], 80.0, 0.0, None, false, 30.0, 14.0);
        assert!((idx.total_height_px() - 110.0).abs() < 0.1);
    }

    #[test]
    fn set_block_height_updates_total() {
        // blocks 100+300 = 400, plus two 16px spacing entries = 432
        let mut idx = make_index(&[(1, 100.0), (2, 200.0)]);
        idx.set_block_height(2, 300.0);
        assert!((idx.total_height_px() - 432.0).abs() < 0.1);
    }

    #[test]
    fn visible_entries_at_scroll_zero() {
        let idx = make_index(&[(1, 100.0), (2, 200.0)]);
        let entries = idx.visible_entries(0.0, 150.0);
        // block 1, its following gap, and block 2 all intersect 0..150.
        assert_eq!(entries.len(), 3);
        assert!(matches!(
            entries[0].entry,
            BlockListEntry::Block { block_id: 1, .. }
        ));
    }

    #[test]
    fn visible_entries_only_intersecting_viewport() {
        let idx = make_index(&[(1, 200.0), (2, 200.0), (3, 200.0)]);
        // viewport 200..400 intersects block 1's gap then block 2.
        let entries = idx.visible_entries(200.0, 200.0);
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            entries[1].entry,
            BlockListEntry::Block { block_id: 2, .. }
        ));
    }

    #[test]
    fn visible_entries_partial_top() {
        let idx = make_index(&[(1, 200.0), (2, 200.0)]);
        let entries = idx.visible_entries(50.0, 200.0);
        assert!((entries[0].partial_offset_px - 50.0).abs() < 0.1);
    }

    #[test]
    fn first_visible_block_at_zero() {
        let idx = make_index(&[(1, 100.0), (2, 200.0)]);
        let (id, partial) = idx.first_visible_block_at(0.0).unwrap();
        assert_eq!(id, 1);
        assert!(partial.abs() < 0.1);
    }

    #[test]
    fn first_visible_block_at_boundary() {
        let idx = make_index(&[(1, 100.0), (2, 200.0)]);
        let (id, partial) = idx.first_visible_block_at(100.0).unwrap();
        assert_eq!(id, 2);
        assert!(partial.abs() < 0.1);
    }

    #[test]
    fn first_visible_block_skips_bootstrap() {
        let mut b = Block::new(1, "cmd".to_string(), std::path::PathBuf::new(), 24, 80);
        b.expanded = true;
        let mut idx = BlockHeightIndex::new();
        idx.rebuild_from(&[b], 50.0, 0.0, None, false, 0.0, 14.0);
        idx.set_block_height(1, 100.0);
        // Scroll 70px: past 50px bootstrap, 20px into block 1
        let (id, partial) = idx.first_visible_block_at(70.0).unwrap();
        assert_eq!(id, 1);
        assert!((partial - 20.0).abs() < 0.1);
    }

    #[test]
    fn grow_block_above_increases_total_height() {
        // blocks 400+200+50 = 650, plus three 16px spacing entries = 698
        let mut idx = make_index(&[(1, 100.0), (2, 200.0), (3, 50.0)]);
        idx.set_block_height(1, 400.0);
        assert!((idx.total_height_px() - 698.0).abs() < 0.1);
    }

    #[test]
    fn viewport_clear_gap_affects_total_height_and_offsets() {
        let mut idx = BlockHeightIndex::new();
        // bootstrap = 0, clear_gap = 150, live_prompt = 40
        idx.rebuild_from(&[], 0.0, 150.0, None, false, 40.0, 14.0);
        assert!((idx.total_height_px() - 190.0).abs() < 0.1);

        // visible entries check
        let entries = idx.visible_entries(0.0, 200.0);
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            entries[0].entry,
            BlockListEntry::ViewportClearGap { height_px: 150.0 }
        ));
        assert!(matches!(
            entries[1].entry,
            BlockListEntry::LivePrompt { height_px: 40.0 }
        ));

        // updating clear gap height
        idx.set_clear_gap_height(100.0);
        assert!((idx.total_height_px() - 140.0).abs() < 0.1);
    }

    #[test]
    fn rebuild_from_entries_includes_agent_blocks() {
        let mut idx = BlockHeightIndex::new();
        idx.rebuild_from_entries(
            0.0,
            0.0,
            None,
            false,
            0.0,
            None,
            &[ChronologicalEntryData::AgentBlock {
                agent_block_id: 0,
                height_px: 136.0,
                order_ms: 1,
            }],
        );
        assert!((idx.total_height_px() - 152.0).abs() < 0.1); // 136 + BlockSpacing 16
        let entries = idx.visible_entries(0.0, 200.0);
        assert!(entries.iter().any(|e| matches!(
            e.entry,
            BlockListEntry::AgentBlock {
                agent_block_id: 0,
                ..
            }
        )));
    }

    #[test]
    fn clear_gap_is_after_all_entries_at_or_before_the_clear_boundary() {
        let mut idx = BlockHeightIndex::new();
        idx.rebuild_from_entries(
            0.0,
            80.0,
            Some(1),
            false,
            0.0,
            Some(10),
            &[
                ChronologicalEntryData::Block {
                    block_id: 1,
                    height_px: 20.0,
                    order_ms: 10,
                },
                ChronologicalEntryData::AgentBlock {
                    agent_block_id: 7,
                    height_px: 30.0,
                    order_ms: 20,
                },
            ],
        );
        let entries = idx.visible_entries(0.0, 200.0);
        assert!(matches!(
            entries[0].entry,
            BlockListEntry::Block { block_id: 1, .. }
        ));
        assert!(matches!(
            entries[1].entry,
            BlockListEntry::BlockSpacing { .. }
        ));
        assert!(matches!(
            entries[2].entry,
            BlockListEntry::ViewportClearGap { .. }
        ));
        assert!(matches!(
            entries[3].entry,
            BlockListEntry::AgentBlock {
                agent_block_id: 7,
                ..
            }
        ));
    }
}
