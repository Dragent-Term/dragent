package components

import "context"

// WelcomeMailer sends the one-time welcome email on registration. Declared
// here as a one-method interface, rather than importing identity/model's
// concrete SmtpMailer, for the same reason as BalanceInitializer/
// OrganizationInitializer — the concrete SmtpMailer already satisfies it
// structurally.
type WelcomeMailer interface {
	SendWelcome(ctx context.Context, email, name string) error
}
