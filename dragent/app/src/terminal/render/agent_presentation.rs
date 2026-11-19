//! Shared presentation layer for agent blocks — one design vocabulary so tool
//! steps, edits, results, and turn summaries read as one coherent system
//! (the Cursor/Claude-Code "the UI argues the agent is competent" feel).
//!
//! Two responsibilities:
//!   1. `tool_presentation` — translate a raw tool call into human language:
//!      a monochrome glyph + a verb + the object (a file basename, a query,
//!      a command) + an outcome detail parsed from the result. No emoji, no
//!      snake_case, no raw arg blobs in the header.
//!   2. Diff cards + shared card chrome + a `receipt` summary for a finished
//!      turn ("Edited 2 files · +49 −12 · ran 3 commands · 12s").

use warpui::color::ColorU;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::agent::edit_diff::{DiffLineKind, EditDiff};
use crate::theme::Theme;

/// What a tool step should show in its one-line header.
pub struct ToolPresentation {
    /// Monochrome geometric glyph (renders in the mono font).
    pub glyph: &'static str,
    /// Human verb, Title Case: "Read", "Searched", "Edited", "Ran".
    pub verb: String,
    /// The object acted on: a file basename, a query, a command.
    pub object: String,
}

/// Normalize a tool name for matching. ToolStep blocks store the HUMANIZED
/// display name ("Read File"), while the pipeline uses snake_case
/// ("read_file") — accept both by lowercasing and folding spaces to
/// underscores, and strip any MCP prefix (`mcp__server__tool`).
pub fn bare_tool(name: &str) -> String {
    let mut n = name;
    for p in ["mcp__", "prompt__", "resource__"] {
        if let Some(rest) = n.strip_prefix(p) {
            n = rest.rsplit("__").next().unwrap_or(rest);
        }
    }
    n.to_lowercase().replace(' ', "_")
}

/// Last path component (basename) for compact display; falls back to the input.
fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// Translate a tool call into glyph + verb + object. `args_summary` is the raw
/// short arg string the pipeline already produces (a path, a pattern, a
/// command); we shape it into the object.
pub fn tool_presentation(name: &str, args_summary: &str) -> ToolPresentation {
    let n = bare_tool(name);
    let n = n.as_str();
    let structured = serde_json::from_str::<serde_json::Value>(args_summary).ok();
    let summary = structured.as_ref().and_then(|value| {
        let keys: &[&str] = match n {
            "skill" => &["name"], "spawn_subagent" => &["description", "task", "role", "query"],
            "run_command" => &["command"], "grep" | "search_files" => &["pattern", "query"],
            _ => &["path", "query", "name", "title", "url"],
        };
        keys.iter().find_map(|key| value[*key].as_str()).map(str::to_string)
    });
    let arg = summary.as_deref().unwrap_or(args_summary.trim());
    // For grep/skill the arg string carries "pattern=… path=…" / "name=…"; take
    // the most useful token as the object.
    let (glyph, verb, object): (&str, &str, String) = match n {
        "read_file" | "read_multiple_files" => ("◇", "Read", basename(arg)),
        "list_directory"
        | "list_directory_with_sizes"
        | "directory_tree"
        | "list_allowed_directories" => ("◇", "Listed", basename(arg)),
        "get_file_info" => ("◇", "Inspected", basename(arg)),
        "write_file" => ("◆", "Wrote", basename(arg)),
        "edit_file" | "apply_patch" => ("◆", "Edited", basename(arg)),
        "batch_edit" => ("◆", "Edited", arg.to_string()), // "N edits"
        "create_directory" => ("◆", "Created", basename(arg)),
        "move_file" => ("◆", "Moved", arg.to_string()),
        "grep" | "search_files" => ("◈", "Searched", extract_after(arg, "pattern=")),
        "search_codebase" => ("◈", "Searched", arg.to_string()),
        "run_command" => ("▷", "Ran", arg.to_string()),
        "web_search" | "fetch_url" => ("◈", "Searched web", arg.to_string()),
        "skill" => ("\u{2022}", "Skill", extract_after(arg, "name=")),
        "spawn_subagent" => ("\u{2022}", "Delegated", arg.to_string()),
        "repl" => ("▷", "Ran", extract_after(arg, "lang=")),
        "sleep" => ("◷", "Waited", arg.to_string()),
        "mcp_auth" => ("\u{2022}", "Authenticated", arg.to_string()),
        // A lead coordinating other agents.
        "coord_task_add" => ("◆", "Planned", arg.to_string()),
        "coord_task_edit" => ("◆", "Changed", arg.to_string()),
        "coord_task_start" if arg.is_empty() => ("▷", "Started", "the ready tasks".into()),
        "coord_task_start" => ("▷", "Started", arg.to_string()),
        "coord_task_show" => ("◇", "Looked at", arg.to_string()),
        "coord_task_read" => ("◇", "Read the screen of", arg.to_string()),
        "coord_task_keys" => ("▷", "Pressed keys in", arg.to_string()),
        "coord_task_cancel" => ("◆", "Cancelled", arg.to_string()),
        "coord_task_reassign" => ("◆", "Moved", arg.to_string()),
        "coord_team_agents" => ("◆", "Added agents", arg.to_string()),
        "coord_task_reopen" => ("◆", "Sent back", arg.to_string()),
        "coord_task_diff" => ("◇", "Read the changes of", arg.to_string()),
        "coord_task_integrate" => ("◆", "Integrated", arg.to_string()),
        "coord_team_status" => ("◇", "Checked", "the plan".into()),
        "coord_team_stop" => ("◆", "Stopped", "the work".into()),
        "coord_msg_send" => ("\u{2022}", "Wrote to", arg.to_string()),
        "coord_msg_inbox" => ("◇", "Read", "the lead's messages".into()),
        _ => ("◇", &title_case(n), arg.to_string()),
    };
    ToolPresentation {
        glyph,
        verb: verb.to_string(),
        object,
    }
}

/// What a tool row's object is, so it takes the terminal's colour for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Command,
    File,
    Query,
    Other,
}

pub fn object_kind(name: &str) -> ObjectKind {
    match bare_tool(name).as_str() {
        "run_command" => ObjectKind::Command,
        "read_file" | "read_multiple_files" | "list_directory" | "list_directory_with_sizes"
        | "directory_tree" | "get_file_info" | "write_file" | "edit_file" | "apply_patch"
        | "create_directory" | "move_file" => ObjectKind::File,
        "grep" | "search_files" | "search_codebase" | "web_search" => ObjectKind::Query,
        _ => ObjectKind::Other,
    }
}

/// A tool row's object in its terminal colour: commands as the input colours
/// them, files in the file colour, searches like a quoted argument.
pub fn object_text(kind: ObjectKind, object: &str, font: FamilyId, size: f32, theme: &Theme) -> Text {
    match kind {
        ObjectKind::Command => command_text("", object, font, size, theme),
        ObjectKind::File => crate::terminal::render::term_colors::file_text(object, font, size, theme),
        ObjectKind::Query => Text::new(object.to_string(), font, size).with_color(theme.link()),
        ObjectKind::Other => Text::new(object.to_string(), font, size).with_color(theme.main_text()),
    }
}

/// A shell command coloured the way the terminal input colours it: the command
/// name green, flags dim, arguments and strings blue, operators in the accent.
/// `prompt` (such as `"$ "`) goes before it, dimmed.
pub fn command_text(prompt: &str, command: &str, font: FamilyId, size: f32, theme: &Theme) -> Text {
    let shift = prompt.len();
    let mut paint = Vec::new();
    if shift > 0 {
        paint.push((0..shift, theme.sub_text(), false));
    }
    paint.extend(
        crate::console::input_bar::command_spans(command, theme)
            .into_iter()
            .map(|(range, color)| (range.start + shift..range.end + shift, color, false)),
    );
    crate::terminal::render::term_colors::painted_text(
        format!("{prompt}{command}"),
        &paint,
        font,
        size,
        theme.main_text(),
    )
}

/// Pull the value after `key=` out of a "key=val …" summary; whole string if absent.
fn extract_after(s: &str, key: &str) -> String {
    if let Some(idx) = s.find(key) {
        let rest = &s[idx + key.len()..];
        let val = rest.split_whitespace().next().unwrap_or(rest);
        return val.trim_matches(['"', '\'']).to_string();
    }
    s.to_string()
}

/// snake_case → Title Case, allocated (fallback for unknown tools).
fn title_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a compact outcome detail from a tool's result string, e.g.
/// "12 matches", "1240 lines", "exit 0". Empty when nothing useful is found.
pub fn result_detail(name: &str, result: &str) -> String {
    let n = bare_tool(name);
    let n = n.as_str();
    let r = result.trim();
    match n {
        "grep" | "search_files" | "search_codebase" => {
            let matches = r.lines().count();
            if matches > 0 && !r.is_empty() {
                format!("{matches} match{}", if matches == 1 { "" } else { "es" })
            } else {
                "no matches".to_string()
            }
        }
        "read_file" | "read_multiple_files" => {
            let lines = r.lines().count();
            format!("{lines} line{}", if lines == 1 { "" } else { "s" })
        }
        _ => String::new(),
    }
}

// ── Diff card ───────────────────────────────────────────────────────────────

/// The diff card shows at most this many ± lines on screen; beyond it a footer
/// notes the remainder. A hard cap (rather than an inner vertical scrollbar)
/// keeps the card's height bounded so a big edit can't shove the input bar
/// off screen — and it avoids nesting two `ClippedScrollable`s (a known
/// framework constraint-passing bug), which is what made the card paint over
/// the input bar. Long lines still scroll horizontally within this cap.
pub const MAX_VISIBLE_DIFF_LINES: usize = 22;

/// How many diff lines a collapsed card hides (0 when it all fits). The caller
/// uses this to decide whether to show the clickable "+N more lines" toggle.
pub fn diff_hidden_lines(diff: &EditDiff) -> usize {
    let total: usize = diff.files.iter().map(|f| f.lines.len()).sum();
    total.saturating_sub(MAX_VISIBLE_DIFF_LINES)
}

/// Render a captured edit diff as a ±hunk card (syntax-neutral, colored
/// gutters). This is the single biggest jump toward looking professional —
/// edits become visible instead of a sentence.
///
/// Long lines soft-wrap inside the card (they do NOT horizontally scroll):
/// a horizontal `ClippedScrollable` here starts a new, higher-z paint layer
/// whose clip is its own bounds — when a block straddles the viewport bottom
/// that layer escapes and paints over the input bar. Wrapping keeps the whole
/// diff in the base paint layer, so the opaque input bar always covers it, and
/// no content is ever hidden. Height is bounded by capping visible lines.
pub fn diff_card(
    diff: &EditDiff,
    expanded: bool,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(0.0);

    // Line budget shared across all files in this diff, so a multi-file edit
    // stays bounded too. When the user has expanded the card, show every
    // captured line; otherwise clip to the display budget and let the caller
    // render a clickable "+N more lines" toggle.
    let mut budget = if expanded {
        usize::MAX
    } else {
        MAX_VISIBLE_DIFF_LINES
    };

    for (fi, file) in diff.files.iter().enumerate() {
        if budget == 0 {
            break;
        }
        if fi > 0 {
            col = col.with_child(spacer(6.0));
        }
        // File header: basename bold + dimmed parent + ±counts.
        let bn = basename(&file.path);
        let mut header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Text::new(bn, mono, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .with_style(Properties {
                        weight: Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
            );
        if file.added > 0 {
            header = header.with_child(
                Text::new(
                    format!("+{}", file.added),
                    mono,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.ui_green())
                .finish(),
            );
        }
        if file.removed > 0 {
            header = header.with_child(
                Text::new(
                    format!("−{}", file.removed),
                    mono,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.ui_red())
                .finish(),
            );
        }

        // Body is width-constrained (Stretch): each line's text soft-wraps
        // within the card instead of overflowing horizontally. No scrollable —
        // see the fn doc for why horizontal scroll here paints over the input bar.
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(0.0);
        let shown = file.lines.len().min(budget);
        budget -= shown;
        for line in &file.lines[..shown] {
            let (gutter, fg, bg) = match line.kind {
                DiffLineKind::Add => ("+", theme.ui_green(), tint(theme.ui_green())),
                DiffLineKind::Remove => ("−", theme.ui_red(), tint(theme.ui_red())),
                DiffLineKind::Context => (" ", theme.disabled_text(), ColorU::transparent_black()),
            };
            // Gutter is fixed width; the code text takes the remaining width and
            // soft-wraps (Text defaults to soft_wrap: true) inside the Expanded.
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_child(
                    Text::new(format!("{gutter} "), mono, crate::theme::text_size(12.0))
                        .with_color(fg)
                        .soft_wrap(false)
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(line.text.clone(), mono, crate::theme::text_size(12.0))
                            .with_color(if line.kind == DiffLineKind::Context {
                                theme.sub_text()
                            } else {
                                theme.main_text()
                            })
                            .finish(),
                    )
                    .finish(),
                );
            body = body.with_child(
                Container::new(row.finish())
                    .with_background_color(bg)
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .finish(),
            );
        }
        if file.truncated && shown == file.lines.len() {
            body = body.with_child(
                Text::new(
                    "  … diff truncated".to_string(),
                    mono,
                    crate::theme::text_size(11.5),
                )
                .with_color(theme.disabled_text())
                .finish(),
            );
        }

        let card = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0)
                .with_child(header.finish())
                .with_child(body.finish())
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
        col = col.with_child(card);
    }

    // NOTE: the "+N more lines" toggle is rendered by the caller (it has the
    // tool_id + render ctx needed to make it clickable — see agent_block_element).

    col.finish()
}

/// A very low-alpha tint of a color for diff-line backgrounds.
fn tint(c: ColorU) -> ColorU {
    ColorU::new(c.r, c.g, c.b, 28)
}

fn spacer(h: f32) -> Box<dyn Element> {
    warpui::elements::ConstrainedBox::new(warpui::elements::Empty::new().finish())
        .with_height(h)
        .finish()
}

// ── Turn receipt ────────────────────────────────────────────────────────────

/// A one-line receipt for a finished turn, built from its child blocks:
/// "Edited 2 files · +49 −12 · ran 3 commands · 12s". Returns None → caller
/// keeps the plain duration.
pub fn turn_receipt(
    files_edited: usize,
    added: usize,
    removed: usize,
    commands: usize,
    reads: usize,
    duration_ms: Option<i64>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if files_edited > 0 {
        let mut s = format!("Edited {files_edited} file{}", plural(files_edited));
        if added + removed > 0 {
            s.push_str(&format!(" · +{added} −{removed}"));
        }
        parts.push(s);
    }
    if commands > 0 {
        parts.push(format!("ran {commands} command{}", plural(commands)));
    }
    if files_edited == 0 && commands == 0 && reads > 0 {
        parts.push(format!("Explored {reads} file{}", plural(reads)));
    }
    // With no countable work, still say something human, not a bare number.
    if parts.is_empty() {
        return match duration_ms {
            Some(ms) => format!("Worked for {}", fmt_duration(ms)),
            None => "Done".to_string(),
        };
    }
    if let Some(ms) = duration_ms {
        parts.push(fmt_duration(ms));
    }
    parts.join(" · ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn fmt_duration(ms: i64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!("{secs}s")
    } else {
        format!("{ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_tool_headers_show_the_subject_and_keep_payloads_out_of_the_title() {
        let skill = tool_presentation("skill", r#"{"name":"review-bugbot","args":"long prompt"}"#);
        assert_eq!(skill.object, "review-bugbot");
        let command = tool_presentation("run_command", r#"{"command":"cargo test","cwd":"/project"}"#);
        assert_eq!(command.object, "cargo test");
    }

    #[test]
    fn a_lead_coordinating_reads_as_what_it_did() {
        // Blocks store the humanized name.
        let p = tool_presentation("Coord Task Add", "Invite API");
        assert_eq!(
            (p.verb.as_str(), p.object.as_str()),
            ("Planned", "Invite API")
        );
        let p = tool_presentation("coord_task_start", "");
        assert_eq!(
            (p.verb.as_str(), p.object.as_str()),
            ("Started", "the ready tasks")
        );
        let p = tool_presentation("coord_msg_send", "t2");
        assert_eq!((p.verb.as_str(), p.object.as_str()), ("Wrote to", "t2"));
    }

    #[test]
    fn presentation_is_human() {
        let p = tool_presentation("read_file", "/Users/x/dev/app/src/manager.rs");
        assert_eq!(p.verb, "Read");
        assert_eq!(p.object, "manager.rs");
        assert_eq!(p.glyph, "◇");

        let g = tool_presentation("grep", "pattern=\"session_facts\" path=src");
        assert_eq!(g.verb, "Searched");
        assert_eq!(g.object, "session_facts");

        let r = tool_presentation("run_command", "npm test -- --run x");
        assert_eq!(r.verb, "Ran");
    }

    #[test]
    fn receipt_reads_like_a_summary() {
        let s = turn_receipt(2, 49, 12, 3, 5, Some(12_000));
        assert_eq!(s, "Edited 2 files · +49 −12 · ran 3 commands · 12s");
        assert_eq!(
            turn_receipt(0, 0, 0, 0, 4, Some(800)),
            "Explored 4 files · 800ms"
        );
        assert_eq!(turn_receipt(0, 0, 0, 0, 0, None), "Done");
        assert_eq!(turn_receipt(0, 0, 0, 0, 0, Some(10_000)), "Worked for 10s");
    }
}
