package relay

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// deviceAuth admits two phones of one account, distinguished by token, so
// revocation tests can hold two live sockets and close exactly one.
func deviceAuth(_ context.Context, token string) (SocketIdentity, error) {
	switch token {
	case "test-account-token":
		return SocketIdentity{UserID: "account-a", DeviceID: "phone-1", ExpiresAt: time.Now().Add(time.Hour)}, nil
	case "test-account-token-b":
		return SocketIdentity{UserID: "account-a", DeviceID: "phone-2", ExpiresAt: time.Now().Add(time.Hour)}, nil
	}
	return SocketIdentity{}, errors.New("bad token")
}

func admitPhone(t *testing.T, c *websocket.Conn, token string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-auth", "v": 1, "e2e": 1, "access_token": token})
	if got := socketRead(t, ctx, c); got["type"] != "relay-ready" {
		t.Fatalf("phone admission: %v", got)
	}
}

func TestCloseDeviceSocketsClosesOnlyItsOwn(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, deviceAuth)

	c1, _ := openSocket(t, srv)
	admitPhone(t, c1, "test-account-token")
	c2, _ := openSocket(t, srv)
	admitPhone(t, c2, "test-account-token-b")

	// Unknown scopes close nothing.
	h.CloseDeviceSockets("account-a", "phone-unknown")
	h.CloseDeviceSockets("account-b", "phone-1")

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	h.CloseDeviceSockets("account-a", "phone-1")
	if code := readCloseCode(t, ctx, c1); code != companionSessionEndedCode {
		t.Fatalf("revoked phone close code %d, want %d", code, companionSessionEndedCode)
	}

	// The other phone stays connected.
	pingCtx, pingCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer pingCancel()
	socketWrite(t, pingCtx, c2, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, pingCtx, c2); got["type"] != "relay-pong" {
		t.Fatalf("surviving phone lost: %v", got)
	}
}

func TestDevicePollRevocationEndsSocket(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	h.SetDeviceAuthorizer(func(_ context.Context, userID, deviceID string) (bool, error) {
		return deviceID != "phone-1", nil
	})
	h.sessionPollInterval = 20 * time.Millisecond

	c1, _ := openSocket(t, srv)
	admitPhone(t, c1, "test-account-token")

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if code := readCloseCode(t, ctx, c1); code != companionSessionEndedCode {
		t.Fatalf("poll-revoked phone close code %d, want %d", code, companionSessionEndedCode)
	}
}

// Storage not answering is not a revocation: the phone stays connected
// (it used to be told its session ended on the next 10 s check).
func TestDevicePollStorageErrorKeepsSocket(t *testing.T) {
	now := time.Now()
	srv, h, _ := companionSocketServer(t, func() time.Time { return now }, deviceAuth)
	h.SetDeviceAuthorizer(func(context.Context, string, string) (bool, error) {
		return false, errors.New("storage timeout")
	})
	h.sessionPollInterval = 20 * time.Millisecond

	c, _ := openSocket(t, srv)
	admitPhone(t, c, "test-account-token")
	time.Sleep(200 * time.Millisecond) // several failed checks

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	socketWrite(t, ctx, c, map[string]any{"type": "relay-ping", "v": 1})
	if got := socketRead(t, ctx, c); got["type"] != "relay-pong" {
		t.Fatalf("phone dropped on a storage error: %v", got)
	}
}

func TestCloseDeviceSocketsNilSafe(t *testing.T) {
	var nilHandler *RPCHandler
	nilHandler.CloseDeviceSockets("account-a", "phone-1") // must not panic
	(&RPCHandler{}).CloseDeviceSockets("account-a", "phone-1")
}
