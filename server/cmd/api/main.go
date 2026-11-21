package main

import (
	"context"

	"database/sql"
	"errors"
	"github.com/google/uuid"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/99designs/gqlgen/graphql/playground"
	awsconfig "github.com/aws/aws-sdk-go-v2/config"
	"github.com/aws/aws-sdk-go-v2/service/kms"
	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"

	"server/graph"
	"server/internal/identity/api"
	"server/internal/identity/components"
	"server/internal/identity/model"
	"server/internal/platform/secretbox"

	billingComponents "server/internal/billing/components"
	billingModel "server/internal/billing/model"
	"server/internal/billing/payments"

	"server/internal/account"
	"server/internal/automations"
	"server/internal/companionpairing"
	"server/internal/companionsession"
	adminconfig "server/internal/config"
	"server/internal/execution"
	"server/internal/feedback"
	"server/internal/integrations"
	"server/internal/organization"
	"server/internal/push"
	"server/internal/relay"
	orgtasks "server/internal/tasks"
	"server/internal/usage"
	"server/internal/voice"

	sessionApi "server/internal/session/api"
	sessionComponents "server/internal/session/components"

	selfhealComponents "server/internal/selfheal/components"
	selfhealModel "server/internal/selfheal/model"

	embeddingsPkg "server/internal/embeddings"

	agentApi "server/internal/agent/api"
	"server/internal/agent/permissions"
	"server/internal/agent/providers"
	"server/internal/agent/tasks"

	"server/internal/platform/config"
	"server/internal/platform/logger"
	"server/internal/platform/postgres"
	"server/internal/platform/ratelimit"
	"server/internal/platform/redis"
)

func main() {
	// Initialize structured logging
	logger.InitLogger()
	slog.Info("bootstrapping dragent backend API server...")

	// Load platform configuration settings
	cfg, err := config.Load()
	if err != nil {
		slog.Error("failed to load configuration settings", "error", err)
		os.Exit(1)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	//  Establish PostgreSQL connection pool
	db, err := postgres.Connect(ctx, cfg.DatabaseURL)
	if err != nil {
		slog.Error("unable to initialize postgres database pool", "error", err)
		os.Exit(1)
	}
	defer db.Close()

	//  Establish Redis client connection
	rClient, err := redis.Connect(ctx, cfg.RedisAddr)
	if err != nil {
		slog.Error("unable to initialize redis client cache", "error", err)
		os.Exit(1)
	}
	defer func() {
		_ = rClient.Close()
	}()

	// Instantiate infrastructure adapters inside models package
	tokenService := model.NewTokenService(cfg.JWTSecret)
	userRepo := model.NewPostgresUserRepository(db.Pool)
	txManager := model.NewPostgresTxManager(db.Pool)
	tokenStore := model.NewRedisTokenStore(rClient.Rdb)
	// Logout ends a session everywhere its access token is checked.
	tokenService.SetSignedOutCheck(tokenStore.IsDeviceSignedOut)

	appBaseURL := "http://localhost:" + cfg.Port
	if os.Getenv("DRAGENT_WEB_ORIGIN") != "" {
		appBaseURL = os.Getenv("DRAGENT_WEB_ORIGIN")
	}

	mailer := model.NewSmtpMailer(cfg.SMTPHost, cfg.SMTPPort, cfg.SMTPUser, cfg.SMTPPass, cfg.SMTPSender, appBaseURL)
	githubOAuth := model.NewGitHubProvider(cfg.GitHubID, cfg.GitHubSecret, appBaseURL+"/auth/callback/github")
	googleOAuth := model.NewGoogleProvider(cfg.GoogleID, cfg.GoogleSecret, appBaseURL+"/auth/callback/google")

	// Instantiate Billing Repositories and Use Cases
	billingRepo := billingModel.NewPostgresBillingRepository(db.Pool)
	billingTxManager := billingModel.NewPostgresTxManager(db.Pool)

	orgRepo := organization.NewPostgresRepository(db.Pool)
	orgHandler := organization.NewHandler(orgRepo, userRepo, func(minRole organization.MembershipRole) func(http.Handler) http.Handler {
		return RequireOrgRole(orgRepo, minRole)
	}, net.LookupTXT, appBaseURL)
	orgHandler.SetMailer(mailer)
	feedbackHandler := feedback.NewHandler(feedback.NewPostgresRepository(db.Pool), userRepo, mailer, cfg.FeedbackInbox)
	automationsHandler := automations.NewHandler(automations.NewPostgresRepository(db.Pool))
	tasksHandler := orgtasks.NewHandler(orgtasks.NewPostgresRepository(db.Pool))
	integrationsHandler := integrations.NewHandler(
		integrations.NewPostgresRepository(db.Pool),
		orgRepo,
		integrationsSealer(cfg),
	)

	billingUC := billingComponents.NewBillingUseCase(
		billingRepo, // BalanceRepository
		billingRepo, // PriceRepository
		billingRepo, // TransactionRepository
		billingRepo, // ProfitRepository
		billingRepo, // SystemConfigRepository
		billingTxManager,
	)
	// A balance row is keyed (user_id, organization_id); GetOrInitBalance's
	// lazy-init path needs to resolve which org when one doesn't exist yet.
	billingUC.SetOrgResolver(orgRepo)

	// The markup lives only in system_configs — there is no compiled fallback,
	// so an absent or invalid row means no request can be billed. Fail here,
	// at boot, rather than letting the service come up and refuse every user.
	if err := billingUC.VerifyMarkupConfigured(context.Background()); err != nil {
		slog.Error("startup: billing markup is not configured",
			"error", err,
			"expected", "system_configs rows markup_numerator + markup_denominator (migration 00015)")
		os.Exit(1)
	}
	billingReconcileCtx, stopBillingReconciler := context.WithCancel(context.Background())
	defer stopBillingReconciler()
	go func() {
		ticker := time.NewTicker(15 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-billingReconcileCtx.Done():
				return
			case <-ticker.C:
				reconcileCtx, cancel := context.WithTimeout(billingReconcileCtx, 10*time.Second)
				settled, err := billingUC.ReconcileUnsettledOperations(reconcileCtx, 100)
				cancel()
				if err != nil {
					slog.Error("billing settlement reconciliation failed", "error", err)
				}
				if settled > 0 {
					slog.Info("billing settlement reconciliation completed", "settled", settled)
				}
			}
		}
	}()

	// Instantiate Agent Pipeline (streaming task manager + provider factory)
	// EXA_API_KEY is not a model provider: webtools.ConfigFromKeys reads it from
	// this map to serve web_search / fetch_url through Exa instead of the task
	// model. /api/models ignores it (fixed catalog), so it stays inert there.
	providerKeys := map[string]string{
		"ANTHROPIC_API_KEY":  cfg.AnthropicAPIKey,
		"OPENAI_API_KEY":     cfg.OpenAIAPIKey,
		"GOOGLE_API_KEY":     cfg.GoogleAPIKey,
		"XAI_API_KEY":        cfg.XAIAPIKey,
		"EXA_API_KEY":        cfg.ExaAPIKey,
		"OPENROUTER_API_KEY": os.Getenv("OPENROUTER_API_KEY"),
		"GROQ_API_KEY":       os.Getenv("GROQ_API_KEY"),
		"DEEPSEEK_API_KEY":   os.Getenv("DEEPSEEK_API_KEY"),
		"ALIBABA_API_KEY":    os.Getenv("ALIBABA_API_KEY"),
		"ZHIPU_API_KEY":      os.Getenv("ZHIPU_API_KEY"),
		"MOONSHOT_API_KEY":   os.Getenv("MOONSHOT_API_KEY"),
		"MINIMAX_API_KEY":    os.Getenv("MINIMAX_API_KEY"),
		"MISTRAL_API_KEY":    os.Getenv("MISTRAL_API_KEY"),
		"TOGETHER_API_KEY":   os.Getenv("TOGETHER_API_KEY"),
		"DEEPINFRA_API_KEY":  os.Getenv("DEEPINFRA_API_KEY"),
		"PERPLEXITY_API_KEY": os.Getenv("PERPLEXITY_API_KEY"),
		"VENICE_API_KEY":     os.Getenv("VENICE_API_KEY"),
		"NVIDIA_API_KEY":     os.Getenv("NVIDIA_API_KEY"),
		"FIREWORKS_API_KEY":  os.Getenv("FIREWORKS_API_KEY"),
		"COHERE_API_KEY":     os.Getenv("COHERE_API_KEY"),
		"META_API_KEY":       os.Getenv("META_API_KEY"),
	}
	providerFactory := providers.NewProviderFactory(providerKeys)
	taskStore := tasks.NewPostgresStore(db.Pool)
	taskManager := tasks.NewManager(providerFactory, permissions.DefaultPolicy(), billingUC, billingRepo, taskStore)
	agentHandler := agentApi.NewHandler(taskManager, providerKeys)

	usageHandler := usage.NewHandler(billingUC)
	// Execution grants (M3): fail-closed only at authorize time. An empty
	// key means ephemeral dev grants — set EXECUTION_GRANT_KEY in prod.
	executionHandler, err := execution.NewHandler(os.Getenv("EXECUTION_GRANT_KEY"), billingUC)
	if err != nil {
		slog.Error("execution grants unavailable", "error", err)
		os.Exit(1)
	}
	accountHandler := account.NewHandler(userRepo, billingRepo, billingUC)
	voiceHandler := voice.NewHandler(billingUC)
	// Realtime shared-session gateway: connection hub fanned out across instances
	// via Redis pub/sub, plus a Redis-backed store for session records + pairing
	// codes (5-min TTL).
	sessionHub := sessionComponents.NewHub(
		sessionComponents.NewRedisBus(rClient.Rdb),
		sessionComponents.NewRedisRateLimiter(rClient.Rdb), // 60 remote cmds/hr/controller
		sessionComponents.NewPostgresAuditor(db.Pool),      // durable remote-command audit trail
	)
	sessionStore := sessionComponents.NewRedisStore(rClient.Rdb)
	sessionHandler := sessionApi.NewHandler(sessionHub, sessionStore, appBaseURL, mailer, sessionApi.IdentityLookup{Users: userRepo})
	sessionHandler.SetForwardDomain(os.Getenv("DRAGENT_FORWARD_DOMAIN"))
	configHandler := adminconfig.NewHandler(billingRepo)
	// Phone pairing store (Redis): one-time codes + rotating device
	// credentials. Declared before socket auth and push registration so
	// admission can reject revoked phones and push registration can
	// resolve companion phones through it (FIX1, §8: companion phones live
	// only here, never in the Postgres devices table).
	pairingStore := companionpairing.NewRedisStore(rClient.Rdb)
	// Mobile push registry: device token leases backed by Postgres, verified
	// against the companion pairing store through the access-JWT device
	// claim inside the handler (the global AuthMiddleware does not propagate
	// that claim). Mounted under RequireAuth below so anonymous callers
	// never reach it.
	pushCfg := push.ConfigFromEnv()
	pushStore := push.NewPostgresStore(db.Pool)
	pushHandler := push.NewHandler(
		pushStore,
		push.NewCompanionDeviceResolver(pairingStore),
		push.NewJWTAuthenticator(cfg.JWTSecret),
	)
	// Relay catalog (§6.5): the server is a blind router. The desktop
	// publishes its sealed catalog blob opaquely; the phone reads it back
	// verbatim. Same-account authorization by construction (the owner key
	// embeds the caller id), enforced by RequireAuth below plus a user_id
	// check inside the handler. Plaintext catalog shapes are gone (410).
	relayCatalogStore := relay.NewMemoryStore()
	relaySealedStore := relay.NewMemorySealedStore()
	relayHandler := relay.NewHandler(relayCatalogStore)
	relayHandler.SetSealedStore(relaySealedStore)
	relayRPC := relay.NewRPCHandler(nil, nil)
	relayRPC.SetSealedStore(relaySealedStore)
	relayRPC.ConfigureSocket(func(ctx context.Context, token string) (relay.SocketIdentity, error) {
		claims, err := tokenService.VerifyAccessToken(token)
		if err != nil {
			return relay.SocketIdentity{}, err
		}
		if claims.ExpiresAt == nil || claims.UserID == uuid.Nil || claims.DeviceID == uuid.Nil {
			return relay.SocketIdentity{}, errors.New("token has no expiry")
		}
		if claims.Scope == companionpairing.AccessScope {
			// Paired phones carry their device id in the token: reject
			// revoked or lapsed devices at admission (fail closed).
			valid, err := pairingStore.IsDeviceValid(ctx, claims.UserID.String(), claims.DeviceID.String())
			if err != nil || !valid {
				return relay.SocketIdentity{}, errors.New("companion device is no longer paired")
			}
		}
		return relay.SocketIdentity{UserID: claims.UserID.String(), DeviceID: claims.DeviceID.String(), ExpiresAt: claims.ExpiresAt.Time}, nil
	}, relayCatalogStore, nil)
	// Fixed 24-hour mobile access survives physical reconnects and API
	// instances; Redis reads never renew the original expiry.
	relayRPC.SetCompanionSessions(companionsession.NewRedisStore(rClient.Rdb))
	// Phone pairing endpoints: the QR code the signed-in desktop shows
	// redeems into a device credential whose access tokens ride the relay
	// above unchanged (same claims shape as portal logins, plus scope).
	pairingHandler := companionpairing.NewHandler(pairingStore, userRepo, tokenService, os.Getenv("DRAGENT_BACKEND_URL"),
		companionpairing.WithOrgResolver(func(ctx context.Context, userID string) string {
			uid, err := uuid.Parse(userID)
			if err != nil {
				return ""
			}
			org, err := orgRepo.GetActiveOrganization(ctx, uid)
			if err != nil {
				return ""
			}
			return org.ID.String()
		}),
		companionpairing.WithSocketCloser(relayRPC),
		// The push registrations table no longer cascades off the devices
		// table (migration 00063: companion phones have no devices row), so
		// revoking a companion device must delete its push registration
		// here, where both stores are owned. Best-effort: failures are
		// logged with the error only, never ids or tokens (§2.3).
		companionpairing.WithPushCleanup(func(ctx context.Context, userID, deviceID string) {
			userUUID, err := uuid.Parse(userID)
			deviceUUID, err2 := uuid.Parse(deviceID)
			if err != nil || err2 != nil {
				return
			}
			if err := pushStore.Delete(ctx, userUUID, deviceUUID); err != nil && !errors.Is(err, push.ErrNotFound) {
				slog.Warn("companion push registration cleanup failed", "error", err)
			}
		}),
		companionpairing.WithRateLimiters(
			companionpairing.NewRedisRateLimiter(rClient.Rdb, 20, time.Minute),
			companionpairing.NewRedisRateLimiter(rClient.Rdb, 10, time.Minute),
		),
	)
	// Cross-instance revocation: the socket poll re-checks the device, so
	// a DELETE served elsewhere still ends the socket within one cadence.
	relayRPC.SetDeviceAuthorizer(pairingStore.IsDeviceValid)
	// E2EE cut-over (§10): pre-cut-over e2e:false devices pair again. The
	// socket admission policy reads the pairing store's cut-over flag:
	// unknown devices skip the check (JWT validity and revocation still
	// apply); known devices without the flag are refused with 4403 before
	// admission and never get relay-ready.
	relayRPC.SetE2EDevicePolicy(func(ctx context.Context, userID, deviceID string) (bool, bool) {
		known, e2e, err := pairingStore.DeviceE2E(ctx, userID, deviceID)
		if err != nil {
			return false, false
		}
		return known, e2e
	})
	// E2EE push intake (§8 steps 2-3): host-notify blobs arriving on the
	// desktop socket are handed to the push dispatcher, which delivers the
	// opaque seal to the phone's live registration. Only when a provider is
	// configured (APNs/FCM credentials present); otherwise the relay keeps
	// today's behaviour — no intake, blobs are dropped after the aggregate
	// counter — and the intake can never panic on an unwired dispatcher.
	if e2ePushConfigured(pushCfg) {
		relayRPC.SetE2ENotifyIntake(newE2ENotifyIntake(pushStore, push.SenderFromConfig(pushCfg)))
	}
	// Cut-over purge (§10.6): stored catalogs and host-state mirrors live in
	// process memory and Redis, not Postgres — a deploy clears the memory
	// stores, and the plaintext HTTP paths below are 410, so nothing writes
	// them again. Device names never reach the pairing store any more; the
	// pairing records keep only the platform class and the e2e flag.

	// Payments (LemonSqueezy subscriptions). Provider is nil (payments disabled)
	// when unconfigured — pass it as a nil INTERFACE, not a typed-nil pointer,
	// so Service.Enabled() reads false instead of panicking on method calls.
	ls := payments.NewLemonSqueezy(cfg.LemonSqueezyAPIKey, cfg.LemonSqueezyStoreID, cfg.LemonSqueezyWebhookSecret)
	var paymentsProvider payments.PaymentProvider
	var orgBillingProvider organization.BillingProvider
	if ls != nil {
		paymentsProvider = ls
		orgBillingProvider = ls
	}
	paymentsRepo := payments.NewPostgresRepository(db.Pool)
	paymentsSvc := payments.NewService(paymentsProvider, paymentsRepo, appBaseURL+"/dashboard")
	paymentsHandler := payments.NewHandler(paymentsSvc)

	// Organization billing (per-seat, one tier per org). Reuses the
	// SAME plans catalog and LemonSqueezy client individual billing does;
	// paymentsRepo satisfies both PlanLookup and SeatBalances already.
	orgBillingSvc := organization.NewBillingService(orgRepo, paymentsRepo, paymentsRepo, orgBillingProvider, appBaseURL+"/dashboard")
	orgBillingSvc.SetMailer(mailer)
	orgBillingSvc.SetReceiptWriter(paymentsRepo)
	orgHandler.SetBillingService(orgBillingSvc)
	// Webhook dispatch: org-attributed events (organization_id in
	// custom_data) route here instead of the per-user switch — one webhook
	// endpoint, shared idempotency, individual billing untouched.
	paymentsSvc.SetOrgApplier(orgBillingSvc)
	// Credit-pack purchases credit whichever org the buyer is acting as,
	// resolved at checkout time (see payments.OrgResolver's doc comment).
	paymentsSvc.SetOrgResolver(orgRepo)

	// Background: periodically evict completed tasks older than 1 hour
	go func() {
		ticker := time.NewTicker(10 * time.Minute)
		defer ticker.Stop()
		for range ticker.C {
			taskManager.Cleanup(time.Hour)
		}
	}()

	// Background: annual-plan renewal reminders (11.G.2) — LemonSqueezy has
	// no push equivalent of Stripe's invoice.upcoming, so this polls instead.
	// Hourly is far more often than needed (renewal dates don't move) but
	// costs nothing extra: ListAnnualRenewalsDue's own renewal_reminder_sent_at
	// guard makes every run but the first no-op per org until its next cycle.
	go func() {
		ticker := time.NewTicker(1 * time.Hour)
		defer ticker.Stop()
		for range ticker.C {
			sent, err := orgBillingSvc.SendRenewalReminders(context.Background(), 7*24*time.Hour)
			if err != nil {
				slog.Error("renewal reminder sweep failed", "error", err)
			} else if sent > 0 {
				slog.Info("renewal reminder sweep completed", "orgs_reminded", sent)
			}
		}
	}()

	// 8. Instantiate Self-Healing Repositories and Use Cases
	selfhealRepo := selfhealModel.NewPostgresSelfHealRepository(db.Pool)
	llmGateway := selfhealModel.NewMultiLLMGateway(cfg.OpenAIAPIKey, cfg.AnthropicAPIKey)
	selfHealUC := selfhealComponents.NewSelfHealUseCase(
		selfhealRepo,
		llmGateway,
		billingUC,
	)

	// 8. Instantiate Identity Use Cases (components)
	sendMagicLinkUC := components.NewSendMagicLinkUseCase(userRepo, tokenStore, mailer, tokenService)
	verifyMagicLinkUC := components.NewVerifyMagicLinkUseCase(userRepo, tokenStore, tokenService, txManager)
	authOAuthUC := components.NewAuthenticateOAuthUseCase(userRepo, tokenStore, tokenService, txManager)
	// Give registration a user_balances row in the same transaction as the user.
	// Without this the row only appears on the first billing operation, so a
	// signup that goes straight to the dashboard 500s on "fetch balance".
	verifyMagicLinkUC.SetBalanceInitializer(billingRepo)
	authOAuthUC.SetBalanceInitializer(billingRepo)
	// Every account gets a personal organization, not opt-in.
	verifyMagicLinkUC.SetOrganizationInitializer(orgRepo)
	authOAuthUC.SetOrganizationInitializer(orgRepo)
	verifyMagicLinkUC.SetWelcomeMailer(mailer)
	authOAuthUC.SetWelcomeMailer(mailer)
	refreshTokenUC := components.NewRefreshTokenUseCase(userRepo, tokenService, txManager)

	// 9. Initialize HTTP Handlers
	authHandlers := api.NewAuthHandlers(
		sendMagicLinkUC,
		verifyMagicLinkUC,
		authOAuthUC,
		tokenStore,
		githubOAuth,
		googleOAuth,
	)
	// Magic-link abuse limits: per inbox and per visitor IP. The portal
	// proxies these requests, so the per-IP limit needs the shared secret
	// that lets it name the visitor's IP (see identity/api/magic_limits.go).
	portalSecret := os.Getenv("AUTH_PORTAL_PROXY_SECRET")
	if portalSecret == "" {
		slog.Warn("AUTH_PORTAL_PROXY_SECRET is not set: magic-link per-IP rate limit is off (per-email limit still applies)")
	}
	authHandlers.SetMagicLinkLimits(api.MagicLinkLimits{
		PerEmail:     ratelimit.NewRedisRateLimiter(rClient.Rdb, envInt("MAGIC_LINK_EMAIL_LIMIT", 5), 15*time.Minute),
		PerIP:        ratelimit.NewRedisRateLimiter(rClient.Rdb, envInt("MAGIC_LINK_IP_LIMIT", 20), 15*time.Minute),
		PortalSecret: portalSecret,
	})

	// 10. Configure HTTP Router and Middlewares
	r := chi.NewRouter()
	r.Use(middleware.RequestID)
	// Not chi's RealIP: it believes client-set X-Real-IP / X-Forwarded-For.
	r.Use(ratelimit.RealIP)
	// Request telemetry — but NOT chi's per-request access log (H1, §2.3):
	// the server emits per-minute aggregate counters only (route template,
	// status class, size bucket, error class), never URIs, query strings,
	// tokens or ids. Health probes are excluded from the counters: App
	// Runner hits /ready every 10s forever, which would drown real traffic
	// in the aggregates (and bill CloudWatch ingestion for noise). Probe
	// failures still surface via App Runner's own service events. Panics
	// are recovered here too, reduced to the route template and the panic
	// type (no stacks or values, which could carry content).
	agg := logger.NewAggregator(time.Minute)
	stopAgg := agg.Start()
	defer stopAgg()
	r.Use(agg.Middleware)
	r.Use(securityHeaders)

	// Auth context injection middleware (applies globally)
	r.Use(AuthMiddleware(tokenService))
	// The browser viewer signs in to /realtime with a ticket, not a token.
	tickets := realtimeTickets{rdb: rClient.Rdb}
	r.Use(tickets.middleware)
	// A revoked phone's token is refused on every route, not only the relay.
	r.Use(pairingHandler.RejectRevokedDevices)
	// Forward hostnames are a separate routing boundary. This must run before
	// ordinary routes such as GraphQL's "/" and "/query", otherwise a forwarded
	// request can be mistaken for the API's own playground.
	r.Use(func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			if sessionHandler.IsForwardHost(req.Host) {
				sessionHandler.ServeForwardHost(w, req)
				return
			}
			next.ServeHTTP(w, req)
		})
	})
	// Request body caps (1 MiB unless a route needs more; see bodylimit.go).
	// After the forward-host split: forwarded apps keep the gateway's own cap.
	r.Use(limitRequestBodies)

	// Routes that need a 60s request timeout (everything except SSE streams).
	r.Group(func(rr chi.Router) {
		rr.Use(middleware.Timeout(60 * time.Second))

		// Authentication routes
		rr.Get("/auth/start", authHandlers.StartAuth)
		rr.Post("/auth/magic/request", authHandlers.RequestMagicLink)
		rr.Get("/auth/magic", authHandlers.VerifyMagicLink)
		rr.Get("/auth/callback/github", authHandlers.GitHubCallback)
		rr.Get("/auth/callback/google", authHandlers.GoogleCallback)
		rr.Get("/auth/desktop/session", authHandlers.PollDesktopSession)
		rr.Post("/auth/desktop/session", authHandlers.PollDesktopSessionWithProof)
		rr.Post("/auth/desktop/exchange", authHandlers.ExchangeDesktopHandoff)
		rr.With(RequireAuth).Post("/auth/desktop/approve", authHandlers.ApproveDesktopSession)
		rr.With(RequireAuth).Post("/realtime/ticket", tickets.handleIssue)

		// GraphQL endpoint and playground
		// No playground and no introspection outside dev/local; see
		// newGraphQLServer for the complexity limit.
		devGraphQL := graphqlDevMode(os.Getenv("APP_ENV"))
		srv := newGraphQLServer(graph.NewExecutableSchema(graph.Config{
			Resolvers: &graph.Resolver{
				UserRepo:               userRepo,
				TokenStore:             tokenStore,
				TokenService:           tokenService,
				VerifyMagicLinkUseCase: verifyMagicLinkUC,
				RefreshTokenUseCase:    refreshTokenUC,
				SelfHealUseCase:        selfHealUC,
			},
		}), devGraphQL)
		if devGraphQL {
			rr.Handle("/", playground.Handler("GraphQL playground", "/query"))
		}
		rr.Handle("/query", srv)

		// Embedding proxy — forwards to Voyage API with server-side key
		embeddingHandler := embeddingsPkg.NewHandler(cfg.VoyageAPIKey)
		embeddingHandler.SetLimits(embeddingsPkg.Limits{
			PerMinute: ratelimit.NewRedisRateLimiter(rClient.Rdb, envInt("EMBEDDINGS_PER_MINUTE_LIMIT", embeddingsPkg.DefaultPerMinute), time.Minute),
			PerDay:    ratelimit.NewRedisRateLimiter(rClient.Rdb, envInt("EMBEDDINGS_DAILY_LIMIT", embeddingsPkg.DefaultPerDay), 24*time.Hour),
		})
		rr.Post("/v1/embeddings", embeddingHandler.Embed)

		// Liveness: the process is up. Cheap, no dependencies — used by the
		// platform to decide whether to restart the container.
		rr.Get("/health", func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte("OK"))
		})

		// Readiness: the process can serve traffic — Postgres AND Redis reachable.
		// The load balancer routes to an instance only while this returns 200, so
		// a container with a dead dependency is pulled from rotation instead of
		// erroring user requests.
		rr.Get("/ready", func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			// Draining: report NOT ready so the LB pulls us from rotation, but
			// keep serving in-flight work until Shutdown completes.
			if draining.Load() {
				w.WriteHeader(http.StatusServiceUnavailable)
				_, _ = w.Write([]byte(`{"ready":false,"draining":true}`))
				return
			}
			ctx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
			defer cancel()
			if err := db.Pool.Ping(ctx); err != nil {
				w.WriteHeader(http.StatusServiceUnavailable)
				_, _ = w.Write([]byte(`{"ready":false,"postgres":"down"}`))
				return
			}
			if err := rClient.Rdb.Ping(ctx).Err(); err != nil {
				w.WriteHeader(http.StatusServiceUnavailable)
				_, _ = w.Write([]byte(`{"ready":false,"redis":"down"}`))
				return
			}
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte(`{"ready":true}`))
		})

		// Payment provider webhook — authenticated by signature, not user token.
		// Must stay OUTSIDE RequireAuth (the provider, not a user, calls it).
		paymentsHandler.RegisterWebhook(rr)
		// Phone pairing redeem + token rotation: unauthenticated (the code
		// / device token in the body is the credential), rate-limited.
		pairingHandler.RegisterPublicRoutes(rr)
	})

	// Agent, usage and account routes — all require a valid access token.
	r.Group(func(rr chi.Router) {
		rr.Use(RequireAuth)
		agentHandler.RegisterRoutes(rr)
		accountHandler.RegisterRoutes(rr)
		voiceHandler.RegisterRoutes(rr)
		usageHandler.RegisterRoutes(rr)
		executionHandler.RegisterRoutes(rr)
		execution.RegisterTelemetryRoutes(rr)
		paymentsHandler.RegisterRoutes(rr)
		// Realtime session REST (create/email/revoke invite links) — host-only,
		// so it stays under RequireAuth.
		sessionHandler.RegisterRoutes(rr)
		orgHandler.RegisterRoutes(rr)
		feedbackHandler.RegisterRoutes(rr)
		automationsHandler.RegisterRoutes(rr, RequireOrgRole(orgRepo, organization.RoleMember))
		tasksHandler.RegisterRoutes(rr, RequireOrgRole(orgRepo, organization.RoleMember))
		integrationsHandler.RegisterRoutes(rr, RequireOrgRole(orgRepo, organization.RoleMember))
		// Device push leases (register/rotate/unregister/list): the handler
		// re-verifies the access JWT itself to enforce the device claim.
		pushHandler.RegisterRoutes(rr)
		// Host catalog for same-account controllers (PUT publish / GET read).
		relayHandler.RegisterRoutes(rr)
		// Phone pairing management (create/list/revoke) for the desktop.
		pairingHandler.RegisterRoutes(rr)
	})

	// Realtime shared-session WebSocket. Mounted OUTSIDE RequireAuth so anonymous
	// View joiners can connect (the soft AuthMiddleware above still fills in the
	// user id when a bearer token is present, which Edit joins require), and
	// OUTSIDE the 60s request Timeout so connections can live indefinitely.
	r.Group(func(rr chi.Router) {
		sessionHandler.RegisterPublicRoutes(rr)
		relayRPC.RegisterSocketRoutes(rr)
		relayRPC.RegisterHostRoutes(rr)
	})
	r.Group(func(rr chi.Router) {
		rr.Use(RequireAuth)
		// Revoked phones lose HTTP RPC the moment they are revoked;
		// portal tokens pass through untouched.
		rr.Use(pairingHandler.RequireValidDevice)
		relayRPC.RegisterRPCRoutes(rr)
	})

	// Admin-only configuration API: requires a valid token AND is_admin.
	r.Group(func(rr chi.Router) {
		rr.Use(RequireAuth)
		rr.Use(RequireAdmin(billingRepo))
		configHandler.RegisterRoutes(rr)
	})

	server := &http.Server{
		Addr:              ":" + cfg.Port,
		Handler:           r,
		ReadHeaderTimeout: 10 * time.Second, // slowloris protection
	}

	// 11. Startup HTTP Server in goroutine
	go func() {
		slog.Info("dragent API server running", "port", cfg.Port)
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			slog.Error("dragent API server failed to listen", "error", err)
			os.Exit(1)
		}
	}()

	// 12. Implement graceful termination on OS interrupt signals
	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)
	<-quit
	slog.Info("SIGTERM received — draining: reporting not-ready so the load balancer stops sending new traffic")

	// 1. Fail readiness so the LB drains NEW requests to healthy instances.
	draining.Store(true)
	// 2. Give the readiness check time to propagate to the LB before we stop
	//    accepting connections (avoids dropping requests already in flight to us).
	time.Sleep(readinessDrainGrace)

	// 3. Stop accepting new connections and wait for in-flight requests
	//    (including active SSE agent streams) to finish, up to the grace window.
	slog.Info("draining in-flight requests before shutdown", "grace", shutdownGrace.String())
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), shutdownGrace)
	defer shutdownCancel()
	if err := server.Shutdown(shutdownCtx); err != nil {
		slog.Error("api server shutdown encountered error (some in-flight requests may have been cut)", "error", err)
	}

	slog.Info("dragent API server terminated cleanly")
}

// e2ePushConfigured reports whether at least one push provider has
// credentials, i.e. delivery is possible (E9c, §8 steps 2-3).
func e2ePushConfigured(cfg push.Config) bool {
	return cfg.APNs.Enabled() || cfg.FCM.Enabled()
}

// newE2ENotifyIntake builds the relay's host-notify intake from the push
// registration store and sender (E9c, §8 steps 2-3). Its signature matches
// relay.NotifyIntakeFunc, so the caller wires it with
// rpcHandler.SetE2ENotifyIntake. A nil store or sender yields nil — never a
// panicking intake — and the caller installs nothing in that case.
func newE2ENotifyIntake(store push.Store, sender push.Sender) relay.NotifyIntakeFunc {
	if store == nil || sender == nil {
		return nil
	}
	return (&push.NotifyDispatcher{Store: store, Sender: sender}).HandleNotify
}

// securityHeaders adds defense-in-depth response headers. Cloudflare can also
// set these at the edge; doing it here too means they're present even on direct
// origin access. Kept minimal — this is a JSON/SSE API, not a browser app.
func securityHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Referrer-Policy", "no-referrer")
		// HSTS is meaningful only over HTTPS (prod terminates TLS at Cloudflare /
		// App Runner). Harmless on plaintext local dev — browsers ignore it there.
		h.Set("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
		next.ServeHTTP(w, r)
	})
}

// draining is flipped true on SIGTERM so /ready reports 503 and the load
// balancer stops routing NEW requests to this instance while in-flight ones
// finish. This is the standard zero-downtime-deploy drain pattern.
var draining atomic.Bool

// Shutdown timing. readinessDrainGrace lets the LB notice we are unready
// before we stop accepting; shutdownGrace bounds how long we wait for
// in-flight requests. Keep shutdownGrace within the platform SIGTERM grace
// period (App Runner/ECS) so we exit cleanly rather than being SIGKILLed.
const (
	readinessDrainGrace = 5 * time.Second
	shutdownGrace       = 25 * time.Second
)

func AuthMiddleware(tokenService *model.TokenService) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			ctx := r.Context()

			// Inject user-provided API keys from optional headers so resolvers can
			// use them instead of the server's shared keys. Keys are never stored.
			if key := r.Header.Get("X-User-Api-Key"); key != "" {
				ctx = context.WithValue(ctx, "user_api_key", key)
			}
			// Optional custom OpenAI-compatible base URL for Ollama/LM Studio/custom models.
			if url := r.Header.Get("X-Custom-Base-Url"); url != "" {
				ctx = context.WithValue(ctx, "custom_base_url", url)
			}
			// Model name for custom OpenAI-compatible endpoints — overrides the
			// model_name field in the request body when a custom base URL is present.
			if model := r.Header.Get("X-Custom-Model"); model != "" {
				ctx = context.WithValue(ctx, "custom_model", model)
			}
			// Platform credentials are never an implicit fallback. Missing, false,
			// or malformed values all fail closed.
			fallbackAllowed := strings.EqualFold(strings.TrimSpace(r.Header.Get("X-Fallback-Allowed")), "true")
			ctx = context.WithValue(ctx, "fallback_allowed", fallbackAllowed)

			// The bearer comes from the Authorization header only. Access
			// tokens are never accepted in a URL (they end up in logs,
			// history and proxies): the phone and desktop authenticate
			// in-band on the relay, and the browser viewer joins /realtime
			// with a short-lived ticket (realtime_ticket.go).
			var token string
			if authHeader := r.Header.Get("Authorization"); authHeader != "" {
				parts := strings.Split(authHeader, " ")
				if len(parts) == 2 && strings.ToLower(parts[0]) == "bearer" {
					token = parts[1]
				}
			}
			if token == "" {
				next.ServeHTTP(w, r.WithContext(ctx))
				return
			}

			claims, err := tokenService.VerifyAccessToken(token)
			if err != nil {
				next.ServeHTTP(w, r.WithContext(ctx))
				return
			}

			ctx = context.WithValue(ctx, "user_id", claims.UserID.String())
			// The signed-in device, so logout can end this session only.
			ctx = context.WithValue(ctx, "device_id", claims.DeviceID.String())
			next.ServeHTTP(w, r.WithContext(ctx))
		})
	}
}

// RequireAuth is a strict middleware applied to agent routes. It rejects any
// request where AuthMiddleware did not inject a valid user_id — i.e. missing,
// malformed, or expired tokens all receive 401 rather than reaching handlers.
func RequireAuth(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		uid, ok := r.Context().Value("user_id").(string)
		if !ok || uid == "" {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusUnauthorized)
			w.Write([]byte(`{"error":"authentication required: provide a valid Bearer token"}`))
			return
		}
		next.ServeHTTP(w, r)
	})
}

// RequireAdmin gates configuration endpoints to users with is_admin = true.
// Runs after RequireAuth, so user_id is guaranteed present.
func RequireAdmin(repo interface {
	IsAdmin(ctx context.Context, userID uuid.UUID) (bool, error)
}) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			uidStr, _ := r.Context().Value("user_id").(string)
			uid, err := uuid.Parse(uidStr)
			if err != nil {
				writeJSONError(w, http.StatusUnauthorized, "unauthorized")
				return
			}
			admin, err := repo.IsAdmin(r.Context(), uid)
			if err != nil {
				writeJSONError(w, http.StatusInternalServerError, "authorization check failed")
				return
			}
			if !admin {
				writeJSONError(w, http.StatusForbidden, "admin privileges required")
				return
			}
			next.ServeHTTP(w, r)
		})
	}
}

// RequireOrgRole gates organization-scoped routes to members holding at
// least minRole, reading the organization id from the "orgID" URL
// parameter. Runs after RequireAuth, so user_id is guaranteed present.
//
// A request from a non-member is rejected (403, not 404 — membership is an
// authorization fact, not a resource lookup, so it must not leak whether
// the organization id itself exists). A member below minRole is rejected
// the same way; only a sufficient role reaches the handler.
func RequireOrgRole(repo interface {
	MembershipRole(ctx context.Context, userID, orgID uuid.UUID) (organization.MembershipRole, error)
}, minRole organization.MembershipRole) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			uidStr, _ := r.Context().Value("user_id").(string)
			uid, err := uuid.Parse(uidStr)
			if err != nil {
				writeJSONError(w, http.StatusUnauthorized, "unauthorized")
				return
			}
			orgID, err := uuid.Parse(chi.URLParam(r, "orgID"))
			if err != nil {
				writeJSONError(w, http.StatusBadRequest, "invalid organization id")
				return
			}
			role, err := repo.MembershipRole(r.Context(), uid, orgID)
			if err != nil {
				if errors.Is(err, sql.ErrNoRows) {
					writeJSONError(w, http.StatusForbidden, "not a member of this organization")
					return
				}
				writeJSONError(w, http.StatusInternalServerError, "authorization check failed")
				return
			}
			if !role.AtLeast(minRole) {
				writeJSONError(w, http.StatusForbidden, "insufficient organization role")
				return
			}
			next.ServeHTTP(w, r)
		})
	}
}

func writeJSONError(w http.ResponseWriter, status int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = w.Write([]byte(`{"error":"` + msg + `"}`))
}

// integrationsSealer: what encrypts the keys of connected services. KMS when
// a key is configured (values sealed earlier by the passphrase still open),
// else the passphrase, else nothing (the server keeps no connections).
func integrationsSealer(cfg *config.Config) secretbox.Sealer {
	var legacy secretbox.Sealer
	if cfg.IntegrationsEncryptionKey != "" {
		legacy = secretbox.Passphrase(cfg.IntegrationsEncryptionKey)
	}
	if cfg.IntegrationsKMSKeyID == "" {
		return legacy
	}
	awsCfg, err := awsconfig.LoadDefaultConfig(context.Background())
	if err != nil {
		slog.Error("integrations: AWS settings unavailable; connections stay off", "error", err)
		return nil
	}
	slog.Info("integrations: keys sealed with KMS", "key", cfg.IntegrationsKMSKeyID, "earlier_values", legacy != nil)
	return secretbox.KMS{Client: kms.NewFromConfig(awsCfg), KeyID: cfg.IntegrationsKMSKeyID, Legacy: legacy}
}
