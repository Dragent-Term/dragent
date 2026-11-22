<h1 align="center">
  <a href="https://dragent.dev"><img src=".github/assets/readme/icon.png" alt="" width="64" valign="middle" /></a>&nbsp;Dragent
</h1>

<p align="center">
  <strong>Stop babysitting AI terminals. Run an agent team.</strong>
</p>

<p align="center">
  Give a lead agent your goal. It splits the work across isolated Git worktrees,
  hands each part to Claude Code, Codex or any agent you use, and asks you only
  when a decision is yours.
</p>

<p align="center">
  <a href="https://dragent.dev/download"><strong>Download</strong></a> &nbsp;·&nbsp;
  <a href="https://dragent.dev/docs">Docs</a> &nbsp;·&nbsp;
  <a href="https://dragent.dev">Website</a> &nbsp;·&nbsp;
  <a href="#build-from-source">Build from source</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0--only-0ea5e9?style=flat" alt="License: AGPL-3.0-only" /></a>
  <img src="https://img.shields.io/badge/macOS%20%7C%20Windows-0ea5e9?style=flat" alt="Platforms: macOS and Windows" />
  <a href="https://github.com/Dragent-Term/dragent/stargazers"><img src="https://img.shields.io/github/stars/Dragent-Term/dragent?style=flat&color=0ea5e9" alt="GitHub stars" /></a>
</p>

<p align="center">
  <img src=".github/assets/readme/team-demo.webp" alt="Giving Dragent a goal: the lead plans, three agents build in their own worktrees, the team asks you once, and the work is ready for review" width="880" />
</p>

## Features

### Run an agent team

Describe the goal, choose the lead and how many agents, and start the team.
The lead plans the work, decides who does what and what has to wait, and each
worker gets its own worktree and files. You follow one page: who started what,
what finished, and the questions that are yours to answer. Reply there, or from
your phone.

<a href="https://dragent.dev/docs/coordination"><img src=".github/assets/readme/coordination.png" alt="An Agent Coordination page: the lead's plan with each task's state, a permission waiting for you and the lead's question" width="880" /></a>

### A real workspace for every agent

Each worker gets a modern terminal, an editor and its own branch. Failed
commands get fixed where they broke, and finished work goes straight to
review. Open any of them when you want the inside view.

<a href="https://dragent.dev/docs/workspaces"><img src=".github/assets/readme/workspace-demo.webp" alt="A workspace: a dev server fails, Dragent diagnoses the port clash and retries, then the agent adds a test and runs it" width="880" /></a>

### An agent built into the workspace

Explore a codebase, plan a change and follow it through. Read the explanation,
expand the tool activity, and approve what the agent wants to run. It works
with Claude, OpenAI, Grok and other models, on our servers or with your own
keys, which never leave your computer.

<a href="https://dragent.dev/docs/agent"><img src=".github/assets/readme/agent.png" alt="Dragent's agent explaining a fix, its tool activity and test output, then asking before it runs a command" width="880" /></a>

### The agent wrote it. You should understand it.

GitHub pull requests and GitLab merge requests from every project in one inbox:
the files, the checks, the discussion. Comment on a line, have an agent review
it, open its workspace, or merge once the checks pass.

<a href="https://dragent.dev/docs/code-review"><img src=".github/assets/readme/review.png" alt="Reviewing a pull request: the changed file, its checks and a reviewer's comment" width="880" /></a>

### The right machine. The same workspace.

Keep the code and the compute on your SSH host and bring its files, terminal,
Git and agents into Dragent. Each machine keeps its own agent accounts, by
subscription or API key, signed in on that machine.

<table>
  <tr>
    <td width="50%" valign="top"><a href="https://dragent.dev/docs/remote-ssh"><img src=".github/assets/readme/ssh-agents.png" alt="A development server over SSH and the agents installed on it" /></a></td>
    <td width="50%" valign="top"><a href="https://dragent.dev/docs/agent-accounts"><img src=".github/assets/readme/accounts.png" alt="Agent Accounts: a machine picker, and Claude Code and Codex accounts by subscription or API key" /></a></td>
  </tr>
</table>

### Your agents, in your pocket

<table>
  <tr>
    <td width="300" valign="top">
      <a href="https://dragent.dev/docs/mobile"><img src=".github/assets/readme/phone-demo.webp" alt="Dragent on an iPhone: a desktop's workspaces, then Claude Code working on a fix and finishing with the tests passing" width="280" /></a>
    </td>
    <td valign="top">
      Pair your phone once by scanning a code on your desktop. When an agent
      needs you, the question reaches your phone: answer it, follow the chat,
      or pick up a terminal, and the work keeps going on your machine.
      <br /><br />
      Every workspace on your desktops is there, the ones that need you first,
      along with your agent teams, tasks and usage.
      <br /><br />
      Phone and desktop talk end to end encrypted: the relay only ever
      forwards ciphertext.
      <br /><br />
      <a href="https://dragent.dev/docs/mobile">More about the phone app</a>
    </td>
  </tr>
</table>

### A browser your agents can use

Every workspace has a real browser beside the code. Agents open the app they
built, click through it and check the screen at desktop and phone widths, and
you can step in whenever you like.
[More about the browser](https://dragent.dev/docs/browser)

<a href="https://dragent.dev/docs/browser"><img src=".github/assets/readme/browser-demo.webp" alt="Claude Code opening a checkout page in the built-in browser, paying, confirming the result and checking the phone width" width="680" /></a>

<details>
<summary><strong>And more</strong>: automations, integrations, CLI and MCP</summary>

<br />

<table>
  <tr>
    <td width="50%" valign="top">
      <strong><a href="https://dragent.dev/docs/automations">Automations</a></strong><br />
      Agents that start on a schedule, in a project or on their own.<br /><br />
      <img src=".github/assets/readme/automations.png" alt="The Automations page with scheduled agents" />
    </td>
    <td width="50%" valign="top">
      <strong><a href="https://dragent.dev/docs/integrations">Integrations</a></strong><br />
      Start work from an issue in Linear, GitHub, GitLab, Jira, Sentry and more.<br /><br />
      <img src=".github/assets/readme/integrations.png" alt="Connected services on the Integrations page" />
    </td>
  </tr>
</table>

- **[CLI](https://dragent.dev/docs/cli):** inspect sessions, create workspaces and start teams from any terminal.
- **[MCP](https://dragent.dev/docs/mcp):** give agents Dragent's coordination, browser and workspace tools.
- **[Agent accounts](https://dragent.dev/docs/agent-accounts):** several sign-ins per agent (Work, Personal), chosen per project or per session.

</details>

## Supported agents

Bring any agent onto the team, and keep your own subscriptions.

<p align="center">
  <a href="https://dragent.dev/docs/agents"><img src=".github/assets/readme/agents.png" alt="Supported agents: Dragent, Claude Code, Codex, OpenCode, Cursor, Gemini CLI, GitHub Copilot, Amp, Antigravity, Auggie, Autohand Code, Charm, Cline, CodeBuddy Code, Codebuff, Command Code, Continue, Devin, Droid, Freebuff, Goose, Grok, Hermes Agent, Jules, Junie, Kilocode, Kimi, Kiro, Letta, MiMo Code, Mistral Vibe, Muse Code, Oh My Pi, Pi, Prime Agent, Qoder CLI, Qwen Code, Rovo Dev and Zero" width="880" /></a>
</p>

Any other command-line agent can be added by its command.

## Install

Download Dragent for macOS or Windows from **[dragent.dev/download](https://dragent.dev/download)**.
The [quickstart](https://dragent.dev/docs/quickstart) takes you from install to
your first agent team.

## Build from source

| Folder | What it is | Built with |
|---|---|---|
| [`dragent/`](dragent/) | The desktop app: terminal, agents, workspaces, coordination, browser | Rust |
| [`mobile/`](mobile/) | The phone companion | Expo / React Native |
| [`auth-portal/`](auth-portal/) | Sign-in and account pages | Next.js |
| [`contracts/`](contracts/) | Wire formats shared by the apps | JSON / schemas |

- **Desktop:** `cd dragent && cargo run -p dragent` (the toolchain version is
  pinned in `dragent/rust-toolchain.toml`).
- **Phone:** `cd mobile && pnpm install && pnpm start`.
- **Auth portal:** `cd auth-portal && npm ci && npm run dev`.

Each folder's README has the details.

The hosted service (accounts, sync and the relay between desktops and
phones) is run by Dragent and isn't in this repository. The apps, the
end-to-end encryption and the wire formats they speak are all here, so you
can check what the relay can see: everything your devices send each other
is end-to-end encrypted, and the relay sees only what it needs to connect
them (which devices are paired, and routing and sign-in details).

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
[LICENSING.md](LICENSING.md) for details, and
[`dragent/NOTICE`](dragent/NOTICE) and
[`dragent/THIRD_PARTY_NOTICES.md`](dragent/THIRD_PARTY_NOTICES.md) for
third-party components.
