package api

import (
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"strings"

	"server/internal/identity/components"
	"server/internal/identity/model"
	"server/internal/identity/types"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

type AuthHandlers struct {
	sendMagicLinkUseCase   *components.SendMagicLinkUseCase
	verifyMagicLinkUseCase *components.VerifyMagicLinkUseCase
	authOAuthUseCase       *components.AuthenticateOAuthUseCase
	tokenStore             model.TokenStore
	githubOAuth            model.OAuthProvider
	googleOAuth            model.OAuthProvider
	magicLimits            MagicLinkLimits
}

func NewAuthHandlers(
	sendMagicLinkUseCase *components.SendMagicLinkUseCase,
	verifyMagicLinkUseCase *components.VerifyMagicLinkUseCase,
	authOAuthUseCase *components.AuthenticateOAuthUseCase,
	tokenStore model.TokenStore,
	githubOAuth model.OAuthProvider,
	googleOAuth model.OAuthProvider,
) *AuthHandlers {
	return &AuthHandlers{
		sendMagicLinkUseCase:   sendMagicLinkUseCase,
		verifyMagicLinkUseCase: verifyMagicLinkUseCase,
		authOAuthUseCase:       authOAuthUseCase,
		tokenStore:             tokenStore,
		githubOAuth:            githubOAuth,
		googleOAuth:            googleOAuth,
	}
}

// GET /auth/start?provider=github&state=xyz
func (h *AuthHandlers) StartAuth(w http.ResponseWriter, r *http.Request) {
	provider := r.URL.Query().Get("provider")
	state := r.URL.Query().Get("state")
	if state == "" {
		http.Error(w, "missing state parameter", http.StatusBadRequest)
		return
	}

	var authURL string
	switch strings.ToLower(provider) {
	case "github":
		authURL = h.githubOAuth.GetAuthURL(state)
	case "google":
		authURL = h.googleOAuth.GetAuthURL(state)
	default:
		http.Error(w, "invalid or unsupported auth provider", http.StatusBadRequest)
		return
	}

	http.Redirect(w, r, authURL, http.StatusTemporaryRedirect)
}

// POST /auth/magic/request
func (h *AuthHandlers) RequestMagicLink(w http.ResponseWriter, r *http.Request) {
	var req types.MagicRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "invalid request body", http.StatusBadRequest)
		return
	}

	if req.Email == "" || req.State == "" {
		http.Error(w, "email and state parameters are required", http.StatusBadRequest)
		return
	}
	if !h.allowMagicLink(w, r, req.Email) {
		return
	}

	cmd := components.SendMagicLinkCommand{
		Email:          req.Email,
		StateID:        req.State,
		FirstName:      req.FirstName,
		LastName:       req.LastName,
		IsRegistration: req.IsRegistration,
	}

	if err := h.sendMagicLinkUseCase.Execute(r.Context(), cmd); err != nil {
		slog.Error("failed to process magic link request", "error", err)
		http.Error(w, "failed to request magic link", http.StatusInternalServerError)
		return
	}

	// Same answer whether or not the address has an account.
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]bool{"success": true})
}

// wantsJSON is the sign-in portal calling server to server: it gets the
// session in the response body instead of a redirect to the app.
func wantsJSON(r *http.Request) bool {
	return strings.Contains(r.Header.Get("Accept"), "application/json")
}

func writeSignInJSON(w http.ResponseWriter, accessToken, refreshToken, stateID string, userID uuid.UUID) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(map[string]string{
		"access_token":  accessToken,
		"refresh_token": refreshToken,
		"state":         stateID,
		"user_id":       userID.String(),
	})
}

// GET /auth/magic?token=xyz&state=abc
func (h *AuthHandlers) VerifyMagicLink(w http.ResponseWriter, r *http.Request) {
	token := r.URL.Query().Get("token")
	state := r.URL.Query().Get("state")
	if token == "" || state == "" {
		http.Error(w, "token and state are required", http.StatusBadRequest)
		return
	}

	result, err := h.verifyMagicLinkUseCase.Execute(r.Context(), token)
	if err != nil {
		slog.Warn("magic link validation failed", "error", err)
		w.WriteHeader(http.StatusUnauthorized)
		_, _ = w.Write([]byte("<html><body><h3>Link expired or invalid. Please try signing in again.</h3></body></html>"))
		return
	}
	if wantsJSON(r) {
		writeSignInJSON(w, result.AccessToken, result.RefreshToken, result.StateID, result.UserID)
		return
	}

	deepLink := fmt.Sprintf("dragent://auth/callback?refresh_token=%s&state=%s&user_id=%s",
		url.QueryEscape(result.RefreshToken), url.QueryEscape(result.StateID), url.QueryEscape(result.UserID.String()))

	w.Header().Set("Location", deepLink)
	w.WriteHeader(http.StatusFound)
	_, _ = w.Write([]byte(fmt.Sprintf(`<html>
<head><title>Sign In Successful</title></head>
<body style="font-family: sans-serif; text-align: center; padding-top: 50px;">
  <h2>Sign In Successful!</h2>
  <p>You can close this tab and return to your terminal workspace.</p>
  <p>If you are not redirected automatically, <a href="%s">click here</a>.</p>
</body>
</html>`, deepLink)))
}

// GET /auth/callback/github?code=xyz&state=abc
func (h *AuthHandlers) GitHubCallback(w http.ResponseWriter, r *http.Request) {
	h.oauthCallback(w, r, "github", h.githubOAuth)
}

// GET /auth/callback/google?code=xyz&state=abc
func (h *AuthHandlers) GoogleCallback(w http.ResponseWriter, r *http.Request) {
	h.oauthCallback(w, r, "google", h.googleOAuth)
}

func (h *AuthHandlers) oauthCallback(w http.ResponseWriter, r *http.Request, provider string, oauthService model.OAuthProvider) {
	code := r.URL.Query().Get("code")
	state := r.URL.Query().Get("state")
	if code == "" || state == "" {
		http.Error(w, "code and state parameters are required", http.StatusBadRequest)
		return
	}

	cmd := components.AuthenticateOAuthCommand{
		Provider:     provider,
		Code:         code,
		StateID:      state,
		OAuthService: oauthService,
	}

	result, err := h.authOAuthUseCase.Execute(r.Context(), cmd)
	if err != nil {
		slog.Error("oauth callback processing failed", "provider", provider, "error", err)
		http.Error(w, "authentication failed", http.StatusUnauthorized)
		return
	}
	if wantsJSON(r) {
		writeSignInJSON(w, result.AccessToken, result.RefreshToken, result.StateID, result.UserID)
		return
	}

	deepLink := fmt.Sprintf("dragent://auth/callback?refresh_token=%s&state=%s&user_id=%s",
		url.QueryEscape(result.RefreshToken), url.QueryEscape(result.StateID), url.QueryEscape(result.UserID.String()))

	w.Header().Set("Location", deepLink)
	w.WriteHeader(http.StatusFound)
	_, _ = w.Write([]byte(fmt.Sprintf(`<html>
<head><title>Sign In Successful</title></head>
<body style="font-family: sans-serif; text-align: center; padding-top: 50px;">
  <h2>Sign In Successful!</h2>
  <p>You can close this tab and return to your terminal workspace.</p>
  <p>If you are not redirected automatically, <a href="%s">click here</a>.</p>
</body>
</html>`, deepLink)))
}

// GET /auth/desktop/session?state=xyz
//
// The state is chosen by whoever starts a sign-in, so it isn't proof of
// anything: someone could start one, send the link to another person, and
// collect their session here. A finished sign-in is therefore only handed
// out after the person who signed in approves it in their browser (POST
// /auth/desktop/approve), having checked that the code shown there matches
// the one on their desktop.
func (h *AuthHandlers) PollDesktopSession(w http.ResponseWriter, r *http.Request) {
	state := r.URL.Query().Get("state")
	if state == "" {
		http.Error(w, "missing state parameter", http.StatusBadRequest)
		return
	}

	accessToken, refreshToken, err := h.tokenStore.GetAuthorizedState(r.Context(), state)
	if err != nil {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	// The signed-in user, from the token this server minted and just took
	// out of its own store (not the caller's state, which isn't an identity).
	userID := ""
	claims := &types.TokenClaims{}
	if _, _, err := jwt.NewParser().ParseUnverified(accessToken, claims); err == nil && claims.UserID != uuid.Nil {
		userID = claims.UserID.String()
	}

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(map[string]interface{}{
		"access_token":  accessToken,
		"refresh_token": refreshToken,
		"user_id":       userID,
	})
}

// POST /auth/desktop/approve {"state": "..."}, signed in.
//
// Releases the sign-in parked under state to the desktop polling for it.
// Only the user who signed in can approve it.
func (h *AuthHandlers) ApproveDesktopSession(w http.ResponseWriter, r *http.Request) {
	userID, _ := r.Context().Value("user_id").(string)
	if userID == "" {
		http.Error(w, "authentication required", http.StatusUnauthorized)
		return
	}
	var body struct {
		State string `json:"state"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&body); err != nil || body.State == "" {
		http.Error(w, "state is required", http.StatusBadRequest)
		return
	}
	if err := h.tokenStore.ApproveState(r.Context(), body.State, userID); err != nil {
		// Unknown, expired and someone else's all look the same.
		http.Error(w, "no sign-in waiting for approval", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]bool{"approved": true})
}
