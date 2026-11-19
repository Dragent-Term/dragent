-- +goose Up
-- +goose StatementBegin

-- Track whether a user has ever added payment beyond the free trial grant.
-- Free-tier users (is_paid = false) are restricted to gpt-5.4-mini by the server.
ALTER TABLE user_balances ADD COLUMN is_paid BOOLEAN NOT NULL DEFAULT FALSE;

-- Llama 4 Scout — latest Groq-hosted model ($0.11 in / $0.34 out per million tokens).
INSERT INTO model_prices (
    model_name,
    input_rate_per_million, cached_input_rate_per_million, output_rate_per_million,
    long_context_threshold,
    long_context_input_rate_per_million,
    long_context_cached_input_rate_per_million,
    long_context_output_rate_per_million,
    is_active
) VALUES
('llama-4-scout', 110000, NULL, 340000, NULL, NULL, NULL, NULL, true)
ON CONFLICT (model_name) DO UPDATE
    SET input_rate_per_million  = EXCLUDED.input_rate_per_million,
        output_rate_per_million = EXCLUDED.output_rate_per_million,
        is_active               = EXCLUDED.is_active;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

ALTER TABLE user_balances DROP COLUMN IF EXISTS is_paid;
DELETE FROM model_prices WHERE model_name = 'llama-4-scout';

-- +goose StatementEnd
