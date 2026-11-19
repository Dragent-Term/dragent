# Dragent development workflow

Run `cargo` from `dragent/`. Toolchain `1.92.0` via
`dragent/rust-toolchain.toml`; build flags via `dragent/.cargo/config.toml`.
No `CLAUDE.md` or `AGENTS.md` in this checkout; `dragent/README.md` points
at the Code reviews guide (`docs/guide/26-pull-requests.md`).

## Start from discovery, not memory

```sh
git branch --show-current; git remote -v; git status --short
git log --oneline -5
dragent capabilities --json | head -60
```

Why: branch and remote names differ per checkout — never hardcode `main`,
`master` or `origin`. `status --short` shows unrelated modifications you
must preserve. `capabilities --json` is the authority for commands, tools,
skills and the result contract; it works signed out.

## Crate boundaries

- `dragent/app` — desktop app and CLI. CLI lives in `app/src/cli/`
  (`mod.rs` dispatch, `account.rs` feedback, `capabilities.rs`,
  `requests.rs`, `presentation.rs`). App-side request handling in
  `app/src/workspace/cli_requests.rs`; private feedback dialog in
  `app/src/workspace/feedback.rs` (`POST /api/v1/feedback`). UI preview in
  `app/src/ui_preview*` with example `app/examples/ui_preview.rs`.
- `dragent/crates/dragent_host` — host runtime read by app and CLI:
  `src/cli_requests.rs` (shared `Request`/`Response`), `src/outcome.rs`
  (result contract), `src/agent_skills.rs` plus `skills/<name>/` packages
  embedded by `crates/dragent_host/build.rs`.
- `dragent/crates/dragent_agent` — agent controller and pipelines.
- Supporting: `crates/dragent_paths` (home, organisation dirs),
  `crates/dragent_persistence`, `crates/dragent_computer`,
  `crates/dragent_pty`, `crates/dragent_terminal`, `crates/warpui*`.

Trace a behavior through UI, CLI flags, shared request types, app handlers
and host code before changing it. Do not infer a working backend from a
toggle in the UI.

## Builds share one small target folder

```sh
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo check -p dragent --lib
```

Two other agents share that folder, so cargo may wait on the lock — that is
expected. The disk is nearly full: prefer `cargo check` and filtered tests;
never build into `dragent/target`, never `cargo clean`, never
`cargo fmt -p dragent` (it reformats hundreds of files). Format only files
you touched:

```sh
rustfmt --edition 2021 <file>
```

## Checks to run

Run the narrowest filter covering the change, from `dragent/` with the
shared target variables above:

```sh
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p dragent --lib cli::account
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p dragent --lib cli::
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p dragent_host --lib agent_skills
CARGO_TARGET_DIR=/Users/jahsminemma/dev/dragent-worker-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo check -p dragent --example ui_preview
```

`cli::account` covers the feedback path; `cli::` covers the whole command
layer including dispatch and capabilities; `agent_skills` covers bundled
skill packaging. All three passed for the feedback change. Prefer these
over full-workspace builds.
