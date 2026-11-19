-- +goose Up
-- +goose StatementBegin

-- Subscription/payment foundation (LemonSqueezy, provider-agnostic schema).

-- 1) Purchasable subscription plans. Provider-agnostic: tier + interval +
--    monthly credit allowance are the entitlement; the provider variant/plan
--    handle just points a checkout at the right row. Annual rows share the
--    SAME tier and monthly_allowance_mc as their monthly sibling — only price
--    and interval differ (annual is the cheaper effective rate).
CREATE TABLE IF NOT EXISTS plans (
    id                    VARCHAR(64) PRIMARY KEY,      -- e.g. 'pro-monthly'
    tier                  VARCHAR(16) NOT NULL,         -- maps to user_balances.tier (pro/ultra/mega)
    billing_interval      VARCHAR(16) NOT NULL CHECK (billing_interval IN ('monthly','annual')),
    monthly_allowance_mc  BIGINT      NOT NULL,         -- credit allowance granted each month (micro-cents)
    price_cents           INTEGER     NOT NULL,         -- charged amount per interval (monthly OR annual total)
    currency              VARCHAR(8)  NOT NULL DEFAULT 'USD',
    display_name          VARCHAR(128) NOT NULL DEFAULT '',
    -- LemonSqueezy variant id for this plan's product. Empty until the
    -- operator creates the product in the LemonSqueezy dashboard and fills it
    -- in (admin or SQL). A plan with a blank provider id cannot be checked out.
    lemonsqueezy_variant_id VARCHAR(64) NOT NULL DEFAULT '',
    is_active             BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at            TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS plans_ls_variant_idx
    ON plans (lemonsqueezy_variant_id) WHERE lemonsqueezy_variant_id <> '';

-- 2) Idempotency + audit log for incoming provider webhooks. The id is a hash
--    of the raw webhook body, so a re-delivered (identical) webhook is a no-op.
CREATE TABLE IF NOT EXISTS payment_events (
    id           VARCHAR(80) PRIMARY KEY,   -- sha256 of raw webhook body
    provider     VARCHAR(32) NOT NULL,      -- 'lemonsqueezy'
    event_name   VARCHAR(64) NOT NULL,      -- provider's event name
    user_id      UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
    external_id  VARCHAR(128) NOT NULL DEFAULT '', -- provider subscription/order id
    payload      JSONB       NOT NULL,
    received_at  TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- 3) Link a user to their provider subscription so the account screen and the
--    customer-portal link can resolve it.
ALTER TABLE users ADD COLUMN IF NOT EXISTS lemonsqueezy_customer_id     VARCHAR(64)  NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN IF NOT EXISTS lemonsqueezy_subscription_id VARCHAR(64)  NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN IF NOT EXISTS subscription_status          VARCHAR(32)  NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN IF NOT EXISTS subscription_plan_id         VARCHAR(64)  NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN IF NOT EXISTS subscription_renews_at       TIMESTAMP WITH TIME ZONE NULL;

-- 4) Seed the Pro plan pair (monthly $20 / annual $216 = $18/mo effective,
--    SAME allowance). Allowance is the monthly charged-spend budget in
--    micro-cents ($1 = 1,000,000 mc). TUNE these numbers to the real business
--    model, and SET the lemonsqueezy_variant_id after creating the products in
--    the LemonSqueezy dashboard — checkout is refused while it is blank.
INSERT INTO plans (id, tier, billing_interval, monthly_allowance_mc, price_cents, display_name) VALUES
    ('pro-monthly', 'pro', 'monthly', 20000000, 2000,  'Pro (monthly)'),
    ('pro-annual',  'pro', 'annual',  20000000, 21600, 'Pro (annual)')
ON CONFLICT (id) DO NOTHING;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE users DROP COLUMN IF EXISTS subscription_renews_at;
ALTER TABLE users DROP COLUMN IF EXISTS subscription_plan_id;
ALTER TABLE users DROP COLUMN IF EXISTS subscription_status;
ALTER TABLE users DROP COLUMN IF EXISTS lemonsqueezy_subscription_id;
ALTER TABLE users DROP COLUMN IF EXISTS lemonsqueezy_customer_id;
DROP TABLE IF EXISTS payment_events;
DROP TABLE IF EXISTS plans;
-- +goose StatementEnd
