package ratelimit

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"
)

func TestMemoryLimiterWindow(t *testing.T) {
	now := time.Unix(1_000, 0)
	l := NewMemoryRateLimiter(2, time.Minute, func() time.Time { return now })
	ctx := context.Background()
	for i := 0; i < 2; i++ {
		if ok, _, _ := l.Allow(ctx, "k"); !ok {
			t.Fatalf("use %d refused", i+1)
		}
	}
	ok, wait, _ := l.Allow(ctx, "k")
	if ok || wait != time.Minute {
		t.Fatalf("third use: ok=%v wait=%v, want refused with 1m", ok, wait)
	}
	if ok, _, _ := l.Allow(ctx, "other"); !ok {
		t.Fatal("keys must be counted separately")
	}
	now = now.Add(time.Minute)
	if ok, _, _ := l.Allow(ctx, "k"); !ok {
		t.Fatal("new window should allow again")
	}
}

func TestCheckWrites429WithRetryAfter(t *testing.T) {
	l := NewMemoryRateLimiter(0, 90*time.Second, nil)
	w := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodPost, "/", nil)
	if Check(w, r, Rule{Limiter: l, Key: "k"}) {
		t.Fatal("Check allowed a request over the limit")
	}
	if w.Code != http.StatusTooManyRequests {
		t.Fatalf("status = %d, want 429", w.Code)
	}
	if got := w.Header().Get("Retry-After"); got != "90" {
		t.Fatalf("Retry-After = %q, want 90", got)
	}
}

func TestCheckFailsClosedWithoutStore(t *testing.T) {
	w := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodPost, "/", nil)
	if Check(w, r, Rule{Limiter: NewRedisRateLimiter(nil, 5, time.Minute), Key: "k"}) {
		t.Fatal("Check allowed a request with no store")
	}
	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("status = %d, want 503", w.Code)
	}
}

// Opt-in against a dedicated test Redis, like companionpairing's tests.
func TestRedisLimiterIntegration(t *testing.T) {
	addr := os.Getenv("DRAGENT_TEST_REDIS_ADDR")
	if addr == "" {
		t.Skip("set DRAGENT_TEST_REDIS_ADDR to run real Redis/Lua integration")
	}
	client := redis.NewClient(&redis.Options{Addr: addr, MaxRetries: -1, DialTimeout: time.Second})
	defer client.Close()
	ctx := context.Background()
	if err := client.Ping(ctx).Err(); err != nil {
		t.Skip("dedicated test Redis is unavailable")
	}
	key := "test-ratelimit-" + uuid.NewString()
	defer client.Del(ctx, key)
	l := NewRedisRateLimiter(client, 2, time.Minute)
	for i := 0; i < 2; i++ {
		if ok, _, err := l.Allow(ctx, key); err != nil || !ok {
			t.Fatalf("use %d: ok=%v err=%v", i+1, ok, err)
		}
	}
	ok, wait, err := l.Allow(ctx, key)
	if err != nil || ok || wait <= 0 || wait > time.Minute {
		t.Fatalf("third use: ok=%v wait=%v err=%v", ok, wait, err)
	}
	if ttl := client.PTTL(ctx, key).Val(); ttl <= 0 {
		t.Fatalf("key has no expiry: %v", ttl)
	}
}
