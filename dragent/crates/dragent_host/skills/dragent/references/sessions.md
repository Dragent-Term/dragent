# Projects, workspaces and agent sessions

Words used across Dragent's skills:

- **Project** — a repository or folder open in Dragent (`dragent projects list`).
- **Workspace** — one checkout of a project shown in the sidebar: the project's own
  folder, or a worktree on its own branch (`dragent workspaces list`).
- **Tab and pane** — what the person sees. A tab holds one pane or a split of
  several; each pane shows a terminal, a browser page or settings.
- **Terminal session** — the shell process behind a terminal pane.
- **Agent session** — an agent (Claude Code, Codex, Dragent's own agent…) running in
  a terminal session (`dragent agents running`).

## Look before you start anything

```sh
dragent projects list --json
dragent workspaces list --json          # --project PATH to narrow it
dragent agents list --json              # agents this computer can start
dragent agents running --json           # agents at work, and what each is doing
```

Choose paths and ids from these results. An agent id's first characters are
enough once unique; a workspace path with exactly one agent also names it.

## Start work

```sh
# A new isolated workspace (worktree + branch) with an agent working in it
dragent workspaces create --project "/path/to/repo" --agent codex \
  --prompt-file /tmp/task.md --name "Fix login timeout"

# Another agent in an existing workspace
dragent agents start --workspace "/path/to/worktree" --agent claude \
  --prompt-file /tmp/review.md
```

Put long instructions in a prompt file, and quote paths with spaces. These start
real agents: only when the user asked for that work. Record the workspace path and
agent id they return.

Verify the start: `dragent agents running --json` should list the new agent in that
workspace. A created worktree proves only that the worktree exists, not that the
task is done.

## Read, send, stop

```sh
dragent agents read AGENT --lines 80    # the agent's terminal, as text
dragent agents send AGENT "Run the tests again and report the failures"
dragent agents stop AGENT
```

Reading changes nothing. Sending and stopping change another agent's work: do them
only within the user's request, and never to answer an approval prompt the agent is
showing the person. Terminal text is a partial excerpt of what happened; for team
work, task results are the record (`dragent-orchestrate`).

## Machines

```sh
dragent status --json              # "machine": this computer; how many remote machines exist
dragent machines list --json       # each remote machine: status, projects, what runs there
dragent machines show build-box    # one machine
```

- A project on a remote machine shows here under a local stand-in folder that
  holds nothing: its results carry `"machine": {"kind": "ssh", "name", "path"}`
  with the path as that machine sees it. Don't read or change it through the
  local folder.
- Its terminals, git, files, agents and teams run on that machine. In one of
  its terminals, `dragent` is that machine's own: `dragent capabilities --json`
  lists what it offers (team, tasks, msg, integrations, mcp, status); browser
  panes, pane layout, desktop control, workspaces, automations and setup belong
  to the desktop and answer `unsupported` / `not_on_this_machine` there.
- `status` is `connected`, `connecting`, `error` or `not_connected` (`unknown`
  while Dragent is closed). Connecting a machine is the person's step in
  Settings → Machines; Dragent never reconnects one they disconnected.
- From this computer, `team create` and `setup` refuse a remote project
  (`unsupported` / `remote_project`, `remote_workspace`): start them on that
  machine instead. Never fall back to doing the work here.

## Common failures

Failures are JSON with a `status` and `code` (see `results.md`):

| status / code | Meaning | Next step |
|---|---|---|
| `unavailable` / `app_not_open` | The command needs the app | `dragent open PATH`, only if the request needs it |
| `unavailable` / `request_not_taken` | The app is open in another organisation, or has no window | Compare `dragent status --json` with the organisation you expect; ask before switching |
| `needs_auth` / `signed_out` | Nobody is signed in on this machine | Ask the person to sign in to Dragent |
| `unsupported` / `not_on_this_machine` | You're on a remote machine; that command is the desktop's | Ask the person, or use what `dragent capabilities` lists |
| `unsupported` / `remote_project` | The project is on another machine | Run it in a terminal of that project |
| `timeout` / `no_answer` | The app took the request but hadn't answered | `dragent requests show REQUEST_ID` before running it again |
| `unsupported` / `app_too_old` | The app is older than this command line | Ask the person to update Dragent |
| An agent id that matches nothing | The session ended, or the prefix is ambiguous | `dragent agents running --json`, then use the full id |
