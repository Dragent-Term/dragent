# Polling watches (not webhooks)

A watch is a scheduled search, not instant delivery. At each scheduled time
the run searches the connected service; it goes on only when the search finds
items it had not found before, and all of them go to its agent at once. Event
triggers needing server webhooks are out of scope.

## Before scheduling a watch

```sh
dragent integrations list --json
dragent integrations search SERVICE QUERY --json
dragent automations help
```

Verify the account is connected and the search returns what the user means.
`--watch-query` is what the service's search box takes; empty means its
recent, open or assigned items. Use `--services write` only when runs must
comment, change status or create items; default to `read`.

## Create and tune

```sh
dragent automations create --name "Triage new bugs" --prompt-file /absolute/triage.md --schedule "*/15 * * * *" --timezone UTC --agent dragent --project /absolute/repo --workspace new --watch linear --watch-query "label:bug" --check-every 15 --services read --paused --json
dragent automations update ID --watch-query "label:bug priority:high" --json
dragent automations update ID --no-watch --json   # back to a plain schedule; explicit
```

`--check-every` sets the schedule to 5, 15, 30 or 60 minutes. The schedule
label reads `Checks Linear every 15 minutes`. Replacing the trigger is
explicit: changing between schedule and watch needs `--watch` or `--no-watch`;
updating another field never touches the trigger.

## What happens on each check

- The first check only notes what is already there (primed); what shows up
  next starts a run. A hand-started `run` on an unprimed watch reports
  `nothing_new` instead of working through the backlog.
- A check that finds nothing leaves no history; a hand-started check records
  `skipped` (`nothing_new`).
- A day's runs are capped (10); beyond that, what's new waits until tomorrow
  (`watch_limit`, reported once).
- A search failure records `watch_failed` once, not every few minutes.

Because watches poll, expect delays up to the check interval, plus duplicate
suppression across retries: prompts must handle repeated runs explicitly and
must not repost for items already handled (the watch remembers up to 1000 ids).
