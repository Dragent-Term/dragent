package model

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
)

func TestAgentStepRejectsBadCustomBaseURL(t *testing.T) {
	t.Setenv("APP_ENV", "production")
	g := NewMultiLLMGatewayFromKeys(map[string]string{})
	for _, raw := range []string{"http://169.254.169.254/v1", "https://u:hunter2@example.com/v1", "file:///etc/passwd"} {
		_, err := g.AgentStep(context.Background(), AgentStepRequest{ModelName: "llama3", CustomBaseUrl: raw})
		if err == nil {
			t.Errorf("AgentStep accepted %q", raw)
		} else if strings.Contains(err.Error(), "hunter2") {
			t.Errorf("error leaks credentials: %v", err)
		}
	}
}

// A well-formed https URL that resolves to loopback passes validation but must
// be refused at dial time, before any request reaches the target.
func TestAgentStepCustomBaseURLRefusedAtDial(t *testing.T) {
	var hits atomic.Int32
	srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { hits.Add(1) }))
	defer srv.Close()
	t.Setenv("APP_ENV", "production")
	g := NewMultiLLMGatewayFromKeys(map[string]string{})
	_, err := g.AgentStep(context.Background(), AgentStepRequest{ModelName: "llama3", CustomBaseUrl: srv.URL + "/v1"})
	if err == nil || !strings.Contains(err.Error(), "not public") {
		t.Fatalf("want dial refusal, got %v", err)
	}
	if hits.Load() != 0 {
		t.Fatal("request reached the loopback server")
	}
}
