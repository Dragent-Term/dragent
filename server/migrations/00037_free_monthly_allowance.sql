-- +goose Up
-- +goose StatementBegin

-- Convert the free allowance from a rolling WEEKLY window to a MONTHLY one,
-- usable at any pace — mirroring the paid pool (one allowance, no sub-window).
-- No weekly reset, no session throttle. The new grant is 1,500,000 mc/month
-- ($1.50 ≈ 150 credits).
--
--   free_weekly_cap_mc   → free_monthly_cap_mc   (default bumped to 1,500,000)
--   free_weekly_used_mc  → free_monthly_used_mc
--   (new)                  free_monthly_reset_at (when the used-counter resets)
--
-- The old weekly anchor columns are dropped: week_anchor_at / current_week_start.

ALTER TABLE user_balances
    RENAME COLUMN free_weekly_cap_mc TO free_monthly_cap_mc;
ALTER TABLE user_balances
    RENAME COLUMN free_weekly_used_mc TO free_monthly_used_mc;

ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS free_monthly_reset_at TIMESTAMPTZ;

-- New monthly grant for everyone, and a fresh one-month window from now. The
-- reset date carries forward the existing weekly anchor's remaining time only
-- loosely — a clean one-month-from-now window is fine and simpler.
UPDATE user_balances
   SET free_monthly_cap_mc   = 1500000,
       free_monthly_reset_at = COALESCE(free_monthly_reset_at, NOW() + INTERVAL '1 month');

ALTER TABLE user_balances
    ALTER COLUMN free_monthly_cap_mc SET DEFAULT 1500000;

-- Drop the now-unused weekly anchor columns.
ALTER TABLE user_balances
    DROP COLUMN IF EXISTS week_anchor_at,
    DROP COLUMN IF EXISTS current_week_start;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS week_anchor_at     TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS current_week_start TIMESTAMPTZ;

UPDATE user_balances
   SET current_week_start = COALESCE(current_week_start, NOW()),
       week_anchor_at     = COALESCE(week_anchor_at, NOW());

ALTER TABLE user_balances
    ALTER COLUMN free_monthly_cap_mc SET DEFAULT 375000;

ALTER TABLE user_balances
    RENAME COLUMN free_monthly_used_mc TO free_weekly_used_mc;
ALTER TABLE user_balances
    RENAME COLUMN free_monthly_cap_mc TO free_weekly_cap_mc;

ALTER TABLE user_balances
    DROP COLUMN IF EXISTS free_monthly_reset_at;

-- +goose StatementEnd
