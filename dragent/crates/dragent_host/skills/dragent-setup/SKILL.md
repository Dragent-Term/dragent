---
name: dragent-setup
description: "Make a repository set itself up in every new Dragent workspace: a versioned .dragent/workspace.json with init steps, launch profiles, cleanup, required env names and files to copy, verified with setup check, init, launch, status, stop and cleanup."
---

# Reproducible workspace setup

Use this skill when the user wants a repository to work in every fresh
workspace without manual steps: dependencies, local config, dev servers,
teardown. When the request is about schedules, use `dragent-automate`;
about acting on service items, `dragent-integrations`; about reporting what
happened, `dragent-standup`; about changing Dragent's own source,
`dragent-contribute`.

## Prerequisites and discovery

Needs Dragent's CLI on PATH (in a Dragent terminal `dragent` is already
there). Repository commands run only for trusted projects, on this machine,
regardless of sign-in state. Confirm what this build has before anything
else:

```sh
dragent capabilities --json   # the setup family is listed when this build has it
dragent setup help            # the commands and their arguments
dragent status --json         # the organisation the commands act in
```

`capabilities` works signed out and while the app is closed. Trust it over
memory: a command it does not list does not exist in this build. Never
switch organisations without being asked; identify the project with
`dragent workspaces list --json` or `dragent projects list` and pass its
path explicitly when more than one could be meant.

## Write the config, then verify it for real

The repository owns one file, `.dragent/workspace.json` (`"version": 1`),
plus an optional uncommitted `.dragent/workspace.local.json` that merges
over it key by key (gitignore the local file; Dragent never edits
`.gitignore`). Full schema and a complete Node example with quoting rules:
`references/config.md`. Why this order matters: writing the file is cheap
and running it is the only proof, so never hand over an unverified config.

```sh
dragent setup check [PATH] --json    # validate, plan for this platform, missing env
SETUP_REQUIRED=1 dragent setup init [PATH] --json      # copy files, run init steps
dragent setup init [PATH] --json     # again: safe, copied files are preserved
dragent setup launch dev [PATH] --json   # start the dev profile (pid, log, ready)
dragent setup status [PATH] --json   # last init result, running launches
dragent setup stop dev [PATH] --json     # stop one launch (or every launch: no profile)
dragent setup cleanup [PATH] --json  # stop launches, then run cleanup steps
```

PATH defaults to the current directory's repository; it may contain
spaces. Worked ordinary and failure cases: `references/examples.md`.

## Read the answer as data

A failure is JSON with `status`, `code`, `next` and `retry_safe`, plus a
matching exit code; act on those, never on message wording. The checks that
matter: `invalid_input`/`invalid_config` names the bad field
(`init[0].cwd`); `needs_approval`/`untrusted_project` means nothing ran —
open the project in Dragent, accept the trust prompt, and run again (there
is no flag to bypass this); `failed`/`step_failed` and
`timeout`/`step_timeout` name the step, exit code and the tail of its
output. Launch prints `pid`, `log_path` and the readiness result; treat
`ready: false` as `unverified`, not done. New worktrees init themselves in
the background when the config exists and the project is trusted; `setup
status` shows that outcome.

## Trust, secrets, Windows

`env_required` holds variable NAMES only — `setup check` lists which are
missing without printing values, and `setup init` fails early naming them.
Keep literal non-secret values in a step's `env`; keep secrets out of both
config files and prompt text. Prefer `{"run": [...]}` (no shell); use
`{"shell": ...}` only for pipelines, remembering it is `sh -c` on macOS
and Linux but PowerShell on Windows — gate shell syntax with `platforms`.
Every path stays inside the worktree; `copy` names explicit files only
(`from` an ignored file, or `from_example` to create one when missing) and
never overwrites with `if_missing`. Stopping signals only the recorded
process whose start time still matches — never by port or executable name —
so check `setup status` rather than killing anything yourself.

## Finish with evidence

Run the project's own build or tests after `init`, and open the launched
URL or port yourself: the command returning is not proof the app works.
This skill creates the two config files, copied local files, installed
dependencies and log files under the organisation's `workspace-setup`
folder; remove only what this task owns (`setup stop`, `setup cleanup`,
the scratch worktree) and leave everything else as found.

Report the config files written, commands run, env names, copied files,
init result, launch pid plus readiness, the verified outcome, and the
concrete next step for anything blocked — without answering approval
prompts yourself.
