//! Pane-layout engine: the workspace layout tree.
//!
//! Principles (the layout tree is the source of truth):
//!
//! - A tab owns a layout tree, not a list of rectangles. Splitting a pane
//!   replaces that pane's node with a new split node; every other branch is
//!   untouched. A pane never decides where it belongs.
//! - Sizes are WEIGHTS, not pixels. Pixels are derived top-down from weights
//!   on every render, so window resizes, sidebar collapses, and nesting can
//!   never strand stale pixel values. Nothing here measures the screen.
//! - A pane is an id, not content. The engine never knows whether a pane
//!   hosts a terminal, an editor, or anything else; the app maps pane ids to
//!   content 1:1 outside this module.
//! - Splits collapse: any split reduced to a single child is replaced by
//!   that child ([`PaneLayout::normalize`] runs after every mutation).
//!
//! Terminology (fixed project-wide):
//!
//! - [`SplitDirection::Horizontal`] arranges children left-to-right (`A | B`).
//! - [`SplitDirection::Vertical`] arranges children top-to-bottom.
//! - UI labels may say Split Right/Left/Up/Down; they map to a direction plus
//!   an insertion side via [`SplitSide`].

use std::collections::HashMap;

use dragent_persistence::{SerializedPaneLayout, SerializedSplitDirection};
use dragent_workspace_model::{LayoutAxis, SurfaceId, SurfaceLayoutNode};

/// Pane identifier. The app maps these 1:1 to tabs; the engine treats them
/// as opaque content keys.
pub type PaneId = usize;

/// Reserved pane id for the editor surface. It lives in the layout tree
/// like any pane (splits, drags, closes) but owns no tab or shell — it
/// renders the workspace's shared document surface.
pub const EDITOR_PANE_ID: PaneId = usize::MAX;

/// Stable split identifier, used to target dividers for resize/equalize and
/// to key per-split UI state (divider handles) outside the engine.
pub type SplitId = u64;

/// Central minimum pane sizes. Individual panes never invent their own: all
/// floors flow through these two constants.
pub const MIN_PANE_WIDTH: f32 = 100.0;
pub const MIN_PANE_HEIGHT: f32 = 100.0;
/// Nominal extent persisted split sizes are expressed in. A restored share
/// is `size / extent`, so saving and restoring must use the same extent.
const PERSISTED_WIDTH: f32 = 1_000_000.0;
const PERSISTED_HEIGHT: f32 = 800_000.0;
/// Develop-layout `preferred_sizes` are shares of this nominal extent.
pub const SURFACE_SHARE_EXTENT: f32 = 1000.0;

/// Arrangement of a split's children.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SplitDirection {
    /// Children arranged left-to-right (`A | B`).
    Horizontal,
    /// Children arranged top-to-bottom.
    Vertical,
}

/// Directional split request from the UI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitSide {
    Left,
    Right,
    Up,
    Down,
}

impl SplitSide {
    /// Left/Right split horizontally, Up/Down vertically.
    pub fn direction(self) -> SplitDirection {
        match self {
            SplitSide::Left | SplitSide::Right => SplitDirection::Horizontal,
            SplitSide::Up | SplitSide::Down => SplitDirection::Vertical,
        }
    }

    /// Left/Up insert the new pane before the target, Right/Down after it.
    pub fn before(self) -> bool {
        matches!(self, SplitSide::Left | SplitSide::Up)
    }
}

/// One node of the layout tree.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayoutNode {
    Pane {
        id: PaneId,
    },
    Split {
        id: SplitId,
        direction: SplitDirection,
        children: Vec<LayoutNode>,
        /// Relative shares; always read through [`LayoutNode::shares`], which
        /// normalizes (repairs length/content drift defensively).
        weights: Vec<f32>,
    },
}

impl LayoutNode {
    /// Normalized shares for this node's children (`Pane` reads as `[1.0]`).
    /// Absolute sums are meaningless; only ratios matter.
    pub fn shares(&self) -> Vec<f32> {
        match self {
            LayoutNode::Pane { .. } => vec![1.0],
            LayoutNode::Split {
                children, weights, ..
            } => {
                if weights.len() != children.len() || children.is_empty() {
                    return vec![1.0 / children.len().max(1) as f32; children.len()];
                }
                let sum: f32 = weights.iter().map(|w| w.max(0.0)).sum();
                if sum <= f32::EPSILON {
                    return vec![1.0 / children.len() as f32; children.len()];
                }
                weights.iter().map(|w| w.max(0.0) / sum).collect()
            }
        }
    }

    fn contains(&self, id: PaneId) -> bool {
        match self {
            LayoutNode::Pane { id: pane } => *pane == id,
            LayoutNode::Split { children, .. } => children.iter().any(|c| c.contains(id)),
        }
    }

    fn panes_in_order(&self, out: &mut Vec<PaneId>) {
        match self {
            LayoutNode::Pane { id } => out.push(*id),
            LayoutNode::Split { children, .. } => {
                for child in children {
                    child.panes_in_order(out);
                }
            }
        }
    }
}

/// Axis-aligned rectangle produced by [`PaneLayout::extents`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Distribute `total` pixels over normalized `shares`, flooring every child
/// at `min`. Surplus/deficit from flooring is rebalanced across the
/// remaining children proportionally (bounded passes, always terminates).
/// If `total` cannot fit all floors, children split it equally (best effort).
fn distribute(total: f32, shares: &[f32], min: f32) -> Vec<f32> {
    let n = shares.len();
    if n == 0 {
        return Vec::new();
    }
    if total <= 0.0 {
        return vec![0.0; n];
    }
    if total < min * n as f32 {
        return vec![total / n as f32; n];
    }
    let mut out: Vec<f32> = shares.iter().map(|s| s.max(0.0) * total).collect();
    // Clamp under-floored children up, taking from the others proportionally.
    for _ in 0..n {
        let mut changed = false;
        let mut deficit = 0.0;
        let mut flexible = 0.0;
        for (size, share) in out.iter().zip(shares.iter()) {
            if *size < min {
                deficit += min - *size;
            } else {
                flexible += share.max(0.0);
            }
        }
        if deficit <= 0.0 {
            break;
        }
        for (size, share) in out.iter_mut().zip(shares.iter()) {
            if *size >= min && flexible > 0.0 {
                let take = deficit * share.max(0.0) / flexible;
                let floored = (*size - take).max(min);
                if (floored - *size).abs() > f32::EPSILON {
                    changed = true;
                }
                *size = floored;
            } else if *size < min {
                *size = min;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// The layout tree of one tab plus focus state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PaneLayout {
    root: LayoutNode,
    next_split_id: SplitId,
    active: Option<PaneId>,
    /// Splits the person sized by hand (a divider dragged or a resize asked
    /// for). Every other split keeps equal shares through splits, closes and
    /// moves; equalizing hands a split back.
    #[serde(default)]
    manual: std::collections::BTreeSet<SplitId>,
}

impl PaneLayout {
    /// A fresh tab: a single pane filling the whole area.
    pub fn new(pane: PaneId) -> Self {
        Self {
            root: LayoutNode::Pane { id: pane },
            next_split_id: 1,
            active: Some(pane),
            manual: Default::default(),
        }
    }

    /// Restore a persisted tree (ids and weights round-trip; the allocator
    /// continues past the highest stored split id).
    pub fn restore(root: LayoutNode, active: Option<PaneId>) -> Self {
        let mut layout = Self {
            root,
            next_split_id: 1,
            active,
            manual: Default::default(),
        };
        layout.normalize();
        // Unequal shares after a restart were sized by hand: keep them.
        layout.manual = layout.uneven_splits();
        let max_id = layout.split_ids().into_iter().max().unwrap_or(0);
        layout.next_split_id = max_id + 1;
        if layout.active.is_some_and(|id| !layout.contains(id)) {
            layout.active = layout.panes().into_iter().next();
        }
        layout
    }

    /// Rebuild the compatibility pane tree from the unified surface graph.
    /// Surface ids remain stable across launches, so the editor and terminal
    /// panes keep their original relative placement and split weights.
    pub fn from_surface_layout(
        root: &SurfaceLayoutNode,
        map_surface: impl Fn(SurfaceId) -> Option<PaneId> + Copy,
        active_surface: Option<SurfaceId>,
    ) -> Option<Self> {
        fn convert(
            node: &SurfaceLayoutNode,
            map_surface: impl Fn(SurfaceId) -> Option<PaneId> + Copy,
        ) -> Option<LayoutNode> {
            match node {
                SurfaceLayoutNode::Stack { surfaces, .. } => {
                    let id = surfaces.iter().find_map(|surface| map_surface(*surface))?;
                    Some(LayoutNode::Pane { id })
                }
                SurfaceLayoutNode::Split {
                    axis,
                    children,
                    preferred_sizes,
                } => {
                    if children.len() < 2 || preferred_sizes.len() != children.len() - 1 {
                        return None;
                    }
                    let children = children
                        .iter()
                        .map(|child| convert(child, map_surface))
                        .collect::<Option<Vec<_>>>()?;
                    // The last child takes the rest of the nominal extent.
                    let mut weights = preferred_sizes.clone();
                    let used: f32 = weights.iter().copied().sum();
                    weights.push((SURFACE_SHARE_EXTENT - used).max(1.0));
                    Some(LayoutNode::Split {
                        id: 0,
                        direction: match axis {
                            LayoutAxis::Horizontal => SplitDirection::Horizontal,
                            LayoutAxis::Vertical => SplitDirection::Vertical,
                        },
                        children,
                        weights,
                    })
                }
            }
        }
        let active = active_surface.and_then(map_surface);
        Some(Self::restore(convert(root, map_surface)?, active))
    }

    /// Renumber every split in `node` depth-first from `*next`, returning
    /// the next free id. Imported trees carry placeholder ids; this gives
    /// them unique stable identities for divider targeting.
    fn renumber(node: &mut LayoutNode, next: &mut SplitId) {
        if let LayoutNode::Split { id, children, .. } = node {
            *id = *next;
            *next += 1;
            for child in children.iter_mut() {
                Self::renumber(child, next);
            }
        }
    }

    fn persisted_direction(direction: SplitDirection) -> SerializedSplitDirection {
        // New convention throughout: Horizontal arranges left|right.
        match direction {
            SplitDirection::Horizontal => SerializedSplitDirection::Horizontal,
            SplitDirection::Vertical => SerializedSplitDirection::Vertical,
        }
    }

    fn engine_direction(direction: SerializedSplitDirection) -> SplitDirection {
        match direction {
            SerializedSplitDirection::Horizontal => SplitDirection::Horizontal,
            SerializedSplitDirection::Vertical => SplitDirection::Vertical,
        }
    }

    /// Export for storage; pair with [`Self::from_persisted`].
    pub fn to_persisted(
        &self,
        leaves: &impl Fn(PaneId) -> SerializedPaneLayout,
    ) -> SerializedPaneLayout {
        self.to_serialized(PERSISTED_WIDTH, PERSISTED_HEIGHT, leaves)
    }

    /// Rebuild a tree written by [`Self::to_persisted`] with its exact shares.
    pub fn from_persisted(
        layout: &SerializedPaneLayout,
        ids: &mut impl Iterator<Item = PaneId>,
    ) -> Self {
        Self::from_serialized(layout, ids, PERSISTED_WIDTH, PERSISTED_HEIGHT)
    }

    /// Export the tree to the persisted binary layout. N-ary splits nest
    /// left-deep; sizes are exact pixels under the given extent (the same
    /// extent the renderer derives every frame, so save/restore round-trips
    /// without drift). `leaves` builds each pane's stored payload.
    pub fn to_serialized(
        &self,
        width: f32,
        height: f32,
        leaves: &impl Fn(PaneId) -> SerializedPaneLayout,
    ) -> SerializedPaneLayout {
        Self::export_node(&self.root, width, height, leaves)
    }

    fn export_node(
        node: &LayoutNode,
        width: f32,
        height: f32,
        leaves: &impl Fn(PaneId) -> SerializedPaneLayout,
    ) -> SerializedPaneLayout {
        match node {
            LayoutNode::Pane { id } => leaves(*id),
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                let (total, min) = match direction {
                    SplitDirection::Horizontal => (width, MIN_PANE_WIDTH),
                    SplitDirection::Vertical => (height, MIN_PANE_HEIGHT),
                };
                let sizes = distribute(total.max(0.0), &node.shares(), min);
                // Left-deep nesting preserves every share exactly.
                let mut acc: Option<SerializedPaneLayout> = None;
                let mut acc_size = 0.0;
                for (child, size) in children.iter().zip(sizes.iter()) {
                    let (cw, ch) = match direction {
                        SplitDirection::Horizontal => (*size, height),
                        SplitDirection::Vertical => (width, *size),
                    };
                    let converted = Self::export_node(child, cw, ch, leaves);
                    match acc.take() {
                        None => {
                            if children.len() == 1 {
                                acc = Some(converted);
                            } else {
                                acc_size = *size;
                                acc = Some(converted);
                            }
                        }
                        Some(left) => {
                            acc = Some(SerializedPaneLayout::Split {
                                direction: Self::persisted_direction(*direction),
                                left_or_top: Box::new(left),
                                right_or_bottom: Box::new(converted),
                                size: acc_size,
                            });
                            acc_size += *size;
                        }
                    }
                }
                acc.expect("split always has children post-normalize")
            }
        }
    }

    /// Rebuild a tree from a persisted binary layout. Sizes become weights
    /// against the given extent (per-level normalization makes the absolute
    /// extent cancel out — only ratios survive, which is exactly right).
    /// Consumes one id per pane leaf in depth-first order; pane ids come
    /// from the caller so restored tabs line up with reconstructed panes.
    pub fn from_serialized(
        layout: &SerializedPaneLayout,
        ids: &mut impl Iterator<Item = PaneId>,
        width: f32,
        height: f32,
    ) -> Self {
        let mut layout = Self {
            root: Self::import_node(layout, ids, width.max(1.0), height.max(1.0)),
            next_split_id: 1,
            manual: Default::default(),
            active: None,
        };
        layout.normalize();
        let mut next = 1;
        Self::renumber(&mut layout.root, &mut next);
        layout.next_split_id = next;
        layout.manual = layout.uneven_splits();
        layout
    }

    fn import_node(
        layout: &SerializedPaneLayout,
        ids: &mut impl Iterator<Item = PaneId>,
        width: f32,
        height: f32,
    ) -> LayoutNode {
        match layout {
            SerializedPaneLayout::Single { .. } => LayoutNode::Pane {
                id: ids.next().unwrap_or(0),
            },
            SerializedPaneLayout::Split {
                direction,
                left_or_top,
                right_or_bottom,
                size,
            } => {
                let direction = Self::engine_direction(*direction);
                let (total, min) = match direction {
                    SplitDirection::Horizontal => (width, MIN_PANE_WIDTH),
                    SplitDirection::Vertical => (height, MIN_PANE_HEIGHT),
                };
                let total = total.max(1.0);
                let first = (size / total).clamp(0.01, 0.99);
                let (lw, lh, rw, rh) = match direction {
                    SplitDirection::Horizontal => {
                        (size.max(min), height, (total - size).max(min), height)
                    }
                    SplitDirection::Vertical => {
                        (width, size.max(min), width, (total - size).max(min))
                    }
                };
                LayoutNode::Split {
                    id: 0, // renumbered by from_serialized
                    direction,
                    children: vec![
                        Self::import_node(left_or_top, ids, lw, lh),
                        Self::import_node(right_or_bottom, ids, rw, rh),
                    ],
                    weights: vec![first, 1.0 - first],
                }
            }
        }
    }

    fn fresh_split_id(&mut self) -> SplitId {
        let id = self.next_split_id;
        self.next_split_id += 1;
        id
    }

    fn split_ids(&self) -> Vec<SplitId> {
        let mut ids = Vec::new();
        fn walk(node: &LayoutNode, out: &mut Vec<SplitId>) {
            if let LayoutNode::Split { id, children, .. } = node {
                out.push(*id);
                for child in children {
                    walk(child, out);
                }
            }
        }
        walk(&self.root, &mut ids);
        ids
    }

    /// True while the tab is a single unsplit pane.
    pub fn is_single(&self) -> bool {
        matches!(self.root, LayoutNode::Pane { .. })
    }

    pub fn root(&self) -> &LayoutNode {
        &self.root
    }

    /// Panes in visual order (depth-first, left-to-right / top-to-bottom).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.panes_in_order(&mut out);
        out
    }

    pub fn pane_count(&self) -> usize {
        self.panes().len()
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.root.contains(id)
    }

    /// Focused pane id.
    pub fn active(&self) -> Option<PaneId> {
        self.active
    }

    /// Focus a pane that exists in the tree; unknown ids are ignored.
    pub fn focus(&mut self, id: PaneId) {
        if self.contains(id) {
            self.active = Some(id);
        }
    }

    /// The orientation of the split directly containing `id`, if any. Drives
    /// the alternating split button (offer the opposite axis).
    pub fn parent_direction(&self, id: PaneId) -> Option<SplitDirection> {
        fn find(node: &LayoutNode, id: PaneId) -> Option<SplitDirection> {
            match node {
                LayoutNode::Pane { .. } => None,
                LayoutNode::Split {
                    direction,
                    children,
                    ..
                } => {
                    if children
                        .iter()
                        .any(|c| matches!(c, LayoutNode::Pane { id: pane } if *pane == id))
                    {
                        return Some(*direction);
                    }
                    children.iter().find_map(|c| find(c, id))
                }
            }
        }
        find(&self.root, id)
    }

    /// Split `target`, opening `new_id` on `side` of it. The new pane takes
    /// half of the target's share; every other branch is untouched. Focus
    /// follows the new pane. Fails (returns false, tree unchanged) when the
    /// target is missing or the id is already present.
    pub fn split_pane(&mut self, target: PaneId, new_id: PaneId, side: SplitSide) -> bool {
        let changed = self.split_pane_unbalanced(target, new_id, side);
        if changed {
            self.rebalance();
        }
        changed
    }

    fn split_pane_unbalanced(&mut self, target: PaneId, new_id: PaneId, side: SplitSide) -> bool {
        if target == new_id || self.contains(new_id) || !self.contains(target) {
            return false;
        }
        let direction = side.direction();
        let split_id = self.fresh_split_id();
        if let LayoutNode::Pane { id } = self.root {
            debug_assert_eq!(id, target);
            let (first, second) = if side.before() {
                (new_id, target)
            } else {
                (target, new_id)
            };
            self.root = LayoutNode::Split {
                id: split_id,
                direction,
                children: vec![
                    LayoutNode::Pane { id: first },
                    LayoutNode::Pane { id: second },
                ],
                weights: vec![0.5, 0.5],
            };
        } else if !Self::split_at_parent(&mut self.root, target, new_id, side, split_id) {
            return false;
        }
        self.active = Some(new_id);
        true
    }

    /// Recursive worker: operate at the target pane's direct parent.
    fn split_at_parent(
        node: &mut LayoutNode,
        target: PaneId,
        new_id: PaneId,
        side: SplitSide,
        split_id: SplitId,
    ) -> bool {
        match node {
            LayoutNode::Pane { .. } => false,
            LayoutNode::Split {
                direction,
                children,
                weights,
                ..
            } => {
                if let Some(pos) = children
                    .iter()
                    .position(|c| matches!(c, LayoutNode::Pane { id } if *id == target))
                {
                    if *direction == side.direction() {
                        // Case 1: same orientation — sibling insert. The new
                        // pane takes half of the target's share.
                        if weights.len() != children.len() {
                            let equal = 1.0 / children.len() as f32;
                            *weights = vec![equal; children.len()];
                        }
                        let half = weights[pos] / 2.0;
                        weights[pos] = half;
                        let at = if side.before() { pos } else { pos + 1 };
                        children.insert(at, LayoutNode::Pane { id: new_id });
                        weights.insert(at, half);
                    } else {
                        // Case 2: other orientation — wrap the target pane in
                        // a nested split. Never flattened.
                        let (first, second) = if side.before() {
                            (new_id, target)
                        } else {
                            (target, new_id)
                        };
                        children[pos] = LayoutNode::Split {
                            id: split_id,
                            direction: side.direction(),
                            children: vec![
                                LayoutNode::Pane { id: first },
                                LayoutNode::Pane { id: second },
                            ],
                            weights: vec![0.5, 0.5],
                        };
                    }
                    true
                } else {
                    children
                        .iter_mut()
                        .any(|c| Self::split_at_parent(c, target, new_id, side, split_id))
                }
            }
        }
    }

    /// Move an existing pane next to another (`before` selects the side).
    /// Used for drag-drop reparenting. `src` may live outside the tree
    /// (e.g. a sidebar tab being dropped in); it is detached first when
    /// present. No-ops on unknown targets or self-moves.
    pub fn move_pane(&mut self, src: PaneId, target: PaneId, before: bool) -> bool {
        let changed = self.move_pane_unbalanced(src, target, before);
        if changed {
            self.rebalance();
        }
        changed
    }

    fn move_pane_unbalanced(&mut self, src: PaneId, target: PaneId, before: bool) -> bool {
        if src == target || !self.contains(target) {
            return false;
        }
        if self.contains(src) {
            if !Self::remove_leaf(&mut self.root, src) {
                return false;
            }
            Self::normalize_node(&mut self.root);
            if !self.contains(target) {
                return false;
            }
        }
        if !Self::insert_sibling(&mut self.root, target, src, before) {
            return false;
        }
        self.active = Some(src);
        true
    }

    /// Directional move for drag-drop: same-orientation drops join the
    /// target's split as a sibling; cross-orientation drops wrap the target
    /// pane in a nested split instead of forcing it into an axis it does
    /// not share. Never flattens.
    pub fn move_pane_oriented(&mut self, src: PaneId, target: PaneId, side: SplitSide) -> bool {
        let changed = self.move_pane_oriented_unbalanced(src, target, side);
        if changed {
            self.rebalance();
        }
        changed
    }

    fn move_pane_oriented_unbalanced(&mut self, src: PaneId, target: PaneId, side: SplitSide) -> bool {
        if src == target || !self.contains(target) {
            return false;
        }
        if self.contains(src) {
            if !Self::remove_leaf(&mut self.root, src) {
                return false;
            }
            Self::normalize_node(&mut self.root);
        }
        if !self.contains(target) {
            return false;
        }
        let direction = side.direction();
        let parent_matches = match Self::find_parent_direction(&self.root, target) {
            None => true, // target is the lone root pane: any wrap works
            Some(parent) => parent == direction,
        };
        let moved = if parent_matches {
            Self::insert_sibling(&mut self.root, target, src, side.before())
        } else {
            let split_id = self.fresh_split_id();
            Self::split_at_parent_existing(&mut self.root, target, src, side, split_id)
        };
        if moved {
            self.active = Some(src);
        }
        moved
    }

    /// Direct parent orientation of the `target` pane, if it has one.
    fn find_parent_direction(node: &LayoutNode, target: PaneId) -> Option<SplitDirection> {
        match node {
            LayoutNode::Pane { .. } => None,
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                if children
                    .iter()
                    .any(|c| matches!(c, LayoutNode::Pane { id } if *id == target))
                {
                    return Some(*direction);
                }
                children
                    .iter()
                    .find_map(|c| Self::find_parent_direction(c, target))
            }
        }
    }

    /// Wrap the `target` pane with an already-existing `src` pane.
    fn split_at_parent_existing(
        node: &mut LayoutNode,
        target: PaneId,
        src: PaneId,
        side: SplitSide,
        split_id: SplitId,
    ) -> bool {
        match node {
            LayoutNode::Pane { .. } => false,
            LayoutNode::Split {
                children, weights, ..
            } => {
                if let Some(pos) = children
                    .iter()
                    .position(|c| matches!(c, LayoutNode::Pane { id } if *id == target))
                {
                    if weights.len() != children.len() {
                        let equal = 1.0 / children.len() as f32;
                        *weights = vec![equal; children.len()];
                    }
                    // The target keeps its full share; the moved pane takes
                    // half of it (same local-weight rule as fresh splits).
                    let half = weights[pos] / 2.0;
                    weights[pos] = half;
                    let (first, second) = if side.before() {
                        (src, target)
                    } else {
                        (target, src)
                    };
                    children[pos] = LayoutNode::Split {
                        id: split_id,
                        direction: side.direction(),
                        children: vec![
                            LayoutNode::Pane { id: first },
                            LayoutNode::Pane { id: second },
                        ],
                        weights: vec![half, half],
                    };
                    true
                } else {
                    children
                        .iter_mut()
                        .any(|c| Self::split_at_parent_existing(c, target, src, side, split_id))
                }
            }
        }
    }

    /// Insert `new_id` as a sibling of the `target` pane. Shared worker for
    /// moves (targets always exist post-removal).
    fn insert_sibling(root: &mut LayoutNode, target: PaneId, new_id: PaneId, before: bool) -> bool {
        match root {
            LayoutNode::Pane { .. } => false,
            LayoutNode::Split {
                children, weights, ..
            } => {
                if let Some(pos) = children
                    .iter()
                    .position(|c| matches!(c, LayoutNode::Pane { id } if *id == target))
                {
                    if weights.len() != children.len() {
                        let equal = 1.0 / children.len() as f32;
                        *weights = vec![equal; children.len()];
                    }
                    let half = weights[pos] / 2.0;
                    weights[pos] = half;
                    let at = if before { pos } else { pos + 1 };
                    children.insert(at, LayoutNode::Pane { id: new_id });
                    weights.insert(at, half);
                    true
                } else {
                    children
                        .iter_mut()
                        .any(|c| Self::insert_sibling(c, target, new_id, before))
                }
            }
        }
    }

    /// Close a pane. Single-child splits collapse (normalization); focus
    /// passes to the visual neighbor. Refuses to empty the tree (the caller
    /// dissolves single-pane layouts instead) and ignores unknown ids.
    pub fn close_pane(&mut self, id: PaneId) -> bool {
        let changed = self.close_pane_unbalanced(id);
        if changed {
            self.rebalance();
        }
        changed
    }

    fn close_pane_unbalanced(&mut self, id: PaneId) -> bool {
        if matches!(self.root, LayoutNode::Pane { id: pane } if pane == id) {
            return false;
        }
        if !self.contains(id) {
            return false;
        }
        let panes = self.panes();
        let successor = panes.iter().position(|pane| *pane == id).and_then(|pos| {
            panes
                .get(pos + 1)
                .or_else(|| pos.checked_sub(1).and_then(|prev| panes.get(prev)))
                .copied()
        });
        if !Self::remove_leaf(&mut self.root, id) {
            return false;
        }
        Self::normalize_node(&mut self.root);
        if self.active == Some(id) {
            self.active = successor.filter(|next| self.contains(*next));
        }
        true
    }

    /// Remove a pane leaf (and its weight, so surviving siblings keep
    /// their relative shares); true when something was removed.
    fn remove_leaf(node: &mut LayoutNode, id: PaneId) -> bool {
        match node {
            LayoutNode::Pane { .. } => false,
            LayoutNode::Split {
                children, weights, ..
            } => {
                if let Some(pos) = children
                    .iter()
                    .position(|c| matches!(c, LayoutNode::Pane { id: pane } if *pane == id))
                {
                    children.remove(pos);
                    if weights.len() == children.len() + 1 {
                        weights.remove(pos);
                    }
                    true
                } else {
                    children.iter_mut().any(|c| Self::remove_leaf(c, id))
                }
            }
        }
    }

    /// Collapse single-child splits bottom-up and repair weight vectors.
    /// After this, every split has ≥2 children with matching weights —
    /// except a lone root pane, which is the valid single-pane tab.
    fn normalize_node(node: &mut LayoutNode) {
        match node {
            LayoutNode::Pane { .. } => {}
            LayoutNode::Split {
                children, weights, ..
            } => {
                for child in children.iter_mut() {
                    Self::normalize_node(child);
                }
                children.retain(
                    |c| !matches!(c, LayoutNode::Split { children: inner, .. } if inner.is_empty()),
                );
                if children.len() == 1 {
                    let only = children.pop().expect("single child present");
                    *node = only;
                } else if !children.is_empty() && weights.len() != children.len() {
                    let equal = 1.0 / children.len() as f32;
                    *weights = vec![equal; children.len()];
                }
            }
        }
    }

    /// Normalize the whole tree (public for restore paths).
    pub fn normalize(&mut self) {
        Self::normalize_node(&mut self.root);
    }

    /// Shift a divider by pixel delta: `divider_idx` separates children
    /// `idx`/`idx+1` of `split_id`. Only that pair changes; both stay above
    /// the `min_px` floor within `extent_px`. No-ops on unknown splits,
    /// bad indices, or non-positive extents.
    pub fn resize_by_delta(
        &mut self,
        split_id: SplitId,
        divider_idx: usize,
        delta_px: f32,
        extent_px: f32,
        min_px: f32,
    ) -> bool {
        let changed = self.resize_by_delta_unmarked(split_id, divider_idx, delta_px, extent_px, min_px);
        if changed {
            // Sized by hand: it keeps its shares from now on.
            self.manual.insert(split_id);
        }
        changed
    }

    fn resize_by_delta_unmarked(
        &mut self,
        split_id: SplitId,
        divider_idx: usize,
        delta_px: f32,
        extent_px: f32,
        min_px: f32,
    ) -> bool {
        if extent_px <= 0.0 {
            return false;
        }
        let Some(node) = Self::find_split_mut(&mut self.root, split_id) else {
            return false;
        };
        let (weights, children_len) = match node {
            LayoutNode::Split {
                weights, children, ..
            } => (weights, children.len()),
            LayoutNode::Pane { .. } => return false,
        };
        if divider_idx + 1 >= children_len {
            return false;
        }
        if weights.len() != children_len {
            let equal = 1.0 / children_len as f32;
            *weights = vec![equal; children_len];
        }
        // Work in normalized shares so the pair sum (and therefore the
        // whole split) is preserved exactly.
        let sum: f32 = weights.iter().map(|w| w.max(0.0)).sum();
        if sum <= f32::EPSILON {
            return false;
        }
        let lo = (min_px / extent_px).clamp(0.0, 1.0);
        let pair = (weights[divider_idx].max(0.0) + weights[divider_idx + 1].max(0.0)) / sum;
        if pair <= 0.0 {
            return false;
        }
        let shift = delta_px / extent_px;
        // Clamp the moved share so neither side drops below its floor. When
        // the pair itself cannot fit two floors, hold the middle.
        let moved = (weights[divider_idx] / sum + shift)
            .clamp(lo.min(pair / 2.0), (pair - lo).max(pair / 2.0));
        weights[divider_idx] = moved * sum;
        weights[divider_idx + 1] = (pair - moved) * sum;
        // Rebalance float drift so shares always sum to the original total.
        let total: f32 = weights.iter().sum();
        if total > f32::EPSILON {
            for w in weights.iter_mut() {
                *w /= total;
            }
        }
        true
    }

    /// Set every child of a split to an equal share (separator equalize).
    pub fn equalize_split(&mut self, split_id: SplitId) -> bool {
        self.manual.remove(&split_id);
        let Some(node) = Self::find_split_mut(&mut self.root, split_id) else {
            return false;
        };
        match node {
            LayoutNode::Split {
                children, weights, ..
            } if !children.is_empty() => {
                let equal = 1.0 / children.len() as f32;
                *weights = vec![equal; children.len()];
                true
            }
            _ => false,
        }
    }

    /// Splits whose children don't share their space evenly.
    fn uneven_splits(&self) -> std::collections::BTreeSet<SplitId> {
        fn walk(node: &LayoutNode, out: &mut std::collections::BTreeSet<SplitId>) {
            if let LayoutNode::Split { id, children, weights, .. } = node {
                if !children.is_empty() && weights.len() == children.len() {
                    let total: f32 = weights.iter().sum();
                    let even = total / children.len() as f32;
                    if weights.iter().any(|w| (w - even).abs() > 0.01 * total.max(f32::EPSILON)) {
                        out.insert(*id);
                    }
                }
                children.iter().for_each(|child| walk(child, out));
            }
        }
        let mut out = Default::default();
        walk(&self.root, &mut out);
        out
    }

    /// Every split not sized by hand shares its space evenly; forgets
    /// splits that no longer exist.
    fn rebalance(&mut self) {
        fn walk(node: &mut LayoutNode, manual: &std::collections::BTreeSet<SplitId>, seen: &mut Vec<SplitId>) {
            if let LayoutNode::Split { id, children, weights, .. } = node {
                seen.push(*id);
                if !children.is_empty() && !manual.contains(id) {
                    *weights = vec![1.0 / children.len() as f32; children.len()];
                }
                children.iter_mut().for_each(|child| walk(child, manual, seen));
            }
        }
        let mut seen = Vec::new();
        walk(&mut self.root, &self.manual, &mut seen);
        self.manual.retain(|id| seen.contains(id));
    }

    /// Make every split share its space evenly.
    pub fn equalize_all(&mut self) {
        self.manual.clear();
        fn even(node: &mut LayoutNode) {
            if let LayoutNode::Split {
                children, weights, ..
            } = node
            {
                if !children.is_empty() {
                    *weights = vec![1.0 / children.len() as f32; children.len()];
                }
                children.iter_mut().for_each(even);
            }
        }
        even(&mut self.root);
    }

    /// Exchange the places of two panes. Their sessions go with them; the
    /// shape of the layout and every share stay as they were.
    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> bool {
        fn relabel(node: &mut LayoutNode, a: PaneId, b: PaneId) {
            match node {
                LayoutNode::Pane { id } if *id == a => *id = b,
                LayoutNode::Pane { id } if *id == b => *id = a,
                LayoutNode::Pane { .. } => {}
                LayoutNode::Split { children, .. } => {
                    children.iter_mut().for_each(|child| relabel(child, a, b))
                }
            }
        }
        if a == b || !self.contains(a) || !self.contains(b) {
            return false;
        }
        relabel(&mut self.root, a, b);
        true
    }

    /// Give `pane` this share (0.1–0.9) of the split directly containing it;
    /// its siblings divide the rest in their current proportions.
    pub fn set_share(&mut self, pane: PaneId, share: f32) -> bool {
        fn parent_of(node: &mut LayoutNode, pane: PaneId) -> Option<(&mut LayoutNode, usize)> {
            let LayoutNode::Split { children, .. } = node else {
                return None;
            };
            if let Some(index) = children
                .iter()
                .position(|c| matches!(c, LayoutNode::Pane { id } if *id == pane))
            {
                return Some((node, index));
            }
            let LayoutNode::Split { children, .. } = node else {
                return None;
            };
            children.iter_mut().find_map(|child| parent_of(child, pane))
        }
        if !share.is_finite() {
            return false;
        }
        let share = share.clamp(0.1, 0.9);
        let Some((node, index)) = parent_of(&mut self.root, pane) else {
            return false;
        };
        let current = node.shares();
        let LayoutNode::Split { id, weights, .. } = node else {
            return false;
        };
        // Asked for by size: the split keeps it from now on.
        self.manual.insert(*id);
        let others: f32 = current
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, s)| *s)
            .sum();
        let siblings = current.len().saturating_sub(1).max(1) as f32;
        *weights = current
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if i == index {
                    share
                } else if others > f32::EPSILON {
                    (1.0 - share) * s / others
                } else {
                    (1.0 - share) / siblings
                }
            })
            .collect();
        true
    }

    /// The tree as JSON for agents: splits with their orientation and each
    /// child's share, panes by id.
    pub fn describe(&self) -> serde_json::Value {
        fn describe_node(node: &LayoutNode, share: f32) -> serde_json::Value {
            let share = ((share as f64) * 100.0).round() / 100.0;
            match node {
                LayoutNode::Pane { id } => serde_json::json!({ "pane": id, "share": share }),
                LayoutNode::Split {
                    id,
                    direction,
                    children,
                    ..
                } => serde_json::json!({
                    "split": id,
                    "arrangement": match direction {
                        SplitDirection::Horizontal => "side_by_side",
                        SplitDirection::Vertical => "stacked",
                    },
                    "share": share,
                    "children": children
                        .iter()
                        .zip(node.shares())
                        .map(|(child, share)| describe_node(child, share))
                        .collect::<Vec<_>>(),
                }),
            }
        }
        describe_node(&self.root, 1.0)
    }

    fn find_split_mut(node: &mut LayoutNode, split_id: SplitId) -> Option<&mut LayoutNode> {
        match node {
            LayoutNode::Pane { .. } => None,
            LayoutNode::Split { id, .. } if *id == split_id => Some(node),
            LayoutNode::Split { children, .. } => children
                .iter_mut()
                .find_map(|c| Self::find_split_mut(c, split_id)),
        }
    }

    /// Divider count of a split node (children − 1), if it exists.
    pub fn divider_count(&self, split_id: SplitId) -> Option<usize> {
        fn find(node: &LayoutNode, split_id: SplitId) -> Option<usize> {
            match node {
                LayoutNode::Pane { .. } => None,
                LayoutNode::Split { id, children, .. } => {
                    if *id == split_id {
                        return Some(children.len().saturating_sub(1));
                    }
                    children.iter().find_map(|c| find(c, split_id))
                }
            }
        }
        find(&self.root, split_id)
    }

    /// Orientation of a split node, if it exists.
    pub fn split_direction(&self, split_id: SplitId) -> Option<SplitDirection> {
        fn find(node: &LayoutNode, split_id: SplitId) -> Option<SplitDirection> {
            match node {
                LayoutNode::Pane { .. } => None,
                LayoutNode::Split {
                    id,
                    direction,
                    children,
                    ..
                } => {
                    if *id == split_id {
                        return Some(*direction);
                    }
                    children.iter().find_map(|c| find(c, split_id))
                }
            }
        }
        find(&self.root, split_id)
    }

    /// Pixel rectangles for every pane within a `width × height` area,
    /// derived top-down from weights. Min floors are enforced per split
    /// (see [`distribute`]); when an area cannot fit all floors, children
    /// share it equally rather than collapsing.
    pub fn pane_rects(
        &self,
        width: f32,
        height: f32,
        min_w: f32,
        min_h: f32,
    ) -> HashMap<PaneId, Rect> {
        let mut out = HashMap::new();
        self.place(
            &self.root,
            Rect {
                x: 0.0,
                y: 0.0,
                width: width.max(0.0),
                height: height.max(0.0),
            },
            min_w,
            min_h,
            &mut out,
        );
        out
    }

    fn place(
        &self,
        node: &LayoutNode,
        rect: Rect,
        min_w: f32,
        min_h: f32,
        panes: &mut HashMap<PaneId, Rect>,
    ) {
        match node {
            LayoutNode::Pane { id } => {
                panes.insert(*id, rect);
            }
            LayoutNode::Split {
                id,
                direction,
                children,
                ..
            } => {
                let shares = node.shares();
                let (total, min) = match direction {
                    SplitDirection::Horizontal => (rect.width, min_w),
                    SplitDirection::Vertical => (rect.height, min_h),
                };
                let sizes = distribute(total, &shares, min);
                let mut offset = 0.0;
                for (child, size) in children.iter().zip(sizes.iter()) {
                    let child_rect = match direction {
                        SplitDirection::Horizontal => Rect {
                            x: rect.x + offset,
                            y: rect.y,
                            width: *size,
                            height: rect.height,
                        },
                        SplitDirection::Vertical => Rect {
                            x: rect.x,
                            y: rect.y + offset,
                            width: rect.width,
                            height: *size,
                        },
                    };
                    offset += *size;
                    self.place(child, child_rect, min_w, min_h, panes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes(layout: &PaneLayout) -> Vec<PaneId> {
        layout.panes()
    }

    #[test]
    fn swap_share_equalize_and_describe() {
        // 1 | (2 / 3)
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        assert_eq!(panes(&layout), vec![1, 2, 3]);

        assert!(layout.swap_panes(1, 3));
        assert_eq!(panes(&layout), vec![3, 2, 1], "places swap, shape stays");
        assert!(!layout.swap_panes(1, 1));
        assert!(!layout.swap_panes(1, 99));

        assert!(layout.set_share(3, 0.7));
        let tree = layout.describe();
        assert_eq!(tree["arrangement"], "side_by_side");
        assert_eq!(tree["children"][0]["pane"], 3);
        assert_eq!(tree["children"][0]["share"], 0.7);
        assert_eq!(tree["children"][1]["arrangement"], "stacked");
        assert_eq!(tree["children"][1]["share"], 0.3);
        // Shares are clamped so no pane vanishes.
        assert!(layout.set_share(3, 2.0));
        assert_eq!(layout.describe()["children"][0]["share"], 0.9);
        assert!(!layout.set_share(42, 0.5), "unknown pane");
        assert!(!layout.set_share(3, f32::NAN));

        layout.equalize_all();
        let tree = layout.describe();
        assert_eq!(tree["children"][0]["share"], 0.5);
        assert_eq!(tree["children"][1]["children"][0]["share"], 0.5);
        assert!(PaneLayout::new(7).describe()["pane"] == 7);
    }

    #[test]
    fn reserved_editor_id_behaves_like_any_pane() {
        // The engine never special-cases the shared editor pane id: view
        // code keeps it out of saves, but splits, moves, focus, geometry,
        // and close all treat it as an ordinary pane.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, EDITOR_PANE_ID, SplitSide::Right));
        assert!(layout.split_pane(EDITOR_PANE_ID, 3, SplitSide::Right));
        assert_eq!(panes(&layout), vec![1, EDITOR_PANE_ID, 3]);
        assert!(layout.contains(EDITOR_PANE_ID));
        let rects = layout.pane_rects(1200.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        // Three side by side share the width equally.
        assert_eq!(rects[&EDITOR_PANE_ID].width, 400.0);
        assert!(layout.move_pane_oriented(EDITOR_PANE_ID, 1, SplitSide::Left));
        assert_eq!(panes(&layout), vec![EDITOR_PANE_ID, 1, 3]);
        assert_eq!(layout.active(), Some(EDITOR_PANE_ID));
        layout.focus(1);
        assert!(layout.close_pane(EDITOR_PANE_ID));
        assert_eq!(panes(&layout), vec![1, 3]);
    }

    #[test]
    fn splits_share_equally_until_sized_by_hand() {
        let shares = |layout: &PaneLayout| match layout.root() {
            LayoutNode::Split { weights, .. } => weights.clone(),
            LayoutNode::Pane { .. } => vec![1.0],
        };
        let split_id = |layout: &PaneLayout| match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            LayoutNode::Pane { .. } => panic!("not split"),
        };
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Right));
        assert!(layout.split_pane(1, 4, SplitSide::Right));
        assert_eq!(shares(&layout), vec![0.25; 4], "every split leaves equal widths");
        assert!(layout.close_pane(4));
        let third = 1.0 / 3.0;
        assert_eq!(shares(&layout), vec![third; 3], "and so does every close");

        // Dragged by hand: kept through later splits and closes.
        let id = split_id(&layout);
        assert!(layout.resize_by_delta(id, 0, 200.0, 1200.0, 100.0));
        let sized = shares(&layout);
        assert!((sized[0] - sized[2]).abs() > 0.1);
        assert!(layout.split_pane(3, 5, SplitSide::Down));
        assert_eq!(&shares(&layout)[..], &sized[..], "a split elsewhere keeps the hand-sized widths");

        // Equalize hands it back.
        assert!(layout.equalize_split(id));
        assert!(layout.split_pane(2, 6, SplitSide::Right));
        assert_eq!(shares(&layout), vec![0.25; 4]);
    }

    #[test]
    fn restored_uneven_layouts_count_as_sized_by_hand() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        let id = match layout.root() { LayoutNode::Split { id, .. } => *id, _ => unreachable!() };
        assert!(layout.resize_by_delta(id, 0, 300.0, 1000.0, 100.0));
        let restored = PaneLayout::restore(layout.root().clone(), layout.active());
        let mut restored = restored;
        let before = match restored.root() { LayoutNode::Split { weights, .. } => weights.clone(), _ => unreachable!() };
        assert!(restored.split_pane(2, 3, SplitSide::Down));
        let after = match restored.root() { LayoutNode::Split { weights, .. } => weights.clone(), _ => unreachable!() };
        assert_eq!(before, after);
    }

    #[test]
    fn single_pane_split_right() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.is_single());
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert_eq!(panes(&layout), vec![1, 2]);
        assert!(matches!(
            layout.root(),
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ..
            }
        ));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].width, 500.0);
        assert_eq!(rects[&2].width, 500.0);
        assert_eq!(rects[&2].x, 500.0);
        assert_eq!(layout.active(), Some(2));
    }

    #[test]
    fn single_pane_split_down() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Down));
        assert!(matches!(
            layout.root(),
            LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ..
            }
        ));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].height, 400.0);
        assert_eq!(rects[&2].height, 400.0);
        assert_eq!(rects[&2].y, 400.0);
    }

    #[test]
    fn split_sides_map_to_direction_and_order() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Left));
        assert_eq!(panes(&layout), vec![2, 1]);
        assert!(matches!(
            layout.root(),
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ..
            }
        ));

        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Up));
        assert_eq!(panes(&layout), vec![2, 1]);
        assert!(matches!(
            layout.root(),
            LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ..
            }
        ));
    }

    #[test]
    fn split_right_then_split_left_pane_down() {
        // A | B, then split A down => A1/A2 stacked beside B.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(1, 3, SplitSide::Down));
        assert_eq!(panes(&layout), vec![1, 3, 2]);
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        // Left subtree keeps its half; the two stacked panes split it evenly.
        assert_eq!(rects[&1].width, 500.0);
        assert_eq!(rects[&3].width, 500.0);
        assert_eq!(rects[&2].x, 500.0);
        assert_eq!(rects[&1].height, 400.0);
        assert_eq!(rects[&3].height, 400.0);
        assert_eq!(rects[&3].y, 400.0);
    }

    #[test]
    fn split_right_then_split_right_pane_down() {
        // A | B, then split B down => B1/B2 stacked at full right-half width.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        assert_eq!(panes(&layout), vec![1, 2, 3]);
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].width, 500.0);
        assert_eq!(rects[&2].width, 500.0);
        assert_eq!(rects[&2].height, 400.0);
        assert_eq!(rects[&3].height, 400.0);
        assert_eq!(rects[&3].x, 500.0);
        assert_eq!(rects[&3].y, 400.0);
    }

    #[test]
    fn split_down_then_split_bottom_pane_right() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Down));
        assert!(layout.split_pane(2, 3, SplitSide::Right));
        assert_eq!(panes(&layout), vec![1, 2, 3]);
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].height, 400.0);
        assert_eq!(rects[&2].width, 500.0);
        assert_eq!(rects[&3].width, 500.0);
        assert_eq!(rects[&2].y, 400.0);
        assert_eq!(rects[&3].y, 400.0);
    }

    #[test]
    fn same_orientation_appends_sibling_with_local_weights() {
        // H[A(0.6), B(0.4)], split B right => H[A(0.6), B(0.2), C(0.2)].
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        // Resize root divider so A holds 60%: move +100px of 1000px.
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, 100.0, 1000.0, MIN_PANE_WIDTH));
        assert!(layout.split_pane(2, 3, SplitSide::Right));
        assert_eq!(panes(&layout), vec![1, 2, 3]);
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert!((rects[&1].width - 600.0).abs() < 0.01);
        assert!((rects[&2].width - 200.0).abs() < 0.01);
        assert!((rects[&3].width - 200.0).abs() < 0.01);
    }

    #[test]
    fn four_pane_grid() {
        // A | B, A down, B down => 2x2 grid of exact quarters.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(1, 3, SplitSide::Down));
        assert!(layout.split_pane(2, 4, SplitSide::Down));
        assert_eq!(panes(&layout), vec![1, 3, 2, 4]);
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        for id in [1, 2, 3, 4] {
            assert_eq!(rects[&id].width, 500.0);
            assert_eq!(rects[&id].height, 400.0);
        }
        assert_eq!((rects[&3].x, rects[&3].y), (0.0, 400.0));
        assert_eq!((rects[&4].x, rects[&4].y), (500.0, 400.0));
    }

    #[test]
    fn deep_nested_split_keeps_ancestors_stable() {
        // A=70% | B=30%, then split B: A must stay exactly 70%.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, 200.0, 1000.0, MIN_PANE_WIDTH));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert!((rects[&1].width - 700.0).abs() < 0.01);
        assert!((rects[&2].width - 300.0).abs() < 0.01);
        assert!((rects[&3].width - 300.0).abs() < 0.01);
        assert_eq!(rects[&2].height, 400.0);
        assert_eq!(rects[&3].height, 400.0);
    }

    #[test]
    fn resize_nested_divider_only_moves_siblings() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        let nested_id = match layout.root() {
            LayoutNode::Split { children, .. } => match &children[1] {
                LayoutNode::Split { id, .. } => *id,
                _ => panic!("expected nested split"),
            },
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(nested_id, 0, 100.0, 800.0, MIN_PANE_HEIGHT));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        // A untouched; B/C shifted by exactly 100px of the 800px height.
        assert_eq!(rects[&1].width, 500.0);
        assert_eq!(rects[&2].height, 500.0);
        assert_eq!(rects[&3].height, 300.0);
    }

    #[test]
    fn resize_root_divider_moves_subtrees() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, -100.0, 1000.0, MIN_PANE_WIDTH));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].width, 400.0);
        assert_eq!(rects[&2].width, 600.0);
        assert_eq!(rects[&3].width, 600.0);
        // Nested 50/50 relationship preserved under the new share.
        assert_eq!(rects[&2].height, 400.0);
        assert_eq!(rects[&3].height, 400.0);
    }

    #[test]
    fn resize_respects_minimums() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        // Try to crush pane 1 far below its 100px floor in a 1000px area.
        assert!(layout.resize_by_delta(root_id, 0, -900.0, 1000.0, MIN_PANE_WIDTH));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].width, MIN_PANE_WIDTH);
        assert_eq!(rects[&2].width, 900.0);
    }

    #[test]
    fn close_leaf_collapses_single_child_splits() {
        // H[A, V[B, C]]; closing C => H[A, B] (no single-child V left).
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        assert!(layout.close_pane(3));
        assert_eq!(panes(&layout), vec![1, 2]);
        assert!(matches!(
            layout.root(),
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children,
                ..
            } if children.len() == 2
        ));
        // B inherits the full right-half share.
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&2].width, 500.0);
        assert_eq!(rects[&2].height, 800.0);
    }

    #[test]
    fn close_nested_leaf_keeps_sibling_share() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(1, 3, SplitSide::Down));
        assert!(layout.close_pane(1));
        assert_eq!(panes(&layout), vec![3, 2]);
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&3].width, 500.0);
        assert_eq!(rects[&3].height, 800.0);
    }

    #[test]
    fn close_last_pane_is_refused_and_close_unknown_is_noop() {
        let mut layout = PaneLayout::new(1);
        assert!(!layout.close_pane(1));
        assert!(layout.is_single());
        assert!(!layout.close_pane(99));
        assert_eq!(panes(&layout), vec![1]);
    }

    #[test]
    fn close_active_pane_focuses_neighbor() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Right));
        layout.focus(2);
        assert!(layout.close_pane(2));
        assert_eq!(layout.active(), Some(3));
        layout.focus(1);
        assert!(layout.close_pane(1));
        assert_eq!(layout.active(), Some(3));
    }

    #[test]
    fn split_rejects_duplicates_and_unknown_targets() {
        let mut layout = PaneLayout::new(1);
        assert!(!layout.split_pane(99, 2, SplitSide::Right));
        assert!(!layout.split_pane(1, 1, SplitSide::Right));
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(!layout.split_pane(1, 2, SplitSide::Right));
        assert_eq!(panes(&layout), vec![1, 2]);
    }

    #[test]
    fn move_oriented_wraps_on_cross_axis() {
        // V[A, B]; move C (outside) above B => V[A, H... no: Up means
        // vertical split with C before B: V[A, V[C, B]].
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Down));
        assert!(layout.move_pane_oriented(3, 2, SplitSide::Up));
        assert_eq!(panes(&layout), vec![1, 3, 2]);
        // Same axis joins as a sibling: H drop onto B's row.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.move_pane_oriented(3, 1, SplitSide::Right));
        assert_eq!(panes(&layout), vec![1, 3, 2]);
        // Cross axis wraps instead of flattening.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.move_pane_oriented(3, 2, SplitSide::Down));
        assert_eq!(panes(&layout), vec![1, 2, 3]);
        assert!(matches!(
            layout.root(),
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                children,
                ..
            } if matches!(&children[1], LayoutNode::Split { direction: SplitDirection::Vertical, .. })
        ));
        assert!(!layout.move_pane_oriented(3, 3, SplitSide::Right));
        assert!(!layout.move_pane_oriented(3, 99, SplitSide::Right));
    }

    #[test]
    fn move_pane_reparents_like_sidebar_drop() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(1, 3, SplitSide::Down));
        // Move 3 after 2: V collapses, root becomes H[1, 2, 3].
        assert!(layout.move_pane(3, 2, false));
        assert_eq!(panes(&layout), vec![1, 2, 3]);
        assert!(!layout.move_pane(3, 3, false));
        // Unknown targets fail; unknown sources are treated as external
        // drops (e.g. sidebar tabs) and succeed.
        assert!(!layout.move_pane(2, 99, false));
        assert!(layout.move_pane(99, 2, false));
        assert_eq!(panes(&layout), vec![1, 2, 99, 3]);
    }

    #[test]
    fn window_resize_keeps_ratios() {
        // Ratios, not pixels, are truth: halving the window halves panes.
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, 200.0, 1000.0, MIN_PANE_WIDTH));
        let wide = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        let narrow = layout.pane_rects(500.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(wide[&1].width, 700.0);
        assert_eq!(narrow[&1].width, 350.0);
        assert_eq!(narrow[&2].width, 150.0);
    }

    #[test]
    fn rapid_split_close_round_trip_is_stable() {
        let mut layout = PaneLayout::new(1);
        let mut next = 2;
        for _ in 0..25 {
            assert!(layout.split_pane(1, next, SplitSide::Right));
            next += 1;
        }
        assert_eq!(layout.pane_count(), 26);
        for id in 2..next {
            assert!(layout.close_pane(id));
        }
        assert_eq!(panes(&layout), vec![1]);
        assert!(layout.is_single());
        // Tree usable after churn; allocator keeps moving forward.
        assert!(layout.split_pane(1, next, SplitSide::Down));
        assert_eq!(layout.pane_count(), 2);
    }

    #[test]
    fn restore_round_trips_ids_weights_and_focus() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        layout.focus(3);
        let restored = PaneLayout::restore(layout.root().clone(), layout.active());
        assert_eq!(restored.panes(), vec![1, 2, 3]);
        assert_eq!(restored.active(), Some(3));
        // Allocator continues past restored ids (no handle-map collisions).
        let mut restored = restored;
        assert!(restored.split_pane(1, 9, SplitSide::Down));
        let ids: std::collections::HashSet<SplitId> = restored.split_ids().into_iter().collect();
        assert_eq!(ids.len(), restored.split_ids().len());
    }

    #[test]
    fn serialized_round_trip_preserves_geometry() {
        use dragent_persistence::SerializedPaneLayout;
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, 200.0, 1000.0, MIN_PANE_WIDTH));
        layout.focus(3);

        let leaf = |id: PaneId| SerializedPaneLayout::Single {
            uuid: uuid::Uuid::nil(),
            cwd: std::path::PathBuf::from("/tmp"),
            shell_title: None,
            is_active: false,
            clear_gap_height_px: 0.0,
            clear_gap_before_block_id: None,
            clear_gap_after_all: false,
            agent_id: None,
            browser_url: None,
        };
        let stored = layout.to_serialized(1000.0, 800.0, &leaf);
        // N-ary/binary nesting: root split holds the left pane plus one
        // nested split for the stacked pair.
        assert!(matches!(stored, SerializedPaneLayout::Split { .. }));

        let before = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        let restored =
            PaneLayout::from_serialized(&stored, &mut vec![1, 2, 3].into_iter(), 1000.0, 800.0);
        assert_eq!(restored.panes(), vec![1, 2, 3]);
        let after = restored.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        for id in [1, 2, 3] {
            for (a, b) in [
                (before[&id].x, after[&id].x),
                (before[&id].y, after[&id].y),
                (before[&id].width, after[&id].width),
                (before[&id].height, after[&id].height),
            ] {
                assert!((a - b).abs() < 0.01, "pane {id} drifted: {a} vs {b}");
            }
        }
        // Restored ids are unique and the allocator continues past them.
        let mut restored = restored;
        assert!(restored.split_pane(1, 9, SplitSide::Down));
        let ids: std::collections::HashSet<SplitId> = restored.split_ids().into_iter().collect();
        assert_eq!(ids.len(), restored.split_ids().len());
    }

    #[test]
    fn parent_direction_drives_alternating_button() {
        let mut layout = PaneLayout::new(1);
        assert_eq!(layout.parent_direction(1), None);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        assert_eq!(layout.parent_direction(1), Some(SplitDirection::Horizontal));
        assert!(layout.split_pane(2, 3, SplitSide::Down));
        assert_eq!(layout.parent_direction(3), Some(SplitDirection::Vertical));
    }

    #[test]
    fn equalize_split_restores_even_shares() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, 300.0, 1000.0, MIN_PANE_WIDTH));
        assert!(layout.equalize_split(root_id));
        let rects = layout.pane_rects(1000.0, 800.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT);
        assert_eq!(rects[&1].width, 500.0);
        assert_eq!(rects[&2].width, 500.0);
        assert!(!layout.equalize_split(9999));
    }

    fn root_shares(layout: &PaneLayout) -> Vec<f32> {
        layout.root().shares()
    }

    fn assert_shares(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 0.002, "shares {actual:?} != {expected:?}");
        }
    }

    #[test]
    fn persisted_split_shares_survive_save_and_restore() {
        let mut layout = PaneLayout::new(1);
        assert!(layout.split_pane(1, 2, SplitSide::Right));
        let root_id = match layout.root() {
            LayoutNode::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        assert!(layout.resize_by_delta(root_id, 0, 120.0, 1000.0, MIN_PANE_WIDTH));
        assert_shares(&root_shares(&layout), &[0.62, 0.38]);
        let leaf = |_: PaneId| SerializedPaneLayout::Single {
            uuid: uuid::Uuid::nil(),
            cwd: std::path::PathBuf::from("/tmp"),
            shell_title: None,
            is_active: false,
            clear_gap_height_px: 0.0,
            clear_gap_before_block_id: None,
            clear_gap_after_all: false,
            agent_id: None,
            browser_url: None,
        };

        let restored =
            PaneLayout::from_persisted(&layout.to_persisted(&leaf), &mut vec![1, 2].into_iter());
        assert_shares(&root_shares(&restored), &[0.62, 0.38]);
        // A second save of the restored tree is identical (no drift per launch).
        let again =
            PaneLayout::from_persisted(&restored.to_persisted(&leaf), &mut vec![1, 2].into_iter());
        assert_shares(&root_shares(&again), &[0.62, 0.38]);
    }

    #[test]
    fn develop_layout_split_shares_survive_restore() {
        let (left, right) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let (left_surface, right_surface) = (
            SurfaceId::from_terminal_pane(left),
            SurfaceId::from_terminal_pane(right),
        );
        let root = SurfaceLayoutNode::Split {
            axis: LayoutAxis::Horizontal,
            children: vec![
                SurfaceLayoutNode::single(left_surface),
                SurfaceLayoutNode::single(right_surface),
            ],
            preferred_sizes: vec![0.62 * SURFACE_SHARE_EXTENT],
        };
        let restored = PaneLayout::from_surface_layout(
            &root,
            |surface| {
                (surface == left_surface)
                    .then_some(1)
                    .or((surface == right_surface).then_some(2))
            },
            Some(left_surface),
        )
        .unwrap();
        assert_shares(&root_shares(&restored), &[0.62, 0.38]);
    }
}
