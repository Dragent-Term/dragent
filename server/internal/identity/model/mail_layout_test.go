package model

import (
	"bytes"
	"encoding/base64"
	"io"
	"mime"
	"mime/multipart"
	"net/mail"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const sampleFeedback = "The diff view is great.\n\nIt froze once <b>today</b>.\n\nReply-to: me@example.com\nAccount: account@example.com\nOrganisation: Acme (org-1)\nApp version: 1.2.3\nOS: macos aarch64\nLogs: included (redacted)"

func TestSplitFeedbackSeparatesDetails(t *testing.T) {
	message, details := splitFeedback(sampleFeedback)
	if message != "The diff view is great.\n\nIt froze once <b>today</b>." {
		t.Fatalf("message = %q", message)
	}
	if len(details) != 6 || details[0] != [2]string{"Reply-to", "me@example.com"} {
		t.Fatalf("details = %v", details)
	}
	// Without a recognised details block, everything is the message.
	if message, details := splitFeedback("Hi\n\nNote: this is mine"); message != "Hi\n\nNote: this is mine" || details != nil {
		t.Fatalf("got %q %v", message, details)
	}
}

func TestFeedbackEmailAttachesFilesAndRepliesToThePerson(t *testing.T) {
	message, details := splitFeedback(sampleFeedback)
	log := []byte("line one\nline two [redacted-key]\n")
	raw := outgoing{
		From:    "login@dragent.dev",
		To:      "support@dragent.dev",
		ReplyTo: "me@example.com",
		Subject: "Feedback: The diff view is great. — ünïcode",
		Body:    email{Heading: "New feedback", Quote: message, QuoteAuthor: "dev@example.com", Details: details, Files: []string{"dragent-diagnostics.log"}},
		Attachments: []MailAttachment{
			{Filename: "dragent-diagnostics.log", MimeType: "text/plain", Data: log},
		},
	}.build()

	msg, err := mail.ReadMessage(bytes.NewReader(raw))
	if err != nil {
		t.Fatal(err)
	}
	if got := msg.Header.Get("Reply-To"); got != "me@example.com" {
		t.Fatalf("Reply-To = %q", got)
	}
	for _, h := range []string{"Date", "Message-ID", "From", "To"} {
		if msg.Header.Get(h) == "" {
			t.Fatalf("missing %s header", h)
		}
	}
	subject, err := new(mime.WordDecoder).DecodeHeader(msg.Header.Get("Subject"))
	if err != nil || subject != "Feedback: The diff view is great. — ünïcode" {
		t.Fatalf("subject = %q (%v)", subject, err)
	}

	_, params, err := mime.ParseMediaType(msg.Header.Get("Content-Type"))
	if err != nil {
		t.Fatal(err)
	}
	reader := multipart.NewReader(msg.Body, params["boundary"])
	var sawAlternative, sawLog bool
	for {
		part, err := reader.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		kind, inner, _ := mime.ParseMediaType(part.Header.Get("Content-Type"))
		switch {
		case kind == "multipart/alternative":
			sawAlternative = true
			alt := multipart.NewReader(part, inner["boundary"])
			for {
				p, err := alt.NextPart()
				if err == io.EOF {
					break
				}
				if err != nil {
					t.Fatal(err)
				}
				body, _ := io.ReadAll(p) // NextPart decodes quoted-printable
				text := string(body)
				if strings.HasPrefix(p.Header.Get("Content-Type"), "text/html") && strings.Contains(text, "<b>today</b>") {
					t.Fatal("feedback text was not escaped in the HTML part")
				}
				if strings.HasPrefix(p.Header.Get("Content-Type"), "text/plain") && !strings.Contains(text, "> It froze once <b>today</b>.") {
					// Plain text keeps the person's words as written.
					t.Fatalf("plain text missing quote: %q", text)
				}
			}
		case part.FileName() == "dragent-diagnostics.log":
			sawLog = true
			data, _ := io.ReadAll(part)
			decoded, err := base64.StdEncoding.DecodeString(strings.NewReplacer("\r", "", "\n", "").Replace(string(data)))
			if err != nil || !bytes.Equal(decoded, log) {
				t.Fatalf("attachment = %q (%v)", decoded, err)
			}
		}
	}
	if !sawAlternative || !sawLog {
		t.Fatalf("alternative=%v log=%v", sawAlternative, sawLog)
	}
}

func TestPlainTextIsHTMLEscapedInHTMLPart(t *testing.T) {
	out := email{Heading: "A <script>", Quote: "x < y & z"}.html()
	if strings.Contains(out, "<script>") || !strings.Contains(out, "x &lt; y &amp; z") {
		t.Fatal("html part does not escape content")
	}
}

func TestCanonicalAddressRejectsHeaderInjection(t *testing.T) {
	if got := canonicalAddress("a@b.c\r\nBcc: x@y.z"); strings.ContainsAny(got, "\r\n") {
		t.Fatalf("got %q", got)
	}
	if canonicalAddress("not an address") != "" {
		t.Fatal("accepted a non-address")
	}
}

// TestWritePreviews renders every email to HTML files for a visual check:
// DRAGENT_MAIL_PREVIEW=/some/dir go test ./internal/identity/model -run Previews
func TestWritePreviews(t *testing.T) {
	dir := os.Getenv("DRAGENT_MAIL_PREVIEW")
	if dir == "" {
		t.Skip("set DRAGENT_MAIL_PREVIEW to write previews")
	}
	message, details := splitFeedback(sampleFeedback)
	previews := map[string]email{
		"magic-link": {
			Heading:    "Sign in to Dragent",
			Paragraphs: []string{"Use the button below to finish signing in. The link works once and expires in 15 minutes."},
			Button:     &emailButton{URL: "https://beta.dragent.dev/auth/magic?token=53ba3a5d0070b5a8128c80c0364ed1e319a99d2917a4322e778592464c18242d&state=64f7cbde-ee6d-46ca-9f54-6fd6896cf1e5", Label: "Sign in to Dragent"},
			Fallback:   true,
			Note:       "Didn't try to sign in? You can ignore this email. Nobody can sign in to your account without this link.",
			Reason:     "You're receiving this because someone asked to sign in to Dragent with this address.",
		},
		"feedback": {
			Heading: "New feedback", Quote: message, QuoteAuthor: "dev@example.com", Details: details,
			Files: []string{"dragent-diagnostics.log", "screenshot.png"}, Note: "Reply to this email to answer them directly.",
			Reason: "Sent from Dragent's Give feedback form.",
		},
	}
	for name, e := range previews {
		if err := os.WriteFile(filepath.Join(dir, name+".html"), []byte(e.html()), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}
