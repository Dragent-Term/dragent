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
	UserID       string `json:"user_id"`
	Approved     bool   `json:"approved"`
}

// ErrStateNotApproved is a parked sign-in nobody has approved yet.
var ErrStateNotApproved = errors.New("sign-in not approved yet")

// ErrStateWrongUser is an approval from someone other than the user who
// signed in.
var ErrStateWrongUser = errors.New("sign-in belongs to another user")

const (
	pollTTL = 10 * time.Minute
	// Once approved, it only needs to last until the desktop's next poll.
	approvedPollTTL = 2 * time.Minute
	// Longer than an access token lives (24h), so a signed-out device's
	// tokens have all expired by the time the mark goes.
	signedOutTTL = 25 * time.Hour
)

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

func (s *RedisTokenStore) AuthorizeState(ctx context.Context, stateID string, userID string, accessToken string, refreshToken string) error {
	data, err := json.Marshal(authPayload{
		AccessToken:  accessToken,
		RefreshToken: refreshToken,
		UserID:       userID,
	})
	if err != nil {
		return err
	}
	return s.rdb.Set(ctx, "poll:"+stateID, data, pollTTL).Err()
}

func (s *RedisTokenStore) readState(ctx context.Context, stateID string) (*authPayload, error) {
	val, err := s.rdb.Get(ctx, "poll:"+stateID).Result()
	if err != nil {
		if errors.Is(err, redis.Nil) {
			return nil, errors.New("authorization state not ready or expired")
		}
		return nil, err
	}
	var payload authPayload
	if err := json.Unmarshal([]byte(val), &payload); err != nil {
		return nil, err
	}
	return &payload, nil
}

func (s *RedisTokenStore) ApproveState(ctx context.Context, stateID string, userID string) error {
	payload, err := s.readState(ctx, stateID)
	if err != nil {
		return err
	}
	if payload.UserID == "" || payload.UserID != userID {
		return ErrStateWrongUser
	}
	payload.Approved = true
	data, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	return s.rdb.Set(ctx, "poll:"+stateID, data, approvedPollTTL).Err()
}

func (s *RedisTokenStore) GetAuthorizedState(ctx context.Context, stateID string) (string, string, error) {
	payload, err := s.readState(ctx, stateID)
	if err != nil {
		return "", "", err
	}
	if !payload.Approved {
		return "", "", ErrStateNotApproved
	}
	return payload.AccessToken, payload.RefreshToken, nil
}

func (s *RedisTokenStore) SignOutDevice(ctx context.Context, deviceID string) error {
	return s.rdb.Set(ctx, "signedout:"+deviceID, "1", signedOutTTL).Err()
}

func (s *RedisTokenStore) IsDeviceSignedOut(ctx context.Context, deviceID string) (bool, error) {
	n, err := s.rdb.Exists(ctx, "signedout:"+deviceID).Result()
	if err != nil {
		return false, err
	}
	return n > 0, nil
}

var _ TokenStore = (*RedisTokenStore)(nil)
