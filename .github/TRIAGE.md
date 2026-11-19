# Triage — labels and response expectations

Keep the label set small at launch; add labels
only when a query or automation consumes them.

## Intake

- Blank issues are disabled (`ISSUE_TEMPLATE/config.yml`): every issue arrives
  via the bug or feature form, already labeled `bug` / `enhancement`.
- Questions go to Discussions; security reports go to the private channel in
  `SECURITY.md` — never the issue tracker.

## Label set (seed)

- Type: `bug`, `enhancement`, `documentation`, `duplicate`, `invalid`,
  `question`, `wontfix`, `good first issue`, `help wanted`
- State: `untriaged` (default until a maintainer looks), `needs-repro`,
  `has-repro`, `pending-user-response`, `ready-to-spec`, `ready-to-implement`,
  `no-autoclose` (stale exemption), `keep-open` (stale exemption)
- Risk/size (match the PR template's risk statement): `risk:low`,
  `risk:medium`, `risk:high`, `size/xs`, `size/s`, `size/m`, `size/l`, `size/xl`
- Area: `server`, `dragent`, `auth-portal`, `ci`, `security`, `docs`
- Origin: `dependencies` (Dependabot), `security` (audit findings)

## Stale pull requests (`.github/workflows/stale.yml`)

- Day 7 of inactivity (no pushes, no comments): automated nudge comment.
- Day ~10 of inactivity: automated final-warning comment.
- Day ~14 of inactivity: auto-close. Any contributor push or comment resets
  the clock.
- Maintainers can apply `no-autoclose` to exempt a PR (releases, blocked or
  long-running work).

## Spec flow labels

- `ready-to-spec`: maintainer agrees the feature is worth speccing; the
  author writes the short spec (problem + design + test plan).
- `ready-to-implement`: spec approved; code PRs welcome (link the spec from
  the PR). Bug fixes skip this flow — they go straight to a code PR once
  reproducible.

## Automation (to add when staffing allows)

- OS/area auto-labeling from issue bodies.
- No triage SLA is promised publicly; aim for a first maintainer response
  (label change or comment) within 3 business days, matching `SECURITY.md`'s
  acknowledgement window.
