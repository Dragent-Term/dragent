-- +goose Up
-- +goose StatementBegin

-- E2EE push for companion phones (FIX1, docs/design/e2ee.md §8). Paired
-- phones live only in the companion pairing store (Redis in production),
-- never in the Postgres devices table, so a registration for a paired E2E
-- phone can never satisfy the device_id foreign key. Drop that FK; the
-- primary key on device_id and the user_id FK (which cascades a user's
-- registrations away with the user) stay. Revocation cleanup moves to the
-- pairing handler's push hook (server/internal/companionpairing + main.go),
-- which deletes the registration best-effort when a device is revoked.
ALTER TABLE device_push_registrations
    DROP CONSTRAINT IF EXISTS device_push_registrations_device_id_fkey;

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Restores the device_id FK dropped above. Rows for companion phones (with
-- no matching devices row) block the restore until they are removed; that
-- failure is intentional — rolling back under E2EE push data must be an
-- explicit cleanup, never silent loss.
ALTER TABLE device_push_registrations
    ADD CONSTRAINT device_push_registrations_device_id_fkey
    FOREIGN KEY (device_id) REFERENCES devices(id) ON DELETE CASCADE;

-- +goose StatementEnd
