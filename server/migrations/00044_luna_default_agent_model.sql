-- +goose Up
-- +goose StatementBegin

-- GPT-5.6 Luna is the default model exposed to new/free users. Keep this as a
-- migration so existing deployments do not retain the older Claude default.
UPDATE system_configs
SET config_value = 'gpt-5.6-luna'
WHERE config_key = 'default_agent_model';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

UPDATE system_configs
SET config_value = 'claude-sonnet-4-6'
WHERE config_key = 'default_agent_model';

-- +goose StatementEnd
