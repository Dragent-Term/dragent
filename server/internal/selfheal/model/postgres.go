package model

import (
	"context"
	"database/sql"
	"errors"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

type PostgresSelfHealRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresSelfHealRepository(pool *pgxpool.Pool) *PostgresSelfHealRepository {
	return &PostgresSelfHealRepository{pool: pool}
}

func (r *PostgresSelfHealRepository) CreateBlockEvent(ctx context.Context, event *BlockEvent) error {
	query := `
		INSERT INTO block_events (id, user_id, command, cwd, exit_code, output_text, output_truncated, created_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
	`
	_, err := r.pool.Exec(ctx, query, event.ID, event.UserID, event.Command, event.Cwd, event.ExitCode, event.OutputText, event.OutputTruncated, event.CreatedAt)
	return err
}

func (r *PostgresSelfHealRepository) CreateBlockAnalysis(ctx context.Context, analysis *BlockAnalysis) error {
	query := `
		INSERT INTO block_analyses (id, block_event_id, explanation, suggested_command, created_at)
		VALUES ($1, $2, $3, $4, $5)
	`
	_, err := r.pool.Exec(ctx, query, analysis.ID, analysis.BlockEventID, analysis.Explanation, analysis.SuggestedCommand, analysis.CreatedAt)
	return err
}

func (r *PostgresSelfHealRepository) GetBlockEvent(ctx context.Context, id uuid.UUID) (*BlockEvent, error) {
	query := `
		SELECT id, user_id, command, cwd, exit_code, output_text, output_truncated, created_at
		FROM block_events WHERE id = $1
	`
	var event BlockEvent
	err := r.pool.QueryRow(ctx, query, id).Scan(
		&event.ID, &event.UserID, &event.Command, &event.Cwd, &event.ExitCode, &event.OutputText, &event.OutputTruncated, &event.CreatedAt,
	)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &event, nil
}

func (r *PostgresSelfHealRepository) GetBlockAnalysis(ctx context.Context, eventID uuid.UUID) (*BlockAnalysis, error) {
	query := `
		SELECT id, block_event_id, explanation, suggested_command, created_at
		FROM block_analyses WHERE block_event_id = $1
	`
	var analysis BlockAnalysis
	err := r.pool.QueryRow(ctx, query, eventID).Scan(
		&analysis.ID, &analysis.BlockEventID, &analysis.Explanation, &analysis.SuggestedCommand, &analysis.CreatedAt,
	)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, sql.ErrNoRows
		}
		return nil, err
	}
	return &analysis, nil
}

var _ SelfHealRepository = (*PostgresSelfHealRepository)(nil)
