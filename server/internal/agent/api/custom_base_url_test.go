package api

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

func TestCustomBaseURLRejectedWith400(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	handler := NewHandler(nil, nil)
	router := chi.NewRouter()
	handler.RegisterRoutes(router)
	ctx := context.WithValue(context.Background(), "user_id", uuid.NewString())

	for _, raw := range []string{"http://169.254.169.254/latest", "https://user:hunter2@example.com/v1", "file:///etc/passwd"} {
		for _, path := range []string{"/api/v1/agent/tasks", "/api/v1/agent/tasks/" + uuid.NewString() + "/resume"} {
			req := httptest.NewRequest(http.MethodPost, path, strings.NewReader(`{"query":"hello"}`)).WithContext(ctx)
			req.Header.Set("X-Custom-Base-Url", raw)
			rec := httptest.NewRecorder()
			router.ServeHTTP(rec, req)
			if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "base URL") {
				t.Errorf("%s with %q: status = %d, want 400; body=%s", path, raw, rec.Code, rec.Body.String())
			}
			if strings.Contains(rec.Body.String(), "hunter2") {
				t.Errorf("%s: response leaks URL credentials: %s", path, rec.Body.String())
			}
		}
	}
}
