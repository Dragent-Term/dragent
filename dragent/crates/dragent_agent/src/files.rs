//! Read-files result types shared by the engine core and the UI.
//! Single definitions — the app re-exports these paths.

use serde::{Deserialize, Serialize};

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct FileContent {
    pub path: PathBuf,
    /// `None` when the file was skipped (binary, too large, denied, or not found).
    pub content: Option<String>,
    /// Human-readable reason the file was skipped, if applicable.
    pub skip_reason: Option<String>,
}

/// Aggregated result of a ReadFiles tool execution.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadFilesResult {
    pub files: Vec<FileContent>,
}

impl ReadFilesResult {
    /// Returns the total number of bytes across all successfully read files.
    pub fn total_bytes(&self) -> usize {
        self.files
            .iter()
            .filter_map(|f| f.content.as_deref())
            .map(|s| s.len())
            .sum()
    }

    /// Returns only the successfully read files.
    pub fn successful(&self) -> impl Iterator<Item = &FileContent> {
        self.files.iter().filter(|f| f.content.is_some())
    }
}

// ── Tool result vocabulary (moved verbatim from the app tool executors) ───────

/// A single match found by the grep tool.
#[derive(Clone, Debug, PartialEq)]
pub struct GrepMatch {
    pub path: std::path::PathBuf,
    pub line_number: u64,
    pub line_text: String,
}

/// Aggregated result of a Grep tool execution.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrepResult {
    pub matches: Vec<GrepMatch>,
    /// `true` when more matches were available but were truncated at `MAX_GREP_MATCHES`.
    pub truncated: bool,
    /// The tool that was actually used: `"rg"` or `"grep"`.
    pub backend: String,
    /// Any error or warning message (e.g. if no backend was found).
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: std::path::PathBuf,
    pub is_dir: bool,
    pub size_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ListDirectoryResult {
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub enum EditFileOutcome {
    Edited {
        diff_hunks: Vec<super::diff::DiffHunk>,
    },
    OldStrNotFound {
        path: std::path::PathBuf,
        old_str: String,
    },
    PermissionDenied {
        path: std::path::PathBuf,
    },
    IoError {
        path: std::path::PathBuf,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub enum WriteFileOutcome {
    Written {
        diff_hunks: Vec<super::diff::DiffHunk>,
    },
    Created,
    PermissionDenied {
        path: std::path::PathBuf,
    },
    IoError {
        path: std::path::PathBuf,
        message: String,
    },
}

/// Saves a snapshot of file contents before an edit so it can be reverted.
#[derive(Clone, Debug, Default)]
pub struct FileCheckpoint {
    snapshots: Vec<(std::path::PathBuf, Option<String>)>, // None = file didn't exist
}

impl FileCheckpoint {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture the current state of `path` before modifying it.
    pub fn snapshot(&mut self, path: &std::path::Path) {
        let content = std::fs::read_to_string(path).ok();
        self.snapshots.push((path.to_path_buf(), content));
    }

    /// Revert all snapshotted files to their previous state.
    pub fn revert(&self) -> Vec<(std::path::PathBuf, Result<(), std::io::Error>)> {
        self.snapshots
            .iter()
            .map(|(path, content)| {
                let result = match content {
                    Some(c) => std::fs::write(path, c).map(|_| ()),
                    None => std::fs::remove_file(path).map(|_| ()),
                };
                (path.clone(), result)
            })
            .collect()
    }
}

/// A single semantic search hit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FragmentHit {
    pub path: std::path::PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchCodebaseResult {
    pub hits: Vec<FragmentHit>,
}

/// Union of every tool executor result, shared by the engine core and the Host.
#[derive(Clone, Debug)]
pub enum ToolResultKind {
    ReadFiles(ReadFilesResult),
    Grep(GrepResult),
    SearchCodebase(SearchCodebaseResult),
    WriteFile(WriteFileOutcome),
    EditFile(EditFileOutcome),
    ListDirectory(ListDirectoryResult),
}

/// Directory persisted by “Always allow”. A directory request grants itself;
/// a file request grants its containing directory so related reads do not
/// prompt one file at a time.
pub fn recommended_read_grant_root(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
}
