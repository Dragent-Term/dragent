package pipeline

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"sort"
	"strings"
	"sync"
	"time"

	agentctx "server/internal/agent/context"
	"server/internal/agent/permissions"
	"server/internal/agent/providers"
	"server/internal/agent/webtools"
)

// AgentMode controls which tools the LLM sees for a given task.
type AgentMode string

const (
	AgentModeNormal      AgentMode = ""            // all tools
	AgentModeCoordinator AgentMode = "coordinator" // read-only + spawn_subagent; no write/exec
	AgentModeWorker      AgentMode = "worker"      // all tools except spawn_subagent
)

// coordinatorBannedTools are removed from the LLM's tool list when in Coordinator mode.
// The Coordinator must delegate writes and execution to Worker sub-agents via spawn_subagent.
var coordinatorBannedTools = map[string]bool{
	"write_file":  true,
	"edit_file":   true,
	"batch_edit":  true,
	"apply_patch": true,
	"run_command": true,
}

// Runner executes the agentic tool-call loop for one task.
//
// Tool execution is fully delegated to the client: when the LLM requests a
// tool call, the runner emits a KindToolCall SSE event and blocks until the
// client POSTs the result via POST /api/v1/agent/tasks/{id}/tool_results.
// This allows the Go server to run on AWS while the client reads/writes files
// on the user's local machine.
type Runner struct {
	provider  providers.LLMProvider
	bareModel string
	policy    *permissions.Policy
	mode      AgentMode
	emit      func(AgentEvent)
	// extraTools holds additional tool definitions (e.g. from MCP servers) that
	// are appended to the standard tool set for this task.
	extraTools    []providers.ToolDefinition
	allowedTools  map[string]struct{}
	restrictTools bool
	// toolResultGate blocks the loop until the client posts each tool result.
	toolResultGate *toolResultGate

	// effort is the user-selected reasoning depth ("" = provider default).
	effort providers.Effort

	// Billing hooks (both optional; nil = unmetered, e.g. BYOK or tests).
	// preflight runs before EVERY provider call and authorises it against the
	// model's minimum balance (no cost estimate); a non-nil error stops the task
	// gracefully with that message. onStepUsage runs after every completed step
	// with the actual token counts so the cost is deducted immediately.
	preflight     func() error
	onStepUsage   func(inputTokens, cachedInputTokens, outputTokens int64)
	onStepAborted func(reason string)

	// webTools executes web_search / fetch_url ON THE SERVER via the active
	// provider's native web tooling on a small model (see webtools package).
	// Nil = delegate to the client like any other tool (tests, legacy paths).
	webTools *webtools.Executor
	// onWebUsage settles a web sub-call's spend: tokens PLUS the provider's
	// flat per-search fee (which onStepUsage cannot express). Optional;
	// nil = web sub-calls go unmetered.
	beforeWeb    func(callID string) error
	onWebUsage   func(callID string, usage webtools.Usage)
	onWebAborted func(callID, reason string)

	// checkpoint persists the complete, provider-ready working state at safe
	// execution boundaries. It is synchronous by design: the next model call or
	// tool side effect must not start until the state needed to resume it has
	// been committed durably.
	checkpoint func(RunCheckpoint) error

	// Read-only results are reusable within a run until a mutating tool is
	// requested. This is a semantic loop guard, not a call-count limit: novel
	// exploration remains unlimited while exact repeated reads cannot trigger
	// the same local work over and over under fresh provider call IDs.
	readCache      map[string]toolResultPayload
	readOnlyStreak int
	modelLimits    *agentctx.ModelLimits
	observe        func(TrajectoryEvent)
	desktopOwned   bool
}

// RunPhase describes what may safely happen after a runner is restored.
// Checkpoints are only written at boundaries where replay cannot duplicate a
// completed local side effect.
type RunPhase string

const (
	RunPhaseReadyModel    RunPhase = "ready_model"
	RunPhaseCallingModel  RunPhase = "calling_model"
	RunPhaseAwaitingTools RunPhase = "awaiting_tools"
	RunPhaseFinalizing    RunPhase = "finalizing"
	RunPhaseCompleted     RunPhase = "completed"
	RunPhaseInterrupted   RunPhase = "interrupted"
)

const RunCheckpointSchemaVersion = 1

// RunCheckpoint is the authoritative working state for one agent run. Events
// remain the UI/audit stream; this projection is what makes execution
// resumable without asking the model to reconstruct its own past from prose.
type RunCheckpoint struct {
	SchemaVersion          int                        `json:"schema_version"`
	Phase                  RunPhase                   `json:"phase"`
	Round                  int                        `json:"round"`
	System                 string                     `json:"system"`
	Messages               []providers.Message        `json:"messages"`
	PendingToolCalls       []providers.ToolCall       `json:"pending_tool_calls,omitempty"`
	TotalInputTokens       int64                      `json:"total_input_tokens"`
	TotalCachedInputTokens int64                      `json:"total_cached_input_tokens"`
	TotalOutputTokens      int64                      `json:"total_output_tokens"`
	LastPromptTokens       int64                      `json:"last_prompt_tokens"`
	Mode                   AgentMode                  `json:"mode,omitempty"`
	Effort                 providers.Effort           `json:"effort,omitempty"`
	ExtraTools             []providers.ToolDefinition `json:"extra_tools,omitempty"`
	AllowedTools           []string                   `json:"allowed_tools,omitempty"`
	RestrictTools          bool                       `json:"restrict_tools,omitempty"`
}

// SetBillingHooks installs the per-call gate and per-step settlement callbacks.
func (r *Runner) SetBillingHooks(
	preflight func() error,
	onStepUsage func(inputTokens, cachedInputTokens, outputTokens int64),
	onStepAborted func(reason string),
) {
	r.preflight = preflight
	r.onStepUsage = onStepUsage
	r.onStepAborted = onStepAborted
}

// SetWebTools installs the server-side web tool executor and the settlement
// callback for its sub-call usage (tokens + flat per-search provider fees).
func (r *Runner) SetWebTools(e *webtools.Executor, before func(callID string) error,
	onUsage func(callID string, usage webtools.Usage), onAborted func(callID, reason string)) {
	r.webTools = e
	r.beforeWeb = before
	r.onWebUsage = onUsage
	r.onWebAborted = onAborted
}

func NewRunner(
	provider providers.LLMProvider,
	bareModel string,
	policy *permissions.Policy,
	mode AgentMode,
	emit func(AgentEvent),
	extraTools []providers.ToolDefinition,
) *Runner {
	return &Runner{
		provider:       provider,
		bareModel:      bareModel,
		policy:         policy,
		mode:           mode,
		emit:           emit,
		extraTools:     extraTools,
		toolResultGate: newToolResultGate(),
		readCache:      make(map[string]toolResultPayload),
	}
}

// Gate returns the tool result gate so the HTTP handler can resolve tool results.
func (r *Runner) Gate() *toolResultGate { return r.toolResultGate }

// SetEffort sets the user-selected reasoning depth for every provider call in
// this task. Adapters drop it for models that reject the parameter, so an
// unsupported pairing degrades to the provider default instead of failing.
func (r *Runner) SetEffort(e providers.Effort) { r.effort = e }

func (r *Runner) SetModelLimits(limits agentctx.ModelLimits) { r.modelLimits = &limits }

func (r *Runner) SetTrajectoryObserver(observer func(TrajectoryEvent)) { r.observe = observer }

// SetDesktopOwned limits this runner to one provider step. The complete loop
// and provider-ready history remain on the user's machine.
func (r *Runner) SetDesktopOwned(enabled bool) { r.desktopOwned = enabled }

// SetAllowedTools applies a provider-visible capability boundary. Calling it
// with an empty slice is intentional and advertises no tools.
func (r *Runner) SetAllowedTools(names []string) {
	r.restrictTools = true
	r.allowedTools = make(map[string]struct{}, len(names))
	for _, name := range names {
		r.allowedTools[name] = struct{}{}
	}
}

func (r *Runner) trace(event TrajectoryEvent) {
	if r.observe != nil {
		r.observe(event)
	}
}

// SetCheckpointHook installs the durable safe-boundary writer. A write error
// stops the run before the next provider call or tool execution.
func (r *Runner) SetCheckpointHook(fn func(RunCheckpoint) error) { r.checkpoint = fn }

func (r *Runner) saveCheckpoint(cp RunCheckpoint) bool {
	if r.checkpoint == nil {
		return true
	}
	cp.SchemaVersion = RunCheckpointSchemaVersion
	cp.Mode = r.mode
	cp.Effort = r.effort
	cp.ExtraTools = r.extraTools
	cp.RestrictTools = r.restrictTools
	cp.AllowedTools = cp.AllowedTools[:0]
	if r.restrictTools {
		cp.AllowedTools = make([]string, 0, len(r.allowedTools))
		for name := range r.allowedTools {
			cp.AllowedTools = append(cp.AllowedTools, name)
		}
		sort.Strings(cp.AllowedTools)
	}
	if err := r.checkpoint(cp); err != nil {
		log.Printf("[runner] durable checkpoint failed phase=%s round=%d: %v", cp.Phase, cp.Round, err)
		r.emit(AgentEvent{Kind: KindError, Error: "agent state could not be saved safely; retry to continue from the last checkpoint", Rounds: cp.Round})
		return false
	}
	r.trace(TrajectoryEvent{Kind: "checkpoint.saved", Phase: cp.Phase, Round: cp.Round})
	return true
}

// Run executes the loop. Safe to call from a goroutine.
func (r *Runner) Run(ctx context.Context, system string, messages []providers.Message) {
	r.run(ctx, RunCheckpoint{System: system, Messages: messages, Phase: RunPhaseReadyModel})
}

// Resume continues from a validated safe-boundary checkpoint. An interrupted
// provider call is retried as the same logical round; completed tool results
// must already have been folded into a ReadyModel checkpoint by the manager.
func (r *Runner) Resume(ctx context.Context, checkpoint RunCheckpoint) {
	if checkpoint.SchemaVersion != RunCheckpointSchemaVersion {
		r.emit(AgentEvent{Kind: KindError, Error: "saved agent state uses an unsupported version"})
		return
	}
	if checkpoint.Phase == RunPhaseFinalizing {
		r.emit(AgentEvent{
			Kind: KindDone, TotalInputTokens: checkpoint.TotalInputTokens,
			TotalCachedInputTokens: checkpoint.TotalCachedInputTokens,
			TotalOutputTokens:      checkpoint.TotalOutputTokens,
			ContextTokens:          checkpoint.LastPromptTokens, Rounds: checkpoint.Round,
		})
		r.trace(TrajectoryEvent{Kind: "run.completed", Phase: RunPhaseCompleted, Round: checkpoint.Round})
		checkpoint.Phase = RunPhaseCompleted
		checkpoint.SchemaVersion = RunCheckpointSchemaVersion
		if r.checkpoint != nil {
			if err := r.checkpoint(checkpoint); err != nil {
				log.Printf("[runner] resumed completion projection failed: %v", err)
			}
		}
		return
	}
	if checkpoint.Phase != RunPhaseReadyModel && checkpoint.Phase != RunPhaseCallingModel {
		r.emit(AgentEvent{Kind: KindError, Error: "saved agent state is not at a safe resume boundary"})
		return
	}
	if checkpoint.RestrictTools {
		r.SetAllowedTools(checkpoint.AllowedTools)
	}
	r.run(ctx, checkpoint)
}

func (r *Runner) run(ctx context.Context, initial RunCheckpoint) {
	toolDefs := r.buildToolDefs()
	system := initial.System
	history := make([]providers.Message, len(initial.Messages))
	copy(history, initial.Messages)

	limits := agentctx.For(r.bareModel)
	if r.modelLimits != nil {
		limits = *r.modelLimits
	}

	totalInput := initial.TotalInputTokens
	totalCachedInput := initial.TotalCachedInputTokens
	totalOutput := initial.TotalOutputTokens
	// lastPromptTokens is the prompt size (input + cached) of the most recent
	// round — the CURRENT context size, reported to the client on KindDone for
	// the context-window meter. Not summed across rounds.
	lastPromptTokens := initial.LastPromptTokens
	round := initial.Round
	var desktopCompactedHistory []providers.Message
	if initial.Phase == RunPhaseCallingModel && round > 0 {
		// CallingModel means that round was entered but no provider result was
		// committed. Re-enter it with the same logical round number.
		round--
	}

	for {
		estimated := agentctx.BudgetUsed(system, history, toolDefs)
		if estimated > limits.CompactionThreshold() {
			log.Printf("[runner] compacting context: estimated=%d threshold=%d model=%s",
				estimated, limits.CompactionThreshold(), r.bareModel)
			if r.preflight != nil {
				if pfErr := r.preflight(); pfErr != nil {
					r.emit(AgentEvent{Kind: KindError, Error: pfErr.Error(), Rounds: round})
					return
				}
			}
			result, compactErr := agentctx.Compact(ctx, r.provider, r.bareModel, system, history, toolDefs, limits)
			if compactErr != nil {
				log.Printf("[runner] compaction error (continuing with original history): %v", compactErr)
				if r.onStepAborted != nil {
					r.onStepAborted("compaction failed before usage was reported")
				}
			} else {
				history = result.Messages
				if r.desktopOwned {
					desktopCompactedHistory = append([]providers.Message(nil), history...)
				}
				log.Printf("[runner] compacted to %d messages (~%d tokens)", len(history), result.EstimatedTokens)
				// The summarization call spent real provider tokens on the
				// user's behalf — settle it like any step (previously this
				// call was invisible to billing and the platform ate it).
				if r.onStepUsage != nil && result.InputTokens+result.OutputTokens > 0 {
					log.Printf(
						"[compact-tokens] model=%s input=%d cached_input=%d output=%d",
						r.bareModel, result.InputTokens, result.CachedInputTokens, result.OutputTokens,
					)
					r.onStepUsage(result.InputTokens, result.CachedInputTokens, result.OutputTokens)
				} else if r.onStepAborted != nil {
					r.onStepAborted("compaction returned no billable usage")
				}
			}
		}

		if !r.saveCheckpoint(RunCheckpoint{
			Phase: RunPhaseReadyModel, Round: round, System: system, Messages: history,
			TotalInputTokens: totalInput, TotalCachedInputTokens: totalCachedInput,
			TotalOutputTokens: totalOutput, LastPromptTokens: lastPromptTokens,
		}) {
			return
		}

		// Pre-flight billing gate: authorise the call against the model's minimum
		// balance BEFORE hitting the provider. On exhaustion the task ends
		// gracefully: everything streamed so far is preserved and the client
		// shows the limit message.
		if r.preflight != nil {
			if pfErr := r.preflight(); pfErr != nil {
				log.Printf("[runner] preflight blocked round %d: %v", round+1, pfErr)
				r.emit(AgentEvent{
					Kind:                   KindError,
					Error:                  pfErr.Error(),
					TotalInputTokens:       totalInput,
					TotalCachedInputTokens: totalCachedInput,
					TotalOutputTokens:      totalOutput,
					Rounds:                 round,
				})
				return
			}
		}

		round++
		if !r.saveCheckpoint(RunCheckpoint{
			Phase: RunPhaseCallingModel, Round: round, System: system, Messages: history,
			TotalInputTokens: totalInput, TotalCachedInputTokens: totalCachedInput,
			TotalOutputTokens: totalOutput, LastPromptTokens: lastPromptTokens,
		}) {
			return
		}
		providerStarted := time.Now()
		r.trace(TrajectoryEvent{Kind: "provider.started", Phase: RunPhaseCallingModel, Round: round})
		stream, err := r.provider.StreamStep(ctx, providers.StepRequest{
			ModelName: r.bareModel,
			System:    system,
			Messages:  history,
			Tools:     toolDefs,
			Effort:    r.effort,
		})
		if err != nil {
			log.Printf("[runner-latency] round=%d provider_start_failed_after=%s error=%v", round, time.Since(providerStarted), err)
			if r.onStepAborted != nil {
				r.onStepAborted("provider rejected the call before usage was reported")
			}
			r.emit(AgentEvent{Kind: KindError, Error: err.Error()})
			return
		}
		log.Printf("[runner-latency] round=%d provider_stream_opened_in=%s", round, time.Since(providerStarted))

		result, done, announced := r.consumeStream(ctx, stream, round, providerStarted)
		if done {
			if r.onStepAborted != nil {
				r.onStepAborted("provider stream ended before usage was reported")
			}
			return
		}
		if result == nil {
			if r.onStepAborted != nil {
				r.onStepAborted("provider returned no usage result")
			}
			return
		}

		totalInput += result.InputTokens
		totalCachedInput += result.CachedInputTokens
		totalOutput += result.OutputTokens
		r.trace(TrajectoryEvent{Kind: "provider.completed", Round: round, DurationMs: time.Since(providerStarted).Milliseconds()})
		lastPromptTokens = result.InputTokens + result.CachedInputTokens

		// Per-step token audit line — one per LLM call INSIDE the agent loop,
		// with running task totals, so spend is traceable step by step in the
		// logs as it happens (the provider layer's [provider-tokens] line has
		// the raw counts; this one adds the loop context).
		log.Printf(
			"[step-tokens] round=%d model=%s input=%d cached_input=%d output=%d step_total=%d task_total=%d",
			round, r.bareModel,
			result.InputTokens, result.CachedInputTokens, result.OutputTokens,
			result.InputTokens+result.CachedInputTokens+result.OutputTokens,
			totalInput+totalCachedInput+totalOutput,
		)

		// Settle THIS step's actual cost immediately (zero-buffer billing) —
		// the next iteration's pre-flight then judges a current balance.
		if r.onStepUsage != nil {
			r.onStepUsage(result.InputTokens, result.CachedInputTokens, result.OutputTokens)
		}

		if len(result.ToolCalls) == 0 {
			if result.Content != "" {
				history = append(history, providers.Message{
					Role:    providers.RoleAssistant,
					Content: result.Content,
				})
			}
			if !r.saveCheckpoint(RunCheckpoint{
				Phase: RunPhaseFinalizing, Round: round, System: system, Messages: history,
				TotalInputTokens: totalInput, TotalCachedInputTokens: totalCachedInput,
				TotalOutputTokens: totalOutput, LastPromptTokens: lastPromptTokens,
			}) {
				return
			}
			r.emit(AgentEvent{
				Kind:                   KindDone,
				TotalInputTokens:       totalInput,
				TotalCachedInputTokens: totalCachedInput,
				TotalOutputTokens:      totalOutput,
				ContextTokens:          lastPromptTokens,
				Rounds:                 round,
			})
			r.trace(TrajectoryEvent{Kind: "run.completed", Phase: RunPhaseCompleted, Round: round})
			// KindDone is synchronously appended by Task.broadcast. Only after the
			// terminal stream event is durable can this state become non-resumable.
			completed := RunCheckpoint{
				Phase: RunPhaseCompleted, Round: round, System: system, Messages: history,
				TotalInputTokens: totalInput, TotalCachedInputTokens: totalCachedInput,
				TotalOutputTokens: totalOutput, LastPromptTokens: lastPromptTokens,
			}
			completed.SchemaVersion = RunCheckpointSchemaVersion
			if r.checkpoint != nil {
				if err := r.checkpoint(completed); err != nil {
					// The finalizing checkpoint and KindDone event are already durable.
					// Keep execution successful; a later repair can project completed.
					log.Printf("[runner] completed checkpoint projection failed round=%d: %v", round, err)
				}
			}
			return
		}
		if r.desktopOwned {
			if !r.delegateDesktopStep(ctx, result.ToolCalls, announced, round) {
				return
			}
			r.emit(AgentEvent{
				Kind:                   KindStepDone,
				AssistantContent:       result.Content,
				TotalInputTokens:       result.InputTokens,
				TotalCachedInputTokens: result.CachedInputTokens,
				TotalOutputTokens:      result.OutputTokens,
				ContextTokens:          lastPromptTokens,
				Rounds:                 1,
				CompactedMessages:      desktopCompactedHistory,
			})
			return
		}
		history = append(history, providers.Message{
			Role:      providers.RoleAssistant,
			Content:   result.Content,
			ToolCalls: result.ToolCalls,
		})
		if !r.saveCheckpoint(RunCheckpoint{
			Phase: RunPhaseAwaitingTools, Round: round, System: system, Messages: history,
			PendingToolCalls: result.ToolCalls, TotalInputTokens: totalInput,
			TotalCachedInputTokens: totalCachedInput, TotalOutputTokens: totalOutput,
			LastPromptTokens: lastPromptTokens,
		}) {
			return
		}

		toolResults, abort := r.delegateTools(ctx, result.ToolCalls, announced, round)
		if abort {
			return
		}

		history = append(history, toolResults...)
		if !r.saveCheckpoint(RunCheckpoint{
			Phase: RunPhaseReadyModel, Round: round, System: system, Messages: history,
			TotalInputTokens: totalInput, TotalCachedInputTokens: totalCachedInput,
			TotalOutputTokens: totalOutput, LastPromptTokens: lastPromptTokens,
		}) {
			return
		}
	}
}

// delegateDesktopStep exposes local calls without waiting for their results.
// Server-native web calls execute within this request and return their full
// result only over the live stream; no task/event/tool-result row is created.
func (r *Runner) delegateDesktopStep(ctx context.Context, calls []providers.ToolCall, announced map[string]bool, round int) bool {
	for _, tc := range calls {
		serverExecuted := r.webTools != nil && r.webTools.Handles(tc.Name)
		if !announced[tc.ID] {
			r.emit(AgentEvent{Kind: KindToolCallStarted, ToolID: tc.ID, ToolName: tc.Name, ToolIcon: toolIcon(tc.Name)})
		}
		r.emit(AgentEvent{
			Kind: KindToolCall, ToolID: tc.ID, ToolName: tc.Name,
			ToolIcon: toolIcon(tc.Name), ToolArgs: tc.ArgsJSON,
			ArgsSummary:      argsSummary(tc.Name, tc.ArgsJSON),
			RequiresApproval: !serverExecuted && r.policy.RequiresApproval(tc.Name),
			ServerExecuted:   serverExecuted,
		})
		r.trace(TrajectoryEvent{Kind: "tool.requested", Round: round, ToolID: tc.ID, ToolName: tc.Name})
		if !serverExecuted {
			continue
		}

		started := time.Now()
		output, isError, usage := r.webTools.ExecuteGated(ctx, tc.Name, tc.ArgsJSON, func() error {
			if r.beforeWeb == nil {
				return nil
			}
			return r.beforeWeb(tc.ID)
		})
		if ctx.Err() != nil {
			r.emit(AgentEvent{Kind: KindError, Error: "desktop step cancelled during web tool execution"})
			return false
		}
		if r.onWebUsage != nil && usage.InputTokens+usage.OutputTokens+usage.SearchCount > 0 {
			r.onWebUsage(tc.ID, usage)
		} else if r.onWebAborted != nil {
			r.onWebAborted(tc.ID, "web provider returned no billable usage")
		}
		summary := output
		if len([]rune(summary)) > 200 {
			summary = string([]rune(summary)[:200]) + "…"
		}
		r.emit(AgentEvent{Kind: KindToolEnd, ToolID: tc.ID, ToolName: tc.Name,
			ResultSummary: summary, DurationMs: time.Since(started).Milliseconds(),
			ToolOutput: output, ToolIsError: isError})
		r.trace(TrajectoryEvent{Kind: "tool.completed", Round: round, ToolID: tc.ID,
			ToolName: tc.Name, DurationMs: time.Since(started).Milliseconds()})
	}
	return true
}

// consumeStream drains the provider stream, forwarding events to the emitter.
// Returns the final StepResult and a bool indicating whether the caller should stop.
//
// EventToolStart is emitted by the streaming reader as soon as the LLM outputs a
// tool call ID + name (before args are fully streamed). We forward it immediately as
// KindToolCallStarted so the client can show a pending indicator — the client must NOT
// execute yet, it waits for KindToolCall (sent later by delegateTools with full args).
func (r *Runner) consumeStream(ctx context.Context, stream <-chan providers.StreamEvent, round int, providerStarted time.Time) (*providers.StepResult, bool, map[string]bool) {
	announced := make(map[string]bool) // tool IDs already sent as KindToolCallStarted
	firstEvent := true
	for {
		select {
		case <-ctx.Done():
			r.emit(AgentEvent{Kind: KindError, Error: "cancelled"})
			return nil, true, nil
		case ev, ok := <-stream:
			if !ok {
				return nil, true, nil
			}
			if firstEvent {
				firstEvent = false
				log.Printf(
					"[runner-latency] round=%d provider_first_event=%s time_to_first_event=%s",
					round, ev.Kind, time.Since(providerStarted),
				)
			}
			switch ev.Kind {
			case providers.EventThinking:
				r.emit(AgentEvent{Kind: KindThinking, Text: ev.Text})

			case providers.EventTextDelta:
				r.emit(AgentEvent{Kind: KindTextDelta, Text: ev.Text})

			case providers.EventToolStart:
				// Early notification: args are still streaming, client shows spinner only.
				if ev.ToolCall != nil && !announced[ev.ToolCall.ID] {
					announced[ev.ToolCall.ID] = true
					r.emit(AgentEvent{
						Kind:     KindToolCallStarted,
						ToolID:   ev.ToolCall.ID,
						ToolName: ev.ToolCall.Name,
						ToolIcon: toolIcon(ev.ToolCall.Name),
					})
				}

			case providers.EventDone:
				return ev.Result, false, announced

			case providers.EventError:
				msg := "stream error"
				if ev.Err != nil {
					msg = ev.Err.Error()
				}
				r.emit(AgentEvent{Kind: KindError, Error: msg})
				return nil, true, nil
			}
		}
	}
}

// delegateTools dispatches all tool calls simultaneously, then waits for all
// results in parallel. Web tools (web_search / fetch_url) execute ON THE
// SERVER via the webtools executor; every other tool is delegated to the
// client, which executes locally and POSTs the result back.
//
// announced is the set of tool IDs that already received a KindToolCallStarted event
// during streaming. delegateTools still emits KindToolCall (with full args) for every
// call — KindToolCallStarted was display-only; KindToolCall is the execution trigger
// for client tools, and display-only (ServerExecuted=true) for server tools.
func (r *Runner) delegateTools(ctx context.Context, calls []providers.ToolCall, announced map[string]bool, round int) ([]providers.Message, bool) {
	serverSide := func(tc providers.ToolCall) bool {
		return r.webTools != nil && r.webTools.Handles(tc.Name)
	}
	cacheableRead := func(name string) bool {
		switch name {
		case "read_file", "grep", "search_files", "search_codebase", "list_directory",
			"list_directory_with_sizes", "directory_tree", "get_file_info",
			"list_allowed_directories":
			return true
		default:
			return false
		}
	}
	mutating := false
	for _, tc := range calls {
		if !cacheableRead(tc.Name) && !serverSide(tc) {
			mutating = true
			break
		}
	}
	if mutating {
		clear(r.readCache)
		r.readOnlyStreak = 0
	}
	cacheKey := func(tc providers.ToolCall) string { return tc.Name + "\x00" + tc.ArgsJSON }
	cached := make(map[string]toolResultPayload)

	// 1. Emit KindToolCall with full args so the client can execute in parallel.
	//    For tools already announced via KindToolCallStarted, the client upgrades
	//    the pending spinner to an executing state; for others it creates the entry.
	//    Server-executed calls carry ServerExecuted=true: the client renders the
	//    tool card but must NOT execute or post a result.
	starts := make(map[string]time.Time, len(calls))
	for _, tc := range calls {
		starts[tc.ID] = time.Now()
		if cacheableRead(tc.Name) {
			if result, ok := r.readCache[cacheKey(tc)]; ok {
				cached[tc.ID] = result
				log.Printf("[runner] reusing unchanged read %q (id=%s)", tc.Name, tc.ID)
				r.emit(AgentEvent{
					Kind: KindToolCall, ToolID: tc.ID, ToolName: tc.Name,
					ToolIcon: toolIcon(tc.Name), ToolArgs: tc.ArgsJSON,
					ArgsSummary: argsSummary(tc.Name, tc.ArgsJSON), ServerExecuted: true,
				})
				r.trace(TrajectoryEvent{Kind: "tool.requested", Round: round, ToolID: tc.ID, ToolName: tc.Name, Reused: true, ReadOnlyStreak: r.readOnlyStreak})
				continue
			}
		}
		argsPreview := tc.ArgsJSON
		if len(argsPreview) > 300 {
			argsPreview = argsPreview[:300] + "…"
		}
		log.Printf("[runner] LLM called tool %q (id=%s) server=%v args=%s", tc.Name, tc.ID, serverSide(tc), argsPreview)
		r.emit(AgentEvent{
			Kind:             KindToolCall,
			ToolID:           tc.ID,
			ToolName:         tc.Name,
			ToolIcon:         toolIcon(tc.Name),
			ToolArgs:         tc.ArgsJSON,
			ArgsSummary:      argsSummary(tc.Name, tc.ArgsJSON),
			RequiresApproval: !serverSide(tc) && r.policy.RequiresApproval(tc.Name),
			ServerExecuted:   serverSide(tc),
		})
		r.trace(TrajectoryEvent{Kind: "tool.requested", Round: round, ToolID: tc.ID, ToolName: tc.Name})
	}

	// 2. Wait for all results concurrently — each call gets its own goroutine.
	//    Client tools block on the result gate; server tools run the webtools
	//    executor directly.
	type collectedResult struct {
		callID string
		result toolResultPayload
		usage  webtools.Usage
		ok     bool
		web    bool
	}
	ch := make(chan collectedResult, len(calls))
	waiting := 0
	for _, tc := range calls {
		if _, ok := cached[tc.ID]; ok {
			continue
		}
		waiting++
		if serverSide(tc) {
			go func(tc providers.ToolCall) {
				output, isErr, usage := r.webTools.ExecuteGated(ctx, tc.Name, tc.ArgsJSON, func() error {
					if r.beforeWeb == nil {
						return nil
					}
					return r.beforeWeb(tc.ID)
				})
				ch <- collectedResult{tc.ID, toolResultPayload{Output: output, IsError: isErr}, usage, ctx.Err() == nil, true}
			}(tc)
			continue
		}
		go func(callID string) {
			res, ok := r.toolResultGate.wait(ctx, callID)
			ch <- collectedResult{callID: callID, result: res, ok: ok}
		}(tc.ID)
	}

	resultMap := make(map[string]toolResultPayload, len(calls))
	for callID, result := range cached {
		resultMap[callID] = result
	}
	for i := 0; i < waiting; i++ {
		cr := <-ch
		if !cr.ok {
			r.emit(AgentEvent{Kind: KindError, Error: "task cancelled while waiting for tool result"})
			return nil, true
		}
		resultMap[cr.callID] = cr.result
		// Settle the web sub-call's spend right away, like any other step:
		// tokens (billed at the task model's rate — a conservative
		// approximation for the small-model sub-call) plus the provider's
		// flat per-search fee.
		if cr.web {
			if r.onWebUsage != nil && cr.usage.InputTokens+cr.usage.OutputTokens+cr.usage.SearchCount > 0 {
				r.onWebUsage(cr.callID, cr.usage)
			} else if r.onWebAborted != nil {
				r.onWebAborted(cr.callID, "web provider returned no billable usage")
			}
		}
	}

	// 3. Build messages and emit KindToolEnd in the original call order.
	messages := make([]providers.Message, 0, len(calls))
	for _, tc := range calls {
		res := resultMap[tc.ID]
		wasCached := false
		if _, ok := cached[tc.ID]; ok {
			wasCached = true
		}
		durationMs := time.Since(starts[tc.ID]).Milliseconds()

		if res.IsError {
			log.Printf("[runner] tool %q (id=%s) FAILED in %dms — %s",
				tc.Name, tc.ID, durationMs, truncateLog(res.Output, 200))
		} else {
			log.Printf("[runner] tool %q (id=%s) OK in %dms — %d chars",
				tc.Name, tc.ID, durationMs, len(res.Output))
		}

		summary := res.Output
		if len(summary) > 200 {
			summary = summary[:200] + "…"
		}
		r.emit(AgentEvent{
			Kind:          KindToolEnd,
			ToolID:        tc.ID,
			ToolName:      tc.Name,
			ResultSummary: summary,
			DurationMs:    durationMs,
		})
		r.trace(TrajectoryEvent{Kind: "tool.completed", Round: round, ToolID: tc.ID, ToolName: tc.Name, DurationMs: durationMs, Reused: wasCached, ReadOnlyStreak: r.readOnlyStreak})

		content := res.Output
		if res.IsError {
			content = fmt.Sprintf("Error: %s", res.Output)
		}
		const maxToolResultChars = 48_000
		if len(content) > maxToolResultChars {
			end := maxToolResultChars
			for end > 0 && content[end]&0xc0 == 0x80 {
				end--
			}
			content = content[:end] + "\n… [tool output truncated to fit context window]"
		}
		if cacheableRead(tc.Name) {
			if !wasCached {
				r.readCache[cacheKey(tc)] = res
			}
			r.readOnlyStreak++
			if wasCached {
				content += "\n\n[Controller: This exact read/search was already completed against unchanged run state, so its prior result was reused. Do not request it again unless a later edit or command could have changed the evidence.]"
			}
			if r.readOnlyStreak > 0 && r.readOnlyStreak%10 == 0 {
				content += "\n\n[Controller exploration checkpoint: Synthesize the evidence gathered so far and act or answer now. Continue reading only if you can name a specific unresolved fact that the next call will establish.]"
				r.trace(TrajectoryEvent{Kind: "controller.exploration_checkpoint", Round: round, ToolID: tc.ID, ToolName: tc.Name, ReadOnlyStreak: r.readOnlyStreak})
			}
		}
		messages = append(messages, providers.Message{
			Role:       providers.RoleTool,
			ToolCallID: tc.ID,
			Content:    content,
		})
	}

	return messages, false
}

// buildToolDefs returns the tools to advertise to the LLM.
// web_search and fetch_url are executed server-side by the webtools executor;
// all other tools are executed by the client.
// MCP tools are prepended so the LLM encounters them before generic local tools.
func (r *Runner) buildToolDefs() []providers.ToolDefinition {
	all := clientToolDefinitions()
	var filtered []providers.ToolDefinition
	switch r.mode {
	case AgentModeCoordinator:
		// Coordinator must not call write/exec tools directly — it delegates to Workers.
		filtered = all[:0]
		for _, t := range all {
			if !coordinatorBannedTools[t.Name] {
				filtered = append(filtered, t)
			}
		}
	case AgentModeWorker:
		// Workers must not recurse by spawning further sub-agents.
		filtered = all[:0]
		for _, t := range all {
			if t.Name != "spawn_subagent" {
				filtered = append(filtered, t)
			}
		}
	default:
		filtered = all
	}
	extraTools := r.extraTools
	if r.restrictTools {
		allowed := r.allowedTools
		restrictedExtra := make([]providers.ToolDefinition, 0, len(extraTools))
		for _, tool := range extraTools {
			if _, ok := allowed[tool.Name]; ok {
				restrictedExtra = append(restrictedExtra, tool)
			}
		}
		extraTools = restrictedExtra
		restrictedLocal := make([]providers.ToolDefinition, 0, len(filtered))
		for _, tool := range filtered {
			if _, ok := allowed[tool.Name]; ok {
				restrictedLocal = append(restrictedLocal, tool)
			}
		}
		filtered = restrictedLocal
	}

	// Prepend MCP tools so the LLM sees them before generic local tools.
	// This biases selection toward MCP when both could apply to a query.
	all_defs := append(extraTools, filtered...)
	mcpNames := make([]string, 0, len(extraTools))
	for _, t := range extraTools {
		mcpNames = append(mcpNames, t.Name)
	}
	log.Printf("[runner] tool list: %d MCP + %d local = %d total. MCP tools: %v",
		len(extraTools), len(filtered), len(all_defs), mcpNames)
	return all_defs
}

// ── Tool result gate ──────────────────────────────────────────────────────────

type toolResultPayload struct {
	Output  string
	IsError bool
}

type toolResultGate struct {
	mu      sync.Mutex
	pending map[string]chan toolResultPayload
}

func newToolResultGate() *toolResultGate {
	return &toolResultGate{pending: make(map[string]chan toolResultPayload)}
}

// wait blocks until the client POSTs a result for callID, or ctx is cancelled.
func (g *toolResultGate) wait(ctx context.Context, callID string) (toolResultPayload, bool) {
	ch := make(chan toolResultPayload, 1)
	g.mu.Lock()
	g.pending[callID] = ch
	g.mu.Unlock()

	defer func() {
		g.mu.Lock()
		delete(g.pending, callID)
		g.mu.Unlock()
	}()

	select {
	case <-ctx.Done():
		return toolResultPayload{}, false
	case r := <-ch:
		return r, true
	}
}

// Resolve delivers a client-posted tool result for callID.
// Returns false if the call ID is unknown or already resolved.
func (g *toolResultGate) Resolve(callID, output string, isError bool) bool {
	g.mu.Lock()
	ch, ok := g.pending[callID]
	g.mu.Unlock()
	if !ok {
		return false
	}
	ch <- toolResultPayload{Output: output, IsError: isError}
	return true
}

// ── Tool definitions (advertised to the LLM) ─────────────────────────────────

// clientToolDefinitions returns the tool schemas that tell the LLM what it can
// ask for. Actual execution happens on the client, not here.
func clientToolDefinitions() []providers.ToolDefinition {
	strType := map[string]interface{}{"type": "string"}
	return []providers.ToolDefinition{
		{
			Name:        "read_file",
			Description: "Read a file on the user's machine. Returns line-numbered output. For large files use offset/limit — a truncation notice in the output gives you the next offset to use.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"path":   map[string]interface{}{"type": "string", "description": "Full absolute path to the file (e.g. /Users/alice/project/src/main.rs). Construct it by joining the working directory from the system prompt with the file's relative path."},
					"offset": map[string]interface{}{"type": "integer", "description": "1-based line number to start reading from (optional)"},
					"limit":  map[string]interface{}{"type": "integer", "description": "Maximum number of lines to return (optional)"},
				},
				"required": []string{"path"},
			},
		},
		{
			Name:        "write_file",
			Description: "Create or overwrite a file on the user's machine. Use only for NEW files — for existing files use edit_file or batch_edit instead. Requires user approval.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"path":    map[string]interface{}{"type": "string", "description": "Full absolute path to the file. Construct by joining the working directory with the relative path."},
					"content": strType,
				},
				"required": []string{"path", "content"},
			},
		},
		{
			Name:        "edit_file",
			Description: "Replace an exact string in a file. FAILS if old_str is not found. FAILS if old_str matches multiple locations and replace_all is not set. You MUST call read_file first and copy old_str verbatim. Requires user approval.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"path":        map[string]interface{}{"type": "string", "description": "Full absolute path to the file. Construct by joining the working directory with the relative path."},
					"old_str":     strType,
					"new_str":     strType,
					"replace_all": map[string]interface{}{"type": "boolean", "description": "Replace all occurrences of old_str instead of requiring exactly one (optional)"},
				},
				"required": []string{"path", "old_str", "new_str"},
			},
		},
		{
			Name:        "list_directory",
			Description: "List files and directories at a path on the user's machine.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"path":      map[string]interface{}{"type": "string", "description": "Full absolute path to the directory. Construct by joining the working directory with the subdirectory name."},
					"recursive": map[string]interface{}{"type": "boolean"},
				},
				"required": []string{"path"},
			},
		},
		{
			Name:        "grep",
			Description: "Search for a regex pattern in files on the user's machine. Use this to locate symbols or strings before calling read_file. Prefer mode=\"files_with_matches\" for large codebases to avoid flooding the context.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"pattern":       strType,
					"path":          map[string]interface{}{"type": "string", "description": "Full absolute path to directory or file to search. Construct by joining the working directory with the subdirectory name."},
					"context_lines": map[string]interface{}{"type": "integer", "description": "Number of lines of context to include before and after each match (like grep -C)"},
					"mode": map[string]interface{}{
						"type":        "string",
						"enum":        []string{"files_with_matches", "content", "count"},
						"description": "Output mode: \"content\" (default) returns matching lines, \"files_with_matches\" returns only file names, \"count\" returns match counts per file",
					},
				},
				"required": []string{"pattern"},
			},
		},
		{
			Name:        "search_codebase",
			Description: "Keyword search across the codebase indexed on the user's machine.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"query":     strType,
					"repo_root": strType,
				},
				"required": []string{"query"},
			},
		},
		{
			Name: "run_command",
			Description: "Run a shell command on the user's machine. Shell state (cwd) persists across calls. " +
				"For long-running commands (dev servers, test suites, build watchers) set wait_ms_before_async " +
				"to a short value like 500-2000 ms. If the command has not exited within that window it is moved " +
				"to the background and you receive a task_id immediately. " +
				"IMPORTANT: after receiving a task_id do NOT poll with manage_task. Simply stop — " +
				"the system will automatically resume you with the full output the moment the process exits. " +
				"Requires user approval.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"command": strType,
					"reason":  map[string]interface{}{"type": "string", "description": "One concise user-visible sentence explaining why this command is needed and what its result will decide"},
					"cwd":     map[string]interface{}{"type": "string", "description": "Working directory (optional)"},
					"wait_ms_before_async": map[string]interface{}{
						"type":        "integer",
						"description": "If set, automatically background the command after this many milliseconds if it has not exited yet. Returns a task_id identifying the live task; completion output is delivered automatically. Omit or 0 to block until done.",
					},
					"acceptance_criteria": map[string]interface{}{
						"type":        "array",
						"items":       map[string]interface{}{"type": "string"},
						"description": "Workspace review only: criterion UUIDs that this verification command checks. The local host records the actual exit code; do not claim unrelated criteria.",
					},
				},
				"required": []string{"command", "reason"},
			},
		},
		{
			Name: "manage_task",
			Description: "Control a background task started by run_command with wait_ms_before_async. " +
				"Status and output polling are intentionally unavailable because completion output is " +
				"delivered automatically. Use action=send_input only when the process visibly needs " +
				"interactive input, or action=kill to terminate it.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"task_id": map[string]interface{}{"type": "string", "description": "The task ID returned by run_command"},
					"action":  map[string]interface{}{"type": "string", "enum": []interface{}{"send_input", "kill"}, "description": "Action to perform"},
					"input":   map[string]interface{}{"type": "string", "description": "Line to write to stdin (only for send_input)"},
				},
				"required": []string{"task_id", "action"},
			},
		},
		{
			Name:        "batch_edit",
			Description: "Edit multiple files atomically. All edits are validated before any file is written — if one fails the entire batch is rejected and no files are changed. Prefer this over multiple edit_file calls for multi-file refactors. Requires user approval.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"edits": map[string]interface{}{
						"type":        "array",
						"description": "List of edits to apply",
						"items": map[string]interface{}{
							"type": "object",
							"properties": map[string]interface{}{
								"path":        map[string]interface{}{"type": "string", "description": "File path to edit"},
								"old_str":     map[string]interface{}{"type": "string", "description": "Exact string to replace"},
								"new_str":     map[string]interface{}{"type": "string", "description": "Replacement string"},
								"replace_all": map[string]interface{}{"type": "boolean", "description": "Replace all occurrences (optional)"},
							},
							"required": []string{"path", "old_str", "new_str"},
						},
					},
				},
				"required": []string{"edits"},
			},
		},
		{
			Name:        "apply_patch",
			Description: "Apply a unified diff (git diff format) to one or more files. Include full --- / +++ file headers and @@ hunk headers. Use for large multi-hunk changes where batch_edit would be unwieldy. Requires user approval.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"patch": map[string]interface{}{"type": "string", "description": "The full unified diff text to apply"},
				},
				"required": []string{"patch"},
			},
		},
		{
			Name:        "spawn_subagent",
			Description: "Spawn a Worker sub-agent to complete a specific subtask autonomously. The worker has full tool access (file reads, writes, shell commands). Use for tasks requiring code execution or file modifications while you coordinate. Returns the worker's complete response.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"query":      map[string]interface{}{"type": "string", "description": "The specific task for the worker to complete"},
					"model_name": map[string]interface{}{"type": "string", "description": "Optional model name override for the worker"},
				},
				"required": []string{"query"},
			},
		},
		{
			Name: "fetch_url",
			Description: "Fetch a URL's content, server-side via Exa, returning the page's main text. " +
				"USE ONLY WHEN NECESSARY — every call costs a paid request. Fetch a page when " +
				"you need its actual contents; do not fetch a URL you have already fetched in " +
				"this conversation, and never guess a URL — find it with web_search first.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"url":    map[string]interface{}{"type": "string", "description": "The full URL to fetch (must start with http:// or https://)"},
					"accept": map[string]interface{}{"type": "string", "description": "Optional Accept header, e.g. \"application/vnd.github+json\" for GitHub API"},
				},
				"required": []string{"url"},
			},
		},
		{
			Name: "web_search",
			Description: "Search the web, server-side via Exa, returning ranked results with " +
				"relevant excerpts and source URLs. " +
				"USE ONLY WHEN NECESSARY — every call costs a paid request. Do NOT use it for " +
				"things you already know, for anything answerable from the codebase (use grep / " +
				"search_codebase / read_file), or to re-check a fact you just looked up. DO use it " +
				"for information that is genuinely current (recent releases, prices, news), for " +
				"library or API details you are unsure about, or to find an exact URL before " +
				"fetching it. Prefer one specific query over several broad ones.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"query":       map[string]interface{}{"type": "string", "description": "The search query"},
					"max_results": map[string]interface{}{"type": "integer", "description": "Maximum number of results to return (default 5, max 10)"},
				},
				"required": []string{"query"},
			},
		},
		{
			Name:        "skill",
			Description: "Load an Agent Skill's instructions. The system prompt lists the available skills under <available_skills>; call this with a skill's name before doing work that skill covers, or with name=\"list\" to list them again. Skills run only by the user's own command are not loaded here. DO NOT use this for queries that connected MCP tools can answer — if an MCP server is listed in the system prompt for the relevant domain (e.g. expo, github, sentry, linear), call that MCP tool directly instead.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"name": map[string]interface{}{
						"type":        "string",
						"description": "Skill name exactly as listed in <available_skills> (e.g. \"review\"), or \"list\" to list the available skills",
					},
					"args": map[string]interface{}{
						"type":        "string",
						"description": "Optional details for this use of the skill; they are passed to it alongside its instructions as the user's arguments.",
					},
				},
				"required": []string{"name"},
			},
		},
		{
			Name:        "repl",
			Description: "Run code in a persistent interpreter session. State (variables, imports, functions, filesystem changes) survives across calls within the same conversation — define once, reuse later. Use for data exploration, prototyping algorithms, testing library APIs, and quick calculations. Prefer repl over run_command for iterative code work.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"code": map[string]interface{}{
						"type":        "string",
						"description": "Code to execute in the persistent session",
					},
					"language": map[string]interface{}{
						"type":        "string",
						"enum":        []interface{}{"python", "node", "bash"},
						"description": "Interpreter to use. Default: python. Each language has its own independent persistent session.",
					},
				},
				"required": []string{"code"},
			},
		},
		{
			Name:        "sleep",
			Description: "Pause for a specified number of milliseconds without holding a shell process. Use this instead of run_command(\"sleep N\") when waiting for an external system to become ready before polling. Maximum: 300,000ms (5 minutes).",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"ms": map[string]interface{}{
						"type":        "integer",
						"description": "Duration to sleep in milliseconds (1–300000)",
					},
				},
				"required": []string{"ms"},
			},
		},
		{
			Name:        "mcp_auth",
			Description: "Authenticate to an MCP server using OAuth 2.0 + PKCE. Opens the authorization URL in the user's browser, captures the OAuth callback on a local port, exchanges the code for tokens, and stores them securely at ~/.dragent/mcp-tokens/. Required before calling tools from OAuth-protected MCP servers.",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"server_name": map[string]interface{}{
						"type":        "string",
						"description": "Name of the MCP server (must match a configured MCP server name)",
					},
					"auth_url": map[string]interface{}{
						"type":        "string",
						"description": "OAuth authorization endpoint URL (e.g. https://auth.example.com/oauth/authorize)",
					},
					"token_url": map[string]interface{}{
						"type":        "string",
						"description": "OAuth token endpoint URL (e.g. https://auth.example.com/oauth/token)",
					},
					"client_id": map[string]interface{}{
						"type":        "string",
						"description": "OAuth client ID (default: dragent)",
					},
				},
				"required": []string{"server_name", "auth_url", "token_url"},
			},
		},
	}
}

// ── Helpers ───────────────────────────────────────────────────────────────────

var iconMap = map[string]string{
	"read_file":       "📄",
	"write_file":      "✏️",
	"edit_file":       "✏️",
	"list_directory":  "📁",
	"grep":            "🔍",
	"search_codebase": "🔍",
	"run_command":     "⚡",
	"manage_task":     "⚙️",
	"web_search":      "🌐",
	"fetch_url":       "🌐",
	"batch_edit":      "✏️",
	"apply_patch":     "🩹",
	"spawn_subagent":  "🤖",
	"skill":           "📚",
	"repl":            "🔁",
	"sleep":           "⏱️",
	"mcp_auth":        "🔑",
}

func truncateLog(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}

func toolIcon(name string) string {
	if icon, ok := iconMap[name]; ok {
		return icon
	}
	return "🔧"
}

func argsSummary(name, argsJSON string) string {
	switch name {
	case "read_file", "edit_file":
		return extractString(argsJSON, "path")
	case "run_command":
		return extractString(argsJSON, "command")
	case "manage_task":
		taskID := extractString(argsJSON, "task_id")
		action := extractString(argsJSON, "action")
		return action + " " + taskID
	case "grep":
		pattern := extractString(argsJSON, "pattern")
		path := extractString(argsJSON, "path")
		if path != "" {
			return pattern + " in " + path
		}
		return pattern
	case "search_codebase":
		return extractString(argsJSON, "query")
	case "web_search":
		return extractString(argsJSON, "query")
	case "fetch_url":
		return extractString(argsJSON, "url")
	case "batch_edit":
		type Edit struct {
			Path string `json:"path"`
		}
		type Args struct {
			Edits []Edit `json:"edits"`
		}
		var args Args
		if err := json.Unmarshal([]byte(argsJSON), &args); err == nil && len(args.Edits) > 0 {
			uniquePaths := make(map[string]bool)
			var paths []string
			for _, e := range args.Edits {
				if e.Path != "" && !uniquePaths[e.Path] {
					uniquePaths[e.Path] = true
					paths = append(paths, e.Path)
				}
			}
			if len(paths) == 1 {
				return "edit " + paths[0]
			}
			return "batch edit: " + strings.Join(paths, ", ")
		}
		return "batch edit"
	case "apply_patch":
		patch := extractString(argsJSON, "patch")
		if len(patch) > 80 {
			return patch[:80] + "…"
		}
		return patch
	case "spawn_subagent":
		query := extractString(argsJSON, "query")
		if len(query) > 80 {
			return query[:80] + "…"
		}
		return query
	default:
		if len(argsJSON) > 80 {
			return argsJSON[:80] + "…"
		}
		return argsJSON
	}
}

func extractString(argsJSON, key string) string {
	var m map[string]interface{}
	if err := json.Unmarshal([]byte(argsJSON), &m); err != nil {
		return ""
	}
	if v, ok := m[key]; ok {
		return fmt.Sprintf("%v", v)
	}
	return ""
}
