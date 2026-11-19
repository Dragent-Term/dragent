-- Independent of the replaceable UI snapshot. A project/branch alone is not
-- evidence that the user created a workspace in the main checkout.
CREATE TABLE local_workspace_catalogue (
    root TEXT PRIMARY KEY NOT NULL,
    workspace_json TEXT NOT NULL,
    closed INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL
);
