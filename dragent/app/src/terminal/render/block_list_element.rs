//! Scrollable block list for normal (non-alt) terminal mode.

use async_channel::Sender;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::terminal::render::block_grid_painter::paint_grid_line;
use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};
use crate::workspace::InputEditor;
use dragent_terminal::StyledLine;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, ParentElement, Point, SavePosition, ScrollData,
    ScrollableElement, SelectableElement, Selection, SelectionFragment, SmartSelectFn, Text,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::string_offset::{ByteOffset, CharOffset};
use warpui::text::{IsRect, SelectionDirection, SelectionType};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::Pixels;
use warpui::ViewHandle;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};
use warpui_core::scene::ZIndex;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text::word_boundaries::WordBoundariesPolicy;
use warpui_core::text::TextBuffer;
use warpui_core::text_layout::ClipConfig;

use crate::console::block_filter::{count_block_matches, BlockFilterState};
use crate::console::dir_listing::{entry_click_hint_label, DirSort, DirView};
use crate::console::escape_parse::{
    styled_to_colored, styled_to_colored_exact, ColoredLine, TextSpan,
};
use crate::icons::{render_icon, AppIcon};
use crate::terminal::model::block::{live_primary_line_count, Block, BlockStatus};
use crate::terminal::model::block_height_tree::{
    BlockHeightIndex, BlockListEntry, BLOCK_HEADER_HEIGHT_PX, COMPLETED_BLOCK_MAX_LINES,
};
use crate::terminal::model::is_signal_exit;
use crate::terminal::render::block_card_element::header_command_line;
use crate::terminal::render::dir_listing_paint::{
    dir_entry_char_width, dir_entry_name_origin_in_body, dir_listing_body_height,
    dir_listing_selection_lines, layout_dir_listing, paint_dir_listing, sort_dir_entries,
    DirBlockState, DirListingHit, DirListingLayout, DirListingPaintParams, DIR_BODY_PAD_LEFT,
};
use crate::terminal::render::grid_metrics::{
    line_terminal_column_count, slice_line_terminal_columns, terminal_column_at_char_index,
    terminal_column_center_x, x_to_terminal_column, CellMetrics,
};
use crate::terminal::render::grid_renderer::{build_alt_screen_column, GridLineParams};
use crate::terminal::render::BlockRenderContext;
use crate::terminal::render::CellGlyphCache;
use crate::terminal::viewport::{ScrollPosition, VisibleEntry, VisibleEntryKind};
use crate::theme::{mono_font_size, Theme, TERMINAL_PADDING_LEFT};

pub const PROMPT_TAIL_MAX_LINES: usize = 3;
pub const BLOCK_LIST_BOTTOM_POSITION_ID: &str = "terminal_block_list_bottom";

pub(crate) const BLOCK_ACCENT_WIDTH: f32 = 3.0;

/// Width of the block options dropdown menu.
pub const BLOCK_MENU_WIDTH: f32 = 220.0;

/// The inline output filter bar shown under a block header while its filter is

/// Retained across scroll frames. History conversion only visits newly appended
/// rows; terminal rewrites, resize, eviction and palette changes invalidate it.
#[derive(Default)]
pub struct BlockTextCache {
    live: HashMap<usize, LiveTextSnapshot>,
    rows: HashMap<usize, (Arc<Vec<ColoredLine>>, usize, Arc<Vec<usize>>)>,
}
struct LiveTextSnapshot {
    emulator: usize,
    revision: u64,
    scrollback_version: u64,
    rewrite: u64,
    evicted: usize,
    cols: usize,
    palette: (ColorU, ColorU, [ColorU; 8], [ColorU; 8]),
    history: Vec<ColoredLine>,
    lines: Arc<Vec<ColoredLine>>,
    leading_trim: usize,
}
impl BlockTextCache {
    fn live_lines(
        &mut self,
        id: usize,
        emu: &dragent_terminal::Emulator,
        theme: Theme,
        exact_colors: bool,
    ) -> (Arc<Vec<ColoredLine>>, usize) {
        let colored = |line: &dragent_terminal::StyledLine| {
            if exact_colors {
                styled_to_colored_exact(line, &theme)
            } else {
                styled_to_colored(line, &theme)
            }
        };
        let grid = emu.grid();
        let identity = emu as *const _ as usize;
        let palette = (
            theme.background,
            theme.foreground,
            theme.ansi_normal,
            theme.ansi_bright,
        );
        let reset = self.live.get(&id).is_none_or(|old| {
            old.emulator != identity
                || old.cols != grid.cols
                || old.palette != palette
                || old.rewrite != grid.scrollback_rewrite_version()
                || old.evicted > grid.evicted()
        });
        if reset {
            self.live.insert(
                id,
                LiveTextSnapshot {
                    emulator: identity,
                    revision: u64::MAX,
                    scrollback_version: u64::MAX,
                    rewrite: grid.scrollback_rewrite_version(),
                    evicted: grid.evicted(),
                    cols: grid.cols,
                    palette,
                    history: Vec::new(),
                    lines: Arc::new(Vec::new()),
                    leading_trim: 0,
                },
            );
        }
        let old = self.live.get_mut(&id).unwrap();
        if old.revision == emu.render_revision()
            && old.scrollback_version == grid.scrollback_version()
        {
            return (old.lines.clone(), old.leading_trim);
        }
        let discarded = grid.evicted() - old.evicted;
        old.history.drain(..discarded.min(old.history.len()));
        let history_len = grid.scrollback_len();
        // Non-append mutations reset above, so all surviving rows remain valid.
        old.history.extend(
            emu.styled_lines_in(old.history.len(), history_len)
                .iter()
                .map(&colored),
        );
        let mut lines = old.history.clone();
        lines.extend(
            emu.styled_lines_in(history_len, live_primary_line_count(emu))
                .iter()
                .map(&colored),
        );
        let start = if cfg!(windows) { 0 } else { 1 };
        lines.drain(..start.min(lines.len()));
        let mut leading_trim = 0;
        if cfg!(windows) {
            leading_trim = lines
                .iter()
                .take_while(|line| line.plain_text().trim().is_empty())
                .count()
                .min(lines.len().saturating_sub(1));
            lines.drain(..leading_trim);
        }
        old.lines = Arc::new(lines);
        old.leading_trim = leading_trim;
        old.revision = emu.render_revision();
        old.scrollback_version = grid.scrollback_version();
        old.evicted = grid.evicted();
        (old.lines.clone(), leading_trim)
    }
    pub(crate) fn row_offsets(
        &mut self,
        id: usize,
        lines: &Arc<Vec<ColoredLine>>,
        cols: usize,
    ) -> Arc<Vec<usize>> {
        if let Some((old, width, offsets)) = self.rows.get(&id) {
            if Arc::ptr_eq(old, lines) && *width == cols {
                return offsets.clone();
            }
        }
        let mut offsets = Vec::with_capacity(lines.len() + 1);
        offsets.push(0);
        for line in lines.iter() {
            let width: usize = line
                .spans
                .iter()
                .map(|s| line_terminal_column_count(&s.text))
                .sum();
            offsets.push(offsets.last().unwrap() + width.div_ceil(cols).max(1));
        }
        let offsets = Arc::new(offsets);
        self.rows.insert(id, (lines.clone(), cols, offsets.clone()));
        offsets
    }
}

pub struct BlockListElement {
    pub(crate) visible_entries: Vec<VisibleEntry>,
    pub(crate) block_lines:
        HashMap<usize, (bool, Arc<Vec<ColoredLine>>, Option<(usize, usize, bool)>)>,
    pub(crate) text_cache: Arc<Mutex<BlockTextCache>>,
    pub(crate) row_offsets: HashMap<usize, Arc<Vec<usize>>>,
    pub(crate) all_block_lines: HashMap<usize, Arc<Vec<ColoredLine>>>,
    pub(crate) bootstrap_lines: Vec<ColoredLine>,
    pub(crate) live_prompt: Option<LivePromptSection>,

    pub(crate) theme: Theme,
    pub(crate) mono: FamilyId,
    pub(crate) cell_metrics: CellMetrics,

    pub(crate) height_index: BlockHeightIndex,

    /// Status of each visible block, captured alongside its header so `paint`
    /// can tint the whole block (not just the small header badge) when it
    /// errored — `paint` only sees the pre-built `headers` elements, not the
    /// original `Block`, so this is the one bit of its status that survives.
    pub(crate) block_status: HashMap<usize, BlockStatus>,
    pub(crate) headers: HashMap<usize, Box<dyn Element>>,
    /// Pre-built elements for visible agent blocks. Keyed by `agent_block_id`.
    pub(crate) agent_block_elements: HashMap<usize, Box<dyn Element>>,
    /// Pre-built fix card elements for blocks in `Ready` heal state. Keyed by `block_id`.
    pub(crate) fix_card_elements: HashMap<usize, Box<dyn Element>>,
    pub(crate) dir_blocks: HashMap<usize, DirBlockState>,
    pub(crate) dir_layouts: HashMap<usize, DirListingLayout>,
    pub(crate) dir_sort: DirSort,
    pub(crate) dir_view: DirView,
    pub(crate) entry_hint: Option<EntryHint>,
    pub(crate) content_width_px: f32,
    pub(crate) dir_hovered_entry: Option<(usize, usize)>,
    pub(crate) dir_hovered_sort: Option<DirSort>,
    pub(crate) dir_hovered_view: Option<DirView>,

    /// Shared sink written by after_layout, drained by TerminalView::rebuild_height_index.
    /// Contains the latest per-block measured pixel heights for ALL visible blocks.
    pub(crate) measured_sink: Arc<Mutex<HashMap<usize, f32>>>,
    /// Shared sink for agent block measured heights.
    pub(crate) measured_agent_sink: Arc<Mutex<HashMap<usize, f32>>>,
    /// Channel sender to notify view coordinator on height changes.
    pub(crate) heights_tx: Sender<()>,

    pub(crate) hovered_link: Option<LinkHint>,
    pub(crate) selected_link: Option<LinkHint>,
    pub(crate) tooltip: Option<Box<dyn Element>>,

    pub(crate) viewport_height: f32,
    /// Scroll offset (px) into the first visible block. The paint origin is shifted
    /// up by this amount, so the visible line window in paint-space is
    /// `[first_entry_offset_px, first_entry_offset_px + viewport_height]`. Used to
    /// cull off-screen lines instead of painting an entire tall block every frame.
    pub(crate) first_entry_offset_px: f32,
    pub(crate) viewport_size: Option<Vector2F>,
    pub(crate) origin: Option<Point>,
    pub(crate) glyph_cache: Arc<Mutex<crate::terminal::render::CellGlyphCache>>,
    /// Owned copy of the active output filter (block id + state) so `paint` can
    /// highlight matched substrings. The `&BlockFilterState` handed to `new` only
    /// lives for construction, so we clone it here.
    pub(crate) match_filter: Option<(usize, BlockFilterState)>,
}

impl BlockListElement {
    pub fn new(
        visible_entries: &[VisibleEntry],
        blocks: &[Block],
        agent_blocks: &[crate::agent::controller::block::AgentBlock],
        bootstrap_lines: &[ColoredLine],
        live_prompt: Option<LivePromptSection>,
        theme: Theme,
        mono: FamilyId,
        cell_metrics: CellMetrics,
        height_index: BlockHeightIndex,
        ctx: &BlockRenderContext<'_>,
        viewport_height: f32,
        first_entry_offset_px: f32,
        measured_sink: Arc<Mutex<HashMap<usize, f32>>>,
        measured_agent_sink: Arc<Mutex<HashMap<usize, f32>>>,
        heights_tx: Sender<()>,
        glyph_cache: Arc<Mutex<CellGlyphCache>>,
        text_cache: Arc<Mutex<BlockTextCache>>,
        heal_states: &std::collections::HashMap<usize, crate::agent::self_heal::BlockHealState>,
        block_filter: Option<(usize, &BlockFilterState)>,
        clear_gap_trigger_block_id: Option<usize>,
        live_checkpoint_ids: &std::collections::HashSet<usize>,
    ) -> Self {
        let mut headers = HashMap::new();
        let mut block_status = HashMap::new();
        let mut block_lines = HashMap::new();
        let mut all_block_lines = HashMap::new();
        let mut dir_blocks = HashMap::new();
        let mut fix_card_elements: HashMap<usize, Box<dyn Element>> = HashMap::new();

        let mut cached_text = text_cache.lock().unwrap();
        cached_text
            .live
            .retain(|id, _| blocks.iter().any(|b| b.id == *id && b.emulator.is_some()));

        // Build a fast lookup of which block IDs are currently on screen so the
        // loop below can skip the expensive per-block work for off-screen blocks.
        // Off-screen running blocks are also skipped.
        let visible_block_ids: std::collections::HashSet<usize> = visible_entries
            .iter()
            .filter_map(|e| {
                if let VisibleEntryKind::Block { block_id } = e.kind {
                    Some(block_id)
                } else {
                    None
                }
            })
            .collect();

        // ── Populate text lines for visible blocks ──
        // Non-visible blocks are skipped — iterating all of them every
        // frame causes O(n_blocks) allocations per paint, which causes jitter
        // during fast output (e.g. yarn test with hundreds of completed blocks).
        for block in blocks {
            let block_id = block.id;
            // The `clear`/`cls` command's own block: the active clear gap
            // already stands in for it visually (see rebuild_height_index),
            // so it must never ALSO get a rendered block entry — unlike the
            // visibility skip below, this applies even while it's still live.
            if Some(block_id) == clear_gap_trigger_block_id {
                continue;
            }
            if !visible_block_ids.contains(&block_id) {
                continue;
            }
            // A failed `ls` (e.g. `bash: s: command not found`) must render as
            // plain error text, never as a clickable listing of the error words.
            if let Some(mut listing) = block
                .listing(&theme)
                .filter(|_| block.status.exit_code().unwrap_or(0) == 0)
            {
                sort_dir_entries(&mut listing.entries, ctx.dir_sort);
                let selection_lines =
                    Arc::new(dir_listing_selection_lines(&listing.entries, &theme));
                block_lines.insert(block_id, (block.expanded, selection_lines.clone(), None));
                all_block_lines.insert(block_id, selection_lines);
                dir_blocks.insert(
                    block_id,
                    DirBlockState {
                        block_id,
                        cwd: block.cwd.clone(),
                        entries: listing.entries,
                    },
                );
            } else if let Some(emu) = &block.emulator {
                // Live running block: keep the complete emulator scrollback for
                // selection and later completion. The indexed renderer only paints
                // rows intersecting the viewport, so history is not lost or eagerly
                // laid out just because a command is still running.
                let start = if cfg!(windows) { 0 } else { 1 };
                let (lines, leading_blank_trim) = cached_text.live_lines(block_id, emu, theme, block.uses_exact_colors());
                let (c_row, c_col) = emu.cursor_position();
                let abs_c_row = emu.grid().scrollback_len() + c_row;
                let adj_row = abs_c_row
                    .saturating_sub(start)
                    .saturating_sub(leading_blank_trim);
                all_block_lines.insert(block_id, lines.clone());
                block_lines.insert(
                    block_id,
                    (
                        block.expanded,
                        lines,
                        Some((adj_row, c_col, emu.cursor_visible())),
                    ),
                );
            } else if let Some(filter) = block_filter
                .filter(|(fid, f)| *fid == block_id && !f.is_empty())
                .map(|(_, f)| f)
            {
                // Completed block with an active output filter: keep matching lines
                // from the retained block history.
                let filtered: Vec<ColoredLine> = block
                    .lines
                    .iter()
                    .filter(|l| filter.matches(&l.plain_text()))
                    .cloned()
                    .collect();
                block_lines.insert(block_id, (block.expanded, Arc::new(filtered.clone()), None));
                all_block_lines.insert(block_id, Arc::new(filtered));
            } else {
                // Completed block: render the same bounded history retained by the
                // model and persistence layers.
                let start = block.lines.len().saturating_sub(COMPLETED_BLOCK_MAX_LINES);
                block_lines.insert(
                    block_id,
                    (
                        block.expanded,
                        if start == 0 {
                            block.lines.clone()
                        } else {
                            Arc::new(block.lines[start..].to_vec())
                        },
                        None,
                    ),
                );
                all_block_lines.insert(block_id, block.lines.clone());
            }
        }

        // ── Agent blocks selection support ─────────────────────────────────────
        for ab in agent_blocks {
            let sel_id = usize::MAX - 1000 - ab.id;
            let lines = Arc::new(agent_block_selection_lines(&ab.kind, &theme));
            block_lines.insert(sel_id, (true, lines.clone(), None));
            all_block_lines.insert(sel_id, lines);
        }

        // ── Visible entries only: build header widgets + fix cards for painting ──
        for entry in visible_entries {
            if let VisibleEntryKind::Block { block_id } = entry.kind {
                if let Some(block) = blocks.iter().find(|b| b.id == block_id) {
                    use crate::agent::self_heal::BlockHealState;
                    let default_state = BlockHealState::Idle;
                    let heal_state = heal_states.get(&block_id).unwrap_or(&default_state);
                    let header_filter = block_filter
                        .filter(|(fid, _)| *fid == block_id)
                        .map(|(_, f)| f);
                    block_status.insert(block_id, block.status);
                    headers.insert(
                        block_id,
                        build_block_header_widget(block, heal_state, header_filter, ctx),
                    );
                    // Build fix card when analysis is ready and not dismissed.
                    //
                    // NOTE: the "working" states (Analyzing / AgentRunning) are
                    // deliberately NOT rendered inline here. While self-heal is
                    // operating, its status is pinned above the input in the
                    // self-heal strip (see terminal_view::render_self_heal_strip)
                    // so it stays glued to the input and never scrolls away in
                    // the block stream. Only the final, actionable states
                    // (Ready / ReadyCollapsed / Error) render as inline cards
                    // attached to their originating block.
                    if let BlockHealState::Ready(ref analysis) = *heal_state {
                        fix_card_elements.insert(
                            block_id,
                            crate::terminal::render::self_heal_element::build_fix_card(
                                block_id, analysis, ctx, theme, mono,
                            ),
                        );
                    } else if let BlockHealState::ReadyCollapsed(_) = *heal_state {
                        fix_card_elements.insert(
                            block_id,
                            crate::terminal::render::self_heal_element::build_fix_available_card(
                                block_id, ctx, theme, mono,
                            ),
                        );
                    } else if let BlockHealState::Error { ref message } = *heal_state {
                        fix_card_elements.insert(
                            block_id,
                            crate::terminal::render::self_heal_element::build_error_card(
                                block_id, message, ctx, theme, mono,
                            ),
                        );
                    }
                }
            }
        }

        // ── Agent block elements ───────────────────────────────────────────────
        let mut agent_block_elements: HashMap<usize, Box<dyn Element>> = HashMap::new();
        for entry in visible_entries {
            if let VisibleEntryKind::AgentBlock { agent_block_id } = entry.kind {
                if let Some(ab) = agent_blocks.iter().find(|b| b.id == agent_block_id) {
                    // Half the gap above and below each message, so neighbours
                    // sit one gap apart; a block that draws nothing takes none.
                    let gap = if crate::terminal::render::agent_transcript::renders_nothing(ab) {
                        0.0
                    } else {
                        crate::terminal::render::agent_transcript::MESSAGE_GAP / 2.0
                    };
                    agent_block_elements.insert(
                        agent_block_id,
                        Container::new(crate::terminal::render::agent_block_element::build_agent_block_element(
                            ab,
                            ctx,
                            theme,
                            mono,
                            live_checkpoint_ids,
                        ))
                        .with_padding_left(crate::theme::TERMINAL_PADDING_LEFT)
                        .with_padding_right(crate::theme::TERMINAL_PADDING_LEFT)
                        .with_padding_top(gap)
                        .with_padding_bottom(gap)
                        .finish(),
                    );
                }
            }
        }

        // Wrapping is indexed once per snapshot/width, then reused for scrolling.
        cached_text
            .rows
            .retain(|id, _| block_lines.contains_key(id));
        drop(cached_text);
        Self {
            text_cache,
            row_offsets: HashMap::new(),
            visible_entries: visible_entries.to_vec(),
            block_lines,
            all_block_lines,
            bootstrap_lines: bootstrap_lines.to_vec(),
            live_prompt,
            theme,
            mono,
            cell_metrics,
            height_index,
            headers,
            block_status,
            agent_block_elements,
            fix_card_elements,
            dir_blocks,
            dir_layouts: HashMap::new(),
            dir_sort: ctx.dir_sort,
            dir_view: ctx.dir_view,
            entry_hint: ctx.entry_hint.cloned(),
            content_width_px: ctx.content_width_px,
            dir_hovered_entry: ctx.dir_hover.entry,
            dir_hovered_sort: ctx.dir_hover.sort,
            dir_hovered_view: ctx.dir_hover.view,
            measured_sink,
            measured_agent_sink,
            heights_tx,
            hovered_link: ctx.hovered_link.cloned(),
            selected_link: ctx.link_hint.cloned(),
            tooltip: if let Some(ref hint) = ctx.entry_hint {
                let label = entry_click_hint_label(hint.is_dir, &hint.name);
                let styles = UiComponentStyles {
                    font_family_id: Some(mono),
                    font_size: Some(crate::theme::text_size(11.0)),
                    font_color: Some(theme.main_text()),
                    background: Some(Fill::Solid(theme.surface_3())),
                    border_color: Some(Fill::Solid(theme.outline())),
                    border_width: Some(1.0),
                    border_radius: Some(CornerRadius::with_all(Radius::Pixels(6.0))),
                    padding: Some(Coords {
                        top: 5.0,
                        bottom: 5.0,
                        left: 8.0,
                        right: 8.0,
                    }),
                    ..Default::default()
                };
                Some(Tooltip::new(label, styles).build().finish())
            } else if let Some(ref hint) = ctx.link_hint {
                let label = crate::console::dir_listing::link_click_hint_label();
                let styles = UiComponentStyles {
                    font_family_id: Some(mono),
                    font_size: Some(crate::theme::text_size(11.0)),
                    font_color: Some(theme.main_text()),
                    background: Some(Fill::Solid(theme.surface_3())),
                    border_color: Some(Fill::Solid(theme.outline())),
                    border_width: Some(1.0),
                    border_radius: Some(CornerRadius::with_all(Radius::Pixels(6.0))),
                    padding: Some(Coords {
                        top: 5.0,
                        bottom: 5.0,
                        left: 8.0,
                        right: 8.0,
                    }),
                    ..Default::default()
                };
                Some(Tooltip::new(label, styles).build().finish())
            } else {
                None
            },
            viewport_height,
            first_entry_offset_px,
            viewport_size: None,
            origin: None,
            glyph_cache,
            match_filter: block_filter
                .filter(|(_, f)| !f.is_empty())
                .map(|(id, f)| (id, f.clone())),
        }
    }
}
pub use super::block_list_geom::*;
pub use super::block_list_header::*;
pub use super::block_list_paint::*;
pub use super::block_list_scroll::*;
pub use super::block_list_select::*;
