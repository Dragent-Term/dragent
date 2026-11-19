//! Input editor state model.
//!
//! Responsible for: editor state records (modes, caches, grids, diffs,
//! menus, hover, chips, rename, carets) and editor actions/events.
//! Data and small impls only; no ViewContext, no rendering.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use warpui::color::ColorU;
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
use warpui::{
    AfterLayoutContext, AppContext, Entity, EventContext, FocusHandle, LayoutContext, PaintContext,
    SizeConstraint, TypedActionView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Hoverable, MouseStateHandle, SELECTED_HIGHLIGHT_COLOR};
use warpui_core::event::DispatchedEvent;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use dragent_document::{DocumentCapabilities, DocumentDiff, GitDiffDisplayRow};
use dragent_editor::{
    Breakpoint, BreakpointRegistry, DiagnosticSeverity, FoldIndex, FoldParser, FoldRegion,
    FoldState, Language, LineChangeKind, ResolvedDiagnostic,
};

use crate::workspace::editor_render::EDITOR_CONTENT_PADDING_LEFT;

use super::text::{
    PLAIN_CANVAS_RIGHT_PADDING, SOURCE_EDITOR_BOTTOM_PADDING, SOURCE_EDITOR_CURSOR_BOTTOM_MARGIN,
    SOURCE_EDITOR_FONT_SIZE, SOURCE_EDITOR_LINE_HEIGHT_RATIO, SOURCE_EDITOR_RIGHT_PADDING,
};
use super::InputEditor;
use crate::console::input_editing::{line_end, line_start};

#[derive(Clone, Debug)]
pub enum InputEditorAction {
    KeyDown(Keystroke),
    /// Open the editor context menu at a window-space pointer position.
    ContextMenuAt(Vector2F),
    /// Dismiss the editor context menu.
    CloseContextMenu,
    /// Move keyboard focus through enabled context-menu rows.
    MoveContextMenuSelection {
        direction: i32,
    },
    /// Activate the currently keyboard-selected context-menu row.
    ActivateContextMenuSelection,
    /// Invoke one of the editor commands exposed by the context menu.
    ContextMenuCommand(EditorContextMenuCommand),
    /// Click inside the editor canvas — resolved in `handle_action` with the live buffer.
    ClickAt(Vector2F),
    /// Click inside the dedicated folding lane. X is intentionally ignored;
    /// the whole lane is the chevron's hit target.
    /// Double-click: select the word under the cursor.
    DoubleClickAt(Vector2F),
    /// Triple-click: select all text in the buffer.
    TripleClickAt(Vector2F),
    /// Reveal a display-only unchanged range in a Git-focused editor view.
    ExpandGitDiffRegion {
        id: usize,
        /// Positive reveals downward from the preceding hunk; negative reveals
        /// upward from the following hunk.
        direction: i8,
    },
    /// Drag inside the editor to extend a text selection.
    DragAt(Vector2F),
    /// Mouse button released after click/drag.
    ReleaseAt(Vector2F),
    /// Pointer movement over the editor. Semantic work is gated by the
    /// platform definition modifier in the view state.
    DefinitionHoverAt(Vector2F),
    /// Pointer left the editor surface.
    ClearDefinitionHover,
    /// Command on macOS, Control on Windows/Linux.
    DefinitionModifierHeld(bool),
    /// Scroll the multiline editor. Positive values mean the user moved toward history.
    ScrollWheel(f32),
    /// Accept the ghost text suggestion
    AcceptGhost,
    /// Click a row in the open `@`-mention popup.
    ClickMentionEntry(usize),
    /// Open the persistent inline problem viewer for a diagnostic underline.
    OpenDiagnostic(usize),
    PreviousDiagnostic,
    NextDiagnostic,
    CloseDiagnostic,
}

#[derive(Clone, Debug)]
pub(crate) struct GitDiffProjection {
    pub(crate) diff: DocumentDiff,
    pub(crate) old_line_count: usize,
    pub(crate) new_line_count: usize,
    pub(crate) expanded_regions:
        std::collections::HashMap<usize, dragent_document::DiffRegionExpansion>,
    pub(crate) rows: Vec<GitDiffDisplayRow>,
    pub(crate) read_only: bool,
}

impl GitDiffProjection {
    pub(crate) fn new(
        diff: DocumentDiff,
        old_line_count: usize,
        new_line_count: usize,
        read_only: bool,
    ) -> Self {
        let expanded_regions = std::collections::HashMap::new();
        let rows =
            diff.display_rows_with_expansions(old_line_count, new_line_count, 1, &expanded_regions);
        Self {
            diff,
            old_line_count,
            new_line_count,
            expanded_regions,
            rows,
            read_only,
        }
    }

    pub(crate) fn expand(&mut self, id: usize, direction: i8) {
        const REVEAL_LINES: usize = 10;
        let Some(region) = self
            .diff
            .unchanged_regions(self.old_line_count, self.new_line_count, 1)
            .into_iter()
            .find(|region| region.id == id)
        else {
            return;
        };
        let expansion = self.expanded_regions.entry(id).or_default();
        if direction < 0 {
            expansion.trailing = (expansion.trailing + REVEAL_LINES).min(region.line_count);
        } else {
            expansion.leading = (expansion.leading + REVEAL_LINES).min(region.line_count);
        }
        self.rows = self.diff.display_rows_with_expansions(
            self.old_line_count,
            self.new_line_count,
            1,
            &self.expanded_regions,
        );
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct EditorViewState {
    pub cursor: usize,
    pub selection_anchor: Option<usize>,
    pub vertical_scroll: f32,
    pub horizontal_scroll: f32,
}

#[derive(Clone, Debug)]
pub enum InputEditorEvent {
    DiffCommentSelection { start: usize, end: usize, start_deleted: bool, end_deleted: bool },
    Changed {
        text: String,
        cursor: usize,
    },
    /// The compact Git projection gained visible rows (for example after an
    /// unchanged-region reveal). Aggregate diff parents use this to extend
    /// their single vertical document rather than creating an inner scroller.
    GitDiffProjectionRowsChanged {
        rows: usize,
    },
    Clicked(Vector2F),
    AcceptGhost,
    /// Request the parent to insert the last word from the previous command.
    InsertLastWord,
    /// Parent should navigate command history (↑/↓).
    HistoryNavigate {
        previous: bool,
    },
    /// An `@`-mention popup is open with a live query — the parent should
    /// fetch/score candidates and push them back via `set_mention_candidates`.
    MentionQueryChanged {
        anchor: usize,
        query: String,
    },
    /// The `@`-mention popup closed (accepted, cancelled, or cursor moved away).
    MentionClosed,
    /// A context-menu command owned by the document workspace rather than the
    /// text buffer itself (for example Markdown preview or the document
    /// terminal). Clipboard and edit commands are executed directly by the
    /// editor so they share the keyboard transaction path.
    ContextMenuCommand {
        command: EditorContextMenuCommand,
        cursor: usize,
    },
    /// Ask the language layer whether this lexical symbol has a semantic
    /// definition. The request id lets the editor reject late responses.
    /// `string_link` carries the import specifier when the hovered range is a
    /// path-like string literal, so the workspace can resolve the target file
    /// even when the language server returns no definition for it.
    DefinitionHoverRequested {
        request_id: u64,
        cursor: usize,
        symbol_range: std::ops::Range<usize>,
        string_link: Option<String>,
    },
    /// Activate a previously validated semantic definition hover.
    DefinitionHoverClicked {
        request_id: u64,
        cursor: usize,
    },
    BreakpointsChanged {
        breakpoints: Vec<Breakpoint>,
    },
}

/// Commands that can be invoked from the editor's secondary-click menu.
/// Keeping this as a typed command is important: the menu is only another
/// input route into editor behavior, not a collection of UI-only callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorContextMenuCommand {
    GoToDefinition,
    GoToTypeDefinition,
    GoToSourceDefinition,
    GoToImplementation,
    GoToReferences,
    FindAllReferences,
    AddSymbolToChat,
    AddSymbolToNewChat,
    RenameSymbol,
    FormatDocument,
    ShowCodeActions,
    ToggleLineComment,
    Undo,
    Redo,
    Cut,
    Copy,
    CopyAndTrim,
    Paste,
    SelectAll,
    ToggleMarkdownPreview,
    ToggleDocumentTerminal,
    CommandPalette,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LanguageNavigationCapabilities {
    pub definition: bool,
    pub type_definition: bool,
    pub implementation: bool,
    pub references: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DefinitionHoverState {
    pub(crate) request_id: u64,
    pub(crate) symbol_range: std::ops::Range<usize>,
    pub(crate) resolved: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DefinitionClickCandidate {
    pub(crate) request_id: u64,
    pub(crate) symbol_range: std::ops::Range<usize>,
    pub(crate) anchor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextMenuEntry {
    Separator,
    Unavailable {
        label: &'static str,
        shortcut: Option<&'static str>,
        submenu: bool,
    },
    Action {
        label: &'static str,
        shortcut: Option<&'static str>,
        command: EditorContextMenuCommand,
        enabled: bool,
    },
}

pub(crate) fn context_menu_shortcut(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

/// Commands whose context-menu shortcuts also work without the menu open —
/// the language/navigation actions. Text-editing chords (undo, copy, paste,
/// select-all, …) are excluded: the editor's own keydown path already owns
/// those and gives them terminal-specific behavior that must be preserved.
pub(crate) fn is_global_editor_command(command: EditorContextMenuCommand) -> bool {
    use EditorContextMenuCommand::*;
    matches!(
        command,
        GoToDefinition
            | GoToTypeDefinition
            | GoToSourceDefinition
            | GoToImplementation
            | GoToReferences
            | FindAllReferences
            | AddSymbolToChat
            | AddSymbolToNewChat
            | RenameSymbol
            | FormatDocument
            | ShowCodeActions
            | ToggleLineComment
            | ToggleMarkdownPreview
            | ToggleDocumentTerminal
            | CommandPalette
    )
}

/// Match a pressed keystroke against a context-menu shortcut label such as
/// "F2", "⌘ F12", "⇧ F12", "⌥ ⇧ F" or the Windows-style "Ctrl ⇧ P".
/// Modifiers must match exactly, so plain "F12" never fires for ⌘F12.
pub(crate) fn context_menu_shortcut_matches(shortcut: &str, keystroke: &Keystroke) -> bool {
    let mut cmd = false;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut key: Option<&str> = None;
    for token in shortcut.split_whitespace() {
        match token {
            "⌘" => cmd = true,
            "⇧" => shift = true,
            "⌥" => alt = true,
            "⌃" | "Ctrl" => ctrl = true,
            other => key = Some(other),
        }
    }
    let Some(key) = key else {
        return false;
    };
    keystroke.cmd == cmd
        && keystroke.ctrl == ctrl
        && keystroke.alt == alt
        && keystroke.shift == shift
        && keystroke.key.eq_ignore_ascii_case(key)
}

pub(crate) fn is_platform_definition_modifier(key: &KeyCode) -> bool {
    if cfg!(target_os = "macos") {
        matches!(key, KeyCode::SuperLeft | KeyCode::SuperRight)
    } else {
        matches!(key, KeyCode::ControlLeft | KeyCode::ControlRight)
    }
}

/// Match the useful part of standard Copy and Trim behavior for this editor's
/// single selection model: when a multi-line selection starts in indentation,
/// remove the first line's common indentation from every selected non-empty
/// line. If a later line is less-indented, retain the original text rather than
/// damaging its relative structure.
pub(crate) fn trim_copy_indentation(chars: &[char], start: usize, end: usize) -> Option<String> {
    if start >= end || !chars[start..end].contains(&'\n') {
        return None;
    }
    let line_start = chars[..start]
        .iter()
        .rposition(|ch| *ch == '\n')
        .map_or(0, |index| index + 1);
    let first_indent = chars[line_start..]
        .iter()
        .take_while(|ch| **ch == ' ' || **ch == '\t')
        .count();
    if first_indent == 0 || start > line_start + first_indent {
        return None;
    }

    let mut line_ranges = Vec::new();
    let mut line_start_in_selection = start;
    for index in start..end {
        if chars[index] == '\n' {
            line_ranges.push(line_start_in_selection..index);
            line_start_in_selection = index + 1;
        }
    }
    if line_start_in_selection < end {
        line_ranges.push(line_start_in_selection..end);
    }

    let mut output = String::new();
    for (line_index, range) in line_ranges.iter().enumerate() {
        let line = &chars[range.clone()];
        let leading = line
            .iter()
            .take_while(|ch| **ch == ' ' || **ch == '\t')
            .count();
        if line.iter().any(|ch| !ch.is_whitespace()) && leading < first_indent {
            return None;
        }
        let trim = if line.iter().any(|ch| !ch.is_whitespace()) {
            first_indent.min(leading)
        } else {
            0
        };
        output.extend(line[trim..].iter());
        if line_index + 1 < line_ranges.len() {
            output.push('\n');
        }
    }
    Some(output)
}

/// Pure core of the toggle-line-comment command (Cmd+/ / Ctrl+/) for
/// languages with a per-line comment token (`//`, `#`, …). `line_starts` are
/// the char-index start of each touched line. Matches VS Code: if every
/// non-blank touched line is already commented, uncomment; otherwise comment
/// every touched (non-blank) line. Returns `None` when nothing changed
/// (e.g. every touched line is blank). The returned edits are `(position,
/// delta)` pairs for repositioning the cursor/selection after applying the
/// new buffer — see `handle_toggle_line_comment` in `mouse.rs`.
pub(crate) fn compute_line_comment_toggle(
    chars: &[char],
    line_starts: &[usize],
    prefix: &str,
) -> Option<(Vec<char>, Vec<(usize, isize)>)> {
    let prefix_chars: Vec<char> = prefix.chars().collect();
    let indent_end = |buf: &[char], line: usize| -> usize {
        line + buf[line..]
            .iter()
            .take_while(|c| **c == ' ' || **c == '\t')
            .count()
    };
    let is_commented = |line: usize| -> Option<bool> {
        let indent = indent_end(chars, line);
        if indent >= chars.len() || chars[indent] == '\n' {
            return None; // blank line — doesn't count either way
        }
        Some(chars[indent..].starts_with(prefix_chars.as_slice()))
    };
    let states: Vec<bool> = line_starts
        .iter()
        .filter_map(|&l| is_commented(l))
        .collect();
    let uncomment = !states.is_empty() && states.iter().all(|&commented| commented);

    let mut next = chars.to_vec();
    let mut edits: Vec<(usize, isize)> = Vec::new();
    for &line in line_starts.iter().rev() {
        let indent = indent_end(&next, line);
        if uncomment {
            if indent >= next.len() || !next[indent..].starts_with(prefix_chars.as_slice()) {
                continue;
            }
            let mut remove = prefix_chars.len();
            if next.get(indent + remove) == Some(&' ') {
                remove += 1;
            }
            next.drain(indent..indent + remove);
            edits.push((indent, -(remove as isize)));
        } else {
            if indent >= next.len() || next[indent] == '\n' {
                // Blank line (including a whitespace-only last line with no
                // trailing newline): leave it untouched rather than inserting
                // a trailing comment marker with nothing to comment.
                continue;
            }
            let insert: Vec<char> = prefix_chars.iter().copied().chain([' ']).collect();
            let delta = insert.len() as isize;
            next.splice(indent..indent, insert);
            edits.push((indent, delta));
        }
    }

    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(position, _)| *position);
    Some((next, edits))
}

/// Pure core of the toggle-comment command for languages with only a block
/// comment token (CSS `/* */`, HTML/Markdown `<!-- -->`). Wraps/unwraps the
/// trimmed `[span_start, span_end)` span as one unit — VS Code's behavior for
/// these languages, since there's no per-line marker to toggle. Returns
/// `None` when the span is empty/whitespace-only. On success, returns the new
/// buffer plus `(trimmed_start, trimmed_end, delta)` describing exactly what
/// changed, for cursor/selection repositioning — see
/// `handle_toggle_line_comment` in `mouse.rs`.
pub(crate) fn compute_block_comment_toggle(
    chars: &[char],
    span_start: usize,
    span_end: usize,
    open: &str,
    close: &str,
) -> Option<(Vec<char>, usize, usize, isize)> {
    if span_end <= span_start || span_end > chars.len() {
        return None;
    }
    let trimmed_start = span_start
        + chars[span_start..span_end]
            .iter()
            .take_while(|c| c.is_whitespace())
            .count();
    let trimmed_end = span_start
        + chars[span_start..span_end]
            .iter()
            .rposition(|c| !c.is_whitespace())
            .map(|index| index + 1)
            .unwrap_or(trimmed_start);
    if trimmed_end <= trimmed_start {
        return None;
    }

    let open_wrapped = format!("{open} ");
    let close_wrapped = format!(" {close}");
    let content: String = chars[trimmed_start..trimmed_end].iter().collect();
    let new_content = if content.starts_with(&open_wrapped) && content.ends_with(&close_wrapped) {
        content[open_wrapped.len()..content.len() - close_wrapped.len()].to_string()
    } else {
        format!("{open_wrapped}{content}{close_wrapped}")
    };
    if new_content == content {
        return None;
    }

    let mut next = chars.to_vec();
    let new_chars: Vec<char> = new_content.chars().collect();
    let delta = new_chars.len() as isize - (trimmed_end - trimmed_start) as isize;
    next.splice(trimmed_start..trimmed_end, new_chars);
    Some((next, trimmed_start, trimmed_end, delta))
}

/// Clip a highlight's `[x_start, x_end]` (row-content x) to the visible
/// horizontal range. `None` when it lies entirely off screen. A zero-width
/// span (an empty selected line) that's on screen stays, so the caller can
/// still draw it as a sliver.
pub(crate) fn clip_highlight_span(
    x_start: f32,
    x_end: f32,
    visible_start: f32,
    visible_end: f32,
) -> Option<(f32, f32)> {
    if x_end < visible_start || x_start > visible_end {
        return None;
    }
    Some((x_start.max(visible_start), x_end.min(visible_end)))
}

/// The whole-line span touched by `[start, end]`: from the start of the
/// earlier position's line to the end of the later position's line (not
/// including its trailing newline). Shared by every "operate on the current
/// line(s)" command below (move, delete, duplicate) so they all agree on
/// what "the touched lines" means for a selection spanning multiple lines —
/// and, since a caret with no selection is `start == end`, this is also just
/// "the current line" in the common case.
pub(crate) fn touched_line_block(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    let block_start = line_start(chars, start.min(end));
    let block_end = line_end(chars, start.max(end));
    (block_start, block_end)
}

/// Pure core of Move Line Up/Down (Alt+Up / Alt+Down): swaps the touched
/// line block with the line immediately above/below it. `None` at a buffer
/// edge (nothing to swap with).
///
/// Returns the new buffer and `delta`: cursor and selection-anchor
/// positions are always inside `[block_start, block_end]` by construction
/// (that's how the block was derived from them in the first place), and
/// this operation only ever *shifts* that block as a contiguous unit — it
/// never splits or reorders characters within it — so `new_position =
/// old_position + delta` is exact for both, not an approximation.
pub(crate) fn compute_move_line(
    chars: &[char],
    block_start: usize,
    block_end: usize,
    up: bool,
) -> Option<(Vec<char>, isize)> {
    let mut next = chars.to_vec();
    if up {
        if block_start == 0 {
            return None;
        }
        let prev_start = line_start(chars, block_start - 1);
        let prev_end = block_start - 1; // the newline sits at block_start - 1
        let block: Vec<char> = chars[block_start..block_end].to_vec();
        let prev_line: Vec<char> = chars[prev_start..prev_end].to_vec();
        let delta = -(prev_line.len() as isize + 1);
        let mut replacement = block;
        replacement.push('\n');
        replacement.extend(prev_line);
        next.splice(prev_start..block_end, replacement);
        Some((next, delta))
    } else {
        if block_end >= chars.len() {
            return None;
        }
        let next_start = block_end + 1; // skip the newline right after the block
        let next_end = line_end(chars, next_start);
        let block: Vec<char> = chars[block_start..block_end].to_vec();
        let next_line: Vec<char> = chars[next_start..next_end].to_vec();
        let delta = next_line.len() as isize + 1;
        let mut replacement = next_line;
        replacement.push('\n');
        replacement.extend(block);
        next.splice(block_start..next_end, replacement);
        Some((next, delta))
    }
}

/// Pure core of Delete Line (Cmd+Shift+K / Ctrl+Shift+K): removes the
/// touched line block along with one adjacent newline, so the line count
/// shrinks by exactly the lines removed rather than leaving a blank line
/// behind. Takes the *leading* newline instead of the trailing one when the
/// block is the last line in the buffer (there is no trailing one).
pub(crate) fn compute_delete_line(
    chars: &[char],
    block_start: usize,
    block_end: usize,
) -> Vec<char> {
    let mut next = chars.to_vec();
    if block_end < chars.len() {
        next.drain(block_start..block_end + 1);
    } else if block_start > 0 {
        next.drain(block_start - 1..block_end);
    } else {
        next.drain(block_start..block_end);
    }
    next
}

/// Pure core of Duplicate Line (Shift+Alt+Down / Shift+Alt+Up): inserts a
/// copy of the touched line block adjacent to itself. `below == true` places
/// the copy after the original; `false` places it before. Either way the
/// *content* the cursor was on ends up at `old_position + delta` — the copy
/// when duplicating down (so the cursor follows into the new line, matching
/// VS Code), or the now-shifted original when duplicating up (so the
/// cursor stays on the same text, which is now one block lower).
pub(crate) fn compute_duplicate_line(
    chars: &[char],
    block_start: usize,
    block_end: usize,
    below: bool,
) -> (Vec<char>, isize) {
    let mut next = chars.to_vec();
    let block: Vec<char> = chars[block_start..block_end].to_vec();
    let delta = block.len() as isize + 1;
    if below {
        let mut insertion = vec!['\n'];
        insertion.extend(block);
        next.splice(block_end..block_end, insertion);
    } else {
        let mut insertion = block;
        insertion.push('\n');
        next.splice(block_start..block_start, insertion);
    }
    (next, delta)
}

/// An `@`-mention chip embedded in the buffer: `buffer[range]` always equals
/// `label` literally — the chip is plain text with an out-of-band range
/// overlay, not a sentinel encoding. See `chip_at`/`clamp_off_chip`/
/// `rebase_chips` for how atomicity (single-unit move/delete) is enforced.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipSpan {
    pub range: std::ops::Range<usize>,
    pub path: std::path::PathBuf,
    pub label: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Search,
    RenameTab(usize),
    /// Inline rename of a top-level workspace tab (header strip). Carries
    /// the surface id; commits via `CommitRenameWorkspaceTab`.
    RenameWorkspaceTab(usize),
    LanguageRename {
        cursor: usize,
    },
    TerminalCommand,
    /// A plain single-line settings field. The String is a unique key used for
    /// element-bounds lookup AND as the first argument in the dispatched
    /// `SettingsAction::UpdateProviderKey(key, text)` action on every change.
    Settings(String),
    /// A plain multi-line editor: text wraps, Enter inserts a newline, vertically
    /// scrollable up to `max_height` pixels. The String is a unique position key.
    MultiLine {
        key: String,
        max_height: u32,
    },
    /// A plain single-line editor that emits [`InputEditorEvent::Changed`] on edit
    /// (so a parent can react) but has no command/search side effects. Enter and
    /// Escape are propagated to the parent. The String is a unique position key.
    Plain {
        key: String,
    },
}

impl InputMode {
    pub(crate) fn publishes_buffer_events(&self) -> bool {
        matches!(
            self,
            Self::TerminalCommand | Self::Plain { .. } | Self::MultiLine { .. }
        )
    }
}

/// Source of per-instance ids for `position_id`. Element positions live in one
/// flat cache keyed by that string, so two editors sharing an id would overwrite
/// each other's saved bounds — and clicks would hit-test against the wrong pane.
pub(crate) static NEXT_INPUT_EDITOR_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct VisualLine {
    pub(crate) text: String,
    pub(crate) abs_char_offset: usize,
    pub(crate) source_line: usize,
    pub(crate) continuation: bool,
    pub(crate) layout: std::rc::Rc<warpui_core::text_layout::Line>,
}

pub(crate) struct VisualLineCache {
    pub(crate) revision: u64,
    pub(crate) available_width: f32,
    pub(crate) lines: Rc<Vec<VisualLine>>,
}

/// Stateful lexical highlighting depends on every preceding physical line,
/// but scrolling does not change that state. Keep one immutable snapshot per
/// document revision so a scroll frame never rescans the complete buffer.
pub(crate) struct LexicalStateCache {
    pub(crate) revision: u64,
    pub(crate) language: Language,
    pub(crate) states: Rc<Vec<dragent_editor::HighlightState>>,
}

/// Compact source-line index used by large-file mode. It stores only the byte
/// and character start of each physical line; text and layout objects are
/// created for visible rows only.
#[derive(Clone)]
pub(crate) struct LargeSourceLineIndex {
    pub(crate) revision: u64,
    pub(crate) language: Language,
    pub(crate) starts: Rc<Vec<(usize, usize)>>,
    pub(crate) comment_states: Rc<Vec<dragent_editor::HighlightState>>,
    pub(crate) total_chars: usize,
    pub(crate) longest_line_chars: usize,
}

impl LargeSourceLineIndex {
    pub(crate) fn build(text: &str, revision: u64, language: Language) -> Self {
        let mut starts = vec![(0, 0)];
        let mut char_index = 0usize;
        let mut line_start_char = 0usize;
        let mut longest_line_chars = 0usize;
        for (byte_index, character) in text.char_indices() {
            if character == '\n' {
                longest_line_chars = longest_line_chars.max(char_index - line_start_char);
                char_index += 1;
                line_start_char = char_index;
                starts.push((byte_index + 1, char_index));
            } else {
                char_index += 1;
            }
        }
        longest_line_chars = longest_line_chars.max(char_index - line_start_char);
        Self {
            revision,
            language,
            starts: Rc::new(starts),
            comment_states: Rc::new(dragent_editor::LexicalHighlighter::line_start_states(
                language, text,
            )),
            total_chars: char_index,
            longest_line_chars,
        }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub(crate) fn line_for_char(&self, cursor: usize) -> usize {
        self.starts
            .partition_point(|(_, char_start)| *char_start <= cursor)
            .saturating_sub(1)
            .min(self.line_count().saturating_sub(1))
    }

    pub(crate) fn line_range(&self, text: &str, line: usize) -> Option<(usize, usize, usize)> {
        let (byte_start, char_start) = *self.starts.get(line)?;
        let (byte_end, char_end) = self.starts.get(line + 1).copied().map_or(
            (text.len(), self.total_chars),
            |(next_byte_start, next_char_start)| {
                (
                    next_byte_start.saturating_sub(1).max(byte_start),
                    next_char_start.saturating_sub(1).max(char_start),
                )
            },
        );
        Some((byte_start, byte_end, char_end.saturating_sub(char_start)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceDisplayRow {
    pub(crate) source_line: usize,
    pub(crate) display_row: usize,
    /// Visual column of the first source glyph. `None` means the row contains
    /// only whitespace, so every structurally active guide column is free.
    pub(crate) first_content_column: Option<usize>,
}

/// Maps source rows to the rows actually painted after folding. Any editor
/// decoration that is expressed in source lines must project through this map
/// before it can be painted in viewport coordinates.
pub(crate) struct SourceDisplayGrid {
    pub(crate) source_rows: Vec<SourceDisplayRow>,
    pub(crate) total_rows: usize,
}

pub(crate) fn first_source_content_column(line: &str, tab_width: usize) -> Option<usize> {
    let tab_width = tab_width.max(1);
    let mut column = 0usize;
    for character in line.chars() {
        match character {
            ' ' => column += 1,
            '\t' => column += tab_width - column % tab_width,
            character if character.is_whitespace() => column += 1,
            _ => return Some(column),
        }
    }
    None
}

impl SourceDisplayGrid {
    pub(crate) fn build(visual_lines: &[VisualLine], visible_indices: Option<&[usize]>) -> Self {
        let total_rows = visible_indices.map_or(visual_lines.len(), |rows| rows.len());
        let mut source_rows = Vec::new();
        for display_row in 0..total_rows {
            let visual_index = visible_indices.map_or(display_row, |rows| rows[display_row]);
            let line = &visual_lines[visual_index];
            if !line.continuation {
                source_rows.push(SourceDisplayRow {
                    source_line: line.source_line,
                    display_row,
                    first_content_column: first_source_content_column(
                        &line.text,
                        super::text::source_tab_size(),
                    ),
                });
            }
        }
        Self {
            source_rows,
            total_rows,
        }
    }

    pub(crate) fn source_span_rows(
        &self,
        start_line: usize,
        end_line: usize,
    ) -> &[SourceDisplayRow] {
        let first = self
            .source_rows
            .partition_point(|row| row.source_line < start_line);
        let after_last = self
            .source_rows
            .partition_point(|row| row.source_line <= end_line);
        &self.source_rows[first..after_last]
    }

    /// Convert one logically continuous syntax guide into visually continuous
    /// row runs. Source glyphs at the guide's exact indentation column occlude
    /// only that row; the same guide can resume at the same x coordinate on the
    /// following row without changing its structural lifetime.
    pub(crate) fn visible_guide_runs(
        &self,
        start_line: usize,
        end_line: usize,
        guide_column: usize,
    ) -> Vec<(usize, usize)> {
        let mut runs = Vec::new();
        let mut run_start = None;
        let mut previous_row = None;

        for row in self.source_span_rows(start_line, end_line) {
            let column_is_free = row
                .first_content_column
                .is_none_or(|content_column| content_column > guide_column);
            let follows_previous =
                previous_row.is_some_and(|previous| row.display_row == previous + 1);

            if column_is_free {
                if run_start.is_none() || !follows_previous {
                    if let (Some(start), Some(end)) = (run_start.take(), previous_row) {
                        runs.push((start, end));
                    }
                    run_start = Some(row.display_row);
                }
            } else if let (Some(start), Some(end)) = (run_start.take(), previous_row) {
                runs.push((start, end));
            }
            previous_row = Some(row.display_row);
        }

        if let (Some(start), Some(end)) = (run_start, previous_row) {
            runs.push((start, end));
        }
        runs
    }
}

pub(crate) struct FoldIndexCache {
    pub(crate) revision: u64,
    pub(crate) index: FoldIndex,
}

pub(crate) struct FoldVisibilityCache {
    pub(crate) revision: u64,
    pub(crate) fold_revision: u64,
    pub(crate) available_width: f32,
    pub(crate) visual_indices: Rc<Vec<usize>>,
}

impl FoldVisibilityCache {
    pub(crate) fn matches(&self, revision: u64, fold_revision: u64, available_width: f32) -> bool {
        self.revision == revision
            && self.fold_revision == fold_revision
            && (self.available_width - available_width).abs() < 0.5
    }
}
#[derive(Clone)]
pub struct InlineRenameState {
    pub editor: ViewHandle<InputEditor>,
    /// First character of the symbol being renamed.
    pub anchor_char: usize,
    /// One past the last character of the symbol being renamed.
    pub end_char: usize,
}

/// Font size of the inline rename input; matches the sidebar-owned rename
/// editor created with `crate::theme::scaled(13.0)`.
pub(crate) const INLINE_RENAME_FONT_SIZE: f32 = 13.0;
/// Minimum usable width for the inline rename input so short symbols still
/// leave room to type a longer replacement name.
pub(crate) const INLINE_RENAME_MIN_WIDTH: f32 = 96.0;

/// Top-left position (in editor-local coordinates) of the rename input plus
/// the pixel width of the symbol being renamed.
///
/// `start_caret_x`/`end_caret_x` are caret positions measured inside the
/// shaped line layout; they are translated out of the scrollable content
/// space into editor-local viewport coordinates here.
pub(crate) fn inline_rename_anchor(
    display_row: usize,
    scroll_y: f32,
    horizontal_scroll: f32,
    line_height: f32,
    gutter_width: f32,
    start_caret_x: f32,
    end_caret_x: f32,
) -> (Vector2F, f32) {
    let content_x = gutter_width + EDITOR_CONTENT_PADDING_LEFT;
    let x_start = content_x + start_caret_x - horizontal_scroll;
    let symbol_width = (end_caret_x - start_caret_x).max(0.0);
    // The widget hangs directly below the symbol's row with a one-pixel gap.
    let y = display_row as f32 * line_height - scroll_y + line_height + 1.0;
    (vec2f(x_start, y), symbol_width)
}

/// Width of the inline rename input: it hugs the name currently typed, but
/// starts out just a little wider than the original symbol (VS Code behaviour)
/// and never shrinks below a comfortable minimum.
pub(crate) fn inline_rename_input_width(symbol_width: f32, text_width: f32) -> f32 {
    const HORIZONTAL_INSET: f32 = 14.0;
    (text_width + HORIZONTAL_INSET)
        .max(symbol_width * 1.35)
        .max(INLINE_RENAME_MIN_WIDTH)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCaretIdentity {
    pub display_name: String,
    pub avatar_url: String,
}

/// Tree-sitter highlight snapshot of the buffer at `revision`, built off the
/// UI thread (see `InputEditor::schedule_syntax_refresh`).
///
/// Rendering never parses. Between an edit and the next snapshot, rows whose
/// text is unchanged still match `by_text` and keep their colors; only rows
/// that changed fall back to the lexical highlighter for that brief moment.
pub(crate) struct SyntaxHighlightCache {
    pub(crate) revision: u64,
    pub(crate) grammar: &'static str,
    /// Per source line: its text and spans (byte ranges within that line).
    pub(crate) lines: Vec<(String, Arc<Vec<dragent_editor::HighlightSpan>>)>,
    pub(crate) line_start_chars: Vec<usize>,
    pub(crate) by_text: HashMap<String, Arc<Vec<dragent_editor::HighlightSpan>>>,
}

impl SyntaxHighlightCache {
    pub(crate) fn build(grammar: &'static str, text: &str, revision: u64) -> Option<Self> {
        let doc = dragent_editor::highlight_document(grammar, text)?;
        let mut lines = Vec::with_capacity(doc.lines.len());
        let mut line_start_chars = Vec::with_capacity(doc.lines.len());
        let mut by_text = HashMap::new();
        let mut chars = 0usize;
        for (index, line_text) in text.split('\n').enumerate() {
            let spans = Arc::new(doc.lines.get(index).cloned().unwrap_or_default());
            line_start_chars.push(chars);
            chars += line_text.chars().count() + 1;
            by_text
                .entry(line_text.to_string())
                .or_insert_with(|| spans.clone());
            lines.push((line_text.to_string(), spans));
        }
        Some(Self {
            revision,
            grammar,
            lines,
            line_start_chars,
            by_text,
        })
    }

    /// Spans for one visual row: `source_line` is 0-based, `abs_char_offset`
    /// is the row's absolute char offset in the buffer, `text` its contents
    /// (a whole line, or a wrapped slice of one). `None` means "use the
    /// lexical highlighter" — e.g. a folded placeholder, or a row edited
    /// since this snapshot.
    pub(crate) fn spans_for_row(
        &self,
        current_revision: u64,
        source_line: usize,
        abs_char_offset: usize,
        text: &str,
    ) -> Option<Vec<dragent_editor::HighlightSpan>> {
        if self.revision == current_revision {
            let (line_text, spans) = self.lines.get(source_line)?;
            let char_start =
                abs_char_offset.checked_sub(*self.line_start_chars.get(source_line)?)?;
            let byte_start = line_text
                .char_indices()
                .nth(char_start)
                .map_or(line_text.len(), |(byte, _)| byte);
            let byte_end = byte_start + text.len();
            if line_text.get(byte_start..byte_end) != Some(text) {
                return None;
            }
            return Some(slice_spans(spans, byte_start, byte_end));
        }
        // Stale snapshot: only whole, unchanged lines are trustworthy.
        self.by_text.get(text).map(|spans| spans.as_ref().clone())
    }
}

/// Spans intersecting `[start, end)` of a line, re-based to that slice.
pub(crate) fn slice_spans(
    spans: &[dragent_editor::HighlightSpan],
    start: usize,
    end: usize,
) -> Vec<dragent_editor::HighlightSpan> {
    spans
        .iter()
        .filter(|span| span.range.start < end && start < span.range.end)
        .map(|span| dragent_editor::HighlightSpan {
            range: span.range.start.max(start) - start..span.range.end.min(end) - start,
            kind: span.kind,
        })
        .collect()
}

#[cfg(test)]
mod syntax_cache_tests {
    use super::*;
    use dragent_editor::TokenKind;

    const SRC: &str = "fn main() {\n    let s = \"hi\";\n}\n";

    #[test]
    fn current_snapshot_serves_whole_and_wrapped_rows() {
        let cache = SyntaxHighlightCache::build("rust", SRC, 7).unwrap();
        let line = "    let s = \"hi\";";
        let abs = "fn main() {\n".chars().count();
        let whole = cache.spans_for_row(7, 1, abs, line).unwrap();
        assert!(whole
            .iter()
            .any(|s| s.kind == TokenKind::Keyword && &line[s.range.clone()] == "let"));
        // A wrapped tail slice gets spans re-based to the slice.
        let tail = "\"hi\";";
        let tail_abs = abs + line.find('"').unwrap();
        let spans = cache.spans_for_row(7, 1, tail_abs, tail).unwrap();
        assert!(spans
            .iter()
            .any(|s| s.kind == TokenKind::String && &tail[s.range.clone()] == "\"hi\""));
    }

    #[test]
    fn stale_snapshot_only_serves_unchanged_lines() {
        let cache = SyntaxHighlightCache::build("rust", SRC, 7).unwrap();
        assert!(cache.spans_for_row(8, 0, 0, "fn main() {").is_some());
        assert!(cache
            .spans_for_row(8, 0, 0, "fn main() { // edited")
            .is_none());
    }

    #[test]
    fn mismatched_row_text_falls_back() {
        let cache = SyntaxHighlightCache::build("rust", SRC, 7).unwrap();
        assert!(cache.spans_for_row(7, 0, 0, "fn main() {…}").is_none());
    }
}
