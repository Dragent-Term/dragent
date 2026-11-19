-- +goose Up
-- +goose StatementBegin
CREATE TABLE organization_domains (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Globally unique, not per-organization: a domain must resolve to at
    -- most one organization, or auto-join (11.A.7) would be ambiguous and a
    -- second organization could claim a domain it doesn't control.
    domain VARCHAR(255) UNIQUE NOT NULL,
    verification_token VARCHAR(64) NOT NULL,
    verified_at TIMESTAMP WITH TIME ZONE,
    auto_join_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_organization_domains_organization_id ON organization_domains (organization_id);
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP INDEX IF EXISTS idx_organization_domains_organization_id;
DROP TABLE IF EXISTS organization_domains;
-- +goose StatementEnd
