-- +goose Up
-- +goose StatementBegin

-- Per-model minimum balance to make an agent call (micro-cents).
--
-- Billing model change: we no longer pre-estimate a call's cost to gate it (the
-- estimate was cache-blind and systematically over-blocked affordable calls).
-- Instead, a user may make a call whenever they hold at least this model's
-- minimum, and we settle the ACTUAL provider-reported cost afterward. Since we
-- settle after every step, any overshoot is bounded to one step; the overage is
-- booked into the weekly+session usage counters (the paid balance floors at 0,
-- never negative), so nothing is confiscated and nothing is silently absorbed.
--
-- Each value = the cost of one small step (~2,000 input + ~500 output tokens) at
-- the model's own standard rates × the 2.5 markup — so cheap models are nearly
-- free to enter and expensive ones carry a proportionally higher bar. Values are
-- admin-tunable afterward via PUT /api/v1/admin/models.
ALTER TABLE model_prices
    ADD COLUMN IF NOT EXISTS min_balance_mc BIGINT NOT NULL DEFAULT 5000;

-- Seed the 17 user-facing models (derived from live rates, July 2026).
UPDATE model_prices SET min_balance_mc =   4625 WHERE model_name = 'gemini-2.5-flash';
UPDATE model_prices SET min_balance_mc =   7500 WHERE model_name = 'grok-build-0.1';
UPDATE model_prices SET min_balance_mc =   9375 WHERE model_name = 'gpt-5.4-mini';
UPDATE model_prices SET min_balance_mc =   9375 WHERE model_name = 'grok-4.3';
UPDATE model_prices SET min_balance_mc =  11250 WHERE model_name = 'claude-haiku-4-5';
UPDATE model_prices SET min_balance_mc =  12500 WHERE model_name = 'gpt-5.6-luna';
UPDATE model_prices SET min_balance_mc =  17500 WHERE model_name = 'grok-4.5';
UPDATE model_prices SET min_balance_mc =  18750 WHERE model_name = 'gemini-3.5-flash';
UPDATE model_prices SET min_balance_mc =  22500 WHERE model_name = 'claude-sonnet-5';
UPDATE model_prices SET min_balance_mc =  25000 WHERE model_name = 'gemini-3.1-pro';
UPDATE model_prices SET min_balance_mc =  31250 WHERE model_name = 'gpt-5.6-terra';
UPDATE model_prices SET min_balance_mc =  31250 WHERE model_name = 'gpt-5.4';
UPDATE model_prices SET min_balance_mc =  33750 WHERE model_name = 'claude-sonnet-4-6';
UPDATE model_prices SET min_balance_mc =  56250 WHERE model_name = 'claude-opus-4-8';
UPDATE model_prices SET min_balance_mc =  62500 WHERE model_name = 'gpt-5.6-sol';
UPDATE model_prices SET min_balance_mc =  62500 WHERE model_name = 'gpt-5.5';
UPDATE model_prices SET min_balance_mc = 112500 WHERE model_name = 'claude-fable-5';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE model_prices DROP COLUMN IF EXISTS min_balance_mc;
-- +goose StatementEnd
