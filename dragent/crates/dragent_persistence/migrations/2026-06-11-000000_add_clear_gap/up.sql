ALTER TABLE terminal_panes ADD COLUMN clear_gap_height_px REAL NOT NULL DEFAULT 0.0;
ALTER TABLE terminal_panes ADD COLUMN clear_gap_before_block_id INTEGER;
ALTER TABLE terminal_panes ADD COLUMN clear_gap_after_all INTEGER NOT NULL DEFAULT 0;
