import type { SessionWriters } from "../features/shared/sessionInput";
import type { TerminalState } from "../terminal/types";
import type { PreviewAutomation, PreviewAutomationRun, PreviewCoordination, PreviewReview } from "./machineTypes";
import type { NewPreviewPullRequest, PreviewPullRequest, PreviewPullRequestDraft } from "./pullRequestTypes";
export type { PreviewAutomation, PreviewAutomationRun, PreviewCoordination, PreviewReview } from "./machineTypes";

export interface DesktopSummary {
  id: string;
  name: string;
  description: string;
  platform: "macos" | "linux";
}

export interface ProjectSummary {
  id: string;
  desktopId: string;
  name: string;
  description?: string;
}

export type PreviewAgent = "dragent" | "codex" | "opencode";
export type BrowserPage = "overview" | "activity" | "settings";
export interface PreviewBrowserState {
  page: BrowserPage;
  history: readonly BrowserPage[];
  index: number;
  revision: number;
}

interface PreviewTabBase {
  id: string;
  workspaceId: string;
  projectId: string;
  title: string;
}

export type PreviewTabInput = { kind: "terminal" } | { kind: "agent"; agent: PreviewAgent } | { kind: "browser" };
export type PreviewTab = PreviewTabBase & PreviewTabInput;
export type PreviewTabDetail =
  | (PreviewTabBase & { kind: "terminal"; terminal: TerminalState })
  | (PreviewTabBase & { kind: "agent"; agent: PreviewAgent; terminal: TerminalState })
  | (PreviewTabBase & { kind: "browser"; browser: PreviewBrowserState });

export interface WorkspaceSummary {
  id: string;
  projectId: string;
  name: string;
  branch: string;
  status: "working" | "review" | "idle";
  updatedAt: number;
}

export interface WorkspaceFile {
  path: string;
  content: string;
}

export interface WorkspaceChange {
  path: string;
  added: number;
  removed: number;
  lines: readonly { kind: "context" | "add" | "remove"; text: string }[];
}

export interface WorkspaceTask {
  id: string;
  title: string;
  status: "todo" | "doing" | "done";
}

export interface WorkspaceDetail extends WorkspaceSummary {
  files: readonly WorkspaceFile[];
  changes: readonly WorkspaceChange[];
  tasks: readonly WorkspaceTask[];
}

/** Serializable store snapshot; writers and actions travel separately. */
export interface PreviewSnapshot {
  desktops: readonly DesktopSummary[];
  projects: readonly ProjectSummary[];
  workspaces: readonly WorkspaceSummary[];
  tabs: readonly PreviewTab[];
  coordinations: readonly PreviewCoordination[];
  automations: readonly PreviewAutomation[];
  automationRuns: readonly PreviewAutomationRun[];
  reviews: readonly PreviewReview[];
  pullRequests: readonly PreviewPullRequest[];
  pullRequestDrafts: readonly PreviewPullRequestDraft[];
  /** Latest per-tab activity timestamp the user has opened; unseen finishes read as needing review. */
  tabSeen: Record<string, number>;
}

export type CreateWorkspaceResult = { ok: true; id: string } | { ok: false; message: string };
export type CreateTabResult = CreateWorkspaceResult;

/** What usePreview() exposes: the snapshot plus local-only actions. */
export interface PreviewValue extends PreviewSnapshot {
  getWorkspace(id: string): WorkspaceDetail | null;
  getTab(id: string): PreviewTabDetail | null;
  getWriters(tabId: string): SessionWriters | null;
  createWorkspace(projectId: string, name: string, branch: string): CreateWorkspaceResult;
  createTab(workspaceId: string, input: PreviewTabInput): CreateTabResult;
  toggleTask(workspaceId: string, taskId: string): void;
  reset(): void;
  navigateBrowser(tabId: string, page: BrowserPage): boolean;
  goBrowser(tabId: string, direction: "back" | "forward"): boolean;
  reloadBrowser(tabId: string): boolean;
  sendCoordinationMessage(desktopId: string, id: string, text: string): boolean;
  stopCoordination(desktopId: string, id: string): boolean;
  configureCoordination(desktopId: string, id: string, options: { parallelism?: number; autoStart?: boolean }): boolean;
  toggleAutomation(desktopId: string, id: string): boolean;
  runAutomation(desktopId: string, id: string): boolean;
  setReviewFileReviewed(desktopId: string, workspaceId: string, path: string, reviewed: boolean): boolean;
  addReviewNote(desktopId: string, workspaceId: string, path: string, body: string): boolean;
  markPullRequestFileViewed(desktopId: string, id: string, headOid: string, path: string, viewed: boolean): boolean;
  /** Record the tab as opened: its finished work drops from needs-review back to idle. */
  markTabSeen(tabId: string): boolean;
  addPullRequestNote(desktopId: string, id: string, headOid: string, note: {
    path: string | null; lineIndex?: number | null; lineEnd?: number | null; body: string; kind: "comment" | "approve" | "request-changes";
  }): boolean;
  createPullRequest(desktopId: string, input: NewPreviewPullRequest): CreateWorkspaceResult;
}
