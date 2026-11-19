import assert from "node:assert/strict";
import { test } from "node:test";
import { filterWorkspaces } from "../src/machines/machineCatalog";
import type { RelayWorkspace } from "../src/transport/relayCatalog";

function workspace(name: string, branch: string | null): RelayWorkspace {
  return Object.freeze({
    id: `ws-${name}`,
    name,
    path: `/repo/${name}`,
    branch,
    project: null,
    project_id: "proj-a",
  });
}

const workspaces: readonly RelayWorkspace[] = [
  workspace("Nav", "main"),
  workspace("Local", null),
  workspace("Api Fix", "feature/api-fix"),
];

test("matches by workspace name", () => {
  assert.deepEqual(
    filterWorkspaces(workspaces, "nav").map((w) => w.name),
    ["Nav"],
  );
});

test("matches by branch", () => {
  assert.deepEqual(
    filterWorkspaces(workspaces, "feature/api").map((w) => w.name),
    ["Api Fix"],
  );
});

test("matching is case-insensitive across name and branch", () => {
  assert.deepEqual(
    filterWorkspaces(workspaces, "NAV").map((w) => w.name),
    ["Nav"],
  );
  assert.deepEqual(
    filterWorkspaces(workspaces, "MAIN").map((w) => w.name),
    ["Nav"],
  );
});

test("query is trimmed before matching", () => {
  assert.deepEqual(
    filterWorkspaces(workspaces, "  nav  ").map((w) => w.name),
    ["Nav"],
  );
});

test("empty or blank query returns all workspaces", () => {
  assert.deepEqual(filterWorkspaces(workspaces, ""), workspaces);
  assert.deepEqual(filterWorkspaces(workspaces, "   "), workspaces);
});

test("null branch is safe and matches by name", () => {
  assert.deepEqual(
    filterWorkspaces(workspaces, "local").map((w) => w.name),
    ["Local"],
  );
  // A branch-only query must not crash on the branchless checkout.
  assert.deepEqual(
    filterWorkspaces(workspaces, "zzz-no-match").map((w) => w.name),
    [],
  );
});
