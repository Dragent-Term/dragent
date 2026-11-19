// Package feedback accepts "Give feedback" submissions from the desktop app:
// free text plus optional screenshots and a redacted diagnostic log. Every
// submission is stored, then the team is emailed a copy.
package feedback

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"mime"
	"net/http"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	identityModel "server/internal/identity/model"
)

const (
	// Limits mirror the desktop form: ten files, 8 MB in total.
	MaxFiles        = 10
	MaxPayloadBytes = 8 << 20
	MaxContentChars = 20_000
	// Submissions per user per hour, so a stuck client can't flood the inbox.
	MaxPerHour = 20
)

// Attachment is one uploaded file.
type Attachment struct {
	Filename string
	MimeType string
	Data     []byte
}

// Submission is a validated feedback request.
type Submission struct {
	UserID         uuid.UUID
	OrganizationID *uuid.UUID
	Content        string
	Attachments    []Attachment
}

type Repository interface {
	ActiveOrganization(ctx context.Context, userID uuid.UUID) (*uuid.UUID, error)
	CountSince(ctx context.Context, userID uuid.UUID, since time.Time) (int, error)
	Save(ctx context.Context, s Submission) (uuid.UUID, error)
}

type UserLookup interface {
	GetByID(ctx context.Context, id uuid.UUID) (*identityModel.User, error)
}

// Mailer notifies the team; nil-safe (the submission is stored regardless).
type Mailer interface {
	SendFeedbackNotification(ctx context.Context, to, fromEmail, content string, attachments []identityModel.MailAttachment) error
}

type Handler struct {
	repo   Repository
	users  UserLookup
	mailer Mailer
	// Inbox that receives the notification copy.
	inbox string
}

func NewHandler(repo Repository, users UserLookup, mailer Mailer, inbox string) *Handler {
	return &Handler{repo: repo, users: users, mailer: mailer, inbox: inbox}
}

func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Post("/api/v1/feedback", h.submit)
}

func (h *Handler) submit(w http.ResponseWriter, r *http.Request) {
	userID := userIDFromContext(r)
	if userID == uuid.Nil {
		writeError(w, http.StatusUnauthorized, "authentication required")
		return
	}
	// A little headroom over the payload limit for multipart framing.
	r.Body = http.MaxBytesReader(w, r.Body, MaxPayloadBytes+(1<<20))
	content, attachments, err := ParseMultipart(r)
	if err != nil {
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			writeError(w, http.StatusRequestEntityTooLarge, "attachments add up to more than 8 MB")
			return
		}
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	ctx := r.Context()
	recent, err := h.repo.CountSince(ctx, userID, time.Now().Add(-time.Hour))
	if err != nil {
		slog.Error("feedback rate check failed", "user_id", userID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not send feedback")
		return
	}
	if recent >= MaxPerHour {
		writeError(w, http.StatusTooManyRequests, "feedback limit reached for now; please wait an hour and retry")
		return
	}
	org, err := h.repo.ActiveOrganization(ctx, userID)
	if err != nil {
		slog.Warn("feedback: active organization lookup failed", "user_id", userID, "error", err)
		org = nil
	}
	id, err := h.repo.Save(ctx, Submission{
		UserID:         userID,
		OrganizationID: org,
		Content:        content,
		Attachments:    attachments,
	})
	if err != nil {
		slog.Error("feedback save failed", "user_id", userID, "error", err)
		writeError(w, http.StatusInternalServerError, "could not send feedback")
		return
	}

	if h.mailer != nil && h.inbox != "" {
		from := userID.String()
		if user, err := h.users.GetByID(ctx, userID); err == nil && user != nil && user.Email != "" {
			from = user.Email
		}
		files := make([]identityModel.MailAttachment, 0, len(attachments))
		for _, a := range attachments {
			files = append(files, identityModel.MailAttachment{Filename: a.Filename, MimeType: a.MimeType, Data: a.Data})
		}
		// The submission is already stored; a mail failure is only logged.
		if err := h.mailer.SendFeedbackNotification(context.WithoutCancel(ctx), h.inbox, from, content, files); err != nil {
			slog.Warn("feedback notification failed", "feedback_id", id, "error", err)
		}
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(w).Encode(map[string]string{"id": id.String()})
}

// ParseMultipart reads `content` and up to MaxFiles attached files. Only
// images and plain-text logs are accepted.
func ParseMultipart(r *http.Request) (string, []Attachment, error) {
	reader, err := r.MultipartReader()
	if err != nil {
		return "", nil, fmt.Errorf("expected a multipart form")
	}
	var content string
	var attachments []Attachment
	total := 0
	for {
		part, err := reader.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			return "", nil, err
		}
		name := part.FormName()
		if part.FileName() == "" {
			if name == "content" {
				data, err := io.ReadAll(io.LimitReader(part, MaxContentChars*4+1))
				if err != nil {
					return "", nil, err
				}
				content = string(data)
			}
			continue
		}
		if len(attachments) >= MaxFiles {
			return "", nil, fmt.Errorf("no more than %d files can be attached", MaxFiles)
		}
		data, err := io.ReadAll(io.LimitReader(part, MaxPayloadBytes-int64(total)+1))
		if err != nil {
			return "", nil, err
		}
		total += len(data)
		if total > MaxPayloadBytes {
			return "", nil, &http.MaxBytesError{Limit: MaxPayloadBytes}
		}
		filename := filepath.Base(part.FileName())
		mimeType := normalizeMime(part.Header.Get("Content-Type"), filename)
		if !allowedMime(mimeType) {
			return "", nil, fmt.Errorf("%s: only images and text logs can be attached", filename)
		}
		attachments = append(attachments, Attachment{Filename: filename, MimeType: mimeType, Data: data})
	}
	content = strings.TrimSpace(content)
	if content == "" {
		return "", nil, fmt.Errorf("feedback text is required")
	}
	if !utf8.ValidString(content) {
		return "", nil, fmt.Errorf("feedback must be valid UTF-8 text")
	}
	if utf8.RuneCountInString(content) > MaxContentChars {
		return "", nil, fmt.Errorf("feedback is too long (max %d characters)", MaxContentChars)
	}
	return content, attachments, nil
}

func normalizeMime(header, filename string) string {
	if mediaType, _, err := mime.ParseMediaType(header); err == nil && mediaType != "application/octet-stream" {
		return mediaType
	}
	if byExt := mime.TypeByExtension(strings.ToLower(filepath.Ext(filename))); byExt != "" {
		if mediaType, _, err := mime.ParseMediaType(byExt); err == nil {
			return mediaType
		}
	}
	return "application/octet-stream"
}

func allowedMime(mimeType string) bool {
	switch mimeType {
	case "image/png", "image/jpeg", "image/gif", "image/webp", "text/plain":
		return true
	}
	return false
}

// ── Postgres ─────────────────────────────────────────────────────────────────

type PostgresRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresRepository(pool *pgxpool.Pool) *PostgresRepository {
	return &PostgresRepository{pool: pool}
}

func (r *PostgresRepository) ActiveOrganization(ctx context.Context, userID uuid.UUID) (*uuid.UUID, error) {
	var org *uuid.UUID
	err := r.pool.QueryRow(ctx, `SELECT active_organization_id FROM users WHERE id = $1`, userID).Scan(&org)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	return org, err
}

func (r *PostgresRepository) CountSince(ctx context.Context, userID uuid.UUID, since time.Time) (int, error) {
	var n int
	err := r.pool.QueryRow(ctx,
		`SELECT COUNT(*) FROM feedback WHERE user_id = $1 AND created_at >= $2`, userID, since,
	).Scan(&n)
	return n, err
}

func (r *PostgresRepository) Save(ctx context.Context, s Submission) (uuid.UUID, error) {
	id := uuid.New()
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return uuid.Nil, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	if _, err := tx.Exec(ctx,
		`INSERT INTO feedback (id, user_id, organization_id, content, created_at) VALUES ($1, $2, $3, $4, NOW())`,
		id, s.UserID, s.OrganizationID, s.Content,
	); err != nil {
		return uuid.Nil, fmt.Errorf("insert feedback: %w", err)
	}
	for _, a := range s.Attachments {
		if _, err := tx.Exec(ctx,
			`INSERT INTO feedback_attachments (id, feedback_id, filename, mime_type, size_bytes, data) VALUES ($1, $2, $3, $4, $5, $6)`,
			uuid.New(), id, a.Filename, a.MimeType, len(a.Data), a.Data,
		); err != nil {
			return uuid.Nil, fmt.Errorf("insert feedback attachment: %w", err)
		}
	}
	if err := tx.Commit(ctx); err != nil {
		return uuid.Nil, err
	}
	return id, nil
}

// ── Helpers ──────────────────────────────────────────────────────────────────

func userIDFromContext(r *http.Request) uuid.UUID {
	if s, ok := r.Context().Value("user_id").(string); ok && s != "" {
		if id, err := uuid.Parse(s); err == nil {
			return id
		}
	}
	return uuid.Nil
}

func writeError(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": msg})
}
