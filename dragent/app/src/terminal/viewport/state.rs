//! Viewport layer: resolves which block-list entries are visible given a
//! `ScrollPosition` and a `BlockHeightIndex`.
//!
//! Intentionally pure (no warpui/UI dependencies) so it can be unit-tested
//! without a window.

use crate::terminal::model::block_height_tree::{
    BlockHeightIndex, BlockListEntry, BLOCK_HEADER_HEIGHT_PX,
};

pub const SCROLL_BOTTOM_EPSILON_PX: f32 = 2.0;

pub fn is_block_list_virtualization_enabled() -> bool {
    std::env::var("DRAGENT_BLOCK_LIST_VIRTUALIZED")
        .ok()
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

// ─────────────────────────────────────────────────────────────────────────────
// Scroll position
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Copy, PartialEq)]
pub struct ContentOffsetPx(pub f32);

#[derive(Clone, Debug, Copy, PartialEq)]
pub enum ScrollMode {
    /// Viewport follows the bottom of the content as it grows.
    FollowBottom,
    /// User has scrolled up; stay at this fixed offset from content top.
    Fixed { offset_from_top: ContentOffsetPx },
}

#[derive(Clone, Debug, Copy, PartialEq)]
pub struct ScrollPosition {
    pub mode: ScrollMode,
}

impl ScrollPosition {
    pub fn follow_bottom() -> Self {
        Self {
            mode: ScrollMode::FollowBottom,
        }
    }

    pub fn fixed(offset_px: f32) -> Self {
        Self {
            mode: ScrollMode::Fixed {
                offset_from_top: ContentOffsetPx(offset_px),
            },
        }
    }

    pub fn scroll_top_px(&self, total_height_px: f32, viewport_height_px: f32) -> f32 {
        let max = (total_height_px - viewport_height_px).max(0.0);
        match self.mode {
            ScrollMode::FollowBottom => max,
            ScrollMode::Fixed {
                offset_from_top: ContentOffsetPx(off),
            } => off.min(max),
        }
    }

    pub fn pin_to_bottom(self) -> Self {
        Self {
            mode: ScrollMode::FollowBottom,
        }
    }

    /// Scroll by `delta_px` (positive = scroll down, negative = scroll up).
    pub fn scroll_by(self, delta_px: f32, total_height_px: f32, viewport_height_px: f32) -> Self {
        let max = (total_height_px - viewport_height_px).max(0.0);
        let current = self.scroll_top_px(total_height_px, viewport_height_px);
        let new_off = (current + delta_px).clamp(0.0, max);
        if new_off >= max - SCROLL_BOTTOM_EPSILON_PX {
            Self::follow_bottom()
        } else {
            Self::fixed(new_off)
        }
    }
}

impl Default for ScrollPosition {
    fn default() -> Self {
        Self::follow_bottom()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Visible entries
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VisibleEntry {
    pub tree_index: usize,
    pub kind: VisibleEntryKind,
    /// Y of this entry's top relative to the viewport origin (may be negative
    /// if the entry is partially scrolled off the top).
    pub y_offset_in_viewport: f32,
    pub height_px: f32,
    /// Pixels of this entry's top scrolled above the viewport.
    pub partial_offset_px: f32,
}

impl VisibleEntry {
    /// The block header has scrolled past the viewport top → show sticky header.
    pub fn header_scrolled_off(&self) -> bool {
        matches!(self.kind, VisibleEntryKind::Block { .. })
            && self.partial_offset_px > BLOCK_HEADER_HEIGHT_PX
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VisibleEntryKind {
    Block { block_id: usize },
    AgentBlock { agent_block_id: usize },
    Bootstrap,
    ViewportClearGap,
    BlockSpacing,
    LivePrompt,
}

// ─────────────────────────────────────────────────────────────────────────────
// ViewportState
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct ViewportState {
    pub visible_entries: Vec<VisibleEntry>,
    pub scroll_offset_px: f32,
    pub max_scroll_offset_px: f32,
    pub is_at_bottom: bool,
}

impl ViewportState {
    pub fn compute(
        index: &BlockHeightIndex,
        scroll: ScrollPosition,
        viewport_height_px: f32,
    ) -> Self {
        let total = index.total_height_px();
        let max_scroll = (total - viewport_height_px).max(0.0);
        let scroll_top = scroll.scroll_top_px(total, viewport_height_px);
        let is_at_bottom = scroll_top >= max_scroll - SCROLL_BOTTOM_EPSILON_PX;

        let (query_top, query_height) = if is_block_list_virtualization_enabled() {
            (scroll_top, viewport_height_px)
        } else {
            (0.0, total)
        };
        let indexed = index.visible_entries(query_top, query_height);

        let visible_entries = indexed
            .into_iter()
            .map(|ie| {
                let kind = match &ie.entry {
                    BlockListEntry::Block { block_id, .. } => VisibleEntryKind::Block {
                        block_id: *block_id,
                    },
                    BlockListEntry::AgentBlock { agent_block_id, .. } => {
                        VisibleEntryKind::AgentBlock {
                            agent_block_id: *agent_block_id,
                        }
                    }
                    BlockListEntry::Bootstrap { .. } => VisibleEntryKind::Bootstrap,
                    BlockListEntry::ViewportClearGap { .. } => VisibleEntryKind::ViewportClearGap,
                    BlockListEntry::BlockSpacing { .. } => VisibleEntryKind::BlockSpacing,
                    BlockListEntry::LivePrompt { .. } => VisibleEntryKind::LivePrompt,
                };
                VisibleEntry {
                    tree_index: ie.tree_index,
                    kind,
                    y_offset_in_viewport: ie.content_top_px - scroll_top,
                    height_px: ie.entry.height_px(),
                    partial_offset_px: ie.partial_offset_px,
                }
            })
            .collect();

        Self {
            visible_entries,
            scroll_offset_px: scroll_top,
            max_scroll_offset_px: max_scroll,
            is_at_bottom,
        }
    }

    /// Compute a `ScrollPosition` that keeps `anchor_block_id` at the same
    /// pixel distance from the viewport top after surrounding blocks change height.
    pub fn preserved_scroll(
        index: &BlockHeightIndex,
        anchor_block_id: usize,
        anchor_offset_in_block: f32,
        viewport_height_px: f32,
    ) -> ScrollPosition {
        let block_top = index.block_top_px(anchor_block_id).unwrap_or(0.0);
        let new_scroll = (block_top + anchor_offset_in_block).max(0.0);
        let total = index.total_height_px();
        let max = (total - viewport_height_px).max(0.0);
        if new_scroll >= max - SCROLL_BOTTOM_EPSILON_PX {
            ScrollPosition::follow_bottom()
        } else {
            ScrollPosition::fixed(new_scroll)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::model::block::{Block, BlockStatus};
    use crate::terminal::model::block_height_tree::BlockHeightIndex;

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

    // ── scroll_top_px ─────────────────────────────────────────────────────────

    #[test]
    fn follow_bottom_at_max() {
        let s = ScrollPosition::follow_bottom();
        assert!((s.scroll_top_px(500.0, 200.0) - 300.0).abs() < 0.1);
    }

    #[test]
    fn follow_bottom_zero_when_content_fits() {
        let s = ScrollPosition::follow_bottom();
        assert!(s.scroll_top_px(100.0, 200.0).abs() < 0.1);
    }

    #[test]
    fn fixed_respects_offset() {
        let s = ScrollPosition::fixed(120.0);
        assert!((s.scroll_top_px(500.0, 200.0) - 120.0).abs() < 0.1);
    }

    #[test]
    fn fixed_clamps_to_max() {
        let s = ScrollPosition::fixed(9999.0);
        assert!((s.scroll_top_px(500.0, 200.0) - 300.0).abs() < 0.1);
    }

    // ── scroll_by ────────────────────────────────────────────────────────────

    #[test]
    fn scroll_by_down() {
        let s = ScrollPosition::fixed(100.0).scroll_by(50.0, 500.0, 200.0);
        assert!((s.scroll_top_px(500.0, 200.0) - 150.0).abs() < 0.1);
    }

    #[test]
    fn scroll_by_up() {
        let s = ScrollPosition::fixed(100.0).scroll_by(-40.0, 500.0, 200.0);
        assert!((s.scroll_top_px(500.0, 200.0) - 60.0).abs() < 0.1);
    }

    #[test]
    fn scroll_by_to_bottom_arms_follow() {
        let s = ScrollPosition::fixed(290.0).scroll_by(20.0, 500.0, 200.0);
        assert_eq!(s.mode, ScrollMode::FollowBottom);
    }

    #[test]
    fn scroll_by_clamps_at_top() {
        let s = ScrollPosition::fixed(10.0).scroll_by(-100.0, 500.0, 200.0);
        assert!(s.scroll_top_px(500.0, 200.0).abs() < 0.1);
    }

    // ── is_at_bottom ─────────────────────────────────────────────────────────

    #[test]
    fn follow_bottom_is_at_bottom() {
        let idx = make_index(&[(1, 500.0)]);
        let state = ViewportState::compute(&idx, ScrollPosition::follow_bottom(), 200.0);
        assert!(state.is_at_bottom);
    }

    #[test]
    fn fixed_at_top_not_at_bottom() {
        let idx = make_index(&[(1, 500.0)]);
        let state = ViewportState::compute(&idx, ScrollPosition::fixed(0.0), 200.0);
        assert!(!state.is_at_bottom);
    }

    // ── visible_entries ───────────────────────────────────────────────────────

    #[test]
    fn all_visible_when_content_fits() {
        // rebuild_from inserts a BlockSpacing after EVERY block (see
        // BLOCK_VERTICAL_GAP_PX): B1, S, B2, S = 4 entries.
        // Total height = 100 + 16 + 80 + 16 = 212, fits in 400px.
        let idx = make_index(&[(1, 100.0), (2, 80.0)]);
        let state = ViewportState::compute(&idx, ScrollPosition::follow_bottom(), 400.0);
        assert_eq!(state.visible_entries.len(), 4);
    }

    #[test]
    fn only_intersecting_blocks_included() {
        // Layout: B1[0,200) S[200,216) B2[216,416) S[416,432) B3[432,632).
        // Window [200,400) intersects the first spacing and block 2.
        let idx = make_index(&[(1, 200.0), (2, 200.0), (3, 200.0)]);
        let state = ViewportState::compute(&idx, ScrollPosition::fixed(200.0), 200.0);
        assert_eq!(state.visible_entries.len(), 2);
        assert_eq!(
            state.visible_entries[0].kind,
            VisibleEntryKind::BlockSpacing
        );
        assert_eq!(
            state.visible_entries[1].kind,
            VisibleEntryKind::Block { block_id: 2 }
        );
    }

    #[test]
    fn partial_block_at_top_has_correct_offsets() {
        // Layout: B1[0,200) S[200,216) B2[216,416). Window [50,250):
        // partial B1, full spacing, partial B2 at y=166.
        let idx = make_index(&[(1, 200.0), (2, 200.0)]);
        let state = ViewportState::compute(&idx, ScrollPosition::fixed(50.0), 200.0);
        assert_eq!(state.visible_entries.len(), 3);
        let top = &state.visible_entries[0];
        assert!((top.partial_offset_px - 50.0).abs() < 0.1);
        assert!((top.y_offset_in_viewport - (-50.0)).abs() < 0.1);
        assert_eq!(
            state.visible_entries[1].kind,
            VisibleEntryKind::BlockSpacing
        );
        let third = &state.visible_entries[2];
        assert_eq!(third.kind, VisibleEntryKind::Block { block_id: 2 });
        assert!((third.y_offset_in_viewport - 166.0).abs() < 0.1);
    }

    #[test]
    fn header_scrolled_off_past_header_height() {
        let idx = make_index(&[(1, 400.0), (2, 200.0)]);
        // scroll_top = BLOCK_HEADER_HEIGHT_PX + 10 → header is off
        let scroll_top = BLOCK_HEADER_HEIGHT_PX + 10.0;
        let state = ViewportState::compute(&idx, ScrollPosition::fixed(scroll_top), 200.0);
        assert!(state.visible_entries[0].header_scrolled_off());
    }

    #[test]
    fn header_not_scrolled_off_within_header_height() {
        let idx = make_index(&[(1, 400.0), (2, 200.0)]);
        // Only 10px scrolled past block top — header still visible
        let state = ViewportState::compute(&idx, ScrollPosition::fixed(10.0), 200.0);
        assert!(!state.visible_entries[0].header_scrolled_off());
    }

    // ── preserved_scroll ─────────────────────────────────────────────────────

    #[test]
    fn preserved_scroll_after_block_above_grows() {
        let mut idx = make_index(&[(1, 100.0), (2, 200.0), (3, 50.0)]);
        // Viewport anchored at block 2 top (scroll_top = 100 + 16 spacing)

        // Block 1 grows by 150px
        idx.set_block_height(1, 250.0);

        // Block 2 is now at px 250 + 16 spacing; scroll should follow
        let new_scroll = ViewportState::preserved_scroll(&idx, 2, 0.0, 200.0);
        let new_top = new_scroll.scroll_top_px(idx.total_height_px(), 200.0);
        assert!((new_top - 266.0).abs() < 0.1, "got {new_top}");
    }

    #[test]
    fn preserved_scroll_unaffected_by_growth_below() {
        let mut idx = make_index(&[(1, 100.0), (2, 200.0), (3, 50.0)]);
        // Watching block 1 at scroll_top=0

        // Block 3 (below) grows
        idx.set_block_height(3, 300.0);

        let new_scroll = ViewportState::preserved_scroll(&idx, 1, 0.0, 100.0);
        let new_top = new_scroll.scroll_top_px(idx.total_height_px(), 100.0);
        assert!(new_top.abs() < 0.1, "got {new_top}");
    }
}
