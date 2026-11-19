import { test } from "node:test";
import assert from "node:assert/strict";
import { chooseCatalog } from "../../machines/heldCatalog";
import { loadingMachineCatalog, unavailableMachineCatalog, type MachineCatalogState } from "../machineCatalogState";

const live = Object.freeze({ status: "live", reason: null, catalog: {}, machine: {}, warning: null }) as unknown as MachineCatalogState;
const down = unavailableMachineCatalog("The machine relay connection was interrupted.");

test("the last catalog stays up while the connection is re-made", () => {
  assert.equal(chooseCatalog(loadingMachineCatalog, live, "connecting", false), live);
  assert.equal(chooseCatalog(down, live, "reconnecting", false), live);
  // Connected again, catalog not delivered yet.
  assert.equal(chooseCatalog(loadingMachineCatalog, live, "ready", false), live);
});

test("a real failure, a long outage or nothing to hold shows the truth", () => {
  assert.equal(chooseCatalog(down, live, "error", false), down);
  assert.equal(chooseCatalog(down, live, "reconnecting", true), down);
  assert.equal(chooseCatalog(down, null, "reconnecting", false), down);
  // The desktop itself said it is unavailable on a working connection.
  assert.equal(chooseCatalog(down, live, "ready", false), down);
});
