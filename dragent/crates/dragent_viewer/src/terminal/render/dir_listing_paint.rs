//! Grid-painted directory listings for block mode.
//!
//! Replaces the warpui widget tree with direct scene painting while preserving
//! layout, colors, and interaction (sort, view toggle, entry select/open).

use std::path::PathBuf;

use warpui::color::ColorU;
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::AppContext;
use warpui_core::presenter::PaintContext;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::console::dir_listing::{
    dir_grid_column_count, modifier_click_label, DirEntry, DirSort, DirView, EntryKind,
    DIR_GRID_ROW_SPACING,
};
use crate::console::escape_parse::{ColoredLine, TextSpan};
use crate::terminal::render::grid_metrics::line_terminal_column_count;
use crate::terminal::render::CellGlyphCache;
use crate::terminal::view::actions::EntryHint;
use crate::theme::Theme;

pub const DIR_BODY_PAD_LEFT: f32 = 14.0;
pub const DIR_BODY_PAD_RIGHT: f32 = 14.0;
pub const DIR_BODY_PAD_TOP: f32 = 20.0;
pub const DIR_BODY_PAD_BOTTOM: f32 = 20.0;
const DIR_TOP_BAR_PAD_BOTTOM: f32 = 6.0;
const DIR_TOP_BAR_MARGIN_BOTTOM: f32 = 8.0;
const DIR_MAIN_PAD_TOP: f32 = 4.0;
const DIR_MAIN_PAD_BOTTOM: f32 = 2.0;
const DIR_FOOTER_PAD_TOP: f32 = 10.0;
pub const DIR_ENTRY_PAD: f32 = 4.0;
const DIR_ENTRY_GLYPH_FONT: f32 = 10.0;
pub const DIR_ENTRY_NAME_FONT: f32 = 11.5;
pub const DIR_ENTRY_GLYPH_SLOT: f32 = 12.0;
pub const DIR_ENTRY_LABEL_GAP: f32 = 6.0;
const DIR_ENTRY_CHIP_HEIGHT: f32 = DIR_ENTRY_PAD * 2.0 + 14.0;

pub fn dir_entry_char_width() -> f32 {
    DIR_ENTRY_NAME_FONT * 0.58
}

pub fn dir_entry_name_origin_in_body(region: &DirEntryRegion) -> Vector2F {
    vec2f(
        region.name_rect.origin().x() + DIR_ENTRY_PAD + DIR_ENTRY_GLYPH_SLOT + DIR_ENTRY_LABEL_GAP,
        region.name_rect.origin().y() + DIR_ENTRY_PAD,
    )
}

fn col_in_entry_region(local: Vector2F, region: &DirEntryRegion, max_cols: usize) -> usize {
    let name_x =
        region.name_rect.origin().x() + DIR_ENTRY_PAD + DIR_ENTRY_GLYPH_SLOT + DIR_ENTRY_LABEL_GAP;
    let rel_x = (local.x() - name_x).max(0.0);
    ((rel_x / dir_entry_char_width()).floor() as usize).min(max_cols)
}

#[derive(Clone, Debug)]
pub struct DirBlockState {
    pub block_id: usize,
    pub cwd: PathBuf,
    pub entries: Vec<DirEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirListingHit {
    Entry {
        block_id: usize,
        entry_index: usize,
        is_dir: bool,
    },
    Sort(DirSort),
    View(DirView),
}

#[derive(Clone, Debug)]
pub struct DirEntryRegion {
    pub entry_index: usize,
    pub name_rect: RectF,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct DirListingLayout {
    pub content_width_px: f32,
    pub body_height: f32,
    pub entry_regions: Vec<DirEntryRegion>,
    /// Y center (element-local) for each selection line / entry index.
    pub line_centers_y: Vec<f32>,
    pub sort_name_rect: RectF,
    pub sort_size_rect: RectF,
    pub view_grid_rect: RectF,
    pub view_list_rect: RectF,
}

pub fn sort_dir_entries(entries: &mut [DirEntry], dir_sort: DirSort) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == EntryKind::Directory;
        let b_dir = b.kind == EntryKind::Directory;
        if a_dir != b_dir {
            return if a_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        match dir_sort {
            DirSort::Size => b.size.unwrap_or(0).cmp(&a.size.unwrap_or(0)),
            DirSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
}

pub fn dir_listing_selection_lines(entries: &[DirEntry], theme: &Theme) -> Vec<ColoredLine> {
    entries
        .iter()
        .map(|entry| {
            let is_dir = entry.kind == EntryKind::Directory;
            let text = if is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };
            let color = if entry.name.starts_with('.') {
                theme.disabled_text()
            } else if is_dir {
                theme.ansi_blue()
            } else {
                theme.main_text()
            };
            ColoredLine {
                wrapped: false,
                spans: vec![TextSpan {
                    text,
                    color: Some(color),
                    bg: None,
                    flags: 0,
                }],
            }
        })
        .collect()
}

pub fn dir_listing_body_height(
    entries: &[DirEntry],
    dir_view: DirView,
    content_width_px: f32,
) -> f32 {
    layout_dir_listing(entries, dir_view, content_width_px).body_height
}

pub fn layout_dir_listing(
    entries: &[DirEntry],
    dir_view: DirView,
    content_width_px: f32,
) -> DirListingLayout {
    let inner_width = content_width_px.max(1.0);
    // No header row is drawn any more, so it reserves no height. This MUST stay
    // in step with `paint_dir_listing`, which now starts the body at
    // `DIR_BODY_PAD_TOP`: entry hit regions are positioned from this offset, so a
    // non-zero value here would put every click target a header's-height below
    // the entry it belongs to, and leave an empty band at the top of the block.
    let top_bar_h = 0.0;

    let (main_h, entry_regions, line_centers_y) = match dir_view {
        DirView::Grid => layout_grid_entries(entries, inner_width, top_bar_h),
        DirView::List => layout_list_entries(entries, inner_width, top_bar_h),
    };

    let footer_h = DIR_FOOTER_PAD_TOP + 13.0;
    let toolbar = layout_toolbar(inner_width, top_bar_h);

    let body_height = DIR_BODY_PAD_TOP + top_bar_h + main_h + footer_h + DIR_BODY_PAD_BOTTOM;

    DirListingLayout {
        content_width_px: inner_width,
        body_height,
        entry_regions,
        line_centers_y,
        sort_name_rect: toolbar.sort_name,
        sort_size_rect: toolbar.sort_size,
        view_grid_rect: toolbar.view_grid,
        view_list_rect: toolbar.view_list,
    }
}

struct ToolbarRects {
    sort_name: RectF,
    sort_size: RectF,
    view_grid: RectF,
    view_list: RectF,
}

fn layout_toolbar(inner_width: f32, top_bar_h: f32) -> ToolbarRects {
    let bar_top = DIR_BODY_PAD_TOP;
    let bar_h = top_bar_h - DIR_TOP_BAR_PAD_BOTTOM - DIR_TOP_BAR_MARGIN_BOTTOM;
    let mut x = inner_width - DIR_BODY_PAD_RIGHT;

    let btn_h = 18.0;
    let btn_y = bar_top + (bar_h - btn_h) * 0.5;

    x -= 22.0;
    let view_list = RectF::new(vec2f(x, btn_y), vec2f(22.0, btn_h));
    x -= 22.0;
    let view_grid = RectF::new(vec2f(x, btn_y), vec2f(22.0, btn_h));
    x -= 10.0;

    x -= 28.0;
    let sort_size = RectF::new(vec2f(x, btn_y), vec2f(28.0, btn_h));
    x -= 10.0;
    x -= 32.0;
    let sort_name = RectF::new(vec2f(x, btn_y), vec2f(32.0, btn_h));

    ToolbarRects {
        sort_name,
        sort_size,
        view_grid,
        view_list,
    }
}

fn entry_display_name(entry: &DirEntry) -> String {
    if entry.kind == EntryKind::Directory {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    }
}

fn paint_text_width(text: &str, font_size: f32) -> f32 {
    line_terminal_column_count(text) as f32 * font_size * 0.58
}

/// Truncate `name` with a trailing ellipsis so it paints within `max_px`.
/// Long names (NTUSER.DAT{guid}.regtrans-ms…) otherwise overflow their grid
/// column and overlap the neighbouring entry.
fn truncate_to_width(name: &str, max_px: f32, font_size: f32) -> String {
    // Chips are sized to their own name's width, so for every non-clamped chip
    // max_px equals the text width MINUS float rounding from the layout's
    // add-then-subtract of the chrome constants. Without slack, exact-fit names
    // truncate ("dev/" → "de…"). One pixel absorbs the rounding and can never
    // cause a visible overlap.
    const SLACK_PX: f32 = 1.0;
    if paint_text_width(name, font_size) <= max_px + SLACK_PX {
        return name.to_string();
    }
    let ellipsis_px = paint_text_width("…", font_size);
    let mut out = String::new();
    let mut used = 0.0;
    for ch in name.chars() {
        let w = paint_text_width(&ch.to_string(), font_size);
        if used + w + ellipsis_px > max_px + SLACK_PX {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push('…');
    out
}

fn grid_chip_width(entry: &DirEntry) -> f32 {
    let name = entry_display_name(entry);
    DIR_ENTRY_PAD * 2.0
        + DIR_ENTRY_GLYPH_SLOT
        + DIR_ENTRY_LABEL_GAP
        + paint_text_width(&name, DIR_ENTRY_NAME_FONT)
}

fn layout_grid_entries(
    entries: &[DirEntry],
    inner_width: f32,
    top_bar_h: f32,
) -> (f32, Vec<DirEntryRegion>, Vec<f32>) {
    if entries.is_empty() {
        return (
            DIR_MAIN_PAD_TOP + DIR_MAIN_PAD_BOTTOM,
            Vec::new(),
            Vec::new(),
        );
    }

    let column_count = dir_grid_column_count(inner_width);
    let col_width = inner_width / column_count as f32;
    let mut y = DIR_BODY_PAD_TOP + top_bar_h + DIR_MAIN_PAD_TOP;
    let mut entry_regions = Vec::new();
    let mut line_centers_y = Vec::new();

    for (row_idx, chunk) in entries.chunks(column_count).enumerate() {
        if row_idx > 0 {
            y += DIR_GRID_ROW_SPACING;
        }
        let row_h = DIR_ENTRY_CHIP_HEIGHT;

        for (col_idx, entry) in chunk.iter().enumerate() {
            let entry_index = row_idx * column_count + col_idx;
            let chip_w = grid_chip_width(entry).min(col_width);
            let x = col_idx as f32 * col_width;
            let chip_rect = RectF::new(vec2f(x, y), vec2f(chip_w, row_h));
            line_centers_y.push(chip_rect.center().y());
            entry_regions.push(DirEntryRegion {
                entry_index,
                name_rect: chip_rect,
                is_dir: entry.kind == EntryKind::Directory,
            });
        }
        y += row_h;
    }

    y += DIR_MAIN_PAD_BOTTOM;
    let main_h = y - (DIR_BODY_PAD_TOP + top_bar_h);
    (main_h, entry_regions, line_centers_y)
}

fn layout_list_entries(
    entries: &[DirEntry],
    inner_width: f32,
    top_bar_h: f32,
) -> (f32, Vec<DirEntryRegion>, Vec<f32>) {
    let row_h = 19.5;
    let mut y = DIR_BODY_PAD_TOP + top_bar_h + DIR_MAIN_PAD_TOP;
    let mut entry_regions = Vec::new();
    let mut line_centers_y = Vec::new();

    for (entry_index, entry) in entries.iter().enumerate() {
        let name_rect = RectF::new(
            vec2f(8.0 + 18.0, y + 4.0),
            vec2f(inner_width - 160.0, row_h - 8.0),
        );
        line_centers_y.push(name_rect.center().y());
        entry_regions.push(DirEntryRegion {
            entry_index,
            name_rect,
            is_dir: entry.kind == EntryKind::Directory,
        });
        y += row_h;
    }

    y += DIR_MAIN_PAD_BOTTOM;
    let main_h = y - (DIR_BODY_PAD_TOP + top_bar_h);
    (main_h, entry_regions, line_centers_y)
}

impl DirListingLayout {
    pub fn hit_test(&self, local: Vector2F) -> Option<DirListingHit> {
        for region in &self.entry_regions {
            if region.name_rect.contains_point(local) {
                return Some(DirListingHit::Entry {
                    block_id: 0,
                    entry_index: region.entry_index,
                    is_dir: region.is_dir,
                });
            }
        }
        None
    }

    pub fn hit_test_selection(
        &self,
        local: Vector2F,
        entries: &[DirEntry],
        dir_view: DirView,
    ) -> Option<(usize, usize)> {
        let entry_count = entries.len();
        for region in &self.entry_regions {
            if region.name_rect.contains_point(local) {
                let entry = entries.get(region.entry_index)?;
                let max_cols = line_terminal_column_count(&entry_display_name(entry)).max(1);
                let col = col_in_entry_region(local, region, max_cols);
                return Some((region.entry_index.min(entry_count.saturating_sub(1)), col));
            }
        }

        let region = match dir_view {
            DirView::Grid => self.grid_region_at_point(local, entry_count)?,
            DirView::List => self.list_region_at_point(local)?,
        };
        let entry = entries.get(region.entry_index)?;
        let max_cols = line_terminal_column_count(&entry_display_name(entry)).max(1);
        let col = if region.name_rect.contains_point(local) {
            col_in_entry_region(local, region, max_cols)
        } else if local.x() < dir_entry_name_origin_in_body(region).x() {
            0
        } else {
            max_cols
        };
        Some((region.entry_index.min(entry_count.saturating_sub(1)), col))
    }

    fn grid_region_at_point(&self, local: Vector2F, entry_count: usize) -> Option<&DirEntryRegion> {
        let column_count = dir_grid_column_count(self.content_width_px).max(1);
        let col_width = self.content_width_px / column_count as f32;
        let col_idx = ((local.x() / col_width).floor() as usize).min(column_count - 1);

        let mut row_ys: Vec<f32> = self
            .entry_regions
            .iter()
            .map(|r| r.name_rect.origin().y())
            .collect();
        row_ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        row_ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);

        let row_y = row_ys
            .iter()
            .min_by(|a, b| {
                (local.y() - *a)
                    .abs()
                    .partial_cmp(&(local.y() - *b).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()?;

        let mut row_regions: Vec<&DirEntryRegion> = self
            .entry_regions
            .iter()
            .filter(|r| (r.name_rect.origin().y() - row_y).abs() < 0.5)
            .collect();
        row_regions.sort_by(|a, b| {
            a.name_rect
                .origin()
                .x()
                .partial_cmp(&b.name_rect.origin().x())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let region = row_regions
            .get(col_idx)
            .or_else(|| row_regions.last())
            .copied()?;
        if region.entry_index >= entry_count {
            return row_regions.last().copied();
        }
        Some(region)
    }

    fn list_region_at_point(&self, local: Vector2F) -> Option<&DirEntryRegion> {
        self.entry_regions.iter().min_by(|a, b| {
            let da = (local.y() - a.name_rect.center().y()).abs()
                + (local.x() - a.name_rect.center().x()).abs() * 0.05;
            let db = (local.y() - b.name_rect.center().y()).abs()
                + (local.x() - b.name_rect.center().x()).abs() * 0.05;
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn line_center_y(&self, line: usize) -> Option<f32> {
        self.line_centers_y.get(line).copied()
    }
}

pub struct DirListingPaintParams<'a> {
    pub block_id: usize,
    pub cwd: &'a PathBuf,
    pub entries: &'a [DirEntry],
    pub layout: &'a DirListingLayout,
    pub theme: &'a Theme,
    pub mono: FamilyId,
    pub dir_sort: DirSort,
    pub dir_view: DirView,
    pub entry_hint: Option<&'a EntryHint>,
    pub hovered_entry: Option<usize>,
    pub hovered_sort: Option<DirSort>,
    pub hovered_view: Option<DirView>,
}

pub fn paint_dir_listing(
    origin: Vector2F,
    params: &DirListingPaintParams<'_>,
    ctx: &mut PaintContext,
    app: &AppContext,
    glyph_cache: &mut CellGlyphCache,
) {
    let theme = params.theme;
    let mono = params.mono;
    let content_w = params.layout.content_width_px;
    let inner_origin = origin + vec2f(DIR_BODY_PAD_LEFT, 0.0);
    // No header row at all: the breadcrumb path, the sort ("name · size") and
    // view (grid/list) controls, and the divider under them are all gone — the
    // block header above already names the command and the entries speak for
    // themselves. The listing body starts straight away. Hit regions for the old
    // controls are disabled to match (see `DirListingLayout::hit_test`), so
    // nothing reacts to clicks in space that no longer shows anything.
    let main_top = origin.y() + DIR_BODY_PAD_TOP;
    match params.dir_view {
        DirView::Grid => {
            paint_grid_body(params, origin, main_top, content_w, ctx, app, glyph_cache)
        }
        DirView::List => {
            paint_list_body(params, origin, main_top, content_w, ctx, app, glyph_cache)
        }
    }

    let footer_y = origin.y() + params.layout.body_height - DIR_BODY_PAD_BOTTOM - 13.0;
    let footer = format!(
        "{} items — click a name, {} to open",
        params.entries.len(),
        modifier_click_label()
    );
    paint_text(
        ctx,
        app,
        glyph_cache,
        mono,
        vec2f(inner_origin.x(), footer_y),
        &footer,
        11.0,
        theme.sub_text(),
        Weight::Normal,
    );
}

fn paint_grid_body(
    params: &DirListingPaintParams<'_>,
    block_origin: Vector2F,
    _main_top: f32,
    _content_w: f32,
    ctx: &mut PaintContext,
    app: &AppContext,
    glyph_cache: &mut CellGlyphCache,
) {
    for region in &params.layout.entry_regions {
        let Some(entry) = params.entries.get(region.entry_index) else {
            continue;
        };
        let chip_origin = block_origin
            + vec2f(
                DIR_BODY_PAD_LEFT + region.name_rect.origin().x(),
                region.name_rect.origin().y(),
            );
        paint_entry_chip(
            params,
            entry,
            region.entry_index,
            chip_origin,
            region.name_rect.size(),
            ctx,
            app,
            glyph_cache,
        );
    }
}

fn paint_list_body(
    params: &DirListingPaintParams<'_>,
    block_origin: Vector2F,
    main_top: f32,
    content_w: f32,
    ctx: &mut PaintContext,
    app: &AppContext,
    glyph_cache: &mut CellGlyphCache,
) {
    let row_h = 19.5;
    let mut y = main_top + DIR_MAIN_PAD_TOP;

    for (entry_index, entry) in params.entries.iter().enumerate() {
        let x = block_origin.x() + DIR_BODY_PAD_LEFT;
        paint_list_row(
            params,
            entry,
            entry_index,
            vec2f(x, y),
            content_w,
            row_h,
            ctx,
            app,
            glyph_cache,
        );
        y += row_h;
    }
}

fn paint_entry_chip(
    params: &DirListingPaintParams<'_>,
    entry: &DirEntry,
    entry_index: usize,
    origin: Vector2F,
    chip_size: Vector2F,
    ctx: &mut PaintContext,
    app: &AppContext,
    glyph_cache: &mut CellGlyphCache,
) {
    let theme = params.theme;
    let is_dir = entry.kind == EntryKind::Directory;
    let selected = params
        .entry_hint
        .is_some_and(|h| h.block_id == params.block_id && h.name == entry.name);
    let hovered = params.hovered_entry == Some(entry_index);

    let chip = RectF::new(origin, chip_size);
    if selected {
        paint_rounded_rect(ctx, chip, theme.link_surface(), None);
    } else if hovered {
        paint_rounded_rect(ctx, chip, theme.fg_overlay_1(), Some(theme.outline()));
    }

    let (glyph, glyph_color) = file_glyph(&entry.name, is_dir, theme);
    let dim = entry.name.starts_with('.');
    let base_color = if dim {
        theme.disabled_text()
    } else if is_dir {
        theme.ansi_blue()
    } else {
        theme.main_text()
    };
    let text_color = if hovered { theme.link() } else { base_color };

    let inner = origin + vec2f(DIR_ENTRY_PAD, DIR_ENTRY_PAD);
    paint_text(
        ctx,
        app,
        glyph_cache,
        params.mono,
        inner,
        glyph,
        DIR_ENTRY_GLYPH_FONT,
        glyph_color,
        Weight::Normal,
    );

    let name_max_px =
        chip_size.x() - DIR_ENTRY_PAD * 2.0 - DIR_ENTRY_GLYPH_SLOT - DIR_ENTRY_LABEL_GAP;
    let name = truncate_to_width(&entry_display_name(entry), name_max_px, DIR_ENTRY_NAME_FONT);
    paint_text(
        ctx,
        app,
        glyph_cache,
        params.mono,
        inner + vec2f(DIR_ENTRY_GLYPH_SLOT + DIR_ENTRY_LABEL_GAP, 0.0),
        &name,
        DIR_ENTRY_NAME_FONT,
        text_color,
        if is_dir {
            Weight::Semibold
        } else {
            Weight::Normal
        },
    );
    if hovered {
        paint_hover_underline(
            ctx,
            inner + vec2f(DIR_ENTRY_GLYPH_SLOT + DIR_ENTRY_LABEL_GAP, 0.0),
            &name,
            DIR_ENTRY_NAME_FONT,
            text_color,
        );
    }
}

/// Underline a hovered entry name, matching how links in block output
/// show they're clickable on hover.
fn paint_hover_underline(
    ctx: &mut PaintContext,
    text_origin: Vector2F,
    text: &str,
    font_size: f32,
    color: ColorU,
) {
    let y = text_origin.y() + font_size * 1.25;
    let rect = RectF::new(
        vec2f(text_origin.x(), y),
        vec2f(paint_text_width(text, font_size), 1.0),
    );
    ctx.scene
        .draw_rect_without_hit_recording(rect)
        .with_background(color);
}

fn paint_list_row(
    params: &DirListingPaintParams<'_>,
    entry: &DirEntry,
    entry_index: usize,
    origin: Vector2F,
    content_w: f32,
    row_h: f32,
    ctx: &mut PaintContext,
    app: &AppContext,
    glyph_cache: &mut CellGlyphCache,
) {
    let theme = params.theme;
    let is_dir = entry.kind == EntryKind::Directory;
    let selected = params
        .entry_hint
        .is_some_and(|h| h.block_id == params.block_id && h.name == entry.name);
    let hovered = params.hovered_entry == Some(entry_index);

    let pill = RectF::new(
        vec2f(origin.x() + 8.0, origin.y() + 4.0),
        vec2f(content_w - 16.0, row_h - 8.0),
    );
    if selected {
        paint_rounded_rect(ctx, pill, theme.link_surface(), None);
    } else if hovered {
        paint_rounded_rect(ctx, pill, theme.fg_overlay_1(), Some(theme.outline()));
    }

    let (glyph, glyph_color) = file_glyph(&entry.name, is_dir, theme);
    let dim = entry.name.starts_with('.');
    let base_color = if dim {
        theme.disabled_text()
    } else if is_dir {
        theme.ansi_blue()
    } else {
        theme.main_text()
    };
    let text_color = if hovered { theme.link() } else { base_color };

    paint_text(
        ctx,
        app,
        glyph_cache,
        params.mono,
        vec2f(origin.x() + 12.0, origin.y() + 4.0),
        glyph,
        10.0,
        glyph_color,
        Weight::Normal,
    );

    let name = if is_dir {
        format!("{}/", entry.name)
    } else {
        entry.name.clone()
    };
    // Stop before the size/date columns at content_w - 120.
    let name_max_px = (content_w - 120.0 - 32.0 - 8.0).max(40.0);
    let name = truncate_to_width(&name, name_max_px, 11.5);
    paint_text(
        ctx,
        app,
        glyph_cache,
        params.mono,
        vec2f(origin.x() + 32.0, origin.y() + 3.0),
        &name,
        11.5,
        text_color,
        if is_dir {
            Weight::Semibold
        } else {
            Weight::Normal
        },
    );
    if hovered {
        paint_hover_underline(
            ctx,
            vec2f(origin.x() + 32.0, origin.y() + 3.0),
            &name,
            11.5,
            text_color,
        );
    }

    if let Some(sz) = entry.size {
        let label = fmt_size(sz);
        paint_text(
            ctx,
            app,
            glyph_cache,
            params.mono,
            vec2f(origin.x() + content_w - 120.0, origin.y() + 4.0),
            &label,
            10.0,
            theme.disabled_text(),
            Weight::Normal,
        );
    }
    if let Some(ref mdf) = entry.modified {
        paint_text(
            ctx,
            app,
            glyph_cache,
            params.mono,
            vec2f(origin.x() + content_w - 56.0, origin.y() + 4.0),
            mdf,
            10.0,
            theme.disabled_text(),
            Weight::Normal,
        );
    }
}

fn paint_rounded_rect(ctx: &mut PaintContext, rect: RectF, fill: ColorU, border: Option<ColorU>) {
    let radius = CornerRadius::with_all(Radius::Pixels(4.0));
    if fill.a > 0 {
        ctx.scene
            .draw_rect_without_hit_recording(rect)
            .with_background(fill)
            .with_corner_radius(radius);
    }
    if let Some(border_color) = border {
        ctx.scene
            .draw_rect_without_hit_recording(rect)
            .with_border(Border::all(1.0).with_border_color(border_color))
            .with_corner_radius(radius);
    }
}

fn paint_text(
    ctx: &mut PaintContext,
    app: &AppContext,
    glyph_cache: &mut CellGlyphCache,
    mono: FamilyId,
    origin: Vector2F,
    text: &str,
    font_size: f32,
    color: ColorU,
    weight: Weight,
) {
    let font_cache = app.font_cache();
    let mut props = Properties::default();
    props.weight = weight;
    let font_id = font_cache.select_font(mono, props);
    let ascent = font_cache.ascent(font_id, font_size);
    let mut x = origin.x();
    for ch in text.chars() {
        if let Some((glyph_id, font_id_resolved)) = glyph_cache.get(font_cache, font_id, ch) {
            ctx.scene.draw_glyph(
                vec2f(x, origin.y() + ascent),
                glyph_id,
                font_id_resolved,
                font_size,
                color,
            );
        }
        x += font_size * 0.58;
    }
}

pub fn file_glyph(name: &str, is_dir: bool, theme: &Theme) -> (&'static str, ColorU) {
    if is_dir {
        ("▸", theme.ansi_blue())
    } else {
        let name_lower = name.to_lowercase();
        if name_lower.ends_with(".rs") {
            ("⬡", ColorU::new(239, 107, 48, 255))
        } else if name_lower.ends_with(".toml")
            || name_lower.ends_with(".json")
            || name_lower.ends_with(".yaml")
            || name_lower.ends_with(".yml")
            || name_lower.ends_with(".lock")
        {
            ("⚙", theme.sub_text())
        } else if name_lower.ends_with(".md") {
            ("✎", ColorU::new(178, 102, 255, 255))
        } else if name_lower.ends_with(".png")
            || name_lower.ends_with(".jpg")
            || name_lower.ends_with(".jpeg")
            || name_lower.ends_with(".gif")
            || name_lower.ends_with(".webp")
        {
            ("⌼", ColorU::new(0, 204, 204, 255))
        } else {
            ("·", theme.disabled_text())
        }
    }
}

pub fn fmt_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::dir_listing::EntryKind;

    fn sample_entry(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            kind: EntryKind::File,
            color: ColorU::new(255, 255, 255, 255),
            size: None,
            modified: None,
        }
    }

    #[test]
    fn grid_chips_size_to_content() {
        let short = grid_chip_width(&sample_entry("a"));
        let long = grid_chip_width(&sample_entry("abcdefghij"));
        assert!(long > short);
        assert!((long - short) > 30.0);
    }

    #[test]
    fn grid_uses_fixed_column_count() {
        let entries: Vec<_> = (0..6)
            .map(|i| sample_entry(&format!("file{i}.txt")))
            .collect();
        let layout = layout_dir_listing(&entries, DirView::Grid, 480.0);
        let column_count = dir_grid_column_count(480.0);
        assert_eq!(column_count, 3);

        let first_row: Vec<_> = layout
            .entry_regions
            .iter()
            .filter(|r| r.name_rect.origin().y() == layout.entry_regions[0].name_rect.origin().y())
            .collect();
        assert_eq!(first_row.len(), column_count);

        let col_width = 480.0 / column_count as f32;
        for (col_idx, region) in first_row.iter().enumerate() {
            assert!((region.name_rect.origin().x() - col_idx as f32 * col_width).abs() < 0.01);
        }
    }

    #[test]
    fn grid_selection_resolves_column_in_same_row() {
        let entries: Vec<_> = (0..4).map(|i| sample_entry(&format!("f{i}.txt"))).collect();
        let layout = layout_dir_listing(&entries, DirView::Grid, 480.0);
        let row_y = layout.entry_regions[0].name_rect.origin().y();
        let col_width = 480.0 / dir_grid_column_count(480.0) as f32;

        let first = layout
            .hit_test_selection(vec2f(col_width * 0.5, row_y + 4.0), &entries, DirView::Grid)
            .expect("first column");
        let second = layout
            .hit_test_selection(vec2f(col_width * 1.5, row_y + 4.0), &entries, DirView::Grid)
            .expect("second column");

        assert_eq!(first.0, 0);
        assert_eq!(second.0, 1);
    }
}
