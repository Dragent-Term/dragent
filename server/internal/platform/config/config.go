package config

import (
	"fmt"
	"os"
	"strconv"
	"strings"

	"github.com/joho/godotenv"
)

type Config struct {
	Port        string
	DatabaseURL string
	RedisAddr   string
	JWTSecret   string
	SMTPHost    string
	SMTPPort    int
	SMTPUser    string
	SMTPPass    string
	SMTPSender  string
	// FeedbackInbox receives a copy of every in-app feedback submission.
	FeedbackInbox   string
	GitHubID        string
	GitHubSecret    string
	GoogleID        string
	GoogleSecret    string
	OpenAIAPIKey    string
	AnthropicAPIKey string
	VoyageAPIKey    string
	GoogleAPIKey    string
	XAIAPIKey       string
	// ExaAPIKey enables the dedicated web-search/fetch provider (empty = fall
	// back to each model provider's own native web tooling).
	ExaAPIKey string
	// IntegrationsEncryptionKey encrypts the keys of connected services
	// (Linear, Jira, …). Empty: the server doesn't keep connections, and
	// the desktop app keeps them on each computer.
	IntegrationsEncryptionKey string
	// IntegrationsKMSKeyID (an AWS KMS key id, ARN or alias) seals those
	// keys with KMS instead: the master key never sits in the server's
	// settings and each use is logged. IntegrationsEncryptionKey, when also
	// set, only opens values sealed before the switch.
	IntegrationsKMSKeyID string

	// LemonSqueezy payments (subscriptions). Empty = payments disabled.
	LemonSqueezyAPIKey        string
	LemonSqueezyStoreID       string
	LemonSqueezyWebhookSecret string
}

func Load() (*Config, error) {
	_ = godotenv.Load()

	port := getEnv("PORT", "8080")
	dbURL := getEnv("DATABASE_URL", "")
	if dbURL == "" {
		return nil, fmt.Errorf("DATABASE_URL is required")
	}

	// Prefer REDIS_URL (a full rediss://:token@host:port with TLS+auth — managed
	// providers like Upstash) and fall back to REDIS_ADDR (bare host:port for
	// local dev). Connect() accepts either form.
	redisAddr := getEnv("REDIS_URL", "")
	if redisAddr == "" {
		redisAddr = getEnv("REDIS_ADDR", "localhost:6379")
	}
	jwtSecret := getEnv("JWT_SECRET", "")
	if jwtSecret == "" {
		return nil, fmt.Errorf("JWT_SECRET is required")
	}
	// HS256 sign-ins are only as strong as this secret: a short one can be
	// guessed offline, and then anyone can sign in as anyone. Development
	// may use a short one.
	if len(jwtSecret) < minJWTSecretBytes && !devEnvironment() {
		return nil, fmt.Errorf("JWT_SECRET must be at least %d bytes (e.g. openssl rand -base64 48)", minJWTSecretBytes)
	}

	smtpHost := getEnv("SMTP_HOST", "localhost")
	smtpPortStr := getEnv("SMTP_PORT", "1025")
	smtpPort, err := strconv.Atoi(smtpPortStr)
	if err != nil {
		smtpPort = 1025
	}
	smtpUser := getEnv("SMTP_USER", "")
	smtpPass := getEnv("SMTP_PASS", "")
	smtpSender := getEnv("SMTP_SENDER", "noreply@dragent.com")
	feedbackInbox := getEnv("FEEDBACK_INBOX", "support@dragent.dev")

	githubID := getEnv("GITHUB_CLIENT_ID", "")
	githubSecret := getEnv("GITHUB_CLIENT_SECRET", "")
	googleID := getEnv("GOOGLE_CLIENT_ID", "")
	googleSecret := getEnv("GOOGLE_CLIENT_SECRET", "")

	openaiAPIKey := getEnv("OPENAI_API_KEY", "")
	anthropicAPIKey := getEnv("ANTHROPIC_API_KEY", "")
	voyageAPIKey := getEnv("VOYAGE_API_KEY", "")
	// Deployed hosts export this as GEMINI_API_KEY (the env var is named after the
	// SSM parameter), while the code addresses it as GOOGLE_API_KEY everywhere.
	// Accept either so Gemini has a server-side key on staging.
	googleAPIKey := getEnv("GOOGLE_API_KEY", getEnv("GEMINI_API_KEY", ""))
	xaiAPIKey := getEnv("XAI_API_KEY", "")
	exaAPIKey := getEnv("EXA_API_KEY", "")
	integrationsKey := getEnv("INTEGRATIONS_ENCRYPTION_KEY", "")
	integrationsKMSKey := getEnv("INTEGRATIONS_KMS_KEY_ID", "")

	lsAPIKey := getEnv("LEMONSQUEEZY_API_KEY", "")
	lsStoreID := getEnv("LEMONSQUEEZY_STORE_ID", "")
	lsWebhookSecret := getEnv("LEMONSQUEEZY_WEBHOOK_SECRET", "")

	return &Config{
		Port:            port,
		DatabaseURL:     dbURL,
		RedisAddr:       redisAddr,
		JWTSecret:       jwtSecret,
		SMTPHost:        smtpHost,
		SMTPPort:        smtpPort,
		SMTPUser:        smtpUser,
		SMTPPass:        smtpPass,
		SMTPSender:      smtpSender,
		FeedbackInbox:   feedbackInbox,
		GitHubID:        githubID,
		GitHubSecret:    githubSecret,
		GoogleID:        googleID,
		GoogleSecret:    googleSecret,
		OpenAIAPIKey:    openaiAPIKey,
		AnthropicAPIKey: anthropicAPIKey,
		VoyageAPIKey:    voyageAPIKey,
		GoogleAPIKey:    googleAPIKey,
		XAIAPIKey:       xaiAPIKey,
		ExaAPIKey:       exaAPIKey,

		IntegrationsEncryptionKey: integrationsKey,
		IntegrationsKMSKeyID:      integrationsKMSKey,

		LemonSqueezyAPIKey:        lsAPIKey,
		LemonSqueezyStoreID:       lsStoreID,
		LemonSqueezyWebhookSecret: lsWebhookSecret,
	}, nil
}

func getEnv(key, defaultVal string) string {
	if value, exists := os.LookupEnv(key); exists {
		return value
	}
	return defaultVal
}

// minJWTSecretBytes is the shortest JWT_SECRET accepted outside development.
const minJWTSecretBytes = 32

func devEnvironment() bool {
	switch strings.ToLower(strings.TrimSpace(os.Getenv("APP_ENV"))) {
	case "dev", "local", "test":
		return true
	default:
		return false
	}
}
