-- +goose Up
-- +goose StatementBegin

-- In-app "Give feedback" submissions from the desktop app. The text carries
-- the metadata the app appends (contact email, version, platform); images and
-- the optional redacted diagnostic log are kept alongside it. Feedback
-- outlives account and organization deletion (SET NULL), like receipts.
CREATE TABLE feedback (
    id               UUID PRIMARY KEY,
    user_id          UUID REFERENCES users(id) ON DELETE SET NULL,
    organization_id  UUID REFERENCES organizations(id) ON DELETE SET NULL,
    content          TEXT NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_feedback_created_at ON feedback (created_at DESC);
CREATE INDEX idx_feedback_user_id ON feedback (user_id, created_at DESC) WHERE user_id IS NOT NULL;

CREATE TABLE feedback_attachments (
    id           UUID PRIMARY KEY,
    feedback_id  UUID NOT NULL REFERENCES feedback(id) ON DELETE CASCADE,
    filename     TEXT NOT NULL,
    mime_type    TEXT NOT NULL,
    size_bytes   INTEGER NOT NULL,
    data         BYTEA NOT NULL
);

CREATE INDEX idx_feedback_attachments_feedback_id ON feedback_attachments (feedback_id);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS feedback_attachments;
DROP TABLE IF EXISTS feedback;
-- +goose StatementEnd
