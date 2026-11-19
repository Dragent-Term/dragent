---
name: dragent-contribute
description: "Implement, test and prepare a reviewable code change in the Dragent repository, including app, host and agent integration. Use for contributing to Dragent itself, not ordinary use of its UI."
---

# Work on Dragent itself

Use this skill to change Dragent's own source in this checkout. Using
Dragent for another project is `dragent-setup`; diagnosing it is
`dragent-doctor`; reporting to the team is `dragent-feedback`. The checkout
on disk is the source of truth; older transcripts are context, not shell
instructions.

## Prerequisites and setup

Rust toolchain `1.92.0` (`dragent/rust-toolchain.toml`), run `cargo` from
`dragent/`. The disk is nearly full: prefer `cargo check` and filtered
tests, never `cargo clean`, never build into `dragent/target`, never
`cargo fmt -p dragent`. Check the checkout first; full layout:
`references/workflow.md`.

```sh
git branch --show-current; git remote -v; git status --short
git log --oneline -5
```

Do not hardcode branch or remote names: discover them with the commands
above. Preserve unrelated modifications, including another session's work.
Skipping this check is how unrelated files get committed by accident.

## Discover capabilities and boundaries

```sh
dragent capabilities --json   # commands, tools, skills, result contract
```

Main crates: `app` (desktop app, CLI in `app/src/cli`, UI preview in
`app/src/ui_preview*`), `crates/dragent_host` (host runtime, shared
`cli_requests`, `outcome`, bundled `skills/`), `crates/dragent_agent`
(agent controller) plus supporting crates (`dragent_paths`,
`dragent_persistence`, `dragent_computer`, …). Trace a behavior through UI,
CLI, shared request types, app handlers and host code before changing it.

## Implement, check and preview

```sh
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo check -p dragent --lib
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p dragent --lib cli::
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p dragent_host --lib agent_skills
```

Run the narrowest filter covering your change (e.g. `cli::account`); the
full suite is in `references/workflow.md`. Format only files you touched
(`rustfmt --edition 2021 <file>`), never the whole tree. For visible UI
changes use the preview in `references/ui-preview.md`: at most one preview
process at a time, and a screenshot never proves a handler — exercise it.

## Review, commit and report

Inspect `git diff --stat` and `git diff`; run whitespace checks. Commit on
your own branch only when the person asked; never push or open a PR
unasked. Use the real configured remote and the intended base branch from
git discovery, not a guessed URL. Details: `references/review-pr.md`.

Report: concrete problem, resulting behavior, files changed, exact checks
with results, what remains untested, and precisely what was committed,
pushed or opened (ids, URLs) versus left uncommitted. Name native vs
runtime evidence separately.
