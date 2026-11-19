//! Versioned wire types shared by the code-index client and service.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SERVICE_ARG: &str = "--dragent-index-service";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub version: u32,
    pub token: String,
    pub client_id: String,
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub version: u32,
    pub result: Result<Response, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Ping,
    Heartbeat,
    SetEnabled(bool),
    SetAutoIndex(bool),
    SetRespectGitignore(bool),
    RegisterRepo {
        root: PathBuf,
        max_files: usize,
    },
    Reindex {
        root: PathBuf,
    },
    RemoveIndex {
        root: PathBuf,
    },
    ReindexAll,
    IndexStatus,
    Search {
        repo_root: PathBuf,
        query: String,
        top_k: usize,
    },
    HybridSearch {
        repo_root: PathBuf,
        query: String,
        top_k: usize,
    },
    ListIndexedPaths {
        repo_root: PathBuf,
    },
    IsReady,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Pong,
    Ack,
    IndexStatus(Vec<RepoIndexStatus>),
    Search(Vec<SearchResult>),
    HybridSearch(Vec<HybridResult>),
    IndexedPaths(Option<(PathBuf, Vec<PathBuf>)>),
    Ready(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoIndexState {
    Indexing,
    Watching,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndexStatus {
    pub root: PathBuf,
    pub state: RepoIndexState,
    pub pending: usize,
    pub files: usize,
    pub chunks: usize,
    pub capped: bool,
    pub synced_secs_ago: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridResult {
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub vector_score: Option<f32>,
    pub bm25_score: Option<f32>,
    pub fused_score: f32,
    pub matched_symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
}
