package push

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
)

// PushMessage is one sealed notification for one device registration.
//
// E2EE (§8): the server is a blind router. The only content is Blob, the
// desktop's opaque sealed payload. There is no sender field: the phone
// learns the sender from the decrypted Noise static key. Title, Body and
// Data are deprecated leftovers of the pre-E2EE plaintext path: transports
// ignore them and never put them on the wire. (MirrorSender in
// relay/events.go still reads them; its removal belongs to the relay owner.)
type PushMessage struct {
	DeviceID uuid.UUID
	UserID   uuid.UUID
	Platform string // apns | fcm
	Token    string
	// Blob is the opaque sealed push payload (Noise X to the phone's push
	// key, §8): exactly SealedPushBlobChars base64url characters. Required.
	Blob string
	// Deprecated: plaintext leftovers, ignored by every transport.
	Title string
	Body  string
	// Deprecated: plaintext leftover, ignored by every transport.
	Data map[string]string
}

// Sender delivers a push message to its device. Implementations must never
// log the device token.
type Sender interface {
	Send(ctx context.Context, msg PushMessage) error
	Name() string
}

var (
	// ErrNotConfigured is returned by a provider sender whose credentials are
	// absent. Callers fall back to the log sender in that case.
	ErrNotConfigured = errors.New("push: provider not configured")
	// ErrUnknownPlatform is returned when a message names no known provider.
	ErrUnknownPlatform = errors.New("push: unknown platform")
	// ErrInvalidToken marks a device token the provider rejected as
	// unregistered: the caller should invalidate the registration.
	ErrInvalidToken = errors.New("push: invalid device token")
	// ErrAuth marks provider authentication/permission failures (sanitized:
	// no key, token, or raw provider body is ever attached).
	ErrAuth = errors.New("push: provider authentication failed")
	// ErrRetryable marks 429/5xx or transport failures worth retrying later.
	ErrRetryable = errors.New("push: provider temporarily unavailable")
)

// IsRetryable reports whether err warrants a later retry (429/5xx/transport).
func IsRetryable(err error) bool { return errors.Is(err, ErrRetryable) }

// IsInvalidToken reports whether the device token was rejected as
// unregistered and the registration should be invalidated.
func IsInvalidToken(err error) bool { return errors.Is(err, ErrInvalidToken) }

// IsAuthError reports provider credential/permission failures.
func IsAuthError(err error) bool { return errors.Is(err, ErrAuth) }

func validateMessage(msg PushMessage) error {
	switch normalizePlatform(msg.Platform) {
	case PlatformAPNs, PlatformFCM:
	default:
		return ErrUnknownPlatform
	}
	if strings.TrimSpace(msg.Token) == "" {
		return errors.New("push: empty device token")
	}
	// Only sealed blobs are accepted. A message carrying plaintext title,
	// body or data without a blob is refused: there is no plaintext
	// fallback (§10).
	if !ValidSealedPushBlob(msg.Blob) {
		return errors.New("push: sealed blob is required")
	}
	return nil
}

// SealedPushBlobChars is the exact wire size of one sealed push payload
// (§8: the 1,120-byte Noise X seal as unpadded base64url: 373*4+2 = 1,494
// characters). Intake and transports accept only this size so length leaks
// nothing.
const SealedPushBlobChars = 1494

// ValidSealedPushBlob reports whether blob is an opaque sealed payload:
// exactly SealedPushBlobChars unpadded base64url characters. Length and
// alphabet are checked; the bytes themselves stay opaque to the server.
func ValidSealedPushBlob(blob string) bool {
	if len(blob) != SealedPushBlobChars {
		return false
	}
	for i := 0; i < len(blob); i++ {
		c := blob[i]
		if c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' || c == '_' {
			continue
		}
		return false
	}
	return true
}

// ── No-credential mode ───────────────────────────────────────────────────────

// LogSender is the no-credential sender: it records only an aggregate-safe
// line (platform and outcome) and reports success. Per the §2.3 logging rule
// it never logs titles, bodies, tokens, blobs, or user or device ids.
// Local dev and unconfigured deployments use it so push code paths stay
// exercised without credentials.
type LogSender struct {
	Log *slog.Logger
}

func (s *LogSender) Name() string { return "log" }

func (s *LogSender) Send(_ context.Context, msg PushMessage) error {
	log := s.Log
	if log == nil {
		log = slog.Default()
	}
	log.Info("push: notification (log mode — no provider credentials)",
		"platform", normalizePlatform(msg.Platform),
		"result", "accepted")
	return nil
}

// ── APNs / FCM providers ─────────────────────────────────────────────────────

// APNsConfig holds Apple Push credentials. All fields come from config/env;
// none are hardcoded. Sandbox selects the development endpoint explicitly;
// production is the default.
type APNsConfig struct {
	KeyID         string // APNs Auth Key ID
	TeamID        string // Apple Developer Team ID
	BundleID      string // app bundle id (topic)
	PrivateKeyPEM string // contents of the .p8 auth key
	Sandbox       bool   // true → api.sandbox.push.apple.com, false → api.push.apple.com
}

// Enabled reports whether sending via APNs is possible.
func (c APNsConfig) Enabled() bool {
	return c.KeyID != "" && c.TeamID != "" && c.BundleID != "" && c.PrivateKeyPEM != ""
}

// FCMConfig holds Firebase Cloud Messaging credentials.
type FCMConfig struct {
	ProjectID       string // GCP project id
	CredentialsJSON string // service-account JSON (or a path the deployer mounts)
}

// Enabled reports whether sending via FCM is possible.
func (c FCMConfig) Enabled() bool {
	return c.ProjectID != "" && c.CredentialsJSON != ""
}

// Config is the push provider configuration, loaded from the environment.
type Config struct {
	APNs APNsConfig
	FCM  FCMConfig
}

// ConfigFromEnv reads provider credentials from the environment:
//
//	APNS_KEY_ID, APNS_TEAM_ID, APNS_BUNDLE_ID, APNS_PRIVATE_KEY, APNS_SANDBOX
//	FCM_PROJECT_ID, FCM_SERVICE_ACCOUNT_JSON
//
// Absent values leave that provider disabled; when nothing is configured the
// server runs in no-credential log mode. APNS_SANDBOX accepts 1/true (any case)
// for the development endpoint; anything else means production.
func ConfigFromEnv() Config {
	sandboxRaw := strings.TrimSpace(os.Getenv("APNS_SANDBOX"))
	sandbox := sandboxRaw == "1" || strings.EqualFold(sandboxRaw, "true")
	return Config{
		APNs: APNsConfig{
			KeyID:         strings.TrimSpace(os.Getenv("APNS_KEY_ID")),
			TeamID:        strings.TrimSpace(os.Getenv("APNS_TEAM_ID")),
			BundleID:      strings.TrimSpace(os.Getenv("APNS_BUNDLE_ID")),
			PrivateKeyPEM: os.Getenv("APNS_PRIVATE_KEY"),
			Sandbox:       sandbox,
		},
		FCM: FCMConfig{
			ProjectID:       strings.TrimSpace(os.Getenv("FCM_PROJECT_ID")),
			CredentialsJSON: os.Getenv("FCM_SERVICE_ACCOUNT_JSON"),
		},
	}
}

// APNsSender delivers to Apple devices via the HTTP/2 provider API
// (api.push.apple.com, or the sandbox host when Config.Sandbox is set).
type APNsSender struct {
	Config APNsConfig
	Log    *slog.Logger

	// Test-only injection (unexported; never set from config/env).
	// Production always uses the official hosts and default client/clock.
	httpClient  *http.Client
	nowFn       func() time.Time
	baseURL     string
	tokenMu     sync.Mutex
	cachedToken string
	cachedAt    time.Time
}

func (s *APNsSender) Name() string { return "apns" }

func (s *APNsSender) Send(ctx context.Context, msg PushMessage) error {
	if !s.Config.Enabled() {
		return ErrNotConfigured
	}
	if err := validateMessage(msg); err != nil {
		return err
	}
	if strings.TrimSpace(s.Config.BundleID) == "" {
		return errors.New("push: apns bundle id is required")
	}
	return s.sendAPNs(ctx, msg, false)
}

// FCMSender delivers to Android devices via the Firebase Cloud Messaging
// HTTP v1 API (OAuth2 service-account flow + projects.messages:send).
type FCMSender struct {
	Config FCMConfig
	Log    *slog.Logger

	// Test-only injection (unexported; never set from config/env).
	httpClient   *http.Client
	nowFn        func() time.Time
	tokenURL     string
	sendURL      string
	tokenMu      sync.Mutex
	cachedToken  string
	cachedExpiry time.Time
	// exMu serializes token exchanges so concurrent cache misses share one
	// mint instead of storming the OAuth endpoint.
	exMu sync.Mutex
}

// noRedirectClient returns a bounded client that never follows redirects.
// Provider hosts are fixed and official, so no redirect is ever needed;
// following one could only leave the fixed host.
func noRedirectClient(timeout time.Duration) *http.Client {
	return &http.Client{
		Timeout: timeout,
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
}

func (s *FCMSender) Name() string { return "fcm" }

func (s *FCMSender) Send(ctx context.Context, msg PushMessage) error {
	if !s.Config.Enabled() {
		return ErrNotConfigured
	}
	if err := validateMessage(msg); err != nil {
		return err
	}
	if strings.TrimSpace(s.Config.ProjectID) == "" {
		return errors.New("push: fcm project id is required")
	}
	return s.sendFCM(ctx, msg, false)
}

// ── Routing ──────────────────────────────────────────────────────────────────

// Router fans a message out to the sender matching its platform. Messages for
// a platform whose provider is not configured fall back to the Fallback
// sender (log mode), so one unconfigured provider never blocks the other.
type Router struct {
	APNs     Sender
	FCM      Sender
	Fallback Sender
}

func (r *Router) Name() string { return "router" }

func (r *Router) Send(ctx context.Context, msg PushMessage) error {
	switch normalizePlatform(msg.Platform) {
	case PlatformAPNs:
		if r.APNs != nil {
			return r.APNs.Send(ctx, msg)
		}
	case PlatformFCM:
		if r.FCM != nil {
			return r.FCM.Send(ctx, msg)
		}
	default:
		return ErrUnknownPlatform
	}
	if r.Fallback != nil {
		return r.Fallback.Send(ctx, msg)
	}
	return ErrNotConfigured
}

// SenderFromConfig builds the sender tree for cfg. With no provider
// credentials it returns a *LogSender (no-credential mode: log instead of
// sending). With partial credentials the missing platform falls back to log
// mode while the configured one sends.
func SenderFromConfig(cfg Config) Sender {
	fallback := &LogSender{}
	var apns, fcm Sender
	if cfg.APNs.Enabled() {
		apns = &APNsSender{Config: cfg.APNs}
	}
	if cfg.FCM.Enabled() {
		fcm = &FCMSender{Config: cfg.FCM}
	}
	if apns == nil && fcm == nil {
		return fallback
	}
	return &Router{APNs: apns, FCM: fcm, Fallback: fallback}
}

var (
	_ Sender = (*LogSender)(nil)
	_ Sender = (*APNsSender)(nil)
	_ Sender = (*FCMSender)(nil)
	_ Sender = (*Router)(nil)
)
