use std::borrow::Cow;
use std::path::{Path, PathBuf};

use warpui::color::ColorU;

use crate::console::escape_parse::ColoredLine;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirSort {
    #[default]
    Name,
    Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirView {
    #[default]
    Grid,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    Executable,
    File,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub color: ColorU,
    pub size: Option<u64>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DirListing {
    pub entries: Vec<DirEntry>,
}

/// Returns true when the command is a plain directory listing (not `ls -l`, etc.).
pub fn is_ls_command(command: &str) -> bool {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.first() != Some(&"ls") {
        return false;
    }
    !parts.iter().any(|p| {
        if let Some(long) = p.strip_prefix("--") {
            // Long options: only specific ones change the output away from the
            // plain grid. Scanning their letters like short flags would false-
            // positive on the flags shells alias in — `--color=auto` contains
            // an `l` and `--show-control-chars` an `h`, `l`, and `d` — which
            // disabled the widget for every alias-expanded `ls` on Git Bash.
            let name = long.split('=').next().unwrap_or(long);
            matches!(name, "directory" | "dereference" | "human-readable")
                || (name == "format" && (long.contains("long") || long.contains("verbose")))
        } else if let Some(short) = p.strip_prefix('-') {
            short.chars().any(|c| matches!(c, 'l' | 'L' | 'h' | 'd'))
        } else {
            false
        }
    })
}

/// Flags Git Bash's default `alias ls='ls -F --color=auto --show-control-chars'`
/// injects. When a block's command text comes straight from the shell's
/// `preexec` hook (i.e. the user typed at the prompt rather than through the
/// app's own input bar), it's this expanded form rather than what was typed.
const LS_ALIAS_INJECTED_FLAGS: &[&str] = &["-F", "--color=auto", "--show-control-chars"];

/// Strips known shell-alias-injected `ls` flags so displayed/copied command
/// text matches what the user actually typed (e.g. `ls`, or `ls -a` for
/// `ls -F --color=auto --show-control-chars -a`) instead of the alias
/// expansion the shell reports.
pub fn strip_ls_alias_flags(command: &str) -> Cow<'_, str> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.first() != Some(&"ls") || !parts.iter().any(|p| LS_ALIAS_INJECTED_FLAGS.contains(p)) {
        return Cow::Borrowed(command);
    }
    Cow::Owned(
        parts
            .into_iter()
            .filter(|p| !LS_ALIAS_INJECTED_FLAGS.contains(p))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn try_parse_listing(
    command: &str,
    lines: &[ColoredLine],
    cwd: &Path,
    theme: &Theme,
) -> Option<DirListing> {
    let first = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // `ls`, `dir`, `gci`, `Get-ChildItem` all list a directory. On Windows the
    // first three are PowerShell aliases for Get-ChildItem.
    if !matches!(first.as_str(), "ls" | "dir" | "gci" | "get-childitem") {
        return None;
    }

    // Ground truth first: a plain listing of the block's own cwd is built from
    // the filesystem, not by scraping the command's output. Scraping breaks in
    // ways users see immediately — Git Bash's `ls` ignores the Windows hidden
    // attribute (NTUSER.DAT, Cookies, SendTo… all dumped into the grid), `-F`
    // decorations and shell quoting leak into names ('Saved Games'/, Cookies@),
    // and long names wrap across terminal lines into garbage fragments.
    //
    // Tried BEFORE the `lines.is_empty()` check below on purpose: this path
    // needs only `command` (for flags) and `cwd`, never the captured output, so
    // it's already answerable the instant a block starts — before the shell has
    // streamed back a single byte. Gating it behind non-empty `lines` meant a
    // LIVE `ls` block flashed raw, unstyled multi-column TTY text (the exact
    // mess described above) for as long as the command took to finish, then
    // snapped to the clean grid on completion. `fs_listing_flags` still rejects
    // any command with a path argument (`ls foo/`), so those correctly fall
    // through to the text-scraping paths below unchanged.
    if let Some(listing) = listing_from_fs(command, cwd, theme) {
        return Some(listing);
    }

    if lines.is_empty() {
        return None;
    }

    // PowerShell / pwsh print a "Mode / LastWriteTime / Length / Name" table. Try
    // that first — Unix `ls` output has no such header, so this is a no-op there.
    if let Some(listing) = try_parse_powershell_listing(lines, theme) {
        return Some(listing);
    }

    // Unix `ls` (plain grid; not `ls -l`).
    if !is_ls_command(command) || looks_like_long_listing(lines) {
        return None;
    }
    let mut entries = Vec::new();
    for line in lines {
        let text = line_plain_text(line);
        let trimmed = text.trim();
        if trimmed.is_empty() || is_noise_line(trimmed) {
            continue;
        }

        let row = parse_row_cells(trimmed, line, cwd, theme);
        for (_, entry) in row {
            entries.push(entry);
        }
    }

    if entries.is_empty() {
        return None;
    }

    Some(DirListing { entries })
}

/// Flags after which a cwd listing can still be answered from the filesystem.
/// Anything outside this set (paths, `-l`, `-t`, `-Recurse`, …) changes what
/// the command actually lists, so the caller must fall back to output parsing.
fn fs_listing_flags(command: &str) -> Option<bool> {
    let mut show_hidden = false;
    for part in command.split_whitespace().skip(1) {
        if let Some(long) = part.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            match name {
                "all" | "almost-all" => show_hidden = true,
                "color" | "show-control-chars" | "classify" => {}
                _ => return None,
            }
        } else if let Some(flags) = part.strip_prefix('-') {
            // PowerShell's -Force (any case) shows hidden files.
            if flags.eq_ignore_ascii_case("force") {
                show_hidden = true;
                continue;
            }
            for c in flags.chars() {
                match c {
                    'a' | 'A' => show_hidden = true,
                    'F' | 'C' | 'G' | '1' | 'x' => {}
                    _ => return None,
                }
            }
        } else {
            // A path argument: entries would live outside the block cwd and
            // click-resolution (cwd.join(name)) would point at the wrong files.
            return None;
        }
    }
    Some(show_hidden)
}

/// Build the listing straight from the block's cwd via `read_dir`.
/// Dotfiles and Windows hidden/system-attribute files stay hidden unless the
/// command asked for them (`-a`/`-A`/`--all`/`-Force`) — matching what every
/// native file browser shows, rather than what MSYS `ls` happens to print.
fn listing_from_fs(command: &str, cwd: &Path, theme: &Theme) -> Option<DirListing> {
    let show_hidden = fs_listing_flags(command)?;
    let read = std::fs::read_dir(cwd).ok()?;

    let mut entries = Vec::new();
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && is_hidden_entry(&item, &name) {
            continue;
        }
        let path = item.path();
        let kind = {
            let k = classify_entry(&path, &name);
            if k == EntryKind::File && is_windows_executable(&name) {
                EntryKind::Executable
            } else {
                k
            }
        };
        let color = match kind {
            EntryKind::Directory => theme.ansi_blue(),
            EntryKind::Executable => theme.ansi_green(),
            EntryKind::Symlink => theme.accent,
            EntryKind::File => theme.main_text(),
        };
        let mut size = None;
        let mut modified = None;
        if let Ok(meta) = item.metadata() {
            if meta.is_file() {
                size = Some(meta.len());
            }
            if let Ok(mod_time) = meta.modified() {
                let datetime: chrono::DateTime<chrono::Local> = mod_time.into();
                modified = Some(datetime.format("%Y-%m-%d").to_string());
            }
        }
        entries.push(DirEntry {
            name,
            kind,
            color,
            size,
            modified,
        });
    }

    // Directories first, then case-insensitive by name — the paint layer sorts
    // the same way, so selection/copy lines match what is displayed.
    entries.sort_by(|a, b| {
        let a_dir = a.kind == EntryKind::Directory;
        let b_dir = b.kind == EntryKind::Directory;
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Some(DirListing { entries })
}

/// Hidden by dotfile convention, or by the hidden/system attribute on Windows
/// (which MSYS `ls` ignores — the NTUSER.DAT family in every profile dir).
fn is_hidden_entry(item: &std::fs::DirEntry, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        if let Ok(meta) = item.metadata() {
            let attrs = meta.file_attributes();
            if attrs & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                return true;
            }
        }
    }
    let _ = item;
    false
}

/// Parse PowerShell `Get-ChildItem` table output:
/// ```text
///     Directory: C:\Users\x\dev
///
/// Mode                 LastWriteTime         Length Name
/// ----                 -------------         ------ ----
/// d----           7/3/2026  8:31 AM                dragent
/// -a---           7/3/2026  8:31 AM           1234 Cargo.toml
/// ```
/// Column positions come from the dash separator, which is robust against spaces
/// in file names and locale-dependent date/time formats.
fn try_parse_powershell_listing(lines: &[ColoredLine], theme: &Theme) -> Option<DirListing> {
    let texts: Vec<String> = lines.iter().map(line_plain_text).collect();
    let header_idx = texts.iter().position(|t| {
        let low = t.to_ascii_lowercase();
        low.contains("mode") && low.contains("lastwritetime") && low.contains("name")
    })?;
    let sep = texts.get(header_idx + 1)?;
    if !sep.trim_start().starts_with("----") {
        return None;
    }
    let cols = dash_column_starts(sep);
    if cols.len() < 2 {
        return None;
    }
    let mode_end = cols[1];
    let len_start = cols[cols.len() - 2];
    let name_start = cols[cols.len() - 1];

    let mut entries = Vec::new();
    for text in texts.iter().skip(header_idx + 2) {
        if text.trim().is_empty() {
            continue;
        }
        let chars: Vec<char> = text.chars().collect();
        let slice = |a: usize, b: usize| -> String {
            let b = b.min(chars.len());
            if a >= b {
                return String::new();
            }
            chars[a..b].iter().collect::<String>().trim().to_string()
        };
        let mode = slice(0, mode_end);
        // A real data row starts with a mode field like "d----" / "-a---"; this
        // skips the "Directory:" header and any stray lines.
        if mode.is_empty()
            || !mode
                .chars()
                .all(|c| matches!(c, 'd' | 'a' | 'r' | 'h' | 's' | 'l' | '-'))
        {
            continue;
        }
        let name = chars
            .get(name_start..)
            .map(|s| s.iter().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let kind = if mode.starts_with('d') {
            EntryKind::Directory
        } else if mode.starts_with('l') {
            EntryKind::Symlink
        } else if is_windows_executable(&name) {
            EntryKind::Executable
        } else {
            EntryKind::File
        };
        let color = match kind {
            EntryKind::Directory => theme.ansi_blue(),
            EntryKind::Executable => theme.ansi_green(),
            _ => theme.main_text(),
        };
        let size = slice(len_start, name_start)
            .replace(',', "")
            .parse::<u64>()
            .ok();
        entries.push(DirEntry {
            name,
            kind,
            color,
            size,
            modified: None,
        });
    }

    if entries.is_empty() {
        None
    } else {
        Some(DirListing { entries })
    }
}

fn is_windows_executable(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    [".exe", ".bat", ".cmd", ".com", ".ps1"]
        .iter()
        .any(|ext| low.ends_with(ext))
}

/// Char index where each run of `-` begins in a table separator line.
fn dash_column_starts(sep: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut in_dashes = false;
    for (i, c) in sep.chars().enumerate() {
        if c == '-' {
            if !in_dashes {
                starts.push(i);
                in_dashes = true;
            }
        } else {
            in_dashes = false;
        }
    }
    starts
}

pub fn resolve_entry_path(cwd: &Path, name: &str) -> PathBuf {
    let clean = name.trim_end_matches('/');
    if clean.starts_with('/') {
        PathBuf::from(clean)
    } else {
        cwd.join(clean)
    }
}

pub fn modifier_click_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Cmd+Click"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl+Click"
    }
}

pub fn entry_click_hint_label(is_dir: bool, name: &str) -> String {
    let action = if is_dir { "enter" } else { "open" };
    format!("Press {} to {action} {name}", modifier_click_label())
}

pub fn link_click_hint_label() -> String {
    format!("Press {} to open in browser", modifier_click_label())
}

fn line_plain_text(line: &ColoredLine) -> String {
    line.spans.iter().map(|s| s.text.as_str()).collect()
}

fn looks_like_long_listing(lines: &[ColoredLine]) -> bool {
    lines.iter().any(|line| {
        let text = line_plain_text(line);
        text.starts_with("total ")
            || (text.len() >= 10
                && matches!(text.as_bytes().first(), Some(b'-' | b'd' | b'l'))
                && text.as_bytes().get(9) == Some(&b' '))
    })
}

fn is_noise_line(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("no such file")
        || lower.contains("permission denied")
        || lower.contains("cannot access")
        // `bash: s: command not found` — an error line must never be split
        // into fake clickable "entries" (bash:, command, found, ...).
        || lower.contains("command not found")
        // PowerShell's equivalent: "... is not recognized as the name of a
        // cmdlet, function, script file, or operable program."
        || lower.contains("is not recognized")
}

fn parse_row_cells(
    line: &str,
    colored: &ColoredLine,
    cwd: &Path,
    theme: &Theme,
) -> Vec<(usize, DirEntry)> {
    if line.contains('\t') {
        return line
            .split('\t')
            .enumerate()
            .filter_map(|(col, cell)| {
                let trimmed = cell.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some((col, entry_from_name(trimmed, colored, cwd, theme)))
                }
            })
            .collect();
    }

    split_space_columns(line)
        .into_iter()
        .enumerate()
        .map(|(col, name)| (col, entry_from_name(&name, colored, cwd, theme)))
        .collect()
}

fn split_space_columns(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut space_run = 0usize;

    for ch in line.chars() {
        if ch == ' ' {
            space_run += 1;
            if space_run >= 2 {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    cells.push(trimmed.to_string());
                }
                current.clear();
                space_run = 0;
            }
        } else {
            if space_run == 1 && !current.is_empty() {
                current.push(' ');
            }
            space_run = 0;
            current.push(ch);
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        cells.push(trimmed.to_string());
    }

    if cells.len() <= 1 && line.contains(' ') {
        return line.split_whitespace().map(str::to_string).collect();
    }

    cells
}
fn entry_from_name(name: &str, line: &ColoredLine, cwd: &Path, theme: &Theme) -> DirEntry {
    let clean = name.trim_end_matches('/');
    let path = cwd.join(clean);
    let kind = classify_entry(&path, name);
    let color = color_for_name(clean, line, kind, theme);

    let mut size = None;
    let mut modified = None;

    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.is_file() {
            size = Some(meta.len());
        }
        if let Ok(mod_time) = meta.modified() {
            let datetime: chrono::DateTime<chrono::Local> = mod_time.into();
            modified = Some(datetime.format("%Y-%m-%d").to_string());
        }
    }

    DirEntry {
        name: clean.to_string(),
        kind,
        color,
        size,
        modified,
    }
}
fn classify_entry(path: &Path, raw_name: &str) -> EntryKind {
    if raw_name.ends_with('/') {
        return EntryKind::Directory;
    }

    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return EntryKind::File;
    };

    if meta.file_type().is_symlink() {
        return EntryKind::Symlink;
    }
    if meta.is_dir() {
        return EntryKind::Directory;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return EntryKind::Executable;
        }
    }

    EntryKind::File
}

fn color_for_name(name: &str, line: &ColoredLine, kind: EntryKind, theme: &Theme) -> ColorU {
    if let Some(ansi) = color_for_name_span(name, line) {
        if kind == EntryKind::Directory && colors_similar(ansi, theme.ansi_blue()) {
            return theme.ansi_blue();
        }
        if kind == EntryKind::Executable && colors_similar(ansi, theme.ansi_green()) {
            return theme.ansi_green();
        }
        if ansi != theme.main_text() {
            return ansi;
        }
    }

    match kind {
        EntryKind::Directory => theme.ansi_blue(),
        EntryKind::Executable => theme.ansi_green(),
        EntryKind::Symlink => theme.accent,
        EntryKind::File => theme.main_text(),
    }
}

fn color_for_name_span(name: &str, line: &ColoredLine) -> Option<ColorU> {
    let mut offset = 0usize;
    for span in &line.spans {
        let span_len = span.text.chars().count();
        let haystack = line_plain_text(line);
        if let Some(pos) = haystack[offset..].find(name) {
            let abs = offset + pos;
            let mut cursor = 0usize;
            for s in &line.spans {
                let len = s.text.chars().count();
                if abs < cursor + len {
                    return s.color;
                }
                cursor += len;
            }
        }
        offset += span_len;
    }
    None
}

fn colors_similar(a: ColorU, b: ColorU) -> bool {
    a.r.abs_diff(b.r) < 30 && a.g.abs_diff(b.g) < 30 && a.b.abs_diff(b.b) < 30
}

/// Maximum directory entries per row in grid view.
pub const DIR_GRID_MAX_COLUMNS: usize = 5;
/// Minimum columns on narrow screens (never collapse to a single column).
pub const DIR_GRID_MIN_COLUMNS: usize = 2;
/// Minimum column width before dropping to fewer columns.
/// Grid cells include a glyph, name (often long), and size/date meta — 120px
/// was too tight and produced 3 cramped columns on ~450px panes.
pub const DIR_GRID_MIN_COLUMN_WIDTH: f32 = 160.0;
pub const DIR_GRID_ROW_SPACING: f32 = 4.0;

const BLOCK_ACCENT_WIDTH: f32 = 3.0;
const BLOCK_OUTPUT_PADDING_LEFT: f32 = crate::theme::TERMINAL_PADDING_LEFT + 14.0;
/// Vertical scrollbar in the block list viewport.
const DIR_GRID_SCROLLBAR_ALLOWANCE: f32 = 12.0;

/// Horizontal chrome inside a block card before the grid (accent stripe + body padding).
pub const DIR_GRID_BLOCK_CHROME: f32 = BLOCK_ACCENT_WIDTH
    + BLOCK_OUTPUT_PADDING_LEFT
    + crate::theme::TERMINAL_PADDING_LEFT
    + DIR_GRID_SCROLLBAR_ALLOWANCE;

/// Estimate the horizontal space available for a directory grid.
pub fn dir_grid_content_width(viewport_width: f32) -> f32 {
    (viewport_width - DIR_GRID_BLOCK_CHROME)
        .max(DIR_GRID_MIN_COLUMN_WIDTH * DIR_GRID_MIN_COLUMNS as f32)
}

/// Column count scales 2 → 3 → 4 → 5 as width grows (~160px per column).
pub fn dir_grid_column_count(content_width: f32) -> usize {
    let cols = (content_width / DIR_GRID_MIN_COLUMN_WIDTH).floor() as usize;
    cols.clamp(DIR_GRID_MIN_COLUMNS, DIR_GRID_MAX_COLUMNS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_grid_columns_scale_with_width() {
        assert_eq!(dir_grid_column_count(80.0), 2);
        assert_eq!(dir_grid_column_count(200.0), 2);
        assert_eq!(dir_grid_column_count(320.0), 2);
        assert_eq!(dir_grid_column_count(400.0), 2);
        assert_eq!(dir_grid_column_count(450.0), 2);
        assert_eq!(dir_grid_column_count(479.0), 2);
        assert_eq!(dir_grid_column_count(480.0), 3);
        assert_eq!(dir_grid_column_count(640.0), 4);
        assert_eq!(dir_grid_column_count(800.0), 5);
        assert_eq!(dir_grid_column_count(900.0), 5);
    }

    #[test]
    fn detects_ls_command() {
        assert!(is_ls_command("ls"));
        assert!(is_ls_command("ls -G"));
        assert!(!is_ls_command("ls -la"));
        assert!(!is_ls_command("grep foo"));
        // Git Bash aliases `ls` to `ls -F --color=auto --show-control-chars`;
        // letters inside long options must not read as short flags (the `l` in
        // `--color`, the `h`/`l`/`d` in `--show-control-chars`).
        assert!(is_ls_command("ls -F --color=auto --show-control-chars"));
        assert!(!is_ls_command("ls --format=long"));
        assert!(!is_ls_command("ls --directory"));
        assert!(!is_ls_command("ls --human-readable -l"));
    }

    #[test]
    fn strips_ls_alias_flags() {
        assert_eq!(
            strip_ls_alias_flags("ls -F --color=auto --show-control-chars"),
            "ls"
        );
        assert_eq!(
            strip_ls_alias_flags("ls -F --color=auto --show-control-chars -a"),
            "ls -a"
        );
        // Nothing to strip: returned unchanged.
        assert_eq!(strip_ls_alias_flags("ls"), "ls");
        assert_eq!(strip_ls_alias_flags("ls -la"), "ls -la");
        assert_eq!(strip_ls_alias_flags("grep foo"), "grep foo");
    }

    #[test]
    fn splits_multi_column_row() {
        assert_eq!(
            split_space_columns("Cargo.toml  README.md  app"),
            vec!["Cargo.toml", "README.md", "app"]
        );
    }

    #[test]
    fn tab_row_preserves_column_positions() {
        let cells = parse_row_cells(
            "app\t\tcrates\tREADME.md",
            &ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: "app\t\tcrates\tREADME.md".into(),
                    color: None,
                    bg: None,
                    flags: 0,
                }],
            },
            Path::new("."),
            &Theme::dark(),
        );
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].0, 0);
        assert_eq!(cells[1].0, 2);
        assert_eq!(cells[2].0, 3);
    }

    #[test]
    fn parses_simple_listing() {
        let theme = Theme::dark();
        let lines = vec![
            ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: "app  crates  README.md".into(),
                    color: None,
                    bg: None,
                    flags: 0,
                }],
            },
            ColoredLine {
                wrapped: false,
                spans: vec![crate::console::escape_parse::TextSpan {
                    text: "Cargo.toml".into(),
                    color: None,
                    bg: None,
                    flags: 0,
                }],
            },
        ];
        // Nonexistent cwd: the fs-backed path bails and the output parser runs.
        let listing =
            try_parse_listing("ls", &lines, Path::new("/dragent-test-nonexistent"), &theme)
                .expect("listing");
        assert_eq!(listing.entries.len(), 4);
        assert_eq!(listing.entries[0].name, "app");
        assert_eq!(listing.entries[3].name, "Cargo.toml");
    }

    #[test]
    fn fs_listing_hides_hidden_and_respects_all_flag() {
        let dir = std::env::temp_dir().join("dragent_dirlist_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("visible.txt"), b"x").unwrap();
        std::fs::write(dir.join(".dotfile"), b"x").unwrap();

        let theme = Theme::dark();
        let listing = listing_from_fs("ls", &dir, &theme).expect("fs listing");
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "visible.txt"], "dotfile must be hidden");

        let listing = listing_from_fs("ls -a", &dir, &theme).expect("fs listing -a");
        assert!(listing.entries.iter().any(|e| e.name == ".dotfile"));

        // Unsupported flags or path args must fall back to output parsing.
        assert!(listing_from_fs("ls -l", &dir, &theme).is_none());
        assert!(listing_from_fs("ls sub", &dir, &theme).is_none());
        // Git Bash alias flags are fine.
        assert!(listing_from_fs("ls -F --color=auto --show-control-chars", &dir, &theme).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_block_gets_the_grid_before_any_output_has_streamed_back() {
        // A block's `lines` are empty from the moment it starts until the
        // command completes and the emulator snapshot is taken — regression
        // test for the bug where a live `ls` block flashed raw, unstyled TTY
        // text (Git Bash's multi-column layout, hidden NTUSER.DAT dumped in)
        // for the command's whole duration, only becoming the clean grid once
        // it finished. `try_parse_listing` must resolve the grid from `cwd`
        // alone, with no captured output at all.
        let dir = std::env::temp_dir().join("dragent_dirlist_live_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("visible.txt"), b"x").unwrap();

        let theme = Theme::dark();
        let listing = try_parse_listing("ls", &[], &dir, &theme)
            .expect("bare `ls` must resolve from cwd even with zero captured lines");
        assert_eq!(listing.entries[0].name, "visible.txt");

        // A path argument still can't be answered from `cwd` alone (the real
        // target may be elsewhere) — must still wait for real output.
        assert!(try_parse_listing("ls sub", &[], &dir, &theme).is_none());
        // A non-listing command must never match, live or not.
        assert!(try_parse_listing("cat foo.txt", &[], &dir, &theme).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
