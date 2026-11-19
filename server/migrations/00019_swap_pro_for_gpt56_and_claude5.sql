-- +goose Up
-- +goose StatementBegin

-- Model catalog refresh (July 2026):
--   • Remove the never-user-facing OpenAI *pro* rows (gpt-5.4-pro, gpt-5.5-pro).
--   • Add the OpenAI GPT-5.6 family: Sol / Terra / Luna (GA 2026-07-09).
--   • Add the Anthropic Claude 5 family: Fable 5 + Sonnet 5.
--
-- Rate unit: micro-cents per million tokens ($1.00/M = 1,000,000 mc).
-- Cache read = the discounted cached-input rate CalculateCost bills against.
-- Cache write = informational (1.25× input); NOT used by CalculateCost today.

-- ── Remove the OpenAI pro rows ────────────────────────────────────────────────
DELETE FROM model_prices WHERE model_name IN ('gpt-5.4-pro', 'gpt-5.5-pro');

-- ── OpenAI GPT-5.6 family ─────────────────────────────────────────────────────
-- Sol   $5.00/$30.00 · Terra $2.50/$15.00 · Luna $1.00/$6.00
-- Cache read = 10% of input (90% discount); cache write = 1.25× input.
-- ⚠️ context_window_tokens = 400000 is a PLACEHOLDER (OpenAI window not yet
--    confirmed for 5.6) — correct it once verified; it drives the client's
--    context-budget UI.
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cached_input_rate_per_million,
    cache_write_rate_per_million,
    output_rate_per_million,
    context_window_tokens,
    is_active,
    is_user_facing
) VALUES
    ('gpt-5.6-sol',   5000000,  500000, 6250000, 30000000, 400000, TRUE, TRUE),
    ('gpt-5.6-terra', 2500000,  250000, 3125000, 15000000, 400000, TRUE, TRUE),
    ('gpt-5.6-luna',  1000000,  100000, 1250000,  6000000, 400000, TRUE, TRUE)
ON CONFLICT (model_name) DO UPDATE SET
    input_rate_per_million        = EXCLUDED.input_rate_per_million,
    cached_input_rate_per_million = EXCLUDED.cached_input_rate_per_million,
    cache_write_rate_per_million  = EXCLUDED.cache_write_rate_per_million,
    output_rate_per_million       = EXCLUDED.output_rate_per_million,
    context_window_tokens         = EXCLUDED.context_window_tokens,
    is_active                     = EXCLUDED.is_active,
    is_user_facing                = EXCLUDED.is_user_facing;

-- ── Anthropic Claude 5 family ─────────────────────────────────────────────────
-- Fable 5:  $10.00/$50.00 (exactly 2× Opus 4.8), cache read $1.00 (90% off),
--           cache write $12.50 (1.25×), 1M context, 128K max output.
-- Sonnet 5: INTRODUCTORY $2.00/$10.00 through 2026-08-31, cache read $0.20,
--           cache write $2.50 (1.25×), 1M context.
--   ⚠️ Sonnet 5 STANDARD pricing starts 2026-09-01: input 3,000,000, cached
--      300,000, cache write 3,750,000, output 15,000,000. The billing code has
--      no time-based pricing — a follow-up migration MUST bump these on Sep 1.
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cached_input_rate_per_million,
    cache_write_rate_per_million,
    output_rate_per_million,
    context_window_tokens,
    is_active,
    is_user_facing
) VALUES
    ('claude-fable-5',  10000000, 1000000, 12500000, 50000000, 1000000, TRUE, TRUE),
    ('claude-sonnet-5',  2000000,  200000,  2500000, 10000000, 1000000, TRUE, TRUE)
ON CONFLICT (model_name) DO UPDATE SET
    input_rate_per_million        = EXCLUDED.input_rate_per_million,
    cached_input_rate_per_million = EXCLUDED.cached_input_rate_per_million,
    cache_write_rate_per_million  = EXCLUDED.cache_write_rate_per_million,
    output_rate_per_million       = EXCLUDED.output_rate_per_million,
    context_window_tokens         = EXCLUDED.context_window_tokens,
    is_active                     = EXCLUDED.is_active,
    is_user_facing                = EXCLUDED.is_user_facing;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

DELETE FROM model_prices WHERE model_name IN (
    'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna',
    'claude-fable-5', 'claude-sonnet-5'
);

-- Restore the pro rows exactly as seeded in migration 00006.
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cached_input_rate_per_million,
    output_rate_per_million,
    long_context_threshold,
    long_context_input_rate_per_million,
    long_context_cached_input_rate_per_million,
    long_context_output_rate_per_million,
    is_active
) VALUES
    ('gpt-5.4-pro', 30000000, NULL, 180000000, 128000, 60000000, NULL, 270000000, true),
    ('gpt-5.5-pro', 30000000, NULL, 180000000, 128000, 60000000, NULL, 270000000, true)
ON CONFLICT (model_name) DO NOTHING;

-- +goose StatementEnd
