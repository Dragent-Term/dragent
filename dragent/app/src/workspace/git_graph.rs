//! P4 scaffold: commit graph (git log --graph).
//!
//! Behind `git-graph-history`. Reuses `diff_head::fetch_commit_diff` for selection.
//! Parses `git log --all --oneline --graph --pretty=format:%H%x1f%s%x1f%an%x1f%ae%x1f%ar%x1f%P`
//! into a DAG lane model for rendering.

use std::path::Path;

#[derive(Clone, Debug)]
pub struct GraphRow {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub email: String,
    pub relative_time: String,
    pub parents: Vec<String>,
    /// Graph lanes for this row: e.g. "* |", "| *", "|\"
    pub graph_cells: Vec<GraphCell>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphCell {
    Commit, // *
    Line,   // |
    Merge,  // \
    Empty,  // space
}

pub fn fetch_graph(_cwd: &Path, _limit: usize) -> Result<Vec<GraphRow>, String> {
    Err("git-graph-history not yet implemented (scaffold)".into())
}

pub fn parse_graph_line(_line: &str) -> Option<GraphRow> {
    None
}
