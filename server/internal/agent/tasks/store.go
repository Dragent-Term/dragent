package tasks

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
	"server/internal/agent/pipeline"
)

var ErrTaskTerminal = errors.New("agent task is already terminal")

type TaskRecord struct {
	ID                uuid.UUID
	UserID            uuid.UUID
	ModelName         string
	Status            Status
	ExecutorID        *uuid.UUID
	LeaseExpiresAt    *time.Time
	LastEventSeq      uint64
	CreatedAt         time.Time
	UpdatedAt         time.Time
	ResumedFromTaskID *uuid.UUID
}

type StoredToolResult struct {
	Output  string
	IsError bool
}

type StoredCheckpoint struct {
	Revision  uint64
	Resumable bool
	State     pipeline.RunCheckpoint
}

// Store is the durable coordination boundary shared by API replicas.
type Store interface {
	Create(ctx context.Context, record TaskRecord) error
	GetForUser(ctx context.Context, id, userID uuid.UUID) (*TaskRecord, error)
	AppendEvent(ctx context.Context, taskID uuid.UUID, event pipeline.AgentEvent, terminalStatus *Status) (uint64, error)
	AppendEvents(ctx context.Context, taskID uuid.UUID, events []pipeline.AgentEvent, terminalStatus *Status) ([]SeqEvent, error)
	ListEventsAfter(ctx context.Context, taskID uuid.UUID, after uint64, limit int) ([]SeqEvent, error)
	SaveToolResult(ctx context.Context, taskID uuid.UUID, callID, output string, isError bool) error
	GetToolResult(ctx context.Context, taskID uuid.UUID, callID string) (*StoredToolResult, error)
	SaveCheckpoint(ctx context.Context, taskID uuid.UUID, checkpoint pipeline.RunCheckpoint) (uint64, error)
	GetCheckpoint(ctx context.Context, taskID uuid.UUID) (*StoredCheckpoint, error)
	DeleteForUser(ctx context.Context, taskID, userID uuid.UUID) error
	DeleteTerminalBefore(ctx context.Context, before time.Time, limit int) (int, error)
	SetStatus(ctx context.Context, taskID uuid.UUID, status Status) error
	Heartbeat(ctx context.Context, executorID uuid.UUID, taskIDs []uuid.UUID, leaseUntil time.Time) error
	FailExpired(ctx context.Context, now time.Time, limit int) (int, error)
}
