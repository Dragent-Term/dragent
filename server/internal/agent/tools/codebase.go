package tools

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"server/internal/agent/providers"
)

func registerCodebaseTools(r *Registry) {
	r.Register(providers.ToolDefinition{
		Name:        "grep",
		Description: "Search for a pattern in files using regular expressions. Returns matching lines with file paths and line numbers.",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"pattern": map[string]interface{}{
					"type":        "string",
					"description": "Regular expression pattern to search for",
				},
				"path": map[string]interface{}{
					"type":        "string",
					"description": "Absolute path to search in (file or directory). Defaults to current working directory.",
				},
				"include": map[string]interface{}{
					"type":        "string",
					"description": "Glob pattern to filter files (e.g. '*.go', '*.rs')",
				},
			},
			"required": []string{"pattern"},
		},
	}, handleGrep)

	r.Register(providers.ToolDefinition{
		Name:        "search_codebase",
		Description: "Search the codebase for files or code matching a natural language query. Returns relevant file paths and code snippets.",
		Parameters: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"query": map[string]interface{}{
					"type":        "string",
					"description": "Natural language search query (e.g. 'authentication middleware', 'database connection')",
				},
				"repo_root": map[string]interface{}{
					"type":        "string",
					"description": "Absolute path to the repository root",
				},
			},
			"required": []string{"query"},
		},
	}, handleSearchCodebase)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

func handleGrep(_ context.Context, argsJSON string) (string, error) {
	var args struct {
		Pattern string `json:"pattern"`
		Path    string `json:"path"`
		Include string `json:"include"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Pattern == "" {
		return "", fmt.Errorf("pattern is required")
	}

	re, err := regexp.Compile(args.Pattern)
	if err != nil {
		return "", fmt.Errorf("invalid pattern: %w", err)
	}

	searchPath := args.Path
	if searchPath == "" {
		searchPath, _ = os.Getwd()
	}

	const maxResults = 50
	var results []string

	err = filepath.WalkDir(searchPath, func(path string, d os.DirEntry, err error) error {
		if err != nil || len(results) >= maxResults {
			return nil
		}
		name := d.Name()
		// Skip hidden directories and common noise
		if d.IsDir() && (strings.HasPrefix(name, ".") || name == "node_modules" || name == "target" || name == "vendor") {
			return filepath.SkipDir
		}
		if d.IsDir() {
			return nil
		}
		// Apply include glob if set
		if args.Include != "" {
			matched, _ := filepath.Match(args.Include, name)
			if !matched {
				return nil
			}
		}
		// Skip binary-looking files by extension
		ext := strings.ToLower(filepath.Ext(name))
		if isBinaryExt(ext) {
			return nil
		}

		content, readErr := os.ReadFile(path)
		if readErr != nil {
			return nil
		}
		lines := strings.Split(string(content), "\n")
		for i, line := range lines {
			if re.MatchString(line) {
				results = append(results, fmt.Sprintf("%s:%d: %s", path, i+1, strings.TrimSpace(line)))
				if len(results) >= maxResults {
					return nil
				}
			}
		}
		return nil
	})
	_ = err

	if len(results) == 0 {
		return "No matches found.", nil
	}
	out := strings.Join(results, "\n")
	if len(results) >= maxResults {
		out += fmt.Sprintf("\n… (showing first %d matches)", maxResults)
	}
	return out, nil
}

func handleSearchCodebase(_ context.Context, argsJSON string) (string, error) {
	var args struct {
		Query    string `json:"query"`
		RepoRoot string `json:"repo_root"`
	}
	if err := parseArgs(argsJSON, &args); err != nil {
		return "", err
	}
	if args.Query == "" {
		return "", fmt.Errorf("query is required")
	}

	searchRoot := args.RepoRoot
	if searchRoot == "" {
		searchRoot, _ = os.Getwd()
	}

	// BM25-style keyword search: extract significant tokens from the query,
	// search file names and content, score by term frequency.
	tokens := tokenize(args.Query)
	if len(tokens) == 0 {
		return "", fmt.Errorf("no searchable terms in query")
	}

	type scored struct {
		path  string
		score int
		lines []string
	}

	var hits []scored
	const maxFiles = 10
	const maxLines = 5

	_ = filepath.WalkDir(searchRoot, func(path string, d os.DirEntry, err error) error {
		if err != nil || len(hits) >= maxFiles {
			return nil
		}
		name := d.Name()
		if d.IsDir() {
			if strings.HasPrefix(name, ".") || name == "node_modules" || name == "target" || name == "vendor" {
				return filepath.SkipDir
			}
			return nil
		}
		ext := strings.ToLower(filepath.Ext(name))
		if isBinaryExt(ext) {
			return nil
		}

		content, err := os.ReadFile(path)
		if err != nil {
			return nil
		}

		lower := strings.ToLower(string(content))
		score := 0
		// Bonus for filename match
		lowerName := strings.ToLower(name)
		for _, tok := range tokens {
			if strings.Contains(lowerName, tok) {
				score += 3
			}
			score += strings.Count(lower, tok)
		}
		if score == 0 {
			return nil
		}

		// Collect matching lines
		lines := strings.Split(string(content), "\n")
		var matchLines []string
		for i, line := range lines {
			ll := strings.ToLower(line)
			for _, tok := range tokens {
				if strings.Contains(ll, tok) {
					matchLines = append(matchLines, fmt.Sprintf("  %d: %s", i+1, strings.TrimSpace(line)))
					if len(matchLines) >= maxLines {
						goto doneLines
					}
					break
				}
			}
		}
	doneLines:
		hits = append(hits, scored{path: path, score: score, lines: matchLines})
		return nil
	})

	// Sort by score descending (simple bubble sort for small N)
	for i := 0; i < len(hits); i++ {
		for j := i + 1; j < len(hits); j++ {
			if hits[j].score > hits[i].score {
				hits[i], hits[j] = hits[j], hits[i]
			}
		}
	}

	if len(hits) == 0 {
		return "No relevant files found.", nil
	}

	var sb strings.Builder
	for _, h := range hits {
		sb.WriteString(fmt.Sprintf("## %s (score: %d)\n", h.path, h.score))
		for _, l := range h.lines {
			sb.WriteString(l + "\n")
		}
		sb.WriteString("\n")
	}
	return sb.String(), nil
}

// ── Helpers ───────────────────────────────────────────────────────────────────

var binaryExts = map[string]bool{
	".png": true, ".jpg": true, ".jpeg": true, ".gif": true, ".ico": true,
	".svg": true, ".woff": true, ".woff2": true, ".ttf": true, ".eot": true,
	".pdf": true, ".zip": true, ".tar": true, ".gz": true, ".bin": true,
	".exe": true, ".dll": true, ".so": true, ".dylib": true,
	".pyc": true, ".class": true, ".o": true, ".a": true,
}

func isBinaryExt(ext string) bool { return binaryExts[ext] }

var stopWords = map[string]bool{
	"the": true, "a": true, "an": true, "and": true, "or": true,
	"is": true, "in": true, "at": true, "to": true, "for": true,
	"of": true, "on": true, "with": true, "that": true, "this": true,
	"how": true, "what": true, "where": true, "when": true, "find": true,
	"search": true, "show": true, "get": true, "list": true,
}

func tokenize(query string) []string {
	words := strings.FieldsFunc(strings.ToLower(query), func(r rune) bool {
		return !('a' <= r && r <= 'z') && !('0' <= r && r <= '9') && r != '_'
	})
	var tokens []string
	for _, w := range words {
		if len(w) >= 3 && !stopWords[w] {
			tokens = append(tokens, w)
		}
	}
	return tokens
}
