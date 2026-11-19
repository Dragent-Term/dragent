package components

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
	billingComponents "server/internal/billing/components"
	"server/internal/selfheal/model"
)

var (
	ErrTooManyConcurrentRequests = errors.New("maximum concurrent AI analysis limit reached (max 2)")
	ErrInsufficientWalletBalance = errors.New("insufficient wallet balance to perform AI analysis")
)

type AnalyzeBlockCommand struct {
	UserID     uuid.UUID
	Command    string
	Cwd        string
	ExitCode   int
	OutputText string
	ModelName  string // Selected by user: e.g. 'claude-3-5-sonnet', 'gpt-4o'
}

type AnalyzeBlockResult struct {
	BlockEventID     uuid.UUID
	Explanation      string
	SuggestedCommand *string
}

type SelfHealUseCase struct {
	repo       model.SelfHealRepository
	llmGateway model.LLMGateway
	billingUC  *billingComponents.BillingUseCase

	// Concurrency limiter per user
	concurrencyMu sync.Mutex
	activeUsers   map[uuid.UUID]int
}

func NewSelfHealUseCase(
	repo model.SelfHealRepository,
	llmGateway model.LLMGateway,
	billingUC *billingComponents.BillingUseCase,
) *SelfHealUseCase {
	return &SelfHealUseCase{
		repo:        repo,
		llmGateway:  llmGateway,
		billingUC:   billingUC,
		activeUsers: make(map[uuid.UUID]int),
	}
}

func (uc *SelfHealUseCase) Execute(ctx context.Context, cmd AnalyzeBlockCommand) (*AnalyzeBlockResult, error) {
	// 1. Record the block execution event in database
	eventID := uuid.New()
	truncated := false
	out := cmd.OutputText
	if len(out) > 5000 {
		out = out[:5000] // limit context block sizes
		truncated = true
	}

	event := &model.BlockEvent{
		ID:              eventID,
		UserID:          cmd.UserID,
		Command:         cmd.Command,
		Cwd:             cmd.Cwd,
		ExitCode:        cmd.ExitCode,
		OutputText:      out,
		OutputTruncated: truncated,
		CreatedAt:       time.Now(),
	}

	if err := uc.repo.CreateBlockEvent(ctx, event); err != nil {
		return nil, fmt.Errorf("failed to record block event: %w", err)
	}

	// If command succeeded (exit code 0), no healing is necessary
	if cmd.ExitCode == 0 {
		return &AnalyzeBlockResult{
			BlockEventID: eventID,
			Explanation:  "Command completed successfully.",
		}, nil
	}

	// 2. Enforce Concurrency Limit (Max 2 concurrent per user)
	if err := uc.acquireConcurrency(cmd.UserID); err != nil {
		return nil, err
	}
	defer uc.releaseConcurrency(cmd.UserID)

	// 3. Resolve the model FIRST so the gate prices what will actually run.
	//
	// Free-tier users are pinned to the configured free model, exactly as the
	// task path does. Without this the model name came straight from the
	// client, so a free user could ask for (and be served) Opus on our keys.
	// GetOrInitBalance is the single source of truth for "fetch, or lazily
	// create on first use" — resolving org context correctly (a balance row
	// is keyed (user_id, organization_id)) is its job alone, not something
	// to reimplement per caller. A balance we cannot read is NOT a free
	// balance: failing open on the one check that decides whether the user
	// can pay would turn a transient DB error into an unbilled LLM call.
	balance, err := uc.billingUC.GetOrInitBalance(ctx, cmd.UserID)
	if err != nil {
		return nil, fmt.Errorf("failed to read user balance: %w", err)
	}

	modelName := cmd.ModelName
	if uc.billingUC.RestrictedToFreeModel(ctx, balance) {
		// No premium-capable funds → the free model, same rule as the agent path.
		modelName = uc.billingUC.FreeTierModel(ctx)
	} else if modelName == "" {
		// Configured self-heal model (config selfheal_model) — swappable in the
		// DB when a model is deprecated. It must be a cataloged model, or the
		// charge is refused (unknown models are not fallback-priced).
		modelName = uc.billingUC.SelfHealModel(ctx)
	}

	// 4. The SAME pre-flight the task path uses. This replaces a hand-rolled
	// HasAvailableCredits check against a hardcoded 5,000 mc minimum, which
	// ignored the model's real min_balance_mc, never advanced expired windows
	// (so a user whose window had already rolled was refused), never ran the
	// monthly renewal (so a paid user past their reset date stayed locked out),
	// and never validated the markup config.
	if gateErr := uc.billingUC.PreflightGate(ctx, cmd.UserID, modelName); gateErr != nil {
		return nil, gateErr
	}

	// 5. Invoke LLM diagnostic failure explanation.
	analysis, err := uc.llmGateway.AnalyzeFailure(ctx, modelName, cmd.Command, cmd.Cwd, out)
	if err != nil {
		return nil, fmt.Errorf("failed to perform LLM analysis: %w", err)
	}

	// 6. Settle. Use Settle rather than ChargeUser: the work has already been
	// delivered, so a billing failure must not fail the request — but it MUST
	// be logged, which is exactly what the discarded `_, _ = ChargeUser(...)`
	// here never did, making it silent uncollected revenue.
	uc.billingUC.Settle(ctx, billingComponents.ChargeUserCommand{
		UserID:            cmd.UserID,
		ModelName:         modelName,
		InputTokens:       analysis.InputTokens,
		CachedInputTokens: 0,
		OutputTokens:      analysis.OutputTokens,
	})

	// 6. Record and persist diagnostic analysis result
	analysisBlock := &model.BlockAnalysis{
		ID:               uuid.New(),
		BlockEventID:     eventID,
		Explanation:      analysis.Explanation,
		SuggestedCommand: analysis.SuggestedCommand,
		CreatedAt:        time.Now(),
	}

	if err := uc.repo.CreateBlockAnalysis(ctx, analysisBlock); err != nil {
		return nil, fmt.Errorf("failed to persist diagnostic card details: %w", err)
	}

	return &AnalyzeBlockResult{
		BlockEventID:     eventID,
		Explanation:      analysisBlock.Explanation,
		SuggestedCommand: analysisBlock.SuggestedCommand,
	}, nil
}

// ── AgentStep ─────────────────────────────────────────────────────────────────

// AgentStepCommand is the per-turn request from the client in the agentic loop.
type AgentStepCommand struct {
	UserID    uuid.UUID
	ModelName string
	System    string
	Messages  []model.AgentMessage
	Tools     []model.AgentTool
	// UserApiKey is the user's own provider API key forwarded from the client request.
	// When non-empty it overrides the server's shared key for this turn only.
	UserApiKey string
	// CustomBaseUrl, when non-empty, overrides the provider endpoint with an
	// OpenAI-compatible base URL (e.g. for Ollama or LM Studio).
	CustomBaseUrl string
}

// AgentStepResult is the per-turn response sent back to the client.
type AgentStepResult struct {
	ToolCalls    []model.AgentToolCall
	Content      string
	InputTokens  int64
	OutputTokens int64
}

// AgentStep runs one LLM turn in the agentic self-heal loop.
// The client maintains the conversation history, executes any tool calls
// locally, and calls AgentStep again with the results until Content is set.
func (uc *SelfHealUseCase) AgentStep(ctx context.Context, cmd AgentStepCommand) (*AgentStepResult, error) {
	if err := uc.acquireConcurrency(cmd.UserID); err != nil {
		return nil, err
	}
	defer uc.releaseConcurrency(cmd.UserID)

	// ── Billing gate (server-key path only) ──────────────────────────────────
	// The self-heal path MUST enforce the same guards as task creation, or a
	// free-tier user's "Fix automatically" sends whatever model the client
	// picked (e.g. claude-sonnet-4-6) straight to the provider — which both
	// bypasses free-tier restriction AND fails (free tier isn't authorized for
	// it). When the user supplies their own key they pay their own provider, so
	// the gate is skipped, mirroring CreateTask.
	if cmd.UserApiKey == "" {
		balance, err := uc.billingUC.GetOrInitBalance(ctx, cmd.UserID)
		if err != nil {
			return nil, fmt.Errorf("fetch balance: %w", err)
		}
		// No premium-capable funds → restricted to the configured free model,
		// regardless of what the client requested (same rule as the agent path).
		// Resolve the model FIRST so the pre-flight prices the call that runs.
		if uc.billingUC.RestrictedToFreeModel(ctx, balance) {
			cmd.ModelName = uc.billingUC.FreeTierModel(ctx)
		} else if cmd.ModelName == "" {
			cmd.ModelName = uc.billingUC.DefaultAgentModel(ctx)
		}
		// Pre-flight gate: authorise the call against the model's minimum
		// balance (no cost estimate). On exhaustion the window-specific limit
		// error is returned and the balance is left untouched.
		if gateErr := uc.billingUC.PreflightGate(ctx, cmd.UserID, cmd.ModelName); gateErr != nil {
			return nil, gateErr
		}
	}

	resp, err := uc.llmGateway.AgentStep(ctx, model.AgentStepRequest{
		ModelName:     cmd.ModelName,
		System:        cmd.System,
		Messages:      cmd.Messages,
		Tools:         cmd.Tools,
		UserApiKey:    cmd.UserApiKey,
		CustomBaseUrl: cmd.CustomBaseUrl,
	})
	if err != nil {
		return nil, fmt.Errorf("agent step failed: %w", err)
	}

	// Debit wallet for tokens used
	if resp.InputTokens+resp.OutputTokens > 0 && cmd.ModelName != "" {
		_, _ = uc.billingUC.ChargeUser(ctx, billingComponents.ChargeUserCommand{
			UserID:       cmd.UserID,
			ModelName:    cmd.ModelName,
			InputTokens:  resp.InputTokens,
			OutputTokens: resp.OutputTokens,
		})
	}

	return &AgentStepResult{
		ToolCalls:    resp.ToolCalls,
		Content:      resp.Content,
		InputTokens:  resp.InputTokens,
		OutputTokens: resp.OutputTokens,
	}, nil
}

func (uc *SelfHealUseCase) acquireConcurrency(userID uuid.UUID) error {
	uc.concurrencyMu.Lock()
	defer uc.concurrencyMu.Unlock()

	active := uc.activeUsers[userID]
	if active >= 2 {
		return ErrTooManyConcurrentRequests
	}
	uc.activeUsers[userID] = active + 1
	return nil
}

func (uc *SelfHealUseCase) releaseConcurrency(userID uuid.UUID) {
	uc.concurrencyMu.Lock()
	defer uc.concurrencyMu.Unlock()

	active := uc.activeUsers[userID]
	if active > 0 {
		uc.activeUsers[userID] = active - 1
	}
}
