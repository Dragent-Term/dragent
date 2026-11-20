package model

import (
	"context"
	"errors"
	"sync"
	"testing"

	"github.com/alicebob/miniredis/v2"
	"github.com/redis/go-redis/v9"
)

func testStore(t *testing.T) *RedisTokenStore {
	t.Helper()
	mr := miniredis.RunT(t)
	return NewRedisTokenStore(redis.NewClient(&redis.Options{Addr: mr.Addr()}))
}

// An approved sign-in is handed out exactly once, even to racing pollers;
// an unapproved one stays parked.
func TestTakeAuthorizedStateOnce(t *testing.T) {
	s := testStore(t)
	ctx := context.Background()
	if err := s.AuthorizeState(ctx, "s1", "u1", "a", "r"); err != nil {
		t.Fatal(err)
	}
	if _, _, err := s.TakeAuthorizedState(ctx, "s1"); !errors.Is(err, ErrStateNotApproved) {
		t.Fatalf("unapproved: %v", err)
	}
	if _, _, err := s.TakeAuthorizedState(ctx, "s1"); !errors.Is(err, ErrStateNotApproved) {
		t.Fatalf("still parked: %v", err)
	}
	if err := s.ApproveState(ctx, "s1", "u1"); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	var mu sync.Mutex
	won := 0
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if a, r, err := s.TakeAuthorizedState(ctx, "s1"); err == nil && a == "a" && r == "r" {
				mu.Lock()
				won++
				mu.Unlock()
			}
		}()
	}
	wg.Wait()
	if won != 1 {
		t.Fatalf("handed out %d times, want 1", won)
	}
}

func TestChallengeFirstWins(t *testing.T) {
	s := testStore(t)
	ctx := context.Background()
	if err := s.SetSignInChallenge(ctx, "s1", "c1"); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSignInChallenge(ctx, "s1", "c1"); err != nil {
		t.Fatalf("same challenge again: %v", err)
	}
	if err := s.SetSignInChallenge(ctx, "s1", "c2"); !errors.Is(err, ErrChallengeTaken) {
		t.Fatalf("replacement: %v", err)
	}
	if c, _ := s.SignInChallenge(ctx, "s1"); c != "c1" {
		t.Fatalf("challenge %q", c)
	}
	if c, err := s.SignInChallenge(ctx, "none"); c != "" || err != nil {
		t.Fatalf("missing: %q %v", c, err)
	}
}

func TestHandoffOnce(t *testing.T) {
	s := testStore(t)
	ctx := context.Background()
	if err := s.SaveHandoff(ctx, "code", "s1", "refresh", "u1"); err != nil {
		t.Fatal(err)
	}
	st, r, u, err := s.TakeHandoff(ctx, "code")
	if err != nil || st != "s1" || r != "refresh" || u != "u1" {
		t.Fatalf("take: %q %q %q %v", st, r, u, err)
	}
	if _, _, _, err := s.TakeHandoff(ctx, "code"); err == nil {
		t.Fatal("handed out twice")
	}
}
