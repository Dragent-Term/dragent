package relay

import "context"

// HostCommandExecutor performs mutations on the authorized desktop and waits
// for its authoritative result. The method name is the dispatch: each method
// carries its own typed params and result, so a fake executor in tests
// exercises the same shape the host must return.
//
//   - CreateTerminal asks the desktop to open a terminal for owner (the
//     caller's device-scoped owner, already authorized by resolveOwner) and
//     waits for the created tab.
//   - CreateWorktree asks the desktop to git worktree-add entry for owner and
//     waits for the created workspace entry.
//
// A server-side snapshot can never execute a terminal or git: implementations
// live on the host adapter (the relay socket's desktop side), never on the
// cloud server — no implementation may spawn local processes on the relay
// itself. Raw host errors must stay out of user-visible messages: handlers
// classify executor failures into fixed strings (never err.Error()), so
// tokens, paths, or internal errors never cross the relay.
//
// A nil executor explicitly disables mutations: tabs.createTerminal and
// worktree.create answer 503 and leave the read-only mirror unchanged. Only a
// successful, shape-validated authoritative host result may answer success.
// The read-only mirrors (tabs.list / worktree.list) change only on the next
// host publish, never as a side effect of a command.
type HostCommandExecutor interface {
	CreateTerminal(context.Context, string) (TabEntry, error)
	CreateWorktree(context.Context, string, WorkspaceEntry) (WorkspaceEntry, error)
}

// SetHostCommands installs the host adapter. Call once, before serving; a nil
// executor disables mutations (503, mirror unchanged) while the read-only
// host snapshots stay available. The same handler table serves HTTP and the
// relay socket, so one setter covers both transports.
func (h *RPCHandler) SetHostCommands(executor HostCommandExecutor) { h.commands = executor }
