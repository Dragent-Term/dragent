-- +goose Up
-- +goose StatementBegin

-- Per-seat organization billing. One tier per org, quantity = member count, reusing the
-- SAME plans catalog individuals buy from — not a separate org product.
ALTER TABLE organizations
    ADD COLUMN plan_id VARCHAR(64) REFERENCES plans(id) ON DELETE RESTRICT,
    ADD COLUMN tier TEXT NOT NULL DEFAULT 'free' CHECK (tier IN ('free', 'pro', 'max', 'ultra')),
    ADD COLUMN monthly_allowance_mc BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN lemonsqueezy_customer_id TEXT,
    ADD COLUMN lemonsqueezy_subscription_id TEXT,
    ADD COLUMN lemonsqueezy_subscription_item_id TEXT,
    ADD COLUMN subscription_status TEXT NOT NULL DEFAULT '',
    ADD COLUMN subscription_renews_at TIMESTAMP WITH TIME ZONE;

-- One LemonSqueezy subscription can belong to at most one organization.
-- Partial (WHERE NOT NULL) so multiple still-Free orgs (NULL) don't collide.
CREATE UNIQUE INDEX organizations_lemonsqueezy_subscription_id_key
    ON organizations (lemonsqueezy_subscription_id)
    WHERE lemonsqueezy_subscription_id IS NOT NULL;

-- Tracks WHICH organization currently sources a member's paid tier, so
-- leaving/removal only downgrades a balance that org actually granted —
-- never a different org's grant or an unrelated personal subscription.
ALTER TABLE user_balances
    ADD COLUMN org_seat_source_id UUID REFERENCES organizations(id) ON DELETE SET NULL;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

ALTER TABLE user_balances DROP COLUMN IF EXISTS org_seat_source_id;

DROP INDEX IF EXISTS organizations_lemonsqueezy_subscription_id_key;

ALTER TABLE organizations
    DROP COLUMN IF EXISTS subscription_renews_at,
    DROP COLUMN IF EXISTS subscription_status,
    DROP COLUMN IF EXISTS lemonsqueezy_subscription_item_id,
    DROP COLUMN IF EXISTS lemonsqueezy_subscription_id,
    DROP COLUMN IF EXISTS lemonsqueezy_customer_id,
    DROP COLUMN IF EXISTS monthly_allowance_mc,
    DROP COLUMN IF EXISTS tier,
    DROP COLUMN IF EXISTS plan_id;

-- +goose StatementEnd
