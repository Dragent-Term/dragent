//! Input editor construction and configuration.
//!
//! Responsible for: constructors, fonts, themes, hover, visual caches,
//! fold state, vim setup, cursor and chip primitives.
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

use super::InputEditor;

impl InputEditor {
    pub fn new(
        placeholder: &str,
        ui_font: FamilyId,
        font_size: f32,
        text_color: ColorU,
        placeholder_color: ColorU,
        cursor_color: ColorU,
        theme: Theme,
        mode: InputMode,
        focus_handle: FocusHandle,
    ) -> Self {
        Self::new_secure(
            placeholder,
            ui_font,
            font_size,
            text_color,
            placeholder_color,
            cursor_color,
            theme,
            mode,
            focus_handle,
            false,
            "",
        )
    }

    pub fn new_secure(
        placeholder: &str,
        ui_font: FamilyId,
        font_size: f32,
        text_color: ColorU,
        placeholder_color: ColorU,
        cursor_color: ColorU,
        theme: Theme,
        mode: InputMode,
        focus_handle: FocusHandle,
        secure: bool,
        initial_text: &str,
    ) -> Self {
        let buffer = initial_text.to_string();
        let cursor = buffer.chars().count();
        let document_capabilities = capabilities_for_text(&buffer);
        Self {
            instance_id: NEXT_INPUT_EDITOR_ID.fetch_add(1, Ordering::Relaxed),
            viewport_handle: None,
            passthrough_keys: None,
            native_slash_commands: false,
            buffer,
            document_capabilities,
            cursor,
            placeholder: placeholder.to_string(),
            ui_font,
            menu_font: ui_font,
            given_font_size: font_size,
            font_scale_basis: crate::theme::font_scale(),
            typography_seen: std::cell::Cell::new(crate::theme::typography_generation()),
            diff_comment_hover: Default::default(),
            // The font roles apply Settings → Font weight to all text, so an
            // editor only sets its own weight for special cases.
            font_weight: Weight::Normal,
            text_color,
            placeholder_color,
            cursor_color,
            cursor_style_override: None,
            remote_caret: None,
            focus_handle,
            mode,
            cursor_visible: true,
            ghost_suffix: None,
            theme,
            blink_handle: None,
            selection_anchor: None,
            drag_anchor: None,
            available_width: std::cell::Cell::new(10000.0),
            layout_width: std::cell::Cell::new(10000.0),
            available_height: std::cell::Cell::new(INPUT_MAX_HEIGHT),
            scroll_state: ClippedScrollStateHandle::new(),
            horizontal_scroll_state: ClippedScrollStateHandle::new(),
            external_horizontal_scroll_state: None,
            follow_bottom: std::cell::Cell::new(true),
            buffer_revision: 0,
            text_revision: 0,
            visual_line_cache: RefCell::new(None),
            lexical_state_cache: RefCell::new(None),
            lexical_dirty_line: std::cell::Cell::new(None),
            lexical_force_full_rebuild: std::cell::Cell::new(false),
            line_layout_cache: RefCell::new(HashMap::new()),
            large_source_line_index: RefCell::new(None),
            resize_reflow_handle: None,
            source_path: None,
            source_language: Language::PlainText,
            breakpoints: BreakpointRegistry::default(),
            fold_index_cache: RefCell::new(None),
            fold_refresh_handle: None,
            fold_parser: None,
            syntax_highlight_cache: RefCell::new(None),
            syntax_refresh_handle: None,
            fold_visibility_cache: RefCell::new(None),
            fold_state: FoldState::default(),
            fold_gutter_hover: MouseStateHandle::default(),
            search_matches: Vec::new(),
            active_search_match: None,
            source_line_changes: Default::default(),
            git_diff_projection: None,
            diff_comment_drag: None,
            diff_comment_hover_line: None,
            source_diagnostic_lines: Default::default(),
            source_diagnostics: Vec::new(),
            source_diagnostics_by_line: Default::default(),
            diagnostic_mouse_states: Vec::new(),
            active_diagnostic: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            token_hint: None,
            vim_enabled: false,
            vim_state: VimState::new_insert(),
            workflow_arg_highlight: None,
            vim_status_hint: None,
            is_agent_mode: false,
            suppress_command_highlight: false,
            secure,
            chip_spans: Vec::new(),
            active_mention: None,
            mention_candidates: Vec::new(),
            mention_selected: 0,
            context_menu_position: None,
            context_menu_cursor: None,
            context_menu_selected: 0,
            language_navigation_capabilities: LanguageNavigationCapabilities::default(),
            definition_modifier_held: false,
            definition_hover: None,
            definition_hover_request_id: 0,
            definition_click_candidate: None,
            last_pointer_position: None,
            definition_hover_mouse_state: MouseStateHandle::default(),
            context_menu_scroll: ClippedScrollStateHandle::new(),
            context_menu_mouse_states: RefCell::new(std::collections::HashMap::new()),
            inline_rename: None,
        }
    }

    /// Live-apply the terminal typography (Settings → Terminal and Input) to
    /// this editor: mono family + scaled size. Caret geometry and text layout
    /// all derive from these fields, so no other invalidation is needed.
    pub fn set_font(&mut self, family: FamilyId, size: f32, ctx: &mut ViewContext<Self>) {
        let typography_changed =
            self.ui_font != family || (self.font_size() - size).abs() >= 0.01;
        self.ui_font = family;
        self.given_font_size = size;
        self.font_scale_basis = crate::theme::font_scale();
        if typography_changed {
            self.line_layout_cache.borrow_mut().clear();
            self.invalidate_visual_lines();
        }
        // A settings event can change line height without changing family or
        // size. A repaint is enough because line height is read dynamically;
        // discarding every shaped source line here caused repeated freezes.
        ctx.notify();
    }

    /// The text size now: the size this editor was given, rescaled by any
    /// Font-size change since. Text shaped under older typography (size,
    /// family or weight) is dropped here, so every window's editors follow
    /// Settings without being told.
    pub fn font_size(&self) -> f32 {
        let generation = crate::theme::typography_generation();
        if self.typography_seen.get() != generation {
            if let (Ok(mut lines), Ok(mut visual)) = (
                self.line_layout_cache.try_borrow_mut(),
                self.visual_line_cache.try_borrow_mut(),
            ) {
                lines.clear();
                visual.take();
                self.typography_seen.set(generation);
            }
        }
        self.given_font_size * crate::theme::font_scale() / self.font_scale_basis
    }

    pub fn set_menu_font(&mut self, family: FamilyId, ctx: &mut ViewContext<Self>) {
        if self.menu_font != family {
            self.menu_font = family;
            ctx.notify();
        }
    }

    pub fn set_font_weight(&mut self, weight: Weight, ctx: &mut ViewContext<Self>) {
        if self.font_weight != weight {
            self.font_weight = weight;
            self.line_layout_cache.borrow_mut().clear();
            self.invalidate_visual_lines();
            ctx.notify();
        }
    }

    pub(crate) fn is_source_editor(&self) -> bool {
        self.source_path.is_some() && matches!(self.mode, InputMode::MultiLine { .. })
    }

    /// The indent unit for this document: whatever the file already uses
    /// (detected from its content), else the Editor settings' Tab size /
    /// insert-spaces preference.
    pub(crate) fn indent_unit(&self) -> String {
        super::text::detect_indent_unit(&self.buffer).unwrap_or_else(super::text::tab_insert_text)
    }

    pub(crate) fn line_height(&self) -> f32 {
        if self.is_source_editor() {
            self.font_size() * SOURCE_EDITOR_LINE_HEIGHT_RATIO
        } else {
            line_height_with_spacing(self.font_size())
        }
    }

    pub(crate) fn line_height_ratio(&self) -> f32 {
        if self.is_source_editor() {
            SOURCE_EDITOR_LINE_HEIGHT_RATIO
        } else {
            crate::console::input_layout::INPUT_LINE_HEIGHT_RATIO
        }
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        self.text_color = theme.main_text();
        self.placeholder_color = theme.disabled_text();
        self.cursor_color = theme.accent;
        self.invalidate_visual_lines();
        ctx.notify();
    }

    pub fn set_document_path(&mut self, path: Option<&Path>, ctx: &mut ViewContext<Self>) {
        self.viewport_handle = Some(ctx.handle());
        let next_path = path.map(Path::to_path_buf);
        let next_language = path.map(Language::from_path).unwrap_or_default();
        if self.source_path != next_path || self.source_language != next_language {
            self.source_path = next_path;
            self.source_language = next_language;
            self.fold_index_cache.get_mut().take();
            self.invalidate_visual_lines();
            self.schedule_fold_refresh(ctx);
            ctx.notify();
        }
    }

    pub fn document_capabilities(&self) -> DocumentCapabilities {
        self.document_capabilities
    }

    pub fn set_document_capabilities(
        &mut self,
        capabilities: DocumentCapabilities,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.document_capabilities != capabilities {
            self.document_capabilities = capabilities;
            self.invalidate_visual_lines();
            ctx.notify();
        }
    }

    pub fn set_language_navigation_capabilities(
        &mut self,
        capabilities: LanguageNavigationCapabilities,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.language_navigation_capabilities != capabilities {
            self.language_navigation_capabilities = capabilities;
            ctx.notify();
        }
    }

    /// Complete the latest semantic hover request. Late responses are ignored
    /// by id, matching the trigger-point generation behavior used by modern editors.
    pub fn set_definition_hover_result(
        &mut self,
        request_id: u64,
        available: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(hover) = self.definition_hover.as_mut() else {
            return;
        };
        if hover.request_id != request_id {
            log::info!(
                "[definition-link] discarding stale hover result id={} (current={})",
                request_id,
                hover.request_id
            );
            return;
        }
        if hover.resolved != available {
            hover.resolved = available;
            log::info!(
                "[definition-link] hover resolved id={} definition_available={}",
                request_id,
                available
            );
            ctx.notify();
        }
    }

    pub(crate) fn clear_definition_hover(&mut self, ctx: &mut ViewContext<Self>) {
        let changed = self.definition_hover.take().is_some()
            || self.definition_click_candidate.take().is_some();
        if changed {
            ctx.notify();
        }
    }

    pub(crate) fn definition_hover_enabled(&self) -> bool {
        self.is_source_editor()
            && self.definition_modifier_held
            && self.language_navigation_capabilities.definition
            && self.document_capabilities.language_server
    }

    pub(crate) fn pointer_is_in_source_content(
        &self,
        position: Vector2F,
        ctx: &ViewContext<Self>,
    ) -> bool {
        let Some(bounds) = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
        else {
            return false;
        };
        bounds.contains_point(position)
            && position.x()
                >= bounds.origin().x() + self.source_gutter_width() + EDITOR_CONTENT_PADDING_LEFT
    }

    /// Return the lexical identifier containing `char_index`. This range is
    /// presentation-only; the LSP response remains the authority on whether it
    /// is a semantic link. Work is bounded to the physical source line.
    pub(crate) fn identifier_range_at(&self, char_index: usize) -> Option<std::ops::Range<usize>> {
        let index = self.large_source_index();
        let line_number = index.line_for_char(char_index);
        let (byte_start, byte_end, _) = index.line_range(&self.buffer, line_number)?;
        let line = self.buffer.get(byte_start..byte_end)?;
        let line_char_start = index.starts.get(line_number)?.1;
        identifier_range_in_line(line, line_char_start, char_index)
    }

    /// Return the content range (quotes excluded) of a path-like string
    /// literal containing `char_index`. An import specifier such as
    /// `"./a/b/c"` is ONE document link to the target file — not a separate
    /// link per path segment.
    pub(crate) fn string_link_range_at(&self, char_index: usize) -> Option<std::ops::Range<usize>> {
        let index = self.large_source_index();
        let line_number = index.line_for_char(char_index);
        let (byte_start, byte_end, _) = index.line_range(&self.buffer, line_number)?;
        let line = self.buffer.get(byte_start..byte_end)?;
        let line_char_start = index.starts.get(line_number)?.1;

        let chars: Vec<char> = line.chars().collect();
        let local = char_index.checked_sub(line_char_start)?.min(chars.len());
        let (open, close) = string_link_span_in_line(&chars, local)?;
        let start = line_char_start + open + 1;
        let end = line_char_start + close;
        Some(start..end)
    }

    pub(crate) fn update_definition_hover_at(
        &mut self,
        position: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        self.last_pointer_position = Some(position);
        self.update_diff_comment_hover(position, ctx);
        if !self.definition_hover_enabled() {
            // Only log when the modifier is held but something else disables
            // the feature — otherwise this fires on every mouse move. Debug
            // level because other editors (e.g. the terminal input) also run
            // this path while the modifier is held.
            if self.definition_modifier_held {
                log::debug!(
                    "[definition-link] hover disabled (source={} modifier={} caps={} lsp={})",
                    self.is_source_editor(),
                    self.definition_modifier_held,
                    self.language_navigation_capabilities.definition,
                    self.document_capabilities.language_server,
                );
            }
            self.clear_definition_hover(ctx);
            return;
        }
        if !self.pointer_is_in_source_content(position, ctx) {
            log::info!(
                "[definition-link] pointer outside source content at {:?}",
                position
            );
            self.clear_definition_hover(ctx);
            return;
        }
        let Some(cursor) = self.char_index_at_position(position, ctx) else {
            log::info!("[definition-link] hit test failed at {:?}", position);
            self.clear_definition_hover(ctx);
            return;
        };
        // Import specifiers ("./a/b/c") link as one string; everything else
        // links as a lexical identifier.
        let string_link_range = self.string_link_range_at(cursor);
        let string_link = string_link_range.clone().map(|range| {
            self.buffer
                .chars()
                .skip(range.start)
                .take(range.end - range.start)
                .collect()
        });
        let Some(symbol_range) = string_link_range
            .clone()
            .or_else(|| self.identifier_range_at(cursor))
        else {
            log::info!("[definition-link] no symbol at char {}", cursor);
            self.clear_definition_hover(ctx);
            return;
        };
        if self
            .definition_hover
            .as_ref()
            .is_some_and(|hover| hover.symbol_range == symbol_range)
        {
            return;
        }
        self.definition_hover_request_id = self.definition_hover_request_id.wrapping_add(1);
        let request_id = self.definition_hover_request_id;
        self.definition_hover = Some(DefinitionHoverState {
            request_id,
            symbol_range: symbol_range.clone(),
            resolved: false,
        });
        self.definition_click_candidate = None;
        log::info!(
            "[definition-link] requesting definition id={} char={} symbol={:?} range={:?}",
            request_id,
            cursor,
            &self
                .buffer
                .chars()
                .skip(symbol_range.start)
                .take(symbol_range.end - symbol_range.start)
                .collect::<String>(),
            symbol_range,
        );
        ctx.emit(InputEditorEvent::DefinitionHoverRequested {
            request_id,
            cursor: cursor.min(symbol_range.end.saturating_sub(1)),
            symbol_range,
            string_link,
        });
        ctx.notify();
    }

    pub(crate) fn invalidate_visual_lines(&mut self) {
        self.buffer_revision = self.buffer_revision.wrapping_add(1);
        self.visual_line_cache.get_mut().take();
        // Preserve the previous fold snapshot until the background refresh is
        // ready. Clearing this cache forced `render` to parse the complete
        // document synchronously after every character.
        self.fold_visibility_cache.get_mut().take();
    }

    pub(crate) fn invalidate_buffer_content(&mut self) {
        let dirty_line = physical_line_for_char(&self.buffer, self.cursor);
        self.lexical_dirty_line.set(Some(
            self.lexical_dirty_line
                .get()
                .map_or(dirty_line, |existing| existing.min(dirty_line)),
        ));
        self.invalidate_buffer_content_inner();
    }

    pub(crate) fn invalidate_buffer_content_fully(&mut self) {
        self.lexical_dirty_line.set(Some(0));
        self.lexical_force_full_rebuild.set(true);
        self.invalidate_buffer_content_inner();
    }

    pub(crate) fn invalidate_buffer_content_inner(&mut self) {
        self.text_revision = self.text_revision.wrapping_add(1);
        self.large_source_line_index.get_mut().take();
        self.invalidate_visual_lines();
        // Re-evaluate only when a byte-size threshold was actually crossed.
        // Comparing the byte predicate directly with `large_file` is wrong:
        // that flag can also be true because of line count or one long row,
        // which would otherwise rescan the complete document on every edit.
        let byte_size = self.buffer.len();
        let previous_byte_size = self.document_capabilities.byte_size;
        let large_changed = (byte_size >= dragent_document::LARGE_FILE_BYTES)
            != (previous_byte_size >= dragent_document::LARGE_FILE_BYTES);
        let extreme_changed = (byte_size >= dragent_document::EXTREME_FILE_BYTES)
            != (previous_byte_size >= dragent_document::EXTREME_FILE_BYTES);
        if large_changed || extreme_changed {
            self.document_capabilities = capabilities_for_text(&self.buffer);
        } else {
            self.document_capabilities.byte_size = byte_size;
        }
    }

    pub(crate) fn fold_index(&self) -> FoldIndex {
        if let Some(cache) = self.fold_index_cache.borrow().as_ref() {
            // A stale structural snapshot is safe presentation data and is
            // vastly preferable to blocking input. It will be replaced by
            // `schedule_fold_refresh` once the latest parse completes.
            return cache.index.clone();
        }
        // Initial load and edits both populate this cache from the background
        // refresh. Rendering must never become a hidden Tree-sitter entry
        // point: until the first snapshot arrives, folding is simply empty.
        FoldIndex::default()
    }

    /// Re-highlight the buffer with its tree-sitter grammar off the UI
    /// thread. Every path that schedules a fold refresh (load, edit, path
    /// change) goes through here first, so highlighting always follows edits.
    pub(crate) fn schedule_syntax_refresh(&mut self, ctx: &mut ViewContext<Self>) {
        // `folding` is the capability that permits whole-file Tree-sitter
        // work; large files keep the viewport-only lexical highlighter.
        if !self.is_source_editor()
            || !self.document_capabilities.syntax_highlighting
            || !self.document_capabilities.folding
            || self.syntax_refresh_handle.is_some()
        {
            return;
        }
        let Some(grammar) =
            dragent_editor::grammar_for(self.source_path.as_deref(), self.source_language)
        else {
            self.syntax_highlight_cache.get_mut().take();
            return;
        };
        let current = self.syntax_highlight_cache.get_mut().as_ref();
        if current.is_some_and(|c| c.revision == self.text_revision && c.grammar == grammar) {
            return;
        }
        let revision = self.text_revision;
        let text = self.buffer.clone();
        self.syntax_refresh_handle = Some(ctx.spawn(
            async move { SyntaxHighlightCache::build(grammar, &text, revision) },
            move |editor, cache, ctx| {
                editor.syntax_refresh_handle = None;
                match cache {
                    Some(cache) => {
                        *editor.syntax_highlight_cache.get_mut() = Some(cache);
                        ctx.notify();
                    }
                    // Grammar not compiled in: stay on lexical highlighting.
                    None => {
                        editor.syntax_highlight_cache.get_mut().take();
                        return;
                    }
                }
                // Coalesce edits made during the parse into one more pass.
                if editor.text_revision != revision {
                    editor.schedule_syntax_refresh(ctx);
                }
            },
        ));
    }

    /// Tree-sitter spans for a visual row, if the snapshot can serve it.
    /// `source_line` is 1-based, as on `VisualLine`.
    pub(crate) fn syntax_spans_for_row(
        &self,
        source_line: usize,
        abs_char_offset: usize,
        text: &str,
    ) -> Option<Vec<dragent_editor::HighlightSpan>> {
        let cache = self.syntax_highlight_cache.borrow();
        let cache = cache.as_ref()?;
        let grammar =
            dragent_editor::grammar_for(self.source_path.as_deref(), self.source_language)?;
        if cache.grammar != grammar {
            return None;
        }
        cache.spans_for_row(
            self.text_revision,
            source_line.checked_sub(1)?,
            abs_char_offset,
            text,
        )
    }

    pub(crate) fn schedule_fold_refresh(&mut self, ctx: &mut ViewContext<Self>) {
        self.schedule_syntax_refresh(ctx);
        if !self.is_source_editor()
            || !self.document_capabilities.folding
            || self.fold_refresh_handle.is_some()
        {
            return;
        }
        let revision = self.text_revision;
        let language = self.source_language;
        let text = self.buffer.clone();
        let parser = self.fold_parser.take();
        self.fold_refresh_handle = Some(ctx.spawn(
            async move {
                let (parser, index) = match parser {
                    Some(parser) => parser.parse(language, text),
                    None => FoldParser::initial(language, text),
                };
                (revision, language, parser, index)
            },
            |editor, (parsed_revision, parsed_language, parser, index), ctx| {
                editor.fold_refresh_handle = None;
                editor.fold_parser = Some(parser);
                if editor.text_revision == parsed_revision
                    && editor.source_language == parsed_language
                    && editor.document_capabilities.folding
                {
                    editor.fold_state.retain_valid(&index);
                    *editor.fold_index_cache.get_mut() = Some(FoldIndexCache {
                        revision: parsed_revision,
                        index,
                    });
                    editor.fold_visibility_cache.get_mut().take();
                    ctx.notify();
                } else {
                    // Coalesce every edit that arrived while parsing into one
                    // refresh of the latest immutable text snapshot.
                    editor.schedule_fold_refresh(ctx);
                }
            },
        ));
    }

    pub(crate) fn large_source_index(&self) -> Rc<LargeSourceLineIndex> {
        if let Some(index) = self.large_source_line_index.borrow().as_ref() {
            if index.revision == self.text_revision && index.language == self.source_language {
                return Rc::new(index.clone());
            }
        }
        let index =
            LargeSourceLineIndex::build(&self.buffer, self.text_revision, self.source_language);
        *self.large_source_line_index.borrow_mut() = Some(index.clone());
        Rc::new(index)
    }

    pub(crate) fn large_source_visual_line(
        &self,
        index: &LargeSourceLineIndex,
        line_number: usize,
        font_cache: &FontCache,
        with_layout: bool,
    ) -> Option<VisualLine> {
        let (byte_start, byte_end, _) = index.line_range(&self.buffer, line_number)?;
        let text = bounded_large_source_line(self.buffer.get(byte_start..byte_end)?).to_string();
        // Rendering creates the actual Text layout. Avoid shaping the full
        // row a second time just to populate VisualLine when the caller only
        // needs source text and offsets for the viewport.
        let layout = if with_layout {
            unwrapped_line_layout(
                &text,
                self.ui_font,
                self.font_size(),
                Properties::default().weight(self.font_weight),
                font_cache,
            )
        } else {
            unwrapped_line_layout(
                "",
                self.ui_font,
                self.font_size(),
                Properties::default().weight(self.font_weight),
                font_cache,
            )
        };
        Some(VisualLine {
            text,
            abs_char_offset: index.starts[line_number].1,
            source_line: line_number + 1,
            continuation: false,
            layout: std::rc::Rc::new(layout),
        })
    }

    pub(crate) fn visible_visual_indices(
        &self,
        lines: &[VisualLine],
        fold_index: &FoldIndex,
        available_width: f32,
    ) -> Rc<Vec<usize>> {
        if let Some(cache) = self.fold_visibility_cache.borrow().as_ref() {
            if cache.matches(
                self.buffer_revision,
                self.fold_state.revision(),
                available_width,
            ) {
                return Rc::clone(&cache.visual_indices);
            }
        }
        let visual_indices = Rc::new(
            lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| {
                    (!self.fold_state.is_line_hidden(line.source_line, fold_index)).then_some(index)
                })
                .collect(),
        );
        *self.fold_visibility_cache.borrow_mut() = Some(FoldVisibilityCache {
            revision: self.buffer_revision,
            fold_revision: self.fold_state.revision(),
            available_width,
            visual_indices: Rc::clone(&visual_indices),
        });
        visual_indices
    }

    pub(crate) fn schedule_width_reflow(&mut self, width: f32, ctx: &mut ViewContext<Self>) {
        self.available_width.set(width);
        if self.visual_line_cache.borrow().is_none() {
            self.layout_width.set(width);
            return;
        }
        if (self.layout_width.get() - width).abs() < 1.0 {
            return;
        }
        if let Some(handle) = self.resize_reflow_handle.take() {
            handle.abort();
        }
        self.resize_reflow_handle = Some(ctx.spawn_abortable(
            Timer::after(Duration::from_millis(120)),
            |view, _, ctx| {
                view.layout_width.set(view.available_width.get());
                view.invalidate_visual_lines();
                view.resize_reflow_handle = None;
                ctx.notify();
            },
            |view, _| {
                view.resize_reflow_handle = None;
            },
        ));
    }

    pub(crate) fn text_layout_width(&self, width: f32) -> f32 {
        if self.source_path.is_some() && matches!(self.mode, InputMode::MultiLine { .. }) {
            (width - self.source_gutter_width() - EDITOR_CONTENT_PADDING_LEFT).max(80.0)
        } else {
            width
        }
    }

    /// Final (largest) line number in this source document. The gutter width
    /// derives from this so every row shares one stable width that still fits
    /// e.g. line 1100. Folding never shrinks it: collapsing lines must not
    /// reflow the whole editor.
    pub(crate) fn source_max_line(&self) -> usize {
        if self.document_capabilities.large_file {
            self.large_source_index().line_count().max(1)
        } else {
            self.buffer.bytes().filter(|byte| *byte == b'\n').count() + 1
        }
    }

    /// Painted gutter width for this document. Every consumer — gutter rows,
    /// the fixed viewport, click/drag hit-testing and content offsets — must
    /// use this so they never disagree.
    pub(crate) fn source_gutter_width(&self) -> f32 {
        let full = editor_render::gutter_width_for_lines(self.source_max_line(), self.font_size());
        if crate::console::editor_settings::EditorSettings::global()
            .editor
            .line_numbers
        {
            full
        } else {
            // Numbers hidden: keep marker/fold lanes, drop the number slot.
            (full - editor_render::gutter_number_width(self.source_max_line(), self.font_size()))
                .max(0.0)
        }
    }

    pub(crate) fn sync_vim_status_hint(&mut self) {
        if !self.vim_enabled {
            self.vim_status_hint = None;
            return;
        }
        self.vim_status_hint = Some(match self.vim_state.mode {
            VimMode::Insert => "INSERT — type normally · Esc → NORMAL (then x deletes, dd deletes line, i to type again)".into(),
            VimMode::Normal => "NORMAL — x delete char · dd delete line · h/j/k/l move · i insert · text stays here until you press Enter".into(),
            VimMode::VisualChar | VimMode::VisualLine => {
                "VISUAL — x or d delete selection · y yank · Esc → NORMAL".into()
            }
        });
    }

    pub fn vim_status_hint(&self) -> Option<&str> {
        self.vim_status_hint.as_deref()
    }

    /// Install the shared "editor is inert" flag (see `passthrough_keys`).
    /// The owning pane flips it true while a foreground command runs.
    pub fn set_passthrough_flag(&mut self, flag: Arc<AtomicBool>) {
        self.passthrough_keys = Some(flag);
    }

    /// Reserve an empty input's initial `/` and subsequent proxy keys for its parent.
    pub fn enable_native_slash_commands(&mut self) {
        self.native_slash_commands = true;
        self.passthrough_keys = Some(Arc::new(AtomicBool::new(false)));
    }

    /// Switch keyboard ownership immediately, without waiting for an editor render.
    pub fn set_native_slash_proxy_active(&self, active: bool) {
        if self.native_slash_commands {
            if let Some(flag) = &self.passthrough_keys {
                flag.store(active, Ordering::Relaxed);
            }
        }
    }

    pub fn set_vim_enabled(&mut self, enabled: bool, ctx: &mut ViewContext<Self>) {
        if self.vim_enabled != enabled {
            self.vim_enabled = enabled;
            if enabled {
                self.vim_state.reset_to_insert();
            }
            self.sync_vim_status_hint();
            ctx.notify();
        }
    }

    pub fn vim_mode_label(&self) -> Option<&'static str> {
        if !self.vim_enabled {
            return None;
        }
        Some(self.vim_state.mode_label())
    }

    pub fn vim_show_cmd(&self) -> &str {
        if self.vim_enabled {
            &self.vim_state.show_cmd
        } else {
            ""
        }
    }

    pub fn set_workflow_arg_highlight(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.workflow_arg_highlight != range {
            self.workflow_arg_highlight = range;
            ctx.notify();
        }
    }

    pub fn set_cursor_char_index(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        self.cursor = index.min(self.buffer.chars().count());
        self.clear_selection();
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        ctx.notify();
    }

    pub(crate) fn capture_view_state(&self) -> EditorViewState {
        EditorViewState {
            cursor: self.cursor,
            selection_anchor: self.selection_anchor,
            vertical_scroll: self.scroll_state.scroll_start().as_f32(),
            horizontal_scroll: self.horizontal_scroll_state.scroll_start().as_f32(),
        }
    }

    pub(crate) fn restore_view_state(
        &mut self,
        state: &EditorViewState,
        ctx: &mut ViewContext<Self>,
    ) {
        let length = self.buffer.chars().count();
        self.set_cursor_char_index(state.cursor.min(length), ctx);
        self.selection_anchor = state.selection_anchor.map(|anchor| anchor.min(length));
        self.scroll_state
            .scroll_to(state.vertical_scroll.into_pixels());
        self.horizontal_scroll_state
            .scroll_to(state.horizontal_scroll.into_pixels());
    }

    /// Move the viewport to the current cursor after an external navigation
    /// command. Ordinary cursor assignment intentionally does not scroll (it
    /// is also used while restoring tabs), so LSP navigation calls this
    /// explicitly once the destination buffer is active.
    pub fn reveal_cursor(&mut self, ctx: &mut ViewContext<Self>) {
        self.cursor_visible = true;
        let visual_lines = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        self.scroll_cursor_into_view_for_layout(&visual_lines, ctx);
        self.scroll_source_cursor_horizontally_into_view(ctx);
        self.schedule_cursor_blink(ctx);
        ctx.notify();
    }

    pub fn set_remote_caret(
        &mut self,
        identity: Option<RemoteCaretIdentity>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.remote_caret != identity {
            self.remote_caret = identity;
            ctx.notify();
        }
    }

    pub fn vim_blocks_history_propagation(&self) -> bool {
        self.vim_enabled && self.vim_state.mode == VimMode::Normal
    }

    pub fn set_token_hint(&mut self, hint: Option<String>, ctx: &mut ViewContext<Self>) {
        if self.token_hint != hint {
            self.token_hint = hint;
            ctx.notify();
        }
    }

    pub(crate) fn cursor_byte_offset(&self) -> usize {
        self.buffer
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len())
    }

    pub(crate) fn char_vec(&self) -> Vec<char> {
        self.buffer.chars().collect()
    }

    pub(crate) fn apply_edit(&mut self, ctx: &mut ViewContext<Self>) {
        self.definition_hover = None;
        self.definition_click_candidate = None;
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        self.publish_buffer_change(true, ctx);
        self.schedule_fold_refresh(ctx);
        if self.is_agent_mode {
            self.refresh_mention_state(ctx);
        }
        ctx.notify();
    }

    pub(crate) fn publish_buffer_change(&self, text_changed: bool, ctx: &mut ViewContext<Self>) {
        if self.mode.publishes_buffer_events() {
            if let InputMode::MultiLine { key, .. } = &self.mode {
                log::debug!(
                    "[editor-input] changed key={} bytes={} cursor={}",
                    key,
                    self.buffer.len(),
                    self.cursor
                );
            }
            ctx.emit(InputEditorEvent::Changed {
                text: self.buffer.clone(),
                cursor: self.cursor,
            });
            return;
        }
        if !text_changed {
            return;
        }
        match &self.mode {
            InputMode::Search => {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::SidebarSearchInput {
                    text: self.buffer.clone(),
                });
            }
            InputMode::Settings(key) => {
                ctx.dispatch_typed_action_deferred(
                    crate::settings::SettingsAction::UpdateProviderKey(
                        key.clone(),
                        self.buffer.clone(),
                    ),
                );
            }
            InputMode::RenameTab(_)
            | InputMode::RenameWorkspaceTab(_)
            | InputMode::LanguageRename { .. }
            | InputMode::TerminalCommand
            | InputMode::Plain { .. }
            | InputMode::MultiLine { .. } => {}
        }
    }

    pub(crate) fn move_cursor_with_selection(
        &mut self,
        new_cursor: usize,
        extend_selection: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let len = self.buffer.chars().count();
        let new_cursor = new_cursor.min(len);
        if extend_selection {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else {
            self.clear_selection();
        }
        self.cursor = new_cursor;
        self.apply_edit(ctx);
    }

    pub(crate) fn delete_range(&mut self, start: usize, end: usize, ctx: &mut ViewContext<Self>) {
        let len = self.buffer.chars().count();
        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            return;
        }
        self.snapshot_undo();
        let byte_start = byte_index_for_char(&self.buffer, start);
        let byte_end = byte_index_for_char(&self.buffer, end);
        self.buffer.replace_range(byte_start..byte_end, "");
        self.invalidate_buffer_content();
        self.cursor = start;
        self.clear_selection();
        self.drop_and_rebase_chips(start, end);
        self.apply_edit(ctx);
    }

    /// Returns the chip span containing `pos` in its interior (strictly between
    /// its edges), if any. Positions exactly at a chip's start/end are boundaries,
    /// not "inside" — moving/typing there is allowed to abut the chip normally.
    pub(crate) fn chip_at(&self, pos: usize) -> Option<&ChipSpan> {
        self.chip_spans
            .iter()
            .find(|c| pos > c.range.start && pos < c.range.end)
    }

    /// If `pos` lands inside a chip's interior, snap it to the chip's near edge
    /// in the direction of travel — the far edge if `moving_right`, else the
    /// start — so a chip is always crossed as a single unit.
    pub(crate) fn clamp_off_chip(&self, pos: usize, moving_right: bool) -> usize {
        match self.chip_at(pos) {
            Some(c) if moving_right => c.range.end,
            Some(c) => c.range.start,
            None => pos,
        }
    }

    /// Snap a position (e.g. from a mouse click) to the nearer edge of the chip
    /// it lands inside, if any — there's no clean "direction" for a raw click.
    pub(crate) fn clamp_off_chip_nearest(&self, pos: usize) -> usize {
        match self.chip_at(pos) {
            Some(c) if pos - c.range.start <= c.range.end - pos => c.range.start,
            Some(c) => c.range.end,
            None => pos,
        }
    }

    /// Widen `[start, end)` to fully cover every chip it partially overlaps,
    /// so a word-delete can never cut a chip in half.
    pub(crate) fn expand_range_to_chips(&self, start: usize, end: usize) -> (usize, usize) {
        let mut lo = start;
        let mut hi = end;
        for chip in &self.chip_spans {
            if chip.range.start < hi && chip.range.end > lo {
                lo = lo.min(chip.range.start);
                hi = hi.max(chip.range.end);
            }
        }
        (lo, hi)
    }

    /// After a non-chip edit at `edit_at` that changed the buffer length by
    /// `delta` chars, shift every chip span that starts at or after `edit_at`
    /// by `delta`, and drop any span that no longer has room to hold `label`.
    pub(crate) fn rebase_chips(&mut self, edit_at: usize, delta: isize) {
        for chip in &mut self.chip_spans {
            if chip.range.start >= edit_at {
                chip.range.start = (chip.range.start as isize + delta).max(0) as usize;
                chip.range.end = (chip.range.end as isize + delta).max(0) as usize;
            }
        }
        self.chip_spans
            .retain(|c| c.range.end - c.range.start == c.label.chars().count());
    }

    /// Drop every chip span that overlaps `[start, end)` (it's being deleted),
    /// and shift every span entirely after `end` left by the deleted length.
    pub(crate) fn drop_and_rebase_chips(&mut self, start: usize, end: usize) {
        let deleted_len = (end - start) as isize;
        self.chip_spans
            .retain(|c| c.range.end <= start || c.range.start >= end);
        for chip in &mut self.chip_spans {
            if chip.range.start >= end {
                chip.range.start = (chip.range.start as isize - deleted_len).max(0) as usize;
                chip.range.end = (chip.range.end as isize - deleted_len).max(0) as usize;
            }
        }
    }

    /// Read-only view of the chip spans currently embedded in the buffer.
    pub fn chip_spans(&self) -> &[ChipSpan] {
        &self.chip_spans
    }

    /// Whether an `@`-mention popup is currently open — parent views must
    /// not steal Up/Down/Enter/Escape (e.g. for command history recall)
    /// while this is true; the editor's own handler owns them for popup nav.
    pub fn mention_active(&self) -> bool {
        self.active_mention.is_some()
    }

    /// Push freshly-scored candidates into an open mention popup. A no-op if
    /// the popup has since closed (e.g. the cursor moved before the results
    /// came back) or `anchor` no longer matches the live trigger.
    pub fn set_mention_candidates(
        &mut self,
        anchor: usize,
        entries: Vec<MentionEntry>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.active_mention != Some(anchor) {
            return;
        }
        self.mention_candidates = entries;
        self.mention_selected = 0;
        ctx.notify();
    }

    /// Re-evaluate the live `@`-mention query against the current cursor
    /// position, closing the popup if the cursor left the query span, the
    /// `@` was deleted, or whitespace was typed into the query. Called from
    /// every path that can move the cursor or edit the buffer.
    pub(crate) fn refresh_mention_state(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(at) = self.active_mention else {
            return;
        };
        let len = self.buffer.chars().count();
        if self.cursor <= at || self.cursor > len {
            self.close_mention(ctx);
            return;
        }
        let chars = self.char_vec();
        if chars.get(at) != Some(&'@') {
            self.close_mention(ctx);
            return;
        }
        let query_chars = &chars[at + 1..self.cursor];
        if query_chars.iter().any(|c| c.is_whitespace()) {
            self.close_mention(ctx);
            return;
        }
        let query: String = query_chars.iter().collect();
        ctx.emit(InputEditorEvent::MentionQueryChanged { anchor: at, query });
    }

    pub(crate) fn close_mention(&mut self, ctx: &mut ViewContext<Self>) {
        if self.active_mention.take().is_some() {
            self.mention_candidates.clear();
            self.mention_selected = 0;
            ctx.emit(InputEditorEvent::MentionClosed);
        }
    }

    /// Replace the typed `@query` at `[mention_anchor, cursor)` with `label`
    /// as a single atomic chip, in one undo step.
    pub(crate) fn insert_chip(
        &mut self,
        mention_anchor: usize,
        path: std::path::PathBuf,
        label: String,
        is_dir: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.snapshot_undo();
        let mut chars = self.char_vec();
        let mention_anchor = mention_anchor.min(chars.len());
        let query_end = self.cursor.min(chars.len());
        let query_end = query_end.max(mention_anchor);
        if mention_anchor < query_end {
            chars.drain(mention_anchor..query_end.min(chars.len()));
        }
        let label_chars: Vec<char> = label.chars().collect();
        for (i, c) in label_chars.iter().enumerate() {
            chars.insert(mention_anchor + i, *c);
        }
        self.buffer = chars.into_iter().collect();
        self.invalidate_buffer_content();
        self.cursor = mention_anchor + label_chars.len();
        self.chip_spans.push(ChipSpan {
            range: mention_anchor..self.cursor,
            path,
            label,
            is_dir,
        });
        self.chip_spans.sort_by_key(|c| c.range.start);
        self.close_mention(ctx);
        self.apply_edit(ctx);
    }

    pub fn set_text(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        self.buffer = text.to_string();
        self.document_capabilities = capabilities_for_text(&self.buffer);
        self.fold_index_cache.get_mut().take();
        self.invalidate_buffer_content_fully();
        self.cursor = self.buffer.chars().count();
        self.clear_selection();
        self.chip_spans.clear();
        self.active_mention = None;
        self.definition_hover = None;
        self.definition_click_candidate = None;
        self.scroll_state.scroll_to(0.0.into_pixels());
        self.effective_horizontal_scroll_state()
            .scroll_to(0.0.into_pixels());
        self.schedule_fold_refresh(ctx);
        ctx.notify();
    }

    /// Synchronize text changed by an external, already-saved refactor while
    /// retaining the user's place in the document. Unlike `set_text`, this
    /// deliberately does not reset either scroll axis or move the caret to the
    /// end of the file.
    pub fn reload_saved_text(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        if self.buffer == text {
            return;
        }
        self.buffer = text.to_string();
        self.document_capabilities = capabilities_for_text(&self.buffer);
        self.fold_index_cache.get_mut().take();
        self.invalidate_buffer_content_fully();
        self.cursor = self.cursor.min(self.buffer.chars().count());
        self.clear_selection();
        self.chip_spans.clear();
        self.active_mention = None;
        self.definition_hover = None;
        self.definition_click_candidate = None;
        self.schedule_fold_refresh(ctx);
        ctx.notify();
    }
}
