// Shared harness for the R2 RPC tests: an authenticated router plus
// frame/post helpers. Auth injection mirrors handler_test.go (X-Test-User
// header stands in for RequireAuth), with distinct helper names.
package relay

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
)

func rpcTestRouter(h *RPCHandler) http.Handler {
	r := chi.NewRouter()
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if user := req.Header.Get("X-Test-User"); user != "" {
				req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
			}
			next.ServeHTTP(w, req)
		})
	})
	h.RegisterRPCRoutes(r)
	return r
}

func newRPCServer() (*httptest.Server, *MemoryHostStore, *EventBus) {
	states := NewMemoryHostStore()
	bus := NewEventBus()
	return httptest.NewServer(rpcTestRouter(NewRPCHandler(states, bus))), states, bus
}

// rpcReplyFrame decodes an rpc-reply (or transport error) body.
// Transport errors carry {"error":"<message>"} with a string payload while
// replies carry an object; decodeReply normalizes both into Error.
type rpcReplyFrame struct {
	Type   string         `json:"type"`
	V      int            `json:"v"`
	ID     string         `json:"id"`
	OK     bool           `json:"ok"`
	Result map[string]any `json:"result"`
	Error  *rpcErrorMsg   `json:"-"`
}

type rpcErrorMsg struct {
	Message       string `json:"message"`
	LoginRequired bool   `json:"loginRequired"`
}

func decodeReply(raw []byte) rpcReplyFrame {
	var reply rpcReplyFrame
	var probe map[string]json.RawMessage
	if err := json.Unmarshal(raw, &probe); err != nil {
		return reply
	}
	_ = json.Unmarshal(raw, &reply)
	if errRaw, ok := probe["error"]; ok {
		var msg string
		if err := json.Unmarshal(errRaw, &msg); err == nil {
			var lr struct {
				LoginRequired bool `json:"loginRequired"`
			}
			_ = json.Unmarshal(raw, &lr)
			reply.Error = &rpcErrorMsg{Message: msg, LoginRequired: lr.LoginRequired}
		} else {
			var obj rpcErrorMsg
			if err := json.Unmarshal(errRaw, &obj); err == nil {
				reply.Error = &obj
			}
		}
	}
	return reply
}

func postFrame(t *testing.T, srv *httptest.Server, user, body string) (int, rpcReplyFrame) {
	t.Helper()
	req, _ := http.NewRequest(http.MethodPost, srv.URL+"/v1/relay/rpc", strings.NewReader(body))
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	var probe map[string]json.RawMessage
	if err := json.Unmarshal(raw, &probe); err != nil {
		t.Fatalf("reply is not JSON (%d): %s", resp.StatusCode, raw)
	}
	return resp.StatusCode, decodeReply(raw)
}

func rpcRequest(t *testing.T, srv *httptest.Server, user, id, method, params string) (int, rpcReplyFrame) {
	t.Helper()
	if params == "" {
		params = `{}`
	}
	return postFrame(t, srv, user, `{"type":"rpc-request","v":1,"id":`+quoteJSON(id)+`,"method":`+quoteJSON(method)+`,"params":`+params+`}`)
}

func quoteJSON(s string) string {
	raw, _ := json.Marshal(s)
	return string(raw)
}

// publishMirror publishes to the removed host-state route, returning the
// HTTP status. After the cut-over it is always 410 (401 anonymous).
func publishMirror(t *testing.T, srv *httptest.Server, user string, body any) int {
	t.Helper()
	raw, _ := json.Marshal(body)
	req, _ := http.NewRequest(http.MethodPut, srv.URL+"/v1/relay/host/state", bytes.NewReader(raw))
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, resp.Body)
	return resp.StatusCode
}

func seedDevice(t *testing.T, srv *httptest.Server) {
	t.Helper()
	status := publishMirror(t, srv, "user-a", map[string]any{
		"device_id": "device-1",
		"tabs": []any{
			map[string]any{"id": 7, "kind": "terminal", "title": "zsh", "active": true, "visible": true},
		},
		"worktrees": []any{
			map[string]any{"name": "relay", "path": "/Users/me/dragent-wt", "branch": "feat/x", "project": "/Users/me/dragent"},
		},
		"listings": []any{
			map[string]any{"path": "/Users/me/dragent-wt", "entries": []any{
				map[string]any{"name": ".gitignore", "kind": "file"},
				map[string]any{"name": "main.go", "kind": "file"},
				map[string]any{"name": "server", "kind": "dir"},
			}},
		},
		"contents": []any{
			map[string]any{"path": "/Users/me/dragent-wt/main.go", "content": "package main\n"},
		},
	})
	if status != http.StatusOK {
		t.Fatalf("seed publish status = %d", status)
	}
}

func TestRPCEnvelopeValidation(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()

	// The HTTP RPC transport is gone (§10.5): every frame is 410 before the
	// body is read — malformed, unknown, oversize, all alike. Anonymous
	// callers get 401 first.
	if status, _ := postFrame(t, srv, "user-a", "not json"); status != http.StatusGone {
		t.Fatalf("malformed frame status = %d, want 410", status)
	}
	if status, _ := postFrame(t, srv, "user-a", `{"type":"rpc-request","v":1,"method":"tabs.list","params":{}}`); status != http.StatusGone {
		t.Fatalf("missing id status = %d, want 410", status)
	}
	status, reply := postFrame(t, srv, "user-a", `{"type":"rpc-request","v":2,"id":"a","method":"tabs.list","params":{}}`)
	if status != http.StatusGone {
		t.Fatalf("bad version: status=%d reply=%+v", status, reply)
	}
	status, reply = postFrame(t, srv, "user-a", `{"type":"rpc-smoke","v":1,"id":"b"}`)
	if status != http.StatusGone {
		t.Fatalf("unknown frame: status=%d reply=%+v", status, reply)
	}
	// A huge body is still 410: the status comes before any body byte.
	big := `{"type":"rpc-request","v":1,"id":"e","method":"tabs.list","params":{"device_id":"device-1","pad":"` + strings.Repeat("x", 4*1024*1024) + `"}}`
	if status, _ := postFrame(t, srv, "user-a", big); status != http.StatusGone {
		t.Fatalf("oversize body status = %d, want 410", status)
	}
	// Content methods over the socket transport are 410 with a repair message.
	status, reply = rpcRequest(t, srv, "user-a", "c", "nope.nope", `{"device_id":"device-1"}`)
	if status != http.StatusGone {
		t.Fatalf("HTTP unknown method: status=%d, want 410", status)
	}
}

func TestRPCAuthRules(t *testing.T) {
	srv, _, _ := newRPCServer()
	defer srv.Close()

	// Anonymous: 401 with loginRequired:true (before the 410).
	status, reply := rpcRequest(t, srv, "", "a", "tabs.list", `{"device_id":"device-1"}`)
	if status != http.StatusUnauthorized || reply.OK || reply.Error == nil || !reply.Error.LoginRequired {
		t.Fatalf("anonymous: status=%d reply=%+v", status, reply)
	}
	// Every content method is 410 once authenticated — foreign, unknown,
	// and missing devices alike, since nothing is served.
	for name, params := range map[string]string{
		"foreign device": `{"device_id":"device-1"}`,
		"unknown device": `{"device_id":"never-there"}`,
		"missing device": `{}`,
	} {
		status, reply = rpcRequest(t, srv, "user-b", name, "tabs.list", params)
		if status != http.StatusGone || reply.OK {
			t.Fatalf("%s: status=%d reply=%+v, want 410", name, status, reply)
		}
	}
	// Host publish is authenticated too, then gone.
	if status := publishMirror(t, srv, "", map[string]any{"device_id": "device-1"}); status != http.StatusUnauthorized {
		t.Fatalf("anonymous publish status = %d, want 401", status)
	}
	if status := publishMirror(t, srv, "user-b", map[string]any{"device_id": "device-1"}); status != http.StatusGone {
		t.Fatalf("authenticated publish status = %d, want 410", status)
	}
}
