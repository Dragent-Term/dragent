import type { Metadata } from "next";
import Image from "next/image";
import Link from "next/link";
import {
  ArrowRight,
  ArrowUpRight,
  BookOpen,
  Check,
  GitBranch,
  GitPullRequest,
  Globe,
  Layers3,
  MessageSquare,
  Play,
  Plug,
  Plus,
  Server,
  Smartphone,
  Sparkles,
  Terminal,
  Workflow,
} from "lucide-react";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { AppDemo } from "@/components/AppDemo";
import { DownloadButton, DownloadNote } from "@/components/DownloadButton";
import { HeroTeamDemo } from "@/components/HeroTeamDemo";
import { HeroGlyphField } from "@/components/HeroGlyphField";
import { agentMarkClass } from "@/lib/agents";

export const metadata: Metadata = { alternates: { canonical: "/" } };
const agents = [
  { id: "dragent", name: "Dragent", file: "dragent-mark.png", builtin: true },
  { id: "claude", name: "Claude Code" },
  { id: "codex", name: "Codex" },
  { id: "opencode", name: "OpenCode" },
  { id: "cursor", name: "Cursor" },
  { id: "gemini", name: "Gemini CLI" },
  { id: "copilot", name: "GitHub Copilot" },
  { id: "amp", name: "Amp" },
  { id: "antigravity", name: "Antigravity" },
  { id: "auggie", name: "Auggie" },
  { id: "autohand", name: "Autohand Code" },
  { id: "charm", name: "Charm", file: "charm.png" },
  { id: "cline", name: "Cline", file: "cline.png" },
  { id: "codebuddy", name: "CodeBuddy Code" },
  { id: "codebuff", name: "Codebuff", file: "codebuff.png" },
  { id: "commandcode", name: "Command Code" },
  { id: "continue", name: "Continue", file: "continue.png" },
  { id: "devin", name: "Devin" },
  { id: "droid", name: "Droid" },
  { id: "freebuff", name: "Freebuff", file: "freebuff.png" },
  { id: "goose", name: "Goose", file: "goose.png" },
  { id: "grok", name: "Grok" },
  { id: "hermes", name: "Hermes Agent", file: "hermes.png" },
  { id: "jules", name: "Jules" },
  { id: "junie", name: "Junie", file: "junie.png" },
  { id: "kilocode", name: "Kilocode", file: "kilocode.png" },
  { id: "kimi", name: "Kimi" },
  { id: "kiro", name: "Kiro" },
  { id: "letta", name: "Letta" },
  { id: "mimocode", name: "MiMo Code" },
  { id: "mistral", name: "Mistral Vibe" },
  { id: "muse", name: "Muse Code", file: "muse.png" },
  { id: "ohmpi", name: "Oh My Pi" },
  { id: "pi", name: "Pi" },
  { id: "primeagent", name: "Prime Agent" },
  { id: "qoder", name: "Qoder CLI" },
  { id: "qwen", name: "Qwen Code" },
  { id: "rovo", name: "Rovo Dev", file: "rovo.png" },
  { id: "zero", name: "Zero" },
];
const teamFlow = [
  {
    who: "Lead",
    title: "The lead plans",
    text: "Splits the goal into tasks, decides who does what, and what must wait for what.",
  },
  {
    who: "Workers",
    title: "The team builds",
    text: "Each worker gets its own worktree and its own files, and messages the others when something shared changes.",
  },
  {
    who: "You",
    you: true,
    title: "It needs you",
    text: "A decision only you can make arrives as a question, on your desktop and your phone.",
  },
  {
    who: "Checker",
    title: "The work is checked",
    text: "Tests, linters, and the original goal, before anything reaches you.",
  },
  {
    who: "You",
    you: true,
    title: "Ready for review",
    text: "One change, with what was done and how it was checked. Comment on a line and it goes back to its worker.",
  },
];

const questions = [
  {
    question: "What is Dragent?",
    answer:
      "Dragent is a desktop workspace for coding agents. It brings projects, Git worktrees, terminal sessions, an editor, agent coordination, remote machines, and code reviews into one app. Start with one agent, or coordinate a team when a task needs several contributors.",
  },
  {
    question: "Can I use the agent subscription I already have?",
    answer:
      "Installed CLI agents use their own provider authentication and billing. Install and sign in to the CLI on the machine where the project runs. Dragent’s built-in agent has separate model and usage settings; it does not replace or include an external agent subscription.",
  },
  {
    question: "Do I have to coordinate multiple agents?",
    answer:
      "No. Start a workspace with a single agent or a terminal. Use separate worktrees for independent tasks, and enable coordination when you want a lead to manage tasks, workers, dependencies, and handoffs.",
  },
  {
    question: "Can I work on a remote machine?",
    answer:
      "Yes. Connect a supported macOS or glibc-based Linux machine over SSH. The project, terminal commands, and agent CLIs run on that host. Authenticate the agents on the host; local provider logins do not automatically transfer. Remote connected-service tools need the desktop bridge to remain signed in and connected.",
  },
  {
    question: "Does code review work with GitLab?",
    answer:
      "Yes. Code reviews supports GitHub pull requests and GitLab merge requests. Authenticate gh or glab on the machine hosting the project. Your Dragent sign-in and Git provider login are separate.",
  },
  {
    question: "Where can I install Dragent?",
    answer:
      "Desktop installers are available for macOS and Windows, with Apple Silicon, Intel, x64, and Arm64 builds. A Linux desktop download is not currently listed. Supported Linux machines can be connected as SSH hosts.",
  },
];
function More({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <Link href={href} className="text-link">
      {children}
      <ArrowUpRight size={14} aria-hidden="true" />
    </Link>
  );
}
function Shot({
  name,
  alt,
  className = "",
}: {
  name: string;
  alt: string;
  className?: string;
}) {
  return (
    <div className={`product-shot ${className}`}>
      <Image
        src={`/screens/${name}.webp`}
        alt={alt}
        width={1440}
        height={900}
        sizes="(max-width: 760px) 100vw, 800px"
      />
    </div>
  );
}
export default function Home() {
  return (
    <>
      <Nav />
      <main id="main-content" className="marketing-home">
        <section className="hero container-wide">
          <div className="hero-backdrop" aria-hidden="true">
            <div className="hero-glow" />
            <div className="hero-matrix">
              <HeroGlyphField />
            </div>
            <div className="hero-stars" />
            <div className="hero-grain" />
            <div className="hero-fade" />
          </div>
          <Link className="hero-kicker" href="/docs/coordination" aria-label="Read how agent teams work">
            <span className="status-dot" aria-hidden="true" /> <span className="hero-kicker-text">Agent teams for the
            agents you already use</span>
            <ArrowRight size={13} aria-hidden="true" />
          </Link>
          <h1>
            <span className="hero-quiet">Stop babysitting AI terminals.</span>
            <br />
            <span className="hero-lit">Run an agent team.</span>
          </h1>
          <p className="hero-intro">
            Give a lead agent your goal. It splits the work across isolated
            Git worktrees,
            <br className="desktop-break" /> hands each part to Claude Code,
            Codex, or any agent you use, and asks you only when a decision is
            yours.
          </p>
          <div className="hero-actions">
            <DownloadButton size="lg" />
            <a className="button button-secondary" href="#product">
              <Play size={14} aria-hidden="true" />
              See it in action
            </a>
          </div>
          <DownloadNote />
          {/* "See it in action" and the footer's Product link land here. */}
          <div id="product">
            <HeroTeamDemo />
          </div>
        </section>

        <section
          className="team-flow-section container-wide"
          aria-labelledby="team-flow-heading"
        >
          <div className="center-heading">
            <p className="eyebrow">How a team works</p>
            <h2 id="team-flow-heading">
              You give the goal.
              <br />
              A team does the work.
            </h2>
            <p>
              Five moments from goal to review. You&rsquo;re in two of them.
            </p>
          </div>
          <ol className="team-flow">
            {teamFlow.map((step, index) => (
              <li key={step.title} className={step.you ? "is-you" : undefined}>
                <span className="team-flow-n">0{index + 1}</span>
                <span className="team-flow-who">{step.who}</span>
                <h3>{step.title}</h3>
                <p>{step.text}</p>
              </li>
            ))}
          </ol>
          <More href="/docs/coordination">Read how teams work</More>
        </section>

        <section
          className="agent-section container-wide"
          aria-labelledby="agents-heading"
        >
          <p className="eyebrow">
            {agents.length} supported agents — or add your own CLI
          </p>
          <h2 id="agents-heading">
            Bring any agent onto the team.
            <br />
            Keep your own subscriptions.
          </h2>
          <div className="agent-grid">
            {agents.map((agent) => {
              const builtin = "builtin" in agent && agent.builtin;
              return (
                <Link
                  href="/docs/agents"
                  key={agent.id}
                  className={builtin ? "is-builtin" : undefined}
                >
                  <span className="agent-tile">
                    <Image
                      src={
                        agent.id === "dragent"
                          ? `/${agent.file}`
                          : `/agents/${agent.file ?? `${agent.id}.svg`}`
                      }
                      alt=""
                      width={22}
                      height={22}
                      className={agentMarkClass(agent.id)}
                    />
                    {builtin && <em className="agent-builtin">Built-in</em>}
                  </span>
                  <span className="agent-name">{agent.name}</span>
                </Link>
              );
            })}
            <Link href="/docs/agents" className="agent-add">
              <span className="agent-tile" aria-hidden="true">
                <Plus size={16} />
              </span>
              <span className="agent-name">Bring any CLI</span>
            </Link>
          </div>
          <Link className="agent-more" href="/docs/agents">
            Choose your agent <ArrowRight size={13} aria-hidden="true" />
          </Link>
        </section>

        <section
          className="workspace-demo-section container-wide"
          aria-labelledby="workspace-demo-heading"
        >
          <div className="center-heading">
            <p className="eyebrow">Where each agent works</p>
            <h2 id="workspace-demo-heading">
              A real workspace
              <br />
              for every agent.
            </h2>
            <p>
              Each worker gets a modern terminal, an editor, and its own
              branch. Failed commands get fixed where they broke, and finished
              work goes straight to review. Open any of them when you want the
              inside view.
            </p>
          </div>
          <AppDemo />
          <div className="workspace-band">
            <div className="workspace-band-copy">
              <p className="eyebrow">The workspace around the command</p>
              <h2>
                Keep the terminal familiar. Make the work easier to finish.
              </h2>
              <p>
                Dragent brings commands, project context, and agent-assisted
                delivery into one focused workspace—without asking you to leave
                the tools you already use.
              </p>
              <More href="/docs">See how the workspace works</More>
            </div>
            <div className="workspace-band-image">
              <Image
                src="/terminal-workspace.jpg"
                alt="Developer working at a desk with a terminal workspace"
                fill
                sizes="(min-width: 1000px) 60vw, 100vw"
              />
            </div>
          </div>
          <p className="workspace-demo-caption">
            Switch between <span>shell</span> and{" "}
            <span className="workspace-demo-agent">agent</span> mode — run
            commands, or ask the agent to write and refactor code.
          </p>
        </section>

        <section id="features" className="product-section container-wide">
          <div className="center-heading">
            <p className="eyebrow">Built around the work</p>
            <h2>
              From the first prompt
              <br />
              to the final review.
            </h2>
            <p>
              Everything you need to keep agents moving.
              <br />
              Without keeping a dozen windows in your head.
            </p>
          </div>
          <div className="feature-bento">
            <article id="coordination" className="bento-item bento-wide">
              <div className="bento-copy">
                <span className="feature-label">
                  <Layers3 size={16} aria-hidden="true" /> Agent coordination
                </span>
                <h3>
                  Give the goal.
                  <br />
                  Keep the team in sync.
                </h3>
                <p>
                  Let a lead break the work into tasks and assign workers. See
                  owners, dependencies, and handoffs. When the team needs a
                  decision, you’re in the conversation.
                </p>
                <More href="/docs/coordination">Meet your agent team</More>
              </div>
              <Shot
                name="coordination-focus"
                alt="Native coordination plan with Codex and Claude Code tasks, task status, and dependencies"
              />
            </article>
            <article id="workspaces" className="bento-item">
              <div className="bento-copy">
                <span className="feature-label">
                  <GitBranch size={16} aria-hidden="true" /> Parallel workspaces
                </span>
                <h3>
                  Different tasks.
                  <br />
                  Different branches.
                </h3>
                <p>
                  Give each change its own Git worktree. Start another agent
                  without putting your current work on hold.
                </p>
                <More href="/docs/workspaces">Make room for parallel work</More>
              </div>
              <Shot
                name="workspace-focus"
                alt="Workspace composer with machine, project, worktree, agent, and model selectors"
              />
            </article>
            <article className="bento-item">
              <div className="bento-copy">
                <span className="feature-label">
                  <Workflow size={16} aria-hidden="true" /> Automations
                </span>
                <h3>
                  The work repeats.
                  <br />
                  The setup shouldn’t.
                </h3>
                <p>
                  Schedule the review, the dependency check, or the daily
                  triage. Pick the agent, prompt, and place to run.
                </p>
                <More href="/docs/automations">
                  Put recurring work on a schedule
                </More>
              </div>
              <Shot
                name="automations-focus"
                alt="Automations list showing scheduled review, dependency check, and issue triage"
              />
            </article>
          </div>
        </section>

        <section id="reviews" className="showcase-section container-wide">
          <div className="showcase-heading">
            <div>
              <p className="eyebrow">
                <GitPullRequest size={14} aria-hidden="true" /> Review & ship
              </p>
              <h2>
                The agent wrote it.
                <br />
                You should understand it.
              </h2>
            </div>
            <div>
              <p>
                Read the diff, follow the discussion, and check CI in one view.
                Bring an agent into the review when you want a second set of
                eyes.
              </p>
              <More href="/docs/code-review">Explore code reviews</More>
            </div>
          </div>
          <div className="review-stage">
            <Shot
              name="review-focus"
              alt="Code review with source files, syntax-highlighted changes, and review comment controls"
            />
            <div className="review-stage-caption">
              <span>
                <Check size={14} />
                GitHub pull requests
              </span>
              <span>
                <Check size={14} />
                GitLab merge requests
              </span>
              <span>
                <MessageSquare size={14} />
                Threads & inline comments
              </span>
              <span>
                <Sparkles size={14} />
                Review with an agent
              </span>
            </div>
          </div>
        </section>

        <section id="remote" className="remote-section container-wide">
          <div className="remote-visual">
            <div className="remote-topology">
              <div>
                <Image src="/dragent-mark.png" width={30} height={30} alt="" />
                <span>Your desktop</span>
              </div>
              <span className="connection-line">
                <b>SSH</b>
              </span>
              <div>
                <Server size={26} />
                <span>Your dev machine</span>
                <small>Code · Tools · Agents</small>
              </div>
            </div>
            <Shot
              name="remote-focus"
              alt="SSH host overview with connection health, runtime, resource usage, and installed tools"
            />
          </div>
          <div className="remote-copy">
            <p className="eyebrow">Remote, without the detour</p>
            <h2>
              The right machine.
              <br />
              The same workspace.
            </h2>
            <p>
              Keep the code and compute on your SSH host. Bring its files,
              terminal, Git, and agents into Dragent.
            </p>
            <ul>
              <li>
                <Check size={15} /> Work where the dependencies already live
              </li>
              <li>
                <Check size={15} /> Inspect the host and its installed agents
              </li>
              <li>
                <Check size={15} /> Coordinate supported CLI agents remotely
              </li>
            </ul>
            <More href="/docs/remote-ssh">Bring your own machine</More>
            <p className="feature-footnote">
              Supported macOS and glibc-based Linux hosts. Agent authentication
              happens on the host.
            </p>
          </div>
        </section>

        <section className="details-section container-wide">
          <div className="center-heading">
            <p className="eyebrow">Nothing left out</p>
            <h2>
              Every feature has
              <br />
              a role on the team.
            </h2>
          </div>
          <div className="detail-grid">
            {[
              {
                icon: MessageSquare,
                title: "Workers that talk",
                text: "Workers message each other and the lead about shared types and API changes, so parallel work fits together.",
                href: "/docs/mcp",
              },
              {
                icon: Globe,
                title: "How the checker sees your UI",
                text: "The built-in browser lets an agent open your app, click through it, and check the screen it was asked to build.",
                href: "/docs/tools",
              },
              {
                icon: Smartphone,
                title: "How your team reaches you",
                text: "When the lead needs a decision, it reaches your phone. Answer from the notification, end-to-end encrypted.",
                href: "/docs/coordination",
              },
              {
                icon: Plug,
                title: "Hand the team a ticket",
                text: "Give a Linear, Jira, or GitHub issue as the goal. Agents read it and work with your connected tools.",
                href: "/docs/integrations",
              },
              {
                icon: Workflow,
                title: "A team that starts itself",
                text: "Automations start agents on a schedule or when something new arrives, and report what they did.",
                href: "/docs/automations",
              },
              {
                icon: Terminal,
                title: "The quick fix, and the inside view",
                text: "Run one agent for a small change. Open any worker's live terminal, editor, and diff whenever you want.",
                href: "/docs/blocks",
              },
            ].map(({ icon: Icon, title, text, href }) => (
              <Link key={title} href={href}>
                <Icon size={21} strokeWidth={1.5} aria-hidden="true" />
                <h3>{title}</h3>
                <p>{text}</p>
                <ArrowUpRight
                  className="detail-arrow"
                  size={14}
                  aria-hidden="true"
                />
              </Link>
            ))}
          </div>
        </section>

        <section className="builtin-section container-wide">
          <div className="builtin-copy">
            <p className="eyebrow">Meet the Dragent agent</p>
            <h2>
              An agent built into
              <br />
              the workspace.
            </h2>
            <p>
              Explore a codebase, plan a change, and follow it through. Read the
              explanation, expand the tool activity, and respond when the agent
              needs your input.
            </p>
            <p>
              Commands run in the background with captured output. Your
              interactive terminal stays free for your own work.
            </p>
            <More href="/docs/agent">Work with Dragent</More>
          </div>
          <Shot
            name="agent-focus"
            alt="Dragent conversation with readable explanations, expanded test output, and a change summary"
          />
        </section>

        <section className="developer-section container-wide">
          <div>
            <p className="eyebrow">Made for the way you build</p>
            <h2>
              Click it. Script it.
              <br />
              Ask an agent to do it.
            </h2>
            <p>
              Use the desktop when you want the full picture. Use the CLI to
              inspect sessions and start work. Give agents coordination tools
              through Dragent MCP.
            </p>
            <More href="/docs/cli">Explore the command line</More>
          </div>
          <div className="terminal-preview">
            <div>
              <Terminal size={14} />
              <span>dragent / cli</span>
              <span>EXAMPLE COMMANDS</span>
            </div>
            <pre>
              <code>
                <span className="cli-comment"># See the work in flight</span>
                {"\n"}
                <span className="cli-prompt">❯</span> dragent workspaces list
                {"\n\n"}
                <span className="cli-comment">
                  # Inspect an agent’s terminal
                </span>
                {"\n"}
                <span className="cli-prompt">❯</span> dragent agents read
                AGENT_ID --lines 80{"\n\n"}
                <span className="cli-comment"># Follow the team</span>
                {"\n"}
                <span className="cli-prompt">❯</span> dragent tasks list{"\n"}
                <span className="cli-prompt">❯</span> dragent msg inbox
              </code>
            </pre>
            <Link href="/docs/mcp">
              Connect your agents with MCP <ArrowUpRight size={14} />
            </Link>
          </div>
        </section>

        <section className="learning-section container-wide">
          <div>
            <BookOpen size={24} strokeWidth={1.4} />
            <h2>
              Less guessing.
              <br />
              More getting started.
            </h2>
            <p>Practical guides, clear concepts, and the commands you need.</p>
            <More href="/docs">Open the documentation</More>
          </div>
          <nav aria-label="Getting started guides">
            {[
              {
                n: "01",
                name: "Your first workspace",
                sub: "A project, an agent, and your first change",
                href: "/docs/quickstart",
              },
              {
                n: "02",
                name: "Coordinate a team",
                sub: "Leads, workers, and a shared plan",
                href: "/docs/coordination",
              },
              {
                n: "03",
                name: "Work on a remote machine",
                sub: "Your SSH host, inside Dragent",
                href: "/docs/remote-ssh",
              },
            ].map((item) => (
              <Link key={item.n} href={item.href}>
                <span>{item.n}</span>
                <div>
                  <h3>{item.name}</h3>
                  <p>{item.sub}</p>
                </div>
                <ArrowRight size={17} aria-hidden="true" />
              </Link>
            ))}
          </nav>
        </section>

        <section className="faq-section container-wide">
          <div>
            <p className="eyebrow">Good questions</p>
            <h2>
              A few things
              <br />
              worth knowing.
            </h2>
            <More href="/docs/faq">More in the docs</More>
          </div>
          <div className="faq-list">
            {questions.map((item) => (
              <details key={item.question}>
                <summary>
                  {item.question}
                  <span aria-hidden="true">+</span>
                </summary>
                <p>{item.answer}</p>
              </details>
            ))}
          </div>
        </section>
        <section className="closing-section">
          <div className="container-wide">
            <Image src="/dragent-mark.png" alt="" width={58} height={58} />
            <h2>
              Hand off the goal.
              <br />
              <span>Keep the decisions.</span>
            </h2>
            <p>Bring your next project and the agents you already pay for.</p>
            <div className="hero-actions">
              <DownloadButton size="lg" />
              <Link className="button button-secondary" href="/docs/quickstart">
                Get started <ArrowRight size={15} />
              </Link>
            </div>
            <DownloadNote />
          </div>
        </section>
      </main>
      <Footer />
    </>
  );
}
