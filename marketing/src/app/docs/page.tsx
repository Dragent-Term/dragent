import type { Metadata } from "next";
import Link from "next/link";
import {
  ArrowRight,
  BookOpen,
  GitBranch,
  GitPullRequest,
  Layers3,
  Plug,
  Server,
  Terminal,
  Workflow,
} from "lucide-react";
import { DocsShell } from "@/components/docs/DocsShell";
import { LegacyHash } from "@/components/docs/LegacyHash";
import { allDocs } from "@/components/docs/catalog";
export const metadata: Metadata = {
  title: "Documentation",
  description:
    "Get started with Dragent. Learn workspaces, agent coordination, SSH machines, automations, code review, and the CLI.",
  alternates: { canonical: "/docs" },
};
const workflows = [
  {
    icon: GitBranch,
    title: "Workspaces & branches",
    text: "Choose the checkout and keep tasks separate.",
    href: "workspaces",
  },
  {
    icon: Layers3,
    title: "Agent coordination",
    text: "Connect leads, workers, tasks, and decisions.",
    href: "coordination",
  },
  {
    icon: Server,
    title: "Remote machines",
    text: "Put the workflow on your own SSH host.",
    href: "remote-ssh",
  },
  {
    icon: GitPullRequest,
    title: "Code reviews",
    text: "Understand the files, checks, and conversation.",
    href: "code-review",
  },
  {
    icon: Workflow,
    title: "Automations",
    text: "Give repeated work an agent and a schedule.",
    href: "automations",
  },
  {
    icon: Plug,
    title: "Connected services",
    text: "Bring your tools and context into the task.",
    href: "integrations",
  },
];
export default function DocsPage() {
  return (
    <DocsShell>
      <LegacyHash ids={allDocs.map((doc) => doc.id)} />
      <header className="docs-hub-header">
        <p className="eyebrow">Dragent documentation</p>
        <h1>
          Good work starts
          <br />
          with a clear picture.
        </h1>
        <p>
          Learn the workspace, find your workflow, and go from a first task to a
          reviewed change.
        </p>
      </header>
      <Link href="/docs/quickstart" className="quickstart-banner">
        <span className="quickstart-icon">
          <BookOpen size={23} />
        </span>
        <div>
          <span>START HERE</span>
          <h2>Your first workspace</h2>
          <p>Add a project, pick an agent, and make a change you can review.</p>
        </div>
        <ArrowRight size={20} aria-hidden="true" />
      </Link>
      <div className="doc-hub-section">
        <h2>Understand the essentials</h2>
        <p>Four concepts make the rest of Dragent easier to use.</p>
        <Link className="concept-link" href="/docs/concepts">
          <span>
            Organisation <b>→</b> Project <b>→</b> Workspace <b>→</b> Session
          </span>
          <ArrowRight size={15} aria-hidden="true" />
        </Link>
      </div>
      <div className="doc-hub-section">
        <h2>Find your workflow</h2>
        <div className="docs-workflows">
          {workflows.map(({ icon: Icon, title, text, href }) => (
            <Link href={`/docs/${href}`} key={href}>
              <Icon size={20} strokeWidth={1.5} aria-hidden="true" />
              <h3>{title}</h3>
              <p>{text}</p>
              <span>
                Read the guide <ArrowRight size={12} aria-hidden="true" />
              </span>
            </Link>
          ))}
        </div>
      </div>
      <div className="doc-hub-section">
        <h2>Build on what you know</h2>
        <div className="docs-reference-links">
          <Link href="/docs/cli">
            <Terminal size={18} />
            <div>
              <strong>Command line</strong>
              <span>Inspect sessions and drive the workspace.</span>
            </div>
            <ArrowRight size={14} />
          </Link>
          <Link href="/docs/mcp">
            <Plug size={18} />
            <div>
              <strong>Dragent MCP</strong>
              <span>Give agents access to coordination and tools.</span>
            </div>
            <ArrowRight size={14} />
          </Link>
        </div>
      </div>
      <div className="doc-hub-help">
        <h2>Need a hand?</h2>
        <p>
          Start with <Link href="/docs/troubleshooting">Troubleshooting</Link>{" "}
          or the <Link href="/docs/faq">FAQ</Link>. For an account or connection
          problem, <a href="mailto:support@dragent.dev">get in touch</a>.
        </p>
      </div>
    </DocsShell>
  );
}
