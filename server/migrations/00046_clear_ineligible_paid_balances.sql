-- +goose Up
-- +goose StatementBegin

-- A free/non-paying account must never spend from the paid monthly pool.
--
-- Older account rows can retain paid_balance_mc after a downgrade or from a
-- pre-subscription-era credit grant even though is_paid is false. The runtime
-- now guards paid-pool eligibility with is_paid; clear the stale values too so
-- account responses cannot expose a negative/ghost paid usage calculation.
-- Renaming balance_credits to paid_balance_mc in migration 00013 preserved the
-- old 1,500,000 trial-grant DEFAULT. Remove that inherited default first so
-- accounts created after this cleanup do not immediately recreate the bug.
ALTER TABLE user_balances
    ALTER COLUMN paid_balance_mc SET DEFAULT 0;

UPDATE user_balances
SET paid_balance_mc = 0,
    paid_monthly_cap_mc = 0,
    paid_monthly_reset_at = NULL,
    updated_at = NOW()
WHERE is_paid = FALSE
  AND (
      paid_balance_mc <> 0
      OR paid_monthly_cap_mc <> 0
      OR paid_monthly_reset_at IS NOT NULL
  );

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Clearing ghost balances is intentionally irreversible: those values were
-- never paid entitlements. Keep the safe zero default on rollback so creating
-- a new free account cannot restore the original defect.
ALTER TABLE user_balances
    ALTER COLUMN paid_balance_mc SET DEFAULT 0;

-- +goose StatementEnd
