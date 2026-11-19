-- +goose Up
-- +goose StatementBegin

-- Set the default trial grant to $1.50 for new accounts.
-- Existing rows are unchanged; this only affects accounts created after this migration.
ALTER TABLE user_balances ALTER COLUMN balance_credits SET DEFAULT 1500000;

-- All rates are stored as micro-cents per million tokens.
-- $1.00 = 1,000,000 micro-cents.  Example: $3.00/M → 3,000,000.
--
-- The prefix-matching fallback in GetModelPrice (longest match wins) means we
-- can insert family prefixes (e.g. "claude-opus-") that cover every variant
-- released under that tier, and add exact overrides for specific versions.

INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES

-- ── Anthropic ─────────────────────────────────────────────────────────────────
-- Opus tier  — $15 in / $75 out per million tokens
('claude-opus-',           15000000, 75000000, true),

-- Sonnet tier — $3 in / $15 out per million tokens
('claude-sonnet-',          3000000, 15000000, true),

-- Haiku tier  — $0.80 in / $4 out per million tokens
('claude-haiku-',            800000,  4000000, true),

-- Catch-all for any claude- model not matched by a family prefix above.
-- Prices the model at Sonnet tier, which is the default model.
('claude-',                 3000000, 15000000, true),

-- ── OpenAI GPT ───────────────────────────────────────────────────────────────
('gpt-4o-mini',              150000,   600000, true),  -- $0.15 in / $0.60 out
('gpt-4o',                  2500000, 10000000, true),  -- $2.50 in / $10 out
('gpt-4-turbo',            10000000, 30000000, true),  -- $10 in / $30 out
('gpt-4',                  30000000, 60000000, true),  -- $30 in / $60 out (prefix for older gpt-4 variants)
('gpt-3.5-turbo',            500000,  1500000, true),  -- $0.50 in / $1.50 out

-- ── OpenAI Reasoning (o-series) ──────────────────────────────────────────────
('o4-mini',                 1100000,  4400000, true),  -- $1.10 in / $4.40 out
('o3-mini',                 1100000,  4400000, true),  -- $1.10 in / $4.40 out
('o3',                     10000000, 40000000, true),  -- $10 in / $40 out
('o1-mini',                 3000000, 12000000, true),  -- $3 in / $12 out
('o1',                     15000000, 60000000, true),  -- $15 in / $60 out

-- ── Google Gemini ─────────────────────────────────────────────────────────────
('gemini-2.5-pro',          1250000, 10000000, true),  -- $1.25 in / $10 out
('gemini-2.5-flash',         300000,  2500000, true),  -- $0.30 in / $2.50 out
('gemini-2.0-flash',         100000,   400000, true),  -- $0.10 in / $0.40 out
('gemini-1.5-pro',          1250000,  5000000, true),  -- $1.25 in / $5 out
('gemini-1.5-flash',          75000,   300000, true),  -- $0.075 in / $0.30 out
('gemini-',                 1250000,  5000000, true),  -- catch-all at 1.5-Pro rates

-- ── Groq-hosted open models ───────────────────────────────────────────────────
-- Llama 3.x 70B
('llama-3.3-70b',            590000,   790000, true),
('llama-3.1-70b',            590000,   790000, true),
('llama3-70b',               590000,   790000, true),
-- Llama 3.x 8B  (faster, cheaper)
('llama-3.1-8b',              50000,    80000, true),
('llama3-8b',                 50000,    80000, true),
-- Llama catch-all (70B rates)
('llama',                    590000,   790000, true),
-- Mixtral
('mixtral-8x7b',             240000,   240000, true),
('mixtral',                  240000,   240000, true),
-- Gemma
('gemma2-9b',                200000,   200000, true),
('gemma',                    200000,   200000, true),

-- ── Cerebras (Qwen) ───────────────────────────────────────────────────────────
('qwen-3-32b',               700000,   700000, true),
('qwen',                     700000,   700000, true)

ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- Remove the stale 'claude-3-5-sonnet' exact entry — the new 'claude-sonnet-'
-- prefix covers all Sonnet variants including 3-5, 4-6, and future releases.
DELETE FROM model_prices WHERE model_name = 'claude-3-5-sonnet';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

ALTER TABLE user_balances ALTER COLUMN balance_credits SET DEFAULT 500000;

DELETE FROM model_prices WHERE model_name IN (
    'claude-opus-', 'claude-sonnet-', 'claude-haiku-', 'claude-',
    'gpt-4-turbo', 'gpt-4', 'gpt-3.5-turbo',
    'o4-mini', 'o3-mini', 'o3', 'o1-mini', 'o1',
    'gemini-2.5-pro', 'gemini-2.5-flash', 'gemini-2.0-flash',
    'gemini-1.5-pro', 'gemini-1.5-flash', 'gemini-',
    'llama-3.3-70b', 'llama-3.1-70b', 'llama3-70b',
    'llama-3.1-8b', 'llama3-8b', 'llama',
    'mixtral-8x7b', 'mixtral',
    'gemma2-9b', 'gemma',
    'qwen-3-32b', 'qwen'
);

-- Restore the original gpt-4o price
UPDATE model_prices SET input_rate_per_million = 5000000, output_rate_per_million = 15000000
    WHERE model_name = 'gpt-4o';

-- Restore the original claude-3-5-sonnet exact entry
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active)
    VALUES ('claude-3-5-sonnet', 3000000, 15000000, true)
    ON CONFLICT DO NOTHING;

-- +goose StatementEnd
