-- +goose Up
-- +goose StatementBegin

-- Step 1b of the (user_id, organization_id) re-key.
-- Proven necessary by live testing, not deferred out of caution: with
-- user_id still the sole primary key, InitializeTx's ON CONFLICT (user_id)
-- DO NOTHING silently no-ops the moment a user already has ANY balance row
-- — so a user switching into a second organization they hold no seat in
-- can never get one, no matter how many times the lazy-init path runs.
-- A composite primary key is the actual fix, not a follow-up.
--
-- Safe to run: confirmed via information_schema that nothing references
-- user_balances by foreign key, and 00053's backfill already leaves every
-- row's organization_id populated (verified: zero NULLs in every
-- environment this has been checked against so far).
ALTER TABLE user_balances ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE user_balances DROP CONSTRAINT user_balances_pkey;
ALTER TABLE user_balances ADD PRIMARY KEY (user_id, organization_id);

-- org_seat_source_id (00052) existed only to track "which org sourced this
-- shared row" under the old one-row-per-user model. Now that each org gets
-- its own real row, organization_id — non-nullable, indexed, part of the
-- primary key — already answers that question unambiguously; the separate
-- tracking column is dead weight the moment the composite key lands, not a
-- later cleanup.
ALTER TABLE user_balances DROP COLUMN org_seat_source_id;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
ALTER TABLE user_balances ADD COLUMN org_seat_source_id UUID REFERENCES organizations(id) ON DELETE SET NULL;
ALTER TABLE user_balances DROP CONSTRAINT user_balances_pkey;
ALTER TABLE user_balances ADD PRIMARY KEY (user_id);
ALTER TABLE user_balances ALTER COLUMN organization_id DROP NOT NULL;
-- +goose StatementEnd
