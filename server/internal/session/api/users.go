package api

import (
	"context"
	"strings"

	"github.com/google/uuid"

	identityModel "server/internal/identity/model"
)

// IdentityUsers is the subset of the identity user repo needed to enrich
// session participants with a real display name + avatar.
type IdentityUsers interface {
	GetByID(ctx context.Context, id uuid.UUID) (*identityModel.User, error)
	GetByEmail(ctx context.Context, email string) (*identityModel.User, error)
}

// IdentityLookup adapts the identity user repository to [UserLookup].
type IdentityLookup struct {
	Users IdentityUsers
}

func (l IdentityLookup) SessionProfile(ctx context.Context, userID string) (displayName, avatarURL string, ok bool) {
	if l.Users == nil {
		return "", "", false
	}
	id, err := uuid.Parse(userID)
	if err != nil {
		return "", "", false
	}
	u, err := l.Users.GetByID(ctx, id)
	if err != nil || u == nil {
		return "", "", false
	}
	name := sessionDisplayName(u)
	return name, u.AvatarURL, true
}

func (l IdentityLookup) SessionProfileByEmail(ctx context.Context, email string) (displayName, avatarURL string, ok bool) {
	if l.Users == nil {
		return "", "", false
	}
	email = strings.TrimSpace(email)
	if email == "" {
		return "", "", false
	}
	u, err := l.Users.GetByEmail(ctx, strings.ToLower(email))
	if err != nil && strings.ToLower(email) != email {
		u, err = l.Users.GetByEmail(ctx, email)
	}
	if err != nil || u == nil {
		return "", "", false
	}
	name := sessionDisplayName(u)
	return name, u.AvatarURL, true
}

func sessionDisplayName(u *identityModel.User) string {
	name := strings.TrimSpace(u.DisplayName)
	if name == "" {
		name = strings.TrimSpace(u.FirstName + " " + u.LastName)
	}
	if name == "" {
		name = u.Email
	}
	return name
}
