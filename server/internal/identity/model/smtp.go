package model

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/smtp"
	"os"
	"strings"
	"time"
)

type SmtpMailer struct {
	host       string
	port       int
	username   string
	password   string
	sender     string
	appBaseURL string
	// devFallback logs mail (sign-in links included) instead of sending it.
	// Only ever true when APP_ENV is dev/local: anywhere else a logged link
	// lets anyone with log access sign in as the recipient.
	devFallback bool
}

// ErrMailNotConfigured is returned by every send when SMTP is not set up and
// the dev log fallback is not allowed.
var ErrMailNotConfigured = errors.New("smtp is not configured")

// IsDevEnv reports whether APP_ENV names a developer environment. Unset
// means production: the safe default, as for the GraphQL playground.
func IsDevEnv(appEnv string) bool {
	switch strings.ToLower(strings.TrimSpace(appEnv)) {
	case "dev", "development", "local":
		return true
	}
	return false
}

// NewSmtpMailer builds the mailer for the APP_ENV of this process. Outside
// dev/local it panics when SMTP config is incomplete, so a deploy missing
// SMTP settings fails at startup instead of silently not sending sign-in mail.
// Callers that prefer an error should use NewSmtpMailerForEnv.
func NewSmtpMailer(host string, port int, username, password, sender, appBaseURL string) *SmtpMailer {
	m, err := NewSmtpMailerForEnv(os.Getenv("APP_ENV"), host, port, username, password, sender, appBaseURL)
	if err != nil {
		panic(err.Error())
	}
	return m
}

// NewSmtpMailerForEnv is NewSmtpMailer with the environment passed in and an
// error instead of a panic.
func NewSmtpMailerForEnv(appEnv, host string, port int, username, password, sender, appBaseURL string) (*SmtpMailer, error) {
	m := &SmtpMailer{
		host:       host,
		port:       port,
		username:   username,
		password:   password,
		sender:     sender,
		appBaseURL: appBaseURL,
	}
	if IsDevEnv(appEnv) {
		// Dev keeps the old behaviour: no real SMTP account means log the mail.
		m.devFallback = host == "localhost" || username == ""
		return m, nil
	}
	if missing := m.missingConfig(); len(missing) > 0 {
		return nil, fmt.Errorf("%w: APP_ENV=%q needs %s set (set APP_ENV=dev to log mail locally instead)",
			ErrMailNotConfigured, appEnv, strings.Join(missing, ", "))
	}
	return m, nil
}

func (m *SmtpMailer) missingConfig() []string {
	var missing []string
	if strings.TrimSpace(m.host) == "" {
		missing = append(missing, "SMTP_HOST")
	}
	if m.port <= 0 {
		missing = append(missing, "SMTP_PORT")
	}
	if strings.TrimSpace(m.username) == "" {
		missing = append(missing, "SMTP_USER")
	}
	if m.password == "" {
		missing = append(missing, "SMTP_PASS")
	}
	if strings.TrimSpace(m.sender) == "" {
		missing = append(missing, "SMTP_SENDER")
	}
	return missing
}

// send is the shared dispatch path every notification method goes through:
// dev-only console-log fallback, then a complete MIME message (text + HTML
// versions, Date and Message-ID headers; providers, Resend included, spam-folder
// mail without them), then SMTP send. logFields are extra slog key-value pairs
// for the dev-fallback log line — e.g. the actual link a developer needs to
// click, so testing a flow locally never requires fishing a URL out of HTML.
// They are never logged outside dev.
func (m *SmtpMailer) send(kind, to, subject string, body email, logFields ...any) error {
	return m.deliver(kind, outgoing{To: to, Subject: subject, Body: body}, logFields...)
}

func (m *SmtpMailer) deliver(kind string, msg outgoing, logFields ...any) error {
	if m.devFallback {
		slog.Info("----------------------------------------------------------------")
		slog.Info(kind+" LOG (Local Dev Fallback)", append([]any{"email", msg.To}, logFields...)...)
		slog.Info("----------------------------------------------------------------")
		return nil
	}
	// Belt and braces for a mailer built without the constructor: never
	// pretend to have sent, never log the message.
	if len(m.missingConfig()) > 0 {
		slog.Error("email not sent: smtp is not configured", "kind", kind)
		return fmt.Errorf("failed to send %s email: %w", kind, ErrMailNotConfigured)
	}
	msg.From = m.sender
	auth := smtp.PlainAuth("", m.username, m.password, m.host)
	addr := fmt.Sprintf("%s:%d", m.host, m.port)
	if err := smtp.SendMail(addr, auth, m.sender, []string{msg.To}, msg.build()); err != nil {
		// No recipient and no logFields here: those carry the address and,
		// for magic links and invites, a live credential URL.
		slog.Error("failed to dispatch email", "kind", kind, "error", err)
		return fmt.Errorf("failed to send %s email: %w", kind, err)
	}
	slog.Info("dispatched email", "kind", kind)
	return nil
}

func (m *SmtpMailer) SendMagicLink(ctx context.Context, address string, token string, stateID string) error {
	link := fmt.Sprintf("%s/auth/magic?token=%s&state=%s", m.appBaseURL, token, stateID)
	body := email{
		Preheader:  "Your sign-in link for Dragent. It expires in 15 minutes.",
		Heading:    "Sign in to Dragent",
		Paragraphs: []string{"Use the button below to finish signing in. The link works once and expires in 15 minutes."},
		Button:     &emailButton{URL: link, Label: "Sign in to Dragent"},
		Fallback:   true,
		Note:       "Didn't try to sign in? You can ignore this email. Nobody can sign in to your account without this link.",
		Reason:     "You're receiving this because someone asked to sign in to Dragent with this address.",
	}
	return m.send("MAGIC LINK", address, "Your Dragent sign-in link", body, "url", link)
}

// SendWelcome is the one-time email sent right after registration completes.
// Best-effort by design: called after the registration transaction commits,
// never inside it — a slow or failing send must not roll back or delay a
// signup that has already succeeded.
func (m *SmtpMailer) SendWelcome(ctx context.Context, address, name string) error {
	greeting := "Hi there,"
	if name != "" {
		greeting = "Hi " + name + ","
	}
	body := email{
		Preheader: "Your Dragent account is ready.",
		Heading:   "Welcome to Dragent",
		Paragraphs: []string{
			greeting,
			"Your account is ready. Dragent is the terminal built for agents: run Claude Code, Codex and the rest side by side, each in its own workspace, and follow them from your phone.",
			"Questions or ideas? Reply to this email. It reaches the team directly.",
		},
		Button: &emailButton{URL: brandSiteURL + "/docs", Label: "Read the guide"},
		Reason: "You're receiving this because you created a Dragent account.",
	}
	return m.send("WELCOME", address, "Welcome to Dragent", body, "name", name)
}

// SendOrganizationInvitation emails an invite to join an organization. The
// invitee need not already have a dragent account — acceptURL carries them
// through sign-in/sign-up first if not.
func (m *SmtpMailer) SendOrganizationInvitation(ctx context.Context, address, orgName, inviterName, role, acceptURL string, expiresAt time.Time) error {
	roleLabel := "a member"
	if role == "admin" {
		roleLabel = "an admin"
	}
	body := email{
		Preheader:  fmt.Sprintf("%s invited you to %s on Dragent.", inviterName, orgName),
		Heading:    fmt.Sprintf("Join %s on Dragent", orgName),
		Paragraphs: []string{fmt.Sprintf("%s invited you to join %s as %s.", inviterName, orgName, roleLabel)},
		Button:     &emailButton{URL: acceptURL, Label: "Accept invitation"},
		Fallback:   true,
		Note:       fmt.Sprintf("This invitation expires on %s. Not expecting it? You can ignore this email.", expiresAt.Format("January 2, 2006")),
		Reason:     "You're receiving this because a Dragent organization invited this address.",
	}
	return m.send("ORGANIZATION INVITATION", address, fmt.Sprintf("You've been invited to join %s on Dragent", orgName), body,
		"org", orgName, "inviter", inviterName, "role", role, "url", acceptURL)
}

// SendMemberAdded confirms to a new member that they've joined an
// organization — sent when an invitation is accepted.
func (m *SmtpMailer) SendMemberAdded(ctx context.Context, address, memberName, orgName, role, addedByName string) error {
	body := email{
		Preheader:  fmt.Sprintf("You're now part of %s.", orgName),
		Heading:    "You're in",
		Paragraphs: []string{fmt.Sprintf("Hi %s,", memberName), fmt.Sprintf("%s added you to %s as %s.", addedByName, orgName, role)},
		Button:     &emailButton{URL: m.appBaseURL + "/dashboard", Label: "Open Dragent"},
		Reason:     "You're receiving this because you joined a Dragent organization.",
	}
	return m.send("MEMBER ADDED", address, fmt.Sprintf("You've joined %s on Dragent", orgName), body,
		"org", orgName, "role", role, "added_by", addedByName)
}

// SendMemberRemoved notifies a member they've been removed from an
// organization by someone else — not sent on a voluntary self-leave, where
// the "removed by" name would just be the recipient's own.
func (m *SmtpMailer) SendMemberRemoved(ctx context.Context, address, memberName, orgName, removedByName string) error {
	body := email{
		Preheader: fmt.Sprintf("You no longer have access to %s.", orgName),
		Heading:   fmt.Sprintf("You've been removed from %s", orgName),
		Paragraphs: []string{
			fmt.Sprintf("Hi %s,", memberName),
			fmt.Sprintf("%s removed you from %s on Dragent. You no longer have access to this organization's workspaces or billing.", removedByName, orgName),
		},
		Reason: "You're receiving this because your Dragent organization membership changed.",
	}
	return m.send("MEMBER REMOVED", address, fmt.Sprintf("You've been removed from %s on Dragent", orgName), body,
		"org", orgName, "removed_by", removedByName)
}

// SendSubscriptionStarted confirms an organization's new paid subscription.
// Sent to owners only.
func (m *SmtpMailer) SendSubscriptionStarted(ctx context.Context, address, orgName, planName string) error {
	body := email{
		Preheader:  fmt.Sprintf("%s is now on %s.", orgName, planName),
		Heading:    fmt.Sprintf("%s is now on %s", orgName, planName),
		Paragraphs: []string{fmt.Sprintf("Your organization's subscription is active. Every current member now has %s.", planName)},
		Button:     &emailButton{URL: m.appBaseURL + "/dashboard", Label: "Open Dragent"},
		Reason:     "You're receiving this because you own this Dragent organization.",
	}
	return m.send("SUBSCRIPTION STARTED", address, fmt.Sprintf("%s is now on %s", orgName, planName), body,
		"org", orgName, "plan", planName)
}

// SendSubscriptionCancelled notifies billing recipients (owners+admins) that
// an organization's subscription ended. dueToPaymentFailure changes the
// wording — a voluntary cancellation and a dunning-exhausted lapse need
// different next steps (nothing to do, vs. "update your payment method").
func (m *SmtpMailer) SendSubscriptionCancelled(ctx context.Context, address, orgName, planName string, dueToPaymentFailure bool) error {
	reason := "was cancelled"
	next := "You can resubscribe any time from Settings."
	if dueToPaymentFailure {
		reason = "ended because a payment could not be completed"
		next = "Update your payment method and resubscribe to restore paid access for your team."
	}
	body := email{
		Preheader:  fmt.Sprintf("%s's %s subscription %s.", orgName, planName, reason),
		Heading:    fmt.Sprintf("%s's %s subscription %s", orgName, planName, reason),
		Paragraphs: []string{"Every member has been moved back to the Free plan.", next},
		Button:     &emailButton{URL: m.appBaseURL + "/dashboard", Label: "Open Dragent"},
		Reason:     "You're receiving this because you manage billing for this Dragent organization.",
	}
	return m.send("SUBSCRIPTION CANCELLED", address, fmt.Sprintf("%s's subscription ended", orgName), body,
		"org", orgName, "plan", planName, "due_to_payment_failure", dueToPaymentFailure)
}

// SendPaymentFailed notifies billing recipients that a renewal charge
// failed. LemonSqueezy's invoice payload doesn't expose attempt-count
// metadata, so there is no escalating "final notice" variant — a known,
// deliberate simplification.
func (m *SmtpMailer) SendPaymentFailed(ctx context.Context, address, orgName string, amountCents int64, currency, invoiceURL string) error {
	amount := formatCents(amountCents, currency)
	button := &emailButton{URL: m.appBaseURL + "/dashboard", Label: "Update payment method"}
	if invoiceURL != "" {
		button = &emailButton{URL: invoiceURL, Label: "View invoice"}
	}
	body := email{
		Preheader: fmt.Sprintf("We couldn't charge %s for %s.", amount, orgName),
		Heading:   fmt.Sprintf("Payment failed for %s", orgName),
		Paragraphs: []string{
			fmt.Sprintf("We couldn't charge %s for your subscription renewal. We'll retry automatically, but please update your payment method so your team keeps paid access.", amount),
		},
		Button: button,
		Reason: "You're receiving this because you manage billing for this Dragent organization.",
	}
	return m.send("PAYMENT FAILED", address, fmt.Sprintf("Payment failed for %s", orgName), body,
		"org", orgName, "amount", amount, "invoice_url", invoiceURL)
}

// SendMemberAddedBilling notifies billing recipients (owners+admins) of the
// estimated cost impact of a new paid seat. The amounts are a list-price
// estimate (seat count × the plan's advertised per-seat price), not an
// exact prorated invoice figure — LemonSqueezy has no proration preview.
func (m *SmtpMailer) SendMemberAddedBilling(ctx context.Context, address, orgName, memberName string, newSeatCount int, perSeatPriceCents, newMonthlyTotalCents int64, currency string) error {
	total := formatCents(newMonthlyTotalCents, currency)
	body := email{
		Preheader:  fmt.Sprintf("%s now has %d seats.", orgName, newSeatCount),
		Heading:    fmt.Sprintf("A paid seat was added to %s", orgName),
		Paragraphs: []string{fmt.Sprintf("%s joined as a billed member.", memberName)},
		Details: [][2]string{
			{"Seats", fmt.Sprintf("%d", newSeatCount)},
			{"Price per seat", formatCents(perSeatPriceCents, currency)},
			{"Estimated monthly total", total},
		},
		Note:   "This is a list-price estimate. The prorated charge was made immediately; the exact amount is on your LemonSqueezy invoice.",
		Reason: "You're receiving this because you manage billing for this Dragent organization.",
	}
	return m.send("MEMBER ADDED BILLING", address, fmt.Sprintf("%s: seat added, ~%s/month", orgName, total), body,
		"org", orgName, "member", memberName, "seats", newSeatCount, "estimated_monthly_total", total)
}

// SendMemberRemovedBilling is SendMemberAddedBilling's counterpart for a
// departing paid seat.
func (m *SmtpMailer) SendMemberRemovedBilling(ctx context.Context, address, orgName, memberName string, newSeatCount int, perSeatPriceCents, newMonthlyTotalCents int64, currency string) error {
	total := formatCents(newMonthlyTotalCents, currency)
	body := email{
		Preheader:  fmt.Sprintf("%s now has %d seats.", orgName, newSeatCount),
		Heading:    fmt.Sprintf("A paid seat was removed from %s", orgName),
		Paragraphs: []string{fmt.Sprintf("%s is no longer a billed member.", memberName)},
		Details: [][2]string{
			{"Seats", fmt.Sprintf("%d", newSeatCount)},
			{"Price per seat", formatCents(perSeatPriceCents, currency)},
			{"Estimated monthly total", total},
		},
		Note:   "This is a list-price estimate. The prorated credit was applied immediately; the exact amount is on your LemonSqueezy invoice.",
		Reason: "You're receiving this because you manage billing for this Dragent organization.",
	}
	return m.send("MEMBER REMOVED BILLING", address, fmt.Sprintf("%s: seat removed, ~%s/month", orgName, total), body,
		"org", orgName, "member", memberName, "seats", newSeatCount, "estimated_monthly_total", total)
}

// SendRenewalUpcoming warns billing recipients (owners+admins) of an
// upcoming ANNUAL-plan renewal charge (a reminder before every monthly
// renewal would be spam). amountCents is seats × list price, the same
// estimate basis as the seat emails.
func (m *SmtpMailer) SendRenewalUpcoming(ctx context.Context, address, orgName, planName string, renewsAt time.Time, amountCents int64, currency string, seatCount int) error {
	date := renewsAt.Format("January 2, 2006")
	body := email{
		Preheader:  fmt.Sprintf("%s renews on %s.", orgName, date),
		Heading:    fmt.Sprintf("%s renews on %s", orgName, date),
		Paragraphs: []string{"Your organization's annual subscription is about to renew."},
		Details: [][2]string{
			{"Plan", planName},
			{"Seats", fmt.Sprintf("%d", seatCount)},
			{"Estimated charge", formatCents(amountCents, currency)},
		},
		Button: &emailButton{URL: m.appBaseURL + "/dashboard", Label: "Manage billing"},
		Note:   "This is a list-price estimate. The exact amount will be on your LemonSqueezy invoice.",
		Reason: "You're receiving this because you manage billing for this Dragent organization.",
	}
	return m.send("RENEWAL UPCOMING", address, fmt.Sprintf("%s renews on %s", orgName, date), body,
		"org", orgName, "plan", planName, "renews_at", renewsAt.Format(time.RFC3339), "estimated_amount", formatCents(amountCents, currency))
}

// formatCents renders a smallest-currency-unit amount for email copy — good
// enough for USD-style two-decimal currencies, which is all any configured
// plan uses today.
func formatCents(cents int64, currency string) string {
	if currency == "" {
		currency = "USD"
	}
	return fmt.Sprintf("%s %.2f", currency, float64(cents)/100)
}

// SendSessionInvite emails a role-scoped invite link for a shared terminal
// session. access is "control" (Edit) or "view_only" (View) and only changes the
// copy — the capability itself is baked into joinURL.
func (m *SmtpMailer) SendSessionInvite(ctx context.Context, address, joinURL, access string) error {
	role := "view"
	perms := "You can watch this terminal session live."
	if access == "control" {
		role = "control"
		perms = "You can watch and run commands in this terminal session."
	}
	body := email{
		Preheader:  "You've been invited to a Dragent terminal session.",
		Heading:    "Join a terminal session",
		Paragraphs: []string{"You've been invited to a shared Dragent session. " + perms},
		Button:     &emailButton{URL: joinURL, Label: "Open session"},
		Fallback:   true,
		Note:       "This link expires in 24 hours. Anyone with it can join, so don't forward it.",
		Reason:     "You're receiving this because someone shared a Dragent session with this address.",
	}
	return m.send("SESSION INVITE", address, "You've been invited to a Dragent terminal session", body, "role", role, "url", joinURL)
}

var _ Mailer = (*SmtpMailer)(nil)

// feedbackFields are the details the desktop appends to feedback, one
// "Name: value" line each, after the person's own words.
var feedbackFields = map[string]bool{
	"Reply-to": true, "Account": true, "Organisation": true,
	"App version": true, "OS": true, "Logs": true,
}

// splitFeedback separates the person's words from the details block the
// app appends after them.
func splitFeedback(content string) (message string, details [][2]string) {
	content = strings.TrimSpace(content)
	cut := strings.LastIndex(content, "\n\n")
	if cut < 0 {
		return content, nil
	}
	for _, line := range strings.Split(content[cut+2:], "\n") {
		name, value, ok := strings.Cut(line, ": ")
		if !ok || !feedbackFields[name] {
			return content, nil
		}
		if name == "Logs" && value == "included (redacted)" {
			value = "Attached, with secrets and emails removed"
		}
		details = append(details, [2]string{name, value})
	}
	return strings.TrimSpace(content[:cut]), details
}

// SendFeedbackNotification forwards an in-app feedback submission to the
// team inbox with its files attached. Replying goes to the person: their
// contact address if they gave one, else their account email.
func (m *SmtpMailer) SendFeedbackNotification(ctx context.Context, to, fromEmail, content string, attachments []MailAttachment) error {
	message, details := splitFeedback(content)
	replyTo := canonicalAddress(fromEmail)
	for _, row := range details {
		if row[0] == "Reply-to" {
			if address := canonicalAddress(row[1]); address != "" {
				replyTo = address
			}
		}
	}
	names := make([]string, 0, len(attachments))
	for _, a := range attachments {
		names = append(names, a.Filename)
	}
	if message == "" {
		message = "(no message)"
	}
	preview := strings.Join(strings.Fields(message), " ")
	if len([]rune(preview)) > 60 {
		preview = string([]rune(preview)[:60]) + "…"
	}
	note := "Reply to this email to answer them directly."
	if replyTo == "" {
		note = "They didn't leave a contact address."
	}
	body := email{
		Preheader:   preview,
		Heading:     "New feedback",
		Quote:       message,
		QuoteAuthor: fromEmail,
		Details:     details,
		Files:       names,
		Note:        note,
		Reason:      "Sent from Dragent's Give feedback form.",
	}
	return m.deliver("FEEDBACK", outgoing{
		To:          to,
		ReplyTo:     replyTo,
		Subject:     "Feedback: " + preview,
		Body:        body,
		Attachments: attachments,
	}, "from", fromEmail, "content", content, "files", names)
}
