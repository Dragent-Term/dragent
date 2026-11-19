import type { ReactNode } from "react";
import { P, H3, UL, LI, Kbd, C, Note, Table } from "./primitives";

export type DocSection = {
  id: string;
  label: string;
  title: string;
  summary?: string;
  body: ReactNode;
};

export type DocGroup = { label: string; sections: DocSection[] };

/**
 * The documentation content. Kept as data so the page and its sticky sidebar
 * stay in sync automatically. This web guide is manually aligned with the
 * in-app user guide; it is not generated from the repository Markdown files.
 */
export const DOC_GROUPS: DocGroup[] = [
  {
    label: "Getting started",
    sections: [
      {
        id: "introduction",
        label: "Introduction",
        title: "What is Dragent?",
        summary:
          "A modern terminal with a built-in AI agent that fixes failed commands and helps you code — output organized into clean, addressable blocks.",
        body: (
          <>
            <P>
              Dragent is a fast desktop terminal for macOS and Windows that
              combines a real terminal, block-based output, collaboration, and a
              first-class AI coding agent. Where a traditional terminal is a
              passive text buffer, Dragent actively understands what you&apos;re
              trying to do.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Block-based output.</b> Every command
                and its output is grouped into a distinct, addressable block you
                can copy, re-run, filter, and share.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Self-healing.</b> When a command
                fails, Dragent diagnoses it and offers a one-click fix.
              </LI>
              <LI>
                <b className="text-theme-fg/80">A real agent.</b> Plan, read and edit
                files, run commands with your approval, search your codebase, and
                use packaged skills — right in the terminal.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Intelligent input.</b> One input bar
                handles both shell commands and natural language, with ghost-text
                autosuggestions and rich editing.
              </LI>
            </UL>
            <Note>
              Dragent&apos;s own code is open-source software licensed under
              the GNU Affero General Public License, version 3 only. It incorporates some
              third-party open-source components, credited on the{" "}
              <a
                href="#licenses"
                className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
              >
                Licenses
              </a>{" "}
              section.
            </Note>
          </>
        ),
      },
      {
        id: "install",
        label: "Install & first run",
        title: "Install & first run",
        body: (
          <>
            <H3>Install</H3>
            <P>
              Download the installer from the <a href="/pricing#download" className="text-accent underline underline-offset-2">download section</a> and choose the build for your operating system and architecture. Dragent updates itself after you restart, so you do not need to reinstall for every release.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">macOS</b> — open the <C>.dmg</C> and
                drag Dragent into Applications, then launch it. It&apos;s
                notarized, so it opens without a developer warning.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Windows</b> — run the installer, then
                launch Dragent from the Start menu.
              </LI>
            </UL>
            <H3>Before you open a project</H3>
            <UL>
              <LI>Make sure your project is available locally and open the folder from the project picker.</LI>
              <LI>Use the integrated terminal first to confirm the project runs with its normal command.</LI>
              <LI>Keep secrets out of the project or add them to <C>.dragentignore</C>; ignored paths are not indexed or sent as context.</LI>
            </UL>
            <H3>First-run onboarding</H3>
            <P>
              The first launch is a short setup, not a required tutorial. You can
              finish it in order or skip a step and configure it later in Settings.
            </P>
            <UL>
              <LI><b className="text-theme-fg/80">Welcome</b> — learn the difference between terminal, agent, and cloud-agent tabs.</LI>
              <LI><b className="text-theme-fg/80">Appearance</b> — choose a solid or image theme, font, and terminal density.</LI>
              <LI><b className="text-theme-fg/80">Workspace</b> — choose or open the folder where you do your work.</LI>
              <LI><b className="text-theme-fg/80">Account</b> — sign in in your browser to use Dragent-hosted models, or continue with your own key/local model.</LI>
            </UL>
            <H3>Your first command</H3>
            <P>
              Create a terminal tab, check the working directory in the context
              row, and run a normal command such as <C>git status</C>. To ask for
              help, switch the input chip to <b className="text-theme-fg/80">Agent</b>,
              describe the outcome you want, and review every proposed command or
              edit before allowing it.
            </P>
            <H3>Signing in</H3>
            <P>
              Dragent signs you in through your <b className="text-theme-fg/80">
              browser</b> — there&apos;s no password to type in the app. Start
              sign-in from onboarding or Settings → Account, complete it in the
              browser, and Dragent finishes automatically. You can also bring your
              own model API keys or use a local model instead.
            </P>
          </>
        ),
      },
      {
        id: "interface",
        label: "The interface",
        title: "The interface",
        body: (
          <>
            <P>
              The window has three areas: a left sidebar, the main terminal in the
              middle, and a right project panel.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Left sidebar</b> — your tabs and
                project intelligence (detected git repo and runnable scripts). The{" "}
                <b className="text-theme-fg/80">+</b> button opens a new tab and lets
                you pick a shell.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Main terminal</b> — the scrolling
                block history, with the command input pinned at the bottom (its
                context row shows your directory, git branch, and change counts).
              </LI>
              <LI>
                <b className="text-theme-fg/80">Right panel</b> — project context: the
                current branch, working-tree changes, and any errors.
              </LI>
            </UL>
            <P>
              Open <b className="text-theme-fg/80">Settings</b> with <Kbd>⌘</Kbd>
              <Kbd>,</Kbd> (Windows: <Kbd>Ctrl</Kbd>
              <Kbd>,</Kbd>).
            </P>
          </>
        ),
      },
    ],
  },
  {
    label: "The terminal",
    sections: [
      {
        id: "blocks",
        label: "Blocks",
        title: "Blocks",
        summary:
          "Every command and its output become a self-contained block you can scan, copy, filter, and act on.",
        body: (
          <>
            <P>
              Instead of an endless wall of text, Dragent groups every command and
              its output into a <b className="text-theme-fg/80">block</b> — bundling
              the command, working directory, status, timing, and output.
            </P>
            <H3>Status & the color stripe</H3>
            <P>
              A block shows a colored stripe: green for success, a distinct color
              for signal exits (e.g. <C>Ctrl+C</C>), and red for other failures. A
              red block is your cue that self-healing can help.
            </P>
            <H3>Block actions</H3>
            <P>
              Open a block&apos;s <b className="text-theme-fg/80">⋯</b> menu (or
              right-click its output) to copy the command, output, or both; scroll
              to it; or open its inline output filter (with a regex toggle) to trim
              long logs without re-running. Click a header to collapse/expand it. A{" "}
              <b className="text-theme-fg/80">sticky header</b> keeps the current
              command visible as you scroll long output.
            </P>
            <H3>Checkpoints</H3>
            <P>
              When you send a message to the agent, Dragent can record an in-memory{" "}
              <b className="text-theme-fg/80">checkpoint</b> of your files, so you can{" "}
              <b className="text-theme-fg/80">restore</b> to that point or revert the
              last agent turn.
            </P>
          </>
        ),
      },
      {
        id: "command-input",
        label: "Command input",
        title: "Command input",
        body: (
          <>
            <P>
              The input bar handles both shell commands and natural-language
              requests, and decides — per keystroke — which you mean, showing a{" "}
              <b className="text-theme-fg/80">shell / agent</b> chip. Force a mode by
              clicking the chip, typing <C>/shell</C> or <C>/agent</C>, or turn
              autodetection off in Settings.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Syntax highlighting</b> and multi-line
                editing (<Kbd>Shift</Kbd>+<Kbd>Enter</Kbd> for a newline; optional
                Vim keys).
              </LI>
              <LI>
                <b className="text-theme-fg/80">Ghost-text autosuggestions</b> from
                history, <C>cd</C> paths, <C>package.json</C> scripts, Makefile
                targets, git branches, and a command database — accept with{" "}
                <Kbd>→</Kbd>.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Path completion</b> with <Kbd>Tab</Kbd>
                , and <C>@</C>-mentions to point the agent at a file.
              </LI>
              <LI>
                <b className="text-theme-fg/80">History</b> — <Kbd>↑</Kbd>/<Kbd>↓</Kbd>{" "}
                open a preview popover (prefix-filtered), and <Kbd>⌘</Kbd>
                <Kbd>R</Kbd> opens fuzzy history search.
              </LI>
            </UL>
            <H3>The &quot;!&quot; scratch shell</H3>
            <P>
              While an AI CLI (like Claude Code) owns the terminal, type <C>!</C>{" "}
              then a command (e.g. <C>!git status</C>) to run it in a persistent,
              per-pane scratch shell shown as a resizable card — with live output,
              its own history, and Stop / Reset controls.
            </P>
          </>
        ),
      },
      {
        id: "tabs-panes",
        label: "Tabs & panes",
        title: "Windows, tabs & panes",
        body: (
          <>
            <P>Organize work across windows, tabs, and split panes.</P>
            <Table
              head={["Action", "Shortcut"]}
              rows={[
                ["New tab", <Kbd key="t">⌘T</Kbd>],
                ["New window", <Kbd key="n">⌘N</Kbd>],
                ["New agent tab", <Kbd key="an">⌘⇧N</Kbd>],
                ["Close tab / pane", <Kbd key="w">⌘W</Kbd>],
                ["Split right (vertical)", <Kbd key="d">⌘D</Kbd>],
                ["Split down (horizontal)", <Kbd key="sd">⌘⇧D</Kbd>],
                [
                  "Previous / next tab",
                  <span key="pn">
                    <Kbd>⌘⇧[</Kbd> / <Kbd>⌘⇧]</Kbd>
                  </span>,
                ],
              ]}
            />
            <P>
              You can drag a sidebar tab onto a pane edge to move that live session
              into a split without spawning a new shell, and pop a pane back out to
              its own tab. <Kbd>⌘</Kbd> is <Kbd>Ctrl</Kbd> on Windows/Linux.
            </P>
          </>
        ),
      },
      {
        id: "shared-sessions",
        label: "Shared sessions",
        title: "Shared sessions",
        summary:
          "Share a live terminal session so teammates can watch — or take control — from the desktop app or a browser.",
        body: (
          <>
            <P>
              Share any tab as a live session and let others follow along in real
              time. Viewers see exactly what you see — the same blocks, themes,
              background, and full-screen apps (like an editor or a running agent)
              — from the Dragent desktop app or straight from a web browser.
            </P>
            <H3>Two roles</H3>
            <UL>
              <LI>
                <b className="text-theme-fg/80">View</b> — watch only. Anyone with the
                link can join; no account required. Viewers are given a friendly
                guest name.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Edit</b> — run commands and type in the
                terminal (including inside full-screen apps). Editing requires
                signing in, and every action is attributed and audited.
              </LI>
            </UL>
            <H3>Start sharing</H3>
            <P>
              Open a tab&apos;s <b className="text-theme-fg/80">⋮</b> menu (in the tab
              bar or the session header) and choose{" "}
              <b className="text-theme-fg/80">Share session</b>. In the share sheet you
              can invite people by email at a chosen role, copy the link, or show a
              QR code. A green <b className="text-theme-fg/80">Live</b> chip appears in
              the session header while you&apos;re hosting.
            </P>
            <H3>Who&apos;s in the session</H3>
            <P>
              Joined participants appear as a row of avatars — in the host&apos;s
              share sheet and in the viewer&apos;s top bar. A viewer can press{" "}
              <b className="text-theme-fg/80">Request edit</b>; the host gets a prompt
              to grant or decline, and once granted the viewer can type
              immediately (no extra sign-in).
            </P>
            <H3>Joining from a browser</H3>
            <P>
              Opening a share link in a browser shows a live viewer with the
              participant bar, a running <b className="text-theme-fg/80">session
              timer</b>, and an <b className="text-theme-fg/80">Open in Dragent</b>{" "}
              button. View links open with no account; edit links prompt you to sign
              in first. When you request edit, you get a notification that the
              request was sent, and a follow-up toast when the host grants or
              declines it — no page reload, and no second sign-in when granted.
            </P>
            <H3>Stop sharing</H3>
            <UL>
              <LI>
                Click the <b className="text-theme-fg/80">Live</b> chip in the session
                header, or choose <b className="text-theme-fg/80">Stop session</b> from
                the tab&apos;s <b className="text-theme-fg/80">⋮</b> menu.
              </LI>
              <LI>
                A session that nobody joins is automatically stopped after 10
                minutes.
              </LI>
            </UL>
            <Note>
              Only the host sees the stop controls. Removing a participant is done
              from the host&apos;s share sheet.
            </Note>
          </>
        ),
      },
      {
        id: "port-forwarding",
        label: "Port forwarding",
        title: "Port forwarding",
        summary:
          "Expose a service running on your machine through a secure, shareable TCP forwarding link.",
        body: (
          <>
            <P>
              Port forwarding is separate from sharing a terminal session. Use it
              when a local web app, API, preview server, or database needs to be
              reached remotely. The process continues to run on your computer;
              Dragent forwards TCP traffic to the local port.
            </P>
            <H3>Forward a port</H3>
            <UL>
              <LI>Start your local service, for example <C>npm run dev</C> on port <C>5173</C>.</LI>
              <LI>Open the tab&apos;s <b className="text-theme-fg/80">Ports</b> bar and choose <b className="text-theme-fg/80">Forward port</b>.</LI>
              <LI>Enter the local port and copy the generated forwarding URL. Each forwarded port appears in its own row.</LI>
              <LI>Open the URL in a browser or send it to someone who needs to view the running service.</LI>
            </UL>
            <H3>How it behaves</H3>
            <P>
              The link is a general TCP tunnel, so HTML, JavaScript modules,
              stylesheets, API requests, WebSockets, and other assets use the same
              forwarding path. The host header and forwarding headers are preserved
              so development servers can identify the original public hostname.
            </P>
            <H3>Security and troubleshooting</H3>
            <UL>
              <LI>Forwarding links are protected by a secret and should be treated like credentials.</LI>
              <LI>Keep the local process running while the port is being used; stopping it makes the link unavailable.</LI>
              <LI>For Vite, add the forwarding hostname to <C>server.allowedHosts</C> if Vite reports that the host is not allowed.</LI>
              <LI>A <C>502</C> usually means the local service is not listening, restarted, or is bound to an unreachable interface; check the local URL first.</LI>
            </UL>
          </>
        ),
      },
    ],
  },
  {
    label: "The AI agent",
    sections: [
      {
        id: "agent",
        label: "The agent & modes",
        title: "The AI agent",
        summary:
          "A first-class coding agent with selectable modes that bound how autonomous it is — and approvals that keep you in control.",
        body: (
          <>
            <P>
              The agent can plan, read and edit files, run commands (with your
              approval), search your codebase, and use skills. It runs in one of
              five <b className="text-theme-fg/80">modes</b>, chosen from the input bar
              or a slash command:
            </P>
            <Table
              head={["Mode", "What it does"]}
              rows={[
                [<b key="a" className="text-theme-fg/80">Agent</b>, "Autonomous multi-step execution (default)."],
                [<b key="p" className="text-theme-fg/80">Plan</b>, "Researches and presents a plan you approve before any changes."],
                [<b key="d" className="text-theme-fg/80">Debug</b>, "Reproduce → instrument → root-cause → fix → verify."],
                [<b key="m" className="text-theme-fg/80">Multitask</b>, "Delegates to parallel sub-agents and synthesizes."],
                [<b key="k" className="text-theme-fg/80">Ask</b>, "Read-only questions and answers; won't modify anything."],
              ]}
            />
            <H3>Approvals</H3>
            <P>
              Anything that changes your system runs through approval. In Settings →
              Agent, choose an approval mode — <b className="text-theme-fg/80">Ask</b>{" "}
              (file changes and commands ask; the default),{" "}
              <b className="text-theme-fg/80">Accept edits</b> (edits run, commands
              ask), or <b className="text-theme-fg/80">Don&apos;t ask</b> (nothing asks,
              except reading files outside your project).
              You can also let read-only commands auto-run, and remember
              &quot;always allow / deny&quot; per tool.
            </P>
            <H3>Budgets & models</H3>
            <P>
              The input bar shows turn count, token usage, and a context-window
              meter; Dragent auto-compacts long sessions so you can keep going.
              Switch models with <C>/model</C>, or add your own provider keys or a
              local (OpenAI-compatible) endpoint in Settings → Models &amp; Keys.
            </P>
          </>
        ),
      },
      {
        id: "self-healing",
        label: "Self-healing",
        title: "Self-healing",
        body: (
          <>
            <P>
              When a command fails, Dragent attaches a <b className="text-theme-fg/80">
              Fix</b> panel to that block. It reads any files referenced in the
              error, diagnoses the failure, and shows an explanation.
            </P>
            <UL>
              <LI>
                If there&apos;s a simple fix, click <b className="text-theme-fg/80">
                Rerun command</b> to run the corrected version.
              </LI>
              <LI>
                For a deeper problem, click <b className="text-theme-fg/80">Fix
                automatically</b> — the agent searches, edits, and re-runs the
                failing command until it passes.
              </LI>
            </UL>
          </>
        ),
      },
      {
        id: "tools",
        label: "Agent tools",
        title: "Agent tools",
        body: (
          <>
            <P>
              The agent works by calling tools, each shown as a labeled block, with
              edits rendered as diff cards. Read-only tools generally run without
              asking; changes and commands follow your approval policy.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Read &amp; inspect</b> — read files,
                list directories, show a tree, get file info, list allowed roots.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Search</b> — <C>grep</C>,{" "}
                <C>search_codebase</C> (hybrid semantic + exact), and a parallel{" "}
                <C>explore</C> sub-agent.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Edit &amp; write</b> — write, edit,
                batch-edit, apply patches, create directories, move files (with a
                guard against blindly overwriting unread files).
              </LI>
              <LI>
                <b className="text-theme-fg/80">Run &amp; orchestrate</b> —{" "}
                <C>run_command</C>, <C>spawn_subagent</C>, task management, a code{" "}
                <C>repl</C>, and <C>skill</C>.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Web</b> — <C>web_search</C> and{" "}
                <C>fetch_url</C> (run on the backend).
              </LI>
              <LI>
                <b className="text-theme-fg/80">Ask you</b> — <C>ask_user</C> presents
                interactive multiple-choice questions.
              </LI>
            </UL>
            <P>
              Connected <b className="text-theme-fg/80">MCP</b> (Model Context
              Protocol) servers add their own tools dynamically.
            </P>
          </>
        ),
      },
      {
        id: "skills",
        label: "Skills",
        title: "Skills",
        body: (
          <>
            <P>
              <b className="text-theme-fg/80">Skills</b> are packaged instructions that
              teach the agent to do a specific task well — reviewing a PR, writing a
              tech spec, scaffolding a component, and more. Settings → Skills shows
              built-in skills, shared skills on this computer, and project skills.
              The agent can choose a matching skill unless it is marked manual-only.
            </P>
            <H3>Add your own</H3>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Shared on this computer</b> — create{" "}
                <C>~/.agents/skills/&lt;name&gt;/SKILL.md</C>. Other tools may also use this folder.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Per project</b> — create{" "}
                <C>.agents/skills/&lt;name&gt;/SKILL.md</C> in the repo. Dragent also reads{" "}
                <C>.dragent/skills/&lt;name&gt;/SKILL.md</C>.
              </LI>
            </UL>
            <P>
              A <C>SKILL.md</C> requires YAML frontmatter with a name and description,
              followed by Markdown instructions. Use Create skill in Settings to write one,
              or View instructions to inspect an existing skill and see its command.
              Project skills override shared and built-in copies with the same name.
            </P>
          </>
        ),
      },
      {
        id: "codebase",
        label: "Codebase intelligence",
        title: "Codebase intelligence",
        body: (
          <>
            <P>
              When you open a project, Dragent builds a <b className="text-theme-fg/80">
              local</b> semantic index so the agent can find relevant code
              quickly. Embeddings are computed on your machine by default, and the
              index is a pure local cache.
            </P>
            <UL>
              <LI>
                Control it in Settings → Code &amp; Index: enable/disable,
                auto-index on changes, respect <C>.gitignore</C>, and ignore
                patterns (paths that are <b className="text-theme-fg/80">never indexed
                or sent to the model</b>).
              </LI>
              <LI>
                The <C>search_codebase</C> tool blends semantic and exact matches,
                falling back to text search when the index isn&apos;t ready.
              </LI>
              <LI>
                Git context (branch, changes vs <C>HEAD</C>) is surfaced in the UI
                and given to the agent.
              </LI>
            </UL>
          </>
        ),
      },
      {
        id: "attachments-voice",
        label: "Attachments & voice",
        title: "Attachments & voice",
        body: (
          <>
            <P>
              Attach <b className="text-theme-fg/80">images and files</b> with the{" "}
              <b className="text-theme-fg/80">+</b> button or by dragging them in.
              Images (png/jpg/webp/gif) are sent as multimodal content so the agent
              can see them; text files are inlined.
            </P>
            <P>
              <b className="text-theme-fg/80">Voice input</b> — click the microphone
              chip and speak. Voice never uses your AI budget: macOS can use a cloud
              engine on paid tiers and falls back to on-device; Windows uses
              built-in dictation.
            </P>
          </>
        ),
      },
    ],
  },
  {
    label: "Customize",
    sections: [
      {
        id: "workflows",
        label: "Workflows",
        title: "Workflows",
        body: (
          <>
            <P>
              <b className="text-theme-fg/80">Workflows</b> are saved, parameterized
              command templates. Open the command palette with <Kbd>⌘⇧P</Kbd>, search, pick one, and press{" "}
              <Kbd>Tab</Kbd> to fill each <C>{"{{argument}}"}</C> before running.
            </P>
            <P>
              Dragent bundles a large catalog (git, docker, kubernetes, aws, npm,
              and more). Add your own to <C>~/.config/dragent/workflows/</C> or a
              project&apos;s <C>./.dragent/workflows/</C>.
            </P>
          </>
        ),
      },
      {
        id: "themes",
        label: "Themes & appearance",
        title: "Themes & appearance",
        body: (
          <>
            <P>
              Dragent ships a large catalog of themes, including{" "}
              <b className="text-theme-fg/80">background-image</b> themes. Choose one in
              Settings → Appearance — search the full catalog, or browse the
              featured set and image themes, each shown as a live preview card.
            </P>
            <UL>
              <LI>
                Image themes keep the picture visible everywhere while popovers,
                menus, and cards stay solid and readable; each theme&apos;s designed
                opacity is respected.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Custom themes</b> — drop a <C>.yaml</C>{" "}
                into <C>~/.config/dragent/themes/</C> (same format as bundled; a
                matching name overrides a bundled theme).
              </LI>
              <LI>
                <b className="text-theme-fg/80">Fonts</b> — pick a family (bundled Fira
                Code + system monospace), size, and line height; typography scales
                across the whole app together.
              </LI>
            </UL>
          </>
        ),
      },
      {
        id: "settings",
        label: "Settings",
        title: "Settings",
        body: (
          <>
            <P>
              Open Settings with <Kbd>⌘</Kbd>
              <Kbd>,</Kbd>. Sections:
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Account</b> — sign in, plan, usage,
                billing.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Agent</b> — approval mode, auto-run safe
                commands, max steps, self-healing, context scope;{" "}
                <b className="text-theme-fg/80">Models &amp; Keys</b>; and{" "}
                <b className="text-theme-fg/80">MCP servers</b>.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Workspace</b> — Appearance, Terminal &amp;
                Input (shell, fonts, cursor, Vim keys, autodetection), Code &amp;
                Index, Keyboard Shortcuts.
              </LI>
              <LI>
                <b className="text-theme-fg/80">System</b> — Privacy and About.
              </LI>
            </UL>
          </>
        ),
      },
    ],
  },
  {
    label: "Account & platform",
    sections: [
      {
        id: "models-keys",
        label: "Models & API keys",
        title: "Models & API keys",
        summary:
          "Choose Dragent-hosted models, connect your own provider key, or use a local OpenAI-compatible endpoint.",
        body: (
          <>
            <P>
              Open Settings → <b className="text-theme-fg/80">Models &amp; Keys</b>
              to choose how requests are routed. The <b className="text-theme-fg/80">Auto</b>
              option hides the current platform default so Dragent can change its
              default provider without changing your workflow.
            </P>
            <H3>Using a Dragent-hosted model</H3>
            <P>
              Sign in and select an available model. Your plan allowance, free
              allowance, and credit settings determine which hosted models are
              available. The selector can lock to the free-tier model when premium
              funds are exhausted.
            </P>
            <H3>Bring your own key</H3>
            <UL>
              <LI>Paste the provider key into the matching provider field and save it; pasted keys are validated when you leave the field.</LI>
              <LI>Only models supported by that provider key are shown in the selector.</LI>
              <LI>Requests served by your key go directly through that provider and are not charged as Dragent platform usage.</LI>
              <LI>Provider web tools also try your provider key first. Platform fallback is only used when you enable fallback and billing authorizes it.</LI>
            </UL>
            <H3>Fallback and local models</H3>
            <P>
              Fallback is an explicit setting. Turn it off when you want a failed
              BYOK request to stop rather than use a platform key. A local or custom
              OpenAI-compatible endpoint keeps inference on the endpoint you
              configure; its key is never forwarded to unrelated providers.
            </P>
            <Note>Never paste a provider key into a prompt or commit it to a project. Remove and rotate a key from the provider dashboard if you suspect it was exposed.</Note>
          </>
        ),
      },
      {
        id: "billing",
        label: "Account & billing",
        title: "Account & billing",
        body: (
          <>
            <P>
              Dragent uses a <b className="text-theme-fg/80">credit</b> model for AI
              usage (1 credit = 1¢). Everyone gets a monthly{" "}
              <b className="text-theme-fg/80">free allowance</b>; paid plans add a plan
              allowance, and you can buy credits (which never expire).
            </P>
            <UL>
              <LI>
                Funds are spent <b className="text-theme-fg/80">plan → free →
                credits</b>. A &quot;Use my credits&quot; setting controls whether
                credits are billed after allowances run out.
              </LI>
              <LI>
                When your premium funds are exhausted, the model selector locks to
                the free model until your allowance resets or you enable/buy
                credits. You can always use your own provider key instead.
              </LI>
              <LI>
                Billing (subscription, payment, buying credits) is managed in the
                web dashboard.
              </LI>
            </UL>
            <H3>Buy credits</H3>
            <P>
              Open the billing page from Settings → Account or the dashboard and
              choose <b className="text-theme-fg/80">Buy credits</b>. Select a
              credit pack, complete checkout, and return to Dragent; the balance is
              refreshed automatically. Credits do not expire and can be used after
              your plan or free allowance has been consumed.
            </P>
            <H3>Subscriptions and usage</H3>
            <UL>
              <LI>Plans include a monthly usage allowance and can be changed or cancelled from the web dashboard.</LI>
              <LI>Usage is reserved before a provider call and settled from the actual tokens returned; unused reservations are released.</LI>
              <LI>Each agent task and web-tool operation is tracked independently so a retry is not charged twice.</LI>
              <LI>Bring-your-own-key requests do not consume Dragent credits, unless an explicitly permitted platform fallback is used.</LI>
            </UL>
            <Note>
              Team plans and cloud sync of settings/secrets are on the roadmap
              and not yet available. Settings stay on your machine.
            </Note>
          </>
        ),
      },
      {
        id: "privacy",
        label: "Privacy & security",
        title: "Privacy & security",
        body: (
          <>
            <P>
              Dragent&apos;s core principle: <b className="text-theme-fg/80">your
              machine executes, the server orchestrates.</b> Commands run locally,
              the client has final say, and changes require approval.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Stays local</b> — terminal history, the
                codebase index and embeddings, credentials, agent memory, and your
                own provider keys.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Leaves only when you use AI</b> —
                self-healing sends the failing command, directory, truncated output,
                and exit code; agent tasks send your request plus needed context.
                There&apos;s no background telemetry of your commands.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Never sent</b> — anything matched by{" "}
                <C>.gitignore</C>, a repo <C>.dragentignore</C>, or your ignore
                patterns.
              </LI>
            </UL>
            <P>
              For maximum control, use your own provider keys or a local model, and
              set provider fallback to off so platform keys are never used.
            </P>
          </>
        ),
      },
    ],
  },
  {
    label: "Reference",
    sections: [
      {
        id: "shortcuts",
        label: "Keyboard shortcuts",
        title: "Keyboard shortcuts",
        body: (
          <>
            <P>
              <Kbd>⌘</Kbd> (Cmd) on macOS corresponds to <Kbd>Ctrl</Kbd> on
              Windows/Linux.
            </P>
            <Table
              head={["Action", "Shortcut"]}
              rows={[
                ["New tab / window / agent tab", <span key="1"><Kbd>⌘T</Kbd> / <Kbd>⌘N</Kbd> / <Kbd>⌘⇧N</Kbd></span>],
                ["Close tab / pane", <Kbd key="2">⌘W</Kbd>],
                ["Split right / down", <span key="3"><Kbd>⌘D</Kbd> / <Kbd>⌘⇧D</Kbd></span>],
                ["Previous / next tab", <span key="4"><Kbd>⌘⇧[</Kbd> / <Kbd>⌘⇧]</Kbd></span>],
                ["Settings", <Kbd key="5">⌘,</Kbd>],
                ["Copy / paste", <span key="6"><Kbd>⌘C</Kbd> (or <Kbd>Ctrl⇧C</Kbd>) / <Kbd>⌘V</Kbd></span>],
                ["Interrupt (SIGINT)", <Kbd key="7">Ctrl+C</Kbd>],
                ["Previous / next command", <span key="8"><Kbd>↑</Kbd> / <Kbd>↓</Kbd></span>],
                ["Accept autosuggestion", <Kbd key="9">→</Kbd>],
                ["Autocomplete path / command", <Kbd key="10">Tab</Kbd>],
                ["History search", <Kbd key="11">⌘R</Kbd>],
                ["Command palette / workflows", <Kbd key="12">⌘⇧P</Kbd>],
                ["Cycle input type (shell / agent)", <Kbd key="13">⌘⇧I</Kbd>],
                ["Send / newline (agent)", <span key="14"><Kbd>Enter</Kbd> / <Kbd>⇧Enter</Kbd></span>],
                ["Agent input — !cmd history", <span key="16"><Kbd>⇧↑</Kbd> / <Kbd>⇧↓</Kbd></span>],
                ["Agent input — accept suggestion", <span key="17"><Kbd>Tab</Kbd> / <Kbd>→</Kbd></span>],
                ["Toggle rich multi-line input", <Kbd key="15">Ctrl+G</Kbd>],
              ]}
            />
            <Note>
              <Kbd>Ctrl+C</Kbd> interrupts the running command, so copy is{" "}
              <Kbd>⌘C</Kbd> (macOS) or <Kbd>Ctrl⇧C</Kbd> (Windows/Linux).
            </Note>
          </>
        ),
      },
      {
        id: "platforms",
        label: "Platform notes",
        title: "Platform notes",
        body: (
          <>
            <P>
              Dragent runs on <b className="text-theme-fg/80">macOS</b> and{" "}
              <b className="text-theme-fg/80">Windows</b>. zsh, bash, and Windows
              PowerShell get full shell integration (blocks, exit codes, directory
              tracking); other shells run without it. Updates are delivered over
              HTTPS and applied on the next restart.
            </P>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Windows</b> — defaults to PowerShell
                (with PowerShell 7 and Git Bash in the &quot;+&quot; picker), draws
                its own titlebar, and settles a block&apos;s output a moment after
                completion (a ConPTY correctness mechanism, not a hang).
              </LI>
              <LI>
                <b className="text-theme-fg/80">Linux</b> is not an officially supported
                target in this release.
              </LI>
            </UL>
          </>
        ),
      },
      {
        id: "troubleshooting",
        label: "Troubleshooting",
        title: "Troubleshooting",
        body: (
          <>
            <UL>
              <LI>
                <b className="text-theme-fg/80">Commands don&apos;t become blocks</b> —
                use a shell with integration (zsh, bash, PowerShell); set it in
                Settings → Terminal &amp; Input.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Ctrl+C doesn&apos;t copy</b> — by design;
                copy is <Kbd>⌘C</Kbd> / <Kbd>Ctrl⇧C</Kbd>.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Model selector is locked</b> — your
                premium funds are exhausted; enable/buy credits, upgrade, wait for
                reset, or use your own key.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Agent won&apos;t edit / run</b> — check
                the mode (Ask is read-only, Plan waits for approval) and your
                approval settings.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Hard-to-read image theme</b> — pick a
                theme whose text suits the image, or a solid theme.
              </LI>
              <LI>
                <b className="text-theme-fg/80">Microphone silent</b> — grant OS
                microphone permission (macOS: System Settings → Privacy &amp;
                Security → Microphone).
              </LI>
            </UL>
          </>
        ),
      },
      {
        id: "faq",
        label: "FAQ",
        title: "Frequently asked questions",
        body: (
          <>
            <H3>Which platforms are supported?</H3>
            <P>macOS and Windows. Linux isn&apos;t an official target yet.</P>
            <H3>Do I need an account to use the AI?</H3>
            <P>
              Sign in to use the Dragent platform&apos;s models, or bring your own
              provider key or a local model.
            </P>
            <H3>Will the agent change files without asking?</H3>
            <P>
              Not unless you allow it — every change and command runs through
              approval, and Ask mode is read-only.
            </P>
            <H3>Where is my code indexed?</H3>
            <P>Locally — the index and embeddings are computed and stored on your machine.</P>
            <H3>How much does it cost?</H3>
            <P>
              Everyone gets a monthly free allowance; paid plans add more, and
              credits (1 credit = 1¢) never expire.
            </P>
            <H3>Is voice a paid feature?</H3>
            <P>No — voice never uses your AI budget.</P>
          </>
        ),
      },
      {
        id: "licenses",
        label: "Licenses",
        title: "Licenses & attributions",
        body: (
          <>
            <P>
              Dragent&apos;s own code is open-source software licensed under
              the GNU Affero General Public License, version 3 only. It incorporates some third-party
              open-source components under their own licenses, which it credits and
              whose notice requirements it honors.
            </P>
            <Table
              head={["Component", "License", "Used for"]}
              rows={[
                [
                  <span key="w">
                    WarpUI (<C>warpui</C>, <C>warpui_core</C>)
                  </span>,
                  "MIT",
                  "The UI framework Dragent is built on",
                ],
                ["Alacritty terminal library", "Apache-2.0", "Portions of the terminal core"],
                ["Warp Themes", "Apache-2.0", "Bundled theme catalog"],
                ["Warp Workflows (commands.dev)", "Apache-2.0", "Bundled workflow catalog"],
                ["Fira Code", "SIL OFL 1.1", "Bundled monospace fonts"],
                ["Lucide", "ISC", "UI icons"],
                ["Windows Terminal assets", "MIT", "Windows graphics/shaders"],
              ]}
            />
            <P>
              These third-party licenses apply only to their respective components.
              Dragent&apos;s own code is licensed under AGPL-3.0-only. Product logos shown in the app
              are trademarks of their owners, used only to identify those products.
            </P>
            <P>
              For the complete license notices and attributions, see the{" "}
              <a
                href="/licenses"
                className="text-theme-fg/85 underline decoration-theme-fg/25 underline-offset-2 hover:decoration-theme-fg/70"
              >
                Open-Source Licenses &amp; Notices
              </a>{" "}
              page.
            </P>
          </>
        ),
      },
    ],
  },
];
