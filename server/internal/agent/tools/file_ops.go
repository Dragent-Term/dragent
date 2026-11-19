package tools

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"server/internal/agent/providers"
)

func registerFileOps(r *Registry) {
	r.Register(providers.ToolDefinition{
		Name:        "read_file",
		Description: "Read the full contents of a file at the given absolute path.",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"path": map[string]interface{}{
					"type":        "string",
					"description": "Absolute path to the file",
				},
			},
			"required": []string{"path"},
		},
	}, handleReadFile)

	r.Register(providers.ToolDefinition{
		Name: "edit_file",
		Description: `Edit a file by replacing an exact string with a new string.
The old_str must exactly match text in the file (including whitespace).
Always read_file first to get the exact content, then use a unique multi-line
anchor as old_str so the replacement is unambiguous. Returns a unified diff.`,
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"path": map[string]interface{}{
					"type":        "string",
					"description": "Absolute path to the file to edit",
				},
				"old_str": map[string]interface{}{
					"type":        "string",
					"description": "Exact string to find and replace (must be unique in the file)",
				},
				"new_str": map[string]interface{}{
					"type":        "string",
					"description": "Replacement string",
				},
			},
			"required": []string{"path", "old_str", "new_str"},
		},
	}, handleEditFile)

	r.Register(providers.ToolDefinition{
		Name:        "write_file",
		Description: "Write content to a file, creating parent directories as needed. Overwrites existing content.",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"path": map[string]interface{}{
					"type":        "string",
					"description": "Absolute path to the file to write",
				},
				"content": map[string]interface{}{
					"type":        "string",
					"description": "Content to write to the file",
				},
			},
			"required": []string{"path", "content"},
		},
	}, handleWriteFile)

	r.Register(providers.ToolDefinition{
		Name:        "list_directory",
		Description: "List files and directories at the given path.",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"path": map[string]interface{}{
					"type":        "string",
					"description": "Absolute path to the directory",
				},
				"recursive": map[string]interface{}{
					"type":        "boolean",
					"description": "Whether to list recursively (default false)",
				},
			},
			"required": []string{"path"},
		},
	}, handleListDirectory)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

func handleReadFile(_ context.Context, argsJSON string) (string, error) {
	var args struct {
		Path string `json:"path"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Path == "" {
		return "", fmt.Errorf("path is required")
	}

	content, err := os.ReadFile(args.Path)
	if err != nil {
		return "", fmt.Errorf("read_file: %w", err)
	}

	const maxBytes = 100 * 1024 // 100 KB
	if len(content) > maxBytes {
		return string(content[:maxBytes]) + "\n… [truncated]", nil
	}
	return string(content), nil
}

func handleEditFile(_ context.Context, argsJSON string) (string, error) {
	var args struct {
		Path   string `json:"path"`
		OldStr string `json:"old_str"`
		NewStr string `json:"new_str"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Path == "" || args.OldStr == "" {
		return "", fmt.Errorf("path and old_str are required")
	}

	content, err := os.ReadFile(args.Path)
	if err != nil {
		return "", fmt.Errorf("edit_file: read: %w", err)
	}

	original := string(content)
	count := strings.Count(original, args.OldStr)
	if count == 0 {
		return "", fmt.Errorf("edit_file: old_str not found in %s", args.Path)
	}
	if count > 1 {
		return "", fmt.Errorf("edit_file: old_str matches %d locations in %s — use a more unique anchor", count, args.Path)
	}

	updated := strings.Replace(original, args.OldStr, args.NewStr, 1)
	if err := os.WriteFile(args.Path, []byte(updated), 0644); err != nil {
		return "", fmt.Errorf("edit_file: write: %w", err)
	}

	// Return a simple diff summary
	oldLines := strings.Count(args.OldStr, "\n") + 1
	newLines := strings.Count(args.NewStr, "\n") + 1
	return fmt.Sprintf("Edited %s: replaced %d line(s) with %d line(s)", args.Path, oldLines, newLines), nil
}

func handleWriteFile(_ context.Context, argsJSON string) (string, error) {
	var args struct {
		Path    string `json:"path"`
		Content string `json:"content"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Path == "" {
		return "", fmt.Errorf("path is required")
	}

	if err := os.MkdirAll(filepath.Dir(args.Path), 0755); err != nil {
		return "", fmt.Errorf("write_file: create dirs: %w", err)
	}
	if err := os.WriteFile(args.Path, []byte(args.Content), 0644); err != nil {
		return "", fmt.Errorf("write_file: %w", err)
	}
	return fmt.Sprintf("Wrote %d bytes to %s", len(args.Content), args.Path), nil
}

func handleListDirectory(_ context.Context, argsJSON string) (string, error) {
	var args struct {
		Path      string `json:"path"`
		Recursive bool   `json:"recursive"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Path == "" {
		return "", fmt.Errorf("path is required")
	}

	var lines []string
	if args.Recursive {
		err := filepath.WalkDir(args.Path, func(path string, d os.DirEntry, err error) error {
			if err != nil {
				return nil
			}
			rel, _ := filepath.Rel(args.Path, path)
			if rel == "." {
				return nil
			}
			// Skip hidden dirs
			if strings.HasPrefix(d.Name(), ".") && d.IsDir() {
				return filepath.SkipDir
			}
			if d.IsDir() {
				lines = append(lines, rel+"/")
			} else {
				lines = append(lines, rel)
			}
			if len(lines) >= 500 {
				return fmt.Errorf("truncated")
			}
			return nil
		})
		_ = err
	} else {
		entries, err := os.ReadDir(args.Path)
		if err != nil {
			return "", fmt.Errorf("list_directory: %w", err)
		}
		for _, e := range entries {
			if e.IsDir() {
				lines = append(lines, e.Name()+"/")
			} else {
				lines = append(lines, e.Name())
			}
		}
	}

	return strings.Join(lines, "\n"), nil
}
