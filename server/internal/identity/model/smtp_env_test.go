package model

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"strings"
	"testing"
)

// captureLogs routes slog to a buffer for the duration of the test.
func captureLogs(t *testing.T) *bytes.Buffer {
	t.Helper()
	var buf bytes.Buffer
	prev := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&buf, nil)))
	t.Cleanup(func() { slog.SetDefault(prev) })
	return &buf
}

func TestIsDevEnv(t *testing.T) {
	for env, want := range map[string]bool{
		"dev": true, "development": true, "LOCAL": true, " dev ": true,
		"": false, "prod": false, "production": false, "staging": false,
	} {
		if got := IsDevEnv(env); got != want {
			t.Errorf("IsDevEnv(%q) = %v, want %v", env, got, want)
		}
	}
}

func TestMailerOutsideDevRejectsIncompleteConfig(t *testing.T) {
	for _, env := range []string{"", "production", "staging"} {
		_, err := NewSmtpMailerForEnv(env, "smtp.example.com", 587, "", "", "login@example.com", "https://app")
		if !errors.Is(err, ErrMailNotConfigured) {
			t.Fatalf("APP_ENV=%q, no user: got %v, want ErrMailNotConfigured", env, err)
		}
		if !strings.Contains(err.Error(), "SMTP_USER") || !strings.Contains(err.Error(), "SMTP_PASS") {
			t.Errorf("error should name the missing settings: %v", err)
		}
	}
	if _, err := NewSmtpMailerForEnv("production", "smtp.example.com", 587, "resend", "secret", "login@example.com", "https://app"); err != nil {
		t.Fatalf("complete config rejected: %v", err)
	}
}

func TestNewSmtpMailerPanicsOutsideDevWithoutSMTP(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	defer func() {
		if recover() == nil {
			t.Fatal("expected a startup panic for missing SMTP config")
		}
	}()
	NewSmtpMailer("localhost", 1025, "", "", "noreply@example.com", "https://app")
}

func TestDevFallbackLogsLinkOnlyInDev(t *testing.T) {
	logs := captureLogs(t)
	m, err := NewSmtpMailerForEnv("dev", "localhost", 1025, "", "", "noreply@example.com", "http://localhost:8080")
	if err != nil {
		t.Fatal(err)
	}
	if err := m.SendMagicLink(context.Background(), "a@example.com", "tok123", "st"); err != nil {
		t.Fatalf("dev fallback should succeed: %v", err)
	}
	if !strings.Contains(logs.String(), "tok123") {
		t.Fatalf("dev fallback should log the link, got %q", logs.String())
	}
}

// A mailer built without the constructor (or with config wiped) outside
// dev must fail the send and keep the link out of the logs.
func TestUnconfiguredMailerFailsWithoutLoggingLink(t *testing.T) {
	logs := captureLogs(t)
	m := &SmtpMailer{host: "smtp.example.com", port: 587, sender: "login@example.com", appBaseURL: "https://app"}
	err := m.SendMagicLink(context.Background(), "a@example.com", "tok123", "st")
	if !errors.Is(err, ErrMailNotConfigured) {
		t.Fatalf("got %v, want ErrMailNotConfigured", err)
	}
	if strings.Contains(logs.String(), "tok123") || strings.Contains(logs.String(), "a@example.com") {
		t.Fatalf("link or address leaked into logs: %q", logs.String())
	}
}

// Real config outside dev never takes the log path, even on localhost.
func TestProductionMailerNeverUsesLogFallback(t *testing.T) {
	m, err := NewSmtpMailerForEnv("production", "localhost", 1025, "user", "pass", "login@example.com", "https://app")
	if err != nil {
		t.Fatal(err)
	}
	if m.devFallback {
		t.Fatal("dev fallback enabled outside dev")
	}
}
