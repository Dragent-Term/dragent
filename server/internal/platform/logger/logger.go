package logger

import (
	"context"
	"log/slog"
	"os"
)

type contextKey string

const TraceIDKey contextKey = "trace_id"
const UserIDKey contextKey = "user_id"

// ContextHandler wraps slog.Handler to inject trace_id and user_id from context.
type ContextHandler struct {
	slog.Handler
}

func (h *ContextHandler) Handle(ctx context.Context, r slog.Record) error {
	if traceID, ok := ctx.Value(TraceIDKey).(string); ok {
		r.AddAttrs(slog.String("trace_id", traceID))
	}
	if userID, ok := ctx.Value(UserIDKey).(string); ok {
		r.AddAttrs(slog.String("user_id", userID))
	}
	return h.Handler.Handle(ctx, r)
}

func InitLogger() {
	handler := &ContextHandler{
		Handler: slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
			Level: slog.LevelInfo,
		}),
	}
	slog.SetDefault(slog.New(handler))
}
