---
name: dragent-doctor
description: "Diagnose Dragent layer by layer with `dragent doctor`: confirm which layer fails, recover in bounds, and prove the fix."
---

# Diagnose a Dragent problem

Use this skill when something in Dragent misbehaves: a command fails, the app
doesn't answer, sign-in or organisation looks wrong, an agent won't start, the
browser or desktop control refuses, an automation never runs. When anything
should be started, split across agents, scheduled, or changed in a connected
service, that is a different skill (`dragent-orchestrate` for team work,
`dragent-automate` for schedules, `dragent-integrations` for service items, and
`dragent-feedback` to report it). A diagnosis never authorizes follow-up work.

## Prerequisites and discovery

Needs the `dragent` launcher (in a Dragent terminal `dragent` is on PATH;
elsewhere `~/.dragent/bin/dragent`). Most checks need the app open and the
user signed in; `doctor` itself runs without either and says which prerequisite
is missing. Confirm what this build has before anything else:

```sh
dragent capabilities --json   # command families, MCP tools, bundled skills, limits
dragent doctor --help          # the checks, options, and report shape
```

`capabilities` works signed out and while the app is closed. Trust it over
memory: a command it does not list does not exist in this build.

## Establish context first

```sh
dragent status --json   # is the app running? version, organisation
```

Commands act in the organisation `status` reports and, for panes/browser/
computer, the terminal in `DRAGENT_HOST_TERMINAL_ID`. Outside a Dragent
terminal there is no pane of yours; don't pretend there is. Never switch
organisations silently. Why this matters: a CLI launched from another
environment can carry a different home, PATH, or organisation and fail for
reasons that have nothing to do with the reported symptom.

## Run doctor and read it as data

```sh
dragent doctor --json                        # every check, JSON for agents
dragent doctor --check app,signin --json      # only the suspect layers
dragent doctor --project /path/to/repo --json
dragent doctor --text                        # the same report for a person
```

Each check reports `{id, layer, status, code, summary, evidence, next}` with
`status` of `ok`, `warn`, `fail`, or `skipped` (with a reason). The report's
`status` is `ok`, or `partial` with the failing checks in `failing`. Act on
`status` and `code`, never on message wording (result contract in
`dragent capabilities --json`). Evidence is bounded and redacted: versions,
`~`-shortened paths, operation ids, timestamps, the failing layer. Never
paste tokens, keys, or file contents into it. Field reference and real
examples: `references/report.md`.

## Recover in bounds, then prove it

`references/diagnoses.md` maps each symptom to its confirming check, the
bounded recovery, and the exact failing path to repeat. Rules:

- Least disruption first: re-run the check, fix the named layer, repeat the
  original failing path. A restart is a disruptive experiment: preserve
  unsaved work and use it only when needed and within the user's request.
- Never delete app data, reset permissions, erase credentials, or reinstall
  everything as a diagnostic shortcut. "Delete everything and reinstall" is
  never a first step.
- `doctor` is read-only: it changes, starts, and writes nothing. It cannot
  fix anything; it only names the layer to fix.

## Verify and report

The check going green is not proof: repeat the original failing path and
check for regressions in the directly shared path. If the failure cannot be
reproduced, report the evidence and bounded uncertainty rather than claiming
it is fixed. `doctor` creates no state, so there is nothing to clean up.

Finish with cause, action taken, verification (the repeated path and its
result), and any concrete prerequisite still blocking progress.
