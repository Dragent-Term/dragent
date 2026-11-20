package model

import (
	"context"
	"errors"
	"net/mail"
	"strings"
	"time"

	"github.com/google/uuid"
)

var ErrInvalidEmail = errors.New("invalid email address")

type User struct {
	ID              uuid.UUID
	Email           string
	FirstName       string
	LastName        string
	DisplayName     string
	AvatarURL       string
	EmailVerifiedAt *time.Time
	CreatedAt       time.Time
	UpdatedAt       time.Time
}

func NewUser(email string, firstName string, lastName string) (*User, error) {
	if _, err := mail.ParseAddress(email); err != nil {
		return nil, ErrInvalidEmail
	}

	displayName := strings.TrimSpace(firstName + " " + lastName)

	return &User{
		ID:          uuid.New(),
		Email:       email,
		FirstName:   firstName,
		LastName:    lastName,
		DisplayName: displayName,
		CreatedAt:   time.Now(),
		UpdatedAt:   time.Now(),
	}, nil
}

type OAuthAccount struct {
	ID             uuid.UUID
	UserID         uuid.UUID
	Provider       string // 'github', 'google'
	ProviderUserID string
	CreatedAt      time.Time
}

type Device struct {
	ID         uuid.UUID
	UserID     uuid.UUID
	Name       string
	Platform   string
	LastSeenAt time.Time
	CreatedAt  time.Time
}

type RefreshToken struct {
	ID        uuid.UUID
	UserID    uuid.UUID
	DeviceID  uuid.UUID
	TokenHash string
	FamilyID  uuid.UUID
	RevokedAt *time.Time
	ExpiresAt time.Time
	CreatedAt time.Time
}

func (rt *RefreshToken) IsExpired() bool {
	return time.Now().After(rt.ExpiresAt)
}

func (rt *RefreshToken) IsRevoked() bool {
	return rt.RevokedAt != nil
}

// Port Interfaces
type UserRepository interface {
	Create(ctx context.Context, user *User) error
	GetByID(ctx context.Context, id uuid.UUID) (*User, error)
	GetByEmail(ctx context.Context, email string) (*User, error)
	UpdateProfile(ctx context.Context, user *User) error

	LinkOAuthAccount(ctx context.Context, account *OAuthAccount) error
	GetOAuthAccount(ctx context.Context, provider string, providerUserID string) (*OAuthAccount, error)

	RegisterDevice(ctx context.Context, device *Device) error

	SaveRefreshToken(ctx context.Context, token *RefreshToken) error
	GetRefreshTokenByHash(ctx context.Context, hash string) (*RefreshToken, error)
	RevokeToken(ctx context.Context, id uuid.UUID) error
	RevokeTokenFamily(ctx context.Context, familyID uuid.UUID) error
	// RevokeDeviceTokens revokes every refresh token of one signed-in device.
	RevokeDeviceTokens(ctx context.Context, userID uuid.UUID, deviceID uuid.UUID) error

	WithTx(tx any) UserRepository
}

type TokenStore interface {
	SaveMagicToken(ctx context.Context, tokenHash string, email string, stateID string, firstName string, lastName string, isRegistration bool) error
	GetMagicToken(ctx context.Context, tokenHash string) (email string, stateID string, firstName string, lastName string, isRegistration bool, err error)
	DeleteMagicToken(ctx context.Context, tokenHash string) error

	// AuthorizeState parks a finished sign-in under its state. It isn't
	// handed out yet: whoever started the sign-in only chose the state, so
	// knowing it proves nothing. The signed-in user approves it first.
	AuthorizeState(ctx context.Context, stateID string, userID string, accessToken string, refreshToken string) error
	// ApproveState releases a parked sign-in, only to the user it belongs to.
	ApproveState(ctx context.Context, stateID string, userID string) error
	// GetAuthorizedState returns the tokens of an approved sign-in only.
	GetAuthorizedState(ctx context.Context, stateID string) (accessToken string, refreshToken string, err error)

	// SignOutDevice ends a device's session: its access tokens are refused
	// from now on, though they haven't expired.
	SignOutDevice(ctx context.Context, deviceID string) error
	IsDeviceSignedOut(ctx context.Context, deviceID string) (bool, error)
}

type Mailer interface {
	SendMagicLink(ctx context.Context, email string, token string, stateID string) error
}

// OAuthUserInfo is the profile a provider reports for the authenticating user.
// Only Email and ProviderUserID are guaranteed; the rest may be empty.
type OAuthUserInfo struct {
	Email          string
	ProviderUserID string
	FirstName      string
	LastName       string
	AvatarURL      string
}

type OAuthProvider interface {
	GetAuthURL(state string) string
	ExchangeCode(ctx context.Context, code string) (string, error)
	FetchUserInfo(ctx context.Context, token string) (*OAuthUserInfo, error)
}

type TxManager interface {
	WithTx(ctx context.Context, fn func(tx any) error) error
}
