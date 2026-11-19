package feedback

import (
	"bytes"
	"context"
	"encoding/json"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/textproto"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	identityModel "server/internal/identity/model"
)

type fakeRepo struct {
	saved  []Submission
	recent int
}

func (f *fakeRepo) ActiveOrganization(context.Context, uuid.UUID) (*uuid.UUID, error) {
	org := uuid.New()
	return &org, nil
}

func (f *fakeRepo) CountSince(context.Context, uuid.UUID, time.Time) (int, error) {
	return f.recent, nil
}

func (f *fakeRepo) Save(_ context.Context, s Submission) (uuid.UUID, error) {
	f.saved = append(f.saved, s)
	return uuid.New(), nil
}

type fakeUsers struct{}

func (fakeUsers) GetByID(context.Context, uuid.UUID) (*identityModel.User, error) {
	return &identityModel.User{Email: "dev@example.com"}, nil
}

type fakeMailer struct {
	to, from, content string
	names             []string
}

func (m *fakeMailer) SendFeedbackNotification(_ context.Context, to, from, content string, files []identityModel.MailAttachment) error {
	m.to, m.from, m.content = to, from, content
	m.names = nil
	for _, f := range files {
		m.names = append(m.names, f.Filename)
	}
	return nil
}

type file struct {
	name, mime string
	data       []byte
}

func form(t *testing.T, content string, files ...file) (*bytes.Buffer, string) {
	t.Helper()
	var body bytes.Buffer
	mw := multipart.NewWriter(&body)
	if err := mw.WriteField("content", content); err != nil {
		t.Fatal(err)
	}
	for i, f := range files {
		header := textproto.MIMEHeader{}
		header.Set("Content-Disposition", `form-data; name="file`+string(rune('0'+i))+`"; filename="`+f.name+`"`)
		header.Set("Content-Type", f.mime)
		part, err := mw.CreatePart(header)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = part.Write(f.data)
	}
	_ = mw.Close()
	return &body, mw.FormDataContentType()
}

func request(t *testing.T, h *Handler, user string, body *bytes.Buffer, contentType string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodPost, "/api/v1/feedback", body)
	req.Header.Set("Content-Type", contentType)
	if user != "" {
		req = req.WithContext(context.WithValue(req.Context(), "user_id", user))
	}
	rec := httptest.NewRecorder()
	h.submit(rec, req)
	return rec
}

func TestSubmitStoresAndNotifies(t *testing.T) {
	repo := &fakeRepo{}
	mail := &fakeMailer{}
	h := NewHandler(repo, fakeUsers{}, mail, "support@dragent.dev")
	body, ct := form(t, "  Love it, but the diff view is slow.\n\nContact: me@example.com  ",
		file{"shot.png", "image/png", []byte{0x89, 'P', 'N', 'G'}},
		file{"dragent-diagnostics.log", "text/plain", []byte("log line")},
	)
	rec := request(t, h, uuid.NewString(), body, ct)
	if rec.Code != http.StatusCreated {
		t.Fatalf("status %d: %s", rec.Code, rec.Body)
	}
	var resp map[string]string
	_ = json.Unmarshal(rec.Body.Bytes(), &resp)
	if _, err := uuid.Parse(resp["id"]); err != nil {
		t.Fatalf("missing id: %v", resp)
	}
	if len(repo.saved) != 1 || len(repo.saved[0].Attachments) != 2 || repo.saved[0].OrganizationID == nil {
		t.Fatalf("unexpected save: %+v", repo.saved)
	}
	if !strings.HasPrefix(repo.saved[0].Content, "Love it") {
		t.Fatalf("content not trimmed: %q", repo.saved[0].Content)
	}
	if mail.to != "support@dragent.dev" || mail.from != "dev@example.com" || len(mail.names) != 2 {
		t.Fatalf("unexpected notification: %+v", mail)
	}
}

func TestSubmitRejectsBadRequests(t *testing.T) {
	h := NewHandler(&fakeRepo{}, fakeUsers{}, nil, "")
	body, ct := form(t, "hi")
	if rec := request(t, h, "", body, ct); rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous: %d", rec.Code)
	}
	body, ct = form(t, "   ")
	if rec := request(t, h, uuid.NewString(), body, ct); rec.Code != http.StatusBadRequest {
		t.Fatalf("empty: %d", rec.Code)
	}
	body, ct = form(t, "hi", file{"run.sh", "application/x-sh", []byte("rm -rf /")})
	if rec := request(t, h, uuid.NewString(), body, ct); rec.Code != http.StatusBadRequest {
		t.Fatalf("script attachment: %d", rec.Code)
	}
	files := make([]file, MaxFiles+1)
	for i := range files {
		files[i] = file{"a.png", "image/png", []byte{1}}
	}
	body, ct = form(t, "hi", files...)
	if rec := request(t, h, uuid.NewString(), body, ct); rec.Code != http.StatusBadRequest {
		t.Fatalf("too many files: %d", rec.Code)
	}
	body, ct = form(t, "hi", file{"big.png", "image/png", make([]byte, MaxPayloadBytes+1)})
	if rec := request(t, h, uuid.NewString(), body, ct); rec.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("oversized: %d", rec.Code)
	}
}

func TestSubmitIsRateLimited(t *testing.T) {
	h := NewHandler(&fakeRepo{recent: MaxPerHour}, fakeUsers{}, nil, "")
	body, ct := form(t, "again")
	if rec := request(t, h, uuid.NewString(), body, ct); rec.Code != http.StatusTooManyRequests {
		t.Fatalf("rate limit: %d", rec.Code)
	}
}
