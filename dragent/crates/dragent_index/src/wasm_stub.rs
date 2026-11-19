//! Wasm stub for the code index.
//!
//! The web viewer renders a live remote session and never indexes code. This
//! module provides the tiny surface the app touches — `get_manager()` returning
//! a no-op [`CodeIndexManager`], the status/result types, and `walk` — so every
//! index call site type-checks and returns "nothing indexed" on wasm.
#![allow(dead_code, unused_variables)]

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoIndexState {
    Indexing,
    Watching,
    Paused,
}

pub struct RepoIndexStatus {
    pub root: PathBuf,
    pub state: RepoIndexState,
    pub pending: usize,
    pub files: usize,
    pub chunks: usize,
    pub capped: bool,
    pub synced_secs_ago: Option<u64>,
}

pub struct SearchResult {
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub score: f32,
}

pub struct HybridHit {
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub vector_score: Option<f32>,
    pub bm25_score: Option<f32>,
    pub fused_score: f32,
    pub matched_symbol: Option<String>,
}

/// Zero-sized no-op manager. Indexing is unavailable on wasm, so every query
/// reports an empty, not-ready index.
pub struct CodeIndexManager {
    _private: (),
}

impl CodeIndexManager {
    pub fn set_auto_index(&self, _on: bool) {}
    pub fn set_enabled(&self, _on: bool) {}
    pub fn init_embedder_background(&self) {}
    pub fn set_respect_gitignore(&self, _respect: bool) {}
    pub fn reindex(&self, _root: &Path) {}
    pub fn remove_index(&self, _root: &Path) {}
    pub fn reindex_all(&self) {}
    pub fn register_repo(&self, _root: PathBuf, _max_files: usize) {}
    pub fn index_status(&self) -> Vec<RepoIndexStatus> {
        Vec::new()
    }
    pub fn search(&self, _repo_root: &Path, _query: &str, _top_k: usize) -> Vec<SearchResult> {
        Vec::new()
    }
    pub fn hybrid_search(&self, _repo_root: &Path, _query: &str, _top_k: usize) -> Vec<HybridHit> {
        Vec::new()
    }
    pub fn list_indexed_paths(&self, _repo_root: &Path) -> Option<(PathBuf, Vec<PathBuf>)> {
        None
    }
    pub fn is_ready(&self) -> bool {
        false
    }
}

static CODE_INDEX_MANAGER: CodeIndexManager = CodeIndexManager { _private: () };

pub fn get_manager() -> &'static CodeIndexManager {
    &CODE_INDEX_MANAGER
}

/// Filesystem walker stub — the viewer has no repositories to enumerate.
pub mod walk {
    use std::path::PathBuf;

    pub fn global_ignore_path() -> Option<PathBuf> {
        None
    }
}
