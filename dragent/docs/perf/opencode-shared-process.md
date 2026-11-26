# Several agent conversations in one shared agent-server process

Research question: today every team worker costs roughly one full agent
runtime process (observed below); can N workers share one `opencode serve`
process via its sessions API instead?

Short answer: **yes, technically** — one server holds many sessions, each
with its own working directory, driven over HTTP — but it changes our
isolation, crash, permission and activity-tracking assumptions, so it needs
a careful pilot before any rollout. No code was changed for this note.

## How it would work

Today the host spawns one process per worker, roughly:

```
dragent --dragent-agent-run <id> -- opencode --auto --prompt <task>
```

The alternative is one long-lived server per machine (or per team):

```
opencode serve --port <port> --hostname 127.0.0.1
```

and one REST session per worker:

- `POST /api/session {"directory": "<worktree>"}` → `{id, ...}` — a new,
  empty conversation bound to that worktree. Verified working on 1.19.3:
  three sessions created in one server, each accepted its own directory.
- `POST /api/session/{id}/prompt {"text": ...}` — send the worker's task;
  the agent loop runs unless `resume` is false.
- `POST /api/session/{id}/shell`, `/command`, `/compact`, `/fork`,
  `/move` cover the rest of the worker lifecycle (run a command in the
  session's cwd, slash commands, compaction, moving a session between
  directories).
- `GET /api/event` — one SSE-style stream for all completions, permission
  requests and lifecycle events (replaces per-process stdout scraping).
- `GET /api/session/active` — which sessions currently own a foreground
  drain in this process.

Isolation primitive is the **location**: config, permissions
(`/api/permission/request` is per location), MCP servers and plugins
resolve per location (roughly: per project directory), not per process.
Each worktree would stay its own location with its own agent config, so
per-worktree config (today injected per process via env such as
`OPENCODE_CONFIG_CONTENT`-style overrides) must move into per-location
config files or per-request fields. That mapping is the main integration
work and is **not yet verified** — it needs a pilot that actually runs a
prompt, not just creates idle sessions.

Auth: `serve` prints `Warning: OPENCODE_SERVER_PASSWORD is not set; server
is unsecured` — a shared server must set a password/token and keep it on
loopback, otherwise any local process can drive every worker's sessions.

## Memory saved (measured 2026-11-23, this Mac, opencode 1.19.3)

| setup | RSS | phys_footprint | note |
|---|---|---|---|
| 1 server, 0 sessions (fresh) | ~157 MB | — | second independent server on this machine: same ~157 MB |
| 1 server, 3 idle sessions | ~152–157 MB | 269 MB (peak 424 MB during startup) | sessions created via REST, no prompts sent |
| 1 worker today (`--auto --prompt`, mid-task) | ~395 MB | — | this team's own worker process; grows with context |

Idle sessions are essentially free inside a warm server: 1 vs 3 sessions
did not move RSS outside noise. The saving is roughly **one runtime per
extra worker**: N workers need ~157 MB + N × per-session state instead of
N × ~400 MB–1 GB.

Not measured (needs provider credentials, deliberately not touched):
per-session cost while the model loop is actually running (tool processes,
LSPs, MCP servers, context). A shared server still pays that per active
session — the win is the deduplicated runtime, plugin host and file
watchers, not the model traffic. Re-measure with a real (cheap) model
before quoting numbers further.

## Risks

- **One crash takes all workers down.** Today a worker's agent process can
  die and the host restarts just that worker; with a shared server, a
  server panic/OOM kills every conversation at once. Mitigation: keep the
  per-process spawner as fallback, supervise the server, and cap agents
  per server (e.g. one server per team, not per machine).
- **cwd/worktree isolation is config, not a process boundary.** Sessions
  accept a `directory`, but file tools, shell commands and MCP servers all
  run inside the one server process. A misdirected `directory` (or a
  `session.move`) can touch another worktree with no OS-level containment.
  Per-location config must pin each session to its worktree, and the host
  must verify `session.directory` after every create.
- **Permissions prompts multiplex.** `auto` approval today is a per-process
  CLI flag; in a server, approvals are per location via
  `/api/permission/request` + the event stream. The host's approval policy
  (what a worker may do unattended) must be re-implemented against that
  API — getting it wrong silently widens every worker's permissions at
  once.
- **Hooks/activity tracking assume one process per worker.** Our worker
  liveness runs through per-process hooks (`opencode_activity.mjs`
  injected per spawn) and per-cwd transcript reads (`opencode.db` keyed by
  latest session in that cwd). Under a shared server, activity arrives on
  the shared event stream and the database holds all workers' sessions
  interleaved — attribution must key on session id end-to-end (spawn,
  heartbeat, transcript, token usage, recovery/resume), or workers go dark
  and cost attribution breaks. Resume (`opencode --session <id>`) also
  changes shape: re-attach becomes `session.get` + `prompt`, not a CLI flag.
- **Noisy-neighbour scheduling.** Model turns, compaction and MCP servers
  for N workers share one event loop; one worker's huge context can stall
  the others' heartbeats. Needs per-server worker caps and the existing
  per-agent memory/CPU thinking from the E1 task.

## Recommendation

Do not switch yet. The mechanism is real (sessions API verified live), and
the idle-memory math favours sharing (~150 MB + sessions vs ~400 MB+
each), but the integration surface — per-location config, permission
multiplexing, session-id-keyed activity tracking, server supervision with
per-process fallback — is a project of its own, and active-session memory
is still unmeasured. Suggested path: a small pilot (one server, 2–3 real
workers on cheap models) measuring active memory, crash behaviour and
tracking fidelity, owned by whoever takes agent plumbing next. Until then,
keep one process per worker.
