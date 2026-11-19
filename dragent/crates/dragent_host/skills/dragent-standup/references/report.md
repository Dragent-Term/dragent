# `dragent standup` report reference

JSON by default for agents (`--json`, automatic when an agent or CI runs the
command); `--text` renders the same report for a person, `--quiet` prints
only key ids and paths. The top-level shape:

```json
{
  "status": "ok",
  "generated_at_ms": 1793462230437,
  "project": null,
  "since_ms": null,
  "limit": 20,
  "truncated": {"workspaces": false, "agents": false, "teams": false, "automations": false},
  "workspaces": [],
  "agents": [],
  "teams": [],
  "automations": [],
  "linked": [],
  "artifacts": [],
  "errors": []
}
```

## Sections

- `workspaces`: sidebar workspaces with `path`, `branch` (from git, absent
  outside a repository), `project` and `evidence: "verified"`.
- `agents`: running agent sessions with `id` (first 8 characters, enough to
  name it), `agent`, `state` (`working`, `waiting`, `needs_input`,
  `failed`, `unknown`), `state_evidence: "verified"`, `since_ms`,
  `cwd`, and `excerpt` (last screen rows, `evidence: "excerpt"`).
  `excerpt_truncated` says text was cut; `excerpt_note` explains a missing
  excerpt (host unreachable, or the session ended before it could be read).
- `teams`: each team with `status`, `updated_at_ms`, `tasks` and
  `waiting_on_user`. A task carries `status`, `kind` (`work`/`review`),
  `review_of`, `qa`, `verdict` (`approve`/`changes` for a finished review),
  `workspace`, `branch` and `evidence: "verified"`. Its `result`, when the
  worker reported one, carries `evidence: "claimed"` with `summary`,
  `files` and `checks`; `tasks_truncated` says the per-team cap applied.
- `automations`: each automation with its `schedule`, `agent` and `runs`
  (newest first, at most `--limit` each, paged through the run store until
  the window or the cap). A run carries `status`, `agent_state`, `at_ms`,
  `workspace`, `branch`, `pane`, and `reply` with `reply_evidence:
  "claimed"` plus `reply_truncated`; a failing run keeps its `error`
  (`step`, `code`, `message`) with `evidence: "verified"`.
- `linked`: groups that share a `workspace`, `branch` or agent `pane`,
  each with `teams`, `tasks`, `agents`, `runs` and a `reason` such as
  `shared workspace`. Example from a real report:

```json
{
  "id": "link-1",
  "workspace": "/Users/jahsminemma/dev/Terminal agent",
  "branch": "feat/git-gaps-scaffold",
  "teams": [],
  "tasks": [],
  "agents": ["cce181d7", "4088e74e", "c1f3c2f7", "75fe358f"],
  "runs": [],
  "reason": "shared workspace"
}
```

- `artifacts`: links (`https://…`, `/absolute/path`, `~/…`) found in
  summaries, replies, checks and excerpts, each with `from` (which item it
  came from) and `evidence: "verified"` for its presence in the record.
  Presence is not proof the target is right: open it before citing it.
- `errors`: one entry per failed source (`source`, `status`, `code`,
  `message`). Any entry makes `status` `"partial"`.

## Options

- `--project PATH`: keep only work of that project. Session automations
  (project-less folders) are excluded while filtering.
- `--since WHEN`: keep items at or after WHEN. Durations: `30m`, `24h`,
  `7d`, `2w` (also `60s`). ISO: `2026-10-25`, `2026-10-25T09:00:00Z`.
  Items without a timestamp are kept: no timestamp is not lateness.
- `--limit N`: at most N items per source and per nested list (tasks per
  team, runs per automation, waiting messages per team). Long strings are
  cut (`MAX 1200` excerpt characters, `2000` elsewhere) with a flag.
- `--setup`: a different report (current configuration); see
  `dragent-10x`. `standup setup` is the same.

## Status values

`ok` means every source read. `partial` means at least one source failed
and `errors` says which; everything else in the report still stands. There
are no other statuses on success. Failures of the command itself follow the
result contract (`ok: false` with `status`, `code`, `message`, `next`,
`retry_safe` and a matching exit code).
