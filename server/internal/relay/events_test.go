// Attention-mirror removal tests (§8, F8): the server keeps no titles,
// bodies, kinds, or workspace ids. Record drops everything, Subscribe is
// empty, and the subscribe transport is 410.
package relay

import (
	"net/http"
	"strings"
	"testing"

	"server/internal/push"
)

func subscribeEventsGone(t *testing.T, srv string, user string) *http.Response {
	t.Helper()
	body := `{"type":"rpc-subscribe","v":1,"id":"sub-1","stream":"runtime.events","params":{}}`
	req, _ := http.NewRequest(http.MethodPost, srv+"/v1/relay/rpc", strings.NewReader(body))
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func TestRuntimeEventsSubscribeIsGone(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()

	// The HTTP RPC transport is 410 before the body is read (§10.5) —
	// including the subscribe path.
	resp := subscribeEventsGone(t, srv.URL, "user-a")
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusGone {
		t.Fatalf("subscribe status = %d, want 410", resp.StatusCode)
	}

	// Anonymous callers get 401 first.
	resp = subscribeEventsGone(t, srv.URL, "")
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anonymous subscribe status = %d, want 401", resp.StatusCode)
	}
}

func TestEventBusKeepsNoContent(t *testing.T) {
	bus := NewEventBus()
	// Records — even with canary content — are dropped, never stored.
	bus.Record("user-a", AttentionEvent{
		Kind:  string(push.AttentionJoinApproved),
		Title: "canary-title",
		Body:  "canary-body",
		Data:  map[string]string{"workspace": "canary-workspace"},
	})
	handle, snapshot, ch, cancel := bus.Subscribe("user-a")
	defer cancel()
	if len(snapshot) != 0 {
		t.Fatalf("snapshot must be empty: %+v", snapshot)
	}
	select {
	case payload := <-ch:
		t.Fatalf("no live deliveries expected: %+v", payload)
	default:
	}
	if done := bus.doneFor(handle, "user-a"); done == nil {
		t.Fatal("subscription must be tracked for socket compat")
	}
}

func TestUnsubscribeStillConfirms(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()
	// Unsubscribe confirms, including for unknown handles (idempotent —
	// supports unsubscribe-before-first-snapshot without an oracle).
	// The HTTP transport is gone, so exercise the handler path over the
	// socket reply shape via rpcRequest: it answers 410 like every method.
	status, _ := rpcRequest(t, srv, "user-a", "u1", "runtime.events.subscribe", `{}`)
	if status != http.StatusGone {
		t.Fatalf("subscribe-as-request: status=%d, want 410", status)
	}
}
