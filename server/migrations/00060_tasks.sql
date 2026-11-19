-- +goose Up
-- +goose StatementBegin

-- The organisation's shared record of work to do. Each organisation has a
-- key prefix and a counter (tasks are DRA-1, DRA-2, …); numbers are given
-- out inside a transaction and never reused, even after a deletion.
CREATE TABLE task_counters (
    organization_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    prefix          TEXT NOT NULL,
    next_number     BIGINT NOT NULL DEFAULT 1
);

CREATE TABLE tasks (
    id               UUID PRIMARY KEY,
    organization_id  UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    number           BIGINT NOT NULL,
    key              TEXT NOT NULL,
    title            TEXT NOT NULL,
    description      TEXT NOT NULL DEFAULT '',
    status           TEXT NOT NULL DEFAULT 'todo',
    priority         TEXT NOT NULL DEFAULT 'none',
    assignee_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    creator_user_id  UUID REFERENCES users(id) ON DELETE SET NULL,
    project_key      TEXT,
    project_label    TEXT,
    labels           TEXT[] NOT NULL DEFAULT '{}',
    due_at           TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at       TIMESTAMPTZ,
    completed_at     TIMESTAMPTZ,
    deleted_at       TIMESTAMPTZ,
    UNIQUE (organization_id, number),
    UNIQUE (organization_id, key)
);

CREATE TABLE task_comments (
    id             UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    task_id        UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    actor_kind     TEXT NOT NULL DEFAULT 'person',
    body           TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    edited_at      TIMESTAMPTZ
);

CREATE TABLE task_activity (
    id              UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    task_id         UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    actor_user_id   UUID REFERENCES users(id) ON DELETE SET NULL,
    actor_kind      TEXT NOT NULL DEFAULT 'person',
    kind            TEXT NOT NULL,
    from_json       JSONB,
    to_json         JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- A link is unique per task by kind + reference: posting the same pair
-- again updates the row instead of adding one.
CREATE TABLE task_links (
    id               UUID PRIMARY KEY,
    organization_id  UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    task_id          UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL,
    title            TEXT NOT NULL,
    url              TEXT,
    reference        TEXT NOT NULL,
    state            TEXT,
    added_by_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (task_id, kind, reference)
);

-- The list filters: status, assignee, project and incremental refresh.
CREATE INDEX idx_tasks_org_status ON tasks (organization_id, status) WHERE deleted_at IS NULL;
CREATE INDEX idx_tasks_org_assignee ON tasks (organization_id, assignee_user_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_tasks_org_project ON tasks (organization_id, project_key) WHERE deleted_at IS NULL;
CREATE INDEX idx_tasks_org_labels ON tasks USING GIN (labels);
-- Most-recently-updated-first page order, and the deleted_ids tail of an
-- incremental refresh. Unscoped (no partial predicate) so both live and
-- deleted rows are covered.
CREATE INDEX idx_tasks_org_updated ON tasks (organization_id, updated_at DESC, id ASC);
CREATE INDEX idx_tasks_org_deleted ON tasks (organization_id, updated_at DESC) WHERE deleted_at IS NOT NULL;
CREATE INDEX idx_task_comments_task ON task_comments (task_id, created_at ASC, id ASC);
CREATE INDEX idx_task_activity_task ON task_activity (task_id, created_at ASC, id ASC);
CREATE INDEX idx_task_links_task ON task_links (task_id, created_at ASC, id ASC);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS task_links;
DROP TABLE IF EXISTS task_activity;
DROP TABLE IF EXISTS task_comments;
DROP TABLE IF EXISTS tasks;
DROP TABLE IF EXISTS task_counters;
-- +goose StatementEnd
