//!  command input — syntax highlighting, ghost text, running state.

use std::ops::Range;
use std::path::Path;

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Text, Wrap,
};
use warpui::fonts::FamilyId;
use warpui_core::fonts::Properties;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::{StyleAndFont, TextStyle};

use crate::agent::DetectedInputType;
use crate::console::cmd_suggest::{is_known_path_command, specs};
use crate::console::escape_parse::ColoredLine;
use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD, ICON_SIZE_SM};

use crate::theme::{Theme, INPUT_BOX_MARGIN, INPUT_FONT_SIZE, INPUT_MIN_HEIGHT};

pub fn interrupt_hint_for_command(command: Option<&str>) -> String {
    let command = command.unwrap_or("").trim();
    if command.is_empty() {
        return "Ctrl+C to interrupt".to_string();
    }
    // Get the first word (executable name)
    let exe = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    // Strip path prefix if any, e.g. /usr/local/bin/python3 -> python3
    let exe_path = std::path::Path::new(&exe);
    let exe_name = exe_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(&exe);

    match exe_name {
        "python" | "python3" | "ipython" => "Ctrl+D or exit() to exit".to_string(),
        "node" => "Ctrl+D or .exit to exit".to_string(),
        "irb" => "Ctrl+D or exit to exit".to_string(),
        "ssh" => "Enter then ~. to disconnect".to_string(),
        "psql" | "mysql" | "sqlite3" | "sqlite" | "pgcli" => {
            "Ctrl+D or \\q / quit / exit to exit".to_string()
        }
        "docker" if command.contains(" exec ") || command.contains(" run ") => {
            "Ctrl+P Ctrl+Q to detach or Ctrl+D to exit".to_string()
        }
        _ => "Ctrl+C to interrupt".to_string(),
    }
}

/// Renders the "command running" state of the input bar — a yellow indicator
/// and a hint that input goes to the process.  The idle state is rendered
/// directly in ConsolePane via `ChildView::new(&command_editor)`.
pub fn render_input_bar(
    theme: &Theme,
    mono_font: FamilyId,
    current_command: Option<&str>,
) -> Box<dyn Element> {
    let theme = theme;
    let mono = mono_font;

    let hint_text = format!(
        "Running — input goes to the process · {}",
        interrupt_hint_for_command(current_command)
    );

    let hint_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
            )
            .with_background_color(theme.ui_yellow())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .with_margin_right(10.0)
            .finish(),
        )
        .with_child(Box::new(
            Text::new(hint_text, mono, crate::theme::text_size(INPUT_FONT_SIZE))
                .with_color(theme.sub_text()),
        ));

    let input_box = Container::new(
        ConstrainedBox::new(hint_row.finish())
            .with_min_height(INPUT_MIN_HEIGHT)
            .finish(),
    )
    .with_uniform_padding(12.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::top(1.0).with_border_color(theme.outline()));

    Container::new(input_box.finish())
        .with_margin_left(0.0)
        .with_margin_right(0.0)
        .with_margin_bottom(INPUT_BOX_MARGIN)
        .finish()
}

/// Abbreviate a directory path for display: replace HOME prefix with `~`,
/// then show at most the last two path components so it stays compact.
pub fn abbrev_path(path: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let path_str = path.to_string_lossy();
    let shortened = if !home.is_empty() && path_str.starts_with(&home) {
        format!("~{}", &path_str[home.len()..])
    } else {
        path_str.into_owned()
    };

    // Keep last two components so it fits in a chip (e.g. "dev/my-project").
    let parts: Vec<&str> = shortened.trim_matches('/').split('/').collect();
    if parts.len() <= 2 {
        shortened
    } else {
        // Always include the leading ~ when the path starts with it.
        if shortened.starts_with('~') {
            let tail = parts[parts.len() - 2..].join("/");
            format!("~/../{}", tail)
        } else {
            format!("…/{}", parts[parts.len() - 2..].join("/"))
        }
    }
}

// ── Shared chip metrics ──────────────────────────────────────────────────
// Chips are built in two places: statically here, and interactively in
// TerminalView (which needs hover state for tooltips). Both read these, so
// the two paths can't drift into slightly different pills.
/// Chip label size.
pub const CHIP_FONT: f32 = 12.0;
/// Horizontal chip padding.
pub const CHIP_PAD_X: f32 = 9.0;
/// Vertical chip padding.
pub const CHIP_PAD_Y: f32 = 4.0;
/// Chip corner rounding.
pub const CHIP_RADIUS: f32 = 5.0;

/// Helper — build a small chip element.
pub fn make_chip(inner: Box<dyn Element>, theme: &Theme) -> Box<dyn Element> {
    Container::new(inner)
        .with_padding_left(CHIP_PAD_X)
        .with_padding_right(CHIP_PAD_X)
        .with_padding_top(CHIP_PAD_Y)
        .with_padding_bottom(CHIP_PAD_Y)
        .with_background_color(theme.surface_2())
        // Full-strength outline: chips are controls, and a hairline at 3/4
        // alpha read as decoration rather than something you could click.
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CHIP_RADIUS)))
        .finish()
}

/// Build the git status chip row shown above the cursor when git info is available.
pub fn render_git_status_chips(
    git: Option<&crate::console::git_status::GitStatus>,
    theme: &Theme,
    mono: FamilyId,
    cwd: Option<&Path>,
) -> Box<dyn Element> {
    render_git_status_chips_inner(
        git,
        theme,
        mono,
        cwd,
        true,
        None,
        false,
        false,
        InteractiveChips::default(),
    )
}

/// Variant where the `+N / -N` chip is supplied by the caller as a pre-built
/// element (so it can carry a click handler that dispatches a typed action).
/// Pass `None` to omit the diff chip entirely.
pub fn render_git_status_chips_with_diff(
    git: Option<&crate::console::git_status::GitStatus>,
    theme: &Theme,
    mono: FamilyId,
    cwd: Option<&Path>,
    diff_chip: Option<Box<dyn Element>>,
    pushing: bool,
    pulling: bool,
    interactive: InteractiveChips,
) -> Box<dyn Element> {
    render_git_status_chips_inner(
        git,
        theme,
        mono,
        cwd,
        false,
        diff_chip,
        pushing,
        pulling,
        interactive,
    )
}

/// Caller-supplied replacements for the folder and branch chips.
///
/// Both need view state a pure render function can't see (which popover is
/// open, the cached branch list), so the owning view builds them and passes
/// them down. `None` falls back to the static, non-clickable chip.
#[derive(Default)]
pub struct InteractiveChips {
    pub folder: Option<Box<dyn Element>>,
    pub branch: Option<Box<dyn Element>>,
    pub changes: Option<Box<dyn Element>>,
    pub ahead: Option<Box<dyn Element>>,
    pub behind: Option<Box<dyn Element>>,
}

fn render_git_status_chips_inner(
    git: Option<&crate::console::git_status::GitStatus>,
    theme: &Theme,
    mono: FamilyId,
    cwd: Option<&Path>,
    show_diff_chip: bool,
    extra_diff_chip: Option<Box<dyn Element>>,
    pushing: bool,
    pulling: bool,
    mut interactive: InteractiveChips,
) -> Box<dyn Element> {
    let mut row = Wrap::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_alignment(MainAxisAlignment::Start)
        .with_spacing(6.0)
        .with_run_spacing(6.0);

    // ── CWD chip ─────────────────────────────────────────────────────────
    if let Some(chip) = interactive.folder.take() {
        row = row.with_child(chip);
    } else if let Some(path) = cwd {
        let label = abbrev_path(path);
        let cwd_inner = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(render_icon(AppIcon::Folder, theme.sub_text(), ICON_SIZE_MD))
            .with_child(Box::new(
                Text::new(label, mono, crate::theme::text_size(CHIP_FONT))
                    .with_color(theme.sub_text()),
            ))
            .finish();
        row = row.with_child(make_chip(cwd_inner, theme));
    }

    // ── Branch chip — GitHub-style git-branch icon ────────────────────────
    if let Some(git) = git {
        if let Some(chip) = interactive.branch.take() {
            row = row.with_child(chip);
        } else {
            let branch_inner = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(render_icon(AppIcon::GitBranch, theme.accent, ICON_SIZE_MD))
                .with_child(Box::new(
                    Text::new(git.branch.clone(), mono, crate::theme::text_size(CHIP_FONT))
                        .with_color(theme.accent),
                ))
                .finish();
            row = row.with_child(make_chip(branch_inner, theme));
        }

        // ── File-change count chip (●N) ───────────────────────────────────────
        if let Some(chip) = interactive.changes.take() {
            row = row.with_child(chip);
        } else if git.has_changes() {
            // All-green: every changed file is fully staged, nothing unstaged or untracked.
            let changes_color = if git.staged > 0 && git.modified == 0 && git.untracked == 0 {
                theme.ansi_green()
            } else {
                theme.ui_yellow()
            };
            let changes_chip = make_chip(
                Box::new(
                    Text::new(
                        format!("\u{25CF} {}", git.total_changes()),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(changes_color),
                ),
                theme,
            );
            row = row.with_child(changes_chip);
        }

        // ── +N / -N line diff stats ───────────────────────────────────────────
        if show_diff_chip && (git.added > 0 || git.deleted > 0) {
            let diff_inner = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(Box::new(
                    Text::new(
                        format!("+{}", git.added),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(theme.ansi_green()),
                ))
                .with_child(Box::new(
                    Text::new(
                        format!("-{}", git.deleted),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(theme.ansi_red()),
                ))
                .finish();
            row = row.with_child(make_chip(diff_inner, theme));
        }

        // ── Ahead (push) chip ─────────────────────────────────────────────────
        if let Some(chip) = interactive.ahead.take() {
            row = row.with_child(chip);
        } else if pushing {
            row = row.with_child(make_chip(
                Box::new(
                    Text::new(
                        "\u{2191} pushing…".to_string(),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(theme.ansi_blue()),
                ),
                theme,
            ));
        } else if git.ahead > 0 {
            // One click pushes the current branch to origin.
            let chip = make_chip(
                Box::new(
                    Text::new(
                        format!("\u{2191}{}", git.ahead),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(theme.ansi_blue()),
                ),
                theme,
            );
            row = row.with_child(
                warpui::elements::EventHandler::new(chip)
                    .on_left_mouse_down(|event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(
                            crate::terminal::view::actions::TerminalAction::PushBranchChanges,
                        );
                        warpui::elements::DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
        }

        // ── Behind (pull) chip ────────────────────────────────────────────────
        if let Some(chip) = interactive.behind.take() {
            row = row.with_child(chip);
        } else if pulling {
            row = row.with_child(make_chip(
                Box::new(
                    Text::new(
                        "\u{2193} pulling…".to_string(),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(theme.ansi_blue()),
                ),
                theme,
            ));
        } else if git.behind > 0 {
            // One click pulls the remote changes for this branch.
            let chip = make_chip(
                Box::new(
                    Text::new(
                        format!("\u{2193}{}", git.behind),
                        mono,
                        crate::theme::text_size(CHIP_FONT),
                    )
                    .with_color(theme.ansi_blue()),
                ),
                theme,
            );
            row = row.with_child(
                warpui::elements::EventHandler::new(chip)
                    .on_left_mouse_down(|event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(
                            crate::terminal::view::actions::TerminalAction::PullBranchChanges,
                        );
                        warpui::elements::DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
        }
    }

    // Caller-supplied diff chip (e.g. with a click handler) added last.
    if let Some(chip) = extra_diff_chip {
        row = row.with_child(chip);
    }

    Container::new(row.finish()).finish()
}

/// Build the history search active hint row.
pub fn render_history_search_hint(theme: &Theme, mono: FamilyId) -> Box<dyn Element> {
    Container::new(Box::new(
        Text::new(
            "Ctrl+R \u{2014} history search active \u{00B7} ESC to close".to_string(),
            mono,
            crate::theme::text_size(11.0),
        )
        .with_color(theme.disabled_text()),
    ))
    .with_margin_top(4.0)
    .finish()
}

pub fn render_workflow_search_hint(theme: &Theme, mono: FamilyId) -> Box<dyn Element> {
    Container::new(Box::new(
        Text::new(
            "Ctrl/Cmd+Shift+P \u{2014} workflow search \u{00B7} Esc to close".to_string(),
            mono,
            crate::theme::text_size(11.0),
        )
        .with_color(theme.disabled_text()),
    ))
    .with_margin_top(4.0)
    .finish()
}

pub fn render_workflow_session_hint(
    name: &str,
    arg_name: &str,
    theme: &Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    Container::new(Box::new(
        Text::new(
            format!(
                "Workflow: {name} \u{00B7} Shift+Tab cycles args \u{00B7} editing `{arg_name}`"
            ),
            mono,
            crate::theme::text_size(11.0),
        )
        .with_color(theme.sub_text()),
    ))
    .with_margin_top(4.0)
    .finish()
}

pub fn render_vim_mode_badge(
    mode: &str,
    show_cmd: &str,
    theme: &Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let label = if show_cmd.is_empty() {
        format!("VIM {mode}")
    } else {
        format!("VIM {mode} {show_cmd}")
    };
    Container::new(
        Container::new(Box::new(
            Text::new(label, mono, crate::theme::text_size(CHIP_FONT)).with_color(theme.background),
        ))
        .with_background_color(theme.accent)
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(4.0),
        ))
        .finish(),
    )
    .with_padding_left(8.0)
    .finish()
}

/// Small chip showing current detected input type + lock state.
/// Clicking it dispatches `ToggleInputTypeLock` (wired in terminal_view).
pub fn render_input_mode_indicator(
    input_type: DetectedInputType,
    is_locked: bool,
    theme: &Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let (label, color) = match (input_type, is_locked) {
        (DetectedInputType::Shell, false) => ("shell", theme.disabled_text()),
        (DetectedInputType::Shell, true) => ("\u{1F512} shell", theme.sub_text()),
        (DetectedInputType::Agent, _) => ("agent", theme.accent),
    };
    Container::new(
        Container::new(Box::new(
            Text::new(label.to_string(), mono, crate::theme::text_size(10.0)).with_color(color),
        ))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .with_background_color(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish(),
    )
    .with_padding_left(6.0)
    .finish()
}

/// Render styled shell prompt lines beside the input (honor PS1).
pub fn render_shell_prompt_prefix(
    lines: &[ColoredLine],
    theme: &Theme,
    mono: FamilyId,
    font_size: f32,
) -> Option<Box<dyn Element>> {
    if lines.is_empty() {
        return None;
    }
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min);
    let props = Properties::default();
    for line in lines {
        let mut text_el = Text::new("", mono, font_size);
        if line.spans.is_empty() {
            continue;
        }
        for span in &line.spans {
            let color = span.color.unwrap_or(theme.main_text());
            text_el.add_text_with_highlights(&span.text, color, props);
        }
        col = col.with_child(Box::new(text_el));
    }
    Some(
        Container::new(col.finish())
            .with_padding_right(8.0)
            .finish(),
    )
}

/// Small hint row shown under the editor when the cursor sits on a known token.
pub fn render_token_hint_row(hint: &str, theme: &Theme, mono: FamilyId) -> Box<dyn Element> {
    render_hint_row(hint, theme.sub_text(), mono)
}

/// A status line that must stand out, e.g. a shell that is still starting.
pub fn render_accent_hint_row(hint: &str, theme: &Theme, mono: FamilyId) -> Box<dyn Element> {
    render_hint_row(hint, theme.accent, mono)
}

fn render_hint_row(hint: &str, color: ColorU, mono: FamilyId) -> Box<dyn Element> {
    Container::new(Box::new(
        Text::new(hint.to_string(), mono, crate::theme::text_size(11.0)).with_color(color),
    ))
    .with_padding_top(4.0)
    .finish()
}

///  token colors: command (green), flags (dim), args (link), pipes (accent).
pub fn word_color(word: &str, is_first: bool, theme: &Theme) -> ColorU {
    if is_first {
        theme.ansi_green()
    } else if word.starts_with('-') {
        theme.disabled_text()
    } else {
        theme.link()
    }
}

pub fn operator_color(theme: &Theme) -> ColorU {
    theme.accent
}

/// Byte-indexed highlight spans for a single command line (shared by render + layout).
///
/// Uses the lexer so operators inside quoted strings don't affect coloring, and
/// so that each pipeline stage (`|`, `&&`, `||`, `;`) resets the command-name
/// color to green for the next word.
pub fn command_spans(command: &str, theme: &Theme) -> Vec<(Range<usize>, ColorU)> {
    use dragent_parser::{lex, Token};

    let tokens = lex(command);
    if tokens.is_empty() {
        return if command.is_empty() {
            vec![]
        } else {
            vec![(0..command.len(), theme.main_text())]
        };
    }

    let mut spans = Vec::with_capacity(tokens.len());
    // 0 = still looking for the command name, 1+ = past it.
    let mut cmd_position: usize = 0;

    for tok in &tokens {
        let range = tok.span.start..tok.span.end;
        let text = tok.span.slice(command);

        let color = match &tok.item {
            Token::Bare => {
                if cmd_position == 0 {
                    if is_env_var_token(text) {
                        // KEY=VALUE prefix — don't count as the command name.
                        theme.disabled_text()
                    } else {
                        cmd_position += 1;
                        theme.ansi_green()
                    }
                } else {
                    word_color(text, false, theme)
                }
            }
            Token::SingleQuoted | Token::DoubleQuoted | Token::BacktickQuoted => theme.link(),
            Token::Pipe
            | Token::LogicalAnd
            | Token::LogicalOr
            | Token::Semicolon
            | Token::Ampersand => {
                // Next word is the command name of the next stage.
                cmd_position = 0;
                operator_color(theme)
            }
            Token::RedirectOut
            | Token::RedirectAppend
            | Token::RedirectIn
            | Token::RedirectErr
            | Token::RedirectErrAppend
            | Token::RedirectBoth => operator_color(theme),
            Token::Comment => theme.disabled_text(),
            Token::CommandSubstOpen | Token::OpenParen | Token::CloseParen => theme.sub_text(),
            Token::Whitespace | Token::Newline => theme.main_text(),
        };

        spans.push((range, color));
    }

    spans
}

/// True when `s` looks like a shell env-var assignment (`KEY=value`).
fn is_env_var_token(s: &str) -> bool {
    match s.find('=') {
        None | Some(0) => false,
        Some(eq) => {
            let key = &s[..eq];
            key.chars()
                .next()
                .map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
    }
}

pub fn style_runs_for_command(
    command: &str,
    font: FamilyId,
    theme: &Theme,
) -> Vec<(Range<usize>, StyleAndFont)> {
    command_spans(command, theme)
        .into_iter()
        .map(|(range, _color)| {
            (
                range,
                StyleAndFont::new(font, Properties::default(), TextStyle::default()),
            )
        })
        .collect()
}

pub fn is_likely_command(text: &str) -> bool {
    use dragent_parser::{classify_command, lite_parse, EmptyRegistry};

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Use the parser to find the real command name, skipping leading KEY=VALUE
    // env-var assignments (e.g. "FOO=bar git push" → first_word = "git").
    let (group, _) = lite_parse(trimmed);
    let first_word = group
        .pipelines
        .first()
        .and_then(|p| p.commands.first())
        .map(|cmd| classify_command(cmd.clone(), &EmptyRegistry).name.item)
        .unwrap_or_default();

    if first_word.is_empty() {
        return false;
    }
    let first_word = first_word.as_str();

    if first_word.starts_with("./")
        || first_word.starts_with("../")
        || first_word.starts_with('/')
        || first_word.starts_with("~/")
    {
        return true;
    }

    if first_word.ends_with(".sh")
        || first_word.ends_with(".py")
        || first_word.ends_with(".js")
        || first_word.ends_with(".pl")
        || first_word.ends_with(".rb")
    {
        return true;
    }

    let builtins = [
        "cd", "echo", "exit", "history", "clear", "source", "export", "unset", "alias", "unalias",
        "type", "which", "jobs", "fg", "bg", "kill", "sh", "bash", "zsh", "sudo", "nohup",
    ];
    if builtins.contains(&first_word) {
        return true;
    }

    if specs().any(|s| s.name == first_word) {
        return true;
    }
    is_known_path_command(first_word)
}

pub fn build_highlighted_command(
    command: &str,
    theme: &Theme,
    mono_family: FamilyId,
    font_size: f32,
    is_cmd: bool,
) -> Text {
    let mut text_el = Text::new("", mono_family, font_size);
    let props = Properties::default();

    if matches!(command.trim(), "/agent" | "/shell") {
        text_el.add_text_with_highlights(command, theme.accent, props);
    } else if is_cmd {
        for (range, color) in command_spans(command, theme) {
            text_el.add_text_with_highlights(&command[range], color, props);
        }
    } else {
        text_el.add_text_with_highlights(command, theme.main_text(), props);
    }

    text_el
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interrupt_hints() {
        assert_eq!(
            interrupt_hint_for_command(Some("python3")),
            "Ctrl+D or exit() to exit"
        );
        assert_eq!(
            interrupt_hint_for_command(Some("python main.py")),
            "Ctrl+D or exit() to exit"
        );
        assert_eq!(
            interrupt_hint_for_command(Some("node")),
            "Ctrl+D or .exit to exit"
        );
        assert_eq!(
            interrupt_hint_for_command(Some("ssh user@host")),
            "Enter then ~. to disconnect"
        );
        assert_eq!(
            interrupt_hint_for_command(Some("psql")),
            "Ctrl+D or \\q / quit / exit to exit"
        );
        assert_eq!(
            interrupt_hint_for_command(Some("docker run -it alpine sh")),
            "Ctrl+P Ctrl+Q to detach or Ctrl+D to exit"
        );
        assert_eq!(
            interrupt_hint_for_command(Some("ls")),
            "Ctrl+C to interrupt"
        );
        assert_eq!(interrupt_hint_for_command(None), "Ctrl+C to interrupt");
    }
}
