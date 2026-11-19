-- +goose Up
-- +goose StatementBegin

-- ── New columns on model_prices ───────────────────────────────────────────────
--
-- cache_write_rate_per_million:
--   Anthropic charges 1.25× the base input rate to write a prompt cache entry.
--   NULL for providers that do not support prompt caching.
--
-- context_window_tokens:
--   Maximum input + output tokens the model supports. NULL = no published limit.
--
-- is_user_facing:
--   TRUE  → this exact model ID appears in the frontend model selector.
--   FALSE → pricing catch-all / prefix row used only for cost lookup.
ALTER TABLE model_prices
    ADD COLUMN cache_write_rate_per_million BIGINT     DEFAULT NULL,
    ADD COLUMN context_window_tokens        BIGINT     DEFAULT NULL,
    ADD COLUMN is_user_facing               BOOLEAN    NOT NULL DEFAULT FALSE;

-- ── claude-haiku-4-5 ──────────────────────────────────────────────────────────
-- Fastest, most cost-efficient Anthropic model.
-- Input $1.00/M · Cache write $1.25/M · Cache read $0.10/M · Output $5.00/M
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cache_write_rate_per_million,
    cached_input_rate_per_million,
    output_rate_per_million,
    long_context_threshold,
    long_context_input_rate_per_million,
    long_context_cached_input_rate_per_million,
    long_context_output_rate_per_million,
    is_active,
    is_user_facing
) VALUES (
    'claude-haiku-4-5',
    1000000,    -- $1.00/M input
    1250000,    -- $1.25/M cache write
    100000,     -- $0.10/M cache read
    5000000,    -- $5.00/M output
    NULL, NULL, NULL, NULL,
    TRUE, TRUE
)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million        = EXCLUDED.input_rate_per_million,
        cache_write_rate_per_million  = EXCLUDED.cache_write_rate_per_million,
        cached_input_rate_per_million = EXCLUDED.cached_input_rate_per_million,
        output_rate_per_million       = EXCLUDED.output_rate_per_million,
        is_active                     = EXCLUDED.is_active,
        is_user_facing                = EXCLUDED.is_user_facing;

-- ── grok-build-0.1 ────────────────────────────────────────────────────────────
-- xAI fast coding model for agentic workflows.
-- Input $1.00/M · Output $2.00/M · 256K context window
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cached_input_rate_per_million,
    output_rate_per_million,
    context_window_tokens,
    is_active,
    is_user_facing
) VALUES (
    'grok-build-0.1',
    1000000,   -- $1.00/M input
    NULL,
    2000000,   -- $2.00/M output
    262144,    -- 256K context window
    TRUE, TRUE
)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        context_window_tokens   = EXCLUDED.context_window_tokens,
        is_active               = EXCLUDED.is_active,
        is_user_facing          = EXCLUDED.is_user_facing;

-- ── Mark the 11 supported user-facing models ─────────────────────────────────
-- All other rows are pricing catch-alls (prefixes) and must NOT appear in
-- the frontend model selector.
UPDATE model_prices
    SET is_user_facing = TRUE
WHERE model_name IN (
    -- Anthropic
    'claude-opus-4-8',
    'claude-sonnet-4-6',
    'claude-haiku-4-5',
    -- OpenAI
    'gpt-5.5',
    'gpt-5.4',
    'gpt-5.4-mini',
    -- Google
    'gemini-3.5-flash',
    'gemini-3.1-pro',
    'gemini-2.5-flash',
    -- xAI
    'grok-4.3',
    'grok-build-0.1'
);

-- ── Set context windows for existing models ───────────────────────────────────
UPDATE model_prices SET context_window_tokens = 1048576  WHERE model_name = 'grok-4.3';          -- 1M
UPDATE model_prices SET context_window_tokens = 200000   WHERE model_name = 'claude-opus-4-8';   -- 200K
UPDATE model_prices SET context_window_tokens = 200000   WHERE model_name = 'claude-sonnet-4-6'; -- 200K
UPDATE model_prices SET context_window_tokens = 1048576  WHERE model_name = 'gemini-2.5-flash';  -- 1M
UPDATE model_prices SET context_window_tokens = 1048576  WHERE model_name = 'gemini-3.5-flash';  -- 1M
UPDATE model_prices SET context_window_tokens = 1048576  WHERE model_name = 'gemini-3.1-pro';    -- 1M
UPDATE model_prices SET context_window_tokens = 128000   WHERE model_name = 'gpt-5.4-mini';
UPDATE model_prices SET context_window_tokens = 128000   WHERE model_name = 'gpt-5.4';
UPDATE model_prices SET context_window_tokens = 128000   WHERE model_name = 'gpt-5.5';

-- ── Backfill cache_write_rate for existing Anthropic models ──────────────────
-- Anthropic cache write = 1.25× standard input rate across all model families.
UPDATE model_prices
    SET cache_write_rate_per_million = ROUND(input_rate_per_million * 1.25)
WHERE model_name IN (
    'claude-opus-4-8',
    'claude-sonnet-4-6',
    'claude-sonnet-',
    'claude-opus-4-',
    'claude-opus-',
    'claude-'
);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

DELETE FROM model_prices
    WHERE model_name IN ('claude-haiku-4-5', 'grok-build-0.1');

UPDATE model_prices
    SET is_user_facing = FALSE;

ALTER TABLE model_prices
    DROP COLUMN IF EXISTS cache_write_rate_per_million,
    DROP COLUMN IF EXISTS context_window_tokens,
    DROP COLUMN IF EXISTS is_user_facing;

-- +goose StatementEnd
