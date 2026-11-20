package main

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/alicebob/miniredis/v2"
	"github.com/redis/go-redis/v9"
)

func ticketsForTest(t *testing.T) (realtimeTickets, *miniredis.Miniredis) {
	mr := miniredis.RunT(t)
	return realtimeTickets{rdb: redis.NewClient(&redis.Options{Addr: mr.Addr()})}, mr
}

func whoami(t realtimeTickets, target string) string {
	var got string
	h := t.middleware(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, _ = r.Context().Value("user_id").(string)
	}))
	h.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest(http.MethodGet, target, nil))
	return got
}

// A ticket signs the viewer in on /realtime only, and only until it expires.
func TestRealtimeTicket(t *testing.T) {
	tickets, mr := ticketsForTest(t)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/realtime/ticket", nil)
	req = req.WithContext(withUser(req, "u1", "d1"))
	tickets.handleIssue(rec, req)
	var body struct {
		Ticket string `json:"ticket"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &body); err != nil || body.Ticket == "" {
		t.Fatalf("issue: %d %s", rec.Code, rec.Body)
	}

	if got := whoami(tickets, "/realtime?ticket="+body.Ticket); got != "u1" {
		t.Fatalf("on /realtime: %q, want u1", got)
	}
	if got := whoami(tickets, "/query?ticket="+body.Ticket); got != "" {
		t.Fatalf("elsewhere: %q, want nobody", got)
	}
	if got := whoami(tickets, "/realtime?ticket="+strings.Repeat("x", 43)); got != "" {
		t.Fatalf("made-up ticket: %q", got)
	}
	mr.FastForward(realtimeTicketTTL + 1)
	if got := whoami(tickets, "/realtime?ticket="+body.Ticket); got != "" {
		t.Fatalf("expired ticket: %q", got)
	}
}

func withUser(r *http.Request, user, device string) context.Context {
	ctx := context.WithValue(r.Context(), "user_id", user)
	return context.WithValue(ctx, "device_id", device)
}
