# Dragent

[Code reviews](docs/guide/26-pull-requests.md) covers GitHub pull requests and GitLab merge requests: the inbox, code and commit diffs, checks, review drafts, publishing, merge controls, and workspace handoffs.

## Running a development build

Every build made without `DRAGENT_RELEASE_EDITION=1` in the environment — debug or `--release`, ours or a contributor's — is the Dev edition ("Dragent Dev"). Only the release workflow sets that variable. A Dev build never touches the installed Dragent: it keeps its own data folder (`~/.dragent-dev`, or `DRAGENT_DEV_HOME`), its own background services (they live in the data folder), its own Keychain entries and its own `dragent-dev://` link scheme, and it ignores the installed app's inherited environment. On first launch it copies your preference files once from the release folder, then never again.

One command builds and runs it (from the repository root; on Windows run `scripts/dev.ps1` instead, with the same options as `-Account`, `-Server`, `-Fresh`, `-ReleaseProfile`, `-Help`):

- `scripts/dev.sh` — local mode (below)
- `scripts/dev.sh --account` — sign in to your own Dragent account, on the same service your installed Dragent uses (the script asks the installed app for its address with `dragent service-address`). The Dev build signs in as a separate device with its own device id, so your installed Dragent stays signed in, and signing out of the Dev build ends only the Dev build's session. Without Dragent installed (or with a version too old to answer), it says so and runs in local mode.
- `scripts/dev.sh --server http://localhost:8080` — use your own server: a Dragent server you run yourself, by its `http(s)://` backend URL
- `scripts/dev.sh --fresh` — after a confirmation prompt, delete the Dev data folder so the next launch starts clean (settings are copied once again)
- `scripts/dev.sh --release-profile` — build with `--release` (still the Dev edition)
- `scripts/dev.sh --help` — all options. Extra arguments after `--` are passed to the app.

Local mode is the default: with no server chosen, the build runs with no Dragent server and no account. It opens straight into the workspace as a local 'Dragent Dev' user and never contacts the server. Anything that needs an account shows 'Needs a Dragent account' instead (account plan and usage, members, teams, billing, phone pairing and browser sign-in, organisation tasks, team automations, server-held connections). The built-in agent still works with your own API key. Pass `--account` to sign in with your own account and use account features.
