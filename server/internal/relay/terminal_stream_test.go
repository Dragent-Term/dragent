// Terminal stream removal tests: after the E2EE cut-over (F1) the terminal
// travels as binary TERM records inside the KK channel. Server-side JSON
// stream subscriptions are removed: subscribing answers 410 and nothing
// reaches the desktop.
package relay

import (
	"testing"

	"github.com/google/uuid"
)

func TestTerminalStreamSubscribeIsGone(t *testing.T) {
	f := newTerminalFixture(t)
	f.publish(t, "account-a", "desktop")
	host := f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")

	handle := "00000000-0000-4000-8000-000000000000"
	socketWrite(t, ctx, phone, map[string]any{
		"type": "rpc-subscribe", "v": 1, "id": uuid.NewString(),
		"stream": "terminal." + handle,
		"params": map[string]any{"device_id": "desktop", "handle": handle},
	})
	reply := socketRead(t, ctx, phone)
	if reply["ok"] != false {
		t.Fatalf("terminal stream admitted: %v", reply)
	}
	assertNothingReachedDesktop(t, host)
}

func TestTerminalStreamUnknownStreamStillRefused(t *testing.T) {
	f := newTerminalFixture(t)
	f.publish(t, "account-a", "desktop")
	_ = f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")

	socketWrite(t, ctx, phone, map[string]any{
		"type": "rpc-subscribe", "v": 1, "id": uuid.NewString(),
		"stream": "terminal.no-such-handle",
		"params": map[string]any{"device_id": "desktop", "handle": "00000000-0000-4000-8000-000000000001"},
	})
	reply := socketRead(t, ctx, phone)
	if reply["ok"] != false {
		t.Fatalf("unknown terminal stream admitted: %v", reply)
	}
}
