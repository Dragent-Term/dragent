// Package components implements the realtime session hub: a per-instance
// registry of live WebSocket connections, bridged across gateway instances by a
// publish/subscribe Bus so a host's output reaches viewers wherever they landed.
package components

import (
	"context"
	"sync"
	"time"

	"github.com/redis/go-redis/v9"
)

// forwardBurstBuffer is deliberately large enough for a modern frontend's
// initial module graph.  Port-forward opens are control messages: dropping one
// leaves the browser waiting until the public gateway times out.
const forwardBurstBuffer = 1024

// busHealthCheck is how long an idle session subscription waits before it
// PINGs Redis to detect a dead connection. go-redis defaults to 3 s, which on
// a pay-per-request Redis is ~864K commands a month for every live session
// room, idle or not. Any delivered message resets the timer, so busy rooms
// never ping; an idle room pings ~86K times a month at 30 s, and a dead
// connection is still noticed and redialed within ~35 s.
const busHealthCheck = 30 * time.Second

// Bus is the message fan-out between gateway instances. One channel per session.
type Bus interface {
	Publish(ctx context.Context, channel string, data []byte) error
	Subscribe(channel string) (Subscription, error)
}

// Subscription delivers messages published to one channel.
type Subscription interface {
	C() <-chan []byte
	Close()
}

// ── In-memory bus (single instance; also used in tests) ──────────────────────

// MemoryBus fans out within one process. A single-instance deployment can use
// it directly; tests use it to exercise the hub without Redis.
type MemoryBus struct {
	mu   sync.Mutex
	subs map[string]map[int]chan []byte
	next int
}

func NewMemoryBus() *MemoryBus {
	return &MemoryBus{subs: make(map[string]map[int]chan []byte)}
}

func (b *MemoryBus) Publish(_ context.Context, channel string, data []byte) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	for _, ch := range b.subs[channel] {
		// Copy: the caller may reuse its buffer. Do not drop forwarding control
		// messages under a frontend module burst; bounded channel backpressure is
		// preferable to an invisible request timeout at the public gateway.
		cp := append([]byte(nil), data...)
		ch <- cp
	}
	return nil
}

func (b *MemoryBus) Subscribe(channel string) (Subscription, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	id := b.next
	b.next++
	ch := make(chan []byte, forwardBurstBuffer)
	if b.subs[channel] == nil {
		b.subs[channel] = make(map[int]chan []byte)
	}
	b.subs[channel][id] = ch
	return &memSub{bus: b, channel: channel, id: id, ch: ch}, nil
}

type memSub struct {
	bus     *MemoryBus
	channel string
	id      int
	ch      chan []byte
	once    sync.Once
}

func (s *memSub) C() <-chan []byte { return s.ch }

func (s *memSub) Close() {
	s.once.Do(func() {
		s.bus.mu.Lock()
		defer s.bus.mu.Unlock()
		if m := s.bus.subs[s.channel]; m != nil {
			delete(m, s.id)
			if len(m) == 0 {
				delete(s.bus.subs, s.channel)
			}
		}
		close(s.ch)
	})
}

// ── Redis bus (multi-instance) ───────────────────────────────────────────────

// RedisBus fans out across gateway instances via Redis pub/sub.
type RedisBus struct {
	rdb *redis.Client
}

func NewRedisBus(rdb *redis.Client) *RedisBus { return &RedisBus{rdb: rdb} }

func (b *RedisBus) Publish(ctx context.Context, channel string, data []byte) error {
	return b.rdb.Publish(ctx, channel, data).Err()
}

func (b *RedisBus) Subscribe(channel string) (Subscription, error) {
	ps := b.rdb.Subscribe(context.Background(), channel)
	sub := &redisSub{ps: ps, ch: make(chan []byte, forwardBurstBuffer), done: make(chan struct{})}
	go sub.pump()
	return sub, nil
}

type redisSub struct {
	ps   *redis.PubSub
	ch   chan []byte
	done chan struct{}
	once sync.Once
}

func (s *redisSub) pump() {
	// go-redis defaults this queue to 100 messages. A Vite page commonly emits
	// more imports than that at once, so the bridge must retain the same burst
	// capacity as the hub-side subscription.
	rc := s.ps.Channel(redis.WithChannelSize(forwardBurstBuffer), redis.WithChannelHealthCheckInterval(busHealthCheck))
	for {
		select {
		case m, ok := <-rc:
			if !ok {
				return
			}
			select {
			case s.ch <- []byte(m.Payload):
			case <-s.done:
				return
			}
		case <-s.done:
			return
		}
	}
}

func (s *redisSub) C() <-chan []byte { return s.ch }

func (s *redisSub) Close() {
	s.once.Do(func() {
		close(s.done)
		_ = s.ps.Close()
	})
}
