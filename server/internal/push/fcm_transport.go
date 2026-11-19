package push

import (
	"bytes"
	"context"
	"crypto"
	"crypto/rsa"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/golang-jwt/jwt/v5"
)

// Official FCM v1 endpoints (Firebase: Send a message using FCM HTTP v1 API).
const (
	fcmOAuthScope   = "https://www.googleapis.com/auth/firebase.messaging"
	fcmTokenURL     = "https://oauth2.googleapis.com/token"
	fcmTokenGrant   = "urn:ietf:params:oauth:grant-type:jwt-bearer"
	fcmSendURLFmt   = "https://fcm.googleapis.com/v1/projects/%s/messages:send"
	fcmMaxGrantSkew = 5 * time.Minute
)

// FCM wire bounds.
const (
	// fcmMaxPayloadBytes caps the v1 send JSON (notification + data).
	fcmMaxPayloadBytes = 4096
	// fcmHTTPTimeout bounds token exchange and send attempts each.
	fcmHTTPTimeout = 10 * time.Second
	// fcmMaxBodyBytes caps token/send response reads.
	fcmMaxBodyBytes = 64 * 1024
	// fcmMaxKeyBytes caps an inline service-account JSON or key file read.
	fcmMaxKeyBytes = 1 << 20
)

// fcmServiceAccount is the subset of the downloaded JSON we use.
type fcmServiceAccount struct {
	Type        string `json:"type"`
	ProjectID   string `json:"project_id"`
	PrivateKey  string `json:"private_key"`
	PrivateKeyX string `json:"private_key_id"`
	ClientEmail string `json:"client_email"`
	TokenURI    string `json:"token_uri"`
}

// parseFCMAccount parses service-account credentials: inline JSON when the
// value starts with "{", otherwise a mounted file path (bounded read).
// Token endpoints other than the official one are rejected.
func parseFCMAccount(raw string) (*fcmServiceAccount, error) {
	text := strings.TrimSpace(raw)
	if text == "" {
		return nil, errors.New("push: fcm service account is required")
	}
	var data []byte
	if strings.HasPrefix(text, "{") {
		data = []byte(text)
	} else {
		if len(text) > 4096 || strings.Contains(text, "\x00") {
			return nil, errors.New("push: fcm service account path is invalid")
		}
		f, err := os.Open(text)
		if err != nil {
			return nil, errors.New("push: fcm service account could not be read")
		}
		// Bound the read before buffering: max+1 detects overflow.
		file, err := io.ReadAll(io.LimitReader(f, fcmMaxKeyBytes+1))
		closeErr := f.Close()
		if err != nil || closeErr != nil || len(file) == 0 || len(file) > fcmMaxKeyBytes {
			return nil, errors.New("push: fcm service account could not be read")
		}
		data = file
	}
	if len(data) > fcmMaxKeyBytes {
		return nil, errors.New("push: fcm service account is too large")
	}
	var acct fcmServiceAccount
	if err := json.Unmarshal(data, &acct); err != nil {
		return nil, errors.New("push: fcm service account is not valid JSON")
	}
	if strings.TrimSpace(acct.PrivateKey) == "" || strings.TrimSpace(acct.ClientEmail) == "" {
		return nil, errors.New("push: fcm service account is missing its key")
	}
	if uri := strings.TrimSpace(acct.TokenURI); uri != "" && uri != fcmTokenURL {
		return nil, errors.New("push: fcm token endpoint is not allowed")
	}
	return &acct, nil
}

// parseFCMPrivateKey parses the service account RSA private key (PKCS#8 or
// PKCS#1). Errors never echo key material.
func parseFCMPrivateKey(pemText string) (crypto.Signer, string, error) {
	block, _ := pem.Decode([]byte(strings.TrimSpace(pemText)))
	if block == nil {
		return nil, "", errors.New("push: fcm private key is not valid PEM")
	}
	switch block.Type {
	case "PRIVATE KEY":
		key, err := x509.ParsePKCS8PrivateKey(block.Bytes)
		if err != nil {
			return nil, "", errors.New("push: fcm private key is not valid PKCS#8")
		}
		signer, ok := key.(crypto.Signer)
		if !ok {
			return nil, "", errors.New("push: fcm private key cannot sign")
		}
		if _, ok := key.(*rsa.PrivateKey); !ok {
			return nil, "", errors.New("push: fcm private key must be RSA")
		}
		return signer, "RS256", nil
	case "RSA PRIVATE KEY":
		key, err := x509.ParsePKCS1PrivateKey(block.Bytes)
		if err != nil {
			return nil, "", errors.New("push: fcm private key is not valid PKCS#1")
		}
		return key, "RS256", nil
	default:
		return nil, "", errors.New("push: fcm private key has an unsupported PEM type")
	}
}

// mintFCMAssertion builds the signed JWT exchanged for an access token.
func mintFCMAssertion(acct *fcmServiceAccount, signer crypto.Signer, alg, tokenURL string, now time.Time) (string, error) {
	method := jwt.GetSigningMethod(alg)
	if method == nil {
		return "", errors.New("push: fcm signing method is unavailable")
	}
	tok := jwt.NewWithClaims(method, jwt.MapClaims{
		"iss":   strings.TrimSpace(acct.ClientEmail),
		"scope": fcmOAuthScope,
		"aud":   tokenURL,
		"iat":   now.Unix(),
		"exp":   now.Add(time.Hour).Unix(),
	})
	if kid := strings.TrimSpace(acct.PrivateKeyX); kid != "" {
		tok.Header["kid"] = kid
	}
	signed, err := tok.SignedString(signer)
	if err != nil {
		return "", errors.New("push: fcm assertion signing failed")
	}
	return signed, nil
}

func (s *FCMSender) fcmNow() time.Time {
	if s.nowFn != nil {
		return s.nowFn()
	}
	return time.Now()
}

func (s *FCMSender) fcmClient() *http.Client {
	if s.httpClient != nil {
		return s.httpClient
	}
	return noRedirectClient(fcmHTTPTimeout)
}

// fcmTokenURL resolves the official OAuth2 endpoint; test overrides stay
// unexported and can never be set from config/env.
func (s *FCMSender) fcmTokenURL() string {
	if s.tokenURL != "" {
		return s.tokenURL
	}
	return fcmTokenURL
}

// fcmSendURL resolves the official v1 send endpoint for the project.
func (s *FCMSender) fcmSendURL() string {
	if s.sendURL != "" {
		return s.sendURL
	}
	return fmt.Sprintf(fcmSendURLFmt, s.Config.ProjectID)
}

// fcmAccessToken returns a cached OAuth2 token or exchanges a fresh one.
// Concurrent cache misses serialize on exMu so only one mint runs at a time;
// waiters re-check the cache and share the winner's token.
func (s *FCMSender) fcmAccessToken(ctx context.Context) (string, error) {
	now := s.fcmNow()
	s.tokenMu.Lock()
	cached, expiry := s.cachedToken, s.cachedExpiry
	s.tokenMu.Unlock()
	if cached != "" && now.Add(fcmMaxGrantSkew).Before(expiry) {
		return cached, nil
	}
	s.exMu.Lock()
	defer s.exMu.Unlock()
	s.tokenMu.Lock()
	cached, expiry = s.cachedToken, s.cachedExpiry
	s.tokenMu.Unlock()
	if cached != "" && now.Add(fcmMaxGrantSkew).Before(expiry) {
		return cached, nil
	}
	return s.exchangeFCMAccessToken(ctx, now)
}

type fcmTokenResponse struct {
	AccessToken string `json:"access_token"`
	ExpiresIn   int64  `json:"expires_in"`
	TokenType   string `json:"token_type"`
}

// exchangeFCMAccessToken mints the signed assertion and trades it for a
// short-lived access token (Google OAuth2 JWT bearer flow).
func (s *FCMSender) exchangeFCMAccessToken(ctx context.Context, now time.Time) (string, error) {
	if err := ctx.Err(); err != nil {
		return "", fmt.Errorf("push: fcm token exchange cancelled: %w", err)
	}
	acct, err := parseFCMAccount(s.Config.CredentialsJSON)
	if err != nil {
		return "", err
	}
	signer, alg, err := parseFCMPrivateKey(acct.PrivateKey)
	if err != nil {
		return "", err
	}
	tokenURL := s.fcmTokenURL()
	assertion, err := mintFCMAssertion(acct, signer, alg, tokenURL, now)
	if err != nil {
		return "", err
	}
	form := url.Values{}
	form.Set("grant_type", fcmTokenGrant)
	form.Set("assertion", assertion)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, tokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return "", errors.New("push: fcm token request could not be built")
	}
	req.Header.Set("content-type", "application/x-www-form-urlencoded")
	resp, err := s.fcmClient().Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return "", fmt.Errorf("push: fcm token exchange cancelled: %w", ctx.Err())
		}
		return "", fmt.Errorf("push: fcm token exchange failed: %w", ErrRetryable)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(resp.Body, fcmMaxBodyBytes))
	if resp.StatusCode != http.StatusOK {
		if resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500 {
			return "", fmt.Errorf("push: fcm token endpoint temporarily unavailable: %w", ErrRetryable)
		}
		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden || resp.StatusCode == http.StatusBadRequest {
			return "", fmt.Errorf("push: fcm service account rejected: %w", ErrAuth)
		}
		return "", fmt.Errorf("push: fcm token exchange rejected (status %d)", resp.StatusCode)
	}
	var parsed fcmTokenResponse
	if err := json.Unmarshal(body, &parsed); err != nil || strings.TrimSpace(parsed.AccessToken) == "" {
		return "", errors.New("push: fcm token response was unreadable")
	}
	// Fail closed: missing/invalid/non-Bearer/overflow TTLs never yield a token.
	if parsed.TokenType != "" && !strings.EqualFold(strings.TrimSpace(parsed.TokenType), "Bearer") {
		return "", errors.New("push: fcm token response was unreadable")
	}
	// Bound the TTL before Duration arithmetic: Google issues ~3600s; allow up
	// to 24h so clock skew never overflows int64 nanoseconds.
	if parsed.ExpiresIn <= 0 || parsed.ExpiresIn > 86400 {
		return "", errors.New("push: fcm token response was unreadable")
	}
	ttl := time.Duration(parsed.ExpiresIn) * time.Second
	if ttl <= 0 {
		ttl = time.Hour
	}
	expiry := now.Add(ttl)
	if skew := fcmMaxGrantSkew; ttl > skew {
		expiry = expiry.Add(-skew)
	}
	s.tokenMu.Lock()
	s.cachedToken, s.cachedExpiry = parsed.AccessToken, expiry
	s.tokenMu.Unlock()
	return parsed.AccessToken, nil
}

// fcmPayload builds the data-only message (§8): the sealed blob under one
// key, high priority, and no notification block — so the system never shows
// server-provided text, and notifications and (Android-only, rate-limited)
// dismissals are indistinguishable on the wire. Every payload for one token
// is byte-identical in size.
func fcmPayload(msg PushMessage) ([]byte, error) {
	if !ValidSealedPushBlob(msg.Blob) {
		return nil, errors.New("push: sealed blob is required")
	}
	envelope := map[string]any{
		"message": map[string]any{
			"token":   strings.TrimSpace(msg.Token),
			"data":    map[string]string{"e": msg.Blob},
			"android": map[string]string{"priority": "HIGH"},
		},
	}
	raw, err := json.Marshal(envelope)
	if err != nil {
		return nil, errors.New("push: notification payload could not be encoded")
	}
	if len(raw) > fcmMaxPayloadBytes {
		return nil, errors.New("push: notification payload is too large")
	}
	return raw, nil
}

type fcmErrorDetail struct {
	Type      string `json:"@type"`
	ErrorCode string `json:"errorCode"`
}

type fcmErrorBody struct {
	Error struct {
		Code    int              `json:"code"`
		Message string           `json:"message"`
		Status  string           `json:"status"`
		Details []fcmErrorDetail `json:"details"`
	} `json:"error"`
}

// fcmUnregistered reports whether the v1 details carry the authoritative
// FcmError UNREGISTERED code. Only this detail invalidates a registration;
// bare 404/NOT_FOUND (e.g. a missing project) must not delete tokens, and no
// message-text heuristics are used.
func fcmUnregistered(body fcmErrorBody) bool {
	for _, d := range body.Error.Details {
		if !strings.EqualFold(strings.TrimSpace(d.ErrorCode), "UNREGISTERED") {
			continue
		}
		if typ := strings.TrimSpace(d.Type); typ == "" || strings.Contains(typ, "FcmError") {
			return true
		}
	}
	return false
}

// sendFCM performs one v1 send; refreshed guards the single token-refresh retry.
func (s *FCMSender) sendFCM(ctx context.Context, msg PushMessage, refreshed bool) error {
	if err := ctx.Err(); err != nil {
		return fmt.Errorf("push: fcm send cancelled: %w", err)
	}
	token := strings.TrimSpace(msg.Token)
	if token == "" {
		return errors.New("push: empty device token")
	}
	access, err := s.fcmAccessToken(ctx)
	if err != nil {
		return err
	}
	payload, err := fcmPayload(msg)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.fcmSendURL(), bytes.NewReader(payload))
	if err != nil {
		return errors.New("push: fcm request could not be built")
	}
	req.Header.Set("authorization", "Bearer "+access)
	req.Header.Set("content-type", "application/json; charset=utf-8")
	resp, err := s.fcmClient().Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return fmt.Errorf("push: fcm send cancelled: %w", ctx.Err())
		}
		return fmt.Errorf("push: fcm delivery failed: %w", ErrRetryable)
	}
	defer resp.Body.Close()
	limited, _ := io.ReadAll(io.LimitReader(resp.Body, fcmMaxBodyBytes))
	if resp.StatusCode == http.StatusOK {
		var ok struct {
			Name string `json:"name"`
		}
		if err := json.Unmarshal(limited, &ok); err != nil || strings.TrimSpace(ok.Name) == "" {
			return errors.New("push: fcm response was unreadable")
		}
		return nil
	}
	var parsed fcmErrorBody
	_ = json.Unmarshal(limited, &parsed)
	switch {
	case fcmUnregistered(parsed):
		return fmt.Errorf("push: fcm device token is no longer registered: %w", ErrInvalidToken)
	case resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500:
		return fmt.Errorf("push: fcm temporarily unavailable (status %d): %w", resp.StatusCode, ErrRetryable)
	case resp.StatusCode == http.StatusUnauthorized:
		if !refreshed {
			s.invalidateFCMToken()
			if _, terr := s.exchangeFCMAccessToken(ctx, s.fcmNow()); terr == nil {
				return s.sendFCM(ctx, msg, true)
			}
		}
		return fmt.Errorf("push: fcm authentication failed: %w", ErrAuth)
	case resp.StatusCode == http.StatusForbidden || strings.EqualFold(strings.TrimSpace(parsed.Error.Status), "PERMISSION_DENIED"):
		return fmt.Errorf("push: fcm sender lacks permission: %w", ErrAuth)
	default:
		return fmt.Errorf("push: fcm rejected the notification (status %d)", resp.StatusCode)
	}
}

// invalidateFCMToken drops the cached access token.
func (s *FCMSender) invalidateFCMToken() {
	s.tokenMu.Lock()
	s.cachedToken = ""
	s.tokenMu.Unlock()
}
