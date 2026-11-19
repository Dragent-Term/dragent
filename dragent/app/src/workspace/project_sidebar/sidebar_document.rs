//! Sidebar document workspace and language navigation.
//!
//! Blocks here are impl ProjectSidebarView: same type, same behavior.

use super::detect;
use super::detect::QuickAction;
use super::model::{ErrorLevel, ProjectInfo, ProjectSidebarModel, SidebarEvent};
use super::sidebar_documents::{render_buffer_search_bar, render_document_breadcrumb};
use super::sidebar_explorer::{
    buffer_search_icon_button, document_action_button, render_language_icon,
};
use super::sidebar_search::render_project_search_multi;
use crate::icons::{render_icon, AppIcon, ICON_SIZE_SM};
use crate::process_ext::NoWindow;
use crate::terminal::render::markdown::render_markdown_document_with_links;
use crate::theme::Theme;
use crate::workspace::{
    editor_render,
    share_sheet::{avatar_sized, avatar_sized_from_source},
    EditorContextMenuCommand, InlineRenameState, InputEditor, InputEditorEvent, InputMode,
    LanguageNavigationCapabilities, LanguageNavigationKind, WorkspaceAction,
};
use dragent_editor::{
    find_buffer_matches, lsp_range_to_char_range, BufferSearchOptions, LineChangeKind, TextRange,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use warpui::color::ColorU;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance,
};
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, DraggableState, Element, Empty,
    EventHandler, Expanded, Fill, Flex, Highlight, Image, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Point, Resizable,
    ResizableStateHandle, SavePosition, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::r#async::Timer;
use warpui::{
    AfterLayoutContext, AppContext, Entity, EntityId, EventContext, LayoutContext, ModelHandle,
    PaintContext, SizeConstraint, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::assets::asset_cache::{AssetSource, AsyncAssetId, AsyncAssetType};
use warpui_core::elements::{ui_line_height_ratio, Clipped, Hoverable, MouseStateHandle};
use warpui_core::event::DispatchedEvent;
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;
use warpui_core::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui_core::ui_components::tool_tip::Tooltip;
use warpui_core::units::Pixels;
use web_time::Instant;

use super::sidebar_gitdiff_file::render_git_file_diff;
use super::sidebar_gitdiff_multi::render_git_multi_diff;
use super::sidebar_githistory::{fetch_git_history, render_git_panel_view};
use super::sidebar_panels::with_alpha;
use super::view::ProjectSidebarView;
use super::view::{
    is_markdown_path, source_line_changes_for_model, GitHistoryState, GitPanelMode,
    LanguageLocationItem, SidebarAction, META_SIZE, MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT,
    MULTI_FILE_SEARCH_OVERSCAN_ROWS,
};

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct EditorUiState {
    files:
        std::collections::HashMap<PathBuf, crate::workspace::input_editor::model::EditorViewState>,
    sidebar_tab: super::SidebarTab,
    explorer_scroll: f32,
    history: bool,
    staged_collapsed: bool,
    changes_collapsed: bool,
}

impl ProjectSidebarView {
    pub(crate) fn persist_editor_ui(&self, app: &warpui::AppContext) -> String {
        let mut states = self.editor_view_states.clone();
        if let Some(key) = &self.editor_document_key {
            states.insert(
                key.clone(),
                self.document_editor
                    .read(app, |editor, _| editor.capture_view_state()),
            );
        }
        serde_json::to_string(&EditorUiState {
            files: states,
            sidebar_tab: self.sidebar_tab,
            explorer_scroll: self.scroll.scroll_start().as_f32(),
            history: self.git_panel_mode == GitPanelMode::History,
            staged_collapsed: self.git_staged_collapsed,
            changes_collapsed: self.git_changes_collapsed,
        })
        .unwrap_or_default()
    }

    pub(crate) fn restore_editor_ui(&mut self, json: &str) {
        if let Ok(state) = serde_json::from_str::<EditorUiState>(json) {
            self.editor_view_states = state.files;
            self.sidebar_tab = state.sidebar_tab;
            self.scroll.scroll_to(Pixels::new(state.explorer_scroll));
            self.git_panel_mode = if state.history {
                GitPanelMode::History
            } else {
                GitPanelMode::Changes
            };
            self.git_staged_collapsed = state.staged_collapsed;
            self.git_changes_collapsed = state.changes_collapsed;
        }
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        for editor in [&self.diff_comment_editor, &self.diff_comment_search] {
            ctx.update_view(editor, |input, ec| input.set_theme(theme, ec));
        }
        // The document editor is a child view with its own cached typography,
        // syntax palette, caret, selection, and scrollbar colors. Updating only
        // this sidebar would repaint the surrounding background while leaving
        // the opened file in the previous theme.
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.buffer_search_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.buffer_replace_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.language_rename_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.git_commit_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.global_search_query_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.global_search_replace_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.global_search_include_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.update_view(&self.global_search_exclude_editor, |editor, editor_ctx| {
            editor.set_theme(theme, editor_ctx);
        });
        ctx.notify();
    }

    pub fn sync_editor_typography(
        &mut self,
        ui_font: FamilyId,
        mono_font: FamilyId,
        ctx: &mut ViewContext<Self>,
    ) {
        self.ui_font = ui_font;
        self.mono_font = mono_font;
        for editor in [&self.diff_comment_editor, &self.diff_comment_search] {
            ctx.update_view(editor, |input, ec| { input.set_font(ui_font, crate::theme::text_size(13.0), ec); input.set_menu_font(ui_font, ec); });
        }
        let size = crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE);
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            // Source buffers use their independent editor metric. A
            // terminal font-size change must not desynchronise text rows from
            // the source editor's gutter, cursor and guide geometry.
            editor.set_font(mono_font, size, editor_ctx);
            editor.set_menu_font(ui_font, editor_ctx);
        });
        ctx.update_view(&self.git_commit_editor, |editor, editor_ctx| {
            editor.set_font(mono_font, crate::theme::scaled(11.5), editor_ctx);
            editor.set_menu_font(ui_font, editor_ctx);
        });
        for editor in [
            &self.language_rename_editor,
            &self.buffer_search_editor,
            &self.buffer_replace_editor,
            &self.git_filter_editor,
            &self.global_search_query_editor,
            &self.global_search_replace_editor,
            &self.global_search_include_editor,
            &self.global_search_exclude_editor,
        ] {
            ctx.update_view(editor, |input, input_ctx| {
                let size = input.font_size();
                input.set_font(mono_font, size, input_ctx);
                input.set_menu_font(ui_font, input_ctx);
            });
        }
        ctx.notify();
    }

    pub fn set_document_editor_text(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_text(text, editor_ctx);
        });
        ctx.notify();
    }

    pub fn reload_saved_document_text(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.reload_saved_text(text, editor_ctx);
        });
        if self.buffer_search_open {
            self.refresh_buffer_search(true, ctx);
        }
        ctx.notify();
    }

    pub fn set_language_navigation_capabilities(
        &mut self,
        capabilities: LanguageNavigationCapabilities,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_language_navigation_capabilities(capabilities, editor_ctx);
        });
        ctx.notify();
    }

    pub fn set_definition_hover_result(
        &mut self,
        request_id: u64,
        available: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_definition_hover_result(request_id, available, editor_ctx);
        });
    }

    pub fn set_document_editor_cursor(&mut self, cursor: usize, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_cursor_char_index(cursor, editor_ctx);
            editor.reveal_cursor(editor_ctx);
        });
        ctx.notify();
    }

    /// Apply an LSP URI+range only after the destination model buffer exists.
    /// This method deliberately synchronizes the reusable editor view from the
    /// active model before converting UTF-16 coordinates, so cross-file opens
    /// cannot navigate against the previous file's text.
    pub fn navigate_active_document_to_range(
        &mut self,
        relative_path: &Path,
        range: TextRange,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(text) = self.model.read(ctx, |model, _| {
            model.buffer.as_ref().and_then(|buffer| {
                (buffer.document().relative_path == relative_path)
                    .then(|| buffer.text().to_string())
            })
        }) else {
            return false;
        };
        let mut char_range = lsp_range_to_char_range(&text, range);
        // Language servers report the ENTIRE document as the definition range
        // for a module specifier ("./a/b/c"). Revealing such a span would
        // highlight the whole file, so collapse any multi-line target to a
        // cursor at its start — the navigation goal is "open this file", not
        // "select everything in it".
        let spans_multiple_lines = text
            .chars()
            .take(char_range.end.min(text.chars().count()))
            .filter(|ch| *ch == '\n')
            .count()
            > text
                .chars()
                .take(char_range.start.min(text.chars().count()))
                .filter(|ch| *ch == '\n')
                .count();
        if spans_multiple_lines {
            char_range = char_range.start..char_range.start;
        }
        let path = relative_path.to_path_buf();
        let needs_document_sync = self.editor_document_path.as_deref() != Some(relative_path);
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            if needs_document_sync {
                editor.set_document_path(Some(relative_path), editor_ctx);
                editor.set_text(&text, editor_ctx);
            }
            editor.navigate_to_char_range(char_range, editor_ctx);
            editor_ctx.focus_self();
        });
        self.editor_document_path = Some(path);
        self.markdown_preview_open = false;
        ctx.notify();
        true
    }

    pub fn replace_document_editor_text_with_undo(
        &mut self,
        text: &str,
        cursor: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.replace_text_with_undo(text, cursor, editor_ctx);
        });
        ctx.notify();
    }

    pub fn set_document_completion(&mut self, suffix: Option<String>, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_ghost_suffix(suffix, editor_ctx);
        });
    }

    pub fn document_text_and_cursor(&self, app: &AppContext) -> Option<(String, usize)> {
        self.document_editor.read(app, |editor, _| {
            Some((editor.get_text().to_string(), editor.cursor_char_index()))
        })
    }

    pub fn buffer_search_open(&self) -> bool {
        self.buffer_search_open
    }

    pub fn buffer_replace_focused(&self, app: &AppContext) -> bool {
        self.buffer_replace_open
            && self
                .buffer_replace_editor
                .read(app, |editor, app| editor.is_focused(app))
    }

    pub fn open_buffer_search(&mut self, seed_selection: bool, ctx: &mut ViewContext<Self>) {
        self.buffer_search_open = true;
        if seed_selection {
            let selected = self
                .document_editor
                .read(ctx, |editor, _| editor.selected_text());
            if let Some(selected) = selected {
                ctx.update_view(&self.buffer_search_editor, |editor, editor_ctx| {
                    editor.set_text(&selected, editor_ctx);
                });
            }
        }
        // Show and focus the bar FIRST so the very next frames paint it; the
        // document-wide scan is deferred below so opening the filter never
        // blocks the frame that reveals it. A zero timer can resolve before
        // the redraw runs, so wait long enough that the bar is definitely on
        // screen before scanning (50ms is imperceptible).
        ctx.update_view(&self.buffer_search_editor, |_, editor_ctx| {
            editor_ctx.focus_self();
        });
        ctx.notify();
        ctx.spawn_abortable(
            Timer::after(std::time::Duration::from_millis(50)),
            |view, _, ctx| view.refresh_buffer_search_at_caret(ctx),
            |_, _| {},
        );
    }

    /// Refresh matches and activate the first match at/after the caret
    /// WITHOUT scrolling — opening Find must not yank the editor away from
    /// the line the user is on (Enter then advances from that match).
    pub(crate) fn refresh_buffer_search_at_caret(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.buffer_search_open {
            return;
        }
        let cursor = self
            .document_editor
            .read(ctx, |editor, _| editor.cursor_char_index());
        let query = self
            .buffer_search_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        // Scan inside the read closure so the document text never has to be
        // cloned out of the editor.
        let found = self.document_editor.read(ctx, |editor, _| {
            find_buffer_matches(editor.get_text(), &query, self.buffer_search_options)
        });
        match found {
            Ok(matches) => {
                self.buffer_search_matches = matches
                    .into_iter()
                    .map(|buffer_match| buffer_match.range)
                    .collect();
                self.buffer_search_error = None;
                // Activate the first match at/after the caret; wrap to the
                // first match when the caret sits after the last one.
                self.buffer_search_active = if self.buffer_search_matches.is_empty() {
                    None
                } else {
                    Some(
                        self.buffer_search_matches
                            .iter()
                            .position(|range| range.start >= cursor)
                            .unwrap_or(0),
                    )
                };
            }
            Err(error) => {
                self.buffer_search_matches.clear();
                self.buffer_search_active = None;
                self.buffer_search_error = Some(error);
            }
        }
        self.apply_active_buffer_match(false, ctx);
        ctx.notify();
    }

    pub fn close_buffer_search(&mut self, ctx: &mut ViewContext<Self>) {
        self.buffer_search_open = false;
        self.buffer_replace_open = false;
        self.buffer_search_matches.clear();
        self.buffer_search_active = None;
        self.buffer_search_error = None;
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_search_matches(Vec::new(), None, editor_ctx);
            editor_ctx.focus_self();
        });
        ctx.notify();
    }

    pub fn navigate_buffer_search(&mut self, previous: bool, ctx: &mut ViewContext<Self>) {
        let count = self.buffer_search_matches.len();
        if count == 0 {
            return;
        }
        self.buffer_search_active = Some(match self.buffer_search_active {
            Some(0) if previous => count - 1,
            Some(index) if previous => index - 1,
            Some(index) => (index + 1) % count,
            None if previous => count - 1,
            None => 0,
        });
        self.apply_active_buffer_match(true, ctx);
    }

    pub fn toggle_buffer_search_case(&mut self, ctx: &mut ViewContext<Self>) {
        self.buffer_search_options.case_sensitive = !self.buffer_search_options.case_sensitive;
        self.refresh_buffer_search_at_caret(ctx);
    }

    pub fn toggle_buffer_search_whole_word(&mut self, ctx: &mut ViewContext<Self>) {
        self.buffer_search_options.whole_word = !self.buffer_search_options.whole_word;
        self.refresh_buffer_search_at_caret(ctx);
    }

    pub fn toggle_buffer_search_regex(&mut self, ctx: &mut ViewContext<Self>) {
        self.buffer_search_options.regex = !self.buffer_search_options.regex;
        self.refresh_buffer_search_at_caret(ctx);
    }

    pub fn toggle_buffer_search_replace(&mut self, ctx: &mut ViewContext<Self>) {
        self.buffer_replace_open = !self.buffer_replace_open;
        if self.buffer_replace_open {
            ctx.update_view(&self.buffer_replace_editor, |_, editor_ctx| {
                editor_ctx.focus_self();
            });
        } else {
            ctx.update_view(&self.buffer_search_editor, |_, editor_ctx| {
                editor_ctx.focus_self();
            });
        }
        ctx.notify();
    }

    pub fn replace_buffer_search_match(&mut self, replace_all: bool, ctx: &mut ViewContext<Self>) {
        let ranges = if replace_all {
            self.buffer_search_matches.clone()
        } else {
            self.buffer_search_active
                .and_then(|index| self.buffer_search_matches.get(index).cloned())
                .into_iter()
                .collect()
        };
        if ranges.is_empty() {
            return;
        }
        let replacement = self
            .buffer_replace_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.replace_char_ranges(&ranges, &replacement, editor_ctx);
        });
    }

    pub(crate) fn refresh_buffer_search(
        &mut self,
        select_first: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let text = self
            .document_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        self.refresh_buffer_search_for_text(&text, select_first, ctx);
    }

    pub(crate) fn refresh_buffer_search_for_text(
        &mut self,
        text: &str,
        select_first: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let query = self
            .buffer_search_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        match find_buffer_matches(text, &query, self.buffer_search_options) {
            Ok(matches) => {
                self.buffer_search_matches = matches
                    .into_iter()
                    .map(|buffer_match| buffer_match.range)
                    .collect();
                self.buffer_search_error = None;
                self.buffer_search_active = if self.buffer_search_matches.is_empty() {
                    None
                } else if select_first {
                    Some(0)
                } else {
                    self.buffer_search_active
                        .map(|index| index.min(self.buffer_search_matches.len() - 1))
                        .or(Some(0))
                };
            }
            Err(error) => {
                self.buffer_search_matches.clear();
                self.buffer_search_active = None;
                self.buffer_search_error = Some(error);
            }
        }
        // Editing the document must only refresh decorations. Re-selecting the
        // active match here moves the caret and scrolls back to the Find result
        // on every keystroke, even when the user is typing elsewhere.
        self.apply_active_buffer_match(select_first, ctx);
        ctx.notify();
    }

    pub(crate) fn apply_active_buffer_match(
        &self,
        reveal_active_match: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let matches = self.buffer_search_matches.clone();
        let active = self.buffer_search_active;
        let active_range = active.and_then(|index| matches.get(index).cloned());
        ctx.update_view(&self.document_editor, |editor, editor_ctx| {
            editor.set_search_matches(matches, active, editor_ctx);
            if reveal_active_match {
                if let Some(range) = active_range {
                    editor.select_char_range(range, editor_ctx);
                }
            }
        });
        ctx.notify();
    }

    pub fn render_document_workspace(
        &self,
        app: &AppContext,
        theme: &Theme,
        ui: FamilyId,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        self.model.read(app, |model, _| {
            let Some(preview) = &model.preview else {
                return Align::new(
                    Text::new("Loading document…", ui, crate::theme::text_size(14.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .finish();
            };
            let is_dirty = !model.document_diff.is_empty();
            let is_git_diff = model.git_file_diff.is_some()
                || model.git_multi_diff.is_some()
                || model.git_diff_loading;
            let is_search_results = model.global_search_results.is_some();
            let is_review_surface = is_git_diff || is_search_results;
            let is_markdown = is_markdown_path(&preview.relative_path);
            // Editor settings, read live so changes apply without reopening.
            let editor_prefs = crate::console::editor_settings::EditorSettings::global().editor.clone();
            let review_tools = editor_prefs.markdown_review_tools;
            let spellcheck = editor_prefs.spellcheck_enabled;
            let word_wrap = editor_prefs.word_wrap;
            let minimap = editor_prefs.minimap_enabled;
            let _ = spellcheck;
            let mut header_actions = Flex::row().with_spacing(6.0);
            // Markdown preview affordance.
            if is_markdown && !is_review_surface && review_tools {
                header_actions = header_actions.with_child(buffer_search_icon_button(
                    if self.markdown_preview_open {
                        AppIcon::Edit
                    } else {
                        AppIcon::Eye
                    },
                    if self.markdown_preview_open {
                        "Edit Markdown"
                    } else {
                        "Open Markdown preview"
                    },
                    "markdown_preview",
                    WorkspaceAction::ToggleMarkdownPreview,
                    theme,
                    self,
                ));
            }
            if !self.markdown_preview_open && !is_review_surface {
                header_actions = header_actions.with_child(document_action_button(
                    AppIcon::Search,
                    "buffer_search",
                    WorkspaceAction::OpenBufferSearch {
                        seed_selection: false,
                    },
                    if self.buffer_search_open {
                        theme.accent
                    } else {
                        theme.sub_text()
                    },
                    self,
                ));
            }
            if is_dirty {
                header_actions = header_actions.with_child(document_action_button(
                    AppIcon::Save,
                    "save",
                    WorkspaceAction::SaveOpenDocument,
                    theme.accent,
                    self,
                ));
            }
            let header_actions = header_actions
                .with_child(document_action_button(
                    AppIcon::XClose,
                    "close_document",
                    WorkspaceAction::CloseOpenDocument,
                    theme.sub_text(),
                    self,
                ))
                .finish();
            let header = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Expanded::new(
                        1.0,
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(4.0)
                            // Flex measures ordinary row children with an
                            // unbounded horizontal constraint. The breadcrumb
                            // itself contains an expanding path segment, so it
                            // must receive the row's finite remaining width.
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    render_document_breadcrumb(
                                        model,
                                        &preview.relative_path,
                                        mono,
                                        theme,
                                    ),
                                )
                                .finish(),
                            )
                            .with_child(if is_dirty {
                                Text::new("●", ui, crate::theme::text_size(10.0))
                                    .with_color(theme.ui_yellow())
                                    .finish()
                            } else {
                                Empty::new().finish()
                            })
                            .with_child(if model.git_diff_loading {
                                Text::new(
                                    "Updating diff…".to_string(),
                                    ui,
                                    crate::theme::text_size(11.0),
                                )
                                .with_color(theme.sub_text())
                                .finish()
                            } else {
                                Empty::new().finish()
                            })
                            .finish(),
                    )
                    .finish(),
                )
                // Save/close are non-flex children and therefore retain their
                // full hit targets. Only the breadcrumb receives the remaining
                // width and ellipsizes underneath that fixed action area.
                .with_child(header_actions)
                .finish();

            let has_retained_git_surface = model.git_file_diff.is_some()
                || model.git_multi_diff.is_some()
                || model.buffer.is_some();
            let body = if let Some(results) = &model.global_search_results {
                render_project_search_multi(
                    results,
                    model.workspace_root(),
                    self.global_search_editor_scroll.clone(),
                    self.global_search_editor_horizontal_scroll.clone(),
                    self.workspace_view_id,
                    self.view_id,
                    &self.global_search_collapsed,
                    self.global_search_active.as_ref(),
                    ui,
                    mono,
                    theme,
                )
            } else if model.git_diff_loading && !has_retained_git_surface {
                Align::new(
                    Text::new(
                        "Loading changes…".to_string(),
                        ui,
                        crate::theme::text_size(13.0),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .finish()
            } else if let Some(diff) = model
                .git_file_diff
                .as_ref()
                .filter(|_| model.git_multi_diff.is_none())
            {
                if diff.target == crate::git_status::diff_head::GitDiffTarget::Unstaged
                    && model.buffer.is_some()
                    && !diff.binary
                    && self.comment_for_file(&diff.relative_path, diff.target).is_none()
                {
                    Container::new(ChildView::new(&self.document_editor).finish())
                        .with_padding_left(4.0)
                        .with_padding_top(12.0)
                        .with_padding_bottom(0.0)
                        .with_padding_right(0.0)
                        .with_background_color(theme.background)
                        .finish()
                } else {
                    let git_panel = self.git_panel.borrow();
                    let term_id = git_panel.terminal_view_id;
                    let git_root = git_panel
                        .cwd
                        .clone()
                        .unwrap_or_else(|| model.workspace_root().to_path_buf());
                    render_git_file_diff(
                        diff,
                        self,
                        &git_root,
                        self.git_diff_scroll.clone(),
                        self.view_id,
                        self.workspace_view_id,
                        term_id,
                        self.git_diff_side_by_side,
                        self.git_diff_show_full_file,
                        &self.git_diff_expanded_regions,
                        self.git_diff_active_hunk,
                        ui,
                        mono,
                        theme,
                    )
                }
            } else if let Some(diff) = &model.git_multi_diff {
                // The document editor is shared by every editable card. While
                // switching files, the model can advance one frame before the
                // child editor has installed the new projection. Never size a
                // card using rows that belong to the previously active file.
                let active_projected_rows = model.git_file_diff.as_ref().and_then(|active| {
                    (self.editor_document_path.as_deref() == Some(active.relative_path.as_path()))
                        .then(|| {
                            self.document_editor
                                .read(app, |editor, _| editor.git_diff_projected_row_count())
                        })
                        .flatten()
                });
                let git_panel = self.git_panel.borrow();
                let multi_diff_root = model
                    .git_review_root()
                    .or(git_panel.cwd.as_deref())
                    .unwrap_or(model.workspace_root());
                render_git_multi_diff(
                    diff,
                    self,
                    multi_diff_root,
                    model.git_diff_loading,
                    self.git_diff_scroll.clone(),
                    self.git_multi_horizontal_scroll.clone(),
                    self.workspace_view_id,
                    self.view_id,
                    &self.git_multi_collapsed,
                    &self.git_multi_hunks_collapsed,
                    model.git_file_diff.as_ref(),
                    active_projected_rows,
                    &self.document_editor,
                    ui,
                    mono,
                    theme,
                )
            } else if let Some(error) = &preview.error {
                Align::new(
                    Text::new(error.clone(), ui, crate::theme::text_size(14.0))
                        .with_color(theme.ui_red())
                        .finish(),
                )
                .finish()
            } else if let Some(path) = &preview.visual_path {
                // Keep visual assets on disk and let the GPU image cache decode
                // them; do not copy binary data into the text buffer.
                Align::new(
                    Container::new(
                        Image::new(
                            AssetSource::LocalFile {
                                path: path.to_string_lossy().into_owned(),
                            },
                            CacheOption::BySize,
                        )
                        .contain()
                        .finish(),
                    )
                    .with_padding_left(24.0)
                    .with_padding_right(24.0)
                    .with_padding_top(24.0)
                    .with_padding_bottom(24.0)
                    .finish(),
                )
                .finish()
            } else {
                // With review tools off, markdown opens as source.
                if self.markdown_preview_open && is_markdown && review_tools {
                    let markdown_text = model
                        .buffer
                        .as_ref()
                        .map(|buffer| buffer.text().to_string())
                        .unwrap_or_else(|| preview.text.clone());
                    let source_path = preview.relative_path.clone();
                    // Rich link rendering
                    // carries the spellcheck-adjacent affordances; off renders
                    // plain text (no link styling) until the squiggle engine
                    // lands. `word_wrap` off removes the reading-column cap so
                    // long lines scroll horizontally instead of wrapping.
                    let rendered: Box<dyn Element> = if spellcheck {
                        render_markdown_document_with_links::<WorkspaceAction, _>(
                            &markdown_text,
                            *theme,
                            ui,
                            mono,
                            crate::theme::scaled(14.0),
                            1.55,
                            self.workspace_view_id,
                            move |target| WorkspaceAction::OpenMarkdownLink {
                                source_path: source_path.clone(),
                                target,
                            },
                        )
                    } else {
                        warpui::elements::Text::new(
                            markdown_text,
                            ui,
                            crate::theme::text_size(14.0),
                        )
                        .with_color(theme.main_text())
                        .finish()
                    };
                    // The Markdown renderer contains table rows with flex
                    // children. Give it a finite reading-column width before
                    // putting it in the vertical scroller; otherwise a
                    // scroll view's unbounded measurement reaches those rows
                    // and warpui correctly rejects an infinite max flex.
                    // NOTE: source-editor grid layout is single-row-per-line
                    // (see input_editor/scroll.rs), so `word_wrap` currently
                    // applies to these markdown/wrapped surfaces; the source
                    // editor keeps horizontal scroll until its layout pass
                    // lands. The flag is still persisted and read here.
                    let rendered_document = Container::new(rendered)
                        .with_padding_left(28.0)
                        .with_padding_right(28.0)
                        .with_padding_top(22.0)
                        .with_padding_bottom(36.0)
                        .finish();
                    let rendered_document: Box<dyn Element> = if word_wrap {
                        Align::new(
                            ConstrainedBox::new(rendered_document)
                                .with_max_width(920.0)
                                .finish(),
                        )
                        .top_center()
                        .finish()
                    } else {
                        rendered_document
                    };
                    let rendered = ClippedScrollable::vertical(
                        self.markdown_preview_scroll.clone(),
                        rendered_document,
                        ScrollbarWidth::Auto,
                        theme.disabled_text().into(),
                        theme.sub_text().into(),
                        theme.panel_background().into(),
                    )
                    .finish();
                    rendered
                } else {
                    // Thin overview rail beside the
                    // source editor. No syntax sampling yet — a proportional
                    // rail that tracks scroll position would live here; the
                    // toggle is persisted and visibly adds/removes the rail.
                    let editor_body = Container::new(ChildView::new(&self.document_editor).finish())
                        .with_padding_left(4.0)
                        // Keep the editor's overlay scrollbar directly against
                        // the document wall instead of nesting it in panel padding.
                        .with_padding_top(12.0)
                        .with_padding_bottom(0.0)
                        .with_padding_right(0.0)
                        .with_background_color(theme.background)
                        .finish();
                    if minimap {
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_child(Expanded::new(1.0, editor_body).finish())
                            .with_child(
                                ConstrainedBox::new(
                                    Container::new(Empty::new().finish())
                                        .with_background_color(theme.surface_2())
                                        .with_border(Border::left(1.0).with_border_color(theme.outline()))
                                        .finish(),
                                )
                                .with_width(48.0)
                                .finish(),
                            )
                            .finish()
                    } else {
                        editor_body
                    }
                }
            };

            let mut column = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max);
            // Shared `nav_header_bar` geometry: the 26px action buttons and
            // breadcrumb center in the bar instead of padding it taller, so
            // this divider joins the terminal navbar's line exactly.
            column = column.with_child(crate::workspace::ide_chrome::nav_header_bar(
                header,
                theme,
                16.0,
                16.0,
                crate::theme::PANEL_HEADER_HEIGHT,
            ));
            if self.buffer_search_open {
                column = column.with_child(render_buffer_search_bar(self, theme, ui, mono));
            }
            column = column.with_child(Expanded::new(1.0, body).finish());

            if let Some((success, message)) = &model.document_notice {
                column = column.with_child(
                    Container::new(
                        Text::new(message.clone(), ui, crate::theme::text_size(META_SIZE))
                            .with_color(if *success {
                                theme.ui_green()
                            } else {
                                theme.ui_red()
                            })
                            .finish(),
                    )
                    .with_padding_left(16.0)
                    .with_padding_bottom(8.0)
                    .finish(),
                );
            }
            // Keep the editor viewport geometry stable while typing. A diff is
            // working-copy state, not inline editor chrome: mounting a full
            // change-review panel on the first keystroke reduced the editor's
            // allocated height and made the document visibly jump upward.
            // Diff/review will be opened explicitly in its own surface.
            let document_surface: Box<dyn Element> = Container::new(column.finish())
                .with_background_color(theme.background)
                .finish();
            let mut document_surface = document_surface;
            // Source-action affordances.
            if review_tools && !self.language_code_actions.is_empty() {
                let mut rows = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_spacing(2.0);
                for (index, (title, action)) in self.language_code_actions.iter().enumerate() {
                    let enabled = action.get("edit").is_some();
                    let label = title.clone();
                    let action = action.clone();
                    let mouse = self
                        .document_action_mouse_states
                        .borrow_mut()
                        .entry(format!("language_code_action_{index}"))
                        .or_insert_with(MouseStateHandle::default)
                        .clone();
                    let workspace_view_id = self.workspace_view_id;
                    rows = rows.with_child(
                        Hoverable::new(mouse, move |state| {
                            Container::new(
                                Text::new(label.clone(), mono, crate::theme::text_size(11.5))
                                    .with_color(if enabled {
                                        theme.main_text()
                                    } else {
                                        theme.disabled_text()
                                    })
                                    .soft_wrap(false)
                                    .finish(),
                            )
                            .with_padding_left(10.0)
                            .with_padding_right(10.0)
                            .with_padding_top(7.0)
                            .with_padding_bottom(7.0)
                            .with_background_color(if enabled && state.is_hovered() {
                                with_alpha(theme.accent, 0.14)
                            } else {
                                ColorU::new(0, 0, 0, 0)
                            })
                            .finish()
                        })
                        .on_click(move |ctx, _app, _position| {
                            if enabled {
                                ctx.dispatch_typed_action_for(
                                    workspace_view_id,
                                    WorkspaceAction::ApplyLanguageCodeAction {
                                        action: action.clone(),
                                    },
                                );
                            }
                        })
                        .with_cursor(if enabled {
                            Cursor::PointingHand
                        } else {
                            Cursor::Arrow
                        })
                        .finish(),
                    );
                }
                let panel = ConstrainedBox::new(
                    Container::new(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_spacing(6.0)
                            .with_child(
                                Text::new("Source actions", ui, crate::theme::text_size(11.0))
                                    .with_color(theme.sub_text())
                                    .finish(),
                            )
                            .with_child(rows.finish())
                            .finish(),
                    )
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(8.0)
                    .with_background_color(theme.surface_2())
                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .finish(),
                )
                .with_max_width(520.0)
                .finish();
                document_surface = Stack::new()
                    .with_child(document_surface)
                    .with_child(Align::new(panel).finish())
                    .finish();
            }

            if !self.language_locations.is_empty() {
                // The references sheet is an overlay, not another document
                // column child. Keeping it inside this bounded Stack prevents
                // it from consuming layout height (which previously left a
                // large blank band below the editor) and gives its scrollable
                // list a real viewport to paint into.
                let references_panel = Resizable::new(
                    self.language_locations_panel_state.clone(),
                    self.render_language_locations_panel(theme, ui, mono),
                )
                .with_dragbar_side(warpui::elements::DragBarSide::Top)
                .with_initial_fraction(0.30)
                .with_dragbar_color(theme.accent_overlay().into())
                .on_resize(|ctx, _| ctx.notify())
                .with_bounds_callback(Box::new(|window_size| {
                    let min_height = 140.0;
                    let max_height = (window_size.y() * 0.65).max(min_height);
                    (min_height, max_height)
                }))
                .finish();
                let pinned_to_bottom = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(references_panel)
                    .finish();
                document_surface = Stack::new()
                    .with_child(document_surface)
                    .with_child(pinned_to_bottom)
                    .finish();
            }
            document_surface
        })
    }
    pub fn render_workspace_tabs(
        &self,
        tabs: Vec<crate::workspace::editor_tabs::HeaderTab>,
        tabs_scroll: ClippedScrollStateHandle,
        theme: &Theme,
        font: FamilyId,
    ) -> Box<dyn Element> {
        crate::workspace::editor_tabs::render(
            tabs,
            theme,
            font,
            self.workspace_view_id,
            &self.document_action_mouse_states,
            &self.document_tab_drag_states,
            tabs_scroll,
        )
    }

    /// References peek panel, docked at the bottom of the editor. The design
    /// combines the two references: VS Code's docked peek chrome (header with
    /// title + result summary, line previews with the match highlighted,
    /// click/Escape interaction) with a file-grouped result list (file
    /// headers with icon, muted directory path and match-count badges) —
    /// rendered in this app's own visual language.
    pub(crate) fn render_language_locations_panel(
        &self,
        theme: &Theme,
        ui: FamilyId,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let total = self.language_locations.len();
        let navigation_root = self.language_locations_root.clone().unwrap_or_default();

        // Group references by file, preserving first-appearance order.
        let mut groups: Vec<(std::path::PathBuf, Vec<usize>)> = Vec::new();
        let mut group_index: HashMap<&Path, usize> = HashMap::new();
        for (index, location) in self.language_locations.iter().enumerate() {
            match group_index.get(location.relative_path.as_path()) {
                Some(&group) => groups[group].1.push(index),
                None => {
                    group_index.insert(location.relative_path.as_path(), groups.len());
                    groups.push((location.relative_path.clone(), vec![index]));
                }
            }
        }
        let file_count = groups.len();
        let summary = format!(
            "{total} {} in {file_count} {}",
            if total == 1 { "result" } else { "results" },
            if file_count == 1 { "file" } else { "files" },
        );

        // ── Header ────────────────────────────────────────────────────────
        let workspace_view_id = self.workspace_view_id;
        let close_mouse = self
            .document_action_mouse_states
            .borrow_mut()
            .entry("language_locations_close".to_string())
            .or_insert_with(MouseStateHandle::default)
            .clone();
        let header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(
                        Text::new(
                            self.language_locations_title
                                .clone()
                                .unwrap_or_else(|| "References".to_string()),
                            ui,
                            crate::theme::text_size(11.5),
                        )
                        .with_color(theme.accent)
                        .finish(),
                    )
                    .with_child(
                        Text::new(summary, ui, crate::theme::text_size(10.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_child(
                Hoverable::new(close_mouse, |state| {
                    Container::new(render_icon(
                        AppIcon::XClose,
                        if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        },
                        12.0,
                    ))
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::DismissLanguageResults,
                    );
                })
                .finish(),
            );

        // ── Grouped results ───────────────────────────────────────────────
        // MainAxisSize::Min is REQUIRED: inside the scrollable the column gets
        // an unbounded main axis, and the default Max sizing collapses it to
        // zero height (empty panel strip).
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(0.0);
        for (path, indices) in &groups {
            let first = &self.language_locations[indices[0]];
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_string();
            let dir_text = path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default();
            let count = indices.len();
            let group_mouse = self
                .document_action_mouse_states
                .borrow_mut()
                .entry(format!("language_group_{}", path.display()))
                .or_insert_with(MouseStateHandle::default)
                .clone();
            let nav_path = path.clone();
            let nav_range = first.range;
            let nav_root = navigation_root.clone();
            let group_header = Hoverable::new(group_mouse, move |state| {
                let count_pill = Container::new(
                    Text::new(count.to_string(), ui, crate::theme::text_size(10.0))
                        .with_color(theme.accent)
                        .finish(),
                )
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(1.0)
                .with_padding_bottom(1.0)
                .with_background_color(theme.accent_bg())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish();
                let row = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_spacing(6.0)
                    .with_child(render_language_icon(&path, theme))
                    .with_child(
                        Text::new(file_name.clone(), ui, crate::theme::text_size(11.5))
                            .with_color(if state.is_hovered() {
                                theme.accent
                            } else {
                                theme.main_text()
                            })
                            .soft_wrap(false)
                            .finish(),
                    )
                    .with_child(
                        Text::new(dir_text.clone(), ui, crate::theme::text_size(9.5))
                            .with_color(theme.disabled_text())
                            .soft_wrap(false)
                            .finish(),
                    )
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(count_pill)
                    .finish();
                Container::new(row)
                    .with_background_color(if state.is_hovered() {
                        theme.sidebar_row_hover()
                    } else {
                        theme.surface_2()
                    })
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(5.0)
                    .with_padding_bottom(5.0)
                    .finish()
            })
            .on_click(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::OpenLanguageLocation {
                        root: nav_root.clone(),
                        relative_path: nav_path.clone(),
                        range: nav_range,
                    },
                );
            })
            .with_cursor(Cursor::PointingHand)
            .finish();
            body = body.with_child(group_header);
            for &index in indices {
                body = body.with_child(self.language_location_row(index, theme, ui, mono));
            }
        }

        let list = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.language_locations_scroll.clone(),
                body.finish(),
                ScrollbarWidth::Auto,
                theme.disabled_text().into(),
                theme.sub_text().into(),
                theme.surface_2().into(),
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_min_height(72.0)
        .finish();

        let bordered_panel = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(
                    Container::new(header.finish())
                        .with_padding_left(12.0)
                        .with_padding_right(8.0)
                        .with_padding_top(6.0)
                        .with_padding_bottom(6.0)
                        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                        .finish(),
                )
                .with_child(Expanded::new(1.0, list).finish())
                .finish(),
        )
        .with_border(Border::top(1.0).with_border_color(theme.border()))
        .finish();

        // Match the docked terminal's themed backdrop: opaque base, clipped
        // image, translucent theme tint, then the panel content. The panel is
        // bounded by Resizable before this is laid out, so `cover()` always
        // receives a finite rectangle and cannot spill or expand the editor.
        match Theme::bg_image_for(theme.id) {
            Some(bg) => {
                let image = Clipped::new(
                    warpui::elements::Image::new(
                        bg.asset_source(),
                        warpui::elements::CacheOption::Original,
                    )
                    .cover()
                    .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                    .finish(),
                )
                .finish();
                let tinted = Container::new(bordered_panel)
                    .with_background_color(theme.background)
                    .finish();
                Container::new(Box::new(Stack::new().with_child(image).with_child(tinted)))
                    .with_background_color(theme.solid_background())
                    .finish()
            }
            None => Container::new(bordered_panel)
                .with_background_color(theme.solid_background())
                .finish(),
        }
    }

    /// One reference row inside the peek panel: a preview of the line with
    /// the match range highlighted, plus the line:column position on the
    /// right. Clicking navigates to the reference.
    pub(crate) fn language_location_row(
        &self,
        index: usize,
        theme: &Theme,
        ui: FamilyId,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let location = &self.language_locations[index];
        let mouse = self
            .document_action_mouse_states
            .borrow_mut()
            .entry(format!("language_location_{index}"))
            .or_insert_with(MouseStateHandle::default)
            .clone();
        let workspace_view_id = self.workspace_view_id;
        let root = self.language_locations_root.clone().unwrap_or_default();
        let path = location.relative_path.clone();
        let range = location.range;
        let position_label = format!(
            "{}:{}",
            location.range.start.line.saturating_add(1),
            location.range.start.character.saturating_add(1)
        );

        let (before, matched, after) = match (&location.line_text, location.match_range) {
            (Some(line), Some((start, end))) => (
                line.chars().take(start).collect::<String>(),
                line.chars()
                    .skip(start)
                    .take(end.saturating_sub(start))
                    .collect::<String>(),
                line.chars().skip(end).collect::<String>(),
            ),
            _ => (String::new(), String::new(), path.display().to_string()),
        };
        let has_preview = location.line_text.is_some();

        let row = Hoverable::new(mouse, move |state| {
            let mut preview = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(0.0);
            if has_preview {
                preview = preview
                    .with_child(
                        Text::new(before.clone(), mono, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text())
                            .soft_wrap(false)
                            .finish(),
                    )
                    .with_child(
                        Container::new(
                            Text::new(matched.clone(), mono, crate::theme::text_size(11.0))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .finish(),
                        )
                        .with_background_color(theme.accent_overlay())
                        .finish(),
                    )
                    .with_child(
                        Text::new(after.clone(), mono, crate::theme::text_size(11.0))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .finish(),
                    );
            } else {
                preview = preview.with_child(
                    Text::new(after.clone(), mono, crate::theme::text_size(11.0))
                        .with_color(theme.sub_text())
                        .soft_wrap(false)
                        .finish(),
                );
            }
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(
                    Expanded::new(
                        1.0,
                        Container::new(preview.finish())
                            .with_padding_left(38.0)
                            .with_padding_right(8.0)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Container::new(
                        Text::new(position_label.clone(), ui, crate::theme::text_size(9.5))
                            .with_color(theme.disabled_text())
                            .finish(),
                    )
                    .with_padding_right(12.0)
                    .finish(),
                );
            Container::new(row.finish())
                .with_background_color(if state.is_hovered() {
                    theme.accent_overlay()
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .finish()
        })
        .on_click(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::OpenLanguageLocation {
                    root: root.clone(),
                    relative_path: path.clone(),
                    range,
                },
            );
        })
        .with_cursor(Cursor::PointingHand)
        .finish();
        row
    }
}
