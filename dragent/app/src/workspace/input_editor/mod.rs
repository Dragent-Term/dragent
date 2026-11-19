pub(crate) mod actions;
pub(crate) mod canvas;
pub(crate) mod core;
pub(crate) mod edit;
pub(crate) mod interact;
pub(crate) mod mouse;
pub(crate) mod overlay;
pub(crate) mod scroll;

pub(crate) mod model;
pub(crate) mod view;
mod viewport;
pub(crate) mod vim;

pub(crate) mod text;

// Re-exported so existing `input_editor::…` paths keep working.
pub use model::*;

// Glob import during migration: text helpers moved verbatim.
use model::*;
use text::*;
// Re-exported so the existing `crate::workspace::SOURCE_EDITOR_FONT_SIZE`
// path keeps working; canonical home is now `text`.
pub(crate) use text::SOURCE_EDITOR_FONT_SIZE;

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

use dragent_document::{
    capabilities_for_text, DiffLineKind, DocumentCapabilities, DocumentDiff, GitDiffDisplayRow,
};
use dragent_editor::{
    Breakpoint, BreakpointRegistry, DiagnosticSeverity, FoldIndex, FoldParser, FoldRegion,
    FoldState, Language, LineChangeKind, ResolvedDiagnostic,
};

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

/// Terminal and auxiliary input editors stop growing after this height and
/// become internally scrollable. Source editors supply their own explicit
/// `MultiLine` height and are therefore unaffected.

pub struct InputEditor {
    /// Unique per editor instance. Disambiguates `position_id()` so split panes
    /// (each with their own editor) never collide in the position cache.
    pub(crate) instance_id: u64,
    pub(crate) viewport_handle: Option<warpui::WeakViewHandle<InputEditor>>,
    /// While true, this editor is INERT for keyboard input: every keystroke
    /// propagates to the parent instead of being consumed. Set by the owning
    /// pane while a foreground command runs — the input bar is collapsed to
    /// zero height then, but can still hold focus, and silently swallowing
    /// printable keys into an invisible buffer made raw-mode prompts (gh
    /// auth login's `y/n`) look dead: Enter/arrows/Ctrl+C propagated to the
    /// pane's PTY forwarding, letters vanished. Read at EVENT time (shared
    /// atomic), so no render-ordering races. The AI input also uses this flag
    /// while its native slash-command keys are forwarded to the wrapped CLI.
    pub(crate) passthrough_keys: Option<Arc<AtomicBool>>,
    /// Let the enclosing AI tool own a slash command from its first keystroke.
    pub(crate) native_slash_commands: bool,
    pub(crate) buffer: String,
    /// Per-buffer performance policy. Large files stay editable; this only
    /// gates expensive editor decorations and language intelligence.
    pub(crate) document_capabilities: DocumentCapabilities,
    pub(crate) cursor: usize,
    pub(crate) placeholder: String,
    pub(crate) ui_font: FamilyId,
    /// Context menus use the application UI face even when the editor buffer
    /// itself uses a monospace face.
    pub(crate) menu_font: FamilyId,
    /// The size given to [`InputEditor::set_font`] (or `new`), at the
    /// Font-size scale of that moment; read it through `font_size()`.
    pub(crate) given_font_size: f32,
    pub(crate) font_scale_basis: f32,
    /// The typography generation the cached layouts below were shaped for.
    pub(crate) typography_seen: std::cell::Cell<u64>,
    /// Hover state for the diff gutter's comment +, kept per line so it
    /// survives redraws.
    pub(crate) diff_comment_hover:
        RefCell<std::collections::HashMap<String, warpui_core::elements::MouseStateHandle>>,
    pub(crate) font_weight: Weight,
    pub(crate) text_color: ColorU,
    pub(crate) placeholder_color: ColorU,
    pub(crate) cursor_color: ColorU,
    pub(crate) cursor_style_override: Option<crate::console::editor_settings::CursorStyle>,
    /// Host-only collaborator caret shown when a shared-session participant is
    /// editing this input. It replaces the ordinary local caret visually.
    pub(crate) remote_caret: Option<RemoteCaretIdentity>,
    /// Owned focus token — queryable directly in `render` without a stored bool.
    pub(crate) focus_handle: FocusHandle,
    pub(crate) mode: InputMode,
    pub(crate) cursor_visible: bool,
    pub ghost_suffix: Option<String>,
    /// Syntax-highlight palette for [`InputMode::TerminalCommand`].
    pub(crate) theme: Theme,
    /// Hint text for the token under the cursor (command intel).
    pub token_hint: Option<String>,
    pub(crate) blink_handle: Option<SpawnedFutureHandle>,
    /// Fixed end of an active selection (`min(anchor, cursor)..max(anchor, cursor)`).
    pub(crate) selection_anchor: Option<usize>,
    /// Character index where the current click/drag started.
    pub(crate) drag_anchor: Option<usize>,
    /// Cached pixel width of the canvas from the previous frame, used in render to
    /// compute visual line breaks. Updated by event handlers via ViewContext.
    pub(crate) available_width: std::cell::Cell<f32>,
    /// Width used for the currently painted line map. During a live window
    /// resize this intentionally trails `available_width` until the resize
    /// settles, keeping reflow work off every resize frame.
    pub(crate) layout_width: std::cell::Cell<f32>,
    /// Last measured clipped viewport height. Multiline rendering uses this
    /// instead of its generous configured maximum to keep overscan truly small.
    pub(crate) available_height: std::cell::Cell<f32>,
    /// Scroll state for the clipped-scrollable textarea container.
    pub(crate) scroll_state: ClippedScrollStateHandle,
    /// Source files do not soft-wrap by default. Horizontal position is kept
    /// independently so vertical navigation and source-column hit-testing do
    /// not compete for the same state handle.
    pub(crate) horizontal_scroll_state: ClippedScrollStateHandle,
    /// When embedded in a multi-file diff, horizontal movement belongs to the
    /// aggregate surface. The editor still uses this handle for hit testing,
    /// cursor reveal, and paints, but does not render a per-file horizontal
    /// scrollbar or translate its content a second time.
    pub(crate) external_horizontal_scroll_state: Option<ClippedScrollStateHandle>,
    /// Mirrors terminal scrollback's FollowBottom mode. Once the user scrolls
    /// into history, new text does not pull the viewport back down until they
    /// scroll to the tail again.
    pub(crate) follow_bottom: std::cell::Cell<bool>,
    /// Monotonic text revision and its corresponding shaped visual lines.
    /// Scrolling must never reshape the full document; edits and width/font
    /// changes are the only operations that invalidate this cache.
    pub(crate) buffer_revision: u64,
    /// Changes only when the source text changes. Syntax state, folding and
    /// compact line indexes must not be invalidated by theme, diagnostics,
    /// font, or viewport updates.
    pub(crate) text_revision: u64,
    pub(crate) visual_line_cache: RefCell<Option<VisualLineCache>>,
    pub(crate) lexical_state_cache: RefCell<Option<LexicalStateCache>>,
    pub(crate) lexical_dirty_line: std::cell::Cell<Option<usize>>,
    pub(crate) lexical_force_full_rebuild: std::cell::Cell<bool>,
    /// Content-addressed shaped layouts: line text → shaped layout. An edit
    /// re-shapes ONLY lines whose text actually changed — rebuilding the whole
    /// document's text layouts on every keystroke was the primary source of
    /// multi-second typing freezes in large files. Keying by text (not line
    /// index) means inserting/removing lines never invalidates the lines
    /// after the edit, and identical lines (e.g. `}`) share one layout.
    pub(crate) line_layout_cache:
        RefCell<HashMap<String, std::rc::Rc<warpui_core::text_layout::Line>>>,
    pub(crate) large_source_line_index: RefCell<Option<LargeSourceLineIndex>>,
    pub(crate) resize_reflow_handle: Option<SpawnedFutureHandle>,
    pub(crate) source_path: Option<PathBuf>,
    pub(crate) source_language: Language,
    pub(crate) breakpoints: BreakpointRegistry,
    pub(crate) fold_index_cache: RefCell<Option<FoldIndexCache>>,
    /// At most one whole-tree fold discovery runs at a time. Edits keep using
    /// the last valid fold snapshot while a background parse catches up; the
    /// callback discards stale results and immediately schedules the newest
    /// revision. Tree-sitter work must never run from `render` after typing.
    pub(crate) fold_refresh_handle: Option<SpawnedFutureHandle>,
    pub(crate) fold_parser: Option<FoldParser>,
    /// Latest tree-sitter highlight snapshot; see `SyntaxHighlightCache`.
    pub(crate) syntax_highlight_cache: RefCell<Option<SyntaxHighlightCache>>,
    /// At most one background highlight parse at a time, like folding.
    pub(crate) syntax_refresh_handle: Option<SpawnedFutureHandle>,
    pub(crate) fold_visibility_cache: RefCell<Option<FoldVisibilityCache>>,
    pub(crate) fold_state: FoldState,
    /// Shared by every visible folding cell. Entering the folding column on
    /// any row reveals all fold points in the viewport, as in VS Code.
    pub(crate) fold_gutter_hover: MouseStateHandle,
    /// Buffer-search matches in character offsets. Kept as presentation state
    /// rather than document state: opening/closing Find never dirties the file.
    pub(crate) search_matches: Vec<std::ops::Range<usize>>,
    pub(crate) active_search_match: Option<usize>,
    pub(crate) source_line_changes: std::collections::HashMap<usize, LineChangeKind>,
    /// Optional Git-specific display transform. It references real current
    /// buffer rows and adds virtual deleted/collapsed rows without changing
    /// `buffer` or its character coordinates.
    pub(crate) diff_comment_drag: Option<(usize, bool)>,
    /// The diff line under the pointer, whose gutter shows the comment +.
    pub(crate) diff_comment_hover_line: Option<(usize, bool)>,
    pub(crate) git_diff_projection: Option<GitDiffProjection>,
    pub(crate) source_diagnostic_lines: std::collections::HashMap<usize, DiagnosticSeverity>,
    pub(crate) source_diagnostics: Vec<ResolvedDiagnostic>,
    pub(crate) source_diagnostics_by_line: std::collections::HashMap<usize, Vec<usize>>,
    pub(crate) diagnostic_mouse_states: Vec<MouseStateHandle>,
    pub(crate) active_diagnostic: Option<usize>,
    /// Undo history: (buffer_snapshot, cursor_snapshot, chip_spans_snapshot), oldest first.
    pub(crate) undo_stack: Vec<(String, usize, Vec<ChipSpan>)>,
    /// Redo history populated when undoing; cleared on any new edit.
    pub(crate) redo_stack: Vec<(String, usize, Vec<ChipSpan>)>,
    /// Vim-style modal editing for terminal command input.
    pub(crate) vim_enabled: bool,
    pub(crate) vim_state: VimState,
    /// Highlight range for active workflow argument (byte offsets).
    pub(crate) workflow_arg_highlight: Option<std::ops::Range<usize>>,
    /// Persistent hint explaining current vim mode.
    pub(crate) vim_status_hint: Option<String>,
    /// When true the input is routed to the agent — suppress shell syntax
    /// highlighting so text renders as plain white.
    pub(crate) is_agent_mode: bool,
    /// Suppress shell syntax highlighting (green commands etc.) WITHOUT the
    /// other agent-mode behaviour (@-mentions, Changed-event gating). Set for
    /// the AI-tool rich input, which is always a plain prompt on dark chrome.
    pub(crate) suppress_command_highlight: bool,
    /// When true, each character is rendered as a bullet (password masking).
    pub(crate) secure: bool,
    /// `@`-mention chips embedded in `buffer`, sorted by `range.start`, non-overlapping.
    pub(crate) chip_spans: Vec<ChipSpan>,
    /// Char index of an open `@` trigger (the `@` itself), while its popup query is live.
    pub(crate) active_mention: Option<usize>,
    /// Candidates for the open mention popup, pushed in by the parent view
    /// via `set_mention_candidates` in response to `MentionQueryChanged`.
    pub(crate) mention_candidates: Vec<MentionEntry>,
    /// Keyboard-selected row in `mention_candidates`.
    pub(crate) mention_selected: usize,
    /// Parent-space anchor for the editor context menu. The popup is positioned
    /// by WarpUI's window-aware overlay layer, outside the scrollable canvas so
    /// it never scrolls with source.
    pub(crate) context_menu_position: Option<Vector2F>,
    pub(crate) context_menu_cursor: Option<usize>,
    pub(crate) context_menu_selected: usize,
    pub(crate) language_navigation_capabilities: LanguageNavigationCapabilities,
    pub(crate) definition_modifier_held: bool,
    pub(crate) definition_hover: Option<DefinitionHoverState>,
    pub(crate) definition_hover_request_id: u64,
    pub(crate) definition_click_candidate: Option<DefinitionClickCandidate>,
    pub(crate) last_pointer_position: Option<Vector2F>,
    pub(crate) definition_hover_mouse_state: MouseStateHandle,
    /// Bounded scroll state for the context-menu popover. The menu must stay
    /// compact like terminal popovers instead of extending beyond the window.
    pub(crate) context_menu_scroll: ClippedScrollStateHandle,
    pub(crate) context_menu_mouse_states:
        RefCell<std::collections::HashMap<usize, MouseStateHandle>>,
    /// Inline widget anchored to a character range in this document (e.g. the
    /// VS Code-style rename input rendered directly under the symbol).
    pub(crate) inline_rename: Option<InlineRenameState>,
}

/// A small editor-backed widget anchored under a character range of this
/// (source) editor. The host renders it as a positioned overlay that tracks
/// scrolling, sized just wider than the anchored range.
impl Entity for InputEditor {
    type Event = InputEditorEvent;
}

pub fn keystroke_to_char(keystroke: &Keystroke) -> Option<char> {
    if keystroke.cmd || keystroke.ctrl || keystroke.alt || keystroke.meta {
        return None;
    }

    if keystroke.key == "space" {
        return Some(' ');
    }

    if keystroke.key.chars().count() == 1 {
        let mut c = keystroke.key.chars().next().unwrap();
        // Newline characters from IME text-input events must not be re-inserted here;
        // shift+enter is already handled by the "enter" branch above.
        if c == '\n' || c == '\r' {
            return None;
        }
        // Shift and CapsLock both uppercase letters; together they cancel.
        if (keystroke.shift ^ keystroke.caps_lock) && c.is_ascii_lowercase() {
            c = c.to_ascii_uppercase();
        }
        return Some(c);
    }
    None
}

pub(crate) fn context_menu_alpha(mut color: ColorU, alpha: f32) -> ColorU {
    color.a = ((color.a as f32) * alpha.clamp(0.0, 1.0)) as u8;
    color
}

#[cfg(test)]
mod performance_tests {
    use super::*;

    #[test]
    fn multiline_document_editor_publishes_buffer_changes() {
        assert!(InputMode::MultiLine {
            key: "project_document_editor".into(),
            max_height: 2_000,
        }
        .publishes_buffer_events());
    }

    #[test]
    fn incremental_lexical_state_matches_a_full_document_scan() {
        let before = "const first = true;\nconst second = false;\nconst third = true;";
        let previous =
            dragent_editor::LexicalHighlighter::line_start_states(Language::TypeScript, before);
        let after = "const first = \"an open string\nconst second = false;\nconst third = true;\"";

        let incremental = lexical_line_start_states_after_edit(
            Language::TypeScript,
            after,
            Some(&previous),
            0,
            false,
        );
        let full =
            dragent_editor::LexicalHighlighter::line_start_states(Language::TypeScript, after);

        assert_eq!(incremental, full);
    }

    #[test]
    fn lexical_state_cache_keeps_a_trailing_empty_physical_line() {
        let states =
            dragent_editor::LexicalHighlighter::line_start_states(Language::Rust, "let x = 1;\n");
        assert_eq!(states.len(), 2);
    }

    #[test]
    fn large_source_index_keeps_utf8_byte_and_character_offsets_separate() {
        let text = "fn main() {\n    let emoji = \"😀\";\n}\n";
        let index = LargeSourceLineIndex::build(text, 1, Language::TypeScript);

        assert_eq!(index.line_count(), 4);
        assert_eq!(index.line_for_char(12), 1);
        let (_, _, line_length) = index.line_range(text, 1).unwrap();
        assert_eq!(line_length, 20);
        assert_eq!(index.total_chars, text.chars().count());
    }

    #[test]
    fn final_source_line_keeps_its_last_character() {
        let text = "first\nfinal_symbol";
        let index = LargeSourceLineIndex::build(text, 1, Language::TypeScript);
        let (byte_start, byte_end, char_length) = index.line_range(text, 1).unwrap();

        assert_eq!(&text[byte_start..byte_end], "final_symbol");
        assert_eq!(char_length, "final_symbol".chars().count());
    }

    #[test]
    fn definition_hover_expands_only_identifier_tokens() {
        let line = "  <AcceptInviteForm value={other_value} />";
        assert_eq!(identifier_range_in_line(line, 100, 106), Some(103..119));
        assert_eq!(identifier_range_in_line(line, 100, 127), Some(127..138));
        assert_eq!(identifier_range_in_line(line, 100, 102), None);
    }

    #[test]
    fn platform_definition_modifier_matches_desktop_convention() {
        if cfg!(target_os = "macos") {
            assert!(is_platform_definition_modifier(&KeyCode::SuperLeft));
            assert!(!is_platform_definition_modifier(&KeyCode::ControlLeft));
        } else {
            assert!(is_platform_definition_modifier(&KeyCode::ControlLeft));
            assert!(!is_platform_definition_modifier(&KeyCode::SuperLeft));
        }
    }

    #[test]
    fn pathological_large_source_lines_are_bounded_before_text_shaping() {
        let ascii = "x".repeat(LARGE_SOURCE_MAX_SHAPED_LINE_BYTES * 8);
        assert_eq!(
            bounded_large_source_line(&ascii).len(),
            LARGE_SOURCE_MAX_SHAPED_LINE_BYTES
        );

        let unicode = format!(
            "{}😀tail",
            "x".repeat(LARGE_SOURCE_MAX_SHAPED_LINE_BYTES - 1)
        );
        let bounded = bounded_large_source_line(&unicode);
        assert!(bounded.len() <= LARGE_SOURCE_MAX_SHAPED_LINE_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn multiline_rendering_is_bounded_by_the_viewport() {
        let range = multiline_render_range(100_000, 50_000.0, 700.0, 20.0);

        assert!(range.start > 0);
        assert!(range.end < 100_000);
        assert!(range.len() <= 1 + 35 + MULTILINE_OVERSCAN_ROWS * 2);
    }

    #[test]
    fn multiline_rendering_clamps_at_document_edges() {
        assert_eq!(multiline_render_range(5, -100.0, 700.0, 20.0), 0..5);

        let range = multiline_render_range(100, 10_000.0, 700.0, 20.0);
        assert_eq!(range.end, 100);
        assert_eq!(range.len(), 36 + MULTILINE_OVERSCAN_ROWS);
    }

    #[test]
    fn multiline_follow_bottom_fills_the_viewport() {
        let range = multiline_render_range(10_000, f32::MAX, 700.0, 20.0);

        assert_eq!(range.end, 10_000);
        assert!(range.len() >= 36);
    }

    #[test]
    fn folded_visual_indices_are_invalidated_by_resize() {
        let cache = FoldVisibilityCache {
            revision: 7,
            fold_revision: 2,
            available_width: 900.0,
            visual_indices: Rc::new(vec![0, 1, 2]),
        };

        assert!(cache.matches(7, 2, 900.0));
        assert!(!cache.matches(7, 2, 500.0));
    }

    #[test]
    fn source_hit_testing_tracks_horizontal_scroll() {
        let viewport_x = 100.0;
        let gutter_width = editor_render::gutter_width_for_lines(99, SOURCE_EDITOR_FONT_SIZE);
        let pointer_x = viewport_x + gutter_width + EDITOR_CONTENT_PADDING_LEFT + 24.0;

        assert!((source_content_x(pointer_x, viewport_x, gutter_width, 0.0) - 24.0).abs() < 0.01);
        assert!((source_content_x(pointer_x, viewport_x, gutter_width, 80.0) - 104.0).abs() < 0.01);
    }

    #[test]
    fn gutter_width_grows_with_the_final_line_number() {
        let small = editor_render::gutter_width_for_lines(999, SOURCE_EDITOR_FONT_SIZE);
        let big = editor_render::gutter_width_for_lines(1100, SOURCE_EDITOR_FONT_SIZE);
        // One extra digit column at the editor's number-text size.
        let number_font_size = (SOURCE_EDITOR_FONT_SIZE - 1.0).max(10.0);
        assert!((big - small - number_font_size * 0.66).abs() < 0.01);
        assert_eq!(editor_render::gutter_digits_for_max_line(1), 3);
        assert_eq!(editor_render::gutter_digits_for_max_line(999), 3);
        assert_eq!(editor_render::gutter_digits_for_max_line(1100), 4);
        assert_eq!(editor_render::gutter_digits_for_max_line(100_000), 6);
    }

    #[test]
    fn inline_rename_anchor_sits_below_the_symbol_row() {
        let line_height = 24.0;
        let gutter_width = editor_render::gutter_width_for_lines(99, SOURCE_EDITOR_FONT_SIZE);
        let (offset, symbol_width) =
            inline_rename_anchor(3, 0.0, 0.0, line_height, gutter_width, 40.0, 88.0);

        // Left edge aligns with the symbol start inside the content area.
        assert_eq!(
            offset.x(),
            gutter_width + EDITOR_CONTENT_PADDING_LEFT + 40.0
        );
        // The widget hangs directly under the symbol's row with a 1px gap.
        assert_eq!(offset.y(), 3.0 * line_height + line_height + 1.0);
        // The symbol's pixel extent is reported so the input can hug it.
        assert_eq!(symbol_width, 48.0);
    }

    #[test]
    fn inline_rename_anchor_counteracts_document_scrolling() {
        let gutter_width = editor_render::gutter_width_for_lines(99, SOURCE_EDITOR_FONT_SIZE);
        let (offset, _) = inline_rename_anchor(2, 120.0, 60.0, 20.0, gutter_width, 10.0, 50.0);
        let unscrolled = inline_rename_anchor(2, 0.0, 0.0, 20.0, gutter_width, 10.0, 50.0);

        // Vertical scroll moves the row up; horizontal scroll shifts content
        // left, and both are compensated so the widget tracks the symbol.
        assert_eq!(offset.y() - unscrolled.0.y(), -120.0);
        assert_eq!(offset.x() - unscrolled.0.x(), -60.0);
    }

    #[test]
    fn inline_rename_anchor_clamps_inverted_ranges() {
        let gutter_width = editor_render::gutter_width_for_lines(99, SOURCE_EDITOR_FONT_SIZE);
        let (_, symbol_width) = inline_rename_anchor(0, 0.0, 0.0, 24.0, gutter_width, 80.0, 40.0);
        assert_eq!(symbol_width, 0.0);
    }

    #[test]
    fn inline_rename_input_width_hugs_the_typed_name_and_symbol() {
        // Starts out just a little wider than the original symbol.
        assert_eq!(inline_rename_input_width(100.0, 10.0), 135.0);
        // Grows once the typed name outgrows it.
        assert_eq!(inline_rename_input_width(100.0, 200.0), 214.0);
        // Never collapses below a usable minimum for short symbols.
        assert_eq!(
            inline_rename_input_width(30.0, 4.0),
            INLINE_RENAME_MIN_WIDTH
        );
    }

    #[test]
    fn import_specifier_is_one_string_link_not_one_per_segment() {
        let line: Vec<char> = "import { X } from \"./a/b/c\";".chars().collect();
        // Content span is 19..26 ("./a/b/c" without the quotes).
        for local in 19..26 {
            let (open, close) = string_link_span_in_line(&line, local).unwrap();
            let content: String = line[open + 1..close].iter().collect();
            assert_eq!(content, "./a/b/c", "local={local}");
        }
        // Hovering outside the string is not a link.
        assert!(string_link_span_in_line(&line, 0).is_none());
        assert!(string_link_span_in_line(&line, 14).is_none()); // inside `from`
    }

    #[test]
    fn string_links_reject_non_path_strings() {
        // A plain package name is not a local path link.
        let line: Vec<char> = "import react from \"react\";".chars().collect();
        assert!(string_link_span_in_line(&line, 19).is_none());
        // Prose with a slash is not a specifier.
        let prose: Vec<char> = "const note = \"and/or other things\";".chars().collect();
        assert!(string_link_span_in_line(&prose, 17).is_none());
        // A relative path is.
        let path: Vec<char> = "export * from \"../shared/util\";".chars().collect();
        let (open, close) = string_link_span_in_line(&path, 20).unwrap();
        let content: String = path[open + 1..close].iter().collect();
        assert_eq!(content, "../shared/util");
    }

    #[test]
    fn string_link_span_includes_hover_on_quotes() {
        let line: Vec<char> = "from \"./a/b\";".chars().collect();
        // Opening quote at 5, closing at 11.
        assert!(string_link_span_in_line(&line, 5).is_some());
        assert!(string_link_span_in_line(&line, 11).is_some());
        // Second string on the same line still pairs correctly.
        let two: Vec<char> = "import \"./a\"; import \"./b/c\";".chars().collect();
        let (open, close) = string_link_span_in_line(&two, 26).unwrap();
        let content: String = two[open + 1..close].iter().collect();
        assert_eq!(content, "./b/c");
    }

    #[test]
    fn context_menu_shortcuts_match_their_keystrokes_exactly() {
        let key = |key: &str, cmd: bool, ctrl: bool, alt: bool, shift: bool| Keystroke {
            ctrl,
            alt,
            shift,
            cmd,
            meta: false,
            key: key.to_string(),
            caps_lock: false,
        };

        // Bare function key: exact modifiers required.
        assert!(context_menu_shortcut_matches(
            "F2",
            &key("f2", false, false, false, false)
        ));
        assert!(!context_menu_shortcut_matches(
            "F2",
            &key("f2", true, false, false, false)
        ));
        // ⌘ F12 (Go to Implementations).
        assert!(context_menu_shortcut_matches(
            "⌘ F12",
            &key("f12", true, false, false, false)
        ));
        assert!(!context_menu_shortcut_matches(
            "⌘ F12",
            &key("f12", false, false, false, false)
        ));
        // ⇧ F12 (Go to References) must not fire for ⌘F12.
        assert!(context_menu_shortcut_matches(
            "⇧ F12",
            &key("F12", false, false, false, true)
        ));
        assert!(!context_menu_shortcut_matches(
            "⇧ F12",
            &key("f12", true, false, false, false)
        ));
        // ⌥ ⇧ F (Format Document).
        assert!(context_menu_shortcut_matches(
            "⌥ ⇧ F",
            &key("f", false, false, true, true)
        ));
        // ⌘ Z and the Windows-style "Ctrl Z" label.
        assert!(context_menu_shortcut_matches(
            "⌘ Z",
            &key("z", true, false, false, false)
        ));
        assert!(context_menu_shortcut_matches(
            "Ctrl Z",
            &key("z", false, true, false, false)
        ));
        // Letters must not cross-match.
        assert!(!context_menu_shortcut_matches(
            "⌘ Z",
            &key("y", true, false, false, false)
        ));
    }

    #[test]
    fn toggle_line_comment_comments_a_single_line() {
        let chars: Vec<char> = "let x = 1;".chars().collect();
        let (next, edits) = compute_line_comment_toggle(&chars, &[0], "//").unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "// let x = 1;");
        assert_eq!(edits, vec![(0, 3)]);
    }

    #[test]
    fn toggle_line_comment_round_trips() {
        let chars: Vec<char> = "let x = 1;".chars().collect();
        let (commented, _) = compute_line_comment_toggle(&chars, &[0], "//").unwrap();
        let (back, _) = compute_line_comment_toggle(&commented, &[0], "//").unwrap();
        let text: String = back.into_iter().collect();
        assert_eq!(text, "let x = 1;");
    }

    #[test]
    fn toggle_line_comment_preserves_indentation() {
        let chars: Vec<char> = "    let x = 1;".chars().collect();
        let (next, _) = compute_line_comment_toggle(&chars, &[0], "//").unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "    // let x = 1;");
    }

    #[test]
    fn toggle_line_comment_uncomments_a_no_space_prefix() {
        // `//text` (no space after the marker) is still recognized as
        // commented and cleanly uncommented, matching VS Code.
        let chars: Vec<char> = "//let x = 1;".chars().collect();
        let (next, _) = compute_line_comment_toggle(&chars, &[0], "//").unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "let x = 1;");
    }

    #[test]
    fn toggle_line_comment_over_multiple_lines_comments_all_when_any_is_uncommented() {
        let chars: Vec<char> = "// a\nb\n// c".chars().collect();
        let line_starts = vec![0, 5, 7];
        let (next, _) = compute_line_comment_toggle(&chars, &line_starts, "//").unwrap();
        let text: String = next.into_iter().collect();
        // Every touched line gets commented, including the two already-commented
        // ones, because "uncomment" only fires when ALL touched lines already
        // match — here `b` doesn't.
        assert_eq!(text, "// // a\n// b\n// // c");
    }

    #[test]
    fn toggle_line_comment_over_multiple_lines_uncomments_when_all_are_commented() {
        let chars: Vec<char> = "// a\n// b".chars().collect();
        let line_starts = vec![0, 5];
        let (next, _) = compute_line_comment_toggle(&chars, &line_starts, "//").unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "a\nb");
    }

    #[test]
    fn toggle_line_comment_skips_blank_lines() {
        let chars: Vec<char> = "a\n\nb".chars().collect();
        let line_starts = vec![0, 2, 3];
        let (next, _) = compute_line_comment_toggle(&chars, &line_starts, "#").unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "# a\n\n# b");
    }

    #[test]
    fn toggle_line_comment_uses_python_hash_prefix() {
        let chars: Vec<char> = "print(1)".chars().collect();
        let (next, _) = compute_line_comment_toggle(&chars, &[0], "#").unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "# print(1)");
    }

    #[test]
    fn toggle_line_comment_on_all_blank_lines_is_a_no_op() {
        let chars: Vec<char> = "   \n\t".chars().collect();
        assert!(compute_line_comment_toggle(&chars, &[0, 4], "//").is_none());
    }

    #[test]
    fn toggle_block_comment_wraps_then_unwraps_css() {
        let chars: Vec<char> = "color: red;".chars().collect();
        let (wrapped, _, _, _) =
            compute_block_comment_toggle(&chars, 0, chars.len(), "/*", "*/").unwrap();
        let wrapped_text: String = wrapped.iter().collect();
        assert_eq!(wrapped_text, "/* color: red; */");

        let (unwrapped, _, _, _) =
            compute_block_comment_toggle(&wrapped, 0, wrapped.len(), "/*", "*/").unwrap();
        let unwrapped_text: String = unwrapped.into_iter().collect();
        assert_eq!(unwrapped_text, "color: red;");
    }

    #[test]
    fn toggle_block_comment_html_uses_angle_delimiters() {
        let chars: Vec<char> = "<div></div>".chars().collect();
        let (wrapped, _, _, _) =
            compute_block_comment_toggle(&chars, 0, chars.len(), "<!--", "-->").unwrap();
        let text: String = wrapped.into_iter().collect();
        assert_eq!(text, "<!-- <div></div> -->");
    }

    #[test]
    fn toggle_block_comment_on_whitespace_only_span_is_a_no_op() {
        let chars: Vec<char> = "   ".chars().collect();
        assert!(compute_block_comment_toggle(&chars, 0, chars.len(), "/*", "*/").is_none());
    }

    #[test]
    fn highlight_on_a_horizontally_scrolled_row_reaches_the_visible_right_edge() {
        // The screenshot bug: viewport 900 wide, scrolled 600 right. A
        // selection continuing past this row used to end at content x 900,
        // i.e. 300px into the screen. It must span the whole visible range.
        let (visible_start, visible_end) = (600.0, 1_500.0);
        let continued_end = visible_end; // continued_range_end_x on a short line
        assert_eq!(
            clip_highlight_span(0.0, continued_end, visible_start, visible_end),
            Some((600.0, 1_500.0))
        );
    }

    #[test]
    fn highlight_is_clipped_so_it_never_exceeds_the_viewport() {
        // A long selected line far wider than the window: clipped to exactly
        // the visible range, so the window-width layout clamp never bites.
        assert_eq!(
            clip_highlight_span(0.0, 20_000.0, 5_000.0, 5_900.0),
            Some((5_000.0, 5_900.0))
        );
    }

    #[test]
    fn highlight_entirely_off_screen_is_not_drawn() {
        assert_eq!(clip_highlight_span(0.0, 100.0, 600.0, 1_500.0), None);
        assert_eq!(clip_highlight_span(2_000.0, 2_100.0, 600.0, 1_500.0), None);
    }

    #[test]
    fn empty_selected_line_on_screen_stays_drawable() {
        assert_eq!(clip_highlight_span(0.0, 0.0, 0.0, 900.0), Some((0.0, 0.0)));
    }

    #[test]
    fn move_line_up_swaps_with_the_previous_line() {
        let chars: Vec<char> = "one\ntwo\nthree".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 4, 6); // "two"
        let (next, delta) = compute_move_line(&chars, block_start, block_end, true).unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "two\none\nthree");
        // Cursor at the "t" of "two" (index 4) should land on the "t" of the
        // now-shifted-up "two" (index 0).
        assert_eq!(4 + delta, 0);
    }

    #[test]
    fn move_line_down_swaps_with_the_next_line() {
        let chars: Vec<char> = "one\ntwo\nthree".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 0, 2); // "one"
        let (next, delta) = compute_move_line(&chars, block_start, block_end, false).unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "two\none\nthree");
        assert_eq!(0 + delta, 4);
    }

    #[test]
    fn move_line_up_at_the_first_line_is_a_no_op() {
        let chars: Vec<char> = "one\ntwo".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 0, 0);
        assert!(compute_move_line(&chars, block_start, block_end, true).is_none());
    }

    #[test]
    fn move_line_down_at_the_last_line_is_a_no_op() {
        let chars: Vec<char> = "one\ntwo".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 5, 5);
        assert!(compute_move_line(&chars, block_start, block_end, false).is_none());
    }

    #[test]
    fn move_line_preserves_a_multi_line_selection_as_one_block() {
        let chars: Vec<char> = "one\ntwo\nthree\nfour".chars().collect();
        // Selection spans "two" and "three".
        let (block_start, block_end) = touched_line_block(&chars, 4, 13);
        let (next, _) = compute_move_line(&chars, block_start, block_end, false).unwrap();
        let text: String = next.into_iter().collect();
        assert_eq!(text, "one\nfour\ntwo\nthree");
    }

    #[test]
    fn delete_line_removes_the_line_and_one_newline() {
        let chars: Vec<char> = "one\ntwo\nthree".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 4, 6); // "two"
        let next = compute_delete_line(&chars, block_start, block_end);
        let text: String = next.into_iter().collect();
        assert_eq!(text, "one\nthree");
    }

    #[test]
    fn delete_last_line_takes_the_leading_newline_not_a_trailing_one() {
        let chars: Vec<char> = "one\ntwo".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 4, 6); // "two"
        let next = compute_delete_line(&chars, block_start, block_end);
        let text: String = next.into_iter().collect();
        assert_eq!(text, "one");
    }

    #[test]
    fn delete_the_only_line_leaves_an_empty_buffer() {
        let chars: Vec<char> = "solo".chars().collect();
        let next = compute_delete_line(&chars, 0, 4);
        assert!(next.is_empty());
    }

    #[test]
    fn duplicate_line_below_inserts_a_copy_after_and_cursor_follows_it() {
        let chars: Vec<char> = "one\ntwo".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 0, 2); // "one"
        let (next, delta) = compute_duplicate_line(&chars, block_start, block_end, true);
        let text: String = next.into_iter().collect();
        assert_eq!(text, "one\none\ntwo");
        // Cursor at index 1 (inside "one") should land inside the *new* copy.
        assert_eq!(1 + delta, 5);
    }

    #[test]
    fn duplicate_line_above_inserts_a_copy_before_and_cursor_stays_on_the_text() {
        let chars: Vec<char> = "one\ntwo".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 0, 2); // "one"
        let (next, delta) = compute_duplicate_line(&chars, block_start, block_end, false);
        let text: String = next.into_iter().collect();
        assert_eq!(text, "one\none\ntwo");
        // Cursor at index 1 should stay on the same text, now shifted down.
        assert_eq!(1 + delta, 5);
    }

    #[test]
    fn duplicate_last_line_with_no_trailing_newline() {
        let chars: Vec<char> = "one\ntwo".chars().collect();
        let (block_start, block_end) = touched_line_block(&chars, 4, 6); // "two"
        let (next, _) = compute_duplicate_line(&chars, block_start, block_end, true);
        let text: String = next.into_iter().collect();
        assert_eq!(text, "one\ntwo\ntwo");
    }

    #[test]
    fn active_line_counteracts_horizontal_document_translation() {
        let viewport_x = 208.0;
        for horizontal_scroll in [0.0, 240.0, 1_200.0] {
            let scrolled_row_origin = vec2f(viewport_x - horizontal_scroll, 320.0);
            let highlight =
                active_line_viewport_rect(scrolled_row_origin, horizontal_scroll, 1_440.0, 24.0);

            assert_eq!(highlight.min_x(), viewport_x);
            assert_eq!(highlight.min_y(), 320.0);
            assert_eq!(highlight.height(), 24.0);
        }
    }

    #[test]
    fn manually_scrolled_source_view_detects_an_offscreen_caret() {
        assert!(visual_row_is_visible(50, 900.0, 300.0, 20.0));
        assert!(!visual_row_is_visible(10, 900.0, 300.0, 20.0));
        assert!(!visual_row_is_visible(70, 900.0, 300.0, 20.0));
    }

    #[test]
    fn source_hit_testing_clamps_clicks_in_the_fixed_gutter() {
        let gutter_width = editor_render::gutter_width_for_lines(99, SOURCE_EDITOR_FONT_SIZE);
        assert_eq!(source_content_x(120.0, 100.0, gutter_width, 0.0), 0.0);
    }

    #[test]
    fn wrapped_input_hit_testing_does_not_count_the_row_start_twice() {
        // The second visual row begins at character 12 in both the flattened
        // buffer and the physical-line layout. Clicking its third character
        // must resolve to 15, not 27.
        assert_eq!(wrapped_row_absolute_index(12, 12, 15), 15);
    }

    #[test]
    fn cursor_in_a_folded_gap_uses_the_preceding_visible_row() {
        let rows = vec![
            ("before fold".to_string(), 0),
            ("after fold".to_string(), 40),
        ];

        // A cursor in the hidden source range must not resolve to the later
        // row, whose starting offset is greater than the cursor position.
        assert_eq!(InputEditor::cursor_visual_row(20, &rows), 0);
    }

    #[test]
    fn source_newline_indent_preserves_and_extends_block_indentation() {
        // Hermetic width: the global settings (2 spaces on some machines)
        // must not leak into expectations.
        let rust: Vec<char> = "    if ready {".chars().collect();
        assert_eq!(
            source_newline_indent_with(&rust, rust.len(), Language::Rust, "    "),
            "        "
        );

        let python: Vec<char> = "  if ready:".chars().collect();
        assert_eq!(
            source_newline_indent_with(&python, python.len(), Language::Python, "    "),
            "      "
        );

        // The configured width flows through, not just the default.
        assert_eq!(
            source_newline_indent_with(&rust, rust.len(), Language::Rust, "  "),
            "      "
        );
    }

    #[test]
    fn guide_rectangles_snap_to_device_pixels() {
        let snapped = pixel_snap_rect(RectF::new(vec2f(2.26, 3.24), vec2f(1.0, 20.02)), 2.0);
        assert_eq!(snapped, RectF::new(vec2f(2.5, 3.0), vec2f(1.0, 20.5)));
    }

    fn display_grid(rows: &[(usize, Option<usize>)]) -> SourceDisplayGrid {
        SourceDisplayGrid {
            source_rows: rows
                .iter()
                .enumerate()
                .map(
                    |(display_row, (source_line, first_content_column))| SourceDisplayRow {
                        source_line: *source_line,
                        display_row,
                        first_content_column: *first_content_column,
                    },
                )
                .collect(),
            total_rows: rows.len(),
        }
    }

    #[test]
    fn tab_indented_guides_use_the_same_column_measure_as_rows() {
        // Go, tab-indented, with Tab size 2 (anything but the fold index's 4):
        //   1 \tswitch x {      <- opener, guide at its indent
        //   2 \tcase 1:          <- same level: text sits on the guide
        //   3 \t\tif y {         <- deeper: guide visible
        //   4 \t\t\tbreak
        //   5 \t\t}
        //   6 \t}
        let lines = [
            "\tswitch x {",
            "\tcase 1:",
            "\t\tif y {",
            "\t\t\tbreak",
            "\t\t}",
            "\t}",
        ];
        let tab = 2;
        let rows: Vec<(usize, Option<usize>)> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| (i + 1, first_source_content_column(line, tab)))
            .collect();
        let grid = display_grid(&rows);
        let guide_column = first_source_content_column(lines[0], tab).unwrap();
        // Rows 3–5 (display rows 2..=4) show the switch guide; the `case`
        // row hides it. The old mixed measure (guide at 4, rows at 2 per tab)
        // hid it on rows 3 and 5 too.
        assert_eq!(grid.visible_guide_runs(2, 5, guide_column), vec![(2, 4)]);
    }

    #[test]
    fn indentation_is_detected_from_the_document() {
        assert_eq!(
            detect_indent_unit("func main() {\n\tif ok {\n\t\treturn\n\t}\n}\n").as_deref(),
            Some("\t")
        );
        assert_eq!(
            detect_indent_unit("function a() {\n  if (b) {\n    c();\n  }\n}\n").as_deref(),
            Some("  ")
        );
        assert_eq!(
            detect_indent_unit("fn a() {\n    if b {\n        c();\n    }\n}\n").as_deref(),
            Some("    ")
        );
        // Block-comment continuations (` * `) don't vote for 1-space.
        assert_eq!(
            detect_indent_unit("/**\n * doc\n */\nfn a() {\n    b();\n}\n").as_deref(),
            Some("    ")
        );
        assert_eq!(detect_indent_unit("no indentation\nhere\n"), None);
    }

    #[test]
    fn go_enter_after_brace_indents_with_a_tab() {
        let go: Vec<char> = "\tif ok {".chars().collect();
        assert_eq!(
            source_newline_indent_with(&go, go.len(), Language::Go, "\t"),
            "\t\t"
        );
        assert_eq!(
            crate::workspace::smart_indent::newline_indent_with(&go, go.len(), Language::Go, "\t")
                .indent,
            "\t\t"
        );
    }

    #[test]
    fn source_tokens_temporarily_occlude_a_logical_guide() {
        let grid = display_grid(&[
            (2, Some(4)),
            (3, None),
            (4, Some(0)),
            (5, None),
            (6, Some(4)),
        ]);

        // The token on source line 4 occupies column zero. The same logical
        // guide resumes at exactly the same column on the following row.
        assert_eq!(grid.visible_guide_runs(2, 6, 0), vec![(0, 1), (3, 4)]);
    }

    #[test]
    fn a_closing_token_hides_only_its_own_guide_column() {
        let grid = display_grid(&[(10, Some(8)), (11, Some(4)), (12, None)]);

        // A child closing token at column four leaves the parent column zero
        // unobstructed, while hiding and then resuming the child guide.
        assert_eq!(grid.visible_guide_runs(10, 12, 0), vec![(0, 2)]);
        assert_eq!(grid.visible_guide_runs(10, 12, 4), vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn unobstructed_and_blank_rows_form_one_full_height_run() {
        let grid = display_grid(&[(20, Some(8)), (21, None), (22, Some(8))]);
        assert_eq!(grid.visible_guide_runs(20, 22, 4), vec![(0, 2)]);
    }

    #[test]
    fn source_content_columns_expand_tabs_to_editor_tab_stops() {
        assert_eq!(first_source_content_column("\t\tvalue", 4), Some(8));
        assert_eq!(first_source_content_column("  \tvalue", 4), Some(4));
        assert_eq!(first_source_content_column("\t  ", 4), None);
    }

    #[test]
    fn multiline_tsx_guide_is_occluded_by_its_delimiter_without_ending() {
        let source = concat!(
            "function Component() {\n",
            "  return (\n",
            "    <button\n",
            "      onClick={handleClick}\n",
            "    >\n",
            "      Save\n",
            "    </button>\n",
            "  );\n",
            "}\n",
        );
        let syntax = FoldIndex::from_text(Language::TypeScript, source);
        let button = syntax
            .regions()
            .find(|region| region.start_line == 3 && region.end_line == 7)
            .expect("tree-sitter should identify the multiline JSX element");
        let guide_lines = button
            .guide_line_range()
            .expect("multiline JSX element has guide rows");
        assert_eq!(button.indent_column, 4);

        let rows = source
            .lines()
            .enumerate()
            .map(|(index, line)| {
                (
                    index + 1,
                    first_source_content_column(line, text::source_tab_size()),
                )
            })
            .collect::<Vec<_>>();
        let grid = display_grid(&rows);

        // Attribute row: visible. Standalone `>` at the guide column:
        // occluded. Content row: visible again at the exact same x. The
        // closing tag is outside this guide's syntax lifetime.
        assert_eq!(
            grid.visible_guide_runs(
                *guide_lines.start(),
                *guide_lines.end(),
                button.indent_column,
            ),
            vec![(3, 3), (5, 5)]
        );
    }

    #[test]
    fn copy_and_trim_removes_common_selected_indentation() {
        let source: Vec<char> = "    one\n      two\n    three".chars().collect();
        let result = trim_copy_indentation(&source, 0, source.len()).expect("multiline trim");
        assert_eq!(result, "one\n  two\nthree");
    }

    #[test]
    fn copy_and_trim_preserves_relative_structure_for_shallow_lines() {
        let source: Vec<char> = "    one\n  two\n    three".chars().collect();
        assert_eq!(trim_copy_indentation(&source, 0, source.len()), None);
    }

    #[test]
    fn copy_and_trim_does_not_change_single_line_selection() {
        let source: Vec<char> = "    one".chars().collect();
        assert_eq!(trim_copy_indentation(&source, 0, source.len()), None);
    }
}

#[cfg(test)]
mod empty_caret_tests {
    use std::collections::HashSet;

    use warpui::elements::{ChildView, ConstrainedBox, Element, Fill};
    use warpui::fonts::FamilyId;
    use warpui::geometry::vector::Vector2F;
    use warpui::{AppContext, Entity, View, ViewHandle};
    use warpui_core::platform::WindowStyle;
    use warpui_core::{App, Presenter, TypedActionView, WindowInvalidation};

    use super::*;
    use crate::theme::Theme;

    const CARET: ColorU = ColorU { r: 1, g: 2, b: 3, a: 255 };

    /// An empty multi-line input in a box much taller than one line.
    struct Harness {
        editor: ViewHandle<InputEditor>,
    }

    impl Entity for Harness {
        type Event = ();
    }

    impl View for Harness {
        fn ui_name() -> &'static str {
            "EmptyCaretHarness"
        }

        fn render(&self, _: &AppContext) -> Box<dyn Element> {
            ConstrainedBox::new(ChildView::new(&self.editor).finish())
                .with_width(300.0)
                .with_height(88.0)
                .finish()
        }
    }

    impl TypedActionView for Harness {
        type Action = ();
    }

    #[test]
    fn the_caret_of_an_empty_multiline_input_sits_on_its_first_line() {
        App::test((), |mut app| async move {
            let app = &mut app;
            let (window_id, harness) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                let editor = ctx.add_typed_action_view(|ec| {
                    InputEditor::new(
                        "What should the agent do with these lines?",
                        FamilyId(0),
                        13.0,
                        ColorU::white(),
                        ColorU::white(),
                        CARET,
                        Theme::dark(),
                        InputMode::MultiLine { key: "caret_test".into(), max_height: 88 },
                        ec.focus_handle(),
                    )
                });
                Harness { editor }
            });
            let editor = harness.read(app, |h, _| h.editor.clone());
            harness.update(app, |_, ctx| ctx.focus(&editor));
            editor.update(app, |e, _| e.cursor_visible = true);
            let line_height = editor.read(app, |e, _| e.font_size() * e.line_height_ratio());
            let mut presenter = Presenter::new(window_id);
            let invalidation = WindowInvalidation {
                updated: HashSet::from([app.root_view_id(window_id).unwrap(), editor.id()]),
                ..Default::default()
            };
            let scene = app.update(move |ctx| {
                presenter.invalidate(invalidation, ctx);
                presenter.build_scene(Vector2F::new(400.0, 300.0), 1.0, None, ctx)
            });
            let caret = scene
                .layers()
                .flat_map(|layer| layer.rects.iter())
                .find(|rect| matches!(rect.background, Fill::Solid(color) if color == CARET))
                .expect("the focused empty input draws a caret");
            assert!(
                caret.bounds.center().y() < line_height,
                "caret centre at y={} is below the first line (height {line_height})",
                caret.bounds.center().y()
            );
        });
    }
}
