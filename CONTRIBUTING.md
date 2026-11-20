# Contributing to Dragent

Thanks for helping build Dragent! This guide covers how to contribute.
Please also read our [Code of Conduct](./CODE_OF_CONDUCT.md).

## License of your contributions (inbound = outbound)

By submitting a contribution you agree it is licensed under the
**GNU Affero General Public License, version 3 only (AGPL-3.0-only)**,
like the rest of this repository (inbound = outbound). You also agree to
the Individual Contributor License Agreement in [`CLA.md`](./CLA.md)
(currently a draft pending counsel review), which grants Nimbletech Ltd
the rights to redistribute contributions under the AGPL-3.0-only and
under any commercial license terms for the project, including future
licensing needs. The repository offers a separate commercial license for
users who cannot meet the AGPL terms (see [`LICENSING.md`](./LICENSING.md));
the CLA is what lets the project offer contributed code under those
commercial terms too — every contribution still lands under AGPL-3.0-only
as well.

Copyright attribution policy: the single copyright line is
`Copyright (c) 2026 Nimbletech Ltd`. Contributors keep copyright in their own
work; do not add per-file author tags. Third-party code keeps its own
license headers — never strip them.

## How to contribute

1. Fork the repo and branch from `main` (`git checkout -b fix/short-name`).
2. Make focused pull requests: one issue per PR, small diffs preferred.
3. Bug-fix and docs PRs can go straight in. For features or behavior
   changes, please open an issue first so we can agree on scope.
4. Fill in the pull-request template (what changed, how tested, linked
   issue). Allow edits from maintainers so we can rebase small fixes.
5. CI must pass (see "PR checks" below). A maintainer will review; please
   respond to review comments and keep the conversation in the PR.

## Changelog entries

Every PR that changes behavior must add a one-line entry under
`CHANGELOG.md` » `Unreleased` (`Added` / `Changed` / `Fixed`). Docs-only and
refactor-only PRs are exempt — mark the PR template checkbox N/A instead.

## Feature specs before code

Features need maintainer readiness before code PRs: the issue gets
`ready-to-spec`, the author writes a short spec (problem + design + test
plan), and code PRs open once a maintainer applies `ready-to-implement`.
Commit the spec under `specs/<issue-number>/` (or keep it in the issue for
small ones) and link it from the PR. Bug fixes skip this flow — once
reproducible, they go straight to a code PR.

## Stale pull requests

PRs with no pushes or comments for 7 days get an automated nudge, a final
warning at ~10 days, and auto-close at ~14 days (see `.github/TRIAGE.md`).
Maintainers can apply `no-autoclose` to exempt long-running PRs.

## Sign the Contributor License Agreement (CLA)

Every contributor signs the CLA once — not per commit — before their first
contribution can merge. [`CLA.md`](./CLA.md) holds the agreement text.

1. Open (or update) your pull request. The CLA Assistant bot comments with
   a link to the CLA.
2. Follow the link and sign in with GitHub to sign. The `license/cla`
   check on the PR turns green once every committer has signed; until
   then the PR stays blocked.
3. If the check still shows you as unsigned after signing, use the recheck
   link in the bot's comment. New commits or new committers re-run the
   check automatically.

Corporate contributors: if your employer or anyone else holds rights in
work you create, get authorization (for example, an authorized signatory or
a corporate agreement on file with the project) before contributing, and
make sure the representations in `CLA.md` hold for your contributions.
When in doubt, have your employer contact the maintainers first.

Signed commits (for example `git commit -S`) remain welcome and encouraged
as extra provenance, but they do not replace the CLA signature above.

## Dev setup per subproject

This is a monorepo. First, at the repository root, run `npm install` once:
it turns on the git hooks (they refuse credential files, scan for secrets
when [gitleaks](https://github.com/gitleaks/gitleaks) is installed, and
check Go and Rust formatting). Then set up only the parts you are changing:

- **Server** (`server/`, Go 1.25): `go build ./...`, `go test ./...`.
  See `server/` sources for package layout (`go.mod` module `server`).
- **Desktop** (`dragent/`, Rust; toolchain per `rust-toolchain.toml`):
  `cargo build`, `cargo test`. Workspace members are listed in
  `dragent/Cargo.toml`; the app crate lives in `dragent/app/`.
- **Auth portal** (`auth-portal/`, Next.js): `npm install`, `npm run dev`,
  `npm run build`. See [`auth-portal/README.md`](./auth-portal/README.md).
- **Mobile** (`mobile/`, Expo / React Native): `pnpm install`,
  follow the Expo workflow in [`mobile/README.md`](./mobile/README.md).

Check each subdirectory README for the authoritative commands; if a README
and this file disagree, the README wins — and please file a PR fixing this
file.

## PR checks

CI (`.github/workflows/`) runs builds, lints, typechecks, and tests for the
touched subprojects, plus secret, dependency, license and workflow checks;
[`.github/REQUIRED_CHECKS.md`](./.github/REQUIRED_CHECKS.md) lists them all. Before pushing, run at minimum the checks for the code
you changed (build + tests for Go/Rust, `lint` + `tsc --noEmit` + `build`
for the Next.js apps). Keep PR titles in conventional-commit style
(`fix: …`, `feat: …`, `docs: …`) so the changelog stays readable.

## Secret hygiene

Never commit `.env` files, API keys, tokens, or private keys. If you push a
secret by accident, rotate it immediately and tell a maintainer — rewriting
history alone is not enough once a secret has been pushed.

## Questions?

Open a discussion or an issue — bug reports and docs fixes are contributions
too. Welcome aboard.
