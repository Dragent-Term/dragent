CREATE TABLE workspace_projects (
    id TEXT PRIMARY KEY NOT NULL,
    root TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created_at_unix_ms BIGINT NOT NULL,
    updated_at_unix_ms BIGINT NOT NULL
);

CREATE TABLE workspace_work_items (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES workspace_projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    status_json TEXT NOT NULL,
    created_at_unix_ms BIGINT NOT NULL,
    updated_at_unix_ms BIGINT NOT NULL
);

CREATE TABLE workspace_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL REFERENCES workspace_projects(id) ON DELETE CASCADE,
    work_item_id TEXT,
    sequence BIGINT NOT NULL,
    timestamp_unix_ms BIGINT NOT NULL,
    kind_json TEXT NOT NULL
);

CREATE INDEX workspace_work_items_project_idx ON workspace_work_items(project_id, updated_at_unix_ms);
CREATE INDEX workspace_events_project_idx ON workspace_events(project_id, id);
