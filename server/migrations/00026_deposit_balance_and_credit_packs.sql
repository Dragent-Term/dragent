-- +goose Up
-- +goose StatementBegin

-- Durable purchased-credit balance. Unlike paid_balance_mc (the subscription
-- monthly allowance, overwritten to paid_monthly_cap_mc on renewal), this is
-- NEVER reset — top-up credits persist until spent. Spent after the monthly
-- allowance and before the free baseline (see billing.selectPool).
ALTER TABLE user_balances ADD COLUMN IF NOT EXISTS deposit_balance_mc BIGINT NOT NULL DEFAULT 0;

-- One-time credit packs. credits_mc is the charged-spend budget granted; larger
-- packs bake in a bulk bonus (the "different credit pricing"). Rate unit:
-- $1 = 1,000,000 mc. lemonsqueezy_variant_id is blank until the operator
-- creates the product and fills it in; a blank pack cannot be checked out.
CREATE TABLE IF NOT EXISTS credit_packs (
    id                      VARCHAR(64) PRIMARY KEY,
    price_cents             INTEGER     NOT NULL,
    credits_mc              BIGINT      NOT NULL,
    currency                VARCHAR(8)  NOT NULL DEFAULT 'USD',
    display_name            VARCHAR(128) NOT NULL DEFAULT '',
    lemonsqueezy_variant_id VARCHAR(64) NOT NULL DEFAULT '',
    is_active               BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at              TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS credit_packs_ls_variant_idx
    ON credit_packs (lemonsqueezy_variant_id) WHERE lemonsqueezy_variant_id <> '';

-- Seed three packs with a volume bonus: $5 = $5 of spend, $20 = $22 (+10%),
-- $50 = $60 (+20%). TUNE the bonus curve; SET variant ids after creating the
-- LemonSqueezy products.
INSERT INTO credit_packs (id, price_cents, credits_mc, display_name) VALUES
    ('pack-5',  500,   5000000,  '$5 credits'),
    ('pack-20', 2000,  22000000, '$20 credits (+10% bonus)'),
    ('pack-50', 5000,  60000000, '$50 credits (+20% bonus)')
ON CONFLICT (id) DO NOTHING;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS credit_packs;
ALTER TABLE user_balances DROP COLUMN IF EXISTS deposit_balance_mc;
-- +goose StatementEnd
