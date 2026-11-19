import { createTerminalState, reduceTerminalDelta } from "../terminal/state";
import type { TerminalDelta, TerminalSurfaceState, WireAgentBlock } from "../transport/protocol";
import type { PreviewAgent, PreviewSnapshot, PreviewTabDetail, PreviewTabInput, WorkspaceDetail, WorkspaceFile } from "./types";

const SEED_TIME = Date.UTC(2026, 0, 15, 10);

export function previewSurface(cwd: string, aiStatus = "Preview ready"): TerminalSurfaceState {
  return {
    command_input: "", command_cursor: 0, ai_input: "", ai_cursor: 0,
    attachments: [], command_running: false, cwd, ai_status: aiStatus,
    activity: "Local preview · no desktop connection",
  };
}

export function initialPreviewTerminal(name: string, cwd: string, time: number, files: readonly WorkspaceFile[]) {
  const deltas: TerminalDelta[] = [
    { kind: "surface_state", state: previewSurface(cwd) },
    { kind: "block_started", block_id: "preview-welcome", command: "pwd && ls", ts_ms: time },
    { kind: "block_lines", block_id: "preview-welcome", base_line: 0,
      lines: [cwd, ...files.slice(0, 3).map((file) => file.path), `${name} · ready`]
        .map((text) => ({ spans: [{ text }] })),
    },
    { kind: "block_completed", block_id: "preview-welcome", exit_code: null, failed: false },
  ];
  return deltas.reduce(reduceTerminalDelta, createTerminalState());
}

export const PREVIEW_AGENT_LABELS: Readonly<Record<PreviewAgent, string>> = Object.freeze({
  dragent: "Dragent", codex: "Codex", opencode: "OpenCode",
});

/** Allocate fresh state for one independently owned tab. */
export function createPreviewTab(workspace: WorkspaceDetail, id: string, input: PreviewTabInput, ordinal = 1): PreviewTabDetail {
  const name = input.kind === "agent" ? PREVIEW_AGENT_LABELS[input.agent] : input.kind === "browser" ? "Browser" : "Terminal";
  const base = { id, workspaceId: workspace.id, projectId: workspace.projectId, title: ordinal === 1 ? name : `${name} ${ordinal}` };
  if (input.kind === "browser") return { ...base, kind: "browser", browser: { page: "overview", history: ["overview"], index: 0, revision: 0 } };
  const cwd = `/preview/${workspace.projectId}/${workspace.id}`;
  if (input.kind === "terminal") return {
    ...base, kind: "terminal", terminal: initialPreviewTerminal(workspace.name, cwd, workspace.updatedAt, workspace.files),
  };
  // One offline working sample: the first workspace's Dragent tab reads as
  // working (thinking/tool/update rows plus a latest unanswered request), so
  // the conversation feed and the working dot can be eyeballed without a
  // desktop. Every other agent tab keeps the single settled reply.
  if (input.agent === "dragent" && workspace.id === "preview-navigation" && ordinal === 1) {
    return { ...base, kind: "agent", agent: input.agent, terminal: dragentWorkingTerminal(cwd, workspace) };
  }
  const deltas: TerminalDelta[] = [
      { kind: "surface_state", state: { ...previewSurface(cwd), ai_tool: name } },
      { kind: "agent_blocks", blocks: [{ kind: "agent_message", id: 1, ts_ms: workspace.updatedAt,
        text: `[Preview · ${name}] ${workspace.name} has sample files, changes and tasks to explore. Replies stay local.` }] },
    ];
  return {
    ...base, kind: "agent", agent: input.agent,
    terminal: deltas.reduce(reduceTerminalDelta, createTerminalState()),
  };
}

/**
 * Offline Dragent-working sample: thinking/tool/update rows with the latest
 * request still open, mirroring the live relay feed (oldest-first, newest
 * last). The trailing `user_query` keeps `previewTabAgentState` on working
 * even after the tab is opened, so the sample stays demonstrably busy.
 */
function dragentWorkingTerminal(cwd: string, workspace: WorkspaceDetail) {
  const at = workspace.updatedAt;
  const blocks: WireAgentBlock[] = [
    { kind: "thinking", id: 1, ts_ms: at - 3000, content: "Considering how the sample workspace is organized…", complete: true },
    { kind: "tool_step", id: 2, ts_ms: at - 2000, name: "edit", icon: "edit", args: "src/navigation.ts", status: "done", result: "Updated the workspace navigation." },
    { kind: "agent_message", id: 3, ts_ms: at - 1000,
      text: `[Preview · Dragent] ${workspace.name} has sample files, changes and tasks to explore. Replies stay local.` },
    { kind: "user_query", id: 4, ts_ms: at, text: "Verify navigation in a narrow window" },
  ];
  const deltas: TerminalDelta[] = [
    { kind: "surface_state", state: { ...previewSurface(cwd, "Working…"), ai_tool: "Dragent" } },
    { kind: "agent_blocks", blocks },
  ];
  return deltas.reduce(reduceTerminalDelta, createTerminalState());
}

export function createDefaultTabs(workspace: WorkspaceDetail): PreviewTabDetail[] {
  return [
    createPreviewTab(workspace, `${workspace.id}-agent`, { kind: "agent", agent: "dragent" }),
    createPreviewTab(workspace, `${workspace.id}-terminal`, { kind: "terminal" }),
    createPreviewTab(workspace, `${workspace.id}-browser`, { kind: "browser" }),
  ];
}

/** Fresh original sample data on every call; never reads a real project. */
export function createPreviewFixtures(): Pick<PreviewSnapshot, "desktops" | "projects"> & { details: WorkspaceDetail[]; tabDetails: PreviewTabDetail[] } {
  const projects = [
    { id: "lumen-web", desktopId: "preview-mac", name: "Lumen web", description: "A sample application with thoughtful navigation and empty states." },
    { id: "relay-api", desktopId: "preview-linux", name: "Relay API", description: "A small example service with a predictable retry policy." },
    { id: "field-notes", desktopId: "preview-mac", name: "Field notes", description: "A short, practical documentation project." },
  ];
  const seeds: Omit<WorkspaceDetail, "updatedAt">[] = [
    {
      id: "preview-navigation", projectId: "lumen-web", name: "Workspace navigation", branch: "feat/workspace-navigation", status: "working",
      files: [
        { path: "src/navigation.ts", content: 'export const sections = ["Overview", "Workspaces", "Files"] as const;\n' },
        { path: "README.md", content: "# Lumen web\n\nThis is an original local preview project. Explore its navigation and sample changes.\n" },
      ],
      changes: [{ path: "src/navigation.ts", added: 1, removed: 1, lines: [
        { kind: "remove", text: 'export const sections = ["Overview", "Files"] as const;' },
        { kind: "add", text: 'export const sections = ["Overview", "Workspaces", "Files"] as const;' },
      ] }],
      tasks: [
        { id: "narrow-layout", title: "Verify navigation in a narrow window", status: "doing" },
        { id: "keyboard-focus", title: "Check keyboard focus order", status: "todo" },
        { id: "section-copy", title: "Choose clear section names", status: "done" },
      ],
    },
    {
      id: "preview-empty-states", projectId: "lumen-web", name: "Review empty states", branch: "fix/empty-states", status: "review",
      files: [
        { path: "src/empty-state.ts", content: 'export const emptyWorkspace = {\n  title: "Your next workspace starts here",\n  action: "Create workspace",\n};\n' },
        { path: "notes/review.md", content: "# Review notes\n\nKeep the message short. Give the reader one clear next action.\n" },
      ],
      changes: [{ path: "src/empty-state.ts", added: 2, removed: 2, lines: [
        { kind: "context", text: "export const emptyWorkspace = {" },
        { kind: "remove", text: '  title: "Nothing found",' },
        { kind: "remove", text: '  action: "Add",' },
        { kind: "add", text: '  title: "Your next workspace starts here",' },
        { kind: "add", text: '  action: "Create workspace",' },
        { kind: "context", text: "};" },
      ] }],
      tasks: [
        { id: "copy-review", title: "Review the empty-state copy", status: "doing" },
        { id: "first-action", title: "Verify the first action has a clear label", status: "done" },
      ],
    },
    {
      id: "preview-retries", projectId: "relay-api", name: "Sync retry policy", branch: "fix/retry-window", status: "working",
      files: [
        { path: "src/retry.ts", content: "export const retryDelaysMs = [500, 1000, 2000] as const;\nexport const retryLimit = retryDelaysMs.length;\n" },
        { path: "README.md", content: "# Relay API\n\nExample service configuration. This preview does not start a service or send requests.\n" },
      ],
      changes: [{ path: "src/retry.ts", added: 1, removed: 1, lines: [
        { kind: "remove", text: "export const retryDelaysMs = [500] as const;" },
        { kind: "add", text: "export const retryDelaysMs = [500, 1000, 2000] as const;" },
        { kind: "context", text: "export const retryLimit = retryDelaysMs.length;" },
      ] }],
      tasks: [
        { id: "bounded-retries", title: "Keep retry attempts bounded", status: "done" },
        { id: "retry-notes", title: "Explain the retry window", status: "todo" },
      ],
    },
    {
      id: "preview-quick-start", projectId: "field-notes", name: "Quick start", branch: "docs/quick-start", status: "idle",
      files: [
        { path: "docs/start.md", content: "# Quick start\n\n1. Choose a project.\n2. Open a workspace.\n3. Review a file, a change, and a task.\n\nThese are local sample steps, not desktop setup instructions.\n" },
        { path: "README.md", content: "# Field notes\n\nShort documentation for exploring this local preview.\n" },
      ],
      changes: [{ path: "docs/start.md", added: 1, removed: 0, lines: [
        { kind: "context", text: "2. Open a workspace." },
        { kind: "add", text: "3. Review a file, a change, and a task." },
      ] }],
      tasks: [
        { id: "outline", title: "Write a three-step outline", status: "done" },
        { id: "proofread", title: "Proofread the quick start", status: "todo" },
      ],
    },
  ];
  const details = seeds.map((seed, index) => {
    const updatedAt = SEED_TIME - index * 15 * 60_000;
    return { ...seed, updatedAt };
  });
  const tabDetails = details.flatMap((workspace) => {
    const defaults = createDefaultTabs(workspace);
    if (workspace.id !== "preview-navigation") return defaults;
    return [defaults[0]!, defaults[1]!,
      createPreviewTab(workspace, `${workspace.id}-terminal-2`, { kind: "terminal" }, 2),
      createPreviewTab(workspace, `${workspace.id}-codex`, { kind: "agent", agent: "codex" }), defaults[2]!,
    ];
  });
  return {
    desktops: [
      { id: "preview-mac", name: "Northstar Mac", description: "Local preview · sample macOS desktop. No desktop is connected.", platform: "macos" },
      { id: "preview-linux", name: "Relay Linux", description: "Local preview · sample Linux desktop. No desktop is connected.", platform: "linux" },
    ],
    projects, details, tabDetails,
  };
}
