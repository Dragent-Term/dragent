import type { SessionWriters } from "../features/shared/sessionInput";
import { reduceTerminalDelta } from "../terminal/state";
import type { TerminalState } from "../terminal/types";
import { prepareSessionText } from "../transport/frames";
import type { TerminalDelta, WireAgentBlock } from "../transport/protocol";
import { createDefaultTabs, createPreviewFixtures, createPreviewTab, PREVIEW_AGENT_LABELS, previewSurface } from "./fixtures";
import { createMachineFixtures } from "./machineFixtures";
import { selectAutomation, selectCoordination, selectMachineWorkspaces } from "./machineSelectors";
import type { PreviewCoordination, PreviewReview } from "./machineTypes";
import { createPullRequestFixtures } from "./pullRequestFixtures";
import { selectPullRequest, selectPullRequestDraft } from "./pullRequestSelectors";
import type { NewPreviewPullRequest, PreviewPullRequest, PreviewPullRequestDraft } from "./pullRequestTypes";
import type {
  BrowserPage, CreateTabResult, CreateWorkspaceResult, PreviewSnapshot, PreviewTab, PreviewTabDetail,
  PreviewTabInput, ProjectSummary, WorkspaceDetail, WorkspaceSummary,
} from "./types";

export const PREVIEW_LIMITS = Object.freeze({
  workspaces: 20, tabsPerWorkspace: 20, browserHistory: 20, workspaceNameCharacters: 64, branchCharacters: 128,
  coordinationMessages: 40, automationRuns: 20, reviewNotes: 40, noteCharacters: 2000,
  parallelWorkers: 8,
  pullRequests: 40, pullRequestNotes: 40,
});
type PreviewAction = "input" | "command" | "agentQuery" | "stop";
type BrowserTab = Extract<PreviewTabDetail, { kind: "browser" }>;
type AgentTab = Extract<PreviewTabDetail, { kind: "agent" }>;

function freeze<T>(value: T): T {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) freeze(child);
    Object.freeze(value);
  }
  return value;
}
function summary(workspace: WorkspaceDetail): WorkspaceSummary {
  const { id, projectId, name, branch, status, updatedAt } = workspace;
  return { id, projectId, name, branch, status, updatedAt };
}
function tabSummary(tab: PreviewTabDetail): PreviewTab {
  const { id, workspaceId, projectId, title } = tab;
  return tab.kind === "agent" ? { id, workspaceId, projectId, title, kind: "agent", agent: tab.agent }
    : { id, workspaceId, projectId, title, kind: tab.kind };
}
function reduce(state: TerminalState, ...deltas: TerminalDelta[]): TerminalState {
  return deltas.reduce(reduceTerminalDelta, state);
}
function tabInput(input: unknown): PreviewTabInput | null {
  if (!input || typeof input !== "object" || !("kind" in input)) return null;
  if (input.kind === "terminal" || input.kind === "browser") return { kind: input.kind };
  if (input.kind === "agent" && "agent" in input &&
    (input.agent === "dragent" || input.agent === "codex" || input.agent === "opencode")) return { kind: "agent", agent: input.agent };
  return null;
}
function validBranch(branch: string): boolean {
  // Local label validation only; never invokes Git or creates a real branch.
  // oxlint-disable-next-line no-control-regex -- Branch labels exclude Git ref control/spacing characters.
  return branch.length <= PREVIEW_LIMITS.branchCharacters && !/[\u0000-\u0020\u007f-\u009f~^:?*[\\\u2028\u2029]/u.test(branch) &&
    branch !== "@" && !branch.startsWith("-") && !branch.includes("..") && !branch.includes("@{") &&
    branch.split("/").every((part) => part.length > 0 && !part.startsWith(".") && !part.endsWith(".") && !part.endsWith(".lock"));
}
function reviewProse(value: unknown, maxCharacters: number): string | null {
  const prepared = prepareSessionText("agentQuery", value);
  // oxlint-disable-next-line no-control-regex -- Prose permits tabs and newlines, not terminal controls.
  return prepared.ok && prepared.text.length <= maxCharacters && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/u.test(prepared.text) ? prepared.text : null;
}

/** Every selector validates the entire desktop → project → workspace → tab chain. */
export function selectPreviewProject(snapshot: PreviewSnapshot, desktopId: string, projectId: string): ProjectSummary | null {
  if (!snapshot.desktops.some((desktop) => desktop.id === desktopId)) return null;
  return snapshot.projects.find((project) => project.id === projectId && project.desktopId === desktopId) ?? null;
}
export function selectPreviewWorkspace(snapshot: PreviewSnapshot, desktopId: string, projectId: string, workspaceId: string): WorkspaceSummary | null {
  if (!selectPreviewProject(snapshot, desktopId, projectId)) return null;
  return snapshot.workspaces.find((workspace) => workspace.id === workspaceId && workspace.projectId === projectId) ?? null;
}
export function selectPreviewTab(snapshot: PreviewSnapshot, desktopId: string, projectId: string, workspaceId: string, tabId: string): PreviewTab | null {
  if (!selectPreviewWorkspace(snapshot, desktopId, projectId, workspaceId)) return null;
  return snapshot.tabs.find((tab) => tab.id === tabId && tab.workspaceId === workspaceId && tab.projectId === projectId) ?? null;
}

/** Isolated local samples: no shell, provider, network, native APIs or persistence. */
export class PreviewStore {
  private snapshot!: PreviewSnapshot;
  private details: readonly WorkspaceDetail[] = [];
  private tabDetails: readonly PreviewTabDetail[] = [];
  private readonly listeners = new Set<() => void>();
  private readonly writers = new Map<string, SessionWriters>();
  private epoch = 0;
  private workspaceSequence = 0;
  private tabSequence = 0;
  private eventSequence = 100;
  private readonly now: () => number;

  constructor(options: { now?: () => number } = {}) { this.now = options.now ?? Date.now; this.seed(); }
  getSnapshot = (): PreviewSnapshot => this.snapshot;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  getWorkspace = (id: string): WorkspaceDetail | null => this.details.find((workspace) => workspace.id === id) ?? null;
  getTab = (id: string): PreviewTabDetail | null => this.tabDetails.find((tab) => tab.id === id) ?? null;
  getWriters = (tabId: string): SessionWriters | null => {
    const tab = this.getTab(tabId);
    if (!tab || tab.kind === "browser") return null;
    const cached = this.writers.get(tabId);
    if (cached) return cached;
    const epoch = this.epoch;
    const writers: SessionWriters = Object.freeze({
      sendInput: (text: string) => this.write(epoch, tabId, "input", text),
      executeCommand: (text: string) => this.write(epoch, tabId, "command", text),
      sendAgentQuery: (text: string) => this.write(epoch, tabId, "agentQuery", text),
      stopAgent: () => this.write(epoch, tabId, "stop"),
    });
    this.writers.set(tabId, writers);
    return writers;
  };

  createWorkspace = (projectId: string, rawName: string, rawBranch: string): CreateWorkspaceResult => {
    const project = this.snapshot.projects.find((project) => project.id === projectId);
    if (!project || !selectPreviewProject(this.snapshot, project.desktopId, projectId)) return { ok: false, message: "Choose a sample project for this workspace." };
    if (typeof rawName !== "string" || !rawName.trim()) return { ok: false, message: "Enter a workspace name." };
    const name = rawName.trim();
    // oxlint-disable-next-line no-control-regex -- Workspace labels cannot contain control characters.
    if (name.length > PREVIEW_LIMITS.workspaceNameCharacters || /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/u.test(name)) {
      return { ok: false, message: `Use a single-line name with at most ${PREVIEW_LIMITS.workspaceNameCharacters} characters.` };
    }
    if (typeof rawBranch !== "string" || !rawBranch.trim()) return { ok: false, message: "Enter an explicit branch for this workspace." };
    const branch = rawBranch.trim();
    if (!validBranch(branch)) return { ok: false, message: `Use a valid branch name with at most ${PREVIEW_LIMITS.branchCharacters} characters.` };
    if (this.details.some((workspace) => workspace.projectId === projectId && workspace.branch === branch)) return { ok: false, message: "That branch already belongs to a workspace in this sample project." };
    if (this.details.length >= PREVIEW_LIMITS.workspaces) return { ok: false, message: `This preview supports ${PREVIEW_LIMITS.workspaces} workspaces. Reset preview to start again.` };
    const id = `preview-new-${this.epoch}-${++this.workspaceSequence}`;
    const workspace: WorkspaceDetail = {
      id, projectId, name, branch, status: "idle", updatedAt: this.timestamp(),
      files: [{ path: "README.md", content: `# ${name}\n\nA local sample workspace on ${branch}. No project was checked out.\n` }],
      changes: [], tasks: [{ id: "explore", title: "Explore this local workspace", status: "todo" }],
    };
    this.details = freeze([workspace, ...this.details]);
    this.tabDetails = freeze([...this.tabDetails, ...createDefaultTabs(workspace)]);
    this.publish();
    return { ok: true, id };
  };

  createTab = (workspaceId: string, rawInput: PreviewTabInput): CreateTabResult => {
    const workspace = this.getWorkspace(workspaceId);
    if (!workspace) return { ok: false, message: "Choose an existing sample workspace." };
    const input = tabInput(rawInput);
    if (!input) return { ok: false, message: "Choose Terminal, Browser, or a supported agent: Dragent, Codex or OpenCode." };
    const siblings = this.tabDetails.filter((tab) => tab.workspaceId === workspaceId);
    if (siblings.length >= PREVIEW_LIMITS.tabsPerWorkspace) return { ok: false, message: `A preview workspace supports ${PREVIEW_LIMITS.tabsPerWorkspace} tabs. Reset preview to start again.` };
    const ordinal = siblings.filter((tab) => tab.kind === input.kind && (input.kind !== "agent" || (tab.kind === "agent" && tab.agent === input.agent))).length + 1;
    const id = `preview-tab-${this.epoch}-${++this.tabSequence}`;
    this.tabDetails = freeze([...this.tabDetails, createPreviewTab(workspace, id, input, ordinal)]);
    this.touchWorkspace(workspace.id);
    this.publish();
    return { ok: true, id };
  };

  toggleTask = (workspaceId: string, taskId: string): void => {
    const workspace = this.getWorkspace(workspaceId);
    if (!workspace || !workspace.tasks.some((task) => task.id === taskId)) return;
    this.details = freeze(this.details.map((previous) => previous.id === workspaceId ? {
      ...previous, updatedAt: this.timestamp(),
      tasks: previous.tasks.map((task) => task.id === taskId ? { ...task, status: task.status === "done" ? "todo" : "done" } : task),
    } : previous));
    this.publish();
  };

  navigateBrowser = (tabId: string, page: BrowserPage): boolean => {
    const tab = this.browserTab(tabId);
    if (!tab || (page !== "overview" && page !== "activity" && page !== "settings")) return false;
    if (page === tab.browser.page) return true;
    const history = [...tab.browser.history.slice(0, tab.browser.index + 1), page].slice(-PREVIEW_LIMITS.browserHistory);
    return this.replaceTab({ ...tab, browser: { page, history, index: history.length - 1, revision: tab.browser.revision + 1 } });
  };
  goBrowser = (tabId: string, direction: "back" | "forward"): boolean => {
    const tab = this.browserTab(tabId);
    if (!tab || (direction !== "back" && direction !== "forward")) return false;
    const index = tab.browser.index + (direction === "back" ? -1 : 1);
    const page = tab.browser.history[index];
    if (page === undefined) return false;
    return this.replaceTab({ ...tab, browser: { ...tab.browser, page, index, revision: tab.browser.revision + 1 } });
  };
  reloadBrowser = (tabId: string): boolean => {
    const tab = this.browserTab(tabId);
    return tab ? this.replaceTab({ ...tab, browser: { ...tab.browser, revision: tab.browser.revision + 1 } }) : false;
  };

  sendCoordinationMessage = (desktopId: string, id: string, raw: string): boolean => {
    const team = selectCoordination(this.snapshot, desktopId, id);
    const prepared = prepareSessionText("agentQuery", raw);
    if (!team || !prepared.ok || prepared.text.length > PREVIEW_LIMITS.noteCharacters) return false;
    return this.replaceCoordination({ ...team, messages: [...team.messages, {
      id: `message-${this.epoch}-${++this.eventSequence}`, from: "you" as const, body: prepared.text, at: this.timestamp(),
    }].slice(-PREVIEW_LIMITS.coordinationMessages) });
  };
  stopCoordination = (desktopId: string, id: string): boolean => {
    const team = selectCoordination(this.snapshot, desktopId, id);
    if (!team || (team.status !== "running" && team.status !== "planning")) return false;
    return this.replaceCoordination({ ...team, status: "stopped", plan: team.plan.map((task) =>
      task.status === "done" ? task : { ...task, status: "cancelled" }), messages: [...team.messages, {
        id: `stop-${this.epoch}-${++this.eventSequence}`, from: "system" as const, at: this.timestamp(),
        body: "Sample coordination stopped locally. Completed work and workspace tabs remain available.",
      }].slice(-PREVIEW_LIMITS.coordinationMessages) });
  };
  configureCoordination = (desktopId: string, id: string, options: { parallelism?: number; autoStart?: boolean }): boolean => {
    const team = selectCoordination(this.snapshot, desktopId, id);
    if (!team || !options || (team.status !== "running" && team.status !== "planning")) return false;
    const { parallelism = team.parallelism, autoStart = team.autoStart } = options;
    if (!Number.isInteger(parallelism) || parallelism < 1 || parallelism > PREVIEW_LIMITS.parallelWorkers || typeof autoStart !== "boolean") return false;
    return this.replaceCoordination({ ...team, parallelism, autoStart });
  };
  toggleAutomation = (desktopId: string, id: string): boolean => {
    const automation = selectAutomation(this.snapshot, desktopId, id);
    if (!automation) return false;
    this.snapshot = freeze({ ...this.snapshot, automations: this.snapshot.automations.map((entry) =>
      entry.id === id ? { ...entry, enabled: !entry.enabled } : entry) });
    this.notify();
    return true;
  };
  runAutomation = (desktopId: string, id: string): boolean => {
    const automation = selectAutomation(this.snapshot, desktopId, id);
    if (!automation) return false;
    const run = {
      id: `sample-run-${this.epoch}-${++this.eventSequence}`, automationId: id,
      workspaceId: automation.workspaceId, at: this.timestamp(), status: "succeeded" as const,
      output: `Sample result for ${automation.name}. The pinned workspace is available to inspect. No scheduler, shell or model ran.`,
    };
    const ownRuns = [run, ...this.snapshot.automationRuns.filter((entry) => entry.automationId === id)]
      .slice(0, PREVIEW_LIMITS.automationRuns);
    this.snapshot = freeze({ ...this.snapshot, automationRuns: [
      ...ownRuns, ...this.snapshot.automationRuns.filter((entry) => entry.automationId !== id),
    ] });
    this.notify();
    return true;
  };
  setReviewFileReviewed = (desktopId: string, workspaceId: string, path: string, reviewed: boolean): boolean => {
    if (!this.reviewableFile(desktopId, workspaceId, path) || typeof reviewed !== "boolean") return false;
    const review = this.reviewState(workspaceId);
    const paths = new Set(review.reviewedPaths);
    if (reviewed) paths.add(path); else paths.delete(path);
    return this.replaceReview({ ...review, reviewedPaths: [...paths] });
  };
  addReviewNote = (desktopId: string, workspaceId: string, path: string, body: string): boolean => {
    const prepared = prepareSessionText("agentQuery", body);
    if (!this.reviewableFile(desktopId, workspaceId, path) || !prepared.ok || prepared.text.length > PREVIEW_LIMITS.noteCharacters) return false;
    const review = this.reviewState(workspaceId);
    if (review.notes.length >= PREVIEW_LIMITS.reviewNotes) return false;
    return this.replaceReview({ ...review, notes: [...review.notes, {
      id: `note-${this.epoch}-${++this.eventSequence}`, path, body: prepared.text,
    }] });
  };
  /** Opening a tab marks its finished work seen: needs-review drops back to idle. */
  markTabSeen = (tabId: string): boolean => {
    const tab = this.getTab(tabId);
    if (!tab || tab.kind === "browser") return false;
    let latest = 0;
    for (const block of tab.terminal.blocks) latest = Math.max(latest, block.tsMs);
    for (const block of tab.terminal.agentBlocks) latest = Math.max(latest, block.tsMs);
    if ((this.snapshot.tabSeen[tabId] ?? -1) >= latest) return false;
    this.snapshot = freeze({ ...this.snapshot, tabSeen: { ...this.snapshot.tabSeen, [tabId]: latest } });
    this.notify();
    return true;
  };
  markPullRequestFileViewed = (desktopId: string, id: string, headOid: string, path: string, viewed: boolean): boolean => {
    const review = selectPullRequest(this.snapshot, desktopId, id);
    if (!review || review.headOid !== headOid || !review.files.some((file) => file.path === path) || typeof viewed !== "boolean") return false;
    const draft = selectPullRequestDraft(this.snapshot, review);
    const paths = new Set(draft.viewedPaths);
    if (viewed) paths.add(path); else paths.delete(path);
    return this.replacePullRequestDraft({ ...draft, viewedPaths: [...paths] });
  };
  addPullRequestNote = (desktopId: string, id: string, headOid: string, note: {
    path: string | null; lineIndex?: number | null; lineEnd?: number | null; body: string; kind: "comment" | "approve" | "request-changes";
  }): boolean => {
    const review = selectPullRequest(this.snapshot, desktopId, id);
    if (!review || review.headOid !== headOid || !note || !["comment", "approve", "request-changes"].includes(note.kind)) return false;
    const file = review.files.find((entry) => entry.path === note.path);
    if (note.path !== null && !file) return false;
    const lineIndex = note.lineIndex ?? null;
    const lineEnd = note.lineEnd ?? null;
    if (lineIndex !== null && (!file || !Number.isSafeInteger(lineIndex) || lineIndex < 0 || lineIndex >= file.lines.length)) return false;
    if (lineEnd !== null && (!file || lineIndex === null || !Number.isSafeInteger(lineEnd) || lineEnd < 0 || lineEnd >= file.lines.length)) return false;
    const body = reviewProse(note.body, PREVIEW_LIMITS.noteCharacters);
    if (body === null) return false;
    const draft = selectPullRequestDraft(this.snapshot, review);
    if (draft.notes.length >= PREVIEW_LIMITS.pullRequestNotes) return false;
    return this.replacePullRequestDraft({ ...draft, notes: [...draft.notes, {
      id: `review-note-${this.epoch}-${++this.eventSequence}`, path: note.path, lineIndex, lineEnd, kind: note.kind, body,
    }] });
  };
  createPullRequest = (desktopId: string, input: NewPreviewPullRequest): CreateWorkspaceResult => {
    if (!input || !selectPreviewWorkspace(this.snapshot, desktopId, input.projectId, input.workspaceId)) return { ok: false, message: "Choose a source workspace on this machine." };
    const workspace = this.getWorkspace(input.workspaceId)!;
    if (input.provider !== "github" && input.provider !== "gitlab") return { ok: false, message: "Choose GitHub or GitLab." };
    const title = typeof input.title === "string" ? input.title.trim() : "";
    // oxlint-disable-next-line no-control-regex -- Provider titles are single-line labels.
    if (!title || title.length > 120 || /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/u.test(title)) return { ok: false, message: "Use a single-line title with at most 120 characters." };
    const description = typeof input.description === "string" ? input.description.trim() : "";
    if (description.length > 4000 || (description && reviewProse(description, 4000) === null)) return { ok: false, message: "Use a shorter description without control characters." };
    const base = typeof input.baseBranch === "string" ? input.baseBranch.trim() : "";
    if (!base || !validBranch(base) || base === workspace.branch) return { ok: false, message: "Choose a valid target branch different from the source branch." };
    if (this.snapshot.pullRequests.length >= PREVIEW_LIMITS.pullRequests) return { ok: false, message: "This preview supports 40 requests. Reset preview to start again." };
    const id = `pull-request-${this.epoch}-${++this.eventSequence}`;
    const number = Math.max(0, ...this.snapshot.pullRequests.filter((review) => review.projectId === input.projectId && review.provider === input.provider).map((review) => review.number)) + 1;
    const review: PreviewPullRequest = {
      id, desktopId, projectId: input.projectId, workspaceId: workspace.id, provider: input.provider, number,
      title, author: "You", state: "open", isDraft: true, createdByMe: true, reviewRequested: false,
      headBranch: workspace.branch, baseBranch: base, headOid: `sample-${id}`, updatedAt: this.timestamp(), description,
      reviewDecision: "required", conflicts: false, mergeRequirementsSatisfied: false, reviewers: [], assignees: ["You"], labels: [],
      files: workspace.changes.map((change) => ({ ...change, status: "modified" })),
      commits: [], checks: [], activity: [{ id: `${id}-created`, author: "You", kind: "system", body: "Created a local sample request. Nothing was published to a repository provider.", at: this.timestamp() }],
    };
    this.snapshot = freeze({ ...this.snapshot, pullRequests: [review, ...this.snapshot.pullRequests] });
    this.notify();
    return { ok: true, id };
  };
  reset = (): void => { this.epoch++; this.writers.clear(); this.seed(); this.notify(); };

  private replaceCoordination(team: PreviewCoordination): boolean {
    this.snapshot = freeze({ ...this.snapshot, coordinations: this.snapshot.coordinations.map((entry) => entry.id === team.id ? team : entry) });
    this.notify();
    return true;
  }
  private replacePullRequestDraft(draft: PreviewPullRequestDraft): boolean {
    this.snapshot = freeze({ ...this.snapshot, pullRequestDrafts: [
      ...this.snapshot.pullRequestDrafts.filter((entry) => entry.reviewId !== draft.reviewId), draft,
    ] });
    this.notify();
    return true;
  }
  private reviewableFile(desktopId: string, workspaceId: string, path: string): boolean {
    return selectMachineWorkspaces(this.snapshot, desktopId).some((workspace) => workspace.id === workspaceId) &&
      (this.getWorkspace(workspaceId)?.changes.some((change) => change.path === path) ?? false);
  }
  private reviewState(workspaceId: string): PreviewReview {
    return this.snapshot.reviews.find((entry) => entry.workspaceId === workspaceId) ?? { workspaceId, reviewedPaths: [], notes: [] };
  }
  private replaceReview(review: PreviewReview): boolean {
    this.snapshot = freeze({ ...this.snapshot, reviews: [...this.snapshot.reviews.filter((entry) => entry.workspaceId !== review.workspaceId), review] });
    this.notify();
    return true;
  }

  private seed(): void {
    const fixtures = createPreviewFixtures();
    this.details = freeze(fixtures.details);
    this.tabDetails = freeze(fixtures.tabDetails);
    this.eventSequence = 100;
    this.snapshot = freeze({ ...createMachineFixtures(), pullRequests: createPullRequestFixtures(), pullRequestDrafts: [], tabSeen: {}, desktops: fixtures.desktops, projects: fixtures.projects, workspaces: this.details.map(summary), tabs: this.tabDetails.map(tabSummary) });
  }
  private timestamp(): number {
    const time = this.now();
    return Number.isSafeInteger(time) && time >= 0 && Number.isFinite(new Date(time).getTime()) ? time : 0;
  }
  private touchWorkspace(id: string): void {
    this.details = freeze(this.details.map((workspace) => workspace.id === id ? { ...workspace, updatedAt: this.timestamp() } : workspace));
  }
  private browserTab(id: string): BrowserTab | null {
    const tab = this.getTab(id);
    return tab?.kind === "browser" ? tab : null;
  }
  private replaceTab(tab: PreviewTabDetail): boolean {
    this.tabDetails = freeze(this.tabDetails.map((previous) => previous.id === tab.id ? tab : previous));
    this.touchWorkspace(tab.workspaceId);
    this.publish();
    return true;
  }
  private publish(): void {
    this.snapshot = freeze({ ...this.snapshot, workspaces: this.details.map(summary), tabs: this.tabDetails.map(tabSummary) });
    this.notify();
  }
  private notify(): void { for (const listener of Array.from(this.listeners)) listener(); }

  private write(epoch: number, id: string, action: PreviewAction, raw?: string): boolean {
    if (epoch !== this.epoch) return false;
    const tab = this.getTab(id);
    if (!tab || tab.kind === "browser") return false;
    if ((tab.kind === "agent") !== (action === "agentQuery" || action === "stop")) return false;
    const workspace = this.getWorkspace(tab.workspaceId);
    if (!workspace || workspace.projectId !== tab.projectId) return false;
    const time = this.timestamp();
    if (action === "stop" && tab.kind === "agent") return this.replaceTab({ ...tab, terminal: this.agent(tab, [{
      kind: "system_note", id: ++this.eventSequence, ts_ms: time, text: `[Preview] ${PREVIEW_AGENT_LABELS[tab.agent]} stopped locally; no remote agent is running.`,
    }], "Preview stopped") });
    const prepared = prepareSessionText(action === "stop" ? "agentQuery" : action, raw);
    if (!prepared.ok) return false;
    if (tab.kind === "agent") return this.replaceTab({ ...tab, terminal: this.agent(tab, [
      { kind: "user_query", id: ++this.eventSequence, ts_ms: time, text: prepared.text },
      { kind: "agent_message", id: ++this.eventSequence, ts_ms: time,
        text: `[Preview] ${PREVIEW_AGENT_LABELS[tab.agent]} fixed local reply: ${workspace.name} has ${workspace.files.length} sample files, ${workspace.changes.length} changes and ${workspace.tasks.length} tasks. No model was contacted.`,
      },
    ], "Preview reply ready") });
    const blockId = `preview-action-${++this.eventSequence}`;
    const input = action === "input";
    return this.replaceTab({ ...tab, terminal: reduce(tab.terminal,
      { kind: "block_started", block_id: blockId, ts_ms: time, command: input ? "[Preview input] Keystrokes recorded locally" : `[Preview command] ${prepared.text}` },
      { kind: "block_lines", block_id: blockId, base_line: 0, lines: [
        { spans: [{ text: input ? `[Preview] ${prepared.byteLength} UTF-8 input bytes recorded:` : "[Preview] Command recorded as text only:" }] },
        { spans: [{ text: JSON.stringify(prepared.text) }] },
        { spans: [{ text: "[Preview] No shell was executed and no input was sent to a desktop." }] },
      ] },
      { kind: "block_completed", block_id: blockId, exit_code: null, failed: false },
    ) });
  }
  private agent(tab: AgentTab, additions: WireAgentBlock[], status: string): TerminalState {
    const previous: WireAgentBlock[] = tab.terminal.agentBlocks.map((block) => ({
      id: block.id, ts_ms: block.tsMs, text: block.text,
      kind: block.kind === "user_query" ? "user_query" : block.kind === "system_note" ? "system_note" : "agent_message",
    }));
    return reduce(tab.terminal,
      { kind: "agent_blocks", blocks: [...previous, ...additions] },
      { kind: "surface_state", state: { ...previewSurface(tab.terminal.surface?.cwd ?? `/preview/${tab.projectId}/${tab.workspaceId}`, status), ai_tool: PREVIEW_AGENT_LABELS[tab.agent] } },
    );
  }
}
