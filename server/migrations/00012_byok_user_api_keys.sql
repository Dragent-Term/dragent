-- +goose Up
-- +goose StatementBegin

-- ── user_api_keys (Bring Your Own Key) ───────────────────────────────────────
-- Stores encrypted API keys that Pro+ users supply for their own AI provider
-- accounts. When a BYOK key is active for a request:
--   • We route the call through the user's key, not ours.
--   • No credit_transaction is created (user is not charged).
--   • No platform_profits row is created (no revenue on BYOK calls).
--
-- Enforcement: only users with is_paid = TRUE (Pro, Ultra, Mega) may add keys.
-- This is checked at the API layer before any INSERT here.
--
-- provider values:
--   'anthropic'      — official https://api.anthropic.com
--   'openai'         — official https://api.openai.com
--   'google'         — official Google AI Studio / Vertex
--   'xai'            — official https://api.x.ai
--   'openai_compat'  — any OpenAI-compatible endpoint (base_url required)
CREATE TABLE user_api_keys (
    id                UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider          VARCHAR(50)  NOT NULL
                          CHECK (provider IN ('anthropic','openai','google','xai','openai_compat')),
    api_key_encrypted TEXT         NOT NULL,     -- AES-256 encrypted before storing
    base_url          VARCHAR(500) DEFAULT NULL, -- NULL = official endpoint
                                                 -- required when provider = 'openai_compat'
    label             VARCHAR(100) DEFAULT NULL, -- optional user-given name
    is_active         BOOLEAN      NOT NULL DEFAULT TRUE,
    last_used_at      TIMESTAMPTZ  DEFAULT NULL,
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- One active key per provider per user. A user can deactivate and add a new key
-- for the same provider, but cannot have two active keys for the same provider.
CREATE UNIQUE INDEX idx_user_api_keys_one_active
    ON user_api_keys(user_id, provider)
    WHERE is_active = TRUE;

CREATE INDEX idx_user_api_keys_user
    ON user_api_keys(user_id);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

DROP INDEX IF EXISTS idx_user_api_keys_one_active;
DROP INDEX IF EXISTS idx_user_api_keys_user;
DROP TABLE IF EXISTS user_api_keys;

-- +goose StatementEnd
