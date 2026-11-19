-- +goose Up
-- +goose StatementBegin

-- Tracks whether an annual-plan renewal reminder has already been sent for
-- the org's CURRENT billing cycle (the renewal-upcoming reminder). NULL means "not sent yet for this
-- cycle" — cleared back to NULL whenever activateOrgSubscription records a
-- new subscription_renews_at (a rollover), so the next cycle gets its own
-- reminder instead of staying permanently silenced by the first one ever
-- sent.
ALTER TABLE organizations ADD COLUMN renewal_reminder_sent_at TIMESTAMPTZ;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE organizations DROP COLUMN IF EXISTS renewal_reminder_sent_at;
-- +goose StatementEnd
