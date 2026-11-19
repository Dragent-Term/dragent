package integrations

import (
	"context"
	"encoding/json"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

type PostgresRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresRepository(pool *pgxpool.Pool) *PostgresRepository {
	return &PostgresRepository{pool: pool}
}

const selectColumns = `
	c.id, c.organization_id, c.owner_user_id,
	COALESCE(NULLIF(TRIM(u.display_name), ''), NULLIF(TRIM(u.first_name || ' ' || u.last_name), ''), u.email),
	c.provider, c.shared, c.account_label, c.config, c.key_hint, c.status,
	c.last_used_at, c.created_at, c.updated_at, c.secrets_encrypted`

func scanConnection(row pgx.Row) (Connection, error) {
	var c Connection
	var config []byte
	var lastUsed *time.Time
	var created, updated time.Time
	err := row.Scan(&c.ID, &c.OrganizationID, &c.OwnerUserID, &c.OwnerName,
		&c.Provider, &c.Shared, &c.Account, &config, &c.KeyHint, &c.Status,
		&lastUsed, &created, &updated, &c.secretsEncrypted)
	if err != nil {
		return c, err
	}
	c.Config = map[string]string{}
	if len(config) > 0 {
		if err := json.Unmarshal(config, &c.Config); err != nil {
			return c, err
		}
	}
	if lastUsed != nil {
		ms := lastUsed.UnixMilli()
		c.LastUsedAt = &ms
	}
	c.CreatedAt = created.UnixMilli()
	c.UpdatedAt = updated.UnixMilli()
	return c, nil
}

func (r *PostgresRepository) List(ctx context.Context, orgID, userID uuid.UUID) ([]Connection, error) {
	rows, err := r.pool.Query(ctx, `SELECT `+selectColumns+`
		FROM integration_connections c JOIN users u ON u.id = c.owner_user_id
		WHERE c.organization_id = $1 AND (c.shared OR c.owner_user_id = $2)
		ORDER BY c.provider, c.shared DESC`, orgID, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Connection
	for rows.Next() {
		c, err := scanConnection(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, c)
	}
	return out, rows.Err()
}

func (r *PostgresRepository) Get(ctx context.Context, orgID, id uuid.UUID) (*Connection, error) {
	c, err := scanConnection(r.pool.QueryRow(ctx, `SELECT `+selectColumns+`
		FROM integration_connections c JOIN users u ON u.id = c.owner_user_id
		WHERE c.organization_id = $1 AND c.id = $2`, orgID, id))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &c, nil
}

func (r *PostgresRepository) Save(ctx context.Context, c Connection) (Connection, error) {
	config, err := json.Marshal(c.Config)
	if err != nil {
		return Connection{}, err
	}
	// Each connection lives in a slot (the organization's shared one for
	// its service, or its owner's personal one); connecting again replaces
	// what the slot held.
	conflict := `ON CONFLICT (organization_id, provider, owner_user_id) WHERE NOT shared`
	if c.Shared {
		conflict = `ON CONFLICT (organization_id, provider) WHERE shared`
	}
	var id uuid.UUID
	err = r.pool.QueryRow(ctx, `
		INSERT INTO integration_connections (
			id, organization_id, owner_user_id, provider, shared, account_label,
			config, secrets_encrypted, key_hint, status, created_at, updated_at
		) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ok', NOW(), NOW())
		`+conflict+` DO UPDATE SET
			owner_user_id = EXCLUDED.owner_user_id,
			account_label = EXCLUDED.account_label,
			config = EXCLUDED.config,
			secrets_encrypted = EXCLUDED.secrets_encrypted,
			key_hint = EXCLUDED.key_hint,
			status = 'ok',
			updated_at = NOW()
		RETURNING id`,
		c.ID, c.OrganizationID, c.OwnerUserID, c.Provider, c.Shared, c.Account,
		config, c.secretsEncrypted, c.KeyHint).Scan(&id)
	if err != nil {
		return Connection{}, err
	}
	saved, err := r.Get(ctx, c.OrganizationID, id)
	if err != nil {
		return Connection{}, err
	}
	if saved == nil {
		return Connection{}, errors.New("saved connection not found")
	}
	return *saved, nil
}

func (r *PostgresRepository) Delete(ctx context.Context, orgID, id uuid.UUID) error {
	_, err := r.pool.Exec(ctx,
		`DELETE FROM integration_connections WHERE organization_id = $1 AND id = $2`, orgID, id)
	return err
}

func (r *PostgresRepository) Used(ctx context.Context, id uuid.UUID, status string, at time.Time) error {
	_, err := r.pool.Exec(ctx,
		`UPDATE integration_connections SET last_used_at = $2, status = $3 WHERE id = $1`, id, at, status)
	return err
}
