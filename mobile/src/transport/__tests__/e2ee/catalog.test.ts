// §12.3 catalog rollback after re-pairing (L6), §6.5 checks, and the §6.4
// phone-side unpair, against real sealing (dragent_e2ee) and the real pin store.
import assert from "node:assert/strict";
import { test } from "node:test";

import { RelayRpcClient, RelayRpcError } from "../../relayRpcClient";
import { listTabs, subscribeMachineCatalog } from "../../relayRpcMethods";
import { PhoneSecurity } from "../../e2ee/security";
import { openSealedCatalogPayload } from "../../e2ee/sealedCatalog";
import { Desk } from "../harness";
import { ACCOUNT, DESKTOP, flush, head, PHONE, world } from "./fixture";

const DESKTOP2 = "0a0b0c0d-0000-4000-8000-0000000000d2";
const ids = { account: ACCOUNT, phone: PHONE, desktop: DESKTOP };

const catalogJson = (n: number) => ({ machine: { name: `Studio ${n}`, platform: "macOS" }, projects: [], workspaces: [], tabs: [] });

/** The relay's stream payload: the blob as a JSON object plus a routing summary. */
function payload(blob: Uint8Array) {
  return {
    catalog: JSON.parse(new TextDecoder().decode(blob)) as unknown,
    machine: { device_id: DESKTOP, online: true, last_seen_at: 1, blob_version: 1, blob_size: blob.length },
  };
}

test("L6: pair, catalog v5, unpair, re-pair (pair-done 9): the relay's old v5 blob is rejected; v9 is shown", async () => {
  const w = world();
  try {
    // A second paired desktop keeps the phone's keys across the unpair.
    const desk2 = new Desk(w.hn, "d2", ACCOUNT, DESKTOP2);
    w.security.commitPairing({ desktop: DESKTOP2, identityKey: desk2.key, desktopName: "Laptop", catalogVersion: 1 });
    const phoneKey = w.native.identityPublicKey()!;
    const me = [{ id: PHONE, key: phoneKey }];
    const v5 = w.desk.sealCatalog(5, catalogJson(5), me);
    const r5 = openSealedCatalogPayload(w.security, ids, payload(v5));
    assert.equal(r5.kind, "catalog");
    assert.equal(w.security.catalogHighWater(DESKTOP), 5);

    await w.security.unpairDesktop(DESKTOP);
    assert.equal(w.security.pin(DESKTOP).state, "tombstoned");
    assert.equal(openSealedCatalogPayload(w.security, ids, payload(v5)).kind, "unpaired", "nothing opens for an unpaired desktop");
    assert.notEqual(w.native.identityPublicKey(), null, "keys stay while another desktop is paired");

    // Re-pair the same desktop: pair-done says it is about to seal version 9.
    w.security.commitPairing({ desktop: DESKTOP, identityKey: w.desk.key, desktopName: "Studio Mac", catalogVersion: 9 });
    const replay = openSealedCatalogPayload(w.security, ids, payload(v5));
    assert.equal(replay.kind, "rejected", "v5 is still wrapped to this phone's key, and still refused");
    assert.equal(replay.kind === "rejected" && replay.reason, "rollback");
    assert.equal(openSealedCatalogPayload(w.security, ids, payload(w.desk.sealCatalog(8, catalogJson(8), me))).kind, "rejected");
    const v9 = w.desk.sealCatalog(9, catalogJson(9), me);
    const r9 = openSealedCatalogPayload(w.security, ids, payload(v9));
    assert.ok(r9.kind === "catalog" && (r9.json as { machine: { name: string } }).machine.name === "Studio 9");

    // Same version, different body: equivocation, last good copy kept.
    const fork = openSealedCatalogPayload(w.security, ids, payload(w.desk.sealCatalog(9, catalogJson(99), me)));
    assert.ok(fork.kind === "rejected" && fork.reason === "equivocation");
    assert.equal((fork.lastGood as { machine: { name: string } }).machine.name, "Studio 9");

    // Sealed by another desktop's key but naming this desktop: wrong sender.
    const forged = desk2.sealCatalog(10, catalogJson(10), me);
    const relabelled = JSON.parse(new TextDecoder().decode(forged)) as Record<string, unknown>;
    relabelled.desktop = DESKTOP;
    assert.equal(openSealedCatalogPayload(w.security, ids, { catalog: relabelled, machine: {} }).kind, "rejected");

    // After an app restart the high-water mark persisted; the accepted v9 reopens and v5 stays refused.
    const restarted = new PhoneSecurity(w.native);
    assert.equal(restarted.catalogHighWater(DESKTOP), 9);
    assert.equal(openSealedCatalogPayload(restarted, ids, payload(v9)).kind, "catalog");
    assert.equal(openSealedCatalogPayload(restarted, ids, payload(v5)).kind, "rejected");
  } finally {
    w.close();
  }
});

test("§6.5: the catalog stream shows only verified blobs; a rejected update keeps the last good copy with the §9 warning", () => {
  const w = world();
  try {
    const me = [{ id: PHONE, key: w.native.identityPublicKey()! }];
    const sent: Record<string, unknown>[] = [];
    const rpc = new RelayRpcClient({ send: (f) => { sent.push(f); return true; }, createRequestId: () => "00000000-0000-4000-8000-0000000000c1" });
    const shown: string[] = [];
    const warnings: string[] = [];
    const errors: RelayRpcError[] = [];
    subscribeMachineCatalog(rpc, {
      onSnapshot: (s) => shown.push(s.catalog.machine?.name ?? ""),
      onUpdate: (s) => shown.push(s.catalog.machine?.name ?? ""),
      onRejected: (m) => warnings.push(m),
      onError: (e) => errors.push(e),
    }, (p) => openSealedCatalogPayload(w.security, ids, p));
    const frame = (seq: number, p: unknown) => rpc.handleMessage({ type: "rpc-stream", v: 1, stream: "catalog", seq, payload: p });
    frame(1, payload(w.desk.sealCatalog(3, catalogJson(3), me)));
    frame(2, payload(w.desk.sealCatalog(2, catalogJson(2), me))); // rollback
    frame(3, { catalog: { projects: [], workspaces: [], tabs: [] }, machine: { device_id: DESKTOP, online: true, last_seen_at: 1 } }); // plaintext injection
    frame(4, payload(w.desk.sealCatalog(4, catalogJson(4), me)));
    assert.deepEqual(shown, ["Studio 3", "Studio 3", "Studio 4"]);
    assert.deepEqual(warnings, ["This desktop's data from the server didn't check out. Showing the last good copy."]);
    assert.equal(errors.length, 0);
    // Without an opener nothing is shown at all (no plaintext path).
    const bare: string[] = [];
    const rpc2 = new RelayRpcClient({ send: () => true, createRequestId: () => "00000000-0000-4000-8000-0000000000c2" });
    const errs2: RelayRpcError[] = [];
    subscribeMachineCatalog(rpc2, { onSnapshot: (s) => bare.push(String(s)), onUpdate: () => {}, onError: (e) => errs2.push(e) }, undefined);
    rpc2.handleMessage({ type: "rpc-stream", v: 1, stream: "catalog", seq: 1, payload: payload(w.desk.sealCatalog(5, catalogJson(5), me)) });
    assert.deepEqual(bare, []);
    assert.equal(errs2.length, 1);
  } finally {
    w.close();
  }
});

test("§6.4 unpairDesktop: channels die at once, pending requests fail, no KK afterwards, push pin and keys go; the server call comes last", async () => {
  const w = world();
  try {
    await w.admit();
    w.pump();
    w.pump();
    assert.equal(w.client.getSnapshot().secure?.state, "open");
    assert.deepEqual(w.native.listPushPins().map((p) => p.desktop), [DESKTOP]);
    const pending = listTabs(w.client.rpc).catch((e: unknown) => e);
    await flush();
    const order: string[] = [];
    const result = await w.security.unpairDesktop(DESKTOP, {
      deleteOnServer: async () => { order.push(`server:${w.native.identityPublicKey() === null ? "keys-gone" : "keys"}`); },
      forgetDeviceToken: async () => { order.push("token"); },
    });
    assert.deepEqual(result, { wasPinned: true, keysDeleted: true }, "the last desktop takes every key with it");
    assert.ok(await pending instanceof RelayRpcError);
    assert.equal(w.client.getSnapshot().secure?.state, "unpaired");
    assert.deepEqual(w.native.listPushPins(), [], "its notifications now show only the generic text");
    assert.equal(w.native.identityPublicKey(), null);
    assert.deepEqual(order, ["token", "server:keys-gone"]);
    // Nothing more goes to that desktop, even after a reconnect.
    const sentBefore = w.socket().binary.length;
    await assert.rejects(listTabs(w.client.rpc));
    w.socket().drop(1006);
    w.clock.advance(1000);
    const s = await w.admit();
    assert.equal(w.socket().binary.length, 0);
    assert.equal(s.binary.filter((f) => head(f).kind === 1).length, 0, "no handshake to an unpaired desktop");
    void sentBefore;
  } finally {
    w.close();
  }
});

test("§6.1/§9: a pin store that fails its check is lost keys — fail closed, never 'empty and continue'", async () => {
  const w = world();
  try {
    w.hn.tamperPinStore();
    const fresh = new PhoneSecurity(w.native);
    assert.equal(fresh.isLost(), true);
    assert.equal(fresh.pin(DESKTOP).state, "lost");
    assert.equal(openSealedCatalogPayload(fresh, ids, { catalog: {}, machine: {} }).kind, "unreadable");
  } finally {
    w.close();
  }
});
