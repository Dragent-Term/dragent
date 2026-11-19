package components

import (
	"context"
	"testing"
)

func TestMemoryBusRetainsFrontendModuleBurst(t *testing.T) {
	bus := NewMemoryBus()
	sub, err := bus.Subscribe("session")
	if err != nil {
		t.Fatalf("subscribe: %v", err)
	}
	defer sub.Close()

	for i := 0; i < forwardBurstBuffer; i++ {
		if err := bus.Publish(context.Background(), "session", []byte("forward-open")); err != nil {
			t.Fatalf("publish %d: %v", i, err)
		}
	}
	if got := len(sub.C()); got != forwardBurstBuffer {
		t.Fatalf("queued %d messages, want %d", got, forwardBurstBuffer)
	}
}
