import Image from "next/image";
import Link from "next/link";
import { P, H3, UL, LI, C, Note, Table } from "./primitives";
import { CodeBlock } from "./CodeBlock";
import type { DocSection } from "./content";

function Figure({ name, alt }: { name: string; alt: string }) {
  return (
    <figure className="doc-figure">
      <a
        href={`/screens/${name}.webp`}
        target="_blank"
        rel="noopener noreferrer"
        aria-label={`Open full-size image: ${alt}`}
      >
        <Image
          src={`/screens/${name}.webp`}
          alt={alt}
          width={1800}
          height={1013}
          sizes="(max-width: 900px) 100vw, 760px"
        />
      </a>
      <figcaption>
        Dragent interface with sample data. Select the image to see it in full.
      </figcaption>
    </figure>
  );
}
export const GUIDES: DocSection[] = [
  {
    id: "quickstart",
    label: "Your first workspace",
    title: "Your first workspace",
    summary:
      "From a repository to a focused agent session and a reviewed change.",
    body: (
      <>
        <Note>
          You need Dragent, Git, a repository, and an installed CLI agent if you
          want to use one. Provider sign-in and your Dragent account are
          separate.
        </Note>
        <H3>1. Open a project</H3>
        <P>
          Launch Dragent, sign in, and select the organisation you want to work
          in. Add your existing repository from Projects. Choose its root
          folder, rather than a nested source directory, so branch and worktree
          operations use the right repository.
        </P>
        <H3>2. Give the task a workspace</H3>
        <P>
          Select <b>New Workspace</b>, choose the project, and describe the
          task. For a separate change, create a new branch and worktree from the
          base branch you want to use. To continue existing work, select its
          branch or use the current checkout.
        </P>
        <Figure name="workspace" alt="Workspace creation in Dragent" />
        <H3>3. Choose one agent to start</H3>
        <P>
          Select an installed agent such as Claude Code or Codex, or the
          built-in Dragent agent. Select a model where the agent supports model
          selection. Start with one agent; coordination is optional.
        </P>
        <P>
          Give the agent a concrete request, a way to verify it, and any
          boundaries. For example: “Fix the expired invitation flow. Add a
          regression test. Do not commit or push.”
        </P>
        <H3>4. Follow the work</H3>
        <P>
          Open the agent’s pane to see its conversation or terminal. Check its
          status and respond when it asks a question or requests permission. The
          built-in agent captures its command output in the conversation instead
          of borrowing your visible shell.
        </P>
        <H3>5. Review before shipping</H3>
        <P>
          Open Changes to inspect the files and diff. Run the relevant checks,
          review the result, and then commit or push when you are ready. Use{" "}
          <Link href="/docs/code-review">Code reviews</Link> for hosted pull
          requests and merge requests.
        </P>
        <P>
          Next, try <Link href="/docs/coordination">coordinating a team</Link>{" "}
          or <Link href="/docs/remote-ssh">opening a remote project</Link>.
        </P>
      </>
    ),
  },
  {
    id: "concepts",
    label: "The Dragent model",
    title: "The Dragent model",
    summary:
      "Organisation, project, workspace, and session: the concepts behind the interface.",
    body: (
      <>
        <H3>The hierarchy</H3>
        <Table
          head={["Concept", "What it represents"]}
          rows={[
            [
              "Organisation",
              "The account context for projects, workspaces, connected services, and billing.",
            ],
            ["Project", "A repository on a particular machine."],
            [
              "Workspace",
              "A task’s working directory, branch, panes, and conversations.",
            ],
            [
              "Worktree",
              "A separate checkout linked to a Git repository. It lets another branch stay checked out without switching your current directory.",
            ],
            [
              "Agent session",
              "A conversation with a coding agent, including its selected provider and account context.",
            ],
            [
              "Terminal",
              "An interactive shell or CLI process running on the workspace’s host.",
            ],
          ]}
        />
        <H3>Independent tasks or a coordinated team</H3>
        <P>
          Separate workspaces are useful for independent changes. Coordination
          adds a lead, tasks, dependencies, workers, and a mailbox when work
          needs to be connected. Running several agents does not mean you have
          to coordinate them.
        </P>
        <H3>Local and remote are execution locations</H3>
        <P>
          A local workspace runs on your desktop. An SSH workspace runs on its
          remote host. Agent CLIs and tools must be available on the machine
          that executes them. A connected-service login can be a different thing
          from an agent provider’s login.
        </P>
        <H3>Git remains the source of truth</H3>
        <P>
          Branches, commits, and worktrees are ordinary Git objects. Check the
          diff and your working tree before deleting a workspace or changing a
          branch. An agent conversation is context for the work, not a
          replacement for version control.
        </P>
      </>
    ),
  },
  {
    id: "workspaces",
    label: "Workspaces & branches",
    title: "Workspaces and branches",
    summary:
      "Give each piece of work a directory, branch, and set of sessions.",
    body: (
      <>
        <H3>Choose a starting point</H3>
        <Table
          head={["Starting point", "Use it when"]}
          rows={[
            [
              "New branch and worktree",
              "You want an isolated checkout for a new task.",
            ],
            [
              "Existing branch",
              "You want to continue or review a branch that already exists.",
            ],
            [
              "Current checkout",
              "You intentionally want to work in the project’s existing directory.",
            ],
          ]}
        />
        <P>
          Choose the base branch before starting a new worktree. When another
          worktree already has an existing branch checked out, reuse that
          checkout rather than attempting to check out the same branch twice.
        </P>
        <H3>Start without coordination</H3>
        <P>
          Choose one agent and enter the task. The workspace keeps its branch,
          terminal panes, and conversation together. Add another pane when you
          want a shell, an editor, or another agent beside the work.
        </P>
        <Figure
          name="workspace"
          alt="Choosing a workspace, branch, and agent"
        />
        <H3>Keep parallel changes separate</H3>
        <P>
          Use a separate worktree for each independent change. Worktrees share
          the repository’s Git object storage, but files on disk are separate.
          Dependencies and environment files may need to be installed or
          configured for each checkout.
        </P>
        <H3>Close or delete deliberately</H3>
        <P>
          Review running agents, uncommitted files, and unpushed commits before
          confirming deletion. Removing a sidebar folder or workspace is a
          different decision from deleting the original repository. Read the
          confirmation and preserve work you still need.
        </P>
      </>
    ),
  },
  {
    id: "agents",
    label: "Choose a coding agent",
    title: "Choose a coding agent",
    summary:
      "Use an installed CLI agent, or work with the built-in Dragent agent.",
    body: (
      <>
        <H3>Use your existing CLI</H3>
        <P>
          Dragent can launch 39 registered CLI agents in a workspace,
          including Claude Code, Codex, OpenCode, Cursor, Gemini CLI, GitHub
          Copilot, Amp, Qwen Code, and the built-in Dragent agent — or add
          your own custom CLI. Install and authenticate the agent on the
          machine where the workspace runs.
        </P>
        <P>
          Agent features vary. Model selection, status reporting, conversation
          recovery, and isolated account profiles depend on the provider’s
          adapter. A CLI being runnable does not mean every integration is
          supported.
        </P>
        <H3>Choose the model</H3>
        <P>
          Use the model picker for the selected agent when it is available.
          Agent and model selection are separate: choosing Claude Code, for
          example, chooses its harness, while the model picker controls the
          model options that harness exposes.
        </P>
        <H3>Built-in Dragent</H3>
        <P>
          The built-in agent uses Dragent’s conversation interface, permissions,
          tools, and model configuration. External CLI agents keep their own
          authentication and provider billing. See{" "}
          <Link href="/docs/models-keys">Models and API keys</Link> for the
          built-in agent.
        </P>
        <H3>Choose the right account</H3>
        <P>
          Use the system login by default. If your provider supports isolated
          profiles, select an account in Settings → Agent Accounts and bind it
          to a project. See{" "}
          <Link href="/docs/agent-accounts">Account profiles</Link> for the
          supported scope and limitations.
        </P>
      </>
    ),
  },
  {
    id: "agent-sleep",
    label: "Idle agents and memory",
    title: "Idle agents and memory",
    summary:
      "Idle agents sleep to free memory and wake when you open their tab.",
    body: (
      <>
        <P>
          Each CLI agent runs as its own process, and many use close to a
          gigabyte of memory even when they are not working. With a dozen
          agent tabs open, that can use up the memory of a laptop. Dragent
          puts idle agents to sleep so your computer stays responsive.
        </P>
        <H3>What sleeping means</H3>
        <P>
          An agent sleeps once it has finished its turn and nobody has used it
          for a while. Its process is stopped, but its tab, its terminal
          history and its conversation stay. The tab is labelled{" "}
          <C>Sleeping</C>.
        </P>
        <P>
          Open the tab to wake the agent. You see <C>Waking …</C> while it
          restarts, and it continues the same conversation. Opening the tab
          from the mobile app wakes it too: the phone shows “Waking the agent
          on your desktop…” and connects once the agent is ready.
        </P>
        <H3>Agents that stay awake</H3>
        <P>Dragent never puts an agent to sleep while it:</P>
        <UL>
          <LI>is working, or is waiting for your answer or approval;</LI>
          <LI>is on screen in any pane;</LI>
          <LI>is in use from the mobile app;</LI>
          <LI>
            is still running something it started that accepts connections,
            such as a development server;
          </LI>
          <LI>
            is a worker in an active{" "}
            <Link href="/docs/coordination">Agent Coordination</Link> team;
          </LI>
          <LI>
            can’t resume its conversation exactly. Only agents whose
            conversation Dragent can resume are put to sleep.
          </LI>
        </UL>
        <P>
          The built-in Dragent agent never sleeps. It does not run as a separate
          process.
        </P>
        <H3>Choose when agents sleep</H3>
        <P>
          Go to Settings → Agent → Behavior and, under Memory, set{" "}
          <C>Put idle agents to sleep after</C> to 15 minutes (the default),
          30 minutes, 1 hour, or Never.
        </P>
        <Table
          head={["Memory on your computer", "What Dragent does"]}
          rows={[
            ["Normal", "Idle agents sleep after the time you chose."],
            [
              "Running low",
              "Idle agents sleep after 2 minutes, and a notice tells you how many were put to sleep and how much memory that freed.",
            ],
            [
              "Critically low",
              "Before you start another agent, Dragent asks: put idle agents to sleep first, start anyway, or cancel.",
            ],
          ]}
        />
        <Note>
          With Never selected, agents only sleep when you choose “Sleep idle
          agents” in the low-memory prompt.
        </Note>
        <H3>If your computer still runs out of memory</H3>
        <P>
          Agents that stay awake still use memory. If your computer slows down
          or restarts unexpectedly, close agent tabs you no longer need, or
          finish their work so they can sleep. On macOS, Activity Monitor’s
          Memory tab shows which processes use the most memory.
        </P>
      </>
    ),
  },
  {
    id: "coordination",
    label: "Agent coordination",
    title: "Coordinate a team of agents",
    summary:
      "Give one lead the goal, divide the work, and keep dependencies and decisions visible.",
    body: (
      <>
        <H3>When to use a team</H3>
        <P>
          Use a team when parts of a task depend on each other: an interface
          before the code that uses it, implementation before review, or
          several changes before one round of testing. One lead plans the work
          as tasks, workers carry them out, and you follow the plan in one
          place. Use independent workspaces when tasks do not need a shared
          plan.
        </P>
        <H3>Start a team</H3>
        <P>
          In workspace creation, turn on coordination, choose the lead agent,
          and select the additional agents that may contribute. The lead is
          already part of the team. Describe the outcome you want and how the
          lead should verify it.
        </P>
        <P>
          From the command line, the agent that runs the create command leads
          the team:
        </P>
        <CodeBlock
          code={
            'dragent team create "Shorten invite links" --agents claude,codex\ndragent tasks add "Shorten the links" --prompt "…" --owns app/src/invites/\ndragent tasks add "Review the invite change" --review t1 --prompt "…"\ndragent tasks add "QA the whole feature" --qa --prompt "…"\ndragent tasks start'
          }
        />
        <P>
          Read <Link href="/docs/cli">the CLI reference</Link> for the full
          command list, and run <C>dragent team help</C> for the current
          options.
        </P>
        <Figure
          name="coordination"
          alt="The Agent Coordination plan, task statuses, and reply-to-lead panel"
        />
        <H3>Roles on the team</H3>
        <P>
          The lead plans the work as tasks, gives each task to a worker, talks
          to the workers through the team mailbox, checks every result, and
          brings finished work into your checkout. The lead does not write the
          code itself. Workers each write code on their own branch. A reviewer
          independently reviews one task&apos;s result and approves it or asks
          for changes. A QA pass tests the whole finished feature the way a
          tester would: it starts the app, walks through every flow you asked
          for plus the edge cases, checks nothing nearby broke, and runs the
          full test suite. The work is not done until QA passes.
        </P>
        <P>
          Reviews and QA passes are tasks too. Add a review with{" "}
          <C>--review t1</C>. Make the QA pass the plan&apos;s final task with{" "}
          <C>--qa</C>. When tasks run side by side, test them together once
          they are all integrated, with <C>--qa --integrated</C>: QA then gets
          a snapshot of your checkout with everything in it.
        </P>
        <H3>How work flows</H3>
        <P>
          Each worker starts from a snapshot of your checkout, including your
          uncommitted changes, on its own branch. Follow-on work carries on in
          the folder of the work it continues. Other work gets a checkout of
          its own. Every checkout shows in the sidebar.
        </P>
        <P>
          Nothing reaches your checkout until the lead brings it in. The lead
          checks what each worker changed, reruns its checks, and integrates
          finished work one task at a time, in plan order, as uncommitted
          changes. Nothing is committed, merged, staged, or pushed
          automatically. A worker&apos;s summary is a claim, not evidence: the
          lead treats it that way.
        </P>
        <H3>Find a coordination in the sidebar</H3>
        <P>
          Each coordination gets its own group under its project, named after
          the request, with how many of its tasks are done. The lead’s
          workspace comes first, marked with a crown, then one card per task
          workspace titled with the task it works on. Under the title are the
          logos of the agents working there, the branch, and its changes; a
          workspace that handled several tasks lists the others below. Click a
          card to open its workspace, or an agent’s logo to open that agent’s
          tab. Click the group’s title to fold it, and use the arrow beside it
          to open the coordination’s page.
        </P>
        <H3>Task statuses</H3>
        <P>
          Each task shows one status. The chip beside it uses the words below.
        </P>
        <Table
          head={["Status", "What it means", "What you do"]}
          rows={[
            [
              "Pending",
              "Planned. It waits for other tasks (the chip names them, as “waits for …”), or it is ready to start.",
              "Nothing. It starts on its own once what it waits for is done.",
            ],
            [
              "Starting",
              "Its worker is being set up: checkout and agent.",
              "Wait.",
            ],
            [
              "Running",
              "A worker is on it. The chip adds detail: “working”, “running”, “idle”, or “waiting for your approval”.",
              "Let it work. Answer if it asks you something.",
            ],
            [
              "Blocked",
              "Its worker needs a decision, access, or another task first. The reason is shown.",
              "Read the reason and answer it, or fix what is missing.",
            ],
            [
              "Done",
              "Its worker finished. Read its stage and verdict before treating it as complete.",
              "Check the result. The lead verifies and integrates it.",
            ],
            [
              "Failed",
              "A real failure: its worker didn't start, its terminal closed, or its agent stopped before finishing. The reason says which. Its checkout keeps the work.",
              "Read its screen, then send it back to start again, or change the plan.",
            ],
            [
              "Needs decision",
              "Not an error. Its review still asked for changes after three rounds, so it stopped to let someone choose what happens next. The reason lists the open findings, and the work is kept on its branch.",
              "Decide: send it back once more, change the plan, or answer the choices the findings depend on. The lead asks you when they are yours to make.",
            ],
            [
              "Cancelled",
              "Taken out of the plan. Its worker stopped.",
              "Nothing.",
            ],
          ]}
        />
        <H3>Stages of finished work</H3>
        <P>
          A finished work task also shows how far it got towards your
          checkout. Each stage builds on the one before it.
        </P>
        <Table
          head={["Stage", "What it means", "What you do"]}
          rows={[
            [
              "Done",
              "Finished, with no review yet.",
              "Wait for its review, or review it yourself.",
            ],
            [
              "Approved",
              "Its review approved it.",
              "Wait for QA and integration.",
            ],
            [
              "Passed QA",
              "The QA pass tested the feature and approved it.",
              "Wait for integration.",
            ],
            [
              "In your checkout",
              "Integrated as uncommitted changes in your checkout.",
              "Read the diff, run your checks, then commit when you are ready.",
            ],
            [
              "Asked for changes",
              "Its review finished but did NOT approve: the work went back to its worker with the findings. QA shows “found problems” instead.",
              "Nothing. The worker tries again and the review looks at the next attempt.",
            ],
          ]}
        />
        <H3>Reviews and “changes asked”</H3>
        <P>
          This is the combination that confuses people: a review task can show{" "}
          <C>done</C> together with <C>asked for changes</C>. That means the
          reviewer finished, but the reviewed work is NOT approved. The work
          goes back to its worker with the findings, and nothing that builds
          on it starts meanwhile. When the worker finishes again, the review
          runs again on the new attempt. A review waiting for the next attempt
          shows as pending with “looks again after the fix”.
        </P>
        <P>
          After three rounds of “asked for changes”, the reviewed task shows{" "}
          <C>needs decision</C> instead. That is not an error: nothing broke,
          and its work is kept. The reason lists what the review still asks
          for, and the lead decides: send it back once more, change the plan,
          or ask you when the findings hang on your choices. QA of
          the integrated work behaves the same way, except it reports to the
          lead rather than to one worker: the lead sends the responsible work
          back, integrates the fix, and QA tests again on its own.
        </P>
        <H3>Messages and talking to the lead</H3>
        <P>
          Use the reply-to-lead area on the team&apos;s page to answer a
          question or add direction. Your message reaches the lead immediately,
          even mid-turn, and the lead answers there because you are reading
          that page. Workers settle what they share directly — an interface
          shape, a schema — and the lead sees those exchanges. You can inspect
          a worker&apos;s pane when you need the full conversation.
        </P>
        <P>
          Every message has a kind, shown as a verb: The lead or a worker{" "}
          <C>wrote to</C>, <C>asked</C>, <C>answered</C>, <C>handed off to</C>,
          or <C>told</C> someone, and a message that <C>needs you</C> is
          waiting for your decision. From the command line:
        </P>
        <CodeBlock
          code={
            'dragent msg send lead "Keep the existing public API unchanged."\ndragent msg inbox\ndragent msg reply 12 "Use the shorter expiry."'
          }
        />
        <H3>Permission prompts</H3>
        <P>
          A worker that waits at a permission prompt shows “waiting for your
          approval”. The lead may answer it inside the task when it is safe:
          reading or writing in the task&apos;s own checkout, running the
          project&apos;s build, tests, or tools, or starting its own
          development server. Each answer is recorded on the timeline. The
          lead asks you — with a message that needs you — before anything
          destructive or irreversible: deleting or overwriting outside the
          checkout, credentials, pushing, publishing, paying, writing to
          outside services, or anything it is unsure of. You can also turn off
          lead-answered prompts for the team; then every prompt waits for you.
        </P>
        <H3>Changing the plan</H3>
        <P>
          Plans change in place. Workers keep what they have done. Adding
          agents, editing tasks that have not started, adding new tasks, or
          moving a task to another agent all keep the team going; only your
          word adds an agent.
        </P>
        <CodeBlock
          code={
            "dragent team agents add codex\ndragent tasks edit t2 --agent codex\ndragent tasks reassign t3 --agent claude --note \"Needs the stronger model.\"\ndragent tasks cancel t4"
          }
        />
        <P>
          Reassigning moves a task under way to another agent, which carries
          on in the same checkout with the work so far. Cancelling is only
          for work nobody needs any more. Never stop the team or start a new
          one just to change the plan.
        </P>
        <H3>Remote machines</H3>
        <P>
          On a supported SSH host, the matching headless runtime provides
          coordination operations. Install and authenticate the required agent
          CLIs on that host. Use a supported CLI lead remotely; the built-in
          Dragent lead currently remains local. Connected-service tools depend
          on the signed-in desktop bridge.
        </P>
        <H3>Good habits</H3>
        <P>
          Follow the plan, but verify before you trust it. Read each
          task&apos;s diff and its check results before treating it as done.
          Check the summary of the whole run once everything is integrated and
          QA has passed. A finished task is one part; the feature is done when
          every task is integrated and tested together.
        </P>
        <Note>
          A team&apos;s progress view helps you follow work. Read the changes
          and verification results before merging; a successful agent launch
          is not proof that the task is correct.
        </Note>
      </>
    ),
  },
  {
    id: "automations",
    label: "Automations",
    title: "Make agent work recurring",
    summary:
      "Define the prompt, schedule, agent, and workspace for repeated tasks.",
    body: (
      <>
        <H3>Create an automation</H3>
        <P>
          Open Automations. Give the automation a descriptive name, select where
          it works, set its schedule or supported trigger, choose the agent, and
          write the prompt. Include a definition of success and state whether
          the agent may edit files or publish anything.
        </P>
        <Figure
          name="automations"
          alt="Automation settings and run history in Dragent"
        />
        <H3>Choose where each run works</H3>
        <Table
          head={["Mode", "Behaviour"]}
          rows={[
            [
              "New worktree",
              "Each run starts a new branch and linked worktree. Choose a base branch and whether the new branch should be pushed.",
            ],
            [
              "Existing branch",
              "Run on the chosen branch, reusing a worktree already on that branch when available.",
            ],
            [
              "Pinned workspace",
              "Reuse the selected workspace or project checkout. Choose this when the work should return to the same directory.",
            ],
          ]}
        />
        <H3>Understand session folders</H3>
        <P>
          A project-less automation can work in a session folder. Without a
          pinned folder, a session-mode automation gets a new folder for each
          run. The automation’s sidebar tags control where its workspaces
          appear; they do not change the code’s repository.
        </P>
        <H3>Inspect the result</H3>
        <P>
          Use run history to open the generated workspace and conversation. A
          run record may indicate that the agent started successfully while the
          agent continues working. Check the agent’s actual output and resulting
          changes. Pause the automation before changing its scope or schedule.
        </P>
        <Note>
          Make sure the selected machine is available. Avoid prompts that
          require unattended agents to wait for a person to approve or answer a
          question. Review plan availability and service-write settings before
          enabling a trigger.
        </Note>
      </>
    ),
  },
  {
    id: "remote-ssh",
    label: "Remote machines & SSH",
    title: "Work on a remote machine",
    summary:
      "Connect an SSH host and keep execution beside the remote repository and tools.",
    body: (
      <>
        <H3>Prepare the host</H3>
        <P>
          Use a supported macOS or glibc-based Linux SSH host. Set up SSH
          authentication, Git, and any agent CLIs or project tools you want to
          run. The remote runtime supports the published operating systems and
          architectures; Alpine-style musl hosts are not supported by the
          current glibc runtime.
        </P>
        <H3>Add the machine</H3>
        <P>
          Open Settings → Machines, add the SSH host, and configure its user,
          port, key, SSH configuration, or jump host as needed. Verify the host
          fingerprint before trusting a connection.
        </P>
        <Figure
          name="remote"
          alt="Adding and managing an SSH machine in Dragent"
        />
        <H3>Open a remote project</H3>
        <P>
          Connect the machine and select a project path on that host. The file
          explorer, terminal, and Git operations act on the remote project. A
          remote path refers to that machine’s filesystem; it is not a local
          desktop path.
        </P>
        <H3>Install the matching runtime and agents</H3>
        <P>
          Dragent provisions a checksum-verified host runtime for supported
          hosts. The desktop and host must use compatible versions. Authenticate
          coding agents on the host itself. Local credentials do not
          automatically sign a remote CLI into the provider.
        </P>
        <H3>MCP and connected services</H3>
        <P>
          Supported remote agents receive Dragent MCP registration through a
          context-aware dispatcher. Team operations execute on the remote host.
          Connected-service requests use your desktop connection; its
          credentials stay on the desktop, which must remain signed in and
          connected.
        </P>
        <H3>Reconnect and troubleshoot</H3>
        <P>
          A transport reconnect can attach to an existing host terminal while
          the host service remains alive. A host shutdown or service restart is
          a separate recovery event. Check the machine connection, runtime
          version, shell, and agent authentication before starting a replacement
          session.
        </P>
      </>
    ),
  },
  {
    id: "code-review",
    label: "Code reviews",
    title: "Review pull requests and merge requests",
    summary:
      "Read files, checks, and conversations before deciding what should ship.",
    body: (
      <>
        <H3>Connect the Git provider</H3>
        <P>
          Code reviews supports GitHub pull requests and GitLab merge requests.
          Authenticate the appropriate CLI on the machine where the project
          runs. Signing in to Dragent is separate from signing in to GitHub or
          GitLab CLI.
        </P>
        <CodeBlock
          code={"gh auth status\n# For GitLab projects:\nglab auth status"}
        />
        <P>
          If authentication is needed from the review screen, use its embedded
          terminal flow. For a remote project, authenticate on the remote
          machine.
        </P>
        <H3>Find the review</H3>
        <P>
          Open Code reviews from the sidebar. Choose a project and review state,
          then search by title, branch, author, or number. Select a review to
          inspect its overview, files, commits, checks, and activity.
        </P>
        <Figure
          name="reviews"
          alt="Changed-file tree and unified diff in Code reviews"
        />
        <H3>Read the diff and comment</H3>
        <P>
          Open Files, choose a file, and select a relevant line to begin an
          inline comment where supported. Inspect conversation threads in
          Activity and check CI status in Checks. Confirm whether an action
          saves a draft or publishes to the Git provider before submitting it.
        </P>
        <H3>Review with an agent</H3>
        <P>
          Use Review with agent to prepare a review workspace and instructions
          tied to the selected review. Check the agent, branch, and prompt
          before starting. Ask it to report actionable findings and
          verification. A review request does not grant permission to edit,
          push, merge, or publish comments.
        </P>
        <H3>Open the workspace</H3>
        <P>
          Use Open workspace when you want to continue in the review’s local
          working context. Check the selected branch, worktree, and any existing
          matching workspace before proceeding. The review browser and an
          editable checkout serve different purposes; inspect changes before
          taking an action that affects the repository.
        </P>
        <H3>Finish deliberately</H3>
        <P>
          Read the checks, resolve relevant discussions, and choose the merge
          action supported by the provider and your permissions. If an action
          reports a partial failure, inspect the provider before retrying a
          write so a comment or review is not published twice.
        </P>
      </>
    ),
  },
  {
    id: "agent-accounts",
    label: "Agent account profiles",
    title: "Choose accounts per project",
    summary:
      "Separate Dragent sign-in, Git authentication, and agent provider accounts.",
    body: (
      <>
        <H3>Three different identities</H3>
        <Table
          head={["Identity", "Used for"]}
          rows={[
            [
              "Dragent account and organisation",
              "Your workspace context, connected services, and Dragent billing.",
            ],
            [
              "Git provider login",
              "GitHub or GitLab repositories, pull requests, merge requests, and checks.",
            ],
            [
              "Coding agent account",
              "The provider account used by an installed CLI such as Claude Code or Codex.",
            ],
          ]}
        />
        <H3>Supported isolated profiles</H3>
        <P>
          Settings → Agent Accounts supports isolated Claude Code and Codex
          profiles on supported Unix hosts. Profiles have their own
          configuration directory and are bound to a machine. Other agents can
          use their normal system authentication; independently isolated storage
          is not claimed for every provider.
        </P>
        <Figure
          name="accounts"
          alt="Agent account profile settings in Dragent"
        />
        <H3>Set a project default</H3>
        <P>
          Add a profile, complete its native sign-in, check its status, and
          select it as the provider’s default for the project. The launch
          captures the chosen account context. An existing conversation should
          not silently resume under a different provider account.
        </P>
        <H3>Remote and Windows limitations</H3>
        <P>
          Authenticate a remote profile on its own host. A local profile does
          not copy credentials to another machine. Isolated profile ACL handling
          is not currently implemented for Windows hosts; use system
          authentication there.
        </P>
      </>
    ),
  },
  {
    id: "integrations",
    label: "Connected services",
    title: "Connect tools to the work",
    summary:
      "Bring issue, review, and service context into supported agent workflows.",
    body: (
      <>
        <H3>Connect a service</H3>
        <P>
          Open Settings → Integrations to see the available services and their
          connection state. Services may require a browser sign-in, a local CLI,
          an organisation connection, or additional scopes. Use the connection
          instructions for that service.
        </P>
        <Figure
          name="integrations"
          alt="Connected services and integration settings"
        />
        <H3>Give the agent context</H3>
        <P>
          Use supported integration tools to search for and read relevant items.
          Include the issue, repository, or error the agent should act on and
          make the requested outcome explicit. A service being connected does
          not mean an agent is authorised to publish changes.
        </P>
        <H3>Use service tools from the CLI</H3>
        <CodeBlock
          code={"dragent integrations services\ndragent integrations help"}
        />
        <P>
          The available tools and service keys depend on the active organisation
          and configured connections. Read-only discovery is a useful first step
          before granting write access.
        </P>
        <H3>Keep account scope clear</H3>
        <P>
          Connections follow the active Dragent account and organisation. For
          SSH agents, service calls are relayed through the connected desktop.
          Changing the account or organisation invalidates requests belonging to
          the earlier context rather than silently switching their identity.
        </P>
      </>
    ),
  },
  {
    id: "cli",
    label: "CLI reference",
    title: "Work from the command line",
    summary:
      "Inspect projects and agent sessions, create workspaces, and coordinate tasks.",
    body: (
      <>
        <H3>Find the command</H3>
        <P>
          Dragent-managed terminals put the <C>dragent</C> command on PATH.
          Outside the app, use the installed launcher under{" "}
          <C>~/.dragent/bin/</C> on macOS, or the corresponding launcher in the
          Windows user profile. Open the desktop app and sign in to the intended
          organisation first.
        </P>
        <CodeBlock
          code={
            "dragent help\ndragent status\ndragent workspaces list\ndragent agents running"
          }
        />
        <H3>Inspect or follow up with an agent</H3>
        <CodeBlock
          code={
            'dragent agents read AGENT_ID --lines 80\ndragent agents send AGENT_ID "Run the focused tests and report the result."'
          }
        />
        <P>
          Replace <C>AGENT_ID</C> with the session identifier shown by{" "}
          <C>agents running</C>. The read command reads the agent’s terminal
          screen; it does not send a prompt or execute the text it returns.
        </P>
        <H3>Create a workspace</H3>
        <CodeBlock
          code={
            'dragent workspaces create --project "/path/to/repo" \\\n  --agent codex --prompt "Fix the expired invitation flow and add a regression test"'
          }
        />
        <P>
          Replace the project path and choose an installed agent. Read{" "}
          <C>dragent workspaces help</C> before selecting branch or worktree
          options.
        </P>
        <H3>Coordinate tasks</H3>
        <CodeBlock
          code={
            "dragent team guide\ndragent team list\ndragent tasks list\ndragent msg inbox"
          }
        />
        <P>
          Team operations enforce the caller’s role. Workers report their
          assigned work and communicate through the mailbox; the lead manages
          the wider plan.
        </P>
        <H3>Automation and service commands</H3>
        <CodeBlock
          code={"dragent automations help\ndragent integrations help"}
        />
        <P>
          Use each command’s help for the current options. JSON and quiet modes
          are intended for scripts; human terminal output uses the formatted
          presentation.
        </P>
        <Note>
          Remote hosts expose the commands implemented by their headless
          runtime, including coordination and service tools. Do not assume that
          every desktop-only command is available on an SSH host.
        </Note>
      </>
    ),
  },
  {
    id: "mcp",
    label: "Dragent MCP",
    title: "Let agents use Dragent tools",
    summary:
      "Expose coordination and connected-service operations to supported coding agents.",
    body: (
      <>
        <H3>What the MCP server does</H3>
        <P>
          The Dragent MCP server gives compatible agents access to Dragent
          coordination and connected-service tools. The CLI and MCP server share
          the underlying operations. The calling agent’s role and account
          context still apply.
        </P>
        <CodeBlock code={"dragent mcp"} />
        <P>
          This starts a stdio server for an MCP client. It is not a normal
          interactive chat command; do not paste it into a shell expecting a
          conversation UI.
        </P>
        <H3>Automatic registration</H3>
        <P>
          Supported config editors handle Claude Code, Codex, Cursor, Gemini
          CLI, Qwen, and OpenCode. Registration preserves other user-owned
          servers. A conflicting user-owned server named <C>dragent</C> is
          reported rather than overwritten.
        </P>
        <H3>Remote registration</H3>
        <P>
          The remote host uses a dispatcher that reads the workspace’s runtime
          context. Global agent configuration does not pin the last connected
          organisation or carry desktop service credentials. Launch the agent
          through its Dragent SSH workspace so it inherits the correct context.
        </P>
        <H3>Enable or disable tools</H3>
        <P>
          Use the agent-tool settings in Dragent to control registration and
          tool availability. A running MCP process also checks whether tools are
          enabled, so removing a registration is not the only enforcement point.
        </P>
        <H3>Bring another MCP server</H3>
        <P>
          Dragent’s own MCP server is different from the MCP servers you add for
          an agent. Configure those tools through the supported settings, keep
          credentials out of repository files, and grant only the access the
          task needs.
        </P>
      </>
    ),
  },
  {
    id: "editor",
    label: "Editor & file explorer",
    title: "Keep the code beside the task",
    summary:
      "Navigate files, inspect changes, and make a focused edit in the workspace.",
    body: (
      <>
        <H3>Browse the workspace</H3>
        <P>
          Open Files in the project sidebar. Expand folders and select a file to
          open it in an editor pane. The explorer uses the selected workspace’s
          root, including a remote root when the workspace is on an SSH machine.
        </P>
        <Figure
          name="editor"
          alt="The Dragent editor with file explorer and workspace context"
        />
        <H3>Create a file or folder</H3>
        <P>
          Use the sticky Files header for New file, New folder, and Refresh.
          Select the parent folder first; selecting a file uses its containing
          directory. Enter the name inline and press Enter to save. Use Escape
          to cancel.
        </P>
        <H3>Inspect a change</H3>
        <P>
          Use Changes to find modified files and open their diffs. Read the
          surrounding code and run the appropriate checks before committing.
          Keep generated files and secrets out of a commit unless they belong
          there.
        </P>
        <H3>Separate review from editing</H3>
        <P>
          The workspace editor shows the working checkout. A hosted review shows
          a particular proposed change. Check the branch and file location
          before editing so a review does not accidentally become a change in an
          unrelated workspace.
        </P>
      </>
    ),
  },
];

GUIDES.push(
  {
    id: "install",
    label: "Install & first run",
    title: "Install Dragent",
    summary: "Choose the right build, sign in, and prepare your first project.",
    body: (
      <>
        <H3>Choose your desktop build</H3>
        <P>
          Open <Link href="/download">Downloads</Link> and select the build for
          your operating system and architecture. On macOS, Apple menu → About
          This Mac identifies Apple Silicon or Intel. On Windows, Settings →
          System → About shows the system type.
        </P>
        <Table
          head={["Platform", "Builds", "Install"]}
          rows={[
            [
              "macOS",
              "Apple Silicon or Intel",
              "Open the DMG, then drag Dragent into Applications.",
            ],
            [
              "Windows",
              "x64 or Arm64",
              "Run the EXE installer, then launch from the Start menu.",
            ],
            [
              "Linux desktop",
              "Not currently listed",
              "Use a supported Linux machine as an SSH host from the desktop app.",
            ],
          ]}
        />
        <H3>Sign in and choose an organisation</H3>
        <P>
          Launch Dragent and complete the account sign-in. Choose the
          organisation that owns the projects you want to work on. Your Dragent
          account is separate from GitHub or GitLab CLI authentication and from
          your coding agent’s provider login.
        </P>
        <H3>Prepare your tools</H3>
        <UL>
          <LI>Install Git and make sure the repository opens correctly.</LI>
          <LI>
            Install the CLI agent you want to use and complete its own sign-in
            on the same machine.
          </LI>
          <LI>
            Install the project’s dependencies and any environment configuration
            it needs.
          </LI>
          <LI>
            For remote work, prepare these tools on the SSH host. Local provider
            credentials do not automatically transfer.
          </LI>
        </UL>
        <H3>Start a focused task</H3>
        <P>
          Follow <Link href="/docs/quickstart">Your first workspace</Link>.
          Start with a small change so you can check the selected project,
          branch, agent, and model before moving on to coordination or
          automation.
        </P>
        <H3>If installation or sign-in fails</H3>
        <P>
          Check that the build matches the machine and try launching from the
          installed application location. Avoid disabling operating system
          protections to work around an unknown installer error. See{" "}
          <Link href="/docs/troubleshooting">Troubleshooting</Link>, or contact
          support with the operating system, architecture, app version, and
          error text. Do not include tokens or passwords.
        </P>
      </>
    ),
  },
  {
    id: "agent",
    label: "The Dragent agent",
    title: "Work with the built-in agent",
    summary:
      "A conversation for the task, with tool activity, background commands, and approvals in context.",
    body: (
      <>
        <H3>Give it the outcome</H3>
        <P>
          Select Dragent in the agent picker. Describe the change or question,
          the project context, and how you want the result verified. You can
          choose the built-in agent’s model separately. Its usage and
          credentials are configured independently from installed CLI agents.
        </P>
        <H3>Follow the conversation</H3>
        <P>
          Responses stay in a readable conversation. Tool activity is grouped
          with the relevant turn, so you can inspect a file read, search, edit,
          or command without losing the explanation around it. Questions and
          permission requests provide explicit places to respond.
        </P>
        <Figure
          name="agent-focus"
          alt="Built-in Dragent agent conversation showing expanded command output and a verification summary"
        />
        <H3>Inspect background commands</H3>
        <P>
          Agent commands run separately from your interactive terminal, with
          captured output and cancellation controls. Expand the command activity
          to read its output. A command reporting success is one part of
          verification; inspect what was run and whether it covers the task.
        </P>
        <H3>Choose permissions deliberately</H3>
        <P>
          Review the selected mode and permission policy before starting. When
          an approval is required, inspect the proposed action, command, and
          working directory. An instruction to investigate or review is not
          permission to publish, push, merge, or delete work.
        </P>
        <H3>Finish with the evidence</H3>
        <P>
          Ask for the changed files, checks performed, and remaining
          limitations. Read the diff and run the relevant checks before treating
          an agent’s summary as a completed, verified change.
        </P>
      </>
    ),
  },
  {
    id: "privacy",
    label: "Privacy & permissions",
    title: "Understand data and permissions",
    summary:
      "Know what runs on the machine, what a provider receives, and which identity an action uses.",
    body: (
      <>
        <H3>Local and remote execution</H3>
        <P>
          Workspace files, shell commands, and installed agent CLIs operate on
          the selected machine. In an SSH workspace, that machine is the remote
          host. Review the project root and working directory before an action
          that changes files.
        </P>
        <H3>AI and connected services</H3>
        <P>
          AI requests can send prompts, selected code, tool results, and other
          task context to the configured provider. External CLI agents also have
          their own data policies and configuration. Connected services receive
          the requests made through their tools. Local execution does not mean
          every part of a workflow stays offline.
        </P>
        <H3>Limit the context</H3>
        <P>
          Keep credentials out of source control and prompts. Review indexing
          exclusions and the files attached to a task. Excluding a file from an
          index is different from preventing a shell command or an agent tool
          from reading it.
        </P>
        <H3>Account and organisation boundaries</H3>
        <P>
          Dragent account scope, Git provider authentication, and agent provider
          authentication are separate. Check the active organisation and machine
          before starting work. Remote service tools rely on the connected
          desktop’s account context.
        </P>
        <H3>Control actions</H3>
        <P>
          Set the agent’s permissions to match the task. Read approval requests
          before accepting them. Review agent output and diffs before sharing
          code or publishing to a connected service.
        </P>
        <Note>
          For collection, retention, account, and contact details, read the{" "}
          <Link href="/privacy">Privacy Policy</Link> and the policies of the
          providers you connect.
        </Note>
      </>
    ),
  },
  {
    id: "platforms",
    label: "Platform support",
    title: "Desktop and remote platform support",
    summary: "Choose a supported desktop, host, and agent configuration.",
    body: (
      <>
        <H3>Desktop downloads</H3>
        <P>
          Dragent provides macOS Apple Silicon and Intel builds, plus Windows
          x64 and Arm64 builds. See <Link href="/download">Downloads</Link> for
          the available installers. A Linux desktop installer is not currently
          listed.
        </P>
        <H3>SSH hosts</H3>
        <P>
          Supported remote runtimes target macOS and glibc-based Linux hosts.
          The desktop and remote runtime must be compatible. Musl-based Linux
          distributions, such as Alpine, are not supported by the current glibc
          runtime.
        </P>
        <H3>Agent feature differences</H3>
        <P>
          Available agent CLIs, models, recovery, account isolation, and tool
          registration depend on the provider and host. Isolated Claude Code and
          Codex account profiles currently require supported Unix hosts; Windows
          uses system authentication. See{" "}
          <Link href="/docs/agent-accounts">Agent account profiles</Link> and{" "}
          <Link href="/docs/remote-ssh">Remote machines</Link> before choosing a
          setup.
        </P>
      </>
    ),
  },
);
