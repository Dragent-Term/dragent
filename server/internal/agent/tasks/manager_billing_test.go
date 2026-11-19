package tasks

import (
	"testing"

	"github.com/google/uuid"
	"server/internal/agent/providers"
)

func TestPlatformBillingActive(t *testing.T) {
	t.Run("platform request is billed", func(t *testing.T) {
		if !platformBillingActive(false, nil) {
			t.Fatal("platform-funded request must be billed")
		}
	})

	t.Run("direct BYOK without fallback is not billed", func(t *testing.T) {
		if platformBillingActive(true, nil) {
			t.Fatal("direct BYOK request must bypass platform billing")
		}
	})

	t.Run("inactive BYOK fallback is not billed", func(t *testing.T) {
		provider := providers.NewBYOKFallbackProvider(nil, nil)
		if platformBillingActive(true, provider) {
			t.Fatal("unused fallback must not be billed")
		}
	})
}

func TestAutoAlwaysUsesPlatformBilling(t *testing.T) {
	for _, modelName := range []string{"", "auto", " AUTO "} {
		req := CreateRequest{
			ModelName: modelName, UserApiKey: "sk-user", CustomBaseURL: "https://example.test/v1",
		}
		enforceAutoCredentialPolicy(&req)
		if req.ModelName != "" || req.UserApiKey != "" || req.CustomBaseURL != "" {
			t.Fatalf("Auto request retained BYOK routing: %+v", req)
		}
		if !platformBillingActive(req.UserApiKey != "" || req.CustomBaseURL != "", nil) {
			t.Fatal("Auto request must enter platform billing")
		}
	}
}

func TestExplicitModelKeepsBYOKRouting(t *testing.T) {
	req := CreateRequest{ModelName: "gpt-5.6-luna", UserApiKey: "sk-user"}
	enforceAutoCredentialPolicy(&req)
	if req.UserApiKey != "sk-user" || req.ModelName != "gpt-5.6-luna" {
		t.Fatalf("explicit model routing changed: %+v", req)
	}
}

func TestPlatformWebAllowedFollowsCredentialOwnership(t *testing.T) {
	tests := []struct {
		name            string
		hasUserProvider bool
		fallbackAllowed bool
		want            bool
	}{
		{name: "platform funded session", want: true},
		{name: "BYOK without fallback", hasUserProvider: true, want: false},
		{name: "BYOK with fallback", hasUserProvider: true, fallbackAllowed: true, want: true},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := platformWebAllowed(test.hasUserProvider, test.fallbackAllowed); got != test.want {
				t.Fatalf("platformWebAllowed() = %v, want %v", got, test.want)
			}
		})
	}
}

func TestCatalogMetadataFollowsCredentialOwnership(t *testing.T) {
	if !catalogMetadataAllowed(false) {
		t.Fatal("platform-funded models should use catalog metadata")
	}
	if catalogMetadataAllowed(true) {
		t.Fatal("BYOK/custom models must not consult the platform model catalog")
	}
}

func TestPlatformFallbackDoesNotOverrideCustomEndpoint(t *testing.T) {
	if platformFallbackAllowed(true, "https://openrouter.ai/api/v1") {
		t.Fatal("custom endpoint must remain direct BYOK when global fallback is enabled")
	}
	if !platformFallbackAllowed(true, "") {
		t.Fatal("named BYOK provider should honor enabled platform fallback")
	}
	if platformFallbackAllowed(false, "") {
		t.Fatal("disabled platform fallback must remain disabled")
	}
}

func TestDesktopBillingPositionIsStableAcrossHTTPRetries(t *testing.T) {
	userID := uuid.New()
	req := CreateRequest{
		UserID: userID, DesktopOwned: true,
		ClientRunID: "local-12345678abcdef", ClientRound: 7,
	}
	identityA, roundA, err := billingPosition(uuid.New(), req)
	if err != nil {
		t.Fatal(err)
	}
	identityB, roundB, err := billingPosition(uuid.New(), req)
	if err != nil {
		t.Fatal(err)
	}
	if identityA != identityB || roundA != roundB || roundA != 6 {
		t.Fatalf("retry billing position changed: (%q,%d) vs (%q,%d)", identityA, roundA, identityB, roundB)
	}
}

func TestDesktopBillingPositionRejectsUnsafeIdentifiers(t *testing.T) {
	for _, runID := range []string{"", "server-task", "local-../../secret", "local-UPPERCASE"} {
		_, _, err := billingPosition(uuid.New(), CreateRequest{
			UserID: uuid.New(), DesktopOwned: true, ClientRunID: runID, ClientRound: 1,
		})
		if err == nil {
			t.Fatalf("run id %q was accepted", runID)
		}
	}
}
