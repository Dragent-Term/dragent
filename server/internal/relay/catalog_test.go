// Sealed catalog validation tests (§6.5): the relay sees routing only —
// desktop id, recipient phone ids, blob version, padded size, publish
// times — and never any catalog field.
package relay

import (
	"strings"
	"testing"
)

func strPtr(s string) *string { return &s }

func TestOwnerKeyScopesByAccount(t *testing.T) {
	a, ok := ownerKey("user-a", "device-1")
	if !ok || a == "" {
		t.Fatal("expected an owner for a valid user/device")
	}
	b, ok := ownerKey("user-b", "device-1")
	if !ok || b == "" {
		t.Fatal("expected an owner for a valid user/device")
	}
	if a == b {
		t.Fatal("same device name on different accounts must resolve to different owners")
	}
	for _, tc := range []struct{ user, device string }{
		{"", "device-1"},
		{"user-a", ""},
		{"user-a", "   "},
		{"user-a", strings.Repeat("d", 129)},
	} {
		if _, ok := ownerKey(tc.user, tc.device); ok {
			t.Fatalf("ownerKey(%q, %q) should be rejected", tc.user, tc.device)
		}
	}
}

func TestSealedCatalogValidation(t *testing.T) {
	good := SealedCatalog{
		V:          1,
		Desktop:    sealedTestDesktop,
		Version:    5,
		Recipients: []SealedRecipient{{Phone: sealedTestPhone, Wrap: sealedB64(t, 136)}},
		Nonce:      sealedB64(t, sealedNonceBytes),
		Body:       sealedB64(t, sealedBodyBucket),
	}
	if err := good.Validate(); err != nil {
		t.Fatalf("valid sealed blob rejected: %v", err)
	}
	// Multiple recipients and max version are fine.
	multi := good
	multi.Version = maxSealedVersion
	multi.Recipients = append(multi.Recipients, SealedRecipient{Phone: sealedTestPhone2, Wrap: sealedB64(t, 200)})
	if err := multi.Validate(); err != nil {
		t.Fatalf("multi-recipient blob rejected: %v", err)
	}
	// A first-pairing desktop seals for nobody yet: 0 recipients is valid
	// (§6.5 fix: it still proves the desktop published, body stays opaque).
	empty := good
	empty.Recipients = nil
	if err := empty.Validate(); err != nil {
		t.Fatalf("empty-recipient blob rejected: %v", err)
	}
	emptySlice := good
	emptySlice.Recipients = []SealedRecipient{}
	if err := emptySlice.Validate(); err != nil {
		t.Fatalf("empty-slice-recipient blob rejected: %v", err)
	}

	bad := func(f func(*SealedCatalog)) SealedCatalog {
		b := good
		f(&b)
		return b
	}
	for name, blob := range map[string]SealedCatalog{
		"bad version":   bad(func(b *SealedCatalog) { b.V = 2 }),
		"zero version":  bad(func(b *SealedCatalog) { b.Version = 0 }),
		"huge version":  bad(func(b *SealedCatalog) { b.Version = maxSealedVersion + 1 }),
		"bad desktop":   bad(func(b *SealedCatalog) { b.Desktop = "not-a-uuid" }),
		"bad recipient": bad(func(b *SealedCatalog) { b.Recipients = []SealedRecipient{{Phone: "x", Wrap: sealedB64(t, 136)}} }),
		"dup recipient": bad(func(b *SealedCatalog) {
			b.Recipients = []SealedRecipient{{Phone: sealedTestPhone, Wrap: sealedB64(t, 136)}, {Phone: sealedTestPhone, Wrap: sealedB64(t, 136)}}
		}),
		"empty wrap":      bad(func(b *SealedCatalog) { b.Recipients = []SealedRecipient{{Phone: sealedTestPhone, Wrap: ""}} }),
		"bad nonce":       bad(func(b *SealedCatalog) { b.Nonce = sealedB64(t, 16) }),
		"thin body":       bad(func(b *SealedCatalog) { b.Body = sealedB64(t, 100) }),
		"unbucketed body": bad(func(b *SealedCatalog) { b.Body = sealedB64(t, 5000) }),
	} {
		if err := blob.Validate(); err == nil {
			t.Fatalf("%s accepted", name)
		}
	}
	// Too many recipients are rejected.
	many := good
	for i := 0; i < maxSealedRecipients+1; i++ {
		many.Recipients = append(many.Recipients, SealedRecipient{Phone: sealedTestPhone, Wrap: sealedB64(t, 136)})
	}
	if err := many.Validate(); err == nil {
		t.Fatal("recipient flood accepted")
	}
}

func TestSealedNormalizedTrimsRouting(t *testing.T) {
	blob := SealedCatalog{
		V:          1,
		Desktop:    "  " + sealedTestDesktop + " ",
		Version:    2,
		Recipients: []SealedRecipient{{Phone: " " + sealedTestPhone + " ", Wrap: " " + sealedB64(t, 136) + " "}},
		Nonce:      " " + sealedB64(t, sealedNonceBytes) + " ",
		Body:       " " + sealedB64(t, sealedBodyBucket) + " ",
	}.Normalized()
	if err := blob.Validate(); err != nil {
		t.Fatalf("normalized blob rejected: %v", err)
	}
	if blob.Desktop != sealedTestDesktop || blob.Recipients[0].Phone != sealedTestPhone {
		t.Fatalf("routing not trimmed: %+v", blob)
	}
}
