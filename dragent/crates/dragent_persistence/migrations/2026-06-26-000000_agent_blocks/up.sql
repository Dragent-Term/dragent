-- Agent block persistence: store every agent block keyed by pane UUID so
-- blocks survive app close/reopen without relying solely on the JSONL transcript.

CREATE TABLE agent_blocks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    pane_uuid       BLOB NOT NULL,
    conversation_id BLOB NOT NULL,
    kind_json       TEXT NOT NULL,
    sort_order      INTEGER NOT NULL
);

CREATE INDEX idx_agent_blocks_pane_uuid ON agent_blocks(pane_uuid);
CREATE INDEX idx_agent_blocks_sort      ON agent_blocks(pane_uuid, sort_order);
