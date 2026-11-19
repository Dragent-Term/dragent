-- Dragent session persistence (async writer thread layout + blocks).

CREATE TABLE app (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    active_tab_id INTEGER NOT NULL DEFAULT 0,
    active_pane_uuid BLOB,
    left_panel_open INTEGER NOT NULL DEFAULT 1,
    left_panel_width REAL,
    right_panel_open INTEGER NOT NULL DEFAULT 1,
    right_panel_width REAL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO app (id, active_tab_id) VALUES (1, 0);

CREATE TABLE tabs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    logical_id INTEGER NOT NULL UNIQUE,
    sort_index INTEGER NOT NULL,
    title TEXT NOT NULL,
    custom_label TEXT,
    color TEXT
);

CREATE TABLE pane_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tab_id INTEGER NOT NULL REFERENCES tabs(id) ON DELETE CASCADE,
    parent_id INTEGER REFERENCES pane_nodes(id) ON DELETE CASCADE,
    is_leaf INTEGER NOT NULL,
    split_direction TEXT,
    flex_size REAL
);

CREATE TABLE terminal_panes (
    pane_node_id INTEGER PRIMARY KEY REFERENCES pane_nodes(id) ON DELETE CASCADE,
    uuid BLOB NOT NULL UNIQUE,
    cwd TEXT NOT NULL,
    shell_title TEXT,
    is_active INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE blocks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pane_uuid BLOB NOT NULL,
    command TEXT NOT NULL,
    cwd TEXT NOT NULL,
    output_json TEXT NOT NULL,
    status TEXT NOT NULL,
    exit_code INTEGER,
    started_at_unix_ms INTEGER,
    duration_ms INTEGER,
    expanded INTEGER NOT NULL DEFAULT 1,
    bookmarked INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL
);

CREATE INDEX idx_blocks_pane_uuid ON blocks(pane_uuid);
CREATE INDEX idx_blocks_sort ON blocks(pane_uuid, sort_order);
