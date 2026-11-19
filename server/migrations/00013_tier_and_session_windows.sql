-- +goose Up
-- +goose StatementBegin

-- ── Subscription tier ─────────────────────────────────────────────────────────
ALTER TABLE user_balances
    ADD COLUMN tier VARCHAR(10) NOT NULL DEFAULT 'free'
        CHECK (tier IN ('free', 'pro', 'ultra', 'mega'));

-- ── Rolling window anchor ─────────────────────────────────────────────────────
-- The start point for every rolling window calculation.
-- Free users:  set to users.created_at and never changes.
-- Paid users:  overwritten to the exact payment timestamp on every billing event.
--              When a user pays on Tuesday 3:45 PM, their week starts Tuesday 3:45 PM.
--              When they next pay on Wednesday 11:20 AM, it resets to that timestamp.
ALTER TABLE user_balances
    ADD COLUMN week_anchor_at    TIMESTAMPTZ DEFAULT NULL,
    ADD COLUMN current_week_start TIMESTAMPTZ DEFAULT NULL;

-- ── PAID credit pool ──────────────────────────────────────────────────────────
-- Subscription credits. These are consumed FIRST on every request.
-- Fallback to the free pool only when this pool's weekly cap is exhausted.
--
-- No rollover rule: at paid_monthly_reset_at the server sets
--   paid_balance_mc = paid_monthly_cap_mc  (forfeiting whatever was left)
-- and advances paid_monthly_reset_at by 30 days.
--
-- Rename the existing balance_credits column to make its role explicit.
ALTER TABLE user_balances RENAME COLUMN balance_credits TO paid_balance_mc;

ALTER TABLE user_balances
    -- Monthly allocation for this tier. Reset target on billing renewal.
    ADD COLUMN paid_monthly_cap_mc    BIGINT      NOT NULL DEFAULT 0,
    -- Next billing renewal timestamp. Advances by 30 days on each renewal.
    ADD COLUMN paid_monthly_reset_at  TIMESTAMPTZ DEFAULT NULL,
    -- Weekly slice of the monthly allocation (monthly / 4).
    -- Determines how fast paid credits can be spent within a single week.
    ADD COLUMN paid_weekly_cap_mc     BIGINT      NOT NULL DEFAULT 0,
    ADD COLUMN paid_weekly_used_mc    BIGINT      NOT NULL DEFAULT 0,
    -- 5-hour burst cap within the weekly window (weekly / 6).
    ADD COLUMN paid_session_cap_mc    BIGINT      NOT NULL DEFAULT 0,
    ADD COLUMN paid_session_used_mc   BIGINT      NOT NULL DEFAULT 0;

-- ── FREE credit pool ──────────────────────────────────────────────────────────
-- Weekly baseline available to EVERY user regardless of tier.
-- For free users: this is their only pool.
-- For paid users: this is the fallback once their paid weekly cap is exhausted.
--
-- Resets every 7 days from week_anchor_at (same rolling clock as paid).
-- No rollover: unused free credits are forfeited at week end.
--
-- Caps are the same for every tier — upgrading adds PAID capacity, not free.
ALTER TABLE user_balances
    ADD COLUMN free_weekly_cap_mc     BIGINT      NOT NULL DEFAULT 375000, -- $0.375/week
    ADD COLUMN free_weekly_used_mc    BIGINT      NOT NULL DEFAULT 0,
    ADD COLUMN free_session_cap_mc    BIGINT      NOT NULL DEFAULT 62500,  -- 375000/6
    ADD COLUMN free_session_used_mc   BIGINT      NOT NULL DEFAULT 0;

-- ── Shared session clock ──────────────────────────────────────────────────────
-- One 5-hour rolling window per user. Both pools share the same clock.
-- When NOW() > session_start_at + 5 hours, both session_used counters reset
-- and session_start_at is set to NOW() on the next incoming request.
ALTER TABLE user_balances
    ADD COLUMN session_start_at TIMESTAMPTZ DEFAULT NULL;

-- ── Backfill: week anchor ─────────────────────────────────────────────────────
UPDATE user_balances ub
    SET week_anchor_at     = u.created_at,
        current_week_start = u.created_at
    FROM users u
    WHERE ub.user_id = u.id;

-- ── Backfill: paid pool caps per tier ────────────────────────────────────────
-- Free accounts have no paid pool.
UPDATE user_balances SET
    paid_monthly_cap_mc  = 0,
    paid_weekly_cap_mc   = 0,
    paid_session_cap_mc  = 0
WHERE tier = 'free';

-- Existing is_paid = TRUE accounts default to Pro caps.
-- The real tier will be set by the billing system on next payment.
UPDATE user_balances SET
    tier                  = 'pro',
    paid_monthly_cap_mc   = 8000000,   -- $8.00 API budget/month (40% of $20)
    paid_weekly_cap_mc    = 2000000,   -- 8,000,000 / 4
    paid_session_cap_mc   = 333333,    -- 2,000,000 / 6
    paid_monthly_reset_at = week_anchor_at + INTERVAL '30 days'
WHERE is_paid = TRUE;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

ALTER TABLE user_balances RENAME COLUMN paid_balance_mc TO balance_credits;

ALTER TABLE user_balances
    DROP COLUMN IF EXISTS tier,
    DROP COLUMN IF EXISTS week_anchor_at,
    DROP COLUMN IF EXISTS current_week_start,
    DROP COLUMN IF EXISTS paid_monthly_cap_mc,
    DROP COLUMN IF EXISTS paid_monthly_reset_at,
    DROP COLUMN IF EXISTS paid_weekly_cap_mc,
    DROP COLUMN IF EXISTS paid_weekly_used_mc,
    DROP COLUMN IF EXISTS paid_session_cap_mc,
    DROP COLUMN IF EXISTS paid_session_used_mc,
    DROP COLUMN IF EXISTS free_weekly_cap_mc,
    DROP COLUMN IF EXISTS free_weekly_used_mc,
    DROP COLUMN IF EXISTS free_session_cap_mc,
    DROP COLUMN IF EXISTS free_session_used_mc,
    DROP COLUMN IF EXISTS session_start_at;

-- +goose StatementEnd
