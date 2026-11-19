package model

import (
	"bytes"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"html"
	"mime"
	"mime/quotedprintable"
	"net/textproto"
	"strings"
	"time"
)

// Brand values shared by every email. The icon is served by the marketing
// site; mail clients block SVG and data URIs, so it is a hosted PNG.
const (
	brandIconURL = "https://dragent.dev/dragent-icon.png"
	brandSiteURL = "https://dragent.dev"
	// Text-safe brand blue: white on it passes WCAG AA, unlike the lighter
	// accent used on dark surfaces.
	brandButton = "#1279b8"
	inkStrong   = "#0b1220"
	inkBody     = "#3b4453"
	inkMuted    = "#6b7482"
	lineColor   = "#e6e9ee"
	pageColor   = "#f3f5f8"
	panelColor  = "#f6f8fa"
)

// MailAttachment is a file sent with an email.
type MailAttachment struct {
	Filename string
	MimeType string
	Data     []byte
}

// email is one message's content. It renders to HTML and to a plain-text
// part with the same words, so every client gets a readable version.
type email struct {
	// Preheader is the grey preview line inbox lists show after the subject.
	Preheader string
	Heading   string
	// Paragraphs are plain text; they are escaped when rendered.
	Paragraphs []string
	// Quote is a block of the sender's own words, shown as written.
	Quote       string
	QuoteAuthor string
	Button      *emailButton
	// Fallback shows the button's link in full for clients that hide buttons.
	Fallback bool
	Details  [][2]string
	Files    []string
	// Note is the small print under the main content.
	Note string
	// Reason says why the person got this email; shown in the footer.
	Reason string
}

type emailButton struct {
	URL, Label string
}

func (e email) html() string {
	var b strings.Builder
	esc := html.EscapeString
	font := `-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif`
	mono := `ui-monospace,SFMono-Regular,Menlo,Consolas,monospace`

	b.WriteString(`<!doctype html><html lang="en"><head><meta charset="utf-8">`)
	b.WriteString(`<meta name="viewport" content="width=device-width,initial-scale=1">`)
	b.WriteString(`<meta name="color-scheme" content="light only"><meta name="supported-color-schemes" content="light only">`)
	fmt.Fprintf(&b, `<title>%s</title></head>`, esc(e.Heading))
	fmt.Fprintf(&b, `<body style="margin:0;padding:0;background:%s;-webkit-text-size-adjust:100%%;">`, pageColor)
	// Hidden preview text, padded so the client doesn't pull body text in after it.
	fmt.Fprintf(&b, `<div style="display:none;max-height:0;overflow:hidden;opacity:0;mso-hide:all;">%s%s</div>`,
		esc(e.Preheader), strings.Repeat("&#8199;&#65279;&#847; ", 40))

	fmt.Fprintf(&b, `<table role="presentation" width="100%%" cellpadding="0" cellspacing="0" border="0" style="background:%s;"><tr><td align="center" style="padding:40px 16px;">`, pageColor)
	b.WriteString(`<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:560px;">`)

	// Brand row.
	fmt.Fprintf(&b, `<tr><td style="padding:0 4px 20px 4px;"><a href="%s" style="text-decoration:none;">`+
		`<img src="%s" width="32" height="32" alt="" style="display:inline-block;vertical-align:middle;border:0;border-radius:8px;">`+
		`<span style="display:inline-block;vertical-align:middle;margin-left:10px;font-family:%s;font-size:17px;font-weight:700;letter-spacing:-0.2px;color:%s;">Dragent</span></a></td></tr>`,
		brandSiteURL, brandIconURL, font, inkStrong)

	// Card.
	fmt.Fprintf(&b, `<tr><td style="background:#ffffff;border:1px solid %s;border-radius:16px;padding:36px 36px 32px 36px;font-family:%s;">`, lineColor, font)
	fmt.Fprintf(&b, `<div style="height:4px;width:40px;border-radius:2px;background:%s;margin:0 0 24px 0;font-size:0;line-height:0;">&nbsp;</div>`, brandButton)
	fmt.Fprintf(&b, `<h1 style="margin:0 0 16px 0;font-size:24px;line-height:1.3;font-weight:700;letter-spacing:-0.3px;color:%s;">%s</h1>`, inkStrong, esc(e.Heading))
	for _, p := range e.Paragraphs {
		fmt.Fprintf(&b, `<p style="margin:0 0 16px 0;font-size:15px;line-height:1.65;color:%s;">%s</p>`, inkBody, esc(p))
	}

	if e.Quote != "" {
		fmt.Fprintf(&b, `<div style="margin:8px 0 24px 0;padding:16px 20px;background:%s;border-left:3px solid %s;border-radius:0 10px 10px 0;">`, panelColor, brandButton)
		if e.QuoteAuthor != "" {
			fmt.Fprintf(&b, `<div style="margin:0 0 8px 0;font-size:12px;font-weight:600;letter-spacing:0.4px;text-transform:uppercase;color:%s;">%s</div>`, inkMuted, esc(e.QuoteAuthor))
		}
		fmt.Fprintf(&b, `<div style="font-size:15px;line-height:1.65;color:%s;white-space:pre-wrap;">%s</div></div>`, inkStrong, esc(e.Quote))
	}

	if e.Button != nil {
		fmt.Fprintf(&b, `<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:8px 0 24px 0;"><tr>`+
			`<td style="border-radius:10px;background:%s;">`+
			`<a href="%s" style="display:inline-block;padding:13px 26px;font-family:%s;font-size:15px;font-weight:600;color:#ffffff;text-decoration:none;border-radius:10px;">%s&nbsp;&rarr;</a>`+
			`</td></tr></table>`, brandButton, esc(e.Button.URL), font, esc(e.Button.Label))
		if e.Fallback {
			fmt.Fprintf(&b, `<p style="margin:0 0 8px 0;font-size:13px;line-height:1.5;color:%s;">Button not working? Paste this link into your browser:</p>`, inkMuted)
			fmt.Fprintf(&b, `<div style="margin:0 0 24px 0;padding:12px 14px;background:%s;border:1px solid %s;border-radius:8px;font-family:%s;font-size:12px;line-height:1.5;word-break:break-all;"><a href="%s" style="color:%s;text-decoration:none;">%s</a></div>`,
				panelColor, lineColor, mono, esc(e.Button.URL), brandButton, esc(e.Button.URL))
		}
	}

	if len(e.Details) > 0 {
		fmt.Fprintf(&b, `<table role="presentation" width="100%%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 24px 0;border:1px solid %s;border-radius:10px;border-collapse:separate;">`, lineColor)
		for i, row := range e.Details {
			border := ""
			if i > 0 {
				border = "border-top:1px solid " + lineColor + ";"
			}
			fmt.Fprintf(&b, `<tr><td style="%spadding:10px 14px;width:34%%;font-size:13px;color:%s;vertical-align:top;">%s</td>`+
				`<td style="%spadding:10px 14px;font-size:13px;color:%s;word-break:break-word;vertical-align:top;">%s</td></tr>`,
				border, inkMuted, esc(row[0]), border, inkStrong, esc(row[1]))
		}
		b.WriteString(`</table>`)
	}

	if len(e.Files) > 0 {
		fmt.Fprintf(&b, `<div style="margin:0 0 8px 0;font-size:12px;font-weight:600;letter-spacing:0.4px;text-transform:uppercase;color:%s;">Attached</div><div style="margin:0 0 24px 0;">`, inkMuted)
		for _, name := range e.Files {
			fmt.Fprintf(&b, `<span style="display:inline-block;margin:0 6px 6px 0;padding:6px 10px;background:%s;border:1px solid %s;border-radius:6px;font-family:%s;font-size:12px;color:%s;">%s</span>`,
				panelColor, lineColor, mono, inkStrong, esc(name))
		}
		b.WriteString(`</div>`)
	}

	if e.Note != "" {
		fmt.Fprintf(&b, `<p style="margin:8px 0 0 0;padding-top:20px;border-top:1px solid %s;font-size:13px;line-height:1.6;color:%s;">%s</p>`, lineColor, inkMuted, esc(e.Note))
	}
	b.WriteString(`</td></tr>`)

	// Footer.
	fmt.Fprintf(&b, `<tr><td style="padding:24px 4px 0 4px;font-family:%s;font-size:12px;line-height:1.6;color:%s;">`, font, inkMuted)
	if e.Reason != "" {
		fmt.Fprintf(&b, `%s<br>`, esc(e.Reason))
	}
	fmt.Fprintf(&b, `<a href="%s" style="color:%s;text-decoration:none;">dragent.dev</a> &middot; The terminal built for agents</td></tr>`, brandSiteURL, inkMuted)

	b.WriteString(`</table></td></tr></table></body></html>`)
	return b.String()
}

func (e email) text() string {
	var b strings.Builder
	b.WriteString(e.Heading + "\n\n")
	for _, p := range e.Paragraphs {
		b.WriteString(p + "\n\n")
	}
	if e.Quote != "" {
		if e.QuoteAuthor != "" {
			b.WriteString(e.QuoteAuthor + ":\n")
		}
		for _, line := range strings.Split(e.Quote, "\n") {
			b.WriteString("> " + line + "\n")
		}
		b.WriteString("\n")
	}
	if e.Button != nil {
		fmt.Fprintf(&b, "%s:\n%s\n\n", e.Button.Label, e.Button.URL)
	}
	for _, row := range e.Details {
		fmt.Fprintf(&b, "%s: %s\n", row[0], row[1])
	}
	if len(e.Details) > 0 {
		b.WriteString("\n")
	}
	if len(e.Files) > 0 {
		b.WriteString("Attached: " + strings.Join(e.Files, ", ") + "\n\n")
	}
	if e.Note != "" {
		b.WriteString(e.Note + "\n\n")
	}
	b.WriteString("--\n")
	if e.Reason != "" {
		b.WriteString(e.Reason + "\n")
	}
	b.WriteString("Dragent · https://dragent.dev\n")
	return b.String()
}

// outgoing is everything needed to build one MIME message.
type outgoing struct {
	From, To, ReplyTo, Subject string
	Body                       email
	Attachments                []MailAttachment
	Date                       time.Time
}

// build renders a standards-complete message: Date and Message-ID headers
// (spam filters penalise mail without them), a text and an HTML version, and
// any attachments.
func (o outgoing) build() []byte {
	var out bytes.Buffer
	domain := "dragent.dev"
	if at := strings.LastIndex(o.From, "@"); at >= 0 {
		domain = o.From[at+1:]
	}
	header := func(k, v string) { fmt.Fprintf(&out, "%s: %s\r\n", k, v) }
	date := o.Date
	if date.IsZero() {
		date = time.Now()
	}
	header("From", fmt.Sprintf("Dragent <%s>", o.From))
	header("To", o.To)
	if o.ReplyTo != "" {
		header("Reply-To", o.ReplyTo)
	}
	header("Subject", mime.QEncoding.Encode("utf-8", o.Subject))
	header("Date", date.Format(time.RFC1123Z))
	header("Message-ID", fmt.Sprintf("<%s@%s>", randomID(), domain))
	header("MIME-Version", "1.0")

	alt := boundary()
	if len(o.Attachments) == 0 {
		header("Content-Type", fmt.Sprintf(`multipart/alternative; boundary="%s"`, alt))
		out.WriteString("\r\n")
		writeAlternative(&out, alt, o.Body)
		return out.Bytes()
	}
	mixed := boundary()
	header("Content-Type", fmt.Sprintf(`multipart/mixed; boundary="%s"`, mixed))
	out.WriteString("\r\n")
	fmt.Fprintf(&out, "--%s\r\nContent-Type: multipart/alternative; boundary=\"%s\"\r\n\r\n", mixed, alt)
	writeAlternative(&out, alt, o.Body)
	for _, a := range o.Attachments {
		kind := a.MimeType
		if kind == "" {
			kind = "application/octet-stream"
		}
		name := mime.QEncoding.Encode("utf-8", strings.NewReplacer(`"`, "", "\r", "", "\n", "").Replace(a.Filename))
		fmt.Fprintf(&out, "--%s\r\nContent-Type: %s; name=\"%s\"\r\nContent-Disposition: attachment; filename=\"%s\"\r\nContent-Transfer-Encoding: base64\r\n\r\n",
			mixed, kind, name, name)
		writeBase64Lines(&out, a.Data)
	}
	fmt.Fprintf(&out, "--%s--\r\n", mixed)
	return out.Bytes()
}

func writeAlternative(out *bytes.Buffer, b string, body email) {
	part := func(kind, content string) {
		fmt.Fprintf(out, "--%s\r\nContent-Type: %s; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n", b, kind)
		w := quotedprintable.NewWriter(out)
		_, _ = w.Write([]byte(content))
		_ = w.Close()
		out.WriteString("\r\n")
	}
	part("text/plain", body.text())
	part("text/html", body.html())
	fmt.Fprintf(out, "--%s--\r\n", b)
}

func writeBase64Lines(out *bytes.Buffer, data []byte) {
	encoded := base64.StdEncoding.EncodeToString(data)
	for len(encoded) > 76 {
		out.WriteString(encoded[:76] + "\r\n")
		encoded = encoded[76:]
	}
	out.WriteString(encoded + "\r\n")
}

func boundary() string { return "dragent-" + randomID() }

func randomID() string {
	var buf [16]byte
	_, _ = rand.Read(buf[:])
	return hex.EncodeToString(buf[:])
}

// canonicalAddress drops anything that could inject a header line.
func canonicalAddress(s string) string {
	s = strings.TrimSpace(strings.NewReplacer("\r", "", "\n", "").Replace(s))
	if s == "" || !strings.Contains(s, "@") {
		return ""
	}
	return textproto.TrimString(s)
}
