package main

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"log/slog"
	"net/http"
	"time"

	"github.com/redis/go-redis/v9"
)

// Realtime tickets let the browser viewer join /realtime as the signed-in
// person without its access token in the WebSocket URL (browsers can't set
// headers on a WebSocket). The portal's server, which holds the session in
// an httpOnly cookie, asks for a ticket; the page connects with
// ?ticket=<t>. A ticket works only on /realtime and only for a minute, so
// one that lands in a log is worth little.

const realtimeTicketTTL = time.Minute

type realtimeTickets struct{ rdb *redis.Client }

type ticketIdentity struct {
	UserID   string `json:"user_id"`
	DeviceID string `json:"device_id"`
}

func ticketKey(ticket string) string {
	sum := sha256.Sum256([]byte(ticket))
	return "rtticket:" + hex.EncodeToString(sum[:])
}

func (t realtimeTickets) issue(ctx context.Context, id ticketIdentity) (string, error) {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	ticket := base64.RawURLEncoding.EncodeToString(b)
	data, err := json.Marshal(id)
	if err != nil {
		return "", err
	}
	if err := t.rdb.Set(ctx, ticketKey(ticket), data, realtimeTicketTTL).Err(); err != nil {
		return "", err
	}
	return ticket, nil
}

func (t realtimeTickets) resolve(ctx context.Context, ticket string) (ticketIdentity, bool) {
	if len(ticket) != 43 {
		return ticketIdentity{}, false
	}
	val, err := t.rdb.Get(ctx, ticketKey(ticket)).Result()
	if err != nil {
		return ticketIdentity{}, false
	}
	var id ticketIdentity
	if json.Unmarshal([]byte(val), &id) != nil || id.UserID == "" {
		return ticketIdentity{}, false
	}
	return id, true
}

// POST /realtime/ticket, signed in → {"ticket", "expires_in"}.
func (t realtimeTickets) handleIssue(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	deviceID, _ := r.Context().Value("device_id").(string)
	ticket, err := t.issue(r.Context(), ticketIdentity{UserID: userID, DeviceID: deviceID})
	if err != nil {
		slog.Error("realtime ticket issue failed", "error", err)
		http.Error(w, "couldn't start the session, try again", http.StatusServiceUnavailable)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"ticket":     ticket,
		"expires_in": int(realtimeTicketTTL.Seconds()),
	})
}

// middleware signs a /realtime request in from its ticket. Only /realtime:
// a ticket is no use anywhere else.
func (t realtimeTickets) middleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/realtime" {
			if uid, _ := r.Context().Value("user_id").(string); uid == "" {
				if ticket := r.URL.Query().Get("ticket"); ticket != "" {
					if id, ok := t.resolve(r.Context(), ticket); ok {
						ctx := context.WithValue(r.Context(), "user_id", id.UserID)
						ctx = context.WithValue(ctx, "device_id", id.DeviceID)
						r = r.WithContext(ctx)
					}
				}
			}
		}
		next.ServeHTTP(w, r)
	})
}
