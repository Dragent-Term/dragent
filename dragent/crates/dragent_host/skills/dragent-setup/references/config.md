# The `.dragent/workspace.json` schema (version 1)

One file at the repository root. The user-local override
`.dragent/workspace.local.json` uses the same schema and merges over the
shared file key by key: `init`, `cleanup`, `env_required` and `copy` lists
replace wholesale when the override sets them, `launch` profiles merge by
name. Keep the local file uncommitted (gitignore it).

## Complete example: a Node service

Repository shape: `package.json` with `ci`, `build`, `dev --port 3000` and
`clean` scripts, a committed `.env.example`, and an ignored local `.env`.

```json
{
  "version": 1,
  "env_required": ["DATABASE_URL"],
  "copy": [
    {"from": ".env", "to": ".env"},
    {"from_example": ".env.example", "to": ".env"}
  ],
  "init": [
    {"run": ["npm", "ci"], "timeout_secs": 600},
    {"run": ["npm", "run", "build"], "timeout_secs": 300}
  ],
  "launch": {
    "dev": {
      "steps": [
        {"run": ["npm", "run", "dev", "--", "--port", "3000"]}
      ],
      "ready": {"port": 3000, "timeout_secs": 30}
    }
  },
  "cleanup": [
    {"run": ["npm", "run", "clean"]}
  ]
}
```

The two `copy` entries work as a pair: the first brings the ignored `.env`
from the project's main checkout when it exists; the second creates `.env`
from the committed example when the worktree still lacks one. Neither ever
overwrites: an existing destination is always preserved, so running `setup
init` twice is safe.

## Sections

- `init`: steps, run in order, stopping at the first failure.
- `launch`: named profiles. Each profile runs its setup steps to completion,
  then its LAST step keeps running in the background (recorded with pid,
  start time, log path). Optional `ready`: `{"port": N}` waits for a TCP
  accept on localhost, `{"url": "..."}` waits for an HTTP 2xx/3xx;
  `timeout_secs` defaults to 30.
- `cleanup`: steps, run after this worktree's launches are stopped.
- `env_required`: variable NAMES only, never values. `init` fails early
  naming the missing ones without printing anything else.
- `copy`: explicit relative files. `from` names an ignored file in the main
  checkout, `from_example` a committed example in the worktree, `to` the
  destination in the worktree, `if_missing` (default true) preserves an
  existing destination. No directories, no globs, no absolute paths.

## Steps

Exactly one of `run` or `shell`:

- `{"run": ["npm", "ci"]}` executes the program directly: no shell, no
  expansion, arguments with spaces passed literally.
- `{"shell": "npm ci && npm run build"}` runs `sh -c` on macOS/Linux and
  PowerShell (`-NoProfile -Command`) on Windows. Quote accordingly: POSIX
  single-quotes do not survive PowerShell, so prefer `run`, or gate the
  step with `platforms`.

Every step may set:

- `cwd`: repository-relative directory, must stay inside the worktree
  (`../escape` and absolute paths are refused with their field path).
- `platforms`: any of `macos`, `linux`, `windows`; omitted means all.
- `timeout_secs`: default 600 (init/cleanup), 1–86400. A timeout kills only
  that step's process tree (its process group) and reports `step_timeout`.
- `env`: literal non-secret values. Secrets stay out: name them in
  `env_required` and read them from the environment.

Skipped-platform steps are reported, not failures. The first failing step
ends the run with its index, exit code and the tail of its output.

## Validation errors

`setup check` refuses a bad config with `invalid_input`/`invalid_config`
and the JSON path of the error: `init[0].cwd`, `launch.dev.steps[1].shell`,
`launch.dev.ready`, `copy[0].from`, `version`. Fix the named field.
