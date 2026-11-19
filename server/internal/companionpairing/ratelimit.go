package companionpairing

import (
	"context"
	"sync"
	"time"

	"github.com/redis/go-redis/v9"
)

// Redeem rate limits (fixed windows). Defaults bound code-guessing and
// IP-wide abuse without tripping a phone that mistypes once or twice.
const (
	defaultRedeemIPLimit   = 20
	defaultRedeemCodeLimit = 10
	defaultRedeemWindow    = time.Minute
)

// RateLimiter bounds how often a key may act. Memory and Redis impls share
// fixed-window semantics: the count resets every window.
type RateLimiter interface {
	Allow(ctx context.Context, key string) (bool, error)
}

// MemoryRateLimiter is the single-instance/test limiter with an injectable
// clock for deterministic tests.
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

// NewMemoryRateLimiter returns a limiter allowing limit uses per window.
func NewMemoryRateLimiter(limit int, window time.Duration, now ...func() time.Time) *MemoryRateLimiter {
	return &MemoryRateLimiter{limit: limit, window: window, counts: make(map[string]memWindow), now: storeClock(now)}
}

func (r *MemoryRateLimiter) Allow(_ context.Context, key string) (bool, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	now := r.now()
	w := r.counts[key]
	if !now.Before(w.reset) {
		w = memWindow{reset: now.Add(r.window)}
	}
	w.count++
	r.counts[key] = w
	return w.count <= r.limit, nil
}

// RedisRateLimiter is the multi-instance limiter: INCR per key, first hit
// starts the window expiry.
type RedisRateLimiter struct {
	client *redis.Client
	limit  int
	window time.Duration
}

// NewRedisRateLimiter returns a limiter allowing limit uses per window.
func NewRedisRateLimiter(client *redis.Client, limit int, window time.Duration) *RedisRateLimiter {
	return &RedisRateLimiter{client: client, limit: limit, window: window}
}

func (r *RedisRateLimiter) Allow(ctx context.Context, key string) (bool, error) {
	if err := ctx.Err(); err != nil {
		return false, err
	}
	if r.client == nil {
		return false, ErrStorage
	}
	n, err := r.client.Incr(ctx, key).Result()
	if err != nil {
		return false, ErrStorage
	}
	if n == 1 {
		_ = r.client.Expire(ctx, key, r.window).Err()
	}
	return n <= int64(r.limit), nil
}
