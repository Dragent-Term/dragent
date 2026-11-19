CREATE TABLE workspace_tool_outcomes (
    command_id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    runtime_run_id TEXT NOT NULL,
    runtime_execution_id TEXT NOT NULL,
    runtime_round BIGINT NOT NULL,
    provider_call_id TEXT NOT NULL,
    output TEXT,
    is_error INTEGER,
    source_message_id TEXT,
    accepted_at_unix_ms BIGINT NOT NULL,
    delivered_at_unix_ms BIGINT,
    terminal_reason TEXT,
    UNIQUE(runtime_run_id, runtime_execution_id, runtime_round, provider_call_id)
);

CREATE INDEX workspace_tool_outcomes_pending_idx
    ON workspace_tool_outcomes(delivered_at_unix_ms, terminal_reason, accepted_at_unix_ms);

CREATE INDEX workspace_tool_outcomes_reply_idx
    ON workspace_tool_outcomes(source_message_id);
