# Dragent

Dragent is a desktop workspace for coding agents. Give a lead agent a goal,
and it plans the work, splits it across worker agents (Claude Code, Codex,
Gemini CLI, OpenCode and others), each in its own Git worktree, and asks you
only when a decision is yours. A paired phone follows along: what your
agents are doing, what needs you, and a way to answer.

## What's here

| Folder | What it is | Built with |
|---|---|---|
| [`dragent/`](dragent/) | The desktop app: terminal, agents, workspaces, coordination, browser | Rust |
| [`server/`](server/) | The API and the relay between desktops and phones | Go |
| [`mobile/`](mobile/) | The phone companion | Expo / React Native |
| [`auth-portal/`](auth-portal/) | Sign-in and account pages | Next.js |
| [`marketing/`](marketing/) | The website and documentation | Next.js |
| [`contracts/`](contracts/) | Wire formats shared by the apps | JSON / schemas |

Phones and desktops talk end to end encrypted: the relay forwards ciphertext
it can't read.

## Building

- **Desktop:** `cd dragent && cargo run -p dragent` (the toolchain version is
  pinned in `dragent/rust-toolchain.toml`).
- **Server:** `cd server && go run ./cmd/api` (needs PostgreSQL and Redis;
  configuration is read from the environment, see
  `server/internal/platform/config`).
- **Phone:** `cd mobile && pnpm install && pnpm start`.
- **Auth portal / website:** `npm ci && npm run dev` in their folders.

Each folder's README has the details.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. Every pull request runs the
checks listed in [.github/REQUIRED_CHECKS.md](.github/REQUIRED_CHECKS.md)
(builds, tests, linters, secret scanning, dependency and license checks) and
needs a maintainer's review. Contributors sign the [CLA](CLA.md).

## Security

Please don't open public issues for security problems. Report them as
described in [SECURITY.md](SECURITY.md).

## License

Dragent is licensed under the GNU Affero General Public License, version 3
only ([AGPL-3.0-only](LICENSE)). If you run a modified version for others
over a network, you must offer them its source. See
[LICENSING.md](LICENSING.md) for details, and [`dragent/NOTICE`](dragent/NOTICE) and
[`dragent/THIRD_PARTY_NOTICES.md`](dragent/THIRD_PARTY_NOTICES.md) for
third-party components.
