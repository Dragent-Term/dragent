package companionsession

import (
	"context"
	"encoding/json"
	"log/slog"
	"time"

	"github.com/redis/go-redis/v9"
)

// Cross-instance revocation events.
//
// A phone's relay socket may live on a different API instance from the one
// that serves its revocation. Rather than every socket re-reading Redis on a
// short cadence (two GETs every 10 s per phone, ~518K commands per phone per
// month), the revoking instance PUBLISHes one event and each API instance
// holds ONE subscription that closes its matching local sockets. Sockets keep
// a slow safety poll so a lost message (subscriber reconnecting, publish
// failing) still ends the socket.

// RevocationChannel is the Redis pub/sub channel carrying revocations.
const RevocationChannel = "companion:revocations:v1"

// Revocation kinds.
const (
	// RevokedDevice: a paired phone was revoked (DELETE device or token
	// reuse). Ends every socket admitted for UserID+DeviceID.
	RevokedDevice = "device"
	// RevokedLease: one companion lease was revoked. Ends every socket
	// admitted on LeaseID.
	RevokedLease = "lease"
)

// Revocation is one published event. Fields carry identifiers only; never
// tokens or secrets.
type Revocation struct {
	Kind     string `json:"k"`
	UserID   string `json:"u"`
	DeviceID string `json:"d,omitempty"`
	LeaseID  string `json:"l,omitempty"`
}

// valid rejects malformed or foreign payloads before they reach a handler.
func (r Revocation) valid() bool {
	if !validIdentity(r.UserID, false) {
		return false
	}
	switch r.Kind {
	case RevokedDevice:
		return validIdentity(r.DeviceID, false) && r.LeaseID == ""
	case RevokedLease:
		return validID(r.LeaseID) && validIdentity(r.DeviceID, true)
	}
	return false
}

// publishTimeout bounds the best-effort publish so a slow Redis never holds
// the revoking request.
const publishTimeout = 2 * time.Second

// PublishRevocation announces a revocation to every API instance. It is
// best-effort: a failed publish is logged (without ids) and the sockets'
// safety poll ends them instead. Nil client is a no-op.
func PublishRevocation(ctx context.Context, client *redis.Client, event Revocation) {
	if client == nil || !event.valid() {
		return
	}
	data, err := json.Marshal(event)
	if err != nil {
		return
	}
	pubCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), publishTimeout)
	defer cancel()
	if err := client.Publish(pubCtx, RevocationChannel, data).Err(); err != nil {
		slog.Warn("companion revocation publish failed", "kind", event.Kind, "error", err)
	}
}

// RevocationSource delivers revocations published by any API instance.
// onEvent runs for every valid event; onResubscribe runs after the
// subscription was lost and re-established (messages may have been missed
// in between), so callers can re-check their sockets at once. The returned
// stop func ends the subscription.
type RevocationSource interface {
	SubscribeRevocations(onEvent func(Revocation), onResubscribe func()) (stop func())
}

// revocationHealthCheck is how long an idle subscription waits before
// pinging Redis. go-redis defaults to 3 s, i.e. ~864K PINGs a month per
// subscription on a pay-per-request plan. One minute keeps dead-connection
// detection bounded (missed events are covered by the safety poll and the
// resubscribe re-check) at ~43K a month per API instance.
const revocationHealthCheck = time.Minute

// SubscribeRevocations holds one subscription on RevocationChannel for the
// process (call it once per instance, not per socket).
func (s *RedisStore) SubscribeRevocations(onEvent func(Revocation), onResubscribe func()) func() {
	if s == nil || s.client == nil || onEvent == nil {
		return func() {}
	}
	return subscribeRevocations(s.client, onEvent, onResubscribe)
}

func subscribeRevocations(client *redis.Client, onEvent func(Revocation), onResubscribe func()) func() {
	ps := client.Subscribe(context.Background(), RevocationChannel)
	ch := ps.ChannelWithSubscriptions(redis.WithChannelHealthCheckInterval(revocationHealthCheck))
	done := make(chan struct{})
	go func() {
		defer close(done)
		subscribed := false
		for item := range ch {
			switch msg := item.(type) {
			case *redis.Subscription:
				if msg.Kind != "subscribe" {
					continue
				}
				// The first confirmation is the initial subscribe; any later
				// one follows a reconnect, so events may have been missed.
				if subscribed && onResubscribe != nil {
					onResubscribe()
				}
				subscribed = true
			case *redis.Message:
				var event Revocation
				if json.Unmarshal([]byte(msg.Payload), &event) != nil || !event.valid() {
					continue
				}
				onEvent(event)
			}
		}
	}()
	return func() {
		_ = ps.Close()
		<-done
	}
}

var _ RevocationSource = (*RedisStore)(nil)
