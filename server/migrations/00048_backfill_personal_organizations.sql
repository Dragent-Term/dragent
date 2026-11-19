-- +goose Up
-- +goose StatementBegin

-- Every account gets an organization, not opt-in. Going forward
-- this happens at signup (identity/components OrganizationInitializer); this
-- backfills every account that predates that code. Idempotent: only touches
-- users with no membership row, so it is safe to rerun.
--
-- Slug/name scheme matches the Go-side organization.PostgresRepository
-- exactly: first 8 hex characters of the user id (no dashes) + "-team" for
-- the slug, display_name (or the email's local part, if no name was ever
-- captured) + "'s Team" for the name.
WITH new_orgs AS (
    INSERT INTO organizations (id, name, slug, created_at, updated_at)
    SELECT
        gen_random_uuid(),
        COALESCE(NULLIF(TRIM(u.display_name), ''), split_part(u.email, '@', 1)) || '''s Team',
        substr(replace(u.id::text, '-', ''), 1, 8) || '-team',
        NOW(),
        NOW()
    FROM users u
    WHERE NOT EXISTS (
        SELECT 1 FROM memberships m WHERE m.user_id = u.id
    )
    RETURNING id, slug
)
INSERT INTO memberships (id, user_id, organization_id, role, created_at)
SELECT
    gen_random_uuid(),
    u.id,
    o.id,
    'owner',
    NOW()
FROM users u
JOIN new_orgs o
    ON o.slug = substr(replace(u.id::text, '-', ''), 1, 8) || '-team'
WHERE NOT EXISTS (
    SELECT 1 FROM memberships m WHERE m.user_id = u.id
);

-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin

-- Down is intentionally a no-op: a personal org backfilled here may already
-- be in use (e.g. later 11.A steps reference it), and the slug scheme makes
-- backfilled orgs indistinguishable from signup-created ones, so there is no
-- safe way to identify and remove only the rows this migration added.

-- +goose StatementEnd
