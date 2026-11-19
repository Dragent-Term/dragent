-- Durable editor recovery and workspace chrome. Draft content remains local.
CREATE TABLE working_copies (
    workspace_root TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    draft_text TEXT NOT NULL,
    base_hash INTEGER NOT NULL,
    base_size INTEGER NOT NULL,
    base_modified_unix_ms INTEGER,
    revision INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (workspace_root, relative_path)
);

CREATE INDEX working_copies_updated_idx ON working_copies(updated_at_unix_ms);

CREATE TABLE workspace_ui_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    state_json TEXT NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
);
