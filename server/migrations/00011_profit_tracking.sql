-- +goose Up
-- +goose StatementBegin

-- ── Enrich credit_transactions with call metadata ─────────────────────────────
ALTER TABLE credit_transactions
    ADD COLUMN model_name    VARCHAR(100) DEFAULT NULL,
    ADD COLUMN input_tokens  BIGINT       DEFAULT NULL,
    ADD COLUMN output_tokens BIGINT       DEFAULT NULL,
    ADD COLUMN call_type     VARCHAR(20)  DEFAULT NULL, -- 'chat' | 'agent' | 'deposit' | 'refund'

    -- Which credit pool this transaction drew from.
    --   'paid' → deducted from the user's subscription allocation (consumed first)
    --   'free' → deducted from the weekly free-tier baseline (fallback)
    --   'deposit' / 'refund' → positive amounts, pool not applicable
    ADD COLUMN pool          VARCHAR(10)  NOT NULL DEFAULT 'paid'
        CHECK (pool IN ('paid', 'free', 'deposit', 'refund'));

-- ── platform_profits ─────────────────────────────────────────────────────────
-- One row per billable API call (our-key calls only).
-- BYOK calls: NO row here — we charge nothing, we pay nothing on their key.
--
-- api_cost_mc     = micro-cents we paid the AI provider
-- user_charged_mc = micro-cents deducted from the user (api_cost × markup)
-- profit_mc       = user_charged_mc - api_cost_mc  (always positive for normal calls)
--
-- pool mirrors credit_transactions.pool so profits can be split by pool type.
-- In practice all profit comes from 'paid' pool calls; 'free' pool calls still
-- generate profit but are tracked separately so we can see the true cost of
-- the free tier subsidy.
CREATE TABLE platform_profits (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    transaction_id  UUID         REFERENCES credit_transactions(id) ON DELETE SET NULL,
    model_name      VARCHAR(100) NOT NULL,
    call_type       VARCHAR(20)  NOT NULL DEFAULT 'chat',  -- 'chat' | 'agent'
    pool            VARCHAR(10)  NOT NULL DEFAULT 'paid'
                        CHECK (pool IN ('paid', 'free')),
    input_tokens    BIGINT       NOT NULL DEFAULT 0,
    output_tokens   BIGINT       NOT NULL DEFAULT 0,
    api_cost_mc     BIGINT       NOT NULL,   -- what we paid the provider
    user_charged_mc BIGINT       NOT NULL,   -- what we deducted from the user
    profit_mc       BIGINT       NOT NULL,   -- user_charged_mc - api_cost_mc
    markup          NUMERIC(5,2) NOT NULL DEFAULT 2.50,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- Useful queries:
--
-- Total profit for a period:
--   SELECT SUM(profit_mc)/1000000.0 AS profit_usd
--   FROM platform_profits WHERE created_at BETWEEN x AND y;
--
-- Free-tier subsidy cost (what the free fallback pool costs us):
--   SELECT SUM(api_cost_mc)/1000000.0 AS subsidy_usd
--   FROM platform_profits WHERE pool = 'free';
--
-- Profit by model:
--   SELECT model_name, SUM(profit_mc)/1000000.0 AS profit_usd
--   FROM platform_profits GROUP BY model_name ORDER BY profit_usd DESC;

CREATE INDEX idx_platform_profits_user    ON platform_profits(user_id);
CREATE INDEX idx_platform_profits_created ON platform_profits(created_at);
CREATE INDEX idx_platform_profits_model   ON platform_profits(model_name);
CREATE INDEX idx_platform_profits_pool    ON platform_profits(pool);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

DROP INDEX IF EXISTS idx_platform_profits_pool;
DROP INDEX IF EXISTS idx_platform_profits_model;
DROP INDEX IF EXISTS idx_platform_profits_created;
DROP INDEX IF EXISTS idx_platform_profits_user;
DROP TABLE IF EXISTS platform_profits;

ALTER TABLE credit_transactions
    DROP COLUMN IF EXISTS model_name,
    DROP COLUMN IF EXISTS input_tokens,
    DROP COLUMN IF EXISTS output_tokens,
    DROP COLUMN IF EXISTS call_type,
    DROP COLUMN IF EXISTS pool;

-- +goose StatementEnd
