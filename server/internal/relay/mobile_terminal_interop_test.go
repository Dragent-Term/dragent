package relay

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"net/http/httptest"
)

// A clearly synthetic desktop on the real host wire. No shell is executed.
// The opt-in shipping-TypeScript test exercises both actual relay sockets.
func startMobileInteropHost(t *testing.T, server *httptest.Server) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	t.Cleanup(cancel)
	conn, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(server.URL, "http")+"/v1/relay/host", nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { conn.CloseNow() })
	socketWrite(t, ctx, conn, map[string]any{"type": "relay-host-auth", "v": 1, "access_token": "test-token", "device_id": "desktop", "expected_user_id": "account-a"})
	if ready := socketRead(t, ctx, conn); ready["type"] != "relay-host-ready" {
		t.Fatal(ready)
	}
	go func() {
		output := []byte("fixture output\n")
		for {
			_, raw, err := conn.Read(ctx)
			if err != nil {
				return
			}
			var frame map[string]any
			if json.Unmarshal(raw, &frame) != nil {
				return
			}
			if frame["type"] == "relay-ping" {
				data, _ := json.Marshal(map[string]any{"type": "relay-pong", "v": 1})
				if conn.Write(ctx, websocket.MessageText, data) != nil {
					return
				}
				continue
			}
			if frame["type"] != "host-request" {
				return
			}
			params, ok := frame["params"].(map[string]any)
			if !ok || params["session_id"] == "" {
				return
			}
			var result any
			switch frame["method"] {
			case "coordination.list":
				result = map[string]any{"items": []any{map[string]any{"id": "team-12345678", "title": "Fixture coordination", "project_path": "/repo", "status": "running", "task_count": 1, "done_count": 0, "running_count": 1, "blocked_count": 0, "needs_user": false, "updated_at": 1}}, "truncated": false}
			case "coordination.get":
				result = map[string]any{"team": map[string]any{"id": "team-12345678", "title": "Fixture coordination", "project_path": "/repo", "status": "running", "goal": "Verify the local companion adapter", "tasks": []any{map[string]any{"id": "task-1", "title": "Fixture worker", "status": "running", "agent": "opencode", "model": nil, "depends_on": []string{}, "result_summary": nil}}, "messages": []any{map[string]any{"id": "1", "from": "lead", "label": "Lead", "kind": "note", "text": "Synthetic fixture message", "created_at": 1}}}, "truncated": false}
			case "terminal.attach":
				result = map[string]any{"rows": 2, "cols": 80}
			case "terminal.read":
				offset, ok := params["offset"].(float64)
				if !ok || offset < 0 || offset > float64(len(output)) {
					return
				}
				result = map[string]any{"base": int(offset), "total": len(output), "data": base64.StdEncoding.EncodeToString(output[int(offset):]), "alive": true}
			case "terminal.input":
				text, ok := params["text"].(string)
				if !ok {
					return
				}
				output = append(output, []byte(text)...)
				result = map[string]bool{"sent": true}
			case "terminal.resize":
				result = map[string]bool{"resized": true}
			case "terminal.screen":
				result = map[string]any{"complete": true, "total": len(output), "delta": map[string]any{"kind": "alt_screen", "rows": 2, "cols": 80, "cursor_row": 1, "cursor_col": 0, "cursor_visible": true, "lines": []any{map[string]any{"spans": []any{map[string]any{"text": "fixture screen", "flags": 0}}}, map[string]any{"spans": []any{}}}}}
			case "terminal.detach":
				result = map[string]bool{"detached": true}
			default:
				return
			}
			reply, _ := json.Marshal(map[string]any{"type": "host-reply", "v": 1, "id": frame["id"], "ok": true, "result": result})
			if conn.Write(ctx, websocket.MessageText, reply) != nil {
				return
			}
		}
	}()
}
