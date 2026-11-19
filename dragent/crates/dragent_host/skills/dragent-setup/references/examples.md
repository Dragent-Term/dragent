# Worked setup sessions

PATH defaults to the current directory's repository in every command; the
examples pass it explicitly because stating the target is what keeps a
command from running in the wrong checkout.

## Ordinary: from nothing to a running dev server

Why in this order: `check` is the only read-only command, so a broken config
or a missing variable is found before anything runs.

```sh
dragent setup check /path/to/repo --json
# trusted, init steps for this platform, "missing_env": ["DATABASE_URL"], next steps

export DATABASE_URL=postgres://localhost/app
dragent setup init /path/to/repo --json
# ok, steps_run 2, copied [".env"]

dragent setup init /path/to/repo --json
# ok again, copied [] — the edited .env was preserved, never overwritten

dragent setup launch dev /path/to/repo --json
# pid, log_path, ready {"ready": true, ...}

curl -s http://127.0.0.1:3000/health   # verify yourself: ready is a probe, not proof
dragent setup status /path/to/repo --json
# last_init ok, launches [{profile dev, running true, ...}]

dragent setup stop dev /path/to/repo --json
dragent setup cleanup /path/to/repo --json   # stops the rest, runs cleanup steps
```

New worktrees of a trusted project with a config init themselves in the
background after creation; `setup status` in the new worktree shows that
outcome. Nothing is created for untrusted projects or repos without a
config.

## Failure: untrusted project

```sh
dragent setup init /path/to/repo --json
# status needs_approval, code untrusted_project — nothing ran
```

Bounded recovery: open the project in Dragent, accept the trust prompt,
run again. There is no flag to bypass this; do not retry blindly and never
answer the prompt for the person.

## Failure: bad config

```sh
dragent setup check /path/to/repo --json
# status invalid_input, code invalid_config,
# message: init[0].cwd: "../escape" escapes the worktree (..)
```

Bounded recovery: fix the named field (`references/config.md` for the
schema), `check` again. Nothing ran — retrying the same file is safe
(`retry_safe: true`).

## Failure: missing environment

```sh
dragent setup init /path/to/repo --json
# status failed, code missing_env, message names SETUP_REQUIRED (names only)
```

Bounded recovery: set the named variables in the environment and run again.
`init` checks these before copying or running anything, so no half-done
state needs cleaning.

## Failure: a step fails, or times out

```sh
dragent setup init /path/to/repo --json
# status failed, code step_failed: init step 1 (npm run build) failed
# with exit 1, plus the tail of its output
```

Bounded recovery: fix the step or the repository, then run again — earlier
steps already ran, and `init` is safe to repeat. A timeout
(`timeout`/`step_timeout`) killed only that step's process tree; raise its
`timeout_secs` or make the step faster. Never kill processes by port or
executable name: `setup stop` signals only the recorded pid whose start
time still matches, so a reused pid can never kill something else.

## Failure: unknown profile

```sh
dragent setup launch nope /path/to/repo --json
# status invalid_input, code unknown_profile (have: dev)
```

Bounded recovery: pick a profile from `setup check`. Nothing started.
