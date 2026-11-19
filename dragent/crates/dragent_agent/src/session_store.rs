//! JSONL-based session transcript persistence.
//!
//! Each session is one `.jsonl` file under:
//!   `~/.dragent/sessions/{hex8(sha256(cwd))}/{pane_uuid}/{session_id}.jsonl`
//!
//! Sessions are scoped to a pane UUID so that two terminal tabs pointing at the
//! same directory cannot see each other's history. On app restart the same pane
//! UUID is restored from SQLite, so continuity is preserved for that specific tab.
//!
//! Entries are written **before** the API call for user messages so that a
//! crash mid-stream doesn't lose the query, and after the Done event for
//! assistant replies.
//!
//! On tab open, `SessionStore::load_last()` finds the most-recently-modified
//! session for the same cwd + pane_uuid and reconstructs the LLM context
//! (`Vec<SessionMessage>`).

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use web_time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::block::AgentBlockKind;
use super::types::SessionMessage;

// ── Transcript entry ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEntry {
    User {
        content: String,
        timestamp: u64,
    },
    Assistant {
        content: String,
        timestamp: u64,
    },
    /// Inserted when old messages are compacted — carries the summary text.
    CompactBoundary {
        summary: String,
        messages_compacted: usize,
        timestamp: u64,
    },
}

impl TranscriptEntry {
    pub fn user(content: &str) -> Self {
        Self::User {
            content: content.to_string(),
            timestamp: now_secs(),
        }
    }
    pub fn assistant(content: &str) -> Self {
        Self::Assistant {
            content: content.to_string(),
            timestamp: now_secs(),
        }
    }
    pub fn compact(summary: &str, messages_compacted: usize) -> Self {
        Self::CompactBoundary {
            summary: summary.to_string(),
            messages_compacted,
            timestamp: now_secs(),
        }
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

pub struct SessionStore {
    pub session_id: Uuid,
    path: PathBuf,
}

impl SessionStore {
    /// Create a brand-new session store for `cwd` scoped to `pane_uuid`.
    /// The `.jsonl` file is created lazily on the first `append()` call.
    pub fn new(cwd: &Path, pane_uuid: Uuid) -> Self {
        let session_id = Uuid::new_v4();
        let path = session_path(cwd, pane_uuid, session_id);
        Self { session_id, path }
    }

    /// Load the most recently modified session for `cwd` + `pane_uuid`, if one exists.
    /// Returns `None` if no session exists or all sessions are empty.
    /// Sessions are isolated per pane so two tabs with the same CWD never share history.
    pub fn load_last(cwd: &Path, pane_uuid: Uuid) -> Option<LoadedSession> {
        let dir = sessions_dir_for_pane(cwd, pane_uuid);
        if !dir.exists() {
            return None;
        }

        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
            .collect();

        // Sort by last-modified time; newest last
        entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());

        for entry in entries.into_iter().rev() {
            let path = entry.path();
            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())?;
            let lines = read_entries(&path);
            if lines.is_empty() {
                continue;
            }

            let history = entries_to_history(&lines);
            let blocks = entries_to_resume_blocks(&lines);
            return Some(LoadedSession {
                session_id,
                path,
                history,
                blocks,
            });
        }
        None
    }

    /// Continue writing to a previously loaded session (used on resume).
    pub fn continue_from(path: PathBuf, session_id: Uuid) -> Self {
        Self { session_id, path }
    }

    /// Append one entry. Creates parent directories and the file on first call.
    pub fn append(&self, entry: &TranscriptEntry) {
        // Ensure directory exists
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            if let Ok(line) = serde_json::to_string(entry) {
                let mut w = BufWriter::new(file);
                let _ = w.write_all(line.as_bytes());
                let _ = w.write_all(b"\n");
            }
        }
    }
}

// ── Loaded session ────────────────────────────────────────────────────────────

/// Data returned when restoring a previous session.
pub struct LoadedSession {
    pub session_id: Uuid,
    pub path: PathBuf,
    /// Conversation history ready to send as LLM context.
    pub history: Vec<SessionMessage>,
    /// Blocks ready to push into the UI (UserQuery + AgentMessage pairs + banner).
    pub blocks: Vec<RestoredBlock>,
}

pub struct RestoredBlock {
    pub kind: AgentBlockKind,
    pub conversation_id: Uuid,
}

// ── Conversion helpers ────────────────────────────────────────────────────────

fn entries_to_history(entries: &[TranscriptEntry]) -> Vec<SessionMessage> {
    entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::User { content, .. } => Some(SessionMessage {
                role: "user".to_string(),
                content: content.clone(),
                attachments: Vec::new(),
            }),
            TranscriptEntry::Assistant { content, .. } => Some(SessionMessage {
                role: "assistant".to_string(),
                content: content.clone(),
                attachments: Vec::new(),
            }),
            TranscriptEntry::CompactBoundary { summary, .. } => Some(SessionMessage {
                role: "user".to_string(),
                content: format!("[Session context: {summary}]"),
                attachments: Vec::new(),
            }),
        })
        .collect()
}

fn entries_to_resume_blocks(entries: &[TranscriptEntry]) -> Vec<RestoredBlock> {
    let conv_id = Uuid::new_v4(); // synthetic; all resumed blocks share one conv
    let mut blocks = Vec::new();

    // Leading banner
    let first_ts = entries.first().and_then(|e| match e {
        TranscriptEntry::User { timestamp, .. }
        | TranscriptEntry::Assistant { timestamp, .. }
        | TranscriptEntry::CompactBoundary { timestamp, .. } => Some(*timestamp),
    });
    if let Some(ts) = first_ts {
        let date = format_unix_date(ts);
        blocks.push(RestoredBlock {
            kind: AgentBlockKind::AgentMessage {
                text: format!("↩ Session resumed from {date}"),
                is_streaming: false,
            },
            conversation_id: conv_id,
        });
    }

    for entry in entries {
        match entry {
            TranscriptEntry::User { content, .. } => {
                blocks.push(RestoredBlock {
                    kind: AgentBlockKind::UserQuery {
                        text: content.clone(),
                        author: None,
                        attached_blocks: Vec::new(),
                        // Transcript does not persist attachment display records
                        // yet, so restored history shows text only.
                        attachments: Vec::new(),
                        checkpoint: None,
                    },
                    conversation_id: conv_id,
                });
            }
            TranscriptEntry::Assistant { content, .. } => {
                blocks.push(RestoredBlock {
                    kind: AgentBlockKind::AgentMessage {
                        text: content.clone(),
                        is_streaming: false,
                    },
                    conversation_id: conv_id,
                });
            }
            TranscriptEntry::CompactBoundary {
                summary,
                messages_compacted,
                ..
            } => {
                blocks.push(RestoredBlock {
                    kind: AgentBlockKind::AgentMessage {
                        text: format!(
                            "⋯ [{messages_compacted} earlier messages summarized: {summary}]"
                        ),
                        is_streaming: false,
                    },
                    conversation_id: conv_id,
                });
            }
        }
    }
    blocks
}

// ── File paths ────────────────────────────────────────────────────────────────

fn sessions_dir_path(cwd: &Path) -> PathBuf {
    dragent_paths::home().join("sessions").join(cwd_hash(cwd))
}

/// Every stored conversation for the project at `cwd` (deleted with it).
pub fn project_sessions_dir(cwd: &Path) -> PathBuf {
    sessions_dir_path(cwd)
}

fn sessions_dir_for_pane(cwd: &Path, pane_uuid: Uuid) -> PathBuf {
    sessions_dir_path(cwd).join(pane_uuid.to_string())
}

fn session_path(cwd: &Path, pane_uuid: Uuid, id: Uuid) -> PathBuf {
    sessions_dir_for_pane(cwd, pane_uuid).join(format!("{id}.jsonl"))
}

fn read_entries(path: &Path) -> Vec<TranscriptEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn cwd_hash(cwd: &Path) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(cwd.to_string_lossy().as_bytes());
    hex::encode(&h.finalize()[..8])
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Prompt history ────────────────────────────────────────────────────────────

/// Append one entry to the global prompt history JSONL.
/// Path: `~/.dragent/history.jsonl`
pub fn log_prompt_history(query: &str, cwd: &std::path::Path) {
    use serde_json::json;
    let path = dragent_paths::home().join("history.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let entry = json!({
            "query": query,
            "cwd": cwd.to_string_lossy(),
            "timestamp": now_secs(),
        });
        if let Ok(line) = serde_json::to_string(&entry) {
            let mut w = std::io::BufWriter::new(file);
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
        }
    }
}

fn format_unix_date(ts: u64) -> String {
    // Simple formatting without chrono dependency here — use chrono via the
    // DateTime::from_timestamp utility.
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt: chrono::DateTime<chrono::Utc>| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "unknown date".to_string())
}
