-- +goose Up
-- +goose StatementBegin

-- GPT-5.6 Luna is the model backed by the monthly free allowance. The agent
-- gate reads this value at runtime, so this migration updates existing
-- deployments as well as providing the correct value for fresh databases.
INSERT INTO system_configs (config_key, config_value, value_type, description)
VALUES (
    'free_tier_model',
    'gpt-5.6-luna',
    'string',
    'Model users without premium-capable funds are restricted to'
)
ON CONFLICT (config_key) DO UPDATE SET
    config_value = EXCLUDED.config_value,
    value_type = EXCLUDED.value_type,
    description = EXCLUDED.description;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

UPDATE system_configs
SET config_value = 'gpt-5.4-mini'
WHERE config_key = 'free_tier_model';

-- +goose StatementEnd
