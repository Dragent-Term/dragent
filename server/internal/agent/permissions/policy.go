package permissions

import "strings"

// Policy determines which tool calls require explicit user approval.
// Destructive operations (file writes, shell commands) are gated by default.
type Policy struct {
	// alwaysAllow is the set of tool names that never need approval.
	alwaysAllow map[string]bool
	// requireApproval overrides specific tools to always need approval.
	requireApproval map[string]bool
}

// DefaultPolicy gates write_file, edit_file, and run_command behind approval.
// Read-only tools (read_file, list_directory, grep, search_codebase, web_search)
// are always allowed without interrupting the loop.
func DefaultPolicy() *Policy {
	return &Policy{
		alwaysAllow: map[string]bool{
			"read_file":       true,
			"list_directory":  true,
			"grep":            true,
			"search_codebase": true,
			"web_search":      true,
			"fetch_url":       true,
			"spawn_subagent":  true,
			"manage_task":     true, // send_input/kill for an existing background task
		},
		requireApproval: map[string]bool{
			"write_file":  true,
			"edit_file":   true,
			"run_command": true,
			"batch_edit":  true,
			"apply_patch": true,
		},
	}
}

// PermissivePolicy allows all tools without approval — useful for trusted
// automated pipelines or SDK consumers that manage their own approval layer.
func PermissivePolicy() *Policy {
	return &Policy{
		alwaysAllow:     map[string]bool{},
		requireApproval: map[string]bool{},
	}
}

// RequiresApproval returns true if the tool must be approved before execution.
func (p *Policy) RequiresApproval(toolName string) bool {
	if p.alwaysAllow[toolName] {
		return false
	}
	if p.requireApproval[toolName] {
		return true
	}
	// Always allow prompt and resource tools as they are read-only/safe
	if strings.HasPrefix(toolName, "prompt__") || strings.HasPrefix(toolName, "resource__") {
		return false
	}
	// For MCP tools, always allow typical read-only/query operations
	if strings.HasPrefix(toolName, "mcp__") {
		lower := strings.ToLower(toolName)
		if strings.Contains(lower, "read") ||
			strings.Contains(lower, "search") ||
			strings.Contains(lower, "list") ||
			strings.Contains(lower, "get") ||
			strings.Contains(lower, "status") ||
			strings.Contains(lower, "info") ||
			strings.Contains(lower, "logs") ||
			strings.Contains(lower, "whoami") ||
			strings.Contains(lower, "doctor") {
			return false
		}
	}
	// Unknown tools are gated by default for safety
	return true
}

// Allow registers a tool as always-allowed (read-only / safe).
func (p *Policy) Allow(toolName string) {
	p.alwaysAllow[toolName] = true
	delete(p.requireApproval, toolName)
}

// Gate registers a tool as requiring approval.
func (p *Policy) Gate(toolName string) {
	p.requireApproval[toolName] = true
	delete(p.alwaysAllow, toolName)
}
