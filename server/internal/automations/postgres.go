package automations

import (
	"context"
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
	a.id, a.organization_id, a.owner_user_id,
	COALESCE(NULLIF(TRIM(u.display_name), ''), NULLIF(TRIM(u.first_name || ' ' || u.last_name), ''), u.email),
	u.email, a.name, a.prompt, a.schedule, a.schedule_label, a.timezone, a.agent,
	a.project_label, a.enabled, a.next_run_at, a.last_run_status, a.last_run_at,
	a.created_at, a.updated_at`

func scanAutomation(row pgx.Row) (Automation, error) {
	var a Automation
	var nextRun, lastRun *time.Time
	var created, updated time.Time
	err := row.Scan(&a.ID, &a.OrganizationID, &a.OwnerUserID, &a.OwnerName, &a.OwnerEmail,
		&a.Name, &a.Prompt, &a.Schedule, &a.ScheduleLabel, &a.Timezone, &a.Agent,
		&a.ProjectLabel, &a.Enabled, &nextRun, &a.LastRunStatus, &lastRun, &created, &updated)
	if err != nil {
		return a, err
	}
	a.NextRunAt = millis(nextRun)
	a.LastRunAt = millis(lastRun)
	a.CreatedAt = created.UnixMilli()
	a.UpdatedAt = updated.UnixMilli()
	return a, nil
}

func millis(t *time.Time) *int64 {
	if t == nil {
		return nil
	}
	ms := t.UnixMilli()
	return &ms
}

func timestamp(ms *int64) *time.Time {
	if ms == nil {
		return nil
	}
	t := time.UnixMilli(*ms).UTC()
	return &t
}

func (r *PostgresRepository) List(ctx context.Context, orgID uuid.UUID) ([]Automation, error) {
	rows, err := r.pool.Query(ctx, `SELECT `+selectColumns+`
		FROM automations a JOIN users u ON u.id = a.owner_user_id
		WHERE a.organization_id = $1
		ORDER BY a.created_at, a.id`, orgID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Automation
	for rows.Next() {
		a, err := scanAutomation(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

func (r *PostgresRepository) Get(ctx context.Context, orgID, id uuid.UUID) (*Automation, error) {
	a, err := scanAutomation(r.pool.QueryRow(ctx, `SELECT `+selectColumns+`
		FROM automations a JOIN users u ON u.id = a.owner_user_id
		WHERE a.organization_id = $1 AND a.id = $2`, orgID, id))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &a, nil
}

func (r *PostgresRepository) Exists(ctx context.Context, id uuid.UUID) (bool, error) {
	var exists bool
	err := r.pool.QueryRow(ctx, `SELECT EXISTS (SELECT 1 FROM automations WHERE id = $1)`, id).Scan(&exists)
	return exists, err
}

func (r *PostgresRepository) Upsert(ctx context.Context, a Automation) error {
	// The WHERE keeps an update from moving an automation to another owner
	// or organization.
	_, err := r.pool.Exec(ctx, `
		INSERT INTO automations (
			id, organization_id, owner_user_id, name, prompt, schedule, schedule_label,
			timezone, agent, project_label, enabled, next_run_at, last_run_status,
			last_run_at, created_at, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
		ON CONFLICT (id) DO UPDATE SET
			name = EXCLUDED.name,
			prompt = EXCLUDED.prompt,
			schedule = EXCLUDED.schedule,
			schedule_label = EXCLUDED.schedule_label,
			timezone = EXCLUDED.timezone,
			agent = EXCLUDED.agent,
			project_label = EXCLUDED.project_label,
			enabled = EXCLUDED.enabled,
			next_run_at = EXCLUDED.next_run_at,
			last_run_status = EXCLUDED.last_run_status,
			last_run_at = EXCLUDED.last_run_at,
			updated_at = EXCLUDED.updated_at
		WHERE automations.owner_user_id = EXCLUDED.owner_user_id
		  AND automations.organization_id = EXCLUDED.organization_id`,
		a.ID, a.OrganizationID, a.OwnerUserID, a.Name, a.Prompt, a.Schedule, a.ScheduleLabel,
		a.Timezone, a.Agent, a.ProjectLabel, a.Enabled, timestamp(a.NextRunAt), a.LastRunStatus,
		timestamp(a.LastRunAt), time.UnixMilli(a.CreatedAt).UTC(), time.UnixMilli(a.UpdatedAt).UTC())
	return err
}

func (r *PostgresRepository) Delete(ctx context.Context, orgID, id uuid.UUID) error {
	_, err := r.pool.Exec(ctx, `DELETE FROM automations WHERE organization_id = $1 AND id = $2`, orgID, id)
	return err
}

func (r *PostgresRepository) CountOwned(ctx context.Context, orgID, userID uuid.UUID) (int, error) {
	var n int
	err := r.pool.QueryRow(ctx, `SELECT COUNT(*) FROM automations
		WHERE organization_id = $1 AND owner_user_id = $2`, orgID, userID).Scan(&n)
	return n, err
}

// tierRank orders plan tiers so the better of two can be picked.
var tierRank = map[string]int{"free": 0, "pro": 1, "max": 2, "ultra": 3, "mega": 4}

func (r *PostgresRepository) Tier(ctx context.Context, orgID, userID uuid.UUID) (string, error) {
	var own, org *string
	err := r.pool.QueryRow(ctx, `SELECT
		(SELECT tier FROM user_balances WHERE user_id = $2 AND organization_id = $1),
		(SELECT tier FROM organizations WHERE id = $1)`, orgID, userID).Scan(&own, &org)
	if err != nil {
		return "", err
	}
	best := "free"
	for _, tier := range []*string{own, org} {
		if tier != nil && tierRank[*tier] > tierRank[best] {
			best = *tier
		}
	}
	return best, nil
}
