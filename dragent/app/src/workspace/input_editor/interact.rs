//! Cursor, selection, menu, and clipboard interaction.
//!
//! Blocks here are impl InputEditor: same type, same behavior.

use super::model::*;
use super::text::*;
use crate::console::command_intel::{diagnostic_cols_on_line, diagnostics_for};
use crate::console::input_bar::{
    build_highlighted_command, is_likely_command, style_runs_for_command,
};
use crate::console::input_editing::{
    line_end, line_start, matching_close_bracket, word_end, word_start,
};
use crate::console::input_layout::{
    caret_x_for_char_index, caret_x_in_layout, caret_x_plain, char_col_at_x,
    char_col_at_x_in_layout, char_index_at_click_plain, line_height_with_spacing,
    selection_cols_on_line, unwrapped_line_layout, visual_subline_layouts,
};
use crate::console::mention_popup::MentionEntry;
use crate::console::vim_mode::{VimContext, VimDispatch, VimEffect, VimMode, VimState};
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::editor_render::{
    self, EDITOR_BREAKPOINT_ZONE_WIDTH, EDITOR_CONTENT_PADDING_LEFT, EDITOR_FOLD_ZONE_WIDTH,
};
use dragent_document::{
    capabilities_for_text, DiffLineKind, DocumentCapabilities, DocumentDiff, GitDiffDisplayRow,
};
use dragent_editor::{
    Breakpoint, BreakpointRegistry, DiagnosticSeverity, FoldIndex, FoldParser, FoldRegion,
    FoldState, Language, LineChangeKind, ResolvedDiagnostic,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance, SingleAxisConfig,
};
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded,
    Fill, Flex, MainAxisAlignment, MainAxisSize, NewScrollable, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Point, SavePosition, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::{Cache as FontCache, FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::keymap::Keystroke;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{
    AfterLayoutContext, AppContext, Entity, EventContext, FocusHandle, LayoutContext, PaintContext,
    SizeConstraint, TypedActionView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Hoverable, MouseStateHandle, SELECTED_HIGHLIGHT_COLOR};
use warpui_core::event::DispatchedEvent;
use warpui_core::event::KeyState;
use warpui_core::event::ModifiersState;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::units::IntoPixels;

use super::context_menu_alpha;
use super::InputEditor;

impl InputEditor {
    pub(crate) fn schedule_cursor_blink(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(handle) = self.blink_handle.take() {
            handle.abort();
        }
        // Settings -> Appearance -> Cursor blink + Blink rate.
        let prefs = crate::console::editor_settings::EditorSettings::global();
        if !prefs.terminal.cursor_blink {
            self.cursor_visible = true;
            return;
        }
        let rate_ms = prefs.terminal.clamped_cursor_blink_rate_ms() as u64;

        self.blink_handle = Some(ctx.spawn_abortable(
            Timer::after(Duration::from_millis(rate_ms)),
            |view, _, ctx| {
                view.cursor_visible = !view.cursor_visible;
                ctx.notify();
                view.schedule_cursor_blink(ctx);
            },
            |view, _| {
                view.blink_handle = None;
            },
        ));
    }

    /// Key under which this editor saves/looks up its rendered bounds.
    ///
    /// MUST be unique per live editor: the position cache is one flat map keyed
    /// by this string, and `char_index_at_position` hit-tests clicks against
    /// whatever bounds it finds. Mode alone is not unique — a split gives every
    /// pane its own `TerminalCommand` editor (and `terminal_view` builds a
    /// `MultiLine { key: "branch_diff_commit_msg" }` per pane), so keying by
    /// mode made panes overwrite each other's bounds. The click then hit-tested
    /// against the *other* pane's rect: `relative_y` went negative, clamped to
    /// 0.0, and every click landed on the first visual row. Hence `instance_id`.
    pub(crate) fn position_id(&self) -> String {
        let id = self.instance_id;
        match &self.mode {
            InputMode::Search => format!("input_editor_search_{id}"),
            InputMode::RenameTab(tab_id) => format!("input_editor_rename_{tab_id}_{id}"),
            InputMode::RenameWorkspaceTab(surface_id) => {
                format!("input_editor_workspace_rename_{surface_id}_{id}")
            }
            InputMode::LanguageRename { .. } => format!("input_editor_language_rename_{id}"),
            InputMode::TerminalCommand => format!("terminal_input_text_{id}"),
            InputMode::Settings(key) => format!("input_editor_settings_{key}_{id}"),
            InputMode::MultiLine { key, .. } => format!("input_editor_multiline_{key}_{id}"),
            InputMode::Plain { key } => format!("input_editor_plain_{key}_{id}"),
        }
    }

    pub(crate) fn viewport_position_id(&self) -> String {
        format!("{}_viewport", self.position_id())
    }

    pub(crate) fn char_index_at_position(
        &self,
        click_pos: Vector2F,
        ctx: &ViewContext<Self>,
    ) -> Option<usize> {
        let bounds = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))?;
        // A click needs exact hit-testing at the current width immediately;
        // resize reflow is deferred only for passive resize frames.
        let text_width = self.text_layout_width(bounds.width());
        self.layout_width.set(text_width);
        self.available_width.set(text_width);
        if self.is_source_editor()
            && self.document_capabilities.large_file
            && self.git_diff_projection.is_none()
        {
            let index = self.large_source_index();
            let scroll_y = self.scroll_state.scroll_start().as_f32().max(0.0);
            let row = ((click_pos.y() - bounds.origin().y() + scroll_y).max(0.0)
                / self.line_height())
            .floor() as usize;
            let line_number = row.min(index.line_count().saturating_sub(1));
            if let Some(line) =
                self.large_source_visual_line(&index, line_number, ctx.font_cache(), true)
            {
                let horizontal_scroll = self.rendered_horizontal_scroll_offset();
                let relative_x = source_content_x(
                    click_pos.x(),
                    bounds.origin().x(),
                    self.source_gutter_width(),
                    horizontal_scroll,
                );
                return Some(
                    line.abs_char_offset
                        + char_col_at_x_in_layout(&line.text, &line.layout, relative_x),
                );
            }
            return Some(self.buffer.chars().count());
        }
        Some(match self.mode {
            InputMode::TerminalCommand | InputMode::MultiLine { .. } => {
                let visual_lines = self.visual_line_map(text_width, ctx.font_cache());
                let scroll_y = self.scroll_state.scroll_start().as_f32().max(0.0);
                let relative_y = (click_pos.y() - bounds.origin().y() + scroll_y).max(0.0);
                let display_row = (relative_y / self.line_height()).floor() as usize;
                let row_index = if self.source_path.is_some() {
                    if let Some(projection) = self.git_diff_projection.as_ref() {
                        let Some(GitDiffDisplayRow::Current { line, .. }) =
                            projection.rows.get(display_row)
                        else {
                            // Deleted and collapsed rows have no current-buffer
                            // coordinate. Clicking them must not move the caret.
                            return None;
                        };
                        visual_lines
                            .iter()
                            .position(|visual| !visual.continuation && visual.source_line == *line)
                            .unwrap_or_else(|| visual_lines.len().saturating_sub(1))
                    } else {
                        let fold_index = self.fold_index();
                        let indices =
                            self.visible_visual_indices(&visual_lines, &fold_index, text_width);
                        indices
                            .get(display_row)
                            .copied()
                            .unwrap_or_else(|| indices.last().copied().unwrap_or(0))
                    }
                } else {
                    display_row.min(visual_lines.len().saturating_sub(1))
                };

                let Some(line) = visual_lines.get(row_index) else {
                    // A resize may invalidate the visual map between pointer
                    // events. Clamp to the document tail instead of allowing a
                    // stale drag row to index past the rebuilt layout.
                    return Some(self.buffer.chars().count());
                };
                let gutter_offset = if self.source_path.is_some()
                    && matches!(self.mode, InputMode::MultiLine { .. })
                {
                    self.source_gutter_width()
                } else {
                    0.0
                };
                let horizontal_scroll = if self.is_source_editor() {
                    self.rendered_horizontal_scroll_offset()
                } else {
                    0.0
                };
                let relative_x = if self.is_source_editor() {
                    source_content_x(
                        click_pos.x(),
                        bounds.origin().x(),
                        self.source_gutter_width(),
                        horizontal_scroll,
                    )
                } else {
                    (click_pos.x() - bounds.origin().x() - gutter_offset).max(0.0)
                };
                if self.is_source_editor() {
                    // Source lines are deliberately unwrapped, and their
                    // layout is built from only `line.text`. Convert the
                    // layout byte position into our character coordinate.
                    line.abs_char_offset
                        + char_col_at_x_in_layout(&line.text, &line.layout, relative_x)
                } else {
                    // A wrapped terminal-input row retains caret offsets from
                    // its complete physical line. `abs_char_offset` already
                    // contains this visual row's start, so first normalize the
                    // layout result to the row before adding the absolute base.
                    // Treating this result as a byte offset counted the wrapped
                    // row start twice and broke click/drag selection below row 1.
                    let row_start = line
                        .layout
                        .caret_positions
                        .first()
                        .map(|caret| caret.start_offset)
                        .unwrap_or(0);
                    let line_index = line.layout.caret_index_for_x_unbounded(relative_x);
                    wrapped_row_absolute_index(line.abs_char_offset, row_start, line_index)
                }
            }
            _ => {
                let scroll_x = self.scroll_state.scroll_start().as_f32();
                let scrolled_click = vec2f(click_pos.x() + scroll_x, click_pos.y());
                let display_owned;
                let display: &str = if self.secure {
                    display_owned = self.display_text();
                    &display_owned
                } else {
                    &self.buffer
                };
                char_index_at_click_plain(
                    display,
                    scrolled_click,
                    bounds,
                    self.ui_font,
                    self.font_size(),
                    ctx.font_cache(),
                )
            }
        })
    }

    pub(crate) fn handle_gutter_click(
        &mut self,
        click_pos: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if self.source_path.is_none() {
            return false;
        }
        let Some(bounds) = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
        else {
            return false;
        };
        let relative_x = click_pos.x() - bounds.origin().x();
        let gutter_width = self.source_gutter_width();
        if !(0.0..=gutter_width).contains(&relative_x) {
            return false;
        }
        if self.git_diff_projection.is_some() {
            self.diff_comment_drag = self.diff_comment_line_at(click_pos, ctx);
            // Git gutters carry old/new coordinates and diff semantics. They
            // are not breakpoint/fold lanes and must not be interpreted as
            // source rows by the normal gutter handler.
            return true;
        }
        let relative_y = click_pos.y() - bounds.origin().y()
            + self.scroll_state.scroll_start().as_f32().max(0.0);
        let row = (relative_y.max(0.0) / self.line_height()).floor() as usize;
        if self.document_capabilities.large_file {
            let index = self.large_source_index();
            let line = index.line_for_char(
                index
                    .starts
                    .get(row.min(index.line_count().saturating_sub(1)))
                    .map(|(_, start)| *start)
                    .unwrap_or(0),
            ) + 1;
            if relative_x > EDITOR_BREAKPOINT_ZONE_WIDTH {
                return true;
            }
            let Some(path) = self.source_path.as_deref() else {
                return true;
            };
            if !self.source_language.supports_breakpoints() {
                return true;
            }
            self.breakpoints.toggle(path, line);
            ctx.emit(InputEditorEvent::BreakpointsChanged {
                breakpoints: self.breakpoints.breakpoints(path).collect(),
            });
            ctx.notify();
            return true;
        }
        let visual_lines = self.visual_line_map(self.layout_width.get(), ctx.font_cache());
        let fold_index = self
            .document_capabilities
            .folding
            .then(|| self.fold_index());
        let visible_indices = fold_index
            .as_ref()
            .map(|index| self.visible_visual_indices(&visual_lines, index, self.layout_width.get()))
            .unwrap_or_else(|| Rc::new((0..visual_lines.len()).collect()));
        let Some(line) = visible_indices
            .get(row)
            .and_then(|index| visual_lines.get(*index))
            .map(|line| line.source_line)
        else {
            return true;
        };
        if relative_x >= gutter_width - EDITOR_FOLD_ZONE_WIDTH
            && fold_index
                .as_ref()
                .is_some_and(|index| index.region_starting_at(line).is_some())
        {
            let index = fold_index
                .as_ref()
                .expect("fold index exists for fold control");
            self.fold_state.toggle(line, index);
            self.fold_visibility_cache.get_mut().take();
            ctx.notify();
            return true;
        }
        if !self.source_language.supports_breakpoints() {
            return true;
        }
        if relative_x > EDITOR_BREAKPOINT_ZONE_WIDTH {
            return false;
        }
        let Some(path) = self.source_path.as_deref() else {
            return false;
        };
        self.breakpoints.toggle(path, line);
        ctx.emit(InputEditorEvent::BreakpointsChanged {
            breakpoints: self.breakpoints.breakpoints(path).collect(),
        });
        ctx.notify();
        true
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.drag_anchor = None;
    }

    pub(crate) fn active_selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor.or(self.drag_anchor)?;
        let len = self.buffer.chars().count();
        let anchor = anchor.min(len);
        let cursor = self.cursor.min(len);
        if anchor == cursor {
            return None;
        }
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    pub(crate) fn context_menu_entries(&self) -> Vec<ContextMenuEntry> {
        let has_selection = self.active_selection_range().is_some();
        let editable = !matches!(self.mode, InputMode::Search | InputMode::Settings(_));
        let navigation = if self.document_capabilities.language_server {
            self.language_navigation_capabilities
        } else {
            LanguageNavigationCapabilities::default()
        };
        let is_markdown = self.source_path.as_ref().is_some_and(|path| {
            matches!(
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "mkdn"
            )
        });

        let mut entries = vec![
            ContextMenuEntry::Action {
                label: "Go to Definition",
                shortcut: Some("F12"),
                command: EditorContextMenuCommand::GoToDefinition,
                enabled: self.is_source_editor() && navigation.definition,
            },
            ContextMenuEntry::Action {
                label: "Go to Type Definition",
                shortcut: None,
                command: EditorContextMenuCommand::GoToTypeDefinition,
                enabled: self.is_source_editor() && navigation.type_definition,
            },
            ContextMenuEntry::Action {
                label: "Go to Source Definition",
                shortcut: None,
                command: EditorContextMenuCommand::GoToSourceDefinition,
                enabled: self.is_source_editor() && navigation.definition,
            },
            ContextMenuEntry::Action {
                label: "Go to Implementations",
                shortcut: Some("⌘ F12"),
                command: EditorContextMenuCommand::GoToImplementation,
                enabled: self.is_source_editor() && navigation.implementation,
            },
            ContextMenuEntry::Action {
                label: "Go to References",
                shortcut: Some("⇧ F12"),
                command: EditorContextMenuCommand::GoToReferences,
                enabled: self.is_source_editor() && navigation.references,
            },
            ContextMenuEntry::Action {
                label: "Add Symbol to Chat",
                shortcut: Some("⌘ L"),
                command: EditorContextMenuCommand::AddSymbolToChat,
                enabled: self.is_source_editor(),
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Action {
                label: "Find All References",
                shortcut: Some("⌥ ⇧ F12"),
                command: EditorContextMenuCommand::FindAllReferences,
                enabled: self.is_source_editor() && navigation.references,
            },
            ContextMenuEntry::Unavailable {
                label: "Find All Implementations",
                shortcut: None,
                submenu: false,
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Action {
                label: "Rename Symbol",
                shortcut: Some("F2"),
                command: EditorContextMenuCommand::RenameSymbol,
                enabled: self.is_source_editor() && self.document_capabilities.language_server,
            },
            ContextMenuEntry::Unavailable {
                label: "Change All Occurrences",
                shortcut: Some("⌘ F2"),
                submenu: false,
            },
            ContextMenuEntry::Action {
                label: "Format Document",
                shortcut: Some("⌥ ⇧ F"),
                command: EditorContextMenuCommand::FormatDocument,
                enabled: self.is_source_editor() && self.document_capabilities.language_server,
            },
            ContextMenuEntry::Action {
                label: "Source Action…",
                shortcut: Some(context_menu_shortcut("⌘ .", "Ctrl .")),
                command: EditorContextMenuCommand::ShowCodeActions,
                enabled: self.is_source_editor() && self.document_capabilities.language_server,
            },
            ContextMenuEntry::Action {
                label: "Toggle Line Comment",
                shortcut: Some(context_menu_shortcut("⌘ /", "Ctrl /")),
                command: EditorContextMenuCommand::ToggleLineComment,
                enabled: self.is_source_editor() && self.source_language.comment_style().is_some(),
            },
            ContextMenuEntry::Separator,
            ContextMenuEntry::Action {
                label: "Cut",
                shortcut: Some(context_menu_shortcut("⌘ X", "Ctrl X")),
                command: EditorContextMenuCommand::Cut,
                enabled: editable && has_selection,
            },
            ContextMenuEntry::Action {
                label: "Copy",
                shortcut: Some(context_menu_shortcut("⌘ C", "Ctrl C")),
                command: EditorContextMenuCommand::Copy,
                enabled: has_selection,
            },
            ContextMenuEntry::Action {
                label: "Paste",
                shortcut: Some(context_menu_shortcut("⌘ V", "Ctrl V")),
                command: EditorContextMenuCommand::Paste,
                // Clipboard contents are intentionally read only when the
                // command runs; reading the platform clipboard from render
                // would make painting impure and can block on native hosts.
                enabled: editable,
            },
        ];

        if !self.is_source_editor() {
            entries.retain(|entry| !matches!(entry, ContextMenuEntry::Unavailable { .. }));
        }

        if is_markdown || self.is_source_editor() {
            // Command Palette removed from the popover by user request.
            // Collect the tail first so no stray separator is left behind.
            let mut tail: Vec<ContextMenuEntry> = Vec::new();
            if is_markdown {
                tail.push(ContextMenuEntry::Action {
                    label: "Open Markdown Preview",
                    shortcut: None,
                    command: EditorContextMenuCommand::ToggleMarkdownPreview,
                    enabled: true,
                });
            }
            if !self.is_source_editor() {
                tail.push(ContextMenuEntry::Action {
                    label: "Open in Terminal",
                    shortcut: None,
                    command: EditorContextMenuCommand::ToggleDocumentTerminal,
                    enabled: true,
                });
            }
            if !tail.is_empty() {
                entries.push(ContextMenuEntry::Separator);
                entries.extend(tail);
            }
        }

        entries
    }

    pub(crate) fn next_context_menu_index(&self, direction: i32) -> usize {
        let entries = self.context_menu_entries();
        if entries.is_empty() {
            return 0;
        }
        let mut index = self.context_menu_selected.min(entries.len() - 1);
        for _ in 0..entries.len() {
            index = if direction.is_negative() {
                if index == 0 {
                    entries.len() - 1
                } else {
                    index - 1
                }
            } else {
                (index + 1) % entries.len()
            };
            if matches!(
                entries[index],
                ContextMenuEntry::Action { enabled: true, .. }
            ) {
                return index;
            }
        }
        self.context_menu_selected
    }

    pub(crate) fn selected_context_menu_command(&self) -> Option<EditorContextMenuCommand> {
        match self.context_menu_entries().get(self.context_menu_selected) {
            Some(ContextMenuEntry::Action {
                command,
                enabled: true,
                ..
            }) => Some(*command),
            _ => None,
        }
    }

    pub(crate) fn render_context_menu(&self) -> Box<dyn Element> {
        let entries = self.context_menu_entries();
        let mut rows = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);

        for (index, entry) in entries.into_iter().enumerate() {
            match entry {
                ContextMenuEntry::Separator => {
                    rows = rows.with_child(
                        Container::new(
                            ConstrainedBox::new(Empty::new().finish())
                                .with_height(1.0)
                                .finish(),
                        )
                        .with_margin_top(5.0)
                        .with_margin_bottom(5.0)
                        .with_margin_left(12.0)
                        .with_margin_right(12.0)
                        .with_background_color(self.theme.outline())
                        .finish(),
                    );
                }
                ContextMenuEntry::Unavailable {
                    label,
                    shortcut,
                    submenu,
                } => {
                    let shortcut = shortcut.unwrap_or_default();
                    let row = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_child(
                            Expanded::new(
                                1.0,
                                Text::new(label, self.menu_font, crate::theme::text_size(11.5))
                                    .with_color(self.theme.disabled_text())
                                    .soft_wrap(false)
                                    .finish(),
                            )
                            .finish(),
                        )
                        .with_child(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(8.0)
                                .with_child(
                                    Text::new(
                                        shortcut.to_string(),
                                        self.menu_font,
                                        crate::theme::text_size(10.0),
                                    )
                                    .with_color(self.theme.disabled_text())
                                    .soft_wrap(false)
                                    .finish(),
                                )
                                .with_child(if submenu {
                                    Text::new(">", self.menu_font, crate::theme::text_size(16.0))
                                        .with_color(self.theme.disabled_text())
                                        .finish()
                                } else {
                                    Empty::new().finish()
                                })
                                .finish(),
                        )
                        .finish();
                    rows = rows.with_child(
                        Container::new(row)
                            .with_padding_left(16.0)
                            .with_padding_right(16.0)
                            .with_padding_top(4.0)
                            .with_padding_bottom(4.0)
                            .finish(),
                    );
                }
                ContextMenuEntry::Action {
                    label,
                    shortcut,
                    command,
                    enabled,
                } => {
                    let mouse = self
                        .context_menu_mouse_states
                        .borrow_mut()
                        .entry(index)
                        .or_insert_with(MouseStateHandle::default)
                        .clone();
                    let selected = self.context_menu_selected == index;
                    let theme = self.theme;
                    let label_color = if enabled {
                        theme.main_text()
                    } else {
                        theme.disabled_text()
                    };
                    let shortcut_text = shortcut.unwrap_or_default().to_string();
                    let row = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_child(
                            Expanded::new(
                                1.0,
                                Text::new(label, self.menu_font, crate::theme::text_size(11.5))
                                    .with_color(label_color)
                                    .soft_wrap(false)
                                    .finish(),
                            )
                            .finish(),
                        )
                        .with_child(
                            Text::new(shortcut_text, self.menu_font, crate::theme::text_size(10.0))
                                .with_color(theme.sub_text())
                                .soft_wrap(false)
                                .finish(),
                        )
                        .finish();
                    let row = Hoverable::new(mouse, move |state| {
                        let highlighted = enabled && (selected || state.is_hovered());
                        Container::new(row)
                            .with_padding_left(16.0)
                            .with_padding_right(16.0)
                            .with_padding_top(4.0)
                            .with_padding_bottom(4.0)
                            .with_background_color(if highlighted {
                                context_menu_alpha(theme.accent, 0.16)
                            } else {
                                ColorU::new(0, 0, 0, 0)
                            })
                            .finish()
                    })
                    .on_click(move |event_ctx, _app, _position| {
                        if enabled {
                            event_ctx.dispatch_typed_action(InputEditorAction::ContextMenuCommand(
                                command,
                            ));
                        }
                    })
                    .with_cursor(if enabled {
                        warpui_core::platform::Cursor::PointingHand
                    } else {
                        warpui_core::platform::Cursor::Arrow
                    })
                    .finish();
                    rows = rows.with_child(row);
                }
            }
        }

        let scroll = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.context_menu_scroll.clone(),
                rows.finish(),
                ScrollbarWidth::Auto,
                self.theme.outline().into(),
                self.theme.sub_text().into(),
                self.theme.surface_1().into(),
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_max_height(560.0)
        .finish();
        ConstrainedBox::new(
            Container::new(scroll)
                .with_background_color(self.theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(self.theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish(),
        )
        .with_width(270.0)
        .finish()
    }

    pub(crate) fn execute_context_menu_command(
        &mut self,
        command: EditorContextMenuCommand,
        ctx: &mut ViewContext<Self>,
    ) {
        let context_cursor = self.context_menu_cursor.unwrap_or(self.cursor);
        self.context_menu_position = None;
        self.context_menu_cursor = None;
        self.context_menu_selected = 0;
        match command {
            EditorContextMenuCommand::GoToDefinition
            | EditorContextMenuCommand::GoToTypeDefinition
            | EditorContextMenuCommand::GoToSourceDefinition
            | EditorContextMenuCommand::GoToImplementation
            | EditorContextMenuCommand::GoToReferences
            | EditorContextMenuCommand::FindAllReferences
            | EditorContextMenuCommand::AddSymbolToChat
            | EditorContextMenuCommand::AddSymbolToNewChat
            | EditorContextMenuCommand::RenameSymbol
            | EditorContextMenuCommand::FormatDocument
            | EditorContextMenuCommand::ShowCodeActions => {
                ctx.emit(InputEditorEvent::ContextMenuCommand {
                    command,
                    cursor: context_cursor,
                });
            }
            EditorContextMenuCommand::ToggleLineComment => self.handle_toggle_line_comment(ctx),
            EditorContextMenuCommand::Undo => self.handle_undo(ctx),
            EditorContextMenuCommand::Redo => self.handle_redo(ctx),
            EditorContextMenuCommand::Cut => self.cut_selection(ctx),
            EditorContextMenuCommand::Copy => self.copy_selection(ctx),
            EditorContextMenuCommand::CopyAndTrim => self.copy_and_trim_selection(ctx),
            EditorContextMenuCommand::Paste => self.paste_clipboard(ctx),
            EditorContextMenuCommand::SelectAll => self.select_all(ctx),
            EditorContextMenuCommand::ToggleMarkdownPreview
            | EditorContextMenuCommand::ToggleDocumentTerminal
            | EditorContextMenuCommand::CommandPalette => {
                ctx.emit(InputEditorEvent::ContextMenuCommand {
                    command,
                    cursor: context_cursor,
                });
            }
        }
        ctx.notify();
    }

    pub(crate) fn copy_selection(&self, ctx: &mut ViewContext<Self>) {
        if let Some(text) = self.selected_text() {
            ctx.clipboard().write(ClipboardContent::plain_text(text));
        }
    }

    pub(crate) fn cut_selection(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(text) = self.selected_text() else {
            return;
        };
        ctx.clipboard().write(ClipboardContent::plain_text(text));
        if let Some((start, end)) = self.active_selection_range() {
            self.delete_range(start, end, ctx);
        }
    }

    pub(crate) fn paste_clipboard(&mut self, ctx: &mut ViewContext<Self>) {
        let text = ctx.clipboard().read().plain_text;
        if !text.is_empty() {
            self.insert_text_at_cursor(&text, ctx);
        }
    }

    pub fn select_all(&mut self, ctx: &mut ViewContext<Self>) {
        if self.buffer.is_empty() {
            return;
        }
        self.selection_anchor = Some(0);
        self.drag_anchor = None;
        self.cursor = self.buffer.chars().count();
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        ctx.notify();
    }

    pub(crate) fn copy_and_trim_selection(&self, ctx: &mut ViewContext<Self>) {
        let Some((start, end)) = self.active_selection_range() else {
            return;
        };
        let chars: Vec<char> = self.buffer.chars().collect();
        let selected: String = chars[start..end].iter().collect();
        let trimmed = trim_copy_indentation(&chars, start, end).unwrap_or(selected);
        ctx.clipboard().write(ClipboardContent::plain_text(trimmed));
    }
}
