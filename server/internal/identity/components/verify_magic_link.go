package components

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"log/slog"
	"time"

	"github.com/google/uuid"
	"server/internal/identity/model"
)

type VerifyMagicLinkUseCase struct {
	userRepo     model.UserRepository
	tokenStore   model.TokenStore
	tokenService *model.TokenService
	txManager    model.TxManager
	// Optional: nil simply skips initialisation (the lazy path still covers it).
	balanceInit   BalanceInitializer
	orgInit       OrganizationInitializer
	welcomeMailer WelcomeMailer
}

// SetBalanceInitializer wires balance creation into registration. A setter, not
// a constructor arg, so existing call sites and tests keep compiling.
func (uc *VerifyMagicLinkUseCase) SetBalanceInitializer(b BalanceInitializer) {
	uc.balanceInit = b
}

// SetOrganizationInitializer wires personal-organization creation into
// registration. A setter, not a constructor arg, for the same reason as
// SetBalanceInitializer.
func (uc *VerifyMagicLinkUseCase) SetOrganizationInitializer(o OrganizationInitializer) {
	uc.orgInit = o
}

// SetWelcomeMailer wires the post-registration welcome email. A setter, not
// a constructor arg, for the same reason as SetBalanceInitializer.
func (uc *VerifyMagicLinkUseCase) SetWelcomeMailer(w WelcomeMailer) {
	uc.welcomeMailer = w
}

func NewVerifyMagicLinkUseCase(
	userRepo model.UserRepository,
	tokenStore model.TokenStore,
	tokenService *model.TokenService,
	txManager model.TxManager,
) *VerifyMagicLinkUseCase {
	return &VerifyMagicLinkUseCase{
		userRepo:     userRepo,
		tokenStore:   tokenStore,
		tokenService: tokenService,
		txManager:    txManager,
	}
}

type VerifyMagicLinkResult struct {
	UserID       uuid.UUID
	AccessToken  string
	RefreshToken string
	StateID      string
}

func (uc *VerifyMagicLinkUseCase) Execute(ctx context.Context, rawToken string) (*VerifyMagicLinkResult, error) {
	hash := sha256.Sum256([]byte(rawToken))
	tokenHash := hex.EncodeToString(hash[:])

	email, stateID, firstName, lastName, isRegistration, err := uc.tokenStore.GetMagicToken(ctx, tokenHash)
	if err != nil {
		return nil, fmt.Errorf("invalid or expired magic token: %w", err)
	}

	_ = uc.tokenStore.DeleteMagicToken(ctx, tokenHash)

	var user *model.User
	var deviceID uuid.UUID = uuid.New()
	var rawRefreshToken string
	var isNewUser bool

	err = uc.txManager.WithTx(ctx, func(tx any) error {
		txRepo := uc.userRepo.WithTx(tx)
		var txErr error
		user, txErr = txRepo.GetByEmail(ctx, email)
		if txErr != nil {
			if !isRegistration {
				return ErrUserNotFound
			}
			isNewUser = true
			now := time.Now()
			user, txErr = model.NewUser(email, firstName, lastName)
			if txErr != nil {
				return txErr
			}
			user.EmailVerifiedAt = &now

			if err := txRepo.Create(ctx, user); err != nil {
				return fmt.Errorf("failed to register user: %w", err)
			}

			// Same transaction as the user row: every account gets a personal
			// organization, not opt-in (see OrganizationInitializer). Must run
			// BEFORE balance init: the balance row is keyed (user_id,
			// organization_id), so the org has to exist first.
			var orgID uuid.UUID
			if uc.orgInit != nil {
				var orgErr error
				orgID, orgErr = uc.orgInit.InitializeTx(ctx, tx, user.ID, user.DisplayName, user.Email)
				if orgErr != nil {
					return fmt.Errorf("failed to initialise personal organization: %w", orgErr)
				}
			}

			// Same transaction as the user row: a user without a balance is a
			// user whose dashboard 500s. Skipped if orgInit is unset — there is
			// no org to key the balance row to; the lazy GetOrInitBalance path
			// still covers it once one exists.
			if uc.balanceInit != nil && orgID != uuid.Nil {
				if err := uc.balanceInit.InitializeTx(ctx, tx, user.ID, orgID); err != nil {
					return fmt.Errorf("failed to initialise balance: %w", err)
				}
			}
		}

		device := &model.Device{
			ID:         deviceID,
			UserID:     user.ID,
			Name:       "Web Browser Session",
			Platform:   "WEB",
			LastSeenAt: time.Now(),
			CreatedAt:  time.Now(),
		}
		if err := txRepo.RegisterDevice(ctx, device); err != nil {
			return fmt.Errorf("failed to register device: %w", err)
		}

		rawRefreshToken, txErr = uc.tokenService.GenerateSecureRandomToken()
		if txErr != nil {
			return txErr
		}

		rtHashBytes := sha256.Sum256([]byte(rawRefreshToken))
		rtHash := hex.EncodeToString(rtHashBytes[:])

		refreshToken := &model.RefreshToken{
			ID:        uuid.New(),
			UserID:    user.ID,
			DeviceID:  deviceID,
			TokenHash: rtHash,
			FamilyID:  uuid.New(),
			ExpiresAt: time.Now().Add(30 * 24 * time.Hour),
			CreatedAt: time.Now(),
		}

		if err := txRepo.SaveRefreshToken(ctx, refreshToken); err != nil {
			return fmt.Errorf("failed to store refresh token: %w", err)
		}

		return nil
	})

	if err != nil {
		return nil, err
	}

	// Sent after commit, never inside the transaction — a signup that already
	// succeeded must not be delayed or rolled back by a slow/failing email.
	if isNewUser && uc.welcomeMailer != nil {
		if err := uc.welcomeMailer.SendWelcome(ctx, user.Email, user.DisplayName); err != nil {
			slog.Error("failed to send welcome email", "user_id", user.ID, "error", err)
		}
	}

	accessToken, err := uc.tokenService.GenerateAccessToken(user.ID, deviceID)
	if err != nil {
		return nil, fmt.Errorf("failed to generate access token: %w", err)
	}

	err = uc.tokenStore.AuthorizeState(ctx, stateID, accessToken, rawRefreshToken)
	if err != nil {
		return nil, fmt.Errorf("failed to store authorized session tokens in cache: %w", err)
	}

	return &VerifyMagicLinkResult{
		UserID:       user.ID,
		AccessToken:  accessToken,
		RefreshToken: rawRefreshToken,
		StateID:      stateID,
	}, nil
}
