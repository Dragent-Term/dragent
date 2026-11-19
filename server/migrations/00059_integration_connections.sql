-- +goose Up
-- +goose StatementBegin

-- Connections to the services work comes from (Linear, Jira, Sentry,
-- Slack, …), made with a key someone pasted in the desktop app. A shared
-- connection is the organization's: every member's app and agents use it,
-- and only owners and admins manage it. A personal one is its owner's
-- alone, in that organization. The keys never leave the server once saved:
-- the desktop app's requests to the service go through the server, which
-- adds them (see internal/integrations).
CREATE TABLE integration_connections (
    id                 UUID PRIMARY KEY,
    organization_id    UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    owner_user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider           TEXT NOT NULL,
    shared             BOOLEAN NOT NULL,
    -- Who the key belongs to, as the service named them when it was checked.
    account_label      TEXT,
    -- The values that aren't secret: a server address, a workspace, an email.
    config             JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- AES-256-GCM of a JSON object holding the secret values.
    secrets_encrypted  TEXT NOT NULL,
    -- The last characters of the key, so people can tell keys apart.
    key_hint           TEXT,
    -- 'ok', or 'rejected' once the service refused the key.
    status             TEXT NOT NULL DEFAULT 'ok',
    last_used_at       TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One shared connection per service in an organization, and one personal
-- connection per service for each member.
CREATE UNIQUE INDEX idx_integration_connections_shared
    ON integration_connections (organization_id, provider) WHERE shared;
CREATE UNIQUE INDEX idx_integration_connections_personal
    ON integration_connections (organization_id, provider, owner_user_id) WHERE NOT shared;
CREATE INDEX idx_integration_connections_owner
    ON integration_connections (owner_user_id, organization_id);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS integration_connections;
-- +goose StatementEnd
