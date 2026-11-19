-- +goose Up
-- +goose StatementBegin

-- Grok 4.3 from xAI ($1.25 input / $2.50 output per million tokens).
INSERT INTO model_prices (
    model_name,
    input_rate_per_million, cached_input_rate_per_million, output_rate_per_million,
    long_context_threshold,
    long_context_input_rate_per_million,
    long_context_cached_input_rate_per_million,
    long_context_output_rate_per_million,
    is_active
) VALUES
('grok-4.3', 1250000, NULL, 2500000, NULL, NULL, NULL, NULL, true)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

DELETE FROM model_prices WHERE model_name = 'grok-4.3';

-- +goose StatementEnd
