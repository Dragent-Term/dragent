-- +goose Up
-- +goose StatementBegin

-- Allow the new 'max' paid tier (between pro and ultra).
ALTER TABLE user_balances DROP CONSTRAINT IF EXISTS user_balances_tier_check;
ALTER TABLE user_balances ADD CONSTRAINT user_balances_tier_check
    CHECK (tier IN ('free', 'pro', 'max', 'ultra', 'mega'));

-- Subscription tiers. monthly_allowance_mc = charged-spend budget per month
-- ($1 = 1,000,000 mc). Annual = same allowance at 10% off the yearly total
-- ($X/mo × 12 × 0.9). pro-monthly / pro-annual already exist (migration 00025).
INSERT INTO plans (id, tier, billing_interval, monthly_allowance_mc, price_cents, display_name) VALUES
    ('max-monthly',   'max',   'monthly',  60000000,   6000,   'Max (monthly)'),
    ('max-annual',    'max',   'annual',   60000000,   64800,  'Max (annual)'),
    ('ultra-monthly', 'ultra', 'monthly',  200000000,  20000,  'Ultra (monthly)'),
    ('ultra-annual',  'ultra', 'annual',   200000000,  216000, 'Ultra (annual)')
ON CONFLICT (id) DO UPDATE SET
    tier                 = EXCLUDED.tier,
    billing_interval     = EXCLUDED.billing_interval,
    monthly_allowance_mc = EXCLUDED.monthly_allowance_mc,
    price_cents          = EXCLUDED.price_cents,
    display_name         = EXCLUDED.display_name;

-- Credit packs: $10 → $1000 with an escalating bulk bonus (0% → +35%). credits_mc
-- is the charged-spend budget granted. Replaces the old $5 pack. TUNE the curve.
DELETE FROM credit_packs WHERE id = 'pack-5';
INSERT INTO credit_packs (id, price_cents, credits_mc, display_name) VALUES
    ('pack-10',   1000,   10000000,    '$10 credits'),
    ('pack-20',   2000,   22000000,    '$20 credits (+10%)'),
    ('pack-50',   5000,   57500000,    '$50 credits (+15%)'),
    ('pack-100',  10000,  120000000,   '$100 credits (+20%)'),
    ('pack-200',  20000,  250000000,   '$200 credits (+25%)'),
    ('pack-500',  50000,  650000000,   '$500 credits (+30%)'),
    ('pack-1000', 100000, 1350000000,  '$1000 credits (+35%)')
ON CONFLICT (id) DO UPDATE SET
    price_cents  = EXCLUDED.price_cents,
    credits_mc   = EXCLUDED.credits_mc,
    display_name = EXCLUDED.display_name;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DELETE FROM credit_packs WHERE id IN ('pack-10','pack-100','pack-200','pack-500','pack-1000');
UPDATE credit_packs SET price_cents = 5000, credits_mc = 60000000, display_name = '$50 credits (+20% bonus)' WHERE id = 'pack-50';
UPDATE credit_packs SET price_cents = 2000, credits_mc = 22000000, display_name = '$20 credits (+10% bonus)' WHERE id = 'pack-20';
INSERT INTO credit_packs (id, price_cents, credits_mc, display_name) VALUES ('pack-5', 500, 5000000, '$5 credits') ON CONFLICT (id) DO NOTHING;
DELETE FROM plans WHERE id IN ('max-monthly','max-annual','ultra-monthly','ultra-annual');
-- Note: leaves the widened tier CHECK in place (narrowing it could reject an
-- existing 'max' subscriber). Re-narrow manually in dev if required.
-- +goose StatementEnd
