-- Refuse to collapse different organisations into a shared legacy catalogue.
CREATE TEMP TABLE require_local_only (nonlocal_count INTEGER CHECK (nonlocal_count = 0));
INSERT INTO require_local_only VALUES (
    (SELECT COUNT(*) FROM local_workspace_catalogue WHERE organisation_id <> 'local') +
    (SELECT COUNT(*) FROM workspace_projects WHERE organisation_id <> 'local')
);
DROP TABLE require_local_only;
CREATE TABLE local_workspace_catalogue_old (
    root TEXT PRIMARY KEY NOT NULL,
    workspace_json TEXT NOT NULL,
    closed INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL
);
INSERT INTO local_workspace_catalogue_old SELECT root, workspace_json, closed, sort_order
    FROM local_workspace_catalogue WHERE organisation_id = 'local';
DROP TABLE local_workspace_catalogue;
ALTER TABLE local_workspace_catalogue_old RENAME TO local_workspace_catalogue;
ALTER TABLE workspace_projects DROP COLUMN organisation_id;
DROP TABLE organisation_context;
