-- +goose Up
-- +goose StatementBegin

-- Automations are defined and run by the desktop app on its owner's
-- machine. This registry mirrors each one so teammates can see what the
-- organization automates (the Team tab), and so the free plan's allowance
-- is counted per account across all of its machines. Project paths stay on
-- the owner's machine; only a display label is shared.
CREATE TABLE automations (
    id               UUID PRIMARY KEY,
    organization_id  UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    owner_user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    prompt           TEXT NOT NULL DEFAULT '',
    schedule         TEXT NOT NULL,
    schedule_label   TEXT NOT NULL DEFAULT '',
    timezone         TEXT NOT NULL DEFAULT '',
    agent            TEXT NOT NULL,
    project_label    TEXT,
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,
    next_run_at      TIMESTAMPTZ,
    last_run_status  TEXT,
    last_run_at      TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_automations_organization ON automations (organization_id, created_at);
CREATE INDEX idx_automations_owner ON automations (owner_user_id, organization_id);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS automations;
-- +goose StatementEnd
