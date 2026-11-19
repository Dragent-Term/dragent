-- Existing records belong to local. New organisations use separate databases.
CREATE TABLE organisation_context (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    organisation_id TEXT NOT NULL
);
-- Pin existing data to local; an empty newly created database is bound by Rust.
INSERT INTO organisation_context (id, organisation_id)
    SELECT 1, 'local' WHERE EXISTS (SELECT 1 FROM tabs)
        OR EXISTS (SELECT 1 FROM workspace_projects)
        OR EXISTS (SELECT 1 FROM local_workspace_catalogue)
        OR EXISTS (SELECT 1 FROM workspace_ui_state);
ALTER TABLE workspace_projects ADD COLUMN organisation_id TEXT NOT NULL DEFAULT 'local';
ALTER TABLE local_workspace_catalogue RENAME TO local_workspace_catalogue_legacy;
CREATE TABLE local_workspace_catalogue (
    organisation_id TEXT NOT NULL,
    root TEXT NOT NULL,
    workspace_json TEXT NOT NULL,
    closed INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL,
    PRIMARY KEY (organisation_id, root)
);
INSERT INTO local_workspace_catalogue (organisation_id, root, workspace_json, closed, sort_order)
    SELECT 'local', root, workspace_json, closed, sort_order FROM local_workspace_catalogue_legacy;
DROP TABLE local_workspace_catalogue_legacy;
