-- +goose Up
ALTER TABLE agent_tasks
    ADD COLUMN resumed_from_task_id UUID REFERENCES agent_tasks(id) ON DELETE SET NULL;

CREATE INDEX agent_tasks_resumed_from_idx
    ON agent_tasks (resumed_from_task_id)
    WHERE resumed_from_task_id IS NOT NULL;

CREATE TABLE agent_task_checkpoints (
    task_id UUID PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    phase TEXT NOT NULL CHECK (phase IN (
        'ready_model',
        'calling_model',
        'awaiting_tools',
        'finalizing',
        'completed',
        'interrupted'
    )),
    round_number INTEGER NOT NULL DEFAULT 0,
    resumable BOOLEAN NOT NULL DEFAULT TRUE,
    state JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX agent_task_checkpoints_resumable_idx
    ON agent_task_checkpoints (updated_at)
    WHERE resumable = TRUE AND phase NOT IN ('completed');

-- Supports the bounded retention worker without scanning active tasks.
CREATE INDEX agent_tasks_terminal_completed_idx
    ON agent_tasks (completed_at)
    WHERE status IN ('done', 'failed', 'cancelled');

-- +goose Down
DROP INDEX agent_tasks_terminal_completed_idx;
DROP TABLE agent_task_checkpoints;
DROP INDEX agent_tasks_resumed_from_idx;
ALTER TABLE agent_tasks DROP COLUMN resumed_from_task_id;
