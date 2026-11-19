package components

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"

	"server/internal/identity/model"
)

var ErrUserNotFound = errors.New("user does not exist")

type SendMagicLinkUseCase struct {
	userRepo     model.UserRepository
	tokenStore   model.TokenStore
	mailer       model.Mailer
	tokenService *model.TokenService
}

func NewSendMagicLinkUseCase(userRepo model.UserRepository, tokenStore model.TokenStore, mailer model.Mailer, tokenService *model.TokenService) *SendMagicLinkUseCase {
	return &SendMagicLinkUseCase{
		userRepo:     userRepo,
		tokenStore:   tokenStore,
		mailer:       mailer,
		tokenService: tokenService,
	}
}

type SendMagicLinkCommand struct {
	Email          string
	StateID        string
	FirstName      string
	LastName       string
	IsRegistration bool
}

func (uc *SendMagicLinkUseCase) Execute(ctx context.Context, cmd SendMagicLinkCommand) error {
	if !cmd.IsRegistration {
		if _, err := uc.userRepo.GetByEmail(ctx, cmd.Email); err != nil {
			// Only a real miss means "no such user" (the portal then offers
			// sign-up). A DB outage or timeout is a server error, not a reason
			// to tell a registered user they have no account.
			if errors.Is(err, sql.ErrNoRows) {
				return ErrUserNotFound
			}
			return fmt.Errorf("failed to look up user for magic link: %w", err)
		}
	}

	rawToken, err := uc.tokenService.GenerateSecureRandomToken()
	if err != nil {
		return fmt.Errorf("failed to generate raw magic token: %w", err)
	}

	hash := sha256.Sum256([]byte(rawToken))
	tokenHash := hex.EncodeToString(hash[:])

	err = uc.tokenStore.SaveMagicToken(ctx, tokenHash, cmd.Email, cmd.StateID, cmd.FirstName, cmd.LastName, cmd.IsRegistration)
	if err != nil {
		return fmt.Errorf("failed to save magic token state: %w", err)
	}

	err = uc.mailer.SendMagicLink(ctx, cmd.Email, rawToken, cmd.StateID)
	if err != nil {
		return fmt.Errorf("failed to dispatch email magic link: %w", err)
	}

	return nil
}
