package tasks

import (
	"context"
	"database/sql"
	"errors"
	"log"
	"sync"
	"time"

	"github.com/google/uuid"
	"server/internal/agent/pipeline"
	"server/internal/agent/providers"
)

// Status represents the lifecycle state of an agent task.
type Status string

const (
	StatusRunning          Status = "running"
	StatusAwaitingApproval Status = "awaiting_approval"
	StatusDone             Status = "done"
	StatusFailed           Status = "failed"
	StatusCancelled        Status = "cancelled"
)

// Task is a single agentic conversation run.
type Task struct {
	ID        uuid.UUID
	UserID    uuid.UUID
	Status    Status
	CreatedAt time.Time
	ModelName string
	// DesktopOwned marks an ephemeral, single-provider-step transport. Its
	// provider-ready state belongs to the desktop and is never persisted here.
	DesktopOwned bool

	// cancel shuts down the runner goroutine
	cancel context.CancelFunc
	// run launches the runner goroutine; invoked once by the first /stream
	// subscriber (see StartRunner) so the client is listening before any event
	// is produced.
	run       func()
	startOnce sync.Once
	// subs is the list of active SSE subscribers
	mu   sync.RWMutex
	subs []chan SeqEvent
	// The bounded memory history serves tests and store-less deployments. With
	// a Store configured, the database is the authoritative replay log and this
	// ring is only a fast local copy.
	seq     uint64
	history []SeqEvent
	// toolResultGate delivers client-posted tool results back to the runner.
	toolResultGate toolResultResolver
	store          Store
	taskCtx        context.Context
	persistOnce    sync.Once
	persistCh      chan persistRequest
	started        bool
}

type persistRequest struct {
	ev       pipeline.AgentEvent
	terminal *Status
	ack      chan error
}

// maxHistory is both the local ring size and durable replay page size.
const maxHistory = 4096

// SeqEvent pairs an event with its monotonic sequence number, so a reconnecting
// client can resume from the last seq it received (SSE Last-Event-ID).
type SeqEvent struct {
	Seq uint64
	Ev  pipeline.AgentEvent
}

type toolResultResolver interface {
	Resolve(callID, output string, isError bool) bool
}

// AgentMode controls which tools are exposed to the LLM for a task.
type AgentMode string

const (
	AgentModeNormal      AgentMode = ""            // all tools available
	AgentModeCoordinator AgentMode = "coordinator" // no write/exec tools; can spawn_subagent
	AgentModeWorker      AgentMode = "worker"      // all tools except spawn_subagent (no recursion)
)

// CreateRequest is the input for creating a new agent task.
type CreateRequest struct {
	UserID    uuid.UUID
	ModelName string
	System    string
	Messages  []providers.Message
	Mode      AgentMode
	// Effort is the user-selected reasoning depth ("" = provider default).
	// Ignored for models that reject the parameter (see providers.ResolveEffort).
	Effort providers.Effort
	// ExtraTools are additional tool definitions (e.g. from MCP servers) that
	// are appended to the standard tool set for this task.
	ExtraTools []providers.ToolDefinition
	// RestrictTools distinguishes an explicitly empty allowlist from the legacy
	// unrestricted request where AllowedTools is nil.
	AllowedTools  []string
	RestrictTools bool

	// UserApiKey is the user's own API key passed via X-User-Api-Key header.
	// When set the task tries this key first.
	UserApiKey string
	// CustomBaseURL is an OpenAI-compatible base URL from X-Custom-Base-Url header.
	// Used together with UserApiKey to reach local models (Ollama, LM Studio, etc.)
	// or any OpenAI-compatible proxy the user operates.
	CustomBaseURL string
	// FallbackAllowed is the user's explicit permission to use platform-funded
	// provider credentials. It is false when the header is absent.
	FallbackAllowed bool
	// ResumeCheckpoint starts a linked execution attempt from durable provider
	// history instead of reconstructing the turn from prose.
	ResumeCheckpoint *pipeline.RunCheckpoint
	ResumeTaskID     *uuid.UUID
	// ResumeRuntimeContext is a bounded controller-generated refresh containing
	// facts that may have changed after the saved system prompt was built (for
	// example, permanent external-path grants accepted during the interrupted
	// turn). It supplements rather than replaces durable provider history.
	ResumeRuntimeContext string
	// DesktopOwned makes the server a stateless, single-provider-step proxy.
	// Provider-ready history and tool outputs remain on the user's machine.
	DesktopOwned bool
	ClientRunID  string
	ClientRound  int
}

// StartRunner launches the task's runner exactly once. The SSE stream handler
// calls it right after subscribing, so events are only produced once a subscriber
// is attached — otherwise the runner races ahead, early tokens are broadcast to
// nobody, and the answer appears all at once instead of streaming.
func (t *Task) StartRunner() {
	t.startOnce.Do(func() {
		if t.run != nil {
			t.mu.Lock()
			t.started = true
			t.mu.Unlock()
			if t.store != nil {
				go t.watchCancellation()
			}
			t.run()
		}
	})
}

func (t *Task) LeaseActive() bool {
	t.mu.RLock()
	started := t.started
	t.mu.RUnlock()
	if !started {
		return false
	}
	if t.taskCtx != nil {
		select {
		case <-t.taskCtx.Done():
			return false
		default:
		}
	}
	return true
}

// Subscribe returns a channel that receives all future events for this task.
// The caller must drain the channel. The channel is closed when the task ends.
func (t *Task) Subscribe(fromSeq uint64) (backlog []SeqEvent, ch <-chan SeqEvent, resumable bool) {
	out := make(chan SeqEvent, 256)
	if t.store != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		cursor := fromSeq
		for {
			events, err := t.store.ListEventsAfter(ctx, t.ID, cursor, maxHistory)
			if err != nil {
				return nil, nil, false
			}
			backlog = append(backlog, events...)
			if len(events) < maxHistory {
				break
			}
			cursor = events[len(events)-1].Seq
		}
	}
	t.mu.Lock()
	defer t.mu.Unlock()

	// If the caller wants events after a seq we no longer hold, the gap is
	// unrecoverable (a missed tool_call would deadlock). Signal the handler to
	// refuse the resume so the client falls back to a clean restart rather than
	// hanging. fromSeq == 0 is a fresh connect and always resumable.
	if t.store == nil && fromSeq > 0 && len(t.history) > 0 && t.history[0].Seq > fromSeq+1 {
		return nil, nil, false
	}
	if t.store == nil {
		for _, se := range t.history {
			if se.Seq > fromSeq {
				backlog = append(backlog, se)
			}
		}
	}
	t.subs = append(t.subs, out)
	return backlog, out, true
}

// CurrentSeq is the highest seq emitted so far — the id a backlog replay ends
// on, so the handler can number live events correctly after it.
func (t *Task) CurrentSeq() uint64 {
	t.mu.RLock()
	defer t.mu.RUnlock()
	return t.seq
}

// Unsubscribe removes a subscriber channel.
func (t *Task) Unsubscribe(ch <-chan SeqEvent) {
	t.mu.Lock()
	defer t.mu.Unlock()
	for i, s := range t.subs {
		if s == ch {
			t.subs = append(t.subs[:i], t.subs[i+1:]...)
			close(s)
			return
		}
	}
}

// ResolveToolResult delivers the client's tool execution result to the runner.
// callID must match the tool_id from the KindToolCall SSE event.
func (t *Task) ResolveToolResult(callID, output string, isError bool) bool {
	if t.store != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		err := t.store.SaveToolResult(ctx, t.ID, callID, output, isError)
		cancel()
		if err != nil {
			return false
		}
	}
	if t.toolResultGate == nil {
		return t.store != nil
	}
	return t.toolResultGate.Resolve(callID, output, isError) || t.store != nil
}

// Cancel cancels the task's context.
func (t *Task) Cancel() {
	if t.store != nil {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		status := StatusCancelled
		_, _ = t.store.AppendEvent(ctx, t.ID, pipeline.AgentEvent{
			Kind: pipeline.KindError, Error: "task cancelled",
		}, &status)
		cancel()
	}
	if t.cancel != nil {
		t.cancel()
	}
}

// broadcast records an event in the replay buffer and sends it to all
// subscribers. A slow subscriber may drop the live copy, but the buffered copy
// lets it recover the event on reconnect.
func (t *Task) broadcast(ev pipeline.AgentEvent) {
	var terminal *Status
	if ev.Kind == pipeline.KindDone {
		s := StatusDone
		terminal = &s
	}
	if ev.Kind == pipeline.KindError {
		s := StatusFailed
		terminal = &s
	}
	if t.store == nil {
		t.mu.Lock()
		t.seq++
		se := SeqEvent{Seq: t.seq, Ev: ev}
		t.publishLocked(se)
		t.mu.Unlock()
		return
	}
	t.startPersistWriter()
	req := persistRequest{ev: ev, terminal: terminal}
	if terminal != nil {
		req.ack = make(chan error, 1)
	}
	t.persistCh <- req
	if req.ack != nil {
		if err := <-req.ack; err != nil && !errors.Is(err, ErrTaskTerminal) {
			log.Printf("[agent-task] durable terminal event append failed task=%s: %v", t.ID, err)
		}
	}
}

func (t *Task) publishLocked(se SeqEvent) {
	t.seq = se.Seq
	t.history = append(t.history, se)
	if len(t.history) > maxHistory {
		t.history = t.history[len(t.history)-maxHistory:]
	}
	for _, ch := range t.subs {
		select {
		case ch <- se:
		default:
			// Slow consumer: drop the live copy; it is replayable from history.
		}
	}
	if se.Ev.Kind == pipeline.KindToolCall && !se.Ev.ServerExecuted && t.store != nil && t.toolResultGate != nil {
		go t.watchToolResult(se.Ev.ToolID)
	}
}

func (t *Task) startPersistWriter() {
	t.persistOnce.Do(func() {
		if t.persistCh == nil {
			t.persistCh = make(chan persistRequest, 512)
		}
		go t.persistEvents()
	})
}

func (t *Task) persistEvents() {
	for first := range t.persistCh {
		batch := []persistRequest{first}
		timer := time.NewTimer(5 * time.Millisecond)
	collect:
		for len(batch) < 64 {
			select {
			case req := <-t.persistCh:
				batch = append(batch, req)
				if req.terminal != nil {
					break collect
				}
			case <-timer.C:
				break collect
			}
		}
		if !timer.Stop() {
			select {
			case <-timer.C:
			default:
			}
		}
		events := make([]pipeline.AgentEvent, len(batch))
		for i := range batch {
			events[i] = batch[i].ev
		}
		terminal := batch[len(batch)-1].terminal
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		stored, err := t.store.AppendEvents(ctx, t.ID, events, terminal)
		cancel()
		if err == nil {
			t.mu.Lock()
			for _, se := range stored {
				t.publishLocked(se)
			}
			t.mu.Unlock()
		} else if !errors.Is(err, ErrTaskTerminal) {
			log.Printf("[agent-task] durable event batch failed task=%s count=%d: %v", t.ID, len(batch), err)
			if t.cancel != nil {
				t.cancel()
			}
		}
		for _, req := range batch {
			if req.ack != nil {
				req.ack <- err
			}
		}
	}
}

// EventsAfter supports cross-replica SSE polling when the executor and stream
// connection are handled by different API instances.
func (t *Task) EventsAfter(ctx context.Context, seq uint64) ([]SeqEvent, error) {
	if t.store == nil {
		return nil, nil
	}
	return t.store.ListEventsAfter(ctx, t.ID, seq, maxHistory)
}

func (t *Task) DurableStatus(ctx context.Context) (Status, error) {
	if t.store == nil {
		return t.Status, nil
	}
	r, err := t.store.GetForUser(ctx, t.ID, t.UserID)
	if err != nil {
		return "", err
	}
	return r.Status, nil
}

func (t *Task) watchToolResult(callID string) {
	ticker := time.NewTicker(250 * time.Millisecond)
	defer ticker.Stop()
	ctx := t.taskCtx
	if ctx == nil {
		ctx = context.Background()
	}
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			result, err := t.store.GetToolResult(ctx, t.ID, callID)
			if err == nil {
				t.toolResultGate.Resolve(callID, result.Output, result.IsError)
				return
			}
			if !errorsIsNoRows(err) {
				log.Printf("[agent-task] tool result poll failed task=%s call=%s: %v", t.ID, callID, err)
			}
		}
	}
}

func (t *Task) watchCancellation() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	ctx := t.taskCtx
	if ctx == nil {
		return
	}
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			status, err := t.DurableStatus(ctx)
			if err == nil && (status == StatusCancelled || status == StatusFailed) {
				if t.cancel != nil {
					t.cancel()
				}
				return
			}
		}
	}
}

func errorsIsNoRows(err error) bool { return errors.Is(err, sql.ErrNoRows) }

func (t *Task) closeAllSubs() {
	t.mu.Lock()
	defer t.mu.Unlock()
	for _, ch := range t.subs {
		close(ch)
	}
	t.subs = nil
}
