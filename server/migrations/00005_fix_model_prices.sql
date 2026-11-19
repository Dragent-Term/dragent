-- +goose Up
-- +goose StatementBegin

-- ── Anthropic corrections ─────────────────────────────────────────────────────
-- Opus 4.x (4.5 onwards) dropped from $15/$75 to $5/$25.
-- The broad 'claude-opus-' prefix is replaced with two narrower ones:
--   'claude-opus-4-'  covers 4.5, 4.6, 4.7, 4.8 and all future 4.x builds ($5/$25)
--   'claude-opus-'    catches anything outside 4.x (e.g. 3.x, all retired) ($15/$75)
--   'claude-opus-4-1' exact override — deprecated model, still priced at old rate
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES
('claude-opus-4-',  5000000, 25000000, true),
('claude-opus-4-1', 15000000, 75000000, true)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- The 'claude-opus-' catch-all stays at $15/$75 for old/retired 3.x Opus models (already seeded).

-- Haiku 4.x is $1/$5, not $0.80/$4 (that was Haiku 3.5).
-- Add a 'claude-haiku-4-' prefix that wins over the 'claude-haiku-' catch-all.
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES
('claude-haiku-4-', 1000000, 5000000, true)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- Fable 5 — new flagship model ($10/$50).
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES
('claude-fable-', 10000000, 50000000, true)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- ── OpenAI corrections ────────────────────────────────────────────────────────
-- o3 dropped from $10/$40 (launch) to $2/$8.
UPDATE model_prices
    SET input_rate_per_million = 2000000, output_rate_per_million = 8000000
    WHERE model_name = 'o3';

-- ── Groq corrections ─────────────────────────────────────────────────────────
-- Qwen3-32B on Groq is $0.29/$0.59, not the Cerebras rate of $0.70/$0.70.
-- Rename/replace the old entry with the Groq-accurate rate.
DELETE FROM model_prices WHERE model_name = 'qwen-3-32b';
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES
-- Groq-hosted Qwen3 32B
('qwen3-32b',  290000,  590000, true),
-- Qwen catch-all (Groq default)
-- Update to Groq rate; Cerebras will need an explicit 'cerebras/' prefix in model name
('qwen',       290000,  590000, true),
-- Llama 4 Scout — new model on Groq ($0.11/$0.34)
('llama-4-scout', 110000, 340000, true),
('llama-4-',      110000, 340000, true)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- ── Gemini additions ──────────────────────────────────────────────────────────
-- Gemini 3.x models now available.
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES
('gemini-3.5-flash',       1500000,  9000000, true),  -- $1.50 in / $9 out
('gemini-3.1-flash-lite',   250000,  1500000, true),  -- $0.25 in / $1.50 out
('gemini-3.1-pro',         2000000, 12000000, true),  -- $2.00 in / $12 out (lower tier)
('gemini-3-flash',          500000,  3000000, true),  -- $0.50 in / $3 out
('gemini-3-',               500000,  3000000, true)   -- catch-all for any gemini-3.x
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- Update the gemini- catch-all to reflect 2.5 Pro as the representative rate.
UPDATE model_prices
    SET input_rate_per_million = 1250000, output_rate_per_million = 10000000
    WHERE model_name = 'gemini-';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Revert Anthropic
DELETE FROM model_prices WHERE model_name IN ('claude-opus-4-', 'claude-opus-4-1', 'claude-haiku-4-', 'claude-fable-');
-- Revert o3
UPDATE model_prices SET input_rate_per_million = 10000000, output_rate_per_million = 40000000 WHERE model_name = 'o3';
-- Revert Groq Qwen
DELETE FROM model_prices WHERE model_name IN ('qwen3-32b', 'llama-4-scout', 'llama-4-');
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active)
    VALUES ('qwen-3-32b', 700000, 700000, true) ON CONFLICT DO NOTHING;
UPDATE model_prices SET input_rate_per_million = 700000, output_rate_per_million = 700000 WHERE model_name = 'qwen';
-- Revert Gemini additions
DELETE FROM model_prices WHERE model_name IN ('gemini-3.5-flash', 'gemini-3.1-flash-lite', 'gemini-3.1-pro', 'gemini-3-flash', 'gemini-3-');
UPDATE model_prices SET input_rate_per_million = 1250000, output_rate_per_million = 5000000 WHERE model_name = 'gemini-';

-- +goose StatementEnd
