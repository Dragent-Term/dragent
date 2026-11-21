package api

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"regexp"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"

	"server/internal/identity/types"
)

// Desktop sign-in hand-off, PKCE style (RFC 7636 / RFC 8252).
//
// The desktop makes a secret (the verifier) when it starts a sign-in and
// sends only its SHA-256 (the challenge) along with the state. When the
// sign-in finishes, the app's refresh token isn't put in the dragent://
// link: any app on the machine can register that scheme and read it, and
// the link lands in browser history. The link carries a one-time code
// instead, and the code is only worth something with the verifier, which
// never left the desktop. Polling needs the verifier too.
//
// Sign-ins that started without a challenge (desktop builds from before
// this) keep the old behaviour until those builds are gone.

// A challenge is base64url (no padding) of a SHA-256: 43 characters.
var challengePattern = regexp.MustCompile(`^[A-Za-z0-9_-]{43}$`)

// A verifier is 43 to 128 unreserved characters (RFC 7636 §4.1).
var verifierPattern = regexp.MustCompile(`^[A-Za-z0-9._~-]{43,128}$`)

func validChallenge(c string) bool { return challengePattern.MatchString(c) }

// verifierMatches reports whether verifier hashes to challenge (S256).
func verifierMatches(verifier, challenge string) bool {
	if !verifierPattern.MatchString(verifier) || !validChallenge(challenge) {
		return false
	}
	sum := sha256.Sum256([]byte(verifier))
	got := base64.RawURLEncoding.EncodeToString(sum[:])
	return subtle.ConstantTimeCompare([]byte(got), []byte(challenge)) == 1
}

// recordChallenge stores the challenge a sign-in starts with. An empty
// challenge is an older desktop or the browser; a malformed one is refused.
func (h *AuthHandlers) recordChallenge(ctx context.Context, state, challenge string) error {
	if challenge == "" {
		return nil
	}
	if !validChallenge(challenge) {
		return errors.New("invalid challenge")
	}
	return h.tokenStore.SetSignInChallenge(ctx, state, challenge)
}

func newHandoffCode() (string, error) {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(b), nil
}

// appHandoff is what a finished sign-in gives the app: either a one-time
// code (PKCE sign-ins) or, for older builds, the refresh token itself.
type appHandoff struct {
	code         string
	refreshToken string
}

func (a appHandoff) deepLink(state string, userID uuid.UUID) string {
	if a.code != "" {
		return fmt.Sprintf("dragent://auth/callback?code=%s&state=%s&user_id=%s",
			url.QueryEscape(a.code), url.QueryEscape(state), url.QueryEscape(userID.String()))
	}
	return fmt.Sprintf("dragent://auth/callback?refresh_token=%s&state=%s&user_id=%s",
		url.QueryEscape(a.refreshToken), url.QueryEscape(state), url.QueryEscape(userID.String()))
}

// handoffFor turns the app's refresh token into what the browser may see.
func (h *AuthHandlers) handoffFor(ctx context.Context, state, refreshToken string, userID uuid.UUID) (appHandoff, error) {
	challenge, err := h.tokenStore.SignInChallenge(ctx, state)
	if err != nil {
		return appHandoff{}, err
	}
	if challenge == "" {
		return appHandoff{refreshToken: refreshToken}, nil
	}
	code, err := newHandoffCode()
	if err != nil {
		return appHandoff{}, err
	}
	if err := h.tokenStore.SaveHandoff(ctx, code, state, refreshToken, userID.String()); err != nil {
		return appHandoff{}, err
	}
	return appHandoff{code: code}, nil
}

func writeSignInJSON(w http.ResponseWriter, accessToken string, handoff appHandoff, stateID string, userID uuid.UUID) {
	body := map[string]string{
		"access_token": accessToken,
		"state":        stateID,
		"user_id":      userID.String(),
	}
	if handoff.code != "" {
		body["handoff_code"] = handoff.code
	} else {
		body["refresh_token"] = handoff.refreshToken
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(body)
}

// finishSignIn answers a completed sign-in: JSON to the portal, otherwise a
// redirect to the app.
func (h *AuthHandlers) finishSignIn(w http.ResponseWriter, r *http.Request, accessToken, refreshToken, state string, userID uuid.UUID) {
	handoff, err := h.handoffFor(r.Context(), state, refreshToken, userID)
	if err != nil {
		slog.Error("sign-in hand-off failed", "error", err)
		http.Error(w, "sign-in failed, try again", http.StatusInternalServerError)
		return
	}
	if wantsJSON(r) {
		writeSignInJSON(w, accessToken, handoff, state, userID)
		return
	}
	deepLink := handoff.deepLink(state, userID)
	w.Header().Set("Location", deepLink)
	w.Header().Set("Cache-Control", "no-store")
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

// checkVerifier answers whether the caller holds the verifier of the
// sign-in that state started.
func (h *AuthHandlers) checkVerifier(ctx context.Context, state, verifier string) bool {
	challenge, err := h.tokenStore.SignInChallenge(ctx, state)
	if err != nil || challenge == "" {
		return false
	}
	return verifierMatches(verifier, challenge)
}

type desktopProof struct {
	State    string `json:"state"`
	Code     string `json:"code"`
	Verifier string `json:"verifier"`
}

// POST /auth/desktop/exchange {"state", "code", "verifier"}
//
// Trades a deep link's one-time code for the app's refresh token. The
// verifier proves this is the desktop that started the sign-in.
func (h *AuthHandlers) ExchangeDesktopHandoff(w http.ResponseWriter, r *http.Request) {
	var req desktopProof
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.State == "" || req.Code == "" {
		http.Error(w, "state, code and verifier are required", http.StatusBadRequest)
		return
	}
	if !h.checkVerifier(r.Context(), req.State, req.Verifier) {
		http.Error(w, "sign-in not recognised", http.StatusUnauthorized)
		return
	}
	state, refreshToken, userID, err := h.tokenStore.TakeHandoff(r.Context(), req.Code)
	if err != nil || state != req.State {
		http.Error(w, "sign-in link used or expired, sign in again", http.StatusUnauthorized)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(map[string]string{
		"refresh_token": refreshToken,
		"user_id":       userID,
	})
}

// POST /auth/desktop/session {"state", "verifier"}
//
// The polling path for desktops that started with a challenge: an approved
// sign-in, handed out once, to the holder of the verifier.
func (h *AuthHandlers) PollDesktopSessionWithProof(w http.ResponseWriter, r *http.Request) {
	var req desktopProof
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.State == "" {
		http.Error(w, "state and verifier are required", http.StatusBadRequest)
		return
	}
	if !h.checkVerifier(r.Context(), req.State, req.Verifier) {
		http.Error(w, "sign-in not recognised", http.StatusUnauthorized)
		return
	}
	h.handOutApproved(w, r, req.State)
}

// handOutApproved writes an approved sign-in's tokens, once, or 204.
func (h *AuthHandlers) handOutApproved(w http.ResponseWriter, r *http.Request, state string) {
	accessToken, refreshToken, err := h.tokenStore.TakeAuthorizedState(r.Context(), state)
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
