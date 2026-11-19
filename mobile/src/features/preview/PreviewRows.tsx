import { ResourceRow as PreviewRowShell, countLabel } from "../../components/ResourceRow";
import {
  AgentMark, BrowserMark, StatusBadge, TerminalMark,
  agentStateLabel, type AgentDotState, type IconName, type StatusTone,
} from "../../components";
import type { DesktopSummary, PreviewAgent, PreviewTab, ProjectSummary, WorkspaceSummary } from "../../preview/types";

export const PLATFORM_LABEL: Record<DesktopSummary["platform"], string> = {
  macos: "macOS",
  linux: "Linux",
};

export const AGENT_LABEL: Record<PreviewAgent, string> = {
  dragent: "Dragent (built-in)",
  codex: "Codex",
  opencode: "OpenCode",
};

export const TAB_KIND_ICON = { terminal: "terminal", agent: "agent", browser: "browser" } as const satisfies Record<
  PreviewTab["kind"],
  IconName
>;

export const WORKSPACE_STATUS: Record<WorkspaceSummary["status"], { label: string; tone: StatusTone }> = {
  working: { label: "Working", tone: "accent" },
  review: { label: "In review", tone: "warning" },
  idle: { label: "Idle", tone: "neutral" },
};

export { ResourceRow as PreviewRowShell, countLabel } from "../../components/ResourceRow";

export function tabKindLabel(tab: PreviewTab): string {
  if (tab.kind === "terminal") return "Terminal tab";
  if (tab.kind === "browser") return "Browser tab";
  return `${AGENT_LABEL[tab.agent]} tab`;
}

export interface DesktopRowProps {
  desktop: DesktopSummary;
  projectCount: number;
  workspaceCount: number;
  tabCount: number;
  onOpen: (id: string) => void;
}

/** A desktop entry: monitor icon, name, platform, and project/workspace counts from data. */
export function DesktopRow({ desktop, projectCount, workspaceCount, tabCount, onOpen }: DesktopRowProps) {
  const detail =
    `${PLATFORM_LABEL[desktop.platform]} · ` +
    `${countLabel(projectCount, "project", "projects")} · ` +
    `${countLabel(workspaceCount, "workspace", "workspaces")} · ` +
    countLabel(tabCount, "open tab", "open tabs");
  return (
    <PreviewRowShell
      icon="monitor"
      title={desktop.name}
      detail={detail}
      accessibilityLabel={`${desktop.name}, ${PLATFORM_LABEL[desktop.platform]} desktop, ${detail}`}
      onPress={() => onOpen(desktop.id)}
    />
  );
}

export interface ProjectRowProps {
  project: ProjectSummary;
  workspaceCount: number;
  tabCount: number;
  onOpen: (projectId: string) => void;
}

/** A project entry: folder icon, name, and its workspace/tab counts. */
export function ProjectRow({ project, workspaceCount, tabCount, onOpen }: ProjectRowProps) {
  const detail =
    `${countLabel(workspaceCount, "workspace", "workspaces")} · ` +
    countLabel(tabCount, "open tab", "open tabs");
  return (
    <PreviewRowShell
      icon="folder"
      title={project.name}
      detail={detail}
      accessibilityLabel={`${project.name}, ${detail}`}
      onPress={() => onOpen(project.id)}
    />
  );
}

export interface WorkspaceRowProps {
  workspace: WorkspaceSummary;
  tabCount: number;
  onOpen: (workspaceId: string) => void;
}

/**
 * A workspace entry: its own name and branch plus tab count/status. Tapping
 * opens the workspace level — never a tab directly.
 */
export function WorkspaceRow({ workspace, tabCount, onOpen }: WorkspaceRowProps) {
  const statusBadge = WORKSPACE_STATUS[workspace.status];
  const detail = `${workspace.branch} · ${countLabel(tabCount, "open tab", "open tabs")}`;
  return (
    <PreviewRowShell
      icon="branch"
      dot={workspace.status === "working" ? "working" : workspace.status === "review" ? "done" : "idle"}
      title={workspace.name}
      detail={detail}
      accessibilityLabel={`${workspace.name}, branch ${workspace.branch}, ${detail}, ${statusBadge.label}`}
      onPress={() => onOpen(workspace.id)}
      trailing={<StatusBadge label={statusBadge.label} tone={statusBadge.tone} />}
    />
  );
}

export interface WorkspaceTabRowProps {
  tab: PreviewTab;
  context: string;
  status: WorkspaceSummary["status"] | null;
  agentState?: AgentDotState | null | undefined;
  onOpen: (tab: PreviewTab) => void;
}

/** A tab inside one workspace: agent mark + liveness dot, title verbatim, workspace context, and agent/kind labeling. */
export function WorkspaceTabRow({ tab, context, status, agentState, onOpen }: WorkspaceTabRowProps) {
  const kindLabel = tabKindLabel(tab);
  const live = agentState && agentState !== "idle" ? agentStateLabel(agentState) : null;
  const detail = tab.kind === "agent"
    ? `${context} · ${AGENT_LABEL[tab.agent]}${live ? ` · ${live}` : ""}`
    : `${context} · ${kindLabel}${live ? ` · ${live}` : ""}`;
  const statusBadge = status ? WORKSPACE_STATUS[status] : null;
  const mark =
    tab.kind === "agent" ? <AgentMark agent={tab.agent} size={22} />
    : tab.kind === "terminal" ? <TerminalMark size={22} />
    : <BrowserMark size={20} />;
  return (
    <PreviewRowShell
      icon={TAB_KIND_ICON[tab.kind]}
      iconNode={mark}
      dot={agentState}
      title={tab.title}
      detail={detail}
      accessibilityLabel={`${kindLabel}: ${tab.title}, ${detail}${statusBadge ? `, ${statusBadge.label}` : ""}`}
      onPress={() => onOpen(tab)}
      trailing={statusBadge ? <StatusBadge label={statusBadge.label} tone={statusBadge.tone} /> : undefined}
    />
  );
}
