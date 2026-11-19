CREATE TABLE canonical_workspace_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    sequence BIGINT NOT NULL,
    envelope_json TEXT NOT NULL,
    created_at_unix_ms BIGINT NOT NULL,
    UNIQUE(work_item_id, sequence)
);

CREATE INDEX canonical_workspace_events_work_idx
    ON canonical_workspace_events(work_item_id, sequence);

CREATE TABLE canonical_workspace_command_outcomes (
    command_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    command_json TEXT NOT NULL,
    events_json TEXT NOT NULL,
    accepted_at_unix_ms BIGINT NOT NULL
);

CREATE INDEX canonical_workspace_commands_work_idx
    ON canonical_workspace_command_outcomes(work_item_id, accepted_at_unix_ms);
