-- +goose Up
-- Rename credit_transactions.amount_credits -> amount_mc.
--
-- The column has ALWAYS stored micro-cents: `ChargeUser` writes
-- `AmountCredits: -userChargedMc` directly. Only the name said "credits".
-- Migration 00013 renamed the balance columns to the _mc suffix when the unit
-- was settled but missed this one, leaving a ledger column whose name is off by
-- a factor of 10,000 from what it holds (1 credit = 1 cent = 10,000 mc).
--
-- No data changes — every stored value was already correct. This is purely
-- removing a trap: anyone reading `amount_credits` as credits would be out by
-- four orders of magnitude.
--
-- NOTE FOR OPERATORS: if you have external dashboards, saved SQL, or analytics
-- querying credit_transactions.amount_credits, update them to amount_mc.
ALTER TABLE credit_transactions RENAME COLUMN amount_credits TO amount_mc;

COMMENT ON COLUMN credit_transactions.amount_mc IS
    'Micro-cents ($1 = 1,000,000 mc). Negative for consumption, positive for '
    'deposits. NOT "credits" — a displayed credit is 1 cent = 10,000 mc.';

-- +goose Down
ALTER TABLE credit_transactions RENAME COLUMN amount_mc TO amount_credits;
