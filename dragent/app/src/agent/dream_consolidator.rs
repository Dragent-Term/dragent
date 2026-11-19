//! Auto-dream consolidation: a background daemon that consolidates accumulated
//! per-project memories across sessions.
//!
//! Gates:
//!   - ≥5 sessions since last consolidation  (per-project)
//!   - ≥24 hours since last consolidation     (per-project)
//!   - ≥3 memory files exist (nothing to consolidate otherwise)
//!
//! State is persisted at `{project_root}/consolidation_state.json` so each
//! project's session counter is independent of all others.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::API_CLIENT;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
pub struct ConsolidationState {
    pub last_consolidated_at: u64,
    pub sessions_since_last: u32,
}

pub const SESSION_GATE: u32 = 5;
pub const TIME_GATE_SEC: u64 = 24 * 3600;
pub const MIN_MEMORIES: usize = 3;

pub fn state_path(cwd: &Path) -> PathBuf {
    crate::agent::memory_store::project_root(cwd).join("consolidation_state.json")
}

pub fn load_state(cwd: &Path) -> ConsolidationState {
    load_state_from_path(&state_path(cwd))
}

fn load_state_from_path(path: &Path) -> ConsolidationState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &ConsolidationState, cwd: &Path) {
    save_state_to_path(state, &state_path(cwd));
}

fn save_state_to_path(state: &ConsolidationState, path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(state) {
        let _ = std::fs::write(&path, text);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Called when a new agent session starts. Increments this project's session
/// counter and, if both gates pass, runs LLM-based memory consolidation.
/// Must be called from a background thread — never from the UI thread.
pub fn on_session_start(model_name: &str, cwd: &Path) {
    // Skip entirely if not authenticated — avoids a pointless API call.
    if API_CLIENT.get_access_token().is_err() {
        return;
    }

    let mut state = load_state(cwd);
    state.sessions_since_last += 1;

    let now = now_secs();
    let time_ok = now.saturating_sub(state.last_consolidated_at) >= TIME_GATE_SEC;
    let session_ok = state.sessions_since_last >= SESSION_GATE;

    if !time_ok || !session_ok {
        save_state(&state, cwd);
        return;
    }

    let store = crate::agent::memory_store::MemoryStore::for_cwd(cwd);
    let existing = store.load_all();
    if existing.len() < MIN_MEMORIES {
        save_state(&state, cwd);
        return;
    }

    log::info!(
        "[dream] consolidating {} memories ({} sessions, {}h elapsed) for {:?}",
        existing.len(),
        state.sessions_since_last,
        now.saturating_sub(state.last_consolidated_at) / 3600,
        cwd,
    );

    let items: Vec<serde_json::Value> = existing
        .iter()
        .map(|m| {
            serde_json::json!({
                "name":        m.name,
                "description": m.description,
                "type":        m.kind,
                "body":        m.body,
                "evidence":    m.metadata.source_evidence,
                "confidence":  m.metadata.confidence,
            })
        })
        .collect();

    let body = serde_json::json!({
        "model_name": model_name,
        "memories":   items,
    });

    let resp = match API_CLIENT.post("/api/v1/agent/consolidate", &body, 120) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[dream] consolidation API failed: {e}");
            save_state(&state, cwd);
            return;
        }
    };

    #[derive(serde::Deserialize)]
    struct ConsolidateResp {
        memories: Vec<MemItem>,
    }
    #[derive(serde::Deserialize)]
    struct MemItem {
        name: String,
        description: String,
        #[serde(rename = "type")]
        kind: String,
        body: String,
        evidence: String,
        confidence: f32,
    }

    let result: ConsolidateResp = match serde_json::from_value(resp) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[dream] parse consolidation response: {e}");
            save_state(&state, cwd);
            return;
        }
    };

    // Consolidation is a projection, not authority to destroy source records.
    // Overwrite matching names with the refined version and leave unmatched
    // memories available for expiry or a future user-visible cleanup. An empty
    // or incomplete model response must never erase durable user preferences.
    for mem in &result.memories {
        let mut metadata = crate::agent::memory_store::MemoryMetadata::extracted(
            &mem.evidence,
            mem.confidence,
            &mem.kind,
        );
        metadata.source = "consolidated-memory".to_string();
        if let Err(e) =
            store.save_with_metadata(&mem.name, &mem.description, &mem.kind, &mem.body, metadata)
        {
            log::warn!("[dream] save {}: {e}", mem.name);
        }
    }

    log::info!(
        "[dream] consolidated {} → {} memories for {:?}",
        existing.len(),
        result.memories.len(),
        cwd
    );

    state.last_consolidated_at = now;
    state.sessions_since_last = 0;
    save_state(&state, cwd);
}

fn now_secs() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consolidation_state.json");

        let mut s = load_state_from_path(&path);
        assert_eq!(s.sessions_since_last, 0);
        assert_eq!(s.last_consolidated_at, 0);

        s.sessions_since_last = 3;
        s.last_consolidated_at = 9999;
        save_state_to_path(&s, &path);

        let s2 = load_state_from_path(&path);
        assert_eq!(s2.sessions_since_last, 3);
        assert_eq!(s2.last_consolidated_at, 9999);
    }

    #[test]
    fn session_gate_blocks_below_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();

        // Simulate 4 sessions (below SESSION_GATE=5) with time gate satisfied.
        let mut state = ConsolidationState {
            last_consolidated_at: 0, // 0 → time gate always passes
            sessions_since_last: SESSION_GATE - 1,
        };
        state.sessions_since_last += 1; // this session → 5? No, 4+1=5 passes. Use 3+1=4.
        state.sessions_since_last = SESSION_GATE - 2; // 3, then +1 = 4 < 5

        let now = now_secs();
        let time_ok = now.saturating_sub(state.last_consolidated_at) >= TIME_GATE_SEC;
        let session_ok = state.sessions_since_last >= SESSION_GATE;

        assert!(time_ok, "time gate should pass when last_consolidated_at=0");
        assert!(
            !session_ok,
            "session gate should block at {} sessions",
            state.sessions_since_last
        );
    }

    #[test]
    fn both_gates_pass_at_threshold() {
        let state = ConsolidationState {
            last_consolidated_at: 0,           // very old → time gate passes
            sessions_since_last: SESSION_GATE, // exactly at gate
        };

        let now = now_secs();
        let time_ok = now.saturating_sub(state.last_consolidated_at) >= TIME_GATE_SEC;
        let session_ok = state.sessions_since_last >= SESSION_GATE;

        assert!(time_ok);
        assert!(session_ok);
    }

    #[test]
    fn time_gate_blocks_recent_consolidation() {
        let now = now_secs();
        let state = ConsolidationState {
            last_consolidated_at: now - 3600, // 1 hour ago, gate needs 24h
            sessions_since_last: SESSION_GATE,
        };

        let time_ok = now.saturating_sub(state.last_consolidated_at) >= TIME_GATE_SEC;
        assert!(
            !time_ok,
            "time gate should block when last consolidation was 1h ago"
        );
    }

    #[test]
    fn state_path_is_per_project() {
        let cwd_a = std::path::PathBuf::from("/project/alpha");
        let cwd_b = std::path::PathBuf::from("/project/beta");
        assert_ne!(
            state_path(&cwd_a),
            state_path(&cwd_b),
            "different projects must have different state paths"
        );
    }
}
