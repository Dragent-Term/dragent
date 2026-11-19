import assert from "node:assert/strict";
import { test } from "node:test";
import { PreviewStore, PREVIEW_LIMITS } from "../src/preview/PreviewStore";
import { MACHINE_VIEWS, parseMachineView } from "../src/preview/machineNavigation";
import { selectAutomation, selectCoordination, selectMachineTab, selectMachineWorkspaces } from "../src/preview/machineSelectors";

test("machine areas follow desktop product order and stay distinct from workspace tab kinds", () => {
  assert.deepEqual(MACHINE_VIEWS.map((area) => area.label), ["Project", "Agent coordination", "Automation", "Code review", "Tasks"]);
  for (const area of MACHINE_VIEWS) assert.equal(parseMachineView(area.id), area.id);
  for (const value of ["agents", ["automation"], undefined, {}, "missing"]) assert.equal(parseMachineView(value), "projects");
  assert.deepEqual([...new Set(new PreviewStore().getSnapshot().tabs.map((tab) => tab.kind))].sort(), ["agent", "browser", "terminal"]);
});

test("machine links validate desktop, project, workspace and agent identity", () => {
  const snapshot = new PreviewStore().getSnapshot();
  assert.equal(selectMachineWorkspaces(snapshot, "preview-mac").length, 3);
  assert.equal(selectMachineWorkspaces(snapshot, "preview-linux").length, 1);
  assert.equal(selectMachineWorkspaces(snapshot, "missing").length, 0);
  for (const team of snapshot.coordinations) assert.ok(selectCoordination(snapshot, team.desktopId, team.id));
  for (const automation of snapshot.automations) assert.ok(selectAutomation(snapshot, automation.desktopId, automation.id));
  assert.equal(selectCoordination(snapshot, "preview-linux", "navigation-team"), null);
  assert.equal(selectAutomation(snapshot, "preview-linux", "navigation-check"), null);
  assert.equal(selectMachineTab(snapshot, "preview-linux", "preview-navigation-agent"), null);
  const badLead = { ...snapshot, coordinations: snapshot.coordinations.map((team) => ({ ...team, leadTabId: "preview-navigation-terminal" })) };
  assert.equal(selectCoordination(badLead, "preview-mac", "navigation-team"), null);
  const badTarget = { ...snapshot, automations: snapshot.automations.map((automation) => ({ ...automation, workspaceId: "preview-retries" })) };
  assert.equal(selectAutomation(badTarget, "preview-mac", "navigation-check"), null);
});

test("coordination controls keep messages bounded, reject foreign writes and preserve finished work", () => {
  const store = new PreviewStore({ now: () => 1000 });
  const before = store.getSnapshot();
  const terminal = store.getTab("preview-navigation-codex");
  assert.equal(store.sendCoordinationMessage("preview-linux", "navigation-team", "hello"), false);
  assert.equal(store.sendCoordinationMessage("preview-mac", "navigation-team", " "), false);
  assert.equal(store.sendCoordinationMessage("preview-mac", "navigation-team", "x".repeat(2001)), false);
  assert.equal(store.configureCoordination("preview-mac", "navigation-team", { parallelism: 9 }), false);
  assert.equal(store.configureCoordination("preview-mac", "navigation-team", { parallelism: 0.5 }), false);
  assert.equal(store.getSnapshot(), before);
  assert.ok(store.configureCoordination("preview-mac", "navigation-team", { parallelism: 3, autoStart: false }));
  for (let i = 0; i < 50; i++) assert.ok(store.sendCoordinationMessage("preview-mac", "navigation-team", `Message ${i}`));
  let team = selectCoordination(store.getSnapshot(), "preview-mac", "navigation-team")!;
  assert.equal(team.messages.length, PREVIEW_LIMITS.coordinationMessages);
  assert.equal(team.messages.at(-1)?.body, "Message 49");
  assert.equal(team.parallelism, 3);
  assert.equal(team.autoStart, false);
  assert.ok(store.stopCoordination("preview-mac", "navigation-team"));
  team = selectCoordination(store.getSnapshot(), "preview-mac", "navigation-team")!;
  assert.equal(team.status, "stopped");
  assert.equal(team.plan[0]?.status, "done");
  assert.ok(team.plan.slice(1).every((task) => task.status === "cancelled"));
  assert.equal(store.configureCoordination("preview-mac", "navigation-team", { autoStart: true }), false);
  assert.equal(store.stopCoordination("preview-mac", "navigation-team"), false);
  assert.equal(store.getTab("preview-navigation-codex"), terminal, "Stopping a sample coordination must not close or write to agent tabs");
  assert.equal(before.coordinations[0]?.status, "running");
  assert.ok(Object.isFrozen(team.messages));
});

test("automation sample runs are isolated and bounded without launching an agent or workspace", () => {
  const store = new PreviewStore({ now: () => 1000 });
  const before = store.getSnapshot();
  assert.equal(store.runAutomation("preview-linux", "navigation-check"), false);
  assert.equal(store.toggleAutomation("preview-linux", "navigation-check"), false);
  assert.equal(store.getSnapshot(), before);
  assert.ok(store.toggleAutomation("preview-mac", "navigation-check"));
  assert.equal(selectAutomation(store.getSnapshot(), "preview-mac", "navigation-check")?.enabled, false);
  for (let i = 0; i < 30; i++) assert.ok(store.runAutomation("preview-mac", "navigation-check"));
  assert.ok(store.runAutomation("preview-mac", "docs-check"));
  const snapshot = store.getSnapshot();
  assert.equal(snapshot.automationRuns.filter((run) => run.automationId === "navigation-check").length, PREVIEW_LIMITS.automationRuns);
  assert.equal(snapshot.automationRuns.filter((run) => run.automationId === "docs-check").length, 1);
  assert.equal(snapshot.tabs, before.tabs);
  assert.equal(snapshot.workspaces, before.workspaces);
  assert.equal(before.automations[0]?.enabled, true);
  assert.match(snapshot.automationRuns[0]!.output, /No scheduler, shell or model ran/);
});

test("workspace review notes and progress stay scoped to machine and changed file", () => {
  const store = new PreviewStore();
  const before = store.getSnapshot();
  assert.equal(store.addReviewNote("preview-linux", "preview-navigation", "src/navigation.ts", "Note"), false);
  assert.equal(store.addReviewNote("preview-mac", "preview-navigation", "README.md", "Note"), false);
  assert.equal(store.addReviewNote("preview-mac", "preview-navigation", "src/navigation.ts", " "), false);
  assert.equal(store.setReviewFileReviewed("preview-mac", "preview-retries", "src/retry-policy.ts", true), false);
  assert.equal(store.getSnapshot(), before);
  assert.ok(store.setReviewFileReviewed("preview-mac", "preview-navigation", "src/navigation.ts", true));
  assert.ok(store.setReviewFileReviewed("preview-mac", "preview-navigation", "src/navigation.ts", true));
  assert.equal(store.getSnapshot().reviews[0]?.reviewedPaths.length, 1);
  assert.ok(store.addReviewNote("preview-mac", "preview-navigation", "src/navigation.ts", "Keep this route clear."));
  for (let i = 1; i < PREVIEW_LIMITS.reviewNotes; i++) assert.ok(store.addReviewNote("preview-mac", "preview-navigation", "src/navigation.ts", `Note ${i}`));
  assert.equal(store.addReviewNote("preview-mac", "preview-navigation", "src/navigation.ts", "Over the limit"), false);
  assert.ok(store.setReviewFileReviewed("preview-mac", "preview-navigation", "src/navigation.ts", false));
  assert.deepEqual(store.getSnapshot().reviews[0]?.reviewedPaths, []);
  assert.equal(store.getSnapshot().reviews[0]?.notes.length, PREVIEW_LIMITS.reviewNotes);
  assert.equal(before.reviews.length, 0);
});

test("reset restores all machine samples and discards their local edits", () => {
  const store = new PreviewStore();
  const other = new PreviewStore();
  store.stopCoordination("preview-mac", "navigation-team");
  store.toggleAutomation("preview-mac", "navigation-check");
  store.runAutomation("preview-mac", "docs-check");
  store.addReviewNote("preview-mac", "preview-navigation", "src/navigation.ts", "A sample note");
  assert.deepEqual(other.getSnapshot().reviews, []);
  store.reset();
  assert.deepEqual(store.getSnapshot(), other.getSnapshot());
});
