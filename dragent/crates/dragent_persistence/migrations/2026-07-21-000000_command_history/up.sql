-- Persistent, GLOBAL command history for the ↑/↓ history popover.
--
-- Deliberately SEPARATE from `blocks`: `blocks` is per-pane and wiped on app
-- close (clear_terminal_blocks_on_close), whereas this table is never cleared by
-- any session-reset path and is shared across every tab/pane. One row per unique
-- command; re-running a command bumps its timestamp and count (upsert).
CREATE TABLE command_history (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    command              TEXT    NOT NULL UNIQUE,
    last_used_at_unix_ms BIGINT  NOT NULL,
    use_count            INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_command_history_last_used
    ON command_history (last_used_at_unix_ms DESC);
