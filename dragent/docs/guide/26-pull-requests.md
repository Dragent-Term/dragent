# Code reviews: pull requests and merge requests

Applies to Dragent 0.2.3 · Updated 2026-10-26

Choose **Code reviews** in the left sidebar. This full-page inbox, separate from workspace tabs, discovers GitHub pull requests and GitLab merge requests for Git projects in your current organisation. The active workspace is the default publishing target.

## Connect and find work

During onboarding, choose **GitHub** or **GitLab** in the Git provider card. Install its CLI if needed, then choose **Connect** to sign in through Dragent's terminal. GitLab users can select GitLab.com or a self-managed instance in the prompts. Connecting the chosen provider is enough to finish onboarding; GitHub is not required for GitLab users. When signing in on an SSH machine, choose GitLab's Device login method and open the displayed URL in your local browser.

For GitHub, install GitHub CLI (`gh`) and run `gh auth login`. For GitLab, install GitLab CLI (`glab`) and run `glab auth login`. Sign in on the machine that owns the project; SSH projects use the remote machine's CLI and credentials. From Code reviews choose **Connections**, pick the project, then **Sign in to GitHub** or **Sign in to GitLab** to open an embedded terminal modal over Code reviews on that machine with the login command. The page stays open and no workspace tab is created. Close the modal when finished; the inbox refreshes automatically. **Manage machine tools** opens Settings → Machines for that machine, where `gh` and `glab` appear under System tools. Dragent does not copy provider tokens between machines. Self-managed GitLab instances use the CLI login for their host.

Filter by project, open/merged/closed state, your authored work, requested reviews, items needing attention, or merge readiness. The search box matches loaded titles, authors, branch names, project names and request numbers. **Search provider** runs remote search. GitHub accepts its usual qualifiers; GitLab accepts plain text plus `author:`, `label:` and `head:`. Load more results when needed, up to 500 per project.

The inbox refreshes while visible and stops polling when you leave. Project results appear as each machine responds, starting with up to 30 reviews per project; **Load more** retrieves the next batch. Review titles and branches appear immediately on selection, followed by the overview, files and discussions as their requests finish. Recently opened reviews are reused when switching between them; **Refresh** always requests fresh data. Subsequent refreshes keep the current content visible. A failed project read keeps its last successful rows and shows a diagnostic. A successful empty response removes the old rows. Results, drafts and in-flight callbacks are scoped to the current organisation. Signing out clears the screen immediately.

## Review a pull request

- **Overview:** description, branch direction, author, change statistics, reviewers, assignees, labels, and the next blocking action. Copy a review summary or the PR link.
- **Files:** tree/list navigation, highlighted unified or aligned split diffs, renamed-file information, and per-head **Viewed** marks. Hide viewed files to focus on what remains. Long patches render progressively. PNG, JPEG and WebP changes can be previewed through the project machine's authenticated provider CLI, with size and dimension limits.
- **Commits:** inspect a single commit, copy its SHA, or return to all changes. Commit comparisons are read-only and cannot accidentally receive PR-head line comments.
- **Checks:** separate passed, failing, running and skipped/cancelled states; inspect failed logs, open the check on its provider, and explicitly rerun failed GitHub Actions jobs or retry failed GitLab CI jobs.
- **Activity:** descriptions and comments render Markdown. Read reviews and complete review-thread conversations, reply, resolve or reopen threads, and jump to their files.

Click a changed line to draft a comment. **Select a line range** lets you choose the last line on the same side of the diff. **Add to review** queues the comment locally; **Review drafts** shows every queued comment before submission. On GitHub, submit a comment-only review, approval, or request for changes as one review. On GitLab, submit comments as discussions, approve through the approvals API, or request changes by publishing GitLab draft notes with a review state. Blocking change requests depend on the GitLab project and tier. GitLab will not publish pre-existing pending notes from this screen; publish or discard those on GitLab first. Direct GitLab discussions post one at a time; if a later post fails, refresh before retrying to avoid duplicates. A conversation comment or thread reply can also be posted independently.

Review drafts and viewed marks are saved on this device inside the organisation's Dragent directory, in `pull-request-reviews.json`. They are not sent to the provider until you submit. Switching requests preserves drafts. When new commits arrive, old line comments are kept as copyable earlier drafts and must be checked against the new diff before reuse.

## Publish and maintain

Choose the project/workspace first, then **New pull request** or **New merge request**. Enter a title and Markdown description, choose a base from the repository branch selector, and optionally create a draft. **Create** uses the current published branch; **Push branch & create** explicitly pushes first without forcing. The provider validates empty changes, duplicates and permissions.

Edit request details and the base branch, request/remove reviewers, manage assignees and labels, change draft/ready state, close, or reopen. The provider remains authoritative for permissions and repository rules.

On GitHub, choose squash, merge commit or rebase when the repository permits it. On GitLab, choose squash or the project's configured merge method; **Rebase onto target** is a separate source-branch update. Merges include an atomic reviewed-head check. Branch updates check the reviewed head immediately before the request; GitLab's rebase endpoint cannot make that check atomic. Auto-merge can be enabled or cancelled. Merge, close, base updates and job reruns have confirmation dialogs. GitHub administrators can explicitly choose a branch-requirement override with a separate acknowledgement. GitLab never offers that bypass. Merging or closing does not automatically delete local workspaces or branches.

## Work with an agent

**Review with agent** opens a setup dialog over Code reviews. It shows the source and target branches, exact reviewed commit, project machine, agent/model selectors, and editable prefilled review instructions. Opening or cancelling setup creates no branch or worktree. The review editor is separate from your project composer, so existing drafts and attachments are preserved.

**Start review** fetches the provider's PR/MR head ref on the project machine, verifies its full commit SHA, and creates an isolated worktree on a named local review branch. It leaves your current checkout and uncommitted work untouched. A clean managed review worktree can be reused only when it still matches that exact commit. Dirty worktrees, changed heads, and unrelated folders are preserved and reported instead of reset or overwritten. Starting a review of newer commits uses a separate snapshot rather than moving an older review underneath a running agent.

The default task asks the agent to inspect changes, relevant code, checks and discussions, run appropriate tests, and present actionable findings with file/line references. It instructs the agent to obtain explicit direction before editing tracked files, committing, pushing, merging or publishing feedback. These are task instructions, not an operating-system sandbox. Review comments and submissions remain explicit actions in Code reviews.

**Open workspace** opens a compact confirmation with the PR/MR, source and target branches, pinned commit and project machine. It performs no fetch or creation until confirmation. Confirming creates or reopens the isolated review checkout with a plain terminal, without launching an agent. Local changes in an existing snapshot are preserved, and an already registered workspace is reused. This PR-specific flow has no branch or worktree selectors: its reviewed commit determines the checkout. General workspace selectors refresh their options without changing the user's selection. Failing checks, conflicts and unresolved findings can also hand a focused task to an agent through review setup. Coordination is off for the review launch.


## Shortcuts

- `Cmd/Ctrl+F`: focus PR search.
- `Cmd/Ctrl+R`: refresh.
- `Cmd/Ctrl+Enter`: submit the current comment, review or form. It does not confirm a merge.
- `Escape`: dismiss a dialog or project menu.
- `Cmd/Ctrl+W`: close an embedded terminal modal first; otherwise leave the PR screen, preserving the underlying workspace.

## Limits and recovery

This screen supports GitHub and GitLab repositories, including authenticated SSH-hosted development contexts. A remote runtime must support the typed review request; update/reconnect older runtimes. GitLab approval, blocking change requests, project merge method, and CI behavior depend on the GitLab instance and repository settings. GitLab comment-only reviews are not an atomic batch.

Binary formats other than the supported raster images, very large responses, unavailable permissions and provider-truncated patches have an **Open full file** or provider fallback. Failed reads do not invalidate successfully loaded sections. Diagnostics use the shared application error modal, outside the input area.

Network operations have time and output limits. If a write times out, refresh before retrying: the provider may already have applied it. Review and merge operations always remain subject to the provider's permissions and policies.

## Development verification

Native previews use production renderers with fictional data and never publish comments or mutate a remote repository:

```sh
DRAGENT_DEMO_CAPTURE=1 DRAGENT_PREVIEW_PR_ONLY=1 cargo run -p dragent --example ui_preview -- /tmp/dragent-pr-preview
cargo test -p dragent_host --lib pull_requests
cargo test -p dragent --lib pull_requests
```

A read-only authenticated smoke check exercises the same transport and parsers:

```sh
cargo run -p dragent_host --example pull_request_read -- /path/to/repository [PR_NUMBER]
```
