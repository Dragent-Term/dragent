/** Machine-wide features link to project/workspace tabs; they are not tab kinds. */
export interface PreviewCoordination {
  id: string;
  desktopId: string;
  projectId: string;
  title: string;
  goal: string;
  status: "planning" | "running" | "done" | "stopped";
  leadTabId: string;
  parallelism: number;
  autoStart: boolean;
  plan: readonly {
    id: string;
    title: string;
    status: "pending" | "working" | "done" | "blocked" | "cancelled";
    dependsOn: readonly string[];
    tabId: string | null;
  }[];
  messages: readonly { id: string; from: "lead" | "you" | "system"; body: string; at: number }[];
}

export interface PreviewAutomation {
  id: string;
  desktopId: string;
  projectId: string;
  workspaceId: string;
  agentTabId: string;
  name: string;
  prompt: string;
  schedule: string;
  timeZone: string;
  enabled: boolean;
  workspaceMode: "pinned";
}

export interface PreviewAutomationRun {
  id: string;
  automationId: string;
  workspaceId: string;
  at: number;
  status: "succeeded";
  output: string;
}

export interface PreviewReview {
  workspaceId: string;
  reviewedPaths: readonly string[];
  notes: readonly { id: string; path: string; body: string }[];
}
