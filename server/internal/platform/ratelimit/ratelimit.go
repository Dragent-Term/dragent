// Package ratelimit holds the fixed-window rate limiters shared by HTTP
// handlers. The Redis limiter is the production one (counts are shared by
// every API instance); the memory limiter is for tests and single-process
// tools. Both follow companionpairing's limiter: a counter per key that
// resets when its window ends.
package ratelimit

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/redis/go-redis/v9"
)

// ErrStorage means the limiter could not reach its store. Callers fail
// closed (503) on it: an abuse limit that disappears with Redis is no limit.
var ErrStorage = errors.New("rate limiter storage unavailable")

// Limiter allows at most a fixed number of uses per key per window.
// Allow counts one use and reports whether it is within the limit; when it
// is not, retryAfter is how long until the window resets.
type Limiter interface {
	Allow(ctx context.Context, key string) (allowed bool, retryAfter time.Duration, err error)
}

// Redis is a Redis-backed fixed-window limiter.
type Redis struct {
	client *redis.Client
	limit  int
	window time.Duration
}

// NewRedisRateLimiter returns a limiter allowing limit uses per window.
func NewRedisRateLimiter(client *redis.Client, limit int, window time.Duration) *Redis {
	return &Redis{client: client, limit: limit, window: window}
}

// incrScript increments the key and starts its expiry on the first use in
// the window, in one round trip and atomically: a separate INCR then EXPIRE
// leaves a key without a TTL (a permanent lockout) if the EXPIRE is lost.
// The PTTL < 0 branch also repairs such a key left by an older writer.
var incrScript = redis.NewScript(`
local n = redis.call('INCR', KEYS[1])
local ttl = redis.call('PTTL', KEYS[1])
if ttl < 0 then
  redis.call('PEXPIRE', KEYS[1], ARGV[1])
  ttl = tonumber(ARGV[1])
end
return {n, ttl}
`)

func (l *Redis) Allow(ctx context.Context, key string) (bool, time.Duration, error) {
	if l == nil || l.client == nil {
		return false, 0, ErrStorage
	}
	res, err := incrScript.Run(ctx, l.client, []string{key}, l.window.Milliseconds()).Int64Slice()
	if err != nil || len(res) != 2 {
		return false, 0, fmt.Errorf("%w: %v", ErrStorage, err)
	}
	if res[0] <= int64(l.limit) {
		return true, 0, nil
	}
	return false, time.Duration(res[1]) * time.Millisecond, nil
}

// Memory is an in-process fixed-window limiter with an injectable clock.
type Memory struct {
	mu     sync.Mutex
	limit  int
	window time.Duration
	now    func() time.Time
	counts map[string]memWindow
}

type memWindow struct {
	count int
	reset time.Time
}

// NewMemoryRateLimiter returns an in-process limiter; now defaults to
// time.Now.
func NewMemoryRateLimiter(limit int, window time.Duration, now func() time.Time) *Memory {
	if now == nil {
		now = time.Now
	}
	return &Memory{limit: limit, window: window, now: now, counts: map[string]memWindow{}}
}

func (l *Memory) Allow(_ context.Context, key string) (bool, time.Duration, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.now()
	w := l.counts[key]
	if !now.Before(w.reset) {
		w = memWindow{reset: now.Add(l.window)}
	}
	w.count++
	l.counts[key] = w
	if w.count <= l.limit {
		return true, 0, nil
	}
	return false, w.reset.Sub(now), nil
}

// Check runs the limiters in order against their keys and writes the
// refusal itself: 429 with Retry-After (whole seconds, at least 1) when a
// limit is hit, 503 when the store is down. It returns false when it wrote
// a response. Nil limiters are skipped.
func Check(w http.ResponseWriter, r *http.Request, checks ...Rule) bool {
	for _, c := range checks {
		if c.Limiter == nil {
			continue
		}
		allowed, wait, err := c.Limiter.Allow(r.Context(), c.Key)
		if err != nil {
			http.Error(w, "rate limiter unavailable", http.StatusServiceUnavailable)
			return false
		}
		if !allowed {
			WriteTooMany(w, wait, c.Message)
			return false
		}
	}
	return true
}

// Rule is one limiter applied to one key.
type Rule struct {
	Limiter Limiter
	Key     string
	// Message is the 429 body; empty means "too many requests".
	Message string
}

// WriteTooMany writes a JSON 429 with a Retry-After header.
func WriteTooMany(w http.ResponseWriter, wait time.Duration, message string) {
	seconds := int((wait + time.Second - 1) / time.Second)
	if seconds < 1 {
		seconds = 1
	}
	if message == "" {
		message = "too many requests"
	}
	w.Header().Set("Retry-After", strconv.Itoa(seconds))
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusTooManyRequests)
	_, _ = fmt.Fprintf(w, "{\"error\":%q}\n", message)
}
