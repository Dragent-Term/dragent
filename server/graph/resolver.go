package graph

import (
	"context"

	"server/internal/identity/components"
	"server/internal/identity/model"
	selfhealComponents "server/internal/selfheal/components"
)

type Resolver struct {
	UserRepo               model.UserRepository
	TokenStore             model.TokenStore
	TokenService           *model.TokenService
	VerifyMagicLinkUseCase *components.VerifyMagicLinkUseCase
	RefreshTokenUseCase    *components.RefreshTokenUseCase
	SelfHealUseCase        *selfhealComponents.SelfHealUseCase
}

// stringFromContext reads a string the auth middleware put in the context,
// or "" when it isn't there.
func stringFromContext(ctx context.Context, key string) string {
	v, _ := ctx.Value(key).(string)
	return v
}
