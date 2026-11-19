-- Append-only conversation event log: the single durable source of truth that
-- conversation projections (agent blocks, session transcripts) converge on.
-- One logical stream per conversation_key (pane UUID + conversation id);
-- readers resume from a seq watermark. Writers must go through
-- event_log::append so per-key sequencing stays gap-free.
CREATE TABLE conversation_events (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_key   TEXT    NOT NULL,
    seq                BIGINT  NOT NULL,
    kind               TEXT    NOT NULL,
    payload_json       TEXT    NOT NULL DEFAULT '{}',
    created_at_unix_ms BIGINT  NOT NULL,
    UNIQUE (conversation_key, seq)
);

CREATE INDEX idx_conversation_events_key_seq
    ON conversation_events (conversation_key, seq);
