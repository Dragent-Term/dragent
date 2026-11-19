-- +goose Up
-- +goose StatementBegin
-- Durable audit trail of remote-command decisions in shared sessions. Session
-- and participant state itself lives in Redis (ephemeral/hot); only these
-- security-relevant decisions are persisted. IDs are stored as text (not FKs):
-- sessions/participants are not Postgres rows, and the record must survive even
-- if the acting user is later deleted.
CREATE TABLE IF NOT EXISTS audit_events (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id     TEXT NOT NULL,
    participant_id TEXT NOT NULL,
    host_user_id   TEXT NOT NULL,
    command        TEXT NOT NULL,
    approved       BOOLEAN NOT NULL,
    created_at     TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_events_session ON audit_events (session_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_events_host_user ON audit_events (host_user_id, created_at DESC);
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS audit_events;
-- +goose StatementEnd
