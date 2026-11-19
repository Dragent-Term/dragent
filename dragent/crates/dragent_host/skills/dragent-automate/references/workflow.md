# End-to-end workflow: from request to verified automation

All commands below were run against this build. Paths must be absolute;
quote paths with spaces. Add `--json` for machine-readable output (on by
default for agents); the examples show `--json` explicitly.

## 1. Ordinary task: a weekday check that files its work where the user finds it

Why each step matters is noted; skipping the marked step is the common mistake.

```sh
# Discover and target. Skipping this picks the wrong project or agent.
dragent capabilities --json
dragent status --json
dragent projects list --json
dragent agents list --json
dragent automations list --json
dragent automations help
```

Write the prompt to a file. It must run unattended: inputs, selection
window, deliverable, success checks, permitted mutations, and what to do
when there is no work. Never embed credentials.

```sh
cat > /tmp/weekday-checks.md <<'EOF'
Review yesterday's changes in /absolute/repo.
Deliverable: a short digest grouped into shipped, risky, stuck, next steps.
Success: every claim names a commit or file; no claim without evidence.
Mutations: none; read-only.
When there is nothing worth reporting, start the reply with "Nothing to report" and change nothing.
If the repo is missing, record that and stop; do not ask questions.
EOF
```

Preview before creating. Skipping preview schedules the wrong wall-clock time.

```sh
dragent automations preview --schedule "0 9 * * 1-5" --timezone UTC --count 5 --json
```

Create paused first so nothing runs before the trial is verified.

```sh
dragent automations create --name "Weekday checks" --prompt-file /tmp/weekday-checks.md --schedule "0 9 * * 1-5" --timezone UTC --agent dragent --project /absolute/repo --workspace new --services read --missed skip --overlap skip --retention 100 --paused --json
# Read it back: a successful create is not evidence the job works.
dragent automations get ID --json
dragent automations preview ID --count 5 --json
```

Choose an output the user can find again. `new` gives each run a fresh
worktree on its own branch (default, isolated); `branch` reuses one branch;
`pinned` reuses one folder every run. Runs are filed into sidebar folders
via `--tag` (repeatable); `--no-tags` leaves them unfoldered. For a digest
the user opens every morning, `pinned` with a tag is findable; for isolated
fixes, `new` with a tag is findable. Do not enable `--push` or
`--services write` unless the task authorises publishing or posting.

Trial only with explicit authorisation, then inspect the actual result.
`accepted` means queued, not done.

```sh
dragent automations run ID --json
# {"status": "accepted", "run_id": "…", "automation_id": "…"}
dragent automations run-get RUN_ID --json
dragent automations runs ID --limit 5 --json
dragent automations prompt get ID
```

`working`, `needs_input` or `ended` is not a successful deliverable. Verify
the files or output the run produced; read its reply in full. Only then
enable future runs:

```sh
dragent automations resume ID
dragent automations get ID --json   # enabled true, next_run_at set
```

To stop future runs without losing history: `dragent automations pause ID`.
To stop a scheduled, queued or starting run: `dragent automations cancel RUN_ID --json`.
Deleting removes the definition and its history: prefer pause.

## 2. The "nothing to do" case without writes

A prompt that can find nothing must end with `Nothing to report` (see file
above) and perform no mutations, no comments, no branch pushes. Such runs
file themselves to the inbox archive. A retry must not repost the same
message: make the prompt idempotent (check for the existing comment or fix
before acting). Verify with `run-get`: its `reply` starts with that phrase
and its `workspace` shows no unexpected changes.

## 3. State carried between runs

- `pinned` reuses the same folder, so files persist across runs.
- `--continue-session` with `pinned` sends each prompt to the agent the
  previous run left running instead of starting another.
- A watch remembers the ids it has seen (up to 1000) and the day's run
  count; the first check only notes what is there.
- `tags` file every run's workspace into the same sidebar folders.
- Retries carry `attempt` and `retry_of`; the definition carries
  `max_retries`, `retry_delay_secs`, `missed`, `overlap`, `retention`.

## 4. Drafting without enabling future runs

When the user asked for a draft, not a schedule, create `--paused` and
present the concrete configuration without resuming:

```sh
dragent automations create --name "Draft: weekly digest" --prompt-file /tmp/draft.md --schedule "0 9 * * 1" --timezone UTC --agent dragent --project /absolute/repo --workspace new --paused --json
dragent automations get ID --json
dragent automations preview ID --count 3 --json
```

Report the definition, the previewed times, and that future runs are off.
Do not run, resume or delete it unasked.

## 5. Important failure cases

- Bad schedule: `create` fails with `invalid_input` / `invalid_schedule`
  and a precise message (five fields, e.g. `"0 9 * * 1-5"`). Fix the
  expression; nothing was created.
- Bad zone: `invalid_input` / `invalid_timezone`. Use an IANA zone from
  `preview` (e.g. `America/New_York`).
- Stale id: `invalid_target` / `no_automation` or `no_run`. List again and
  pick a current id; prefixes must be unique.
- Paused trial: `run` on a paused automation fails `invalid_input` /
  `automation_paused`. Resume first, only if authorised.
- Already finished cancel: `invalid_target` / `run_not_cancellable`. Pick a
  scheduled, queued or starting run from `runs`.
- Timeout (`timeout` / `no_answer`, `retry_safe` false): it may have
  happened. Check `dragent requests show REQUEST_ID` before retrying, and
  use `--idempotency-key` for commands with outside effects.
