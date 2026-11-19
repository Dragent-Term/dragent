-- +goose Up
-- +goose StatementBegin

-- Add long-context and cached-input pricing columns.
-- NULL means "not applicable for this model".
ALTER TABLE model_prices
    ADD COLUMN cached_input_rate_per_million          BIGINT DEFAULT NULL,
    ADD COLUMN long_context_threshold                 BIGINT DEFAULT NULL, -- input tokens above which the long-context rate applies
    ADD COLUMN long_context_input_rate_per_million    BIGINT DEFAULT NULL,
    ADD COLUMN long_context_cached_input_rate_per_million BIGINT DEFAULT NULL,
    ADD COLUMN long_context_output_rate_per_million   BIGINT DEFAULT NULL;

-- Clean slate — remove every row and re-seed only the supported model list.
DELETE FROM model_prices;

-- All rates are in micro-cents per million tokens ($1.00 = 1,000,000).
--
-- Columns: model_name,
--          input_rate, cached_input_rate, output_rate,
--          lc_threshold, lc_input_rate, lc_cached_input_rate, lc_output_rate,
--          is_active

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

-- ── Anthropic ─────────────────────────────────────────────────────────────────
-- Full 1M context window at standard rates — no separate long-context tier.
-- Cached read = $0.50/M for Opus, $0.30/M for Sonnet (10% of base input rate).
('claude-opus-4-8',    5000000,  500000, 25000000, NULL, NULL, NULL, NULL, true),
('claude-sonnet-4-6',  3000000,  300000, 15000000, NULL, NULL, NULL, NULL, true),
-- Prefix catch-all for any future claude- variant not explicitly listed.
-- Defaults to Sonnet rates.
('claude-sonnet-',     3000000,  300000, 15000000, NULL, NULL, NULL, NULL, true),
('claude-opus-4-',     5000000,  500000, 25000000, NULL, NULL, NULL, NULL, true),
('claude-opus-',       5000000,  500000, 25000000, NULL, NULL, NULL, NULL, true),
('claude-',            3000000,  300000, 15000000, NULL, NULL, NULL, NULL, true),

-- ── OpenAI ────────────────────────────────────────────────────────────────────
-- Long-context tier kicks in above 128k input tokens.
-- gpt-5.5: short $5/$30, long $10/$45; cached short $0.50, cached long $1.00
('gpt-5.5',
    5000000,   500000, 30000000,
    128000,
    10000000, 1000000, 45000000,
    true),

-- gpt-5.4: short $2.50/$15, long $5/$22.50; cached short $0.25, cached long $0.50
('gpt-5.4',
    2500000,   250000, 15000000,
    128000,
    5000000,  500000, 22500000,
    true),

-- gpt-5.4-mini: $0.75/$4.50, cached $0.075 — no long-context tier published
('gpt-5.4-mini',   750000,  75000, 4500000, NULL, NULL, NULL, NULL, true),

-- gpt-5.4-nano and gpt-5.4-pro for completeness (not in the selector but may be
-- used by compat providers or API callers directly).
('gpt-5.4-nano',   200000,  20000, 1250000, NULL, NULL, NULL, NULL, true),
('gpt-5.4-pro',  30000000,    NULL, 180000000,
    128000,
    60000000, NULL, 270000000,
    true),
('gpt-5.5-pro',  30000000,    NULL, 180000000,
    128000,
    60000000, NULL, 270000000,
    true),

-- gpt- prefix catch-all for any gpt model not listed above.
('gpt-',           2500000,  250000, 15000000, NULL, NULL, NULL, NULL, true),

-- ── Google Gemini ─────────────────────────────────────────────────────────────
-- Confirmed from Google AI pricing page.
('gemini-3.5-flash',  1500000, NULL,  9000000, NULL, NULL, NULL, NULL, true),
('gemini-3.1-pro',    2000000, NULL, 12000000, NULL, NULL, NULL, NULL, true),
-- Catch-alls for older/other Gemini variants.
('gemini-2.5-pro',    1250000, NULL, 10000000, NULL, NULL, NULL, NULL, true),
('gemini-2.5-flash',   300000, NULL,  2500000, NULL, NULL, NULL, NULL, true),
('gemini-',           1500000, NULL,  9000000, NULL, NULL, NULL, NULL, true);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

ALTER TABLE model_prices
    DROP COLUMN IF EXISTS cached_input_rate_per_million,
    DROP COLUMN IF EXISTS long_context_threshold,
    DROP COLUMN IF EXISTS long_context_input_rate_per_million,
    DROP COLUMN IF EXISTS long_context_cached_input_rate_per_million,
    DROP COLUMN IF EXISTS long_context_output_rate_per_million;

-- +goose StatementEnd
