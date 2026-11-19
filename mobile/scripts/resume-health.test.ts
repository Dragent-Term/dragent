import assert from "node:assert/strict";
import { test } from "node:test";
import { connectionBack } from "../src/machines/resumeHealth";
import type { RelaySocketSnapshot } from "../src/transport/RelaySocketClient";
import type { MachineCatalogState } from "../src/transport/machineCatalogState";

const socket = (patch: Partial<RelaySocketSnapshot>): RelaySocketSnapshot =>
  ({ status: "ready", error: null, loginRequired: false, connectionEpoch: 1, retryAttempt: 0, ...patch }) as RelaySocketSnapshot;
const live = { status: "live" } as MachineCatalogState;
const loading = { status: "loading" } as MachineCatalogState;
const open = { state: "open", problem: null, message: null } as const;

test("the desktop is back once the relay, the catalog and the encrypted channel all are", () => {
  assert.equal(connectionBack(socket({ secure: open }), live), true);
});

test("anything still on its way counts as not back", () => {
  assert.equal(connectionBack(socket({ status: "reconnecting", secure: open }), live), false);
  assert.equal(connectionBack(socket({ secure: open }), loading), false);
  assert.equal(connectionBack(socket({ secure: { state: "retrying", problem: "handshake", message: null } as never }), live), false);
  assert.equal(connectionBack(socket({ secure: { state: "connecting", problem: null, message: null } }), live), false);
});
