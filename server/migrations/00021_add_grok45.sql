-- +goose Up
-- +goose StatementBegin

-- Add xAI Grok 4.5. Confirmed from xAI docs (July 2026), 500K context window.
--   ≤200K: $2.00 in / $0.50 cached / $6.00 out
--   ≥200K: $4.00 in / $1.00 cached / $12.00 out
--   (once a request's prompt reaches 200k tokens, the WHOLE request bills at
--    the higher tier — handled by long_context_threshold in CalculateCost).
-- Rate unit: micro-cents per million tokens ($1.00/M = 1,000,000 mc).
-- No cache_write_rate: xAI publishes a cache READ discount only.
INSERT INTO model_prices (
    model_name,
    input_rate_per_million,
    cached_input_rate_per_million,
    output_rate_per_million,
    long_context_threshold,
    long_context_input_rate_per_million,
    long_context_cached_input_rate_per_million,
    long_context_output_rate_per_million,
    context_window_tokens,
    is_active,
    is_user_facing
) VALUES (
    'grok-4.5',
    2000000,    -- $2.00/M input
    500000,     -- $0.50/M cache read
    6000000,    -- $6.00/M output
    200000,     -- 200K tier threshold
    4000000,    -- $4.00/M input  (≥200K)
    1000000,    -- $1.00/M cached (≥200K)
    12000000,   -- $12.00/M output (≥200K)
    500000,     -- 500K context window
    TRUE, TRUE
)
ON CONFLICT (model_name) DO UPDATE SET
    input_rate_per_million                     = EXCLUDED.input_rate_per_million,
    cached_input_rate_per_million              = EXCLUDED.cached_input_rate_per_million,
    output_rate_per_million                    = EXCLUDED.output_rate_per_million,
    long_context_threshold                     = EXCLUDED.long_context_threshold,
    long_context_input_rate_per_million        = EXCLUDED.long_context_input_rate_per_million,
    long_context_cached_input_rate_per_million = EXCLUDED.long_context_cached_input_rate_per_million,
    long_context_output_rate_per_million       = EXCLUDED.long_context_output_rate_per_million,
    context_window_tokens                      = EXCLUDED.context_window_tokens,
    is_active                                  = EXCLUDED.is_active,
    is_user_facing                             = EXCLUDED.is_user_facing;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DELETE FROM model_prices WHERE model_name = 'grok-4.5';
-- +goose StatementEnd
