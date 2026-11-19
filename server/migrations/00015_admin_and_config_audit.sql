-- +goose Up
-- +goose StatementBegin

-- Admin flag gates the configuration endpoints (model pricing, system constants).
ALTER TABLE users ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- Typed metadata for system_configs so the config endpoint can validate values
-- and expose them safely. value_type constrains parsing; description documents
-- each knob for operators.
ALTER TABLE system_configs
    ADD COLUMN IF NOT EXISTS value_type  VARCHAR(16) NOT NULL DEFAULT 'string'
        CHECK (value_type IN ('int','float','duration_sec','string','bool')),
    ADD COLUMN IF NOT EXISTS description TEXT        NOT NULL DEFAULT '';

-- Audit trail for every configuration change (who changed what, when).
CREATE TABLE IF NOT EXISTS config_audit_log (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id    UUID        NOT NULL REFERENCES users(id),
    entity      VARCHAR(32) NOT NULL,          -- 'model_price' | 'system_config'
    entity_key  VARCHAR(128) NOT NULL,         -- model_name or config_key
    old_value   JSONB,
    new_value   JSONB       NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_config_audit_entity ON config_audit_log (entity, entity_key, created_at DESC);

-- Seed the business-logic constants that were hardcoded in Go, so they become
-- runtime-configurable with types the endpoint can validate.
INSERT INTO system_configs (config_key, config_value, value_type, description) VALUES
    ('markup_numerator',        '5',      'int',          'Markup numerator (charged = api_cost * num / denom)'),
    ('markup_denominator',      '2',      'int',          'Markup denominator'),
    ('min_required_credits_mc', '5000',   'int',          'Minimum micro-cents required to start a task (~$0.005)'),
    ('free_tier_model',         'gpt-5.4-mini', 'string',  'Model free-tier users are restricted to'),
    ('quota_5h_duration_sec',   '18000',  'duration_sec', 'Rolling session window length (5h)'),
    ('quota_5h_limit_tokens',   '500000', 'int',          'Max tokens per rolling 5h session window'),
    ('quota_7d_duration_sec',   '604800', 'duration_sec', 'Rolling week window length (7d)'),
    ('quota_7d_limit_tokens',   '2000000','int',          'Max tokens per rolling 7d window')
ON CONFLICT (config_key) DO UPDATE
    SET value_type = EXCLUDED.value_type,
        description = EXCLUDED.description;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE IF EXISTS config_audit_log;
ALTER TABLE system_configs DROP COLUMN IF EXISTS value_type, DROP COLUMN IF EXISTS description;
ALTER TABLE users DROP COLUMN IF EXISTS is_admin;
-- +goose StatementEnd
