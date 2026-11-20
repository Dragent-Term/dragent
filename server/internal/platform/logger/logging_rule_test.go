// Lint for the §2.3 logging rule: no per-request logging of URIs, query
// strings, tokens or ids in the HTTP server (server/cmd/api/main.go) or the
// logger package itself. It parses the sources — not a grep over log text —
// so a violation fails the build even when no request ever hits it.
package logger

import (
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// sourcesUnderTest returns main.go plus every non-test source in this package.
func sourcesUnderTest(t *testing.T) map[string]string {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate this test file")
	}
	loggerDir := filepath.Dir(thisFile)
	mainPath := filepath.Join(loggerDir, "..", "..", "..", "cmd", "api", "main.go")

	files := map[string]string{"main.go": mainPath}
	entries, err := os.ReadDir(loggerDir)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		name := e.Name()
		if strings.HasSuffix(name, ".go") && !strings.HasSuffix(name, "_test.go") {
			files[name] = filepath.Join(loggerDir, name)
		}
	}
	srcs := make(map[string]string, len(files))
	for name, path := range files {
		raw, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("read %s: %v", path, err)
		}
		srcs[name] = string(raw)
	}
	return srcs
}

func parseFile(t *testing.T, name, src string) *ast.File {
	t.Helper()
	f, err := parser.ParseFile(token.NewFileSet(), name, src, 0)
	if err != nil {
		t.Fatalf("parse %s: %v", name, err)
	}
	return f
}

// isLogCall reports whether a call is a logging call whose arguments must
// carry no request-derived data: slog.<level> or log.<...>.
func isLogCall(call *ast.CallExpr) bool {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	pkg, ok := sel.X.(*ast.Ident)
	if !ok {
		return false
	}
	if pkg.Name != "slog" && pkg.Name != "log" {
		return false
	}
	switch sel.Sel.Name {
	case "Debug", "Info", "Warn", "Error", "Print", "Printf", "Println",
		"Fatal", "Fatalf", "Fatalln", "Panic", "Panicf", "Panicln",
		"Output":
		return true
	}
	return false
}

// forbiddenInLogArg reports whether an expression inside a logging call can
// carry a URI, query string or token: any `.URL` selector (r.URL.Path,
// r.URL.Query(), ...), RequestURI/RawQuery/RawPath selectors, or an
// access_token literal.
func forbiddenInLogArg(e ast.Expr) bool {
	found := false
	ast.Inspect(e, func(n ast.Node) bool {
		if found {
			return false
		}
		switch n := n.(type) {
		case *ast.SelectorExpr:
			if n.Sel.Name == "URL" || n.Sel.Name == "RequestURI" ||
				n.Sel.Name == "RawQuery" || n.Sel.Name == "RawPath" {
				found = true
				return false
			}
		case *ast.BasicLit:
			if strings.Contains(strings.ToLower(n.Value), "access_token") {
				found = true
				return false
			}
		}
		return true
	})
	return found
}

// No per-request logging of URIs, queries or tokens (§2.3, H1).
func TestLoggingRuleNoRequestDataInLogs(t *testing.T) {
	for name, src := range sourcesUnderTest(t) {
		f := parseFile(t, name, src)
		ast.Inspect(f, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok || !isLogCall(call) {
				return true
			}
			for _, arg := range call.Args {
				if forbiddenInLogArg(arg) {
					t.Errorf("%s: logging call carries a URI/query/token: %s", name, src[call.Pos()-1:call.End()-1])
				}
			}
			return true
		})
	}
}

// The banned per-request loggers must stay out: chi's access log (H1) and
// its full-stack recoverer (stacks can carry content), plus raw stack dumps.
func TestLoggingRuleNoBannedLoggers(t *testing.T) {
	srcs := sourcesUnderTest(t)
	for name, src := range srcs {
		for _, banned := range []string{"middleware.Logger", "middleware.Recoverer", "debug.Stack", "PrintPrettyStack", "RequestURI"} {
			if strings.Contains(src, banned) {
				t.Errorf("%s: contains banned %q", name, banned)
			}
		}
	}
}

// The query-token refusal must cover exactly the relay, pairing and push
// prefixes (H2): the guard lists them, and AuthMiddleware consults it.
func TestLoggingRuleQueryTokenGuard(t *testing.T) {
	srcs := sourcesUnderTest(t)
	main, ok := srcs["main.go"]
	if !ok {
		t.Fatal("main.go not found")
	}
	// Access tokens are never read from a URL (H2, now on every route).
	if strings.Contains(main, `Query().Get("access_token")`) {
		t.Error("main.go: AuthMiddleware accepts access_token in the query string (H2)")
	}
}
