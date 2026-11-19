package components

import (
	"context"
	"sync"
	"time"

	"github.com/redis/go-redis/v9"
)

// Remote-command rate limit: per controller, per session.
const (
	CommandRateLimit  = 60
	CommandRateWindow = time.Hour
)

func rateKey(sessionID, participantID string) string {
	return "ratelimit:cmd:" + sessionID + ":" + participantID
}

// RateLimiter bounds how often a controller may request commands.
type RateLimiter interface {
	// Allow records one use of key and reports whether it is within the limit.
	Allow(ctx context.Context, key string) (bool, error)
}

// AllowAll never limits (single-instance dev, or tests that don't exercise it).
type AllowAll struct{}

func (AllowAll) Allow(context.Context, string) (bool, error) { return true, nil }

// ── Redis fixed-window limiter ───────────────────────────────────────────────

type RedisRateLimiter struct {
	rdb    *redis.Client
	limit  int
	window time.Duration
}

func NewRedisRateLimiter(rdb *redis.Client) *RedisRateLimiter {
	return &RedisRateLimiter{rdb: rdb, limit: CommandRateLimit, window: CommandRateWindow}
}

func (r *RedisRateLimiter) Allow(ctx context.Context, key string) (bool, error) {
	n, err := r.rdb.Incr(ctx, key).Result()
	if err != nil {
		return false, err
	}
	if n == 1 {
		// First hit in this window: start the expiry.
		_ = r.rdb.Expire(ctx, key, r.window).Err()
	}
	return n <= int64(r.limit), nil
}

// ── In-memory fixed-window limiter (tests) ───────────────────────────────────

type MemoryRateLimiter struct {
	mu     sync.Mutex
	limit  int
	window time.Duration
	counts map[string]memWindow
	now    func() time.Time
}

type memWindow struct {
	count int
	reset time.Time
}

func NewMemoryRateLimiter(limit int, window time.Duration) *MemoryRateLimiter {
	return &MemoryRateLimiter{
		limit:  limit,
		window: window,
		counts: make(map[string]memWindow),
		now:    time.Now,
	}
}

func (r *MemoryRateLimiter) Allow(_ context.Context, key string) (bool, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	now := r.now()
	w := r.counts[key]
	if now.After(w.reset) {
		w = memWindow{count: 0, reset: now.Add(r.window)}
	}
	w.count++
	r.counts[key] = w
	return w.count <= r.limit, nil
}
