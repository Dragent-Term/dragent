-- +goose Up
-- +goose StatementBegin
CREATE TABLE invitations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    email VARCHAR(255) NOT NULL,
    role VARCHAR(50) NOT NULL CHECK (role IN ('admin', 'member')),
    status VARCHAR(20) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'rejected', 'canceled')),
    inviter_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- One pending invitation per (organization, email) at a time — prevents
-- spamming repeat invites to the same address while one is still open.
-- Once it's accepted/rejected/canceled a new one can be created freely.
CREATE UNIQUE INDEX idx_invitations_org_email_pending
    ON invitations (organization_id, email)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_invitations_email ON invitations (email);
CREATE INDEX IF NOT EXISTS idx_invitations_organization_id ON invitations (organization_id);
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP INDEX IF EXISTS idx_invitations_organization_id;
DROP INDEX IF EXISTS idx_invitations_email;
DROP INDEX IF EXISTS idx_invitations_org_email_pending;
DROP TABLE IF EXISTS invitations;
-- +goose StatementEnd
