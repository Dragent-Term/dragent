-- +goose Up
CREATE TABLE agent_tasks (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running','awaiting_approval','done','failed','cancelled')),
    executor_id UUID,
    lease_expires_at TIMESTAMPTZ,
    last_event_seq BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX agent_tasks_user_created_idx ON agent_tasks (user_id, created_at DESC);
CREATE INDEX agent_tasks_expired_lease_idx ON agent_tasks (lease_expires_at)
    WHERE status IN ('running','awaiting_approval');

CREATE TABLE agent_task_events (
    task_id UUID NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    event JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (task_id, seq)
);

CREATE TABLE agent_tool_results (
    task_id UUID NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
    call_id TEXT NOT NULL,
    output TEXT NOT NULL,
    is_error BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (task_id, call_id)
);

-- +goose Down
DROP TABLE agent_tool_results;
DROP TABLE agent_task_events;
DROP TABLE agent_tasks;
