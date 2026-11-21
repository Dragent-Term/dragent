package api

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/google/uuid"
)

const testVerifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk" // RFC 7636 appendix B

func challengeOf(v string) string {
	sum := sha256.Sum256([]byte(v))
	return base64.RawURLEncoding.EncodeToString(sum[:])
}

func post(h http.HandlerFunc, path, body string) *httptest.ResponseRecorder {
	w := httptest.NewRecorder()
	h(w, httptest.NewRequest(http.MethodPost, path, strings.NewReader(body)))
	return w
}

func TestVerifierMatchesRFC7636Example(t *testing.T) {
	if got := challengeOf(testVerifier); got != "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM" {
		t.Fatalf("challenge %s", got)
	}
	if !verifierMatches(testVerifier, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM") {
		t.Fatal("RFC example verifier refused")
	}
	if verifierMatches("x"+testVerifier[1:], "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM") {
		t.Fatal("wrong verifier accepted")
	}
	if verifierMatches("short", challengeOf("short")) {
		t.Fatal("too-short verifier accepted")
	}
}

// With a challenge, the deep link carries a one-time code, never the
// refresh token; the code is only redeemable with the verifier, once.
func TestDeepLinkCarriesACodeRedeemableOnlyWithTheVerifier(t *testing.T) {
	store := newParkedStates()
	h := NewAuthHandlers(nil, nil, nil, store, nil, nil)
	ctx := context.Background()
	if err := h.recordChallenge(ctx, "s1", challengeOf(testVerifier)); err != nil {
		t.Fatal(err)
	}

	w := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodGet, "/auth/magic", nil)
	h.finishSignIn(w, r, "browser-access", "app-refresh", "s1", uuid.New())
	link := w.Header().Get("Location")
	if strings.Contains(link, "app-refresh") || strings.Contains(w.Body.String(), "app-refresh") {
		t.Fatalf("refresh token leaked into the redirect: %s", link)
	}
	u, err := url.Parse(link)
	if err != nil || u.Query().Get("code") == "" {
		t.Fatalf("no code in %q", link)
	}
	code := u.Query().Get("code")

	if got := post(h.ExchangeDesktopHandoff, "/auth/desktop/exchange",
		`{"state":"s1","code":"`+code+`","verifier":"`+strings.Repeat("a", 43)+`"}`).Code; got != http.StatusUnauthorized {
		t.Fatalf("wrong verifier: %d, want 401", got)
	}
	ok := post(h.ExchangeDesktopHandoff, "/auth/desktop/exchange",
		`{"state":"s1","code":"`+code+`","verifier":"`+testVerifier+`"}`)
	if ok.Code != http.StatusOK {
		t.Fatalf("exchange: %d %s", ok.Code, ok.Body)
	}
	var body map[string]string
	_ = json.Unmarshal(ok.Body.Bytes(), &body)
	if body["refresh_token"] != "app-refresh" {
		t.Fatalf("exchange body %v", body)
	}
	if got := post(h.ExchangeDesktopHandoff, "/auth/desktop/exchange",
		`{"state":"s1","code":"`+code+`","verifier":"`+testVerifier+`"}`).Code; got != http.StatusUnauthorized {
		t.Fatalf("second exchange: %d, want 401", got)
	}
}

// The portal's JSON gets the code too, not the app's refresh token.
func TestPortalJSONGetsTheCodeNotTheRefreshToken(t *testing.T) {
	store := newParkedStates()
	h := NewAuthHandlers(nil, nil, nil, store, nil, nil)
	_ = h.recordChallenge(context.Background(), "s1", challengeOf(testVerifier))
	w := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodGet, "/auth/magic", nil)
	r.Header.Set("Accept", "application/json")
	h.finishSignIn(w, r, "browser-access", "app-refresh", "s1", uuid.New())
	var body map[string]string
	_ = json.Unmarshal(w.Body.Bytes(), &body)
	if body["handoff_code"] == "" || body["refresh_token"] != "" || body["access_token"] != "browser-access" {
		t.Fatalf("portal body %v", body)
	}
}

// Desktop builds from before PKCE send no challenge and keep working.
func TestSignInWithoutChallengeKeepsTheOldLink(t *testing.T) {
	h := NewAuthHandlers(nil, nil, nil, newParkedStates(), nil, nil)
	w := httptest.NewRecorder()
	h.finishSignIn(w, httptest.NewRequest(http.MethodGet, "/auth/magic", nil), "a", "app-refresh", "s1", uuid.New())
	if !strings.Contains(w.Header().Get("Location"), "refresh_token=app-refresh") {
		t.Fatalf("legacy link %q", w.Header().Get("Location"))
	}
}

// A sign-in started with a challenge is collected only with the verifier,
// after approval, once; the old GET path no longer hands it out.
func TestPollingNeedsTheVerifier(t *testing.T) {
	store := newParkedStates()
	h := NewAuthHandlers(nil, nil, nil, store, nil, nil)
	ctx := context.Background()
	_ = h.recordChallenge(ctx, "s1", challengeOf(testVerifier))
	_ = store.AuthorizeState(ctx, "s1", "u1", "a", "r")
	_ = store.ApproveState(ctx, "s1", "u1")

	if got := poll(h, "s1"); got != http.StatusNoContent {
		t.Fatalf("GET without verifier: %d, want 204", got)
	}
	if got := post(h.PollDesktopSessionWithProof, "/auth/desktop/session",
		`{"state":"s1","verifier":"`+strings.Repeat("b", 43)+`"}`).Code; got != http.StatusUnauthorized {
		t.Fatalf("wrong verifier: %d, want 401", got)
	}
	if got := post(h.PollDesktopSessionWithProof, "/auth/desktop/session",
		`{"state":"s1","verifier":"`+testVerifier+`"}`).Code; got != http.StatusOK {
		t.Fatalf("right verifier: %d, want 200", got)
	}
	if got := post(h.PollDesktopSessionWithProof, "/auth/desktop/session",
		`{"state":"s1","verifier":"`+testVerifier+`"}`).Code; got != http.StatusNoContent {
		t.Fatalf("second collect: %d, want 204", got)
	}
}

// Someone who learns a state can't swap in their own challenge.
func TestChallengeCannotBeReplaced(t *testing.T) {
	h := NewAuthHandlers(nil, nil, nil, newParkedStates(), nil, nil)
	ctx := context.Background()
	if err := h.recordChallenge(ctx, "s1", challengeOf(testVerifier)); err != nil {
		t.Fatal(err)
	}
	if err := h.recordChallenge(ctx, "s1", challengeOf(strings.Repeat("c", 43))); err == nil {
		t.Fatal("second challenge accepted")
	}
	if err := h.recordChallenge(ctx, "s2", "not-a-challenge"); err == nil {
		t.Fatal("malformed challenge accepted")
	}
}
