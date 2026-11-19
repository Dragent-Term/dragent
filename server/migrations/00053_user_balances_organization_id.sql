-- +goose Up
-- +goose StatementBegin

-- Step 1 of the (user_id, organization_id) re-key.
-- Deliberately additive and backward-compatible on its own: user_id stays
-- the primary key, no code changes required to run this. Backfills every
-- existing row to the user's own PERSONAL org (their oldest membership —
-- reliable because InitializeTx creates the personal org + membership
-- atomically at signup, so it is always the first membership a user ever
-- has). NOT NULL is deferred to a later migration, once every write path
-- (ActivateSubscription/GrantOrgSeat/RevokeOrgSeat) is updated to always
-- set it and the backfill is confirmed complete in every environment this
-- runs against, not just local dev.
ALTER TABLE user_balances ADD COLUMN organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE;

UPDATE user_balances ub
SET organization_id = (
    SELECT m.organization_id
    FROM memberships m
    WHERE m.user_id = ub.user_id
    ORDER BY m.created_at ASC
    LIMIT 1
)
WHERE ub.organization_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_user_balances_organization_id ON user_balances (organization_id);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP INDEX IF EXISTS idx_user_balances_organization_id;
ALTER TABLE user_balances DROP COLUMN IF EXISTS organization_id;
-- +goose StatementEnd
