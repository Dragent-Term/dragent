// Screen modes travel inside the KK channel after the E2EE cut-over (F1):
// the server JSON terminal proxy is removed. Attach and screen answer 410
// and nothing reaches the desktop.
package relay

import (
	"testing"
)

func TestTabsAttachIsGone(t *testing.T) {
	f := newTerminalFixture(t)
	f.publish(t, "account-a", "desktop")
	host := f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")

	sendTerminalRPC(t, phone, ctx, "tabs.attach", "desktop", map[string]any{"tab_id": 1})
	reply := socketRead(t, ctx, phone)
	if reply["ok"] != false {
		t.Fatalf("tabs.attach admitted: %v", reply)
	}
	assertNothingReachedDesktop(t, host)
}

func TestTabsScreenIsGone(t *testing.T) {
	f := newTerminalFixture(t)
	f.publish(t, "account-a", "desktop")
	host := f.host(t, "account-a", "desktop")
	phone, ctx, _ := f.phone(t, "account-a", "desktop", "")

	sendTerminalRPC(t, phone, ctx, "tabs.screen", "desktop", map[string]any{"handle": "00000000-0000-4000-8000-000000000000"})
	reply := socketRead(t, ctx, phone)
	if reply["ok"] != false {
		t.Fatalf("tabs.screen admitted: %v", reply)
	}
	assertNothingReachedDesktop(t, host)
}
