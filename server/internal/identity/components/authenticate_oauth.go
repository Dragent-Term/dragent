package components

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/google/uuid"
	"server/internal/identity/model"
)

// BalanceInitializer creates a user's billing row. Declared here as a one-method
// interface, rather than importing the billing package, so identity does not
// take a dependency on billing internals — the concrete PostgresBillingRepository
// already satisfies it.
//
// Registration must create this row. It is also created lazily on the first
// billing operation, which is why the gap went unnoticed while everyone signed
// up through the desktop app and immediately ran a query. A web signup reaches
// the dashboard first, and /api/v1/account then failed with
// "fetch balance: sql: no rows in result set" and rendered no profile at all.
//
// Takes organizationID: the balance row is now keyed (user_id,
// organization_id), not user_id alone — the caller must
// resolve OrganizationInitializer's result FIRST and pass it here.
type BalanceInitializer interface {
	InitializeTx(ctx context.Context, tx any, userID, organizationID uuid.UUID) error
}

type AuthenticateOAuthUseCase struct {
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
func (uc *AuthenticateOAuthUseCase) SetBalanceInitializer(b BalanceInitializer) {
	uc.balanceInit = b
}

// SetOrganizationInitializer wires personal-organization creation into
// registration. A setter, not a constructor arg, for the same reason as
// SetBalanceInitializer.
func (uc *AuthenticateOAuthUseCase) SetOrganizationInitializer(o OrganizationInitializer) {
	uc.orgInit = o
}

// SetWelcomeMailer wires the post-registration welcome email. A setter, not
// a constructor arg, for the same reason as SetBalanceInitializer.
func (uc *AuthenticateOAuthUseCase) SetWelcomeMailer(w WelcomeMailer) {
	uc.welcomeMailer = w
}

func NewAuthenticateOAuthUseCase(
	userRepo model.UserRepository,
	tokenStore model.TokenStore,
	tokenService *model.TokenService,
	txManager model.TxManager,
) *AuthenticateOAuthUseCase {
	return &AuthenticateOAuthUseCase{
		userRepo:     userRepo,
		tokenStore:   tokenStore,
		tokenService: tokenService,
		txManager:    txManager,
	}
}

type AuthenticateOAuthCommand struct {
	Provider     string
	Code         string
	StateID      string
	OAuthService model.OAuthProvider
}

type AuthenticateOAuthResult struct {
	UserID uuid.UUID
	// AccessToken is the browser's session; RefreshToken is the app's,
	// separate so signing out of one leaves the other.
	AccessToken  string
	RefreshToken string
	StateID      string
}

func (uc *AuthenticateOAuthUseCase) Execute(ctx context.Context, cmd AuthenticateOAuthCommand) (*AuthenticateOAuthResult, error) {
	token, err := cmd.OAuthService.ExchangeCode(ctx, cmd.Code)
	if err != nil {
		return nil, fmt.Errorf("oauth code exchange failed: %w", err)
	}

	info, err := cmd.OAuthService.FetchUserInfo(ctx, token)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch oauth user info: %w", err)
	}

	var user *model.User
	var sessions signInSessions
	var isNewUser bool

	err = uc.txManager.WithTx(ctx, func(tx any) error {
		txRepo := uc.userRepo.WithTx(tx)
		var txErr error

		user, txErr = txRepo.GetByEmail(ctx, info.Email)
		if txErr != nil {
			isNewUser = true
			now := time.Now()
			user, txErr = model.NewUser(info.Email, info.FirstName, info.LastName)
			if txErr != nil {
				return txErr
			}
			user.AvatarURL = info.AvatarURL
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
		} else if backfillProfile(user, info) {
			if err := txRepo.UpdateProfile(ctx, user); err != nil {
				return fmt.Errorf("failed to update user profile: %w", err)
			}
		}

		_, txErr = txRepo.GetOAuthAccount(ctx, cmd.Provider, info.ProviderUserID)
		if txErr != nil {
			account := &model.OAuthAccount{
				ID:             uuid.New(),
				UserID:         user.ID,
				Provider:       cmd.Provider,
				ProviderUserID: info.ProviderUserID,
				CreatedAt:      time.Now(),
			}

			if err := txRepo.LinkOAuthAccount(ctx, account); err != nil {
				return fmt.Errorf("failed to link oauth account: %w", err)
			}
		}

		sessions, txErr = issueSignInSessions(ctx, txRepo, uc.tokenService, user.ID, fmt.Sprintf("%s OAuth Session", cmd.Provider))
		if txErr != nil {
			return txErr
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

	accessToken, err := uc.tokenService.GenerateAccessToken(user.ID, sessions.browserDevice)
	if err != nil {
		return nil, fmt.Errorf("failed to generate access token: %w", err)
	}
	desktopAccess, err := uc.tokenService.GenerateAccessToken(user.ID, sessions.desktopDevice)
	if err != nil {
		return nil, fmt.Errorf("failed to generate access token: %w", err)
	}

	err = uc.tokenStore.AuthorizeState(ctx, cmd.StateID, user.ID.String(), desktopAccess, sessions.desktopRefresh)
	if err != nil {
		return nil, fmt.Errorf("failed to store authorized session tokens in cache: %w", err)
	}

	return &AuthenticateOAuthResult{
		UserID:       user.ID,
		AccessToken:  accessToken,
		RefreshToken: sessions.desktopRefresh,
		StateID:      cmd.StateID,
	}, nil
}

// backfillProfile fills profile fields an existing user is missing from the
// provider's data (magic-link signups have typed names but no photo; users
// created before names/avatars were captured have neither). Provider data
// never overwrites what a user already has. Reports whether anything changed.
func backfillProfile(user *model.User, info *model.OAuthUserInfo) bool {
	changed := false
	if user.FirstName == "" && user.LastName == "" && (info.FirstName != "" || info.LastName != "") {
		user.FirstName = info.FirstName
		user.LastName = info.LastName
		changed = true
	}
	if user.DisplayName == "" {
		if displayName := strings.TrimSpace(user.FirstName + " " + user.LastName); displayName != "" {
			user.DisplayName = displayName
			changed = true
		}
	}
	if user.AvatarURL == "" && info.AvatarURL != "" {
		user.AvatarURL = info.AvatarURL
		changed = true
	}
	return changed
}
