-- +goose Up
-- +goose StatementBegin

-- Record the provider's subscription status on the balance row.
--
-- advanceMonthlyRenewal grants a new monthly allowance purely on the calendar:
-- once paid_monthly_reset_at is in the past, it refills the balance. That is
-- correct only while the subscription is actually being paid for.
--
-- It is not, during dunning. A failed renewal leaves the subscription alive
-- with status 'past_due', the caps intact, and the reset date in the past —
-- exactly the state that triggers a grant. The user then received the full
-- allowance for a payment that never landed, and the catch-up loop re-granted
-- it every 30 days for as long as dunning continued.
--
-- (A cleanly ended subscription was never affected: EndSubscription NULLs the
-- reset date and zeroes the caps, so the clock cannot fire.)
ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS paid_status VARCHAR(32) NOT NULL DEFAULT '';

COMMENT ON COLUMN user_balances.paid_status IS
    'Provider subscription status as of the last webhook (active, on_trial, '
    'past_due, ...). Only active/on_trial may receive a clock-driven monthly '
    'renewal; past_due keeps ACCESS but must never be granted new allowance.';

-- Existing paid rows predate the column and have been renewing on the clock.
-- Treat them as active so nobody is stranded by the deploy; a real status
-- arrives with their next subscription webhook.
UPDATE user_balances SET paid_status = 'active' WHERE is_paid AND paid_status = '';

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE user_balances DROP COLUMN IF EXISTS paid_status;
-- +goose StatementEnd
