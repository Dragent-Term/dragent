package tasks

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"server/internal/agent/pipeline"
)

type PostgresStore struct{ pool *pgxpool.Pool }

func NewPostgresStore(pool *pgxpool.Pool) *PostgresStore { return &PostgresStore{pool: pool} }

func (s *PostgresStore) Create(ctx context.Context, r TaskRecord) error {
	_, err := s.pool.Exec(ctx, `INSERT INTO agent_tasks
		(id,user_id,model_name,status,executor_id,lease_expires_at,created_at,updated_at,resumed_from_task_id)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$7,$8)`, r.ID, r.UserID, r.ModelName, r.Status,
		r.ExecutorID, r.LeaseExpiresAt, r.CreatedAt, r.ResumedFromTaskID)
	return err
}

func (s *PostgresStore) GetForUser(ctx context.Context, id, userID uuid.UUID) (*TaskRecord, error) {
	var r TaskRecord
	var seq int64
	err := s.pool.QueryRow(ctx, `SELECT id,user_id,model_name,status,executor_id,
		lease_expires_at,last_event_seq,created_at,updated_at,resumed_from_task_id FROM agent_tasks
		WHERE id=$1 AND user_id=$2`, id, userID).Scan(&r.ID, &r.UserID, &r.ModelName,
		&r.Status, &r.ExecutorID, &r.LeaseExpiresAt, &seq, &r.CreatedAt, &r.UpdatedAt,
		&r.ResumedFromTaskID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, sql.ErrNoRows
	}
	r.LastEventSeq = uint64(seq)
	return &r, err
}

func (s *PostgresStore) AppendEvent(ctx context.Context, taskID uuid.UUID, event pipeline.AgentEvent, terminal *Status) (uint64, error) {
	events, err := s.AppendEvents(ctx, taskID, []pipeline.AgentEvent{event}, terminal)
	if err != nil {
		return 0, err
	}
	return events[0].Seq, nil
}

func (s *PostgresStore) AppendEvents(ctx context.Context, taskID uuid.UUID, events []pipeline.AgentEvent, terminal *Status) ([]SeqEvent, error) {
	if len(events) == 0 {
		return nil, nil
	}
	payloads := make([][]byte, len(events))
	for i, event := range events {
		payload, err := json.Marshal(event)
		if err != nil {
			return nil, err
		}
		payloads[i] = payload
	}
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	var lastSeq int64
	err = tx.QueryRow(ctx, `UPDATE agent_tasks SET last_event_seq=last_event_seq+$2,
		updated_at=NOW() WHERE id=$1 AND status IN ('running','awaiting_approval') RETURNING last_event_seq`, taskID, len(events)).Scan(&lastSeq)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrTaskTerminal
	}
	if err != nil {
		return nil, err
	}
	firstSeq := lastSeq - int64(len(events)) + 1
	stored := make([]SeqEvent, len(events))
	for i, payload := range payloads {
		seq := firstSeq + int64(i)
		if _, err = tx.Exec(ctx, `INSERT INTO agent_task_events(task_id,seq,event) VALUES($1,$2,$3)`, taskID, seq, payload); err != nil {
			return nil, err
		}
		stored[i] = SeqEvent{Seq: uint64(seq), Ev: events[i]}
	}
	if terminal != nil {
		if _, err = tx.Exec(ctx, `UPDATE agent_tasks SET status=$2,completed_at=NOW(),
			lease_expires_at=NULL,updated_at=NOW() WHERE id=$1`, taskID, *terminal); err != nil {
			return nil, err
		}
	}
	if err = tx.Commit(ctx); err != nil {
		return nil, err
	}
	return stored, nil
}

func (s *PostgresStore) ListEventsAfter(ctx context.Context, taskID uuid.UUID, after uint64, limit int) ([]SeqEvent, error) {
	if limit <= 0 || limit > 4096 {
		limit = 4096
	}
	rows, err := s.pool.Query(ctx, `SELECT seq,event FROM agent_task_events
		WHERE task_id=$1 AND seq>$2 ORDER BY seq LIMIT $3`, taskID, int64(after), limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var events []SeqEvent
	for rows.Next() {
		var seq int64
		var payload []byte
		var ev pipeline.AgentEvent
		if err := rows.Scan(&seq, &payload); err != nil {
			return nil, err
		}
		if err := json.Unmarshal(payload, &ev); err != nil {
			return nil, err
		}
		events = append(events, SeqEvent{Seq: uint64(seq), Ev: ev})
	}
	return events, rows.Err()
}

func (s *PostgresStore) SaveToolResult(ctx context.Context, taskID uuid.UUID, callID, output string, isError bool) error {
	_, err := s.pool.Exec(ctx, `INSERT INTO agent_tool_results(task_id,call_id,output,is_error)
		VALUES($1,$2,$3,$4) ON CONFLICT(task_id,call_id) DO NOTHING`, taskID, callID, output, isError)
	return err
}

func (s *PostgresStore) GetToolResult(ctx context.Context, taskID uuid.UUID, callID string) (*StoredToolResult, error) {
	var r StoredToolResult
	err := s.pool.QueryRow(ctx, `SELECT output,is_error FROM agent_tool_results WHERE task_id=$1 AND call_id=$2`, taskID, callID).Scan(&r.Output, &r.IsError)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, sql.ErrNoRows
	}
	return &r, err
}

func (s *PostgresStore) SaveCheckpoint(ctx context.Context, taskID uuid.UUID, checkpoint pipeline.RunCheckpoint) (uint64, error) {
	payload, err := json.Marshal(checkpoint)
	if err != nil {
		return 0, err
	}
	resumable := checkpoint.Phase != pipeline.RunPhaseCompleted
	var revision int64
	err = s.pool.QueryRow(ctx, `INSERT INTO agent_task_checkpoints
		(task_id,schema_version,revision,phase,round_number,resumable,state)
		VALUES($1,$2,1,$3,$4,$5,$6)
		ON CONFLICT(task_id) DO UPDATE SET
			schema_version=EXCLUDED.schema_version,
			revision=agent_task_checkpoints.revision+1,
			phase=EXCLUDED.phase,
			round_number=EXCLUDED.round_number,
			resumable=EXCLUDED.resumable,
			state=EXCLUDED.state,
			updated_at=NOW()
		RETURNING revision`, taskID, checkpoint.SchemaVersion, checkpoint.Phase,
		checkpoint.Round, resumable, payload).Scan(&revision)
	return uint64(revision), err
}

func (s *PostgresStore) GetCheckpoint(ctx context.Context, taskID uuid.UUID) (*StoredCheckpoint, error) {
	var revision int64
	var resumable bool
	var payload []byte
	err := s.pool.QueryRow(ctx, `SELECT revision,resumable,state
		FROM agent_task_checkpoints WHERE task_id=$1`, taskID).Scan(&revision, &resumable, &payload)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, sql.ErrNoRows
	}
	if err != nil {
		return nil, err
	}
	var checkpoint pipeline.RunCheckpoint
	if err := json.Unmarshal(payload, &checkpoint); err != nil {
		return nil, err
	}
	return &StoredCheckpoint{Revision: uint64(revision), Resumable: resumable, State: checkpoint}, nil
}

func (s *PostgresStore) DeleteForUser(ctx context.Context, taskID, userID uuid.UUID) error {
	result, err := s.pool.Exec(ctx, `DELETE FROM agent_tasks WHERE id=$1 AND user_id=$2`, taskID, userID)
	if err != nil {
		return err
	}
	if result.RowsAffected() == 0 {
		return sql.ErrNoRows
	}
	return nil
}

func (s *PostgresStore) DeleteTerminalBefore(ctx context.Context, before time.Time, limit int) (int, error) {
	if limit <= 0 || limit > 1_000 {
		limit = 250
	}
	result, err := s.pool.Exec(ctx, `WITH stale AS (
		SELECT id FROM agent_tasks
		WHERE status IN ('done','failed','cancelled')
			AND completed_at IS NOT NULL AND completed_at < $1
		ORDER BY completed_at
		FOR UPDATE SKIP LOCKED
		LIMIT $2
	)
	DELETE FROM agent_tasks task USING stale WHERE task.id=stale.id`, before, limit)
	if err != nil {
		return 0, err
	}
	return int(result.RowsAffected()), nil
}

func (s *PostgresStore) SetStatus(ctx context.Context, taskID uuid.UUID, status Status) error {
	terminal := status == StatusDone || status == StatusFailed || status == StatusCancelled
	_, err := s.pool.Exec(ctx, `UPDATE agent_tasks SET status=$2,updated_at=NOW(),
		completed_at=CASE WHEN $3 THEN NOW() ELSE completed_at END,
		lease_expires_at=CASE WHEN $3 THEN NULL ELSE lease_expires_at END WHERE id=$1`, taskID, status, terminal)
	return err
}

func (s *PostgresStore) Heartbeat(ctx context.Context, executorID uuid.UUID, ids []uuid.UUID, until time.Time) error {
	if len(ids) == 0 {
		return nil
	}
	_, err := s.pool.Exec(ctx, `UPDATE agent_tasks SET lease_expires_at=$3,updated_at=NOW()
		WHERE executor_id=$1 AND id=ANY($2) AND status IN ('running','awaiting_approval')`, executorID, ids, until)
	return err
}

func (s *PostgresStore) FailExpired(ctx context.Context, now time.Time, limit int) (int, error) {
	if limit <= 0 || limit > 500 {
		limit = 100
	}
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return 0, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	rows, err := tx.Query(ctx, `SELECT id,last_event_seq FROM agent_tasks
		WHERE status IN ('running','awaiting_approval') AND lease_expires_at < $1
		ORDER BY lease_expires_at FOR UPDATE SKIP LOCKED LIMIT $2`, now, limit)
	if err != nil {
		return 0, err
	}
	type expired struct {
		id  uuid.UUID
		seq int64
	}
	var tasks []expired
	for rows.Next() {
		var e expired
		if err := rows.Scan(&e.id, &e.seq); err != nil {
			rows.Close()
			return 0, err
		}
		tasks = append(tasks, e)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return 0, err
	}
	event, _ := json.Marshal(pipeline.AgentEvent{Kind: pipeline.KindError,
		Error: "task interrupted because its server executor stopped; retry the request"})
	for _, e := range tasks {
		seq := e.seq + 1
		if _, err := tx.Exec(ctx, `INSERT INTO agent_task_events(task_id,seq,event) VALUES($1,$2,$3)`, e.id, seq, event); err != nil {
			return 0, err
		}
		if _, err := tx.Exec(ctx, `UPDATE agent_tasks SET status='failed',last_event_seq=$2,
			lease_expires_at=NULL,completed_at=$3,updated_at=$3 WHERE id=$1`, e.id, seq, now); err != nil {
			return 0, err
		}
	}
	if err := tx.Commit(ctx); err != nil {
		return 0, err
	}
	return len(tasks), nil
}

var _ Store = (*PostgresStore)(nil)
