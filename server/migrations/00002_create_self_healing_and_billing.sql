-- +goose Up
-- +goose StatementBegin
CREATE TABLE block_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    command TEXT NOT NULL,
    cwd TEXT NOT NULL,
    exit_code INT NOT NULL,
    output_text TEXT NOT NULL,
    output_truncated BOOLEAN NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE TABLE block_analyses (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    block_event_id UUID NOT NULL REFERENCES block_events(id) ON DELETE CASCADE,
    explanation TEXT NOT NULL,
    suggested_command TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE TABLE user_balances (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    balance_credits BIGINT NOT NULL DEFAULT 500000, -- Default credit grant (e.g. $0.50)
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE TABLE model_prices (
    model_name VARCHAR(100) PRIMARY KEY, -- e.g. 'claude-3-5-sonnet', 'gpt-4o'
    input_rate_per_million BIGINT NOT NULL,  -- Micro-cents per million input tokens
    output_rate_per_million BIGINT NOT NULL, -- Micro-cents per million output tokens
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE credit_transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    amount_credits BIGINT NOT NULL, -- negative for consumption, positive for deposits
    description VARCHAR(255) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE TABLE system_configs (
    config_key VARCHAR(100) PRIMARY KEY,
    config_value VARCHAR(255) NOT NULL,
    description VARCHAR(255),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Indices for performance
CREATE INDEX idx_block_events_user ON block_events(user_id);
CREATE INDEX idx_block_analyses_event ON block_analyses(block_event_id);
CREATE INDEX idx_credit_transactions_user ON credit_transactions(user_id);

-- Seed default pricing rates
INSERT INTO model_prices (model_name, input_rate_per_million, output_rate_per_million, is_active) VALUES
('claude-3-5-sonnet', 3000000, 15000000, true), -- $3.00 input, $15.00 output per million tokens
('gpt-4o', 5000000, 15000000, true),            -- $5.00 input, $15.00 output per million tokens
('gpt-4o-mini', 150000, 600000, true);          -- $0.15 input, $0.60 output per million tokens

-- Seed initial default quota limits
INSERT INTO system_configs (config_key, config_value, description) VALUES
('quota_5h_duration_sec', '18000', 'Duration of the short-term rolling window in seconds (5 hours)'),
('quota_5h_limit_tokens', '500000', 'Token limit for the short-term rolling window'),
('quota_7d_duration_sec', '604800', 'Duration of the long-term rolling window in seconds (7 days)'),
('quota_7d_limit_tokens', '2000000', 'Token limit for the long-term rolling window');
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP INDEX IF EXISTS idx_credit_transactions_user;
DROP INDEX IF EXISTS idx_block_analyses_event;
DROP INDEX IF EXISTS idx_block_events_user;

DROP TABLE IF EXISTS system_configs;
DROP TABLE IF EXISTS credit_transactions;
DROP TABLE IF EXISTS model_prices;
DROP TABLE IF EXISTS user_balances;
DROP TABLE IF EXISTS block_analyses;
DROP TABLE IF EXISTS block_events;
-- +goose StatementEnd
