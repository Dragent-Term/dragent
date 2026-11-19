# Worked examples

Keys and addresses below are illustrations: use what your own `list` and
`show` return. Nothing here changes anything unless the person asked for it.

## Find the task, then read it in full

```sh
dragent issues list --status open --assignee me --json
dragent issues list --search "login redirect" --json
```

```sh
dragent issues show DRA-12 --json
```

Quote the task's status, assignee and labels from `show`; the one-line rows
of `list` are not enough to act on. A key that no longer exists fails with
`no_such_task`: list again instead of guessing.

## A follow-up you found while working

The person asked you to fix the login redirect; you notice retries are
missing too. File it as its own task:

```sh
dragent issues create "Retry the login redirect after SSO" --label bug --json
```

One task per piece of work, titled so it reads alone. Leave status,
priority and assignee unset: triage is the person's. Report the new key.

## Moving your own task with the work

```sh
dragent issues update DRA-12 --status in_progress --json
dragent issues comment DRA-12 "Reproduced it; the fix is up for review." --json
dragent issues link DRA-12 --pr https://github.com/acme/app/pull/88 --json
```

Move only the task you are on. Comment what you did and how you checked it;
the history already records the status move. Link the pull request so the
task's page shows where the work is.

## Reading history before touching a shared task

```sh
dragent issues show DRA-12 --json
```

When the history shows someone else moved it today, ask before changing it.
Never assign people, close or delete a task you don't own, and never rewrite
its description: comment instead.

## Recovering from an ambiguous write timeout

```json
{"ok": false, "status": "timeout", "code": "write_outcome_unknown",
 "message": "Couldn't reach Dragent: ... timed out", "retry_safe": false}
```

The change may exist. Do not repeat it.

```sh
dragent issues show DRA-12 --json     # is your change already in the task?
```

If it is there, you are done: report it. If not, run the command once more.
For `create`, search the exact title first. When the app answered late,
`dragent requests show ID` gives the outcome of the original request.
