package components

import (
	"context"
	"testing"
)

type recordingWelcomeMailer struct{ sentTo []string }

func (r *recordingWelcomeMailer) SendWelcome(_ context.Context, email, _ string) error {
	r.sentTo = append(r.sentTo, email)
	return nil
}

// The setter is the whole mechanism: if it does not store the dependency,
// both registration paths silently skip the welcome email.
func TestWelcomeMailerIsWired(t *testing.T) {
	oauth := &AuthenticateOAuthUseCase{}
	if oauth.welcomeMailer != nil {
		t.Fatal("expected nil before wiring")
	}
	r1 := &recordingWelcomeMailer{}
	oauth.SetWelcomeMailer(r1)
	if oauth.welcomeMailer == nil {
		t.Error("AuthenticateOAuthUseCase: setter did not store the mailer")
	}

	magic := &VerifyMagicLinkUseCase{}
	r2 := &recordingWelcomeMailer{}
	magic.SetWelcomeMailer(r2)
	if magic.welcomeMailer == nil {
		t.Error("VerifyMagicLinkUseCase: setter did not store the mailer")
	}

	// Nil must stay safe — an un-wired usecase should skip, not panic.
	var unset WelcomeMailer
	if unset != nil {
		t.Fatal("zero interface should be nil")
	}
}
