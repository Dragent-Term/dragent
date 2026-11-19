-- +goose Up
-- +goose StatementBegin

-- Record whether the credit-usage setting was chosen BY THE USER.
--
-- A top-up purchase enables credit spending, on the reasoning that buying
-- credits is itself consent. But the upsert applied that unconditionally, so it
-- also overwrote a deliberate opt-out: a user who switched credits OFF in
-- Settings and later bought a pack to bank for future use had spending silently
-- switched back ON. That contradicts migration 00029's contract.
--
-- One boolean cannot express the difference between "never chose" and "chose
-- no", which is exactly the distinction the purchase path needs. This column
-- supplies it: purchases may only flip consent for users who never expressed a
-- preference.
ALTER TABLE user_balances
    ADD COLUMN IF NOT EXISTS credit_usage_set_by_user BOOLEAN NOT NULL DEFAULT FALSE;

COMMENT ON COLUMN user_balances.credit_usage_set_by_user IS
    'TRUE once the user has explicitly set allow_credit_usage themselves. A '
    'top-up purchase may enable credit spending only while this is FALSE — an '
    'explicit choice, in either direction, is never overwritten automatically.';

-- Anyone already opted in got there either by purchase or by using the toggle.
-- Treat existing opt-ins as explicit: the safe error is to respect a preference
-- that was actually automatic, not to overwrite one that was deliberate.
UPDATE user_balances SET credit_usage_set_by_user = TRUE WHERE allow_credit_usage;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE user_balances DROP COLUMN IF EXISTS credit_usage_set_by_user;
-- +goose StatementEnd
