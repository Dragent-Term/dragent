//! Terminal input: focus, editors, attachments, side commands.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT, CMD_HIST_CHROME_PX,
    CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y, SCROLL_BOTTOM_EPSILON_PX,
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
    pub(crate) fn send_focus_csi(&self, focus_in: bool, ctx: &mut ViewContext<Self>) {
        let bytes = self.model.read(ctx, |m, _| {
            if focus_in {
                m.focus_in_bytes()
            } else {
                m.focus_out_bytes()
            }
        });
        if let Some(bytes) = bytes {
            self.manager.writer().write(&bytes);
        }
    }

    pub(crate) fn ai_rich_input_panel_open<C: ReadModel>(&self, ctx: &C) -> bool {
        self.rich_input_active && self.active_ai_tool(ctx).is_some()
    }

    pub(crate) fn set_rich_input_proxy_chars(&mut self, chars: usize, ctx: &ViewContext<Self>) {
        self.rich_input_proxy_chars = chars;
        self.rich_input_editor.read(ctx, |editor, _| {
            editor.set_native_slash_proxy_active(chars > 0);
        });
    }

    pub(crate) fn paste_to_rich_input_editor(
        &mut self,
        content: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.rich_input_editor, |editor, ectx| {
            editor.insert_text_at_cursor(content, ectx);
            ectx.focus_self();
        });
        ctx.notify();
    }

    pub(crate) fn local_input_active(&self, ctx: &ViewContext<Self>) -> bool {
        if self.uses_agent_transcript() { return true; }
        self.model.read(ctx, |m, _| {
            !m.alt_screen_active() && !m.is_command_running()
        })
    }

    pub(crate) fn submit_command_editor(&mut self, ctx: &mut ViewContext<Self>) {
        if self.host_notice_active { return; }
        // Enter on the history popover runs the previewed command: close the
        // popover (keeping the command in the editor) and fall through to submit.
        let history_shell_config = self.cmd_hist_previous_input_config.take();
        if self.cmd_hist_open {
            self.close_cmd_history(false, ctx);
        }
        let completion_open = self.path_picker.read(ctx, |m, _| m.open);
        if completion_open {
            self.handle_action(&TerminalAction::AcceptCompletion, ctx);
            self.restore_history_input_config(ctx);
            return;
        }
        let command = if let Some(session) = self.workflow_session.take() {
            session.finalize()
        } else {
            self.command_editor
                .read(ctx, |editor, _| editor.get_text().trim().to_string())
        };
        if command.is_empty() {
            self.restore_history_input_config(ctx);
            ctx.notify();
            return;
        }

        // Slash commands for explicitly switching input mode.
        match command.as_str() {
            "/agent" => {
                self.input_config.input_type = DetectedInputType::Agent;
                self.input_config.is_locked = true;
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    editor.set_text("", ectx);
                });
                self.sync_input_placeholder(ctx);
                self.restore_history_input_config(ctx);
                ctx.notify();
                return;
            }
            "/shell" => {
                self.input_config.input_type = DetectedInputType::Shell;
                self.input_config.is_locked = true;
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    editor.set_text("", ectx);
                });
                self.sync_input_placeholder(ctx);
                self.restore_history_input_config(ctx);
                ctx.notify();
                return;
            }
            _ => {}
        }

        // User-invoked skills are slash commands even when model invocation is
        // disabled. Keep the visible user turn as the slash command itself;
        // the agent's system prompt carries the skill's instructions for this
        // request. Never paste SKILL.md into the visible transcript.
        if command.starts_with('/') {
            let cwd = self.cwd(ctx);
            if crate::agent::skill_discovery::invoked_skill(&command, &cwd).is_some() {
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    editor.set_text("", ectx)
                });
                self.input_config.input_type = DetectedInputType::Agent;
                self.input_config.is_locked = true;
                self.submit_agent_query(command.clone(), ctx);
                self.restore_history_input_config(ctx);
                ctx.notify();
                return;
            }
        }

        self.sync_workflow_highlight(ctx);

        let input_type = if history_shell_config.is_some() {
            DetectedInputType::Shell
        } else {
            self.effective_input_type(ctx)
        };
        match input_type {
            DetectedInputType::Agent => {
                self.resolve_mention_chips(ctx);
                self.submit_agent_query(command, ctx);
            }
            DetectedInputType::Shell => {
                self.submit_shell_command_inner(command, ctx);
            }
        }
        if let Some(previous) = history_shell_config {
            self.input_config = previous;
            self.sync_input_placeholder(ctx);
        }
    }

    /// Apply a slash-palette selection. Skill rows are completions: selecting
    /// one inserts `/skill-name ` into the composer so the user can add the
    /// task prompt. The skill is sent only when the completed user prompt is
    /// submitted.
    pub(crate) fn run_slash_command(
        &mut self,
        kind: SlashCommandKind,
        ctx: &mut ViewContext<Self>,
    ) {
        match kind {
            SlashCommandKind::InputMode(t) => {
                self.input_config.input_type = t;
                self.input_config.is_locked = true;
                self.sync_input_placeholder(ctx);
            }
            SlashCommandKind::Clear => {
                self.pending_attachments.clear();
                self.attached_blocks.clear();
                self.handle_action(&TerminalAction::ClearAgentConversation, ctx);
            }
            SlashCommandKind::ChatMode(mode) => {
                self.handle_action(&TerminalAction::SetChatMode { mode }, ctx);
            }
            SlashCommandKind::OpenModelMenu => {
                self.handle_action(&TerminalAction::ToggleModelMenu, ctx);
            }
            SlashCommandKind::Skill(skill) => {
                let command = format!("/{}", skill.slash_name().trim_start_matches('/'));
                let command = format!("{command} ");
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    editor.set_text(&command, ectx);
                    ectx.focus_self();
                });
                self.slash_palette_selected = 0;
                ctx.notify();
                return;
            }
        }
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text("", ectx);
        });
        self.slash_palette_selected = 0;
        ctx.notify();
    }

    /// Load each `@`-mention chip left in the command editor as an attachment,
    /// reusing the exact same `load`/`pending_attachments` pipeline the "+"
    /// file picker already uses — chips resolve content at submit time, not
    /// at pick time, since a chip is more likely to be added/removed
    /// mid-composition than a deliberate one-shot attach.
    pub(crate) fn resolve_mention_chips(&mut self, ctx: &mut ViewContext<Self>) {
        const MAX_ATTACHMENTS: usize = 5;
        let chip_paths: Vec<std::path::PathBuf> = self.command_editor.read(ctx, |editor, _| {
            editor.chip_spans().iter().map(|c| c.path.clone()).collect()
        });
        for path in chip_paths {
            if self.pending_attachments.len() >= MAX_ATTACHMENTS {
                self.show_error_notice(("Attachment failed".to_string(), format!(
                    "Maximum {MAX_ATTACHMENTS} attachments per message."
                )), ctx);
                break;
            }
            match crate::agent::attachments::load(&path) {
                Ok(att) => self.pending_attachments.push(att),
                Err(msg) => {
                    log::warn!("[mention] {msg}");
                    self.show_error_notice(("Attachment failed".to_string(), msg), ctx);
                }
            }
        }
    }

    // ── Attachments & voice input ─────────────────────────────────────────────

    /// Pending-attachments strip: sits directly ABOVE the input box on its own
    /// surface (like the agent activity strip). Images render as thumbnails
    /// with an × badge (click image → carousel; click × → remove); text files
    /// stay compact "name ×" chips. Wraps onto extra rows when full.
    pub(crate) fn render_attachment_strip(
        &self,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        use warpui_core::assets::asset_cache::AssetSource;
        use warpui_core::image_cache::CacheOption;

        let mut row = warpui::elements::Wrap::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_run_spacing(8.0);
        let mut image_ordinal = 0usize;
        for (i, att) in self.pending_attachments.iter().enumerate() {
            if let Some(path) = att.image_path() {
                let ordinal = image_ordinal;
                image_ordinal += 1;

                let thumb = warpui::elements::Image::new(
                    AssetSource::LocalFile {
                        path: path.to_string_lossy().into_owned(),
                    },
                    CacheOption::Original,
                )
                .contain()
                .finish();
                let framed = Container::new(
                    warpui::elements::ConstrainedBox::new(thumb)
                        .with_width(46.0)
                        .with_height(46.0)
                        .finish(),
                )
                .with_background_color(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(6.0),
                ))
                .finish();
                let clickable = EventHandler::new(framed)
                    .on_left_mouse_down(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(TerminalAction::OpenImageViewer {
                            index: ordinal,
                        });
                        DispatchEventResult::StopPropagation
                    })
                    .finish();

                // × badge overlaid on the top-right corner of the thumbnail.
                let badge_inner = Container::new(
                    Text::new("×".to_string(), mono, crate::theme::text_size(11.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_background_color(warpui::color::ColorU {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 190,
                })
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(7.0),
                ))
                .finish();
                let badge = EventHandler::new(badge_inner)
                    .on_left_mouse_down(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(TerminalAction::RemovePendingAttachment {
                            index: i,
                        });
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                let offset = OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(2.0, -4.0),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::TopRight,
                    ChildAnchor::TopRight,
                );
                row = row.with_child(Box::new(
                    Stack::new()
                        .with_child(clickable)
                        .with_positioned_child(badge, offset),
                ));
            } else {
                let label = format!("≡ {} ×", att.name());
                let chip_inner = Container::new(
                    Text::new(label, mono, crate::theme::text_size(11.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .with_background_color(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(9.0),
                ))
                .finish();
                let chip = EventHandler::new(chip_inner)
                    .on_left_mouse_down(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(TerminalAction::RemovePendingAttachment {
                            index: i,
                        });
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                row = row.with_child(chip);
            }
        }
        Container::new(row.finish())
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::top(1.0).with_border_color(theme.outline()))
            .finish()
    }

    /// Full-size image carousel overlay (scrim click closes; ‹ › navigate).
    pub(crate) fn render_image_viewer(&self, current: usize, theme: &Theme) -> Box<dyn Element> {
        use warpui_core::assets::asset_cache::AssetSource;
        use warpui_core::image_cache::CacheOption;
        let mono = self.mono_family;

        let images: Vec<(String, String)> = self
            .pending_attachments
            .iter()
            .filter_map(|a| {
                a.image_path()
                    .map(|p| (p.to_string_lossy().into_owned(), a.name().to_string()))
            })
            .collect();
        if images.is_empty() {
            return warpui::elements::Empty::new().finish();
        }
        let current = current.min(images.len() - 1);
        let (path, name) = images[current].clone();

        let big =
            warpui::elements::Image::new(AssetSource::LocalFile { path }, CacheOption::Original)
                .contain()
                .finish();
        // Size from the live viewport so the image gets most of the window on
        // any screen, and the card never overflows small ones.
        let vw = self.output_viewport_width_px.max(400.0);
        let vh = self.output_viewport_height_px.max(300.0);
        let max_w = (vw * 0.92).clamp(320.0, 1600.0);
        let max_h = (vh * 0.82).clamp(240.0, 1100.0);
        let picture = Container::new(
            warpui::elements::ConstrainedBox::new(big)
                .with_max_width(max_w)
                .with_max_height(max_h)
                .with_min_width((max_w * 0.4).min(360.0))
                .with_min_height((max_h * 0.4).min(260.0))
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(8.0),
        ))
        .with_uniform_padding(8.0)
        .finish();

        let nav_btn = |label: &str, action: TerminalAction| -> Box<dyn Element> {
            let inner = Container::new(
                Text::new(label.to_string(), mono, crate::theme::text_size(16.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(theme.surface_2())
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(6.0),
            ))
            .finish();
            EventHandler::new(inner)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(action.clone());
                    DispatchEventResult::StopPropagation
                })
                .finish()
        };

        let mut controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(14.0);
        if images.len() > 1 {
            controls =
                controls.with_child(nav_btn("‹", TerminalAction::ImageViewerNav { delta: -1 }));
        }
        controls = controls.with_child(
            Text::new(
                format!("{name}   ·   {} / {}", current + 1, images.len()),
                mono,
                crate::theme::text_size(12.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
        if images.len() > 1 {
            controls =
                controls.with_child(nav_btn("›", TerminalAction::ImageViewerNav { delta: 1 }));
        }
        controls = controls.with_child(nav_btn("\u{00d7}", TerminalAction::CloseImageViewer));

        // Clicks inside the card stay in the card; clicks on the scrim close.
        let card = EventHandler::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.0)
                .with_child(picture)
                .with_child(controls.finish())
                .finish(),
        )
        .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
        .finish();

        let scrim = Container::new(warpui::elements::Align::new(card).finish())
            .with_background_color(warpui::color::ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 190,
            })
            .finish();
        EventHandler::new(scrim)
            .on_left_mouse_down(|event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::CloseImageViewer);
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    pub(crate) fn diagram_viewer_zoom_button(
        &self,
        label: &str,
        action: TerminalAction,
        theme: &Theme,
    ) -> Box<dyn Element> {
        let button = Container::new(
            Text::new(
                label.to_string(),
                self.mono_family,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(4.0),
        ))
        .finish();
        EventHandler::new(button)
            .on_left_mouse_down(move |event_ctx, _app, _position| {
                let next_action = match action {
                    TerminalAction::DiagramViewerZoomIn => TerminalAction::DiagramViewerZoomIn,
                    TerminalAction::DiagramViewerZoomOut => TerminalAction::DiagramViewerZoomOut,
                    TerminalAction::DiagramViewerZoomReset => {
                        TerminalAction::DiagramViewerZoomReset
                    }
                    _ => return DispatchEventResult::StopPropagation,
                };
                event_ctx.dispatch_typed_action(next_action);
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    pub(crate) fn render_diagram_viewer(
        &self,
        path: &str,
        width: f32,
        height: f32,
        zoom: f32,
        theme: &Theme,
    ) -> Box<dyn Element> {
        use warpui_core::assets::asset_cache::AssetSource;
        use warpui_core::image_cache::CacheOption;
        let mono = self.mono_family;

        let big = warpui::elements::Image::new(
            AssetSource::LocalFile {
                path: path.to_string(),
            },
            CacheOption::Original,
        )
        .contain()
        .finish();
        // Size from the live viewport so the diagram gets most of the window on
        // any screen. Diagrams are vector-rendered on demand (CacheOption::
        // Original), so scaling up costs nothing in quality — fit the diagram's
        // own aspect ratio as large as possible inside the viewport box instead
        // of capping at its raw SVG viewBox size (which made this dialog barely
        // bigger than the inline block for most diagrams).
        let vw = self.output_viewport_width_px.max(400.0);
        let vh = self.output_viewport_height_px.max(300.0);
        let box_w = (vw * 0.94).clamp(320.0, 2000.0);
        let box_h = (vh * 0.88).clamp(240.0, 1400.0);
        let aspect = if width > 0.0 { height / width } else { 1.0 };
        // Wide diagrams look artificially short when the viewer fits the whole
        // SVG inside the pane width. Make the viewport height the primary scale
        // so expanded diagrams use the available screen height; horizontal
        // overflow is handled by the scroll container below.
        let zoom = zoom.clamp(0.25, 4.0);
        let disp_h = (box_h * zoom).clamp(120.0, 8000.0);
        let disp_w = (disp_h / aspect.max(0.01)).max(240.0);
        let image = warpui::elements::ConstrainedBox::new(big)
            .with_width(disp_w)
            .with_height(disp_h)
            .finish();
        let image = Align::new(image).finish();
        let image_scroll_state = self.diagram_horizontal_scroll.clone();
        let drag_last = self.diagram_drag_last.clone();
        let image_scroll = ClippedScrollable::horizontal(
            image_scroll_state.clone(),
            image,
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.main_text().into(),
            theme.surface_1().into(),
        )
        .finish();
        let image_scroll = EventHandler::new(image_scroll)
            .on_left_mouse_down({
                let drag_last = drag_last.clone();
                move |_event_ctx, _app, position| {
                    *drag_last.lock().unwrap() = Some(position);
                    DispatchEventResult::StopPropagation
                }
            })
            .on_mouse_dragged({
                let drag_last = drag_last.clone();
                move |event_ctx, _app, position| {
                    let previous = drag_last.lock().unwrap().replace(position);
                    if let Some(previous) = previous {
                        let delta_x = previous.x() - position.x();
                        if delta_x.abs() > f32::EPSILON {
                            image_scroll_state.scroll_by(Pixels::new(delta_x));
                            event_ctx.notify();
                        }
                    }
                    DispatchEventResult::StopPropagation
                }
            })
            .on_left_mouse_up({
                let drag_last = drag_last.clone();
                move |_event_ctx, _app, _position| {
                    *drag_last.lock().unwrap() = None;
                    DispatchEventResult::StopPropagation
                }
            })
            .finish();
        let picture = Container::new(
            warpui::elements::ConstrainedBox::new(image_scroll)
                .with_width(box_w)
                .with_height(box_h)
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(8.0),
        ))
        .with_uniform_padding(8.0)
        .finish();

        let close_btn = {
            let inner = Container::new(
                Text::new("\u{00d7}".to_string(), mono, crate::theme::text_size(16.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(theme.surface_2())
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(6.0),
            ))
            .finish();
            EventHandler::new(inner)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::CloseDiagramViewer);
                    DispatchEventResult::StopPropagation
                })
                .finish()
        };

        let controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(14.0)
            .with_child(
                Text::new("diagram".to_string(), mono, crate::theme::text_size(12.5))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_child(self.diagram_viewer_zoom_button(
                "−",
                TerminalAction::DiagramViewerZoomOut,
                theme,
            ))
            .with_child(
                Text::new(
                    format!("{}%", (zoom * 100.0).round() as i32),
                    mono,
                    crate::theme::text_size(11.0),
                )
                .with_color(theme.disabled_text())
                .finish(),
            )
            .with_child(self.diagram_viewer_zoom_button(
                "+",
                TerminalAction::DiagramViewerZoomIn,
                theme,
            ))
            .with_child(self.diagram_viewer_zoom_button(
                "reset",
                TerminalAction::DiagramViewerZoomReset,
                theme,
            ))
            .with_child(close_btn)
            .finish();

        // Clicks inside the card stay in the card; clicks on the scrim close.
        let card = EventHandler::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.0)
                .with_child(picture)
                .with_child(controls)
                .finish(),
        )
        .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
        .finish();

        let scrim = Container::new(warpui::elements::Align::new(card).finish())
            .with_background_color(warpui::color::ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 190,
            })
            .finish();
        EventHandler::new(scrim)
            .on_left_mouse_down(|event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::CloseDiagramViewer);
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    /// Append quoted file paths to the AI-tool rich input, expanding the
    /// panel so the user sees them land. Used by both the picker and
    /// drag-and-drop when an AI CLI owns the input.
    pub(crate) fn insert_paths_into_rich_input(
        &mut self,
        paths: &[std::path::PathBuf],
        ctx: &mut ViewContext<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        if !self.rich_input_active {
            self.rich_input_active = true;
            self.last_dims = None;
        }
        let joined = paths
            .iter()
            .map(|p| super::terminal_helpers::shell_quote(&p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        ctx.update_view(&self.rich_input_editor, |editor, ectx| {
            let mut text = editor.get_text().to_string();
            if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                text.push(' ');
            }
            text.push_str(&joined);
            editor.set_text(&text, ectx);
            ectx.focus_self();
        });
        ctx.notify();
    }

    /// Route incoming files (picker or drag-and-drop) by input mode: AI tool →
    /// paths into the input text; terminal → pending attachments for the agent.
    pub(crate) fn receive_context_files(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.active_ai_tool(ctx).is_some() {
            self.insert_paths_into_rich_input(&paths, ctx);
            return;
        }
        const MAX_ATTACHMENTS: usize = 5;
        for path in paths {
            if self.pending_attachments.len() >= MAX_ATTACHMENTS {
                self.show_error_notice(("Attachment failed".to_string(), format!(
                    "Maximum {MAX_ATTACHMENTS} attachments per message."
                )), ctx);
                break;
            }
            match crate::agent::attachments::load(&path) {
                Ok(att) => self.pending_attachments.push(att),
                Err(msg) => {
                    log::warn!("[attach] {msg}");
                    self.show_error_notice(("Attachment failed".to_string(), msg), ctx);
                }
            }
        }
        if !self.pending_attachments.is_empty() {
            self.force_agent_input_mode(ctx);
        }
        ctx.notify();
    }

    /// Open the native file picker (non-blocking) and load selections as
    /// pending attachments when the user closes it. The panel must NOT run a
    /// modal loop inside this action handler — AppContext is borrowed here,
    /// and a nested event loop re-enters it ("RefCell already borrowed" abort).
    pub(crate) fn attach_context_files(&mut self, ctx: &mut ViewContext<Self>) {
        let (tx, rx) = async_channel::bounded::<Vec<std::path::PathBuf>>(1);
        ctx.spawn_stream_local(
            rx,
            |view, paths: Vec<std::path::PathBuf>, ctx| view.receive_context_files(paths, ctx),
            |_, _| {},
        );
        crate::agent::attachments::pick_files_async(tx);
    }

    /// Attach an editor symbol (file + 1-based line range) to this agent's
    /// composer, mirroring the file-attach flow with a line number:
    /// - AI CLI owns input (Claude etc.): append `'path:line'` (or
    ///   `'path:start-end'`) to the rich input so the CLI reads the file
    ///   itself at that position.
    /// - Built-in Dragent path: load the file into `pending_attachments`
    ///   (whole-file content, same as the "+" picker) and append the same
    ///   `path:line` reference to the command editor so the model knows
    ///   which position matters.
    pub(crate) fn attach_editor_symbol(
        &mut self,
        path: &std::path::Path,
        line: u32,
        end_line: u32,
        ctx: &mut ViewContext<Self>,
    ) {
        let line = line.max(1);
        let end_line = end_line.max(line);
        let reference = if end_line > line {
            format!("'{}:{}-{}'", path.display(), line, end_line)
        } else {
            format!("'{}:{}'", path.display(), line)
        };
        if self.active_ai_tool(ctx).is_some() {
            if !self.rich_input_active {
                self.rich_input_active = true;
                self.last_dims = None;
            }
            ctx.update_view(&self.rich_input_editor, |editor, ectx| {
                let mut text = editor.get_text().to_string();
                if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                    text.push(' ');
                }
                text.push_str(&reference);
                text.push(' ');
                editor.set_text(&text, ectx);
                ectx.focus_self();
            });
        } else {
            const MAX_ATTACHMENTS: usize = 5;
            match crate::agent::attachments::load(path) {
                Ok(att) => {
                    if self.pending_attachments.len() >= MAX_ATTACHMENTS {
                        self.show_error_notice(("Attachment failed".to_string(), format!(
                            "Maximum {MAX_ATTACHMENTS} attachments per message."
                        )), ctx);
                    } else {
                        self.pending_attachments.push(att);
                    }
                }
                Err(msg) => {
                    log::warn!("[attach-symbol] {msg}");
                    self.show_error_notice(("Attachment failed".to_string(), msg), ctx);
                }
            }
            self.force_agent_input_mode(ctx);
            ctx.update_view(&self.command_editor, |editor, ectx| {
                let mut text = editor.get_text().to_string();
                if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                    text.push(' ');
                }
                text.push_str(&reference);
                text.push(' ');
                editor.set_text(&text, ectx);
                ectx.focus_self();
            });
        }
        ctx.notify();
    }
}
