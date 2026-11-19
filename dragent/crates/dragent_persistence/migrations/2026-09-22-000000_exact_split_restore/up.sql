-- Persist the identity/presentation of every pane that participates in a split.
-- Version the direction convention so legacy databases can still be decoded.
ALTER TABLE tabs ADD COLUMN hidden_from_sidebar INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tabs ADD COLUMN owner_pane_uuid BLOB;
ALTER TABLE app ADD COLUMN pane_layout_version INTEGER NOT NULL DEFAULT 0;

-- A split tree and its lightweight member-tab rows intentionally reference
-- the same stable terminal UUID. The original UNIQUE constraint prevented the
-- whole snapshot transaction from committing whenever a split existed.
ALTER TABLE terminal_panes RENAME TO terminal_panes_with_unique_uuid;
CREATE TABLE terminal_panes (
    pane_node_id INTEGER PRIMARY KEY REFERENCES pane_nodes(id) ON DELETE CASCADE,
    uuid BLOB NOT NULL,
    cwd TEXT NOT NULL,
    shell_title TEXT,
    is_active INTEGER NOT NULL DEFAULT 0,
    clear_gap_height_px REAL NOT NULL DEFAULT 0,
    clear_gap_before_block_id INTEGER,
    clear_gap_after_all INTEGER NOT NULL DEFAULT 0
);
INSERT INTO terminal_panes (
    pane_node_id, uuid, cwd, shell_title, is_active,
    clear_gap_height_px, clear_gap_before_block_id, clear_gap_after_all
)
SELECT
    pane_node_id, uuid, cwd, shell_title, is_active,
    clear_gap_height_px, clear_gap_before_block_id, clear_gap_after_all
FROM terminal_panes_with_unique_uuid;
DROP TABLE terminal_panes_with_unique_uuid;
