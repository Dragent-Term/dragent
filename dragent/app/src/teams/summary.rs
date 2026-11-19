//! Turns what agents report into what a person can read: one plain line for
//! each task and a sentence for the whole run. Newer workers write a plain
//! summary and keep technical notes apart; older reports are one dense block,
//! so the first sentence is cleaned up and the rest stays behind "Show details".

use dragent_host::team::{Task, TaskStatus, Team, TeamStatus};

/// Longest line shown for a task before its details are opened.
const HEADLINE_LIMIT: usize = 170;
/// Longest part of the lead's last message shown before it's opened.
const LEAD_PREVIEW_LIMIT: usize = 420;

/// `text` cut at the last clause or word before `limit` characters.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    let end = [", ", "; ", " - ", " + "]
        .iter()
        .filter_map(|separator| cut.rfind(separator))
        .max()
        .filter(|end| *end >= limit / 2)
        .or_else(|| cut.rfind(' '))
        .unwrap_or(cut.len());
    format!("{}…", cut[..end].trim_end())
}

/// Takes out what only means something to whoever is reading the branch:
/// bracketed asides, and a leading `t3:` or `Committed a4c51f0:`.
fn without_noise(text: &str, branch: Option<&str>) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    if let Some(branch) = branch {
        out = out.replace(&format!(" on branch {branch}"), "");
    }
    let mut rest = out.trim();
    if let Some((first, after)) = rest.split_once(": ") {
        let commit = first
            .strip_prefix("Committed ")
            .is_some_and(|hash| hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit()));
        if commit {
            rest = after;
        }
    }
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    if words.first().is_some_and(|word| is_task_number(word)) {
        words.remove(0);
        if words.first().is_some_and(|word| word.trim_end_matches(':').eq_ignore_ascii_case("done")) {
            words.remove(0);
        }
    }
    words.join(" ").replace(" ,", ",").replace(" .", ".").replace(" :", ":")
}

/// `t3`, `T4:`: how agents refer to their own task, which means nothing here.
fn is_task_number(word: &str) -> bool {
    let word = word.trim_end_matches(':');
    word.strip_prefix(['t', 'T'])
        .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

/// Up to the first full stop that ends a sentence: not one inside `0.88`.
fn first_sentence(text: &str) -> &str {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (index, (position, ch)) in chars.iter().enumerate() {
        if *ch == '.' && chars.get(index + 1).is_some_and(|(_, next)| next.is_whitespace()) {
            return &text[..=*position];
        }
    }
    text
}

/// One plain line for what a task reported.
pub(super) fn headline(summary: &str, branch: Option<&str>) -> String {
    let cleaned = without_noise(&plain(summary), branch);
    let line = clip(first_sentence(&cleaned), HEADLINE_LIMIT);
    let mut chars = line.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => summary.trim().to_string(),
    }
}

/// One plain line for what a task was asked to do.
pub(super) fn brief(prompt: &str) -> String {
    let prompt = plain(prompt);
    let first = prompt.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    clip(first_sentence(first), HEADLINE_LIMIT)
}

/// How a run stands, in a sentence.
pub(super) fn standing(team: &Team) -> String {
    let total = team.tasks.len();
    let count = |status| team.tasks.iter().filter(|task| task.status == status).count();
    let done = count(TaskStatus::Done);
    let running = count(TaskStatus::Running);
    let tasks = if total == 1 { "task" } else { "tasks" };
    match team.status {
        TeamStatus::Done if total == done => format!("All {total} {tasks} finished."),
        TeamStatus::Done => format!("Finished, with {done} of {total} {tasks} done."),
        TeamStatus::Stopped if total == done => format!("Stopped after all {total} {tasks} finished."),
        TeamStatus::Stopped => format!("Stopped before the end: {done} of {total} {tasks} finished."),
        TeamStatus::Planning => "The lead is still planning.".to_string(),
        _ if running > 0 => format!("{done} of {total} {tasks} finished, {running} in progress."),
        _ => format!("{done} of {total} {tasks} finished."),
    }
}

/// Files the run changed and checks it ran, over all its tasks.
pub(super) fn totals(team: &Team) -> (usize, usize) {
    let mut files = std::collections::BTreeSet::new();
    let mut checks = 0;
    for task in &team.tasks {
        if let Some(result) = &task.result {
            files.extend(result.files.iter().map(String::as_str));
            checks += result.checks.len();
        }
        if let Some(integrated) = &task.integrated {
            files.extend(integrated.files.iter().map(String::as_str));
            checks += integrated.checks.len();
        }
    }
    (files.len(), checks)
}

/// Why a task of a finished run didn't get done.
pub(super) fn why_unfinished(task: &Task) -> String {
    match (&task.reason, task.status) {
        (Some(reason), _) if !reason.trim().is_empty() => clip(reason.trim(), HEADLINE_LIMIT),
        (_, TaskStatus::Cancelled) => "Cancelled when the run stopped.".to_string(),
        (_, TaskStatus::Failed) => "It failed.".to_string(),
        (_, TaskStatus::NeedsDecision) => "It waits for a decision on its review.".to_string(),
        _ => "It never started.".to_string(),
    }
}

/// The start of the lead's last message: its first paragraph, kept short.
/// `None` for anything that isn't worth showing as an account of the run.
pub(super) fn lead_preview(message: &str) -> Option<(String, bool)> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let first = message.split("\n\n").next().unwrap_or(message).trim();
    let preview = clip(&plain(first), LEAD_PREVIEW_LIMIT);
    let more = preview.chars().count() < message.chars().count();
    Some((preview, more))
}

/// Markdown as the words a person reads: no `**`, backticks, `#` or link
/// syntax. Blocks stay on their own lines; list items keep a bullet.
pub(super) fn plain(markdown: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut out = String::new();
    let mut ordered: Vec<Option<u64>> = Vec::new();
    let newline = |out: &mut String| {
        let trimmed = out.trim_end_matches(' ').len();
        out.truncate(trimmed);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
    };
    for event in Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
        match event {
            Event::Text(text) | Event::Code(text) => out.push_str(&text),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak | Event::Rule => newline(&mut out),
            Event::Start(Tag::List(start)) => ordered.push(start),
            Event::End(TagEnd::List(_)) => {
                ordered.pop();
            }
            Event::Start(Tag::Item) => {
                newline(&mut out);
                match ordered.last_mut() {
                    Some(Some(number)) => {
                        out.push_str(&format!("{number}. "));
                        *number += 1;
                    }
                    _ => out.push_str("• "),
                }
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::TableRow | TagEnd::TableHead) => {
                newline(&mut out)
            }
            Event::End(TagEnd::TableCell) => out.push_str("  "),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Headings as bold lines: in a card or a detail a `## Title` would
/// otherwise draw as large as the page's own title. Code blocks are kept.
pub(super) fn quiet_headings(markdown: &str) -> String {
    let mut fenced = false;
    markdown
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
            }
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            let title = trimmed[hashes..].trim();
            if fenced || !(1..=6).contains(&hashes) || !trimmed[hashes..].starts_with(' ') || title.is_empty() {
                return line.to_string();
            }
            format!("**{}**", title.trim_end_matches('#').trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `12m`, `1h 5m`, or `under a minute`.
pub(super) fn duration(task: &Task) -> Option<String> {
    let minutes = (task.finished_at_ms? - task.started_at_ms?) / 60_000;
    Some(match minutes {
        0 => "under a minute".to_string(),
        1..=59 => format!("{minutes}m"),
        _ if minutes % 60 == 0 => format!("{}h", minutes / 60),
        _ => format!("{}h {}m", minutes / 60, minutes % 60),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_drops_markdown_marks() {
        assert_eq!(
            plain("## Done\n\n**Added** `invite()` and [docs](https://x.dev).\n\n- one\n- two"),
            "Done\nAdded invite() and docs.\n• one\n• two"
        );
        assert_eq!(headline("**Fixed** the `parse` bug.", None), "Fixed the parse bug.");
        assert_eq!(quiet_headings("## Plan\n```\n# keep\n```"), "**Plan**\n```\n# keep\n```");
    }

    #[test]
    fn a_dense_report_becomes_one_readable_line() {
        let branch = "scaffold-greenfield-expo-sdk58-router-ho-t1-9fa0";
        let report = format!(
            "Scaffolded greenfield Expo SDK58 app on branch {branch} (commit d089dff). Fresh \
             package.json (expo ~58.0.19, RN 0.88, React 19.3). Legacy src files removed."
        );
        assert_eq!(headline(&report, Some(branch)), "Scaffolded greenfield Expo SDK58 app.");
    }

    #[test]
    fn a_leading_task_number_or_commit_is_dropped() {
        assert_eq!(
            headline("Committed a4c51f0: camera-free hook and tests. Only five files changed.", None),
            "Camera-free hook and tests."
        );
        assert_eq!(
            headline("t3: added the detection screen (VisionCamera). Committed a3a514a.", None),
            "Added the detection screen."
        );
    }

    #[test]
    fn an_agents_reference_to_its_own_task_is_dropped() {
        assert_eq!(headline("T4 greenfield integration. Checks pass.", None), "Greenfield integration.");
        assert_eq!(headline("T5 DONE via simulator build.", None), "Via simulator build.");
        assert_eq!(headline("QA pass over the rebuild : all green.", None), "QA pass over the rebuild: all green.");
        // Only a whole word counts: this is a real first word.
        assert_eq!(headline("Tests pass.", None), "Tests pass.");
    }

    #[test]
    fn a_version_number_does_not_end_the_sentence() {
        assert_eq!(headline("Uses RN 0.88 now. Tests pass.", None), "Uses RN 0.88 now.");
    }

    #[test]
    fn a_long_line_stops_at_a_clause() {
        let long = format!("Added {}", "a thing, ".repeat(40));
        let line = headline(&long, None);
        assert!(line.chars().count() <= HEADLINE_LIMIT + 1, "{line}");
        assert!(line.ends_with('…'), "{line}");
    }

    #[test]
    fn a_plain_summary_stays_as_written() {
        assert_eq!(
            headline("People can invite teammates by email, and links expire after a week.", None),
            "People can invite teammates by email, and links expire after a week."
        );
    }

    #[test]
    fn the_lead_preview_says_when_there_is_more() {
        assert_eq!(lead_preview("  "), None);
        assert_eq!(lead_preview("All done."), Some(("All done.".to_string(), false)));
        assert!(lead_preview("First part.\n\nSecond part.").unwrap().1);
    }
}
