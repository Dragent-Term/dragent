package components

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"time"

	"github.com/google/uuid"
	"server/internal/identity/model"
)

var (
	ErrInvalidToken = errors.New("invalid or expired refresh token")
	ErrTokenRevoked = errors.New("refresh token has been revoked previously")
)

type RefreshTokenUseCase struct {
	userRepo     model.UserRepository
	tokenService *model.TokenService
	txManager    model.TxManager
}

func NewRefreshTokenUseCase(
	userRepo model.UserRepository,
	tokenService *model.TokenService,
	txManager model.TxManager,
) *RefreshTokenUseCase {
	return &RefreshTokenUseCase{
		userRepo:     userRepo,
		tokenService: tokenService,
		txManager:    txManager,
	}
}

type RefreshTokenCommand struct {
	RefreshToken string
}

type RefreshTokenResult struct {
	AccessToken  string
	RefreshToken string
	UserID       uuid.UUID
}

func (uc *RefreshTokenUseCase) Execute(ctx context.Context, cmd RefreshTokenCommand) (*RefreshTokenResult, error) {
	hashBytes := sha256.Sum256([]byte(cmd.RefreshToken))
	incomingHash := hex.EncodeToString(hashBytes[:])

	token, err := uc.userRepo.GetRefreshTokenByHash(ctx, incomingHash)
	if err != nil {
		return nil, ErrInvalidToken
	}

	if token.IsRevoked() {
		_ = uc.userRepo.RevokeTokenFamily(ctx, token.FamilyID)
		return nil, ErrTokenRevoked
	}

	if token.IsExpired() {
		return nil, ErrInvalidToken
	}

	var newAccessToken string
	var newRawRefreshToken string

	err = uc.txManager.WithTx(ctx, func(tx any) error {
		txRepo := uc.userRepo.WithTx(tx)

		if err := txRepo.RevokeToken(ctx, token.ID); err != nil {
			return err
		}

		var genErr error
		newRawRefreshToken, genErr = uc.tokenService.GenerateSecureRandomToken()
		if genErr != nil {
			return genErr
		}

		newHashBytes := sha256.Sum256([]byte(newRawRefreshToken))
		newHash := hex.EncodeToString(newHashBytes[:])

		newRefreshToken := &model.RefreshToken{
			ID:        uuid.New(),
			UserID:    token.UserID,
			DeviceID:  token.DeviceID,
			TokenHash: newHash,
			FamilyID:  token.FamilyID,
			ExpiresAt: time.Now().Add(30 * 24 * time.Hour),
			CreatedAt: time.Now(),
		}

		if err := txRepo.SaveRefreshToken(ctx, newRefreshToken); err != nil {
			return err
		}

		var tokenErr error
		newAccessToken, tokenErr = uc.tokenService.GenerateAccessToken(token.UserID, token.DeviceID)
		return tokenErr
	})

	if err != nil {
		return nil, err
	}

	return &RefreshTokenResult{
		AccessToken:  newAccessToken,
		RefreshToken: newRawRefreshToken,
		UserID:       token.UserID,
	}, nil
}
