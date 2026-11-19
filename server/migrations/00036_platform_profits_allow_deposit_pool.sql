-- +goose Up
-- +goose StatementBegin

-- FIX: credit (deposit) usage was silently given away for free.
--
-- platform_profits.pool was created (migration 00011) with a CHECK constraint
-- that only permits ('paid', 'free') — while credit_transactions.pool permits
-- ('paid', 'free', 'deposit', 'refund'). Settlement writes BOTH rows in a single
-- transaction, tagged with the pool that funded the step. Once a user's monthly
-- allowance is exhausted and they opt in to spending purchased credits, steps
-- settle against the 'deposit' pool:
--
--   1. user_balances.deposit_balance_mc is decremented,
--   2. the credit_transactions insert (pool='deposit') succeeds,
--   3. the platform_profits insert (pool='deposit') VIOLATES this CHECK,
--   4. the whole transaction rolls back — undoing the decrement,
--   5. Settle() swallows the error ("step NOT billed").
--
-- Net effect: every credit-funded step ran for free and the deposit balance
-- never moved. This is a direct revenue loss. It stayed hidden because 'paid'
-- and 'free' pool charges pass the constraint — the bug only bites once the
-- deposit pool becomes the settling pool.
--
-- Bring the constraint in line with credit_transactions.pool (and with the
-- table's own comment: "pool mirrors credit_transactions.pool"). The inline
-- CHECK from CREATE TABLE is auto-named platform_profits_pool_check.

ALTER TABLE platform_profits
    DROP CONSTRAINT IF EXISTS platform_profits_pool_check;

ALTER TABLE platform_profits
    ADD CONSTRAINT platform_profits_pool_check
        CHECK (pool IN ('paid', 'free', 'deposit', 'refund'));

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Restore the original (narrower) constraint. NOTE: this will fail if any
-- 'deposit'/'refund' rows exist by the time it runs — which is the whole point
-- of the fix, so a rollback is only clean before any credit-funded settlement.
ALTER TABLE platform_profits
    DROP CONSTRAINT IF EXISTS platform_profits_pool_check;

ALTER TABLE platform_profits
    ADD CONSTRAINT platform_profits_pool_check
        CHECK (pool IN ('paid', 'free'));

-- +goose StatementEnd
