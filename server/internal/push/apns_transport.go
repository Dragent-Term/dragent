package push

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
)

// Official APNs endpoints (Apple: Sending notification requests to APNs).
// Production ships here; development/sandbox is explicit via APNsConfig.Sandbox.
const (
	apnsProdBaseURL    = "https://api.push.apple.com"
	apnsSandboxBaseURL = "https://api.sandbox.push.apple.com"
)

// APNs wire bounds.
const (
	// apnsMaxPayloadBytes is Apple's 4 KiB notification payload cap.
	apnsMaxPayloadBytes = 4096
	// apnsTokenReuse caps JWT reuse below Apple's 60-minute iat validity.
	apnsTokenReuse = 50 * time.Minute
	// apnsHTTPTimeout bounds one delivery attempt; ctx cancellation wins first.
	apnsHTTPTimeout = 10 * time.Second
	// apnsMaxBodyBytes caps an error payload read.
	apnsMaxBodyBytes = 16 * 1024
)

// parseAPNsPrivateKey parses a .p8 (PKCS#8) EC P-256 private key.
// Errors are sanitized: no key material is ever echoed.
func parseAPNsPrivateKey(pemBytes string) (*ecdsa.PrivateKey, error) {
	raw := strings.TrimSpace(pemBytes)
	if raw == "" {
		return nil, errors.New("push: apns private key is required")
	}
	// An env file holds one line per value: the key may arrive with its line
	// breaks written as \n.
	if !strings.Contains(raw, "\n") && strings.Contains(raw, `\n`) {
		raw = strings.ReplaceAll(raw, `\n`, "\n")
	}
	block, _ := pem.Decode([]byte(raw))
	if block == nil {
		return nil, errors.New("push: apns private key is not valid PEM")
	}
	var key any
	var err error
	switch block.Type {
	case "PRIVATE KEY":
		key, err = x509.ParsePKCS8PrivateKey(block.Bytes)
		if err != nil {
			return nil, errors.New("push: apns private key is not valid PKCS#8")
		}
	case "EC PRIVATE KEY":
		key, err = x509.ParseECPrivateKey(block.Bytes)
		if err != nil {
			return nil, errors.New("push: apns private key is not valid EC")
		}
	default:
		return nil, errors.New("push: apns private key has an unsupported PEM type")
	}
	ec, ok := key.(*ecdsa.PrivateKey)
	if !ok {
		return nil, errors.New("push: apns private key is not ECDSA")
	}
	if ec.Curve == nil || ec.Curve.Params().BitSize != 256 {
		return nil, errors.New("push: apns private key must be P-256")
	}
	return ec, nil
}

// mintAPNsJWT builds the ES256 provider token per Apple docs:
// header {alg: ES256, kid: KeyID}, claims {iss: TeamID, iat: now}.
func mintAPNsJWT(key *ecdsa.PrivateKey, keyID, teamID string, now time.Time) (string, error) {
	if strings.TrimSpace(keyID) == "" || strings.TrimSpace(teamID) == "" {
		return "", errors.New("push: apns key id and team id are required")
	}
	tok := jwt.NewWithClaims(jwt.SigningMethodES256, jwt.MapClaims{
		"iss": teamID,
		"iat": now.Unix(),
	})
	tok.Header["kid"] = keyID
	signed, err := tok.SignedString(key)
	if err != nil {
		return "", errors.New("push: apns token signing failed")
	}
	return signed, nil
}

// apnsJWT returns a cached token or mints a fresh one when older than reuse.
func (s *APNsSender) apnsJWT(now time.Time) (string, error) {
	s.tokenMu.Lock()
	tok, issued := s.cachedToken, s.cachedAt
	s.tokenMu.Unlock()
	if tok != "" && now.Sub(issued) < apnsTokenReuse && !issued.After(now.Add(time.Minute)) {
		return tok, nil
	}
	key, err := parseAPNsPrivateKey(s.Config.PrivateKeyPEM)
	if err != nil {
		return "", err
	}
	fresh, err := mintAPNsJWT(key, strings.TrimSpace(s.Config.KeyID), strings.TrimSpace(s.Config.TeamID), now)
	if err != nil {
		return "", err
	}
	s.tokenMu.Lock()
	s.cachedToken, s.cachedAt = fresh, now
	s.tokenMu.Unlock()
	return fresh, nil
}

// invalidateAPNsJWT drops the cached token so the next send mints a fresh one
// (used once on ExpiredProviderToken).
func (s *APNsSender) invalidateAPNsJWT() {
	s.tokenMu.Lock()
	s.cachedToken = ""
	s.tokenMu.Unlock()
}

func (s *APNsSender) apnsNow() time.Time {
	if s.nowFn != nil {
		return s.nowFn()
	}
	return time.Now()
}

func (s *APNsSender) apnsClient() *http.Client {
	if s.httpClient != nil {
		return s.httpClient
	}
	return noRedirectClient(apnsHTTPTimeout)
}

// apnsBaseURL resolves the official host; test overrides stay unexported and
// can never be set from config/env (production cannot be redirected).
func (s *APNsSender) apnsBaseURL() string {
	if s.baseURL != "" {
		return s.baseURL
	}
	if s.Config.Sandbox {
		return apnsSandboxBaseURL
	}
	return apnsProdBaseURL
}

// E2EE push text (§8): the only title and body our gateway or Apple ever
// see. The real notification text travels inside the sealed blob, readable
// only by the phone. The NSE overwrites this placeholder on success and
// shows it untouched on any failure.
const (
	apnsPlaceholderTitle = "Dragent"
	apnsPlaceholderBody  = "An agent needs you"
)

// apnsCollapseID derives a fixed-length, opaque collapse id from the
// sealed blob: base64url of the first 16 SHA-256 bytes (22 characters).
// The server cannot see the inner notification id (§8), so it uses this
// per-blob value instead: identical blobs coalesce, different blobs stay
// distinct, and nothing about the sender or the content leaks. Length is
// constant, so headers reveal no size signal either.
func apnsCollapseID(blob string) string {
	sum := sha256.Sum256([]byte(blob))
	return base64.RawURLEncoding.EncodeToString(sum[:16])
}

// apnsPayload builds the fixed-size alert carrying only the placeholder
// text and the opaque sealed blob (§8). It never carries a sender or
// desktop id, and never notification text, kind or workspace: those stay
// inside the blob. Every payload is byte-identical in size.
func apnsPayload(msg PushMessage) ([]byte, error) {
	if !ValidSealedPushBlob(msg.Blob) {
		return nil, errors.New("push: sealed blob is required")
	}
	payload := map[string]any{
		"aps": map[string]any{
			"alert":           map[string]string{"title": apnsPlaceholderTitle, "body": apnsPlaceholderBody},
			"mutable-content": 1,
			"sound":           "default",
		},
		"e": msg.Blob,
	}
	raw, err := json.Marshal(payload)
	if err != nil {
		return nil, errors.New("push: notification payload could not be encoded")
	}
	return raw, nil
}

type apnsErrorResponse struct {
	Reason string `json:"reason"`
}

// sendAPNs performs one authenticated POST; refreshed indicates a JWT retry.
func (s *APNsSender) sendAPNs(ctx context.Context, msg PushMessage, refreshed bool) error {
	if err := ctx.Err(); err != nil {
		return fmt.Errorf("push: apns send cancelled: %w", err)
	}
	token := strings.TrimSpace(msg.Token)
	if token == "" {
		return errors.New("push: empty device token")
	}
	now := s.apnsNow()
	jwtToken, err := s.apnsJWT(now)
	if err != nil {
		return err
	}
	payload, err := apnsPayload(msg)
	if err != nil {
		return err
	}
	url := strings.TrimRight(s.apnsBaseURL(), "/") + "/3/device/" + token
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(payload))
	if err != nil {
		return errors.New("push: apns request could not be built")
	}
	req.Header.Set("authorization", "bearer "+jwtToken)
	req.Header.Set("apns-topic", strings.TrimSpace(s.Config.BundleID))
	req.Header.Set("apns-push-type", "alert")
	req.Header.Set("apns-priority", "10")
	req.Header.Set("apns-collapse-id", apnsCollapseID(msg.Blob))
	req.Header.Set("apns-expiration", "0")
	req.Header.Set("apns-id", uuid.NewString())
	req.Header.Set("content-type", "application/json")

	resp, err := s.apnsClient().Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return fmt.Errorf("push: apns send cancelled: %w", ctx.Err())
		}
		return fmt.Errorf("push: apns delivery failed: %w", ErrRetryable)
	}
	defer resp.Body.Close()
	limited, _ := io.ReadAll(io.LimitReader(resp.Body, apnsMaxBodyBytes))

	switch {
	case resp.StatusCode == http.StatusOK:
		return nil
	case resp.StatusCode == http.StatusTooManyRequests || resp.StatusCode >= 500:
		return fmt.Errorf("push: apns temporarily unavailable (status %d): %w", resp.StatusCode, ErrRetryable)
	case resp.StatusCode == http.StatusGone:
		return fmt.Errorf("push: apns device token is no longer registered: %w", ErrInvalidToken)
	}
	var parsed apnsErrorResponse
	_ = json.Unmarshal(limited, &parsed)
	switch parsed.Reason {
	case "BadDeviceToken":
		return fmt.Errorf("push: apns rejected the device token: %w", ErrInvalidToken)
	case "Unregistered":
		return fmt.Errorf("push: apns device token is no longer registered: %w", ErrInvalidToken)
	case "ExpiredProviderToken":
		if !refreshed {
			s.invalidateAPNsJWT()
			return s.sendAPNs(ctx, msg, true)
		}
		return fmt.Errorf("push: apns provider token expired: %w", ErrAuth)
	case "InvalidProviderToken", "MissingProviderToken", "InvalidProviderTokenKeyId", "MissingTopic", "InvalidTopic":
		return fmt.Errorf("push: apns provider credentials invalid: %w", ErrAuth)
	case "BadCertificate", "BadCertificateEnvironment", "Forbidden":
		return fmt.Errorf("push: apns provider credentials invalid: %w", ErrAuth)
	case "BadCollapseId", "BadExpiration", "BadMessageId", "BadPriority", "BadTopic", "DeviceTokenNotForTopic", "DuplicateHeaders", "IdleTimeout", "MissingDeviceToken", "BadDeviceTokenSize", "BadPath", "MethodNotAllowed", "PayloadEmpty", "PayloadTooLarge", "BadPushType", "":
		if resp.StatusCode == http.StatusBadRequest {
			return errors.New("push: apns rejected the notification")
		}
		return fmt.Errorf("push: apns rejected the notification (status %d)", resp.StatusCode)
	default:
		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
			return fmt.Errorf("push: apns provider credentials invalid: %w", ErrAuth)
		}
		return fmt.Errorf("push: apns rejected the notification (status %d)", resp.StatusCode)
	}
}
