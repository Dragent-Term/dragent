package providers

import (
	"context"
	"fmt"
	"io"
	"log"
	"net/http"
	"strconv"
	"strings"
	"time"
)

const (
	maxRateLimitRetries  = 4
	defaultRateLimitWait = 30 * time.Second
	maxRateLimitWait     = 90 * time.Second
)

// doStreamRequest executes an HTTP streaming request with two layers of retry:
//
//  1. Inner: retries once on stale TLS connection errors (bad record MAC, EOF, etc.)
//  2. Outer: retries up to maxRateLimitRetries times on HTTP 429, waiting the
//     duration indicated by the Retry-After header or the JSON error message body.
//
// makeReq must create a fresh *http.Request on each call — request bodies are
// consumed on first use. Non-429 error responses are returned to the caller as-is.
// During the backoff sleep the caller's channel stays open so the client loading
// shimmer continues animating without any visible interruption.
func doStreamRequest(
	ctx context.Context,
	client *http.Client,
	makeReq func() (*http.Request, error),
	providerName string,
) (*http.Response, error) {
	for rlAttempt := 0; rlAttempt <= maxRateLimitRetries; rlAttempt++ {
		resp, err := doOnce(ctx, client, makeReq)
		if err != nil {
			return nil, err
		}
		if resp.StatusCode != http.StatusTooManyRequests {
			return resp, nil
		}

		b, _ := io.ReadAll(resp.Body)
		resp.Body.Close()

		if rlAttempt == maxRateLimitRetries {
			return nil, fmt.Errorf("%s: rate limit after %d retries: %s", providerName, maxRateLimitRetries, b)
		}

		wait := parseRetryAfter(resp, b)
		log.Printf("[%s] rate limited, retrying in %v (attempt %d/%d)", providerName, wait, rlAttempt+1, maxRateLimitRetries)

		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("%s: context cancelled during rate-limit backoff: %w", providerName, ctx.Err())
		case <-time.After(wait):
		}
	}
	// unreachable
	return nil, fmt.Errorf("%s: unexpected state in doStreamRequest", providerName)
}

// doOnce performs a single HTTP request, retrying once on stale TLS connection errors.
func doOnce(ctx context.Context, client *http.Client, makeReq func() (*http.Request, error)) (*http.Response, error) {
	var lastErr error
	for attempt := 0; attempt < 2; attempt++ {
		req, err := makeReq()
		if err != nil {
			return nil, err
		}
		resp, err := client.Do(req)
		if err != nil {
			if attempt == 0 && isStaleConnError(err) {
				lastErr = err
				continue
			}
			return nil, err
		}
		return resp, nil
	}
	return nil, lastErr
}

// parseRetryAfter extracts the wait duration from a 429 response.
// Checks the Retry-After header first (integer or decimal seconds), then
// falls back to parsing "try again in X.Xs" from the JSON error body.
func parseRetryAfter(resp *http.Response, body []byte) time.Duration {
	if ra := resp.Header.Get("Retry-After"); ra != "" {
		if secs, err := strconv.ParseFloat(strings.TrimSpace(ra), 64); err == nil && secs > 0 {
			return clampWait(time.Duration(secs * float64(time.Second)))
		}
	}

	s := string(body)
	const needle = "try again in "
	if idx := strings.Index(s, needle); idx >= 0 {
		rest := s[idx+len(needle):]
		if end := strings.IndexByte(rest, 's'); end > 0 {
			if secs, err := strconv.ParseFloat(strings.TrimSpace(rest[:end]), 64); err == nil && secs > 0 {
				return clampWait(time.Duration(secs * float64(time.Second)))
			}
		}
	}

	return defaultRateLimitWait
}

func clampWait(d time.Duration) time.Duration {
	if d > maxRateLimitWait {
		return maxRateLimitWait
	}
	return d
}
