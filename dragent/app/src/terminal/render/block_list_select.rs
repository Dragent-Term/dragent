//! Scrollable block list for normal (non-alt) terminal mode.

use async_channel::Sender;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::terminal::render::block_grid_painter::paint_grid_line;
use crate::terminal::render::block_list_element::BlockListElement;
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
use crate::console::escape_parse::{styled_to_colored, ColoredLine, TextSpan};
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

impl SelectableElement for BlockListElement {
    fn get_selection(
        &self,
        selection_start: Vector2F,
        selection_end: Vector2F,
        is_rect: IsRect,
    ) -> Option<Vec<SelectionFragment>> {
        let (raw_start_block, raw_start_line, raw_start_col) =
            self.resolve_point(selection_start)?;
        let (raw_end_block, raw_end_line, raw_end_col) = self.resolve_point(selection_end)?;

        let (start_block, start_line, start_col, end_block, end_line, end_col) = {
            let (start, end) = self.order_selection_endpoints(
                (raw_start_block, raw_start_line, raw_start_col),
                (raw_end_block, raw_end_line, raw_end_col),
            );
            (start.0, start.1, start.2, end.0, end.1, end.2)
        };

        let mut selection_text = String::new();
        let mut collecting = false;

        if start_block == usize::MAX {
            // Bootstrap section
            for (idx, line) in self.bootstrap_lines.iter().enumerate() {
                let mut line_text = String::new();
                for span in &line.spans {
                    line_text.push_str(&span.text);
                }

                if idx == start_line {
                    collecting = true;
                    if end_block == usize::MAX && idx == end_line {
                        selection_text
                            .push_str(&Self::selection_slice(&line_text, start_col, end_col));
                        break;
                    } else {
                        selection_text.push_str(&Self::selection_slice(
                            &line_text,
                            start_col,
                            line_terminal_column_count(&line_text),
                        ));
                        selection_text.push('\n');
                    }
                } else if collecting {
                    if end_block == usize::MAX && idx == end_line {
                        selection_text.push_str(&Self::selection_slice(&line_text, 0, end_col));
                        break;
                    } else {
                        selection_text.push_str(&line_text);
                        selection_text.push('\n');
                    }
                }
            }
        } else if start_block == usize::MAX - 1 {
            if let Some(prompt) = &self.live_prompt {
                for (idx, line) in prompt.lines.iter().enumerate() {
                    let colored = styled_to_colored(line, &self.theme);
                    let mut line_text = String::new();
                    for span in &colored.spans {
                        line_text.push_str(&span.text);
                    }

                    if idx == start_line {
                        collecting = true;
                        if end_block == usize::MAX - 1 && idx == end_line {
                            selection_text
                                .push_str(&Self::selection_slice(&line_text, start_col, end_col));
                            break;
                        } else {
                            selection_text.push_str(&Self::selection_slice(
                                &line_text,
                                start_col,
                                line_terminal_column_count(&line_text),
                            ));
                            selection_text.push('\n');
                        }
                    } else if collecting {
                        if end_block == usize::MAX - 1 && idx == end_line {
                            selection_text.push_str(&Self::selection_slice(&line_text, 0, end_col));
                            break;
                        } else {
                            selection_text.push_str(&line_text);
                            selection_text.push('\n');
                        }
                    }
                }
            }
        } else {
            // For same-block agent block selections, bypass the fixed-cell-height coordinate
            // mapping (which is inaccurate for markdown's variable-height rendering) and
            // delegate to the rendered element tree, which uses glyph-accurate positions.
            if start_block == end_block {
                // sel_id = usize::MAX - 1000 - agent_block_id, so agent_block_id = base - sel_id.
                const AGENT_SEL_BASE: usize = usize::MAX - 1000;
                if let Some(candidate_id) = AGENT_SEL_BASE.checked_sub(start_block) {
                    if let Some(el) = self.agent_block_elements.get(&candidate_id) {
                        if let Some(selectable) = el.as_selectable_element() {
                            return selectable.get_selection(
                                selection_start,
                                selection_end,
                                is_rect,
                            );
                        }
                    }
                }
            }

            // Block section — iterate in layout order (shell and agent blocks interleaved)
            let mut ordered_block_ids = Vec::new();
            for item_entry in self.height_index.visible_entries(0.0, f32::INFINITY) {
                match item_entry.entry {
                    BlockListEntry::Block { block_id, .. } => {
                        ordered_block_ids.push(block_id);
                    }
                    BlockListEntry::AgentBlock { agent_block_id, .. } => {
                        ordered_block_ids.push(usize::MAX - 1000 - agent_block_id);
                    }
                    _ => {}
                }
            }

            'outer: for block_id in ordered_block_ids {
                if block_id == start_block {
                    collecting = true;
                }

                if collecting {
                    let Some((expanded, truncated_lines, _)) = self.block_lines.get(&block_id)
                    else {
                        if block_id == end_block {
                            break;
                        }
                        continue;
                    };
                    if !*expanded || truncated_lines.is_empty() {
                        if block_id == end_block {
                            break;
                        }
                        continue;
                    }

                    for (paint_line, line) in truncated_lines.iter().enumerate() {
                        let mut line_text = String::new();
                        for span in &line.spans {
                            line_text.push_str(&span.text);
                        }

                        let is_start = block_id == start_block && paint_line == start_line;
                        let is_end = block_id == end_block && paint_line == end_line;

                        if is_start && is_end {
                            selection_text
                                .push_str(&Self::selection_slice(&line_text, start_col, end_col));
                            break 'outer;
                        } else if is_start {
                            selection_text.push_str(&Self::selection_slice(
                                &line_text,
                                start_col,
                                line_terminal_column_count(&line_text),
                            ));
                            selection_text.push('\n');
                        } else if is_end {
                            selection_text.push_str(&Self::selection_slice(&line_text, 0, end_col));
                            break 'outer;
                        } else if collecting {
                            let is_middle_block = block_id != start_block && block_id != end_block;
                            let is_after_start_in_start_block = block_id == start_block
                                && block_id != end_block
                                && paint_line > start_line;
                            let is_before_end_in_end_block = block_id == end_block
                                && block_id != start_block
                                && paint_line < end_line;
                            let is_between_same_block = block_id == start_block
                                && block_id == end_block
                                && paint_line > start_line
                                && paint_line < end_line;
                            if is_middle_block
                                || is_between_same_block
                                || is_after_start_in_start_block
                                || is_before_end_in_end_block
                            {
                                selection_text.push_str(&line_text);
                                selection_text.push('\n');
                            }
                        }
                    }
                }

                if block_id == end_block {
                    break;
                }
            }
        }

        if selection_text.is_empty() {
            return None;
        }

        Some(vec![SelectionFragment {
            text: selection_text,
            origin: Point::from_vec2f(selection_start, ZIndex::Normal(0)),
        }])
    }

    fn expand_selection(
        &self,
        absolute_point: Vector2F,
        direction: SelectionDirection,
        unit: SelectionType,
        word_boundaries_policy: &WordBoundariesPolicy,
    ) -> Option<Vector2F> {
        if matches!(unit, SelectionType::Simple | SelectionType::Rect) {
            return Some(absolute_point);
        }
        let res = (|| {
            let (block_id, line_idx, col_idx) = self.resolve_point(absolute_point)?;
            let line_str = self.line_text_at_paint_index(block_id, line_idx)?;

            match unit {
                SelectionType::Simple | SelectionType::Rect => Some(absolute_point),
                SelectionType::Semantic => {
                    let col_offset = CharOffset::from(col_idx);
                    let target_offset = if matches!(direction, SelectionDirection::Backward) {
                        line_str
                            .word_starts_backward_from_offset_exclusive(col_offset)
                            .ok()?
                            .with_policy(word_boundaries_policy)
                            .next()?
                    } else {
                        line_str
                            .word_ends_from_offset_exclusive(col_offset)
                            .ok()?
                            .with_policy(word_boundaries_policy)
                            .next()?
                    };
                    let target_col = line_str.to_offset(target_offset).ok()?.as_usize();
                    self.point_for_position(block_id, line_idx, target_col)
                }
                SelectionType::Lines => {
                    let target_col = if matches!(direction, SelectionDirection::Backward) {
                        0
                    } else {
                        line_str.chars().count()
                    };
                    self.point_for_position(block_id, line_idx, target_col)
                }
            }
        })();
        res.or(Some(absolute_point))
    }

    fn is_point_semantically_before(
        &self,
        absolute_point: Vector2F,
        absolute_point_other: Vector2F,
    ) -> Option<bool> {
        self.compare_points_by_paint_position(absolute_point, absolute_point_other)
            .map(|ord| ord == std::cmp::Ordering::Less)
    }

    fn smart_select(
        &self,
        absolute_point: Vector2F,
        smart_select_fn: SmartSelectFn,
    ) -> Option<(Vector2F, Vector2F)> {
        let res = (|| {
            let (block_id, line_idx, col_idx) = self.resolve_point(absolute_point)?;
            let line_str = self.line_text_at_paint_index(block_id, line_idx)?;

            let char_offset = col_idx;
            let byte_offset = line_str
                .char_indices()
                .nth(char_offset)
                .map_or(ByteOffset::from(line_str.len()), |c| ByteOffset::from(c.0));

            let smart_select_range = smart_select_fn(&line_str, byte_offset)?;
            let smart_select_start = line_str[..smart_select_range.start.as_usize()]
                .chars()
                .count();
            let smart_select_end = line_str[..smart_select_range.end.as_usize()]
                .chars()
                .count();

            let start_pt = self.point_for_position(block_id, line_idx, smart_select_start)?;
            let end_pt = self.point_for_position(block_id, line_idx, smart_select_end)?;

            Some((start_pt, end_pt))
        })();
        res.or(Some((absolute_point, absolute_point)))
    }

    fn calculate_clickable_bounds(&self, _current_selection: Option<Selection>) -> Vec<RectF> {
        vec![]
    }
}
