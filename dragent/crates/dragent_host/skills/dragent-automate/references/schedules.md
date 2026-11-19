# Schedules: time zones, DST, policies, history

## Cron and time zones

Five fields: minute hour day month weekday, e.g. `"0 9 * * 1-5"` is weekdays
at 9:00. Names work for months and weekdays (`mon`, `jan`); Sunday is 0 or 7.
A schedule carries an IANA timezone (`--timezone ZONE`, default: the
machine's). Runs happen at local wall-clock times in that zone: `0 9 * * *`
with `America/New_York` runs at 9:00 in New York. Always pass the user's real
zone; never default to an example. Read the definition back: `get --json`
shows `schedule`, `schedule_label`, `timezone`, `missed`, `overlap`,
`max_retries`, `retry_delay_secs`, `retention`.

## Next-run preview (creates nothing)

```sh
dragent automations preview --schedule "0 9 * * *" --timezone UTC --count 5 --json
dragent automations preview ID --count 5 --json
dragent automations preview --schedule "30 2 * * *" --timezone America/New_York --from 2026-03-07T00:00:00Z --count 3 --json
```

`--count` is 1-100 (default 5); `--from` is RFC3339 or milliseconds
(default: now). `--schedule` previews a candidate; an id previews the stored
definition. Both fail as `invalid_input` (`invalid_schedule` or
`invalid_timezone`) with a precise message when the input is bad.

## Daylight saving

A skipped local time never runs that day; a repeated local time runs once,
at its first occurrence.

- Spring forward: `30 2 * * *` in `America/New_York` skips 2026-03-08
  (02:30 never happens) and runs next on 2026-03-09 02:30 EDT. Preview shows
  Mar 7 then Mar 9.
- Fall back: `30 2 * * *` in `Australia/Sydney` repeats 02:30 on 2026-04-05;
  it runs once at the first 02:30 (+11:00), then next on 2026-04-06.

Preview a DST boundary with `--from` before promising a time.

## Missed runs (app closed or machine asleep)

An occurrence that goes stale before it starts is `skipped` (`missed_window`)
by default, and the next occurrence is planned; a closed laptop never wakes
up to a burst. With `--missed catch-up`, the most recent stale occurrence
runs once on return instead of being skipped. Visible as `missed` (`skip` |
`catch_up`) on the definition.

```sh
dragent automations create … --missed catch-up --json
dragent automations update ID --missed skip --json
```

## Overlapping runs (previous still starting)

- `skip` (default): the new occurrence is `skipped` (`already_running`).
- `queue`: it stays `queued` and starts when the previous run leaves the
  starting states.
- `allow`: it starts alongside the previous run.

Visible as `overlap` (`skip` | `queue` | `allow`).

```sh
dragent automations create … --overlap queue --json
```

## Retry policy (bounded, with backoff)

`--max-retries N` (0-10, default 0) retries a failed start; `--retry-delay
SECS` (10-86400, default 300) is the base delay. Backoff doubles each time:
base, base*2, base*4, … capped at a day. Retries wait as queued runs with a
future time and show `attempt` (1, 2, …) and `retry_of` (the failed run).
Prompts must be idempotent so a retry never double-posts.

```sh
dragent automations create … --max-retries 3 --retry-delay 300 --json
```

## Pause, resume, cancellation

```sh
dragent automations pause ID
dragent automations resume ID
dragent automations cancel RUN_ID --json   # scheduled, queued or starting
dragent automations cancel ID --json       # an automation id cancels its plan
```

Pausing removes the plan; resuming plans the next occurrence again. Cancelling
a plan records it `skipped` (`cancelled`); cancelling a queued or starting
run records it `cancelled`. Finished runs cannot be cancelled
(`run_not_cancellable`). Use pause for temporary suspension, not to claim a
running agent stopped; inspect agents separately.

## Run history, retention, reading a result

```sh
dragent automations runs ID --limit 10 --json
dragent automations runs ID RUN_ID --json   # one run's result
dragent automations run-get RUN_ID --json
```

History is newest first with `status`, timing (`scheduled_at`, `started_at`,
`finished_at`, `at`), outcome (`error`, `error_code`, `reply`, agent state),
`attempt`, `warning`, `workspace` and `branch`. `--retention N` (1-1000,
default 100) keeps the plan plus the newest N past runs; older finished runs
are pruned (never one still queued, starting, or whose agent is still
followed). Partial updates preserve every other field; replacing the trigger is
explicit (`--watch` to add, `--no-watch` to remove).
