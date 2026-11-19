package graph

import (
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
