//! Terminal git diff bar.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    BranchDiffOp, TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT,
    CMD_HIST_CHROME_PX, CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y,
    SCROLL_BOTTOM_EPSILON_PX,
};
use crate::agent::input_classifier::{
    heuristic::HeuristicInputClassifier, ClassifyContext, InputClassifier,
};
use crate::agent::{DetectedInputType, InputConfig, TabSessionKind};
#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use crate::console::ai_toolbar::{self, AiPanelActions, AiTool};
use crate::console::block_filter::{block_output_lines, count_block_matches};
use crate::console::command_intel;
use crate::console::dir_listing::{resolve_entry_path, DirSort, DirView};
use crate::console::editor_settings::EditorSettings;
use crate::console::escape_parse::styled_to_colored;
use crate::console::fs_suggest::should_suggest_paths;
use crate::console::history_search::HistorySearchState;
use crate::console::input_bar;
use crate::console::input_editing;
use crate::console::input_overlays;
use crate::console::path_picker::PathPicker;
use crate::console::prefixed_history;
use crate::console::shell_aliases::ShellAliases;
use crate::console::suggest_engine::SuggestEngine;
use crate::console::workflow_search::WorkflowSearchState;
use crate::console::workflows::{Workflow, WorkflowSession, WorkflowStore};
use crate::icons::ICON_SIZE_SM;
use crate::process_ext::NoWindow;
use crate::setup::UiSettings;
use crate::terminal::input::{
    encode_keystroke, encode_paste, map_pointer_to_cell, modifiers_from_ui, mouse_event,
    scroll_arrow_bytes, should_encode_wheel_as_mouse,
    should_forward_key_to_pty_while_ai_panel_open, should_passthrough_mouse, wheel_button,
    RichInputKeyContext,
};
use crate::terminal::manager::TerminalManager;
use crate::terminal::model::block_height_tree::BlockHeightIndex;
use crate::terminal::model::{
    ShellLinkStatus, SizeInfo, SizeReason, SizeUpdate, TerminalEvent, TerminalModel,
};
use crate::terminal::render::{
    build_alt_screen_ui, build_block_options_menu, build_bootstrap_grid_ui,
    build_sticky_block_header, build_visible_block_list_column, dir_grid_content_width,
    extract_prompt_tail, sample_ai_chrome, BlockListScrollAdapter, BlockRenderContext,
    CellGlyphCache, CellMetrics, DiagramCanvasState, StickyBlockHeader, TerminalSizeElement,
    BLOCK_HEADER_HEIGHT, BLOCK_LIST_BOTTOM_POSITION_ID,
};
use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};
use crate::terminal::viewport::{
    is_block_list_virtualization_enabled, ScrollMode, ScrollPosition, ViewportState,
};
use crate::terminal::TerminalEventListener;
use crate::theme::{Theme, INPUT_FONT_SIZE, TERMINAL_PADDING_LEFT};
use crate::workspace::persistence::PersistedBlock;
use crate::workspace::{InputEditor, InputEditorEvent, InputMode, DEFAULT_SESSION_TITLE};
use async_channel::Sender;
use dragent_pty::{resolve_shell, PtyEvent, SessionPlatform};
use dragent_resource_policy::{ActivityLevel, MemoryPressure, ResourceBudget};
use dragent_shell_hooks::ShellKind;
use dragent_terminal::{MouseButton, MouseEventKind, TermMode};
use parking_lot::{FairMutex, RwLock};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, Dismiss, DispatchEventResult, Element, EventHandler, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, SelectableArea,
    SelectionHandle, SelectionUpdateArgs, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::Vector2F;
use warpui::keymap::Keystroke;
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::{IntoPixels, Pixels};
use warpui::{
    r#async::Timer, AppContext, Entity, EntityId, FocusContext, GetSingletonModelHandle,
    ModelHandle, ReadModel, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{LiveElement, ScrollTarget, ScrollToPositionMode};
use warpui_core::event::{KeyState, ModifiersState};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::scene::Border;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::WindowId;
use web_time::Instant;

impl TerminalView {
    pub(crate) fn render_git_diff_bar(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        // Mutual exclusion: while the git panel (commit/stage/discard) is open,
        // the Agent Changes card yields. It reappears when the panel closes —
        // this is render-time state, so no re-open bookkeeping is needed.
        if self.branch_diff_expanded {
            return None;
        }
        // Closed via its header; new agent edits clear this (see subscription).
        if self.git_diff_dismissed {
            return None;
        }
        // Only show for files the agent actually wrote/edited in this session.
        let agent_paths = self
            .agent_controller
            .read(app, |c, _| c.agent_modified_files.clone());
        if agent_paths.is_empty() {
            return None;
        }

        let git_status = self.git_repo_status.read(app, |m, _| m.status.clone())?;

        // Filter git diff to only agent-touched files.
        // Match by the filename tail of each agent path against diff_files.
        let diff_files: Vec<_> = git_status
            .diff_files
            .iter()
            .filter(|f| {
                agent_paths.iter().any(|ap| {
                    // ap is relative path like "app/src/agent/controller/mod.rs"
                    // f.filename is "mod.rs", f.dir_short is "…/agent/controller"
                    // Match if the agent path ends with filename (fast path)
                    ap.ends_with(&f.filename) || ap.contains(&f.filename)
                })
            })
            .cloned()
            .collect();

        if diff_files.is_empty() {
            return None;
        }

        let added: u32 = diff_files.iter().map(|f| f.added).sum();
        let deleted: u32 = diff_files.iter().map(|f| f.deleted).sum();
        let n_files = diff_files.len() as u32;

        let theme = self.theme;
        let mono = self.mono_family;

        // The header is a close trigger (clicking anywhere on it dismisses the
        // card), so it carries an × instead of an expand chevron.
        let close_icon =
            crate::icons::render_icon(crate::icons::AppIcon::XClose, theme.sub_text(), 12.0);

        let header_label = format!(
            "{n_files} {} changed",
            if n_files == 1 { "file" } else { "files" }
        );

        let header_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(
                Text::new(header_label, mono, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Text::new(format!("+{added}"), mono, crate::theme::text_size(12.5))
                    .with_color(theme.ansi_green())
                    .with_style(warpui::fonts::Properties {
                        weight: warpui::fonts::Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_child(
                Text::new(format!("-{deleted}"), mono, crate::theme::text_size(12.5))
                    .with_color(theme.ui_red())
                    .with_style(warpui::fonts::Properties {
                        weight: warpui::fonts::Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_child(Box::new(warpui::elements::Expanded::new(
                1.0,
                warpui::elements::Empty::new().finish(),
            )))
            .with_child(close_icon)
            .finish();

        let header_clickable = EventHandler::new(
            Container::new(header_row)
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .finish(),
        )
        .on_left_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::ToggleGitDiff);
            DispatchEventResult::StopPropagation
        })
        .finish();

        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header_clickable);

        let can_revert = self
            .agent_controller
            .read(app, |c, _| c.has_turn_checkpoint());

        {
            let review_btn = EventHandler::new(
                Container::new(
                    Text::new("Review".to_string(), mono, crate::theme::text_size(12.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_border(
                    warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                )
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ))
                .with_background_color(theme.surface_2())
                .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::ReviewAgentChanges);
                DispatchEventResult::StopPropagation
            })
            .finish();

            // Restore tracked files to the pre-turn checkpoint (G3).
            let revert_btn: Option<Box<dyn Element>> = if can_revert {
                Some(
                    EventHandler::new(
                        Container::new(
                            Text::new(
                                "⟲ Revert agent edits".to_string(),
                                mono,
                                crate::theme::text_size(12.0),
                            )
                            .with_color(theme.ui_red())
                            .finish(),
                        )
                        .with_padding_left(8.0)
                        .with_padding_right(8.0)
                        .with_padding_top(4.0)
                        .with_padding_bottom(4.0)
                        .with_border(
                            warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                        )
                        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                            warpui_core::scene::Radius::Pixels(4.0),
                        ))
                        .finish(),
                    )
                    .on_left_mouse_down(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(TerminalAction::RevertLastAgentTurn);
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
                )
            } else {
                None
            };

            for file in &diff_files {
                let fname = file.filename.clone();
                let dir = file.dir_short.clone();
                let fa = file.added;
                let fd = file.deleted;

                let ext = std::path::Path::new(&fname)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                let icon_color = match ext {
                    "rs" => warpui_core::color::ColorU {
                        r: 222,
                        g: 110,
                        b: 38,
                        a: 255,
                    },
                    "ts" | "tsx" => warpui_core::color::ColorU {
                        r: 49,
                        g: 120,
                        b: 198,
                        a: 255,
                    },
                    "js" | "jsx" => warpui_core::color::ColorU {
                        r: 247,
                        g: 223,
                        b: 30,
                        a: 255,
                    },
                    "py" => warpui_core::color::ColorU {
                        r: 55,
                        g: 118,
                        b: 171,
                        a: 255,
                    },
                    _ => theme.sub_text(),
                };
                let icon_label = match ext {
                    "rs" => "R",
                    "ts" | "tsx" => "T",
                    "js" | "jsx" => "J",
                    "py" => "P",
                    "go" => "G",
                    _ => "·",
                };
                let icon_el = Container::new(
                    Text::new(icon_label.to_string(), mono, crate::theme::text_size(10.0))
                        .with_color(warpui_core::color::ColorU {
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                        })
                        .finish(),
                )
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .with_background_color(icon_color)
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(3.0),
                ))
                .finish();

                let file_row = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(icon_el)
                    .with_child(
                        Text::new(fname, mono, crate::theme::text_size(12.5))
                            .with_color(theme.main_text())
                            .with_style(warpui::fonts::Properties {
                                weight: warpui::fonts::Weight::Medium,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .with_child(
                        Text::new(dir, mono, crate::theme::text_size(11.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .with_child(Box::new(warpui::elements::Expanded::new(
                        1.0,
                        warpui::elements::Empty::new().finish(),
                    )))
                    .with_child(
                        Text::new(format!("+{fa}"), mono, crate::theme::text_size(12.0))
                            .with_color(theme.ansi_green())
                            .with_style(warpui::fonts::Properties {
                                weight: warpui::fonts::Weight::Medium,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .with_child(
                        Text::new(format!("-{fd}"), mono, crate::theme::text_size(12.0))
                            .with_color(theme.ui_red())
                            .with_style(warpui::fonts::Properties {
                                weight: warpui::fonts::Weight::Medium,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .finish();

                col = col.with_child(
                    Container::new(file_row)
                        .with_padding_left(12.0)
                        .with_padding_right(12.0)
                        .with_padding_top(6.0)
                        .with_padding_bottom(6.0)
                        .with_border(
                            warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                        )
                        .finish(),
                );
            }

            // "Accept All" — the primary action: keep every agent edit and
            // retire the review card.
            let accept_all_btn = EventHandler::new(
                Container::new(
                    Text::new("Accept All".to_string(), mono, crate::theme::text_size(12.0))
                        .with_color(theme.background)
                        .with_style(warpui::fonts::Properties {
                            weight: warpui::fonts::Weight::Semibold,
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(theme.accent)
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ))
                .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::AcceptAllAgentChanges);
                DispatchEventResult::StopPropagation
            })
            .finish();

            col = col.with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(Box::new(warpui::elements::Expanded::new(
                            1.0,
                            warpui::elements::Empty::new().finish(),
                        )))
                        .with_child(review_btn)
                        .with_children(revert_btn.into_iter().collect::<Vec<_>>())
                        .with_child(accept_all_btn)
                        .finish(),
                )
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_border(
                    warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                )
                .finish(),
            );
        }

        // Card language matches the composer: same 10px gutter, radius 12,
        // hairline border — it reads as a sibling sitting directly above the
        // input box.
        let card = Container::new(col.finish())
            .with_background_color(theme.surface_1())
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(12.0),
            ))
            .with_margin_left(10.0)
            .with_margin_right(10.0)
            .with_margin_bottom(4.0)
            .finish();

        Some(card)
    }

    /// Shared visual style for the small hover tooltips in the branch diff
    /// panel (stage/discard/close icon buttons) — mirrors
    /// `BlockRenderContext`'s `tooltip_styles` in block_output.rs.
    pub(crate) fn branch_diff_tooltip_styles(&self) -> UiComponentStyles {
        UiComponentStyles {
            font_family_id: Some(self.mono_family),
            font_size: Some(crate::theme::text_size(11.0)),
            font_color: Some(self.theme.main_text()),
            background: Some(Fill::Solid(self.theme.surface_2())),
            border_color: Some(Fill::Solid(self.theme.outline())),
            border_width: Some(1.0),
            border_radius: Some(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(6.0),
            )),
            padding: Some(Coords {
                top: 5.0,
                bottom: 5.0,
                left: 8.0,
                right: 8.0,
            }),
            ..Default::default()
        }
    }

    /// Overlay a small text tooltip below `child` while `hovered` is true —
    /// same `Stack` + `OffsetPositioning` recipe `Button::with_tooltip` and
    /// `BlockOutput::with_tooltip` both use, just callable directly on plain
    /// `Hoverable`-wrapped elements instead of requiring their own types.
    pub(crate) fn with_branch_diff_tooltip(
        &self,
        child: Box<dyn Element>,
        hovered: bool,
        label: &str,
    ) -> Box<dyn Element> {
        if !hovered {
            return child;
        }
        let tooltip = Tooltip::new(label.to_string(), self.branch_diff_tooltip_styles())
            .build()
            .finish();
        Stack::new()
            .with_child(child)
            .with_positioned_overlay_child(
                tooltip,
                OffsetPositioning::offset_from_parent(
                    Vector2F::new(0.0, 6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomMiddle,
                    ChildAnchor::TopMiddle,
                ),
            )
            .finish()
    }

    /// Panel opened by clicking the +N/-N chip in the input bar.
    /// Shows ALL uncommitted changes on the branch with staging, commit, and push controls.
    pub(crate) fn render_branch_diff_panel(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        if !self.branch_diff_expanded {
            return None;
        }

        let git_status = self.git_repo_status.read(app, |m, _| m.status.clone())?;
        let diff_files = git_status.diff_files.clone();
        if diff_files.is_empty() {
            return None;
        }

        let added: u32 = diff_files.iter().map(|f| f.added).sum();
        let deleted: u32 = diff_files.iter().map(|f| f.deleted).sum();
        let n_files = diff_files.len() as u32;
        let n_staged = self.branch_diff_staged.len();
        // "Everything here is staged" = every file in THIS list is staged, tested
        // by containment — the same test each row uses for its own +/− button.
        //
        // Comparing counts (`n_staged == diff_files.len()`) looked equivalent but
        // is not: the two come from different sources. `branch_diff_staged` is
        // re-synced from the git index, which can legitimately contain paths that
        // are no longer in `diff_files` (a file staged and then reverted in the
        // working tree, or staged from outside the app), while `diff_files` lists
        // the working-tree changes. Any drift made the counts differ, and the
        // header flipped back to "Stage all" even though every row still showed
        // as staged — visible the next time anything triggered a repaint, such as
        // pressing Generate.
        let all_staged = !diff_files.is_empty()
            && diff_files
                .iter()
                .all(|f| self.branch_diff_staged.contains(&f.rel_path));

        let theme = self.theme;
        let mono = self.mono_family;

        // ── Header row ──────────────────────────────────────────────────
        let (stage_all_label, stage_all_tooltip, stage_all_color) = if all_staged {
            ("−", "Unstage all", theme.sub_text())
        } else {
            ("+", "Stage all", theme.ansi_green())
        };
        let stage_all_mouse_state = self.block_render_context(app).mouse_state("stage_all_btn");
        let stage_all_hovered = stage_all_mouse_state.lock().unwrap().is_hovered();
        let stage_all_btn = self.with_branch_diff_tooltip(
            Hoverable::new(stage_all_mouse_state, move |state| {
                let hovered = state.is_hovered();
                Container::new(
                    Text::new(
                        stage_all_label.to_string(),
                        mono,
                        crate::theme::text_size(16.0),
                    )
                    .with_color(stage_all_color)
                    .finish(),
                )
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .with_background_color(if hovered {
                    theme.surface_3()
                } else {
                    warpui_core::color::ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |event_ctx, _app, _pos| {
                if all_staged {
                    event_ctx.dispatch_typed_action(TerminalAction::UnstageAllBranchFiles);
                } else {
                    event_ctx.dispatch_typed_action(TerminalAction::StageAllBranchFiles);
                }
            })
            .finish(),
            stage_all_hovered,
            stage_all_tooltip,
        );

        let discard_all_mouse_state = self
            .block_render_context(app)
            .mouse_state("discard_all_btn");
        let discard_all_hovered = discard_all_mouse_state.lock().unwrap().is_hovered();
        let discard_all_btn = self.with_branch_diff_tooltip(
            Hoverable::new(discard_all_mouse_state, move |state| {
                let hovered = state.is_hovered();
                Container::new(crate::icons::render_icon(
                    crate::icons::AppIcon::Undo2,
                    theme.ui_red(),
                    16.0,
                ))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if hovered {
                    theme.surface_3()
                } else {
                    warpui_core::color::ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::DiscardAllBranchChanges);
            })
            .finish(),
            discard_all_hovered,
            "Discard all",
        );

        let close_mouse_state = self
            .block_render_context(app)
            .mouse_state("branch_diff_close_btn");
        let close_hovered = close_mouse_state.lock().unwrap().is_hovered();
        let close_btn = self.with_branch_diff_tooltip(
            Hoverable::new(close_mouse_state, move |state| {
                let hovered = state.is_hovered();
                Container::new(crate::icons::render_icon(
                    crate::icons::AppIcon::XClose,
                    if hovered {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    },
                    12.0,
                ))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(|event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::ToggleBranchDiff);
            })
            .finish(),
            close_hovered,
            "Close",
        );

        let header_row = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(
                    Text::new(
                        format!(
                            "{n_files} {} changed",
                            if n_files == 1 { "file" } else { "files" }
                        ),
                        mono,
                        crate::theme::text_size(12.5),
                    )
                    .with_color(theme.main_text())
                    .with_style(warpui::fonts::Properties {
                        weight: warpui::fonts::Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
                )
                .with_child(
                    Text::new(format!("+{added}"), mono, crate::theme::text_size(11.5))
                        .with_color(theme.ansi_green())
                        .finish(),
                )
                .with_child(
                    Text::new(format!("-{deleted}"), mono, crate::theme::text_size(11.5))
                        .with_color(theme.ui_red())
                        .finish(),
                )
                .with_child(Box::new(Expanded::new(
                    1.0,
                    warpui::elements::Empty::new().finish(),
                )))
                .with_child(discard_all_btn)
                .with_child(stage_all_btn)
                .with_child(close_btn)
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(8.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish();

        // ── Scrollable file list ────────────────────────────────────────
        let mut file_col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        for file in &diff_files {
            let fname = file.filename.clone();
            let dir = file.dir_short.clone();
            let fa = file.added;
            let fd = file.deleted;
            let rel = file.rel_path.clone();
            let is_staged = self.branch_diff_staged.contains(&rel);

            // +/− stage toggle button
            let (btn_label, btn_text_color, btn_border_color, btn_bg) = if is_staged {
                ("−", theme.sub_text(), theme.outline(), theme.surface_2())
            } else {
                (
                    "+",
                    theme.ansi_green(),
                    theme.ansi_green(),
                    theme.surface_1(),
                )
            };
            let rel_btn = rel.clone();
            let stage_mouse_state = self
                .block_render_context(app)
                .mouse_state(&format!("stage_file_btn:{rel}"));
            let stage_hovered = stage_mouse_state.lock().unwrap().is_hovered();
            let stage_tooltip_label = if is_staged { "Unstage" } else { "Stage" };
            let stage_btn = self.with_branch_diff_tooltip(
                Hoverable::new(stage_mouse_state, move |_state| {
                    Container::new(
                        Text::new(btn_label.to_string(), mono, crate::theme::text_size(14.0))
                            .with_color(btn_text_color)
                            .finish(),
                    )
                    .with_padding_left(5.0)
                    .with_padding_right(5.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .with_background_color(btn_bg)
                    .with_border(
                        warpui_core::scene::Border::all(1.0).with_border_color(btn_border_color),
                    )
                    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                        warpui_core::scene::Radius::Pixels(4.0),
                    ))
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::StageBranchFile {
                        rel_path: rel_btn.clone(),
                    });
                })
                .finish(),
                stage_hovered,
                stage_tooltip_label,
            );

            let rel_discard = rel.clone();
            let discard_mouse_state = self
                .block_render_context(app)
                .mouse_state(&format!("discard_file_btn:{rel}"));
            let discard_hovered = discard_mouse_state.lock().unwrap().is_hovered();
            let discard_btn = self.with_branch_diff_tooltip(
                Hoverable::new(discard_mouse_state, move |state| {
                    let hovered = state.is_hovered();
                    Container::new(crate::icons::render_icon(
                        crate::icons::AppIcon::Undo2,
                        if hovered {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        },
                        13.0,
                    ))
                    .with_padding_left(5.0)
                    .with_padding_right(5.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .with_background_color(if hovered {
                        theme.surface_3()
                    } else {
                        theme.surface_2()
                    })
                    .with_border(
                        warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                    )
                    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                        warpui_core::scene::Radius::Pixels(4.0),
                    ))
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::DiscardBranchFile {
                        rel_path: rel_discard.clone(),
                    });
                })
                .finish(),
                discard_hovered,
                "Discard changes",
            );

            let ext = std::path::Path::new(&fname)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let icon_color = match ext {
                "rs" => warpui_core::color::ColorU {
                    r: 222,
                    g: 110,
                    b: 38,
                    a: 255,
                },
                "ts" | "tsx" => warpui_core::color::ColorU {
                    r: 49,
                    g: 120,
                    b: 198,
                    a: 255,
                },
                "js" | "jsx" => warpui_core::color::ColorU {
                    r: 247,
                    g: 223,
                    b: 30,
                    a: 255,
                },
                "py" => warpui_core::color::ColorU {
                    r: 55,
                    g: 118,
                    b: 171,
                    a: 255,
                },
                _ => theme.sub_text(),
            };
            let icon_label = match ext {
                "rs" => "R",
                "ts" | "tsx" => "T",
                "js" | "jsx" => "J",
                "py" => "P",
                "go" => "G",
                _ => "·",
            };
            let icon_el = Container::new(
                Text::new(icon_label.to_string(), mono, crate::theme::text_size(9.0))
                    .with_color(warpui_core::color::ColorU {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 255,
                    })
                    .finish(),
            )
            .with_padding_left(3.0)
            .with_padding_right(3.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .with_background_color(icon_color)
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(3.0),
            ))
            .finish();

            let file_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(icon_el)
                .with_child(
                    Text::new(fname, mono, crate::theme::text_size(12.0))
                        .with_color(theme.main_text())
                        .with_style(warpui::fonts::Properties {
                            weight: warpui::fonts::Weight::Medium,
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_child(
                    Text::new(dir, mono, crate::theme::text_size(11.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(Box::new(Expanded::new(
                    1.0,
                    warpui::elements::Empty::new().finish(),
                )))
                .with_child(
                    Text::new(format!("+{fa}"), mono, crate::theme::text_size(11.5))
                        .with_color(theme.ansi_green())
                        .finish(),
                )
                .with_child(
                    Text::new(format!("-{fd}"), mono, crate::theme::text_size(11.5))
                        .with_color(theme.ui_red())
                        .finish(),
                )
                .with_child(stage_btn)
                .with_child(discard_btn)
                .finish();

            file_col = file_col.with_child(
                Container::new(file_row)
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(8.0)
                    .with_background_color(if is_staged {
                        theme.surface_2()
                    } else {
                        theme.surface_1()
                    })
                    .with_border(
                        warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                    )
                    .finish(),
            );
        }

        let file_scrollable = ClippedScrollable::vertical(
            self.branch_diff_scroll.clone(),
            file_col.finish(),
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish();

        let file_list = ConstrainedBox::new(file_scrollable)
            .with_max_height(160.0)
            .finish();

        // ── Commit message footer ────────────────────────────────────────
        let commit_input = crate::shared_input::shared_input(
            &theme,
            &self.branch_diff_commit_editor,
            crate::shared_input::SharedInputOpts {
                ..Default::default()
            },
        );

        let is_generating = self.branch_diff_op == BranchDiffOp::Generating;
        let generate_btn: Box<dyn Element> = if is_generating {
            let millis = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            // See the note on FRAMES above — braille is tofu on the web build.
            let frames = ["◐", "◓", "◑", "◒"];
            let spinner = frames[(millis / 100) as usize % frames.len()];
            let inner = Container::new(
                Text::new(
                    format!("{spinner} Generating…"),
                    mono,
                    crate::theme::text_size(11.5),
                )
                .with_color(theme.disabled_text())
                .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(5.0),
            ))
            .finish();
            Box::new(LiveElement::new(inner, Duration::from_millis(100)))
        } else {
            EventHandler::new(
                Container::new(
                    Text::new("Generate".to_string(), mono, crate::theme::text_size(11.5))
                        .with_color(theme.accent)
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.accent))
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(5.0),
                ))
                .finish(),
            )
            .on_left_mouse_down(|event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::GenerateBranchCommitMsg);
                DispatchEventResult::StopPropagation
            })
            .finish()
        };

        let msg_row = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(Box::new(Expanded::new(1.0, commit_input)))
                .with_child(generate_btn)
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(8.0)
        .with_padding_bottom(6.0)
        .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
        .finish();

        // ── Action row (commit + push) ────────────────────────────────────
        let is_committing = self.branch_diff_op == BranchDiffOp::Committing;
        let commit_active = n_staged > 0 && !self.branch_diff_op.is_busy();
        let commit_label = if is_committing {
            "Committing…".to_string()
        } else if n_staged > 0 {
            format!(
                "Commit {n_staged} {}",
                if n_staged == 1 { "file" } else { "files" }
            )
        } else {
            "Commit".to_string()
        };
        let commit_color = if commit_active {
            theme.main_text()
        } else {
            theme.disabled_text()
        };

        let commit_btn = EventHandler::new(
            Container::new(
                Text::new(commit_label, mono, crate::theme::text_size(11.5))
                    .with_color(commit_color)
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(5.0),
            ))
            .with_background_color(if commit_active {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .finish(),
        )
        .on_left_mouse_down(move |event_ctx, _app, _pos| {
            if commit_active {
                event_ctx.dispatch_typed_action(TerminalAction::CommitBranchChanges);
            }
            DispatchEventResult::StopPropagation
        })
        .finish();

        let push_btn = EventHandler::new(
            Container::new(
                Text::new(
                    "↑ Commit & Push".to_string(),
                    mono,
                    crate::theme::text_size(11.5),
                )
                .with_color(if self.branch_diff_op.is_busy() {
                    theme.disabled_text()
                } else {
                    theme.ansi_blue()
                })
                .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(5.0),
            ))
            .finish(),
        )
        .on_left_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::CommitAndPushBranch);
            DispatchEventResult::StopPropagation
        })
        .finish();

        let staged_label = if n_staged > 0 {
            format!("{n_staged} staged")
        } else {
            "None staged".to_string()
        };

        let action_row = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(
                    Text::new(staged_label, mono, crate::theme::text_size(11.0))
                        .with_color(if n_staged > 0 {
                            theme.ansi_green()
                        } else {
                            theme.disabled_text()
                        })
                        .finish(),
                )
                .with_child(Box::new(Expanded::new(
                    1.0,
                    warpui::elements::Empty::new().finish(),
                )))
                .with_child(commit_btn)
                .with_child(push_btn)
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(7.0)
        .with_padding_bottom(8.0)
        .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
        .finish();

        // ── Assemble card ────────────────────────────────────────────────
        let card_content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header_row)
            .with_child(file_list)
            .with_child(msg_row)
            .with_child(action_row)
            .finish();

        let card = Container::new(card_content)
            .with_background_color(theme.surface_1())
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(8.0),
            ))
            .with_margin_left(12.0)
            .with_margin_right(12.0)
            .with_margin_bottom(6.0)
            .finish();

        Some(card)
    }
}
