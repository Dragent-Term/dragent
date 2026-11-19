package providers

import (
	"context"
	"errors"
	"testing"
)

type fallbackTestProvider struct {
	err   error
	calls int
}

func (p *fallbackTestProvider) StreamStep(context.Context, StepRequest) (<-chan StreamEvent, error) {
	p.calls++
	if p.err != nil {
		return nil, p.err
	}
	ch := make(chan StreamEvent)
	close(ch)
	return ch, nil
}
func (*fallbackTestProvider) SupportedTools() []ToolDefinition   { return nil }
func (*fallbackTestProvider) ProviderName() string               { return "test" }
func (*fallbackTestProvider) Capabilities() ProviderCapabilities { return ProviderCapabilities{} }

func TestBYOKFallbackAuthorizesBeforePlatformSubmission(t *testing.T) {
	primary := &fallbackTestProvider{err: errors.New("401 invalid api key")}
	platform := &fallbackTestProvider{}
	provider := NewBYOKFallbackProvider(primary, platform)
	provider.SetBeforeFallback(func() error { return errors.New("credits unavailable") })
	if _, err := provider.StreamStep(context.Background(), StepRequest{}); err == nil || err.Error() != "credits unavailable" {
		t.Fatalf("error=%v, want authorization failure", err)
	}
	if platform.calls != 0 {
		t.Fatalf("platform provider calls=%d, want 0", platform.calls)
	}
	if provider.UsedFallback() {
		t.Fatal("fallback must not activate when authorization fails")
	}
}
