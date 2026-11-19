-- +goose Up
-- Give paid plans a REAL 5-hour session cap.
--
-- Activation was writing the monthly allowance into all three caps, so
-- paid_session_cap_mc == paid_monthly_cap_mc and the 5-hour window could never
-- bind — a subscriber could burn a whole month in one afternoon, while the
-- dashboard rendered a session meter that was mathematically unable to fill.
--
-- Deliberately NOT restoring migration 00013's weekly throttle. The plans are
-- sold as "$N of agent usage / month"; capping the week at monthly/4 means a
-- bursty user can never actually reach the amount they paid for, because the
-- weekly window does not roll over. The session cap alone achieves what we
-- want (no burning the month at once) without breaking that promise.
--
-- Per-plan rather than a global constant so a plan can be tuned in SQL.
-- NULL = derive from the allowance at activation time.
ALTER TABLE plans
    ADD COLUMN IF NOT EXISTS session_cap_mc BIGINT NULL;

COMMENT ON COLUMN plans.session_cap_mc IS
    'Spend ceiling for one rolling 5-hour window. NULL derives 10% of the '
    'allowance at activation ($2.00 on Pro''s $20). Ten full sessions reach the '
    'whole allowance, so none of it is stranded, while one runaway session '
    'cannot consume the month.';

UPDATE plans SET session_cap_mc = monthly_allowance_mc * 10 / 100 WHERE session_cap_mc IS NULL;

-- Backfill live subscribers whose session cap is currently the useless
-- allowance-sized value. Only touch rows that match the broken pattern, so a
-- hand-tuned cap set by an operator survives.
UPDATE user_balances b
SET    paid_session_cap_mc = GREATEST(b.paid_monthly_cap_mc * 10 / 100, 1)
WHERE  b.is_paid
  AND  b.paid_monthly_cap_mc > 0
  AND  b.paid_session_cap_mc >= b.paid_monthly_cap_mc;

-- +goose Down
ALTER TABLE plans DROP COLUMN IF EXISTS session_cap_mc;
