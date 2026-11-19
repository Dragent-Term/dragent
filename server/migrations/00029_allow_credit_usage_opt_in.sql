-- +goose Up
-- Opt-in gate for spending purchased credits (the deposit pool).
--
-- Before this, deposit credits were spent automatically and unconditionally
-- whenever the paid pool could not serve a call — INCLUDING when the paid pool
-- was blocked only by the rolling 5-hour session or weekly window while the
-- monthly allowance was still intact. A user throttled by a window silently
-- spent real money instead of being told to wait for the window to roll.
--
-- Default FALSE: credits are never touched until the user explicitly turns
-- this on. Existing users therefore stop auto-spending on the next deploy,
-- which is the safe direction to fail — a blocked request is recoverable, an
-- unwanted charge is not.
ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS allow_credit_usage BOOLEAN NOT NULL DEFAULT FALSE;

COMMENT ON COLUMN user_balances.allow_credit_usage IS
    'User opt-in for spending the deposit (purchased credit) pool. When false, '
    'the deposit pool is skipped entirely and the user is told which window or '
    'pool blocked them instead.';

-- +goose Down
ALTER TABLE user_balances DROP COLUMN IF EXISTS allow_credit_usage;
