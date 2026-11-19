-- +goose Up
-- +goose StatementBegin

-- E2EE relay cut-over (docs/design/e2ee.md §10, revision 9).
--
-- The phone relay is end-to-end encrypted with a hard cut-over (D1): every
-- phone pairs again, old app versions lose access, and the server becomes a
-- blind router. This migration does the Postgres half:
--
-- 1. e2e flag on auth devices. Devices redeemed through the new pairing
--    flow are E2EE-capable; pre-cut-over rows read as e2e=false and are
--    refused before admission (relay-error + close 4403). The flag defaults
--    false so existing rows fail closed until the phone pairs again.
-- 2. Sealed catalog table. The relay stores the desktop's sealed catalog
--    blob opaquely (§6.5): routing only (owner, desktop, version,
--    recipients, nonce, body) — never machine names, projects, paths,
--    branches, tab titles, or agent states. The in-memory/Redis stores are
--    the serving path; this table is the durable backing for production.
-- 3. Purge. Pre-cut-over plaintext has no Postgres home: relay catalogs and
--    host-state mirrors lived in process memory/Redis (cleared by the
--    deploy), the MirrorSender content copy is deleted in code, and relay,
--    pairing and push logs are aggregate counters only (no per-request
--    access log, no tokens/titles/names in logs). Nothing to DELETE here;
--    the DOWN migration drops what UP creates.
ALTER TABLE devices ADD COLUMN IF NOT EXISTS e2e BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE IF NOT EXISTS relay_sealed_catalogs (
    owner TEXT PRIMARY KEY,
    desktop_id UUID NOT NULL,
    version BIGINT NOT NULL CHECK (version >= 1),
    recipients JSONB NOT NULL,
    nonce TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_relay_sealed_catalogs_desktop ON relay_sealed_catalogs(desktop_id);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

DROP INDEX IF EXISTS idx_relay_sealed_catalogs_desktop;
DROP TABLE IF EXISTS relay_sealed_catalogs;

ALTER TABLE devices DROP COLUMN IF EXISTS e2e;

-- +goose StatementEnd
