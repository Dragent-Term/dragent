-- +goose Up
-- +goose StatementBegin

-- Permanent payment-receipt history. Distinct from
-- credit_transactions (internal balance-ledger movements, not what was
-- actually charged) and payment_events (webhook idempotency log, not
-- structured as history). Never deleted by any code path, including
-- 11.F.3's entitlement-row deletion on leave/remove — financial records and
-- entitlement state have different lifecycles. user_id/organization_id use
-- ON DELETE SET NULL, mirroring payment_events' own precedent, so deleting
-- an account or org can never delete its receipts.
CREATE TABLE receipts (
    id                  UUID PRIMARY KEY,
    user_id             UUID REFERENCES users(id) ON DELETE SET NULL,
    organization_id     UUID REFERENCES organizations(id) ON DELETE SET NULL,
    amount_cents        BIGINT NOT NULL,
    currency            TEXT NOT NULL,
    provider            TEXT NOT NULL,
    provider_reference  TEXT NOT NULL DEFAULT '',
    description         TEXT NOT NULL DEFAULT '',
    kind                TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT receipts_has_owner CHECK (user_id IS NOT NULL OR organization_id IS NOT NULL)
);

CREATE INDEX idx_receipts_user_id ON receipts (user_id, created_at DESC) WHERE user_id IS NOT NULL;
CREATE INDEX idx_receipts_organization_id ON receipts (organization_id, created_at DESC) WHERE organization_id IS NOT NULL;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS receipts;
-- +goose StatementEnd
