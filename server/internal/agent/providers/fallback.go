package providers

import (
	"context"
	"strings"
	"sync"
)

// BYOKFallbackProvider tries the primary (user-supplied key) provider first.
// On an authentication error it transparently retries the same request with
// the fallback (server-side key) provider and records that the fallback was used.
//
// This implements the header-based BYOK pattern: the client sends X-User-Api-Key
// (and optionally X-Custom-Base-Url); if those credentials fail, the server falls
// back to its own key and the billing layer charges the user's credit window.
type BYOKFallbackProvider struct {
	primary        LLMProvider
	fallback       LLMProvider
	mu             sync.Mutex
	used           bool // set true the first time fallback is triggered
	beforeFallback func() error
}

// SetBeforeFallback installs the billing authorization that must succeed
// before any request is submitted with platform credentials.
func (f *BYOKFallbackProvider) SetBeforeFallback(fn func() error) {
	f.mu.Lock()
	f.beforeFallback = fn
	f.mu.Unlock()
}

func (f *BYOKFallbackProvider) authorizeFallback() error {
	f.mu.Lock()
	fn := f.beforeFallback
	f.mu.Unlock()
	if fn != nil {
		return fn()
	}
	return nil
}

func NewBYOKFallbackProvider(primary, fallback LLMProvider) *BYOKFallbackProvider {
	return &BYOKFallbackProvider{primary: primary, fallback: fallback}
}

// UsedFallback reports whether the server-side key was used for any request.
// Billing skips charging when this returns false (user paid their own provider).
func (f *BYOKFallbackProvider) UsedFallback() bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.used
}

func (f *BYOKFallbackProvider) markFallback() {
	f.mu.Lock()
	f.used = true
	f.mu.Unlock()
}

func (f *BYOKFallbackProvider) isFallbackActive() bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.used
}

func (f *BYOKFallbackProvider) ProviderName() string {
	if f.isFallbackActive() {
		return f.fallback.ProviderName()
	}
	return f.primary.ProviderName() + "+byok"
}

func (f *BYOKFallbackProvider) SupportedTools() []ToolDefinition {
	if f.isFallbackActive() {
		return f.fallback.SupportedTools()
	}
	return f.primary.SupportedTools()
}

func (f *BYOKFallbackProvider) Capabilities() ProviderCapabilities {
	if f.isFallbackActive() {
		return f.fallback.Capabilities()
	}
	return f.primary.Capabilities()
}

// StreamStep routes to the active provider. On an auth error from the primary it
// transparently retries the entire request with the fallback provider and flips
// UsedFallback() to true so the billing layer knows to charge the user.
func (f *BYOKFallbackProvider) StreamStep(ctx context.Context, req StepRequest) (<-chan StreamEvent, error) {
	if f.isFallbackActive() {
		return f.fallback.StreamStep(ctx, req)
	}

	primaryCh, err := f.primary.StreamStep(ctx, req)
	if err != nil {
		if isAuthError(err.Error()) {
			if authErr := f.authorizeFallback(); authErr != nil {
				return nil, authErr
			}
			f.markFallback()
			return f.fallback.StreamStep(ctx, req)
		}
		return nil, err
	}

	outCh := make(chan StreamEvent, 32)
	go func() {
		for ev := range primaryCh {
			if ev.Kind == EventError && ev.Err != nil && isAuthError(ev.Err.Error()) {
				if authErr := f.authorizeFallback(); authErr != nil {
					outCh <- StreamEvent{Kind: EventError, Err: authErr}
					close(outCh)
					return
				}
				f.markFallback()
				fallbackCh, fallbackErr := f.fallback.StreamStep(ctx, req)
				if fallbackErr != nil {
					outCh <- StreamEvent{Kind: EventError, Err: fallbackErr}
					close(outCh)
					return
				}
				for fallbackEv := range fallbackCh {
					outCh <- fallbackEv
				}
				close(outCh)
				return
			}
			outCh <- ev
		}
		close(outCh)
	}()
	return outCh, nil
}

// isAuthError returns true when an error message signals an authentication failure.
// Covers the error formats used by Anthropic, OpenAI, Gemini, and xAI.
func isAuthError(msg string) bool {
	msg = strings.ToLower(msg)
	return strings.Contains(msg, "401") ||
		strings.Contains(msg, "invalid api key") ||
		strings.Contains(msg, "invalid_api_key") ||
		strings.Contains(msg, "authentication_error") ||
		strings.Contains(msg, "incorrect api key") ||
		strings.Contains(msg, "api key not valid") ||
		strings.Contains(msg, "unauthenticated") ||
		strings.Contains(msg, "unauthorized") ||
		strings.Contains(msg, "permission_denied")
}
