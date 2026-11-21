# Required checks before a pull request can merge

Set these in **Settings → Branches → Branch protection** (or a ruleset) for
`main`. A pull request merges only when every one of them is green.

## Status checks (all required)

| Check | Workflow | What it guards |
|---|---|---|
| `server (go vet / build / test)` | ci.yml | Go format, vet, build and tests under the race detector |
| `dragent (cargo metadata / check)` | ci.yml | Rust manifests locked, changed files formatted, workspace builds |
| `dragent (tests)` | ci.yml | The desktop workspace's tests |
| `auth-portal (typecheck / lint / build)` | ci.yml | Types, lint and a production build |
| `mobile (typecheck / lint / test)` | ci.yml | Types, lint and the phone app's tests |
| `ciphertext-only-proof`, `clients` | e2ee.yml | The relay never sees plaintext; clients interoperate |
| `secret-scan (gitleaks)` | security.yml | No credentials in the change or the history |
| `dependency-review` | security.yml | No new dependency with a known vulnerability or a disallowed license |
| `govulncheck (server)` | security.yml | The server never reaches a known-vulnerable function |
| `cargo-deny (licenses / sources / bans / advisories)` | security.yml | Every Rust dependency is AGPL-compatible and from a reviewed source |
| `codeql (…)` | security.yml | Static analysis of Go and JavaScript/TypeScript |
| `actionlint`, `zizmor`, `hadolint` | workflow-lint.yml | Workflows have no mistakes, injection or excess permissions; the Dockerfile is sound (when they change) |
| `license/cla` | CLA Assistant app (cla-assistant.io) | Every committer signed the CLA |

Not a merge gate, but published: `scorecard.yml` rates the repository's
security practices weekly (OpenSSF Scorecard). Releases carry signed build
provenance (`gh attestation verify <installer> --repo <owner>/<repo>`) and
an SPDX SBOM.

## Local hooks

`npm install` at the repository root turns on the git hooks (husky):
pre-commit refuses credential files, scans the change for secrets (when
gitleaks is installed) and checks Go and Rust formatting of what's staged;
pre-push scans the commits being pushed. They're a convenience: CI is
the gate.

## Rules on the branch

- Require a pull request; no direct pushes to `main`, for anyone.
- Require approval from a code owner (see `CODEOWNERS`), and dismiss stale
  approvals when new commits are pushed.
- Require the branch to be up to date before merging.
- Require conversation resolution.
- Require signed commits.
- Block force pushes and deletion.

## Repository settings

- Actions → General: **require approval for all outside collaborators**
  before workflows run on their pull requests, and set the default
  `GITHUB_TOKEN` permission to read-only.
- Actions → Runners: no self-hosted runners on the public repository.
- Security: enable private vulnerability reporting, Dependabot alerts and
  secret scanning with push protection.
