//! Append-only conversation event log.
//!
//! This is the durability convergence point: instead of N stores each keeping
//! their own version of a conversation (agent blocks for rendering, JSONL for
//! LLM history, run files for resume), producers append full-fidelity events
//! here keyed by conversation, and each read model projects what it needs.
//! One mechanism, partitioned keys — the same isolation the pane-UUID and
//! work-item scoping already provides, without a second source of truth.
//!
//! Convergence map (migrate in this order; each step keeps old readers green):
//! 1. (done here) Envelope + writer/reader + migration. No producers yet.
//! 2. Emit block-save events alongside `agent_blocks` writes (dual-write with
//!    the SQLite row as authority until readers migrate).
//! 3. Rebuild session transcripts and block snapshots from the log; retire the
//!    JSONL writer once restore paths read the log.
//! 4. Backfill note: history predating this table stays in the legacy stores;
//!    readers must union legacy rows below the log's per-key floor until a
//!    retention cutoff passes.

use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

/// One durable conversation event.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationEvent {
    pub id: i64,
    pub conversation_key: String,
    pub seq: i64,
    pub kind: String,
    pub payload_json: String,
    pub created_at_unix_ms: i64,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = crate::schema::conversation_events)]
struct NewConversationEvent<'a> {
    conversation_key: &'a str,
    seq: i64,
    kind: &'a str,
    payload_json: &'a str,
    created_at_unix_ms: i64,
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Append an event to a conversation stream. Sequence numbers are gap-free
/// per key (transactional max+1), so readers can resume from a watermark.
pub fn append(
    conn: &mut SqliteConnection,
    key: &str,
    event_kind: &str,
    payload: &str,
) -> Result<i64> {
    use crate::schema::conversation_events::dsl::*;
    conn.transaction::<i64, anyhow::Error, _>(|c| {
        let max_seq: Option<i64> = conversation_events
            .filter(conversation_key.eq(key))
            .select(diesel::dsl::max(seq))
            .first(c)?;
        let next = max_seq.unwrap_or(0) + 1;
        diesel::insert_into(conversation_events)
            .values(NewConversationEvent {
                conversation_key: key,
                seq: next,
                kind: event_kind,
                payload_json: payload,
                created_at_unix_ms: now_unix_ms(),
            })
            .execute(c)?;
        Ok(next)
    })
    .context("append conversation event")
}

/// Read a conversation slice in seq order, resuming after `after_seq`.
pub fn read_range(
    conn: &mut SqliteConnection,
    key: &str,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<ConversationEvent>> {
    use crate::schema::conversation_events::dsl::*;
    let rows: Vec<(i32, String, i64, String, String, i64)> = conversation_events
        .filter(conversation_key.eq(key).and(seq.gt(after_seq)))
        .order(seq.asc())
        .limit(limit)
        .select((
            id,
            conversation_key,
            seq,
            kind,
            payload_json,
            created_at_unix_ms,
        ))
        .load(conn)
        .context("read conversation events")?;
    Ok(rows
        .into_iter()
        .map(
            |(row_id, row_key, row_seq, row_kind, row_payload, row_ts)| ConversationEvent {
                id: row_id as i64,
                conversation_key: row_key,
                seq: row_seq,
                kind: row_kind,
                payload_json: row_payload,
                created_at_unix_ms: row_ts,
            },
        )
        .collect())
}

/// Highest seq for a key, or 0 when the stream is empty. Readers store this
/// as their watermark and pass it back as `after_seq`.
pub fn latest_seq(conn: &mut SqliteConnection, key: &str) -> Result<i64> {
    use crate::schema::conversation_events::dsl::*;
    let max_seq: Option<i64> = conversation_events
        .filter(conversation_key.eq(key))
        .select(diesel::dsl::max(seq))
        .first(conn)
        .context("latest conversation seq")?;
    Ok(max_seq.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::setup_connection;
    use tempfile::tempdir;

    fn test_conn() -> (tempfile::TempDir, SqliteConnection) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("event-log.sqlite");
        let conn = setup_connection(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn append_sequences_gap_free_per_key() {
        let (_dir, mut conn) = test_conn();
        assert_eq!(append(&mut conn, "a", "user", "{}").unwrap(), 1);
        assert_eq!(append(&mut conn, "a", "assistant", "{}").unwrap(), 2);
        // Independent per-key streams.
        assert_eq!(append(&mut conn, "b", "user", "{}").unwrap(), 1);
        assert_eq!(latest_seq(&mut conn, "a").unwrap(), 2);
        assert_eq!(latest_seq(&mut conn, "b").unwrap(), 1);
        assert_eq!(latest_seq(&mut conn, "missing").unwrap(), 0);
    }

    #[test]
    fn read_range_resumes_from_watermark_in_order() {
        let (_dir, mut conn) = test_conn();
        for i in 0..5 {
            append(&mut conn, "a", "tool", &format!("{{\"i\":{i}}}")).unwrap();
        }
        let page = read_range(&mut conn, "a", 2, 10).unwrap();
        assert_eq!(page.len(), 3);
        assert_eq!(page[0].seq, 3);
        assert_eq!(page[2].seq, 5);
        assert_eq!(page[0].payload_json, "{\"i\":2}");
        let capped = read_range(&mut conn, "a", 0, 2).unwrap();
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[1].seq, 2);
    }
}
