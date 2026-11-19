// Package secretbox encrypts small secrets the server keeps (the keys of
// services people connect, see internal/integrations) with AES-256-GCM.
package secretbox

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
)

// Encrypt encrypts plaintext using AES-256-GCM with a key derived from passphrase.
// Returns base64-encoded ciphertext (nonce prepended).
func Encrypt(plaintext, passphrase string) (string, error) {
	if passphrase == "" {
		return "", errors.New("encryption key required")
	}
	key := deriveKey(passphrase)

	block, err := aes.NewCipher(key)
	if err != nil {
		return "", fmt.Errorf("create cipher: %w", err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", fmt.Errorf("create GCM: %w", err)
	}

	nonce := make([]byte, gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", fmt.Errorf("generate nonce: %w", err)
	}

	sealed := gcm.Seal(nonce, nonce, []byte(plaintext), nil)
	return base64.StdEncoding.EncodeToString(sealed), nil
}

// Decrypt reverses Encrypt.
func Decrypt(encoded, passphrase string) (string, error) {
	if passphrase == "" {
		return "", errors.New("encryption key required")
	}
	data, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return "", fmt.Errorf("decode ciphertext: %w", err)
	}

	key := deriveKey(passphrase)
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", fmt.Errorf("create cipher: %w", err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", fmt.Errorf("create GCM: %w", err)
	}

	if len(data) < gcm.NonceSize() {
		return "", errors.New("ciphertext too short")
	}
	nonce, ciphertext := data[:gcm.NonceSize()], data[gcm.NonceSize():]
	plain, err := gcm.Open(nil, nonce, ciphertext, nil)
	if err != nil {
		return "", fmt.Errorf("decrypt: %w", err)
	}
	return string(plain), nil
}

// deriveKey stretches a passphrase into a 32-byte AES-256 key via SHA-256.
func deriveKey(passphrase string) []byte {
	h := sha256.Sum256([]byte(passphrase))
	return h[:]
}
