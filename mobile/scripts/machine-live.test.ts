import assert from "node:assert/strict";
import { test } from "node:test";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  parseMachinesListResult,
  parseMachineSummary,
  parseRelayCatalog,
  parseRelayCatalogSnapshotPayload,
} from "../src/transport/relayCatalog";
import {
  initialMachineCatalogState,
  loadingMachineCatalog,
  unavailableMachineCatalog,
} from "../src/transport/machineCatalogState";
import {
  isLegacyCatalog,
  knownProjectKeys,
  knownWorkspaceKeys,
  orphanWorkspaces,
  projectKey,
  tabsForProject,
  tabsForWorkspace,
  unplacedTabs,
  workspaceBranchLabel,
  workspaceProjectKey,
  workspacesForProject,
} from "../src/machines/machineCatalog";
import { createRequire } from "node:module";
import {
  machinePath,
  machineProjectPath,
  machineTabPath,
  machineWorkspacePath,
  singleParam,
} from "../src/machines/routes";

// The installed router's own encoder: `resolveHref` applies one
// `encodeURIComponent` per dynamic segment (expo-router/build/link/href.js),
// and `useLocalSearchParams` applies one `decodeURIComponent` with a raw
// fallback (expo-router/build/hooks/useLocalSearchParams.js). Tests below
// drive this real encoder and the hook's exact decode expression.
const require = createRequire(import.meta.url);
const { resolveHref } = require("expo-router/build/link/href") as {
  resolveHref: (href: { pathname: string; params?: Record<string, string> }) => string;
};

/** The installed hook's decode, verbatim: decode once, raw on malformed. */
function hookDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

const enriched = {
  machine: { name: "Studio", platform: "macOS", version: "1.2.3" },
  projects: [
    { id: "proj-a", name: "Web", path: "/repo/web" },
    { id: "proj-b", name: "API", path: "/repo/api" },
  ],
  workspaces: [
    { id: "ws-1", name: "Nav", path: "/repo/web-wt", branch: "main", project_id: "proj-a" },
    { id: "ws-2", name: "Local", path: "/tmp/local", branch: null, project_id: "proj-a" },
    { id: "ws-3", name: "Foreign", path: "/repo/foreign", branch: "dev", project_id: "proj-ghost" },
    { name: "Legacy", path: "/repo/legacy", branch: "old" },
  ],
  tabs: [
    { id: 7, kind: "terminal", title: "zsh", active: true, visible: true, workspace_id: "ws-1", project_id: "proj-a" },
    { id: 9, kind: "agent", title: "Agent", active: false, visible: true, workspace_id: "ws-1", project_id: "proj-a", agent_kind: "codex", agent_state: "working" },
    { id: 11, kind: "browser", title: "Docs", active: false, visible: false, workspace_id: "ws-2", project_id: "proj-a" },
    { id: 13, kind: "terminal", title: "Orphan", active: false, visible: true, workspace_id: "ws-ghost", project_id: "proj-a" },
    { id: 15, kind: "terminal", title: "Legacy", active: false, visible: true },
  ],
};

test("enriched catalog parses with ids, agent identity and machine metadata", () => {
  const catalog = parseRelayCatalog(enriched);
  assert.ok(catalog);
  assert.equal(catalog!.machine?.name, "Studio");
  assert.equal(catalog!.machine?.platform, "macOS");
  assert.equal(catalog!.projects[0]!.id, "proj-a");
  assert.equal(catalog!.workspaces[0]!.project_id, "proj-a");
  assert.equal(catalog!.workspaces[1]!.branch, null);
  assert.equal(catalog!.tabs[1]!.agent_kind, "codex");
  assert.equal(catalog!.tabs[1]!.agent_state, "working");
  // Legacy entries without relationship ids stay readable with null ids.
  assert.equal(catalog!.workspaces[3]!.id, null);
  assert.equal(catalog!.workspaces[3]!.project_id, null);
  assert.equal(catalog!.tabs[4]!.workspace_id, null);
});

test("catalog reader stays strict and bounded like the server", () => {
  // Unknown kinds, oversize ids and out-of-range tab ids are rejected.
  for (const raw of [
    { ...enriched, tabs: [{ id: 1, kind: "editor" }] },
    { ...enriched, projects: [{ id: "x".repeat(2000), name: "W", path: "/p" }] },
    { ...enriched, tabs: [{ id: 2 ** 53, kind: "terminal" }] },
    { ...enriched, tabs: [{ id: 1, kind: "terminal", agent_state: "napping" }] },
    { ...enriched, projects: Array.from({ length: 129 }, (_, i) => ({ name: `p${i}`, path: `/p${i}` })) },
    { ...enriched, workspaces: Array.from({ length: 257 }, (_, i) => ({ name: `w${i}`, path: `/w${i}` })) },
    { ...enriched, tabs: Array.from({ length: 1025 }, (_, i) => ({ id: i, kind: "terminal" })) },
    { ...enriched, machine: { name: "", platform: "macOS" } },
  ]) {
    assert.equal(parseRelayCatalog(raw), null);
  }
  // Agent identity is explicit only: a terminal without agent_kind stays terminal.
  const catalog = parseRelayCatalog(enriched)!;
  assert.equal(catalog.tabs[0]!.kind, "terminal");
  assert.equal(catalog.tabs[0]!.agent_kind, null);
});

test("branch hierarchy groups workspaces by explicit project key only", () => {
  const catalog = parseRelayCatalog(enriched)!;
  const [web, api] = catalog.projects;
  assert.equal(projectKey(web!), "proj-a");
  const webWorkspaces = workspacesForProject(catalog, web!);
  assert.deepEqual(webWorkspaces.map((w) => w.name), ["Nav", "Local"]);
  assert.deepEqual(workspacesForProject(catalog, api!), []);
  // Branch labels: real branch verbatim, branchless checkout honest.
  assert.equal(workspaceBranchLabel(webWorkspaces[0]!), "main");
  assert.equal(workspaceBranchLabel(webWorkspaces[1]!), "No branch");
  // Foreign project ids are never pulled into a project by array order.
  assert.ok(!webWorkspaces.some((w) => w.name === "Foreign"));
  assert.ok(!webWorkspaces.some((w) => w.name === "Legacy"));
});

test("orphan and legacy entries are suppressed from grouped views, not guessed", () => {
  const catalog = parseRelayCatalog(enriched)!;
  const projects = knownProjectKeys(catalog);
  const orphans = orphanWorkspaces(catalog, projects);
  assert.deepEqual(orphans.map((w) => w.name).sort(), ["Foreign", "Legacy"]);
  assert.equal(workspaceProjectKey(orphans.find((w) => w.name === "Legacy")!), null);
  // Tabs follow explicit workspace keys.
  const nav = catalog.workspaces.find((w) => w.name === "Nav")!;
  assert.deepEqual(tabsForWorkspace(catalog, nav).map((t) => t.id), [7, 9]);
  // Foreign and legacy tabs are unplaced, never attributed to Nav.
  const unplaced = unplacedTabs(catalog, knownWorkspaceKeys(catalog));
  assert.deepEqual(unplaced.map((t) => t.id).sort((a, b) => a - b), [13, 15]);
  // Project tabs include workspace tabs; the foreign tab stays out.
  assert.deepEqual(
    tabsForProject(catalog, catalog.projects[0]!).map((t) => t.id).sort((a, b) => a - b),
    [7, 9, 11],
  );
});

test("legacy catalogs without relationship ids are readable but never placed", () => {
  const catalog = parseRelayCatalog({
    projects: [{ name: "Web", path: "/repo" }],
    workspaces: [{ name: "Nav", path: "/repo", branch: "main", project: "Web" }],
    tabs: [{ id: 7, kind: "terminal", title: "zsh", active: true, visible: true }],
  })!;
  assert.equal(isLegacyCatalog(catalog), true);
  // Legacy path-shaped project linkage is not treated as an id match.
  assert.deepEqual(workspacesForProject(catalog, catalog.projects[0]!), []);
  assert.deepEqual(tabsForWorkspace(catalog, catalog.workspaces[0]!), []);
  assert.equal(isLegacyCatalog(parseRelayCatalog(enriched)!), false);
});

test("machine discovery entries validate device, freshness and liveness", () => {
  const now = Date.now();
  const list = parseMachinesListResult({
    machines: [
      { device_id: "desktop-1", name: "Studio", platform: "macOS", version: "1.0", last_seen_at: now, online: true },
      { device_id: "desktop-2", name: "Linux", platform: "Linux", last_seen_at: now - 120_000, online: false },
    ],
  });
  assert.ok(list);
  assert.equal(list!.length, 2);
  assert.equal(list![0]!.online, true);
  assert.equal(list![1]!.online, false);
  assert.equal(list![1]!.version, null);
  for (const entry of [
    { device_id: "", name: "S", platform: "macOS", last_seen_at: now, online: true },
    { device_id: "d", name: "S", platform: "macOS", last_seen_at: -1, online: true },
    { device_id: "d", name: "S", platform: "macOS", last_seen_at: now },
    { name: "S", platform: "macOS", last_seen_at: now, online: true },
    { device_id: "d", name: "S", platform: "macOS", last_seen_at: now, online: "yes" },
  ]) {
    assert.equal(parseMachineSummary(entry), null);
    assert.equal(parseMachinesListResult({ machines: [entry] }), null);
  }
  // An overlong list is rejected even when every entry is valid.
  assert.equal(
    parseMachinesListResult({
      machines: Array.from({ length: 257 }, (_, i) => (
        { device_id: `d${i}`, name: "S", platform: "P", last_seen_at: now, online: true }
      )),
    }),
    null,
  );
  assert.equal(parseMachinesListResult({ device_id: "d" }), null);
});

test("catalog stream payloads replace wholesale with machine liveness", () => {
  const now = Date.now();
  // Offline replacement: the payload still parses; machine.online === false
  // is what the UI keys the offline state on — never stale data as live.
  const offline = parseRelayCatalogSnapshotPayload({
    catalog: enriched,
    machine: { device_id: "desktop-1", name: "Studio", platform: "macOS", last_seen_at: now - 60_000, online: false },
  });
  assert.ok(offline);
  assert.equal(offline!.machine.online, false);
  assert.equal(offline!.catalog.tabs.length, 5);
  const online = parseRelayCatalogSnapshotPayload({
    catalog: enriched,
    machine: { device_id: "desktop-1", name: "Studio", platform: "macOS", last_seen_at: now, online: true },
  });
  assert.ok(online);
  assert.equal(online!.machine.online, true);
  // Malformed halves never produce a half snapshot.
  assert.equal(parseRelayCatalogSnapshotPayload({ catalog: enriched }), null);
  assert.equal(
    parseRelayCatalogSnapshotPayload({
      catalog: enriched,
      machine: { device_id: "d", name: "S", platform: "P", last_seen_at: now, online: "yes" },
    }),
    null,
  );
});

test("catalog states keep offline, error, empty and loading distinct", () => {
  assert.equal(loadingMachineCatalog.status, "loading");
  assert.equal(loadingMachineCatalog.catalog, null);
  const failed = unavailableMachineCatalog("The live machine catalog is unreachable right now.");
  assert.equal(failed.status, "unavailable");
  assert.match(failed.reason, /unreachable/);
  assert.equal(failed.catalog, null);
  // Missing gateway or device starts unavailable — never a previous machine.
  for (const source of [
    { gateway: null, deviceId: null },
    { gateway: "wss://relay.example/realtime", deviceId: null },
    { gateway: null, deviceId: "desktop-1" },
  ]) {
    const state = initialMachineCatalogState(source);
    assert.equal(state.status, "unavailable");
    assert.equal(state.catalog, null);
    assert.equal(state.machine, null);
  }
  const ready = initialMachineCatalogState({ gateway: "wss://relay.example/realtime", deviceId: "desktop-1" });
  assert.equal(ready.status, "loading");
});

test("simple machine identifiers retain readable URLs", () => {
  assert.deepEqual(machinePath("desktop"), { pathname: "/machine/[deviceId]", params: { deviceId: "desktop" } });
  assert.deepEqual(machineWorkspacePath("d", "w").params, { deviceId: "d", workspaceId: "w" });
  assert.deepEqual(machineTabPath("d", 7).params, { deviceId: "d", tabId: "7" });
  assert.equal(singleParam(undefined), "");
  assert.equal(singleParam(["a", "b"]), "");
  assert.equal(singleParam("r1~broken~GG"), "");
});

test("complex identifiers survive repeated framework decoding without collisions", () => {
  const ids = ["/repo/web", "a/b", "a%2Fb", "100%", "a+b", "ws-üñï", "a?b#c&d=e", "device 1", "r1~a~2Fb", "literal~2F", "r1.plain"];
  const hrefs = new Set<string>();
  for (const id of ids) {
    const href = resolveHref(machinePath(id));
    assert.ok(!hrefs.has(href), `route collision for ${id}`);
    hrefs.add(href);
    const segments = href.split("/");
    assert.equal(segments.length, 3, `route segment split: ${href}`);
    let param = segments[2]!;
    // Both linking and hooks can decode; the route token remains stable.
    for (let pass = 0; pass < 3; pass++) {
      param = hookDecode(param);
      assert.equal(singleParam(param), id);
    }
  }
});

test("machine/project/workspace helpers preserve their independent identities", () => {
  const href = resolveHref(machineProjectPath("desktop 1", "/repo/100%2F literal"));
  const segments = href.split("/");
  assert.equal(segments.length, 5);
  assert.equal(segments[3], "project");
  assert.equal(singleParam(hookDecode(hookDecode(segments[2]!))), "desktop 1");
  assert.equal(singleParam(hookDecode(hookDecode(segments[4]!))), "/repo/100%2F literal");
  const workspace = resolveHref(machineWorkspacePath("desktop 1", "/workspace/branch"));
  assert.equal(singleParam(hookDecode(workspace.split("/")[4]!)), "/workspace/branch");
});

test("every helper pathname resolves to a real route file", () => {
  const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
  for (const file of [
    "app/machine/[deviceId]/index.tsx",
    "app/machine/[deviceId]/project/[projectId].tsx",
    "app/machine/[deviceId]/workspace/[workspaceId].tsx",
    "app/machine/[deviceId]/tab/[tabId].tsx",
    "app/machine/[deviceId]/_layout.tsx",
  ]) {
    assert.ok(existsSync(path.join(root, file)), `missing route file ${file}`);
  }
});
