# Improvement playbook: patterns with trade-offs

Recommend from evidence, in small numbers. Each pattern names its focused
skill (which owns the exact commands, verification and undo), the trade-off
to state plainly, and what stays manual.

## 1. A repeatable check becomes an automation (`dragent-automate`)

Evidence: the user runs the same triage on a schedule (dependency audit,
inbox sweep, flaky-test watch). Set up only with an explicit schedule,
destination and write permission. Trade-off: unattended runs cannot ask
questions, so the prompt must define the no-work case and forbid repeated
external posts; a misfiring schedule annoys people silently. Verify with a
paused definition first, then one authorized trial run inspected end to end.
Undo: `automations pause ID` (keeps history) or `delete ID`. Stays manual:
judgment calls the check surfaces.

## 2. Independent coding work becomes a team (`dragent-orchestrate`)

Evidence: separable tasks with clear done criteria the user currently runs
one by one. Trade-off: coordination overhead and agent cost against
calendar time; reviews and a QA pass are part of the plan, not optional
extras. Verify each task's checks on its branch before merging anything.
Undo: `team stop` / `tasks cancel`; branches remain until deleted. Stays
manual: the plan's scope decisions and the final merge.

## 3. A long server or log gets its own pane (`dragent-panes`)

Evidence: a dev server or tail the user restarts because its output scrolls
away inside a working terminal. Trade-off: panes are local layout, not
documentation; nobody else sees them. Verify the process stays up and its
output is readable. Undo: close the pane. Stays manual: reading the log.

## 4. Repeated issue lookups use a connected service (`dragent-integrations`)

Evidence: the user pastes issue text the integration could have fetched
with context. Trade-off: connecting a service and granting write scope
widens what agents can touch; prefer read-only unless posting is the job.
Verify the search returns the right items before building on it. Undo:
revoke in Settings → Integrations. Stays manual: the reply itself.

## 5. A repository that breaks in fresh worktrees gets setup (`dragent-setup`)

Evidence: agents stall on install, env files or toolchains that the main
checkout has and worktrees lack. Trade-off: setup scripts run for every
workspace, so keep them fast and hermetic. Verify by creating a workspace
and running the check there. Undo: revert the setup file. Stays manual:
project-specific secrets, which never belong in the script.

## What not to propose

More agents, more schedules, or broader permissions as an end in itself;
any change the user did not ask for; anything justified by invented usage
statistics. When the evidence is thin, the deliverable is the focused
question, not a configuration.
