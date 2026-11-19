---
name: dragent-automate
description: "Create, preview, run or troubleshoot scheduled Dragent agents and polling service watches, including cron, time zones, missed/overlap/retry policies, run isolation and run results."
---

# Scheduled agents and polling watches

Use this when the user wants work to start on its own later. For a one-off
task to do now, use workspaces and agents directly (`dragent` skill). For
reading or changing service items by hand, use `dragent-integrations`. For
splitting work across agents, use `dragent-orchestrate`.

## Prerequisites

- The CLI: `dragent` on PATH in a Dragent terminal, else
  `~/.dragent/bin/dragent` (macOS/Linux) or
  `%USERPROFILE%\.dragent\bin\dragent.cmd` (Windows).
- Sign-in for anything but `preview --schedule` and `help`; `run` also needs
  the app open. If sign-in, the app or a command is missing, say so.
- Never switch organisation or workspace silently; never approve for the person.

## Discover what this build does

```sh
dragent capabilities --json   # command families, skills, result contract
dragent automations help      # subcommands and options for this build
```

Trust these over memory: a subcommand not listed there does not exist here.

## Identify the target first

```sh
dragent status --json
dragent projects list --json
dragent workspaces list --json
dragent agents list --json
dragent automations list --json
```

Take ids and paths from these results. Reuse an automation when changing the
same job; read `automations get ID --json` before any update.

## Workflow (details: `references/workflow.md`)

```sh
dragent automations preview --schedule "0 9 * * *" --timezone UTC --count 5 --json
dragent automations create --name "Weekday checks" --prompt-file /absolute/job.md --schedule "0 9 * * 1-5" --timezone UTC --agent dragent --project /absolute/repo --workspace new --services read --missed skip --overlap skip --paused --json
dragent automations get ID --json
dragent automations preview ID --count 5 --json
dragent automations run ID --json          # authorised trial only; answers accepted + run_id
dragent automations run-get RUN_ID --json  # inspect the actual result
dragent automations runs ID --limit 5 --json
```

Creating proves nothing: a successful create is not evidence the job works.
A trial run is `accepted`, not done: read its result, then verify the files
or output it produced. Choose an output the user can find again (a pinned
folder, a branch, or tagged worktrees; see workflow). Prompts that can find
nothing must say `Nothing to report` and change nothing. Pinned workspaces,
`--continue-session`, and a watch's seen ids carry state between runs.
Drafting without future runs means `--paused` plus the concrete config.

## Read results as data

Failures are JSON with `status`, `code`, `next`, `retry_safe` and a matching
exit code; act on those, never on message wording. `invalid_input` means
nothing happened (fix the arguments); `invalid_target` means the id is stale
(list again); `accepted` means follow the returned `run_id`. Details and the
retry/timeout rules are in `references/results.md` of the `dragent` skill.

## Failures, verification, cleanup

`pause` stops future scheduling; `cancel RUN_ID` stops a scheduled, queued
or starting run; `resume` restarts. `update` changes only supplied fields.
Deleting removes history: prefer pause. Common cases and the exact recovery
for each are in `references/schedules.md` and `references/watches.md`.

Verify by reading back the definition, previewing, and inspecting the trial's
result and its output. Delete or pause what you created unless the user asked
for future runs. Report ids, schedule, timezone, target, write permissions,
trial result with evidence, whether future runs are on, and how to clean up.
