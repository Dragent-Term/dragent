use dragent_terminal::{Color as TermColor, Emulator, Flags};
use warpui::color::ColorU;

use crate::theme::Theme;

#[derive(Debug, Clone, Default)]
pub struct TextSpan {
    pub text: String,
    pub color: Option<ColorU>,
    pub bg: Option<ColorU>,
    /// SGR attribute bits (bold/italic/underline/…). Same bit layout as
    /// `dragent_terminal::Flags` masked to `SGR_MASK`, with `INVERSE` excluded —
    /// reverse video is already resolved into `color`/`bg` by
    /// [`styled_to_colored`]. Zero for synthetic/decoration spans.
    pub flags: u16,
}

/// How strongly dim text (SGR 2) is drawn relative to its normal colour.
const DIM_TEXT_OPACITY: f32 = 0.6;

impl TextSpan {
    /// The colour this span's text is drawn in: the app's own, faded when it
    /// asked for dim text.
    pub fn foreground(&self, default: ColorU) -> ColorU {
        let color = self.color.unwrap_or(default);
        if self.flags & Flags::DIM.bits() == 0 {
            return color;
        }
        ColorU {
            a: (f32::from(color.a) * DIM_TEXT_OPACITY) as u8,
            ..color
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColoredLine {
    /// Mirrors `StyledLine::wrapped`: this row auto-wrapped into the next, so
    /// the two are one logical line (link detection joins them).
    pub wrapped: bool,
    pub spans: Vec<TextSpan>,
}

impl ColoredLine {
    /// Concatenated plain text of all spans (no styling), for search/filtering.
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn apply_semantic_fallback(&mut self, theme: &Theme) {
        let combined: String = self.spans.iter().map(|s| s.text.as_str()).collect();
        let lower = combined.to_lowercase();

        let color = if lower.contains("error")
            || lower.contains("err!")
            || lower.contains("failed")
            || lower.contains("eaddrinuse")
            || lower.contains("npm err")
            || lower.contains("eaddrinuse:")
        {
            Some(theme.ansi_red())
        } else if lower.contains("npm warn")
            || lower.contains("deprecated")
            || lower.contains("warn")
            || lower.contains("warning")
        {
            Some(theme.ui_yellow())
        } else if lower.contains("success")
            || lower.contains("found 0 vulnerabilities")
            || lower.contains("✓")
        {
            Some(theme.ansi_green())
        } else if combined.trim_start().starts_with("at ") || combined.contains("    at ") {
            Some(theme.sub_text())
        } else {
            return;
        };

        if self.spans.iter().all(|s| s.color.is_none()) {
            for span in &mut self.spans {
                span.color = color;
            }
        }
    }
}

/// Incremental escape-sequence parser that turns PTY bytes into display lines.
#[cfg(test)]
pub struct EscapeLineParser {
    buffer: String,
    spans: Vec<TextSpan>,
    current_color: Option<ColorU>,
    escape: bool,
    csi: bool,
    csi_params: String,
}

#[cfg(test)]
impl Default for EscapeLineParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl EscapeLineParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            spans: Vec::new(),
            current_color: None,
            escape: false,
            csi: false,
            csi_params: String::new(),
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8], theme: &Theme) -> Vec<ColoredLine> {
        let text = String::from_utf8_lossy(bytes);
        let mut lines = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0usize;

        while i < chars.len() {
            let ch = chars[i];
            if self.escape {
                if ch == '[' {
                    self.csi = true;
                    self.csi_params.clear();
                } else {
                    self.escape = false;
                }
                i += 1;
                continue;
            }

            if self.csi {
                if ch.is_ascii_alphabetic() {
                    match ch {
                        'm' => self.apply_sgr(&self.csi_params.clone(), theme),
                        // Cursor moves / line clears redraw in place — reset our line buffer.
                        'G' | 'H' | 'f' | 'K' => {
                            self.buffer.clear();
                            self.spans.clear();
                        }
                        _ => {}
                    }
                    self.csi = false;
                    self.escape = false;
                } else {
                    self.csi_params.push(ch);
                }
                i += 1;
                continue;
            }

            if ch == '\x1b' {
                self.escape = true;
                i += 1;
                continue;
            }

            if ch == '\r' {
                // PTY programs (including macOS `ls`) often terminate rows with `\r\n`.
                // Treat that as a normal newline; only a lone `\r` redraws the row in place.
                if i + 1 < chars.len() && chars[i + 1] == '\n' {
                    lines.push(self.flush_line(theme));
                    i += 2;
                    continue;
                }
                self.buffer.clear();
                self.spans.clear();
                i += 1;
                continue;
            }

            if ch == '\n' {
                lines.push(self.flush_line(theme));
                i += 1;
                continue;
            }

            if ch.is_control() && ch != '\t' {
                i += 1;
                continue;
            }

            self.push_char(ch);
            i += 1;
        }

        lines
    }

    pub fn flush_pending(&mut self, theme: &Theme) -> Option<ColoredLine> {
        if self.buffer.is_empty() && self.spans.is_empty() {
            return None;
        }
        Some(self.flush_line(theme))
    }

    /// Returns the plain-text content of the in-progress (unflushed) line
    /// without modifying any parser state.
    pub fn peek_pending(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    fn push_char(&mut self, ch: char) {
        if self.buffer.is_empty() || self.span_color() != self.current_color {
            self.spans.push(TextSpan {
                text: String::new(),
                color: self.current_color,
                bg: None,
                flags: 0,
            });
        }
        self.buffer.push(ch);
        if let Some(last) = self.spans.last_mut() {
            last.text.push(ch);
        }
    }

    fn span_color(&self) -> Option<ColorU> {
        self.spans.last().and_then(|s| s.color)
    }

    fn flush_line(&mut self, theme: &Theme) -> ColoredLine {
        let mut line = ColoredLine {
            wrapped: false,
            spans: std::mem::take(&mut self.spans),
        };
        self.buffer.clear();
        if line.spans.is_empty() {
            line.spans.push(TextSpan {
                text: String::new(),
                color: self.current_color,
                bg: None,
                flags: 0,
            });
        }
        line.apply_semantic_fallback(theme);
        line
    }

    fn apply_sgr(&mut self, params: &str, theme: &Theme) {
        for param in params.split(';') {
            match param {
                "0" | "" => self.current_color = None,
                "31" => self.current_color = Some(theme.ansi_red()),
                "32" => self.current_color = Some(theme.ansi_green()),
                "33" => self.current_color = Some(theme.ui_yellow()),
                "34" => self.current_color = Some(theme.ansi_blue()),
                "35" => self.current_color = Some(theme.accent),
                "36" => self.current_color = Some(theme.accent),
                "90" => self.current_color = Some(theme.sub_text()),
                "91" => self.current_color = Some(theme.ansi_red()),
                "92" => self.current_color = Some(theme.ansi_green()),
                "93" => self.current_color = Some(theme.ui_yellow()),
                "94" => self.current_color = Some(theme.ansi_blue()),
                _ => {}
            }
        }
    }
}

// ── Emulator → ColoredLine ──────────────────────────────────────────────────

/// Snapshot all non-blank lines from an emulator grid into `ColoredLine`s.
///
/// Used by `Block::on_completed` to capture output before releasing the
/// emulator, and by `TerminalSession::refresh_bootstrap_lines` for prompt text.
pub fn snapshot_emulator_lines(emu: &Emulator, theme: &Theme) -> Vec<ColoredLine> {
    emulator_content_lines(emu)
        .iter()
        .map(|l| styled_to_colored(l, theme))
        .collect()
}

/// [`snapshot_emulator_lines`] using only the colours the app asked for.
pub fn snapshot_emulator_lines_exact(emu: &Emulator, theme: &Theme) -> Vec<ColoredLine> {
    emulator_content_lines(emu)
        .iter()
        .map(|l| styled_to_colored_exact(l, theme))
        .collect()
}

fn emulator_content_lines(emu: &Emulator) -> Vec<dragent_terminal::StyledLine> {
    let grid = emu.grid();
    let total = grid.total_lines();
    let mut lines = grid.styled_lines_in(0, total);
    while lines.last().is_some_and(|l| l.is_blank()) {
        lines.pop();
    }
    lines
}

/// Convert a single emulator `StyledLine` to a `ColoredLine`, tinting plain
/// command output that reads like an error, warning or success.
pub fn styled_to_colored(line: &dragent_terminal::StyledLine, theme: &Theme) -> ColoredLine {
    let mut out = styled_to_colored_exact(line, theme);
    out.apply_semantic_fallback(theme);
    out
}

/// Convert a line using only the colours the app asked for. Full-screen apps
/// choose their own colours, so nothing is inferred from the words on a row.
pub fn styled_to_colored_exact(line: &dragent_terminal::StyledLine, theme: &Theme) -> ColoredLine {
    let spans: Vec<TextSpan> = line
        .spans
        .iter()
        .map(|span| {
            let mut fg = term_color_to_ui(span.style.fg, theme);
            let mut bg = term_color_to_ui(span.style.bg, theme);
            if span.style.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                fg = fg.or_else(|| Some(theme.background));
                bg = bg.or_else(|| Some(theme.main_text()));
            }
            // Carry the SGR attributes (bold/italic/underline/…). INVERSE is
            // dropped — it's already resolved into fg/bg above.
            let flags = (span.style.flags & Flags::SGR_MASK).bits() & !Flags::INVERSE.bits();
            TextSpan {
                text: span.text.clone(),
                color: fg,
                bg,
                flags,
            }
        })
        .collect();
    ColoredLine {
        wrapped: line.wrapped,
        spans: if spans.is_empty() {
            vec![TextSpan {
                text: String::new(),
                color: None,
                bg: None,
                flags: 0,
            }]
        } else {
            spans
        },
    }
}

fn term_color_to_ui(color: TermColor, theme: &Theme) -> Option<ColorU> {
    match color {
        TermColor::Default => None,
        TermColor::Rgb(r, g, b) => Some(ColorU::new(r, g, b, 255)),
        TermColor::Indexed(idx) => Some(indexed_color(idx, theme)),
        TermColor::Named(n) => Some(indexed_color(n.index(), theme)),
    }
}

fn indexed_color(idx: u8, theme: &Theme) -> ColorU {
    match idx {
        // ANSI 0–15 come straight from the active theme's palette, so terminal
        // output colors match the theme exactly.
        0..=15 => theme.ansi(idx as usize),
        16..=231 => {
            let v = idx - 16;
            let scale = |c: u8| if c == 0 { 0 } else { 55 + c * 40 };
            ColorU::new(scale(v / 36), scale((v % 36) / 6), scale(v % 6), 255)
        }
        232..=255 => {
            let level = 8 + (idx - 232) * 10;
            ColorU::new(level, level, level, 255)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &ColoredLine) -> String {
        line.spans.iter().map(|s| s.text.clone()).collect()
    }

    #[test]
    fn dim_text_fades_the_apps_own_colour() {
        let red = ColorU::new(200, 10, 10, 255);
        let mut span = TextSpan {
            text: "x".to_string(),
            color: Some(red),
            bg: None,
            flags: 0,
        };
        assert_eq!(span.foreground(ColorU::white()), red);
        span.flags = Flags::DIM.bits();
        assert_eq!(span.foreground(ColorU::white()).r, 200);
        assert!(span.foreground(ColorU::white()).a < 255);
    }

    #[test]
    fn exact_colors_never_tint_a_row_by_its_words() {
        let theme = Theme::dark();
        let line = dragent_terminal::StyledLine {
            spans: vec![dragent_terminal::StyledSpan {
                text: "error: build failed".to_string(),
                style: dragent_terminal::Style {
                    fg: TermColor::Default,
                    bg: TermColor::Default,
                    flags: Flags::empty(),
                    underline_color: None,
                },
            }],
            wrapped: false,
        };
        assert_eq!(styled_to_colored_exact(&line, &theme).spans[0].color, None);
        assert_eq!(
            styled_to_colored(&line, &theme).spans[0].color,
            Some(theme.ansi_red())
        );
    }

    #[test]
    fn carriage_return_clears_in_progress_line() {
        let mut parser = EscapeLineParser::new();
        let theme = Theme::dark();
        let lines = parser.push_bytes(b"ab\rls\n", &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "ls");
    }

    #[test]
    fn crlf_preserves_ls_row() {
        let mut parser = EscapeLineParser::new();
        let theme = Theme::dark();
        let lines = parser.push_bytes(b"app\tcrates\r\nCargo.toml\r\n", &theme);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "app\tcrates");
        assert_eq!(line_text(&lines[1]), "Cargo.toml");
    }

    #[test]
    fn cursor_positioning_clears_line_buffer() {
        let mut parser = EscapeLineParser::new();
        let theme = Theme::dark();
        let lines = parser.push_bytes(b"%\x1b[120Gls\n", &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "ls");
    }
}
