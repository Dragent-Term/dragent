//! UI-independent document buffers, large-file capabilities, and compact review diffs.

use dragent_workspace_model::DocumentRef;
use similar::{ChangeTag, TextDiff};
use std::sync::Arc;

pub const MAX_DIFF_LINES: usize = 5_000;

/// These are feature-policy thresholds, not file-opening limits. A local
/// desktop editor should be able to load a file larger than either value when
/// the machine can provide the memory for it. They only tell expensive
/// subsystems when to use their reduced-work path.
pub const LARGE_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const LARGE_FILE_LINES: usize = 20_000;
pub const EXTREME_FILE_BYTES: usize = 128 * 1024 * 1024;
/// Keep language-server work bounded for generated files with a very long
/// physical line, following the same kind of guard used by mature editors.
pub const LANGUAGE_SERVER_MAX_LINE_CHARS: usize = 20_000;

/// Capabilities derived once for a document and shared by the editor layers.
/// Opening a file is intentionally independent from these feature switches:
/// basic text editing remains available while expensive IDE work can opt out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentCapabilities {
    pub byte_size: usize,
    pub line_count: usize,
    pub longest_line_chars: usize,
    pub large_file: bool,
    pub extreme_file: bool,
    pub syntax_highlighting: bool,
    pub language_server: bool,
    pub folding: bool,
    pub indentation_guides: bool,
    pub minimap: bool,
}

impl DocumentCapabilities {
    pub fn notice(&self) -> Option<&'static str> {
        self.large_file.then_some(
            "Large file mode: expensive language and rendering features are reduced for responsiveness.",
        )
    }
}

/// Derive feature policy from the loaded text. This is intentionally a
/// single source of truth so individual views do not grow unrelated size
/// checks and arbitrary hard limits.
pub fn capabilities_for_text(text: &str) -> DocumentCapabilities {
    let byte_size = text.len();
    let line_count = text.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let longest_line_chars = text
        .split('\n')
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let large_file = byte_size >= LARGE_FILE_BYTES
        || line_count >= LARGE_FILE_LINES
        || longest_line_chars > LANGUAGE_SERVER_MAX_LINE_CHARS;
    let extreme_file = byte_size >= EXTREME_FILE_BYTES || longest_line_chars >= 100_000;

    DocumentCapabilities {
        byte_size,
        line_count,
        longest_line_chars,
        large_file,
        extreme_file,
        // Large-file rendering is viewport based: only visible rows are
        // tokenized, so keep the existing syntax/theme palette even for
        // extreme files. The expensive whole-buffer caches remain disabled
        // independently below.
        syntax_highlighting: true,
        // Language servers auto-enable for EVERY document: the sync pipeline
        // is async and debounced (75ms, background worker), so a large buffer
        // no longer needs to be excluded. Large-file mode still reduces
        // folding, guides and the minimap.
        language_server: true,
        folding: !large_file,
        indentation_guides: !large_file,
        minimap: !large_file,
    }
}

#[derive(Clone, Debug)]
pub struct DocumentBuffer {
    document: DocumentRef,
    base_text: Arc<str>,
    text: String,
    revision: u64,
    saved_revision: u64,
    capabilities: DocumentCapabilities,
}

impl DocumentBuffer {
    pub fn new(document: DocumentRef, text: String) -> Self {
        let capabilities = capabilities_for_text(&text);
        Self {
            document,
            base_text: Arc::from(text.as_str()),
            text,
            revision: 0,
            saved_revision: 0,
            capabilities,
        }
    }

    pub fn document(&self) -> &DocumentRef {
        &self.document
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn base_text(&self) -> &Arc<str> {
        &self.base_text
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn capabilities(&self) -> DocumentCapabilities {
        self.capabilities
    }

    pub fn replace_text(&mut self, text: String) -> bool {
        if self.text == text {
            return false;
        }
        self.text = text;
        self.capabilities = capabilities_for_text(&self.text);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Replace a clean buffer with a newer snapshot that has already been
    /// written to disk. External refactors use this path so the editor updates
    /// immediately without turning the synchronized text into an unsaved
    /// working copy.
    pub fn reload_saved_text(&mut self, text: String) -> bool {
        if self.text == text {
            return false;
        }
        self.text = text;
        self.base_text = Arc::from(self.text.as_str());
        self.capabilities = capabilities_for_text(&self.text);
        self.revision = self.revision.wrapping_add(1);
        self.saved_revision = self.revision;
        true
    }

    pub fn revert(&mut self) {
        self.text.clear();
        self.text.push_str(&self.base_text);
        self.revision = self.revision.wrapping_add(1);
        self.saved_revision = self.revision;
        self.capabilities = capabilities_for_text(&self.text);
    }

    pub fn mark_saved(&mut self) {
        self.base_text = Arc::from(self.text.as_str());
        self.saved_revision = self.revision;
    }

    pub fn diff(&self) -> DocumentDiff {
        if self.capabilities.large_file && self.base_text.as_ref() != self.text.as_str() {
            // A full diff is an O(document-size) allocation-heavy operation.
            // Keep dirty-state semantics without making every keystroke in a
            // large file re-diff the entire document.
            return DocumentDiff {
                truncated: true,
                ..DocumentDiff::default()
            };
        }
        DocumentDiff::between(&self.base_text, &self.text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffHunkKind {
    Normal,
    Deleted,
}

impl Default for DiffHunkKind {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffHunk {
    pub kind: DiffHunkKind,
    pub lines: Vec<DiffLine>,
    pub old_start: Option<usize>,
    pub old_end: Option<usize>,
    pub new_start: Option<usize>,
    pub new_end: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentDiff {
    pub hunks: Vec<DiffHunk>,
    pub truncated: bool,
}

/// A run of equal source lines omitted between compact diff excerpts.
///
/// Coordinates are one-based source line numbers in both snapshots. The
/// region is display metadata only: expanding it must never edit either text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffUnchangedRegion {
    pub id: usize,
    pub old_start: usize,
    pub new_start: usize,
    pub line_count: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffRegionExpansion {
    /// Lines revealed from the edge nearest the preceding hunk.
    pub leading: usize,
    /// Lines revealed from the edge nearest the following hunk.
    pub trailing: usize,
}

/// One row in a Git-focused presentation of a real current buffer.
///
/// `Current` rows address the current snapshot and remain eligible for normal
/// editor hit-testing. `Deleted` and `Collapsed` rows are display-only and
/// must never be translated into editable buffer offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitDiffDisplayRow {
    Current {
        old_line: Option<usize>,
        line: usize,
        kind: DiffLineKind,
    },
    Deleted {
        old_line: usize,
        text: String,
    },
    Collapsed(DiffUnchangedRegion),
}

impl DocumentDiff {
    /// Project compact hunks into editor display rows without manufacturing a
    /// patch string. Expanded unchanged regions map back to real current lines.
    pub fn display_rows(
        &self,
        old_line_count: usize,
        new_line_count: usize,
        minimum_hidden_lines: usize,
        expanded_regions: &std::collections::HashSet<usize>,
    ) -> Vec<GitDiffDisplayRow> {
        let expansions = expanded_regions
            .iter()
            .map(|id| {
                (
                    *id,
                    DiffRegionExpansion {
                        leading: usize::MAX,
                        trailing: 0,
                    },
                )
            })
            .collect();
        self.display_rows_with_expansions(
            old_line_count,
            new_line_count,
            minimum_hidden_lines,
            &expansions,
        )
    }

    pub fn display_rows_with_expansions(
        &self,
        old_line_count: usize,
        new_line_count: usize,
        minimum_hidden_lines: usize,
        expansions: &std::collections::HashMap<usize, DiffRegionExpansion>,
    ) -> Vec<GitDiffDisplayRow> {
        let regions = self.unchanged_regions(old_line_count, new_line_count, minimum_hidden_lines);
        let mut regions = regions.into_iter().peekable();
        let mut rows = Vec::new();

        for (hunk_index, hunk) in self.hunks.iter().enumerate() {
            while regions.peek().is_some_and(|region| region.id <= hunk_index) {
                let region = regions.next().expect("peeked unchanged region");
                append_unchanged_region_rows(&mut rows, region, expansions);
            }

            rows.extend(hunk.lines.iter().filter_map(|line| match line.kind {
                DiffLineKind::Removed => Some(GitDiffDisplayRow::Deleted {
                    old_line: line.old_line?,
                    text: line.text.clone(),
                }),
                DiffLineKind::Context | DiffLineKind::Added => Some(GitDiffDisplayRow::Current {
                    old_line: line.old_line,
                    line: line.new_line?,
                    kind: line.kind,
                }),
            }));
        }

        for region in regions {
            append_unchanged_region_rows(&mut rows, region, expansions);
        }
        rows
    }
}

fn append_unchanged_region_rows(
    rows: &mut Vec<GitDiffDisplayRow>,
    region: DiffUnchangedRegion,
    expansions: &std::collections::HashMap<usize, DiffRegionExpansion>,
) {
    let expansion = expansions.get(&region.id).copied().unwrap_or_default();
    let leading = expansion.leading.min(region.line_count);
    let trailing = expansion
        .trailing
        .min(region.line_count.saturating_sub(leading));
    let push_current = |rows: &mut Vec<GitDiffDisplayRow>, offset: usize| {
        rows.push(GitDiffDisplayRow::Current {
            old_line: Some(region.old_start + offset),
            line: region.new_start + offset,
            kind: DiffLineKind::Context,
        });
    };
    for offset in 0..leading {
        push_current(rows, offset);
    }
    let hidden = region.line_count.saturating_sub(leading + trailing);
    if hidden > 0 {
        rows.push(GitDiffDisplayRow::Collapsed(DiffUnchangedRegion {
            id: region.id,
            old_start: region.old_start + leading,
            new_start: region.new_start + leading,
            line_count: hidden,
        }));
    }
    for offset in region.line_count.saturating_sub(trailing)..region.line_count {
        push_current(rows, offset);
    }
}

impl DocumentDiff {
    /// Cheap dirty marker used while a precise diff is being computed away
    /// from the input/UI path.
    pub fn pending() -> Self {
        Self {
            truncated: true,
            ..Self::default()
        }
    }

    pub fn between(base: &str, current: &str) -> Self {
        Self::between_with_context(base, current, 3)
    }

    pub fn between_with_context(base: &str, current: &str, context_lines: usize) -> Self {
        if base == current {
            return Self::default();
        }
        let diff = TextDiff::from_lines(base, current);
        let mut result = Self::default();
        let mut line_count = 0;
        for ops in diff.grouped_ops(context_lines) {
            let mut hunk = DiffHunk::default();
            for op in ops {
                for change in diff.iter_changes(&op) {
                    if line_count >= MAX_DIFF_LINES {
                        result.truncated = true;
                        break;
                    }
                    let kind = match change.tag() {
                        ChangeTag::Equal => DiffLineKind::Context,
                        ChangeTag::Insert => DiffLineKind::Added,
                        ChangeTag::Delete => DiffLineKind::Removed,
                    };
                    hunk.lines.push(DiffLine {
                        kind,
                        old_line: change.old_index().map(|line| line + 1),
                        new_line: change.new_index().map(|line| line + 1),
                        text: change.value().trim_end_matches('\n').to_string(),
                    });
                    line_count += 1;
                }
                if result.truncated {
                    break;
                }
            }
            if !hunk.lines.is_empty() {
                hunk.old_start = hunk.lines.iter().filter_map(|line| line.old_line).min();
                hunk.old_end = hunk.lines.iter().filter_map(|line| line.old_line).max();
                hunk.new_start = hunk.lines.iter().filter_map(|line| line.new_line).min();
                hunk.new_end = hunk.lines.iter().filter_map(|line| line.new_line).max();
                if hunk
                    .lines
                    .iter()
                    .all(|line| matches!(line.kind, DiffLineKind::Removed | DiffLineKind::Context))
                    && hunk
                        .lines
                        .iter()
                        .any(|line| line.kind == DiffLineKind::Removed)
                {
                    hunk.kind = DiffHunkKind::Deleted;
                }
                result.hunks.push(hunk);
            }
            if result.truncated {
                break;
            }
        }
        result
    }

    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty() && !self.truncated
    }

    /// Return the equal-line gaps hidden by this compact diff.
    ///
    /// A grouped diff already carries context lines inside each hunk. This
    /// method describes only the ranges *between* those excerpts so a view can
    /// render interactive placeholders and selectively reveal real source
    /// lines. Region ids are stable for a given hunk layout.
    pub fn unchanged_regions(
        &self,
        old_line_count: usize,
        new_line_count: usize,
        minimum_hidden_lines: usize,
    ) -> Vec<DiffUnchangedRegion> {
        if self.hunks.is_empty() {
            return Vec::new();
        }

        let bounds = self
            .hunks
            .iter()
            .map(|hunk| {
                let old_start = hunk.lines.iter().filter_map(|line| line.old_line).min();
                let old_end = hunk.lines.iter().filter_map(|line| line.old_line).max();
                let new_start = hunk.lines.iter().filter_map(|line| line.new_line).min();
                let new_end = hunk.lines.iter().filter_map(|line| line.new_line).max();
                (old_start, old_end, new_start, new_end)
            })
            .collect::<Vec<_>>();

        let mut regions = Vec::new();
        let mut old_cursor = 1usize;
        let mut new_cursor = 1usize;
        for (index, (old_start, old_end, new_start, new_end)) in bounds.iter().enumerate() {
            let old_gap = old_start.unwrap_or(old_cursor).saturating_sub(old_cursor);
            let new_gap = new_start.unwrap_or(new_cursor).saturating_sub(new_cursor);
            let line_count = old_gap.min(new_gap);
            if line_count >= minimum_hidden_lines {
                regions.push(DiffUnchangedRegion {
                    id: index,
                    old_start: old_cursor,
                    new_start: new_cursor,
                    line_count,
                });
            }
            old_cursor = old_end.map_or(old_cursor, |line| line.saturating_add(1));
            new_cursor = new_end.map_or(new_cursor, |line| line.saturating_add(1));
        }

        let old_gap = old_line_count.saturating_add(1).saturating_sub(old_cursor);
        let new_gap = new_line_count.saturating_add(1).saturating_sub(new_cursor);
        let line_count = old_gap.min(new_gap);
        if line_count >= minimum_hidden_lines {
            regions.push(DiffUnchangedRegion {
                id: self.hunks.len(),
                old_start: old_cursor,
                new_start: new_cursor,
                line_count,
            });
        }
        regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn document() -> DocumentRef {
        DocumentRef::from_relative_path(PathBuf::from("src/lib.rs")).unwrap()
    }

    #[test]
    fn tracks_edits_and_compact_diff() {
        let mut buffer = DocumentBuffer::new(document(), "one\ntwo\nthree\n".into());
        buffer.replace_text("one\nchanged\nthree\n".into());
        let diff = buffer.diff();
        assert!(buffer.is_dirty());
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Removed && line.text == "two"));
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Added && line.text == "changed"));
    }

    #[test]
    fn reloading_a_saved_snapshot_keeps_the_buffer_clean() {
        let mut buffer = DocumentBuffer::new(document(), "old\n".into());

        assert!(buffer.reload_saved_text("new\n".into()));
        assert_eq!(buffer.text(), "new\n");
        assert_eq!(buffer.base_text().as_ref(), "new\n");
        assert!(!buffer.is_dirty());
        assert!(buffer.diff().is_empty());
    }

    #[test]
    fn full_context_diff_retains_unchanged_file_rows() {
        let base = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
        let current = "one\ntwo\nthree\nchanged\nfive\nsix\nseven\n";
        let diff = DocumentDiff::between_with_context(base, current, 7);
        assert_eq!(diff.hunks.len(), 1);
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Context && line.text == "one"));
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Context && line.text == "seven"));
    }

    #[test]
    fn compact_diff_reports_expandable_unchanged_regions() {
        let base = (1..=30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let current = base.replace("line 15\n", "changed 15\n");
        let diff = DocumentDiff::between(&base, &current);
        let regions = diff.unchanged_regions(30, 30, 1);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].old_start, 1);
        assert_eq!(regions[0].new_start, 1);
        assert_eq!(regions[0].line_count, 11);
        assert_eq!(regions[1].old_start, 19);
        assert_eq!(regions[1].new_start, 19);
        assert_eq!(regions[1].line_count, 12);
    }

    #[test]
    fn unchanged_region_coordinates_account_for_insertions() {
        let base = (1..=20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let current = base.replace("line 10\n", "inserted\nline 10\n");
        let diff = DocumentDiff::between(&base, &current);
        let regions = diff.unchanged_regions(20, 21, 1);
        let trailing = regions.last().unwrap();
        assert_eq!(trailing.old_start + 1, trailing.new_start);
        assert_eq!(trailing.line_count, 8);
    }

    #[test]
    fn git_display_rows_keep_deleted_content_virtual() {
        let diff = DocumentDiff::between("one\ntwo\nthree\n", "one\nthree\n");
        let rows = diff.display_rows(3, 2, 1, &Default::default());
        assert!(rows.iter().any(|row| matches!(
            row,
            GitDiffDisplayRow::Deleted { old_line: 2, text } if text == "two"
        )));
        assert!(!rows.iter().any(|row| matches!(
            row,
            GitDiffDisplayRow::Current {
                old_line: _,
                line: 2,
                kind: DiffLineKind::Removed
            }
        )));
    }

    #[test]
    fn expanding_git_region_restores_real_current_line_mapping() {
        let base = (1..=30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let current = base.replace("line 15\n", "changed 15\n");
        let diff = DocumentDiff::between(&base, &current);
        let compact = diff.display_rows(30, 30, 1, &Default::default());
        let region = compact
            .iter()
            .find_map(|row| match row {
                GitDiffDisplayRow::Collapsed(region) => Some(region.clone()),
                _ => None,
            })
            .expect("collapsed prefix");
        let expanded = diff.display_rows(30, 30, 1, &std::collections::HashSet::from([region.id]));
        assert!(expanded.iter().any(|row| matches!(
            row,
            GitDiffDisplayRow::Current {
                old_line: Some(_),
                line,
                kind: DiffLineKind::Context
            }
                if *line == region.new_start
        )));
    }

    #[test]
    fn partial_git_region_expansion_reveals_bounded_edges() {
        let base = (1..=60)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let current = base.replace("line 30\n", "changed 30\n");
        let diff = DocumentDiff::between(&base, &current);
        let region = diff.unchanged_regions(60, 60, 1)[0].clone();
        let expansions = std::collections::HashMap::from([(
            region.id,
            DiffRegionExpansion {
                leading: 3,
                trailing: 2,
            },
        )]);
        let rows = diff.display_rows_with_expansions(60, 60, 1, &expansions);
        let remaining = rows
            .iter()
            .find_map(|row| match row {
                GitDiffDisplayRow::Collapsed(hidden) if hidden.id == region.id => Some(hidden),
                _ => None,
            })
            .expect("partially hidden region remains");
        assert_eq!(remaining.line_count, region.line_count - 5);
        assert!(rows.iter().any(|row| matches!(
            row,
            GitDiffDisplayRow::Current { line, .. } if *line == region.new_start
        )));
        assert!(rows.iter().any(|row| matches!(
            row,
            GitDiffDisplayRow::Current { line, .. }
                if *line == region.new_start + region.line_count - 1
        )));
    }

    #[test]
    fn accepts_large_buffers_and_reduces_expensive_features() {
        let text = "x".repeat(LARGE_FILE_BYTES + 1);
        let buffer = DocumentBuffer::new(document(), text);
        assert!(buffer.capabilities().large_file);
        // Language servers auto-enable for EVERY document: the sync pipeline
        // is async and debounced, so large files keep LSP while losing the
        // expensive whole-buffer features.
        assert!(buffer.capabilities().language_server);
        assert!(!buffer.capabilities().folding);
    }

    #[test]
    fn long_lines_activate_viewport_rendering_without_losing_language_work() {
        let text = "x".repeat(LANGUAGE_SERVER_MAX_LINE_CHARS + 1);
        let capabilities = capabilities_for_text(&text);
        assert!(capabilities.large_file);
        assert!(capabilities.language_server);
        assert_eq!(capabilities.byte_size, text.len());
    }

    #[test]
    fn many_lines_activate_viewport_rendering_before_memory_size_does() {
        let text = "x\n".repeat(LARGE_FILE_LINES);
        assert!(text.len() < LARGE_FILE_BYTES);
        let capabilities = capabilities_for_text(&text);
        assert!(capabilities.large_file);
        assert_eq!(capabilities.line_count, LARGE_FILE_LINES + 1);
        assert!(capabilities.syntax_highlighting);
    }

    #[test]
    fn revert_clears_dirty_state() {
        let mut buffer = DocumentBuffer::new(document(), "before".into());
        buffer.replace_text("after".into());
        buffer.revert();
        assert_eq!(buffer.text(), "before");
        assert!(!buffer.is_dirty());
    }

    /// Development-only scaling probe for the synchronous model work that was
    /// historically performed after every editor change.
    #[test]
    #[ignore = "manual editor performance profile"]
    fn profile_replace_and_full_diff() {
        for target_bytes in [100_000usize, 1_000_000, 5_000_000] {
            let unit = "export const value = { nested: [1, 2, 3], enabled: true };\n";
            let source = unit.repeat(target_bytes.div_ceil(unit.len()));
            let mut buffer = DocumentBuffer::new(document(), source);
            let mut edited = buffer.text().to_owned();
            edited.insert(edited.len() / 2, 'x');

            let replace_started = Instant::now();
            assert!(buffer.replace_text(edited));
            let replace_elapsed = replace_started.elapsed();
            let diff_started = Instant::now();
            let diff = buffer.diff();
            let diff_elapsed = diff_started.elapsed();
            eprintln!(
                "[editor-profile] model bytes={} replace_ms={:.3} diff_ms={:.3} hunks={} truncated={}",
                buffer.text().len(),
                replace_elapsed.as_secs_f64() * 1_000.0,
                diff_elapsed.as_secs_f64() * 1_000.0,
                diff.hunks.len(),
                diff.truncated
            );
        }
    }
}
