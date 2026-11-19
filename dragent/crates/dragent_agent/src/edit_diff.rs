//! Structured diffs captured at edit time, keyed by the tool call id, so the
//! agent-block renderer can show a real ±hunk card instead of the flat string
//! "Applied 1 replacement(s) in …".
//!
//! The edit tools (`edit_file`, `batch_edit`, `apply_patch`, `write_file`) run
//! client-side with a known `CALL_ID`; they record the change here, and
//! `agent_block_element` looks it up by the ToolStep's `tool_id`. This keeps
//! the LLM-facing tool result a short string while the UI gets rich data —
//! the two never mix. Session-lived (in memory); the persisted text summary is
//! the fallback after a restart.

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

/// One line of a rendered diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineKind {
    Context,
    Add,
    Remove,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// A per-file change ready to render.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileDiff {
    /// Full path (renderer shows the basename + a dimmed parent).
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<DiffLine>,
    /// True when `lines` was clipped for display (very large edit).
    pub truncated: bool,
}

/// All file changes made by one tool call (batch_edit / apply_patch touch many).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EditDiff {
    pub files: Vec<FileDiff>,
}

impl EditDiff {
    pub fn total_added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }
    pub fn total_removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }
}

static DIFFS: Lazy<Mutex<HashMap<String, EditDiff>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Max diff lines shown per file before clipping (keeps the card bounded).
const MAX_LINES_PER_FILE: usize = 60;

/// Record a diff under a tool call id (no-op if call_id is empty).
pub fn record(call_id: &str, diff: EditDiff) {
    if call_id.is_empty() || diff.files.is_empty() {
        return;
    }
    DIFFS.lock().unwrap().insert(call_id.to_string(), diff);
}

/// Look up a captured diff for a ToolStep by its id.
pub fn get(call_id: &str) -> Option<EditDiff> {
    DIFFS.lock().unwrap().get(call_id).cloned()
}

/// Diff cards whose "+N more lines" the user clicked to expand, keyed by the
/// same tool call id. Session-lived; a collapsed card is the default.
static EXPANDED: Lazy<Mutex<std::collections::HashSet<String>>> =
    Lazy::new(|| Mutex::new(std::collections::HashSet::new()));

/// Whether this diff card is currently expanded (showing all captured lines).
pub fn is_expanded(call_id: &str) -> bool {
    EXPANDED
        .lock()
        .map(|s| s.contains(call_id))
        .unwrap_or(false)
}

/// Flip a diff card between collapsed (line-budgeted) and expanded (all lines).
pub fn toggle_expanded(call_id: &str) {
    if call_id.is_empty() {
        return;
    }
    if let Ok(mut s) = EXPANDED.lock() {
        if !s.remove(call_id) {
            s.insert(call_id.to_string());
        }
    }
}

/// Build a single-file diff from an exact old→new block replacement
/// (edit_file / batch_edit): the replaced text is `Remove`, the new text is
/// `Add`. A trailing run of identical lines is elided so a small change inside
/// a big block doesn't render the whole block twice.
pub fn from_replacement(path: &str, old_block: &str, new_block: &str) -> FileDiff {
    let old_lines: Vec<&str> = old_block.split('\n').collect();
    let new_lines: Vec<&str> = new_block.split('\n').collect();

    // Trim common prefix/suffix so only the genuinely changed span is shown as
    // ±, with a couple of context lines around it.
    let mut pre = 0;
    while pre < old_lines.len() && pre < new_lines.len() && old_lines[pre] == new_lines[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < old_lines.len() - pre
        && suf < new_lines.len() - pre
        && old_lines[old_lines.len() - 1 - suf] == new_lines[new_lines.len() - 1 - suf]
    {
        suf += 1;
    }
    const CTX: usize = 2;
    let ctx_start = pre.saturating_sub(CTX);

    let mut lines = Vec::new();
    for l in &old_lines[ctx_start..pre] {
        lines.push(DiffLine {
            kind: DiffLineKind::Context,
            text: (*l).to_string(),
        });
    }
    let removed_slice = &old_lines[pre..old_lines.len() - suf];
    let added_slice = &new_lines[pre..new_lines.len() - suf];
    for l in removed_slice {
        lines.push(DiffLine {
            kind: DiffLineKind::Remove,
            text: (*l).to_string(),
        });
    }
    for l in added_slice {
        lines.push(DiffLine {
            kind: DiffLineKind::Add,
            text: (*l).to_string(),
        });
    }
    let ctx_end = (old_lines.len() - suf + CTX).min(old_lines.len());
    for l in &old_lines[old_lines.len() - suf..ctx_end] {
        lines.push(DiffLine {
            kind: DiffLineKind::Context,
            text: (*l).to_string(),
        });
    }

    let added = added_slice.len();
    let removed = removed_slice.len();
    let truncated = lines.len() > MAX_LINES_PER_FILE;
    if truncated {
        lines.truncate(MAX_LINES_PER_FILE);
    }
    FileDiff {
        path: path.to_string(),
        added,
        removed,
        lines,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_isolates_the_changed_span() {
        let old = "fn a() {\n    let x = 1;\n    do_thing(x);\n}";
        let new = "fn a() {\n    let x = 2;\n    do_thing(x);\n}";
        let fd = from_replacement("a.rs", old, new);
        assert_eq!(fd.added, 1);
        assert_eq!(fd.removed, 1);
        // Only the changed line pair is ±; the fn header/body are context.
        assert!(
            fd.lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Add && l.text.contains("= 2"))
        );
        assert!(
            fd.lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Remove && l.text.contains("= 1"))
        );
        assert!(
            fd.lines
                .iter()
                .any(|l| l.kind == DiffLineKind::Context && l.text.contains("fn a()"))
        );
    }

    #[test]
    fn record_and_get_roundtrip() {
        let cid = format!("call-{}", uuid::Uuid::new_v4());
        record(
            &cid,
            EditDiff {
                files: vec![from_replacement("x.rs", "a", "b")],
            },
        );
        let got = get(&cid).unwrap();
        assert_eq!(got.total_added() + got.total_removed(), 2);
    }
}
