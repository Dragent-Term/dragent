## What & why

<!-- One-paragraph ELI5, then the reason this change is needed. -->

## Linked issue

<!-- Required for outside contributors: Fixes #123. Maintainers: link or explain why none. -->

## Readiness / spec

<!-- Features: link the approved spec (`specs/<issue-number>/` or the issue) and confirm the issue carries `ready-to-spec` → `ready-to-implement`. Bug fixes / docs: N/A with reason. -->

## How I tested it

<!-- Commands run and their result, e.g. `go vet ./...`, `cargo check`, `npx tsc --noEmit`.
Every added test must fail without this change, or say why no test was added. -->

- [ ] `ci.yml` jobs that cover the touched paths pass (or are unrelated — explain)

## Security impact

<!-- Blast radius, trust boundaries touched, input handling, authN/authZ, secrets.
Write `None` with a one-line reason if nothing security-sensitive changes. -->

## Secrets check

- [ ] No secrets, tokens, private keys, or private customer data are included
      in this PR (code, fixtures, screenshots, or logs)

## License / attribution check

- [ ] `LICENSE`, `LICENSING.md`, and `NOTICE` / third-party notices are intact;
      no third-party license text was altered; new files carry the project
      license header where the project uses one

## Checklist

- [ ] CLA signed (CLA Assistant check passes / PR carries the `cla-signed` label; if stale, I commented `recheck`)
- [ ] Small and focused; risk stated above with a revert plan for anything risky
- [ ] Self-reviewed the diff; cross-platform impact considered where relevant
- [ ] Screenshots / recordings attached for UI or behavior changes, else N/A with reason
- [ ] Docs updated (or N/A with reason)
- [ ] Changelog entry added under `CHANGELOG.md` » `Unreleased`
      (or N/A: docs-only / refactor-only)
