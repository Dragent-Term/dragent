package push

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"strings"
	"testing"

	"github.com/google/uuid"
)

func wrapForTest(err error) error { return fmt.Errorf("push: test wrap: %w", err) }

// All credential-looking values below are "test-" prefixed fixtures for the
// interface tests. No real credentials appear in code or tests, and no test
// performs a real provider call: genuine delivery is exercised only against
// httptest servers in apns_transport_test.go and fcm_transport_test.go.

func TestSenderFromConfigEmptyIsLogMode(t *testing.T) {
	t.Setenv("APNS_KEY_ID", "")
	t.Setenv("APNS_TEAM_ID", "")
	t.Setenv("APNS_BUNDLE_ID", "")
	t.Setenv("APNS_PRIVATE_KEY", "")
	t.Setenv("APNS_SANDBOX", "")
	t.Setenv("FCM_PROJECT_ID", "")
	t.Setenv("FCM_SERVICE_ACCOUNT_JSON", "")

	s := SenderFromConfig(ConfigFromEnv())
	if s.Name() != "log" {
		t.Fatalf("sender = %q, want log mode with no credentials", s.Name())
	}
	msg := PushMessage{DeviceID: uuid.New(), UserID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("log-mode Send: %v", err)
	}
}

func TestConfigFromEnvPlaceholders(t *testing.T) {
	t.Setenv("FCM_PROJECT_ID", "test-project")
	t.Setenv("FCM_SERVICE_ACCOUNT_JSON", `{"test":true}`)
	t.Setenv("APNS_KEY_ID", "TESTKEYID")
	t.Setenv("APNS_SANDBOX", "")

	cfg := ConfigFromEnv()
	if !cfg.FCM.Enabled() {
		t.Fatal("FCM should be enabled with test placeholders")
	}
	// APNs needs the full set; a lone key id must not enable it.
	if cfg.APNs.Enabled() {
		t.Fatal("APNs must stay disabled with only a key id")
	}
	if cfg.FCM.ProjectID != "test-project" {
		t.Fatalf("project = %q", cfg.FCM.ProjectID)
	}
	if cfg.APNs.Sandbox {
		t.Fatal("APNs sandbox must default to production")
	}
}

func TestConfigSandboxEnvExplicit(t *testing.T) {
	t.Setenv("APNS_SANDBOX", "true")
	t.Setenv("APNS_KEY_ID", "TESTKEYID")
	t.Setenv("APNS_TEAM_ID", "TESTTEAMID")
	t.Setenv("APNS_BUNDLE_ID", "dev.dragent.test")
	t.Setenv("APNS_PRIVATE_KEY", "test-pem")
	if cfg := ConfigFromEnv(); !cfg.APNs.Sandbox {
		t.Fatal("APNS_SANDBOX=true must select the sandbox endpoint")
	}
	t.Setenv("APNS_SANDBOX", "1")
	if cfg := ConfigFromEnv(); !cfg.APNs.Sandbox {
		t.Fatal("APNS_SANDBOX=1 must select the sandbox endpoint")
	}
	t.Setenv("APNS_SANDBOX", "")
	if cfg := ConfigFromEnv(); cfg.APNs.Sandbox {
		t.Fatal("empty APNS_SANDBOX must mean production")
	}
}

func TestRouterRoutesAndFallsBack(t *testing.T) {
	cfg := Config{
		FCM: FCMConfig{ProjectID: "test-project", CredentialsJSON: `{"test":true}`},
	}
	s := SenderFromConfig(cfg)
	router, ok := s.(*Router)
	if !ok {
		t.Fatalf("sender = %T, want *Router with one provider configured", s)
	}
	if router.FCM == nil || router.FCM.Name() != "fcm" {
		t.Fatalf("FCM leg = %+v", router.FCM)
	}

	ctx := context.Background()
	// The FCM leg is configured, so the router must invoke it: placeholder
	// credentials are invalid, and a configured provider must never report
	// stub success. Any error here proves routing reached FCM, not fallback.
	fcmMsg := PushMessage{DeviceID: uuid.New(), Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}
	if err := s.Send(ctx, fcmMsg); err == nil {
		t.Fatal("FCM leg with placeholder credentials reported success")
	}
	// APNs is unconfigured: the message falls back to log mode, not an error.
	apnsMsg := PushMessage{DeviceID: uuid.New(), Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')}
	if err := s.Send(ctx, apnsMsg); err != nil {
		t.Fatalf("unconfigured-platform fallback: %v", err)
	}
	// Unknown platforms are programmer errors, not fallback candidates.
	odd := PushMessage{DeviceID: uuid.New(), Platform: "sms", Token: "test-token", Title: "hi"}
	if err := s.Send(ctx, odd); err == nil {
		t.Fatal("unknown platform was accepted")
	}
}

func TestProviderSendersRequireConfig(t *testing.T) {
	ctx := context.Background()
	if err := (&APNsSender{}).Send(ctx, PushMessage{Platform: "apns", Token: "test-token"}); err == nil {
		t.Fatal("unconfigured APNs sender succeeded")
	}
	if err := (&FCMSender{}).Send(ctx, PushMessage{Platform: "fcm", Token: "test-token"}); err == nil {
		t.Fatal("unconfigured FCM sender succeeded")
	}
}

func TestProviderSendersValidateMessage(t *testing.T) {
	ctx := context.Background()
	apns := &APNsSender{Config: APNsConfig{KeyID: "test-key", TeamID: "test-team", BundleID: "dev.dragent.test", PrivateKeyPEM: "test-pem"}}
	if err := apns.Send(ctx, PushMessage{Platform: "apns", Token: "  "}); err == nil {
		t.Fatal("empty token was accepted")
	}
	// Fake PEM is a config error, never stub success.
	if err := apns.Send(ctx, PushMessage{Platform: "apns", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
		t.Fatal("APNs sender with an invalid key reported success")
	}
	fcm := &FCMSender{Config: FCMConfig{ProjectID: "test-project", CredentialsJSON: `{"test":true}`}}
	if err := fcm.Send(ctx, PushMessage{Platform: "carrier-pigeon", Token: "test-token"}); err == nil {
		t.Fatal("unknown platform was accepted")
	}
	// Placeholder service-account JSON is invalid, never stub success.
	if err := fcm.Send(ctx, PushMessage{Platform: "fcm", Token: "test-token", Blob: sealedBlobForTest('A')}); err == nil {
		t.Fatal("FCM sender with placeholder credentials reported success")
	}
}

func TestFailureClassificationHelpers(t *testing.T) {
	if !IsRetryable(wrapForTest(ErrRetryable)) {
		t.Fatal("IsRetryable must follow errors.Is")
	}
	if IsRetryable(wrapForTest(ErrAuth)) {
		t.Fatal("auth errors must not classify as retryable")
	}
	if !IsInvalidToken(wrapForTest(ErrInvalidToken)) {
		t.Fatal("IsInvalidToken must follow errors.Is")
	}
	if !IsAuthError(wrapForTest(ErrAuth)) {
		t.Fatal("IsAuthError must follow errors.Is")
	}
	if IsInvalidToken(wrapForTest(ErrRetryable)) {
		t.Fatal("retryable errors must not classify as invalid tokens")
	}
}

func TestLogSenderLogsNoContent(t *testing.T) {
	var buf bytes.Buffer
	log := slog.New(slog.NewJSONHandler(&buf, nil))
	s := &LogSender{Log: log}
	userID, deviceID := uuid.New(), uuid.New()
	msg := PushMessage{
		DeviceID: deviceID, UserID: userID, Platform: "fcm",
		Token: "test-secret-token", Blob: sealedBlobForTest('A'),
		Title: "secret title canary", Body: "secret body canary",
	}
	if err := s.Send(context.Background(), msg); err != nil {
		t.Fatalf("log-mode Send: %v", err)
	}
	out := buf.String()
	for _, canary := range []string{
		"secret title canary", "secret body canary", "test-secret-token",
		userID.String(), deviceID.String(), sealedBlobForTest('A'),
	} {
		if strings.Contains(out, canary) {
			t.Fatalf("log line leaks %q: %s", canary, out)
		}
	}
	if !strings.Contains(out, "fcm") {
		t.Fatalf("log line lost the aggregate-safe platform: %s", out)
	}
}
