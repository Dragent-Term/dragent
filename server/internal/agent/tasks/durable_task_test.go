package tasks

import (
	"context"
	"database/sql"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"server/internal/agent/pipeline"
	"server/internal/agent/providers"
)

type memoryTaskStore struct {
	mu          sync.Mutex
	records     map[uuid.UUID]TaskRecord
	events      map[uuid.UUID][]SeqEvent
	tools       map[uuid.UUID]map[string]StoredToolResult
	checkpoints map[uuid.UUID]StoredCheckpoint
}

func newMemoryTaskStore() *memoryTaskStore {
	return &memoryTaskStore{records: map[uuid.UUID]TaskRecord{}, events: map[uuid.UUID][]SeqEvent{}, tools: map[uuid.UUID]map[string]StoredToolResult{}, checkpoints: map[uuid.UUID]StoredCheckpoint{}}
}
func (s *memoryTaskStore) Create(_ context.Context, r TaskRecord) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.records[r.ID] = r
	return nil
}
func (s *memoryTaskStore) GetForUser(_ context.Context, id, user uuid.UUID) (*TaskRecord, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.records[id]
	if !ok || r.UserID != user {
		return nil, sql.ErrNoRows
	}
	c := r
	return &c, nil
}
func (s *memoryTaskStore) AppendEvent(ctx context.Context, id uuid.UUID, ev pipeline.AgentEvent, status *Status) (uint64, error) {
	es, err := s.AppendEvents(ctx, id, []pipeline.AgentEvent{ev}, status)
	if err != nil {
		return 0, err
	}
	return es[0].Seq, nil
}
func (s *memoryTaskStore) AppendEvents(_ context.Context, id uuid.UUID, evs []pipeline.AgentEvent, status *Status) ([]SeqEvent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.records[id]
	if r.Status == StatusDone || r.Status == StatusFailed || r.Status == StatusCancelled {
		return nil, ErrTaskTerminal
	}
	out := make([]SeqEvent, len(evs))
	for i, ev := range evs {
		r.LastEventSeq++
		out[i] = SeqEvent{Seq: r.LastEventSeq, Ev: ev}
		s.events[id] = append(s.events[id], out[i])
	}
	if status != nil {
		r.Status = *status
	}
	s.records[id] = r
	return out, nil
}
func (s *memoryTaskStore) ListEventsAfter(_ context.Context, id uuid.UUID, after uint64, limit int) ([]SeqEvent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []SeqEvent
	for _, e := range s.events[id] {
		if e.Seq > after {
			out = append(out, e)
			if len(out) == limit {
				break
			}
		}
	}
	return out, nil
}
func (s *memoryTaskStore) SaveToolResult(_ context.Context, id uuid.UUID, call, output string, isErr bool) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.tools[id] == nil {
		s.tools[id] = map[string]StoredToolResult{}
	}
	if _, ok := s.tools[id][call]; !ok {
		s.tools[id][call] = StoredToolResult{Output: output, IsError: isErr}
	}
	return nil
}
func (s *memoryTaskStore) GetToolResult(_ context.Context, id uuid.UUID, call string) (*StoredToolResult, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.tools[id][call]
	if !ok {
		return nil, sql.ErrNoRows
	}
	return &r, nil
}
func (s *memoryTaskStore) SaveCheckpoint(_ context.Context, id uuid.UUID, checkpoint pipeline.RunCheckpoint) (uint64, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stored := s.checkpoints[id]
	stored.Revision++
	stored.Resumable = checkpoint.Phase != pipeline.RunPhaseCompleted
	stored.State = checkpoint
	s.checkpoints[id] = stored
	return stored.Revision, nil
}
func (s *memoryTaskStore) GetCheckpoint(_ context.Context, id uuid.UUID) (*StoredCheckpoint, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stored, ok := s.checkpoints[id]
	if !ok {
		return nil, sql.ErrNoRows
	}
	copy := stored
	return &copy, nil
}
func (s *memoryTaskStore) DeleteForUser(_ context.Context, id, user uuid.UUID) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	record, ok := s.records[id]
	if !ok || record.UserID != user {
		return sql.ErrNoRows
	}
	delete(s.records, id)
	delete(s.events, id)
	delete(s.tools, id)
	delete(s.checkpoints, id)
	return nil
}
func (*memoryTaskStore) DeleteTerminalBefore(context.Context, time.Time, int) (int, error) {
	return 0, nil
}
func (s *memoryTaskStore) SetStatus(_ context.Context, id uuid.UUID, status Status) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.records[id]
	r.Status = status
	s.records[id] = r
	return nil
}
func (*memoryTaskStore) Heartbeat(context.Context, uuid.UUID, []uuid.UUID, time.Time) error {
	return nil
}
func (*memoryTaskStore) FailExpired(context.Context, time.Time, int) (int, error) { return 0, nil }

func TestDurableEventsReplayFromAnotherTaskInstance(t *testing.T) {
	store := newMemoryTaskStore()
	id, user := uuid.New(), uuid.New()
	if err := store.Create(context.Background(), TaskRecord{ID: id, UserID: user, Status: StatusRunning}); err != nil {
		t.Fatal(err)
	}
	owner := &Task{ID: id, UserID: user, Status: StatusRunning, store: store, persistCh: make(chan persistRequest, 16)}
	owner.broadcast(pipeline.AgentEvent{Kind: pipeline.KindTextDelta, Text: "hello"})
	owner.broadcast(pipeline.AgentEvent{Kind: pipeline.KindDone})
	remote := &Task{ID: id, UserID: user, Status: StatusDone, store: store}
	backlog, ch, ok := remote.Subscribe(0)
	if !ok {
		t.Fatal("durable replay refused")
	}
	defer remote.Unsubscribe(ch)
	if len(backlog) != 2 || backlog[0].Seq != 1 || backlog[0].Ev.Text != "hello" || backlog[1].Ev.Kind != pipeline.KindDone {
		t.Fatalf("backlog=%+v", backlog)
	}
}

type captureResolver struct{ ch chan StoredToolResult }

func (r *captureResolver) Resolve(_ string, output string, isError bool) bool {
	select {
	case r.ch <- StoredToolResult{Output: output, IsError: isError}:
	default:
	}
	return true
}

func TestToolResultCrossesReplicaThroughStore(t *testing.T) {
	store := newMemoryTaskStore()
	id, user := uuid.New(), uuid.New()
	_ = store.Create(context.Background(), TaskRecord{ID: id, UserID: user, Status: StatusRunning})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	resolver := &captureResolver{ch: make(chan StoredToolResult, 1)}
	owner := &Task{ID: id, UserID: user, Status: StatusRunning, store: store, taskCtx: ctx, toolResultGate: resolver, persistCh: make(chan persistRequest, 16)}
	owner.broadcast(pipeline.AgentEvent{Kind: pipeline.KindToolCall, ToolID: "call-1", ToolName: "run"})
	remote := &Task{ID: id, UserID: user, store: store}
	if !remote.ResolveToolResult("call-1", "ok", false) {
		t.Fatal("remote result was not persisted")
	}
	select {
	case got := <-resolver.ch:
		if got.Output != "ok" || got.IsError {
			t.Fatalf("result=%+v", got)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("executor did not receive persisted tool result")
	}
}

func TestResumeRefusesUnresolvedToolInsteadOfReplayingIt(t *testing.T) {
	store := newMemoryTaskStore()
	id, user := uuid.New(), uuid.New()
	_ = store.Create(context.Background(), TaskRecord{
		ID: id, UserID: user, ModelName: "test-model", Status: StatusFailed,
	})
	_, _ = store.SaveCheckpoint(context.Background(), id, pipeline.RunCheckpoint{
		SchemaVersion: pipeline.RunCheckpointSchemaVersion,
		Phase:         pipeline.RunPhaseAwaitingTools,
		Round:         1,
		PendingToolCalls: []providers.ToolCall{{
			ID: "write-1", Name: "write_file", ArgsJSON: `{"path":"/workspace/app.ts"}`,
		}},
	})
	manager := &Manager{store: store}
	_, err := manager.Resume(context.Background(), id, user, CreateRequest{})
	if err == nil || !strings.Contains(err.Error(), "refusing to replay") {
		t.Fatalf("resume error=%v, want ambiguous-side-effect refusal", err)
	}
}

func TestDeleteForUserRemovesAllDurableTaskData(t *testing.T) {
	store := newMemoryTaskStore()
	id, user := uuid.New(), uuid.New()
	_ = store.Create(context.Background(), TaskRecord{
		ID: id, UserID: user, ModelName: "test-model", Status: StatusDone,
	})
	store.events[id] = []SeqEvent{{Seq: 1, Ev: pipeline.AgentEvent{Kind: pipeline.KindDone}}}
	store.tools[id] = map[string]StoredToolResult{"call-1": {Output: "sensitive output"}}
	store.checkpoints[id] = StoredCheckpoint{State: pipeline.RunCheckpoint{Phase: pipeline.RunPhaseCompleted}}

	manager := &Manager{store: store, tasks: map[uuid.UUID]*Task{
		id: {ID: id, UserID: user, Status: StatusDone, store: store},
	}}
	if err := manager.DeleteForUser(context.Background(), id, user); err != nil {
		t.Fatal(err)
	}
	if _, ok := store.records[id]; ok || store.events[id] != nil || store.tools[id] != nil {
		t.Fatal("task data was not deleted through the cascade boundary")
	}
	if _, ok := manager.tasks[id]; ok {
		t.Fatal("deleted task remained in the in-process registry")
	}
}
