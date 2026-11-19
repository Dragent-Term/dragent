package config

import "testing"

func TestShortJWTSecretIsRefusedOutsideDevelopment(t *testing.T) {
	t.Setenv("DATABASE_URL", "postgres://localhost/test")
	t.Setenv("JWT_SECRET", "too-short")
	t.Setenv("APP_ENV", "production")
	if _, err := Load(); err == nil {
		t.Fatal("a short JWT_SECRET was accepted in production")
	}
	t.Setenv("APP_ENV", "dev")
	if _, err := Load(); err != nil {
		t.Fatalf("development refused a short secret: %v", err)
	}
	t.Setenv("APP_ENV", "production")
	t.Setenv("JWT_SECRET", "0123456789abcdef0123456789abcdef")
	if _, err := Load(); err != nil {
		t.Fatalf("a 32-byte secret was refused: %v", err)
	}
}
