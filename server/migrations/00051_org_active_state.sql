-- +goose Up
-- +goose StatementBegin
-- dragent has no session table (access is short-lived JWTs + device-scoped
-- refresh tokens, unlike the reference product's session rows), so "active
-- organization/team" is tracked per-user, not per-session. ON DELETE SET
-- NULL, not CASCADE: deleting an organization/team must not delete the
-- user, only clear the pointer, and every active-state read falls back to
-- another membership when this is null (never a hard error).
ALTER TABLE users ADD COLUMN active_organization_id UUID REFERENCES organizations(id) ON DELETE SET NULL;
ALTER TABLE users ADD COLUMN active_team_id UUID REFERENCES teams(id) ON DELETE SET NULL;
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE users DROP COLUMN IF EXISTS active_team_id;
ALTER TABLE users DROP COLUMN IF EXISTS active_organization_id;
-- +goose StatementEnd
