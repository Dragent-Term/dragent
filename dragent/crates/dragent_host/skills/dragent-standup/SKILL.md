---
name: dragent-standup
description: "Summarize what existing Dragent agents, teams, workspaces and automation runs did while the user was away, with evidence of completion, blockers and unfinished work."
---

# Evidence-based agent recap

Use this skill when the user asks what happened while they were away: what
agents, teams and automation runs did, what is done, and what needs them.
When anything should be started, sent, changed or closed, that is a different
skill: `dragent-orchestrate` to split work across agents, `dragent-automate`
for schedules, `dragent-integrations` to act on service items. A recap never
authorizes follow-up work; handle that as separate scope when asked.

## Prerequisites and discovery

Needs Dragent's CLI on PATH (in a Dragent terminal `dragent` is already
there). The recap reads local records, so it works while signed out, but live
excerpts need the host reachable; without it the report is `partial` (see
below), never an error. Confirm what this build has before anything else:

```sh
dragent capabilities --json   # command families, MCP tools, bundled skills, limits
dragent standup --help        # the recap's own options
```

`capabilities` works signed out and while the app is closed. Trust it over
memory: a command it does not list does not exist in this build.

## Establish scope first

Ask or infer the project (`--project PATH`) and the time window
(`--since 24h`); without a window, report what the records actually cover
rather than inventing one. Reads act in the current organisation: never
switch it silently. Why this matters: an unbounded recap across every
project mixes unrelated work and hides the decision the user faces.

```sh
dragent standup --json                              # everything recent, bounded
dragent standup --project /path/to/repo --json      # one project only
dragent standup --since 7d --limit 10 --json        # one week, 10 items per source
dragent standup --text                              # the same report for a person
```

`--limit N` caps each source (default 20, at most 100); long texts arrive
truncated with a `*_truncated` flag. Field reference: `references/report.md`.

## Read the answer as data

A successful report carries `"status": "ok"`, or `"partial"` when one source
failed: each failure is an entry in `errors` (`source`, `status`, `code`,
`message`) and the rest of the report still stands. Never match on message
wording; act on `status` and `code` (`references/report.md`, and the result
contract in `dragent capabilities --json`). Every item carries `evidence`:

- `verified` — a recorded result or finished run status (a task's stored
  outcome, a run's `failed`/`started` state, a workspace's branch).
- `claimed` — an agent's own statement (a result summary, a run reply).
- `excerpt` — terminal text (an agent's screen, last rows first).

`scope` says which machines the report covers: agent sessions, teams and
automations are this computer's; workspaces on remote machines are listed with
their `machine`, and those machines' own teams and sessions are named under
`scope.not_covered`. Say so in the recap rather than reporting them as idle.

`linked` groups one workspace, agent session, team task and automation run
that share a workspace, branch or pane, with the `reason`. Report linked work
once, not three times. `artifacts` collects links found in those records;
verify each before relying on it. Terminal quiet is not evidence of idleness:
a quiet screen with a recorded `done` result is done, and a promise to test
is not a passing check.

## Failures and bounded recovery

| Failure | Meaning | Next step |
|---|---|---|
| `errors` entry, source `host`, `app_not_open` | Host unreachable: aliveness and excerpts unchecked | Report local records as `partial`; retry once the host is back |
| `invalid_input` / `usage` | Bad flag, e.g. `--since bogus` | Fix the argument; nothing happened (`retry_safe: true`) |
| `needs_auth` / `signed_out` | Follow-up writes need sign-in | Ask the person to sign in; keep the recap read-only |

Never retry indefinitely and never fabricate missing history. Say which part
of the recap is incomplete. Worked ordinary and failure cases:
`references/examples.md`.

## Verify and report

The command returning is not proof: cross-check a claimed done against the
worktree (`git status`, recent commits, the diff on the task's branch) and a
claimed test pass against the check output. This skill creates no state, so
there is nothing to clean up.

Finish with a concise recap grouped by outcome (done and verified, ready for
review, blocked or needs-user, still running, failed or uncertain), with
workspace paths, branch, task/run ids, artifact links, evidence for each
claim, which agents are still active, and the concrete next decision for
anything blocked — without answering its approval prompt yourself.
