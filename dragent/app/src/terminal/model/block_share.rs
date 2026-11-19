//! Host-side block snapshots for shared sessions.
//!
//! The desktop renders the terminal as a list of command *blocks* (cards), not a
//! flat cell grid. To reproduce that UI in a remote viewer we broadcast the block
//! structure — each block's command, status, and styled output lines — rather
//! than raw PTY bytes. This module snapshots a block into a transport-neutral
//! form and converts styled lines to the wire color format.

use dragent_session_protocol::{WireLine, WireSpan};
use warpui::color::ColorU;

use crate::console::escape_parse::ColoredLine;

/// A styled snapshot of the alt-screen grid plus cursor, for viewers that render
/// full-screen programs (vim, htop, …) without re-running the VT parser.
#[derive(Clone, Debug)]
pub struct AltScreenSnapshot {
    pub rows: usize,
    pub cols: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub lines: Vec<ColoredLine>,
}

/// A point-in-time snapshot of one command block, ready to diff and broadcast.
#[derive(Clone, Debug)]
pub struct HostBlockSnapshot {
    pub id: usize,
    /// Original creation time, preserved across resync/reconnect so command
    /// blocks keep their position relative to agent blocks.
    pub created_at_unix_ms: i64,
    pub command: String,
    pub cwd: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub failed: bool,
    /// Styled output lines (live emulator snapshot while running, stored lines
    /// once completed).
    pub lines: Vec<ColoredLine>,
    /// Set when this block's output is a directory listing. The host resolves it
    /// from its own filesystem, which a viewer cannot do — its filesystem is a
    /// different machine's — and scraping the `ls` text back into entries loses
    /// items. Sending the resolved entries is what keeps the two views identical.
    pub dir_entries: Option<Vec<crate::console::dir_listing::DirEntry>>,
}

/// Convert resolved directory entries to their wire form.
pub fn wire_dir_entries(
    entries: &[crate::console::dir_listing::DirEntry],
) -> Vec<dragent_session_protocol::WireDirEntry> {
    use crate::console::dir_listing::EntryKind;
    entries
        .iter()
        .map(|e| dragent_session_protocol::WireDirEntry {
            name: e.name.clone(),
            kind: match e.kind {
                EntryKind::Directory => "directory",
                EntryKind::Executable => "executable",
                EntryKind::Symlink => "symlink",
                EntryKind::File => "file",
            }
            .to_string(),
            size: e.size,
            color: Some([e.color.r, e.color.g, e.color.b, e.color.a]),
        })
        .collect()
}

/// Inverse of [`wire_dir_entries`], for the mirror.
pub fn dir_entries_from_wire(
    entries: &[dragent_session_protocol::WireDirEntry],
    theme: &crate::theme::Theme,
) -> Vec<crate::console::dir_listing::DirEntry> {
    use crate::console::dir_listing::{DirEntry, EntryKind};
    entries
        .iter()
        .map(|e| DirEntry {
            name: e.name.clone(),
            kind: match e.kind.as_str() {
                "directory" => EntryKind::Directory,
                "executable" => EntryKind::Executable,
                "symlink" => EntryKind::Symlink,
                _ => EntryKind::File,
            },
            color: e
                .color
                .map(|c| ColorU::new(c[0], c[1], c[2], c[3]))
                .unwrap_or_else(|| theme.main_text()),
            size: e.size,
            modified: None,
        })
        .collect()
}

fn wire_color(color: Option<ColorU>) -> Option<[u8; 4]> {
    color.map(|c| [c.r, c.g, c.b, c.a])
}

fn from_wire_color(color: Option<[u8; 4]>) -> Option<ColorU> {
    color.map(|c| ColorU::new(c[0], c[1], c[2], c[3]))
}

/// Inverse of [`wire_lines`]: rebuild the desktop `ColoredLine` list from the
/// wire form, for a viewer rendering the HOST's snapshot.
///
/// This is what makes a mirror show the host's exact view. The alternative —
/// re-running the VT parser over the raw byte stream — only matches while the
/// viewer's column count and shell-hook stream stay perfectly in step with the
/// host's; any drift wraps lines at the wrong column or loses a block boundary,
/// and neither can be undone once parsed. Colors were already resolved against
/// the host's theme, so they are copied verbatim.
pub fn colored_lines(lines: &[WireLine]) -> Vec<ColoredLine> {
    lines
        .iter()
        .map(|line| ColoredLine {
            wrapped: false,
            spans: line
                .spans
                .iter()
                .map(|span| crate::console::escape_parse::TextSpan {
                    text: span.text.clone(),
                    color: from_wire_color(span.fg),
                    bg: from_wire_color(span.bg),
                    flags: span.flags,
                })
                .collect(),
        })
        .collect()
}

/// Convert the desktop `ColoredLine` list to the protocol wire form. Colors are
/// already resolved against the host theme, so viewers paint them verbatim.
pub fn wire_lines(lines: &[ColoredLine]) -> Vec<WireLine> {
    lines
        .iter()
        .map(|line| WireLine {
            spans: line
                .spans
                .iter()
                .map(|span| WireSpan {
                    text: span.text.clone(),
                    fg: wire_color(span.color),
                    bg: wire_color(span.bg),
                    // SGR attributes (bold/italic/underline/…), same bit layout as
                    // dragent_terminal::Flags SGR_MASK, INVERSE already in fg/bg.
                    flags: span.flags,
                })
                .collect(),
        })
        .collect()
}
