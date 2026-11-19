/** Invoked by the opt-in Go test against its isolated loopback fixture. */
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { RelaySocketClient } from "../src/transport/RelaySocketClient";
import { listTabs, createTerminal, listMachines, subscribeMachineCatalog, subscribeRuntimeEvents } from "../src/transport/relayRpcMethods";
import type { RealtimeSocket } from "../src/transport/socket";
import { revokeCompanionSession } from "../src/transport/companionSession";
import { attachCompanionTab, readCompanionTab, inputCompanionTab, resizeCompanionTab, screenCompanionTab, detachCompanionTab } from "../src/transport/relayRpcMethods";
import { getCoordination, listCoordinations } from "../src/coordination/rpc";

async function waitFor(ready: () => boolean, label: string) {
  for (let attempt = 0; !ready(); attempt++) {
    assert.ok(attempt < 250, label);
    await delay(20);
  }
}
async function main() {
  const gateway = process.env.DRAGENT_TEST_RELAY_URL;
  const token = process.env.DRAGENT_TEST_RELAY_TOKEN;
  assert.ok(gateway && token, "Go loopback fixture is required");
  const common = { gateway, resolveAccessToken: async () => token,
    createSocket: (url: string) => new WebSocket(url) as unknown as RealtimeSocket };
  const discovery = new RelaySocketClient(common);
  const client = new RelaySocketClient({ ...common, deviceId: "desktop" });
  try {
    discovery.connect(); client.connect();
    await waitFor(() => discovery.getSnapshot().status === "ready" && client.getSnapshot().status === "ready", "relay admission deadline");
    assert.equal(client.getSnapshot().sessionStatus, "active");
    const originalExpiry = client.getSnapshot().sessionEndsAt;
    assert.ok(typeof originalExpiry === "number" && originalExpiry > Date.now() + 23 * 60 * 60 * 1000);
    const originalEpoch = client.getSnapshot().connectionEpoch;
    client.notifyNetworkChange();
    await waitFor(() => client.getSnapshot().status === "ready" && client.getSnapshot().connectionEpoch > originalEpoch, "companion resume deadline");
    assert.equal(client.getSnapshot().sessionEndsAt, originalExpiry, "reconnecting must preserve the original session expiry");
    const machines = await listMachines(discovery.rpc);
    assert.equal(machines.length, 1);
    assert.equal(machines[0]?.device_id, "desktop");
    assert.equal(machines[0]?.online, true);
    const tabs = await listTabs(client.rpc);
    assert.equal(tabs[0]?.title, "Desktop shell");
    await assert.rejects(createTerminal(client.rpc), /connect the desktop/i);
    await assert.rejects(client.rpc.request("unknown.method", {}), /unknown method/i);

    let catalogName: string | null = null;
    let workspaceId: string | null = null;
    let catalogError: Error | null = null;
    const catalog = subscribeMachineCatalog(client.rpc, {
      onSnapshot: (snapshot) => { catalogName = snapshot.catalog.projects[0]?.name ?? null; workspaceId = snapshot.catalog.tabs[0]?.workspace_id ?? null; },
      onUpdate: (snapshot) => { catalogName = snapshot.catalog.projects[0]?.name ?? null; },
      onError: (error) => { catalogError = error; },
    }, undefined);
    await waitFor(() => catalogName !== null || catalogError !== null, "catalog snapshot deadline");
    assert.equal(catalogError, null);
    assert.equal(catalogName, "Dragent");
    assert.equal(workspaceId, "fixture-workspace");

    const received: string[] = [];
    let eventSnapshotCount: number | null = null;
    const stream = subscribeRuntimeEvents(client.rpc, {
      onSnapshot: (events) => { eventSnapshotCount = events.length; },
      onEvent: (event) => received.push(event.kind),
    });
    await waitFor(() => eventSnapshotCount !== null, "event snapshot deadline");
    assert.equal(eventSnapshotCount, 0);
    const fixtureURL = (path: string) => new URL(path, gateway.replace(/^ws/u, "http"));
    assert.equal((await fetch(fixtureURL("/_test/event"), { method: "POST" })).status, 204);
    await waitFor(() => received.length > 0, "live event deadline");
    assert.deepEqual(received, ["control_granted"]);
    await stream.unsubscribe();
    assert.equal((await fetch(fixtureURL("/_test/catalog"), { method: "POST" })).status, 204);
    await waitFor(() => catalogName === "Updated Dragent" || catalogError !== null, "live catalog update deadline");
    assert.equal(catalogError, null);
    assert.equal(catalogName, "Updated Dragent");
    await catalog.unsubscribe();
    const coordination = await listCoordinations(client.rpc, { project_id: "fixture-project" });
    assert.equal(coordination.items[0]?.title, "Fixture coordination");
    assert.equal(coordination.truncated, false);
    const team = await getCoordination(client.rpc, "team-12345678");
    assert.equal(team.team.tasks[0]?.agent, "opencode");
    assert.equal(team.team.messages[0]?.text, "Synthetic fixture message");
    await assert.rejects(listCoordinations(client.rpc, { project_id: "foreign-project" }));
    const attachment = await attachCompanionTab(client.rpc, 3);
    const initialOutput = await readCompanionTab(client.rpc, attachment.handle, 0);
    assert.equal(atob(initialOutput.data), "fixture output\n");
    assert.deepEqual(await inputCompanionTab(client.rpc, attachment.handle, "mobile input\n"), { sent: true });
    const nextOutput = await readCompanionTab(client.rpc, attachment.handle, initialOutput.base + atob(initialOutput.data).length);
    assert.equal(atob(nextOutput.data), "mobile input\n");
    assert.deepEqual(await resizeCompanionTab(client.rpc, attachment.handle, 24, 80), { resized: true });
    const screen = await screenCompanionTab(client.rpc, attachment.handle);
    assert.equal(screen.complete, true);
    assert.equal(screen.delta?.kind, "alt_screen");
    assert.deepEqual(await detachCompanionTab(client.rpc, attachment.handle), { detached: true });
    await assert.rejects(inputCompanionTab(client.rpc, attachment.handle, "must not send"));
    await revokeCompanionSession(client.rpc);
    await waitFor(() => client.getSnapshot().sessionStatus === "ended", "companion revocation deadline");
    assert.equal(discovery.getSnapshot().status, "ready");
    assert.equal((await listMachines(discovery.rpc)).length, 1, "revocation must leave another session alive");
    console.log("Go ↔ mobile discovery, hierarchy, coordination, terminals, events, fixed-expiry resume and isolated revocation passed");
  } finally { discovery.close(); client.close(); }
}
void main().catch((error: unknown) => { console.error(error); process.exitCode = 1; });
