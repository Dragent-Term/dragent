//! Scrollable block list for normal (non-alt) terminal mode.

use async_channel::Sender;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::terminal::render::block_grid_painter::paint_grid_line;
use crate::terminal::render::block_list_element::BlockListElement;
use crate::terminal::render::LivePromptSection;
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

pub fn build_visible_block_list_column(
    visible: &[VisibleEntry],
    blocks: &[Block],
    agent_blocks: &[crate::agent::controller::block::AgentBlock],
    bootstrap_lines: &[ColoredLine],
    live_prompt: Option<LivePromptSection>,
    ctx: &BlockRenderContext<'_>,
    height_index: BlockHeightIndex,
    font_cache: &warpui::fonts::Cache,
    viewport_height: f32,
    first_entry_offset_px: f32,
    measured_sink: Arc<Mutex<HashMap<usize, f32>>>,
    measured_agent_sink: Arc<Mutex<HashMap<usize, f32>>>,
    heights_tx: Sender<()>,
    glyph_cache: Arc<Mutex<CellGlyphCache>>,
    text_cache: Arc<Mutex<crate::terminal::render::block_list_element::BlockTextCache>>,
    heal_states: &std::collections::HashMap<usize, crate::agent::self_heal::BlockHealState>,
    block_filter: Option<(usize, &BlockFilterState)>,
    clear_gap_trigger_block_id: Option<usize>,
    live_checkpoint_ids: &std::collections::HashSet<usize>,
) -> Box<dyn Element> {
    let cell_metrics = CellMetrics::for_terminal(font_cache, ctx.mono);
    Box::new(BlockListElement::new(
        visible,
        blocks,
        agent_blocks,
        bootstrap_lines,
        live_prompt,
        ctx.theme,
        ctx.mono,
        cell_metrics,
        height_index,
        ctx,
        viewport_height,
        first_entry_offset_px,
        measured_sink,
        measured_agent_sink,
        heights_tx,
        glyph_cache,
        text_cache,
        heal_states,
        block_filter,
        clear_gap_trigger_block_id,
        live_checkpoint_ids,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockListScrollAdapter
// ─────────────────────────────────────────────────────────────────────────────

/// `ScrollableElement` adapter that clips the visible-block child to the
/// viewport and handles programmatic scroll position updates.
pub struct BlockListScrollAdapter {
    child: Box<dyn Element>,
    scroll_pos: Arc<Mutex<ScrollPosition>>,
    total_height_px: f32,
    partial_first_entry_px: f32,
    handle_wheel: bool,
    viewport_size: Option<Vector2F>,
    origin: Option<Point>,
}

impl BlockListScrollAdapter {
    pub fn new(
        child: Box<dyn Element>,
        scroll_pos: Arc<Mutex<ScrollPosition>>,
        total_height_px: f32,
        partial_first_entry_px: f32,
        handle_wheel: bool,
    ) -> Self {
        Self {
            child,
            scroll_pos,
            total_height_px,
            partial_first_entry_px,
            handle_wheel,
            viewport_size: None,
            origin: None,
        }
    }
}

impl Element for BlockListScrollAdapter {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = constraint.max.max(Vector2F::zero());
        let child_constraint = SizeConstraint {
            min: vec2f(size.x(), 0.0),
            max: vec2f(size.x(), f32::INFINITY),
        };
        self.child.layout(child_constraint, ctx, app);
        self.viewport_size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.viewport_size.unwrap_or_default();
        ctx.scene
            .start_layer(ClipBounds::BoundedBy(RectF::new(origin, size)));
        let child_origin = vec2f(origin.x(), origin.y() - self.partial_first_entry_px);
        self.child.paint(child_origin, ctx, app);
        ctx.scene.stop_layer();
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.viewport_size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

impl ScrollableElement for BlockListScrollAdapter {
    fn scroll_data(&self, _app: &AppContext) -> Option<ScrollData> {
        let viewport_h = self.viewport_size?.y();
        let scroll_top = self
            .scroll_pos
            .lock()
            .unwrap()
            .scroll_top_px(self.total_height_px, viewport_h);
        Some(ScrollData {
            scroll_start: Pixels::new(scroll_top),
            visible_px: Pixels::new(viewport_h),
            total_size: Pixels::new(self.total_height_px),
        })
    }

    fn scroll(&mut self, delta: Pixels, ctx: &mut EventContext) {
        let viewport_h = self.viewport_size.map_or(1.0, |s| s.y());
        let new_pos = self.scroll_pos.lock().unwrap().scroll_by(
            -delta.as_f32(),
            self.total_height_px,
            viewport_h,
        );
        *self.scroll_pos.lock().unwrap() = new_pos;
        ctx.notify();
    }

    fn should_handle_scroll_wheel(&self) -> bool {
        self.handle_wheel
    }
}

pub(crate) fn agent_block_selection_lines(
    kind: &crate::agent::controller::block::AgentBlockKind,
    theme: &Theme,
) -> Vec<ColoredLine> {
    let mut lines = Vec::new();
    match kind {
        crate::agent::controller::block::AgentBlockKind::UserQuery { text, .. }
        | crate::agent::controller::block::AgentBlockKind::AgentMessage { text, .. }
        | crate::agent::controller::block::AgentBlockKind::SystemNote { text } => {
            for line in text.lines() {
                lines.push(ColoredLine {
                    wrapped: false,
                    spans: vec![crate::console::escape_parse::TextSpan {
                        text: line.to_string(),
                        color: Some(theme.main_text()),
                        bg: None,
                        flags: 0,
                    }],
                });
            }
        }
        crate::agent::controller::block::AgentBlockKind::ApprovalRequest {
            command,
            reason,
            ..
        } => {
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: reason.clone(),
                    color: Some(theme.disabled_text()),
                    bg: None,
                    flags: 0,
                }],
            });
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: format!("$ {command}"),
                    color: Some(theme.main_text()),
                    bg: None,
                    flags: 0,
                }],
            });
        }
        crate::agent::controller::block::AgentBlockKind::UserQuestion {
            questions,
            answers,
            ..
        } => {
            for (i, q) in questions.iter().enumerate() {
                lines.push(ColoredLine {
                    wrapped: false,
                    spans: vec![crate::console::escape_parse::TextSpan {
                        text: q.question.clone(),
                        color: Some(theme.main_text()),
                        bg: None,
                        flags: 0,
                    }],
                });
                let picked = answers.get(i).cloned().unwrap_or_default();
                if !picked.is_empty() {
                    lines.push(ColoredLine {
                        wrapped: false,
                        spans: vec![crate::console::escape_parse::TextSpan {
                            text: format!("→ {}", picked.join(", ")),
                            color: Some(theme.sub_text()),
                            bg: None,
                            flags: 0,
                        }],
                    });
                }
            }
        }
        crate::agent::controller::block::AgentBlockKind::ActionResult {
            command,
            exit_code,
            output_preview,
            ..
        } => {
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: format!("$ {command}"),
                    color: Some(theme.disabled_text()),
                    bg: None,
                    flags: 0,
                }],
            });
            let exit_str = match exit_code {
                Some(0) => "exit 0".to_string(),
                Some(c) => format!("exit {c}"),
                None => "done".to_string(),
            };
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: exit_str,
                    color: Some(theme.main_text()),
                    bg: None,
                    flags: 0,
                }],
            });
            for line in output_preview.lines() {
                lines.push(ColoredLine {
                    wrapped: false,
                    spans: vec![crate::console::escape_parse::TextSpan {
                        text: line.to_string(),
                        color: Some(theme.disabled_text()),
                        bg: None,
                        flags: 0,
                    }],
                });
            }
        }
        crate::agent::controller::block::AgentBlockKind::TodoList { todos } => {
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: "# Todos".to_string(),
                    color: Some(theme.main_text()),
                    bg: None,
                    flags: 0,
                }],
            });
            for todo in todos {
                let marker = if todo.status == "completed" {
                    "✓"
                } else if todo.status == "in_progress" {
                    "•"
                } else {
                    " "
                };
                lines.push(ColoredLine {
                    wrapped: false,
                    spans: vec![crate::console::escape_parse::TextSpan {
                        text: format!("[{marker}] {}", todo.content),
                        color: Some(theme.sub_text()),
                        bg: None,
                        flags: 0,
                    }],
                });
            }
        }
        crate::agent::controller::block::AgentBlockKind::ToolStep {
            name,
            args_summary,
            result_summary,
            ..
        } => {
            let detail = result_summary
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(args_summary.as_str());
            let text = if detail.is_empty() {
                name.clone()
            } else {
                format!("{name}: {detail}")
            };
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text,
                    color: Some(theme.sub_text()),
                    bg: None,
                    flags: 0,
                }],
            });
        }
        crate::agent::controller::block::AgentBlockKind::ThinkingStep { content, .. } => {
            let truncated = content.chars().take(120).collect::<String>();
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: format!("🧠 {truncated}"),
                    color: Some(theme.disabled_text()),
                    bg: None,
                    flags: 0,
                }],
            });
        }
        crate::agent::controller::block::AgentBlockKind::PlanReview { content, .. } => {
            for line in content.lines().take(10) {
                lines.push(ColoredLine {
                    wrapped: false,
                    spans: vec![crate::console::escape_parse::TextSpan {
                        text: line.to_string(),
                        color: Some(theme.main_text()),
                        bg: None,
                        flags: 0,
                    }],
                });
            }
        }
        crate::agent::controller::block::AgentBlockKind::QueuedMessage { text, .. } => {
            lines.push(ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: format!("[queued] {text}"),
                    color: Some(theme.sub_text()),
                    bg: None,
                    flags: 0,
                }],
            });
        }
        crate::agent::controller::block::AgentBlockKind::Loading { .. } => {}
        crate::agent::controller::block::AgentBlockKind::Error { message, .. } => {
            for line in message.lines() {
                lines.push(ColoredLine {
                    wrapped: false,
                    spans: vec![crate::console::escape_parse::TextSpan {
                        text: line.to_string(),
                        color: Some(theme.ui_red()),
                        bg: None,
                        flags: 0,
                    }],
                });
            }
        }
        crate::agent::controller::block::AgentBlockKind::Empty => {}
        crate::agent::controller::block::AgentBlockKind::ActivityGroup { blocks, expanded } => {
            if *expanded {
                for child in blocks {
                    lines.extend(agent_block_selection_lines(&child.kind, theme));
                }
            }
        }
        crate::agent::controller::block::AgentBlockKind::TurnGroup {
            children, expanded, ..
        } => {
            if *expanded {
                for child in children {
                    lines.extend(agent_block_selection_lines(&child.kind, theme));
                }
            }
        }
    }
    lines
}
