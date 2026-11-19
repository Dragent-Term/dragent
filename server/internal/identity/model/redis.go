package model

import (
	"context"
	"encoding/json"
	"errors"
	"time"

	"github.com/redis/go-redis/v9"
)

type RedisTokenStore struct {
	rdb *redis.Client
}

func NewRedisTokenStore(rdb *redis.Client) *RedisTokenStore {
	return &RedisTokenStore{rdb: rdb}
}

type magicPayload struct {
	Email          string `json:"email"`
	StateID        string `json:"state_id"`
	FirstName      string `json:"first_name"`
	LastName       string `json:"last_name"`
	IsRegistration bool   `json:"is_registration"`
}

type authPayload struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
}

func (s *RedisTokenStore) SaveMagicToken(ctx context.Context, tokenHash string, email string, stateID string, firstName string, lastName string, isRegistration bool) error {
	payload := magicPayload{
		Email:          email,
		StateID:        stateID,
		FirstName:      firstName,
		LastName:       lastName,
		IsRegistration: isRegistration,
	}
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}

	key := "magic:" + tokenHash
	return s.rdb.Set(ctx, key, data, 15*time.Minute).Err()
}

// GetMagicToken consumes the link: read and deleted in one step, so two
// redemptions racing for the same link can't both sign in.
func (s *RedisTokenStore) GetMagicToken(ctx context.Context, tokenHash string) (string, string, string, string, bool, error) {
	key := "magic:" + tokenHash
	val, err := s.rdb.GetDel(ctx, key).Result()
	if err != nil {
		if errors.Is(err, redis.Nil) {
			return "", "", "", "", false, errors.New("magic token not found or expired")
		}
		return "", "", "", "", false, err
	}

	var payload magicPayload
	if err := json.Unmarshal([]byte(val), &payload); err != nil {
		return "", "", "", "", false, err
	}

	return payload.Email, payload.StateID, payload.FirstName, payload.LastName, payload.IsRegistration, nil
}

func (s *RedisTokenStore) DeleteMagicToken(ctx context.Context, tokenHash string) error {
	key := "magic:" + tokenHash
	return s.rdb.Del(ctx, key).Err()
}

func (s *RedisTokenStore) AuthorizeState(ctx context.Context, stateID string, accessToken string, refreshToken string) error {
	payload := authPayload{
		AccessToken:  accessToken,
		RefreshToken: refreshToken,
	}
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}

	key := "poll:" + stateID
	return s.rdb.Set(ctx, key, data, 10*time.Minute).Err()
}

func (s *RedisTokenStore) GetAuthorizedState(ctx context.Context, stateID string) (string, string, error) {
	key := "poll:" + stateID
	val, err := s.rdb.Get(ctx, key).Result()
	if err != nil {
		if errors.Is(err, redis.Nil) {
			return "", "", errors.New("authorization state not ready or expired")
		}
		return "", "", err
	}

	var payload authPayload
	if err := json.Unmarshal([]byte(val), &payload); err != nil {
		return "", "", err
	}

	return payload.AccessToken, payload.RefreshToken, nil
}

var _ TokenStore = (*RedisTokenStore)(nil)
