-- +goose Up
-- +goose StatementBegin

-- Remove the 5-hour session window and the paid weekly throttle entirely.
--
-- The product now has a single budget dimension: the paid pool is the whole
-- monthly allowance, charged directly and usable at any pace; the free pool is
-- a weekly allowance, likewise usable at any pace. There is no rolling 5-hour
-- session and no weekly THROTTLE on the paid balance. Charges decrement the
-- monthly balance (or the free weekly used-counter) — never a session or
-- paid-weekly counter, because those no longer exist.
--
-- The free weekly ALLOWANCE stays (free_weekly_cap_mc / free_weekly_used_mc /
-- current_week_start) — that is the free tier's grant, not a throttle.

ALTER TABLE user_balances
    DROP COLUMN IF EXISTS paid_weekly_cap_mc,
    DROP COLUMN IF EXISTS paid_weekly_used_mc,
    DROP COLUMN IF EXISTS paid_session_cap_mc,
    DROP COLUMN IF EXISTS paid_session_used_mc,
    DROP COLUMN IF EXISTS free_session_cap_mc,
    DROP COLUMN IF EXISTS free_session_used_mc,
    DROP COLUMN IF EXISTS session_start_at;

-- The per-plan 5-hour session ceiling (migration 00031) is gone with the window.
ALTER TABLE plans
    DROP COLUMN IF EXISTS session_cap_mc;

-- The Redis token rate-limiter (CheckQuota) was dead code and is deleted; its
-- config keys no longer do anything. Harmless if absent.
DELETE FROM system_configs
 WHERE config_key IN (
   'quota_5h_duration_sec', 'quota_5h_limit_tokens',
   'quota_7d_duration_sec', 'quota_7d_limit_tokens'
 );

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
-- Irreversible in practice — the dropped per-user session/weekly counters
-- cannot be reconstructed. Re-add the columns empty so a rollback at least
-- restores the schema shape; behaviour would need the old code too.
ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS paid_weekly_cap_mc   BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS paid_weekly_used_mc  BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS paid_session_cap_mc  BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS paid_session_used_mc BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS free_session_cap_mc  BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS free_session_used_mc BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS session_start_at     TIMESTAMPTZ NULL;
ALTER TABLE plans
    ADD COLUMN IF NOT EXISTS session_cap_mc BIGINT NULL;
-- +goose StatementEnd
