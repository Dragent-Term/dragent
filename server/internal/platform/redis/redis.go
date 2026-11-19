package redis

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/redis/go-redis/v9"
)

type Client struct {
	Rdb *redis.Client
}

// Connect accepts EITHER a full Redis URL or a bare host:port.
//
//   - "rediss://:<token>@host:6379"  → TLS + auth (managed providers: Upstash,
//     ElastiCache-with-auth). ParseURL wires TLS and the password for us.
//   - "redis://host:6379"            → plaintext URL form.
//   - "host:6379"                    → bare address (local dev, no TLS/auth).
//
// This keeps local dev a plaintext localhost while production points at a
// managed, TLS-secured, authenticated endpoint — a config change, not a code
// change, when we swap providers.
func Connect(ctx context.Context, redisAddr string) (*Client, error) {
	var opts *redis.Options
	if strings.HasPrefix(redisAddr, "redis://") || strings.HasPrefix(redisAddr, "rediss://") {
		parsed, err := redis.ParseURL(redisAddr)
		if err != nil {
			return nil, fmt.Errorf("invalid REDIS_ADDR url: %w", err)
		}
		opts = parsed
	} else {
		opts = &redis.Options{Addr: redisAddr}
	}
	// Apply our timeouts/pool regardless of source.
	opts.DialTimeout = 5 * time.Second
	opts.ReadTimeout = 3 * time.Second
	opts.WriteTimeout = 3 * time.Second
	if opts.PoolSize == 0 {
		opts.PoolSize = 50 // concurrency-safe default
	}

	rdb := redis.NewClient(opts)
	if err := rdb.Ping(ctx).Err(); err != nil {
		return nil, fmt.Errorf("failed to ping redis server: %w", err)
	}

	slog.Info("connected to redis server successfully")
	return &Client{Rdb: rdb}, nil
}

func (c *Client) Close() error {
	if c.Rdb != nil {
		slog.Info("closing redis connection pool")
		return c.Rdb.Close()
	}
	return nil
}
