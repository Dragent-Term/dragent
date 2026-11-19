-- +goose Up
-- Make webhook idempotency survive a failed apply.
--
-- Before this, `payment_events` recorded an event BEFORE the credit grant or
-- subscription activation ran, in a separate transaction. If the apply failed
-- (transient DB error, deadlock), the handler returned 500 and the provider
-- retried — but the retry hashed to the same id, saw the existing row, and
-- returned "already processed" without ever granting the credits. The customer
-- was charged and the retry mechanism actively guaranteed it could never
-- self-heal.
--
-- With `processed_at`, a row is a CLAIM. Only a claim that completed its apply
-- is terminal; an unfinished one can be re-claimed by the provider's retry.
ALTER TABLE payment_events
    ADD COLUMN IF NOT EXISTS processed_at TIMESTAMP WITH TIME ZONE NULL;

-- Existing rows ran under the old record-then-apply flow. Treat them as done:
-- almost all succeeded, and re-applying a historical top-up would double-credit
-- — the safer error here is to skip, not to repeat.
UPDATE payment_events SET processed_at = received_at WHERE processed_at IS NULL;

-- Finding unfinished claims (retry/alerting) should not scan the table.
CREATE INDEX IF NOT EXISTS idx_payment_events_unprocessed
    ON payment_events (received_at) WHERE processed_at IS NULL;

COMMENT ON COLUMN payment_events.processed_at IS
    'Set only after the event was successfully applied. NULL means a claim '
    'whose apply did not complete — a provider retry is allowed to re-run it.';

-- +goose Down
DROP INDEX IF EXISTS idx_payment_events_unprocessed;
ALTER TABLE payment_events DROP COLUMN IF EXISTS processed_at;
