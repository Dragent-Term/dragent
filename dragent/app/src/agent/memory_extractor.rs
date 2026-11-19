//! Auto-memory extraction: after a session reaches ≥20 messages + ≥3 tool calls,
//! trigger a background LLM call that extracts typed facts and saves them to the
//! project's persistent memory store.
//!
//! Runs entirely in a background thread — never blocks the UI.

use std::path::Path;

use crate::agent::controller::SessionMessage;
use crate::api::API_CLIENT;

/// Cheap, client-side check: does this user message look like an explicit
/// instruction on HOW the agent should work, or a correction of its approach?
///
/// When true, we capture the rule into persistent memory eagerly — without
/// waiting for the deep-session (≥20 message) extraction threshold — so a
/// preference stated in a short session ("always run tests first", "don't add
/// comments") still becomes a durable rule. False positives are harmless: an
/// extra best-effort background extraction that yields nothing memorable.
/// False negatives simply fall back to the normal threshold.
pub fn looks_like_directive(msg: &str) -> bool {
    let m = msg.to_lowercase();
    // Standing-rule phrasing.
    const RULE_CUES: &[&str] = &[
        "always ",
        "never ",
        "from now on",
        "going forward",
        "in the future",
        "in future",
        "each time",
        "every time",
        "whenever ",
        "by default",
        "make sure",
        "be sure to",
        "remember to",
        "remember that",
        "please always",
        "please never",
        "please don't",
        "please do not",
        "don't ",
        "do not ",
        "you should always",
        "you must",
        "i want you to",
        "i'd like you to",
        "instead of",
        "instead,",
        "stop ",
        ", not ",
    ];
    // Correction / dissatisfaction phrasing.
    const CORRECTION_CUES: &[&str] = &[
        "that's not",
        "thats not",
        "not what i",
        "that is not what",
        "i said",
        "i asked you",
        "i told you",
        "why did you",
        "you were supposed",
        "you should have",
        "no, ",
        "actually,",
        "don't do that",
        "stop doing",
        "that's wrong",
        "that is wrong",
        "revert",
        "undo that",
        "not like that",
    ];
    RULE_CUES.iter().any(|c| m.contains(c)) || CORRECTION_CUES.iter().any(|c| m.contains(c))
}

/// Try to extract memories from `history` and persist them to the memory store
/// for `cwd`. Silently no-ops on any error — extraction is best-effort.
pub fn try_extract_and_save(model_name: &str, history: &[SessionMessage], cwd: &Path) {
    // Skip if not authenticated — avoids a failing API call and log noise.
    if API_CLIENT.get_access_token().is_err() {
        return;
    }

    let msgs: Vec<serde_json::Value> = history
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();

    let body = serde_json::json!({
        "model_name": model_name,
        "messages":   msgs,
    });

    let resp = match API_CLIENT.post("/api/v1/agent/extract", &body, 90) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[memory-extract] API call failed: {e}");
            return;
        }
    };

    #[derive(serde::Deserialize)]
    struct ExtractResp {
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

    let result: ExtractResp = match serde_json::from_value(resp) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[memory-extract] parse response: {e}");
            return;
        }
    };

    let store = crate::agent::memory_store::MemoryStore::for_cwd(cwd);
    let mut saved = 0usize;
    for mem in &result.memories {
        if store
            .save_with_metadata(
                &mem.name,
                &mem.description,
                &mem.kind,
                &mem.body,
                crate::agent::memory_store::MemoryMetadata::extracted(
                    &mem.evidence,
                    mem.confidence,
                    &mem.kind,
                ),
            )
            .is_ok()
        {
            saved += 1;
        }
    }

    log::info!(
        "[memory-extract] saved {saved}/{} memories for {:?}",
        result.memories.len(),
        cwd
    );
}

#[cfg(test)]
mod tests {
    use super::looks_like_directive;

    #[test]
    fn detects_explicit_instructions() {
        for m in [
            "Always run the tests before committing",
            "From now on, don't add comments to the code",
            "Please never force-push to main",
            "Use tabs, not spaces",
            "Make sure to update the changelog each time",
            "I want you to keep responses short",
        ] {
            assert!(looks_like_directive(m), "should be a directive: {m}");
        }
    }

    #[test]
    fn detects_corrections() {
        for m in [
            "No, that's not what I asked for",
            "Why did you delete that file?",
            "Actually, use the other approach instead",
            "revert that change",
            "you were supposed to run it first",
        ] {
            assert!(looks_like_directive(m), "should be a correction: {m}");
        }
    }

    #[test]
    fn ignores_ordinary_requests() {
        for m in [
            "Add a login page",
            "What does this function do?",
            "Fix the failing test in auth.rs",
            "Can you explain the architecture?",
        ] {
            assert!(!looks_like_directive(m), "should NOT be a directive: {m}");
        }
    }
}
