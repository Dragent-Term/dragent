package components

import (
	"context"
	"encoding/json"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"

	"server/internal/session/types"
)

// fakeSink collects frames and lets a test wait for (or assert the absence of) a
// frame of a given type.
type fakeSink struct {
	mu  sync.Mutex
	all [][]byte
	ch  chan []byte
}

func newFakeSink() *fakeSink { return &fakeSink{ch: make(chan []byte, 128)} }

func (s *fakeSink) Send(data []byte) {
	cp := append([]byte(nil), data...)
	s.mu.Lock()
	s.all = append(s.all, cp)
	s.mu.Unlock()
	select {
	case s.ch <- cp:
	default:
	}
}

func (s *fakeSink) waitType(t *testing.T, typ string) []byte {
	t.Helper()
	deadline := time.After(2 * time.Second)
	for {
		select {
		case data := <-s.ch:
			var env types.Envelope
			_ = json.Unmarshal(data, &env)
			if env.Type == typ {
				return data
			}
		case <-deadline:
			t.Fatalf("timed out waiting for %q", typ)
			return nil
		}
	}
}

func (s *fakeSink) assertNoType(t *testing.T, typ string, within time.Duration) {
	t.Helper()
	deadline := time.After(within)
	for {
		select {
		case data := <-s.ch:
			var env types.Envelope
			_ = json.Unmarshal(data, &env)
			if env.Type == typ {
				t.Fatalf("unexpected %q frame", typ)
			}
		case <-deadline:
			return
		}
	}
}

func plainHub() *Hub { return NewHub(NewMemoryBus(), AllowAll{}, NoopAuditor{}) }

func deltaFrame() []byte {
	b, _ := json.Marshal(map[string]any{
		"type": "delta",
		"delta": map[string]any{
			"kind": "output_chunk", "block_id": "b1", "seq": 1, "data_base64": "aGk=",
		},
	})
	return b
}

func viewerParticipant(name string) types.Participant {
	return types.Participant{ID: uuid.New(), DisplayName: name, Role: types.RoleViewer, Present: true}
}

// hostWithViewer opens a session with one viewer and waits for the initial
// participant update, returning the pieces a test needs.
func hostWithViewer(t *testing.T, hub *Hub, sessionID string) (host *fakeSink, p types.Participant) {
	t.Helper()
	host = newFakeSink()
	hub.RegisterHost(sessionID, "host-user", "", "", host)
	p = viewerParticipant("Viewer")
	hub.RegisterViewer(context.Background(), sessionID, p, "viewer-user", newFakeSink())
	host.waitType(t, types.TypeParticipantsUpdated)
	return host, p
}

func TestHostBroadcastReachesViewer(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	hub.RegisterHost("s1", "u1", "", "", host)
	viewer := newFakeSink()
	p := viewerParticipant("Phone")
	hub.RegisterViewer(ctx, "s1", p, "viewer-user", viewer)

	pu := host.waitType(t, types.TypeParticipantsUpdated)
	var list types.ServerParticipantsUpdated
	_ = json.Unmarshal(pu, &list)
	if len(list.Participants) != 1 || list.Participants[0].DisplayName != "Phone" {
		t.Fatalf("participants not updated: %+v", list.Participants)
	}

	hub.BroadcastDelta(ctx, "s1", deltaFrame())
	if got := viewer.waitType(t, types.TypeDelta); len(got) == 0 {
		t.Fatal("viewer did not receive delta")
	}
}

func TestViewerHistoryReplayIsOrderedBeforeLiveDeltas(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	hub.RegisterHost("ordered", "host-user", "", "", newFakeSink())

	first := deltaFrame()
	seed := newFakeSink()
	seedParticipant := viewerParticipant("Seed")
	hub.RegisterViewer(ctx, "ordered", seedParticipant, "viewer-user", seed)
	hub.BroadcastDelta(ctx, "ordered", first)
	// Receiving it proves the bus subscriber committed the frame to history.
	seed.waitType(t, types.TypeDelta)
	hub.UnregisterViewer(ctx, "ordered", seedParticipant)

	viewer := newFakeSink()
	hub.RegisterViewer(ctx, "ordered", viewerParticipant("Refresh"), "viewer-user", viewer)
	if got := viewer.waitType(t, types.TypeDelta); string(got) != string(first) {
		t.Fatalf("history frame was not replayed first: got %s want %s", got, first)
	}

	second := []byte(`{"type":"delta","delta":{"kind":"output_chunk","block_id":"b2","seq":2,"data_base64":"d29ybGQ="}}`)
	hub.BroadcastDelta(ctx, "ordered", second)
	if got := viewer.waitType(t, types.TypeDelta); string(got) != string(second) {
		t.Fatalf("live frame overtook history replay: got %s want %s", got, second)
	}
}

func TestCrossInstanceFanOut(t *testing.T) {
	bus := NewMemoryBus() // shared "Redis": two gateway instances
	hub1 := NewHub(bus, AllowAll{}, NoopAuditor{})
	hub2 := NewHub(bus, AllowAll{}, NoopAuditor{})
	ctx := context.Background()

	host := newFakeSink()
	hub1.RegisterHost("s3", "u1", "", "", host)
	viewer := newFakeSink()
	p := viewerParticipant("RemotePhone")
	hub2.RegisterViewer(ctx, "s3", p, "viewer-user", viewer) // viewer on the OTHER instance

	host.waitType(t, types.TypeParticipantsUpdated)
	hub1.BroadcastDelta(ctx, "s3", deltaFrame())
	viewer.waitType(t, types.TypeDelta)
}

// An EXPLICIT end (a reason was recorded) ends the session for viewers at once.
func TestHostExplicitEndEndsViewer(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	hub.RegisterHost("s4", "u1", "", "", host)
	viewer := newFakeSink()
	hub.RegisterViewer(ctx, "s4", viewerParticipant("V"), "viewer-user", viewer)
	host.waitType(t, types.TypeParticipantsUpdated)

	hub.SetEndReason("s4", "The host ended this session.")
	hub.UnregisterHost(ctx, "s4")
	viewer.waitType(t, types.TypeEnded)
}

// An UNEXPECTED host drop must NOT end the session immediately — the desktop
// reconnects with backoff, and ending here is what made viewers see "the host
// ended the session" on a transient blip while the host was still running.
func TestHostDropWaitsForReconnect(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	hub.RegisterHost("s4b", "u1", "", "", host)
	viewer := newFakeSink()
	hub.RegisterViewer(ctx, "s4b", viewerParticipant("V"), "viewer-user", viewer)
	host.waitType(t, types.TypeParticipantsUpdated)

	hub.UnregisterHost(ctx, "s4b") // no reason recorded == unexpected drop
	viewer.assertNoType(t, types.TypeEnded, 300*time.Millisecond)
}

// Reconnecting within the grace period keeps the session alive: the pending
// teardown sees a newer host epoch and does nothing.
func TestHostReconnectCancelsTeardown(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	hub.RegisterHost("s4c", "u1", "", "", host)
	viewer := newFakeSink()
	hub.RegisterViewer(ctx, "s4c", viewerParticipant("V"), "viewer-user", viewer)
	host.waitType(t, types.TypeParticipantsUpdated)

	hub.UnregisterHost(ctx, "s4c")
	hub.RegisterHost("s4c", "u1", "", "", newFakeSink()) // host came back

	viewer.assertNoType(t, types.TypeEnded, 300*time.Millisecond)
}

func TestSetAccessGrantsControl(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s5")

	hub.SetParticipantAccess(ctx, "s5", p.ID.String(), types.AccessControl)
	for {
		pu := host.waitType(t, types.TypeParticipantsUpdated)
		var list types.ServerParticipantsUpdated
		_ = json.Unmarshal(pu, &list)
		if len(list.Participants) == 1 && list.Participants[0].Role == types.RoleController {
			return
		}
	}
}

func TestRemoveParticipantKicksViewer(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	viewer := newFakeSink()
	hub.RegisterHost("s5r", "host-user", "", "", host)
	p := viewerParticipant("V")
	hub.RegisterViewer(ctx, "s5r", p, "viewer-user", viewer)

	hub.RemoveParticipant(ctx, "s5r", p.ID.String())
	// The removed viewer is told, and everyone's list drops them.
	if typ := viewer.waitType(t, types.TypeRemoved); typ == nil {
		t.Fatal("removed viewer never got a Removed frame")
	}
	for {
		frame := host.waitType(t, types.TypeParticipantsUpdated)
		var m types.ServerParticipantsUpdated
		_ = json.Unmarshal(frame, &m)
		removed := true
		for _, part := range m.Participants {
			if part.ID == p.ID {
				removed = false
			}
		}
		if removed {
			return
		}
	}
}

func TestDenyAccessReachesViewer(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	viewer := newFakeSink()
	hub.RegisterHost("s5d", "host-user", "", "", host)
	p := viewerParticipant("V")
	hub.RegisterViewer(ctx, "s5d", p, "viewer-user", viewer)

	frame, _ := json.Marshal(types.AccessDenied())
	hub.SendToViewer(ctx, "s5d", p.ID.String(), frame)
	got := viewer.waitType(t, types.TypeAccessDenied)
	var m types.Simple
	_ = json.Unmarshal(got, &m)
	if m.Type != types.TypeAccessDenied {
		t.Fatalf("expected access_denied, got %q", m.Type)
	}
}

// ── Step 4: remote-command enforcement ───────────────────────────────────────

func TestExecDeniedWithoutControl(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s6")

	// A plain viewer's exec is dropped by the host instance.
	hub.RequestCommand(ctx, "s6", p.ID.String(), "rm -rf /")
	host.assertNoType(t, types.TypeCommandRequested, 300*time.Millisecond)
}

func TestControllerExecReachesHost(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s7")
	hub.SetParticipantAccess(ctx, "s7", p.ID.String(), types.AccessControl)

	hub.RequestCommand(ctx, "s7", p.ID.String(), "ls -la")
	frame := host.waitType(t, types.TypeCommandRequested)
	var m types.ServerCommandRequested
	_ = json.Unmarshal(frame, &m)
	if m.From != p.ID || m.Command != "ls -la" || m.RequestID == uuid.Nil {
		t.Fatalf("command requested mismatch: %+v", m)
	}
	if !m.AutoApproved {
		t.Fatal("an edit-role participant's command runs without approval")
	}
}

func TestAgentQueryDeniedWithoutControl(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s-agent-view")

	hub.RequestAgentQuery(ctx, "s-agent-view", p.ID.String(), uuid.NewString(), "fix it")
	host.assertNoType(t, types.TypeAgentQueryRequested, 300*time.Millisecond)
}

func TestControllerAgentQueryAndStopReachHost(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s-agent-edit")
	hub.SetParticipantAccess(ctx, "s-agent-edit", p.ID.String(), types.AccessControl)

	requestID := uuid.New()
	hub.RequestAgentQuery(ctx, "s-agent-edit", p.ID.String(), requestID.String(), "  fix it  ")
	frame := host.waitType(t, types.TypeAgentQueryRequested)
	var query types.ServerAgentQueryRequested
	_ = json.Unmarshal(frame, &query)
	if query.RequestID != requestID || query.From != p.ID || query.Query != "fix it" {
		t.Fatalf("agent query mismatch: %+v", query)
	}

	hub.RequestAgentStop(ctx, "s-agent-edit", p.ID.String())
	stopFrame := host.waitType(t, types.TypeAgentStopRequested)
	var stop types.ServerAgentStopRequested
	_ = json.Unmarshal(stopFrame, &stop)
	if stop.From != p.ID {
		t.Fatalf("agent stop mismatch: %+v", stop)
	}
}

func TestAgentQueryRequestIDIsIdempotent(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s-agent-dedup")
	hub.SetParticipantAccess(ctx, "s-agent-dedup", p.ID.String(), types.AccessControl)

	requestID := uuid.NewString()
	hub.RequestAgentQuery(ctx, "s-agent-dedup", p.ID.String(), requestID, "one task")
	host.waitType(t, types.TypeAgentQueryRequested)
	hub.RequestAgentQuery(ctx, "s-agent-dedup", p.ID.String(), requestID, "one task")
	host.assertNoType(t, types.TypeAgentQueryRequested, 300*time.Millisecond)
}

func TestInputDeniedWithoutControl(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s6i")

	// A plain viewer's raw input is dropped by the host instance.
	hub.ForwardInput(ctx, "s6i", p.ID.String(), "GWhpCg==")
	host.assertNoType(t, types.TypeInputReceived, 300*time.Millisecond)
}

func TestControllerInputReachesHost(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s7i")
	hub.SetParticipantAccess(ctx, "s7i", p.ID.String(), types.AccessControl)

	hub.ForwardInput(ctx, "s7i", p.ID.String(), "GWhpCg==")
	frame := host.waitType(t, types.TypeInputReceived)
	var m types.ServerInputReceived
	_ = json.Unmarshal(frame, &m)
	if m.From != p.ID || m.DataBase64 != "GWhpCg==" {
		t.Fatalf("input received mismatch: %+v", m)
	}
}

func TestViewerPointerReachesHostWithIdentity(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s-pointer")

	position := types.PointerPosition{X: 0.25, Y: 0.75}
	hub.ForwardPointer(ctx, "s-pointer", p.ID.String(), position)
	frame := host.waitType(t, types.TypePointerMoved)
	var m types.ServerPointerMoved
	_ = json.Unmarshal(frame, &m)
	if m.From != p.ID || m.Position != position {
		t.Fatalf("pointer mismatch: %+v", m)
	}
}

func TestRequestAccessReachesHost(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s9a")

	hub.RequestAccess(ctx, "s9a", p.ID.String())
	frame := host.waitType(t, types.TypeAccessRequested)
	var m types.ServerAccessRequested
	_ = json.Unmarshal(frame, &m)
	if m.Participant != p.ID {
		t.Fatalf("access request participant mismatch: %+v", m)
	}
}

func TestViewersSeeHostInPresence(t *testing.T) {
	hub := plainHub()
	ctx := context.Background()
	host := newFakeSink()
	viewer := newFakeSink()
	hub.RegisterHost("s9b", "host-user", "Host Person", "https://cdn.test/host.png", host)
	p := viewerParticipant("V")
	hub.RegisterViewer(ctx, "s9b", p, "viewer-user", viewer)

	// The viewer's participant list includes a synthetic host presence entry.
	frame := viewer.waitType(t, types.TypeParticipantsUpdated)
	var m types.ServerParticipantsUpdated
	_ = json.Unmarshal(frame, &m)
	sawHost := false
	for _, part := range m.Participants {
		if part.ID == types.HostParticipantID && part.Role == types.RoleHost {
			if part.DisplayName != "Host Person" || part.AvatarURL != "https://cdn.test/host.png" {
				t.Fatalf("host profile mismatch: %+v", part)
			}
			sawHost = true
		}
	}
	if !sawHost {
		t.Fatalf("viewer presence list missing host entry: %+v", m.Participants)
	}
}

func TestSameUserAutoApproved(t *testing.T) {
	audit := &MemoryAuditor{}
	hub := NewHub(NewMemoryBus(), AllowAll{}, audit)
	ctx := context.Background()
	host := newFakeSink()
	hub.RegisterHost("s10", "alice", "", "", host)
	p := viewerParticipant("Alice-phone")
	hub.RegisterViewer(ctx, "s10", p, "alice", newFakeSink()) // SAME account as the host
	host.waitType(t, types.TypeParticipantsUpdated)
	hub.SetParticipantAccess(ctx, "s10", p.ID.String(), types.AccessControl)

	hub.RequestCommand(ctx, "s10", p.ID.String(), "ls")
	frame := host.waitType(t, types.TypeCommandRequested)
	var m types.ServerCommandRequested
	_ = json.Unmarshal(frame, &m)
	if !m.AutoApproved {
		t.Fatal("the host's own account should be auto-approved (no prompt)")
	}
	// Auto-approved commands are audited immediately, without a ResolveCommand.
	if events := audit.Events(); len(events) != 1 || !events[0].Approved {
		t.Fatalf("want 1 auto-approved audit event, got %+v", events)
	}
}

func TestApprovalIsAudited(t *testing.T) {
	audit := &MemoryAuditor{}
	hub := NewHub(NewMemoryBus(), AllowAll{}, audit)
	ctx := context.Background()
	host := newFakeSink()
	hub.RegisterHost("s8", "host-user", "", "", host)
	p := viewerParticipant("Ctrl")
	hub.RegisterViewer(ctx, "s8", p, "viewer-user", newFakeSink())
	host.waitType(t, types.TypeParticipantsUpdated)
	hub.SetParticipantAccess(ctx, "s8", p.ID.String(), types.AccessControl)

	hub.RequestCommand(ctx, "s8", p.ID.String(), "deploy")
	frame := host.waitType(t, types.TypeCommandRequested)
	var m types.ServerCommandRequested
	_ = json.Unmarshal(frame, &m)

	hub.ResolveCommand(ctx, "s8", m.RequestID.String(), true)
	events := audit.Events()
	if len(events) != 1 {
		t.Fatalf("want 1 audit event, got %d", len(events))
	}
	e := events[0]
	if !e.Approved || e.Command != "deploy" || e.HostUser != "host-user" || e.ParticipantID != p.ID.String() {
		t.Fatalf("audit event wrong: %+v", e)
	}
}

func TestRateLimitDropsExcess(t *testing.T) {
	hub := NewHub(NewMemoryBus(), NewMemoryRateLimiter(2, time.Hour), NoopAuditor{})
	ctx := context.Background()
	host, p := hostWithViewer(t, hub, "s9")
	hub.SetParticipantAccess(ctx, "s9", p.ID.String(), types.AccessControl)

	for i := 0; i < 3; i++ {
		hub.RequestCommand(ctx, "s9", p.ID.String(), "cmd")
	}
	// Exactly two get through; the third is rate-limited.
	host.waitType(t, types.TypeCommandRequested)
	host.waitType(t, types.TypeCommandRequested)
	host.assertNoType(t, types.TypeCommandRequested, 300*time.Millisecond)
}
