//! Terminal chips and branch menu rendering.
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

/// One choice in the input's pill menu.
pub(crate) struct PillMenuRow {
    pub label: String,
    pub subtitle: String,
    pub selected: bool,
    pub action: TerminalAction,
}

impl TerminalView {
    /// Common tail for switch/create: clear the busy flag, surface git's own
    /// error if it failed, and force a status refresh so the chip updates
    /// immediately rather than waiting on the `.git/HEAD` watcher debounce.
    pub(crate) fn finish_branch_op(
        &mut self,
        rx: async_channel::Receiver<Result<(), String>>,
        ctx: &mut ViewContext<Self>,
    ) {
        let git_status_handle = self.git_repo_status.clone();
        ctx.spawn_stream_local(
            rx,
            move |view, result, vctx| {
                view.branch_diff_op = BranchDiffOp::None;
                if let Err(err) = result {
                    view.show_error_notice(("Branch switch failed".to_string(), err), vctx);
                }
                let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                vctx.update_model(&git_status_handle, |model, mctx| {
                    model.refresh_bypass_throttle(refresh_cwd, mctx);
                });
                vctx.notify();
            },
            |_, _| {},
        );
    }

    /// Tooltip anchored ABOVE its trigger.
    ///
    /// `with_branch_diff_tooltip` puts the tooltip below, which is right for
    /// panel buttons but wrong for the chips row — it sits just above the
    /// input at the window's bottom edge, so a tooltip below would be clipped
    /// or cover the input the chip is describing.
    pub(crate) fn with_chip_tooltip(
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
                    Vector2F::new(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopMiddle,
                    ChildAnchor::BottomMiddle,
                ),
            )
            .finish()
    }

    /// One chip in the status row: bordered pill, hover-lit, tooltipped, and
    /// clickable when an action is supplied. Every chip goes through here so
    /// the row stays visually uniform.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_status_chip(
        &self,
        app: &AppContext,
        key: &str,
        icon: Option<crate::icons::AppIcon>,
        label: String,
        color: warpui::color::ColorU,
        tooltip: &str,
        active: bool,
        theme: &Theme,
        mono: FamilyId,
        action: Option<TerminalAction>,
    ) -> Box<dyn Element> {
        use crate::console::input_bar::{CHIP_FONT, CHIP_PAD_X, CHIP_PAD_Y, CHIP_RADIUS};
        let ms = self.block_render_context(app).mouse_state(key);
        let hovered = ms.lock().unwrap().is_hovered();
        let th = *theme;
        let clickable = action.is_some();
        let pill = Hoverable::new(ms, move |state| {
            let mut inner = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0);
            if let Some(icon) = icon {
                inner = inner.with_child(crate::icons::render_icon(
                    icon,
                    color,
                    crate::icons::ICON_SIZE_MD,
                ));
            }
            if !label.is_empty() {
                inner = inner.with_child(Box::new(
                    Text::new(label.clone(), mono, crate::theme::text_size(CHIP_FONT))
                        .with_color(color),
                ));
            }
            let hot = active || (clickable && state.is_hovered());
            Container::new(inner.finish())
                .with_padding_left(CHIP_PAD_X)
                .with_padding_right(CHIP_PAD_X)
                .with_padding_top(CHIP_PAD_Y)
                .with_padding_bottom(CHIP_PAD_Y)
                .with_background_color(if hot { th.surface_3() } else { th.surface_2() })
                .with_border(Border::all(1.0).with_border_color(if hot {
                    color
                } else {
                    th.outline()
                }))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CHIP_RADIUS)))
                .finish()
        });
        let pill = if clickable {
            pill.with_cursor(Cursor::PointingHand)
        } else {
            pill
        };
        let chip = match action {
            Some(action) => EventHandler::new(pill.finish())
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(action.clone());
                    DispatchEventResult::StopPropagation
                })
                .finish(),
            None => pill.finish(),
        };
        self.with_chip_tooltip(chip, hovered, tooltip)
    }

    /// A chip that opens a popover — same pill, plus the open-state highlight.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_chip_trigger(
        &self,
        app: &AppContext,
        key: &'static str,
        icon: crate::icons::AppIcon,
        label: String,
        color: warpui::color::ColorU,
        open: bool,
        tooltip: &str,
        theme: &Theme,
        mono: FamilyId,
        action: TerminalAction,
    ) -> Box<dyn Element> {
        self.render_status_chip(
            app,
            key,
            Some(icon),
            label,
            color,
            tooltip,
            open,
            theme,
            mono,
            Some(action),
        )
    }

    /// The cwd chip: click to browse into a subdirectory or up to a parent.
    pub(crate) fn render_folder_chip(
        &self,
        app: &AppContext,
        cwd: &std::path::Path,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let open = self.folder_menu_open;
        let trigger = self.render_chip_trigger(
            app,
            "folder-chip",
            crate::icons::AppIcon::Folder,
            crate::console::input_bar::abbrev_path(cwd),
            if open {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            open,
            "Working directory — click to change",
            theme,
            mono,
            TerminalAction::ToggleFolderMenu,
        );
        if !open {
            return trigger;
        }

        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Start);

        // Ancestors first, nearest last, so "go up" is one predictable click.
        // Capped at four so a deep path can't push the real contents offscreen.
        let ancestors: Vec<_> = cwd.ancestors().skip(1).take(4).collect();
        for (i, anc) in ancestors.iter().enumerate().rev() {
            list = list.with_child(self.popover_row(
                app,
                &format!("folder-up-{i}"),
                format!("../{}", anc.file_name().map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| anc.to_string_lossy().into_owned())),
                Some(crate::icons::AppIcon::ArrowUp),
                theme.sub_text(),
                theme,
                mono,
                TerminalAction::ChangeDirectory {
                    path: anc.to_string_lossy().into_owned(),
                },
            ));
        }

        match self.folder_list.as_ref() {
            None => {
                list = list.with_child(
                    Container::new(
                        Text::new("Loading…".to_string(), mono, crate::theme::text_size(11.0))
                            .with_color(theme.disabled_text())
                            .finish(),
                    )
                    .with_uniform_padding(8.0)
                    .finish(),
                );
            }
            Some(dirs) if dirs.is_empty() => {
                list = list.with_child(
                    Container::new(
                        Text::new(
                            "No subdirectories".to_string(),
                            mono,
                            crate::theme::text_size(11.0),
                        )
                        .with_color(theme.disabled_text())
                        .finish(),
                    )
                    .with_uniform_padding(8.0)
                    .finish(),
                );
            }
            Some(dirs) => {
                for (i, name) in dirs.iter().enumerate() {
                    list = list.with_child(self.popover_row(
                        app,
                        &format!("folder-in-{i}"),
                        name.clone(),
                        Some(crate::icons::AppIcon::Folder),
                        theme.main_text(),
                        theme,
                        mono,
                        TerminalAction::ChangeDirectory {
                            path: cwd.join(name).to_string_lossy().into_owned(),
                        },
                    ));
                }
            }
        }

        let scroll = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.folder_scroll.clone(),
                list.finish(),
                ScrollbarWidth::Auto,
                theme.outline().into(),
                theme.sub_text().into(),
                theme.surface_1().into(),
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_max_height(240.0)
        .finish();
        let popover = Self::dismissible_popover(
            Self::popover_shell(scroll, 260.0, theme),
            TerminalAction::CloseFolderMenu,
        );
        Self::anchor_above(trigger, popover)
    }

    /// Branch switcher, rendered as a panel above the composer when the
    /// bottom-bar branch chip is clicked (the git controls moved to the
    /// workspace bottom bar; this panel is their anchor surface).
    pub(crate) fn render_branch_menu_panel(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let Some(git_status) = self.git_repo_status.read(app, |m, _| m.status.clone()) else {
            return warpui::elements::Empty::new().finish();
        };
        let current = git_status.branch.clone();

        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Start);
        match self.branch_list.as_ref() {
            None => {
                list = list.with_child(
                    Container::new(
                        Text::new(
                            "Loading branches…".to_string(),
                            mono,
                            crate::theme::text_size(11.0),
                        )
                        .with_color(theme.disabled_text())
                        .finish(),
                    )
                    .with_uniform_padding(8.0)
                    .finish(),
                );
            }
            Some(branches) => {
                for (i, name) in branches.iter().enumerate() {
                    let is_current = name == &current;
                    list = list.with_child(self.popover_row(
                        app,
                        &format!("branch-{i}"),
                        if is_current {
                            format!("{name}  ✓")
                        } else {
                            name.clone()
                        },
                        Some(crate::icons::AppIcon::GitBranch),
                        if is_current {
                            theme.accent
                        } else {
                            theme.main_text()
                        },
                        theme,
                        mono,
                        TerminalAction::SwitchBranch {
                            name: name.clone(),
                            force: false,
                        },
                    ));
                }
            }
        }

        let scroll = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.branch_scroll.clone(),
                list.finish(),
                ScrollbarWidth::Auto,
                theme.outline().into(),
                theme.sub_text().into(),
                theme.surface_1().into(),
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_max_height(220.0)
        .finish();

        let new_branch_field = crate::shared_input::shared_input(
            theme,
            &self.new_branch_editor,
            crate::shared_input::SharedInputOpts {
                ..Default::default()
            },
        );

        let body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(scroll)
            .with_child(
                Container::new(
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_height(1.0)
                        .finish(),
                )
                .with_background_color(theme.outline())
                .with_margin_top(4.0)
                .with_margin_bottom(4.0)
                .finish(),
            )
            .with_child(new_branch_field)
            .with_child(
                Container::new(
                    Text::new(
                        "Enter to create branch".to_string(),
                        mono,
                        crate::theme::text_size(10.0),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .finish(),
            )
            .finish();

        // Header row so the panel reads as the branch switcher.
        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::SpaceBetween)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(crate::icons::render_icon(
                        crate::icons::AppIcon::GitBranch,
                        theme.accent,
                        12.0,
                    ))
                    .with_child(
                        Text::new(
                            format!("Branches · {current}"),
                            mono,
                            crate::theme::text_size(11.0),
                        )
                        .with_color(theme.main_text())
                        .finish(),
                    )
                    .finish(),
            )
            .with_child(
                Hoverable::new(
                    self.block_render_context(app)
                        .mouse_state("branch-menu-close"),
                    move |state| {
                        Container::new(crate::icons::render_icon(
                            crate::icons::AppIcon::XClose,
                            if state.is_hovered() {
                                theme.main_text()
                            } else {
                                theme.sub_text()
                            },
                            12.0,
                        ))
                        .with_uniform_padding(4.0)
                        .finish()
                    },
                )
                .with_cursor(Cursor::PointingHand)
                .on_click(|event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::CloseBranchMenu);
                })
                .finish(),
            )
            .finish();

        Self::dismissible_popover(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(
                        Container::new(header)
                            .with_padding_left(8.0)
                            .with_padding_right(8.0)
                            .with_padding_top(6.0)
                            .with_padding_bottom(6.0)
                            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                            .finish(),
                    )
                    .with_child(
                        Container::new(body)
                            .with_padding_left(4.0)
                            .with_padding_right(4.0)
                            .with_padding_top(4.0)
                            .with_padding_bottom(4.0)
                            .finish(),
                    )
                    .finish(),
            )
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish(),
            TerminalAction::CloseBranchMenu,
        )
    }

    /// The agent's mode picker (Agent / Plan / Ask…) in the input's controls row.
    pub(crate) fn render_mode_selector(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        use crate::agent::chat_mode::ChatMode;
        let mode = self.chat_mode;
        let rows = ChatMode::ALL
            .iter()
            .map(|m| PillMenuRow {
                label: m.label().to_string(),
                subtitle: m.subtitle().to_string(),
                selected: *m == mode,
                action: TerminalAction::SetChatMode { mode: *m },
            })
            .collect();
        self.render_pill_menu(app, theme, mono, mode.label().to_string(), rows)
    }

    /// The input's soft pill trigger with its menu above it: the mode picker's
    /// look, shared by everything that sits in that place (the agent's modes,
    /// a terminal agent's Terminal / Chat switch). Opens with `ToggleModeMenu`.
    pub(crate) fn render_pill_menu(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
        label: String,
        rows: Vec<PillMenuRow>,
    ) -> Box<dyn Element> {
        let open = self.mode_menu_open;

        // Soft pill: tinted surface, no border chrome; the
        // accent tint deepens when open or hovered.
        let theme_t = *theme;
        let mode_label = label;
        let trigger_ms = self
            .block_render_context(app)
            .mouse_state("mode-menu-trigger");
        let trigger = EventHandler::new(
            Hoverable::new(trigger_ms, move |state| {
                let hot = open || state.is_hovered();
                let bg = if hot {
                    warpui::color::ColorU::new(
                        theme_t.accent.r,
                        theme_t.accent.g,
                        theme_t.accent.b,
                        46,
                    )
                } else {
                    theme_t.surface_2()
                };
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(5.0)
                        .with_child(
                            Text::new(mode_label.clone(), mono, crate::theme::text_size(11.5))
                                .with_color(theme_t.accent)
                                .with_style(warpui::fonts::Properties {
                                    weight: warpui::fonts::Weight::Medium,
                                    ..Default::default()
                                })
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(9.0)
                .with_padding_right(7.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .with_background_color(bg)
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(6.0),
                ))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
        .on_left_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::ToggleModeMenu);
            DispatchEventResult::StopPropagation
        })
        .finish();

        if !open {
            return trigger;
        }

        // ── Dropdown list (rendered above the trigger) ──
        let theme = *theme;
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0);
        for row in rows {
            let selected = row.selected;
            let ms = self
                .block_render_context(app)
                .mouse_state(&format!("mode-row-{}", row.label));
            let (row_label, row_subtitle, row_action) = (row.label, row.subtitle, row.action);
            let row = EventHandler::new(
                Hoverable::new(ms, move |state| {
                    let hovered = state.is_hovered();
                    let bg = if selected {
                        warpui::color::ColorU::new(
                            theme.accent.r,
                            theme.accent.g,
                            theme.accent.b,
                            30,
                        )
                    } else if hovered {
                        theme.surface_3()
                    } else {
                        theme.surface_2()
                    };
                    let name_color = if selected {
                        theme.accent
                    } else {
                        theme.main_text()
                    };
                    Container::new(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_spacing(1.0)
                            .with_child(
                                Text::new(row_label.clone(), mono, crate::theme::text_size(12.5))
                                    .with_color(name_color)
                                    .with_style(warpui::fonts::Properties {
                                        weight: warpui::fonts::Weight::Medium,
                                        ..Default::default()
                                    })
                                    .finish(),
                            )
                            .with_child(
                                Text::new(
                                    row_subtitle.clone(),
                                    mono,
                                    crate::theme::text_size(10.5),
                                )
                                .with_color(theme.sub_text())
                                .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .with_background_color(bg)
                    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                        warpui_core::scene::Radius::Pixels(5.0),
                    ))
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(row_action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }

        let popover = ConstrainedBox::new(
            Container::new(list.finish())
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(theme.surface_1())
                .with_border(
                    warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                )
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(8.0),
                ))
                .finish(),
        )
        .with_width(250.0)
        .finish();

        Stack::new()
            .with_child(trigger)
            .with_positioned_overlay_child(
                popover,
                OffsetPositioning::offset_from_parent(
                    Vector2F::new(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                ),
            )
            .finish()
    }

    /// The slash-command palette — a popover of matching commands shown
    /// while the buffer is a bare `/`-prefixed token (no space yet, i.e.
    /// still composing the command name). `None` while that shape isn't
    /// met, so callers can skip the overlay wrapper entirely.
    pub(crate) fn render_slash_palette(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Option<Box<dyn Element>> {
        let text = self
            .command_editor
            .read(app, |e, _| e.get_text().to_string());
        let candidates = slash_command_candidates(&text, &self.cwd(app))?;
        let selected = self
            .slash_palette_selected
            .min(candidates.len().saturating_sub(1));

        let last_idx = candidates.len().saturating_sub(1);
        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (idx, cmd) in candidates.iter().enumerate() {
            let is_selected = idx == selected;
            let is_last = idx == last_idx;
            let theme_row = *theme;
            let name = cmd.name.clone();
            // Keep the palette compact. Descriptions are discovery metadata,
            // not a second document: the full text is available in Settings
            // and the skill body is loaded only after invocation.
            let description: String = cmd.description.chars().take(96).collect();
            let click_name = name.clone();
            let ms = self
                .block_render_context(app)
                .mouse_state(&format!("slash-row-{name}"));
            let row = EventHandler::new(
                Hoverable::new(ms, move |state| {
                    let hovered = state.is_hovered();
                    let bg = if is_selected {
                        warpui::color::ColorU::new(
                            theme_row.accent.r,
                            theme_row.accent.g,
                            theme_row.accent.b,
                            30,
                        )
                    } else if hovered {
                        theme_row.surface_3()
                    } else {
                        theme_row.surface_1()
                    };
                    let name_color = if is_selected {
                        theme_row.accent
                    } else {
                        theme_row.main_text()
                    };
                    let mut row_container = Container::new(
                        Flex::row()
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(10.0)
                            .with_child(
                                ConstrainedBox::new(
                                    Text::new_inline(
                                        name.clone(),
                                        mono,
                                        crate::theme::text_size(12.5),
                                    )
                                    .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                                    .with_color(name_color)
                                    .with_style(warpui::fonts::Properties {
                                        weight: warpui::fonts::Weight::Medium,
                                        ..Default::default()
                                    })
                                    .finish(),
                                )
                                .with_width(155.0)
                                .finish(),
                            )
                            .with_child(Box::new(Expanded::new(
                                1.0,
                                Text::new_inline(
                                    description.clone(),
                                    mono,
                                    crate::theme::text_size(10.5),
                                )
                                .soft_wrap(false)
                                .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                                .with_color(theme_row.sub_text())
                                .finish(),
                            )))
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(bg);
                    if !is_last {
                        row_container = row_container.with_border(
                            warpui_core::scene::Border::bottom(1.0)
                                .with_border_color(theme_row.outline()),
                        );
                    }
                    row_container.finish()
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::RunSlashCommand {
                    name: click_name.clone(),
                });
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }

        let scrollable = ClippedScrollable::vertical(
            self.slash_scroll_state.clone(),
            list.finish(),
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish();
        let popover = ConstrainedBox::new(
            Container::new(scrollable)
                .with_background_color(theme.surface_1())
                .with_border(
                    warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                )
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(8.0),
                ))
                .finish(),
        )
        .with_width(480.0)
        .with_max_height(440.0)
        .finish();

        Some(popover)
    }

    pub(crate) fn render_context_meter(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Option<Box<dyn Element>> {
        let (ctx_tokens, model) = self.agent_controller.read(app, |c, _| {
            (c.current_context_tokens(), c.last_model_name().to_string())
        });
        if ctx_tokens == 0 || model.is_empty() {
            return None;
        }
        // Real per-model window from the account store (the values verified
        // against provider docs). No account data yet → no meter.
        let window = {
            use warpui::SingletonEntity;
            let account = crate::settings::AccountStore::handle(app);
            app.read_model(&account, |store, _| {
                store.data.as_ref().and_then(|d| {
                    d.models
                        .iter()
                        .find(|m| m.id == model)
                        .map(|m| m.context_window)
                })
            })
        };
        let window = match window {
            Some(w) if w > 0 => w as u64,
            _ => return None,
        };

        let pct = ((ctx_tokens as f64 / window as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u64;
        let color = if pct >= 90 {
            theme.ui_red()
        } else if pct >= 75 {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let label = format!(
            "{pct}% · {}/{}",
            humanize_tokens(ctx_tokens),
            humanize_tokens(window)
        );

        Some(
            Container::new(
                Text::new(label, mono, crate::theme::text_size(11.5))
                    .with_color(color)
                    .finish(),
            )
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish(),
        )
    }

    /// Close the input-bar dropdowns (mode / model).
    ///
    /// Called from the pane-level focus handlers, which fire on any click no
    /// child consumed — the popover rows all StopPropagation, so a click inside
    /// a popover never reaches here and only genuine outside-clicks dismiss.
    /// Returns true if anything was actually open, so callers can skip a
    /// needless repaint.
    pub(crate) fn close_input_bar_menus(&mut self) -> bool {
        let was_open = self.mode_menu_open || self.model_menu_open;
        self.mode_menu_open = false;
        self.model_menu_open = false;
        was_open
    }
}
