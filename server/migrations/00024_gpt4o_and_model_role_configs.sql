-- +goose Up
-- +goose StatementBegin

-- 1) Catalog gpt-4o so metered calls that resolve to it are priced instead of
--    refused with ErrUnknownModel. Internal-task model only (is_user_facing =
--    FALSE) — it is selectable by config, not by end users.
--    OpenAI gpt-4o standard rates (verified): $2.50 in / $1.25 cached / $10 out,
--    128K context. Rate unit: micro-cents per million tokens ($1.00/M = 1e6 mc).
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cached_input_rate_per_million,
    output_rate_per_million,
    context_window_tokens,
    min_balance_mc,
    is_active,
    is_user_facing
) VALUES (
    'gpt-4o',
    2500000,    -- $2.50/M input
    1250000,    -- $1.25/M cache read
    10000000,   -- $10.00/M output
    128000,     -- 128K context
    25000,      -- ~$0.025 minimum (one small step ×2.5 markup)
    TRUE, FALSE
)
ON CONFLICT (model_name) DO UPDATE SET
    input_rate_per_million        = EXCLUDED.input_rate_per_million,
    cached_input_rate_per_million = EXCLUDED.cached_input_rate_per_million,
    output_rate_per_million       = EXCLUDED.output_rate_per_million,
    context_window_tokens         = EXCLUDED.context_window_tokens,
    min_balance_mc                = EXCLUDED.min_balance_mc,
    is_active                     = EXCLUDED.is_active,
    is_user_facing                = EXCLUDED.is_user_facing;

-- 2) Move hardcoded internal-task model names into system_configs so a
--    deprecated model can be swapped in the DB without a code change. Each
--    value must name a row that exists in model_prices (metered) or an
--    Anthropic model (web extraction, which calls the Anthropic API directly).
--    (free_tier_model already exists from migration 00015.)
INSERT INTO system_configs (config_key, config_value, value_type, description) VALUES
    ('default_agent_model', 'claude-sonnet-4-6',            'string', 'Fallback agent model when the client sends no model name'),
    ('selfheal_model',      'gpt-5.4-mini',                 'string', 'Model used to diagnose failed terminal commands (self-heal)'),
    ('web_extract_model',   'claude-haiku-4-5-20251001',    'string', 'Anthropic model used to extract main content from fetched web pages (must be an Anthropic model)')
ON CONFLICT (config_key) DO UPDATE
    SET value_type  = EXCLUDED.value_type,
        description = EXCLUDED.description;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DELETE FROM system_configs WHERE config_key IN ('default_agent_model', 'selfheal_model', 'web_extract_model');
DELETE FROM model_prices WHERE model_name = 'gpt-4o';
-- +goose StatementEnd
