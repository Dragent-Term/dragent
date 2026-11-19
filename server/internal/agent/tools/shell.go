package tools

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"server/internal/agent/providers"
)

func registerShellTools(r *Registry) {
	r.Register(providers.ToolDefinition{
		Name:        "run_command",
		Description: "Execute a shell command and return its stdout and stderr. Use only when necessary — prefer read_file/edit_file for file changes.",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"command": map[string]interface{}{
					"type":        "string",
					"description": "The shell command to run",
				},
				"cwd": map[string]interface{}{
					"type":        "string",
					"description": "Working directory for the command (optional)",
				},
				"timeout_seconds": map[string]interface{}{
					"type":        "integer",
					"description": "Maximum execution time in seconds (default 30, max 120)",
				},
			},
			"required": []string{"command"},
		},
	}, handleRunCommand)
}

func handleRunCommand(ctx context.Context, argsJSON string) (string, error) {
	var args struct {
		Command        string `json:"command"`
		Cwd            string `json:"cwd"`
		TimeoutSeconds int    `json:"timeout_seconds"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Command == "" {
		return "", fmt.Errorf("command is required")
	}

	timeout := args.TimeoutSeconds
	if timeout <= 0 {
		timeout = 30
	}
	if timeout > 120 {
		timeout = 120
	}

	execCtx, cancel := context.WithTimeout(ctx, time.Duration(timeout)*time.Second)
	defer cancel()

	cmd := exec.CommandContext(execCtx, "sh", "-c", args.Command)
	if args.Cwd != "" {
		cmd.Dir = args.Cwd
	}

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err := cmd.Run()

	var sb strings.Builder
	if stdout.Len() > 0 {
		sb.WriteString(stdout.String())
	}
	if stderr.Len() > 0 {
		if sb.Len() > 0 {
			sb.WriteString("\nSTDERR:\n")
		}
		sb.WriteString(stderr.String())
	}

	output := sb.String()
	const maxOutput = 20 * 1024
	if len(output) > maxOutput {
		output = output[:maxOutput] + "\n… [output truncated]"
	}

	if err != nil {
		if execCtx.Err() == context.DeadlineExceeded {
			return output, fmt.Errorf("command timed out after %ds", timeout)
		}
		// Include exit code in output but don't return as error — let LLM see the output
		return fmt.Sprintf("Exit code: %v\n%s", cmd.ProcessState.ExitCode(), output), nil
	}

	if output == "" {
		return "(no output)", nil
	}
	return output, nil
}
