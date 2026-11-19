package secretbox

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"strings"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/kms"
	"github.com/aws/aws-sdk-go-v2/service/kms/types"
)

// Sealer encrypts the secrets the server keeps, and opens them again.
type Sealer interface {
	Seal(ctx context.Context, plaintext string) (string, error)
	Open(ctx context.Context, sealed string) (string, error)
}

// Passphrase seals with a key derived from a passphrase held in the
// server's settings (Encrypt/Decrypt).
type Passphrase string

func (p Passphrase) Seal(_ context.Context, plaintext string) (string, error) {
	return Encrypt(plaintext, string(p))
}

func (p Passphrase) Open(_ context.Context, sealed string) (string, error) {
	return Decrypt(sealed, string(p))
}

// KMSAPI is the part of AWS KMS a KMS sealer uses.
type KMSAPI interface {
	GenerateDataKey(ctx context.Context, in *kms.GenerateDataKeyInput, opts ...func(*kms.Options)) (*kms.GenerateDataKeyOutput, error)
	Decrypt(ctx context.Context, in *kms.DecryptInput, opts ...func(*kms.Options)) (*kms.DecryptOutput, error)
}

// kmsPrefix marks a value sealed by KMS (envelope encryption, version 1).
const kmsPrefix = "kms1:"

// encryptionContext is bound into every data key: a sealed value only opens
// for this purpose, and each use shows in the KMS audit log under it.
var encryptionContext = map[string]string{"app": "dragent", "purpose": "integration-credentials"}

// KMS seals each value with its own data key from AWS KMS (envelope
// encryption): the master key never leaves KMS and is never in the server's
// settings, and every opening is a logged KMS call. Values sealed before
// (by a passphrase) still open through Legacy when it's set.
type KMS struct {
	Client KMSAPI
	KeyID  string
	Legacy Sealer
}

func (k KMS) Seal(ctx context.Context, plaintext string) (string, error) {
	out, err := k.Client.GenerateDataKey(ctx, &kms.GenerateDataKeyInput{
		KeyId:             aws.String(k.KeyID),
		KeySpec:           types.DataKeySpecAes256,
		EncryptionContext: encryptionContext,
	})
	if err != nil {
		return "", fmt.Errorf("kms data key: %w", err)
	}
	defer clear(out.Plaintext)
	sealed, err := sealWith(out.Plaintext, []byte(plaintext))
	if err != nil {
		return "", err
	}
	return kmsPrefix + base64.StdEncoding.EncodeToString(out.CiphertextBlob) + ":" + sealed, nil
}

func (k KMS) Open(ctx context.Context, sealed string) (string, error) {
	rest, ok := strings.CutPrefix(sealed, kmsPrefix)
	if !ok {
		if k.Legacy == nil {
			return "", errors.New("value wasn't sealed by KMS and no earlier key is set")
		}
		return k.Legacy.Open(ctx, sealed)
	}
	wrapped, body, ok := strings.Cut(rest, ":")
	if !ok {
		return "", errors.New("malformed sealed value")
	}
	blob, err := base64.StdEncoding.DecodeString(wrapped)
	if err != nil {
		return "", fmt.Errorf("decode data key: %w", err)
	}
	out, err := k.Client.Decrypt(ctx, &kms.DecryptInput{
		CiphertextBlob:    blob,
		KeyId:             aws.String(k.KeyID),
		EncryptionContext: encryptionContext,
	})
	if err != nil {
		return "", fmt.Errorf("kms decrypt: %w", err)
	}
	defer clear(out.Plaintext)
	return openWith(out.Plaintext, body)
}

func sealWith(key, plaintext []byte) (string, error) {
	gcm, err := newGCM(key)
	if err != nil {
		return "", err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", fmt.Errorf("generate nonce: %w", err)
	}
	return base64.StdEncoding.EncodeToString(gcm.Seal(nonce, nonce, plaintext, nil)), nil
}

func openWith(key []byte, encoded string) (string, error) {
	data, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return "", fmt.Errorf("decode ciphertext: %w", err)
	}
	gcm, err := newGCM(key)
	if err != nil {
		return "", err
	}
	if len(data) < gcm.NonceSize() {
		return "", errors.New("ciphertext too short")
	}
	plain, err := gcm.Open(nil, data[:gcm.NonceSize()], data[gcm.NonceSize():], nil)
	if err != nil {
		return "", fmt.Errorf("decrypt: %w", err)
	}
	return string(plain), nil
}

func newGCM(key []byte) (cipher.AEAD, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("create cipher: %w", err)
	}
	return cipher.NewGCM(block)
}
