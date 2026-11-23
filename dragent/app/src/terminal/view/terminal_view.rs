//! Minimal interactive terminal pane (Sprint 1–2).

#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use crate::process_ext::NoWindow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use web_time::Instant;

use parking_lot::{FairMutex, RwLock};
use uuid::Uuid;

use crate::icons::ICON_SIZE_SM;
use async_channel::Sender;
use dragent_pty::{resolve_shell, PtyEvent, SessionPlatform};
use dragent_resource_policy::{ActivityLevel, MemoryPressure, ResourceBudget};
use dragent_shell_hooks::ShellKind;
use dragent_terminal::{MouseButton, MouseEventKind, TermMode};
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

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use crate::agent::input_classifier::{
    heuristic::HeuristicInputClassifier, ClassifyContext, InputClassifier,
};
use crate::agent::{DetectedInputType, InputConfig, TabSessionKind};
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

use crate::terminal::viewport::{
    is_block_list_virtualization_enabled, ScrollMode, ScrollPosition, ViewportState,
};
use crate::terminal::TerminalEventListener;
use crate::theme::{Theme, INPUT_FONT_SIZE, TERMINAL_PADDING_LEFT};
use crate::workspace::persistence::PersistedBlock;
use crate::workspace::{InputEditor, InputEditorEvent, InputMode, DEFAULT_SESSION_TITLE};

/// Horizontal padding subtracted before computing columns (left + right).
pub(crate) const RESIZE_PADDING_X: f32 = TERMINAL_PADDING_LEFT;
/// Vertical padding subtracted before computing rows.
pub(crate) const RESIZE_PADDING_Y: f32 = 6.0;
/// Tolerance when comparing block-list bottom sentinel to the scroll viewport.
pub(crate) const SCROLL_BOTTOM_EPSILON_PX: f32 = 2.0;
/// Approximate rendered height of the block options (⋯) dropdown, used to keep it
/// from overflowing the viewport bottom (6 items + 2 dividers + padding).
pub(crate) const BLOCK_MENU_EST_HEIGHT: f32 = 200.0;

use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchDiffOp {
    None,
    Generating,
    Committing,
    Pushing,
    Pulling,
}

impl BranchDiffOp {
    pub(crate) fn is_busy(self) -> bool {
        self != BranchDiffOp::None
    }
}

// ── `!` scratch shell ──────────────────────────────────────────────────────────
//
// A persistent shell spawned per pane the first time the user runs `!cmd` while
// an AI CLI tool owns the terminal. It runs on a real PTY (via dragent_pty), so
// it behaves like a normal terminal: STATE persists across commands (cd/env),
// output STREAMS live (a PTY isn't block-buffered like a pipe), and a running
// process can be interrupted with Ctrl+C (`handle.interrupt()` → SIGINT to the
// foreground process group). Raw PTY bytes are fed through a real terminal
// emulator (the same `dragent_terminal::Emulator` the main terminal uses), so
// prompts, `\r` redraws, the zsh line editor, RPROMPT, and `ls` column layout
// all render correctly — a naive ANSI-strip-and-append renderer mangles them
// (carriage returns get deleted, so each prompt/ZLE redraw concatenates onto the
// previous text). The transcript survives closing the card (× only hides it).

/// Breathing room kept below the last block, in px.
///
/// The scroll range used to end at the exact content height, so the final pixel
/// of content sat flush against the input bar — and anything the height index
/// under-measured (a self-heal card's buttons, the last line of an agent reply)
/// was clipped behind it with no way to scroll further, because "scrolled to the
/// bottom" had already been reached. Extending the range past the content lets
/// the tail clear the input and leaves a visible margin there.
// Extra breathing room below the last block. This is especially important in
// the docked editor terminal, where the viewport can be resized down to a
// compact height and the final output must remain scrollable above the input
// chrome rather than ending flush against it.

pub(crate) const BLOCK_LIST_BOTTOM_GAP_PX: f32 = 96.0;

/// History popover row geometry. Chrome = grip 20 + header 28 (12.5px bold
/// label + 2/7 padding + border) + footer 29 + list padding 8; a row = ~18px of
/// 12.5px mono plus 6+6 padding. Both rounded UP so rounding error costs a row
/// rather than overrunning. Shared by the row count and the background-image
/// height so the two can never disagree.
pub(crate) const CMD_HIST_CHROME_PX: f32 = 88.0;
pub(crate) const CMD_HIST_ROW_PX: f32 = 30.0;

/// Smallest the history popover's requested size may be. This is the user's
/// drag intent, not a hard box — the band itself is sized by its content — so it
/// just needs to still imply the 3-row minimum (84 chrome + 3×30 = 174).
pub(crate) const CMD_HIST_MIN_HEIGHT_PX: f32 = 180.0;

/// Which way an in-progress alt-screen drag has committed to. Resolved once,
/// after the finger/pointer has moved past [`ALT_PAN_AXIS_THRESHOLD_PX`], and
/// then held for the rest of the gesture so it can't flip mid-drag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AltPanAxis {
    /// Panning the grid sideways — motion is withheld from the TUI.
    Horizontal,
    /// Left to the TUI (its own vertical scrolling) — the grid does not pan.
    Vertical,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AltLeftGesture {
    /// Forwarded to the app, which runs its own selection. Holds the last cell
    /// reported, since a terminal reports drags per cell, not per pixel.
    App { last_cell: (usize, usize) },
    /// Selecting locally: the app didn't ask for the mouse, or Option is held.
    Local,
    /// Cmd/Ctrl+click that opened a link. The rest of the click goes nowhere.
    Link,
}

/// How far a drag must travel before its axis is decided. Below this the
/// gesture is still ambiguous, so nothing pans and nothing is swallowed.
pub(crate) const ALT_PAN_AXIS_THRESHOLD_PX: f32 = 6.0;

/// An in-progress drag across the alt-screen of a shared-session mirror.
pub(crate) struct AltPanDrag {
    /// Where the gesture began, for measuring total travel.
    pub(crate) start: Vector2F,
    /// Previous x, for the incremental pan delta.
    pub(crate) last_x: f32,
    /// `None` until the gesture has moved far enough to commit to an axis.
    pub(crate) axis: Option<AltPanAxis>,
}

pub struct TerminalView {
    pub(crate) launch_shell: Option<String>,
    pub(crate) theme: Theme,
    pub(crate) mono_family: FamilyId,
    pub(crate) cell_metrics: CellMetrics,
    pub(crate) model: ModelHandle<TerminalModel>,
    pub(crate) manager: TerminalManager,
    /// The session core, kept so a shared-session mirror (remote mode) can feed
    /// it broadcast output directly via [`feed_remote_output`](Self::feed_remote_output).
    pub(crate) session_core: Arc<FairMutex<crate::terminal::model::TerminalSessionCore>>,
    /// True when this is a shared-session **mirror** (no local PTY; input, if
    /// any, forwards to the host) rather than a local terminal.
    pub(crate) is_remote: bool,
    /// A remote **viewer** without control: the input is disabled entirely (no
    /// typing, no PTY forwarding) and shows a padlock + "request edit" prompt.
    /// Flipped false the moment the host grants control (see
    /// [`set_remote_read_only`](Self::set_remote_read_only)). Always false for a
    /// local terminal.
    pub(crate) remote_read_only: bool,
    /// The pop-up terminal over a page (sign-ins, installers): web links it
    /// shows open in the system browser, never in Dragent's browser pane.
    pub(crate) links_open_in_system_browser: bool,
    /// A shell statement written once, ahead of the next submitted command
    /// (the pop-up terminal's system `BROWSER`); the block shows the command.
    pub(crate) pending_shell_setup: Option<String>,
    /// Mirror only: this viewer authenticated as the HOST's own account (e.g. the
    /// user's second device). Such a controller may drive the host's terminal
    /// SIZE (self-sizes + resizes the host to match — WYSIWYG). A teammate
    /// (different account) never resizes the host; it adopts the host's grid and
    /// letterboxes it to its own window. Always false for a local terminal.
    pub(crate) remote_is_host_account: bool,
    /// Mirror only: the viewer explicitly closed the AI-tool rich input, so the
    /// host's surface updates must stop reopening it. `apply_remote_surface`
    /// re-derives the panel from "does the host have a tool active", and the host
    /// broadcasts that on every output chunk — so without this a deliberate close
    /// was undone within milliseconds. Cleared when the viewer reopens the panel,
    /// or when the host's tool exits (the next tool session starts fresh).
    pub(crate) remote_rich_input_closed: bool,
    /// Mirror only: horizontal pan for the alt-screen when the host's grid is
    /// WIDER than this window (a phone / narrow browser). The grid is scaled to
    /// fit the host's rows but never below a readable floor, so excess width is
    /// panned rather than shrunk into illegibility. Driven by dragging the
    /// scrollbar only — the wheel is deliberately left to the TUI inside
    /// (`horizontal_no_wheel`), so vertical scrolling still reaches Claude Code /
    /// vim instead of panning the grid.
    pub(crate) remote_alt_h_scroll: ClippedScrollStateHandle,
    /// Mirror only: an in-progress drag across the alt-screen. On a phone there
    /// is no wheel and no scrollbar to grab, so dragging the CONTENT is the only
    /// way to reach the part of the host's grid that doesn't fit. The drag
    /// commits to one axis: a sideways drag pans the grid (and is withheld from
    /// the TUI so it can't also scroll itself), while an up/down drag is left
    /// entirely to the TUI — on a touch screen that IS how you scroll Claude
    /// Code. `None` when no drag is in progress. Shared (not `&mut self`)
    /// because the element callbacks that drive it only get `&AppContext`.
    pub(crate) alt_pan_drag: Arc<Mutex<Option<AltPanDrag>>>,
    /// The AI tool the **host** has active, from its broadcast surface state. On
    /// a mirror this — not the local `current_running_command` (which a viewer
    /// never sets) — is what drives the AI-tool input panel, so the viewer shows
    /// the same panel the host shows (Claude Code, Cursor Agent, Ollama, …).
    /// Always `None` for a local terminal.
    pub(crate) remote_ai_tool: Option<AiTool>,
    /// Mirror (controller) only: when this participant last typed in its own
    /// input. A controlling viewer reflects the host's live command/AI text
    /// (host → web), but ONLY while it isn't actively typing — otherwise the
    /// host's echo of the viewer's OWN forwarded keystrokes would clobber the
    /// in-flight local edit and jump the cursor. Local edits win inside a short
    /// window; once idle, host edits reflect. Bumped on every local input change.
    pub(crate) last_local_input_edit: Instant,
    /// The host terminal's grid dimensions (rows, cols), from its broadcast
    /// surface state. A mirror sizes its emulator to these so the raw byte stream
    /// — laid out for the host's grid — renders exactly like the host's screen.
    /// When the host grid is wider than the viewer's window, the alt-screen grid
    /// scrolls horizontally (see `render_alt_screen_full`) rather than clipping.
    /// `None` for a local terminal or before the first surface update.
    pub(crate) remote_dims: Option<(u16, u16)>,
    /// Host-side: a controlling viewer's requested grid size. While set, the
    /// shared PTY follows the VIEWER's window (so a TUI re-renders to fill it)
    /// instead of the host's own window. Cleared when the session ends. `None`
    /// on a mirror or an unshared/uncontrolled pane.
    pub(crate) remote_requested_dims: Option<(u16, u16)>,
    pub(crate) resize_tx: Sender<Vector2F>,
    pub(crate) heights_tx: Sender<()>,
    pub(crate) pane_has_focus: bool,
    pub(crate) shift_held: bool,
    pub(crate) ctrl_held: bool,
    pub(crate) alt_held: bool,
    pub(crate) cmd_held: bool,
    pub(crate) drag_selecting: bool,
    /// Who the alt-screen left-button gesture in progress belongs to, fixed at
    /// press so a modifier or mode change mid-drag can't split it in two.
    pub(crate) alt_left_gesture: Option<AltLeftGesture>,
    /// A local selection's press, until it moves far enough to count as a drag:
    /// (anchor row, anchor col, press position).
    pub(crate) alt_press: Option<(usize, usize, Vector2F)>,
    pub(crate) last_mouse_pos: Vector2F,
    pub(crate) last_dims: Option<(u16, u16)>,
    pub(crate) last_alt_active: bool,
    pub(crate) last_rich_input_active: bool,
    pub(crate) block_scroll_state: ClippedScrollStateHandle,
    pub(crate) agent_transcript_scroll: ClippedScrollStateHandle,
    pub(crate) agent_transcript_follow: std::cell::Cell<bool>,
    pub(crate) agent_transcript_last_scroll: std::cell::Cell<f32>,
    pub(crate) block_scroll_pos: Arc<Mutex<ScrollPosition>>,
    pub(crate) block_scroll_handle: ScrollStateHandle,
    pub(crate) block_height_index: BlockHeightIndex,
    pub(crate) auto_scroll: bool,
    /// Shared with `command_editor`: true while a foreground command runs, so
    /// the collapsed (zero-height) editor propagates every keystroke instead
    /// of silently eating printable keys into an invisible buffer. See
    /// InputEditor::passthrough_keys for the bug this prevents.
    pub(crate) command_input_passthrough: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) command_editor: ViewHandle<InputEditor>,
    pub(crate) rich_input_editor: ViewHandle<InputEditor>,
    pub(crate) feedback_editor: ViewHandle<InputEditor>,
    /// Inline editor for typing a custom answer to an agent `ask_user` question.
    pub(crate) question_editor: ViewHandle<InputEditor>,
    /// Embedded single-line editor for the active block output filter query.
    pub(crate) block_filter_editor: ViewHandle<InputEditor>,
    pub(crate) command_history: Vec<String>,
    pub(crate) command_history_idx: Option<usize>,
    /// Cursor into `command_history` for the AI rich input's Shift+Up/Down recall
    /// (separate from the command editor's `command_history_idx`).
    pub(crate) rich_input_history_idx: Option<usize>,
    pub(crate) history_recall_prefix: Option<String>,
    pub(crate) previous_command: Option<String>,
    pub(crate) shell_aliases: ShellAliases,
    pub(crate) suggest_engine: SuggestEngine,
    pub(crate) git_repo_status: ModelHandle<crate::git_status::GitRepoStatusModel>,
    pub(crate) current_running_command: Option<String>,
    pub(crate) rich_input_active: bool,
    /// `0` = rich input composes locally as usual. `>0` = live-proxy mode —
    /// triggered by typing `/` as the first character, every keystroke is
    /// forwarded live to the wrapped CLI's own PTY input instead of the rich
    /// editor's buffer, so the CLI's own interactive UI (e.g. its native
    /// slash-command popup) works through rich input. Counts characters sent
    /// since the trigger, purely to know when Backspace has erased back to
    /// nothing (see `TerminalAction::ForwardRichInputProxyKey`).
    pub(crate) rich_input_proxy_chars: usize,
    pub(crate) pane_view_id: EntityId,
    /// The window this pane lives in. Captured at construction so per-window
    /// operations (focus lookup, window bounds) target THIS window rather than
    /// `window_ids().next()` (which is always the first window → broke input in
    /// second+ windows).
    pub(crate) window_id: WindowId,
    pub(crate) output_viewport_width_px: f32,
    pub(crate) output_viewport_height_px: f32,
    pub(crate) entry_hint: Option<EntryHint>,
    pub(crate) link_hint: Option<LinkHint>,
    pub(crate) entry_mouse_states:
        RefCell<HashMap<String, warpui_core::elements::MouseStateHandle>>,
    pub(crate) shimmer_states:
        RefCell<HashMap<String, warpui_core::elements::shimmering_text::ShimmeringTextStateHandle>>,
    pub(crate) diagram_states: RefCell<HashMap<String, DiagramCanvasState>>,
    /// Files the user attached for the NEXT agent query.
    /// Drained on submit; images go multimodal, text files inline.
    pub(crate) pending_attachments: Vec<crate::agent::attachments::LoadedAttachment>,
    /// Persistent scratch shell driven by the `!` prefix while an AI CLI tool
    /// owns the terminal. State (cd/env) persists across commands; output history
    /// survives closing the card. Spawned lazily on the first `!cmd`.
    pub(crate) side_shell: Option<SideShell>,
    /// Whether the scratch-shell card is currently shown. `×` sets this false
    /// (the shell keeps running); the next `!` reopens it with history intact.
    pub(crate) side_card_open: bool,
    /// True while a repaint loop is scheduled for the live scratch-shell output
    /// — prevents stacking duplicate timers on reopen.
    pub(crate) side_poll_armed: bool,
    /// Commands run in this session's scratch shell, newest LAST. Browsed via the
    /// history popover when the AI input starts with `!` (the global command
    /// history popover doesn't open in AI-tool mode). Session-only, not persisted.
    pub(crate) side_cmd_history: Vec<String>,
    /// Drag-resizable height (px) of the scratch-shell card's output area.
    pub(crate) side_card_height_px: f32,
    /// Drag-to-resize state for the scratch-shell card (same DragResizeElement
    /// primitive as the history popover).
    pub(crate) side_card_drag: warpui_core::elements::DragResizeHandle,
    /// Scroll state for the scratch-shell output — lets the user scroll up
    /// through the emulator's scrollback.
    pub(crate) side_scroll: ClippedScrollStateHandle,
    /// Text-selection state for copying from the scratch-shell output.
    pub(crate) side_selectable: SelectionHandle,
    /// Whether the scratch-shell output auto-scrolls to follow the newest line.
    /// A manual scroll-up turns it off; the Jump-to-bottom button turns it back on.
    pub(crate) side_follow_tail: bool,
    /// Last-seen value of the scratch shell's output-activity counter, and when it
    /// last changed. Drives showing the Stop button only while a process is
    /// actively producing output (a running job) rather than at an idle prompt.
    pub(crate) side_activity_seen: u64,
    pub(crate) side_activity_at: web_time::Instant,
    /// Mic button lifecycle: Idle → Connecting (permissions, token, socket)
    /// → Recording (capture live). Split so the UI never claims "recording"
    /// before audio is actually flowing.
    pub(crate) voice_ui: crate::voice::VoiceUiState,
    /// Session generation. Bumped on every start AND every clear; events from
    /// an abandoned session (stuck thread, force-cleared stop) carry a stale
    /// generation and are dropped — without this, a ghost session's Partials
    /// kept injecting text and its Live re-lit the mic after the user stopped.
    pub(crate) voice_gen: u64,
    /// Current spinner frame while `voice_ui` is `Connecting`.
    pub(crate) voice_spin: usize,
    /// Active voice capture session; Some while recording.
    pub(crate) voice_session: Option<crate::voice::VoiceSession>,
    /// Append-only dictation folding (pure logic, unit-tested in voice::fold).
    pub(crate) voice_fold: crate::voice::fold::VoiceFold,
    pub(crate) voice_pending_confirm: Option<String>,
    /// Set by submit while recording: the next snapshot re-bases onto the
    /// cleared input so post-send speech starts a fresh conversation.
    pub(crate) voice_rebase_pending: bool,
    /// One-shot notice under the input (attachment errors, voice errors).
    pub(crate) input_notice: Option<String>,
    /// `input_notice` currently holds the host-connection notice, so the
    /// host binding may replace or clear it without wiping other notices.
    pub(crate) host_notice_active: bool,
    /// The host notice is the coloured "Starting <shell>…" line.
    pub(crate) shell_starting_notice: bool,
    pub(crate) host_error_notice: Option<String>,
    /// Recovery buttons while this pane's host session is unavailable
    /// (Retry Resume, Start New Session); empty whenever it is connected.
    pub(crate) host_recovery_actions: Vec<HostRecoveryAction>,
    /// Command that was running when the session was last saved, used to
    /// restart it if its process did not survive.
    pub(crate) host_restart_command: Option<String>,
    /// Full-size image viewer: Some(ordinal among pending IMAGE attachments).
    pub(crate) image_viewer: Option<usize>,
    /// Full-size Mermaid diagram viewer: Some((svg path, intrinsic width, intrinsic height)).
    pub(crate) diagram_viewer: Option<(String, f32, f32)>,
    pub(crate) diagram_viewer_zoom: f32,
    pub(crate) diagram_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) diagram_drag_last: Arc<Mutex<Option<Vector2F>>>,
    pub(crate) block_selectable_handle: SelectionHandle,
    pub(crate) block_selected_text: Arc<RwLock<Option<String>>>,
    pub(crate) path_picker: ModelHandle<PathPicker>,
    pub(crate) history_search: HistorySearchState,
    /// Warp-style command-history popover: ↑/↓ at an empty prompt opens a
    /// resizable list of recent commands (persisted, with relative timestamps).
    pub(crate) cmd_hist_open: bool,
    /// (command, created_at_unix_ms), newest-first — fetched on open.
    pub(crate) cmd_hist_items: Vec<(String, i64)>,
    /// Selected row (index into `cmd_hist_items`).
    pub(crate) cmd_hist_selected: usize,
    /// Input buffer to restore when the popover is dismissed with Esc.
    pub(crate) cmd_hist_saved: String,
    /// Previous input routing while the main history popover temporarily
    /// treats its selected entry as a shell command.
    pub(crate) cmd_hist_previous_input_config: Option<InputConfig>,
    /// True when the popover targets the AI-tool `!` scratch shell (session
    /// history, previews as `!cmd` into the rich input) rather than the main
    /// command editor (global persisted history).
    pub(crate) cmd_hist_rich: bool,
    /// Drag-to-resize height of the popover, in px.
    pub(crate) cmd_hist_height_px: f32,
    /// Drag-to-resize state for the popover. `DragResizeElement` captures the
    /// drag/up via raw events, so motion is position-independent (smooth) and it
    /// works in deltas — no absolute-Y feedback loop that made Resizable shake.
    pub(crate) cmd_hist_drag: warpui_core::elements::DragResizeHandle,
    pub(crate) editor_settings: EditorSettings,
    pub(crate) workflow_store: WorkflowStore,
    pub(crate) workflow_search: WorkflowSearchState,
    pub(crate) workflow_session: Option<WorkflowSession>,
    /// Scroll state for the workflow picker results. The picker is rendered as
    /// a bounded surface above the input, so a large catalog never pushes into
    /// the input/status chrome.
    pub(crate) workflow_scroll_state: ClippedScrollStateHandle,
    pub(crate) completion_scroll_state: ClippedScrollStateHandle,
    pub(crate) dir_sort: DirSort,
    pub(crate) dir_view: DirView,
    pub(crate) pane_uuid: Uuid,
    pub(crate) event_listener: TerminalEventListener,
    /// Block whose header is currently pinned to the top of the viewport (sticky header).
    pub(crate) sticky_header: Option<StickyBlockHeader>,
    /// Docked editor terminals use the full viewport for output and do not show
    /// the normal terminal's floating sticky command header.
    pub(crate) sticky_headers_enabled: bool,
    /// Docked terminals paint their backdrop inside their own ChildView layer.
    /// A WorkspaceView background is composited below sibling editor ChildViews
    /// and therefore cannot reliably hide the editor behind the dock.
    pub(crate) docked_backdrop_enabled: bool,
    /// Block whose options dropdown (⋯) is currently open, if any.
    pub(crate) open_block_menu: Option<usize>,
    /// Viewport-relative point to anchor the open block menu to, when it was
    /// opened via right-click. `None` falls back to header-anchored positioning
    /// (used when opened via the "⋯" button).
    pub(crate) block_menu_anchor: Option<Vector2F>,
    /// The copy button currently showing its checkmark.
    pub(crate) copy_confirmation: super::copy_confirmation::CopyConfirmation,
    /// The detected link (URL or file path) whose action menu — "Open in
    /// Editor" / "Reveal in Finder", or "Open in Browser" / "Copy Link" for a
    /// URL — is currently open, if any. Set by a plain click on a link;
    /// Cmd/Ctrl+click bypasses this and opens directly.
    pub(crate) open_link_menu: Option<crate::terminal::view::actions::LinkHint>,
    /// Viewport-relative point to anchor the open link menu to (the click
    /// position), mirroring `block_menu_anchor`.
    pub(crate) link_menu_anchor: Option<Vector2F>,
    /// The link/path currently under the pointer in normal block output, if
    /// any. Lives here (not on the transient paint element) because the
    /// paint element is rebuilt on every `ctx.notify()` — including the one
    /// hover-change itself triggers — so element-local hover state gets
    /// wiped the instant it's set. `BlockListElement` is seeded from this
    /// each render, the same way it already is for `link_hint` (selected).
    pub(crate) hovered_link: Option<crate::terminal::view::actions::LinkHint>,
    /// Same idea as `hovered_link`, for the alt-screen grid.
    pub(crate) alt_hovered_link: Option<Vec<(usize, usize, usize)>>,
    /// `ls` card hover — see `DirHover`.
    pub(crate) dir_hover: crate::terminal::view::actions::DirHover,
    /// UserQuery block whose checkpoint restore preview popover is open, if any.
    pub(crate) open_checkpoint_preview: Option<usize>,
    /// The single block whose output filter bar is currently shown, plus its
    /// filter state. Only one filter bar is active at a time (it takes keyboard
    /// focus for the query).
    pub(crate) block_filter: Option<(usize, crate::console::block_filter::BlockFilterState)>,
    /// Zero-based index of the currently selected matching output line.
    pub(crate) block_filter_match_index: usize,
    /// Y offset (0..=BLOCK_HEADER_HEIGHT) to push the sticky header up as the next block approaches.
    pub(crate) sticky_push_offset: f32,
    /// Measured pixel heights fed back from the after_layout pass.
    /// Overrides the estimate in rebuild_height_index when present.
    /// Shared (via Arc) with BlockListElement so after_layout can write directly.
    pub(crate) measured_block_heights: Arc<Mutex<HashMap<usize, f32>>>,
    pub(crate) measured_agent_block_heights: Arc<Mutex<HashMap<usize, f32>>>,
    pub(crate) glyph_cache: Arc<Mutex<CellGlyphCache>>,
    pub(crate) block_text_cache: Arc<Mutex<crate::terminal::render::block_list_element::BlockTextCache>>,
    /// Height of the live prompt section last rendered (cell_h * prompt line count).
    pub(crate) live_prompt_height_px: f32,
    /// Per-pane input routing config (Shell vs Agent, locked or autodetected).
    pub(crate) input_config: InputConfig,
    /// The agent execution mode (Agent/Plan/Debug/Multitask/Ask)
    /// chosen in the input bar. Applied to each agent turn at submit time.
    pub(crate) chat_mode: crate::agent::chat_mode::ChatMode,
    /// True while the modes dropdown is open.
    pub(crate) mode_menu_open: bool,
    /// True while the model quick-switch dropdown is open.
    pub(crate) model_menu_open: bool,
    pub(crate) agent_model_override: Option<String>,
    /// Keyboard-selected row in the slash-command palette, when open. "Open"
    /// itself is derived each render from the live buffer text (see
    /// `slash_command_candidates`), not tracked as a separate bool, so it
    /// can't desync from what's actually typed.
    pub(crate) slash_palette_selected: usize,
    /// Scroll state for the bounded slash-command palette.
    pub(crate) slash_scroll_state: ClippedScrollStateHandle,
    /// True while the session history panel is open.
    pub(crate) history_panel_open: bool,
    /// Chat lines (step groups, step details) the person opened, by key.
    pub(crate) chat_open: std::collections::HashSet<String>,
    /// A terminal agent's tab shows its conversation as a chat instead of
    /// its terminal (switched from the navbar).
    pub(crate) cli_chat_on: bool,
    /// What was last read of that conversation.
    pub(crate) cli_chat: Option<super::terminal_cli_chat::CliChat>,
    /// Bumped on every switch, so a read started for an old state is dropped.
    pub(crate) cli_chat_generation: u64,
    /// A message sent from the chat that its conversation doesn't show yet.
    pub(crate) cli_chat_sent: Option<super::terminal_cli_chat::SentPrompt>,
    /// The chat run over ACP, for agents that speak it.
    pub(crate) acp_chat: Option<super::terminal_acp_chat::AcpChat>,
    /// How many of the conversation's latest rows the chat draws.
    pub(crate) chat_window: usize,
    /// A model picked in the chat for the CLI's session, shown until the
    /// session's own replies say which model it runs.
    pub(crate) cli_chat_model_pick: Option<String>,
    /// The effort picked in the chat for the CLI's session (its `/effort`).
    pub(crate) cli_chat_effort_pick: Option<String>,
    /// The level under the pointer while the effort slider is dragged; set
    /// when it's let go.
    pub(crate) effort_drag: Option<String>,
    /// Counts drag moves, so a late "let go" timer knows it's stale.
    pub(crate) effort_drag_moves: u64,
    /// This pane's past conversations, fetched from local SQLite when the
    /// history panel opens (see `TerminalAction::ToggleHistoryPanel`).
    pub(crate) conversation_summaries: Vec<dragent_persistence::ConversationSummary>,
    /// How this tab was created — determines initial input_config defaults.
    pub(crate) tab_session_kind: TabSessionKind,
    /// Stable agent identity for this session (`agent_id` from the agent
    /// registry: `"claude"`, `"dragent"`, `"custom:<slug>"`, …). Set at
    /// launch, persisted per pane, restored on relaunch — so the tab chip,
    /// rail, and input editor resolve the agent definition without
    /// re-detecting it from the terminal process. `None` for plain shells
    /// (identity then falls back to command detection).
    pub agent_id: Option<String>,
    /// Monotonically incremented on each input change; used to cancel stale classifies.
    pub(crate) classify_generation: u64,
    /// True when the most recent Enter went to the agent stub (feeds follow-up detection).
    pub(crate) last_submit_was_agent: bool,
    /// The next agent query comes from an automation (see `submit_automation_query`).
    pub(crate) automation_submit: bool,
    /// Dragent's agent was working at its last event (see
    /// `notify_agent_moments`).
    pub(crate) agent_turn_busy: bool,
    /// That automation lets it post to connected services.
    pub(crate) automation_service_writes: bool,
    /// Agent backend — default is MockAgentApi; replaced in Layer 6+ by GraphqlAgentApi.
    pub(crate) agent_api: Arc<dyn crate::agent::AgentApi>,
    /// Per-pane agent conversation controller (Layer 4).
    pub(crate) agent_controller: ModelHandle<crate::agent::AgentController>,
    /// Per-pane self-heal store — tracks analysis state for failed shell blocks (Layer 5).
    pub(crate) self_heal_store: ModelHandle<crate::agent::self_heal::SelfHealStore>,
    /// Blocks whose analysis was started by proactive error scanning (not a
    /// user click). With "Auto-fix on failure" off, their Ready state renders
    /// as a compact "Fix available" chip until revealed.
    pub(crate) auto_analyzed_blocks: std::collections::HashSet<usize>,
    /// Auto-analyzed blocks whose full fix card the user expanded.
    pub(crate) revealed_fix_blocks: std::collections::HashSet<usize>,
    /// True while the 1 s agent-activity repaint ticker is armed (live
    /// elapsed counters on running commands / the activity strip).
    pub(crate) agent_ticker_armed: bool,
    /// An open confirmation modal (checkpoint revert / destructive action).
    pub(crate) pending_confirm: Option<crate::confirm::ConfirmRequest>,
    /// This pane owns the close-confirmation modal (only one pane shows it).
    pub(crate) showing_close_confirm: bool,
    /// Real-time git watcher: sender feeding scoped refreshes, current
    /// watched cwd, and the stop flag of the running watcher thread.
    pub(crate) git_watch_tx: async_channel::Sender<Vec<PathBuf>>,
    pub(crate) git_watch_cwd: Option<PathBuf>,
    pub(crate) git_watch_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Monotonic token for the delayed agent-root synchronization. Every new
    /// status event invalidates older timers, making the 500 ms delay a real
    /// debounce instead of a queue of independent Git subprocesses.
    pub(crate) agent_roots_sync_generation: u64,
    /// Last cwd/project-root pair applied to the agent and index. Status events
    /// also carry title/link changes, so this prevents unrelated events from
    /// repeating filesystem warming and spawning `git rev-parse` threads.
    pub(crate) agent_roots_sync_key: Option<(PathBuf, Option<PathBuf>)>,
    /// Blink phase for the alt-screen/bootstrap grid cursor (true = shown).
    pub(crate) alt_cursor_blink_on: bool,
    /// True while the self-rearming blink timer is running.
    pub(crate) alt_blink_timer_armed: bool,
    pub attached_blocks: Vec<crate::agent::api::session::AttachedBlock>,
    /// Tracks when rebuild_height_index last ran, to debounce rapid OutputAppended events.
    pub(crate) last_height_rebuild_at: Option<Instant>,
    /// True when a deferred height-rebuild timer is already scheduled for this frame.
    pub(crate) output_rebuild_pending: bool,
    /// Whether this pane is currently visible in the workspace. Background panes
    /// keep their PTY and agent work alive, but suspend animation and use a much
    /// lower output-coalescing frequency until they become visible again.
    pub(crate) workspace_active: bool,
    /// When the agent approved a `run_command` and it's running in the PTY, this
    /// holds (task_id, call_id) so BlockCompleted can POST the output back.
    pub(crate) pending_agent_run_command: Option<(String, String)>,
    /// Whether the git diff bar above the input is expanded to show per-file details.
    /// User clicked the Agent Changes header to close it. Cleared automatically
    /// when the agent touches new files (see the agent_controller subscription).
    pub(crate) git_diff_dismissed: bool,
    /// Last seen count of agent-modified files, to detect growth (new edits
    /// re-surface a dismissed Agent Changes card).
    pub(crate) last_agent_file_count: usize,
    /// Recursive agent block count last observed. Structural changes are sent
    /// immediately to shared participants; token-only updates remain throttled.
    pub(crate) last_shared_agent_block_count: usize,
    /// When this pane last projected its conversation for a phone.
    pub(crate) phone_agent_published_at: Option<web_time::Instant>,
    /// A trailing projection is already scheduled.
    pub(crate) phone_agent_timer: bool,
    /// Whether the full branch diff panel (opened from the +N/-N chip) is expanded.
    pub(crate) branch_diff_expanded: bool,
    /// Files the user has selected for the next commit (by rel_path from repo root).
    pub(crate) branch_diff_staged: std::collections::HashSet<String>,
    /// Scroll state for the file list inside the branch diff panel.
    pub(crate) branch_diff_scroll: ClippedScrollStateHandle,
    /// Scroll state for the checkpoint restore preview popover's file list.
    pub(crate) checkpoint_preview_scroll: ClippedScrollStateHandle,
    /// Single-line editor for typing the git commit message.
    pub(crate) branch_diff_commit_editor: ViewHandle<crate::workspace::InputEditor>,
    /// True while a commit or push operation is in flight.
    pub(crate) branch_diff_op: BranchDiffOp,
    /// The pane's error toast: `(title, body)`. Started life as commit-only,
    /// now carries any real failure — errors belong in a dismissable card, not
    /// wedged into the input bar's notice line where long text is unreadable
    /// and nothing can be copied.
    pub(crate) error_toast: Option<(String, String)>,
    /// Scroll state for the commit error toast body.
    pub(crate) branch_diff_error_scroll: ClippedScrollStateHandle,
    /// True briefly after the user clicks the copy button in the commit error toast.
    pub(crate) commit_error_copied: bool,

    // ── Branch & folder chip popovers ────────────────────────────────────
    /// Branch popover open on the branch chip.
    pub(crate) branch_menu_open: bool,
    /// Local branches, newest-checked-out first. `None` until the listing
    /// returns, which is what the popover renders as "Loading…".
    pub(crate) branch_list: Option<Vec<String>>,
    /// Single-line editor for the new-branch name inside the branch popover.
    pub(crate) new_branch_editor: ViewHandle<crate::workspace::InputEditor>,
    /// Folder popover open on the cwd chip.
    pub(crate) folder_menu_open: bool,
    /// Subdirectory names of the current cwd (not full paths). `None` while loading.
    pub(crate) folder_list: Option<Vec<String>>,
    /// Scroll state for the branch popover's list.
    pub(crate) branch_scroll: ClippedScrollStateHandle,
    /// Scroll state for the folder popover's list.
    pub(crate) folder_scroll: ClippedScrollStateHandle,
    /// Search box inside the model picker.
    pub(crate) model_search_editor: ViewHandle<InputEditor>,
    /// Live query from that box; empty shows every model.
    pub(crate) model_search: String,
    /// Scroll state for the model picker's list.
    pub(crate) model_scroll: ClippedScrollStateHandle,
}

/// A button offered while a pane's host session is unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRecoveryAction {
    pub label: String,
    /// Start a new conversation instead of resuming the recorded one.
    pub fresh: bool,
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        // `notify` owns an OS thread for every recursive watcher. A terminal
        // tab may outlive its UI activity, but once the view itself is dropped
        // the watcher must not remain detached for the rest of the process.
        self.stop_git_file_watcher();
        crate::realtime::phone_agent::unregister(self.pane_uuid);
    }
}

impl TerminalView {
    /// Recovery actions for an unavailable host session, placed above the
    /// input so they are reachable whichever input surface is showing.
    fn render_host_restart_row(&self, theme: &crate::theme::Theme) -> Option<Box<dyn Element>> {
        if self.host_recovery_actions.is_empty() {
            return None;
        }
        let pane_uuid = self.pane_uuid;
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for (i, action) in self.host_recovery_actions.iter().enumerate() {
            let fresh = action.fresh;
            let button = Container::new(Box::new(
                Text::new(action.label.clone(), self.mono_family, crate::theme::text_size(12.0))
                    .with_color(theme.accent),
            ))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish();
            let button = EventHandler::new(button)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(
                        crate::workspace::WorkspaceAction::RestartHostSession { pane_uuid, fresh },
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish();
            row = row.with_child(if i == 0 {
                button
            } else {
                Container::new(button).with_padding_left(8.0).finish()
            });
        }
        Some(
            Container::new(row.finish())
                .with_padding_left(12.0)
                .with_padding_bottom(6.0)
                .finish(),
        )
    }

    /// Open a web link shown in this terminal: the workspace opens it like
    /// every link (Dragent's browser unless the user turned that off), and
    /// the pop-up terminal over a page always in the system browser.
    pub(crate) fn open_web_link(&self, url: String, ctx: &mut ViewContext<Self>) {
        if self.links_open_in_system_browser {
            ctx.open_url(&url);
        } else {
            ctx.dispatch_typed_action_deferred(
                crate::workspace::WorkspaceAction::OpenExternalUrl { url },
            );
        }
    }

    /// Make this the pop-up terminal over a page: links it shows, and pages
    /// the programs in its shell open, go to the system browser. The shell
    /// statement rides in front of the first command, and the shell keeps it.
    pub(crate) fn use_system_browser(&mut self, shell_setup: Option<String>) {
        self.links_open_in_system_browser = true;
        self.pending_shell_setup = shell_setup;
    }

    pub fn submit_shell_command(&mut self, command: &str, ctx: &mut ViewContext<Self>) {
        self.submit_shell_command_with_focus(command, true, ctx);
    }

    /// A background pane launch must not queue keyboard focus for its input.
    pub(crate) fn submit_shell_command_preserving_focus(
        &mut self,
        command: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        self.submit_shell_command_with_focus(command, false, ctx);
    }

    fn submit_shell_command_with_focus(
        &mut self,
        command: &str,
        take_focus: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let command = command.trim().to_string();
        if command.is_empty() {
            return;
        }
        self.last_submit_was_agent = false;

        if self.command_history.last().map(String::as_str) != Some(command.as_str()) {
            self.command_history.push(command.clone());
        }
        crate::console::shell_history::record(&command);
        self.previous_command = Some(command.clone());
        self.command_history_idx = None;
        self.history_recall_prefix = None;
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text("", ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        // Controller → host: `set_text` fires no Changed event, so forward the
        // cleared command line explicitly — otherwise the host keeps the stale
        // text this controller just submitted (it only receives the PTY command).
        self.forward_control_surface(
            dragent_session_protocol::ControlSurface {
                command_input: Some(String::new()),
                command_cursor: Some(0),
                ..Default::default()
            },
            ctx,
        );
        self.record_command_for_suggestions(&command, ctx);
        self.current_running_command = Some(command.clone());
        ctx.update_model(&self.model, |m, mctx| {
            m.begin_command(command.clone(), mctx);
        });
        // Foreground command owns the keyboard now — make the collapsed input
        // bar inert so keys reach the PTY (cleared when the command finishes).
        self.command_input_passthrough
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let mode = self.active_term_mode(ctx);
        let setup = self.pending_shell_setup.take();
        self.write_command_after(setup.as_deref(), &command, mode);
        crate::confirm::set_foreground_command(self.pane_uuid, Some(command.clone()));
        self.pin_to_bottom(ctx);
        if take_focus {
            self.ensure_command_input_focused(ctx);
        }
        ctx.notify();
    }

    pub(crate) fn active_ai_tool_inner<C: ReadModel>(&self, ctx: &C) -> Option<AiTool> {
        // A mirror has no local running command — it reflects the host's tool,
        // pushed via the broadcast surface state, so the same AI-tool input
        // panel appears on the viewer as on the host.
        if self.is_remote {
            return self.remote_ai_tool;
        }
        let alt_active = self.model.read(ctx, |m, _| m.alt_screen_active());
        self.current_running_command.as_deref().and_then(|cmd| {
            // Alt-screen tools only count once their TUI is actually up — a
            // one-shot invocation (`gemini -p "..."`) is just a command.
            AiTool::from_command(cmd).filter(|t| !t.is_alt_screen() || alt_active)
        })
    }

    /// Registry artwork for the agent input panel: stored session identity
    /// first, running command's binary second. Builtins only — customs have
    /// no settings access from this layer (their identity still resolves in
    /// rail and tab chips via the workspace layer).
    pub(crate) fn session_agent_image(
        &self,
        is_dark: bool,
    ) -> Option<crate::workspace::AgentImage> {
        use crate::workspace::{builtin_image_for_cli, builtin_image_for_id};
        if let Some(id) = &self.agent_id {
            if let Some(image) = builtin_image_for_id(id, is_dark) {
                return Some(image);
            }
        }
        if let Some(binary) = self
            .current_running_command
            .as_deref()
            .and_then(|command| command.split_whitespace().next())
        {
            if let Some(image) = builtin_image_for_cli(binary, is_dark) {
                return Some(image);
            }
        }
        None
    }
}

impl Entity for TerminalView {
    type Event = ();
}

impl View for TerminalView {
    fn ui_name() -> &'static str {
        "TerminalView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let conversation = self.uses_agent_transcript();
        let alt_active = self.model.read(app, |m, _| m.alt_screen_active());
        // A terminal agent shown as a chat keeps its input panel: same path.
        if (alt_active || self.showing_cli_chat(app)) && !conversation {
            // Alt-screen (vim, a TUI, an AI CLI) still needs the close-confirm
            // modal overlaid — otherwise quitting the app while a process runs
            // here silently returns Cancel with no visible modal, and the only
            // way out is closing the tab. Overlay it on top of the full-screen
            // grid, same as the normal path.
            let alt = self.render_docked_backdrop(self.render_alt_screen_full(app));
            let alt = self.overlay_phone_control(alt);
            return self.overlay_confirm_modals(alt, self.theme);
        }

        let mouse_passthrough = !conversation && self.model.read(app, |m, _| m.terminal_mouse_active());
        let theme = self.theme;
        let scroll_area = if conversation {
            self.render_agent_transcript(app)
        } else {
        let viewport_h = self.output_viewport_height_px.max(1.0);

        // Compute which entries are visible given the current logical scroll position.
        let viewport_state = ViewportState::compute(
            &self.block_height_index,
            *self.block_scroll_pos.lock().unwrap(),
            viewport_h,
        );
        let partial_first_px = if is_block_list_virtualization_enabled() {
            viewport_state
                .visible_entries
                .first()
                .map(|e| e.partial_offset_px)
                .unwrap_or(0.0)
        } else {
            viewport_state.scroll_offset_px
        };
        // Scrollable range = content + a bottom gap, so the last block can always
        // be scrolled clear of the input bar instead of ending flush against it.
        let total_height_px = self.block_height_index.total_height_px() + BLOCK_LIST_BOTTOM_GAP_PX;

        let build_t0 = std::env::var_os("DRAGENT_LOG_PTY")
            .is_some()
            .then(web_time::Instant::now);
        let (inner_child, use_block_list) =
            self.build_block_list_child(&viewport_state.visible_entries, partial_first_px, app);
        if let Some(t0) = build_t0 {
            // This build runs under the shared core lock (model.read → with_core),
            // so its duration is how long each render blocks the PTY reader.
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if ms > 3.0 {
                log::info!(
                    "[render] block-list build held core lock {:.1}ms (visible_entries={})",
                    ms,
                    viewport_state.visible_entries.len(),
                );
            }
        }

        let pane_view_id = self.pane_view_id;
        let block_selected_text = self.block_selected_text.clone();
        let on_block_selection = move |args: SelectionUpdateArgs,
                                       ctx: &mut warpui::EventContext,
                                       _app: &AppContext| {
            *block_selected_text.write() = args.selection.clone();
            if args.selection.as_ref().is_some_and(|t| !t.is_empty()) {
                ctx.dispatch_typed_action_for(
                    pane_view_id,
                    TerminalAction::ClearInputEditorSelection,
                );
                if let Some(text) = args.selection {
                    ctx.dispatch_typed_action_for(pane_view_id, TerminalAction::CopyText { text });
                }
            }
        };

        // Phase 1.5 block-list path: Scrollable → BlockListScrollAdapter → SelectableArea → visible Flex.
        // Bootstrap/grid path: legacy ClippedScrollable (unchanged).
        let scrollable: Box<dyn Element> = if use_block_list {
            let selectable_output = SelectableArea::new(
                self.block_selectable_handle.clone(),
                on_block_selection,
                inner_child,
            )
            .on_selection_updated(|event_ctx, _app, cursor_pos| {
                event_ctx.dispatch_typed_action(TerminalAction::DragSelectScrollCheck {
                    cursor_y: cursor_pos.y(),
                });
            })
            .finish();

            let adapter = BlockListScrollAdapter::new(
                selectable_output,
                self.block_scroll_pos.clone(),
                total_height_px,
                partial_first_px,
                !mouse_passthrough,
            );
            // Overlaid scrollbar: a non-overlaid one SHRINKS the child's
            // constraint by the scrollbar width plus its padding, so block
            // separators and output stopped a few px short of the window edge
            // and left a permanent gutter — even with nothing to scroll. Float
            // it over the content instead so the output spans the full width.
            Scrollable::vertical(
                self.block_scroll_handle.clone(),
                adapter.finish_scrollable(),
                ScrollbarWidth::Auto,
                theme.disabled_text().into(),
                theme.main_text().into(),
                theme.background.into(),
            )
            .with_overlayed_scrollbar()
            .finish()
        } else {
            // Bootstrap / no-integration mode: keep old ClippedScrollable.
            let selectable_output = SelectableArea::new(
                self.block_selectable_handle.clone(),
                on_block_selection,
                inner_child,
            )
            .on_selection_updated(|event_ctx, _app, cursor_pos| {
                event_ctx.dispatch_typed_action(TerminalAction::DragSelectScrollCheck {
                    cursor_y: cursor_pos.y(),
                });
            })
            .finish();
            let aligned_output = if self.auto_scroll {
                Align::new(selectable_output).bottom_left().finish()
            } else {
                Align::new(selectable_output).top_left().finish()
            };
            let scroll_content = ConstrainedBox::new(aligned_output)
                .with_min_height(viewport_h)
                .finish();
            ClippedScrollable::vertical(
                self.block_scroll_state.clone(),
                scroll_content,
                ScrollbarWidth::Auto,
                theme.disabled_text().into(),
                theme.main_text().into(),
                theme.background.into(),
            )
            .with_overlayed_scrollbar()
            .finish()
        };

        // Overlay a sticky block header when the user has scrolled past a block's top.
        // A borrowed main-terminal pane can also be rendered in the editor's
        // dock. Keep sticky headers enabled for the main terminal, but never
        // paint them in the docked presentation.
        let scroll_area: Box<dyn Element> = if self.sticky_headers_enabled
            && !self.docked_backdrop_enabled
        {
            if let Some(ref info) = self.sticky_header {
                let render_ctx = self.block_render_context(app);
                let sticky_el = build_sticky_block_header(info, &render_ctx);
                let sticky_content = if let Some((filter_id, filter)) = self
                    .block_filter
                    .as_ref()
                    .filter(|(filter_id, _)| *filter_id == info.id)
                {
                    let theme_for_filter = self.theme;
                    let match_count = self.model.read(app, |m, _| {
                        m.with_core(|core| {
                            core.blocks()
                                .blocks()
                                .iter()
                                .find(|block| block.id == *filter_id)
                                .map(|block| count_block_matches(block, filter, &theme_for_filter))
                                .unwrap_or(0)
                        })
                    });
                    let filter_bar =
                        crate::terminal::render::block_list_element::build_block_filter_bar(
                            *filter_id,
                            &self.block_filter_editor,
                            filter.is_empty(),
                            filter.use_regex,
                            filter.regex_error,
                            match_count,
                            &render_ctx,
                        );
                    let filter_box = Container::new(filter_bar)
                        .with_padding_left(10.0)
                        .with_padding_right(13.0)
                        .with_padding_bottom(8.0)
                        .with_border(Border::left(3.0).with_border_color(theme.accent))
                        .finish();
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(sticky_el)
                        .with_child(filter_box)
                        .finish()
                } else {
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(sticky_el)
                        .finish()
                };
                let offset = OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(0.0, -self.sticky_push_offset),
                    // ParentBySize clamps the overlay to the parent's width so it
                    // does not bleed into the right sidebar.
                    ParentOffsetBounds::ParentBySize,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                );
                Box::new(
                    Stack::new()
                        .with_child(scrollable)
                        .with_positioned_child(sticky_content, offset),
                )
            } else {
                scrollable
            }
        } else {
            scrollable
        };

        scroll_area
        };

        // Overlay the per-block options dropdown when open (the filter bar renders
        // inline inside the block header).
        let scroll_area = self.wrap_block_menu_overlay(scroll_area, app);
        // Overlay the link action menu ("Open in Editor" / "Reveal in
        // Finder", etc.) when a detected link/path was clicked.
        let scroll_area = self.wrap_link_menu_overlay(scroll_area, app);
        // Overlay the checkpoint restore preview popover when open.
        let scroll_area = self.wrap_checkpoint_preview_overlay(scroll_area, app);

        let block_id = format!("terminal_block_viewport_{}", self.pane_uuid);
        let scroll_viewport = SavePosition::new(
            Container::new(scroll_area)
                .with_background_color(theme.pane_surface())
                .finish(),
            &block_id,
        )
        .finish();
        // Right-click anywhere in the block output area opens/repositions that
        // block's options menu exactly at the click point (works even while a
        // block is still running, unlike the header's "⋯" button).
        let scroll_viewport = {
            let tag = block_id.clone();
            EventHandler::new(scroll_viewport)
                .on_right_mouse_down(move |event_ctx, _app, position| {
                    if conversation { return DispatchEventResult::PropagateToParent; }
                    if let Some(rect) = event_ctx.element_position_by_id(&tag) {
                        let local = position - rect.origin();
                        event_ctx.dispatch_typed_action(TerminalAction::OpenBlockMenuAtClick {
                            local_x: local.x(),
                            local_y: local.y(),
                        });
                    }
                    DispatchEventResult::StopPropagation
                })
                .finish()
        };
        let wrapped = self.wrap_terminal_input(scroll_viewport, false, mouse_passthrough);
        let terminal_area = TerminalSizeElement::new(self.resize_tx.clone(), wrapped).finish();

        let output_id = format!("terminal_output_viewport_{}", self.pane_uuid);
        // FocusOutput only fires when NO child handles the click (no with_always_handle here),
        // so clicks on embedded editors (e.g. feedback editor in approval blocks) keep their focus.
        let terminal_with_focus = EventHandler::new(
            Container::new(SavePosition::new(terminal_area, &output_id).finish())
                .with_background_color(theme.pane_surface())
                .finish(),
        )
        .on_left_mouse_down(|event_ctx, _app, _position| {
            event_ctx.dispatch_typed_action(TerminalAction::FocusOutput);
            DispatchEventResult::PropagateToParent
        })
        .finish();
        // Scroll and mouse-up always fire regardless of whether a child handled the event.
        let output_area = EventHandler::new(terminal_with_focus)
            .on_left_mouse_up(|event_ctx, _app, _position| {
                event_ctx.dispatch_typed_action(TerminalAction::ClearBlockHints);
                event_ctx.dispatch_typed_action(TerminalAction::UserScrollInteraction);
                DispatchEventResult::PropagateToParent
            })
            .on_scroll_wheel(
                move |event_ctx, _app, _position, delta, _precise, _modifiers| {
                    if !conversation && !mouse_passthrough && delta.y().abs() > f32::EPSILON {
                        event_ctx.dispatch_typed_action(TerminalAction::BlockScroll {
                            delta_y: delta.y(),
                        });
                    }
                    DispatchEventResult::StopPropagation
                },
            )
            .with_always_handle()
            .finish();

        let mut pane_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max);
        // The history popover FLOATS over the bottom of the output area rather
        // than sitting in the column beside it. As a column sibling it consumed
        // real layout space, so the `Expanded` output above had to shrink and the
        // terminal content visibly slid up while the popover was being resized.
        // Stacked here it still lands directly on top of the input bar (the
        // output area ends exactly where the input begins), but growing it now
        // covers output instead of displacing it.
        //
        // The stack is inside `Expanded`, so its height is BOUNDED — unlike the
        // `MainAxisSize::Min` column the band used to live in, where an infinite
        // height constraint made a background Image resolve to an invalid rect.
        // A bottom-anchored Flex (spacer + band) is used rather than `Align`,
        // which re-anchors its child's layout each frame.
        // The rich `!` popover renders in the AI-tool alt-screen path instead.
        let hist_popover = if self.cmd_hist_rich {
            None
        } else {
            self.render_cmd_history_popover()
        };
        let output_area: Box<dyn Element> = match hist_popover {
            Some(cmd_hist) => {
                let pinned_to_bottom = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(Box::new(Expanded::new(
                        1.0,
                        warpui::elements::Empty::new().finish(),
                    )))
                    .with_child(cmd_hist)
                    .finish();
                Box::new(
                    Stack::new()
                        .with_child(output_area)
                        .with_child(pinned_to_bottom),
                )
            }
            None => output_area,
        };
        pane_col = pane_col.with_child(Box::new(Expanded::new(1.0, output_area)));

        if let Some(history_overlay) = self.render_history_search_overlay(app) {
            pane_col = pane_col.with_child(history_overlay);
        }
        if let Some(workflow_overlay) = self.render_workflow_search_overlay(app) {
            pane_col = pane_col.with_child(workflow_overlay);
        }
        if let Some(path_overlay) = self.render_path_picker_overlay(app) {
            pane_col = pane_col.with_child(path_overlay);
        }

        let active_ai = self.active_ai_tool(app);
        let non_altscreen_ai = match active_ai {
            Some(t) if !t.is_alt_screen() => Some(t),
            _ => None,
        };
        if let Some(diff_bar) = self.render_git_diff_bar(app) {
            pane_col = pane_col.with_child(diff_bar);
        }
        if let Some(branch_panel) = self.render_branch_diff_panel(app) {
            pane_col = pane_col.with_child(branch_panel);
        }
        if self.branch_menu_open {
            pane_col =
                pane_col.with_child(self.render_branch_menu_panel(app, &theme, self.mono_family));
        }
        if let Some(activity_strip) = self.render_agent_activity_strip() {
            pane_col = pane_col.with_child(activity_strip);
        }
        if let Some(heal_strip) = self.render_self_heal_strip(app) {
            pane_col = pane_col.with_child(heal_strip);
        }
        // `!`-command output card, pinned directly above the AI input.
        if let Some(side_card) = self.render_side_command_card() {
            pane_col = pane_col.with_child(side_card);
        }

        // Pending attachments: a strip of thumbnails/chips sitting directly ON
        // TOP of the input (its own surface, like the agent activity strip) —
        // never inside the input box.
        if !self.pending_attachments.is_empty() {
            pane_col = pane_col.with_child(self.render_attachment_strip(&theme, self.mono_family));
        }

        if let Some(restart) = self.render_host_restart_row(&theme) {
            pane_col = pane_col.with_child(restart);
        }

        if let Some(ai_tool) = non_altscreen_ai {
            let git_status = self.git_repo_status.read(app, |m, _| m.status.clone());
            let git_cwd = self.git_repo_status.read(app, |m, _| m.cwd.clone());
            let chrome = self.model.read(app, |m, _| {
                m.with_core(|core| sample_ai_chrome(core.active_emulator(), &theme))
            });
            let panel = ai_toolbar::render_ai_input_panel(
                &self.rich_input_editor,
                &theme,
                chrome,
                self.mono_family,
                git_status.as_ref(),
                Some(git_cwd.as_path()),
                self.rich_input_active,
                self.rich_input_proxy_chars > 0,
                self.voice_ui,
                self.voice_spin,
                self.input_notice.as_deref(),
                None,
                ai_tool,
                self.session_agent_image(theme.is_dark),
                &self.ai_panel_actions_with_view(app),
                self.chat_model_picker(app).map(|_| self.render_model_selector(app, &theme, self.mono_family)),
            );
            pane_col = pane_col.with_child(panel);
        } else {
            pane_col = pane_col.with_child(self.render_command_input_box(app));
        }

        let root: Box<dyn Element> = Container::new(pane_col.finish())
            .with_background_color(theme.pane_surface())
            .finish();
        let root = self.render_docked_backdrop(root);
        // Uniform ~10% tint over the ENTIRE pane (output + input bar). Added here,
        // before the toast/viewer/confirm-modal overlays below, so those stay
        // crisp and untinted on top of it.
        let root = self.with_terminal_scrim(root);

        // Overlay the commit-error toast if one is pending.
        let root = if let Some((ref title, ref err)) = self.error_toast {
            let toast = self.render_error_toast(title, err, &theme);
            let offset = OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(-12.0, -12.0),
                ParentOffsetBounds::ParentBySize,
                ParentAnchor::BottomRight,
                ChildAnchor::BottomRight,
            );
            Box::new(
                Stack::new()
                    .with_child(root)
                    .with_positioned_child(toast, offset),
            ) as Box<dyn Element>
        } else {
            root
        };

        // Image viewer carousel overlays the pane when open.
        let root = if let Some(idx) = self.image_viewer {
            let viewer = self.render_image_viewer(idx, &theme);
            Box::new(Stack::new().with_child(root).with_child(viewer)) as Box<dyn Element>
        } else {
            root
        };

        // Full-size Mermaid diagram viewer overlays the pane when open.
        let root = if let Some((ref path, width, height)) = self.diagram_viewer {
            let viewer =
                self.render_diagram_viewer(path, width, height, self.diagram_viewer_zoom, &theme);
            Box::new(Stack::new().with_child(root).with_child(viewer)) as Box<dyn Element>
        } else {
            root
        };

        // Confirmation guards overlay everything (close-confirm and pending
        // destructive-action modals) — shared with the alt-screen path.
        let root = self.overlay_phone_control(root);
        let root = self.overlay_confirm_modals(root, theme);

        self.wrap_outer_key_handler(root, app, active_ai)
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        self.pane_has_focus = true;
        self.send_focus_csi(true, ctx);
        // Hand focus on to the input inside, if it has one. A full-screen
        // program or running command keeps it right here: asking to focus
        // itself again would queue that request behind any other pending one,
        // and two panes doing so pass focus back and forth forever.
        if focus_ctx.is_self_focused() && !self.keeps_focus_itself(ctx) {
            self.ensure_command_input_focused(ctx);
        }
        ctx.emit(());
        ctx.notify();
    }

    fn on_blur(&mut self, _blur_ctx: &warpui::BlurContext, ctx: &mut ViewContext<Self>) {
        // The command editor normally owns keyboard focus, so leaving this
        // pane arrives as DescendentBlurred. Keep focus only when it moved to
        // another child of this same terminal (e.g. its rich input).
        if self.pane_has_focus(ctx) {
            return;
        }
        self.pane_has_focus = false;
        self.drag_selecting = false;
        self.send_focus_csi(false, ctx);
        ctx.notify();
    }
}

impl TerminalView {
    pub fn set_docked_backdrop_enabled(&mut self, enabled: bool) {
        self.docked_backdrop_enabled = enabled;
    }

    /// Same paint recipe as the command-history popover, but covering the full
    /// terminal ChildView: opaque base, optional image, translucent theme tint,
    /// then content.
    pub(crate) fn render_docked_backdrop(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        if !self.docked_backdrop_enabled {
            return content;
        }
        let theme = self.theme;
        match Theme::bg_image_for(theme.id) {
            Some(bg) => {
                let image = warpui_core::elements::Clipped::new(
                    warpui::elements::Image::new(
                        bg.asset_source(),
                        warpui::elements::CacheOption::Original,
                    )
                    .cover()
                    .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                    .finish(),
                )
                .finish();
                let tinted = Container::new(content)
                    .with_background_color(theme.background)
                    .finish();
                Container::new(Box::new(Stack::new().with_child(image).with_child(tinted)))
                    .with_background_color(theme.solid_background())
                    .finish()
            }
            None => Container::new(content)
                .with_background_color(theme.solid_background())
                .finish(),
        }
    }

    pub fn set_sticky_headers_enabled(&mut self, enabled: bool) {
        self.sticky_headers_enabled = enabled;
        if !enabled {
            self.sticky_header = None;
            self.sticky_push_offset = 0.0;
        }
    }
}

impl TypedActionView for TerminalView {
    type Action = TerminalAction;

    fn handle_action(&mut self, action: &TerminalAction, ctx: &mut ViewContext<Self>) {
        match action {
            TerminalAction::SelectDirEntry { .. }
            | TerminalAction::SelectLink { .. }
            | TerminalAction::OpenDirEntry { .. }
            | TerminalAction::OpenLink { .. }
            | TerminalAction::OpenLinkExternal { .. }
            | TerminalAction::RevealLinkInFinder { .. }
            | TerminalAction::CopyLinkTarget { .. }
            | TerminalAction::CloseLinkMenu
            | TerminalAction::HoverLink { .. }
            | TerminalAction::ClearHoverLink
            | TerminalAction::HoverAltLink { .. }
            | TerminalAction::ClearHoverAltLink
            | TerminalAction::SetDirHover(_)
            | TerminalAction::ToggleBlockExpansion { .. }
            | TerminalAction::ToggleAgentThinking { .. }
            | TerminalAction::ToggleToolStep { .. }
            | TerminalAction::ToggleChatLine { .. }
            | TerminalAction::CopyText { .. }
            | TerminalAction::CopyWithConfirmation { .. }
            | TerminalAction::ClearInputEditorSelection
            | TerminalAction::SetDirSort(_)
            | TerminalAction::SetDirView(_)
            | TerminalAction::FocusOutput
            | TerminalAction::ClearBlockHints
            | TerminalAction::FocusInput
            | TerminalAction::ApproveAgentAction { .. }
            | TerminalAction::AllowToolAlways { .. }
            | TerminalAction::RejectAgentAction { .. }
            | TerminalAction::PickAgentQuestionOption { .. }
            | TerminalAction::ConfirmAgentQuestion { .. }
            | TerminalAction::CancelQueuedMessage { .. }
            | TerminalAction::ToggleModeMenu
            | TerminalAction::SetChatMode { .. }
            | TerminalAction::ToggleModelMenu
            | TerminalAction::SetAgentModel { .. }
            | TerminalAction::SetChatEffort { .. }
            | TerminalAction::EffortPointer { .. }
            | TerminalAction::SlashPaletteMove { .. }
            | TerminalAction::RunSlashCommand { .. }
            | TerminalAction::ToggleHistoryPanel
            | TerminalAction::ResumeConversation { .. }
            | TerminalAction::ForwardRichInputProxyKey { .. }
            | TerminalAction::ClearAgentConversation
            | TerminalAction::AnalyzeBlock { .. }
            | TerminalAction::RunBlockFix { .. }
            | TerminalAction::DismissBlockFix { .. }
            | TerminalAction::RevealBlockFix { .. }
            | TerminalAction::CancelAgentCommand { .. }
            | TerminalAction::AutoFixBlock { .. }
            | TerminalAction::ToggleGitDiff
            | TerminalAction::AcceptAllAgentChanges
            | TerminalAction::ToggleBranchDiff
            | TerminalAction::ToggleBranchMenu
            | TerminalAction::CloseBranchMenu
            | TerminalAction::BranchListLoaded { .. }
            | TerminalAction::SwitchBranch { .. }
            | TerminalAction::CreateBranchFromInput
            | TerminalAction::ToggleFolderMenu
            | TerminalAction::CloseFolderMenu
            | TerminalAction::FolderListLoaded { .. }
            | TerminalAction::ChangeDirectory { .. }
            | TerminalAction::StageBranchFile { .. }
            | TerminalAction::StageAllBranchFiles
            | TerminalAction::UnstageAllBranchFiles
            | TerminalAction::DiscardBranchFile { .. }
            | TerminalAction::DiscardBranchFileConfirmed { .. }
            | TerminalAction::DiscardAllBranchChanges
            | TerminalAction::DiscardAllBranchChangesConfirmed
            | TerminalAction::GenerateBranchCommitMsg
            | TerminalAction::CommitBranchChanges
            | TerminalAction::DismissCommitError
            | TerminalAction::CopyCommitError { .. }
            | TerminalAction::PushBranchChanges
            | TerminalAction::CommitAndPushBranch
            | TerminalAction::PullBranchChanges
            | TerminalAction::RevertLastAgentTurn
            | TerminalAction::ToggleCheckpointPreview { .. }
            | TerminalAction::CloseCheckpointPreview
            | TerminalAction::DismissConfirm
            | TerminalAction::ResolveConfirm { .. } => {}
            _ => {
                self.entry_hint = None;
                self.link_hint = None;
            }
        }

        match action {
            TerminalAction::FocusOutput
            | TerminalAction::FocusInput
            | TerminalAction::ClearBlockHints
            | TerminalAction::AttachDroppedFiles { .. }
            | TerminalAction::RemovePendingAttachment { .. }
            | TerminalAction::ClearInputEditorSelection
            | TerminalAction::HistoryUp
            | TerminalAction::HistoryDown
            | TerminalAction::RichHistoryUp
            | TerminalAction::RichHistoryDown
            | TerminalAction::AcceptGhost
            | TerminalAction::Submit
            | TerminalAction::InsertNewline
            | TerminalAction::Interrupt
            | TerminalAction::ToggleRichInput
            | TerminalAction::SubmitRichInput
            | TerminalAction::ToggleVoiceInput
            | TerminalAction::OpenFilePicker
            | TerminalAction::ShiftHeld(..)
            | TerminalAction::CtrlHeld(..)
            | TerminalAction::AltHeld(..)
            | TerminalAction::CmdHeld(..)
            | TerminalAction::Copy
            | TerminalAction::Paste
            | TerminalAction::SendPty(..)
            | TerminalAction::TerminalMouse { .. }
            | TerminalAction::TerminalWheel { .. }
            | TerminalAction::BlockScroll { .. }
            | TerminalAction::UserScrollInteraction
            | TerminalAction::DragSelectScrollCheck { .. }
            | TerminalAction::TabComplete
            | TerminalAction::TabCompletePrev
            | TerminalAction::AcceptCompletion
            | TerminalAction::DismissCompletion
            | TerminalAction::ClickPathCompletion { .. }
            | TerminalAction::ToggleVimMode
            | TerminalAction::ToggleInputTypeLock
            | TerminalAction::ForwardRichInputProxyKey { .. } => {
                self.handle_terminal_input_action(action, ctx)
            }
            TerminalAction::CloseCmdHistory
            | TerminalAction::CmdHistResizeBy { .. }
            | TerminalAction::CmdHistoryClick { .. }
            | TerminalAction::OpenSideCmdHistory
            | TerminalAction::HistorySearchOpen
            | TerminalAction::HistorySearchClose
            | TerminalAction::HistorySearchUp
            | TerminalAction::HistorySearchDown
            | TerminalAction::HistorySearchAccept
            | TerminalAction::HistorySearchClick { .. }
            | TerminalAction::HistorySearchBackspace
            | TerminalAction::HistorySearchQueryAppend(..)
            | TerminalAction::WorkflowSearchOpen
            | TerminalAction::WorkflowSearchClose
            | TerminalAction::WorkflowSearchUp
            | TerminalAction::WorkflowSearchDown
            | TerminalAction::WorkflowSearchAccept
            | TerminalAction::WorkflowSearchClick { .. }
            | TerminalAction::WorkflowSearchBackspace
            | TerminalAction::WorkflowSearchQueryAppend(..)
            | TerminalAction::WorkflowArgPrev
            | TerminalAction::WorkflowArgNext
            | TerminalAction::ToggleHistoryPanel => {
                self.handle_terminal_history_action(action, ctx)
            }
            TerminalAction::SelectDirEntry { .. }
            | TerminalAction::OpenDirEntry { .. }
            | TerminalAction::SelectLink { .. }
            | TerminalAction::OpenLink { .. }
            | TerminalAction::OpenLinkExternal { .. }
            | TerminalAction::RevealLinkInFinder { .. }
            | TerminalAction::CopyLinkTarget { .. }
            | TerminalAction::CloseLinkMenu
            | TerminalAction::HoverLink { .. }
            | TerminalAction::ClearHoverLink
            | TerminalAction::HoverAltLink { .. }
            | TerminalAction::ClearHoverAltLink
            | TerminalAction::SetDirHover(_)
            | TerminalAction::ToggleBlockExpansion { .. }
            | TerminalAction::ToggleAgentThinking { .. }
            | TerminalAction::ToggleToolStep { .. }
            | TerminalAction::ToggleChatLine { .. }
            | TerminalAction::SetChatView { .. }
            | TerminalAction::StopChatAgent
            | TerminalAction::AnswerAcpPermission { .. }
            | TerminalAction::ShowEarlierChat
            | TerminalAction::JumpToLatestChat
            | TerminalAction::ToggleDiffExpand { .. }
            | TerminalAction::CopyText { .. }
            | TerminalAction::CopyWithConfirmation { .. }
            | TerminalAction::ToggleBlockMenu { .. }
            | TerminalAction::CloseBlockMenu
            | TerminalAction::TerminateBlock { .. }
            | TerminalAction::OpenBlockMenuAtClick { .. }
            | TerminalAction::CopyBlockOutput { .. }
            | TerminalAction::CopyBlockCommandAndOutput { .. }
            | TerminalAction::ScrollBlockToTop { .. }
            | TerminalAction::ScrollBlockToBottom { .. }
            | TerminalAction::ToggleBlockFilter { .. }
            | TerminalAction::CloseBlockFilter { .. }
            | TerminalAction::ToggleBlockFilterRegex { .. }
            | TerminalAction::BlockFilterMatch { .. }
            | TerminalAction::SetDirSort(..)
            | TerminalAction::SetDirView(..)
            | TerminalAction::DetachBlock { .. } => self.handle_terminal_blocks_action(action, ctx),
            TerminalAction::ToggleModeMenu
            | TerminalAction::ToggleModelMenu
            | TerminalAction::SetAgentModel { .. }
            | TerminalAction::SetChatEffort { .. }
            | TerminalAction::EffortPointer { .. }
            | TerminalAction::SetChatMode { .. }
            | TerminalAction::SlashPaletteMove { .. }
            | TerminalAction::RunSlashCommand { .. }
            | TerminalAction::ResumeConversation { .. }
            | TerminalAction::ApproveAgentAction { .. }
            | TerminalAction::AllowToolAlways { .. }
            | TerminalAction::RejectAgentAction { .. }
            | TerminalAction::PickAgentQuestionOption { .. }
            | TerminalAction::ConfirmAgentQuestion { .. }
            | TerminalAction::MoveAgentQuestionCursor { .. }
            | TerminalAction::PickAgentQuestionCursor { .. }
            | TerminalAction::PickAgentQuestionDigit { .. }
            | TerminalAction::SubmitAgentQuestionText { .. }
            | TerminalAction::CancelQueuedMessage { .. }
            | TerminalAction::ApprovePlan { .. }
            | TerminalAction::RefinePlan { .. }
            | TerminalAction::ClearAgentConversation
            | TerminalAction::StopAgentStream
            | TerminalAction::ContinueAgent { .. }
            | TerminalAction::ReviewAgentChanges
            | TerminalAction::AnalyzeBlock { .. }
            | TerminalAction::RevealBlockFix { .. }
            | TerminalAction::AutoFixBlock { .. }
            | TerminalAction::CancelAgentCommand { .. }
            | TerminalAction::OpenInAgent { .. }
            | TerminalAction::RunBlockFix { .. }
            | TerminalAction::DismissBlockFix { .. }
            | TerminalAction::AcceptAllAgentChanges
            | TerminalAction::RestoreToMessageConfirmed { .. }
            | TerminalAction::RevertLastAgentTurn
            | TerminalAction::RevertLastAgentTurnConfirmed => {
                self.handle_terminal_agent_action(action, ctx)
            }
            TerminalAction::ToggleGitDiff
            | TerminalAction::ToggleBranchDiff
            | TerminalAction::StageBranchFile { .. }
            | TerminalAction::StageAllBranchFiles
            | TerminalAction::UnstageAllBranchFiles
            | TerminalAction::DiscardBranchFile { .. }
            | TerminalAction::DiscardBranchFileConfirmed { .. }
            | TerminalAction::DiscardAllBranchChanges
            | TerminalAction::DiscardAllStagedChanges
            | TerminalAction::DiscardAllStagedChangesConfirmed
            | TerminalAction::DiscardAllBranchChangesConfirmed
            | TerminalAction::GenerateBranchCommitMsg
            | TerminalAction::CommitBranchChanges
            | TerminalAction::DismissCommitError
            | TerminalAction::CopyCommitError { .. }
            | TerminalAction::CommitAndPushBranch
            | TerminalAction::ToggleCheckpointPreview { .. }
            | TerminalAction::CloseCheckpointPreview
            | TerminalAction::PullBranchChanges
            | TerminalAction::PushBranchChanges
            | TerminalAction::ToggleBranchMenu
            | TerminalAction::CloseBranchMenu
            | TerminalAction::BranchListLoaded { .. }
            | TerminalAction::CreateBranchFromName { .. }
            | TerminalAction::SwitchBranch { .. }
            | TerminalAction::CreateBranchFromInput
            | TerminalAction::ToggleFolderMenu
            | TerminalAction::CloseFolderMenu
            | TerminalAction::FolderListLoaded { .. }
            | TerminalAction::ChangeDirectory { .. } => {
                self.handle_terminal_git_action(action, ctx)
            }
            TerminalAction::OpenImageViewer { .. }
            | TerminalAction::CloseImageViewer
            | TerminalAction::ImageViewerNav { .. }
            | TerminalAction::OpenDiagramViewer { .. }
            | TerminalAction::CloseDiagramViewer
            | TerminalAction::DiagramViewerZoomIn
            | TerminalAction::DiagramViewerZoomOut
            | TerminalAction::DiagramViewerZoomReset => {
                self.handle_terminal_viewer_action(action, ctx)
            }
            TerminalAction::AcceptRichGhost
            | TerminalAction::CloseSideCommand
            | TerminalAction::ResetSideShell
            | TerminalAction::CancelSideCommand
            | TerminalAction::SideCardResizeBy { .. }
            | TerminalAction::SideShellFollowTail { .. } => {
                self.handle_terminal_side_action(action, ctx)
            }
            TerminalAction::DismissConfirm
            | TerminalAction::ResolveConfirm { .. } => {
                self.handle_terminal_correction_action(action, ctx)
            }
            TerminalAction::TakeBackFromPhone => self.take_back_from_phone(ctx),
        }
    }
}
