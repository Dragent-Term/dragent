package secretbox

import (
	"bytes"
	"context"
	"errors"
	"maps"
	"strings"
	"testing"

	"github.com/aws/aws-sdk-go-v2/service/kms"
)

// fakeKMS wraps data keys by XOR with a master key it never hands out, and
// checks the encryption context like KMS does.
type fakeKMS struct {
	master    byte
	decrypts  int
	generates int
}

func (f *fakeKMS) wrap(b []byte) []byte {
	out := make([]byte, len(b))
	for i := range b {
		out[i] = b[i] ^ f.master
	}
	return out
}

func (f *fakeKMS) GenerateDataKey(_ context.Context, in *kms.GenerateDataKeyInput, _ ...func(*kms.Options)) (*kms.GenerateDataKeyOutput, error) {
	f.generates++
	if !maps.Equal(in.EncryptionContext, encryptionContext) {
		return nil, errors.New("context")
	}
	key := bytes.Repeat([]byte{byte(f.generates)}, 32)
	return &kms.GenerateDataKeyOutput{Plaintext: append([]byte(nil), key...), CiphertextBlob: f.wrap(key)}, nil
}

func (f *fakeKMS) Decrypt(_ context.Context, in *kms.DecryptInput, _ ...func(*kms.Options)) (*kms.DecryptOutput, error) {
	f.decrypts++
	if !maps.Equal(in.EncryptionContext, encryptionContext) {
		return nil, errors.New("InvalidCiphertextException")
	}
	return &kms.DecryptOutput{Plaintext: f.wrap(in.CiphertextBlob)}, nil
}

func TestKMSSealsEachValueWithItsOwnDataKey(t *testing.T) {
	ctx := context.Background()
	fake := &fakeKMS{master: 0x5a}
	sealer := KMS{Client: fake, KeyID: "alias/test"}
	a, err := sealer.Seal(ctx, `{"api_key":"lin_123"}`)
	if err != nil {
		t.Fatal(err)
	}
	b, _ := sealer.Seal(ctx, `{"api_key":"lin_123"}`)
	if !strings.HasPrefix(a, kmsPrefix) || a == b {
		t.Fatalf("expected distinct kms1 values, got %q and %q", a, b)
	}
	if strings.Contains(a, "lin_123") {
		t.Fatal("plaintext visible in sealed value")
	}
	plain, err := sealer.Open(ctx, a)
	if err != nil || plain != `{"api_key":"lin_123"}` {
		t.Fatalf("open = %q, %v", plain, err)
	}
	if fake.generates != 2 || fake.decrypts != 1 {
		t.Fatalf("kms calls: %d generates, %d decrypts", fake.generates, fake.decrypts)
	}
}

func TestKMSOpensValuesSealedByThePassphrase(t *testing.T) {
	ctx := context.Background()
	old, err := Passphrase("earlier").Seal(ctx, "secret")
	if err != nil {
		t.Fatal(err)
	}
	withLegacy := KMS{Client: &fakeKMS{master: 1}, KeyID: "k", Legacy: Passphrase("earlier")}
	if got, err := withLegacy.Open(ctx, old); err != nil || got != "secret" {
		t.Fatalf("legacy open = %q, %v", got, err)
	}
	if _, err := (KMS{Client: &fakeKMS{master: 1}, KeyID: "k"}).Open(ctx, old); err == nil {
		t.Fatal("opened a passphrase value without the passphrase")
	}
}

func TestKMSRejectsTamperedValues(t *testing.T) {
	ctx := context.Background()
	sealer := KMS{Client: &fakeKMS{master: 7}, KeyID: "k"}
	sealed, _ := sealer.Seal(ctx, "secret")
	wrapped, body, _ := strings.Cut(strings.TrimPrefix(sealed, kmsPrefix), ":")
	flipped := []byte(body)
	flipped[10] ^= 1
	if _, err := sealer.Open(ctx, kmsPrefix+wrapped+":"+string(flipped)); err == nil {
		t.Fatal("tampered value opened")
	}
	if _, err := sealer.Open(ctx, kmsPrefix+"no-separator"); err == nil {
		t.Fatal("malformed value opened")
	}
}
