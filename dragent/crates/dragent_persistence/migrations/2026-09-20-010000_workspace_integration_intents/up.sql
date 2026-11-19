CREATE TABLE workspace_integration_intents (
    attempt_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    command_id TEXT NOT NULL UNIQUE,
    command_json TEXT NOT NULL,
    integration_plan_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('prepared', 'blocked', 'completed')),
    created_at_unix_ms BIGINT NOT NULL,
    completed_at_unix_ms BIGINT,
    last_error TEXT
);

CREATE INDEX workspace_integration_intents_recovery_idx
    ON workspace_integration_intents(state, created_at_unix_ms);
