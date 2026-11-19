package api

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

func TestModelsCatalogMarksGPT56LunaAsFreeTier(t *testing.T) {
	handler := NewHandler(nil, map[string]string{"OPENAI_API_KEY": "configured"})
	req := httptest.NewRequest(http.MethodGet, "/api/models", nil)
	req = req.WithContext(context.WithValue(req.Context(), "user_id", "test-user"))
	rec := httptest.NewRecorder()

	handler.availableModels(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body=%s", rec.Code, http.StatusOK, rec.Body.String())
	}

	var response ModelsResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &response); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	freeLabels := map[string]bool{}
	for _, provider := range response.Providers {
		for _, model := range provider.Models {
			if strings.Contains(model.Name, "(Free tier)") {
				freeLabels[model.ID] = true
			}
		}
	}
	if len(freeLabels) != 1 || !freeLabels["gpt-5.6-luna"] {
		t.Fatalf("free-tier labels = %v, want only gpt-5.6-luna", freeLabels)
	}
}

func TestTaskCreationRoutesShareValidationAndAuth(t *testing.T) {
	handler := NewHandler(nil, nil)
	router := chi.NewRouter()
	handler.RegisterRoutes(router)

	for _, path := range []string{
		"/api/v1/agent/tasks",
		"/api/v1/agent/tasks/stream",
		"/api/v1/agent/steps/stream",
	} {
		t.Run(path+" rejects empty task", func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, path, strings.NewReader(`{}`))
			rec := httptest.NewRecorder()
			router.ServeHTTP(rec, req)
			if rec.Code != http.StatusBadRequest {
				t.Fatalf("status = %d, want %d; body=%s", rec.Code, http.StatusBadRequest, rec.Body.String())
			}
		})

		t.Run(path+" requires authenticated user", func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, path, strings.NewReader(`{"query":"hello"}`))
			rec := httptest.NewRecorder()
			router.ServeHTTP(rec, req)
			if rec.Code != http.StatusUnauthorized {
				t.Fatalf("status = %d, want %d; body=%s", rec.Code, http.StatusUnauthorized, rec.Body.String())
			}
		})
	}
}

func TestUnifiedDesktopStreamRejectsDurableExecution(t *testing.T) {
	handler := NewHandler(nil, nil)
	request := httptest.NewRequest(http.MethodPost, "/api/v1/agent/steps/stream", strings.NewReader(`{"query":"hello"}`))
	request = request.WithContext(context.WithValue(request.Context(), "user_id", uuid.New().String()))
	recorder := httptest.NewRecorder()

	handler.createAndStreamDesktopStep(recorder, request)

	if recorder.Code != http.StatusUpgradeRequired {
		t.Fatalf("status = %d, want %d; body=%s", recorder.Code, http.StatusUpgradeRequired, recorder.Body.String())
	}
}
