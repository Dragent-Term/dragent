# Diagnoses: symptom → check → recovery → re-run

Each row: the symptom, the `dragent doctor` check that confirms it, the
bounded least-disruptive recovery, and the original failing path to repeat as
proof. Run the check first; never start from the recovery.

| Symptom | Confirm with | Bounded recovery | Prove it by repeating |
|---|---|---|---|
| `dragent` missing or stale (wrong version, points elsewhere) | `launcher` (`launcher_missing`, `launcher_stale`) | Open the Dragent app once so it writes its launchers, then rerun `--check launcher` | The original `dragent …` command |
| CLI and app disagree (`version_mismatch`, `request_unreadable`) | `app` | Update the Dragent app to match this command line, then rerun `--check app` | The request that refused (`dragent requests show ID` for a timed-out one) |
| App unavailable (not open, no answer) | `app` (`app_not_open`, `no_answer`) | `dragent open PATH` starts it, if the path needs the app; then rerun `--check app` | The command that needed the app |
| Wrong workspace or organisation | `signin`, plus `dragent status --json` vs the visible app | Run from the right Dragent terminal (one whose environment names its pane); never switch organisations silently | The command in the failing workspace/organisation |
| Sign-in failure | `signin` (`signed_out`) | Ask the person to open Dragent and sign in, then rerun `--check signin` | `dragent whoami`, then the failing command |
| Provider account failure (binding refuses, stale entry) | `agents` (`account_binding_broken`) | `dragent agents accounts status ID` to see it; rebind with `dragent agents accounts default --agent KEY --account ID\|system\|inherit`, or remove the stale entry with `dragent agents accounts remove ID` | Rerun `--check agents`, then start the agent |
| Agent executable missing | `agents` (`agents_ready` lists per-agent `installed`) | Install that agent's CLI, then rerun `--check agents` | `dragent agents list`, then start the agent |
| MCP transport failure (`dragent mcp` lists no tools) | `mcp` (`mcp_no_tools`) | `dragent integrations list`; fix what it names, then rerun `--check mcp` | The exact tool call that failed |
| Tool permission denied (computer action refused) | `desktop` | Read the refusal's `code` (`dragent capabilities --json` lists computer codes); fix Settings or ask the person for the OS grant — only the person grants OS permissions — then rerun `--check desktop` | The exact refused action |
| Browser failure (pane unreachable, page won't load) | `browser` | From a Dragent terminal with the app open: `dragent browser status`, `snapshot`, `console`; fix the target server or address, not the pane | The exact `dragent browser …` command |
| Desktop OS permission failure | `desktop` | `dragent computer permissions` shows what is missing; the person grants it in System Settings (desktop control must also be enabled in Dragent's Settings), then rerun `--check desktop` | The exact `dragent computer …` command |
| Stale desktop target (element/window gone) | `desktop` | Take a fresh `dragent computer snapshot APP` — element numbers expire after an action or minutes — and act on the new numbers | The action with fresh element numbers |
| Automation not running | `automations` | `scheduler: idle (app closed)` means the app must be open; a failed last run names its `error` — read it with `dragent automations runs ID --limit 1`, fix what it says, and let the schedule (or one `dragent automations run ID`) try again | Rerun `--check automations`, then the schedule |

## When it doesn't reproduce

State the evidence and the uncertainty: which check failed with which `code`,
what the repeated path did instead, and what would confirm it (e.g. "fails
only while the app is closed" or "one failed run among passing ones").
Do not claim it is fixed, do not retry in a loop, and do not escalate to
deleting data or reinstalling. If the trail is inconclusive, collect the
bounded evidence (`--check` output, `dragent status --json`, the repeated
command's result) and report it as unverified: cause unknown, action none,
verification pending.

## What not to do

- Delete app data, reset permissions, erase credentials, or reinstall as a
  first (or second) step. These destroy evidence and rarely touch the layer
  `doctor` named.
- Answer approval prompts, switch organisations, or stop/close/message things
  you didn't start, unless the user asked.
- Retry a timed-out command blindly: `dragent requests show REQUEST_ID` says
  whether the app finished it. Give commands with outside effects an
  `--idempotency-key` so a retry can't repeat them.
