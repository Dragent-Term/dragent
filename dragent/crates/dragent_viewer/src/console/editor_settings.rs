//! Persisted editor + terminal preferences (vim mode, shell, typography, cursor).

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use warpui::fonts::Weight;

// ── Terminal preferences ──────────────────────────────────────────────────────

/// How the terminal cursor is drawn in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    Block,
    #[default]
    Bar,
    Underline,
}

impl CursorStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Block => "Block",
            Self::Bar => "Bar",
            Self::Underline => "Underline",
        }
    }
}

pub const FONT_SIZE_MIN: f32 = 10.0;
pub const FONT_SIZE_MAX: f32 = 20.0;
pub const LINE_HEIGHT_PCT_MIN: u32 = 100;
pub const LINE_HEIGHT_PCT_MAX: u32 = 200;
pub const TAB_SIZE_MIN: u32 = 1;
pub const TAB_SIZE_MAX: u32 = 8;
pub const CURSOR_BLINK_RATE_MIN_MS: u32 = 200;
pub const CURSOR_BLINK_RATE_MAX_MS: u32 = 2000;
pub const AUTO_SAVE_DELAY_MIN_MS: u32 = 100;
pub const AUTO_SAVE_DELAY_MAX_MS: u32 = 10000;

// Scrollback bounds live in `dragent_terminal` (single source of truth — the
// grid enforces the same limits); re-exported here for settings UI + tests.
pub use dragent_terminal::{
    SCROLLBACK_DEFAULT_LINES as SCROLLBACK_LINES_DEFAULT,
    SCROLLBACK_MAX_LINES as SCROLLBACK_LINES_MAX, SCROLLBACK_MIN_LINES as SCROLLBACK_LINES_MIN,
};

/// How macOS Option keys act in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MacOptionAsAlt {
    #[default]
    Auto,
    True,
    False,
    Left,
    Right,
}

impl MacOptionAsAlt {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::True => "Both as Alt",
            Self::False => "Never",
            Self::Left => "Left as Alt",
            Self::Right => "Right as Alt",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::True => "true",
            Self::False => "false",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "true" => Some(Self::True),
            "false" => Some(Self::False),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    /// Whether Option should be treated as Alt. WarpUI keystrokes do not
    /// distinguish left/right Option, so Left/Right behave like True
    /// (documented limitation); Auto follows the platform (macOS only).
    pub fn treats_option_as_alt(self) -> bool {
        match self {
            Self::True | Self::Left | Self::Right => true,
            Self::False => false,
            Self::Auto => cfg!(target_os = "macos"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalPrefs {
    /// Absolute path of the shell launched in new sessions. `None` = resolve
    /// the system default ($SHELL / login shell), exactly as before.
    #[serde(default)]
    pub default_shell: Option<String>,
    /// Code font: terminals, editors, diffs and code everywhere. `None`
    /// uses the platform default.
    #[serde(default)]
    pub font_family: Option<String>,
    /// Interface font: every other piece of text. `None` uses the system
    /// font (San Francisco on macOS, bundled Inter elsewhere).
    #[serde(default)]
    pub ui_font_family: Option<String>,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Line height as a percentage of font size (120 = 1.2×).
    #[serde(default = "default_line_height_pct")]
    pub line_height_pct: u32,
    #[serde(default)]
    pub cursor_style: CursorStyle,
    #[serde(default = "default_true")]
    pub cursor_blink: bool,
    /// Caret blink interval in milliseconds. Applies to both the editor
    /// caret and the terminal grid cursor.
    #[serde(default = "default_cursor_blink_rate_ms")]
    pub cursor_blink_rate_ms: u32,
    /// How macOS Option keys act in the PTY.
    #[serde(default)]
    pub mac_option_as_alt: MacOptionAsAlt,
    /// Map JIS ¥ key to backslash for PTY.
    #[serde(default)]
    pub jis_yen_to_backslash: bool,
    /// Auto-copy terminal selection.
    #[serde(default)]
    pub clipboard_on_select: bool,
    /// Strip gutter whitespace on copy.
    #[serde(default = "default_true")]
    pub copy_trims_gutter: bool,
    /// Extra chars that break words.
    /// `None` = default word boundaries.
    #[serde(default)]
    pub word_separator: Option<String>,
    /// Middle-click pastes.
    /// `None` = platform default (Linux pastes, elsewhere not).
    #[serde(default)]
    pub middle_click_paste: Option<bool>,
    /// Kept terminal history, in lines. There is no unlimited mode: values
    /// clamp to `[SCROLLBACK_LINES_MIN, SCROLLBACK_LINES_MAX]`, and a stored
    /// `0` (the legacy unlimited marker) loads as the default.
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
}

fn default_font_size() -> f32 {
    14.0
}

fn default_line_height_pct() -> u32 {
    // Compact default spacing keeps dense terminal output readable without
    // making command history consume the whole viewport.
    115
}

fn default_cursor_blink_rate_ms() -> u32 {
    800
}

fn default_tab_size() -> u32 {
    4
}

fn default_auto_save_delay_ms() -> u32 {
    1000
}

fn default_scrollback_lines() -> usize {
    SCROLLBACK_LINES_DEFAULT
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        Self {
            default_shell: None,
            font_family: None,
            ui_font_family: None,
            font_size: default_font_size(),
            line_height_pct: default_line_height_pct(),
            cursor_style: CursorStyle::Bar,
            cursor_blink: true,
            cursor_blink_rate_ms: default_cursor_blink_rate_ms(),
            mac_option_as_alt: MacOptionAsAlt::Auto,
            jis_yen_to_backslash: false,
            clipboard_on_select: false,
            copy_trims_gutter: true,
            word_separator: None,
            middle_click_paste: None,
            scrollback_lines: default_scrollback_lines(),
        }
    }
}

impl TerminalPrefs {
    pub fn clamped_font_size(&self) -> f32 {
        self.font_size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
    }

    pub fn line_height_ratio(&self) -> f32 {
        (self
            .line_height_pct
            .clamp(LINE_HEIGHT_PCT_MIN, LINE_HEIGHT_PCT_MAX) as f32)
            / 100.0
    }

    pub fn clamped_cursor_blink_rate_ms(&self) -> u32 {
        self.cursor_blink_rate_ms
            .clamp(CURSOR_BLINK_RATE_MIN_MS, CURSOR_BLINK_RATE_MAX_MS)
    }

    /// Resolve middle-click paste: explicit pref wins, otherwise Linux-only.
    pub fn middle_click_paste_enabled(&self) -> bool {
        match self.middle_click_paste {
            Some(v) => v,
            None => cfg!(target_os = "linux"),
        }
    }

    /// Effective scrollback cap, in lines. A stored `0` (legacy unlimited)
    /// loads as the default; anything else clamps to the mandatory bounds.
    pub fn clamped_scrollback_lines(&self) -> usize {
        if self.scrollback_lines == 0 {
            SCROLLBACK_LINES_DEFAULT
        } else {
            self.scrollback_lines
                .clamp(SCROLLBACK_LINES_MIN, SCROLLBACK_LINES_MAX)
        }
    }

    /// Extra word-break chars from the word-separator setting, if set.
    pub fn extra_word_separators(&self) -> Option<&str> {
        self.word_separator
            .as_deref()
            .filter(|s| !s.is_empty())
    }
}

// ── Source-editor preferences ───────────────────────────────────────────────
// NOTE: word wrap is deliberately not a setting yet. The visual-line stack
// (click mapping, scrolling, fold projection, gutter sync) assumes one
// visual row per physical line; wrapping needs a dedicated layout pass.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorPrefs {
    /// Weight of all text in the app (Light … Bold); heavier text such as
    /// headings moves by the same number of steps.
    #[serde(default)]
    pub font_weight: Weight,
    /// Tab stop width in spaces (1–8).
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
    /// Tab key inserts spaces; when false it inserts a tab character.
    #[serde(default = "default_true")]
    pub insert_spaces: bool,
    /// Gutter with line numbers on source editors.
    /// Gutter with line numbers on source editors.
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    /// Render spaces as visible dots.
    #[serde(default)]
    pub show_whitespace: bool,
    /// Indent-guide rails on source editors.
    #[serde(default = "default_true")]
    pub indent_guides: bool,
    /// Debounced save after edits.
    #[serde(default)]
    pub auto_save: bool,
    /// Delay before an autosave in milliseconds, clamped 100–10000.
    #[serde(default = "default_auto_save_delay_ms")]
    pub auto_save_delay_ms: u32,
    /// Miniature overview rail.
    #[serde(default)]
    pub minimap_enabled: bool,
    /// Empty/`None` follows the terminal code font.
    #[serde(default)]
    pub font_family: Option<String>,
    /// Soft-wrap long lines in source editors and
    /// markdown surfaces. Source grid layout is single-row-per-line, so
    /// `true` applies to markdown/wrapped surfaces and is persisted for
    /// the source editor until its layout pass lands.
    #[serde(default = "default_true")]
    pub word_wrap: bool,
    /// Squiggles in markdown preview.
    #[serde(default = "default_true")]
    pub spellcheck_enabled: bool,
    /// Review affordances on markdown.
    #[serde(default = "default_true")]
    pub markdown_review_tools: bool,
    /// Single-click opens a reusable preview.
    #[serde(default = "default_true")]
    pub preview_tabs: bool,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self {
            font_weight: Weight::Normal,
            tab_size: default_tab_size(),
            insert_spaces: true,
            line_numbers: true,
            show_whitespace: false,
            indent_guides: true,
            auto_save: false,
            auto_save_delay_ms: default_auto_save_delay_ms(),
            minimap_enabled: false,
            font_family: None,
            word_wrap: true,
            spellcheck_enabled: true,
            markdown_review_tools: true,
            preview_tabs: true,
        }
    }
}

impl EditorPrefs {
    pub fn clamped_tab_size(&self) -> usize {
        (self.tab_size.clamp(TAB_SIZE_MIN, TAB_SIZE_MAX)) as usize
    }

    pub fn clamped_auto_save_delay_ms(&self) -> u32 {
        self.auto_save_delay_ms
            .clamp(AUTO_SAVE_DELAY_MIN_MS, AUTO_SAVE_DELAY_MAX_MS)
    }
}

// ── EditorSettings ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    #[serde(default)]
    pub vim_mode_enabled: bool,
    #[serde(default = "default_true")]
    pub input_autodetection_enabled: bool,
    #[serde(default)]
    pub terminal: TerminalPrefs,
    #[serde(default)]
    pub editor: EditorPrefs,
}

fn default_true() -> bool {
    true
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            vim_mode_enabled: false,
            input_autodetection_enabled: true,
            terminal: TerminalPrefs::default(),
            editor: EditorPrefs::default(),
        }
    }
}

impl EditorSettings {
    pub fn config_path() -> PathBuf {
        // No config dir in the browser; the wasm viewer just falls back to
        // defaults (the read below errors → `unwrap_or_default`).
        #[cfg(not(target_family = "wasm"))]
        let base = dragent_paths::home();
        #[cfg(target_family = "wasm")]
        let base = PathBuf::from(".");
        base.join("settings.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
        Self::refresh_global(self);
    }

    pub fn toggle_vim_mode(&mut self) -> bool {
        self.vim_mode_enabled = !self.vim_mode_enabled;
        self.save();
        self.vim_mode_enabled
    }

    // ── Global snapshot ───────────────────────────────────────────────────────
    //
    // Render paths (grid metrics, cursor painting) and session spawns read the
    // terminal prefs through this cached snapshot; `save()` refreshes it.

    fn global_cell() -> &'static RwLock<Arc<EditorSettings>> {
        static GLOBAL: OnceLock<RwLock<Arc<EditorSettings>>> = OnceLock::new();
        GLOBAL.get_or_init(|| RwLock::new(Arc::new(EditorSettings::load())))
    }

    /// Current settings snapshot (cheap: one RwLock read + Arc clone).
    pub fn global() -> Arc<EditorSettings> {
        Self::global_cell()
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    fn refresh_global(settings: &EditorSettings) {
        let snapshot = Arc::new(settings.clone());
        match Self::global_cell().write() {
            Ok(mut g) => *g = snapshot,
            Err(mut p) => **p.get_mut() = snapshot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_prefs_scrollback_cap() {
        // Fresh default is 10,000 — and legacy files without the key agree.
        assert_eq!(TerminalPrefs::default().scrollback_lines, 10_000);
        let legacy: EditorSettings = serde_json::from_str(r#"{"vim_mode_enabled":true}"#).unwrap();
        assert_eq!(legacy.terminal.clamped_scrollback_lines(), 10_000);
        // A stored 0 (legacy unlimited marker) loads as the default.
        let zero: EditorSettings =
            serde_json::from_str(r#"{"terminal":{"scrollback_lines":0}}"#).unwrap();
        assert_eq!(zero.terminal.scrollback_lines, 0);
        assert_eq!(zero.terminal.clamped_scrollback_lines(), 10_000);
        // Anything else clamps to the mandatory [1_000, 200_000].
        let mut p = TerminalPrefs::default();
        p.scrollback_lines = 500;
        assert_eq!(p.clamped_scrollback_lines(), 1_000);
        p.scrollback_lines = 1_000;
        assert_eq!(p.clamped_scrollback_lines(), 1_000);
        p.scrollback_lines = 50_000;
        assert_eq!(p.clamped_scrollback_lines(), 50_000);
        p.scrollback_lines = 200_000;
        assert_eq!(p.clamped_scrollback_lines(), 200_000);
        p.scrollback_lines = 999_999;
        assert_eq!(p.clamped_scrollback_lines(), 200_000);
        // Round-trips through JSON.
        p.scrollback_lines = 25_000;
        let json = serde_json::to_string(&p).unwrap();
        let back: TerminalPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back.scrollback_lines, 25_000);
        assert_eq!(back.clamped_scrollback_lines(), 25_000);
    }

    #[test]
    fn terminal_prefs_default_from_legacy_json() {
        // Old settings.json without the `terminal` section loads with defaults.
        let s: EditorSettings = serde_json::from_str(r#"{"vim_mode_enabled":true}"#).unwrap();
        assert!(s.vim_mode_enabled);
        assert_eq!(s.terminal.font_size, 14.0);
        assert_eq!(s.terminal.line_height_pct, 115);
        assert_eq!(s.terminal.cursor_style, CursorStyle::Bar);
        assert!(s.terminal.cursor_blink);
        assert!(s.terminal.default_shell.is_none());
        assert!(s.terminal.font_family.is_none());
        assert!(s.terminal.ui_font_family.is_none());
        // Input defaults.
        assert_eq!(s.terminal.mac_option_as_alt, MacOptionAsAlt::Auto);
        assert!(!s.terminal.jis_yen_to_backslash);
        assert!(!s.terminal.clipboard_on_select);
        assert!(s.terminal.copy_trims_gutter);
        assert!(s.terminal.word_separator.is_none());
        assert!(s.terminal.middle_click_paste.is_none());
    }

    #[test]
    fn terminal_prefs_round_trip() {
        let mut s = EditorSettings::default();
        s.terminal.default_shell = Some("/bin/zsh".into());
        s.terminal.font_family = Some("Menlo".into());
        s.terminal.ui_font_family = Some("Inter".into());
        s.terminal.font_size = 16.0;
        s.terminal.line_height_pct = 150;
        s.terminal.cursor_style = CursorStyle::Bar;
        s.terminal.cursor_blink = false;
        s.terminal.mac_option_as_alt = MacOptionAsAlt::Left;
        s.terminal.jis_yen_to_backslash = true;
        s.terminal.clipboard_on_select = true;
        s.terminal.copy_trims_gutter = false;
        s.terminal.word_separator = Some(":".into());
        s.terminal.middle_click_paste = Some(true);
        let json = serde_json::to_string(&s).unwrap();
        let r: EditorSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(r.terminal, s.terminal);
        assert_eq!(r.terminal.line_height_ratio(), 1.5);
    }

    #[test]
    fn terminal_input_legacy_json_and_helpers() {
        // Partial terminal JSON from older files still loads with new defaults.
        let s: EditorSettings =
            serde_json::from_str(r#"{"terminal":{"font_size":14.0}}"#).unwrap();
        assert_eq!(s.terminal.mac_option_as_alt, MacOptionAsAlt::Auto);
        assert!(s.terminal.copy_trims_gutter);
        assert!(s.terminal.word_separator.is_none());
        // Helpers.
        assert_eq!(MacOptionAsAlt::parse("left"), Some(MacOptionAsAlt::Left));
        assert_eq!(MacOptionAsAlt::Auto.as_str(), "auto");
        assert!(!MacOptionAsAlt::False.treats_option_as_alt());
        assert!(MacOptionAsAlt::True.treats_option_as_alt());
        let mut p = TerminalPrefs::default();
        p.word_separator = Some(String::new());
        assert!(p.extra_word_separators().is_none());
        p.word_separator = Some(":".into());
        assert_eq!(p.extra_word_separators(), Some(":"));
    }

    #[test]
    fn terminal_prefs_clamping() {
        let mut p = TerminalPrefs::default();
        p.font_size = 99.0;
        p.line_height_pct = 900;
        assert_eq!(p.clamped_font_size(), FONT_SIZE_MAX);
        assert_eq!(p.line_height_ratio(), 2.0);
    }

    #[test]
    fn editor_prefs_default_from_legacy_json() {
        // Old settings.json without `editor` or blink-rate loads defaults.
        let s: EditorSettings = serde_json::from_str(r#"{"vim_mode_enabled":true}"#).unwrap();
        assert_eq!(s.editor.font_weight, Weight::Normal);
        assert_eq!(s.editor.tab_size, 4);
        assert!(s.editor.insert_spaces);
        assert!(s.editor.line_numbers);
        assert!(!s.editor.show_whitespace);
        assert!(s.editor.indent_guides);
        assert_eq!(s.terminal.cursor_blink_rate_ms, 800);
        // Editor defaults.
        assert!(!s.editor.auto_save);
        assert_eq!(s.editor.auto_save_delay_ms, 1000);
        assert!(!s.editor.minimap_enabled);
        assert!(s.editor.font_family.is_none());
        assert!(s.editor.word_wrap);
        assert!(s.editor.spellcheck_enabled);
        assert!(s.editor.markdown_review_tools);
        assert!(s.editor.preview_tabs);
    }

    #[test]
    fn editor_prefs_round_trip_and_clamping() {
        use warpui::fonts::Weight as W;
        let mut s = EditorSettings::default();
        s.editor.font_weight = W::Bold;
        s.editor.tab_size = 2;
        s.editor.insert_spaces = false;
        s.editor.line_numbers = false;
        s.editor.show_whitespace = true;
        s.editor.indent_guides = false;
        s.editor.auto_save = true;
        s.editor.auto_save_delay_ms = 2500;
        s.editor.minimap_enabled = true;
        s.editor.font_family = Some("Menlo".into());
        s.editor.word_wrap = false;
        s.editor.spellcheck_enabled = false;
        s.editor.markdown_review_tools = false;
        s.editor.preview_tabs = false;
        s.terminal.cursor_blink_rate_ms = 1200;
        let json = serde_json::to_string(&s).unwrap();
        let r: EditorSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(r.editor.font_weight, W::Bold);
        assert_eq!(r.editor.clamped_tab_size(), 2);
        assert!(!r.editor.insert_spaces);
        assert!(!r.editor.line_numbers);
        assert!(r.editor.show_whitespace);
        assert!(!r.editor.indent_guides);
        assert!(r.editor.auto_save);
        assert_eq!(r.editor.clamped_auto_save_delay_ms(), 2500);
        assert!(r.editor.minimap_enabled);
        assert_eq!(r.editor.font_family.as_deref(), Some("Menlo"));
        assert!(!r.editor.word_wrap);
        assert!(!r.editor.spellcheck_enabled);
        assert!(!r.editor.markdown_review_tools);
        assert!(!r.editor.preview_tabs);
        assert_eq!(r.terminal.clamped_cursor_blink_rate_ms(), 1200);
        s.editor.tab_size = 99;
        s.terminal.cursor_blink_rate_ms = 99;
        s.editor.auto_save_delay_ms = 99;
        assert_eq!(s.editor.clamped_tab_size(), TAB_SIZE_MAX as usize);
        assert_eq!(
            s.terminal.clamped_cursor_blink_rate_ms(),
            CURSOR_BLINK_RATE_MIN_MS
        );
        assert_eq!(
            s.editor.clamped_auto_save_delay_ms(),
            AUTO_SAVE_DELAY_MIN_MS
        );
        s.editor.auto_save_delay_ms = 99_999;
        assert_eq!(
            s.editor.clamped_auto_save_delay_ms(),
            AUTO_SAVE_DELAY_MAX_MS
        );
    }
}
