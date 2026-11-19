package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"server/internal/agent/providers"
)

// Handler is a function that executes a tool call.
// argsJSON is the raw JSON from the LLM; it returns a plain-text result.
type Handler func(ctx context.Context, argsJSON string) (string, error)

// Registry holds all registered tools and dispatches calls by name.
type Registry struct {
	mu    sync.RWMutex
	tools map[string]entry
}

type entry struct {
	def     providers.ToolDefinition
	handler Handler
}

func NewRegistry() *Registry {
	r := &Registry{tools: make(map[string]entry)}
	registerBuiltins(r)
	return r
}

// Register adds a tool to the registry. Overwrites any existing tool with the same name.
func (r *Registry) Register(def providers.ToolDefinition, handler Handler) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.tools[def.Name] = entry{def: def, handler: handler}
}

// Execute calls the named tool with the given argsJSON.
func (r *Registry) Execute(ctx context.Context, name, argsJSON string) (string, error) {
	r.mu.RLock()
	e, ok := r.tools[name]
	r.mu.RUnlock()
	if !ok {
		return "", fmt.Errorf("unknown tool: %s", name)
	}
	return e.handler(ctx, argsJSON)
}

// Definitions returns the tool definitions for all registered tools.
func (r *Registry) Definitions() []providers.ToolDefinition {
	r.mu.RLock()
	defer r.mu.RUnlock()
	defs := make([]providers.ToolDefinition, 0, len(r.tools))
	for _, e := range r.tools {
		defs = append(defs, e.def)
	}
	return defs
}

// ── Arg helpers ───────────────────────────────────────────────────────────────

func parseArgs(argsJSON string, dst interface{}) error {
	if err := json.Unmarshal([]byte(argsJSON), dst); err != nil {
		return fmt.Errorf("invalid tool args: %w", err)
	}
	return nil
}

// registerBuiltins wires up all built-in tool handlers.
func registerBuiltins(r *Registry) {
	registerFileOps(r)
	registerCodebaseTools(r)
	registerShellTools(r)
}
