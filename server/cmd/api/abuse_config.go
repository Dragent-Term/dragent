package main

import (
	"log/slog"

	"github.com/99designs/gqlgen/graphql"
	"github.com/99designs/gqlgen/graphql/handler"
	"github.com/99designs/gqlgen/graphql/handler/extension"
	"github.com/99designs/gqlgen/graphql/handler/lru"
	"github.com/99designs/gqlgen/graphql/handler/transport"
	"github.com/vektah/gqlparser/v2/ast"
	"os"
	"strconv"
	"strings"
)

// graphqlComplexityLimit bounds one GraphQL operation (gqlgen counts one
// per field by default). The schema's real operations score well under 20.
const graphqlComplexityLimit = 100

// graphqlDevMode reports whether APP_ENV allows the playground and
// introspection. Unset means production: the safe default.
func graphqlDevMode(appEnv string) bool {
	switch strings.ToLower(strings.TrimSpace(appEnv)) {
	case "dev", "development", "local":
		return true
	}
	return false
}

// envInt reads a positive integer from the environment, falling back to
// def (with a warning) when it is unset or invalid.
func envInt(name string, def int) int {
	raw := strings.TrimSpace(os.Getenv(name))
	if raw == "" {
		return def
	}
	n, err := strconv.Atoi(raw)
	if err != nil || n <= 0 {
		slog.Warn("ignoring invalid integer env var", "name", name, "default", def)
		return def
	}
	return n
}

// newGraphQLServer builds the /query handler by hand instead of
// handler.NewDefaultServer, which turns introspection on everywhere and sets
// no complexity limit. The schema has no subscriptions or uploads, so it
// has no websocket or multipart transports either.
func newGraphQLServer(es graphql.ExecutableSchema, dev bool) *handler.Server {
	srv := handler.New(es)
	srv.AddTransport(transport.Options{})
	srv.AddTransport(transport.GET{})
	srv.AddTransport(transport.POST{})
	srv.SetQueryCache(lru.New[*ast.QueryDocument](1000))
	if dev {
		srv.Use(extension.Introspection{})
	}
	srv.Use(extension.AutomaticPersistedQuery{Cache: lru.New[string](100)})
	srv.Use(extension.FixedComplexityLimit(graphqlComplexityLimit))
	return srv
}
