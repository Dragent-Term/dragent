import assert from "node:assert/strict";
import { test } from "node:test";
import { PreviewStore, PREVIEW_LIMITS, selectPreviewProject, selectPreviewWorkspace, selectPreviewTab } from "../src/preview/PreviewStore";
import { previewTabAgentState } from "../src/preview/agentStatus";
import type { BrowserPage, PreviewSnapshot, PreviewTabInput } from "../src/preview/types";
import { TERMINAL_LIMITS } from "../src/terminal/state";
import { MAX_WRITE_BYTES, prepareSessionText } from "../src/transport/frames";

const workspaceId = "preview-navigation";
const terminalId = `${workspaceId}-terminal`;
const agentId = `${workspaceId}-agent`;
const browserId = `${workspaceId}-browser`;
function terminal(store: PreviewStore, id = terminalId) {
  const tab = store.getTab(id);
  assert.ok(tab && tab.kind !== "browser");
  return tab.terminal;
}
function browser(store: PreviewStore, id = browserId) {
  const tab = store.getTab(id);
  assert.ok(tab?.kind === "browser");
  return tab.browser;
}
function assertHierarchy(snapshot: PreviewSnapshot) {
  for (const items of [snapshot.desktops, snapshot.projects, snapshot.workspaces, snapshot.tabs]) {
    assert.equal(new Set(items.map((item) => item.id)).size, items.length);
  }
  for (const project of snapshot.projects) assert.ok(snapshot.desktops.some((desktop) => desktop.id === project.desktopId));
  for (const workspace of snapshot.workspaces) {
    const project = snapshot.projects.find((project) => project.id === workspace.projectId)!;
    assert.ok(project);
    assert.ok(selectPreviewWorkspace(snapshot, project.desktopId, project.id, workspace.id));
    const tabs = snapshot.tabs.filter((tab) => tab.workspaceId === workspace.id);
    assert.ok(tabs.length >= 3 && tabs.length <= PREVIEW_LIMITS.tabsPerWorkspace);
    assert.ok(tabs.some((tab) => tab.kind === "agent" && tab.agent === "dragent"));
    assert.ok(tabs.some((tab) => tab.kind === "terminal"));
    assert.ok(tabs.some((tab) => tab.kind === "browser"));
    assert.ok(tabs.every((tab) => tab.projectId === workspace.projectId));
  }
  for (const tab of snapshot.tabs) {
    assert.ok(snapshot.workspaces.some((workspace) => workspace.id === tab.workspaceId && workspace.projectId === tab.projectId));
    assert.equal("terminal" in tab || "browser" in tab, false, "Summaries must not contain mutable surface state");
  }
}

test("snapshots are stable, deeply immutable and isolated between providers", () => {
  const store = new PreviewStore({ now: () => 1000 });
  const other = new PreviewStore();
  const before = store.getSnapshot();
  assert.equal(before, store.getSnapshot());
  assert.equal(before.projects.length, 3);
  assert.equal(before.workspaces.length, 4);
  assert.equal(before.desktops.length, 2);
  assert.equal(before.tabs.length, 14);
  assert.ok(Object.isFrozen(before.workspaces));
  const detail = store.getWorkspace(workspaceId)!;
  assert.equal("terminal" in detail, false);
  assert.notEqual(detail, other.getWorkspace(workspaceId));
  assert.notEqual(terminal(store), terminal(other));
  assert.throws(() => { (detail.files as unknown[]).push({}); }, TypeError);
  assert.ok(Object.isFrozen(terminal(store).blocks[0]));
  assert.ok(Object.isFrozen(browser(store).history));
  let notifications = 0;
  const unsubscribe = store.subscribe(() => { notifications++; });
  store.toggleTask("missing", "missing");
  store.toggleTask(workspaceId, "missing");
  assert.equal(store.getSnapshot(), before);
  assert.equal(notifications, 0);
  store.toggleTask(workspaceId, detail.tasks[0]!.id);
  assert.equal(notifications, 1);
  assert.equal(store.getWorkspace(workspaceId)!.tasks[0]!.status, "done");
  assert.equal(detail.tasks[0]!.status, "doing");
  assert.equal(other.getWorkspace(workspaceId)!.tasks[0]!.status, "doing");
  unsubscribe();
  store.toggleTask(workspaceId, detail.tasks[0]!.id);
  assert.equal(store.getWorkspace(workspaceId)!.tasks[0]!.status, "todo");
  assert.equal(notifications, 1);
});

test("original fixture identities, branches and desktop ownership are preserved", () => {
  const store = new PreviewStore();
  const snapshot = store.getSnapshot();
  assert.equal("desktop" in snapshot, false);
  assert.deepEqual(snapshot.desktops.map((desktop) => [desktop.id, desktop.platform]), [["preview-mac", "macos"], ["preview-linux", "linux"]]);
  assert.deepEqual(snapshot.projects.map((project) => [project.id, project.desktopId]), [
    ["lumen-web", "preview-mac"], ["relay-api", "preview-linux"], ["field-notes", "preview-mac"],
  ]);
  assert.deepEqual(snapshot.workspaces.map((workspace) => [workspace.id, workspace.branch]), [
    ["preview-navigation", "feat/workspace-navigation"], ["preview-empty-states", "fix/empty-states"],
    ["preview-retries", "fix/retry-window"], ["preview-quick-start", "docs/quick-start"],
  ]);
  assertHierarchy(snapshot);
  const primary = snapshot.tabs.filter((tab) => tab.workspaceId === workspaceId);
  assert.equal(primary.length, 5);
  assert.equal(primary.filter((tab) => tab.kind === "terminal").length, 2);
  assert.deepEqual(primary.filter((tab) => tab.kind === "agent").map((tab) => tab.agent), ["dragent", "codex"]);
  for (const workspace of snapshot.workspaces) {
    assert.equal(store.getTab(`${workspace.id}-terminal`)!.kind, "terminal");
    const agent = store.getTab(`${workspace.id}-agent`)!;
    assert.ok(agent.kind === "agent" && agent.agent === "dragent");
    const detail = store.getWorkspace(workspace.id)!;
    assert.ok(detail.files.length > 0 && detail.tasks.length > 0);
    assert.equal(new Set(detail.files.map((file) => file.path)).size, detail.files.length);
    for (const change of detail.changes) {
      assert.ok(detail.files.some((file) => file.path === change.path));
      assert.equal(change.added, change.lines.filter((line) => line.kind === "add").length);
      assert.equal(change.removed, change.lines.filter((line) => line.kind === "remove").length);
    }
    const state = terminal(store, `${workspace.id}-terminal`);
    assert.equal(state.blocks[0]!.command, "pwd && ls");
    assert.ok(state.blocks[0]!.lines.length >= 3 && state.blocks[0]!.lines.length <= 6);
    assert.equal(state.blocks[0]!.lines[0]!.spans[0]!.text, `/preview/${workspace.projectId}/${workspace.id}`);
    assert.match(state.surface?.activity ?? "", /Local preview/);
    assert.equal(state.agentBlocks.length, 0);
    assert.equal(terminal(store, agent.id).blocks.length, 0);
  }
});

test("workspace creation requires an explicit valid branch and publishes three independent tabs atomically", () => {
  const store = new PreviewStore({ now: () => 2000 });
  const before = store.getSnapshot();
  let notifications = 0;
  store.subscribe(() => { notifications++; assertHierarchy(store.getSnapshot()); });
  for (const [project, name, branch] of [
    ["unknown", "Example", "feature/example"], ["lumen-web", "   ", "feature/example"],
    ["lumen-web", "a\nb", "feature/example"], ["lumen-web", "x".repeat(65), "feature/example"],
  ]) assert.equal(store.createWorkspace(project!, name!, branch!).ok, false);
  for (const name of [null, {}, "\u0085", "bad\u0000name", "bad\u2028name", "bad\u2029name"]) {
    assert.equal(store.createWorkspace("lumen-web", name as never, "feature/example").ok, false);
  }
  for (const branch of [undefined, null, {}, "", " ", "bad branch", "a..b", "/bad", "bad/", "a//b", "@", "-bad", "a@{b", ".hidden", "bad.lock", "a\\b", "a?b", "x".repeat(129)]) {
    const result = store.createWorkspace("lumen-web", "Example", branch as never);
    assert.equal(result.ok, false, String(branch));
    if (!result.ok) assert.ok(result.message.length > 0);
  }
  assert.equal(store.createWorkspace("lumen-web", "Duplicate", "feat/workspace-navigation").ok, false);
  assert.equal(store.getSnapshot(), before);
  assert.equal(notifications, 0);
  const created = store.createWorkspace("lumen-web", "  Human label  ", "  feature/explicit-branch  ");
  assert.ok(created.ok);
  assert.equal(notifications, 1);
  const workspace = store.getWorkspace(created.id)!;
  assert.equal(workspace.name, "Human label");
  assert.equal(workspace.branch, "feature/explicit-branch");
  assert.equal(workspace.updatedAt, 2000);
  const tabs = store.getSnapshot().tabs.filter((tab) => tab.workspaceId === created.id);
  assert.deepEqual(tabs.map((tab) => [tab.id, tab.title, tab.kind]), [
    [`${created.id}-agent`, "Dragent", "agent"], [`${created.id}-terminal`, "Terminal", "terminal"], [`${created.id}-browser`, "Browser", "browser"],
  ]);
  assert.notEqual(terminal(store, tabs[0]!.id), terminal(store, tabs[1]!.id));
  assert.ok(store.createWorkspace("relay-api", "Other project", "feature/explicit-branch").ok);
  for (let i = store.getSnapshot().workspaces.length; i < PREVIEW_LIMITS.workspaces; i++) {
    assert.ok(store.createWorkspace("relay-api", `Example ${i}`, `feature/example-${i}`).ok);
  }
  assert.equal(store.getSnapshot().workspaces.length, 20);
  const full = store.getSnapshot();
  assert.equal(store.createWorkspace("relay-api", "Too many", "feature/overflow").ok, false);
  assert.equal(store.getSnapshot(), full);
});

test("independently created tabs have explicit titles, unique IDs and per-workspace bounds", () => {
  const store = new PreviewStore();
  const before = store.getSnapshot();
  for (const input of [null, {}, { kind: "other" }, { kind: "agent" }, { kind: "agent", agent: "unknown" }]) {
    assert.equal(store.createTab(workspaceId, input as never).ok, false);
  }
  assert.equal(store.createTab("lumen-web", { kind: "terminal" }).ok, false);
  assert.equal(store.createTab("missing", { kind: "terminal" }).ok, false);
  assert.equal(store.getSnapshot(), before);
  const inputs: PreviewTabInput[] = [{ kind: "terminal" }, { kind: "agent", agent: "dragent" }, { kind: "agent", agent: "codex" }, { kind: "agent", agent: "opencode" }, { kind: "browser" }];
  const titles = ["Terminal 3", "Dragent 2", "Codex 2", "OpenCode", "Browser 2"];
  inputs.forEach((input, i) => {
    const result = store.createTab(workspaceId, input);
    assert.ok(result.ok);
    assert.equal(store.getTab(result.id)!.title, titles[i]);
    assert.equal(store.getTab(result.id)!.workspaceId, workspaceId);
    assert.equal(store.getTab(result.id)!.projectId, "lumen-web");
  });
  while (store.getSnapshot().tabs.filter((tab) => tab.workspaceId === workspaceId).length < 20) assert.ok(store.createTab(workspaceId, { kind: "terminal" }).ok);
  const full = store.getSnapshot();
  assert.equal(store.createTab(workspaceId, { kind: "browser" }).ok, false);
  assert.equal(store.getSnapshot(), full);
  assert.ok(store.createTab("preview-empty-states", { kind: "terminal" }).ok);
  assertHierarchy(store.getSnapshot());
});

test("writers are tab scoped, stable, and reject wrong kinds without mutating any surface", () => {
  const store = new PreviewStore({ now: () => 3000 });
  const first = store.getWriters(terminalId)!;
  const secondId = "preview-navigation-terminal-2";
  const second = terminal(store, secondId);
  const dragent = terminal(store, agentId);
  const codex = terminal(store, "preview-navigation-codex");
  const other = store.getWorkspace("preview-retries");
  assert.equal(store.getWriters(workspaceId), null);
  assert.equal(store.getTab(workspaceId), null);
  assert.equal(store.getWriters(browserId), null);
  assert.equal(store.getWriters("missing"), null);
  assert.equal(first, store.getWriters(terminalId));
  const before = store.getSnapshot();
  assert.equal(first.sendAgentQuery("wrong kind"), false);
  assert.equal(first.stopAgent(), false);
  assert.equal(store.getWriters(agentId)!.sendInput("wrong kind"), false);
  assert.equal(store.getWriters(agentId)!.executeCommand("wrong kind"), false);
  assert.equal(store.getSnapshot(), before);
  const text = "  echo 'café' $(example)\n  ";
  assert.ok(first.executeCommand(text));
  assert.ok(first.sendInput("\u001b[A\r"));
  const blocks = terminal(store).blocks;
  assert.equal(blocks[1]!.command, `[Preview command] ${text}`);
  assert.equal(blocks[1]!.lines[1]!.spans[0]!.text, JSON.stringify(text));
  assert.equal(blocks[2]!.lines[1]!.spans[0]!.text, JSON.stringify("\u001b[A\r"));
  assert.equal(blocks[2]!.status, "completed");
  assert.equal(blocks[2]!.exitCode, null);
  assert.equal(terminal(store, secondId), second);
  assert.equal(terminal(store, agentId), dragent);
  assert.equal(terminal(store, "preview-navigation-codex"), codex);
  assert.equal(store.getWorkspace("preview-retries"), other);
  assert.ok(store.getWriters(secondId)!.sendInput("second only"));
  assert.equal(terminal(store).blocks.length, 3);
  assert.equal(first, store.getWriters(terminalId));
});

test("Dragent, Codex, OpenCode and two instances of the same provider have isolated conversations", () => {
  const store = new PreviewStore();
  const second = store.createTab(workspaceId, { kind: "agent", agent: "dragent" });
  const opencode = store.createTab(workspaceId, { kind: "agent", agent: "opencode" });
  assert.ok(second.ok && opencode.ok);
  const ids = [agentId, "preview-navigation-codex", opencode.id, second.id];
  for (const [i, id] of ids.entries()) {
    const before = ids.map((other) => terminal(store, other));
    const writer = store.getWriters(id)!;
    assert.ok(writer.sendAgentQuery(`\u0085 Question ${i}? \u3000`));
    const state = terminal(store, id);
    assert.equal(state.agentBlocks.at(-2)!.text, `Question ${i}?`);
    assert.match(state.agentBlocks.at(-1)!.text, new RegExp(`\\[Preview\\] ${store.getTab(id)!.title.split(" ")[0]} fixed local reply`));
    assert.equal(state.surface?.ai_status, "Preview reply ready");
    ids.forEach((other, index) => { if (other !== id) assert.equal(terminal(store, other), before[index]); });
    assert.ok(writer.stopAgent());
    assert.equal(terminal(store, id).surface?.ai_status, "Preview stopped");
    assert.equal(terminal(store, id).agentBlocks.at(-1)!.kind, "system_note");
  }
  assert.equal(terminal(store).blocks.length, 1);
});

test("writers share live UTF-8 validation including Go trim and invalid surrogate replacement", () => {
  const store = new PreviewStore();
  const writer = store.getWriters(terminalId)!;
  const agent = store.getWriters(agentId)!;
  const cases = ["", " ", "\u0085", "\ufeff", "\ud800", "é".repeat(MAX_WRITE_BYTES / 2), "é".repeat(MAX_WRITE_BYTES / 2 + 1), `\u0085${"é".repeat(MAX_WRITE_BYTES / 2)}\u3000`];
  for (const text of cases) {
    for (const [kind, write] of [["input", writer.sendInput], ["command", writer.executeCommand], ["agentQuery", agent.sendAgentQuery]] as const) {
      const before = store.getSnapshot();
      const expected = prepareSessionText(kind, text).ok;
      assert.equal(write(text), expected, `${kind} with ${text.length} characters`);
      if (!expected) assert.equal(store.getSnapshot(), before);
    }
  }
  assert.equal(writer.sendInput(null as never), false);
  assert.equal(agent.sendAgentQuery(null as never), false);
});

test("raw input stays exact and repeated commands/conversations use shared reducer bounds", () => {
  const store = new PreviewStore();
  const writer = store.getWriters(terminalId)!;
  const agent = store.getWriters(agentId)!;
  const raw = " \t café 👩🏽‍💻\n\u001b[A\r ";
  assert.ok(writer.sendInput(raw));
  const block = terminal(store).blocks.at(-1)!;
  assert.equal(block.lines[1]!.spans[0]!.text, JSON.stringify(raw));
  assert.match(block.lines[0]!.spans[0]!.text, new RegExp(`${Buffer.byteLength(raw)} UTF-8 input bytes`));
  assert.ok(writer.sendInput("é".repeat(MAX_WRITE_BYTES / 2)));
  assert.ok(terminal(store).blocks.at(-1)!.truncated);
  assert.ok(terminal(store).historyTruncated);
  for (const line of terminal(store).blocks.at(-1)!.lines) assert.ok(line.spans.reduce((n, span) => n + span.text.length, 0) <= TERMINAL_LIMITS.lineCharacters);
  const count = Math.max(TERMINAL_LIMITS.blocks, TERMINAL_LIMITS.agentBlocks) + 10;
  for (let i = 0; i < count; i++) {
    assert.ok(writer.executeCommand(`example ${i}`));
    assert.ok(agent.sendAgentQuery(`question ${i}`));
  }
  assert.ok(terminal(store).blocks.length <= TERMINAL_LIMITS.blocks);
  assert.equal(terminal(store).blocks.at(-1)!.command, `[Preview command] example ${count - 1}`);
  const conversation = terminal(store, agentId);
  assert.equal(conversation.agentBlocks.length, TERMINAL_LIMITS.agentBlocks);
  assert.equal(conversation.agentBlocks.at(-2)!.text, `question ${count - 1}`);
  assert.equal(conversation.historyTruncated, true);
});

test("browser pages, navigation, reload and bounded history stay isolated per tab", () => {
  const store = new PreviewStore();
  const second = store.createTab(workspaceId, { kind: "browser" });
  assert.ok(second.ok);
  const untouched = browser(store, second.id);
  const initial = store.getSnapshot();
  assert.equal(store.navigateBrowser(browserId, "overview"), true);
  assert.equal(store.getSnapshot(), initial);
  assert.equal(store.goBrowser(browserId, "back"), false);
  assert.equal(store.goBrowser(browserId, "forward"), false);
  for (const id of ["missing", terminalId, agentId, workspaceId]) {
    assert.equal(store.navigateBrowser(id, "activity"), false);
    assert.equal(store.goBrowser(id, "back"), false);
    assert.equal(store.reloadBrowser(id), false);
  }
  assert.equal(store.navigateBrowser(browserId, "https://example.invalid" as never), false);
  assert.equal(store.goBrowser(browserId, "other" as never), false);
  assert.equal(store.getSnapshot(), initial);
  assert.ok(store.navigateBrowser(browserId, "activity"));
  assert.ok(store.navigateBrowser(browserId, "settings"));
  assert.deepEqual(browser(store).history, ["overview", "activity", "settings"]);
  assert.ok(store.goBrowser(browserId, "back"));
  assert.equal(browser(store).page, "activity");
  assert.ok(store.goBrowser(browserId, "forward"));
  assert.equal(browser(store).page, "settings");
  assert.ok(store.goBrowser(browserId, "back"));
  assert.ok(store.navigateBrowser(browserId, "overview"));
  assert.deepEqual(browser(store).history, ["overview", "activity", "overview"]);
  assert.equal(store.goBrowser(browserId, "forward"), false);
  const beforeReload = browser(store);
  assert.ok(store.reloadBrowser(browserId));
  assert.deepEqual(browser(store), { ...beforeReload, revision: beforeReload.revision + 1 });
  const pages: BrowserPage[] = ["activity", "settings", "overview"];
  for (let i = 0; i < 50; i++) assert.ok(store.navigateBrowser(browserId, pages[i % 3]!));
  assert.equal(browser(store).history.length, 20);
  assert.equal(browser(store).index, 19);
  let backCount = 0;
  while (store.goBrowser(browserId, "back")) backCount++;
  assert.equal(backCount, 19);
  assert.equal(browser(store, second.id), untouched);
  assert.equal(terminal(store).blocks.length, 1);
  store.reset();
  assert.deepEqual(browser(store), { page: "overview", history: ["overview"], index: 0, revision: 0 });
  assert.equal(store.navigateBrowser(second.id, "activity"), false);
  assert.equal(store.goBrowser(second.id, "back"), false);
  assert.equal(store.reloadBrowser(second.id), false);
});

test("reset restores fresh samples atomically and rejects all previously issued writers", () => {
  const store = new PreviewStore();
  const seeded = store.getSnapshot();
  const created = store.createWorkspace("lumen-web", "Temporary", "feature/temporary");
  const extra = store.createTab(workspaceId, { kind: "agent", agent: "codex" });
  assert.ok(created.ok && extra.ok);
  const writers = [store.getWriters(terminalId)!, store.getWriters(agentId)!, store.getWriters(`${created.id}-terminal`)!, store.getWriters(extra.id)!];
  assert.ok(writers[0]!.executeCommand("example"));
  assert.ok(writers[1]!.sendAgentQuery("example"));
  assert.ok(store.navigateBrowser(browserId, "settings"));
  let notifications = 0;
  store.subscribe(() => { notifications++; assertHierarchy(store.getSnapshot()); });
  store.reset();
  assert.equal(notifications, 1);
  assert.deepEqual(store.getSnapshot(), seeded);
  assert.notEqual(store.getSnapshot(), seeded);
  assert.equal(store.getWorkspace(created.id), null);
  assert.equal(store.getTab(extra.id), null);
  for (const writer of writers) {
    assert.equal(writer.sendInput("x"), false);
    assert.equal(writer.executeCommand("x"), false);
    assert.equal(writer.sendAgentQuery("x"), false);
    assert.equal(writer.stopAgent(), false);
  }
  assert.equal(notifications, 1);
  const fresh = store.getWriters(terminalId)!;
  assert.notEqual(fresh, writers[0]);
  assert.equal(fresh, store.getWriters(terminalId));
  assert.ok(fresh.executeCommand("fresh"));
  const nextWorkspace = store.createWorkspace("lumen-web", "Temporary", "feature/temporary");
  const nextTab = store.createTab(workspaceId, { kind: "agent", agent: "codex" });
  assert.ok(nextWorkspace.ok && nextTab.ok);
  assert.notEqual(nextWorkspace.id, created.id);
  assert.notEqual(nextTab.id, extra.id);
});

test("selectors reject every foreign parent including another workspace in the same project", () => {
  const store = new PreviewStore();
  const snapshot = store.getSnapshot();
  assert.equal(selectPreviewProject(snapshot, "preview-linux", "lumen-web"), null);
  assert.equal(selectPreviewProject(snapshot, "missing", "lumen-web"), null);
  assert.equal(selectPreviewProject(snapshot, "preview-mac", "missing"), null);
  assert.equal(selectPreviewWorkspace(snapshot, "preview-mac", "lumen-web", "preview-empty-states")?.id, "preview-empty-states");
  const routeWriters = (desktop: string, project: string, workspace: string, tabId: string) => {
    const tab = selectPreviewTab(store.getSnapshot(), desktop, project, workspace, tabId);
    return tab ? store.getWriters(tab.id) : null;
  };
  for (const route of [
    ["preview-linux", "lumen-web", workspaceId, terminalId],
    ["preview-mac", "field-notes", workspaceId, terminalId],
    ["preview-mac", "lumen-web", "preview-empty-states", terminalId],
    ["preview-mac", "lumen-web", workspaceId, "preview-empty-states-terminal"],
    ["missing", "lumen-web", workspaceId, terminalId],
    ["preview-mac", "lumen-web", "missing", terminalId],
    ["preview-mac", "lumen-web", workspaceId, "missing"],
  ] as const) assert.equal(routeWriters(route[0], route[1], route[2], route[3]), null);
  for (const corrupt of [
    { ...snapshot, desktops: [] }, { ...snapshot, projects: [] }, { ...snapshot, workspaces: [] },
    { ...snapshot, tabs: snapshot.tabs.map((tab) => tab.id === terminalId ? { ...tab, projectId: "relay-api" } : tab) },
    { ...snapshot, tabs: snapshot.tabs.map((tab) => tab.id === terminalId ? { ...tab, workspaceId: "preview-empty-states" } : tab) },
  ]) assert.equal(selectPreviewTab(corrupt, "preview-mac", "lumen-web", workspaceId, terminalId), null);
  assert.equal(store.getSnapshot(), snapshot);
  const writer = routeWriters("preview-mac", "lumen-web", workspaceId, terminalId);
  assert.equal(writer, store.getWriters(terminalId));
  assert.ok(writer!.executeCommand("local mac example"));
  assert.equal(terminal(store, "preview-retries-terminal").blocks.length, 1);
});

test("invalid clocks never produce invalid metadata dates", () => {
  for (const now of [Infinity, NaN, -1, Number.MAX_SAFE_INTEGER]) {
    const store = new PreviewStore({ now: () => now });
    const created = store.createWorkspace("lumen-web", "Clock", "feature/clock");
    assert.ok(created.ok);
    assert.equal(store.getWorkspace(created.id)!.updatedAt, 0);
    assert.ok(store.getWriters(`${created.id}-terminal`)!.sendInput("text"));
    assert.equal(store.getWorkspace(created.id)!.updatedAt, 0);
  }
});

test("all preview actions run without network or browser persistence access", (t) => {
  const forbiddenFetch = t.mock.method(globalThis, "fetch", async () => { throw new Error("Preview must not fetch"); });
  t.after(() => forbiddenFetch.mock.restore());
  for (const key of ["WebSocket", "localStorage", "sessionStorage"]) {
    const previous = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, get: () => { throw new Error(`Preview must not access ${key}`); } });
    t.after(() => {
      if (previous) Object.defineProperty(globalThis, key, previous);
      else Reflect.deleteProperty(globalThis, key);
    });
  }
  const store = new PreviewStore();
  const created = store.createWorkspace("field-notes", "No IO", "feature/no-io");
  assert.ok(created.ok);
  const terminalWriter = store.getWriters(`${created.id}-terminal`)!;
  const agent = store.getWriters(`${created.id}-agent`)!;
  assert.ok(terminalWriter.executeCommand("$(example) && curl https://example.invalid"));
  assert.ok(terminalWriter.sendInput("café\t🙂\r\u001b[A\n"));
  assert.ok(agent.sendAgentQuery("Read sample diff"));
  assert.ok(agent.stopAgent());
  assert.ok(store.navigateBrowser(`${created.id}-browser`, "settings"));
  assert.ok(store.reloadBrowser(`${created.id}-browser`));
  assert.ok(store.goBrowser(`${created.id}-browser`, "back"));
  store.toggleTask(created.id, "explore");
  assert.equal(store.getWorkspace(created.id)!.tasks[0]!.status, "done");
  store.reset();
  assert.equal(terminalWriter.sendInput("stale"), false);
  assert.equal(forbiddenFetch.mock.callCount(), 0);
});

test("the first workspace ships an offline Dragent-working sample", () => {
  const store = new PreviewStore();
  const tab = store.getTab(`${workspaceId}-agent`);
  assert.ok(tab && tab.kind === "agent" && tab.agent === "dragent");
  assert.deepEqual(
    tab.terminal.agentBlocks.map((block) => block.kind),
    ["thinking", "tool_step", "agent_message", "user_query"],
  );
  assert.equal(tab.terminal.agentBlocks.at(-1)!.text, "Verify navigation in a narrow window");
  assert.equal(tab.terminal.surface?.ai_status, "Working…");
  assert.equal(previewTabAgentState(tab, 0), "working");
  // Opening the tab records it seen, but the open request keeps the sample
  // demonstrably busy so the working UI stays eyeballable offline.
  assert.equal(store.markTabSeen(tab.id), true);
  assert.equal(previewTabAgentState(tab, store.getSnapshot().tabSeen[tab.id] ?? 0), "working");

  // Other workspaces keep the single settled Dragent reply.
  const other = store.getTab("preview-retries-agent");
  assert.ok(other && other.kind === "agent" && other.agent === "dragent");
  assert.deepEqual(other.terminal.agentBlocks.map((block) => block.kind), ["agent_message"]);
});
