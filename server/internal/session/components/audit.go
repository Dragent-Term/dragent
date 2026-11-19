package components

import (
	"context"
	"sync"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

// AuditEvent is one remote-command decision, recorded durably.
type AuditEvent struct {
	SessionID     string
	ParticipantID string
	HostUser      string
	Command       string
	Approved      bool
	At            time.Time
}

// Auditor records remote-command decisions (approve/deny) for the audit trail.
type Auditor interface {
	RecordCommand(ctx context.Context, e AuditEvent) error
}

// NoopAuditor discards events (dev without persistence).
type NoopAuditor struct{}

func (NoopAuditor) RecordCommand(context.Context, AuditEvent) error { return nil }

// MemoryAuditor collects events for assertions in tests.
type MemoryAuditor struct {
	mu     sync.Mutex
	events []AuditEvent
}

func (m *MemoryAuditor) RecordCommand(_ context.Context, e AuditEvent) error {
	m.mu.Lock()
	m.events = append(m.events, e)
	m.mu.Unlock()
	return nil
}

func (m *MemoryAuditor) Events() []AuditEvent {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]AuditEvent(nil), m.events...)
}

// PostgresAuditor writes to the audit_events table (migration 00038).
type PostgresAuditor struct {
	pool *pgxpool.Pool
}

func NewPostgresAuditor(pool *pgxpool.Pool) *PostgresAuditor { return &PostgresAuditor{pool: pool} }

func (a *PostgresAuditor) RecordCommand(ctx context.Context, e AuditEvent) error {
	_, err := a.pool.Exec(ctx,
		`INSERT INTO audit_events
		   (session_id, participant_id, host_user_id, command, approved, created_at)
		 VALUES ($1, $2, $3, $4, $5, $6)`,
		e.SessionID, e.ParticipantID, e.HostUser, e.Command, e.Approved, e.At,
	)
	return err
}
