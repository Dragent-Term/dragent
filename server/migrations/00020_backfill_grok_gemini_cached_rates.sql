-- +goose Up
-- +goose StatementBegin

-- Backfill the cached-input rates that were left NULL for xAI (Grok) and Google
-- (Gemini) models. While NULL, CalculateCost's `if cachedRate == 0 { cachedRate
-- = inputRate }` fallback billed cached tokens at the FULL input rate — a real
-- overcharge (e.g. gemini-3.5-flash cached tokens billed at $1.50/M instead of
-- $0.15/M). All rates confirmed from provider docs (xAI + Google, July 2026).
-- Rate unit: micro-cents per million tokens ($1.00/M = 1,000,000 mc).
--
-- Grok models are also TIERED at a 200K-token prompt: at/above 200k every token
-- in the request bills at the higher rate. The billing code already supports
-- this via long_context_threshold, so we set the tier too.

-- ── xAI Grok (the models the user flagged) ────────────────────────────────────
-- grok-4.3   ≤200K: $1.25 / cached $0.20 / $2.50   ·  ≥200K: $2.50 / $0.40 / $5.00
UPDATE model_prices SET
    cached_input_rate_per_million              = 200000,   -- $0.20/M cache read
    long_context_threshold                     = 200000,
    long_context_input_rate_per_million        = 2500000,  -- $2.50/M
    long_context_cached_input_rate_per_million = 400000,   -- $0.40/M
    long_context_output_rate_per_million       = 5000000   -- $5.00/M
WHERE model_name = 'grok-4.3';

-- grok-build-0.1  ≤200K: $1.00 / cached $0.20 / $2.00  ·  ≥200K: $2.00 / $0.40 / $4.00
UPDATE model_prices SET
    cached_input_rate_per_million              = 200000,   -- $0.20/M cache read
    long_context_threshold                     = 200000,
    long_context_input_rate_per_million        = 2000000,  -- $2.00/M
    long_context_cached_input_rate_per_million = 400000,   -- $0.40/M
    long_context_output_rate_per_million       = 4000000   -- $4.00/M
WHERE model_name = 'grok-build-0.1';

-- ── Google Gemini (same NULL-cached gap) ──────────────────────────────────────
-- Flat-rate (no 200K tier):
UPDATE model_prices SET cached_input_rate_per_million = 30000    -- $0.03/M
    WHERE model_name = 'gemini-2.5-flash';
UPDATE model_prices SET cached_input_rate_per_million = 150000   -- $0.15/M
    WHERE model_name = 'gemini-3.5-flash';

-- gemini-3.1-pro  ≤200K: $2.00 / cached $0.20 / $12.00 · ≥200K: $4.00 / $0.40 / $18.00
UPDATE model_prices SET
    cached_input_rate_per_million              = 200000,   -- $0.20/M cache read
    long_context_threshold                     = 200000,
    long_context_input_rate_per_million        = 4000000,  -- $4.00/M
    long_context_cached_input_rate_per_million = 400000,   -- $0.40/M
    long_context_output_rate_per_million       = 18000000  -- $18.00/M
WHERE model_name = 'gemini-3.1-pro';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Revert to the pre-backfill state (NULL cached + no long-context tier).
UPDATE model_prices SET
    cached_input_rate_per_million              = NULL,
    long_context_threshold                     = NULL,
    long_context_input_rate_per_million        = NULL,
    long_context_cached_input_rate_per_million = NULL,
    long_context_output_rate_per_million       = NULL
WHERE model_name IN ('grok-4.3', 'grok-build-0.1', 'gemini-3.1-pro');

UPDATE model_prices SET cached_input_rate_per_million = NULL
    WHERE model_name IN ('gemini-2.5-flash', 'gemini-3.5-flash');

-- +goose StatementEnd
