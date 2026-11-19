-- +goose Up
-- +goose StatementBegin

-- Mobile push leases. Each row is one device's registration for push
-- delivery (APNs/FCM token), keyed by the existing devices table: the FK
-- ties every lease to a known device, and deleting a device (or its user)
-- cascades the lease away. Register/rotate renew expires_at; unregister
-- deletes the row; a cleanup job deletes lapsed rows.
CREATE TABLE device_push_registrations (
    device_id UUID PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    platform TEXT NOT NULL CHECK (platform IN ('apns', 'fcm')),
    push_token TEXT NOT NULL,
    foreground BOOLEAN NOT NULL DEFAULT FALSE,
    expires_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_device_push_registrations_user ON device_push_registrations(user_id);
CREATE INDEX idx_device_push_registrations_expires ON device_push_registrations(expires_at);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP INDEX IF EXISTS idx_device_push_registrations_expires;
DROP INDEX IF EXISTS idx_device_push_registrations_user;
DROP TABLE IF EXISTS device_push_registrations;
-- +goose StatementEnd
