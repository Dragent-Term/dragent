-- +goose Up
-- +goose StatementBegin

-- BYOK keys are passed via request headers (X-User-Api-Key, X-Custom-Base-Url,
-- X-Custom-Model) and never persisted. This table is no longer used.
DROP TABLE IF EXISTS user_api_keys;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

CREATE TABLE user_api_keys (
    id                UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider          VARCHAR(50)  NOT NULL
                          CHECK (provider IN ('anthropic','openai','google','xai','openai_compat')),
    api_key_encrypted TEXT         NOT NULL,
    base_url          VARCHAR(500) DEFAULT NULL,
    label             VARCHAR(100) DEFAULT NULL,
    is_active         BOOLEAN      NOT NULL DEFAULT TRUE,
    last_used_at      TIMESTAMPTZ  DEFAULT NULL,
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_user_api_keys_one_active
    ON user_api_keys(user_id, provider)
    WHERE is_active = TRUE;

CREATE INDEX idx_user_api_keys_user
    ON user_api_keys(user_id);

-- +goose StatementEnd
