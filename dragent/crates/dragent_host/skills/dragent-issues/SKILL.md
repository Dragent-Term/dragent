---
name: dragent-issues
description: "Work with the organisation's task list in Dragent: find tasks, create follow-ups you discover, move your task's status, comment progress, and link your pull request. Use for shared tasks rather than a connected tracker item."
---

# Organisation task work

Use this when the work lives in the organisation's own task list in Dragent:
follow-up tasks you find while working, the status of the task you are on,
progress notes for it, and the pull request that closes it. A Linear, Jira or
GitHub item is `dragent-integrations`; the lead/worker plan of a team session
is `dragent-orchestrate`.

In a Dragent terminal `dragent` is on PATH (else `~/.dragent/bin/dragent`, or
`%USERPROFILE%\.dragent\bin\dragent.cmd` on Windows). The person must be
signed in. The same operations exist as the `issues_*` tools of Dragent's MCP
server (`dragent mcp`) and as `dragent issues` in a shell, with the same
results and failures. Prefer the tools you have; use the CLI when one is
missing.

## 1. Find the task first

```sh
dragent issues list --json                        # open tasks, most recently updated first
dragent issues list --status all --search "login redirect" --json
dragent issues show DRA-12 --json                 # the task, its comments and history
```

A task is named by its key (`DRA-12`) or id. `list` filters: `--status
open|closed|all|KEYS` (keys: backlog, todo, in_progress, in_review, done,
canceled), `--assignee me|none|EMAIL`, `--label L`, `--project here|KEY`
(`here` is the checkout you run in), `--search WORDS`. Read `show` before
changing anything: the history says who did what, and a stale key fails with
`no_such_task` — list again instead of guessing.

## 2. The changes you may make

```sh
dragent issues create "Follow up: retry the login redirect" --label bug --json
dragent issues update DRA-12 --status in_progress --json
dragent issues comment DRA-12 "Reproduced it; the fix is in review." --json
dragent issues link DRA-12 --pr https://github.com/acme/app/pull/88 --json
dragent issues link DRA-12 --item linear:ENG-12 --json
```

- **Follow-ups you find.** File a task for work the person would want done
  (a bug you noticed, a step the plan missed). One task per piece of work,
  titled so it reads alone. Leave status, priority and assignee unset unless
  the person said otherwise.
- **Your own task's status.** Move it as the work moves (`in_progress` when
  you start, `in_review` with the pull request, `done` when it lands). Only
  yours: a task someone else owns is theirs to move.
- **Progress notes.** Comment what you did and how you checked it, briefly.
  The history already records status moves; don't narrate those.
- **Your pull request.** Link it with `--pr URL` so the task's page shows
  where the work is. A tracker item goes with `--item PROVIDER:KEY`
  (like `linear:ENG-12`).

Changes you make as an agent are marked as yours in the task's history. A
failure is JSON with `status`, `code`, `next` and `retry_safe`: act on those,
not the message's wording.

## 3. What you must not do unasked

- **Assign people.** Never set or clear an assignee unless the person asked:
  an assignment lands in someone's list under their name.
- **Close or delete others' tasks.** Never mark done, cancel, delete or
  restore a task you don't own. Your own task is done when its work landed
  and you checked it.
- **Rewrite descriptions.** A description is shared context, not your notes:
  comment instead.
- **Repeat a timed-out write.** It may have happened: `show` the task first
  and look for your change before trying again.

Worked examples: `references/examples.md`.
