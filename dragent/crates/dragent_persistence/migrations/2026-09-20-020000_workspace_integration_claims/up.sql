ALTER TABLE workspace_integration_intents RENAME TO workspace_integration_intents_without_claims;

CREATE TABLE workspace_integration_intents (
    attempt_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    command_id TEXT NOT NULL UNIQUE,
    command_json TEXT NOT NULL,
    integration_plan_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('prepared', 'applying', 'blocked', 'completed')),
    claim_owner TEXT,
    claim_expires_at_unix_ms BIGINT,
    created_at_unix_ms BIGINT NOT NULL,
    completed_at_unix_ms BIGINT,
    last_error TEXT
);

INSERT INTO workspace_integration_intents (
    attempt_id, project_id, work_item_id, command_id, command_json,
    integration_plan_json, state, claim_owner, claim_expires_at_unix_ms,
    created_at_unix_ms, completed_at_unix_ms, last_error
)
SELECT attempt_id, project_id, work_item_id, command_id, command_json,
       integration_plan_json, state, NULL, NULL,
       created_at_unix_ms, completed_at_unix_ms, last_error
FROM workspace_integration_intents_without_claims;

DROP TABLE workspace_integration_intents_without_claims;

CREATE INDEX workspace_integration_intents_recovery_idx
    ON workspace_integration_intents(state, created_at_unix_ms);

CREATE UNIQUE INDEX workspace_integration_intents_work_fence_idx
    ON workspace_integration_intents(work_item_id)
    WHERE state <> 'completed';
