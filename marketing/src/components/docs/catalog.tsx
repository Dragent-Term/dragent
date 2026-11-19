import { Children, cloneElement, isValidElement, type ReactNode } from "react";
import { DOC_GROUPS, type DocSection } from "./content";
import { GUIDES } from "./guides";
import { H3, LI, P, UL } from "./primitives";
import { headingId, nodeText } from "./text";

const existing = DOC_GROUPS.flatMap((group) => group.sections);
const sections = new Map(
  [...existing, ...GUIDES].map((section) => [section.id, section]),
);
sections.set("introduction", {
  id: "introduction",
  label: "What is Dragent?",
  title: "What is Dragent?",
  summary:
    "A desktop workspace for coding agents, your terminal, and the work you ship.",
  body: (
    <>
      <P>
        Dragent brings projects, Git worktrees, coding agents, terminal panes,
        and code review into one workspace. Use one agent for a focused task,
        keep independent changes on separate branches, or coordinate a lead and
        workers against a shared plan.
      </P>
      <H3>Start with the work</H3>
      <UL>
        <LI>
          Create a workspace for an existing repository and choose an agent.
        </LI>
        <LI>Use isolated worktrees when tasks need independent checkouts.</LI>
        <LI>
          Follow changes, checks, and review discussion alongside the code.
        </LI>
        <LI>
          Connect an SSH machine when the project needs remote tools or compute.
        </LI>
      </UL>
      <H3>Keep your existing agents</H3>
      <P>
        Installed CLI agents use their own tools and provider authentication.
        Dragent also includes its own agent with a conversation interface, tool
        activity, and approval flows. Available features depend on the selected
        provider and host.
      </P>
      <H3>Choose your next step</H3>
      <P>
        Install Dragent, follow Your first workspace, then learn the project and
        session model. Coordination, automations, remote machines, and connected
        services are workflows you can add when you need them.
      </P>
    </>
  ),
});

const structure = [
  {
    label: "Get started",
    ids: ["introduction", "install", "quickstart", "concepts"],
  },
  { label: "Your workspace", ids: ["workspaces", "tabs-panes", "editor"] },
  {
    label: "Work with agents",
    ids: [
      "agents",
      "agent",
      "agent-sleep",
      "coordination",
      "agent-accounts",
      "models-keys",
      "tools",
    ],
  },
  {
    label: "Review & automate",
    ids: ["code-review", "automations", "workflows"],
  },
  {
    label: "Remote & collaboration",
    ids: ["remote-ssh", "shared-sessions", "port-forwarding"],
  },
  { label: "Terminal", ids: ["blocks", "command-input", "self-healing"] },
  {
    label: "Extend Dragent",
    ids: [
      "integrations",
      "mcp",
      "cli",
      "skills",
      "codebase",
      "attachments-voice",
    ],
  },
  {
    label: "Configuration",
    ids: ["themes", "settings", "shortcuts", "privacy", "billing", "platforms"],
  },
  { label: "Help & reference", ids: ["troubleshooting", "faq", "licenses"] },
];
export const groups = structure.map((group) => ({
  label: group.label,
  sections: group.ids.map((id) => {
    const section = sections.get(id);
    if (!section) throw new Error(`Missing documentation: ${id}`);
    return section;
  }),
}));
export const allDocs = groups.flatMap((group) => group.sections);
export function getDoc(id: string) {
  return allDocs.find((section) => section.id === id);
}
export const navigation = groups.map((group) => ({
  label: group.label,
  sections: group.sections.map(({ id, label, title, summary }) => ({
    id,
    label,
    title,
    summary: summary ?? title,
  })),
}));
export const searchEntries = allDocs.map((doc) => ({
  id: doc.id,
  title: doc.title,
  label: doc.label,
  description: doc.summary ?? doc.title,
  keywords: nodeText(doc.body).slice(0, 2400),
}));
export type DocNavigation = typeof navigation;
export type SearchEntry = (typeof searchEntries)[number];

export function headings(node: ReactNode): { id: string; title: string }[] {
  if (Array.isArray(node)) return node.flatMap(headings);
  if (!isValidElement<{ children?: ReactNode; id?: string }>(node)) return [];
  if (node.type === H3) {
    const title = nodeText(node.props.children);
    return [{ id: node.props.id ?? headingId(title), title }];
  }
  return headings(node.props.children);
}
/** Preserve existing cross-chapter hash links when converting the old guide to routes. */
export function resolveDocLinks(node: ReactNode): ReactNode {
  if (Array.isArray(node)) return Children.map(node, resolveDocLinks);
  if (!isValidElement<{ children?: ReactNode; href?: string }>(node))
    return node;
  const href = node.props.href;
  return cloneElement(node, {
    ...(href?.startsWith("#") && getDoc(href.slice(1))
      ? { href: `/docs/${href.slice(1)}` }
      : {}),
    ...(node.props.children !== undefined
      ? { children: resolveDocLinks(node.props.children) }
      : {}),
  });
}
export function docGroup(doc: DocSection) {
  return (
    groups.find((group) =>
      group.sections.some((section) => section.id === doc.id),
    )?.label ?? "Documentation"
  );
}
