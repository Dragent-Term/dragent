package model

import (
	"context"
	"database/sql"
	"errors"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
)

type dbConn interface {
	Exec(ctx context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

type PostgresUserRepository struct {
	pool *pgxpool.Pool
	tx   pgx.Tx
}

func NewPostgresUserRepository(pool *pgxpool.Pool) *PostgresUserRepository {
	return &PostgresUserRepository{pool: pool}
}

func (r *PostgresUserRepository) WithTx(tx any) UserRepository {
	if tx == nil {
		return r
	}
	pgTx, ok := tx.(pgx.Tx)
	if !ok {
		return r
	}
	return &PostgresUserRepository{
		pool: r.pool,
		tx:   pgTx,
	}
}

func (r *PostgresUserRepository) getDB() dbConn {
	if r.tx != nil {
		return r.tx
	}
	return r.pool
}

func (r *PostgresUserRepository) Create(ctx context.Context, user *User) error {
	db := r.getDB()
	query := `
		INSERT INTO users (id, email, first_name, last_name, display_name, avatar_url, email_verified_at, created_at, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
	`
	_, err := db.Exec(ctx, query, user.ID, user.Email, user.FirstName, user.LastName, user.DisplayName, user.AvatarURL, user.EmailVerifiedAt, user.CreatedAt, user.UpdatedAt)
	return err
}

func (r *PostgresUserRepository) GetByID(ctx context.Context, id uuid.UUID) (*User, error) {
	db := r.getDB()
	query := `
		SELECT id, email, first_name, last_name, display_name, avatar_url, email_verified_at, created_at, updated_at
		FROM users WHERE id = $1
	`
	var user User
	err := db.QueryRow(ctx, query, id).Scan(&user.ID, &user.Email, &user.FirstName, &user.LastName, &user.DisplayName, &user.AvatarURL, &user.EmailVerifiedAt, &user.CreatedAt, &user.UpdatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &user, nil
}

func (r *PostgresUserRepository) GetByEmail(ctx context.Context, email string) (*User, error) {
	db := r.getDB()
	query := `
		SELECT id, email, first_name, last_name, display_name, avatar_url, email_verified_at, created_at, updated_at
		FROM users WHERE email = $1
	`
	var user User
	err := db.QueryRow(ctx, query, email).Scan(&user.ID, &user.Email, &user.FirstName, &user.LastName, &user.DisplayName, &user.AvatarURL, &user.EmailVerifiedAt, &user.CreatedAt, &user.UpdatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &user, nil
}

func (r *PostgresUserRepository) UpdateProfile(ctx context.Context, user *User) error {
	db := r.getDB()
	query := `
		UPDATE users SET first_name = $2, last_name = $3, display_name = $4, avatar_url = $5, updated_at = NOW()
		WHERE id = $1
	`
	_, err := db.Exec(ctx, query, user.ID, user.FirstName, user.LastName, user.DisplayName, user.AvatarURL)
	return err
}

func (r *PostgresUserRepository) LinkOAuthAccount(ctx context.Context, account *OAuthAccount) error {
	db := r.getDB()
	query := `
		INSERT INTO oauth_accounts (id, user_id, provider, provider_user_id, created_at)
		VALUES ($1, $2, $3, $4, $5)
	`
	_, err := db.Exec(ctx, query, account.ID, account.UserID, account.Provider, account.ProviderUserID, account.CreatedAt)
	return err
}

func (r *PostgresUserRepository) GetOAuthAccount(ctx context.Context, provider string, providerUserID string) (*OAuthAccount, error) {
	db := r.getDB()
	query := `
		SELECT id, user_id, provider, provider_user_id, created_at
		FROM oauth_accounts WHERE provider = $1 AND provider_user_id = $2
	`
	var acc OAuthAccount
	err := db.QueryRow(ctx, query, provider, providerUserID).Scan(&acc.ID, &acc.UserID, &acc.Provider, &acc.ProviderUserID, &acc.CreatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &acc, nil
}

func (r *PostgresUserRepository) RegisterDevice(ctx context.Context, device *Device) error {
	db := r.getDB()
	query := `
		INSERT INTO devices (id, user_id, name, platform, last_seen_at, created_at)
		VALUES ($1, $2, $3, $4, $5, $6)
		ON CONFLICT (id) DO UPDATE 
		SET last_seen_at = EXCLUDED.last_seen_at
	`
	_, err := db.Exec(ctx, query, device.ID, device.UserID, device.Name, device.Platform, device.LastSeenAt, device.CreatedAt)
	return err
}

func (r *PostgresUserRepository) SaveRefreshToken(ctx context.Context, token *RefreshToken) error {
	db := r.getDB()
	query := `
		INSERT INTO refresh_tokens (id, user_id, device_id, token_hash, family_id, revoked_at, expires_at, created_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
	`
	_, err := db.Exec(ctx, query, token.ID, token.UserID, token.DeviceID, token.TokenHash, token.FamilyID, token.RevokedAt, token.ExpiresAt, token.CreatedAt)
	return err
}

func (r *PostgresUserRepository) GetRefreshTokenByHash(ctx context.Context, hash string) (*RefreshToken, error) {
	db := r.getDB()
	query := `
		SELECT id, user_id, device_id, token_hash, family_id, revoked_at, expires_at, created_at
		FROM refresh_tokens WHERE token_hash = $1
	`
	var token RefreshToken
	err := db.QueryRow(ctx, query, hash).Scan(&token.ID, &token.UserID, &token.DeviceID, &token.TokenHash, &token.FamilyID, &token.RevokedAt, &token.ExpiresAt, &token.CreatedAt)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &token, nil
}

func (r *PostgresUserRepository) RevokeToken(ctx context.Context, id uuid.UUID) error {
	db := r.getDB()
	_, err := db.Exec(ctx, `UPDATE refresh_tokens SET revoked_at = NOW() WHERE id = $1`, id)
	return err
}

func (r *PostgresUserRepository) RevokeTokenFamily(ctx context.Context, familyID uuid.UUID) error {
	db := r.getDB()
	query := `
		UPDATE refresh_tokens SET revoked_at = NOW() WHERE family_id = $1 AND revoked_at IS NULL
	`
	_, err := db.Exec(ctx, query, familyID)
	return err
}

func (r *PostgresUserRepository) RevokeDeviceTokens(ctx context.Context, userID uuid.UUID, deviceID uuid.UUID) error {
	db := r.getDB()
	_, err := db.Exec(ctx, `UPDATE refresh_tokens SET revoked_at = NOW() WHERE user_id = $1 AND device_id = $2 AND revoked_at IS NULL`, userID, deviceID)
	return err
}

type PostgresTxManager struct {
	pool *pgxpool.Pool
}

func NewPostgresTxManager(pool *pgxpool.Pool) *PostgresTxManager {
	return &PostgresTxManager{pool: pool}
}

func (m *PostgresTxManager) WithTx(ctx context.Context, fn func(tx any) error) error {
	tx, err := m.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() {
		_ = tx.Rollback(ctx)
	}()

	if err := fn(tx); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

var _ UserRepository = (*PostgresUserRepository)(nil)
var _ TxManager = (*PostgresTxManager)(nil)
