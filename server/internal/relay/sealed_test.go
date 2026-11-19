// Sealed catalog test helpers (§6.5): build valid opaque blobs and drive
// the PUT/GET catalog endpoints. Desktops are fixed canonical UUIDs;
// bodies are exactly one 4 KiB bucket of random-looking bytes, wraps are
// opaque, and the nonce is 24 bytes — all base64url encoded.
package relay

import (
	"bytes"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

const (
	sealedTestDesktop  = "01234567-89ab-4cde-8f01-23456789abcd"
	sealedTestDesktop2 = "33333333-4444-4555-8666-777777777777"
	sealedTestPhone    = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
	sealedTestPhone2   = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"
)

func sealedB64(t *testing.T, n int) string {
	t.Helper()
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	return base64.URLEncoding.EncodeToString(b)
}

// makeSealedBlob returns a valid sealed blob for desktop/version wrapped to
// phones. Every size is exact: 24-byte nonce, one 4 KiB body bucket.
func makeSealedBlob(t *testing.T, desktop string, version uint64, phones ...string) SealedCatalog {
	t.Helper()
	if len(phones) == 0 {
		phones = []string{sealedTestPhone}
	}
	recipients := make([]SealedRecipient, 0, len(phones))
	for _, p := range phones {
		recipients = append(recipients, SealedRecipient{Phone: p, Wrap: sealedB64(t, 136)})
	}
	blob := SealedCatalog{
		V:          1,
		Desktop:    desktop,
		Version:    version,
		Recipients: recipients,
		Nonce:      sealedB64(t, sealedNonceBytes),
		Body:       sealedB64(t, sealedBodyBucket),
	}.Normalized()
	if err := blob.Validate(); err != nil {
		t.Fatalf("test blob invalid: %v", err)
	}
	return blob
}

// sealedPutBody flattens device_id alongside the blob fields, the PUT shape.
func sealedPutBody(device string, blob SealedCatalog) map[string]any {
	recipients := make([]any, 0, len(blob.Recipients))
	for _, r := range blob.Recipients {
		recipients = append(recipients, map[string]any{"phone": r.Phone, "wrap": r.Wrap})
	}
	return map[string]any{
		"device_id":  device,
		"v":          blob.V,
		"desktop":    blob.Desktop,
		"version":    blob.Version,
		"recipients": recipients,
		"nonce":      blob.Nonce,
		"body":       blob.Body,
	}
}

// publishSealed PUTs one sealed blob as user/device, returning the status.
func publishSealed(t *testing.T, srv *httptest.Server, user, device string, blob SealedCatalog) int {
	t.Helper()
	raw, _ := json.Marshal(sealedPutBody(device, blob))
	req, _ := http.NewRequest(http.MethodPut, srv.URL+"/v1/relay/catalog", bytes.NewReader(raw))
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, resp.Body)
	return resp.StatusCode
}

// readSealed GETs one sealed blob, returning status and body bytes.
func readSealed(t *testing.T, srv *httptest.Server, user, device string) (int, []byte) {
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/v1/relay/catalog?device_id="+device, nil)
	if user != "" {
		req.Header.Set("X-Test-User", user)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, raw
}

// decodeSealed decodes a GET body into a blob.
func decodeSealed(t *testing.T, raw []byte) SealedCatalog {
	t.Helper()
	var blob SealedCatalog
	if err := json.Unmarshal(raw, &blob); err != nil {
		t.Fatalf("sealed body is not a blob: %s", raw)
	}
	return blob
}
