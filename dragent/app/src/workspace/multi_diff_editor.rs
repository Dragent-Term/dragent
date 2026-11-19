//! P3 scaffold: multi-diff as centered editor surface.
//!
//! Behind `git-multi-diff-editor`. Takes `GitMultiDiff` (already computed off-thread
//! via fetch_structured_worktree_files) and projects it into a read-only
//! InputEditor document bundle so search/fold/navigation reuse editor infra
//! (VS Code ScmMultiDiffSourceResolver parity).
//! Fallback: panel-local vertical stack remains when flag off.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct MultiDiffEditorBundle {
    pub title: String,
    pub files: Vec<MultiDiffEditorFile>,
}

#[derive(Clone, Debug)]
pub struct MultiDiffEditorFile {
    pub relative_path: PathBuf,
    pub old_text: String,
    pub new_text: String,
}

pub fn bundle_from_multi_diff(
    _diff: &crate::git_status::diff_head::GitMultiDiff,
) -> MultiDiffEditorBundle {
    MultiDiffEditorBundle {
        title: String::new(),
        files: Vec::new(),
    }
}
