# Skill evals (offline half)

Repeatable, offline evaluation for Dragent's bundled skills. Live runs
against real models are a separate step; nothing here runs a model.

## What's covered

- `scenarios/*.json` — one file per skill, each holding one or more
  scenarios. Every skill in `crates/dragent_host/skills/` has at least one
  positive scenario, plus the boundary cases from the spec:
  - arrange panes without starting a team (`panes-arrange-no-team`)
  - summarize workers without messaging them (`standup-recap-no-messages`)
  - test an embedded page, not desktop control (`browser-embedded-not-computer`)
  - operate a native app through desktop control (`computer-native-app`)
  - read an issue without posting (`integrations-read-only`)
  - diagnose an unavailable service without reinstalling (`doctor-unavailable-service`)
  - draft an automation paused, never enabled (`automate-draft-paused`)
  - update a page without duplicating it (`page-update-no-duplicate`)
  - take over a team without starting another lead (`orchestrate-takeover-no-create`)
  - preserve customized resources during setup (`setup-preserve-custom`)
- Failure-recovery cases: a timed-out command checked with `requests show`
  before retrying, a stale pane/element id, an ambiguous search result,
  pagination, two agents on the same team, an integration conflict.
- `run.py` — the runner (Python 3 standard library only).
- `fakes/fake_agent.py` — a scripted fake agent that proves the runner.
- `agent.json` — agent command templates (`{request}`, `{scenario}`,
  `{workspace}`, `{home}`, `{fakes}`).
- The static checks live in `dragent/app/src/cli/skill_evals.rs`
  (test-only): skill commands/flags must exist in this build, scenarios
  must parse and reference real commands, and coverage must hold.

Each scenario has: `id`, `request` (in the person's words), `expected_skill`,
`must_not_use_skills`, `fixture` (one of `empty`, `page`, `server`,
`team-context`, `service-results`, `no-app`), `expect`/`forbid` (commands by
family/subcommand, e.g. `"panes open"`), `success` (evidence phrases
separated by `;`), and `notes`.

## Run the static checks

From `dragent/` with the shared target folder:

```sh
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -p dragent --lib cli::skill_evals
```

This verifies every `dragent <family> <subcommand>` in skill code blocks
against this build's help texts and parsers, every flag against its
command's help, and every scenario file against the command tables.
Real drift found in a skill is listed in `KNOWN_DRIFT` in `skill_evals.rs`
with a reason (currently empty: no drift) rather than fixed in the skill.

## Run the offline suite (canned, no model)

```sh
cd crates/dragent_host/skill-evals
python3 run.py --agent fake --canned \
  --binary /Users/jahsminemma/dev/dragent-worker-target/debug/dragent \
  [--select SCENARIO_ID ...] [--out DIR]
```

`--canned` answers `dragent` calls from fixture responses; the real binary
is still used for the build version and the command list. `--agent
fake-wrong` runs a deliberately wrong agent that must score as failing.
Everything temporary (throwaway `DRAGENT_HOME`, fixture git repo, shim,
call log) lives under one per-run temp dir that is removed afterwards;
only the reports in `--out` (default `reports/`) remain.

## Run live evaluations with a real agent

1. Add the agent to `agent.json`, e.g.
   `"live": "claude -p {request}"`, with `"model"` set to the model name.
2. Run without `--canned` (the shim forwards to `--binary`, recording
   every call). Use `--select` to start with one scenario, `--keep` to
   keep temp dirs on failure, `--timeout` for long runs.
3. Live runs act on the real app and services: only run scenarios whose
   writes you intend (the runner never adds `--draft` itself).

## What the report means

`eval-<stamp>.json` carries agent, model, build version, platform and
per-scenario results; `eval-<stamp>.md` is the readable summary.

Per scenario:

- **Skill** — `ok` when the run's declared skill (`Using skill: <name>` in
  the transcript, else the plurality vote over non-discovery calls) equals
  `expected_skill`.
- **Expect** — every `expect` command prefix-matched a recorded call.
- **Forbid** — no `forbid` command ran, and no `must_not_use_skills`
  skill's command family was used.
- **Duplicates** — repeated side-effect calls with identical arguments
  (an `--idempotency-key` retry never counts).
- **Unsupported** — calls the binary rejected as unknown.
- **Evidence** — matched vs needed phrases from `success` (at least half,
  rounded up, must appear verbatim in the final answer).

A scenario passes only when all six hold.
