import type { PreviewSnapshot, PreviewTab, WorkspaceSummary } from "./types";
import type { PreviewAutomation, PreviewCoordination } from "./machineTypes";

export function selectMachineWorkspaces(snapshot: PreviewSnapshot, desktopId: string): readonly WorkspaceSummary[] {
  if (!snapshot.desktops.some((desktop) => desktop.id === desktopId)) return [];
  const projects = new Set(snapshot.projects.filter((project) => project.desktopId === desktopId).map((project) => project.id));
  return snapshot.workspaces.filter((workspace) => projects.has(workspace.projectId));
}

export function selectMachineTab(snapshot: PreviewSnapshot, desktopId: string, tabId: string): PreviewTab | null {
  const tab = snapshot.tabs.find((entry) => entry.id === tabId);
  return tab && selectMachineWorkspaces(snapshot, desktopId).some((workspace) =>
    workspace.id === tab.workspaceId && workspace.projectId === tab.projectId) ? tab : null;
}

export function selectCoordination(snapshot: PreviewSnapshot, desktopId: string, id: string): PreviewCoordination | null {
  const team = snapshot.coordinations.find((entry) => entry.id === id && entry.desktopId === desktopId);
  if (!team || !snapshot.projects.some((project) => project.id === team.projectId && project.desktopId === desktopId)) return null;
  const lead = selectMachineTab(snapshot, desktopId, team.leadTabId);
  if (!lead || lead.kind !== "agent" || lead.projectId !== team.projectId) return null;
  return team.plan.every((task) => {
    if (task.tabId === null) return true;
    const worker = selectMachineTab(snapshot, desktopId, task.tabId);
    return worker?.kind === "agent" && worker.projectId === team.projectId;
  }) ? team : null;
}

export function selectAutomation(snapshot: PreviewSnapshot, desktopId: string, id: string): PreviewAutomation | null {
  const automation = snapshot.automations.find((entry) => entry.id === id && entry.desktopId === desktopId);
  if (!automation) return null;
  const agent = selectMachineTab(snapshot, desktopId, automation.agentTabId);
  return agent?.kind === "agent" && agent.workspaceId === automation.workspaceId && agent.projectId === automation.projectId ? automation : null;
}
